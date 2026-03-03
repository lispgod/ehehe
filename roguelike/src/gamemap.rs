use std::collections::HashMap;

use crate::grid_vec::GridVec;
use crate::noise::{fbm, value_noise, NoiseSeed};
use crate::typeenums::{Floor, Props};
use crate::typedefs::{create_2d_array, CoordinateUnit, MyPoint, RenderPacket, SPAWN_POINT};
use crate::voxel::Voxel;

/// The game map: a simple 2D grid of voxels.
pub struct GameMap {
    pub width: CoordinateUnit,
    pub height: CoordinateUnit,
    pub voxels: Vec<Vec<Voxel>>,
    /// Tracks the world turn at which each fire tile was ignited.
    /// Used for deterministic burnout.
    pub fire_turns: HashMap<GridVec, u32>,
    /// Tracks the world turn at which each sand cloud tile was placed.
    /// Sand clouds dissipate after `SAND_CLOUD_LIFETIME` world turns.
    pub sand_cloud_turns: HashMap<GridVec, u32>,
    /// Stores the previous floor type before a sand cloud was placed.
    /// Used to restore the original floor when the cloud dissipates.
    pub sand_cloud_previous_floor: HashMap<GridVec, Option<Floor>>,
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
    /// Creates a new game map as a giant midwestern town.
    ///
    /// The generation pipeline:
    ///   1. **Desert base** — noise-driven arid terrain (sand, dirt, gravel).
    ///   2. **Street grid** — multiple horizontal avenues and vertical cross
    ///      streets forming a dense town grid.
    ///   3. **Buildings** — deterministically placed houses, saloons, stables
    ///      with walls and wood-plank interiors filling every city block.
    ///   4. **Landmarks** — a large Town Hall and oversized Grand Saloon.
    ///   5. **Parks** — small green parks with trees and benches scattered
    ///      throughout the town.
    ///   6. **Street props** — benches, lamp posts, barrels, crates,
    ///      hitching posts, water troughs, signs placed along streets.
    ///   7. **Decorative elements** — cacti, dead trees, rocks in open areas.
    ///   8. **Spawn clearing** — guaranteed open area around the player spawn.
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

                let prop = if x == 0 || y == 0 || x == width - 1 || y == height - 1 {
                    Some(Props::Wall)
                } else {
                    None
                };

