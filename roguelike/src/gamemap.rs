use std::collections::{HashMap, HashSet};

use crate::components::Faction;
use crate::grid_vec::GridVec;
use crate::noise::{fbm, value_noise, NoiseSeed};
use crate::typeenums::{Floor, HeightTier, Props, WallMaterial};
use crate::typedefs::{create_2d_array, CoordinateUnit, MyPoint, RenderPacket, SPAWN_POINT};
use crate::voxel::Voxel;

/// Side length of a map chunk (tiles). Used for chunk-based spatial queries.
pub const CHUNK_SIZE: CoordinateUnit = 16;

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
    /// Construction material for each wall tile.
    /// Only populated for tiles that have `Props::Wall`.
    pub wall_materials: HashMap<GridVec, WallMaterial>,
    /// Tiles flagged as breach points — shared walls between adjacent
    /// buildings that are thinner and cheaper to breach.
    pub breach_points: HashSet<GridVec>,
    /// Height tier for each building tile (floor area).
    /// Used to determine rooftop advantage and sight-line bonuses.
    pub building_heights: HashMap<GridVec, HeightTier>,
    /// Faction anchor buildings: (center position, faction, building name).
    /// Factions seed from these defensible positions rather than randomly.
    pub faction_anchors: Vec<(GridVec, Faction, String)>,
}

/// A rectangular building footprint used during town generation.
struct Building {
    x: CoordinateUnit,
    y: CoordinateUnit,
    w: CoordinateUnit,
    h: CoordinateUnit,
    /// What kind of building: 0=house, 1=saloon, 2=stable, 3=general store,
    /// 4=sheriff's office, 5=post office, 6=church, 7=bank, 8=hotel,
    /// 9=jail, 10=undertaker, 11=blacksmith.
    kind: u32,
    /// Construction material determining wall breachability and flammability.
    material: WallMaterial,
    /// Height tier — single story, double story, or tower.
    height_tier: HeightTier,
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
    ///   5. **Street props** — benches, lamp posts, barrels, crates,
    ///      hitching posts, water troughs, signs placed along streets.
    ///   6. **Decorative elements** — cacti, dead trees, rocks in open areas.
    ///   7. **Spawn clearing** — guaranteed open area around the player spawn.
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

