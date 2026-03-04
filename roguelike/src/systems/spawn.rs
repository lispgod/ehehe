use std::collections::HashSet;

use bevy::prelude::*;

use crate::components::{
    AiLookDir, AiMemory, AiPersonality, AiState, BlocksMovement, Caliber, CombatStats, Energy, Faction, Health,
    Inventory, Item, ItemKind, LootTable, Name, PatrolOrigin, Position, Renderable, Speed, Stamina, Viewshed,
};
use crate::grid_vec::GridVec;
use crate::typedefs::RatColor;

// ───────────────────────── Procedural NPC Names ───────────────────
//
// All NPC names are single-word for UI readability and variety.
// Each faction has a distinct name generator:
//   - Cowboys (Outlaws/Lawmen): Western first names and surnames
//   - Indians: Compound nature names (e.g., Eaglefoot, SittingBear)
//   - Mexicans (Vaqueros): Spanish names
//   - Sheriff: Famous lawman surnames
//   - Civilians: Nicknames (e.g., Dusty, Slim, Maverick)

/// Cowboy names — used by Outlaws and Lawmen factions.
const COWBOY_NAMES: &[&str] = &[
    "Wyatt", "Morgan", "Hank", "Earl", "Jasper",
    "Clayton", "Emmett", "Levi", "Abel", "Jesse",
    "Silas", "Caleb", "Josiah", "Amos", "Enoch",
    "Rufus", "Virgil", "Cyrus", "Hector", "Magnus",
    "Colt", "Boone", "Clint", "Deacon", "Flint",
    "Gideon", "Harlan", "Knox", "Nash", "Quinn",
    "Redford", "Briggs", "Dalton", "Graves", "Hollis",
    "Larkin", "Mcgraw", "Pickett", "Slade", "Tucker",
];

/// Indian compound names — nature-inspired single words.
const INDIAN_NAMES: &[&str] = &[
    "Eaglefoot", "SittingBear", "RedCloud", "IronHawk", "TallElk",
    "SwiftWolf", "RunningDeer", "LoneBull", "ThunderSky", "SilentArrow",
    "PaintedMoon", "BrokenFeather", "RisingFire", "BurningWind", "HighStar",
    "Chayton", "Takoda", "Nashoba", "Akecheta", "Wahkan",
    "DarkCrow", "WildHorse", "StrongBear", "GreyWolf", "WhiteEagle",
    "Kohana", "Kuruk", "Mato", "Ohanzee", "Hototo",
    "StillWater", "StoneFox", "Blackwind", "Sunhawk", "Crowfoot",
    "Ironbow", "Dustwolf", "Ashfeather", "Deepsky", "Fireheart",
];

/// Mexican names — used by Vaqueros faction.
const MEXICAN_NAMES: &[&str] = &[
    "Carlos", "Miguel", "Diego", "Rafael", "Alejandro",
    "Fernando", "Santiago", "Joaquin", "Esteban", "Mateo",
    "Rodrigo", "Emilio", "Ignacio", "Salvador", "Teodoro",
    "Montoya", "Vega", "Salazar", "Guerrero", "Delgado",
    "Reyes", "Espinoza", "Castillo", "Navarro", "Fuentes",
    "Herrera", "Rojas", "Mendoza", "Coronado", "Cortez",
    "Pancho", "Cisco", "Lobo", "Cruz", "Rico",
    "Bravo", "Diablo", "Fuego", "Oro", "Toro",
];

/// Sheriff names — famous lawman surnames.
const SHERIFF_NAMES: &[&str] = &[
    "Bassett", "Tilghman", "Hickok", "Wallace", "Masterson",
    "Garrett", "Heck", "Reeves", "Selman", "Canton",
    "Plummer", "Allison", "Earp", "Holiday", "Bullock",
    "Horn", "Hardin", "Holliday", "Ringo", "Dillon",
    "Mather", "Stoudenmire", "Courtright", "Breckinridge", "Bridges",
];

/// Civilian nicknames — colorful Wild West nicknames.
const CIVILIAN_NAMES: &[&str] = &[
    "Dusty", "Slim", "Rattlesnake", "Two-Gun", "One-Eye",
    "Whiskey", "Ironjaw", "Red", "Trigger", "Sidewinder",
    "Buckshot", "Tombstone", "Cactus", "Copperhead", "Dynamite",
    "Grizzly", "Hawk", "Longshot", "Maverick", "Sundown",
    "Tex", "Lucky", "Bones", "Doc", "Shorty",
    "Patches", "Rusty", "Ace", "Preacher", "Dutch",
    "Smiley", "Pops", "Gopher", "Yank", "Pepper",
];

