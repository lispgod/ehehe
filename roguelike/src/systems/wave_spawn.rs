use std::collections::HashSet;

use bevy::prelude::*;

use crate::components::{
    AiState, BlocksMovement, CombatStats, Energy, Health, Hostile, Name, Position,
    Renderable, Speed, Viewshed,
};
use crate::grid_vec::GridVec;
use crate::noise::value_noise;
use crate::resources::{GameMapResource, MapSeed, TurnCounter};
use crate::typedefs::RatColor;

/// Monster archetypes for wave spawning (same templates as initial spawning).
struct WaveMonsterTemplate {
    name: &'static str,
    symbol: &'static str,
    fg: RatColor,
    health: i32,
    attack: i32,
    defense: i32,
    speed: i32,
    sight_range: i32,
}

const WAVE_TEMPLATES: &[WaveMonsterTemplate] = &[
    WaveMonsterTemplate {
        name: "Goblin",
        symbol: "g",
        fg: RatColor::Rgb(0, 200, 0),
        health: 8,
        attack: 3,
        defense: 1,
        speed: 80,
        sight_range: 8,
    },
    WaveMonsterTemplate {
        name: "Orc",
        symbol: "o",
        fg: RatColor::Rgb(180, 0, 0),
        health: 16,
        attack: 4,
        defense: 2,
        speed: 60,
        sight_range: 6,
    },
    WaveMonsterTemplate {
        name: "Rat",
        symbol: "r",
        fg: RatColor::Rgb(160, 120, 80),
        health: 4,
        attack: 2,
        defense: 0,
        speed: 120,
        sight_range: 5,
    },
];

/// Minimum distance (squared) from the player when spawning new wave enemies.
const MIN_WAVE_SPAWN_DIST_SQ: i32 = 10 * 10;

/// Maximum distance (squared) from the player for wave spawns — keep them
/// relatively close so the player encounters them quickly.
const MAX_WAVE_SPAWN_DIST_SQ: i32 = 30 * 30;

/// How often (in turns) a new wave spawns.
const WAVE_INTERVAL: u32 = 5;

/// Base number of enemies per wave.
const WAVE_BASE_COUNT: u32 = 2;

/// Additional enemies per wave cycle (scales with turn count).
const WAVE_SCALE_PER_CYCLE: u32 = 1;

/// Multiplier mixed with turn number to produce per-wave seed variation.
const WAVE_SEED_MULTIPLIER: u64 = 13337;

/// Seed offset for the Y-axis noise, ensuring independent x/y coordinates.
const Y_AXIS_SEED_OFFSET: u64 = 7777;

/// Seed offset for monster template selection, decorrelated from position noise.
const TEMPLATE_SEED_OFFSET: u64 = 98765;

/// Spawns waves of enemies as turns progress.
///
/// Every `WAVE_INTERVAL` turns, spawns a batch of enemies near the player.
/// The batch size grows over time, creating the escalating pressure of
/// Vampire Survivors-style gameplay.
pub fn wave_spawn_system(
    mut commands: Commands,
    turn_counter: Res<TurnCounter>,
    map: Res<GameMapResource>,
    seed: Res<MapSeed>,
    player_query: Query<&Position, With<crate::components::Player>>,
    existing_positions: Query<&Position>,
) {
    let turn = turn_counter.0;

    // Only spawn on wave intervals (and not on turn 0).
    if turn == 0 || turn % WAVE_INTERVAL != 0 {
        return;
    }

    let Ok(player_pos) = player_query.single() else {
        return;
    };
    let player_vec = player_pos.as_grid_vec();

    // Collect occupied positions to avoid stacking entities.
    let occupied: HashSet<GridVec> = existing_positions.iter().map(|p| p.as_grid_vec()).collect();

    // How many enemies to spawn this wave.
    let wave_number = turn / WAVE_INTERVAL;
    let count = WAVE_BASE_COUNT + wave_number.saturating_sub(1) * WAVE_SCALE_PER_CYCLE;

    // Use turn-seeded noise for deterministic but varied spawn positions.
    let wave_seed = seed.0.wrapping_add(turn as u64 * WAVE_SEED_MULTIPLIER);
    let mut spawned = 0u32;
    let mut attempt = 0u32;

    while spawned < count && attempt < count * 20 {
        // Generate candidate position using noise.
        let nx = value_noise(attempt as i32, turn as i32, wave_seed);
        let ny = value_noise(turn as i32, attempt as i32, wave_seed.wrapping_add(Y_AXIS_SEED_OFFSET));

        // Map noise to an offset within the spawn ring around the player.
        let range = 30; // half-width of the search area
        let dx = (nx * (range * 2) as f64) as i32 - range;
        let dy = (ny * (range * 2) as f64) as i32 - range;
        let candidate = player_vec + GridVec::new(dx, dy);

        attempt += 1;

        // Check distance constraints.
        let dist_sq = candidate.distance_squared(player_vec);
        if dist_sq < MIN_WAVE_SPAWN_DIST_SQ || dist_sq > MAX_WAVE_SPAWN_DIST_SQ {
            continue;
        }

        // Check map bounds and passability.
        if !map.0.is_passable(&candidate) {
            continue;
        }

        // Check for existing entities.
        if occupied.contains(&candidate) {
            continue;
        }

        // Select monster template deterministically.
        let template_noise = value_noise(candidate.x, candidate.y, wave_seed.wrapping_add(TEMPLATE_SEED_OFFSET));
        let idx = (template_noise * WAVE_TEMPLATES.len() as f64) as usize;
        let template = &WAVE_TEMPLATES[idx.min(WAVE_TEMPLATES.len() - 1)];

        commands.spawn((
            Position {
                x: candidate.x,
                y: candidate.y,
            },
            Name(template.name.into()),
            Renderable {
                symbol: template.symbol.into(),
                fg: template.fg,
                bg: RatColor::Black,
            },
            BlocksMovement,
            Hostile,
            Health {
                current: template.health,
                max: template.health,
            },
            CombatStats {
                attack: template.attack,
                defense: template.defense,
            },
            Speed(template.speed),
            Energy(0),
            AiState::Idle,
            Viewshed {
                range: template.sight_range,
                visible_tiles: HashSet::new(),
                revealed_tiles: HashSet::new(),
                dirty: true,
            },
        ));

        spawned += 1;
    }
}
