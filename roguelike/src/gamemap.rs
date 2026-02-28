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
    /// Creates a new game map filled with a simple pattern of floor and furniture tiles.
    pub fn new(width: CoordinateUnit, height: CoordinateUnit) -> Self {
        // Positions of trees placed around the spawn point
        let spawn_trees: &[(CoordinateUnit, CoordinateUnit)] = &[
            (SPAWN_X - 3, SPAWN_Y + 2),
            (SPAWN_X + 4, SPAWN_Y + 1),
            (SPAWN_X - 2, SPAWN_Y - 3),
            (SPAWN_X + 3, SPAWN_Y - 2),
            (SPAWN_X + 5, SPAWN_Y + 3),
            (SPAWN_X - 4, SPAWN_Y - 1),
            (SPAWN_X + 1, SPAWN_Y + 4),
            (SPAWN_X - 1, SPAWN_Y - 4),
        ];

        let mut voxels = Vec::with_capacity(height as usize);
        for y in 0..height {
            let mut row = Vec::with_capacity(width as usize);
            for x in 0..width {
                let floor = match ((x + y) % 4) as u8 {
                    0 => Floor::Grass,
                    1 => Floor::Dirt,
                    2 => Floor::Gravel,
                    _ => Floor::Sand,
                };

                let furniture = if x == 0 || y == 0 || x == width - 1 || y == height - 1 {
                    Some(Furniture::Wall)
                } else if spawn_trees.contains(&(x, y)) {
                    Some(Furniture::Tree)
                } else if (x % 7 == 0) && (y % 5 == 0) {
                    Some(Furniture::Tree)
                } else {
                    None
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
        self.create_render_packet_with_visibility(center, render_width, render_height, None)
    }

    /// Creates a RenderPacket with per-tile visibility support.
    /// When `visible_tiles` is `Some`, tiles outside the set are dimmed.
    /// When `None`, all tiles are shown at full brightness.
    pub fn create_render_packet_with_visibility(
        &self,
        center: &MyPoint,
        render_width: u16,
        render_height: u16,
        visible_tiles: Option<&std::collections::HashSet<MyPoint>>,
    ) -> RenderPacket {
        let w_radius = render_width as CoordinateUnit / 2;
        let h_radius = render_height as CoordinateUnit / 2;

        let bottom_left = (center.0 - w_radius, center.1 - h_radius);

        let mut grid = create_2d_array(render_width as usize, render_height as usize);

        for ry in 0..render_height as CoordinateUnit {
            for rx in 0..render_width as CoordinateUnit {
                let world_x = bottom_left.0 + rx;
                let world_y = bottom_left.1 + ry;

                if let Some(voxel) = self.get_voxel_at(&(world_x, world_y)) {
                    let visible = visible_tiles
                        .map(|vt| vt.contains(&(world_x, world_y)))
                        .unwrap_or(true);
                    grid[ry as usize][rx as usize] = voxel.to_graphic(visible);
                }
            }
        }

        grid
    }
}

impl Default for GameMap {
    fn default() -> Self {
        GameMap::new(80, 50)
    }
}
