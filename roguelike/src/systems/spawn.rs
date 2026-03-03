use std::collections::HashSet;

use bevy::prelude::*;

use crate::components::{
    AiLookDir, AiMemory, AiPersonality, AiState, BlocksMovement, Caliber, CombatStats, Energy, ExpReward, Faction, Health, Hostile,
    Inventory, Item, ItemKind, LootTable, Name, PatrolOrigin, Position, Renderable, Speed, Stamina, Viewshed,
};
use crate::grid_vec::GridVec;
use crate::typedefs::RatColor;

// ───────────────────────── Procedural NPC Names ───────────────────

const FIRST_NAMES: &[&str] = &[
    "Silas", "Ezekiel", "Cornelius", "Jebediah", "Obadiah",
    "Elijah", "Caleb", "Josiah", "Amos", "Enoch",
    "Rufus", "Virgil", "Cyrus", "Hector", "Magnus",
    "Bartholomew", "Thaddeus", "Solomon", "Augustus", "Percival",
    "Clayton", "Emmett", "Levi", "Abel", "Jesse",
    "Wyatt", "Morgan", "Hank", "Earl", "Jasper",
];

const NICKNAMES: &[&str] = &[
    "Dusty", "Slim", "Rattlesnake", "Two-Gun", "One-Eye",
    "Whiskey", "Ironjaw", "Red", "Trigger", "Sidewinder",
    "Buckshot", "Tombstone", "Cactus", "Copperhead", "Dynamite",
    "Grizzly", "Hawk", "Longshot", "Maverick", "Sundown",
];

const LAST_NAMES: &[&str] = &[
    "Crowley", "Boone", "Shaw", "Hollister", "Cartwright",
    "McAllister", "Dalton", "Cassidy", "Harlan", "Garrett",
    "Ringo", "Earp", "Masterson", "Holliday", "Calhoun",
    "Braddock", "Pickett", "Stanton", "Thornton", "Wainwright",
];

/// Generates a procedural 1850s cowboy-themed name from position hash.
/// About 30% of NPCs get a nickname (e.g., "Dusty" Silas Crowley).
fn generate_npc_name(x: i32, y: i32) -> String {
    /// Out of 10, how many NPCs receive a nickname prefix.
    const NICKNAME_CHANCE: usize = 3;

    let hash1 = (x.wrapping_mul(7919) ^ y.wrapping_mul(104729)).unsigned_abs() as usize;
    let hash2 = (x.wrapping_mul(1009) ^ y.wrapping_mul(7529)).unsigned_abs() as usize;
    let hash3 = (x.wrapping_mul(2903) ^ y.wrapping_mul(3571)).unsigned_abs() as usize;
    let hash4 = (x.wrapping_mul(5381) ^ y.wrapping_mul(9103)).unsigned_abs() as usize;

    let first = FIRST_NAMES[hash1 % FIRST_NAMES.len()];
    let last = LAST_NAMES[hash2 % LAST_NAMES.len()];

    if hash3 % 10 < NICKNAME_CHANCE {
        let nick = NICKNAMES[hash4 % NICKNAMES.len()];
        format!("\"{nick}\" {first} {last}")
    } else {
        format!("{first} {last}")
    }
}

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
    // Tier 1: Wildlife (lowercase symbols — animals)
    MonsterTemplate { name: "Coyote", symbol: "c", fg: RatColor::Rgb(220, 170, 100), health: 100, attack: 2, defense: 0, speed: 140, sight_range: 6, exp_reward: 3, faction: Faction::Wildlife, ammo: 0 },
    MonsterTemplate { name: "Rattlesnake", symbol: "s", fg: RatColor::Rgb(100, 200, 60), health: 100, attack: 3, defense: 1, speed: 60, sight_range: 8, exp_reward: 5, faction: Faction::Wildlife, ammo: 0 },
    // Tier 2: Outlaws (uppercase symbols — human NPCs)
    MonsterTemplate { name: "Outlaw", symbol: "O", fg: RatColor::Rgb(240, 200, 130), health: 100, attack: 4, defense: 0, speed: 90, sight_range: 8, exp_reward: 8, faction: Faction::Outlaws, ammo: 0 },
    // Tier 3: Vaqueros (uppercase symbols — human NPCs)
    MonsterTemplate { name: "Vaquero", symbol: "V", fg: RatColor::Rgb(180, 200, 80), health: 100, attack: 5, defense: 0, speed: 85, sight_range: 10, exp_reward: 12, faction: Faction::Vaqueros, ammo: 0 },
    // Tier 4: Lawmen (uppercase symbols — human NPCs)
    MonsterTemplate { name: "Cowboy", symbol: "C", fg: RatColor::Rgb(230, 180, 100), health: 100, attack: 6, defense: 0, speed: 80, sight_range: 12, exp_reward: 18, faction: Faction::Lawmen, ammo: 10 },
    // Tier 5: Outlaws - Gunslinger (uppercase symbols — human NPCs)
    MonsterTemplate { name: "Gunslinger", symbol: "G", fg: RatColor::Rgb(255, 80, 80), health: 100, attack: 8, defense: 0, speed: 100, sight_range: 14, exp_reward: 30, faction: Faction::Outlaws, ammo: 15 },
];

