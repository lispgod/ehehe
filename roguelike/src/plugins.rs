use std::collections::HashSet;

use bevy::prelude::*;

use crate::components::{
    BlocksMovement, Caliber, CameraFollow, CombatStats, Energy,
    Health, Inventory, Item, ItemKind, Outfit, Stamina, Name, Player, Position,
    Renderable, Speed, Viewshed, ACTION_COST,
};
use crate::events::{AiRangedAttackIntent, AttackIntent, DamageEvent, DropItemIntent, MeleeWideIntent, MolotovCastIntent, MoveIntent, PickupItemIntent, RangedAttackIntent, SpellCastIntent, ThrowItemIntent, UseItemIntent};
use crate::gamemap::GameMap;
use crate::grid_vec::GridVec;
use crate::noise::value_noise;
use crate::resources::{
    BloodMap, CameraPosition, Collectibles, CombatLog, CursorPosition, DynamicRng, ExtraWorldTicks, GameMapResource, GameState, InputState,
    KillCount, MapSeed, PendingExp, PendingNpcExp, RestartRequested, SoundEvents, SpectatingAfterDeath, SpatialIndex, SpellParticles, TurnCounter,
    TurnState,
};
use crate::systems::{ai, camera, combat, input, inventory, movement, projectile, render, spawn, spatial_index, spell, turn, visibility};
use crate::systems::spawn::MONSTER_TEMPLATES;
use crate::typedefs::{RatColor, SPAWN_POINT, SPAWN_X, SPAWN_Y};

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
        // Use an existing MapSeed if the user inserted one, otherwise use a
        // time-based seed for a unique experience each playthrough.
        let seed = app
            .world()
            .get_resource::<MapSeed>()
            .map(|s| s.0)
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(42)
            });

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
            .add_message::<MolotovCastIntent>()
            // ── Resources ──
            .insert_resource(MapSeed(seed))
            .insert_resource(GameMapResource(GameMap::new(400, 280, seed)))
            .insert_resource(CameraPosition(SPAWN_POINT))
            .init_resource::<SpatialIndex>()
            .init_resource::<CombatLog>()
            .init_resource::<TurnCounter>()
            .init_resource::<KillCount>()
            .init_resource::<PendingExp>()
            .init_resource::<PendingNpcExp>()
            .init_resource::<SpellParticles>()
            .init_resource::<InputState>()
            .init_resource::<RestartRequested>()
            .init_resource::<CursorPosition>()
            .init_resource::<Collectibles>()
            .init_resource::<ExtraWorldTicks>()
            .init_resource::<SoundEvents>()
            .init_resource::<BloodMap>()
            .init_resource::<SpectatingAfterDeath>()
            .init_resource::<DynamicRng>()
            .init_resource::<crate::resources::GodMode>()
            // ── States ──
            .init_state::<GameState>()
            .add_sub_state::<TurnState>()
            // ── Startup ──
            .add_systems(Startup, (spawn_player, spawn_monsters).chain())
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
            .add_systems(PreUpdate, input::input_system)
            .add_systems(PreUpdate, restart_system)
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
                    movement::cactus_damage_system,
                    movement::dive_stamina_system,
                    inventory::pickup_system,
                    inventory::auto_pickup_system,
                    inventory::use_item_system,
                    inventory::drop_item_system,
                    inventory::throw_system,
                    inventory::reload_system,
                    spell::spell_system,
                    spell::molotov_system,
                    combat::ranged_attack_system,
                    combat::melee_wide_system,
                    combat::combat_system,
                    projectile::projectile_system,
                    combat::apply_damage_system,
                    combat::death_system,
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
            // ── World turn: energy accumulation + AI + action resolution ──
            .add_systems(
                Update,
                (
                    ai::energy_accumulate_system,
                    ai::ai_system,
                    combat::ai_ranged_attack_system,
                    turn::fire_system,
                )
                    .chain()
                    .after(RoguelikeSet::Consequence)
                    .run_if(in_state(TurnState::WorldTurn)),
            )
            .add_systems(
                Update,
                turn::end_world_turn
                    .after(turn::fire_system)
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
fn spawn_player(mut commands: Commands, seed: Res<MapSeed>, map: Res<GameMapResource>) {
    do_spawn_player(&mut commands, seed.0, &map);
}

/// Spawns monsters on passable tiles using deterministic noise placement.
/// Monsters are spawned near the Enemy Stronghold area, using the map seed for
/// deterministic placement.
fn spawn_monsters(mut commands: Commands, map: Res<GameMapResource>, seed: Res<MapSeed>) {
    do_spawn_monsters(&mut commands, &map, seed.0);
}

/// Generates a procedural outfit description for the player character.
/// Uses the map seed to produce a different look each playthrough.
fn generate_outfit(seed: u64) -> String {
    const HATS: &[&str] = &[
        "a dusty Stetson", "a wide-brimmed gambler hat", "a worn felt hat",
        "a battered cavalry hat", "a low-crown Boss of the Plains",
        "a sun-bleached plantation hat", "a creased cattleman hat",
        "no hat — just wind-swept hair",
    ];
    const SHIRTS: &[&str] = &[
        "a faded red flannel shirt", "a collarless muslin pullover",
        "a stained white cotton shirt", "a dark wool vest over a henley",
        "a patched buckskin shirt", "a dusty denim work shirt",
        "a striped calico shirt", "a sweat-soaked chambray shirt",
    ];
    const BOTTOMS: &[&str] = &[
        "canvas trousers held up by suspenders", "worn leather chaps over dungarees",
        "dark wool trousers tucked into boots", "faded denim jeans with frayed cuffs",
        "buckskin leggings", "dust-caked cavalry trousers",
        "patched corduroy pants", "brown cotton work pants",
    ];
    const EXTRAS: &[&str] = &[
        "a sun-faded bandana around the neck", "a leather gun belt slung low",
        "spurs that jingle with every step", "a rawhide lariat coiled at the hip",
        "a tattered serape draped over one shoulder", "a tobacco pouch in the breast pocket",
        "a pocket watch chain glinting at the waist", "dust on every inch of cloth",
    ];

    // Prime multipliers and bit-shifts decorrelate selections across categories.
    let h = (seed.wrapping_mul(7919) >> 3) as usize % HATS.len();
    let s = (seed.wrapping_mul(104729) >> 5) as usize % SHIRTS.len();
    let b = (seed.wrapping_mul(3571) >> 7) as usize % BOTTOMS.len();
    let e = (seed.wrapping_mul(9103) >> 2) as usize % EXTRAS.len();

    format!("Wearing {}, {}, {}, and {}.",
        HATS[h], SHIRTS[s], BOTTOMS[b], EXTRAS[e])
}

/// Helper: spawns the player entity.
fn do_spawn_player(commands: &mut Commands, seed: u64, map: &GameMapResource) {
    // Find a saloon interior tile, falling back to default spawn point.
    let spawn_pos = map.0.find_saloon_interior()
        .unwrap_or(GridVec::new(SPAWN_X, SPAWN_Y));

    // Spawn starting weapon: Colt Pocket (.31 caliber)
    let caliber = Caliber::Cal31;
    let colt_pocket = commands.spawn((
        Item,
        Name("Colt Pocket".into()),
        Renderable {
            symbol: "p".into(),
            fg: RatColor::Rgb(160, 150, 140),
            bg: RatColor::Black,
        },
        ItemKind::Gun {
            loaded: 5,
            capacity: 5,
            caliber,
            attack: caliber.damage(),
            name: "Colt Pocket".into(),
        },
    )).id();

    // Spawn starting knife
    let knife = commands.spawn((
        Item,
        Name("Bowie Knife".into()),
        Renderable {
            symbol: "/".into(),
            fg: RatColor::Rgb(192, 192, 210),
            bg: RatColor::Black,
        },
        ItemKind::Knife { attack: 4 },
    )).id();

    // Spawn starting whiskey
    let whiskey = commands.spawn((
        Item,
        Name("Whiskey Bottle".into()),
        Renderable {
            symbol: "w".into(),
            fg: RatColor::Rgb(180, 120, 60),
            bg: RatColor::Black,
        },
        ItemKind::Whiskey { heal: 10 },
    )).id();

    // Spawn starting molotov cocktail
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

    commands.spawn((
        Position {
            x: spawn_pos.x,
            y: spawn_pos.y,
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
            current: 100,
            max: 100,
        },
        Stamina {
            current: 50,
            max: 50,
        },
        CombatStats {
            attack: 5,
            defense: 0,
        },
        Speed(ACTION_COST),
        Energy(0),
    )).insert((
        Inventory { items: vec![colt_pocket, knife, whiskey, molotov] },
        Outfit(generate_outfit(seed)),
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

    for y in 1..map.0.height - 1 {
        for x in 1..map.0.width - 1 {
            let pos = GridVec::new(x, y);

            if pos.distance_squared(SPAWN_POINT) < min_spawn_dist_sq {
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

            spawn::spawn_monster(commands, template, x, y, 0, 0, 0, 0, 0.25);
        }
    }
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
    (mut extra_ticks, mut blood_map, mut spectating, mut dynamic_rng, mut god_mode): (ResMut<ExtraWorldTicks>, ResMut<BloodMap>, ResMut<SpectatingAfterDeath>, ResMut<DynamicRng>, ResMut<crate::resources::GodMode>),
) {
    if !restart.0 {
        return;
    }
    restart.0 = false;

    for entity in &all_entities {
        commands.entity(entity).despawn();
    }

    combat_log.clear();
    kill_count.0 = 0;
    turn_counter.0 = 0;
    pending_exp.0 = 0;
    spell_particles.particles.clear();
    *input_state = InputState::default();
    camera.0 = SPAWN_POINT;
    *cursor = CursorPosition::default();
    *collectibles = Collectibles::default();
    extra_ticks.0 = 0;
    blood_map.stains.clear();
    spectating.0 = false;
    god_mode.0 = false;
    dynamic_rng.reset();
    *game_map = GameMapResource(GameMap::new(400, 280, seed.0));

    next_game_state.set(GameState::Playing);

    do_spawn_player(&mut commands, seed.0, &game_map);
    do_spawn_monsters(&mut commands, &game_map, seed.0);
}
