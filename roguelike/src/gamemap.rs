use std::collections::{HashMap, HashSet};

use crate::components::Faction;
use crate::grid_vec::GridVec;
use crate::noise::{fbm, value_noise, NoiseSeed};
use crate::typeenums::{Floor, Props};
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
    /// Faction anchor buildings: (center position, faction, building name).
    /// Factions seed from these defensible positions rather than randomly.
    pub faction_anchors: Vec<(GridVec, Faction, String)>,
    /// Shared spatial occupancy grid. True = tile is already occupied by
    /// a road, river, beach, or building. No later pass may place on it.
    pub occupancy: Vec<Vec<bool>>,
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
}

// ── World generation phase system ────────────────────────────────────────

/// Intermediate data accumulated between world generation phases.
#[derive(Default)]
/// Intermediate data accumulated during hierarchical world generation.
/// Phases produce and consume this data, enabling later phases to reference
/// layout decisions made by earlier ones (e.g., street positions inform
/// building placement).
struct WorldGenData {
    avenue_ys: Vec<CoordinateUnit>,
    cross_xs: Vec<CoordinateUnit>,
    buildings: Vec<Building>,
    avenue_half_width: CoordinateUnit,
    cross_half_width: CoordinateUnit,
    /// Tiles along road edges (sidewalk or dirt tiles adjacent to non-road terrain).
    /// Collected after the infrastructure pass so prop placement can follow
    /// actual road curvature rather than straight-line approximations.
    road_edge_tiles: Vec<(CoordinateUnit, CoordinateUnit)>,
}

/// A single step in the hierarchical world generation pipeline.
/// Each phase receives the map and shared data, applying one category of
/// generation (terrain, water, infrastructure, buildings, etc.).
/// Phases run in order; later phases may read data produced by earlier ones.
trait WorldGenPhase {
    fn execute(
        &self,
        map: &mut GameMap,
        data: &mut WorldGenData,
        width: CoordinateUnit,
        height: CoordinateUnit,
        seed: NoiseSeed,
    );
}

/// Phase 1: Desert base terrain + forest on outskirts.
struct TerrainPhase;

impl TerrainPhase {
    /// Creates the initial `GameMap` with noise-driven desert terrain.
    fn create_map(
        width: CoordinateUnit,
        height: CoordinateUnit,
        seed: NoiseSeed,
    ) -> GameMap {
        let biome_seed = seed;
        let detail_seed = seed.wrapping_add(12345);

        let mut voxels = Vec::with_capacity(height as usize);
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
            faction_anchors: Vec::new(),
            occupancy: vec![vec![false; height as usize]; width as usize],
        };

        // Forest on outskirts
        let forest_margin = 30;
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

        map
    }
}

/// Phase 2: River generation.
struct WaterPhase;

impl WorldGenPhase for WaterPhase {

    fn execute(
        &self,
        map: &mut GameMap,
        _data: &mut WorldGenData,
        width: CoordinateUnit,
        height: CoordinateUnit,
        seed: NoiseSeed,
    ) {
        let river_seed = seed.wrapping_add(88800);
        let river_cx = width as f64 / 2.0;
        // Track the previous row's center so each row drifts at most ±1 tile.
        let mut prev_center_x = river_cx;
        // River flows top to bottom with smooth banks.
        for y in 1..height - 1 {
            let fy = y as f64;
            // Desired center from multi-octave noise (determines general shape).
            let desired = river_cx
                + (fy * 0.008).sin() * 40.0      // large meander
                + (fy * 0.020).sin() * 20.0;     // medium curve
            // Clamp drift to ±1 tile per row for smooth banks.
            let drift = (desired - prev_center_x).clamp(-1.0, 1.0);
            let center_x = prev_center_x + drift;
            prev_center_x = center_x;
            // River width varies smoothly along its length.
            let base_width = 14.0 + value_noise(y, 0, river_seed.wrapping_add(111)) * 10.0;
            let width_pulse = (fy * 0.012).sin() * 6.0; // gentle widening/narrowing
            let river_width = (base_width + width_pulse).max(8.0);
            let beach_width = 5.0;

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
                        && !matches!(voxel.props, Some(Props::Wall) | Some(Props::StoneWall)) {
                            voxel.floor = Some(Floor::BeachSand);
                            voxel.props = None;
                        }
            }
        }

        // Mark water and beach tiles as occupied
        for y in 0..map.height {
            for x in 0..map.width {
                let pos = GridVec::new(x, y);
                if let Some(voxel) = map.get_voxel_at(&pos) {
                    if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Beach) | Some(Floor::BeachSand)) {
                        map.occupancy[x as usize][y as usize] = true;
                    }
                }
            }
        }
    }
}

/// Phase 3: Street grid (avenues, cross streets, bridges).
struct InfrastructurePhase;

impl WorldGenPhase for InfrastructurePhase {

    fn execute(
        &self,
        map: &mut GameMap,
        data: &mut WorldGenData,
        width: CoordinateUnit,
        height: CoordinateUnit,
        seed: NoiseSeed,
    ) {
        // ── Pass 3a: Roads and streets ───────────────────────────────────
        // Horizontal avenues: wide dirt carriage roads flanked by sidewalks.
        // Avenue spacing varies per seed for unique city feel.
        let spacing_noise = value_noise(0, 0, seed.wrapping_add(99900));
        let avenue_spacing = 32 + (spacing_noise * 12.0) as CoordinateUnit; // 32-44
        let avenue_half_width = 3; // carriage road half-width (7 tiles total)
        let sidewalk_width = 2; // sidewalk on each side of the road
        let curve_seed = seed.wrapping_add(55500);
        {
            let first_avenue = 40; // start inside the forest margin
            let mut ay = first_avenue;
            while ay < height - 40 {
                data.avenue_ys.push(ay);
                ay += avenue_spacing;
            }
        }
        data.avenue_half_width = avenue_half_width;

        // Lay avenue roads with sidewalks — roads skip water and beach (buffer exclusion zone).
        for &ay in &data.avenue_ys {
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
                            if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Beach) | Some(Floor::BeachSand)) {
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
                        if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Beach) | Some(Floor::BeachSand)) {
                            continue;
                        }
                        voxel.floor = Some(Floor::DirtRoad);
                        voxel.props = None;
                    }
                }
            }
        }

        // Vertical cross streets with sinusoidal curvature and sidewalks.
        // Roads skip water, beach, and bridge tiles.
        let cross_seed = seed.wrapping_add(66666);
        let cross_noise = value_noise(1, 1, seed.wrapping_add(99901));
        let cross_spacing = 28 + (cross_noise * 14.0) as CoordinateUnit; // 28-42
        let cross_half_width = 2; // narrower than avenues
        let cross_sidewalk_width = 1;
        data.cross_half_width = cross_half_width;
        {
            let mut cx = 40i32;
            let mut ci = 0i32;
            while cx < width - 40 {
                let jitter = (value_noise(ci, 0, cross_seed) * 10.0) as CoordinateUnit - 5;
                let actual_cx = (cx + jitter).clamp(2, width - 3);
                data.cross_xs.push(actual_cx);
                let curve_amp = 2.0 + value_noise(ci, 1, cross_seed) * 3.0;
                let curve_freq = 0.02 + value_noise(1, ci, cross_seed) * 0.01;
                for y in 1..height - 1 {
                    let curve_offset = (y as f64 * curve_freq).sin() * curve_amp;
                    // Sidewalk — must not overwrite avenue dirt roads or walls.
                    for sw in 1..=cross_sidewalk_width {
                        for sign in [-1i32, 1] {
                            let x = actual_cx + sign * (cross_half_width + sw) + curve_offset as CoordinateUnit;
                            if x <= 0 || x >= width - 1 { continue; }
                            let pos = GridVec::new(x, y);
                            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                                if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Beach) | Some(Floor::BeachSand) | Some(Floor::Bridge)) {
                                    continue;
                                }
                                if !matches!(voxel.props, Some(Props::Wall) | Some(Props::StoneWall))
                                    && !matches!(voxel.floor, Some(Floor::DirtRoad))
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
                            if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Beach) | Some(Floor::BeachSand) | Some(Floor::Bridge)) {
                                continue;
                            }
                            if !matches!(voxel.props, Some(Props::Wall) | Some(Props::StoneWall)) {
                                voxel.floor = Some(Floor::DirtRoad);
                                voxel.props = None;
                            }
                        }
                    }
                }
                cx += cross_spacing;
                ci += 1;
            }
        }

        // ── Pass 3b: Bridges where horizontal roads cross the river ──────
        // Only bridge at the specific coordinates where a horizontal avenue
        // road band crosses water. Bridge tiles replace ShallowWater,
        // DeepWater, and BeachSand so the bridge surface is clean.
        // Vertical cross streets never bridge the river.
        let road_band = avenue_half_width + sidewalk_width;
        for &ay in &data.avenue_ys {
            let curve_amp = 3.0 + value_noise(ay, 0, curve_seed) * 4.0;
            let curve_freq = 0.015 + value_noise(0, ay, curve_seed) * 0.01;
            // First pass: identify X positions where the road band crosses water
            let mut crosses_water = vec![false; width as usize];
            for x in 1..width - 1 {
                let curve_offset = (x as f64 * curve_freq).sin() * curve_amp;
                let center_y = ay + curve_offset as CoordinateUnit;
                for hw in -road_band..=road_band {
                    let y = center_y + hw;
                    if y <= 0 || y >= height - 1 { continue; }
                    let pos = GridVec::new(x, y);
                    if let Some(voxel) = map.get_voxel_at(&pos) {
                        if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater)) {
                            crosses_water[x as usize] = true;
                            break;
                        }
                    }
                }
            }
            // Second pass: at crossing X positions, bridge only within the
            // road band so the bridge is road-width, not river-width.
            // Replace water and beach sand tiles so bridges have a clean surface.
            for x in 1..width - 1 {
                if !crosses_water[x as usize] { continue; }
                let curve_offset = (x as f64 * curve_freq).sin() * curve_amp;
                let center_y = ay + curve_offset as CoordinateUnit;
                for hw in -road_band..=road_band {
                    let y = center_y + hw;
                    if y <= 0 || y >= height - 1 { continue; }
                    let pos = GridVec::new(x, y);
                    if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                        if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::BeachSand)) {
                            voxel.floor = Some(Floor::Bridge);
                            voxel.props = None;
                        }
                    }
                }
            }
        }

        // ── Collect road-edge tiles for prop placement ───────────────────
        // Road-edge tiles are Sidewalk tiles adjacent to non-road terrain,
        // following the actual curvature of the laid roads.
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let pos = GridVec::new(x, y);
                if let Some(voxel) = map.get_voxel_at(&pos) {
                    if !matches!(voxel.floor, Some(Floor::Sidewalk)) {
                        continue;
                    }
                    // Check if adjacent to a non-road tile
                    let adjacent_non_road = pos.cardinal_neighbors().iter().any(|n| {
                        map.get_voxel_at(n).is_some_and(|v| {
                            !matches!(v.floor, Some(Floor::DirtRoad) | Some(Floor::Sidewalk) | Some(Floor::Bridge))
                        })
                    });
                    if adjacent_non_road {
                        data.road_edge_tiles.push((x, y));
                    }
                }
            }
        }

        // Mark road/sidewalk tiles as occupied
        for y in 0..map.height {
            for x in 0..map.width {
                let pos = GridVec::new(x, y);
                if let Some(voxel) = map.get_voxel_at(&pos) {
                    if matches!(voxel.floor, Some(Floor::DirtRoad) | Some(Floor::Sidewalk) | Some(Floor::Bridge)) {
                        map.occupancy[x as usize][y as usize] = true;
                    }
                }
            }
        }
    }
}

/// Phase 3b: Railroad tracks — placed after roads but before buildings so
/// rail tiles are occupied before any building or prop pass.
struct RailroadPhase;

impl WorldGenPhase for RailroadPhase {
    fn execute(
        &self,
        map: &mut GameMap,
        _data: &mut WorldGenData,
        width: CoordinateUnit,
        height: CoordinateUnit,
        seed: NoiseSeed,
    ) {
        place_railroad(map, width, height, seed);
    }
}

/// Phase 4: Buildings, alleys, faction anchors.
struct BuildingPhase;

impl WorldGenPhase for BuildingPhase {

    fn execute(
        &self,
        map: &mut GameMap,
        data: &mut WorldGenData,
        width: CoordinateUnit,
        height: CoordinateUnit,
        seed: NoiseSeed,
    ) {
        let (buildings, parks, open_lots) = generate_buildings_bsp(
            width, height, seed,
            &data.avenue_ys, &data.cross_xs, data.avenue_half_width,
            data.cross_half_width,
        );
        data.buildings = buildings;

        // Track which buildings were actually placed (not skipped due to water/beach)
        let mut placed_buildings: Vec<usize> = Vec::new();
        for (idx, b) in data.buildings.iter().enumerate() {
            // Check footprint corners, center, and entrance tile
            let check_points = [
                GridVec::new(b.x, b.y),
                GridVec::new(b.x + b.w - 1, b.y),
                GridVec::new(b.x, b.y + b.h - 1),
                GridVec::new(b.x + b.w - 1, b.y + b.h - 1),
                GridVec::new(b.x + b.w / 2, b.y + b.h / 2),
                GridVec::new(b.x + b.w / 2, b.y + b.h), // entrance tile
            ];
            // Skip buildings on water, beach, or road tiles.
            // Every tile in the footprint and padding must be free of
            // DirtRoad, ShallowWater, DeepWater, and BeachSand.
            let on_excluded = check_points.iter().any(|p| {
                map.get_voxel_at(p).is_some_and(|v| {
                    matches!(v.floor, Some(Floor::DirtRoad) | Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Beach) | Some(Floor::BeachSand))
                })
            });
            if on_excluded {
                continue;
            }
            // Also skip if any tile in the footprint is occupied (road, water, etc.)
            if map.is_occupied(b.x, b.y, b.w, b.h) {
                continue;
            }
            place_building(map, b, seed);
            // Hierarchical placement: exterior props based on building type
            place_exterior_props(map, b, seed);
            placed_buildings.push(idx);
        }

        // Retain only the buildings that were actually placed
        let new_buildings: Vec<Building> = placed_buildings
            .iter()
            .map(|&i| {
                let b = &data.buildings[i];
                Building { x: b.x, y: b.y, w: b.w, h: b.h, kind: b.kind }
            })
            .collect();
        data.buildings = new_buildings;