/// Spawns a hostile entity from a `MonsterTemplate` at the given position,
/// with optional stat bonuses for wave scaling.
///
/// NPCs with ammo > 0 get an Inventory containing a gun item matching
/// their faction, making their inventory structure identical to the player's.
/// Some NPCs also receive throwable items (dynamite, molotovs).
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

    // Build NPC inventory items.
    let mut inv_items: Vec<Entity> = Vec::new();

    // Deterministic hash based on position for weapon/item assignment.
    let item_hash = (x.wrapping_mul(31) ^ y.wrapping_mul(17)).unsigned_abs();

    // NPCs with ammo get a gun in their inventory (same structure as player).
    if template.ammo > 0 {
        // Use position-based hash to deterministically assign varied weapons.
        // Some NPCs get rifles, some revolvers, from the full period-accurate pool.
        // Weapon damage is equivalent to caliber (.31 = 31 damage, etc.)
        let weapon_pool: &[(&str, Caliber, i32, &str)] = &[
            // (name, caliber, capacity, symbol)
            ("Colt Sheriff", Caliber::Cal36, 5, "p"),
            ("Colt Army", Caliber::Cal44, 6, "p"),
            ("Colt Pocket", Caliber::Cal31, 5, "p"),
            ("Remington New Model Army", Caliber::Cal44, 6, "p"),
            ("Starr 1858 DA", Caliber::Cal44, 6, "p"),
            ("Savage 1856", Caliber::Cal36, 6, "p"),
            ("Adams Revolver", Caliber::Cal44, 5, "p"),
            ("Hawken Rifle", Caliber::Cal50, 1, "r"),
            ("Springfield 1842", Caliber::Cal69, 1, "r"),
            ("Springfield 1855", Caliber::Cal58, 1, "r"),
            ("Enfield 1853", Caliber::Cal577, 1, "r"),
        ];
        let weapon_idx = (item_hash as usize) % weapon_pool.len();
        let (gun_name, caliber, capacity, symbol) = weapon_pool[weapon_idx];
        let gun = commands.spawn((
            Item,
            Name(String::from(gun_name)),
            Renderable {
                symbol: String::from(symbol),
                fg: RatColor::Rgb(140, 140, 160),
                bg: RatColor::Black,
            },
            ItemKind::Gun {
                loaded: template.ammo.min(capacity),
                capacity,
                caliber,
                attack: caliber.damage(),
                name: String::from(gun_name),
            },
        )).id();
        inv_items.push(gun);
    }

    // Deterministic item assignment based on position.
    // Some humanoid NPCs carry throwable items (dynamite or molotovs).
    if !matches!(template.faction, Faction::Wildlife) {
        if item_hash.is_multiple_of(5) {
            let dynamite = commands.spawn((
                Item,
                Name("Dynamite Stick".into()),
                Renderable {
                    symbol: "*".into(),
                    fg: RatColor::Rgb(255, 165, 0),
                    bg: RatColor::Black,
                },
                ItemKind::Grenade { damage: 8, radius: 2 },
            )).id();
            inv_items.push(dynamite);
        } else if item_hash.is_multiple_of(7) {
            let molotov = commands.spawn((
                Item,
                Name("Molotov Cocktail".into()),
                Renderable {
                    symbol: "m".into(),
                    fg: RatColor::Rgb(255, 100, 0),
                    bg: RatColor::Black,
                },
                ItemKind::Molotov { damage: 6, radius: 4 },
            )).id();
            inv_items.push(molotov);
        }
    }

    // Humanoid NPCs get procedurally generated cowboy names.
    // Wildlife keeps its species name.
    let npc_name: String = if matches!(template.faction, Faction::Wildlife) {
        template.name.into()
    } else {
        generate_npc_name(x, y)
    };

    // Compute personality based on faction and position hash.
    let is_ranged = template.ammo > 0;
    let personality_hash = item_hash.wrapping_mul(7) ^ (x.wrapping_add(y)).unsigned_abs();
    let aggression = 0.3 + (personality_hash % 7) as f64 * 0.1; // 0.3 – 0.9
    let courage = if matches!(template.faction, Faction::Wildlife) {
        0.2 // Animals flee easily
    } else {
        0.3 + (personality_hash % 5) as f64 * 0.15 // 0.3 – 0.9
    };
    let preferred_range = if is_ranged { 5 + (personality_hash % 6) as i32 } else { 1 };

    // Humanoid NPCs get a small stamina pool for special actions.
    let npc_stamina = if matches!(template.faction, Faction::Wildlife) {
        0
    } else {
        20 + (scaled_health / 3)
    };

    commands.spawn((
        Position { x, y },
        Name(npc_name),
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
        AiState::Patrolling,
        AiLookDir(GridVec::new(0, -1)), // default: looking south
        PatrolOrigin(GridVec::new(x, y)),
        AiMemory::default(),
        AiPersonality {
            aggression,
            courage,
            preferred_range,
        },
        LootTable { drop_chance },
        ExpReward(template.exp_reward + exp_bonus),
        Viewshed {
            range: template.sight_range,
            visible_tiles: HashSet::new(),
            revealed_tiles: HashSet::new(),
            dirty: true,
        },
        Inventory { items: inv_items },
        Stamina {
            current: npc_stamina,
            max: npc_stamina,
        },
    ));
}
