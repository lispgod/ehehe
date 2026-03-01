use bevy::{app::AppExit, prelude::*};
use bevy_ratatui::event::KeyMessage;
use ratatui::crossterm::event::KeyCode;

use crate::components::{Mana, Player};
use crate::events::{MoveIntent, PickupItemIntent, SpellCastIntent, UseItemIntent};
use crate::resources::{CombatLog, GameState, HelpVisible, TurnState, WelcomeVisible};

/// Default radius for the player's area-of-effect spell.
const SPELL_RADIUS: i32 = 3;

/// Mana cost for casting the AoE spell.
const SPELL_MANA_COST: i32 = 10;

/// All keybindings, generated from the exhaustive match arms below.
/// Used by the `?` help overlay to display available commands.
pub const KEYBINDINGS: &[(&str, &str)] = &[
    ("W / ↑", "Move north"),
    ("S / ↓", "Move south"),
    ("A / ←", "Move west"),
    ("D / →", "Move east"),
    ("F / Space", "Cast AoE spell (costs 10 mana)"),
    ("G", "Pick up item on ground"),
    ("1-9", "Use inventory item by slot"),
    ("P", "Pause / Resume"),
    ("? / /", "Toggle this help screen"),
    ("Q / Esc", "Quit game"),
];

/// Reads keyboard input. Global keys (quit, pause, help) are always handled.
/// Movement keys are only processed while `TurnState::AwaitingInput`,
/// which transitions the game into `PlayerTurn` so that the action is
/// resolved before the next input is accepted.
pub fn input_system(
    mut messages: MessageReader<KeyMessage>,
    mut exit: MessageWriter<AppExit>,
    mut move_intents: MessageWriter<MoveIntent>,
    mut spell_intents: MessageWriter<SpellCastIntent>,
    mut use_item_intents: MessageWriter<UseItemIntent>,
    mut pickup_intents: MessageWriter<PickupItemIntent>,
    player_query: Query<(Entity, Option<&Mana>), With<Player>>,
    game_state: Res<State<GameState>>,
    mut next_game_state: ResMut<NextState<GameState>>,
    turn_state: Option<Res<State<TurnState>>>,
    mut next_turn_state: Option<ResMut<NextState<TurnState>>>,
    mut help_visible: ResMut<HelpVisible>,
    mut welcome_visible: ResMut<WelcomeVisible>,
    mut combat_log: ResMut<CombatLog>,
) {
    let Ok((player_entity, player_mana)) = player_query.single() else {
        return;
    };

    let awaiting_input = turn_state
        .as_ref()
        .is_some_and(|s| *s.get() == TurnState::AwaitingInput);

    for message in messages.read() {
        // Dismiss the welcome screen on any key press.
        if welcome_visible.0 {
            welcome_visible.0 = false;
            continue; // consume the key that dismissed the welcome
        }

        // Exhaustive input handling — every arm here corresponds to a KEYBINDINGS entry.
        match message.code {
            // ── Global keys (always active) ─────────────────────
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
            KeyCode::Char('?') | KeyCode::Char('/') => {
                help_visible.0 = !help_visible.0;
            }
            // ── Movement keys (only while awaiting input) ───────
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
            // ── Spell cast: area-of-effect attack around the player ──
            KeyCode::Char('f') | KeyCode::Char(' ') if awaiting_input => {
                // Check mana before casting.
                let has_mana = player_mana
                    .map(|m| m.current >= SPELL_MANA_COST)
                    .unwrap_or(false);
                if has_mana {
                    spell_intents.write(SpellCastIntent {
                        caster: player_entity,
                        radius: SPELL_RADIUS,
                    });
                    if let Some(next) = &mut next_turn_state {
                        next.set(TurnState::PlayerTurn);
                    }
                } else {
                    combat_log.push("Not enough mana to cast spell!".into());
                }
            }
            // ── Pickup item on ground ───────────────────────────
            KeyCode::Char('g') if awaiting_input => {
                pickup_intents.write(PickupItemIntent {
                    picker: player_entity,
                });
                if let Some(next) = &mut next_turn_state {
                    next.set(TurnState::PlayerTurn);
                }
            }
            // ── Use inventory item by slot (1-9) ────────────────
            KeyCode::Char(c @ '1'..='9') if awaiting_input => {
                let idx = (c as usize) - ('1' as usize);
                use_item_intents.write(UseItemIntent {
                    user: player_entity,
                    item_index: idx,
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