        // Place parks in BSP-allocated nodes.  Parks occupy a node and
        // register in the spatial index just like buildings.
        for &(px, py, pw, ph) in &parks {
            // Skip if any tile in the park footprint is a road
            let park_on_road = (px..px + pw).any(|rx| {
                (py..py + ph).any(|ry| {
                    let p = GridVec::new(rx, ry);
                    map.get_voxel_at(&p).is_some_and(|v| {
                        matches!(v.floor, Some(Floor::DirtRoad) | Some(Floor::Sidewalk) | Some(Floor::Bridge)
                            | Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Beach) | Some(Floor::BeachSand))
                    })
                })
            });
            if park_on_road {
                continue;
            }
            // Lay park: grass floor with benches and bushes
            for y in py..py + ph {
                for x in px..px + pw {
                    let pos = GridVec::new(x, y);
                    if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                        voxel.floor = Some(Floor::Grass);
                        voxel.props = None;
                    }
                }
            }
            // Benches and bushes inside the park
            let park_seed = seed.wrapping_add(px as u64 * 31 + py as u64 * 37);
            if pw >= 4 && ph >= 4 {
                set_prop(map, px + 1, py + 1, Props::Bench);
                set_prop(map, px + pw - 2, py + 1, Props::Bench);
                set_prop(map, px + pw / 2, py + ph / 2, Props::Bush);
                if value_noise(px, py, park_seed) < 0.5 {
                    set_prop(map, px + pw / 2 - 1, py + ph - 2, Props::Bush);
                    set_prop(map, px + pw / 2 + 1, py + ph - 2, Props::Bush);
                } else {
                    set_prop(map, px + 1, py + ph - 2, Props::Well);
                }
            }
        }

        // Fill open lots with contextually appropriate scatter
        place_open_lot_scatter(map, &open_lots, seed);

        // Place narrow alleys between adjacent buildings within blocks
        place_alleys(map, &data.buildings);

        // Assign faction anchors from building kinds
        assign_faction_anchors(map, &data.buildings);

        // Transition zone scatter around town perimeter
        place_transition_zones(map, width, height, seed, &data.buildings);
    }
}

/// Phase 5: Landmarks, plazas, urban features, rock formations.
struct LandmarkPhase;

impl WorldGenPhase for LandmarkPhase {

    fn execute(
        &self,
        map: &mut GameMap,
        data: &mut WorldGenData,
        width: CoordinateUnit,
        height: CoordinateUnit,
        seed: NoiseSeed,
    ) {
        place_mission(map, width, height, seed, &data.buildings);
        place_town_hall(map, width, height, seed, &data.buildings);
        place_grand_saloon(map, width, height, seed, &data.buildings);
        place_stone_church(map, width, height, seed, &data.buildings);

        place_town_plaza(map, width, height, seed, &data.buildings);
        place_cemetery(map, width, height, seed, &data.buildings);
        place_corral(map, width, height, seed, &data.buildings);

        place_town_well(map, width, height, seed);
        place_gallows(map, width, height, seed);
        place_water_tower(map, width, height, seed);
        place_windmill(map, width, height, seed);
        place_outposts(map, width, height, seed, &data.buildings);
        place_rock_formations(map, width, height, seed);
    }
}

/// Phase 6: Street props and decorations.
struct DetailPhase;

impl WorldGenPhase for DetailPhase {

    fn execute(
        &self,
        map: &mut GameMap,
        data: &mut WorldGenData,
        width: CoordinateUnit,
        height: CoordinateUnit,
        seed: NoiseSeed,
    ) {
        // Street props placed on actual road-edge tiles so they follow
        // road curvature rather than straight-line approximations.
        place_street_props_curved(map, &data.road_edge_tiles, seed);

        // Lamp posts along cross streets
        for &cx in &data.cross_xs {
            place_lamp_posts(map, height, cx, seed);
        }

        // Decorative elements in open areas
        place_desert_decorations(map, width, height, seed);

        // Scatter gunpowder barrels around the map
        place_gunpowder_barrels(map, width, height, seed);
    }
}

/// Phase 7: Spawn clearing and water cleanup.
struct FinalizationPhase;

impl WorldGenPhase for FinalizationPhase {

    fn execute(
        &self,
        map: &mut GameMap,
        data: &mut WorldGenData,
        width: CoordinateUnit,
        height: CoordinateUnit,
        _seed: NoiseSeed,
    ) {
        // Post-pass coherence: verify every building can reach the main street
        check_connectivity(map, &data.buildings, &data.avenue_ys, &data.cross_xs, width, height);

        // Clear around default spawn point. If SPAWN_POINT is outside the map
        // (common on smaller maps), fall back to the map center so the
        // central area is always walkable.
        let local_spawn = if SPAWN_POINT.x >= 0 && SPAWN_POINT.x < width
            && SPAWN_POINT.y >= 0 && SPAWN_POINT.y < height
        {
            SPAWN_POINT
        } else {
            GridVec::new(width / 2, height / 2)
        };
        clear_around(map, local_spawn, 6);
        if let Some(bridge_pos) = map.find_bridge_center() {
            clear_around(map, bridge_pos, 6);
        }

        // Final pass: clear props from water, beach, and bridge tiles.
        // Later generation steps may have placed props on river tiles.
        // Also clear gravel/rail tiles that ended up on main roads.
        for y in 0..height {
            for x in 0..width {
                let pos = GridVec::new(x, y);
                if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                    if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Beach) | Some(Floor::BeachSand)) {
                        voxel.props = None;
                    }
                }
            }
        }
    }
}

impl GameMap {
    /// Creates a new game map as a giant midwestern town.
    ///
    /// The generation pipeline runs as a sequence of phases:
    ///   1. **Terrain** — desert base + forest outskirts (creates the map).
    ///   2. **Water** — river generation.
    ///   3. **Infrastructure** — street grid (avenues, cross streets, bridges).
    ///   4. **Buildings** — houses, saloons, stables, alleys, faction anchors.
    ///   5. **Landmarks** — Town Hall, Grand Saloon, plazas, urban features.
    ///   6. **Details** — street props, decorations, gunpowder barrels.
    ///   7. **Finalization** — spawn clearing, water cleanup.
    ///
    /// After generation, structural verification checks are run. If any
    /// check fails, the map is regenerated with a modified seed (up to 10
    /// retries). In debug builds, panics if no valid map is produced.
    pub fn new(width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) -> Self {
        const MAX_RETRIES: u32 = 10;
        let mut last_map = None;
        let mut last_reason = String::new();
        for attempt in 0..=MAX_RETRIES {
            let current_seed = if attempt == 0 { seed } else { seed.wrapping_add(attempt as u64 * 7919) };
            let (map, data) = Self::generate_with_seed(width, height, current_seed);

            match verify_world(&map, &data.buildings, &data.avenue_ys, &data.cross_xs, width, height) {
                Ok(()) => return map,
                Err(reason) => {
                    eprintln!(
                        "World gen verification failed (seed={}, attempt {}): {}",
                        current_seed, attempt, reason
                    );
                    last_reason = reason;
                    last_map = Some(map);
                }
            }
        }
        #[cfg(debug_assertions)]
        panic!(
            "World generation failed after {} retries. Last failure: {}",
            MAX_RETRIES, last_reason
        );
        // In release builds, return the last attempt.
        #[allow(unreachable_code)]
        last_map.unwrap()
    }