                row.push(Voxel {
                    floor: Some(floor),
                    props: prop,
                });
            }
            voxels.push(row);
        }

        let mut map = GameMap {
            width,
            height,
            voxels,
            fire_turns: HashMap::new(),
            sand_cloud_turns: HashMap::new(),
            sand_cloud_previous_floor: HashMap::new(),
        };

        // ── Step 2: Street grid ─────────────────────────────────────
        // Multiple horizontal avenues spanning the map width.
        let avenue_spacing = 28;
        let avenue_half_width = 2;
        let mut avenue_ys: Vec<CoordinateUnit> = Vec::new();
        {
            let first_avenue = 20;
            let mut ay = first_avenue;
            while ay < height - 20 {
                avenue_ys.push(ay);
                for y in (ay - avenue_half_width)..=(ay + avenue_half_width) {
                    for x in 1..width - 1 {
                        if let Some(voxel) = map.get_voxel_at_mut(&GridVec::new(x, y)) {
                            voxel.floor = Some(Floor::Dirt);
                            voxel.props = None;
                        }
                    }
                }
                ay += avenue_spacing;
            }
        }

        // Vertical cross streets every ~30 tiles with noise-based jitter.
        let cross_seed = seed.wrapping_add(66666);
        let cross_spacing = 26;
        let cross_half_width = 1;
        let mut cross_xs: Vec<CoordinateUnit> = Vec::new();
        {
            let mut cx = 20i32;
            let mut ci = 0i32;
            while cx < width - 20 {
                let jitter = (value_noise(ci, 0, cross_seed) * 6.0) as CoordinateUnit - 3;
                let actual_cx = (cx + jitter).clamp(2, width - 3);
                cross_xs.push(actual_cx);
                for x in (actual_cx - cross_half_width)..=(actual_cx + cross_half_width) {
                    for y in 1..height - 1 {
                        if let Some(voxel) = map.get_voxel_at_mut(&GridVec::new(x, y))
                            && !matches!(voxel.props, Some(Props::Wall))
                        {
                            voxel.floor = Some(Floor::Dirt);
                            voxel.props = None;
                        }
                    }
                }
                cx += cross_spacing;
                ci += 1;
            }
        }

        // ── Step 3: Generate buildings filling every city block ──────
        let buildings = generate_buildings(width, height, seed, &avenue_ys, avenue_half_width);

        for b in &buildings {
            place_building(&mut map, b, seed);
        }

        // ── Step 4: Landmark buildings ──────────────────────────────
        place_town_hall(&mut map, width, height, seed);
        place_grand_saloon(&mut map, width, height, seed);

        // ── Step 5: Small parks ─────────────────────────────────────
        place_parks(&mut map, width, height, seed);

        // ── Step 6: Street props along every avenue ──────────────
        for &ay in &avenue_ys {
            place_street_props(&mut map, width, ay, avenue_half_width, seed);
        }

        // ── Step 7: Decorative elements in open areas ───────────────
        place_desert_decorations(&mut map, width, height, seed);

        // ── Step 8: Spawn clearing ──────────────────────────────────
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

    /// Returns `true` if the tile at `point` is passable (no blocking props).
    pub fn is_passable(&self, point: &MyPoint) -> bool {
        self.get_voxel_at(point)
            .is_some_and(|v| match &v.props {
                Some(f) => !f.blocks_movement(),
                None => true,
            })
    }

    /// Finds a passable interior tile inside a saloon (building with a Piano).
    /// Scans the map for Piano prop, then returns a nearby empty wood-plank tile.
    /// Returns `None` if no saloon is found.
    /// Search is deterministic (left-to-right, top-to-bottom) for reproducible spawns.
    pub fn find_saloon_interior(&self) -> Option<GridVec> {
        for y in 1..self.height - 1 {
            for x in 1..self.width - 1 {
                let pos = GridVec::new(x, y);
                if let Some(voxel) = self.get_voxel_at(&pos)
                    && matches!(voxel.props, Some(Props::Piano))
                {
                    // Found a piano — look for an adjacent empty wood-plank tile
                    for dy in -2..=2i32 {
                        for dx in -2..=2i32 {
                            let candidate = pos + GridVec::new(dx, dy);
                            if let Some(v) = self.get_voxel_at(&candidate)
                                && v.props.is_none()
                                && matches!(v.floor, Some(Floor::WoodPlanks))
                            {
                                return Some(candidate);
                            }
                        }
                    }
                }
            }
        }
        None
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
/// Types 0-11: House, Saloon, Stable, General Store, Sheriff's Office,
/// Post Office, Church, Bank, Hotel, Jail, Undertaker, Blacksmith.
const BUILDING_TYPE_COUNT: u32 = 12;

/// Generates deterministic building footprints for the western town.
///
/// Buildings are placed in rows between every pair of adjacent avenues,
/// filling the entire map with dense city blocks.
/// Uses noise for position jitter and building kind selection.
fn generate_buildings(
    width: CoordinateUnit,
    height: CoordinateUnit,
    seed: NoiseSeed,
    avenue_ys: &[CoordinateUnit],
    avenue_half_width: CoordinateUnit,
) -> Vec<Building> {
    let mut buildings = Vec::new();
    let bldg_seed = seed.wrapping_add(11111);

    // Build rows between every pair of adjacent avenues.
    // Also add rows above the first avenue and below the last.
    let mut row_bands: Vec<(CoordinateUnit, CoordinateUnit)> = Vec::new();

    // Band above the first avenue
    if let Some(&first) = avenue_ys.first() {
        let top = 4;
        let bot = first - avenue_half_width - 2;
        if bot - top >= 6 {
            row_bands.push((top, bot));
        }
    }

    // Bands between each pair of avenues
    for pair in avenue_ys.windows(2) {
        let top = pair[0] + avenue_half_width + 2;
        let bot = pair[1] - avenue_half_width - 2;
        if bot - top >= 6 {
            row_bands.push((top, bot));
        }
    }

    // Band below the last avenue
    if let Some(&last) = avenue_ys.last() {
        let top = last + avenue_half_width + 2;
        let bot = height - 4;
        if bot - top >= 6 {
            row_bands.push((top, bot));
        }
    }

    for (row_idx, &(row_min_y, row_max_y)) in row_bands.iter().enumerate() {
        // Vary the starting offset per row for less grid-like placement
        let row_offset_noise = value_noise(row_idx as i32, row_min_y, bldg_seed.wrapping_add(6666));
        let mut cx = 4 + (row_offset_noise * 6.0) as CoordinateUnit;
        let mut bldg_index = 0u32;
        let band_height = row_max_y - row_min_y;
        while cx < width - 6 {
            let noise = value_noise(cx, bldg_index as i32 + row_idx as i32, bldg_seed);
            let kind_noise = value_noise(bldg_index as i32, cx + row_idx as i32, bldg_seed.wrapping_add(2222));

            let bw = 6 + (noise * 6.0) as CoordinateUnit; // width 6–11
            let max_h = band_height.min(10);
            let bh = 5 + (noise * (max_h - 5).max(1) as f64) as CoordinateUnit; // height 5–max_h
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

            // Tighter gaps between buildings for a denser town
            let gap_noise = value_noise(cx + 1, bldg_index as i32, bldg_seed.wrapping_add(4444));
            cx += bw + 2 + (noise * 2.0 + gap_noise * 3.0) as CoordinateUnit;
            bldg_index += 1;
        }
    }

    buildings
}

/// Places a building on the map: walls around the perimeter, wood plank floor,
/// and interior props based on building kind.
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
                    voxel.props = Some(Props::Wall);
                    voxel.floor = Some(Floor::WoodPlanks);
                } else {
                    voxel.props = None;
                    voxel.floor = Some(Floor::WoodPlanks);
                }
            }
        }
    }

    // Place interior props based on building kind
    let interior_x = b.x + 1;
    let interior_y = b.y + 1;
    let iw = b.w - 2;
    let ih = b.h - 2;

    match b.kind {
        0 => {
            // House: table and chairs
            if iw >= 2 && ih >= 2 {
                set_prop(map, interior_x + 1, interior_y + 1, Props::Table);
                set_prop(map, interior_x, interior_y + 1, Props::Chair);
                if iw >= 3 {
                    set_prop(map, interior_x + 2, interior_y + 1, Props::Chair);
                }
                if ih >= 3 {
                    set_prop(map, interior_x + iw - 1, interior_y, Props::Barrel);
                }
            }
        }
        1 => {
            // Saloon: piano, tables, chairs, barrels
            if iw >= 4 && ih >= 3 {
                set_prop(map, interior_x, interior_y, Props::Piano);
                set_prop(map, interior_x + 2, interior_y + 1, Props::Table);
                set_prop(map, interior_x + 1, interior_y + 1, Props::Chair);
                set_prop(map, interior_x + 3, interior_y + 1, Props::Chair);
                if iw >= 5 {
                    set_prop(map, interior_x + iw - 1, interior_y, Props::Barrel);
                    set_prop(map, interior_x + iw - 1, interior_y + 1, Props::Barrel);
                }
            }
        }
        2 => {
            // Stable: hitching posts, water trough, hay bales, some crates
            if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x, interior_y, Props::HitchingPost);
                set_prop(map, interior_x + 2, interior_y, Props::HitchingPost);
                set_prop(map, interior_x + 1, interior_y + ih - 1, Props::WaterTrough);
                if iw >= 4 {
                    set_prop(map, interior_x + iw - 1, interior_y + ih - 1, Props::Crate);
                    set_prop(map, interior_x + iw - 1, interior_y, Props::HayBale);
                }
                if iw >= 5 {
                    set_prop(map, interior_x + iw - 2, interior_y, Props::HayBale);
                }
            }
        }
        3 => {
            // General store: barrels, crates, table
            if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x, interior_y, Props::Barrel);
                set_prop(map, interior_x + 1, interior_y, Props::Crate);
                if iw >= 4 {
                    set_prop(map, interior_x + iw - 1, interior_y, Props::Crate);
                }
                let noise = value_noise(b.x, b.y, furn_seed);
                if noise > 0.5 && ih >= 3 {
                    set_prop(map, interior_x + 1, interior_y + ih - 1, Props::Table);
                }
            }
        }
        4 => {
            // Sheriff's office: table (desk), chair, barrel (lock-up), sign
            if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x + 1, interior_y, Props::Table);
                set_prop(map, interior_x, interior_y, Props::Chair);
                if iw >= 4 {
                    set_prop(map, interior_x + iw - 1, interior_y + ih - 1, Props::Barrel);
                }
                if ih >= 3 {
                    set_prop(map, interior_x + iw - 1, interior_y, Props::Sign);
                }
            }
        }
        5 => {
            // Post office: table (counter), crates (parcels), sign
            if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x + 1, interior_y, Props::Table);
                set_prop(map, interior_x, interior_y, Props::Crate);
                if iw >= 4 {
                    set_prop(map, interior_x + iw - 1, interior_y, Props::Crate);
                }
                if ih >= 3 {
                    set_prop(map, interior_x, interior_y + ih - 1, Props::Sign);
                }
            }
        }
        6 => {
            // Church: benches (pews) in rows, table (altar)
            if iw >= 3 && ih >= 3 {
                set_prop(map, interior_x + iw / 2, interior_y, Props::Table);
                for row in 1..ih.min(4) {
                    set_prop(map, interior_x, interior_y + row, Props::Bench);
                    if iw >= 4 {
                        set_prop(map, interior_x + iw - 1, interior_y + row, Props::Bench);
                    }
                }
            }
        }
        7 => {
            // Bank: table (counter), barrels (vault), crates (strongboxes)
            if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x + 1, interior_y, Props::Table);
                set_prop(map, interior_x, interior_y + ih - 1, Props::Barrel);
                if iw >= 4 {
                    set_prop(map, interior_x + iw - 1, interior_y + ih - 1, Props::Barrel);
                    set_prop(map, interior_x + iw - 1, interior_y, Props::Crate);
                }
            }
        }
        8 => {
            // Hotel: table, chairs in rows (rooms suggested by props layout)
            if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x, interior_y, Props::Table);
                set_prop(map, interior_x + 1, interior_y, Props::Chair);
                if ih >= 3 {
                    set_prop(map, interior_x, interior_y + 2, Props::Bench);
                    if iw >= 4 {
                        set_prop(map, interior_x + iw - 1, interior_y + 2, Props::Bench);
                    }
                }
                if iw >= 5 {
                    set_prop(map, interior_x + iw - 1, interior_y, Props::Barrel);
                }
            }
        }
        9 => {
            // Jail: barrels (cells), sign (wanted poster)
            if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x, interior_y, Props::Sign);
                set_prop(map, interior_x + iw - 1, interior_y, Props::Barrel);
                if ih >= 3 {
                    set_prop(map, interior_x, interior_y + ih - 1, Props::Barrel);
                    set_prop(map, interior_x + iw - 1, interior_y + ih - 1, Props::Barrel);
                }
            }
        }
        10 => {
            // Undertaker: tables (slabs), crates (coffins)
            if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x + 1, interior_y, Props::Table);
                if iw >= 4 {
                    set_prop(map, interior_x + 3.min(iw - 1), interior_y, Props::Table);
                }
                set_prop(map, interior_x, interior_y + ih - 1, Props::Crate);
                if iw >= 4 {
                    set_prop(map, interior_x + iw - 1, interior_y + ih - 1, Props::Crate);
                }
            }
        }
        _ => {
            // Blacksmith: barrels (water quench), crates (supplies), hitching post (anvil stand-in)
            if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x, interior_y, Props::HitchingPost);
                set_prop(map, interior_x + 1, interior_y + ih - 1, Props::Barrel);
                if iw >= 4 {
                    set_prop(map, interior_x + iw - 1, interior_y, Props::Crate);
                    set_prop(map, interior_x + iw - 1, interior_y + ih - 1, Props::WaterTrough);
                }
            }
        }
    }
}

