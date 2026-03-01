use crate::grid_vec::GridVec;
use crate::noise::{fbm, value_noise, NoiseSeed};
use crate::typeenums::{Floor, Furniture};
use crate::typedefs::{create_2d_array, CoordinateUnit, MyPoint, RenderPacket, SPAWN_POINT, GATE_POINT};
use crate::voxel::Voxel;

/// The game map: a simple 2D grid of voxels.
pub struct GameMap {
    pub width: CoordinateUnit,
    pub height: CoordinateUnit,
    pub voxels: Vec<Vec<Voxel>>,
}

impl GameMap {
    /// Creates a new game map using layered noise for natural terrain.
    ///
    /// The generation pipeline:
    ///   1. **Biome layer** — low-frequency fBm selects the dominant floor type
    ///      (grass, dirt, sand, gravel) in broad, natural regions.
    ///   2. **Detail layer** — higher-frequency noise adds local variation
    ///      (tall grass, flowers, moss) within biome regions.
    ///   3. **Tree density layer** — separate fBm controls forest density,
    ///      producing organic clusters and natural clearings.
    ///   4. **Spawn clearing** — a guaranteed open area around the spawn point
    ///      so the player always starts in a navigable space.
    ///   5. **Undergrowth** — bushes and rocks placed at medium noise density
    ///      to fill the space between trees naturally.
    pub fn new(width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) -> Self {
        let mut voxels = Vec::with_capacity(height as usize);

        // Different seed offsets for decorrelated noise layers.
        let biome_seed = seed;
        let detail_seed = seed.wrapping_add(12345);
        let tree_seed = seed.wrapping_add(67890);
        let undergrowth_seed = seed.wrapping_add(24680);

        for y in 0..height {
            let mut row = Vec::with_capacity(width as usize);
            for x in 0..width {
                let fx = x as f64;
                let fy = y as f64;

                // ── Floor selection ─────────────────────────────────
                // Low-frequency biome noise: broad terrain regions.
                let biome = fbm(fx, fy, 4, 0.03, 0.5, biome_seed);
                // Higher-frequency detail: local variation.
                let detail = fbm(fx, fy, 3, 0.1, 0.5, detail_seed);

                let floor = select_floor(biome, detail);

                // ── Furniture placement ─────────────────────────────
                let furniture = if x == 0 || y == 0 || x == width - 1 || y == height - 1 {
                    // Map border walls.
                    Some(Furniture::Wall)
                } else {
                    select_furniture(
                        fx,
                        fy,
                        biome,
                        tree_seed,
                        undergrowth_seed,
                        GridVec::new(x, y),
                    )
                };

                row.push(Voxel {
                    floor: Some(floor),
                    furniture,
                    voxel_pos: GridVec::new(x, y),
                });
            }
            voxels.push(row);
        }

        let mut map = GameMap {
            width,
            height,
            voxels,
        };

        // Place initial scorched earth around the Hell Gate position.
        for dy in -2..=2_i32 {
            for dx in -2..=2_i32 {
                let pos = GATE_POINT + GridVec::new(dx, dy);
                let dist_sq = (dx * dx + dy * dy) as f64;
                if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                    if dist_sq <= 1.0 {
                        voxel.floor = Some(Floor::Lava);
                    } else {
                        voxel.floor = Some(Floor::ScorchedEarth);
                    }
                }
            }
        }