                let prop = None;

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
            wall_materials: HashMap::new(),
            breach_points: HashSet::new(),
            building_heights: HashMap::new(),
            faction_anchors: Vec::new(),
        };

        // ── Step 1b: Forest on outskirts ────────────────────────────
        let forest_margin = 30; // tiles from edge where forest is dense
        let forest_seed = seed.wrapping_add(77700);
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let dist_to_edge = x.min(y).min(width - 1 - x).min(height - 1 - y);
                if dist_to_edge >= forest_margin {
                    continue;
                }
                let density = (1.0 - dist_to_edge as f64 / forest_margin as f64).powi(2);
                let noise = value_noise(x, y, forest_seed);
                if noise < density * 0.7 {
                    let pos = GridVec::new(x, y);
                    if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                        voxel.floor = Some(Floor::Grass);
                        voxel.props = Some(Props::Tree);
                    }
                }
            }
        }
        // Carve small paths through the forest
        let path_seed = seed.wrapping_add(77711);
        for i in 0..8i32 {
            let angle = value_noise(i, 0, path_seed) * std::f64::consts::TAU;
            let cx = width / 2;
            let cy = height / 2;
            let mut px = cx as f64;
            let mut py = cy as f64;
            let dx = angle.cos();
            let dy = angle.sin();
            while px > 0.0 && px < (width - 1) as f64 && py > 0.0 && py < (height - 1) as f64 {
                let ix = px as CoordinateUnit;
                let iy = py as CoordinateUnit;
                for oy in -1..=1i32 {
                    for ox in -1..=1i32 {
                        let p = GridVec::new(ix + ox, iy + oy);
                        if let Some(voxel) = map.get_voxel_at_mut(&p)
                            && matches!(voxel.props, Some(Props::Tree))
                        {
                            voxel.props = None;
                        }
                    }
                }
                px += dx;
                py += dy;
            }
        }

        // ── Step 2: River through center ────────────────────────────
        let river_seed = seed.wrapping_add(88800);
        let river_cx = width as f64 / 2.0;
        // River flows top to bottom with layered sinusoidal wobble and
        // noise-driven meanders for a more natural, organic shape.
        for y in 1..height - 1 {
            let fy = y as f64;
            // Multi-octave wobble: large sweeping bends + medium curves + fine noise
            let wobble = (fy * 0.008).sin() * 40.0      // large meander
                + (fy * 0.020).sin() * 20.0              // medium curve
                + (fy * 0.045).cos() * 8.0               // small ripple
                + value_noise(0, y, river_seed) * 10.0   // per-row noise jitter
                + value_noise(y, 0, river_seed.wrapping_add(222)) * 5.0;
            let center_x = river_cx + wobble;
            // River width varies smoothly along its length.
            let base_width = 7.0 + value_noise(y, 0, river_seed.wrapping_add(111)) * 5.0;
            let width_pulse = (fy * 0.012).sin() * 3.0; // gentle widening/narrowing
            let river_width = (base_width + width_pulse).max(4.0);
            let beach_width = 2.0;

            for x in 1..width - 1 {
                let fx = x as f64;
                let dist = (fx - center_x).abs();
                let pos = GridVec::new(x, y);
                if dist < river_width * 0.4 {
                    if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                        voxel.floor = Some(Floor::DeepWater);
                        voxel.props = None;
                    }
                } else if dist < river_width {
                    if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                        voxel.floor = Some(Floor::ShallowWater);
                        voxel.props = None;
                    }
                } else if dist < river_width + beach_width
                    && let Some(voxel) = map.get_voxel_at_mut(&pos)
                        && !matches!(voxel.props, Some(Props::Wall)) {
                            voxel.floor = Some(Floor::Beach);
                            voxel.props = None;
                        }
            }
        }

        // ── Step 3: Curved street grid with sidewalks ────────────────
        // Horizontal avenues: wide dirt carriage roads flanked by sidewalks
        // Avenue spacing varies per seed for unique city feel.
        // Compute avenue Y positions first so bridges can align to them.
        let spacing_noise = value_noise(0, 0, seed.wrapping_add(99900));
        let avenue_spacing = 32 + (spacing_noise * 12.0) as CoordinateUnit; // 32-44
        let avenue_half_width = 3; // carriage road half-width (7 tiles total)
        let sidewalk_width = 2; // sidewalk on each side of the road
        let mut avenue_ys: Vec<CoordinateUnit> = Vec::new();
        let curve_seed = seed.wrapping_add(55500);
        {
            let first_avenue = 40; // start inside the forest margin
            let mut ay = first_avenue;
            while ay < height - 40 {
                avenue_ys.push(ay);
                ay += avenue_spacing;
            }
        }

        // Place bridges at avenue Y positions so they connect flush with roads.
        let bridge_ys: Vec<CoordinateUnit> = avenue_ys.clone();
        for &by in &bridge_ys {
            for dy in -3..=3i32 {
                let y = by + dy;
                if y <= 0 || y >= height - 1 { continue; }
                for x in 1..width - 1 {
                    let pos = GridVec::new(x, y);
                    if let Some(voxel) = map.get_voxel_at(&pos)
                        && matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater))
                            && let Some(voxel) = map.get_voxel_at_mut(&pos) {
                                voxel.floor = Some(Floor::Bridge);
                                voxel.props = None;
                            }
                }
            }
        }

        // Lay avenue roads with sidewalks
        for &ay in &avenue_ys {
            let curve_amp = 3.0 + value_noise(ay, 0, curve_seed) * 4.0;
            let curve_freq = 0.015 + value_noise(0, ay, curve_seed) * 0.01;
            for x in 1..width - 1 {
                let curve_offset = (x as f64 * curve_freq).sin() * curve_amp;
                // Sidewalk (outer band)
                for sw in 1..=sidewalk_width {
                    for sign in [-1i32, 1] {
                        let y = ay + sign * (avenue_half_width + sw) + curve_offset as CoordinateUnit;
                        if y <= 0 || y >= height - 1 { continue; }
                        let pos = GridVec::new(x, y);
                        if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                            if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Bridge)) {
                                continue;
                            }
                            voxel.floor = Some(Floor::Sidewalk);
                            voxel.props = None;
                        }
                    }
                }
                // Carriage road (inner band)
                for hw in -avenue_half_width..=avenue_half_width {
                    let y = ay + hw + curve_offset as CoordinateUnit;
                    if y <= 0 || y >= height - 1 { continue; }
                    let pos = GridVec::new(x, y);
                    if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                        // Don't pave over river
                        if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Bridge)) {
                            continue;
                        }
                        voxel.floor = Some(Floor::Dirt);
                        voxel.props = None;
                    }
                }
            }
        }

        // Vertical cross streets with sinusoidal curvature and sidewalks
        let cross_seed = seed.wrapping_add(66666);
        // Cross street spacing varies per seed.
        let cross_noise = value_noise(1, 1, seed.wrapping_add(99901));
        let cross_spacing = 28 + (cross_noise * 14.0) as CoordinateUnit; // 28-42
        let cross_half_width = 2; // narrower than avenues
        let cross_sidewalk_width = 1;
        let mut cross_xs: Vec<CoordinateUnit> = Vec::new();
        {
            let mut cx = 40i32;
            let mut ci = 0i32;
            while cx < width - 40 {
                let jitter = (value_noise(ci, 0, cross_seed) * 10.0) as CoordinateUnit - 5;
                let actual_cx = (cx + jitter).clamp(2, width - 3);
                cross_xs.push(actual_cx);
                let curve_amp = 2.0 + value_noise(ci, 1, cross_seed) * 3.0;
                let curve_freq = 0.02 + value_noise(1, ci, cross_seed) * 0.01;
                for y in 1..height - 1 {
                    let curve_offset = (y as f64 * curve_freq).sin() * curve_amp;
                    // Sidewalk — cross streets are laid after avenues, so we
                    // must not overwrite existing avenue dirt roads or building walls.
                    for sw in 1..=cross_sidewalk_width {
                        for sign in [-1i32, 1] {
                            let x = actual_cx + sign * (cross_half_width + sw) + curve_offset as CoordinateUnit;
                            if x <= 0 || x >= width - 1 { continue; }
                            let pos = GridVec::new(x, y);
                            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                                if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Bridge)) {
                                    continue;
                                }
                                if !matches!(voxel.props, Some(Props::Wall))
                                    && !matches!(voxel.floor, Some(Floor::Dirt))
                                {
                                    voxel.floor = Some(Floor::Sidewalk);
                                    voxel.props = None;
                                }
                            }
                        }
                    }
                    // Carriage road
                    for hw in -cross_half_width..=cross_half_width {
                        let x = actual_cx + hw + curve_offset as CoordinateUnit;
                        if x <= 0 || x >= width - 1 { continue; }
                        let pos = GridVec::new(x, y);
                        if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                            if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Bridge)) {
                                continue;
                            }
                            if !matches!(voxel.props, Some(Props::Wall)) {
                                voxel.floor = Some(Floor::Dirt);
                                voxel.props = None;
                            }
                        }
                    }
                }
                cx += cross_spacing;
                ci += 1;
            }
        }

        // ── Step 4: Generate buildings filling city blocks ───────────
        let buildings = generate_buildings(width, height, seed, &avenue_ys, avenue_half_width);

        // Detect shared walls between adjacent buildings before placing them.
        // Two buildings share a wall when their perimeters overlap or are
        // separated by at most 1 tile (narrow gap).
        let shared_walls = detect_shared_walls(&buildings);

        for b in &buildings {
            // Don't place buildings on water
            let center = GridVec::new(b.x + b.w / 2, b.y + b.h / 2);
            if let Some(voxel) = map.get_voxel_at(&center)
                && matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Beach)) {
                    continue;
                }
            place_building(&mut map, b, seed);
        }

        // Mark shared walls as breach points and record wall materials
        for &pos in &shared_walls {
            if let Some(voxel) = map.get_voxel_at(&pos)
                && matches!(voxel.props, Some(Props::Wall)) {
                    map.breach_points.insert(pos);
                }
        }

        // Record wall materials and building heights for all placed buildings
        for b in &buildings {
            let center = GridVec::new(b.x + b.w / 2, b.y + b.h / 2);
            if let Some(voxel) = map.get_voxel_at(&center)
                && matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Beach)) {
                    continue;
                }
            record_building_metadata(&mut map, b);
        }

        // Place narrow alleys between adjacent buildings within blocks
        place_alleys(&mut map, &buildings);

        // ── Step 4b: Assign faction anchors from building kinds ──────
        assign_faction_anchors(&mut map, &buildings);

        // ── Step 5: Landmark buildings ──────────────────────────────
        place_mission(&mut map, width, height, seed);
        place_town_hall(&mut map, width, height, seed);
        place_grand_saloon(&mut map, width, height, seed);

        // ── Step 5b: Town plaza (open killzone) ─────────────────────
        place_town_plaza(&mut map, width, height, seed);
        place_cemetery(&mut map, width, height, seed);
        place_corral(&mut map, width, height, seed);

        // ── Step 5c: Additional urban features ──────────────────────
        place_town_well(&mut map, width, height, seed);
        place_gallows(&mut map, width, height, seed);
        place_water_tower(&mut map, width, height, seed);
        place_railroad(&mut map, width, height, seed);
        place_windmill(&mut map, width, height, seed);

        // ── Step 6: Street props along every avenue ──────────────
        for &ay in &avenue_ys {
            place_street_props(&mut map, width, ay, avenue_half_width, seed);
        }
        // ── Step 6b: Lamp posts along cross streets ─────────────
        for &cx in &cross_xs {
            place_lamp_posts(&mut map, height, cx, seed);
        }

        // ── Step 7: Decorative elements in open areas ───────────────
        place_desert_decorations(&mut map, width, height, seed);

        // ── Step 9: Spawn clearing (bottom-left) ───────────────────
        clear_around(&mut map, SPAWN_POINT, 6);

        // ── Cleanup: prune breach points for tiles no longer walls ──
        // Landmark placement (mission, plaza, etc.) may overwrite
        // previously detected shared walls.
        let invalid_breach: Vec<GridVec> = map.breach_points.iter()
            .filter(|pos| {
                !map.get_voxel_at(pos)
                    .is_some_and(|v| matches!(v.props, Some(Props::Wall)))
            })
            .copied()
            .collect();
        for pos in invalid_breach {
            map.breach_points.remove(&pos);
        }

        map
    }

    /// Get a reference to the voxel at the given map coordinate.
    #[inline]
    pub fn get_voxel_at(&self, point: &MyPoint) -> Option<&Voxel> {
        let GridVec { x, y } = *point;
        if x >= 0 && x < self.width && y >= 0 && y < self.height {
            Some(&self.voxels[y as usize][x as usize])
        } else {
            None
        }
    }

    /// Get a mutable reference to the voxel at the given map coordinate.
    #[inline]
    pub fn get_voxel_at_mut(&mut self, point: &MyPoint) -> Option<&mut Voxel> {
        let GridVec { x, y } = *point;
        if x >= 0 && x < self.width && y >= 0 && y < self.height {
            Some(&mut self.voxels[y as usize][x as usize])
        } else {
            None
        }
    }

    /// Returns `true` if the tile at `point` is passable (no blocking props).
    #[inline]
    pub fn is_passable(&self, point: &MyPoint) -> bool {
        self.get_voxel_at(point)
            .is_some_and(|v| match &v.props {
                Some(f) => !f.blocks_movement(),
                None => true,
            })
    }

    /// Returns `true` if the tile is suitable for spawning an entity:
    /// passable, no props, and not fire or water.
    pub fn is_spawnable(&self, point: &MyPoint) -> bool {
        if !self.is_passable(point) { return false; }
        match self.get_voxel_at(point) {
            Some(v) => {
                v.props.is_none()
                    && !matches!(v.floor, Some(Floor::Fire) | Some(Floor::DeepWater) | Some(Floor::ShallowWater))
            }
            None => false,
        }
    }

    /// Finds a passable tile right outside the door of a house.
    /// Scans the map near the bottom-left for a building doorway.
    /// Returns `None` if no suitable location is found.
    pub fn find_house_exterior(&self) -> Option<GridVec> {
        // Search near SPAWN_POINT for a building with a doorway
        let search_radius = 40;
        let sx = SPAWN_POINT.x;
        let sy = SPAWN_POINT.y;
        for y in sy.saturating_sub(search_radius).max(1)..=(sy + search_radius).min(self.height - 2) {
            for x in sx.saturating_sub(search_radius).max(1)..=(sx + search_radius).min(self.width - 2) {
                let pos = GridVec::new(x, y);
                if let Some(voxel) = self.get_voxel_at(&pos)
                    && voxel.props.is_none()
                    && matches!(voxel.floor, Some(Floor::Dirt) | Some(Floor::Sand) | Some(Floor::Gravel) | Some(Floor::Grass) | Some(Floor::Sidewalk))
                {
                    // Check if there's an adjacent wall (meaning we're just outside a building)
                    let has_adjacent_wall = pos.cardinal_neighbors().iter().any(|n| {
                        self.get_voxel_at(n).is_some_and(|v| matches!(v.props, Some(Props::Wall)))
                    });
                    if has_adjacent_wall {
                        return Some(pos);
                    }
                }
            }
        }
        None
    }

    /// Creates a RenderPacket with visibility-based dimming.
    ///
    /// The entire map is always visible (no fog of war / hidden areas).
    /// Tiles are rendered in two states:
    /// - **Visible** (in player's current FOV): full brightness.
    /// - **Not visible** (outside player's FOV): dimmed but still shown.
    ///
    /// When `visible_tiles` is `None`, all tiles render at full brightness.
    pub fn create_render_packet_with_fog(
        &self,
        center: &MyPoint,
        render_width: u16,
        render_height: u16,
        visible_tiles: Option<&std::collections::HashSet<MyPoint>>,
        _revealed_tiles: Option<&std::collections::HashSet<MyPoint>>,
    ) -> RenderPacket {
        let w_radius = render_width as CoordinateUnit / 2;
        let h_radius = render_height as CoordinateUnit / 2;

        let bottom_left = *center - GridVec::new(w_radius, h_radius);

        let mut grid = create_2d_array(render_width as usize, render_height as usize);

        // Note: iteration is already viewport-scoped (only tiles within render_width/height
        // are processed), which is equivalent to chunk-based culling for the viewport.
        for ry in 0..render_height as CoordinateUnit {
            for rx in 0..render_width as CoordinateUnit {
                let world_pos = bottom_left + GridVec::new(rx, ry);

                if let Some(voxel) = self.get_voxel_at(&world_pos) {
                    let is_visible = visible_tiles
                        .map(|vt| vt.contains(&world_pos))
                        .unwrap_or(true);

                    // Always show the map — visible at full brightness, rest dimmed
                    grid[ry as usize][rx as usize] = voxel.to_graphic(is_visible);
                }
            }
        }

        grid
    }

    /// Returns which chunk coordinates overlap the given viewport.
    pub fn active_chunks(&self, center: &GridVec, render_width: u16, render_height: u16) -> Vec<(CoordinateUnit, CoordinateUnit)> {
        let w_radius = render_width as CoordinateUnit / 2;
        let h_radius = render_height as CoordinateUnit / 2;
        let min_x = (center.x - w_radius).div_euclid(CHUNK_SIZE);
        let max_x = (center.x + w_radius).div_euclid(CHUNK_SIZE);
        let min_y = (center.y - h_radius).div_euclid(CHUNK_SIZE);
        let max_y = (center.y + h_radius).div_euclid(CHUNK_SIZE);
        let mut chunks = Vec::new();
        for cy in min_y..=max_y {
            for cx in min_x..=max_x {
                chunks.push((cx, cy));
            }
        }
        chunks
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

/// District types that influence which building kinds appear in each area.
/// Derived from Voronoi-style partitioning of the town into themed zones.
const DISTRICT_RESIDENTIAL: u32 = 0;
const DISTRICT_COMMERCIAL: u32 = 1;
const DISTRICT_LIVERY: u32 = 2;

/// Number of distinct district types.
/// District 3 is the cantina row (saloons, hotels, entertainment)
/// and is handled by the catch-all arm of `district_building_kind`.
const DISTRICT_COUNT: u32 = 4;

/// Assigns a district type based on position using Voronoi-style partitioning.
/// Deterministic based on position and seed.
fn get_district(x: CoordinateUnit, y: CoordinateUnit, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) -> u32 {
    let district_seed = seed.wrapping_add(222333);
    // Place ~8 Voronoi sites across the town
    let num_sites = 8;
    let mut min_dist = i64::MAX;
    let mut best_district = DISTRICT_RESIDENTIAL;
    for i in 0..num_sites {
        let sx = (value_noise(i, 0, district_seed) * (width - 80) as f64) as CoordinateUnit + 40;
        let sy = (value_noise(0, i, district_seed) * (height - 80) as f64) as CoordinateUnit + 40;
        let dx = (x - sx) as i64;
        let dy = (y - sy) as i64;
        let dist = dx * dx + dy * dy;
        if dist < min_dist {
            min_dist = dist;
            // Assign district type based on site index
            best_district = (value_noise(i, i, district_seed.wrapping_add(444)) * DISTRICT_COUNT as f64) as u32;
            best_district = best_district.min(DISTRICT_COUNT - 1);
        }
    }
    best_district
}

/// Selects a building kind based on district type, using weighted random tables.
fn district_building_kind(district: u32, noise: f64) -> u32 {
    match district {
        DISTRICT_RESIDENTIAL => {
            // Mostly houses, some churches, hotels
            if noise < 0.55 { 0 }       // House
            else if noise < 0.70 { 6 }   // Church
            else if noise < 0.85 { 8 }   // Hotel
            else { 5 }                    // Post Office
        }
        DISTRICT_COMMERCIAL => {
            // General stores, banks, post offices, sheriff
            if noise < 0.30 { 3 }        // General Store
            else if noise < 0.50 { 7 }   // Bank
            else if noise < 0.65 { 4 }   // Sheriff's Office
            else if noise < 0.80 { 9 }   // Jail
            else { 5 }                    // Post Office
        }
        DISTRICT_LIVERY => {
            // Stables, blacksmiths, general stores
            if noise < 0.35 { 2 }        // Stable
            else if noise < 0.60 { 11 }  // Blacksmith
            else if noise < 0.80 { 3 }   // General Store
            else { 10 }                   // Undertaker
        }
        // Cantina row (district 3) / catch-all: saloons, hotels.
        _ => {
            if noise < 0.40 { 1 }        // Saloon
            else if noise < 0.65 { 8 }   // Hotel
            else if noise < 0.80 { 0 }   // House
            else { 1 }                    // Saloon
        }
    }
}

/// Generates deterministic building footprints for the western town.
///
/// Buildings are placed in rows between every pair of adjacent avenues,
/// filling the entire map with dense city blocks. Buildings are larger
/// (10-18 wide, 8-16 tall) so interiors feel like actual rooms.
/// Uses Voronoi-based districts for themed building type selection.
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
        let bot = first - avenue_half_width - 4;
        if bot - top >= 8 {
            row_bands.push((top, bot));
        }
    }

    // Bands between each pair of avenues
    for pair in avenue_ys.windows(2) {
        let top = pair[0] + avenue_half_width + 4;
        let bot = pair[1] - avenue_half_width - 4;
        if bot - top >= 8 {
            row_bands.push((top, bot));
        }
    }

    // Band below the last avenue
    if let Some(&last) = avenue_ys.last() {
        let top = last + avenue_half_width + 4;
        let bot = height - 4;
        if bot - top >= 8 {
            row_bands.push((top, bot));
        }
    }

    for (row_idx, &(row_min_y, row_max_y)) in row_bands.iter().enumerate() {
        // Vary the starting offset per row for less grid-like placement
        let row_offset_noise = value_noise(row_idx as i32, row_min_y, bldg_seed.wrapping_add(6666));
        let mut cx = 4 + (row_offset_noise * 6.0) as CoordinateUnit;
        let mut bldg_index = 0u32;
        let band_height = row_max_y - row_min_y;
        while cx < width - 10 {
            let noise = value_noise(cx, bldg_index as i32 + row_idx as i32, bldg_seed);
            let kind_noise = value_noise(bldg_index as i32, cx + row_idx as i32, bldg_seed.wrapping_add(2222));

            // Larger buildings: 10-18 wide, 8-16 tall
            let bw = 10 + (noise * 9.0) as CoordinateUnit; // width 10–18
            let max_h = band_height.min(16);
            let bh = 8.max(max_h - (noise * 4.0) as CoordinateUnit); // height 8–max_h
            let by_jitter = (value_noise(cx, row_min_y, bldg_seed.wrapping_add(3333)) * 2.0) as CoordinateUnit;
            let by = row_min_y + by_jitter;

            // Don't exceed row bounds or map bounds
            if by + bh <= row_max_y && by > 0 && by + bh < height - 1 && cx + bw < width - 1 {
                // District-based building type
                let district = get_district(cx + bw / 2, by + bh / 2, width, height, seed);
                let kind = district_building_kind(district, kind_noise);
                let kind = kind.min(BUILDING_TYPE_COUNT - 1);
                let material = building_material(kind, noise);
                let height_tier = building_height(kind, noise);
                buildings.push(Building {
                    x: cx,
                    y: by,
                    w: bw,
                    h: bh,
                    kind,
                    material,
                    height_tier,
                });
            }

            // Gap between buildings: wider gaps (2-6 tiles) to create open
            // spaces and breathing room between structures.
            let gap_noise = value_noise(cx + 1, bldg_index as i32, bldg_seed.wrapping_add(4444));
            cx += bw + 2 + (gap_noise * 5.0) as CoordinateUnit;
            bldg_index += 1;
        }
    }

    buildings
}