/// Helper: sets a prop at a position if within bounds, not occupied by a wall,
/// and not on a dirt road tile.
fn set_prop(map: &mut GameMap, x: CoordinateUnit, y: CoordinateUnit, prop: Props) {
    let pos = GridVec::new(x, y);
    if let Some(voxel) = map.get_voxel_at_mut(&pos)
        && !matches!(voxel.props, Some(Props::Wall))
        && !matches!(voxel.floor, Some(Floor::Dirt)) {
            voxel.props = Some(prop);
        }
}

/// Places a large Town Hall building near the center of the map.
/// The Town Hall is 18×12 with a grand interior containing tables, chairs,
/// benches and signs (maps/notices).
/// Skipped on maps too small to fit the building.
fn place_town_hall(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) {
    let tw: CoordinateUnit = 18;
    let th: CoordinateUnit = 12;
    if width < tw + 6 || height < th + 6 {
        return; // map too small for a town hall
    }
    let th_seed = seed.wrapping_add(111222);
    let cx = width / 2 + (value_noise(0, 0, th_seed) * 20.0) as CoordinateUnit - 10;
    let cy = height / 3;
    let tx = (cx - tw / 2).clamp(2, width - tw - 2);
    let ty = (cy - th / 2).clamp(2, height - th - 2);

    // Lay down walls and wood-plank floor
    for y in ty..ty + th {
        for x in tx..tx + tw {
            let pos = GridVec::new(x, y);
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                let is_border = x == tx || x == tx + tw - 1 || y == ty || y == ty + th - 1;
                let is_door = y == ty + th - 1 && x == tx + tw / 2;
                let is_back_door = y == ty && x == tx + tw / 2;
                if is_border && !is_door && !is_back_door {
                    voxel.props = Some(Props::Wall);
                } else {
                    voxel.props = None;
                }
                voxel.floor = Some(Floor::WoodPlanks);
            }
        }
    }

    // Interior props: long meeting table, chairs, signs
    let ix = tx + 2;
    let iy = ty + 2;
    // Central table row
    for dx in 2..tw - 4 {
        set_prop(map, tx + dx, iy + 2, Props::Table);
    }
    // Chairs along table
    for dx in (2..tw - 4).step_by(2) {
        set_prop(map, tx + dx, iy + 1, Props::Chair);
        set_prop(map, tx + dx, iy + 3, Props::Chair);
    }
    // Signs on the walls
    set_prop(map, ix, iy, Props::Sign);
    set_prop(map, ix + 1, iy, Props::Sign);
    // Barrels in corners
    set_prop(map, tx + 1, ty + 1, Props::Barrel);
    set_prop(map, tx + tw - 2, ty + 1, Props::Barrel);
    set_prop(map, tx + 1, ty + th - 2, Props::Crate);
    set_prop(map, tx + tw - 2, ty + th - 2, Props::Crate);
}