        map
    }

    /// Get a reference to the voxel at the given map coordinate.
    pub fn get_voxel_at(&self, point: &MyPoint) -> Option<&Voxel> {
        let GridVec { x, y } = *point;
        if x >= 0 && x < self.width && y >= 0 && y < self.height {
            Some(&self.voxels[y as usize][x as usize])
        } else {
            None
        }
    }

    /// Get a mutable reference to the voxel at the given map coordinate.
    pub fn get_voxel_at_mut(&mut self, point: &MyPoint) -> Option<&mut Voxel> {
        let GridVec { x, y } = *point;
        if x >= 0 && x < self.width && y >= 0 && y < self.height {
            Some(&mut self.voxels[y as usize][x as usize])
        } else {
            None
        }
    }

    /// Returns `true` if the tile at `point` is passable (no furniture blocking).
    pub fn is_passable(&self, point: &MyPoint) -> bool {
        self.get_voxel_at(point)
            .is_some_and(|v| v.furniture.is_none())
    }

    /// Creates a RenderPacket (2D grid of GraphicTriples) for display,
    /// centered on the given position with the given render dimensions.
    pub fn create_render_packet(
        &self,
        center: &MyPoint,
        render_width: u16,
        render_height: u16,
    ) -> RenderPacket {
        self.create_render_packet_with_fog(center, render_width, render_height, None, None)
    }

    /// Creates a RenderPacket with full fog-of-war support.
    ///
    /// Tiles are rendered in three states:
    /// - **Visible** (in `visible_tiles`): full brightness.
    /// - **Revealed** (in `revealed_tiles` but not `visible_tiles`): heavily dimmed
    ///   to show the player has been there, but the area is not currently lit.
    /// - **Unseen** (in neither set): solid black.
    ///
    /// When both sets are `None`, all tiles render at full brightness (no FOV).
    pub fn create_render_packet_with_fog(
        &self,
        center: &MyPoint,
        render_width: u16,
        render_height: u16,
        visible_tiles: Option<&std::collections::HashSet<MyPoint>>,
        revealed_tiles: Option<&std::collections::HashSet<MyPoint>>,
    ) -> RenderPacket {
        let w_radius = render_width as CoordinateUnit / 2;
        let h_radius = render_height as CoordinateUnit / 2;

        let bottom_left = *center - GridVec::new(w_radius, h_radius);

        let mut grid = create_2d_array(render_width as usize, render_height as usize);

        for ry in 0..render_height as CoordinateUnit {
            for rx in 0..render_width as CoordinateUnit {
                let world_pos = bottom_left + GridVec::new(rx, ry);

                if let Some(voxel) = self.get_voxel_at(&world_pos) {
                    let is_visible = visible_tiles
                        .map(|vt| vt.contains(&world_pos))
                        .unwrap_or(true);
                    let is_revealed = revealed_tiles
                        .map(|rt| rt.contains(&world_pos))
                        .unwrap_or(true);

                    if is_visible {
                        grid[ry as usize][rx as usize] = voxel.to_graphic(true);
                    } else if is_revealed {
                        grid[ry as usize][rx as usize] = voxel.to_graphic(false);
                    }
                    // else: unseen → stays as the default black cell
                }
            }
        }

        grid
    }
}

/// Selects a floor tile from layered noise values.
///
/// The biome value (0–1) chooses the dominant terrain, and the detail
/// value adds local variation within each biome band.
fn select_floor(biome: f64, detail: f64) -> Floor {
    if biome < 0.30 {
        // Low biome → sandy/gravelly terrain
        if detail < 0.4 {
            Floor::Sand
        } else if detail < 0.7 {
            Floor::Gravel
        } else {
            Floor::Dirt
        }
    } else if biome < 0.50 {
        // Transition zone → dirt with some grass
        if detail < 0.3 {
            Floor::Dirt
        } else if detail < 0.6 {
            Floor::Grass
        } else {
            Floor::Gravel
        }
    } else if biome < 0.75 {
        // Forest biome → mostly grass with variation
        if detail < 0.15 {
            Floor::Flowers
        } else if detail < 0.45 {
            Floor::Grass
        } else if detail < 0.70 {
            Floor::TallGrass
        } else if detail < 0.85 {
            Floor::Moss
        } else {
            Floor::Dirt
        }
    } else {
        // Dense forest → lush undergrowth
        if detail < 0.2 {
            Floor::Moss
        } else if detail < 0.55 {
            Floor::TallGrass
        } else if detail < 0.8 {
            Floor::Grass
        } else {
            Floor::Flowers
        }
    }
}

