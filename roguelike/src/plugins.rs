use std::collections::HashSet;

use bevy::prelude::*;

use crate::components::{
    BlocksMovement, Caliber, CameraFollow, CombatStats, Energy,
    GroupFollower, GroupLeader,
    Health, Inventory, Item, ItemKind, Stamina, Name, Player, Position,
    Renderable, Speed, Viewshed, ACTION_COST,
};
use crate::events::{AiRangedAttackIntent, AttackIntent, DamageEvent, DropItemIntent, MeleeWideIntent, MolotovCastIntent, MoveIntent, PickupItemIntent, RangedAttackIntent, SpellCastIntent, ThrowItemIntent, UseItemIntent};
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

        let game_map = GameMap::new(400, 280, seed);
        // Compute actual player spawn position so camera+cursor start centered on it.
        let player_spawn = game_map.find_house_exterior()
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
            .add_message::<DropItemIntent>()
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
                    ai::leader_death_system,
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

/// Helper: spawns the player entity.
fn do_spawn_player(commands: &mut Commands, _seed: u64, map: &GameMapResource) {
    // Find a saloon interior tile, falling back to default spawn point.
    let spawn_pos = map.0.find_house_exterior()
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

/// Helper: spawns monsters in distinct faction groups across the map.
/// Factions seed from defensible anchor buildings when available,
/// with additional groups placed at deterministic grid positions.
/// Many enemies are spawned to create a densely populated town.
fn do_spawn_monsters(commands: &mut Commands, map: &GameMapResource, seed: u64) {
    let group_seed = seed.wrapping_add(54321);
    let min_spawn_dist_sq = 8 * 8; // enemies can be fairly close to spawn
    let group_radius = 5i32; // NPCs spawn within this radius of group center

    // Faction-template pairs: each group spawns NPCs from one faction.
    let faction_templates: &[&[usize]] = &[
        &[0, 1],    // Wildlife: Coyote(0), Rattlesnake(1)
        &[2, 5],    // Outlaws: Outlaw(2), Gunslinger(5)
        &[3],       // Vaqueros: Vaquero(3)
        &[4],       // Lawmen: Cowboy(4)
        &[6],       // Civilians: Civilian(6)
        &[7, 8],    // Indians: Indian Brave(7), Indian Scout(8)
        &[9, 10],   // Sheriff: Sheriff(9), Deputy(10)
    ];

    // ── Faction anchor spawns ──────────────────────────────────────
    // Each faction seeds from its defensible anchor building.
    let anchor_radius = 8i32;
    for (anchor_pos, faction, _name) in &map.0.faction_anchors {
        let templates: &[usize] = match faction {
            crate::components::Faction::Outlaws => &[2, 5],
            crate::components::Faction::Vaqueros => &[3],
            crate::components::Faction::Sheriff => &[9, 10],
            crate::components::Faction::Lawmen => &[4],
            crate::components::Faction::Civilians => &[6],
            crate::components::Faction::Indians => &[7, 8],
            crate::components::Faction::Wildlife => &[0, 1],
        };

        let anchor_group_size = 5 + (value_noise(anchor_pos.x, anchor_pos.y, group_seed.wrapping_add(77777)) * 4.0) as i32;
        let mut spawned = 0;
        let mut leader_entity: Option<Entity> = None;
        for dy in -anchor_radius..=anchor_radius {
            for dx in -anchor_radius..=anchor_radius {
                if spawned >= anchor_group_size { break; }
                let pos = GridVec::new(anchor_pos.x + dx, anchor_pos.y + dy);
                if pos.distance_squared(SPAWN_POINT) < min_spawn_dist_sq { continue; }
                if !map.0.is_passable(&pos) { continue; }
                if let Some(voxel) = map.0.get_voxel_at(&pos) {
                    if voxel.props.is_some() { continue; }
                    if matches!(voxel.floor, Some(crate::typeenums::Floor::Fire) | Some(crate::typeenums::Floor::DeepWater) | Some(crate::typeenums::Floor::ShallowWater)) {
                        continue;
                    }
                }
                let tile_noise = value_noise(pos.x, pos.y, group_seed.wrapping_add(88888));
                if tile_noise > 0.40 { continue; }

                let template_idx = templates[(spawned as usize) % templates.len()];
                let template = &MONSTER_TEMPLATES[template_idx];
                let ent = spawn::spawn_monster(commands, template, pos.x, pos.y, 0, 0, 0.25);

                if spawned == 0 && !matches!(template.faction, crate::components::Faction::Wildlife) {
                    commands.entity(ent).insert(GroupLeader);
                    leader_entity = Some(ent);
                } else if let Some(leader) = leader_entity {
                    commands.entity(ent).insert(GroupFollower { leader });
                }
                spawned += 1;
            }
            if spawned >= anchor_group_size { break; }
        }
    }

    // ── Grid-based spawns (additional groups) ──────────────────────
    // Dense grid: scan every 18 tiles (smaller cells = more groups).
    let cell_size = 18i32;
    let mut group_idx = 0u64;

    for cy in (cell_size..map.0.height - cell_size).step_by(cell_size as usize) {
        for cx in (cell_size..map.0.width - cell_size).step_by(cell_size as usize) {
            let center = GridVec::new(cx, cy);

            if center.distance_squared(SPAWN_POINT) < min_spawn_dist_sq {
                continue;
            }

            let noise = value_noise(cx, cy, group_seed);
            if noise > 0.55 {
                continue; // ~55% of cells get a group (up from 25%)
            }

            // Select faction for this group based on position hash.
            let faction_idx = (group_idx as usize) % faction_templates.len();
            let templates = faction_templates[faction_idx];
            group_idx += 1;

            // Spawn 4-8 NPCs per group within the radius.
            let group_size_noise = value_noise(cx, cy, group_seed.wrapping_add(11111));
            let group_size = 4 + (group_size_noise * 5.0) as i32; // 4-8

            let mut spawned = 0;
            let mut leader_entity: Option<Entity> = None;
            for dy in -group_radius..=group_radius {
                for dx in -group_radius..=group_radius {
                    if spawned >= group_size {
                        break;
                    }
                    let pos = GridVec::new(cx + dx, cy + dy);
                    if pos.distance_squared(SPAWN_POINT) < min_spawn_dist_sq {
                        continue;
                    }
                    if !map.0.is_passable(&pos) {
                        continue;
                    }
                    // Skip tiles with props or water/fire
                    if let Some(voxel) = map.0.get_voxel_at(&pos) {
                        if voxel.props.is_some() {
                            continue;
                        }
                        if matches!(voxel.floor, Some(crate::typeenums::Floor::Fire) | Some(crate::typeenums::Floor::DeepWater) | Some(crate::typeenums::Floor::ShallowWater)) {
                            continue;
                        }
                    }

                    let tile_noise = value_noise(pos.x, pos.y, group_seed.wrapping_add(22222));
                    if tile_noise > 0.35 {
                        continue;
                    }

                    let template_idx = templates[(spawned as usize) % templates.len()];
                    let template = &MONSTER_TEMPLATES[template_idx];
                    let ent = spawn::spawn_monster(commands, template, pos.x, pos.y, 0, 0, 0.25);

                    // First humanoid NPC in a group becomes the leader.
                    if spawned == 0 && !matches!(template.faction, crate::components::Faction::Wildlife) {
                        commands.entity(ent).insert(GroupLeader);
                        leader_entity = Some(ent);
                    } else if let Some(leader) = leader_entity {
                        commands.entity(ent).insert(GroupFollower { leader });
                    }

                    spawned += 1;
                }
                if spawned >= group_size {
                    break;
                }
            }
        }
    }

    // ── Strategic spawns near the player ─────────────────────────
    // Place extra enemy clusters within 15-30 tiles of spawn so the
    // player encounters opposition immediately.
    let near_seed = seed.wrapping_add(99999);
    let near_cluster_count = 6;
    for i in 0..near_cluster_count {
        let angle_noise = value_noise(i, 0, near_seed);
        let dist_noise = value_noise(0, i, near_seed);
        let angle = angle_noise * std::f64::consts::TAU;
        let dist = 15.0 + dist_noise * 20.0;
        let cx = SPAWN_POINT.x + (angle.cos() * dist) as i32;
        let cy = SPAWN_POINT.y + (angle.sin() * dist) as i32;

        // Pick a combat-oriented faction for nearby enemies
        let nearby_templates: &[usize] = match i % 3 {
            0 => &[2, 5],  // Outlaws + Gunslingers
            1 => &[3],     // Vaqueros
            _ => &[7, 8],  // Indians
        };

        let mut spawned = 0;
        let near_group_size = 3 + (value_noise(i, 1, near_seed) * 3.0) as i32; // 3-5 per cluster
        for dy in -4..=4i32 {
            for dx in -4..=4i32 {
                if spawned >= near_group_size {
                    break;
                }
                let pos = GridVec::new(cx + dx, cy + dy);
                if pos.distance_squared(SPAWN_POINT) < min_spawn_dist_sq {
                    continue;
                }
                if !map.0.is_passable(&pos) {
                    continue;
                }
                // Skip tiles with props or water/fire
                if let Some(voxel) = map.0.get_voxel_at(&pos) {
                    if voxel.props.is_some() {
                        continue;
                    }
                    if matches!(voxel.floor, Some(crate::typeenums::Floor::Fire) | Some(crate::typeenums::Floor::DeepWater) | Some(crate::typeenums::Floor::ShallowWater)) {
                        continue;
                    }
                }
                let tile_noise = value_noise(pos.x, pos.y, near_seed.wrapping_add(33333));
                if tile_noise > 0.25 {
                    continue;
                }
                let template_idx = nearby_templates[(spawned as usize) % nearby_templates.len()];
                let template = &MONSTER_TEMPLATES[template_idx];
                spawn::spawn_monster(commands, template, pos.x, pos.y, 0, 0, 0.30);
                spawned += 1;
            }
            if spawned >= near_group_size {
                break;
            }
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
    mut spell_particles: ResMut<SpellParticles>,
    mut input_state: ResMut<InputState>,
    mut next_game_state: ResMut<NextState<GameState>>,
    mut seed: ResMut<MapSeed>,
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

    // Generate a new seed each restart so the map is different every time.
    let new_seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(seed.0.wrapping_add(1));
    seed.0 = new_seed;

    combat_log.clear();
    kill_count.0 = 0;
    turn_counter.0 = 0;
    spell_particles.particles.clear();
    *input_state = InputState::default();
    *game_map = GameMapResource(GameMap::new(400, 280, seed.0));
    let player_spawn = game_map.0.find_house_exterior()
        .unwrap_or(GridVec::new(SPAWN_X, SPAWN_Y));
    camera.0 = player_spawn;
    *cursor = CursorPosition::at(player_spawn);
    *collectibles = Collectibles::default();
    extra_ticks.0 = 0;
    blood_map.stains.clear();
    spectating.0 = false;
    god_mode.0 = false;
    dynamic_rng.reset();

    next_game_state.set(GameState::Playing);

    do_spawn_player(&mut commands, seed.0, &game_map);
    do_spawn_monsters(&mut commands, &game_map, seed.0);
}
