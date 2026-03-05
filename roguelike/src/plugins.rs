use std::collections::HashSet;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::components::{
    BlocksMovement, Caliber, CameraFollow, CombatStats, Energy, Faction,
    Health, Inventory, Item, ItemKind, Stamina, Name, Player, Position,
    Renderable, Speed, Viewshed, ACTION_COST,
};
use crate::events::{AiRangedAttackIntent, AttackIntent, DamageEvent, MeleeWideIntent, MolotovCastIntent, MoveIntent, PickupItemIntent, RangedAttackIntent, SpellCastIntent, ThrowItemIntent, UseItemIntent};
use crate::gamemap::GameMap;
use crate::grid_vec::GridVec;
use crate::noise::value_noise;
use crate::resources::{
    BloodMap, CameraPosition, Collectibles, CombatLog, CursorPosition, DynamicRng, ExtraWorldTicks, GameMapResource, GameState, InputState,
    KillCount, MapSeed, RestartRequested, SoundEvents, SpectatingAfterDeath, SpatialIndex, SpellParticles, TurnCounter,
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
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
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

// ─────────────────────────── Plugins ───────────────────────────────
//
// Following the Bevy plugin best practice (see `examples/app/plugin.rs`),
// the game is split into domain-specific plugins. Each plugin owns the
// systems for a single responsibility. `RoguelikePlugin` composes them
// all together so `main.rs` stays minimal.

/// Top-level Bevy plugin. Registers resources, messages, states, startup
/// systems, and adds all domain sub-plugins.
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

        let game_map = GameMap::new(400, 280, seed);
        // Compute actual player spawn position so camera+cursor start centered on it.
        let player_spawn = game_map.find_bridge_center()
            .unwrap_or(GridVec::new(SPAWN_X, SPAWN_Y));

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
            .add_message::<ThrowItemIntent>()
            .add_message::<MolotovCastIntent>()
            // ── Resources ──
            .insert_resource(MapSeed(seed))
            .insert_resource(GameMapResource(game_map))
            .insert_resource(CameraPosition(player_spawn))
            .init_resource::<SpatialIndex>()
            .init_resource::<CombatLog>()
            .init_resource::<TurnCounter>()
            .init_resource::<KillCount>()
            .init_resource::<SpellParticles>()
            .init_resource::<InputState>()
            .init_resource::<RestartRequested>()
            .insert_resource(CursorPosition::at(player_spawn))
            .init_resource::<Collectibles>()
            .init_resource::<ExtraWorldTicks>()
            .init_resource::<SoundEvents>()
            .init_resource::<BloodMap>()
            .init_resource::<SpectatingAfterDeath>()
            .init_resource::<DynamicRng>()
            .init_resource::<crate::resources::GodMode>()
            .init_resource::<crate::resources::StarLevel>()
            .init_resource::<crate::resources::PropHealth>()
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
            // ── Domain sub-plugins ──
            .add_plugins((
                InputPlugin,
                ActionPlugin,
                WorldPlugin,
                ViewPlugin,
            ));
    }
}

/// Handles player input and game restart logic.
/// Runs in `PreUpdate` so intents are emitted before `Update` processes them.
struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, input::input_system)
            .add_systems(PreUpdate, restart_system);
    }
}

/// Processes gameplay actions: movement, combat, inventory, spells, and
/// projectiles. All systems are gated on `GameState::Playing`.
struct ActionPlugin;