/// Places a Grand Saloon — a large 20×14 saloon with piano, many tables,
/// chairs, and barrels. Placed in the southern half of the map.
/// Skipped on maps too small to fit the building.
fn place_grand_saloon(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) {
    let sw: CoordinateUnit = 20;
    let sh: CoordinateUnit = 14;
    if width < sw + 6 || height < sh + 6 {
        return; // map too small for a grand saloon
    }
    let gs_seed = seed.wrapping_add(333444);
    let cx = width / 2 - 20 + (value_noise(1, 1, gs_seed) * 40.0) as CoordinateUnit;
    let cy = height * 2 / 3;
    let sx = (cx - sw / 2).clamp(2, width - sw - 2);
    let sy = (cy - sh / 2).clamp(2, height - sh - 2);

    // Lay down walls and wood-plank floor
    for y in sy..sy + sh {
        for x in sx..sx + sw {
            let pos = GridVec::new(x, y);
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                let is_border = x == sx || x == sx + sw - 1 || y == sy || y == sy + sh - 1;
                // Two wide doorways
                let is_main_door = y == sy + sh - 1 && (x == sx + sw / 2 || x == sx + sw / 2 - 1);
                let is_side_door = x == sx && y == sy + sh / 2;
                if is_border && !is_main_door && !is_side_door {
                    voxel.props = Some(Props::Wall);
                } else {
                    voxel.props = None;
                }
                voxel.floor = Some(Floor::WoodPlanks);
            }
        }
    }

    // Interior: piano in corner, multiple table+chair clusters, barrel bar
    set_prop(map, sx + 1, sy + 1, Props::Piano);
    // Bar (barrels along the back wall)
    for dx in 3..sw - 3 {
        set_prop(map, sx + dx, sy + 1, Props::Barrel);
    }
    // Tables and chairs in a grid pattern
    for row in 0..3 {
        for col in 0..3 {
            let tx = sx + 3 + col * 5;
            let ty = sy + 4 + row * 3;
            if tx + 2 < sx + sw - 1 && ty + 1 < sy + sh - 1 {
                set_prop(map, tx, ty, Props::Table);
                set_prop(map, tx - 1, ty, Props::Chair);
                set_prop(map, tx + 1, ty, Props::Chair);
            }
        }
    }
    // Corner barrels
    set_prop(map, sx + sw - 2, sy + 1, Props::Barrel);
    set_prop(map, sx + sw - 2, sy + sh - 2, Props::Crate);
}

