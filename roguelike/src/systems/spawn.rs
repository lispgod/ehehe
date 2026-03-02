use std::collections::HashSet;

use bevy::prelude::*;

use crate::components::{
    AiLookDir, AiState, Ammo, BlocksMovement, CombatStats, Energy, ExpReward, Faction, Health, Hostile,
    LootTable, Name, Position, Renderable, Speed, Viewshed,
};
use crate::grid_vec::GridVec;
use crate::typedefs::RatColor;

/// Monster archetype for procedural spawning.
///
/// Used by both initial map population and wave-based spawning from the gate.
/// Contains all the static data needed to construct a hostile entity.
pub struct MonsterTemplate {
    pub name: &'static str,
    pub symbol: &'static str,
    pub fg: RatColor,
    pub health: i32,
    pub attack: i32,
    pub defense: i32,
    pub speed: i32,
    pub sight_range: i32,
    pub exp_reward: i32,
    pub faction: Faction,
    /// Ammo supply for ranged attacks. 0 means melee only.
    pub ammo: i32,
}

/// Shared monster templates used by both initial and wave spawning.
/// Each enemy type has its own movement speed:
/// - Coyote: 140 (very fast — acts ~1.4x per tick)
/// - Rattlesnake: 60 (slow — acts every ~1.7 ticks)
/// - Outlaw: 90, Vaquero: 85, Cowboy: 80, Gunslinger: 100
pub const MONSTER_TEMPLATES: &[MonsterTemplate] = &[
    // Tier 1: Wildlife
    MonsterTemplate { name: "Coyote", symbol: "c", fg: RatColor::Rgb(220, 170, 100), health: 4, attack: 2, defense: 0, speed: 140, sight_range: 6, exp_reward: 3, faction: Faction::Wildlife, ammo: 0 },
    MonsterTemplate { name: "Rattlesnake", symbol: "~", fg: RatColor::Rgb(100, 200, 60), health: 8, attack: 3, defense: 1, speed: 60, sight_range: 8, exp_reward: 5, faction: Faction::Wildlife, ammo: 0 },
    // Tier 2: Outlaws
    MonsterTemplate { name: "Outlaw", symbol: "o", fg: RatColor::Rgb(240, 200, 130), health: 12, attack: 4, defense: 1, speed: 90, sight_range: 8, exp_reward: 8, faction: Faction::Outlaws, ammo: 0 },
    // Tier 3: Vaqueros
    MonsterTemplate { name: "Vaquero", symbol: "v", fg: RatColor::Rgb(180, 200, 80), health: 15, attack: 5, defense: 2, speed: 85, sight_range: 10, exp_reward: 12, faction: Faction::Vaqueros, ammo: 0 },
    // Tier 4: Lawmen (Cowboys and Sheriffs are Lawmen)
    MonsterTemplate { name: "Cowboy", symbol: "C", fg: RatColor::Rgb(230, 180, 100), health: 20, attack: 6, defense: 3, speed: 80, sight_range: 12, exp_reward: 18, faction: Faction::Lawmen, ammo: 10 },
    // Tier 5: Outlaws - Gunslinger (skilled, high-tier revolver)
    MonsterTemplate { name: "Gunslinger", symbol: "G", fg: RatColor::Rgb(255, 80, 80), health: 28, attack: 8, defense: 4, speed: 100, sight_range: 14, exp_reward: 30, faction: Faction::Outlaws, ammo: 15 },
];

/// Spawns a hostile entity from a `MonsterTemplate` at the given position,
/// with optional stat bonuses for wave scaling.
///
/// This is the single spawn point for all hostile NPCs — both initial map
/// population and wave spawning use this helper, ensuring consistent
/// component bundles.
pub fn spawn_monster(
    commands: &mut Commands,
    template: &MonsterTemplate,
    x: i32,
    y: i32,
    health_bonus: i32,
    attack_bonus: i32,
    defense_bonus: i32,
    exp_bonus: i32,
    drop_chance: f64,
) {
    let scaled_health = template.health + health_bonus;
    let scaled_attack = template.attack + attack_bonus;
    let scaled_defense = template.defense + defense_bonus;

    commands.spawn((
        Position { x, y },
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
    )).insert((
        AiState::Idle,
        AiLookDir(GridVec::new(0, -1)), // default: looking south
        LootTable { drop_chance },
        ExpReward(template.exp_reward + exp_bonus),
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
}