/// Determines the wall material for a building based on its kind and noise.
/// Churches, banks, jails use stone; stables, houses use timber; rest adobe.
fn building_material(kind: u32, noise: f64) -> WallMaterial {
    match kind {
        6 | 7 | 9 => WallMaterial::Stone,     // Church, Bank, Jail
        0 | 2 | 11 => {                        // House, Stable, Blacksmith
            if noise < 0.4 { WallMaterial::Timber } else { WallMaterial::Adobe }
        }
        _ => WallMaterial::Adobe,              // Saloon, Store, Sheriff, etc.
    }
}

/// Determines the height tier for a building based on its kind and noise.
/// Churches and banks tend to be taller; stables and houses single-story.
fn building_height(kind: u32, noise: f64) -> HeightTier {
    match kind {
        6 => HeightTier::Tower,                // Church — bell tower
        7 | 8 => HeightTier::DoubleStory,      // Bank, Hotel — two floors
        1 => {                                  // Saloon — sometimes two-story
            if noise > 0.5 { HeightTier::DoubleStory } else { HeightTier::SingleStory }
        }
        4 | 9 => {                              // Sheriff, Jail — sometimes watchtower
            if noise > 0.7 { HeightTier::Tower } else { HeightTier::SingleStory }
        }
        _ => HeightTier::SingleStory,
    }
}