/// Places 3-5 small parks throughout the town.
/// Each park is a grassy area with trees, benches, and open space.
fn place_parks(
    map: &mut GameMap,
    width: CoordinateUnit,
    height: CoordinateUnit,
    seed: NoiseSeed,
) {
    let park_seed = seed.wrapping_add(555666);
    let park_count = 3 + (value_noise(0, 0, park_seed) * 3.0) as i32; // 3-5 parks
    let mut placed = 0;
    let mut pi = 0i32;
    while placed < park_count && pi < 20 {
        let px_noise = value_noise(pi, 0, park_seed);
        let py_noise = value_noise(0, pi, park_seed.wrapping_add(1));
        let park_cx = 30 + (px_noise * (width - 60) as f64) as CoordinateUnit;
        let park_cy = 30 + (py_noise * (height - 60) as f64) as CoordinateUnit;
        let park_w = 8 + (value_noise(pi, 1, park_seed) * 5.0) as CoordinateUnit; // 8-12
        let park_h = 6 + (value_noise(1, pi, park_seed) * 5.0) as CoordinateUnit; // 6-10

        pi += 1;

        // Don't place parks too close to spawn
        let dist_sq = GridVec::new(park_cx, park_cy).distance_squared(SPAWN_POINT);
        if dist_sq < 100 {
            continue;
        }

        // Lay down grass floor and clear props
        for y in park_cy..park_cy + park_h {
            for x in park_cx..park_cx + park_w {
                if let Some(voxel) = map.get_voxel_at_mut(&GridVec::new(x, y)) {
                    // Don't overwrite border walls
                    if !matches!(voxel.props, Some(Props::Wall)) {
                        voxel.floor = Some(Floor::Grass);
                        voxel.props = None;
                    }
                }
            }
        }

        // Place trees around the edges
        for dx in (0..park_w).step_by(3) {
            set_prop(map, park_cx + dx, park_cy, Props::Tree);
            set_prop(map, park_cx + dx, park_cy + park_h - 1, Props::Tree);
        }
        for dy in (0..park_h).step_by(3) {
            set_prop(map, park_cx, park_cy + dy, Props::Tree);
            set_prop(map, park_cx + park_w - 1, park_cy + dy, Props::Tree);
        }

        // Benches in the interior
        if park_w >= 6 && park_h >= 4 {
            set_prop(map, park_cx + 2, park_cy + park_h / 2, Props::Bench);
            set_prop(map, park_cx + park_w - 3, park_cy + park_h / 2, Props::Bench);
        }

        // Water trough (fountain stand-in)
        if park_w >= 8 && park_h >= 6 {
            set_prop(map, park_cx + park_w / 2, park_cy + park_h / 2, Props::WaterTrough);
        }

        placed += 1;
    }
}