/// Selects furniture (trees, bushes, rocks) based on noise-driven density.
///
/// The tree density is controlled by a separate fBm layer so forest
/// clusters form organically. A Euclidean-distance clearing around the
/// spawn point guarantees the player starts in open space.
fn select_furniture(
    fx: f64,
    fy: f64,
    biome: f64,
    tree_seed: NoiseSeed,
    undergrowth_seed: NoiseSeed,
    pos: GridVec,
) -> Option<Furniture> {
    // ── Spawn clearing ──────────────────────────────────────────
    // Tiles within Euclidean distance < 6 from spawn are kept clear.
    // We use squared distance to avoid a sqrt per tile.
    let dist_sq = pos.distance_squared(SPAWN_POINT) as f64;
    let clearing_radius_sq = 6.0 * 6.0;
    if dist_sq < clearing_radius_sq {
        return None;
    }

    // ── Gate clearing ───────────────────────────────────────────
    // Keep the area around the Hell Gate clear so the player can approach.
    let gate_dist_sq = pos.distance_squared(GATE_POINT) as f64;
    let gate_clearing_sq = 4.0 * 4.0;
    if gate_dist_sq < gate_clearing_sq {
        return None;
    }

    // Smooth transition zone (radius 6–10): reduced density.
    let transition_radius_sq = 10.0 * 10.0;
    let transition_factor = if dist_sq < transition_radius_sq {
        (dist_sq - clearing_radius_sq) / (transition_radius_sq - clearing_radius_sq)
    } else {
        1.0
    };

    // ── Tree density ────────────────────────────────────────────
    // fBm controls where forests cluster; biome modulates overall density.
    let tree_noise = fbm(fx, fy, 4, 0.05, 0.5, tree_seed);
    let base_density = biome * 0.5 + 0.1; // denser in high-biome areas
    let tree_threshold = 1.0 - (base_density * transition_factor);

    // Per-tile jitter prevents perfectly smooth cluster edges.
    let jitter = value_noise(pos.x, pos.y, tree_seed.wrapping_add(99999));

    if tree_noise > tree_threshold && jitter > 0.3 {
        // High density area → trees (with occasional dead trees)
        let variety = value_noise(pos.x, pos.y, tree_seed.wrapping_add(11111));
        if variety < 0.12 {
            return Some(Furniture::DeadTree);
        }
        return Some(Furniture::Tree);
    }

    // ── Undergrowth (bushes, rocks) ─────────────────────────────
    let under_noise = fbm(fx, fy, 3, 0.08, 0.5, undergrowth_seed);
    let under_jitter = value_noise(pos.x, pos.y, undergrowth_seed.wrapping_add(77777));

    if under_noise > 0.62 && under_jitter > 0.6 && transition_factor > 0.5 {
        let pick = value_noise(pos.x, pos.y, undergrowth_seed.wrapping_add(33333));
        if pick < 0.6 {
            return Some(Furniture::Bush);
        }
        return Some(Furniture::Rock);
    }

    None
}

