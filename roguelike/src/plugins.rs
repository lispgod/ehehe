use std::collections::HashSet;

use bevy::prelude::*;

use crate::components::{
    AiState, BlocksMovement, CameraFollow, CombatStats, Energy, Health, HellGate, Hostile,
    Inventory, LootTable, Mana, Name, Player, Position, Renderable, Speed, Viewshed, ACTION_COST,
};
use crate::events::{AttackIntent, DamageEvent, MoveIntent, PickupItemIntent, SpellCastIntent, UseItemIntent};
use crate::gamemap::GameMap;
use crate::grid_vec::GridVec;
use crate::noise::value_noise;
use crate::resources::{
    CameraPosition, CombatLog, GameMapResource, GameState, HelpVisible, KillCount, MapSeed,
    SpatialIndex, SpellParticles, TurnCounter, TurnState, WelcomeVisible,
};
use crate::systems::{ai, camera, combat, corruption, input, inventory, movement, render, spatial_index, spell, turn, visibility, wave_spawn};
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
            // ── Resources ──
            .insert_resource(MapSeed(seed))
            .insert_resource(GameMapResource(GameMap::new(120, 80, seed)))
            .insert_resource(CameraPosition(SPAWN_POINT))
            .init_resource::<SpatialIndex>()
            .init_resource::<CombatLog>()
            .init_resource::<TurnCounter>()
            .init_resource::<KillCount>()
            .init_resource::<HelpVisible>()
            .init_resource::<SpellParticles>()
            .init_resource::<WelcomeVisible>()
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
            .add_systems(PreUpdate, input::input_system)
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
                    inventory::use_item_system,
                    spell::spell_system,
                    combat::combat_system,
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
            // ── World turn: energy accumulation + AI + wave spawning + corruption + action resolution ──
            .add_systems(
                Update,
                (
                    ai::energy_accumulate_system,
                    ai::ai_system,
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
                (render::particle_tick_system, render::draw_system)
                    .chain()
                    .in_set(RoguelikeSet::Render),
            );
    }
}

/// Spawns the player entity with all required ECS components.
fn spawn_player(mut commands: Commands) {
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
        Mana {
            current: 50,
            max: 50,
        },
        CombatStats {
            attack: 5,
            defense: 2,
        },
        Speed(ACTION_COST), // normal speed: one action per tick
        Energy(0),
        Inventory::default(),
        Viewshed {
            range: 15,
            visible_tiles: HashSet::new(),
            revealed_tiles: HashSet::new(),
            dirty: true, // compute on first frame
        },
    ));
}

/// Monster archetypes for procedural spawning (hellish theme).
struct MonsterTemplate {
    name: &'static str,
    symbol: &'static str,
    fg: RatColor,
    health: i32,
    attack: i32,
    defense: i32,
    speed: i32,
    sight_range: i32,
}

const MONSTER_TEMPLATES: &[MonsterTemplate] = &[
    MonsterTemplate {
        name: "Imp",
        symbol: "i",
        fg: RatColor::Rgb(255, 80, 0),
        health: 6,
        attack: 3,
        defense: 1,
        speed: 90,
        sight_range: 8,
    },
    MonsterTemplate {
        name: "Demon",
        symbol: "D",
        fg: RatColor::Rgb(200, 0, 0),
        health: 18,
        attack: 5,
        defense: 2,
        speed: 60,
        sight_range: 6,
    },
    MonsterTemplate {
        name: "Hellhound",
        symbol: "h",
        fg: RatColor::Rgb(200, 60, 20),
        health: 10,
        attack: 4,
        defense: 1,
        speed: 120,
        sight_range: 10,
    },
];

/// Spawns monsters on passable tiles using deterministic noise placement.
/// Monsters are spawned near the Hell Gate area, using the map seed for
/// deterministic placement.
fn spawn_monsters(mut commands: Commands, map: Res<GameMapResource>, seed: Res<MapSeed>) {
    let spawn_seed = seed.0.wrapping_add(54321);
    let template_seed = seed.0.wrapping_add(98765);
    let min_spawn_dist_sq = 12 * 12; // min squared distance from player spawn
    let gate_exclusion_sq = 3 * 3; // don't spawn on/near gate tile

    for y in 1..map.0.height - 1 {
        for x in 1..map.0.width - 1 {
            let pos = GridVec::new(x, y);

            // Skip tiles near the spawn point.
            if pos.distance_squared(SPAWN_POINT) < min_spawn_dist_sq {
                continue;
            }

            // Skip tiles too close to the gate.
            if pos.distance_squared(GATE_POINT) < gate_exclusion_sq {
                continue;
            }

            // Skip impassable tiles.
            if !map.0.is_passable(&pos) {
                continue;
            }

            // Noise-based spawn chance (~2% of passable tiles).
            let noise = value_noise(x, y, spawn_seed);
            if noise > 0.02 {
                continue;
            }

            // Select monster type deterministically.
            // value_noise returns [0, 1); the .min() is defensive against float edge cases.
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
                Viewshed {
                    range: template.sight_range,
                    visible_tiles: HashSet::new(),
                    revealed_tiles: HashSet::new(),
                    dirty: true,
                },
            ));
        }
    }
}

/// Spawns the Hell Gate entity — the main objective the player must destroy to win.
///
/// The gate is a destructible, hostile entity with high health that blocks movement.
/// Monsters emerge from it each wave, and the surrounding land corrupts over time.
fn spawn_hell_gate(mut commands: Commands) {
    commands.spawn((
        Position {
            x: GATE_X,
            y: GATE_Y,
        },
        HellGate,
        Hostile,
        Name("Gate of Hell".into()),
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
