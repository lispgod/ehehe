use std::collections::HashSet;

use bevy::prelude::*;

use crate::components::{
    AiState, Ammo, BlocksMovement, CombatStats, Energy, ExpReward, Faction, Health, HellGate, Hostile, LootTable, Name,
    Position, Renderable, Speed, Viewshed,
};
use crate::grid_vec::GridVec;
use crate::noise::value_noise;
use crate::resources::{GameMapResource, MapSeed, TurnCounter};
use crate::typedefs::{RatColor, GATE_POINT};

/// Monster archetypes for wave spawning (modern setting, same tiers as initial spawning).
struct WaveMonsterTemplate {
    name: &'static str,
    symbol: &'static str,
    fg: RatColor,
    health: i32,
    attack: i32,
    defense: i32,
    speed: i32,
    sight_range: i32,
    exp_reward: i32,
    faction: Faction,
    /// Ammo supply for ranged attacks. 0 means melee only.
    ammo: i32,
}

const WAVE_TEMPLATES: &[WaveMonsterTemplate] = &[
    // Tier 1: Wildlife
    WaveMonsterTemplate { name: "Coyote", symbol: "c", fg: RatColor::Rgb(160, 120, 80), health: 4, attack: 2, defense: 0, speed: 110, sight_range: 6, exp_reward: 3, faction: Faction::Wildlife, ammo: 0 },
    WaveMonsterTemplate { name: "Rattlesnake", symbol: "~", fg: RatColor::Rgb(60, 100, 40), health: 8, attack: 3, defense: 1, speed: 120, sight_range: 8, exp_reward: 5, faction: Faction::Wildlife, ammo: 0 },
    // Tier 2: Outlaws
    WaveMonsterTemplate { name: "Outlaw", symbol: "o", fg: RatColor::Rgb(194, 178, 128), health: 12, attack: 4, defense: 1, speed: 90, sight_range: 8, exp_reward: 8, faction: Faction::Outlaws, ammo: 0 },
    // Tier 3: Vaqueros
    WaveMonsterTemplate { name: "Vaquero", symbol: "v", fg: RatColor::Rgb(107, 112, 60), health: 15, attack: 5, defense: 2, speed: 85, sight_range: 10, exp_reward: 12, faction: Faction::Vaqueros, ammo: 0 },
    // Tier 4: Cowboys (has ranged attacks)
    WaveMonsterTemplate { name: "Cowboy", symbol: "C", fg: RatColor::Rgb(160, 130, 90), health: 20, attack: 6, defense: 3, speed: 80, sight_range: 12, exp_reward: 18, faction: Faction::Lawmen, ammo: 10 },
    WaveMonsterTemplate { name: "Gunslinger", symbol: "G", fg: RatColor::Rgb(60, 60, 60), health: 28, attack: 8, defense: 4, speed: 100, sight_range: 14, exp_reward: 30, faction: Faction::Outlaws, ammo: 15 },
];

/// Determines which faction tier to spawn based on wave number.
/// Returns (start_index, end_index_exclusive) into WAVE_TEMPLATES.
fn wave_faction(wave_number: u32) -> (usize, usize) {
    match wave_number {
        0..=3 => (0, 2),    // Wildlife: Rat, Feral Dog
        4..=7 => (1, 3),    // Bandits: Feral Dog, Bandit
        8..=12 => (2, 4),   // Scavengers: Bandit, Scavenger
        13..=18 => (3, 5),  // Military: Scavenger, Soldier
        _ => (4, 6),        // Spec Ops: Soldier, Spec Ops
    }
}

/// Minimum distance (squared) from the gate when spawning new wave enemies.
const MIN_WAVE_SPAWN_DIST_SQ: i32 = 2 * 2;

/// Maximum distance (squared) from the gate for wave spawns.
const MAX_WAVE_SPAWN_DIST_SQ: i32 = 8 * 8;

/// How often (in turns) a new wave spawns.
const WAVE_INTERVAL: u32 = 3;

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

