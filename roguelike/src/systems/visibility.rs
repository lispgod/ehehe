use std::collections::HashSet;

use bevy::prelude::*;

use crate::components::{Player, Position, Viewshed};
use crate::grid_vec::GridVec;
use crate::resources::{CursorPosition, GameMapResource};
use crate::typedefs::{CoordinateUnit, MyPoint};

/// Recomputes the `visible_tiles` set for every entity whose `Viewshed` is
/// dirty (e.g., because the entity moved). Uses recursive symmetric
/// shadowcasting — the mathematically correct O(visible_tiles) algorithm
/// that eliminates the artifacts of Bresenham ray casting.
///
/// Reference: Albert Ford, "Symmetric Shadowcasting" (2017).
/// The algorithm guarantees:
///   1. **Symmetry** — if A can see B then B can see A.
///   2. **Completeness** — no visible tile is missed.
///   3. **Efficiency** — each visible tile is visited at most once per octant.
pub fn visibility_system(
    game_map: Res<GameMapResource>,
    mut query: Query<(Entity, &Position, &mut Viewshed)>,
    player_query: Query<Entity, With<Player>>,
    cursor: Res<CursorPosition>,
) {
    let player_entity = player_query.single().ok();

    for (entity, pos, mut viewshed) in &mut query {
        if !viewshed.dirty {
            continue;
        }

        viewshed.visible_tiles.clear();
        let origin = pos.as_grid_vec();
        let range = viewshed.range;

        // The origin is always visible.
        viewshed.visible_tiles.insert(origin);

        // Cast shadows in all 8 octants via the cardinal/diagonal transform.
        for octant in 0..8u8 {
            shadowcast_octant(
                &game_map,
                &mut viewshed.visible_tiles,
                origin,
                range,
                octant,
                1,
                Slope { y: 1, x: 1 }, // start slope = 1/1
                Slope { y: 0, x: 1 }, // end slope   = 0/1
            );
        }

        // Directional FOV: filter player's visible tiles to a cone toward the cursor.
        if player_entity == Some(entity) {
            let cursor_dir = cursor.0 - origin;
            if cursor_dir != GridVec::ZERO {
                let (cdx, cdy) = (cursor_dir.x as f64, cursor_dir.y as f64);
                let cursor_len = (cdx * cdx + cdy * cdy).sqrt();
                // cos_threshold of 0.0 gives a 180° cone (hemisphere)
                let cos_threshold = 0.0_f64;

                viewshed.visible_tiles.retain(|&tile| {
                    let diff = tile - origin;
                    if diff == GridVec::ZERO {
                        return true; // always see own tile
                    }
                    let (dx, dy) = (diff.x as f64, diff.y as f64);
                    let len = (dx * dx + dy * dy).sqrt();
                    let dot = (dx * cdx + dy * cdy) / (len * cursor_len);
                    dot >= cos_threshold
                });
            }
        }

        // Merge visible into revealed (fog of war memory).
        // Clone the visible set to avoid overlapping borrows.
        let newly_visible: Vec<MyPoint> = viewshed.visible_tiles.iter().copied().collect();
        viewshed.revealed_tiles.extend(newly_visible);
        viewshed.dirty = false;
    }
}

// ───────────────────────── Shadowcasting internals ─────────────────────────

/// Rational slope represented as y/x to avoid floating-point.
/// Invariant: `x > 0`.
#[derive(Clone, Copy)]
struct Slope {
    y: CoordinateUnit,
    x: CoordinateUnit,
}

impl Slope {
    /// self > other  ⟺  self.y / self.x > other.y / other.x
    #[allow(dead_code)]
    fn gt(self, other: Self) -> bool {
        self.y * other.x > other.y * self.x
    }

    /// self >= other
    fn ge(self, other: Self) -> bool {
        self.y * other.x >= other.y * self.x
    }
}

