use bevy::prelude::*;

use crate::resources::{TurnCounter, TurnState};

/// Advances the turn state from `PlayerTurn` → `WorldTurn`.
/// Runs only during `TurnState::PlayerTurn` after all player-phase systems.
pub fn end_player_turn(mut next_state: ResMut<NextState<TurnState>>) {
    next_state.set(TurnState::WorldTurn);
}

/// Advances the turn state from `WorldTurn` → `AwaitingInput`.
/// Increments the turn counter each world turn, which drives wave spawning.
/// Runs only during `TurnState::WorldTurn` after all world-phase systems.
pub fn end_world_turn(
    mut next_state: ResMut<NextState<TurnState>>,
    mut turn_counter: ResMut<TurnCounter>,
) {
    turn_counter.0 += 1;
    next_state.set(TurnState::AwaitingInput);
}