/// Spawns waves of enemies emerging from the Hell Gate as turns progress.
///
/// Every `WAVE_INTERVAL` turns, spawns a batch of enemies near the gate.
/// Monsters get progressively stronger as the wave number increases.
/// Spawning stops when the gate is destroyed.
pub fn wave_spawn_system(
    mut commands: Commands,
    turn_counter: Res<TurnCounter>,
    map: Res<GameMapResource>,
    seed: Res<MapSeed>,
    gate_query: Query<(), With<HellGate>>,
    existing_positions: Query<&Position>,
) {
    let turn = turn_counter.0;

    // Only spawn on wave intervals (and not on turn 0).
    if turn == 0 || turn % WAVE_INTERVAL != 0 {
        return;
    }

    // No spawning if the gate has been destroyed.
    if gate_query.is_empty() {
        return;
    }

    let gate_vec = GATE_POINT;

    // Collect occupied positions to avoid stacking entities.
    let occupied: HashSet<GridVec> = existing_positions.iter().map(|p| p.as_grid_vec()).collect();

    // How many enemies to spawn this wave.
    let wave_number = turn / WAVE_INTERVAL;
    let count = WAVE_BASE_COUNT + wave_number.saturating_sub(1) * WAVE_SCALE_PER_CYCLE;

    // Stat scaling: monsters get stronger as waves progress.
    // Bonuses start small and ramp up gradually:
    //   wave 1: +2 HP, +0 atk, +0 def
    //   wave 3: +6 HP, +1 atk, +1 def
    //   wave 6: +12 HP, +3 atk, +2 def
    let health_bonus = (wave_number as i32) * 2;
    let attack_bonus = (wave_number as i32) / 2;
    let defense_bonus = (wave_number as i32) / 3;

    // Use turn-seeded noise for deterministic but varied spawn positions.
    let wave_seed = seed.0.wrapping_add(turn as u64 * WAVE_SEED_MULTIPLIER);
    let mut spawned = 0u32;
    let mut attempt = 0u32;

    while spawned < count && attempt < count * 20 {
        // Generate candidate position using noise near the gate.
        let nx = value_noise(attempt as i32, turn as i32, wave_seed);
        let ny = value_noise(turn as i32, attempt as i32, wave_seed.wrapping_add(Y_AXIS_SEED_OFFSET));

        // Map noise to an offset within the spawn ring around the gate.
        let range = 8; // half-width of the search area
        let dx = (nx * (range * 2) as f64) as i32 - range;
        let dy = (ny * (range * 2) as f64) as i32 - range;
        let candidate = gate_vec + GridVec::new(dx, dy);

        attempt += 1;

        // Check distance constraints from gate.
        let dist_sq = candidate.distance_squared(gate_vec);
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

        // Select monster template from the appropriate faction tier.
        let (faction_start, faction_end) = wave_faction(wave_number);
        let faction_len = faction_end - faction_start;
        let template_noise = value_noise(candidate.x, candidate.y, wave_seed.wrapping_add(TEMPLATE_SEED_OFFSET));
        let idx = faction_start + (template_noise * faction_len as f64) as usize;
        let template = &WAVE_TEMPLATES[idx.min(faction_end - 1)];

        let scaled_health = template.health + health_bonus;
        let scaled_attack = template.attack + attack_bonus;
        let scaled_defense = template.defense + defense_bonus;

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
            template.faction,
            Health {
                current: scaled_health,
                max: scaled_health,
            },
            CombatStats {
                attack: scaled_attack,
                defense: scaled_defense,
            },
            Speed(template.speed),
            Energy(0),
            AiState::Idle,
            LootTable { drop_chance: 0.30 },
            ExpReward(template.exp_reward + (wave_number as i32)),
            Viewshed {
                range: template.sight_range,
                visible_tiles: HashSet::new(),
                revealed_tiles: HashSet::new(),
                dirty: true,
            },
            Ammo {
                current: template.ammo,
                max: template.ammo,
            },
        ));

        spawned += 1;
    }
}