/// Detects wall tiles that are shared between adjacent buildings or separated
/// by only a narrow gap (0-1 tile). These are flagged as potential breach points.
fn detect_shared_walls(buildings: &[Building]) -> HashSet<GridVec> {
    let mut shared = HashSet::new();

    for (i, a) in buildings.iter().enumerate() {
        for b in &buildings[i + 1..] {
            // Check if buildings are adjacent or share a wall.
            // Horizontal adjacency (buildings side by side in same row)
            if a.y < b.y + b.h && b.y < a.y + a.h {
                // a's right edge touches or is 1 tile from b's left edge
                let gap_right = b.x - (a.x + a.w);
                if (0..=1).contains(&gap_right) {
                    let overlap_y_min = a.y.max(b.y);
                    let overlap_y_max = (a.y + a.h).min(b.y + b.h);
                    for y in overlap_y_min..overlap_y_max {
                        // Mark the right wall of a and left wall of b
                        shared.insert(GridVec::new(a.x + a.w - 1, y));
                        shared.insert(GridVec::new(b.x, y));
                    }
                }
                // b's right edge touches or is 1 tile from a's left edge
                let gap_left = a.x - (b.x + b.w);
                if (0..=1).contains(&gap_left) {
                    let overlap_y_min = a.y.max(b.y);
                    let overlap_y_max = (a.y + a.h).min(b.y + b.h);
                    for y in overlap_y_min..overlap_y_max {
                        shared.insert(GridVec::new(b.x + b.w - 1, y));
                        shared.insert(GridVec::new(a.x, y));
                    }
                }
            }
            // Vertical adjacency (buildings stacked in same column)
            if a.x < b.x + b.w && b.x < a.x + a.w {
                let gap_below = b.y - (a.y + a.h);
                if (0..=1).contains(&gap_below) {
                    let overlap_x_min = a.x.max(b.x);
                    let overlap_x_max = (a.x + a.w).min(b.x + b.w);
                    for x in overlap_x_min..overlap_x_max {
                        shared.insert(GridVec::new(x, a.y + a.h - 1));
                        shared.insert(GridVec::new(x, b.y));
                    }
                }
                let gap_above = a.y - (b.y + b.h);
                if (0..=1).contains(&gap_above) {
                    let overlap_x_min = a.x.max(b.x);
                    let overlap_x_max = (a.x + a.w).min(b.x + b.w);
                    for x in overlap_x_min..overlap_x_max {
                        shared.insert(GridVec::new(x, b.y + b.h - 1));
                        shared.insert(GridVec::new(x, a.y));
                    }
                }
            }
        }
    }

    shared
}

/// Records wall material and height tier metadata for a placed building.
fn record_building_metadata(map: &mut GameMap, b: &Building) {
    for y in b.y..b.y + b.h {
        for x in b.x..b.x + b.w {
            let pos = GridVec::new(x, y);
            let is_border = x == b.x || x == b.x + b.w - 1 || y == b.y || y == b.y + b.h - 1;
            if is_border {
                // Record wall material for wall tiles
                if let Some(voxel) = map.get_voxel_at(&pos)
                    && matches!(voxel.props, Some(Props::Wall)) {
                        map.wall_materials.insert(pos, b.material);
                    }
            }
            // Record height tier for all building tiles
            map.building_heights.insert(pos, b.height_tier);
        }
    }
}

/// Places narrow alley floor tiles in the gaps between adjacent buildings
/// within the same block. Alleys are shadowed ambush terrain.
fn place_alleys(map: &mut GameMap, buildings: &[Building]) {
    for (i, a) in buildings.iter().enumerate() {
        for b in &buildings[i + 1..] {
            // Only horizontal adjacency with 1-2 tile gap
            if a.y < b.y + b.h && b.y < a.y + a.h {
                let gap = b.x as i64 - (a.x + a.w) as i64;
                if (1..=2).contains(&gap) {
                    let overlap_y_min = a.y.max(b.y);
                    let overlap_y_max = (a.y + a.h).min(b.y + b.h);
                    for y in overlap_y_min..overlap_y_max {
                        for gx in 0..gap as CoordinateUnit {
                            let pos = GridVec::new(a.x + a.w + gx, y);
                            if let Some(voxel) = map.get_voxel_at_mut(&pos)
                                && voxel.props.is_none() && !matches!(voxel.floor, Some(Floor::WoodPlanks)) {
                                    voxel.floor = Some(Floor::Alley);
                                }
                        }
                    }
                }
                // Check reverse direction
                let gap_rev = a.x as i64 - (b.x + b.w) as i64;
                if (1..=2).contains(&gap_rev) {
                    let overlap_y_min = a.y.max(b.y);
                    let overlap_y_max = (a.y + a.h).min(b.y + b.h);
                    for y in overlap_y_min..overlap_y_max {
                        for gx in 0..gap_rev as CoordinateUnit {
                            let pos = GridVec::new(b.x + b.w + gx, y);
                            if let Some(voxel) = map.get_voxel_at_mut(&pos)
                                && voxel.props.is_none() && !matches!(voxel.floor, Some(Floor::WoodPlanks)) {
                                    voxel.floor = Some(Floor::Alley);
                                }
                        }
                    }
                }
            }
        }
    }
}

/// Assigns faction anchor buildings — each faction seeds from a specific
/// defensible building type rather than spawning randomly.
fn assign_faction_anchors(map: &mut GameMap, buildings: &[Building]) {
    // Map building kinds to factions:
    // 1=Saloon → Outlaws, 2=Stable → Vaqueros, 4=Sheriff → Sheriff,
    // 7=Bank → Lawmen, 8=Hotel → Civilians, 0=House → Indians (outskirts)
    let mut used_kinds: HashSet<u32> = HashSet::new();
    let mut indians_anchor_assigned = false;

    for b in buildings {
        let center = GridVec::new(b.x + b.w / 2, b.y + b.h / 2);
        let (faction, name) = match b.kind {
            1 if !used_kinds.contains(&1) => {
                used_kinds.insert(1);
                (Faction::Outlaws, "Cantina".to_string())
            }
            2 if !used_kinds.contains(&2) => {
                used_kinds.insert(2);
                (Faction::Vaqueros, "Livery Stable".to_string())
            }
            4 if !used_kinds.contains(&4) => {
                used_kinds.insert(4);
                (Faction::Sheriff, "Marshal's Office".to_string())
            }
            7 if !used_kinds.contains(&7) => {
                used_kinds.insert(7);
                (Faction::Lawmen, "Bank".to_string())
            }
            8 if !used_kinds.contains(&8) => {
                used_kinds.insert(8);
                (Faction::Civilians, "Hotel".to_string())
            }
            0 if !indians_anchor_assigned => {
                // Use first house on outskirts for Indians
                let dist_to_center = center.distance_squared(GridVec::new(
                    map.width / 2, map.height / 2
                ));
                if dist_to_center > (map.width / 4) * (map.width / 4) {
                    indians_anchor_assigned = true;
                    (Faction::Indians, "Hacienda".to_string())
                } else {
                    continue;
                }
            }
            _ => continue,
        };
        map.faction_anchors.push((center, faction, name));
    }
}

/// Places a building on the map: walls around the perimeter, wood plank floor,
/// and interior props based on building kind.
/// Supports non-rectangular shapes for larger buildings.
const SHAPE_RECT: u32 = 0;
const SHAPE_ROUNDED: u32 = 1;
const SHAPE_L: u32 = 2;

/// Corner radius for rounded-corner building shapes.
const ROUNDED_CORNER_RADIUS: CoordinateUnit = 2;

/// L-shape notch is ~1/N of building dimensions.
const L_SHAPE_NOTCH_DIVISOR: CoordinateUnit = 3;

