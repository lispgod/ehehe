use crate::noise::{fbm, value_noise, NoiseSeed};
use crate::typeenums::{Floor, Furniture};
use crate::typedefs::{create_2d_array, CoordinateUnit, MyPoint, RenderPacket, SPAWN_X, SPAWN_Y};
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
                        x,
                        y,
                    )
                };

                row.push(Voxel {
                    floor: Some(floor),
                    furniture,
                    voxel_pos: (x, y),
                });
            }
            voxels.push(row);
        }

        GameMap {
            width,
            height,
            voxels,
        }
    }

    /// Get a reference to the voxel at the given map coordinate.
    pub fn get_voxel_at(&self, point: &MyPoint) -> Option<&Voxel> {
        let (x, y) = *point;
        if x >= 0 && x < self.width && y >= 0 && y < self.height {
            Some(&self.voxels[y as usize][x as usize])
        } else {
            None
        }
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

        let bottom_left = (center.0 - w_radius, center.1 - h_radius);

        let mut grid = create_2d_array(render_width as usize, render_height as usize);

        for ry in 0..render_height as CoordinateUnit {
            for rx in 0..render_width as CoordinateUnit {
                let world_x = bottom_left.0 + rx;
                let world_y = bottom_left.1 + ry;
                let world_pos = (world_x, world_y);

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
    x: CoordinateUnit,
    y: CoordinateUnit,
) -> Option<Furniture> {
    // ── Spawn clearing ──────────────────────────────────────────
    // Tiles within Euclidean distance < 6 from spawn are kept clear.
    // We compare squared distances to avoid a sqrt per tile.
    let dx = (x - SPAWN_X) as f64;
    let dy = (y - SPAWN_Y) as f64;
    let dist_sq = dx * dx + dy * dy;
    let clearing_radius_sq = 6.0 * 6.0;
    if dist_sq < clearing_radius_sq {
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
    let jitter = value_noise(x, y, tree_seed.wrapping_add(99999));

    if tree_noise > tree_threshold && jitter > 0.3 {
        // High density area → trees (with occasional dead trees)
        let variety = value_noise(x, y, tree_seed.wrapping_add(11111));
        if variety < 0.12 {
            return Some(Furniture::DeadTree);
        }
        return Some(Furniture::Tree);
    }

    // ── Undergrowth (bushes, rocks) ─────────────────────────────
    let under_noise = fbm(fx, fy, 3, 0.08, 0.5, undergrowth_seed);
    let under_jitter = value_noise(x, y, undergrowth_seed.wrapping_add(77777));

    if under_noise > 0.62 && under_jitter > 0.6 && transition_factor > 0.5 {
        let pick = value_noise(x, y, undergrowth_seed.wrapping_add(33333));
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
