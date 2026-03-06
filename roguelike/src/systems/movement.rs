use bevy::prelude::*;

use crate::components::{BlocksMovement, Dead, Health, PlayerControlled, Position, Stamina, Viewshed};
use crate::events::MoveIntent;
use crate::grid_vec::GridVec;
use crate::resources::{BloodMap, CombatLog, CursorPosition, GameMapResource, GameState, InputState, SpatialIndex, TurnCounter, TurnState};
/// Health threshold below which entities leave blood trails when moving.
const BLOOD_DRIP_THRESHOLD: i32 = 40;

/// HP lost per wound tick while wounded (below max HP) and moving.
const WOUND_DAMAGE_PER_TICK: i32 = 1;

/// Number of steps between wound damage ticks.
const WOUND_DAMAGE_INTERVAL: u32 = 5;

/// Processes `MoveIntent` events: checks the target tile on the `GameMap` for
/// walkability *and* the `SpatialIndex` for entities that block movement.
///
/// When the target tile is blocked by another entity, movement is simply
/// skipped (no bump-to-attack). Combat is now initiated explicitly via
/// the punch command (G key).
///
/// **Spatial index atomicity**: after each successful move, the spatial index
/// is updated inline (entity removed from old tile, added to new tile). This
/// ensures that when processing multiple `MoveIntent`s in a single frame
/// (e.g., from AI during WorldTurn), subsequent intents see accurate
/// occupancy data. Without this, two entities could move to the same tile
/// simultaneously because the index would still show their original positions.
///
/// Also marks the entity's `Viewshed` as dirty so FOV is recalculated.
pub fn movement_system(
    mut intents: MessageReader<MoveIntent>,
    game_map: Res<GameMapResource>,
    mut spatial: ResMut<SpatialIndex>,
    mut cursor: ResMut<CursorPosition>,
    mut blood_map: ResMut<BloodMap>,
    turn_counter: Res<TurnCounter>,
    blockers: Query<(), With<BlocksMovement>>,
    players: Query<(), With<PlayerControlled>>,
    mut healths: Query<&mut Health>,
    mut movers: Query<(&mut Position, Option<&mut Viewshed>)>,
    dead_query: Query<(), With<Dead>>,
) {
    for intent in intents.read() {
        let Ok((mut pos, viewshed)) = movers.get_mut(intent.entity) else {
            continue;
        };

        let target = pos.as_grid_vec() + GridVec::new(intent.dx, intent.dy);

        // 1. Check map tile walkability (no blocking props).
        let tile_passable = game_map.0.is_passable(&target);

        // 2. Check spatial index for blocking entities at the target.
        let entity_blocked = spatial.entities_at(&target).iter().any(|&e| {
            e != intent.entity && blockers.contains(e)
        });

        let is_player = players.contains(intent.entity);

        if tile_passable && !entity_blocked {
            let old_pos = pos.as_grid_vec();

            // ── Blood trail: wounded entities leave blood below 40 HP ─
            if let Ok(hp) = healths.get(intent.entity)
                && hp.current < BLOOD_DRIP_THRESHOLD {
                    blood_map.stains.insert(old_pos, turn_counter.0);
                }

            // ── Wound damage: wounded entities lose HP every few steps ─
            if turn_counter.0.is_multiple_of(WOUND_DAMAGE_INTERVAL)
                && !dead_query.contains(intent.entity)
                    && let Ok(mut hp) = healths.get_mut(intent.entity)
                        && hp.current < hp.max
                        && hp.current > 0 {
                            hp.apply_damage(WOUND_DAMAGE_PER_TICK);
                        }

            let delta = GridVec::new(intent.dx, intent.dy);
            pos.x = target.x;
            pos.y = target.y;

            // ── Maintain spatial index invariant ─────────────────
            // Atomically move the entity in the index so subsequent
            // intents in this frame see accurate occupancy.
            spatial.move_entity(&old_pos, target, intent.entity);

            // ── Cursor follows player movement ──────────────────
            // When the player moves, the cursor moves by the same delta
            // so the player keeps looking in the same relative direction.
            if is_player {
                cursor.pos += delta;
            }

            // Mark viewshed dirty so visibility is recalculated.
            if let Some(mut vs) = viewshed {
                vs.dirty = true;
            }
        }
    }

    // Periodically prune old blood stains to prevent unbounded growth.
    blood_map.prune(turn_counter.0);
}

/// Checks if the player has reached any edge of the map.
/// Transitions to Victory state when the player escapes the town.
///
/// Uses `Single` (see `examples/ecs/fallible_params.rs`): the system is
/// automatically skipped when the player entity doesn't exist.
pub fn victory_check_system(
    player_pos: Single<&Position, With<PlayerControlled>>,
    game_map: Res<GameMapResource>,
    mut next_state: ResMut<NextState<GameState>>,
    mut combat_log: ResMut<CombatLog>,
    state: Res<State<GameState>>,
) {
    if *state.get() != GameState::Playing {
        return;
    }
    let gv = player_pos.as_grid_vec();
    // Win by reaching any edge of the map (escape the town)
    if gv.x <= 1 || gv.y <= 1 || gv.x >= game_map.0.width - 2 || gv.y >= game_map.0.height - 2 {
        combat_log.push("You escaped the town! YOU WIN!".into());
        next_state.set(GameState::Victory);
    }
}

/// Water tiles now block movement entirely (no swimming), so there is
/// nothing to slow down. This system is kept as a no-op stub so the
/// plugin's system registration doesn't need to change.
pub fn water_slowdown_system(
    _player_pos: Single<&Position, With<PlayerControlled>>,
    _game_map: Res<GameMapResource>,
    _extra_ticks: ResMut<crate::resources::ExtraWorldTicks>,
    _combat_log: ResMut<CombatLog>,
    _turn_state: Option<Res<State<TurnState>>>,
) {
    // Water is impassable; this system is intentionally a no-op.
}

/// Consumes pending dive stamina after movement is processed.
/// The input system sets `dive_stamina_pending` and the movement system
/// processes the move intents. This system deducts the stamina.
pub fn dive_stamina_system(
    mut input_state: ResMut<InputState>,
    mut player_query: Query<&mut Stamina, With<PlayerControlled>>,
) {
    if input_state.dive_stamina_pending > 0 {
        if let Ok(mut stamina) = player_query.single_mut() {
            stamina.spend(input_state.dive_stamina_pending);
        }
        input_state.dive_stamina_pending = 0;
    }
    if input_state.ability_stamina_pending > 0 {
        if let Ok(mut stamina) = player_query.single_mut() {
            stamina.spend(input_state.ability_stamina_pending);
        }
        input_state.ability_stamina_pending = 0;
    }
}