fn place_building(map: &mut GameMap, b: &Building, seed: NoiseSeed) {
    // Determine building shape based on noise
    let shape_noise = value_noise(b.x + b.y, b.w + b.h, seed.wrapping_add(77777));
    let shape_type = if b.w >= 8 && b.h >= 8 {
        (shape_noise * 3.0) as u32 // SHAPE_RECT, SHAPE_ROUNDED, or SHAPE_L
    } else {
        SHAPE_RECT
    };

    // For L-shape, compute notch dimensions
    let notch_w = b.w / L_SHAPE_NOTCH_DIVISOR;
    let notch_h = b.h / L_SHAPE_NOTCH_DIVISOR;
    // Notch is in top-right corner
    let notch_x_start = b.x + b.w - notch_w;
    let notch_y_start = b.y + b.h - notch_h;

    let corner_radius = ROUNDED_CORNER_RADIUS;

    for y in b.y..b.y + b.h {
        for x in b.x..b.x + b.w {
            let pos = GridVec::new(x, y);

            // L-shape: skip the notch area entirely
            if shape_type == SHAPE_L && x >= notch_x_start && y >= notch_y_start {
                continue;
            }

            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                let mut is_border = x == b.x || x == b.x + b.w - 1 || y == b.y || y == b.y + b.h - 1;
                // For L-shape, the notch edges are also borders
                if shape_type == SHAPE_L {
                    // Recompute border for L-shape
                    let in_main = x >= b.x && x < b.x + b.w && y >= b.y && y < b.y + b.h
                        && !(x >= notch_x_start && y >= notch_y_start);
                    if in_main {
                        let left_out = x == b.x;
                        let right_out = x == b.x + b.w - 1 && y < notch_y_start;
                        let right_notch_edge = x == notch_x_start - 1 && y >= notch_y_start;
                        // right edge for L: either original right wall (above notch) or notch inner edge
                        let top_out = y == b.y + b.h - 1 && x < notch_x_start;
                        let top_notch_edge = y == notch_y_start - 1 && x >= notch_x_start;
                        let bottom_out = y == b.y;
                        is_border = left_out || right_out || right_notch_edge || top_out || top_notch_edge || bottom_out;
                    }
                }

                // Door positions
                let is_door = (y == b.y + b.h - 1 && x == b.x + b.w / 2 && (shape_type != SHAPE_L || x < notch_x_start))
                    || (y == b.y && x == b.x + b.w / 2);

                // Side doors for bigger buildings
                let is_side_door = if b.w >= 9 || b.h >= 9 {
                    (x == b.x && y == b.y + b.h / 2)
                    || (x == b.x + b.w - 1 && y == b.y + b.h / 2 && (shape_type != SHAPE_L || y < notch_y_start))
                } else {
                    false
                };

                // Rounded corners: skip corner walls.
                // True when the tile is within `corner_radius` of both a
                // horizontal and a vertical building edge (i.e. in any of the
                // four corner regions).
                let is_corner = if shape_type == SHAPE_ROUNDED {
                    (b.x + b.w - 1 - x < corner_radius || x - b.x < corner_radius)
                        && (b.y + b.h - 1 - y < corner_radius || y - b.y < corner_radius)
                } else {
                    false
                };

                if is_border && !is_door && !is_side_door && !is_corner {
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
            // House: dining area, bedroom corner, storage
            if iw >= 4 && ih >= 4 {
                // Dining table with chairs
                set_prop(map, interior_x + 2, interior_y + 2, Props::Table);
                set_prop(map, interior_x + 1, interior_y + 2, Props::Chair);
                set_prop(map, interior_x + 3, interior_y + 2, Props::Chair);
                set_prop(map, interior_x + 2, interior_y + 1, Props::Chair);
                set_prop(map, interior_x + 2, interior_y + 3, Props::Chair);
                // Bedroom area (far corner)
                set_prop(map, interior_x + iw - 1, interior_y + ih - 1, Props::Bench);
                set_prop(map, interior_x + iw - 2, interior_y + ih - 1, Props::Bench);
                // Storage
                set_prop(map, interior_x, interior_y, Props::Barrel);
                set_prop(map, interior_x + iw - 1, interior_y, Props::Crate);
                if iw >= 6 {
                    set_prop(map, interior_x, interior_y + ih - 1, Props::Barrel);
                }
            } else if iw >= 2 && ih >= 2 {
                set_prop(map, interior_x + 1, interior_y + 1, Props::Table);
                set_prop(map, interior_x, interior_y + 1, Props::Chair);
            }
        }
        1 => {
            // Saloon: piano, bar counter, multiple table/chair clusters
            if iw >= 6 && ih >= 5 {
                set_prop(map, interior_x, interior_y, Props::Piano);
                // Bar (barrels along back wall)
                for dx in 2..iw.min(8) {
                    set_prop(map, interior_x + dx, interior_y, Props::Barrel);
                }
                // Table clusters in a grid
                for row in 0..(ih - 2) / 3 {
                    for col in 0..(iw - 1) / 4 {
                        let tx = interior_x + 1 + col * 4;
                        let ty = interior_y + 2 + row * 3;
                        if tx + 1 < interior_x + iw && ty < interior_y + ih {
                            set_prop(map, tx, ty, Props::Table);
                            set_prop(map, tx - 1, ty, Props::Chair);
                            set_prop(map, tx + 1, ty, Props::Chair);
                        }
                    }
                }
            } else if iw >= 4 && ih >= 3 {
                set_prop(map, interior_x, interior_y, Props::Piano);
                set_prop(map, interior_x + 2, interior_y + 1, Props::Table);
                set_prop(map, interior_x + 1, interior_y + 1, Props::Chair);
                set_prop(map, interior_x + 3, interior_y + 1, Props::Chair);
            }
        }
        2 => {
            // Stable: hitching posts along walls, hay bales, water troughs
            if iw >= 5 && ih >= 4 {
                // Hitching posts along one wall
                for dx in (0..iw).step_by(3) {
                    set_prop(map, interior_x + dx, interior_y, Props::HitchingPost);
                }
                // Hay bales along opposite wall
                for dx in (0..iw).step_by(2) {
                    set_prop(map, interior_x + dx, interior_y + ih - 1, Props::HayBale);
                }
                // Water trough and crates
                set_prop(map, interior_x + iw / 2, interior_y + ih / 2, Props::WaterTrough);
                set_prop(map, interior_x + iw - 1, interior_y + ih / 2, Props::Crate);
            } else if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x, interior_y, Props::HitchingPost);
                set_prop(map, interior_x + 2, interior_y, Props::HitchingPost);
                set_prop(map, interior_x + 1, interior_y + ih - 1, Props::WaterTrough);
            }
        }
        3 => {
            // General store: shelves (barrels/crates) along walls, counter
            if iw >= 5 && ih >= 4 {
                // Counter (tables)
                for dx in 1..iw - 1 {
                    set_prop(map, interior_x + dx, interior_y + ih / 2, Props::Table);
                }
                // Shelves along walls
                set_prop(map, interior_x, interior_y, Props::Barrel);
                set_prop(map, interior_x, interior_y + 1, Props::Crate);
                set_prop(map, interior_x + iw - 1, interior_y, Props::Crate);
                set_prop(map, interior_x + iw - 1, interior_y + 1, Props::Barrel);
                if ih >= 6 {
                    set_prop(map, interior_x, interior_y + ih - 1, Props::Crate);
                    set_prop(map, interior_x + iw - 1, interior_y + ih - 1, Props::Barrel);
                }
            } else if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x, interior_y, Props::Barrel);
                set_prop(map, interior_x + 1, interior_y, Props::Crate);
            }
        }
        4 => {
            // Sheriff's office: desk, cells in back, wanted posters
            if iw >= 5 && ih >= 4 {
                // Desk area
                set_prop(map, interior_x + 2, interior_y + 1, Props::Table);
                set_prop(map, interior_x + 1, interior_y + 1, Props::Chair);
                set_prop(map, interior_x + 3, interior_y + 1, Props::Chair);
                // Wanted posters
                set_prop(map, interior_x, interior_y, Props::Sign);
                set_prop(map, interior_x + 1, interior_y, Props::Sign);
                // Cell area (barrels as bars)
                set_prop(map, interior_x + iw - 1, interior_y + ih - 1, Props::Barrel);
                set_prop(map, interior_x + iw - 2, interior_y + ih - 1, Props::Barrel);
                set_prop(map, interior_x + iw - 1, interior_y + ih - 2, Props::Barrel);
            } else if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x + 1, interior_y, Props::Table);
                set_prop(map, interior_x, interior_y, Props::Chair);
            }
        }
        5 => {
            // Post office: service counter, mail storage
            if iw >= 5 && ih >= 4 {
                // Counter
                for dx in 1..iw - 1 {
                    set_prop(map, interior_x + dx, interior_y + 2, Props::Table);
                }
                // Mail crates behind counter
                set_prop(map, interior_x, interior_y, Props::Crate);
                set_prop(map, interior_x + 1, interior_y, Props::Crate);
                set_prop(map, interior_x + iw - 1, interior_y, Props::Crate);
                // Sign
                set_prop(map, interior_x + iw / 2, interior_y + ih - 1, Props::Sign);
            } else if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x + 1, interior_y, Props::Table);
                set_prop(map, interior_x, interior_y, Props::Crate);
            }
        }
        6 => {
            // Church: pews in rows, altar at front
            if iw >= 5 && ih >= 5 {
                // Altar
                set_prop(map, interior_x + iw / 2, interior_y, Props::Table);
                set_prop(map, interior_x + iw / 2 - 1, interior_y, Props::Sign);
                set_prop(map, interior_x + iw / 2 + 1, interior_y, Props::Sign);
                // Pew rows (benches on both sides of center aisle)
                for row in 2..ih.min(8) {
                    set_prop(map, interior_x + 1, interior_y + row, Props::Bench);
                    if iw >= 6 {
                        set_prop(map, interior_x + iw - 2, interior_y + row, Props::Bench);
                    }
                }
            } else if iw >= 3 && ih >= 3 {
                set_prop(map, interior_x + iw / 2, interior_y, Props::Table);
                for row in 1..ih.min(4) {
                    set_prop(map, interior_x, interior_y + row, Props::Bench);
                }
            }
        }
        7 => {
            // Bank: counter, vault area, strongboxes
            if iw >= 5 && ih >= 4 {
                // Counter
                for dx in 1..iw - 1 {
                    set_prop(map, interior_x + dx, interior_y + ih / 2, Props::Table);
                }
                // Vault (barrels as heavy door / safe)
                set_prop(map, interior_x, interior_y, Props::Barrel);
                set_prop(map, interior_x + 1, interior_y, Props::Barrel);
                set_prop(map, interior_x, interior_y + 1, Props::Barrel);
                // Strongboxes
                set_prop(map, interior_x + iw - 1, interior_y, Props::Crate);
                set_prop(map, interior_x + iw - 1, interior_y + ih - 1, Props::Crate);
                set_prop(map, interior_x + iw - 2, interior_y + ih - 1, Props::Crate);
            } else if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x + 1, interior_y, Props::Table);
                set_prop(map, interior_x, interior_y + ih - 1, Props::Barrel);
            }
        }
        8 => {
            // Hotel: lobby, guest rooms suggested by bed rows
            if iw >= 5 && ih >= 4 {
                // Lobby area
                set_prop(map, interior_x + 1, interior_y, Props::Table);
                set_prop(map, interior_x, interior_y, Props::Chair);
                set_prop(map, interior_x + 2, interior_y, Props::Chair);
                // Guest rooms (beds/benches in rows)
                for row in (2..ih).step_by(2) {
                    set_prop(map, interior_x, interior_y + row, Props::Bench);
                    if iw >= 6 {
                        set_prop(map, interior_x + iw - 1, interior_y + row, Props::Bench);
                    }
                }
                // Storage
                set_prop(map, interior_x + iw - 1, interior_y, Props::Barrel);
            } else if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x, interior_y, Props::Table);
                set_prop(map, interior_x + 1, interior_y, Props::Chair);
            }
        }
        9 => {
            // Jail: cells, wanted posters, desk
            if iw >= 5 && ih >= 4 {
                // Desk
                set_prop(map, interior_x + 1, interior_y, Props::Table);
                set_prop(map, interior_x, interior_y, Props::Chair);
                // Wanted posters
                set_prop(map, interior_x + 2, interior_y, Props::Sign);
                // Cell bars (barrels forming cell walls)
                for dy in 0..ih.min(5) {
                    set_prop(map, interior_x + iw - 1, interior_y + dy, Props::Barrel);
                }
                set_prop(map, interior_x + iw - 2, interior_y + ih - 1, Props::Barrel);
            } else if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x, interior_y, Props::Sign);
                set_prop(map, interior_x + iw - 1, interior_y, Props::Barrel);
            }
        }
        10 => {
            // Undertaker: preparation tables, coffins (crates)
            if iw >= 5 && ih >= 4 {
                // Tables in center
                set_prop(map, interior_x + 2, interior_y + 1, Props::Table);
                set_prop(map, interior_x + 2, interior_y + ih / 2, Props::Table);
                if iw >= 7 {
                    set_prop(map, interior_x + 4, interior_y + 1, Props::Table);
                }
                // Coffins (crates) along wall
                for dy in (0..ih).step_by(2) {
                    set_prop(map, interior_x, interior_y + dy, Props::Crate);
                }
                set_prop(map, interior_x + iw - 1, interior_y + ih - 1, Props::Crate);
            } else if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x + 1, interior_y, Props::Table);
                set_prop(map, interior_x, interior_y + ih - 1, Props::Crate);
            }
        }
        _ => {
            // Blacksmith: forge area, anvil, quench barrel, supplies
            if iw >= 5 && ih >= 4 {
                // Anvil (hitching post stand-in)
                set_prop(map, interior_x + iw / 2, interior_y + ih / 2, Props::HitchingPost);
                // Quench barrel
                set_prop(map, interior_x + iw / 2 + 1, interior_y + ih / 2, Props::Barrel);
                // Water trough
                set_prop(map, interior_x, interior_y + ih - 1, Props::WaterTrough);
                // Supplies along walls
                set_prop(map, interior_x + iw - 1, interior_y, Props::Crate);
                set_prop(map, interior_x + iw - 1, interior_y + 1, Props::Crate);
                set_prop(map, interior_x, interior_y, Props::Barrel);
            } else if iw >= 3 && ih >= 2 {
                set_prop(map, interior_x, interior_y, Props::HitchingPost);
                set_prop(map, interior_x + 1, interior_y + ih - 1, Props::Barrel);
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

/// Places the focal mission/church building near the center of the map.
/// This is a large, thick-walled stone structure with a courtyard, bell tower,
/// and multiple interior rooms that functions as a natural fortress and
/// late-game combat anchor.
fn place_mission(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) {
    let mw: CoordinateUnit = 24;
    let mh: CoordinateUnit = 18;
    if width < mw + 10 || height < mh + 10 {
        return; // map too small for a mission
    }
    let m_seed = seed.wrapping_add(555666);
    let cx = width / 2 + (value_noise(2, 2, m_seed) * 10.0) as CoordinateUnit - 5;
    let cy = height / 2 + (value_noise(3, 3, m_seed) * 10.0) as CoordinateUnit - 5;
    let mx = (cx - mw / 2).clamp(2, width - mw - 2);
    let my = (cy - mh / 2).clamp(2, height - mh - 2);

    // Lay down thick stone walls and interior
    for y in my..my + mh {
        for x in mx..mx + mw {
            let pos = GridVec::new(x, y);
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                let is_border = x == mx || x == mx + mw - 1 || y == my || y == my + mh - 1;
                // Main entrance (south wall center)
                let is_main_door = y == my + mh - 1 && (x == mx + mw / 2 || x == mx + mw / 2 - 1);
                // Side entrance (east wall)
                let is_side_door = x == mx + mw - 1 && y == my + mh / 2;
                // Back entrance (north wall)
                let is_back_door = y == my && x == mx + mw / 2;

                if is_border && !is_main_door && !is_side_door && !is_back_door {
                    voxel.props = Some(Props::Wall);
                    voxel.floor = Some(Floor::WoodPlanks);
                    map.wall_materials.insert(pos, WallMaterial::Stone);
                } else {
                    voxel.props = None;
                    voxel.floor = Some(Floor::WoodPlanks);
                }
            }
        }
    }

    // Interior dividing wall creating multiple rooms
    // Nave (main hall) on left, side rooms on right
    let divider_x = mx + mw * 2 / 3;
    for y in my + 1..my + mh - 1 {
        let pos = GridVec::new(divider_x, y);
        // Door in the divider
        if y != my + mh / 2 && y != my + mh / 3
            && let Some(voxel) = map.get_voxel_at_mut(&pos) {
                voxel.props = Some(Props::Wall);
                map.wall_materials.insert(pos, WallMaterial::Stone);
                map.breach_points.insert(pos);
            }
    }

    // Courtyard: open area in the center-right rooms (no roof)
    let court_x = divider_x + 1;
    let court_y = my + 2;
    let court_w = (mx + mw - 1) - court_x - 1;
    let court_h = mh / 2 - 2;
    for dy in 0..court_h {
        for dx in 0..court_w {
            let pos = GridVec::new(court_x + dx, court_y + dy);
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                voxel.floor = Some(Floor::Plaza);
                voxel.props = None;
            }
        }
    }

    // Bell tower: 3×3 tower structure in top-left corner of mission
    let tower_x = mx + 1;
    let tower_y = my + 1;
    for dy in 0..3i32 {
        for dx in 0..3i32 {
            let pos = GridVec::new(tower_x + dx, tower_y + dy);
            map.building_heights.insert(pos, HeightTier::Tower);
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                voxel.floor = Some(Floor::Rooftop);
            }
        }
    }

    // Record all mission tiles as Tower height
    for y in my..my + mh {
        for x in mx..mx + mw {
            let pos = GridVec::new(x, y);
            map.building_heights.entry(pos).or_insert(HeightTier::DoubleStory);
        }
    }

    // Interior props: altar, pews, signs
    let ix = mx + 1;
    let iy = my + 1;
    // Altar at the north end of the nave
    set_prop(map, ix + 4, iy, Props::Table);
    set_prop(map, ix + 3, iy, Props::Sign);
    set_prop(map, ix + 5, iy, Props::Sign);
    // Pew rows in the nave
    for row in 3..mh - 4 {
        set_prop(map, ix + 2, iy + row, Props::Bench);
        set_prop(map, ix + 6, iy + row, Props::Bench);
    }
    // Storage in side rooms
    set_prop(map, divider_x + 2, my + mh / 2 + 2, Props::Barrel);
    set_prop(map, divider_x + 3, my + mh / 2 + 2, Props::Crate);

    // Register mission as a faction anchor (contested/neutral)
    map.faction_anchors.push((
        GridVec::new(mx + mw / 2, my + mh / 2),
        Faction::Civilians,
        "Mission".to_string(),
    ));
}