impl Plugin for ActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
                Update,
                spatial_index::spatial_index_system.in_set(RoguelikeSet::Index),
            )
            .add_systems(
                Update,
                (
                    movement::movement_system,
                    movement::cactus_damage_system,
                    movement::dive_stamina_system,
                    inventory::pickup_system,
                    inventory::auto_pickup_system,
                    inventory::use_item_system,
                    inventory::throw_system,
                    inventory::reload_system,
                    spell::spell_system,
                    spell::molotov_system,
                    spell::water_bucket_system,
                    combat::ranged_attack_system,
                    combat::melee_wide_system,
                    combat::combat_system,
                    projectile::projectile_system,
                )
                    .chain()
                    .in_set(RoguelikeSet::Action)
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(
                Update,
                (
                    spell::explosive_projectile_system,
                    combat::apply_damage_system,
                    combat::death_system,
                    movement::victory_check_system,
                    movement::water_slowdown_system,
                )
                    .chain()
                    .after(projectile::projectile_system)
                    .in_set(RoguelikeSet::Action)
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

/// Manages turn state transitions, fire spreading, star level decay, and
/// AI behaviour. Turn-phase systems are gated on their respective sub-states.
struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
                Update,
                turn::end_player_turn_system
                    .after(RoguelikeSet::Consequence)
                    .run_if(in_state(TurnState::PlayerTurn)),
            )
            .add_systems(
                Update,
                (
                    ai::energy_accumulate_system,
                    ai::ai_system,
                    combat::ai_ranged_attack_system,
                    turn::fire_system,
                    turn::star_level_system,
                )
                    .chain()
                    .after(RoguelikeSet::Consequence)
                    .run_if(in_state(TurnState::WorldTurn)),
            )
            .add_systems(
                Update,
                turn::end_world_turn_system
                    .after(turn::star_level_system)
                    .run_if(in_state(TurnState::WorldTurn)),
            );
    }
}

/// Visibility, camera tracking, and terminal rendering.
/// Consequence systems are gated on `GameState::Playing`; render systems
/// run unconditionally to show overlays (pause, death, victory screens).
struct ViewPlugin;

impl Plugin for ViewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
                Update,
                (
                    visibility::visibility_system,
                    camera::camera_follow_system,
                )
                    .chain()
                    .in_set(RoguelikeSet::Consequence)
                    .run_if(in_state(GameState::Playing)),
            )
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
fn spawn_player(mut commands: Commands, map: Res<GameMapResource>) {
    do_spawn_player(&mut commands, &map);
}

/// Spawns monsters on passable tiles using deterministic noise placement.
/// Monsters are spawned near the Enemy Stronghold area, using the map seed for
/// deterministic placement.
fn spawn_monsters(mut commands: Commands, map: Res<GameMapResource>, seed: Res<MapSeed>) {
    do_spawn_monsters(&mut commands, &map, seed.0);
}

/// Helper: spawns the player entity.
fn do_spawn_player(commands: &mut Commands, map: &GameMapResource) {
    // Find a saloon interior tile, falling back to default spawn point.
    let spawn_pos = map.0.find_bridge_center()
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
            blunt_damage: 5,
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
        ItemKind::Knife { attack: 4, blunt_damage: 6 },
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
        ItemKind::Whiskey { heal: 10, blunt_damage: 4 },
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
        ItemKind::Molotov { damage: 6, radius: 4, blunt_damage: 4 },
    )).id();

    // Spawn starting water bucket
    let water_bucket = commands.spawn((
        Item,
        Name("Water Bucket".into()),
        Renderable {
            symbol: "u".into(),
            fg: RatColor::Rgb(100, 150, 255),
            bg: RatColor::Black,
        },
        ItemKind::WaterBucket { uses: 3, radius: 2, blunt_damage: 3 },
    )).id();

    commands.spawn((
        Position {
            x: spawn_pos.x,
            y: spawn_pos.y,
        },
        Player,
        Name("Rogue".into()),
        Renderable {
            symbol: "@".into(),
            fg: RatColor::White,
            bg: RatColor::Black,
        },
        CameraFollow,
        BlocksMovement,
        Faction::Civilians,
        Health {
            current: 100,
            max: 100,
        },
        Stamina {
            current: 100,
            max: 100,
        },
        CombatStats {
            attack: 5,
        },
        Speed(ACTION_COST),
        Energy(0),
    )).insert((
        Inventory { items: vec![colt_pocket, knife, whiskey, molotov, water_bucket] },
        Viewshed {
            range: 40,
            visible_tiles: HashSet::new(),
            revealed_tiles: HashSet::new(),
            dirty: true,
        },
    ));
}

