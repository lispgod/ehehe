use crate::grid_vec::GridVec;
use crate::noise::{fbm, value_noise, NoiseSeed};
use crate::typeenums::{Floor, Furniture};
use crate::typedefs::{create_2d_array, CoordinateUnit, MyPoint, RenderPacket, SPAWN_POINT};
use crate::voxel::Voxel;

/// The game map: a simple 2D grid of voxels.
pub struct GameMap {
    pub width: CoordinateUnit,
    pub height: CoordinateUnit,
    pub voxels: Vec<Vec<Voxel>>,
}

/// A rectangular building footprint used during town generation.
struct Building {
    x: CoordinateUnit,
    y: CoordinateUnit,
    w: CoordinateUnit,
    h: CoordinateUnit,
    /// What kind of building: 0=house, 1=saloon, 2=stable, 3=general store,
    /// 4=sheriff's office, 5=post office.
    kind: u32,
}

impl GameMap {
    /// Creates a new game map as a cowboy western town.
    ///
    /// The generation pipeline:
    ///   1. **Desert base** — noise-driven arid terrain (sand, dirt, gravel).
    ///   2. **Main street** — a wide dirt road running horizontally.
    ///   3. **Buildings** — deterministically placed houses, saloons, stables
    ///      with walls and wood-plank interiors.
    ///   4. **Street furniture** — benches, lamp posts, barrels, crates,
    ///      hitching posts, water troughs, signs placed along streets.
    ///   5. **Decorative elements** — cacti, dead trees, rocks in open areas.
    ///   6. **Spawn clearing** — guaranteed open area around the player spawn.
    pub fn new(width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) -> Self {
        let mut voxels = Vec::with_capacity(height as usize);

        let biome_seed = seed;
        let detail_seed = seed.wrapping_add(12345);

        // ── Step 1: Base desert terrain ─────────────────────────────
        for y in 0..height {
            let mut row = Vec::with_capacity(width as usize);
            for x in 0..width {
                let fx = x as f64;
                let fy = y as f64;

                let biome = fbm(fx, fy, 4, 0.03, 0.5, biome_seed);
                let detail = fbm(fx, fy, 3, 0.1, 0.5, detail_seed);

                let floor = select_desert_floor(biome, detail);

                let furniture = if x == 0 || y == 0 || x == width - 1 || y == height - 1 {
                    Some(Furniture::Wall)
                } else {
                    None
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

        // ── Step 2: Main street (horizontal dirt road) ──────────────
        let street_y = height / 2;
        let street_half_width = 2;
        for y in (street_y - street_half_width)..=(street_y + street_half_width) {
            for x in 1..width - 1 {
                if let Some(voxel) = map.get_voxel_at_mut(&GridVec::new(x, y)) {
                    voxel.floor = Some(Floor::Dirt);
                    voxel.furniture = None;
                }
            }
        }

        // ── Step 3: Generate buildings ──────────────────────────────
        let buildings = generate_buildings(width, height, seed);

        for b in &buildings {
            place_building(&mut map, b, seed);
        }

        // ── Step 4: Street furniture along the main street ──────────
        place_street_furniture(&mut map, width, street_y, street_half_width, seed);

        // ── Step 5: Decorative elements in open areas ───────────────
        place_desert_decorations(&mut map, width, height, seed);

        // ── Step 6: Spawn clearing ──────────────────────────────────
        clear_around(&mut map, SPAWN_POINT, 6);

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

/// Selects a desert/arid floor tile from layered noise values.
fn select_desert_floor(biome: f64, detail: f64) -> Floor {
    if biome < 0.35 {
        if detail < 0.5 {
            Floor::Sand
        } else if detail < 0.8 {
            Floor::Gravel
        } else {
            Floor::Dirt
        }
    } else if biome < 0.60 {
        if detail < 0.4 {
            Floor::Dirt
        } else if detail < 0.7 {
            Floor::Sand
        } else {
            Floor::Gravel
        }
    } else if biome < 0.80 {
        if detail < 0.3 {
            Floor::Sand
        } else if detail < 0.5 {
            Floor::Dirt
        } else if detail < 0.75 {
            Floor::Gravel
        } else {
            Floor::Grass
        }
    } else {
        // Occasional sparse grass/dry vegetation at high biome values
        if detail < 0.3 {
            Floor::Dirt
        } else if detail < 0.6 {
            Floor::Sand
        } else if detail < 0.85 {
            Floor::Grass
        } else {
            Floor::TallGrass
        }
    }
}

/// Number of distinct building types used during town generation.
const BUILDING_TYPE_COUNT: u32 = 6;

/// Generates deterministic building footprints for the western town.
///
/// Buildings are placed in rows above and below the main street.
/// Uses noise for position jitter and building kind selection.
fn generate_buildings(
    width: CoordinateUnit,
    height: CoordinateUnit,
    seed: NoiseSeed,
) -> Vec<Building> {
    let mut buildings = Vec::new();
    let street_y = height / 2;
    let bldg_seed = seed.wrapping_add(11111);

    // Building rows: above and below the main street
    let rows: &[(CoordinateUnit, CoordinateUnit)] = &[
        (street_y + 5, street_y + 5 + 12),   // south row
        (street_y - 16, street_y - 5),        // north row
    ];

    for &(row_min_y, row_max_y) in rows {
        let mut cx = 6;
        let mut bldg_index = 0u32;
        while cx < width - 10 {
            let noise = value_noise(cx, bldg_index as i32, bldg_seed);
            let kind_noise = value_noise(bldg_index as i32, cx, bldg_seed.wrapping_add(2222));

            let bw = 6 + (noise * 6.0) as CoordinateUnit; // width 6–11
            let bh = 5 + (noise * 4.0) as CoordinateUnit; // height 5–8
            let by_jitter = (value_noise(cx, row_min_y, bldg_seed.wrapping_add(3333)) * 3.0) as CoordinateUnit;
            let by = row_min_y + by_jitter;

            // Don't exceed row bounds or map bounds
            if by + bh <= row_max_y && by > 0 && by + bh < height - 1 && cx + bw < width - 1 {
                let kind = (kind_noise * BUILDING_TYPE_COUNT as f64) as u32;
                buildings.push(Building {
                    x: cx,
                    y: by,
                    w: bw,
                    h: bh,
                    kind: kind.min(BUILDING_TYPE_COUNT - 1),
                });
            }

            cx += bw + 3 + (noise * 3.0) as CoordinateUnit; // gap between buildings
            bldg_index += 1;
        }
    }

    buildings
}

/// Places a building on the map: walls around the perimeter, wood plank floor,
/// and interior furniture based on building kind.
fn place_building(map: &mut GameMap, b: &Building, seed: NoiseSeed) {
    let furn_seed = seed.wrapping_add(55555);

    for y in b.y..b.y + b.h {
        for x in b.x..b.x + b.w {
            let pos = GridVec::new(x, y);
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                let is_border = x == b.x || x == b.x + b.w - 1 || y == b.y || y == b.y + b.h - 1;
                // Leave a doorway in the center of the bottom wall
                let is_door = y == b.y + b.h - 1
                    && x == b.x + b.w / 2;

                if is_border && !is_door {
                    voxel.furniture = Some(Furniture::Wall);
                    voxel.floor = Some(Floor::WoodPlanks);
                } else {
                    voxel.furniture = None;
                    voxel.floor = Some(Floor::WoodPlanks);
                }
            }
        }
    }

    // Place interior furniture based on building kind
    let interior_x = b.x + 1;
    let interior_y = b.y + 1;
    let iw = b.w - 2;
    let ih = b.h - 2;

    match b.kind {
        0 => {
            // House: table and chairs
            if iw >= 2 && ih >= 2 {
                set_furniture(map, interior_x + 1, interior_y + 1, Furniture::Table);
                set_furniture(map, interior_x, interior_y + 1, Furniture::Chair);
                if iw >= 3 {
                    set_furniture(map, interior_x + 2, interior_y + 1, Furniture::Chair);
                }
                if ih >= 3 {
                    set_furniture(map, interior_x + iw - 1, interior_y, Furniture::Barrel);
                }
            }
        }
        1 => {
            // Saloon: piano, tables, chairs, barrels
            if iw >= 4 && ih >= 3 {
                set_furniture(map, interior_x, interior_y, Furniture::Piano);
                set_furniture(map, interior_x + 2, interior_y + 1, Furniture::Table);
                set_furniture(map, interior_x + 1, interior_y + 1, Furniture::Chair);
                set_furniture(map, interior_x + 3, interior_y + 1, Furniture::Chair);
                if iw >= 5 {
                    set_furniture(map, interior_x + iw - 1, interior_y, Furniture::Barrel);
                    set_furniture(map, interior_x + iw - 1, interior_y + 1, Furniture::Barrel);
                }
            }
        }
        2 => {
            // Stable: hitching posts, water trough, some crates
            if iw >= 3 && ih >= 2 {
                set_furniture(map, interior_x, interior_y, Furniture::HitchingPost);
                set_furniture(map, interior_x + 2, interior_y, Furniture::HitchingPost);
                set_furniture(map, interior_x + 1, interior_y + ih - 1, Furniture::WaterTrough);
                if iw >= 4 {
                    set_furniture(map, interior_x + iw - 1, interior_y + ih - 1, Furniture::Crate);
                }
            }
        }
        3 => {
            // General store: barrels, crates, table
            if iw >= 3 && ih >= 2 {
                set_furniture(map, interior_x, interior_y, Furniture::Barrel);
                set_furniture(map, interior_x + 1, interior_y, Furniture::Crate);
                if iw >= 4 {
                    set_furniture(map, interior_x + iw - 1, interior_y, Furniture::Crate);
                }
                let noise = value_noise(b.x, b.y, furn_seed);
                if noise > 0.5 && ih >= 3 {
                    set_furniture(map, interior_x + 1, interior_y + ih - 1, Furniture::Table);
                }
            }
        }
        4 => {
            // Sheriff's office: table (desk), chair, barrel (lock-up), sign
            if iw >= 3 && ih >= 2 {
                set_furniture(map, interior_x + 1, interior_y, Furniture::Table);
                set_furniture(map, interior_x, interior_y, Furniture::Chair);
                if iw >= 4 {
                    set_furniture(map, interior_x + iw - 1, interior_y + ih - 1, Furniture::Barrel);
                }
                if ih >= 3 {
                    set_furniture(map, interior_x + iw - 1, interior_y, Furniture::Sign);
                }
            }
        }
        _ => {
            // Post office: table (counter), crates (parcels), sign
            if iw >= 3 && ih >= 2 {
                set_furniture(map, interior_x + 1, interior_y, Furniture::Table);
                set_furniture(map, interior_x, interior_y, Furniture::Crate);
                if iw >= 4 {
                    set_furniture(map, interior_x + iw - 1, interior_y, Furniture::Crate);
                }
                if ih >= 3 {
                    set_furniture(map, interior_x, interior_y + ih - 1, Furniture::Sign);
                }
            }
        }
    }
}

/// Helper: sets furniture at a position if within bounds and not occupied by a wall.
fn set_furniture(map: &mut GameMap, x: CoordinateUnit, y: CoordinateUnit, furn: Furniture) {
    let pos = GridVec::new(x, y);
    if let Some(voxel) = map.get_voxel_at_mut(&pos) {
        if !matches!(voxel.furniture, Some(Furniture::Wall)) {
            voxel.furniture = Some(furn);
        }
    }
}

/// Places street furniture (benches, lamp posts, barrels, signs, hitching posts)
/// along the main street sidewalks.
fn place_street_furniture(
    map: &mut GameMap,
    width: CoordinateUnit,
    street_y: CoordinateUnit,
    street_half_width: CoordinateUnit,
    seed: NoiseSeed,
) {
    let furn_seed = seed.wrapping_add(77777);
    let sidewalk_north = street_y - street_half_width - 1;
    let sidewalk_south = street_y + street_half_width + 1;

    for x in (4..width - 4).step_by(4) {
        let noise = value_noise(x, sidewalk_north, furn_seed);
        let furn = match (noise * 6.0) as u32 {
            0 => Furniture::LampPost,
            1 => Furniture::Bench,
            2 => Furniture::Barrel,
            3 => Furniture::HitchingPost,
            4 => Furniture::Sign,
            _ => Furniture::Crate,
        };
        set_furniture(map, x, sidewalk_north, furn);
    }

    for x in (6..width - 4).step_by(4) {
        let noise = value_noise(x, sidewalk_south, furn_seed.wrapping_add(1111));
        let furn = match (noise * 6.0) as u32 {
            0 => Furniture::Bench,
            1 => Furniture::LampPost,
            2 => Furniture::WaterTrough,
            3 => Furniture::Barrel,
            4 => Furniture::HitchingPost,
            _ => Furniture::Sign,
        };
        set_furniture(map, x, sidewalk_south, furn);
    }
}

/// Places desert decorations (cacti, dead trees, rocks, fences)
/// in open areas using noise-driven density.
fn place_desert_decorations(
    map: &mut GameMap,
    width: CoordinateUnit,
    height: CoordinateUnit,
    seed: NoiseSeed,
) {
    let deco_seed = seed.wrapping_add(99999);
    let density_seed = seed.wrapping_add(88888);

    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let pos = GridVec::new(x, y);
            // Skip if already has furniture or is near spawn
            if let Some(voxel) = map.get_voxel_at(&pos) {
                if voxel.furniture.is_some() {
                    continue;
                }
                if matches!(voxel.floor, Some(Floor::WoodPlanks)) {
                    continue;
                }
            } else {
                continue;
            }

            let dist_sq = pos.distance_squared(SPAWN_POINT) as f64;
            if dist_sq < 36.0 {
                continue;
            }

            let noise = value_noise(x, y, deco_seed);
            let density = fbm(x as f64, y as f64, 3, 0.06, 0.5, density_seed);

            // ~4% of tiles pass the noise check; density threshold creates
            // natural-looking clusters rather than uniform scatter.
            if noise > 0.04 || density < 0.45 {
                continue;
            }

            let pick = value_noise(y, x, deco_seed.wrapping_add(44444));
            let furn = if pick < 0.35 {
                Furniture::Cactus
            } else if pick < 0.55 {
                Furniture::DeadTree
            } else if pick < 0.75 {
                Furniture::Rock
            } else {
                Furniture::Bush
            };
            set_furniture(map, x, y, furn);
        }
    }
}

/// Clears all furniture within a given radius of a point.
fn clear_around(map: &mut GameMap, center: GridVec, radius: CoordinateUnit) {
    let radius_sq = radius * radius;
    let map_width = map.width;
    let map_height = map.height;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let pos = center + GridVec::new(dx, dy);
            if pos.distance_squared(center) <= radius_sq {
                if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                    // Don't clear border walls
                    let is_border = pos.x == 0 || pos.y == 0
                        || pos.x == map_width - 1 || pos.y == map_height - 1;
                    if !is_border {
                        voxel.furniture = None;
                    }
                }
            }
        }
    }
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

    #[test]
    fn game_map_has_buildings_with_wood_floors() {
        let map = GameMap::new(120, 80, 42);
        let mut wood_count = 0;
        for y in 0..80 {
            for x in 0..120 {
                if matches!(map.voxels[y][x].floor, Some(Floor::WoodPlanks)) {
                    wood_count += 1;
                }
            }
        }
        assert!(wood_count > 50, "Map should contain buildings with wood plank floors, found {wood_count}");
    }

    #[test]
    fn game_map_has_western_furniture() {
        let map = GameMap::new(120, 80, 42);
        let mut has_bench = false;
        let mut has_barrel = false;
        let mut has_cactus = false;
        for y in 0..80 {
            for x in 0..120 {
                match &map.voxels[y][x].furniture {
                    Some(Furniture::Bench) => has_bench = true,
                    Some(Furniture::Barrel) => has_barrel = true,
                    Some(Furniture::Cactus) => has_cactus = true,
                    _ => {}
                }
            }
        }
        assert!(has_bench, "Map should contain benches");
        assert!(has_barrel, "Map should contain barrels");
        assert!(has_cactus, "Map should contain cacti");
    }
}
