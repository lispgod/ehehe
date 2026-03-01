use std::collections::HashSet;

use bevy::prelude::*;

use crate::components::{
    AiState, BlocksMovement, CameraFollow, CombatStats, Energy, Health, Hostile, Name,
    Player, Position, Renderable, Speed, Viewshed, ACTION_COST,
};
use crate::events::{AttackIntent, DamageEvent, MoveIntent, SpellCastIntent};
use crate::gamemap::GameMap;
use crate::grid_vec::GridVec;
use crate::noise::value_noise;
use crate::resources::{
    CameraPosition, CombatLog, GameMapResource, GameState, KillCount, MapSeed, SpatialIndex,
    TurnCounter, TurnState,
};
use crate::systems::{ai, camera, combat, input, movement, render, spatial_index, spell, turn, visibility, wave_spawn};
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
            // ── Resources ──
            .insert_resource(MapSeed(seed))
            .insert_resource(GameMapResource(GameMap::new(120, 80, seed)))
            .insert_resource(CameraPosition(SPAWN_POINT))
            .init_resource::<SpatialIndex>()
            .init_resource::<CombatLog>()
            .init_resource::<TurnCounter>()
            .init_resource::<KillCount>()
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
            // ── World turn: energy accumulation + AI + wave spawning + action resolution ──
            .add_systems(
                Update,
                (
                    ai::energy_accumulate_system,
                    ai::ai_system,
                    wave_spawn::wave_spawn_system,
                )
                    .chain()
                    .after(RoguelikeSet::Consequence)
                    .run_if(in_state(TurnState::WorldTurn)),
            )
            .add_systems(
                Update,
                turn::end_world_turn
                    .after(wave_spawn::wave_spawn_system)
                    .run_if(in_state(TurnState::WorldTurn)),
            )
            // ── Render (always runs — shows PAUSED overlay when paused) ──
            .add_systems(
                Update,
                render::draw_system.in_set(RoguelikeSet::Render),
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
        CombatStats {
            attack: 5,
            defense: 2,
        },
        Speed(ACTION_COST), // normal speed: one action per tick
        Energy(0),
        Viewshed {
            range: 15,
            visible_tiles: HashSet::new(),
            revealed_tiles: HashSet::new(),
            dirty: true, // compute on first frame
        },
    ));
}

/// Monster archetypes for procedural spawning.
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
        name: "Goblin",
        symbol: "g",
        fg: RatColor::Rgb(0, 200, 0),
        health: 8,
        attack: 3,
        defense: 1,
        speed: 80,
        sight_range: 8,
    },
    MonsterTemplate {
        name: "Orc",
        symbol: "o",
        fg: RatColor::Rgb(180, 0, 0),
        health: 16,
        attack: 4,
        defense: 2,
        speed: 60,
        sight_range: 6,
    },
    MonsterTemplate {
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

/// Spawns monsters on passable tiles using deterministic noise placement.
///
/// Monster locations are derived from the map seed, ensuring deterministic
/// spawning: same seed → same monsters at same positions.
fn spawn_monsters(mut commands: Commands, map: Res<GameMapResource>, seed: Res<MapSeed>) {
    let spawn_seed = seed.0.wrapping_add(54321);
    let template_seed = seed.0.wrapping_add(98765);
    let min_spawn_dist_sq = 12 * 12; // min squared distance from player spawn

    for y in 1..map.0.height - 1 {
        for x in 1..map.0.width - 1 {
            let pos = GridVec::new(x, y);

            // Skip tiles near the spawn point.
            if pos.distance_squared(SPAWN_POINT) < min_spawn_dist_sq {
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