/// Helper: spawns NPCs in faction groups near the first bridge.
/// Indians on the left bank, Mexicans/Vaqueros on the right bank,
/// plus Civilians and Sheriffs near their faction anchor buildings.
fn do_spawn_monsters(commands: &mut Commands, map: &GameMapResource, seed: u64) {
    let group_seed = seed.wrapping_add(54321);
    let min_spawn_dist_sq = 10 * 10; // keep clear zone around player spawn

    // Find bridge center for faction placement
    let bridge_center = map.0.find_bridge_center()
        .unwrap_or(GridVec::new(map.0.width / 2, map.0.height / 2));
    let river_cx = map.0.river_center_x(bridge_center.y) as i32;
    let bridge_y = bridge_center.y;

    // ── Indians on left bank (x < river_center), facing right ──────
    let indian_templates: &[usize] = &[2, 3]; // Indian Brave, Indian Scout
    let mut indian_count = 0;
    let target_indians = 8 + (value_noise(bridge_center.x, bridge_center.y, group_seed.wrapping_add(1111)) * 4.0) as i32;
    for dy in -15i32..=15 {
        for dx in -15i32..=-1 {
            if indian_count >= target_indians { break; }
            let pos = GridVec::new(river_cx + dx, bridge_y + dy);
            if pos.distance_squared(SPAWN_POINT) < min_spawn_dist_sq { continue; }
            if !map.0.is_spawnable(&pos) { continue; }
            // Only spawn on Beach or Sand/Dirt tiles (not water)
            let ok_floor = map.0.get_voxel_at(&pos).is_some_and(|v| {
                matches!(v.floor, Some(crate::typeenums::Floor::Beach)
                    | Some(crate::typeenums::Floor::Sand)
                    | Some(crate::typeenums::Floor::Dirt)
                    | Some(crate::typeenums::Floor::Gravel)
                    | Some(crate::typeenums::Floor::Grass)
                    | Some(crate::typeenums::Floor::Sidewalk)
                    | Some(crate::typeenums::Floor::Bridge))
            });
            if !ok_floor { continue; }
            let tile_noise = value_noise(pos.x, pos.y, group_seed.wrapping_add(22222));
            if tile_noise > 0.40 { continue; }
            let template_idx = indian_templates[(indian_count as usize) % indian_templates.len()];
            let template = &MONSTER_TEMPLATES[template_idx];
            let ent = spawn::spawn_monster(commands, template, pos.x, pos.y, 0, 0);
            // Indians look right (toward Mexicans)
            commands.entity(ent).insert(crate::components::AiLookDir(GridVec::new(1, 0)));
            indian_count += 1;
        }
        if indian_count >= target_indians { break; }
    }

    // ── Mexicans/Vaqueros on right bank (x > river_center), facing left ──
    let vaquero_templates: &[usize] = &[0]; // Vaquero
    let mut vaquero_count = 0;
    let target_vaqueros = 8 + (value_noise(bridge_center.x + 1, bridge_center.y, group_seed.wrapping_add(2222)) * 4.0) as i32;
    for dy in -15i32..=15 {
        for dx in 1i32..=15 {
            if vaquero_count >= target_vaqueros { break; }
            let pos = GridVec::new(river_cx + dx, bridge_y + dy);
            if pos.distance_squared(SPAWN_POINT) < min_spawn_dist_sq { continue; }
            if !map.0.is_spawnable(&pos) { continue; }
            let ok_floor = map.0.get_voxel_at(&pos).is_some_and(|v| {
                matches!(v.floor, Some(crate::typeenums::Floor::Beach)
                    | Some(crate::typeenums::Floor::Sand)
                    | Some(crate::typeenums::Floor::Dirt)
                    | Some(crate::typeenums::Floor::Gravel)
                    | Some(crate::typeenums::Floor::Grass)
                    | Some(crate::typeenums::Floor::Sidewalk)
                    | Some(crate::typeenums::Floor::Bridge))
            });
            if !ok_floor { continue; }
            let tile_noise = value_noise(pos.x, pos.y, group_seed.wrapping_add(33333));
            if tile_noise > 0.40 { continue; }
            let template_idx = vaquero_templates[(vaquero_count as usize) % vaquero_templates.len()];
            let template = &MONSTER_TEMPLATES[template_idx];
            let ent = spawn::spawn_monster(commands, template, pos.x, pos.y, 0, 0);
            // Vaqueros look left (toward Indians)
            commands.entity(ent).insert(crate::components::AiLookDir(GridVec::new(-1, 0)));
            vaquero_count += 1;
        }
        if vaquero_count >= target_vaqueros { break; }
    }

    // ── Civilians near town buildings (from faction_anchors) ────────
    let anchor_radius = 8i32;
    for (anchor_pos, faction, _name) in &map.0.faction_anchors {
        let templates: &[usize] = match faction {
            crate::components::Faction::Civilians => &[1],
            _ => continue,
        };
        let base_size = 3 + (value_noise(anchor_pos.x, anchor_pos.y, group_seed.wrapping_add(44444)) * 4.0) as i32;
        let mut spawned = 0;
        for dy in -anchor_radius..=anchor_radius {
            for dx in -anchor_radius..=anchor_radius {
                if spawned >= base_size { break; }
                let pos = GridVec::new(anchor_pos.x + dx, anchor_pos.y + dy);
                if pos.distance_squared(SPAWN_POINT) < min_spawn_dist_sq { continue; }
                if !map.0.is_spawnable(&pos) { continue; }
                let tile_noise = value_noise(pos.x, pos.y, group_seed.wrapping_add(55555));
                if tile_noise > 0.40 { continue; }
                let template_idx = templates[(spawned as usize) % templates.len()];
                let template = &MONSTER_TEMPLATES[template_idx];
                spawn::spawn_monster(commands, template, pos.x, pos.y, 0, 0);
                spawned += 1;
            }
            if spawned >= base_size { break; }
        }
    }

    // ── Sheriffs near sheriff office buildings ──────────────────────
    for (anchor_pos, faction, _name) in &map.0.faction_anchors {
        let templates: &[usize] = match faction {
            crate::components::Faction::Sheriff => &[4, 5],
            _ => continue,
        };
        let base_size = 2 + (value_noise(anchor_pos.x, anchor_pos.y, group_seed.wrapping_add(66666)) * 3.0) as i32;
        let mut spawned = 0;
        for dy in -anchor_radius..=anchor_radius {
            for dx in -anchor_radius..=anchor_radius {
                if spawned >= base_size { break; }
                let pos = GridVec::new(anchor_pos.x + dx, anchor_pos.y + dy);
                if pos.distance_squared(SPAWN_POINT) < min_spawn_dist_sq { continue; }
                if !map.0.is_spawnable(&pos) { continue; }
                let tile_noise = value_noise(pos.x, pos.y, group_seed.wrapping_add(77777));
                if tile_noise > 0.40 { continue; }
                let template_idx = templates[(spawned as usize) % templates.len()];
                let template = &MONSTER_TEMPLATES[template_idx];
                spawn::spawn_monster(commands, template, pos.x, pos.y, 0, 0);
                spawned += 1;
            }
            if spawned >= base_size { break; }
        }
    }

    // ── Flavor: scatter a few of each faction around the map ───────
    let scatter_seed = group_seed.wrapping_add(99999);
    let scatter_factions: &[(usize, u64)] = &[
        (2, 1), (3, 2), // Indians
        (0, 3),         // Vaqueros
        (1, 4),         // Civilians
        (4, 5),         // Sheriff
    ];
    for &(template_idx, offset) in scatter_factions {
        let template = &MONSTER_TEMPLATES[template_idx];
        let mut placed = 0;
        let max_place = 1 + (value_noise(0, offset as i32, scatter_seed) * 2.0) as i32;
        for attempt in 0..200 {
            if placed >= max_place { break; }
            let ax = (value_noise(attempt, offset as i32, scatter_seed.wrapping_add(1)) * map.0.width as f64) as i32;
            let ay = (value_noise(offset as i32, attempt, scatter_seed.wrapping_add(2)) * map.0.height as f64) as i32;
            let pos = GridVec::new(ax.clamp(1, map.0.width - 2), ay.clamp(1, map.0.height - 2));
            if pos.distance_squared(SPAWN_POINT) < min_spawn_dist_sq { continue; }
            if !map.0.is_spawnable(&pos) { continue; }
            spawn::spawn_monster(commands, template, pos.x, pos.y, 0, 0);
            placed += 1;
        }
    }
}

