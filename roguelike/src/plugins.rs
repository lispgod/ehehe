use std::collections::HashSet;

use bevy::prelude::*;

use crate::components::{
    AiState, BlocksMovement, Caliber, CameraFollow, CombatStats, Energy, Experience, ExpReward, Faction, Health, HellGate, Hostile,
    Ammo, Inventory, Item, ItemKind, Level, LootTable, Stamina, Name, Player, Position, Renderable, Speed, Viewshed, ACTION_COST,
};
use crate::events::{AiRangedAttackIntent, AttackIntent, DamageEvent, DropItemIntent, MeleeWideIntent, MoveIntent, PickupItemIntent, RangedAttackIntent, SpellCastIntent, ThrowItemIntent, UseItemIntent};
use crate::gamemap::GameMap;
use crate::grid_vec::GridVec;
use crate::noise::value_noise;
use crate::resources::{
    CameraPosition, Collectibles, CombatLog, CursorPosition, GameMapResource, GameState, InputState,
    KillCount, MapSeed, PendingExp, RestartRequested, SpatialIndex, SpellParticles, TurnCounter,
    TurnState,
};
use crate::systems::{ai, camera, combat, corruption, input, inventory, movement, projectile, render, spatial_index, spell, turn, visibility, wave_spawn};
use crate::typedefs::{RatColor, SPAWN_POINT, SPAWN_X, SPAWN_Y, GATE_POINT, GATE_X, GATE_Y};

// ─────────────────────────── System Sets ───────────────────────────

/// Top-level system ordering for the roguelike.
///
/// ```text
///   Index → Action → Consequence → Render
/// ```
///
/// - **Index**: rebuild the spatial index (runs unconditionally).
/// - **Action**: process player and NPC actions (movement, combat).
/// - **Consequence**: recalculate derived state (FOV, camera).
/// - **Render**: draw the frame (runs unconditionally).
#[derive(SystemSet, Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub enum RoguelikeSet {
    /// Rebuild spatial index so later systems have O(1) position lookups.
    Index,
    /// Process intents — movement, combat, AI.
    Action,
    /// Recalculate derived state — visibility, camera follow.
    Consequence,
    /// Draw the frame to the terminal.
    Render,
}

// ─────────────────────────── Plugin ────────────────────────────────

/// Bevy plugin that registers all roguelike ECS systems, resources, and
/// startup logic. Adding this plugin is the only step needed to wire up the
/// game — `main.rs` stays minimal.
pub struct RoguelikePlugin;

impl Plugin for RoguelikePlugin {
    fn build(&self, app: &mut App) {
        // Use an existing MapSeed if the user inserted one, otherwise default.
        let seed = app
            .world()
            .get_resource::<MapSeed>()
            .map(|s| s.0)
            .unwrap_or(42);

        app.add_plugins(bevy::state::app::StatesPlugin)
            // ── Messages ──
            .add_message::<MoveIntent>()
            .add_message::<AttackIntent>()
            .add_message::<DamageEvent>()
            .add_message::<SpellCastIntent>()
            .add_message::<UseItemIntent>()
            .add_message::<PickupItemIntent>()
            .add_message::<RangedAttackIntent>()
            .add_message::<MeleeWideIntent>()
            .add_message::<AiRangedAttackIntent>()
            .add_message::<DropItemIntent>()
            .add_message::<ThrowItemIntent>()
            // ── Resources ──
            .insert_resource(MapSeed(seed))
            .insert_resource(GameMapResource(GameMap::new(120, 80, seed)))
            .insert_resource(CameraPosition(SPAWN_POINT))
            .init_resource::<SpatialIndex>()
            .init_resource::<CombatLog>()
            .init_resource::<TurnCounter>()
            .init_resource::<KillCount>()
            .init_resource::<PendingExp>()
            .init_resource::<SpellParticles>()
            .init_resource::<InputState>()
            .init_resource::<RestartRequested>()
            .init_resource::<CursorPosition>()
            .init_resource::<Collectibles>()
            // ── States ──
            .init_state::<GameState>()
            .add_sub_state::<TurnState>()
            // ── Startup ──
            .add_systems(Startup, (spawn_player, spawn_monsters, spawn_hell_gate).chain())
            // ── System-set ordering ──
            .configure_sets(
                Update,
                (
                    RoguelikeSet::Index,
                    RoguelikeSet::Action,
                    RoguelikeSet::Consequence,
                    RoguelikeSet::Render,
                )
                    .chain(),
            )
            // ── Input (PreUpdate — emits intents before Update processes them) ──
            .add_systems(PreUpdate, (input::input_system, restart_system).chain())
            // ── Index (always runs) ──
            .add_systems(
                Update,
                spatial_index::spatial_index_system.in_set(RoguelikeSet::Index),
            )
            // ── Action (gated on Playing state) ──
            .add_systems(
                Update,
                (
                    movement::movement_system,
                    inventory::pickup_system,
                    inventory::auto_pickup_system,
                    inventory::use_item_system,
                    inventory::drop_item_system,
                    inventory::throw_system,
                    inventory::reload_system,
                    spell::spell_system,
                    combat::ranged_attack_system,
                    combat::melee_wide_system,
                    combat::combat_system,
                    projectile::projectile_system,
                    combat::apply_damage_system,
                    combat::death_system,
                    combat::level_up_system,
                )
                    .chain()
                    .in_set(RoguelikeSet::Action)
                    .run_if(in_state(GameState::Playing)),
            )
            // ── Consequence (gated on Playing state) ──
            .add_systems(
                Update,
                (
                    visibility::visibility_system,
                    camera::camera_follow_system,
                )
                    .chain()
                    .in_set(RoguelikeSet::Consequence)
                    .run_if(in_state(GameState::Playing)),
            )
            // ── Turn transitions (gated on specific turn phases) ──
            .add_systems(
                Update,
                turn::end_player_turn
                    .after(RoguelikeSet::Consequence)
                    .run_if(in_state(TurnState::PlayerTurn)),
            )
            // ── World turn: energy accumulation + AI + wave spawning + corruption + action resolution ──
            .add_systems(
                Update,
                (
                    ai::energy_accumulate_system,
                    ai::ai_system,
                    combat::ai_ranged_attack_system,
                    wave_spawn::wave_spawn_system,
                    corruption::corruption_system,
                )
                    .chain()
                    .after(RoguelikeSet::Consequence)
                    .run_if(in_state(TurnState::WorldTurn)),
            )
            .add_systems(
                Update,
                turn::end_world_turn
                    .after(corruption::corruption_system)
                    .run_if(in_state(TurnState::WorldTurn)),
            )
            // ── Render (always runs — shows PAUSED overlay when paused) ──
            .add_systems(
                Update,
                (render::cursor_blink_system, render::particle_tick_system)
                    .in_set(RoguelikeSet::Render),
            )
            .add_systems(
                Update,
                render::draw_system
                    .in_set(RoguelikeSet::Render)
                    .after(render::cursor_blink_system)
                    .after(render::particle_tick_system),
            );
    }
}

