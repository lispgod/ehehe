use bevy::prelude::*;

use crate::components::{Health, Stamina, Player};
use crate::resources::{TurnCounter, TurnState};

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

/// Advances the turn state from `WorldTurn` → `AwaitingInput`.
/// Increments the turn counter each world turn, which drives wave spawning.
/// Also regenerates player stamina and health each turn.
/// Runs only during `TurnState::WorldTurn` after all world-phase systems.
pub fn end_world_turn(
    mut next_state: ResMut<NextState<TurnState>>,
    mut turn_counter: ResMut<TurnCounter>,
    mut player_query: Query<(&mut Stamina, &mut Health), With<Player>>,
) {
    turn_counter.0 += 1;

    if let Ok((mut stamina, mut health)) = player_query.single_mut() {
        // Regenerate player stamina using the pool's recover method.
        stamina.recover(STAMINA_REGEN_PER_TURN);

        // Regenerate player health (slower than stamina — every N turns).
        if turn_counter.0 % HEALTH_REGEN_INTERVAL == 0 {
            health.heal(HEALTH_REGEN_PER_TURN);
        }
    }

    next_state.set(TurnState::AwaitingInput);
}
