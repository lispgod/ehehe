use bevy::prelude::*;

use crate::components::{Health, Mana, Player};
use crate::resources::{TurnCounter, TurnState};

/// Mana regenerated per world turn.
const MANA_REGEN_PER_TURN: i32 = 2;

/// Health regenerated per turn (passive, slower than mana).
const HEALTH_REGEN_PER_TURN: i32 = 1;

/// Health regeneration only triggers every N turns to keep it slower than mana.
const HEALTH_REGEN_INTERVAL: u32 = 3;

/// Advances the turn state from `PlayerTurn` → `WorldTurn`.
/// Runs only during `TurnState::PlayerTurn` after all player-phase systems.
pub fn end_player_turn(mut next_state: ResMut<NextState<TurnState>>) {
    next_state.set(TurnState::WorldTurn);
}

/// Advances the turn state from `WorldTurn` → `AwaitingInput`.
/// Increments the turn counter each world turn, which drives wave spawning.
/// Also regenerates player mana and health each turn.
/// Runs only during `TurnState::WorldTurn` after all world-phase systems.
pub fn end_world_turn(
    mut next_state: ResMut<NextState<TurnState>>,
    mut turn_counter: ResMut<TurnCounter>,
    mut player_query: Query<(&mut Mana, &mut Health), With<Player>>,
) {
    turn_counter.0 += 1;

    if let Ok((mut mana, mut health)) = player_query.single_mut() {
        // Regenerate player mana.
        mana.current = (mana.current + MANA_REGEN_PER_TURN).min(mana.max);

        // Regenerate player health (slower than mana — every N turns).
        if turn_counter.0 % HEALTH_REGEN_INTERVAL == 0 {
            health.current = (health.current + HEALTH_REGEN_PER_TURN).min(health.max);
        }
    }

    next_state.set(TurnState::AwaitingInput);
}
