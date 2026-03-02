use bevy::prelude::*;

use crate::components::{Health, Stamina, Player};
use crate::resources::{ExtraWorldTicks, TurnCounter, TurnState};

/// Stamina regenerated per world turn.
const STAMINA_REGEN_PER_TURN: i32 = 2;

/// Health regenerated per turn (passive, slower than stamina).
const HEALTH_REGEN_PER_TURN: i32 = 1;

/// Health regeneration only triggers every N turns to keep it slower than stamina.
const HEALTH_REGEN_INTERVAL: u32 = 3;

/// Advances the turn state from `PlayerTurn` → `WorldTurn`.
/// Runs only during `TurnState::PlayerTurn` after all player-phase systems.
pub fn end_player_turn(mut next_state: ResMut<NextState<TurnState>>) {
    next_state.set(TurnState::WorldTurn);
}

/// Advances the turn state from `WorldTurn` → `AwaitingInput`, or stays in
/// `WorldTurn` if `ExtraWorldTicks` has remaining ticks (physical movement
/// costs 2 ticks). Increments the turn counter and regenerates player stats.
pub fn end_world_turn(
    mut next_state: ResMut<NextState<TurnState>>,
    mut turn_counter: ResMut<TurnCounter>,
    mut player_query: Query<(&mut Stamina, &mut Health), With<Player>>,
    mut extra_ticks: ResMut<ExtraWorldTicks>,
) {
    turn_counter.0 += 1;

    if let Ok((mut stamina, mut health)) = player_query.single_mut() {
        // Regenerate player stamina using the pool's recover method.
        stamina.recover(STAMINA_REGEN_PER_TURN);

        // Regenerate player health (slower than stamina — every N turns).
        if turn_counter.0.is_multiple_of(HEALTH_REGEN_INTERVAL) {
            health.heal(HEALTH_REGEN_PER_TURN);
        }
    }

    if extra_ticks.0 > 0 {
        extra_ticks.0 -= 1;
        // Stay in WorldTurn for the extra tick — don't transition yet.
    } else {
        next_state.set(TurnState::AwaitingInput);
    }
}