/// Places a town plaza — an open killzone at the heart of the map.
/// The plaza is flanked by buildings with window-facing tiles overlooking it.
fn place_town_plaza(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) {
    let pw: CoordinateUnit = 16;
    let ph: CoordinateUnit = 12;
    if width < pw + 20 || height < ph + 20 {
        return;
    }
    let p_seed = seed.wrapping_add(777888);
    // Place plaza slightly offset from exact center to avoid overlapping the mission
    let cx = width / 2 + 15 + (value_noise(5, 5, p_seed) * 10.0) as CoordinateUnit;
    let cy = height / 2 + 10 + (value_noise(6, 6, p_seed) * 10.0) as CoordinateUnit;
    let px = (cx - pw / 2).clamp(2, width - pw - 2);
    let py = (cy - ph / 2).clamp(2, height - ph - 2);

    // Clear the plaza area — open exposed killzone
    for y in py..py + ph {
        for x in px..px + pw {
            let pos = GridVec::new(x, y);
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                voxel.floor = Some(Floor::Plaza);
                voxel.props = None;
            }
        }
    }

    // Place a central fountain/monument (rock as focal point)
    let center = GridVec::new(px + pw / 2, py + ph / 2);
    if let Some(voxel) = map.get_voxel_at_mut(&center) {
        voxel.props = Some(Props::Rock);
    }
    // Benches around the monument
    set_prop(map, center.x - 2, center.y, Props::Bench);
    set_prop(map, center.x + 2, center.y, Props::Bench);
    set_prop(map, center.x, center.y - 2, Props::Bench);
    set_prop(map, center.x, center.y + 2, Props::Bench);
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
                // Skip road tiles — no decorations on dirt roads or sidewalks.
                if matches!(voxel.floor, Some(Floor::Dirt) | Some(Floor::Sidewalk)) {
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

/// Places a small cemetery on the outskirts of the town.
fn place_cemetery(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) {
    let cw: CoordinateUnit = 14;
    let ch: CoordinateUnit = 10;
    if width < cw + 20 || height < ch + 20 {
        return;
    }
    let c_seed = seed.wrapping_add(999111);
    // Place in the lower-right area away from center
    let cx = width * 3 / 4 + (value_noise(7, 7, c_seed) * 10.0) as CoordinateUnit;
    let cy = height * 3 / 4 + (value_noise(8, 8, c_seed) * 10.0) as CoordinateUnit;
    let px = (cx - cw / 2).clamp(2, width - cw - 2);
    let py = (cy - ch / 2).clamp(2, height - ch - 2);

    // Fence perimeter with gate
    for y in py..py + ch {
        for x in px..px + cw {
            let pos = GridVec::new(x, y);
            let is_border = x == px || x == px + cw - 1 || y == py || y == py + ch - 1;
            let is_gate = y == py + ch - 1 && x == px + cw / 2;
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                if is_border && !is_gate {
                    voxel.props = Some(Props::Fence);
                    voxel.floor = Some(Floor::Grass);
                } else {
                    voxel.props = None;
                    voxel.floor = Some(Floor::Grass);
                }
            }
        }
    }

    // Gravestones (rocks in rows)
    for row in (2..ch - 2).step_by(2) {
        for col in (2..cw - 2).step_by(3) {
            set_prop(map, px + col, py + row, Props::Rock);
        }
    }
}