/// Returns `true` if the tile at `point` blocks line-of-sight.
fn is_opaque(game_map: &GameMapResource, point: MyPoint) -> bool {
    match game_map.0.get_voxel_at(&point) {
        Some(v) => v.furniture.is_some(),
        None => true, // off-map ⇒ opaque
    }
}

/// Transforms octant-local (row, col) into world coordinates.
///
/// The 8 octants are indexed 0..7, each covering a 45° sector.
/// Octant 0 is "north-northeast": row increases upward, col goes right.
fn transform(origin: MyPoint, octant: u8, row: CoordinateUnit, col: CoordinateUnit) -> MyPoint {
    match octant {
        0 => origin + GridVec::new(col, row),
        1 => origin + GridVec::new(row, col),
        2 => origin + GridVec::new(row, -col),
        3 => origin + GridVec::new(col, -row),
        4 => origin + GridVec::new(-col, -row),
        5 => origin + GridVec::new(-row, -col),
        6 => origin + GridVec::new(-row, col),
        7 => origin + GridVec::new(-col, row),
        _ => unreachable!(),
    }
}

/// Recursive symmetric shadowcasting for a single octant.
///
/// `row`        — current distance from origin (increases outward).
/// `start`/`end` — the visible angular window, as rational slopes.
///
/// The key insight: we scan each row from `start` to `end`, tracking
/// transitions between opaque and transparent tiles. Each contiguous
/// opaque run narrows the visible window for the next row; each
/// transparent run spawns a recursive sub-scan with the adjusted window.
fn shadowcast_octant(
    game_map: &GameMapResource,
    visible: &mut HashSet<MyPoint>,
    origin: MyPoint,
    range: CoordinateUnit,
    octant: u8,
    row: CoordinateUnit,
    mut start: Slope,
    end: Slope,
) {
    if row > range {
        return;
    }
    // Ensure the window is still valid.
    if !start.ge(end) {
        return;
    }

    let range_sq = range * range;
    let mut prev_opaque = false;
    let mut saved_start = start;

    // Column range for this row: from ceil(row * end) to floor(row * start).
    let min_col = round_down(row, end);
    let max_col = round_up(row, start);

    for col in (min_col..=max_col).rev() {
        let dist_sq = row * row + col * col;
        if dist_sq > range_sq {
            continue;
        }

        let world = transform(origin, octant, row, col);
        visible.insert(world);

        let cur_opaque = is_opaque(game_map, world);
        if cur_opaque {
            if !prev_opaque {
                // Transition transparent → opaque: save start slope for the
                // recursive sub-scan that will use the narrowed window.
                saved_start = start;
            }
            // Shrink the start slope past this opaque tile.
            start = Slope {
                y: 2 * col - 1,
                x: 2 * row,
            };
        } else if prev_opaque {
            // Transition opaque → transparent: recurse with the saved window
            // narrowed by the opaque run we just passed.
            let next_end = Slope {
                y: 2 * col + 1,
                x: 2 * row,
            };
            shadowcast_octant(
                game_map, visible, origin, range, octant, row + 1, saved_start, next_end,
            );
        }
        prev_opaque = cur_opaque;
    }

    // If the last tile in the row was transparent, continue scanning.
    if !prev_opaque {
        shadowcast_octant(
            game_map, visible, origin, range, octant, row + 1, start, end,
        );
    }
}

/// Computes the maximum column index for this row: ⌈row × slope⌉.
/// Uses ceiling division to ensure we don't miss the first column in the scan.
fn round_up(row: CoordinateUnit, slope: Slope) -> CoordinateUnit {
    (row * slope.y + slope.x - 1).div_euclid(slope.x)
}

/// Computes the minimum column index for this row using the half-tile
/// centre convention of the symmetric shadowcasting algorithm.
/// Returns ⌊(2 × row × slope + 1) / (2 × slope.x)⌋, clamped to ≥ 0.
fn round_down(row: CoordinateUnit, slope: Slope) -> CoordinateUnit {
    ((row * 2 * slope.y + slope.x) / (2 * slope.x)).max(0)
}
