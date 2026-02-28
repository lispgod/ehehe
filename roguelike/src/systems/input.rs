use bevy::{app::AppExit, prelude::*};
use bevy_ratatui::event::KeyMessage;
use ratatui::crossterm::event::KeyCode;

use crate::components::Player;
use crate::events::MoveIntent;
use crate::resources::GameState;

/// Reads keyboard input. Global keys (quit, pause) are always handled.
/// Movement keys are only processed while `GameState::Playing`.
pub fn input_system(
    mut messages: MessageReader<KeyMessage>,
    mut exit: MessageWriter<AppExit>,
    mut move_intents: MessageWriter<MoveIntent>,
    player_query: Query<Entity, With<Player>>,
    state: Res<State<GameState>>,
    mut next_state: ResMut<NextState<GameState>>,
) {
    let Ok(player_entity) = player_query.single() else {
        return;
    };

    for message in messages.read() {
        match message.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                exit.write_default();
            }
            KeyCode::Char('p') => {
                let new = match state.get() {
                    GameState::Playing => GameState::Paused,
                    GameState::Paused => GameState::Playing,
                };
                next_state.set(new);
            }
            // Movement keys only processed while playing
            KeyCode::Char('w') | KeyCode::Up if *state.get() == GameState::Playing => {
                move_intents.write(MoveIntent {
                    entity: player_entity,
                    dx: 0,
                    dy: 1,
                });
            }
            KeyCode::Char('s') | KeyCode::Down if *state.get() == GameState::Playing => {
                move_intents.write(MoveIntent {
                    entity: player_entity,
                    dx: 0,
                    dy: -1,
                });
            }
            KeyCode::Char('a') | KeyCode::Left if *state.get() == GameState::Playing => {
                move_intents.write(MoveIntent {
                    entity: player_entity,
                    dx: -1,
                    dy: 0,
                });
            }
            KeyCode::Char('d') | KeyCode::Right if *state.get() == GameState::Playing => {
                move_intents.write(MoveIntent {
                    entity: player_entity,
                    dx: 1,
                    dy: 0,
                });
            }
            _ => {}
        }
    }
}