/// Places a livestock corral with fenced area and hay bales.
fn place_corral(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) {
    let cw: CoordinateUnit = 12;
    let ch: CoordinateUnit = 8;
    if width < cw + 20 || height < ch + 20 {
        return;
    }
    let c_seed = seed.wrapping_add(888222);
    let cx = width / 4 + (value_noise(9, 9, c_seed) * 10.0) as CoordinateUnit;
    let cy = height * 3 / 4 + (value_noise(10, 10, c_seed) * 10.0) as CoordinateUnit;
    let px = (cx - cw / 2).clamp(2, width - cw - 2);
    let py = (cy - ch / 2).clamp(2, height - ch - 2);

    // Fence perimeter with gate
    for y in py..py + ch {
        for x in px..px + cw {
            let pos = GridVec::new(x, y);
            let is_border = x == px || x == px + cw - 1 || y == py || y == py + ch - 1;
            let is_gate = y == py + ch - 1 && x == px + cw / 2;
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                if is_border && !is_gate {
                    voxel.props = Some(Props::Fence);
                    voxel.floor = Some(Floor::Dirt);
                } else {
                    voxel.props = None;
                    voxel.floor = Some(Floor::Dirt);
                }
            }
        }
    }

    // Hay bales and water troughs inside
    set_prop(map, px + 2, py + 2, Props::HayBale);
    set_prop(map, px + 5, py + 2, Props::HayBale);
    set_prop(map, px + 8, py + 2, Props::HayBale);
    set_prop(map, px + cw / 2, py + ch / 2, Props::WaterTrough);
    set_prop(map, px + 2, py + ch - 3, Props::HitchingPost);
    set_prop(map, px + cw - 3, py + ch - 3, Props::HitchingPost);
}

/// Places a town well — a circular stone structure with water.
fn place_town_well(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) {
    if width < 60 || height < 60 { return; }
    let w_seed = seed.wrapping_add(444555);
    // Place near the town center, offset from plaza
    let cx = width / 2 - 10 + (value_noise(4, 4, w_seed) * 8.0) as CoordinateUnit;
    let cy = height / 2 - 10 + (value_noise(5, 5, w_seed) * 8.0) as CoordinateUnit;
    let wx = cx.clamp(4, width - 4);
    let wy = cy.clamp(4, height - 4);

    // 3×3 well structure: stone ring with water center
    for dy in -1..=1i32 {
        for dx in -1..=1i32 {
            let pos = GridVec::new(wx + dx, wy + dy);
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                if dx == 0 && dy == 0 {
                    voxel.floor = Some(Floor::ShallowWater);
                    voxel.props = None;
                } else {
                    voxel.props = Some(Props::Well);
                    voxel.floor = Some(Floor::Sidewalk);
                }
            }
        }
    }
}

/// Places a gallows structure — wooden platform with posts.
fn place_gallows(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) {
    if width < 80 || height < 60 { return; }
    let g_seed = seed.wrapping_add(555777);
    // Place near the town center, slightly offset
    let cx = width / 2 + 20 + (value_noise(3, 3, g_seed) * 10.0) as CoordinateUnit;
    let cy = height / 2 - 15 + (value_noise(4, 4, g_seed) * 10.0) as CoordinateUnit;
    let gx = cx.clamp(4, width - 8);
    let gy = cy.clamp(4, height - 8);

    // 4×4 gallows platform
    for dy in 0..4i32 {
        for dx in 0..4i32 {
            let pos = GridVec::new(gx + dx, gy + dy);
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                voxel.floor = Some(Floor::WoodPlanks);
                voxel.props = None;
            }
        }
    }
    // Gallows posts at corners and crossbeam
    set_prop(map, gx, gy, Props::Gallows);
    set_prop(map, gx, gy + 3, Props::Gallows);
    set_prop(map, gx + 1, gy, Props::Gallows);
}

/// Places a tall water tower near the edge of town.
fn place_water_tower(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) {
    if width < 80 || height < 60 { return; }
    let wt_seed = seed.wrapping_add(666888);
    let cx = width / 4 + (value_noise(6, 6, wt_seed) * 20.0) as CoordinateUnit;
    let cy = height / 4 + (value_noise(7, 7, wt_seed) * 10.0) as CoordinateUnit;
    let tx = cx.clamp(4, width - 8);
    let ty = cy.clamp(4, height - 8);

    // 5×5 water tower: 4 leg posts, tank on top (represented as tower height)
    for dy in 0..5i32 {
        for dx in 0..5i32 {
            let pos = GridVec::new(tx + dx, ty + dy);
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                voxel.floor = Some(Floor::Dirt);
                voxel.props = None;
            }
            map.building_heights.insert(pos, HeightTier::Tower);
        }
    }
    // Four corner legs
    set_prop(map, tx, ty, Props::WaterTower);
    set_prop(map, tx + 4, ty, Props::WaterTower);
    set_prop(map, tx, ty + 4, Props::WaterTower);
    set_prop(map, tx + 4, ty + 4, Props::WaterTower);
}

/// Places railroad tracks along the southern edge of the town.
fn place_railroad(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) {
    if width < 80 || height < 80 { return; }
    let rail_seed = seed.wrapping_add(777999);
    // Railroad runs horizontally near the southern third of the map
    let rail_y = height * 3 / 4 + 15 + (value_noise(8, 8, rail_seed) * 6.0) as CoordinateUnit;
    let rail_y = rail_y.clamp(40, height - 10);

    for x in 4..width - 4 {
        // Slight wobble for realism
        let wobble = (value_noise(x, rail_y, rail_seed) * 1.5) as CoordinateUnit;
        let y = rail_y + wobble;
        if y <= 0 || y >= height - 1 { continue; }
        let pos = GridVec::new(x, y);
        if let Some(voxel) = map.get_voxel_at(&pos) {
            // Don't place tracks over water or buildings
            if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater)) { continue; }
            if matches!(voxel.props, Some(Props::Wall)) { continue; }
        }
        if let Some(voxel) = map.get_voxel_at_mut(&pos) {
            voxel.floor = Some(Floor::Gravel);
            voxel.props = Some(Props::RailTrack);
        }
        // Gravel bed on either side of the track
        for &dy in &[-1i32, 1] {
            let side_pos = GridVec::new(x, y + dy);
            if let Some(voxel) = map.get_voxel_at_mut(&side_pos)
                && voxel.props.is_none() && !matches!(voxel.floor, Some(Floor::WoodPlanks) | Some(Floor::ShallowWater) | Some(Floor::DeepWater)) {
                    voxel.floor = Some(Floor::Gravel);
                }
        }
    }
}

/// Places a windmill on the outskirts of town.
fn place_windmill(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) {
    if width < 80 || height < 60 { return; }
    let wm_seed = seed.wrapping_add(888111);
    let cx = width * 3 / 4 + (value_noise(9, 9, wm_seed) * 15.0) as CoordinateUnit;
    let cy = height / 4 + (value_noise(10, 10, wm_seed) * 10.0) as CoordinateUnit;
    let mx = cx.clamp(4, width - 8);
    let my = cy.clamp(4, height - 8);

    // 5×5 windmill base with stone walls
    for dy in 0..5i32 {
        for dx in 0..5i32 {
            let pos = GridVec::new(mx + dx, my + dy);
            let is_border = dx == 0 || dx == 4 || dy == 0 || dy == 4;
            let is_door = dy == 4 && dx == 2;
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                if is_border && !is_door {
                    voxel.props = Some(Props::Wall);
                    voxel.floor = Some(Floor::WoodPlanks);
                    map.wall_materials.insert(pos, WallMaterial::Stone);
                } else {
                    voxel.props = None;
                    voxel.floor = Some(Floor::WoodPlanks);
                }
            }
            map.building_heights.insert(pos, HeightTier::Tower);
        }
    }
    // Windmill blades (decorative props extending from tower)
    set_prop(map, mx + 2, my - 1, Props::Windmill);
    set_prop(map, mx - 1, my + 2, Props::Windmill);
    set_prop(map, mx + 5, my + 2, Props::Windmill);
    // Interior: grain storage
    set_prop(map, mx + 1, my + 1, Props::Barrel);
    set_prop(map, mx + 3, my + 1, Props::Barrel);
    set_prop(map, mx + 2, my + 2, Props::Crate);
}

