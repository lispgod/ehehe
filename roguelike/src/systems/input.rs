use bevy::{app::AppExit, prelude::*};
use bevy_ratatui::event::KeyMessage;
use ratatui::crossterm::event::KeyCode;

use crate::components::Player;
use crate::events::MoveIntent;

/// Reads keyboard input and emits `MoveIntent` events for the player entity.
pub fn input_system(
    mut messages: MessageReader<KeyMessage>,
    mut exit: MessageWriter<AppExit>,
    mut move_intents: MessageWriter<MoveIntent>,
    player_query: Query<Entity, With<Player>>,
) {
    let Ok(player_entity) = player_query.single() else {
        return;
    };

    for message in messages.read() {
        match message.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                exit.write_default();
            }
            KeyCode::Char('w') | KeyCode::Up => {
                move_intents.write(MoveIntent {
                    entity: player_entity,
                    dx: 0,
                    dy: 1,
                });
            }
            KeyCode::Char('s') | KeyCode::Down => {
                move_intents.write(MoveIntent {
                    entity: player_entity,
                    dx: 0,
                    dy: -1,
                });
            }
            KeyCode::Char('a') | KeyCode::Left => {
                move_intents.write(MoveIntent {
                    entity: player_entity,
                    dx: -1,
                    dy: 0,
                });
            }
            KeyCode::Char('d') | KeyCode::Right => {
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