/// Spawns the player entity with all required ECS components.
fn spawn_player(mut commands: Commands) {
    do_spawn_player(&mut commands);
}

/// Monster archetypes for procedural spawning (modern setting).
struct MonsterTemplate {
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

const MONSTER_TEMPLATES: &[MonsterTemplate] = &[
    // Tier 1: Wildlife
    MonsterTemplate { name: "Coyote", symbol: "c", fg: RatColor::Rgb(160, 120, 80), health: 4, attack: 2, defense: 0, speed: 110, sight_range: 6, exp_reward: 3, faction: Faction::Wildlife, ammo: 0 },
    MonsterTemplate { name: "Rattlesnake", symbol: "~", fg: RatColor::Rgb(60, 100, 40), health: 8, attack: 3, defense: 1, speed: 120, sight_range: 8, exp_reward: 5, faction: Faction::Wildlife, ammo: 0 },
    // Tier 2: Outlaws
    MonsterTemplate { name: "Outlaw", symbol: "o", fg: RatColor::Rgb(194, 178, 128), health: 12, attack: 4, defense: 1, speed: 90, sight_range: 8, exp_reward: 8, faction: Faction::Outlaws, ammo: 0 },
    // Tier 3: Vaqueros
    MonsterTemplate { name: "Vaquero", symbol: "v", fg: RatColor::Rgb(107, 112, 60), health: 15, attack: 5, defense: 2, speed: 85, sight_range: 10, exp_reward: 12, faction: Faction::Vaqueros, ammo: 0 },
    // Tier 4: Cowboys (has ranged attacks)
    MonsterTemplate { name: "Cowboy", symbol: "C", fg: RatColor::Rgb(160, 130, 90), health: 20, attack: 6, defense: 3, speed: 80, sight_range: 12, exp_reward: 18, faction: Faction::Cowboys, ammo: 10 },
    MonsterTemplate { name: "Gunslinger", symbol: "G", fg: RatColor::Rgb(60, 60, 60), health: 28, attack: 8, defense: 4, speed: 100, sight_range: 14, exp_reward: 30, faction: Faction::Cowboys, ammo: 15 },
];

/// Spawns monsters on passable tiles using deterministic noise placement.
/// Monsters are spawned near the Enemy Stronghold area, using the map seed for
/// deterministic placement.
fn spawn_monsters(mut commands: Commands, map: Res<GameMapResource>, seed: Res<MapSeed>) {
    do_spawn_monsters(&mut commands, &map, seed.0);
}

/// Spawns the Enemy Stronghold entity — the main objective the player must destroy to win.
///
/// The stronghold is a destructible, hostile entity with high health that blocks movement.
/// Enemies emerge from it each wave, and the surrounding land corrupts over time.
fn spawn_hell_gate(mut commands: Commands) {
    do_spawn_hell_gate(&mut commands);
}

/// Helper: spawns the player entity.
fn do_spawn_player(commands: &mut Commands) {
    // Spawn starting weapon: Colt Navy
    let colt_navy = commands.spawn((
        Item,
        Name("Colt Navy".into()),
        Renderable {
            symbol: "P".into(),
            fg: RatColor::Rgb(140, 140, 160),
            bg: RatColor::Black,
        },
        ItemKind::Gun {
            loaded: 6,
            capacity: 6,
            caliber: Caliber::Cal36,
            attack: 5,
            name: "Colt Navy".into(),
        },
    )).id();

    commands.spawn((
        Position {
            x: SPAWN_X,
            y: SPAWN_Y,
        },
        Player,
        Name("Player".into()),
        Renderable {
            symbol: "@".into(),
            fg: RatColor::White,
            bg: RatColor::Black,
        },
        CameraFollow,
        BlocksMovement,
        Health {
            current: 30,
            max: 30,
        },
        Stamina {
            current: 50,
            max: 50,
        },
        Ammo {
            current: 30,
            max: 30,
        },
        CombatStats {
            attack: 5,
            defense: 2,
        },
        Speed(ACTION_COST),
        Energy(0),
    )).insert((
        Inventory { items: vec![colt_navy] },
        Level(1),
        Experience {
            current: 0,
            next_level: 20,
        },
        Viewshed {
            range: 40,
            visible_tiles: HashSet::new(),
            revealed_tiles: HashSet::new(),
            dirty: true,
        },
    ));
}

/// Helper: spawns monsters on passable tiles using deterministic noise placement.
fn do_spawn_monsters(commands: &mut Commands, map: &GameMapResource, seed: u64) {
    let spawn_seed = seed.wrapping_add(54321);
    let template_seed = seed.wrapping_add(98765);
    let min_spawn_dist_sq = 12 * 12;
    let gate_exclusion_sq = 3 * 3;

    for y in 1..map.0.height - 1 {
        for x in 1..map.0.width - 1 {
            let pos = GridVec::new(x, y);

            if pos.distance_squared(SPAWN_POINT) < min_spawn_dist_sq {
                continue;
            }
            if pos.distance_squared(GATE_POINT) < gate_exclusion_sq {
                continue;
            }
            if !map.0.is_passable(&pos) {
                continue;
            }

            let noise = value_noise(x, y, spawn_seed);
            if noise > 0.02 {
                continue;
            }

            let template_noise = value_noise(x, y, template_seed);
            let idx = (template_noise * MONSTER_TEMPLATES.len() as f64) as usize;
            let template = &MONSTER_TEMPLATES[idx.min(MONSTER_TEMPLATES.len() - 1)];

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
                LootTable { drop_chance: 0.25 },
                ExpReward(template.exp_reward),
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
    }
}

/// Helper: spawns the Enemy Stronghold entity.
fn do_spawn_hell_gate(commands: &mut Commands) {
    commands.spawn((
        Position {
            x: GATE_X,
            y: GATE_Y,
        },
        HellGate,
        Hostile,
        Name("Outlaw Hideout".into()),
        Renderable {
            symbol: "Ω".into(),
            fg: RatColor::Rgb(255, 0, 0),
            bg: RatColor::Rgb(80, 0, 0),
        },
        BlocksMovement,
        Health {
            current: 100,
            max: 100,
        },
        CombatStats {
            attack: 0,
            defense: 3,
        },
    ));
}

/// System that handles game restart by despawning all entities and re-spawning.
fn restart_system(
    mut commands: Commands,
    mut restart: ResMut<RestartRequested>,
    all_entities: Query<Entity>,
    mut combat_log: ResMut<CombatLog>,
    mut kill_count: ResMut<KillCount>,
    mut turn_counter: ResMut<TurnCounter>,
    mut pending_exp: ResMut<PendingExp>,
    mut spell_particles: ResMut<SpellParticles>,
    mut input_state: ResMut<InputState>,
    mut next_game_state: ResMut<NextState<GameState>>,
    seed: Res<MapSeed>,
    mut game_map: ResMut<GameMapResource>,
    mut camera: ResMut<CameraPosition>,
    mut cursor: ResMut<CursorPosition>,
    mut collectibles: ResMut<Collectibles>,
) {
    if !restart.0 {
        return;
    }
    restart.0 = false;

    for entity in &all_entities {
        commands.entity(entity).despawn();
    }

    combat_log.messages.clear();
    kill_count.0 = 0;
    turn_counter.0 = 0;
    pending_exp.0 = 0;
    spell_particles.particles.clear();
    *input_state = InputState::default();
    camera.0 = SPAWN_POINT;
    *cursor = CursorPosition::default();
    *collectibles = Collectibles::default();
    *game_map = GameMapResource(GameMap::new(120, 80, seed.0));

    next_game_state.set(GameState::Playing);

    do_spawn_player(&mut commands);
    do_spawn_monsters(&mut commands, &game_map, seed.0);
    do_spawn_hell_gate(&mut commands);
}