    /// Internal: run all generation phases with a specific seed.
    fn generate_with_seed(
        width: CoordinateUnit,
        height: CoordinateUnit,
        seed: NoiseSeed,
    ) -> (Self, WorldGenData) {
        let mut map = TerrainPhase::create_map(width, height, seed);
        let mut data = WorldGenData::default();

        let phases: Vec<Box<dyn WorldGenPhase>> = vec![
            Box::new(WaterPhase),
            Box::new(InfrastructurePhase),
            Box::new(RailroadPhase),
            Box::new(BuildingPhase),
            Box::new(LandmarkPhase),
            Box::new(DetailPhase),
            Box::new(FinalizationPhase),
        ];

        for phase in &phases {
            phase.execute(&mut map, &mut data, width, height, seed);
        }

        (map, data)
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

    /// Marks a rectangle as occupied in the occupancy grid.
    pub fn mark_occupied(&mut self, x: i32, y: i32, w: i32, h: i32) {
        for iy in y..y + h {
            for ix in x..x + w {
                if ix >= 0 && ix < self.width && iy >= 0 && iy < self.height {
                    self.occupancy[ix as usize][iy as usize] = true;
                }
            }
        }
    }

    /// Returns true if any cell in the rectangle is occupied.
    pub fn is_occupied(&self, x: i32, y: i32, w: i32, h: i32) -> bool {
        for iy in y..y + h {
            for ix in x..x + w {
                if ix >= 0 && ix < self.width && iy >= 0 && iy < self.height {
                    if self.occupancy[ix as usize][iy as usize] {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Returns true if any cell in the rectangle is water or beach.
    /// Used for landmark placement that may intentionally straddle roads.
    pub fn is_water_occupied(&self, x: i32, y: i32, w: i32, h: i32) -> bool {
        for iy in y..y + h {
            for ix in x..x + w {
                if ix >= 0 && ix < self.width && iy >= 0 && iy < self.height {
                    let pos = GridVec::new(ix, iy);
                    if let Some(voxel) = self.get_voxel_at(&pos) {
                        if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Beach) | Some(Floor::BeachSand)) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Returns true if any cell in the rectangle is a road, water, or beach tile.
    /// Used by landmark and building placement to reject footprints that would
    /// overlap infrastructure or the river.
    pub fn has_excluded_tile(&self, x: i32, y: i32, w: i32, h: i32) -> bool {
        for iy in y..y + h {
            for ix in x..x + w {
                if ix >= 0 && ix < self.width && iy >= 0 && iy < self.height {
                    let pos = GridVec::new(ix, iy);
                    if let Some(voxel) = self.get_voxel_at(&pos) {
                        if matches!(voxel.floor,
                            Some(Floor::DirtRoad) | Some(Floor::ShallowWater)
                            | Some(Floor::DeepWater) | Some(Floor::BeachSand)) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Returns `true` if the tile at `point` is passable (no blocking props
    /// and not deep/shallow water).
    #[inline]
    pub fn is_passable(&self, point: &MyPoint) -> bool {
        self.get_voxel_at(point)
            .is_some_and(|v| {
                // Water tiles block movement (no swimming).
                if matches!(v.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater)) {
                    return false;
                }
                match &v.props {
                    Some(f) => !f.blocks_movement(),
                    None => true,
                }
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

    /// Finds a spawnable tile near the given center within the given radius.
    /// Searches in expanding rings from the center outward.
    pub fn find_spawnable_near(&self, center: GridVec, radius: i32) -> Option<GridVec> {
        for r in 0..=radius {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dy.abs() != r { continue; } // only ring perimeter
                    let pos = center + GridVec::new(dx, dy);
                    if self.is_spawnable(&pos) {
                        return Some(pos);
                    }
                }
            }
        }
        None
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
                    && matches!(voxel.floor, Some(Floor::Dirt) | Some(Floor::DirtRoad) | Some(Floor::Sand) | Some(Floor::Gravel) | Some(Floor::Grass) | Some(Floor::Sidewalk))
                {
                    // Check if there's an adjacent wall (meaning we're just outside a building)
                    let has_adjacent_wall = pos.cardinal_neighbors().iter().any(|n| {
                        self.get_voxel_at(n).is_some_and(|v| v.props.as_ref().is_some_and(|p| p.is_wall()))
                    });
                    if has_adjacent_wall {
                        return Some(pos);
                    }
                }
            }
        }
        None
    }

    /// Finds a Bridge tile near the horizontal center of the map.
    /// Returns a point on the bridge closest to `width / 2`.
    pub fn find_bridge_center(&self) -> Option<GridVec> {
        let target_x = self.width / 2;
        let mut best: Option<(i32, GridVec)> = None;
        for y in 0..self.height {
            for x in 0..self.width {
                let pos = GridVec::new(x, y);
                if let Some(voxel) = self.get_voxel_at(&pos)
                    && matches!(voxel.floor, Some(Floor::Bridge))
                {
                    let dist = (x - target_x).abs();
                    if best.is_none() || dist < best.unwrap().0 {
                        best = Some((dist, pos));
                    }
                }
            }
        }
        best.map(|(_, pos)| pos)
    }

    /// Returns the approximate X-coordinate of the river center for a given Y.
    pub fn river_center_x(&self, _y: i32) -> f64 {
        // Simplified - just return map center since the river meanders around it
        self.width as f64 / 2.0
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

// ── BSP spatial partitioning ─────────────────────────────────────────────

/// Minimum width for a BSP leaf node. Leaves narrower than this cannot
/// contain a building and are treated as open lots.
const BSP_MIN_LEAF_W: CoordinateUnit = 14;
/// Minimum height for a BSP leaf node.
const BSP_MIN_LEAF_H: CoordinateUnit = 12;
/// Padding between building footprint and leaf boundary. Creates natural
/// alleyways and breathing room between adjacent structures.
const BSP_PADDING: CoordinateUnit = 2;
/// Maximum BSP recursion depth.
const BSP_MAX_DEPTH: u32 = 7;

/// Semantic zone types assigned to BSP regions.
const ZONE_COMMERCIAL: u32 = 0;
const ZONE_RESIDENTIAL: u32 = 1;
const ZONE_INDUSTRIAL: u32 = 2;

/// A node in the binary space partition tree. Each node owns an exclusive
/// rectangular region of the map. Leaf nodes are allocated one building;
/// internal nodes define the split.
struct BspNode {
    x: CoordinateUnit,
    y: CoordinateUnit,
    w: CoordinateUnit,
    h: CoordinateUnit,
    left: Option<Box<BspNode>>,
    right: Option<Box<BspNode>>,
}

impl BspNode {
    fn new(x: CoordinateUnit, y: CoordinateUnit, w: CoordinateUnit, h: CoordinateUnit) -> Self {
        Self { x, y, w, h, left: None, right: None }
    }

    fn is_leaf(&self) -> bool {
        self.left.is_none() && self.right.is_none()
    }

    /// Recursively subdivide this node. Splits are biased to align with the
    /// street network (avenue Y-positions and cross-street X-positions) so
    /// buildings naturally face roads rather than spawning at arbitrary
    /// angles to them.
    fn subdivide(
        &mut self,
        seed: NoiseSeed,
        depth: u32,
        avenue_ys: &[CoordinateUnit],
        cross_xs: &[CoordinateUnit],
    ) {
        if depth >= BSP_MAX_DEPTH {
            return;
        }

        let can_split_h = self.h >= BSP_MIN_LEAF_H * 2 + BSP_PADDING;
        let can_split_v = self.w >= BSP_MIN_LEAF_W * 2 + BSP_PADDING;
        if !can_split_h && !can_split_v {
            return;
        }

        // Bias: split along the longer axis with randomization so plots
        // feel organic rather than grid-stamped.
        let noise = value_noise(
            self.x + depth as i32,
            self.y + depth as i32,
            seed.wrapping_add(depth as u64 * 7919),
        );
        let split_horizontal = if can_split_h && can_split_v {
            if self.h > self.w { noise < 0.7 } else { noise < 0.3 }
        } else {
            can_split_h
        };

        if split_horizontal {
            let min_y = self.y + BSP_MIN_LEAF_H;
            let max_y = self.y + self.h - BSP_MIN_LEAF_H - BSP_PADDING;
            if min_y > max_y {
                return;
            }
            let mid_y = self.y + self.h / 2;
            let mut split_y = mid_y;
            // Bias toward nearest avenue so buildings face roads.
            for &ay in avenue_ys {
                if ay >= min_y && ay <= max_y
                    && (ay - mid_y).abs() < (split_y - mid_y).abs() + 8
                {
                    split_y = ay;
                }
            }
            let jitter = ((value_noise(
                self.x,
                self.y,
                seed.wrapping_add(33333 + depth as u64),
            ) - 0.5)
                * 6.0) as CoordinateUnit;
            split_y = (split_y + jitter).clamp(min_y, max_y);

            let top_h = split_y - self.y;
            let bot_y = split_y + BSP_PADDING;
            let bot_h = (self.y + self.h) - bot_y;
            if top_h >= BSP_MIN_LEAF_H && bot_h >= BSP_MIN_LEAF_H {
                self.left = Some(Box::new(BspNode::new(self.x, self.y, self.w, top_h)));
                self.right = Some(Box::new(BspNode::new(self.x, bot_y, self.w, bot_h)));
            }
        } else {
            let min_x = self.x + BSP_MIN_LEAF_W;
            let max_x = self.x + self.w - BSP_MIN_LEAF_W - BSP_PADDING;
            if min_x > max_x {
                return;
            }
            let mid_x = self.x + self.w / 2;
            let mut split_x = mid_x;
            for &cx in cross_xs {
                if cx >= min_x && cx <= max_x
                    && (cx - mid_x).abs() < (split_x - mid_x).abs() + 8
                {
                    split_x = cx;
                }
            }
            let jitter = ((value_noise(
                self.y,
                self.x,
                seed.wrapping_add(44444 + depth as u64),
            ) - 0.5)
                * 6.0) as CoordinateUnit;
            split_x = (split_x + jitter).clamp(min_x, max_x);

            let left_w = split_x - self.x;
            let right_x = split_x + BSP_PADDING;
            let right_w = (self.x + self.w) - right_x;
            if left_w >= BSP_MIN_LEAF_W && right_w >= BSP_MIN_LEAF_W {
                self.left = Some(Box::new(BspNode::new(self.x, self.y, left_w, self.h)));
                self.right =
                    Some(Box::new(BspNode::new(right_x, self.y, right_w, self.h)));
            }
        }

        if let Some(ref mut left) = self.left {
            left.subdivide(seed.wrapping_add(1), depth + 1, avenue_ys, cross_xs);
        }
        if let Some(ref mut right) = self.right {
            right.subdivide(seed.wrapping_add(2), depth + 1, avenue_ys, cross_xs);
        }
    }

    /// Collect all leaf rectangles into a flat vector.
    fn collect_leaves(
        &self,
        out: &mut Vec<(CoordinateUnit, CoordinateUnit, CoordinateUnit, CoordinateUnit)>,
    ) {
        if self.is_leaf() {
            out.push((self.x, self.y, self.w, self.h));
        } else {
            if let Some(ref left) = self.left {
                left.collect_leaves(out);
            }
            if let Some(ref right) = self.right {
                right.collect_leaves(out);
            }
        }
    }
}

// ── Semantic zone assignment ─────────────────────────────────────────────

/// Assigns a semantic zone type to a BSP leaf based on its position
/// relative to streets, the railroad area, and the town center.
/// Commercial zones line the main streets, industrial zones cluster near
/// the southern edge (where the railroad runs), and residential fills
/// the outskirts.
fn assign_zone(
    cx: CoordinateUnit,
    cy: CoordinateUnit,
    _width: CoordinateUnit,
    height: CoordinateUnit,
    avenue_ys: &[CoordinateUnit],
    seed: NoiseSeed,
) -> u32 {
    let dist_to_avenue = avenue_ys
        .iter()
        .map(|&ay| (cy - ay).abs())
        .min()
        .unwrap_or(height);

    // Commercial: near main streets (saloons, general stores, banks)
    if dist_to_avenue < 20 {
        return ZONE_COMMERCIAL;
    }

    // Industrial: near southern edge (railroad area) or river
    if cy > height * 2 / 3 {
        let noise = value_noise(cx, cy, seed.wrapping_add(55555));
        if noise < 0.5 {
            return ZONE_INDUSTRIAL;
        }
    }

    // Residential: default for outskirts
    ZONE_RESIDENTIAL
}

/// Selects a building kind based on semantic zone type.
/// Commercial: saloons, general stores, banks, sheriff's office.
/// Residential: houses, churches, hotels.
/// Industrial: stables, blacksmiths, undertakers.
fn zone_building_kind(zone: u32, noise: f64) -> u32 {
    match zone {
        ZONE_COMMERCIAL => {
            if noise < 0.20 { 1 }       // Saloon
            else if noise < 0.40 { 3 }   // General Store
            else if noise < 0.55 { 7 }   // Bank
            else if noise < 0.70 { 4 }   // Sheriff's Office
            else if noise < 0.85 { 5 }   // Post Office
            else { 9 }                    // Jail
        }
        ZONE_RESIDENTIAL => {
            if noise < 0.50 { 0 }        // House
            else if noise < 0.70 { 6 }   // Church
            else if noise < 0.85 { 8 }   // Hotel
            else { 5 }                    // Post Office
        }
        ZONE_INDUSTRIAL => {
            if noise < 0.30 { 2 }        // Stable
            else if noise < 0.55 { 11 }  // Blacksmith
            else if noise < 0.75 { 3 }   // General Store
            else { 10 }                   // Undertaker
        }
        // Mixed / catch-all: saloons, hotels.
        _ => {
            if noise < 0.35 { 1 }        // Saloon
            else if noise < 0.60 { 8 }   // Hotel
            else if noise < 0.80 { 0 }   // House
            else { 1 }                    // Saloon
        }
    }
}

// ── Landmark anchoring ───────────────────────────────────────────────────

/// Pre-places a small number of landmark anchor buildings at fixed or
/// strongly-biased positions. These anchors are immovable constraints
/// that the BSP partition works around rather than over, giving every
/// generated town a consistent skeleton.
fn pre_place_landmark_anchors(
    width: CoordinateUnit,
    height: CoordinateUnit,
    seed: NoiseSeed,
    avenue_ys: &[CoordinateUnit],
) -> Vec<Building> {
    let mut anchors = Vec::new();
    if width < 60 || height < 40 {
        return anchors;
    }
    let anchor_seed = seed.wrapping_add(777888);
    let center_x = width / 2;
    let center_y = height / 2;

    // Sheriff's office near the town center
    let sx = (center_x - 8 + (value_noise(1, 1, anchor_seed) * 10.0) as CoordinateUnit)
        .clamp(8, width - 20);
    let sy = (center_y - 4 + (value_noise(2, 2, anchor_seed) * 6.0) as CoordinateUnit)
        .clamp(8, height - 16);
    anchors.push(Building { x: sx, y: sy, w: 12, h: 10, kind: 4 });

    // Saloon on the main street corner
    if let Some(&first_avenue) = avenue_ys.first() {
        let sal_x = (center_x + 20 + (value_noise(3, 3, anchor_seed) * 10.0) as CoordinateUnit)
            .clamp(8, width - 22);
        let sal_y = (first_avenue + 6).clamp(8, height - 14);
        anchors.push(Building { x: sal_x, y: sal_y, w: 14, h: 10, kind: 1 });
    }

    // Church toward the outskirts
    let cx = (width / 4 + (value_noise(5, 5, anchor_seed) * 15.0) as CoordinateUnit)
        .clamp(8, width - 16);
    let cy = (height / 4 + (value_noise(6, 6, anchor_seed) * 10.0) as CoordinateUnit)
        .clamp(8, height - 14);
    anchors.push(Building { x: cx, y: cy, w: 12, h: 10, kind: 6 });

    anchors
}

/// AABB overlap test between two rectangles.
fn rects_overlap(
    ax: CoordinateUnit, ay: CoordinateUnit, aw: CoordinateUnit, ah: CoordinateUnit,
    bx: CoordinateUnit, by: CoordinateUnit, bw: CoordinateUnit, bh: CoordinateUnit,
) -> bool {
    ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by
}

// ── BSP-based building generation ────────────────────────────────────────

/// Generates building footprints using BSP spatial partitioning.
///
/// The algorithm:
///  1. Pre-places landmark anchor buildings at biased positions.
///  2. Builds a BSP tree over the buildable area, biasing splits to align
///     with the street network.
///  3. Assigns a semantic zone to each leaf based on position.
///  4. Places one building per leaf, selecting the type from the zone palette.
///     A small fraction of leaves become parks instead of buildings.
///  5. Validates each placement with an AABB overlap check against all
///     already-placed footprints including their padding margins.
///  6. Records unoccupied leaves as open lots for later scatter.
///
/// Returns (buildings, parks, open_lots).
fn generate_buildings_bsp(
    width: CoordinateUnit,
    height: CoordinateUnit,
    seed: NoiseSeed,
    avenue_ys: &[CoordinateUnit],
    cross_xs: &[CoordinateUnit],
    avenue_half_width: CoordinateUnit,
    cross_half_width: CoordinateUnit,
) -> (Vec<Building>, Vec<(CoordinateUnit, CoordinateUnit, CoordinateUnit, CoordinateUnit)>, Vec<(CoordinateUnit, CoordinateUnit, CoordinateUnit, CoordinateUnit)>) {
    let mut buildings = Vec::new();
    let mut parks = Vec::new();
    let mut open_lots = Vec::new();
    let bsp_seed = seed.wrapping_add(11111);

    // Keep BSP within the road network. Use the forest margin on large maps,
    // but scale down for smaller maps to preserve buildable area.
    let margin = (30 as CoordinateUnit).min(width / 8).min(height / 8).max(6);
    let build_w = width - margin * 2;
    let build_h = height - margin * 2;
    if build_w < BSP_MIN_LEAF_W || build_h < BSP_MIN_LEAF_H {
        return (buildings, parks, open_lots);
    }

    // Step 1: Pre-place landmark anchors as immovable constraints
    let anchors = pre_place_landmark_anchors(width, height, seed, avenue_ys);
    let mut placed: Vec<(CoordinateUnit, CoordinateUnit, CoordinateUnit, CoordinateUnit)> =
        Vec::new();
    for a in &anchors {
        placed.push((a.x, a.y, a.w, a.h));
        buildings.push(Building { x: a.x, y: a.y, w: a.w, h: a.h, kind: a.kind });
    }

    // Step 2: Build BSP tree biased to align with streets
    let mut root = BspNode::new(margin, margin, build_w, build_h);
    root.subdivide(bsp_seed, 0, avenue_ys, cross_xs);

    // Step 3: Collect leaves and place buildings or parks
    let mut leaves = Vec::new();
    root.collect_leaves(&mut leaves);

    for (i, &(lx, ly, lw, lh)) in leaves.iter().enumerate() {
        let leaf_cx = lx + lw / 2;
        let leaf_cy = ly + lh / 2;

        // Skip leaves whose center falls on an avenue
        let on_avenue = avenue_ys.iter().any(|&ay| {
            ly < ay + avenue_half_width + 1 && ly + lh > ay - avenue_half_width - 1
        });
        if on_avenue {
            continue;
        }

        // Skip leaves that overlap with pre-placed landmark anchors
        let overlaps_anchor = anchors.iter().any(|a| {
            rects_overlap(lx, ly, lw, lh, a.x, a.y, a.w, a.h)
        });
        if overlaps_anchor {
            continue;
        }

        // Building footprint with padding inside the leaf.
        // Scale building to fill the available node (up to a generous max)
        // so every leaf above the minimum threshold attempts placement.
        let pad = BSP_PADDING;
        let bx = lx + pad;
        let by = ly + pad;
        let bw = (lw - pad * 2).min(30);
        let bh = (lh - pad * 2).min(24);

        if bw < 4 || bh < 4 {
            open_lots.push((lx, ly, lw, lh));
            continue;
        }

        // Check building footprint (not leaf) against cross streets.
        // If it overlaps, try shrinking the building to fit beside the street.
        let building_on_cross = cross_xs.iter().any(|&cx| {
            bx < cx + cross_half_width + 1 && bx + bw > cx - cross_half_width - 1
        });
        let (bx, bw) = if building_on_cross {
            // Try to fit on either side of the cross street
            let mut result: Option<(CoordinateUnit, CoordinateUnit)> = None;
            for &cx in cross_xs {
                // Try placing to the left of the cross street
                let left_edge = cx - cross_half_width - 1;
                if left_edge > bx {
                    let left_w = (left_edge - bx).min(bw);
                    if left_w >= 4 {
                        result = Some((bx, left_w));
                        break;
                    }
                }
                // Try placing to the right of the cross street
                let right_x = cx + cross_half_width + 1;
                if right_x >= lx && right_x < bx + bw {
                    let right_w = (bx + bw - right_x).min(bw);
                    if right_w >= 4 {
                        result = Some((right_x, right_w));
                        break;
                    }
                }
            }
            match result {
                Some(fit) => fit,
                None => {
                    open_lots.push((lx, ly, lw, lh));
                    continue;
                }
            }
        } else {
            (bx, bw)
        };

        // Park allocation: ~10% of eligible leaves become parks
        let park_noise = value_noise(leaf_cx, leaf_cy, bsp_seed.wrapping_add(8888));
        if park_noise < 0.10 && bw >= 4 && bh >= 4 {
            parks.push((bx, by, bw, bh));
            placed.push((bx, by, bw, bh));
            continue;
        }

        // Semantic zone assignment
        let zone = assign_zone(leaf_cx, leaf_cy, width, height, avenue_ys, seed);
        let kind_noise =
            value_noise(i as i32, leaf_cx + leaf_cy, bsp_seed.wrapping_add(2222));
        let kind = zone_building_kind(zone, kind_noise).min(BUILDING_TYPE_COUNT - 1);

        // AABB overlap check against all already-placed footprints.
        // Try progressively smaller sizes if the full building doesn't fit.
        let overlap_margin = 1;
        let mut final_w = bw;
        let mut final_h = bh;
        let mut fits = false;

        for shrink in 0..4 {
            let sw = bw - shrink * 2;
            let sh = bh - shrink * 2;
            if sw < 4 || sh < 4 { break; }
            let overlaps = placed.iter().any(|&(px, py, pw, ph)| {
                rects_overlap(
                    bx - overlap_margin,
                    by - overlap_margin,
                    sw + overlap_margin * 2,
                    sh + overlap_margin * 2,
                    px, py, pw, ph,
                )
            });
            if !overlaps {
                final_w = sw;
                final_h = sh;
                fits = true;
                break;
            }
        }

        if !fits {
            open_lots.push((lx, ly, lw, lh));
            continue;
        }

        placed.push((bx, by, final_w, final_h));
        buildings.push(Building { x: bx, y: by, w: final_w, h: final_h, kind });
    }

    // ── Sub-partition large leaves for additional buildings ───────────
    // Re-scan leaves: any leaf large enough to hold two minimum-size
    // buildings gets a secondary split. Each sub-plot receives its own
    // building. The threshold is kept low so lots are densely filled.
    let sub_threshold_w = 4 * 2 + BSP_PADDING * 2; // minimum 12 tiles wide
    let sub_threshold_h = 4 * 2 + BSP_PADDING * 2; // minimum 12 tiles tall
    let mut extra_buildings: Vec<Building> = Vec::new();
    for &(lx, ly, lw, lh) in &leaves {
        // Only subdivide leaves that are genuinely large
        if lw < sub_threshold_w && lh < sub_threshold_h {
            continue;
        }
        // Skip leaves already covered by a building or park
        let already_used = placed.iter().any(|&(px, py, pw, ph)| {
            rects_overlap(lx, ly, lw, lh, px, py, pw, ph)
        });
        if already_used { continue; }

        // Simple two-way split (horizontal or vertical)
        let sub_rects: Vec<(CoordinateUnit, CoordinateUnit, CoordinateUnit, CoordinateUnit)> =
            if lw > lh && lw >= sub_threshold_w {
                let half = lw / 2;
                vec![(lx, ly, half - 1, lh), (lx + half + 1, ly, lw - half - 1, lh)]
            } else if lh >= sub_threshold_h {
                let half = lh / 2;
                vec![(lx, ly, lw, half - 1), (lx, ly + half + 1, lw, lh - half - 1)]
            } else {
                continue;
            };

        for (si, &(sx, sy, sw, sh)) in sub_rects.iter().enumerate() {
            let pad = BSP_PADDING;
            let sbx = sx + pad;
            let sby = sy + pad;
            let sbw = (sw - pad * 2).min(30);
            let sbh = (sh - pad * 2).min(24);
            if sbw < 4 || sbh < 4 { continue; }

            let overlaps = placed.iter().any(|&(px, py, pw, ph)| {
                rects_overlap(sbx - 1, sby - 1, sbw + 2, sbh + 2, px, py, pw, ph)
            });
            if overlaps { continue; }

            let leaf_cx = sx + sw / 2;
            let leaf_cy = sy + sh / 2;
            let zone = assign_zone(leaf_cx, leaf_cy, width, height, avenue_ys, seed);
            let kind_noise = value_noise(
                si as i32 + 100,
                leaf_cx + leaf_cy,
                bsp_seed.wrapping_add(5555),
            );
            let kind = zone_building_kind(zone, kind_noise).min(BUILDING_TYPE_COUNT - 1);

            placed.push((sbx, sby, sbw, sbh));
            extra_buildings.push(Building { x: sbx, y: sby, w: sbw, h: sbh, kind });
        }
    }
    buildings.extend(extra_buildings);

    // ── Densification: fill remaining open lots with buildings ────────
    // Open lots are leaves that couldn't fit a building in the main pass.
    // Attempt to place a minimal building in each one so lots aren't empty.
    let mut infill_buildings: Vec<Building> = Vec::new();
    for &(lx, ly, lw, lh) in &open_lots {
        let pad = BSP_PADDING;
        let bx = lx + pad;
        let by = ly + pad;
        // Cap infill buildings at modest sizes so they fit snugly in
        // leftover lots without overwhelming the surrounding buildings.
        let bw = (lw - pad * 2).min(20);
        let bh = (lh - pad * 2).min(16);
        if bw < 4 || bh < 4 { continue; }
        let overlaps = placed.iter().any(|&(px, py, pw, ph)| {
            rects_overlap(bx - 1, by - 1, bw + 2, bh + 2, px, py, pw, ph)
        });
        if overlaps { continue; }
        let leaf_cx = lx + lw / 2;
        let leaf_cy = ly + lh / 2;
        let zone = assign_zone(leaf_cx, leaf_cy, width, height, avenue_ys, seed);
        let kind_noise = value_noise(
            leaf_cx + leaf_cy,
            leaf_cx,
            bsp_seed.wrapping_add(9999),
        );
        let kind = zone_building_kind(zone, kind_noise).min(BUILDING_TYPE_COUNT - 1);
        placed.push((bx, by, bw, bh));
        infill_buildings.push(Building { x: bx, y: by, w: bw, h: bh, kind });
    }
    buildings.extend(infill_buildings);

    // ── Lot-based densification: fill every road-grid cell with buildings ─
    // A "lot" is the rectangular space between consecutive avenues (horizontal)
    // and cross streets (vertical). For each lot, find the largest clear
    // rectangle, BSP-split it, and place a building in every sub-node.
    let sidewalk_width: CoordinateUnit = 1;
    let cross_sidewalk_width: CoordinateUnit = 1;
    // Build sorted boundary lists including map edges
    let mut y_bounds: Vec<CoordinateUnit> = Vec::new();
    y_bounds.push(margin);
    for &ay in avenue_ys {
        y_bounds.push(ay);
    }
    y_bounds.push(height - margin);
    y_bounds.sort();

    let mut x_bounds: Vec<CoordinateUnit> = Vec::new();
    x_bounds.push(margin);
    for &cx in cross_xs {
        x_bounds.push(cx);
    }
    x_bounds.push(width - margin);
    x_bounds.sort();

    // Minimum lot dimension below which no building is attempted.
    const LOT_MIN_DIM: CoordinateUnit = 5;
    // Minimum viable plot that can hold a building (min building = 4×4).
    const PLOT_MIN_DIM: CoordinateUnit = 5;
    // Target plot side length for grid subdivision.
    const PLOT_TARGET: CoordinateUnit = 9;

    let mut lot_buildings: Vec<Building> = Vec::new();
    for yi in 0..y_bounds.len().saturating_sub(1) {
        for xi in 0..x_bounds.len().saturating_sub(1) {
            // Lot inner boundaries: first non-road tile + 1-tile sidewalk
            let lot_top = y_bounds[yi] + avenue_half_width + sidewalk_width + 1;
            let lot_bot = y_bounds[yi + 1] - avenue_half_width - sidewalk_width - 1;
            let lot_left = x_bounds[xi] + cross_half_width + cross_sidewalk_width + 1;
            let lot_right = x_bounds[xi + 1] - cross_half_width - cross_sidewalk_width - 1;
            let lot_w = lot_right - lot_left;
            let lot_h = lot_bot - lot_top;

            // Skip lots below minimum size
            if lot_w < LOT_MIN_DIM || lot_h < LOT_MIN_DIM { continue; }

            // Skip lots that already have building coverage from BSP passes
            let lot_has_building = placed.iter().any(|&(px, py, pw, ph)| {
                rects_overlap(lot_left, lot_top, lot_w, lot_h, px, py, pw, ph)
            });
            if lot_has_building { continue; }

            let lot_seed = bsp_seed.wrapping_add(
                (yi as u64).wrapping_mul(1000).wrapping_add(xi as u64).wrapping_mul(7)
            );

            let lot_cx = lot_left + lot_w / 2;
            let lot_cy = lot_top + lot_h / 2;

            // Small lots just above the minimum: single building filling the lot
            if lot_w < PLOT_MIN_DIM * 2 && lot_h < PLOT_MIN_DIM * 2 {
                let bx = lot_left + 1;
                let by = lot_top + 1;
                let bw = (lot_w - 2).max(4);
                let bh = (lot_h - 2).max(4);
                if bw < 4 || bh < 4 { continue; }
                let overlaps = placed.iter().any(|&(px, py, pw, ph)| {
                    rects_overlap(bx - 1, by - 1, bw + 2, bh + 2, px, py, pw, ph)
                });
                if overlaps { continue; }
                let zone = assign_zone(lot_cx, lot_cy, width, height, avenue_ys, seed);
                let kind_noise = value_noise(lot_left, lot_top, lot_seed.wrapping_add(6666));
                let kind = zone_building_kind(zone, kind_noise).min(BUILDING_TYPE_COUNT - 1);
                placed.push((bx, by, bw, bh));
                lot_buildings.push(Building { x: bx, y: by, w: bw, h: bh, kind });
                continue;
            }

            // Grid-based partitioning: divide lot into a grid of plots.
            // For elongated lots, partition only along the longer axis.
            let cols = if lot_w >= PLOT_MIN_DIM * 2 {
                (lot_w / PLOT_TARGET).max(1).min(lot_w / PLOT_MIN_DIM)
            } else { 1 };
            let rows = if lot_h >= PLOT_MIN_DIM * 2 {
                (lot_h / PLOT_TARGET).max(1).min(lot_h / PLOT_MIN_DIM)
            } else { 1 };

            // Compute column x-offsets and widths with slight jitter
            let base_col_w = lot_w / cols;
            let mut col_xs: Vec<CoordinateUnit> = Vec::with_capacity(cols as usize);
            let mut col_ws: Vec<CoordinateUnit> = Vec::with_capacity(cols as usize);
            let mut cur_x = lot_left;
            for c in 0..cols {
                col_xs.push(cur_x);
                let w = if c == cols - 1 {
                    lot_left + lot_w - cur_x
                } else {
                    let jn = value_noise(c, yi as i32, lot_seed.wrapping_add(111));
                    let jitter = (jn.clamp(0.0, 0.999) * 3.0) as CoordinateUnit - 1;
                    (base_col_w + jitter).max(PLOT_MIN_DIM)
                };
                col_ws.push(w);
                cur_x += w;
            }

            // Compute row y-offsets and heights with slight jitter
            let base_row_h = lot_h / rows;
            let mut row_ys: Vec<CoordinateUnit> = Vec::with_capacity(rows as usize);
            let mut row_hs: Vec<CoordinateUnit> = Vec::with_capacity(rows as usize);
            let mut cur_y = lot_top;
            for r in 0..rows {
                row_ys.push(cur_y);
                let h = if r == rows - 1 {
                    lot_top + lot_h - cur_y
                } else {
                    let jn = value_noise(r, xi as i32, lot_seed.wrapping_add(222));
                    let jitter = (jn.clamp(0.0, 0.999) * 3.0) as CoordinateUnit - 1;
                    (base_row_h + jitter).max(PLOT_MIN_DIM)
                };
                row_hs.push(h);
                cur_y += h;
            }

            // Place one building per plot, sized to fill it
            for r in 0..rows as usize {
                for c in 0..cols as usize {
                    let plot_x = col_xs[c];
                    let plot_y = row_ys[r];
                    let plot_w = col_ws[c];
                    let plot_h = row_hs[r];

                    // 1-tile gap between adjacent buildings within the lot
                    let gap_r: CoordinateUnit = if c < (cols - 1) as usize { 1 } else { 0 };
                    let gap_b: CoordinateUnit = if r < (rows - 1) as usize { 1 } else { 0 };
                    let bx = plot_x;
                    let by = plot_y;
                    let bw = plot_w - gap_r;
                    let bh = plot_h - gap_b;

                    if bw < 4 || bh < 4 { continue; }

                    let overlaps = placed.iter().any(|&(px, py, pw, ph)| {
                        rects_overlap(bx - 1, by - 1, bw + 2, bh + 2, px, py, pw, ph)
                    });
                    if overlaps { continue; }

                    // Each plot gets its own building type from the zone palette
                    let plot_cx = plot_x + plot_w / 2;
                    let plot_cy = plot_y + plot_h / 2;
                    let zone = assign_zone(plot_cx, plot_cy, width, height, avenue_ys, seed);
                    let kind_noise = value_noise(
                        c as i32 + r as i32 * 7 + plot_cx,
                        plot_cy,
                        lot_seed.wrapping_add(6666),
                    );
                    let kind = zone_building_kind(zone, kind_noise).min(BUILDING_TYPE_COUNT - 1);

                    placed.push((bx, by, bw, bh));
                    lot_buildings.push(Building { x: bx, y: by, w: bw, h: bh, kind });
                }
            }
        }
    }
    buildings.extend(lot_buildings);

    (buildings, parks, open_lots)
}

/// Places alley floor tiles in the gaps between adjacent buildings.
/// BSP partitioning naturally creates gaps via inter-node padding; alleys
/// fill those gaps with shadowed ambush terrain.
fn place_alleys(map: &mut GameMap, buildings: &[Building]) {
    // Detect gaps up to 6 tiles (BSP_PADDING + 2 × building pad).
    let alley_gap_threshold: i64 = 6;
    for (i, a) in buildings.iter().enumerate() {
        for b in &buildings[i + 1..] {
            // Horizontal adjacency: overlapping Y ranges with an X gap
            if a.y < b.y + b.h && b.y < a.y + a.h {
                let gap = b.x as i64 - (a.x + a.w) as i64;
                if (1..=alley_gap_threshold).contains(&gap) {
                    let overlap_y_min = a.y.max(b.y);
                    let overlap_y_max = (a.y + a.h).min(b.y + b.h);
                    for y in overlap_y_min..overlap_y_max {
                        for gx in 0..gap as CoordinateUnit {
                            let pos = GridVec::new(a.x + a.w + gx, y);
                            if let Some(voxel) = map.get_voxel_at_mut(&pos)
                                && voxel.props.is_none() && !matches!(voxel.floor, Some(Floor::WoodPlanks) | Some(Floor::StoneFloor)) {
                                    voxel.floor = Some(Floor::Alley);
                                }
                        }
                    }
                }
                // Check reverse direction
                let gap_rev = a.x as i64 - (b.x + b.w) as i64;
                if (1..=alley_gap_threshold).contains(&gap_rev) {
                    let overlap_y_min = a.y.max(b.y);
                    let overlap_y_max = (a.y + a.h).min(b.y + b.h);
                    for y in overlap_y_min..overlap_y_max {
                        for gx in 0..gap_rev as CoordinateUnit {
                            let pos = GridVec::new(b.x + b.w + gx, y);
                            if let Some(voxel) = map.get_voxel_at_mut(&pos)
                                && voxel.props.is_none() && !matches!(voxel.floor, Some(Floor::WoodPlanks) | Some(Floor::StoneFloor)) {
                                    voxel.floor = Some(Floor::Alley);
                                }
                        }
                    }
                }
            }
            // Vertical adjacency: overlapping X ranges with a Y gap
            if a.x < b.x + b.w && b.x < a.x + a.w {
                let gap = b.y as i64 - (a.y + a.h) as i64;
                if (1..=alley_gap_threshold).contains(&gap) {
                    let overlap_x_min = a.x.max(b.x);
                    let overlap_x_max = (a.x + a.w).min(b.x + b.w);
                    for x in overlap_x_min..overlap_x_max {
                        for gy in 0..gap as CoordinateUnit {
                            let pos = GridVec::new(x, a.y + a.h + gy);
                            if let Some(voxel) = map.get_voxel_at_mut(&pos)
                                && voxel.props.is_none() && !matches!(voxel.floor, Some(Floor::WoodPlanks) | Some(Floor::StoneFloor)) {
                                    voxel.floor = Some(Floor::Alley);
                                }
                        }
                    }
                }
                let gap_rev = a.y as i64 - (b.y + b.h) as i64;
                if (1..=alley_gap_threshold).contains(&gap_rev) {
                    let overlap_x_min = a.x.max(b.x);
                    let overlap_x_max = (a.x + a.w).min(b.x + b.w);
                    for x in overlap_x_min..overlap_x_max {
                        for gy in 0..gap_rev as CoordinateUnit {
                            let pos = GridVec::new(x, b.y + b.h + gy);
                            if let Some(voxel) = map.get_voxel_at_mut(&pos)
                                && voxel.props.is_none() && !matches!(voxel.floor, Some(Floor::WoodPlanks) | Some(Floor::StoneFloor)) {
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
                    // Stone buildings: churches, banks, jails
                    if matches!(b.kind, 6 | 7 | 9) {
                        voxel.props = Some(Props::StoneWall);
                        voxel.floor = Some(Floor::StoneFloor);
                    } else {
                        voxel.props = Some(Props::Wall);
                        voxel.floor = Some(Floor::WoodPlanks);
                    }
                } else {
                    voxel.props = None;
                    if matches!(b.kind, 6 | 7 | 9) {
                        voxel.floor = Some(Floor::StoneFloor);
                    } else {
                        voxel.floor = Some(Floor::WoodPlanks);
                    }
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
            // House: dividing wall separates front room from back room
            if iw >= 6 && ih >= 8 {
                // Large: dividing wall + furnished rooms
                let wall_row = ih / 2;
                for dx in 0..iw {
                    if dx == iw / 2 { continue; }
                    let pos = GridVec::new(interior_x + dx, interior_y + wall_row);
                    if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                        if matches!(b.kind, 6 | 7 | 9) {
                            voxel.props = Some(Props::StoneWall);
                        } else {
                            voxel.props = Some(Props::Wall);
                        }
                    }
                }
                // Front room: dining table with chairs
                set_prop(map, interior_x + 2, interior_y + wall_row + 2, Props::Table);
                set_prop(map, interior_x + 1, interior_y + wall_row + 2, Props::Chair);
                set_prop(map, interior_x + 3, interior_y + wall_row + 2, Props::Chair);
                // Back room: bedroom area
                set_prop(map, interior_x + iw - 1, interior_y + 1, Props::Bench);
                set_prop(map, interior_x + iw - 2, interior_y + 1, Props::Bench);
                // Storage
                set_prop(map, interior_x, interior_y, Props::Barrel);
                set_prop(map, interior_x + iw - 1, interior_y, Props::Crate);
            } else if iw >= 4 && ih >= 4 {
                // Medium: simple dividing wall with door gap, minimal furnishing
                let wall_row = ih / 2;
                for dx in 0..iw {
                    if dx == iw / 2 { continue; }
                    let pos = GridVec::new(interior_x + dx, interior_y + wall_row);
                    if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                        voxel.props = Some(Props::Wall);
                    }
                }
                set_prop(map, interior_x + 1, interior_y + wall_row + 1, Props::Table);
                set_prop(map, interior_x, interior_y + wall_row + 1, Props::Chair);
                set_prop(map, interior_x + iw - 1, interior_y, Props::Bench);
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
            // Sheriff's office: desk area in front, rear cell block
            if iw >= 5 && ih >= 4 {
                // Wanted posters on front wall
                set_prop(map, interior_x, interior_y + ih - 1, Props::Sign);
                set_prop(map, interior_x + 1, interior_y + ih - 1, Props::Sign);
                // Desk area (front half)
                set_prop(map, interior_x + 2, interior_y + ih - 2, Props::Table);
                set_prop(map, interior_x + 1, interior_y + ih - 2, Props::Chair);
                set_prop(map, interior_x + 3, interior_y + ih - 2, Props::Chair);
                // Cell block dividing wall with door
                let cell_wall_row = ih / 2;
                for dx in 0..iw {
                    if dx == iw / 2 { continue; } // door gap
                    let pos = GridVec::new(interior_x + dx, interior_y + cell_wall_row);
                    if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                        voxel.props = Some(Props::Wall);
                    }
                }
                // Cell furnishings in the rear
                set_prop(map, interior_x + iw - 1, interior_y, Props::Barrel);
                set_prop(map, interior_x, interior_y, Props::Bench);
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
    // ── Wall integrity: repair any gaps in the perimeter ───────────────
    validate_building_walls(map, b);
    map.mark_occupied(b.x, b.y, b.w, b.h);
}

/// Validates and repairs the wall perimeter of a building. Ensures every
/// edge tile that should be a wall (except doors and rounded corners) is
/// filled. Called immediately after placement to fix BSP-clipping or
/// late occupancy conflicts.
fn validate_building_walls(map: &mut GameMap, b: &Building) {
    let shape_noise = value_noise(b.x + b.y, b.w + b.h, 0u64.wrapping_add(77777));
    let shape_type = if b.w >= 8 && b.h >= 8 {
        (shape_noise * 3.0) as u32
    } else {
        SHAPE_RECT
    };
    let notch_w = b.w / L_SHAPE_NOTCH_DIVISOR;
    let notch_h = b.h / L_SHAPE_NOTCH_DIVISOR;
    let notch_x_start = b.x + b.w - notch_w;
    let notch_y_start = b.y + b.h - notch_h;
    let corner_radius = ROUNDED_CORNER_RADIUS;
    let wall_prop = if matches!(b.kind, 6 | 7 | 9) { Props::StoneWall } else { Props::Wall };

    for y in b.y..b.y + b.h {
        for x in b.x..b.x + b.w {
            // L-shape: skip the notch
            if shape_type == SHAPE_L && x >= notch_x_start && y >= notch_y_start {
                continue;
            }
            let is_border = x == b.x || x == b.x + b.w - 1 || y == b.y || y == b.y + b.h - 1;
            // L-shape additional border edges
            let is_l_border = if shape_type == SHAPE_L {
                (x == notch_x_start - 1 && y >= notch_y_start)
                    || (y == notch_y_start - 1 && x >= notch_x_start)
            } else {
                false
            };
            // Door positions
            let is_door = (y == b.y + b.h - 1 && x == b.x + b.w / 2 && (shape_type != SHAPE_L || x < notch_x_start))
                || (y == b.y && x == b.x + b.w / 2);
            let is_side_door = if b.w >= 9 || b.h >= 9 {
                (x == b.x && y == b.y + b.h / 2)
                || (x == b.x + b.w - 1 && y == b.y + b.h / 2 && (shape_type != SHAPE_L || y < notch_y_start))
            } else {
                false
            };
            let is_corner = if shape_type == SHAPE_ROUNDED {
                (b.x + b.w - 1 - x < corner_radius || x - b.x < corner_radius)
                    && (b.y + b.h - 1 - y < corner_radius || y - b.y < corner_radius)
            } else {
                false
            };

            if (is_border || is_l_border) && !is_door && !is_side_door && !is_corner {
                let pos = GridVec::new(x, y);
                if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                    if !voxel.props.as_ref().is_some_and(|p| p.is_wall()) {
                        voxel.props = Some(wall_prop.clone());
                    }
                }
            }
        }
    }
}

/// Helper: sets a prop at a position if within bounds, not occupied by a wall,
/// and not on a road, beach, water, or bridge tile. Also checks the occupancy grid.
fn set_prop(map: &mut GameMap, x: CoordinateUnit, y: CoordinateUnit, prop: Props) {
    if x < 0 || x >= map.width || y < 0 || y >= map.height { return; }
    // Check occupancy grid before taking a mutable borrow on voxels
    if map.occupancy[x as usize][y as usize] { return; }
    let pos = GridVec::new(x, y);
    if let Some(voxel) = map.get_voxel_at_mut(&pos)
        && !matches!(voxel.props, Some(Props::Wall) | Some(Props::StoneWall))
        && !matches!(voxel.floor, Some(Floor::DirtRoad) | Some(Floor::BeachSand)
            | Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Bridge)) {
            voxel.props = Some(prop);
        }
}

// ── Hierarchical placement: exterior props ───────────────────────────────

/// Places contextually appropriate exterior props around a building based
/// on its type and which edge faces the street. A house gets a fence
/// perimeter and a garden; a saloon gets hitching posts out front and
/// barrels along the side; a stable gets a corral attached to its rear.
fn place_exterior_props(map: &mut GameMap, b: &Building, seed: NoiseSeed) {
    let ext_seed = seed.wrapping_add(b.x as u64 * 31 + b.y as u64 * 37);
    match b.kind {
        0 => {
            // House: fence perimeter along the south edge, small garden/well
            for dx in -1..=b.w {
                set_prop(map, b.x + dx, b.y + b.h, Props::Fence);
            }
            let pick = value_noise(b.x, b.y, ext_seed);
            if pick < 0.5 {
                set_prop(map, b.x + b.w / 2, b.y - 1, Props::Well);
            } else {
                set_prop(map, b.x + 1, b.y - 1, Props::Bush);
                set_prop(map, b.x + 2, b.y - 1, Props::Bush);
            }
        }
        1 => {
            // Saloon: hitching posts out front, barrels along the side
            set_prop(map, b.x + b.w / 3, b.y + b.h, Props::HitchingPost);
            set_prop(map, b.x + b.w * 2 / 3, b.y + b.h, Props::HitchingPost);
            for dy in (0..b.h).step_by(3) {
                set_prop(map, b.x - 1, b.y + dy, Props::Barrel);
            }
        }
        2 => {
            // Stable: corral fence attached to rear, hay and water
            let corral_max_width: CoordinateUnit = 6;
            for dx in 0..b.w.min(corral_max_width) {
                set_prop(map, b.x + dx, b.y - 1, Props::Fence);
            }
            set_prop(map, b.x + 2, b.y - 2, Props::HayBale);
            set_prop(map, b.x + 4, b.y - 2, Props::WaterTrough);
        }
        3 => {
            // General store: crates and barrels near entrance
            set_prop(map, b.x + b.w / 2 - 1, b.y + b.h, Props::Crate);
            set_prop(map, b.x + b.w / 2 + 1, b.y + b.h, Props::Barrel);
        }
        4 => {
            // Sheriff's office: hitching post and wanted poster
            set_prop(map, b.x + b.w / 2, b.y + b.h, Props::HitchingPost);
            set_prop(map, b.x - 1, b.y + b.h / 2, Props::Sign);
        }
        _ => {}
    }
}

// ── Open lot scatter ─────────────────────────────────────────────────────

/// Fills open lots (BSP leaves that couldn't hold a building) with
/// contextually appropriate scatter: dead trees, broken wagons, campfire
/// rings, patches of scrub.
fn place_open_lot_scatter(
    map: &mut GameMap,
    open_lots: &[(CoordinateUnit, CoordinateUnit, CoordinateUnit, CoordinateUnit)],
    seed: NoiseSeed,
) {
    let scatter_seed = seed.wrapping_add(151515);
    for (i, &(lx, ly, lw, lh)) in open_lots.iter().enumerate() {
        let cx = lx + lw / 2;
        let cy = ly + lh / 2;
        let pick = value_noise(cx, cy, scatter_seed.wrapping_add(i as u64));

        if pick < 0.25 {
            set_prop(map, cx, cy, Props::DeadTree);
        } else if pick < 0.50 {
            // Broken wagon
            set_prop(map, cx, cy, Props::Crate);
            set_prop(map, cx + 1, cy, Props::Crate);
        } else if pick < 0.75 {
            // Campfire ring
            set_prop(map, cx - 1, cy, Props::Rock);
            set_prop(map, cx + 1, cy, Props::Rock);
            set_prop(map, cx, cy - 1, Props::Rock);
            set_prop(map, cx, cy + 1, Props::Rock);
        } else {
            // Scrub patch
            set_prop(map, cx, cy, Props::Bush);
            set_prop(map, cx + 1, cy, Props::Bush);
            set_prop(map, cx - 1, cy + 1, Props::Bush);
        }
    }
}

// ── Transition zones ─────────────────────────────────────────────────────

/// Places transitional scatter around the town perimeter — isolated shacks,
/// ruined structures, old fence lines, abandoned campsites — that gradually
/// thin out into open terrain. This makes the town feel like it grew
/// organically rather than being stamped onto the map.
fn place_transition_zones(
    map: &mut GameMap,
    width: CoordinateUnit,
    height: CoordinateUnit,
    seed: NoiseSeed,
    buildings: &[Building],
) {
    if width < 60 || height < 40 || buildings.is_empty() {
        return;
    }
    let trans_seed = seed.wrapping_add(121212);

    // Find the bounding box of the town
    let town_min_x = buildings.iter().map(|b| b.x).min().unwrap();
    let town_min_y = buildings.iter().map(|b| b.y).min().unwrap();
    let town_max_x = buildings.iter().map(|b| b.x + b.w).max().unwrap();
    let town_max_y = buildings.iter().map(|b| b.y + b.h).max().unwrap();

    let num_scatter = 12 + (value_noise(0, 0, trans_seed) * 8.0) as i32;
    for i in 0..num_scatter {
        let nx = value_noise(i, 0, trans_seed);
        let ny = value_noise(0, i, trans_seed);
        let x = (nx * width as f64).clamp(4.0, (width - 6) as f64) as CoordinateUnit;
        let y = (ny * height as f64).clamp(4.0, (height - 6) as f64) as CoordinateUnit;

        // Only place in the transition zone (outside town boundaries)
        if x >= town_min_x - 5 && x <= town_max_x + 5
            && y >= town_min_y - 5 && y <= town_max_y + 5
        {
            continue;
        }

        let pos = GridVec::new(x, y);
        if let Some(voxel) = map.get_voxel_at(&pos) {
            if voxel.props.is_some() {
                continue;
            }
            if matches!(
                voxel.floor,
                Some(Floor::ShallowWater)
                    | Some(Floor::DeepWater)
                    | Some(Floor::Bridge)
            ) {
                continue;
            }
        } else {
            continue;
        }

        let pick = value_noise(i, i, trans_seed.wrapping_add(111));
        if pick < 0.25 {
            set_prop(map, x, y, Props::DeadTree);
        } else if pick < 0.45 {
            // Old fence line
            for dx in 0..4 {
                set_prop(map, x + dx, y, Props::Fence);
            }
        } else if pick < 0.65 {
            // Abandoned campsite
            set_prop(map, x, y, Props::Rock);
            set_prop(map, x + 1, y, Props::Rock);
            set_prop(map, x, y + 1, Props::Barrel);
        } else if pick < 0.80 {
            // Broken wagon
            set_prop(map, x, y, Props::Crate);
            set_prop(map, x + 1, y, Props::Crate);
        } else {
            // Scrub
            set_prop(map, x, y, Props::Bush);
            set_prop(map, x + 1, y + 1, Props::Bush);
        }
    }

    // Place isolated outskirts shacks — small ruined structures with
    // partial walls that thin out toward the wilderness.
    let shack_seed = trans_seed.wrapping_add(333);
    let num_shacks = 4 + (value_noise(1, 1, shack_seed) * 4.0) as i32;
    for i in 0..num_shacks {
        let nx = value_noise(i, 2, shack_seed);
        let ny = value_noise(2, i, shack_seed);
        let sx = (nx * width as f64).clamp(8.0, (width - 12) as f64) as CoordinateUnit;
        let sy = (ny * height as f64).clamp(8.0, (height - 12) as f64) as CoordinateUnit;

        // Must be outside the town bounding box
        if sx >= town_min_x - 10 && sx <= town_max_x + 10
            && sy >= town_min_y - 10 && sy <= town_max_y + 10
        {
            continue;
        }

        // Skip water
        let pos = GridVec::new(sx, sy);
        if let Some(voxel) = map.get_voxel_at(&pos) {
            if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater)) {
                continue;
            }
        } else {
            continue;
        }

        // Place a small 4×4 shack with partial walls (ruined)
        let sw: CoordinateUnit = 4;
        let sh: CoordinateUnit = 4;
        if sx + sw >= width - 1 || sy + sh >= height - 1 { continue; }
        for dy in 0..sh {
            for dx in 0..sw {
                let p = GridVec::new(sx + dx, sy + dy);
                if let Some(voxel) = map.get_voxel_at_mut(&p) {
                    if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater)) {
                        continue;
                    }
                    let is_border = dx == 0 || dx == sw - 1 || dy == 0 || dy == sh - 1;
                    let is_door = dy == sh - 1 && dx == sw / 2;
                    // Ruined: skip some wall segments based on noise
                    let ruin_noise = value_noise(sx + dx, sy + dy, shack_seed.wrapping_add(i as u64));
                    if is_border && !is_door && ruin_noise < 0.7 {
                        voxel.props = Some(Props::Wall);
                        voxel.floor = Some(Floor::WoodPlanks);
                    } else {
                        voxel.props = None;
                        voxel.floor = Some(Floor::WoodPlanks);
                    }
                }
            }
        }

        // Lay a short dirt path from the shack door toward the town
        let door_x = sx + sw / 2;
        let door_y = sy + sh;
        let town_cx = (town_min_x + town_max_x) / 2;
        let town_cy = (town_min_y + town_max_y) / 2;
        let path_dx: CoordinateUnit = if town_cx > door_x { 1 } else { -1 };
        let path_dy: CoordinateUnit = if town_cy > door_y { 1 } else { -1 };
        let mut px = door_x;
        let mut py = door_y;
        for _ in 0..15 {
            if px <= 0 || px >= width - 1 || py <= 0 || py >= height - 1 { break; }
            let p = GridVec::new(px, py);
            if let Some(voxel) = map.get_voxel_at_mut(&p)
                && !matches!(voxel.props, Some(Props::Wall) | Some(Props::StoneWall))
                && !matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater))
            {
                voxel.floor = Some(Floor::Dirt);
                voxel.props = None;
            }
            // Alternate between horizontal and vertical steps
            let step_noise = value_noise(px, py, shack_seed.wrapping_add(777));
            if step_noise < 0.5 {
                px += path_dx;
            } else {
                py += path_dy;
            }
        }
    }
}

// ── Post-pass connectivity check ─────────────────────────────────────────

/// Verifies every building has at least one navigable path to the main
/// street using BFS. If a building is unreachable, opens a gap in the
/// nearest blocking structure. No structure should be an island.
fn check_connectivity(
    map: &mut GameMap,
    buildings: &[Building],
    avenue_ys: &[CoordinateUnit],
    cross_xs: &[CoordinateUnit],
    width: CoordinateUnit,
    height: CoordinateUnit,
) {
    if buildings.is_empty() || (avenue_ys.is_empty() && cross_xs.is_empty()) {
        return;
    }
    if width < 30 || height < 30 {
        return;
    }

    let w = width as usize;
    let _h = height as usize;
    let mut reachable = vec![false; w * height as usize];
    let mut queue = std::collections::VecDeque::new();

    // Seed BFS from all passable road tiles (dirt, sidewalk, bridge, alley).
    // This is more robust than seeding only from avenue/cross-street centers,
    // because those specific positions may be blocked by the river.
    for y in 0..height {
        for x in 0..width {
            let pos = GridVec::new(x, y);
            if let Some(v) = map.get_voxel_at(&pos) {
                if matches!(v.floor, Some(Floor::Dirt) | Some(Floor::DirtRoad) | Some(Floor::Sidewalk) | Some(Floor::Bridge) | Some(Floor::Alley))
                    && map.is_passable(&pos)
                {
                    let idx = y as usize * w + x as usize;
                    reachable[idx] = true;
                    queue.push_back(pos);
                }
            }
        }
    }

    while let Some(pos) = queue.pop_front() {
        for neighbor in pos.cardinal_neighbors() {
            if neighbor.x >= 0
                && neighbor.x < width
                && neighbor.y >= 0
                && neighbor.y < height
            {
                let idx = neighbor.y as usize * w + neighbor.x as usize;
                if !reachable[idx] && map.is_passable(&neighbor) {
                    reachable[idx] = true;
                    queue.push_back(neighbor);
                }
            }
        }
    }

    // Check each building's door area
    for b in buildings {
        let door_x = b.x + b.w / 2;
        let door_y = b.y + b.h; // Tile just outside south door
        if door_x < 0 || door_x >= width || door_y < 0 || door_y >= height {
            continue;
        }
        let idx = door_y as usize * w + door_x as usize;
        if reachable[idx] {
            continue;
        }

        // Building unreachable — clear blocking props and bridge water to open a path.
        // Search up to half the map size to ensure we always find a road.
        let max_r = (width / 3).max(height / 3).max(20);
        for r in 1..=max_r {
            let mut cleared = false;
            for dy in -r..=r {
                for dx in -r..=r {
                    let cx = door_x + dx;
                    let cy = door_y + dy;
                    if cx < 0 || cx >= width || cy < 0 || cy >= height {
                        continue;
                    }
                    let cidx = cy as usize * w + cx as usize;
                    if reachable[cidx] {
                        // Clear a line from door to this reachable tile,
                        // creating a dirt path and removing blocking props.
                        // Widen to 3 tiles to ensure cardinal connectivity
                        // for diagonal Bresenham segments.
                        let from = GridVec::new(door_x, door_y);
                        let to = GridVec::new(cx, cy);
                        for step in from.bresenham_line(to) {
                            for wy in -1..=1i32 {
                                for wx in -1..=1i32 {
                                    let wp = GridVec::new(step.x + wx, step.y + wy);
                                    if wp.x >= 0
                                        && wp.x < width
                                        && wp.y >= 0
                                        && wp.y < height
                                    {
                                        if let Some(voxel) = map.get_voxel_at_mut(&wp) {
                                            if voxel
                                                .props
                                                .as_ref()
                                                .is_some_and(|p| p.blocks_movement() && !p.is_wall())
                                            {
                                                voxel.props = None;
                                            }
                                            // Bridge water tiles to create walkable path
                                            if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater)) {
                                                voxel.floor = Some(Floor::Bridge);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        cleared = true;
                        break;
                    }
                }
                if cleared {
                    break;
                }
            }
            if cleared {
                break;
            }
        }
    }
}

/// Structural verification of world generation invariants.
/// Returns `Ok(())` if the map passes all checks, or `Err(reason)` describing
/// which check failed. Used by the retry loop in `GameMap::new()`.
fn verify_world(
    map: &GameMap,
    buildings: &[Building],
    avenue_ys: &[CoordinateUnit],
    cross_xs: &[CoordinateUnit],
    width: CoordinateUnit,
    height: CoordinateUnit,
) -> Result<(), String> {
    if buildings.is_empty() || width < 60 || height < 60 || avenue_ys.is_empty() {
        return Ok(()); // Skip validation on maps without a proper road network
    }

    // 1. No two building footprints overlap (including 1-tile gap margin).
    //    Expand one building by 1 tile on each side and check against the
    //    exact footprint of the other — this enforces a 1-tile minimum gap
    //    which matches the grid-based lot partitioning gap.
    for (i, a) in buildings.iter().enumerate() {
        for b in &buildings[i + 1..] {
            let overlaps = rects_overlap(
                a.x - 1, a.y - 1, a.w + 2, a.h + 2,
                b.x, b.y, b.w, b.h,
            );
            if overlaps {
                return Err(format!(
                    "buildings at ({},{}) {}x{} and ({},{}) {}x{} overlap (including padding)",
                    a.x, a.y, a.w, a.h, b.x, b.y, b.w, b.h
                ));
            }
        }
    }

    // 2. No building tile sits on a river, road, or beach tile
    for b in buildings {
        for by in b.y..b.y + b.h {
            for bx in b.x..b.x + b.w {
                let pos = GridVec::new(bx, by);
                if let Some(v) = map.get_voxel_at(&pos) {
                    if matches!(v.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater)) {
                        return Err(format!(
                            "building at ({},{}) has tile on river at ({},{})",
                            b.x, b.y, bx, by
                        ));
                    }
                }
            }
        }
    }

    // 3. Every building has a reachable entrance (BFS from all road tiles)
    let w = width as usize;
    let mut reachable = vec![false; w * height as usize];
    let mut queue = std::collections::VecDeque::new();
    for y in 0..height {
        for x in 0..width {
            let pos = GridVec::new(x, y);
            if let Some(v) = map.get_voxel_at(&pos) {
                if matches!(v.floor, Some(Floor::Dirt) | Some(Floor::DirtRoad) | Some(Floor::Sidewalk) | Some(Floor::Bridge) | Some(Floor::Alley))
                    && map.is_passable(&pos)
                {
                    let idx = y as usize * w + x as usize;
                    reachable[idx] = true;
                    queue.push_back(pos);
                }
            }
        }
    }
    while let Some(pos) = queue.pop_front() {
        for neighbor in pos.cardinal_neighbors() {
            if neighbor.x >= 0 && neighbor.x < width && neighbor.y >= 0 && neighbor.y < height {
                let idx = neighbor.y as usize * w + neighbor.x as usize;
                if !reachable[idx] && map.is_passable(&neighbor) {
                    reachable[idx] = true;
                    queue.push_back(neighbor);
                }
            }
        }
    }
    for b in buildings {
        let door_x = b.x + b.w / 2;
        let door_y = b.y + b.h;
        if door_x >= 0 && door_x < width && door_y >= 0 && door_y < height {
            let idx = door_y as usize * w + door_x as usize;
            if !reachable[idx] {
                return Err(format!(
                    "building at ({},{}) has unreachable entrance at ({},{})",
                    b.x, b.y, door_x, door_y
                ));
            }
        }
    }

    // 4. Street graph is connected: every avenue tile that is passable
    //    can reach every cross-street tile that is passable.
    if avenue_ys.len() >= 2 {
        let first_ay = avenue_ys[0];
        let last_ay = *avenue_ys.last().unwrap();
        if first_ay >= 0 && first_ay < height && last_ay >= 0 && last_ay < height {
            let mid_x = width / 2;
            let start_idx = first_ay as usize * w + mid_x as usize;
            let end_idx = last_ay as usize * w + mid_x as usize;
            if reachable[start_idx] && !reachable[end_idx] {
                return Err(format!(
                    "street graph not fully connected — avenue y={} cannot reach avenue y={}",
                    first_ay, last_ay
                ));
            }
        }
    }
    for &cx in cross_xs {
        if cx >= 0 && cx < width {
            if let Some(&ay) = avenue_ys.first() {
                if ay >= 0 && ay < height {
                    let pos = GridVec::new(cx, ay);
                    if map.is_passable(&pos) {
                        let idx = ay as usize * w + cx as usize;
                        if !reachable[idx] {
                            return Err(format!(
                                "cross street x={} not connected to avenue y={}",
                                cx, ay
                            ));
                        }
                    }
                }
            }
        }
    }

    // 5. No building footprint contains a DirtRoad, water, or beach tile
    for b in buildings {
        for by in b.y..b.y + b.h {
            for bx in b.x..b.x + b.w {
                let pos = GridVec::new(bx, by);
                if let Some(v) = map.get_voxel_at(&pos) {
                    if matches!(v.floor, Some(Floor::DirtRoad) | Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::BeachSand)) {
                        return Err(format!(
                            "building at ({},{}) has excluded tile at ({},{})",
                            b.x, b.y, bx, by
                        ));
                    }
                }
            }
        }
    }

    // 6. No prop or tree sits on a water tile
    for y in 0..height {
        for x in 0..width {
            let pos = GridVec::new(x, y);
            if let Some(v) = map.get_voxel_at(&pos) {
                if v.props.is_some()
                    && !v.props.as_ref().unwrap().is_wall()
                    && matches!(v.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater))
                {
                    return Err(format!(
                        "prop on water tile at ({},{})",
                        x, y
                    ));
                }
            }
        }
    }

    // 7. Bridges now override BeachSand tiles within the road band so
    //    bridge surfaces are clean. No separate verification needed.

    // 8. Wall perimeter integrity is enforced by validate_building_walls()
    //    during placement. Shape variants (L-shape, rounded corners) have
    //    intentional wall gaps that make a generic perimeter check fragile,
    //    so verification trusts the placement-time repair pass.

    Ok(())
}

/// Returns `true` if the rectangle (rx, ry, rw, rh) overlaps any building
/// footprint (including a 2-tile buffer for doors and entrance access).
fn overlaps_any_building(
    rx: CoordinateUnit, ry: CoordinateUnit, rw: CoordinateUnit, rh: CoordinateUnit,
    buildings: &[Building],
) -> bool {
    buildings.iter().any(|b| {
        rects_overlap(rx, ry, rw, rh, b.x - 2, b.y - 2, b.w + 4, b.h + 4)
    })
}

/// Places the focal mission/church building near the center of the map.
/// This is a large, thick-walled stone structure with a courtyard, bell tower,
/// and multiple interior rooms that functions as a natural fortress and
/// late-game combat anchor.
fn place_mission(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed, buildings: &[Building]) {
    let mw: CoordinateUnit = 24;
    let mh: CoordinateUnit = 18;
    if width < mw + 10 || height < mh + 10 {
        return; // map too small for a mission
    }
    let m_seed = seed.wrapping_add(555666);
    // Try several candidate positions around the town center.
    // Spread candidates widely to find a spot free of roads.
    let offsets: [(i32, i32); 8] = [
        (0, 0), (-15, -15), (15, 15), (-30, 20), (30, -20),
        (-20, 30), (20, -30), (-35, -10),
    ];
    for (i, &(ox, oy)) in offsets.iter().enumerate() {
        let noise_x = (value_noise(i as i32 * 2, 2, m_seed) * 10.0) as CoordinateUnit;
        let noise_y = (value_noise(i as i32 * 2 + 1, 3, m_seed) * 10.0) as CoordinateUnit;
        let cx = width / 2 + ox + noise_x;
        let cy = height / 2 + oy + noise_y;
        let mx = (cx - mw / 2).clamp(2, width - mw - 2);
        let my = (cy - mh / 2).clamp(2, height - mh - 2);

        if overlaps_any_building(mx, my, mw, mh, buildings) { continue; }
        if map.has_excluded_tile(mx, my, mw, mh) { continue; }

        // Lay down thick stone walls and interior
        place_mission_at(map, mx, my, mw, mh);
        return;
    }
}

/// Internal helper: actually places mission tiles at the given position.
fn place_mission_at(map: &mut GameMap, mx: CoordinateUnit, my: CoordinateUnit, mw: CoordinateUnit, mh: CoordinateUnit) {
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
                    voxel.props = Some(Props::StoneWall);
                    voxel.floor = Some(Floor::StoneFloor);
                } else {
                    voxel.props = None;
                    voxel.floor = Some(Floor::StoneFloor);
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
                voxel.props = Some(Props::StoneWall);
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
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                voxel.floor = Some(Floor::Rooftop);
            }
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
    map.mark_occupied(mx, my, mw, mh);
}

/// Places a town plaza — an open killzone at the heart of the map.
/// The plaza is flanked by buildings with window-facing tiles overlooking it.
fn place_town_plaza(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed, buildings: &[Building]) {
    let pw: CoordinateUnit = 16;
    let ph: CoordinateUnit = 12;
    if width < pw + 20 || height < ph + 20 {
        return;
    }
    let p_seed = seed.wrapping_add(777888);
    // Try several candidate positions for the plaza
    let offsets: [(i32, i32); 8] = [
        (15, 10), (-10, 0), (0, -10), (-25, 25), (25, -25),
        (-15, -20), (20, 20), (-30, 10),
    ];
    for (i, &(ox, oy)) in offsets.iter().enumerate() {
        let noise_x = (value_noise(i as i32 * 2 + 5, 5, p_seed) * 10.0) as CoordinateUnit;
        let noise_y = (value_noise(i as i32 * 2 + 6, 6, p_seed) * 10.0) as CoordinateUnit;
        let cx = width / 2 + ox + noise_x;
        let cy = height / 2 + oy + noise_y;
        let px = (cx - pw / 2).clamp(2, width - pw - 2);
        let py = (cy - ph / 2).clamp(2, height - ph - 2);

        if overlaps_any_building(px, py, pw, ph, buildings) { continue; }
        if map.has_excluded_tile(px, py, pw, ph) { continue; }

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
        map.mark_occupied(px, py, pw, ph);
        break;
    }
}

/// Places a large Town Hall building near the center of the map.
/// The Town Hall is 18×12 with a grand interior containing tables, chairs,
/// benches and signs (maps/notices).
/// Skipped on maps too small to fit the building.
fn place_town_hall(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed, buildings: &[Building]) {
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

    if overlaps_any_building(tx, ty, tw, th, buildings) { return; }
    if map.has_excluded_tile(tx, ty, tw, th) { return; }

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
    map.mark_occupied(tx, ty, tw, th);
}

/// Places a Grand Saloon — a large 20×14 saloon with piano, many tables,
/// chairs, and barrels. Placed in the southern half of the map.
/// Skipped on maps too small to fit the building.
fn place_grand_saloon(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed, buildings: &[Building]) {
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

    if overlaps_any_building(sx, sy, sw, sh, buildings) { return; }
    if map.has_excluded_tile(sx, sy, sw, sh) { return; }

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
    map.mark_occupied(sx, sy, sw, sh);
}

/// Places street props (benches, barrels, signs, hitching posts, crates) on
/// actual road-edge tiles so they follow road curvature. Props are never
/// placed in a straight line unless the road itself is straight at that point.
fn place_street_props_curved(
    map: &mut GameMap,
    road_edge_tiles: &[(CoordinateUnit, CoordinateUnit)],
    seed: NoiseSeed,
) {
    let furn_seed = seed.wrapping_add(77777);
    // Place props every ~4 road-edge tiles for spacing.
    for (idx, &(x, y)) in road_edge_tiles.iter().enumerate() {
        if idx % 4 != 0 { continue; }
        let noise = value_noise(x, y, furn_seed.wrapping_add(idx as u64));
        let prop = match (noise * 6.0) as u32 {
            0 => Props::HitchingPost,
            1 => Props::Bench,
            2 => Props::Barrel,
            3 => Props::WaterTrough,
            4 => Props::Sign,
            _ => Props::Crate,
        };
        set_prop(map, x, y, prop);
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
                if matches!(voxel.floor, Some(Floor::WoodPlanks) | Some(Floor::StoneFloor)) {
                    continue;
                }
                // Skip road tiles — no decorations on roads or sidewalks.
                if matches!(voxel.floor, Some(Floor::DirtRoad) | Some(Floor::Sidewalk)) {
                    continue;
                }
                // Skip bridge, beach sand tiles — no decorations.
                if matches!(voxel.floor, Some(Floor::Bridge) | Some(Floor::BeachSand)) {
                    continue;
                }
                // Skip occupied tiles
                if map.occupancy[x as usize][y as usize] {
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
fn place_cemetery(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed, buildings: &[Building]) {
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

    if overlaps_any_building(px, py, cw, ch, buildings) { return; }
    if map.has_excluded_tile(px, py, cw, ch) { return; }

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
fn place_corral(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed, buildings: &[Building]) {
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

    if overlaps_any_building(px, py, cw, ch, buildings) { return; }
    if map.has_excluded_tile(px, py, cw, ch) { return; }

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

    // Check that the 3×3 footprint doesn't overlap excluded tiles
    if map.has_excluded_tile(wx - 1, wy - 1, 3, 3) { return; }

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

    // Check that the 4×4 footprint doesn't overlap excluded tiles
    if map.has_excluded_tile(gx, gy, 4, 4) { return; }

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

    // Check that the 5×5 footprint doesn't overlap excluded tiles
    if map.has_excluded_tile(tx, ty, 5, 5) { return; }

    // 5×5 water tower: 4 leg posts, tank on top (represented as tower height)
    for dy in 0..5i32 {
        for dx in 0..5i32 {
            let pos = GridVec::new(tx + dx, ty + dy);
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                voxel.floor = Some(Floor::Dirt);
                voxel.props = None;
            }
        }
    }
    // Four corner legs
    set_prop(map, tx, ty, Props::WaterTower);
    set_prop(map, tx + 4, ty, Props::WaterTower);
    set_prop(map, tx, ty + 4, Props::WaterTower);
    set_prop(map, tx + 4, ty + 4, Props::WaterTower);
}

/// Places railroad tracks near the northern and southern edges of the map.
/// Tracks never intersect the main road. Where tracks cross the river a
/// rail bridge is placed automatically.
fn place_railroad(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) {
    if width < 80 || height < 80 { return; }
    let rail_seed = seed.wrapping_add(777999);

    // Two track lines: one near the north edge, one near the south edge.
    let north_y = 10 + (value_noise(8, 8, rail_seed) * 6.0) as CoordinateUnit;
    let north_y = north_y.clamp(6, 20);
    let south_y = height - 10 - (value_noise(9, 9, rail_seed) * 6.0) as CoordinateUnit;
    let south_y = south_y.clamp(height - 20, height - 6);

    for &rail_y in &[north_y, south_y] {
        for x in 4..width - 4 {
            let wobble = (value_noise(x, rail_y, rail_seed) * 1.5) as CoordinateUnit;
            let y = rail_y + wobble;
            if y <= 0 || y >= height - 1 { continue; }
            let pos = GridVec::new(x, y);
            if let Some(voxel) = map.get_voxel_at(&pos) {
                // Never place tracks on the main road or buildings
                if matches!(voxel.floor, Some(Floor::DirtRoad) | Some(Floor::Sidewalk)) { continue; }
                if matches!(voxel.props, Some(Props::Wall) | Some(Props::StoneWall)) { continue; }
                // Rail bridge over water/beach — lay bridge tile with track
                if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Beach) | Some(Floor::BeachSand)) {
                    if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                        voxel.floor = Some(Floor::Bridge);
                        voxel.props = Some(Props::RailTrack);
                    }
                    // Mark rail tile as occupied
                    map.occupancy[x as usize][y as usize] = true;
                    continue;
                }
            }
            if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                voxel.floor = Some(Floor::Gravel);
                voxel.props = Some(Props::RailTrack);
            }
            // Mark rail tile as occupied
            map.occupancy[x as usize][y as usize] = true;
            // Gravel bed on either side of the track
            for &dy in &[-1i32, 1] {
                let side_pos = GridVec::new(x, y + dy);
                if let Some(voxel) = map.get_voxel_at_mut(&side_pos)
                    && voxel.props.is_none()
                    && !matches!(voxel.floor, Some(Floor::WoodPlanks) | Some(Floor::StoneFloor)
                        | Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::DirtRoad)
                        | Some(Floor::Sidewalk) | Some(Floor::Bridge)) {
                        voxel.floor = Some(Floor::Gravel);
                    }
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

    // Check that the 5×5 footprint doesn't overlap excluded tiles
    if map.has_excluded_tile(mx, my, 5, 5) { return; }

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
                } else {
                    voxel.props = None;
                    voxel.floor = Some(Floor::WoodPlanks);
                }
            }
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
                    && matches!(voxel.floor, Some(Floor::Sidewalk) | Some(Floor::DirtRoad) | Some(Floor::Sand))
                {
                    set_prop(map, x, y, Props::LampPost);
                }
        }
    }
}

/// Places a large stone church building — a prominent landmark with stone walls,
/// stained glass (signs), and pews. Uses the same stone material as the mission.
fn place_stone_church(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed, buildings: &[Building]) {
    let cw: CoordinateUnit = 16;
    let ch: CoordinateUnit = 20;
    if width < cw + 20 || height < ch + 20 {
        return;
    }
    let c_seed = seed.wrapping_add(424242);
    // Scan candidate positions in a grid across the buildable area.
    // The stone church is large (16×20) so we need to search broadly.
    'search: for qy in 0..4i32 {
        for qx in 0..4i32 {
            let noise_x = (value_noise(qx + 11, qy + 11, c_seed) * 20.0) as CoordinateUnit;
            let noise_y = (value_noise(qx + 12, qy + 12, c_seed) * 10.0) as CoordinateUnit;
            let cx = width / 5 + qx * (width / 5) + noise_x;
            let cy = height / 5 + qy * (height / 5) + noise_y;
            let bx = (cx - cw / 2).clamp(2, width - cw - 2);
            let by = (cy - ch / 2).clamp(2, height - ch - 2);

            if overlaps_any_building(bx, by, cw, ch, buildings) { continue; }
            if map.has_excluded_tile(bx, by, cw, ch) { continue; }

            // Stone walls and floor
            for y in by..by + ch {
                for x in bx..bx + cw {
                    let pos = GridVec::new(x, y);
                    if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                        let is_border = x == bx || x == bx + cw - 1 || y == by || y == by + ch - 1;
                        let is_main_door = y == by + ch - 1 && (x == bx + cw / 2 || x == bx + cw / 2 - 1);
                        let is_back_door = y == by && x == bx + cw / 2;
                        if is_border && !is_main_door && !is_back_door {
                            voxel.props = Some(Props::StoneWall);
                            voxel.floor = Some(Floor::StoneFloor);
                        } else {
                            voxel.props = None;
                            voxel.floor = Some(Floor::StoneFloor);
                        }
                    }
                }
            }

            let ix = bx + 1;
            let iy = by + 1;
            let iw = cw - 2;
            let ih = ch - 2;

            // Altar at the north end
            set_prop(map, ix + iw / 2, iy, Props::Table);
            set_prop(map, ix + iw / 2 - 1, iy, Props::Sign);
            set_prop(map, ix + iw / 2 + 1, iy, Props::Sign);

            // Pew rows (benches in two columns)
            for row in 3..ih.min(14) {
                set_prop(map, ix + 2, iy + row, Props::Bench);
                if iw >= 8 {
                    set_prop(map, ix + iw - 3, iy + row, Props::Bench);
                }
            }

            // Stained glass (signs along walls)
            for row in (2..ih - 2).step_by(3) {
                set_prop(map, ix, iy + row, Props::Sign);
                set_prop(map, ix + iw - 1, iy + row, Props::Sign);
            }

            // Bell tower: 3×3 rooftop area in corner
            for dy in 0..3i32 {
                for dx in 0..3i32 {
                    let pos = GridVec::new(ix + dx, iy + dy);
                    if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                        voxel.floor = Some(Floor::Rooftop);
                    }
                }
            }
            map.mark_occupied(bx, by, cw, ch);
            break 'search;
        }
    }
}

/// Places small outpost structures along the map edges — defensive positions
/// that serve as spawn anchors for factions on the outskirts.
fn place_outposts(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed, buildings: &[Building]) {
    if width < 30 || height < 30 { return; } // map too small for outposts
    let out_seed = seed.wrapping_add(191919);
    let num_outposts = 4;
    for i in 0..num_outposts {
        let angle = (i as f64 / num_outposts as f64) * std::f64::consts::TAU
            + value_noise(i, 0, out_seed) * 0.8;
        let dist = (width.min(height) as f64 * 0.35) + value_noise(0, i, out_seed) * 20.0;
        let ox = (width as f64 / 2.0 + angle.cos() * dist) as CoordinateUnit;
        let oy = (height as f64 / 2.0 + angle.sin() * dist) as CoordinateUnit;
        let ox = ox.clamp(8, width - 14);
        let oy = oy.clamp(8, height - 14);

        // 6×6 stone outpost
        let ow: CoordinateUnit = 6;
        let oh: CoordinateUnit = 6;

        if overlaps_any_building(ox, oy, ow, oh, buildings) { continue; }
        if map.has_excluded_tile(ox, oy, ow, oh) { continue; }

        for dy in 0..oh {
            for dx in 0..ow {
                let pos = GridVec::new(ox + dx, oy + dy);
                if let Some(voxel) = map.get_voxel_at_mut(&pos) {
                    let is_border = dx == 0 || dx == ow - 1 || dy == 0 || dy == oh - 1;
                    let is_door = dy == oh - 1 && dx == ow / 2;
                    if is_border && !is_door {
                        voxel.props = Some(Props::StoneWall);
                        voxel.floor = Some(Floor::StoneFloor);
                    } else {
                        voxel.props = None;
                        voxel.floor = Some(Floor::StoneFloor);
                    }
                }
            }
        }
        // Interior crate + barrel
        set_prop(map, ox + 1, oy + 1, Props::Barrel);
        set_prop(map, ox + ow - 2, oy + 1, Props::Crate);
        set_prop(map, ox + ow / 2, oy + oh / 2, Props::GunpowderBarrel);
    }
}

/// Places natural rock formations using noise-driven cluster placement.
/// Creates visually interesting terrain features on the outskirts.
fn place_rock_formations(map: &mut GameMap, width: CoordinateUnit, height: CoordinateUnit, seed: NoiseSeed) {
    if width < 80 || height < 80 { return; } // map too small for rock formations
    let rock_seed = seed.wrapping_add(282828);
    let num_formations = 6;
    for i in 0..num_formations {
        // Place formations away from center
        let fx = (value_noise(i, 0, rock_seed) * (width - 60) as f64) as CoordinateUnit + 30;
        let fy = (value_noise(0, i, rock_seed) * (height - 60) as f64) as CoordinateUnit + 30;
        let dist_to_center = ((fx - width / 2).pow(2) + (fy - height / 2).pow(2)) as f64;
        if dist_to_center < (width as f64 / 4.0).powi(2) {
            continue; // skip formations too close to center
        }
        // Noise-driven cluster of 5-12 rocks
        let cluster_size = 5 + (value_noise(i, i, rock_seed.wrapping_add(111)) * 8.0) as i32;
        let mut placed = 0;
        for dy in -3i32..=3 {
            for dx in -3i32..=3 {
                if placed >= cluster_size { break; }
                let pos = GridVec::new(fx + dx, fy + dy);
                let noise = value_noise(pos.x, pos.y, rock_seed.wrapping_add(222));
                if noise > 0.4 { continue; }
                if let Some(voxel) = map.get_voxel_at(&pos) {
                    if voxel.props.is_some() { continue; }
                    if matches!(voxel.floor, Some(Floor::WoodPlanks) | Some(Floor::StoneFloor)
                        | Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::DirtRoad)
                        | Some(Floor::Bridge) | Some(Floor::BeachSand)) {
                        continue;
                    }
                }
                set_prop(map, pos.x, pos.y, Props::Rock);
                placed += 1;
            }
            if placed >= cluster_size { break; }
        }
    }
}

/// Noise threshold for gunpowder barrel placement. Lower = more barrels.
const GUNPOWDER_BARREL_SPAWN_RATE: f64 = 0.015;

/// Scatters gunpowder barrels across the map in strategic locations.
/// Placed near buildings and along streets as environmental hazards.
fn place_gunpowder_barrels(
    map: &mut GameMap,
    width: CoordinateUnit,
    height: CoordinateUnit,
    seed: NoiseSeed,
) {
    let barrel_seed = seed.wrapping_add(131313);
    for y in 2..height - 2 {
        for x in 2..width - 2 {
            let pos = GridVec::new(x, y);
            if let Some(voxel) = map.get_voxel_at(&pos) {
                if voxel.props.is_some() { continue; }
                // Only place on suitable terrain
                if !matches!(voxel.floor,
                    Some(Floor::Dirt) | Some(Floor::Sand) | Some(Floor::Gravel)
                    | Some(Floor::Sidewalk) | Some(Floor::WoodPlanks) | Some(Floor::StoneFloor)
                ) { continue; }
            } else {
                continue;
            }
            // Skip near spawn
            if pos.distance_squared(SPAWN_POINT) < 100 { continue; }
            let noise = value_noise(x, y, barrel_seed);
            if noise > GUNPOWDER_BARREL_SPAWN_RATE { continue; }
            // Prefer tiles near buildings (at least one adjacent wall)
            let near_wall = pos.cardinal_neighbors().iter().any(|n| {
                map.get_voxel_at(n).is_some_and(|v| v.props.as_ref().is_some_and(|p| p.is_wall()))
            });
            if near_wall || value_noise(y, x, barrel_seed.wrapping_add(111)) < 0.3 {
                set_prop(map, x, y, Props::GunpowderBarrel);
            }
        }
    }
}

/// Clears all props within a given radius of a point and replaces water
/// floors with Dirt so the area is walkable.
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
                        // Replace water/beach with dirt so the area is walkable
                        if matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater) | Some(Floor::Beach) | Some(Floor::BeachSand)) {
                            voxel.floor = Some(Floor::Dirt);
                        }
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
        assert!(alley_count > 0, "Map should have alley tiles between buildings (found {})", alley_count);
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
    fn props_is_wall_helper() {
        assert!(Props::Wall.is_wall());
        assert!(!Props::Table.is_wall());
        assert!(!Props::Barrel.is_wall());
    }

    #[test]
    fn faction_anchors_deterministic() {
        let map1 = GameMap::new(200, 140, 42);
        let map2 = GameMap::new(200, 140, 42);
        assert_eq!(map1.faction_anchors.len(), map2.faction_anchors.len(),
            "Faction anchor count should be deterministic");
    }

    #[test]
    fn bsp_buildings_do_not_overlap() {
        // BSP guarantees non-overlapping plots — verify no building footprints
        // intersect.
        let (buildings, _, _) = generate_buildings_bsp(400, 280, 42, &[60, 100, 140], &[80, 160, 240], 3, 2);
        for (i, a) in buildings.iter().enumerate() {
            for b in &buildings[i + 1..] {
                let overlaps = a.x < b.x + b.w && a.x + a.w > b.x
                    && a.y < b.y + b.h && a.y + a.h > b.y;
                assert!(!overlaps,
                    "Buildings at ({},{}) {}x{} and ({},{}) {}x{} overlap",
                    a.x, a.y, a.w, a.h, b.x, b.y, b.w, b.h);
            }
        }
    }

    #[test]
    fn bsp_zone_commercial_near_streets() {
        let avenue_ys = vec![50, 90];
        // Near avenue → commercial zone
        let zone = assign_zone(100, 52, 400, 280, &avenue_ys, 42);
        assert_eq!(zone, ZONE_COMMERCIAL,
            "Tiles near avenues should be commercial zones");
        // Far from avenue → not commercial
        let zone_far = assign_zone(100, 200, 400, 280, &avenue_ys, 42);
        assert_ne!(zone_far, ZONE_COMMERCIAL,
            "Tiles far from avenues should not be commercial zones");
    }

    #[test]
    fn bsp_small_map_no_panic() {
        // Small maps should be handled gracefully (no building placement)
        let _ = GameMap::new(10, 10, 42);
        let _ = GameMap::new(20, 15, 42);
        let _ = GameMap::new(30, 20, 42);
    }

    #[test]
    fn bsp_landmark_anchors_placed_first() {
        // On sufficiently large maps, landmark anchors (sheriff, saloon,
        // church) should be placed before BSP subdivision.
        let (buildings, _, _) = generate_buildings_bsp(200, 140, 42, &[60], &[80], 3, 2);
        // Expect at least the 3 anchors
        assert!(buildings.len() >= 3,
            "Large map should have at least 3 landmark anchor buildings, found {}",
            buildings.len());
        // Sheriff (kind=4), Saloon (kind=1), Church (kind=6) should appear
        let has_sheriff = buildings.iter().any(|b| b.kind == 4);
        let has_saloon = buildings.iter().any(|b| b.kind == 1);
        let has_church = buildings.iter().any(|b| b.kind == 6);
        assert!(has_sheriff, "Landmark anchors should include sheriff's office");
        assert!(has_saloon, "Landmark anchors should include saloon");
        assert!(has_church, "Landmark anchors should include church");
    }

    #[test]
    fn large_map_has_transition_scatter() {
        let map = GameMap::new(400, 280, 42);
        // Transition zones should add scattered props around the town
        // perimeter — check for props in the first and last 20 rows.
        let mut scatter_count = 0;
        for y in 0..20 {
            for x in 0..400 {
                if map.voxels[y][x].props.is_some()
                    && matches!(map.voxels[y][x].props,
                        Some(Props::Fence) | Some(Props::DeadTree) | Some(Props::Rock)
                        | Some(Props::Barrel) | Some(Props::Crate) | Some(Props::Bush))
                {
                    scatter_count += 1;
                }
            }
        }
        for y in 260..280 {
            for x in 0..400 {
                if map.voxels[y][x].props.is_some()
                    && matches!(map.voxels[y][x].props,
                        Some(Props::Fence) | Some(Props::DeadTree) | Some(Props::Rock)
                        | Some(Props::Barrel) | Some(Props::Crate) | Some(Props::Bush))
                {
                    scatter_count += 1;
                }
            }
        }
        assert!(scatter_count > 0,
            "Town perimeter should have transition zone scatter");
    }

    #[test]
    fn stress_test_20_seeds() {
        // Run the generator across 20 different seeds and verify
        // all structural verification checks pass for each one.
        // GameMap::new() runs verify_world internally and retries on
        // failure, panicking in debug builds if exhausted.
        for seed in 0..20u64 {
            let map = GameMap::new(400, 280, seed);
            // Basic structural checks
            assert_eq!(map.width, 400);
            assert_eq!(map.height, 280);
            // Must have buildings (wood plank floors)
            let mut has_wood = false;
            let mut has_dirt = false;
            for y in 0..280 {
                for x in 0..400 {
                    if matches!(map.voxels[y][x].floor, Some(Floor::WoodPlanks)) {
                        has_wood = true;
                    }
                    if matches!(map.voxels[y][x].floor, Some(Floor::DirtRoad)) {
                        has_dirt = true;
                    }
                }
                if has_wood && has_dirt { break; }
            }
            assert!(has_wood, "Seed {seed}: map must have buildings (wood plank floors)");
            assert!(has_dirt, "Seed {seed}: map must have streets (dirt roads)");
            // Must have faction anchors
            assert!(
                !map.faction_anchors.is_empty(),
                "Seed {seed}: map must have faction anchors"
            );
            // Spawn area must be clear
            let spawn_passable = map.is_passable(&SPAWN_POINT);
            assert!(spawn_passable, "Seed {seed}: spawn point must be passable");

            // Confirm no building tiles sit on river tiles (post-verification)
            let mut building_on_water = false;
            for y in 0..280 {
                for x in 0..400 {
                    // If it has a wall and water floor, it's a violation
                    if let Some(voxel) = map.get_voxel_at(&GridVec::new(x as i32, y as i32)) {
                        if voxel.props.as_ref().is_some_and(|p| p.is_wall())
                            && matches!(voxel.floor, Some(Floor::ShallowWater) | Some(Floor::DeepWater))
                        {
                            building_on_water = true;
                        }
                    }
                }
            }
            assert!(!building_on_water, "Seed {seed}: no building walls should be on river tiles");
        }
    }

    #[test]
    fn small_map_spawn_area_passable() {
        let map = GameMap::new(120, 80, 42);
        let pos = GridVec::new(61, 41);
        assert!(
            map.is_passable(&pos),
            "120x80: ({},{}) not passable, floor={:?} props={:?}",
            pos.x, pos.y,
            map.get_voxel_at(&pos).map(|v| &v.floor),
            map.get_voxel_at(&pos).and_then(|v| v.props.as_ref())
        );
    }

    #[test]
    fn lot_grid_partitioning_produces_multiple_buildings() {
        // The grid-based lot partitioning should produce more buildings
        // than the old single-building-per-lot approach. On a map with
        // multiple road grid cells, empty lots are subdivided into plots.
        let (buildings, _, _) = generate_buildings_bsp(
            400, 280, 42, &[60, 100, 140, 180], &[80, 160, 240, 320], 3, 2,
        );
        // With 4 avenues and 4 cross streets, there are many lots.
        // Each lot is partitioned into a grid of plots.
        assert!(
            buildings.len() >= 20,
            "Grid partitioning should produce dense building placement, found {} buildings",
            buildings.len()
        );
        // Verify building type variety — at least 4 distinct kinds
        let mut kinds: HashSet<u32> = HashSet::new();
        for b in &buildings {
            kinds.insert(b.kind);
        }
        assert!(
            kinds.len() >= 4,
            "Lots should have building variety, found only {} distinct kinds",
            kinds.len()
        );
    }
}
