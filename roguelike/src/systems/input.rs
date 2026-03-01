use bevy::{app::AppExit, prelude::*};
use bevy_ratatui::event::KeyMessage;
use ratatui::crossterm::event::KeyCode;

use crate::components::Player;
use crate::events::{MoveIntent, SpellCastIntent};
use crate::resources::{GameState, TurnState};

/// Default radius for the player's area-of-effect spell.
const SPELL_RADIUS: i32 = 3;

/// Reads keyboard input. Global keys (quit, pause) are always handled.
/// Movement keys are only processed while `TurnState::AwaitingInput`,
/// which transitions the game into `PlayerTurn` so that the action is
/// resolved before the next input is accepted.
pub fn input_system(
    mut messages: MessageReader<KeyMessage>,
    mut exit: MessageWriter<AppExit>,
    mut move_intents: MessageWriter<MoveIntent>,
    mut spell_intents: MessageWriter<SpellCastIntent>,
    player_query: Query<Entity, With<Player>>,
    game_state: Res<State<GameState>>,
    mut next_game_state: ResMut<NextState<GameState>>,
    turn_state: Option<Res<State<TurnState>>>,
    mut next_turn_state: Option<ResMut<NextState<TurnState>>>,
) {
    let Ok(player_entity) = player_query.single() else {
        return;
    };

    let awaiting_input = turn_state
        .as_ref()
        .is_some_and(|s| *s.get() == TurnState::AwaitingInput);

    for message in messages.read() {
        match message.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                exit.write_default();
            }
            KeyCode::Char('p') => {
                let new = match game_state.get() {
                    GameState::Playing => GameState::Paused,
                    GameState::Paused => GameState::Playing,
                    GameState::Victory => GameState::Victory,
                };
                next_game_state.set(new);
            }
            // Movement keys only processed while awaiting input
            KeyCode::Char('w') | KeyCode::Up if awaiting_input => {
                emit_move(&mut move_intents, &mut next_turn_state, player_entity, 0, 1);
            }
            KeyCode::Char('s') | KeyCode::Down if awaiting_input => {
                emit_move(&mut move_intents, &mut next_turn_state, player_entity, 0, -1);
            }
            KeyCode::Char('a') | KeyCode::Left if awaiting_input => {
                emit_move(&mut move_intents, &mut next_turn_state, player_entity, -1, 0);
            }
            KeyCode::Char('d') | KeyCode::Right if awaiting_input => {
                emit_move(&mut move_intents, &mut next_turn_state, player_entity, 1, 0);
            }
            // Spell cast: area-of-effect attack around the player
            KeyCode::Char('f') | KeyCode::Char(' ') if awaiting_input => {
                spell_intents.write(SpellCastIntent {
                    caster: player_entity,
                    radius: SPELL_RADIUS,
                });
                if let Some(next) = &mut next_turn_state {
                    next.set(TurnState::PlayerTurn);
                }
            }
            _ => {}
        }
    }
}

/// Helper: emits a `MoveIntent` and advances the turn state to `PlayerTurn`.
fn emit_move(
    move_intents: &mut MessageWriter<MoveIntent>,
    next_turn_state: &mut Option<ResMut<NextState<TurnState>>>,
    entity: Entity,
    dx: i32,
    dy: i32,
) {
    move_intents.write(MoveIntent { entity, dx, dy });
    if let Some(next) = next_turn_state {
        next.set(TurnState::PlayerTurn);
    }
}