/// Generates a single-word faction-appropriate name from position hash.
/// Each faction has its own distinct pool:
///   - Indians: Compound nature names (Eaglefoot, SittingBear)
///   - Vaqueros: Spanish first names or surnames
///   - Sheriff: Famous lawman surnames
///   - Civilians: Wild West nicknames
///   - Outlaws/Lawmen/default: Cowboy first names or surnames
fn generate_npc_name(x: i32, y: i32, faction: &Faction) -> String {
    let hash = (x.wrapping_mul(7919) ^ y.wrapping_mul(104729)).unsigned_abs() as usize;

    let pool = match faction {
        Faction::Indians => INDIAN_NAMES,
        Faction::Vaqueros => MEXICAN_NAMES,
        Faction::Sheriff => SHERIFF_NAMES,
        Faction::Civilians => CIVILIAN_NAMES,
        _ => COWBOY_NAMES,
    };

    pool[hash % pool.len()].to_string()
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
    pub speed: i32,
    pub sight_range: i32,
    pub faction: Faction,
    /// Whether this NPC carries a gun in inventory.
    pub has_gun: bool,
}

/// Shared monster templates used by both initial and wave spawning.
/// NPC speeds are tuned so that movement frequency roughly matches the
/// player's movement rate (1 action per 3 world ticks).
/// - Coyote: 50 (fast — acts ~every 2 ticks)
/// - Rattlesnake: 20 (slow — acts every ~5 ticks)
/// - Outlaw: 34, Vaquero: 32, Cowboy: 30, Gunslinger: 38
pub const MONSTER_TEMPLATES: &[MonsterTemplate] = &[
    // Tier 1: Wildlife (lowercase symbols — animals keep unique glyphs)
    MonsterTemplate { name: "Coyote", symbol: "c", fg: RatColor::Rgb(220, 170, 100), health: 100, attack: 2, speed: 50, sight_range: 6, faction: Faction::Wildlife, has_gun: false },
    MonsterTemplate { name: "Rattlesnake", symbol: "s", fg: RatColor::Rgb(100, 200, 60), health: 100, attack: 3, speed: 20, sight_range: 8, faction: Faction::Wildlife, has_gun: false },
    // Tier 2: Outlaws — all human NPCs use '@', distinguished by faction color (red-orange)
    MonsterTemplate { name: "Outlaw", symbol: "@", fg: RatColor::Rgb(255, 140, 60), health: 100, attack: 4, speed: 34, sight_range: 8, faction: Faction::Outlaws, has_gun: false },
    // Tier 3: Vaqueros — faction color: lime green
    MonsterTemplate { name: "Vaquero", symbol: "@", fg: RatColor::Rgb(140, 220, 60), health: 100, attack: 5, speed: 32, sight_range: 10, faction: Faction::Vaqueros, has_gun: false },
    // Tier 4: Lawmen — faction color: sky blue
    MonsterTemplate { name: "Cowboy", symbol: "@", fg: RatColor::Rgb(100, 180, 255), health: 100, attack: 6, speed: 30, sight_range: 12, faction: Faction::Lawmen, has_gun: true },
    // Tier 5: Outlaws - Gunslinger — same faction color as Outlaws (red-orange)
    MonsterTemplate { name: "Gunslinger", symbol: "@", fg: RatColor::Rgb(255, 100, 50), health: 100, attack: 8, speed: 38, sight_range: 14, faction: Faction::Outlaws, has_gun: true },
    // Tier 6: Civilians — faction color: off-white/off-gray (player is pure white)
    MonsterTemplate { name: "Civilian", symbol: "@", fg: RatColor::Rgb(200, 195, 185), health: 60, attack: 2, speed: 28, sight_range: 8, faction: Faction::Civilians, has_gun: false },
    // Tier 7: Indians — faction color: warm brown
    MonsterTemplate { name: "Indian Brave", symbol: "@", fg: RatColor::Rgb(200, 120, 60), health: 120, attack: 5, speed: 40, sight_range: 12, faction: Faction::Indians, has_gun: false },
    MonsterTemplate { name: "Indian Scout", symbol: "@", fg: RatColor::Rgb(200, 120, 60), health: 80, attack: 4, speed: 45, sight_range: 14, faction: Faction::Indians, has_gun: false },
    // Tier 8: Sheriff and deputies — faction color: gold
    MonsterTemplate { name: "Sheriff", symbol: "@", fg: RatColor::Rgb(255, 215, 0), health: 150, attack: 8, speed: 32, sight_range: 14, faction: Faction::Sheriff, has_gun: true },
    MonsterTemplate { name: "Deputy", symbol: "@", fg: RatColor::Rgb(255, 215, 0), health: 100, attack: 6, speed: 30, sight_range: 12, faction: Faction::Sheriff, has_gun: true },
];