/// Bundles all mutable resources needed by `restart_system` into a single
/// `SystemParam`, following the Bevy best practice of using `#[derive(SystemParam)]`
/// to stay under the 16-parameter system limit (see `examples/ecs/ecs_guide.rs`).
#[derive(SystemParam)]
struct RestartResources<'w> {
    combat_log: ResMut<'w, CombatLog>,
    kill_count: ResMut<'w, KillCount>,
    turn_counter: ResMut<'w, TurnCounter>,
    spell_particles: ResMut<'w, SpellParticles>,
    input_state: ResMut<'w, InputState>,
    next_game_state: ResMut<'w, NextState<GameState>>,
    seed: ResMut<'w, MapSeed>,
    game_map: ResMut<'w, GameMapResource>,
    camera: ResMut<'w, CameraPosition>,
    cursor: ResMut<'w, CursorPosition>,
    collectibles: ResMut<'w, Collectibles>,
    extra_ticks: ResMut<'w, ExtraWorldTicks>,
    blood_map: ResMut<'w, BloodMap>,
    spectating: ResMut<'w, SpectatingAfterDeath>,
    dynamic_rng: ResMut<'w, DynamicRng>,
    god_mode: ResMut<'w, crate::resources::GodMode>,
    star_level: ResMut<'w, crate::resources::StarLevel>,
    prop_health: ResMut<'w, crate::resources::PropHealth>,
}