impl Default for GameMap {
    fn default() -> Self {
        GameMap::new(80, 50, 42)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_map_dimensions() {
        let map = GameMap::new(40, 30, 0);
        assert_eq!(map.width, 40);
        assert_eq!(map.height, 30);
        assert_eq!(map.voxels.len(), 30);
        assert_eq!(map.voxels[0].len(), 40);
    }

    #[test]
    fn game_map_border_is_walls() {
        let map = GameMap::new(20, 15, 42);
        // Top and bottom rows
        for x in 0..20 {
            assert!(
                map.voxels[0][x as usize].furniture.is_some(),
                "Bottom border at x={x} should have wall"
            );
            assert!(
                map.voxels[14][x as usize].furniture.is_some(),
                "Top border at x={x} should have wall"
            );
        }
        // Left and right columns
        for y in 0..15 {
            assert!(
                map.voxels[y as usize][0].furniture.is_some(),
                "Left border at y={y} should have wall"
            );
            assert!(
                map.voxels[y as usize][19].furniture.is_some(),
                "Right border at y={y} should have wall"
            );
        }
    }

    #[test]
    fn game_map_border_not_passable() {
        let map = GameMap::new(20, 15, 42);
        // Borders should be impassable
        assert!(!map.is_passable(&GridVec::new(0, 0)));
        assert!(!map.is_passable(&GridVec::new(19, 0)));
        assert!(!map.is_passable(&GridVec::new(0, 14)));
        assert!(!map.is_passable(&GridVec::new(19, 14)));
    }

    #[test]
    fn game_map_out_of_bounds_not_passable() {
        let map = GameMap::new(10, 10, 0);
        assert!(!map.is_passable(&GridVec::new(-1, 5)));
        assert!(!map.is_passable(&GridVec::new(5, -1)));
        assert!(!map.is_passable(&GridVec::new(10, 5)));
        assert!(!map.is_passable(&GridVec::new(5, 10)));
    }

    #[test]
    fn game_map_get_voxel_at_valid() {
        let map = GameMap::new(10, 10, 0);
        assert!(map.get_voxel_at(&GridVec::new(5, 5)).is_some());
    }

    #[test]
    fn game_map_get_voxel_at_out_of_bounds() {
        let map = GameMap::new(10, 10, 0);
        assert!(map.get_voxel_at(&GridVec::new(-1, 0)).is_none());
        assert!(map.get_voxel_at(&GridVec::new(0, -1)).is_none());
        assert!(map.get_voxel_at(&GridVec::new(10, 0)).is_none());
        assert!(map.get_voxel_at(&GridVec::new(0, 10)).is_none());
    }

    #[test]
    fn game_map_spawn_area_is_clear() {
        let map = GameMap::new(120, 80, 42);
        // The spawn point area (within radius 6 of SPAWN_POINT) should be clear
        for dy in -5..=5 {
            for dx in -5..=5 {
                let pos = SPAWN_POINT + GridVec::new(dx, dy);
                let dist_sq = pos.distance_squared(SPAWN_POINT) as f64;
                if dist_sq < 36.0 {
                    assert!(
                        map.is_passable(&pos),
                        "Spawn area tile ({}, {}) should be passable",
                        pos.x,
                        pos.y
                    );
                }
            }
        }
    }

    #[test]
    fn game_map_deterministic_with_same_seed() {
        let map1 = GameMap::new(30, 20, 42);
        let map2 = GameMap::new(30, 20, 42);
        for y in 0..20 {
            for x in 0..30 {
                assert_eq!(
                    map1.voxels[y][x].floor, map2.voxels[y][x].floor,
                    "Floor mismatch at ({x}, {y})"
                );
                assert_eq!(
                    map1.voxels[y][x].furniture, map2.voxels[y][x].furniture,
                    "Furniture mismatch at ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn game_map_different_seeds_differ() {
        let map1 = GameMap::new(30, 20, 42);
        let map2 = GameMap::new(30, 20, 99);
        let mut any_different = false;
        for y in 1..19 {
            for x in 1..29 {
                if map1.voxels[y][x].furniture != map2.voxels[y][x].furniture {
                    any_different = true;
                    break;
                }
            }
            if any_different {
                break;
            }
        }
        assert!(any_different, "Different seeds should produce different maps");
    }

    #[test]
    fn game_map_all_voxels_have_floor() {
        let map = GameMap::new(20, 15, 42);
        for y in 0..15 {
            for x in 0..20 {
                assert!(
                    map.voxels[y][x].floor.is_some(),
                    "Voxel at ({x}, {y}) should have a floor"
                );
            }
        }
    }

    #[test]
    fn game_map_render_packet_dimensions() {
        let map = GameMap::new(80, 50, 42);
        let center = GridVec::new(40, 25);
        let packet = map.create_render_packet(&center, 20, 10);
        assert_eq!(packet.len(), 10);
        assert_eq!(packet[0].len(), 20);
    }
}