/// Spawns a hostile entity from a `MonsterTemplate` at the given position,
/// with optional stat bonuses for wave scaling.
///
/// NPCs with `has_gun` set get an Inventory containing a gun item matching
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
) -> Entity {
    let scaled_health = template.health + health_bonus;
    let scaled_attack = template.attack + attack_bonus;

    // Build NPC inventory items.
    let mut inv_items: Vec<Entity> = Vec::new();

    // Deterministic hash based on position for weapon/item assignment.
    let item_hash = (x.wrapping_mul(31) ^ y.wrapping_mul(17)).unsigned_abs();

    // NPCs with a gun get one in their inventory (same structure as player).
    if template.has_gun {
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
                loaded: capacity,
                capacity,
                caliber,
                attack: caliber.damage(),
                name: String::from(gun_name),
                blunt_damage: 5,
            },
        )).id();
        inv_items.push(gun);
    }

    // Indians get a bow instead of a gun.
    if matches!(template.faction, Faction::Indians) {
        let bow = commands.spawn((
            Item,
            Name("Bow".into()),
            Renderable {
                symbol: ")".into(),
                fg: RatColor::Rgb(139, 90, 43),
                bg: RatColor::Black,
            },
            ItemKind::Bow { attack: scaled_attack, blunt_damage: 3 },
        )).id();
        inv_items.push(bow);
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
                ItemKind::Grenade { damage: 8, radius: 2, blunt_damage: 3 },
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
                ItemKind::Molotov { damage: 6, radius: 4, blunt_damage: 4 },
            )).id();
            inv_items.push(molotov);
        }
    }

    // Humanoid NPCs get procedurally generated cowboy names.
    // Wildlife keeps its species name.
    let npc_name: String = if matches!(template.faction, Faction::Wildlife) {
        template.name.into()
    } else {
        let base_name = generate_npc_name(x, y, &template.faction);
        // Deterministic profession prefix based on position hash
        let prefix_hash = (x.wrapping_mul(13) ^ y.wrapping_mul(9973)).unsigned_abs() as usize;
        let prefix_roll = prefix_hash % 100;
        let prefix: Option<&str> = match template.faction {
            Faction::Civilians => {
                if prefix_roll < 30 {
                    let prefixes = ["Dr.", "Barman", "Sailor", "Cowboy", "Farmer", "Miner", "Preacher", "Rancher", "Baker", "Tailor"];
                    Some(prefixes[prefix_hash / 100 % prefixes.len()])
                } else { None }
            }
            Faction::Indians => {
                if prefix_roll < 15 {
                    let prefixes = ["Chief", "Brave", "Shaman", "Elder"];
                    Some(prefixes[prefix_hash / 100 % prefixes.len()])
                } else { None }
            }
            Faction::Vaqueros => {
                if prefix_roll < 15 {
                    let prefixes = ["Capitan", "Bandido", "Vaquero", "Don"];
                    Some(prefixes[prefix_hash / 100 % prefixes.len()])
                } else { None }
            }
            Faction::Sheriff => {
                if template.name.contains("Sheriff") {
                    Some("Sheriff")
                } else {
                    Some("Deputy")
                }
            }
            _ => {
                if prefix_roll < 10 {
                    let prefixes = ["Outlaw", "Gunslinger", "Drifter"];
                    Some(prefixes[prefix_hash / 100 % prefixes.len()])
                } else { None }
            }
        };
        match prefix {
            Some(p) => format!("{p} {base_name}"),
            None => base_name,
        }
    };

    // Compute personality based on faction and position hash.
    // AI is highly aggressive — all factions are combat-ready.
    let is_ranged = template.has_gun || matches!(template.faction, Faction::Indians);
    let personality_hash = item_hash.wrapping_mul(7) ^ (x.wrapping_add(y)).unsigned_abs();
    let aggression = 0.7 + (personality_hash % 4) as f64 * 0.075; // 0.7 – 0.925
    let courage = if matches!(template.faction, Faction::Wildlife) {
        0.2 // Animals flee easily
    } else {
        0.7 + (personality_hash % 4) as f64 * 0.075 // 0.7 – 0.925
    };
    let preferred_range = if is_ranged { 3 + (personality_hash % 4) as i32 } else { 1 };

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
        template.faction,
        Health {
            current: scaled_health,
            max: scaled_health,
        },
        CombatStats {
            attack: scaled_attack,
        },
        Speed(template.speed),
        Energy(0),
    )).insert((
        AiState::Idle,
        AiLookDir(GridVec::new(0, -1)), // default: looking south
        PatrolOrigin(GridVec::new(x, y)),
        AiMemory::default(),
        AiPersonality {
            aggression,
            courage,
            preferred_range,
        },
        LootTable,
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
    )).id()
}