/// Places street props (benches, lamp posts, barrels, signs, hitching posts)
/// along the main street sidewalks.
fn place_street_props(
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
        let prop = match (noise * 6.0) as u32 {
            0 => Props::HitchingPost,
            1 => Props::Bench,
            2 => Props::Barrel,
            3 => Props::WaterTrough,
            4 => Props::Sign,
            _ => Props::Crate,
        };
        set_prop(map, x, sidewalk_north, prop);
    }

    for x in (6..width - 4).step_by(4) {
        let noise = value_noise(x, sidewalk_south, furn_seed.wrapping_add(1111));
        let prop = match (noise * 6.0) as u32 {
            0 => Props::Bench,
            1 => Props::Crate,
            2 => Props::WaterTrough,
            3 => Props::Barrel,
            4 => Props::HitchingPost,
            _ => Props::Sign,
        };
        set_prop(map, x, sidewalk_south, prop);
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
            // Skip if already has props or is near spawn
            if let Some(voxel) = map.get_voxel_at(&pos) {
                if voxel.props.is_some() {
                    continue;
                }
                if matches!(voxel.floor, Some(Floor::WoodPlanks)) {
                    continue;
                }
                // Skip road tiles — no decorations on dirt roads.
                if matches!(voxel.floor, Some(Floor::Dirt)) {
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
            let prop = if pick < 0.30 {
                Props::Cactus
            } else if pick < 0.50 {
                Props::DeadTree
            } else if pick < 0.65 {
                Props::Rock
            } else if pick < 0.80 {
                Props::Bush
            } else {
                Props::HayBale
            };
            set_prop(map, x, y, prop);
        }
    }
}