/// System that handles game restart by despawning all entities and re-spawning.
fn restart_system(
    mut commands: Commands,
    mut restart: ResMut<RestartRequested>,
    all_entities: Query<Entity>,
    mut res: RestartResources,
) {
    if !restart.0 {
        return;
    }
    restart.0 = false;

    for entity in &all_entities {
        commands.entity(entity).despawn();
    }

    // Generate a new seed each restart so the map is different every time.
    let new_seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(res.seed.0.wrapping_add(1));
    res.seed.0 = new_seed;

    res.combat_log.clear();
    res.kill_count.0 = 0;
    res.turn_counter.0 = 0;
    res.spell_particles.particles.clear();
    *res.input_state = InputState::default();
    *res.game_map = GameMapResource(GameMap::new(400, 280, res.seed.0));
    let player_spawn = res.game_map.0.find_bridge_center()
        .unwrap_or(GridVec::new(SPAWN_X, SPAWN_Y));
    res.camera.0 = player_spawn;
    *res.cursor = CursorPosition::at(player_spawn);
    *res.collectibles = Collectibles::default();
    res.extra_ticks.0 = 0;
    res.blood_map.stains.clear();
    res.spectating.0 = false;
    res.god_mode.0 = false;
    res.dynamic_rng.reset();
    *res.star_level = crate::resources::StarLevel::default();
    res.prop_health.hp.clear();

    res.next_game_state.set(GameState::Playing);

    do_spawn_player(&mut commands, &res.game_map);
    do_spawn_monsters(&mut commands, &res.game_map, res.seed.0);
}