/// Places lamp posts along cross streets at regular intervals.
fn place_lamp_posts(map: &mut GameMap, height: CoordinateUnit, street_x: CoordinateUnit, seed: NoiseSeed) {
    let lamp_seed = seed.wrapping_add(111333);
    for y in (40..height - 40).step_by(12) {
        let noise = value_noise(street_x, y, lamp_seed);
        if noise > 0.4 { continue; } // skip some positions for variety
        // Place lamp posts on both sides of the street
        for &dx in &[-3i32, 3] {
            let x = street_x + dx;
            let pos = GridVec::new(x, y);
            if let Some(voxel) = map.get_voxel_at(&pos)
                && voxel.props.is_none()
                    && matches!(voxel.floor, Some(Floor::Sidewalk) | Some(Floor::Dirt) | Some(Floor::Sand))
                {
                    set_prop(map, x, y, Props::LampPost);
                }
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
    fn game_map_border_is_passable_for_escape() {
        let map = GameMap::new(20, 15, 42);
        // Borders should be passable (no wall props) so player can escape
        let mut any_border_passable = false;
        for x in 0..20 {
            if map.is_passable(&GridVec::new(x, 0)) { any_border_passable = true; }
            if map.is_passable(&GridVec::new(x, 14)) { any_border_passable = true; }
        }
        for y in 0..15 {
            if map.is_passable(&GridVec::new(0, y)) { any_border_passable = true; }
            if map.is_passable(&GridVec::new(19, y)) { any_border_passable = true; }
        }
        assert!(any_border_passable, "At least some border tiles should be passable for escape");
    }

    #[test]
    fn game_map_border_tiles_exist() {
        let map = GameMap::new(20, 15, 42);
        // Border tiles should have floor data (they exist on the map)
        assert!(map.get_voxel_at(&GridVec::new(0, 0)).is_some());
        assert!(map.get_voxel_at(&GridVec::new(19, 0)).is_some());
        assert!(map.get_voxel_at(&GridVec::new(0, 14)).is_some());
        assert!(map.get_voxel_at(&GridVec::new(19, 14)).is_some());
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
        let map = GameMap::new(200, 140, 42);
        let mut has_bench = false;
        let mut has_barrel = false;
        let mut has_cactus = false;
        for y in 0..140 {
            for x in 0..200 {
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

    #[test]
    fn large_map_has_wall_materials() {
        let map = GameMap::new(200, 140, 42);
        assert!(
            !map.wall_materials.is_empty(),
            "Map should track wall materials for building walls"
        );
        // Should have at least two different material types
        let has_adobe = map.wall_materials.values().any(|m| *m == WallMaterial::Adobe);
        let has_timber = map.wall_materials.values().any(|m| *m == WallMaterial::Timber);
        assert!(
            has_adobe || has_timber,
            "Map should have diverse wall materials"
        );
    }

    #[test]
    fn large_map_has_breach_points() {
        let map = GameMap::new(400, 280, 42);
        assert!(
            !map.breach_points.is_empty(),
            "Map should have breach points between adjacent buildings"
        );
    }

    #[test]
    fn large_map_has_building_heights() {
        let map = GameMap::new(200, 140, 42);
        assert!(
            !map.building_heights.is_empty(),
            "Map should track building height tiers"
        );
        let has_single = map.building_heights.values().any(|h| *h == HeightTier::SingleStory);
        let has_double = map.building_heights.values().any(|h| *h == HeightTier::DoubleStory);
        assert!(
            has_single || has_double,
            "Map should have diverse building heights"
        );
    }

    #[test]
    fn large_map_has_faction_anchors() {
        let map = GameMap::new(400, 280, 42);
        assert!(
            !map.faction_anchors.is_empty(),
            "Map should assign faction anchor buildings"
        );
    }

    #[test]
    fn large_map_has_alleys() {
        let map = GameMap::new(400, 280, 42);
        let mut alley_count = 0;
        for y in 0..280 {
            for x in 0..400 {
                if matches!(map.voxels[y][x].floor, Some(Floor::Alley)) {
                    alley_count += 1;
                }
            }
        }
        assert!(alley_count > 0, "Map should have alley tiles between buildings");
    }

    #[test]
    fn large_map_has_plaza() {
        let map = GameMap::new(400, 280, 42);
        let mut plaza_count = 0;
        for y in 0..280 {
            for x in 0..400 {
                if matches!(map.voxels[y][x].floor, Some(Floor::Plaza)) {
                    plaza_count += 1;
                }
            }
        }
        assert!(plaza_count > 0, "Map should have plaza tiles");
    }

    #[test]
    fn large_map_has_rooftop_tiles() {
        let map = GameMap::new(400, 280, 42);
        let mut rooftop_count = 0;
        for y in 0..280 {
            for x in 0..400 {
                if matches!(map.voxels[y][x].floor, Some(Floor::Rooftop)) {
                    rooftop_count += 1;
                }
            }
        }
        assert!(rooftop_count > 0, "Map should have rooftop tiles from bell tower");
    }

    #[test]
    fn breach_points_are_wall_tiles() {
        let map = GameMap::new(400, 280, 42);
        for pos in &map.breach_points {
            if let Some(voxel) = map.get_voxel_at(pos) {
                assert!(
                    matches!(voxel.props, Some(Props::Wall)),
                    "Breach point at ({}, {}) should be a wall tile",
                    pos.x, pos.y
                );
            }
        }
    }

    #[test]
    fn building_material_assignment_is_deterministic() {
        // Same kind + noise should always produce the same material
        assert_eq!(building_material(6, 0.5), WallMaterial::Stone);  // Church
        assert_eq!(building_material(7, 0.5), WallMaterial::Stone);  // Bank
        assert_eq!(building_material(0, 0.2), WallMaterial::Timber); // House (low noise)
        assert_eq!(building_material(0, 0.6), WallMaterial::Adobe);  // House (high noise)
        assert_eq!(building_material(1, 0.5), WallMaterial::Adobe);  // Saloon
    }

    #[test]
    fn building_height_assignment_is_deterministic() {
        assert_eq!(building_height(6, 0.5), HeightTier::Tower);        // Church
        assert_eq!(building_height(7, 0.5), HeightTier::DoubleStory);  // Bank
        assert_eq!(building_height(8, 0.5), HeightTier::DoubleStory);  // Hotel
        assert_eq!(building_height(0, 0.5), HeightTier::SingleStory);  // House
        assert_eq!(building_height(2, 0.5), HeightTier::SingleStory);  // Stable
    }

    #[test]
    fn shared_wall_detection_adjacent_buildings() {
        // Two buildings touching side-by-side
        let buildings = vec![
            Building { x: 10, y: 10, w: 8, h: 8, kind: 0, material: WallMaterial::Adobe, height_tier: HeightTier::SingleStory },
            Building { x: 18, y: 10, w: 8, h: 8, kind: 0, material: WallMaterial::Adobe, height_tier: HeightTier::SingleStory },
        ];
        let shared = detect_shared_walls(&buildings);
        // The right wall of first building and left wall of second should be shared
        assert!(!shared.is_empty(), "Adjacent buildings should have shared walls");
        // Check specific wall tiles
        assert!(shared.contains(&GridVec::new(17, 12)), "Right wall of first building should be shared");
        assert!(shared.contains(&GridVec::new(18, 12)), "Left wall of second building should be shared");
    }

    #[test]
    fn shared_wall_detection_gap_buildings() {
        // Two buildings with a 1-tile gap
        let buildings = vec![
            Building { x: 10, y: 10, w: 8, h: 8, kind: 0, material: WallMaterial::Timber, height_tier: HeightTier::SingleStory },
            Building { x: 19, y: 10, w: 8, h: 8, kind: 0, material: WallMaterial::Timber, height_tier: HeightTier::SingleStory },
        ];
        let shared = detect_shared_walls(&buildings);
        assert!(!shared.is_empty(), "Buildings with 1-tile gap should still detect shared walls");
    }

    #[test]
    fn shared_wall_detection_far_buildings() {
        // Two buildings far apart — no shared walls
        let buildings = vec![
            Building { x: 10, y: 10, w: 8, h: 8, kind: 0, material: WallMaterial::Adobe, height_tier: HeightTier::SingleStory },
            Building { x: 50, y: 10, w: 8, h: 8, kind: 0, material: WallMaterial::Adobe, height_tier: HeightTier::SingleStory },
        ];
        let shared = detect_shared_walls(&buildings);
        assert!(shared.is_empty(), "Distant buildings should not have shared walls");
    }

    #[test]
    fn props_is_wall_helper() {
        assert!(Props::Wall.is_wall());
        assert!(!Props::Table.is_wall());
        assert!(!Props::Barrel.is_wall());
    }

    #[test]
    fn wall_materials_and_breach_points_deterministic() {
        let map1 = GameMap::new(200, 140, 42);
        let map2 = GameMap::new(200, 140, 42);
        assert_eq!(map1.wall_materials.len(), map2.wall_materials.len(),
            "Wall material count should be deterministic");
        assert_eq!(map1.breach_points.len(), map2.breach_points.len(),
            "Breach point count should be deterministic");
        assert_eq!(map1.building_heights.len(), map2.building_heights.len(),
            "Building heights count should be deterministic");
        assert_eq!(map1.faction_anchors.len(), map2.faction_anchors.len(),
            "Faction anchor count should be deterministic");
    }
}