/// Clears all props within a given radius of a point.
fn clear_around(map: &mut GameMap, center: GridVec, radius: CoordinateUnit) {
    let radius_sq = radius * radius;
    let map_width = map.width;
    let map_height = map.height;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let pos = center + GridVec::new(dx, dy);
            if pos.distance_squared(center) <= radius_sq
                && let Some(voxel) = map.get_voxel_at_mut(&pos) {
                    // Don't clear border walls
                    let is_border = pos.x == 0 || pos.y == 0
                        || pos.x == map_width - 1 || pos.y == map_height - 1;
                    if !is_border {
                        voxel.props = None;
                    }
                }
        }
    }
}

impl Default for GameMap {
    fn default() -> Self {
        GameMap::new(400, 280, 42)
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
                map.voxels[0][x as usize].props.is_some(),
                "Bottom border at x={x} should have wall"
            );
            assert!(
                map.voxels[14][x as usize].props.is_some(),
                "Top border at x={x} should have wall"
            );
        }
        // Left and right columns
        for y in 0..15 {
            assert!(
                map.voxels[y as usize][0].props.is_some(),
                "Left border at y={y} should have wall"
            );
            assert!(
                map.voxels[y as usize][19].props.is_some(),
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
        let map = GameMap::new(400, 280, 42);
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
                    map1.voxels[y][x].props, map2.voxels[y][x].props,
                    "Props mismatch at ({x}, {y})"
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
                if map1.voxels[y][x].props != map2.voxels[y][x].props {
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
        let packet = map.create_render_packet_with_fog(&center, 20, 10, None, None);
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
    fn game_map_has_western_props() {
        let map = GameMap::new(120, 80, 42);
        let mut has_bench = false;
        let mut has_barrel = false;
        let mut has_cactus = false;
        for y in 0..80 {
            for x in 0..120 {
                match &map.voxels[y][x].props {
                    Some(Props::Bench) => has_bench = true,
                    Some(Props::Barrel) => has_barrel = true,
                    Some(Props::Cactus) => has_cactus = true,
                    _ => {}
                }
            }
        }
        assert!(has_bench, "Map should contain benches");
        assert!(has_barrel, "Map should contain barrels");
        assert!(has_cactus, "Map should contain cacti");
    }
}
