use bevy::{app::AppExit, prelude::*};
use bevy_ratatui::event::KeyMessage;
use ratatui::crossterm::event::KeyCode;

use crate::components::{Inventory, Mana, Player};
use crate::events::{MeleeWideIntent, MoveIntent, PickupItemIntent, RangedAttackIntent, SpellCastIntent, UseItemIntent};
use crate::resources::{CombatLog, GameState, InputMode, InputState, RestartRequested, TurnState};

/// Default radius for the player's area-of-effect spell.
const SPELL_RADIUS: i32 = 3;

/// Stamina cost for throwing a grenade.
const SPELL_MANA_COST: i32 = 10;

/// Range for the targeted ranged attack.
const RANGED_ATTACK_RANGE: i32 = 8;

/// A single command binding entry: the key(s) that trigger it, a short name, and documentation.
pub struct CommandBinding {
    /// Key combination string shown in the help/welcome screen.
    pub key: &'static str,
    /// Short action name.
    pub name: &'static str,
    /// Longer description / documentation for the command.
    pub docs: &'static str,
}

/// All keybindings, generated from the exhaustive match arms below.
/// Used by the `?` help overlay to display available commands.
pub const KEYBINDINGS: &[CommandBinding] = &[
    CommandBinding { key: "W / ↑", name: "Move north", docs: "Move the player one tile north (up on the map)." },
    CommandBinding { key: "S / ↓", name: "Move south", docs: "Move the player one tile south (down on the map)." },
    CommandBinding { key: "A / ←", name: "Move west", docs: "Move the player one tile west (left on the map)." },
    CommandBinding { key: "D / →", name: "Move east", docs: "Move the player one tile east (right on the map)." },
    CommandBinding { key: "F / Space", name: "Throw grenade", docs: "Throw a frag grenade dealing area damage around the player (costs 10 stamina)." },
    CommandBinding { key: "R", name: "Shoot (ranged)", docs: "Fire your weapon at the nearest visible enemy within 8 tiles." },
    CommandBinding { key: "E", name: "Melee sweep", docs: "Swing your melee weapon hitting all adjacent enemies." },
    CommandBinding { key: ". / 5", name: "Wait", docs: "Skip the current turn without acting." },
    CommandBinding { key: "G", name: "Pick up item", docs: "Pick up an item on the ground at your position." },
    CommandBinding { key: "I", name: "Open inventory", docs: "Open the inventory screen to view and use items." },
    CommandBinding { key: "1-9", name: "Use item", docs: "Quickly use an inventory item by slot number." },
    CommandBinding { key: "P", name: "Pause / Resume", docs: "Toggle pause state." },
    CommandBinding { key: "? / /", name: "Help", docs: "Toggle this help screen." },
    CommandBinding { key: "Q / Esc", name: "Quit", docs: "Open the quit confirmation prompt." },
];

/// Reads keyboard input. Global keys (quit, pause, help) are always handled.
/// Movement keys are only processed while `TurnState::AwaitingInput`,
/// which transitions the game into `PlayerTurn` so that the action is
/// resolved before the next input is accepted.
///
/// When the game is in `GameState::Dead`, only quit (Q/Esc) and restart (R) work.
/// When `InputMode::Inventory` is active, keys navigate the inventory overlay.
pub fn input_system(
    mut messages: MessageReader<KeyMessage>,
    mut exit: MessageWriter<AppExit>,
    mut move_intents: MessageWriter<MoveIntent>,
    mut spell_intents: MessageWriter<SpellCastIntent>,
    mut use_item_intents: MessageWriter<UseItemIntent>,
    mut pickup_intents: MessageWriter<PickupItemIntent>,
    mut ranged_intents: MessageWriter<RangedAttackIntent>,
    mut melee_wide_intents: MessageWriter<MeleeWideIntent>,
    player_query: Query<(Entity, Option<&Mana>, Option<&Inventory>), With<Player>>,
    game_state: Res<State<GameState>>,
    mut next_game_state: ResMut<NextState<GameState>>,
    turn_state: Option<Res<State<TurnState>>>,
    mut next_turn_state: Option<ResMut<NextState<TurnState>>>,
    mut combat_log: ResMut<CombatLog>,
    mut input_state: ResMut<InputState>,
    mut restart_requested: ResMut<RestartRequested>,
) {
    // Handle Dead and Victory states: only Q/Esc to quit, R to restart.
    if *game_state.get() == GameState::Dead || *game_state.get() == GameState::Victory {
        for message in messages.read() {
            match message.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    exit.write_default();
                }
                KeyCode::Char('r') => {
                    restart_requested.0 = true;
                }
                _ => {}
            }
        }
        return;
    }

    let Ok((player_entity, player_mana, player_inv)) = player_query.single() else {
        // Player entity is gone (should only happen transiently).
        for message in messages.read() {
            if matches!(message.code, KeyCode::Char('q') | KeyCode::Esc) {
                exit.write_default();
            }
        }
        return;
    };

    let awaiting_input = turn_state
        .as_ref()
        .is_some_and(|s| *s.get() == TurnState::AwaitingInput);

    // ── Inventory input mode ────────────────────────────────────
    if input_state.mode == InputMode::Inventory {
        let item_count = player_inv.map_or(0, |inv| inv.items.len());
        for message in messages.read() {
            match message.code {
                KeyCode::Char('i') | KeyCode::Esc => {
                    input_state.mode = InputMode::Game;
                }
                KeyCode::Up | KeyCode::Char('w') => {
                    if input_state.inv_selection > 0 {
                        input_state.inv_selection -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('s') => {
                    if item_count > 0 && input_state.inv_selection < item_count - 1 {
                        input_state.inv_selection += 1;
                    }
                }
                KeyCode::Enter => {
                    if item_count > 0 && input_state.inv_selection < item_count {
                        use_item_intents.write(UseItemIntent {
                            user: player_entity,
                            item_index: input_state.inv_selection,
                        });
                        if let Some(next) = &mut next_turn_state {
                            next.set(TurnState::PlayerTurn);
                        }
                        input_state.mode = InputMode::Game;
                        // Adjust selection so it doesn't exceed the new last index.
                        let new_count = item_count.saturating_sub(1);
                        if new_count > 0 && input_state.inv_selection >= new_count {
                            input_state.inv_selection = new_count - 1;
                        } else if new_count == 0 {
                            input_state.inv_selection = 0;
                        }
                    } else {
                        combat_log.push("No item selected.".into());
                    }
                }
                _ => {}
            }
        }
        return;
    }

    // ── Normal game input mode ──────────────────────────────────
    for message in messages.read() {
        // Dismiss the welcome screen on any key press.
        if input_state.welcome_visible {
            input_state.welcome_visible = false;
            continue; // consume the key that dismissed the welcome
        }

        // Handle quit confirmation mode.
        if input_state.quit_confirm {
            match message.code {
                KeyCode::Enter => {
                    exit.write_default();
                }
                _ => {
                    input_state.quit_confirm = false;
                }
            }
            continue;
        }

        // Exhaustive input handling — every arm here corresponds to a KEYBINDINGS entry.
        match message.code {
            // ── Global keys (always active) ─────────────────────
            KeyCode::Char('q') | KeyCode::Esc => {
                input_state.quit_confirm = true;
            }
            KeyCode::Char('p') => {
                let new = match game_state.get() {
                    GameState::Playing => GameState::Paused,
                    GameState::Paused => GameState::Playing,
                    _ => *game_state.get(),
                };
                next_game_state.set(new);
            }
            KeyCode::Char('?') | KeyCode::Char('/') => {
                input_state.help_visible = !input_state.help_visible;
            }
            // ── Open inventory ───────────────────────────────────
            KeyCode::Char('i') if awaiting_input => {
                input_state.mode = InputMode::Inventory;
                input_state.inv_selection = 0;
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
            // ── Wait / skip turn ────────────────────────────────
            KeyCode::Char('.') | KeyCode::Char('5') if awaiting_input => {
                combat_log.push("You wait...".into());
                if let Some(next) = &mut next_turn_state {
                    next.set(TurnState::PlayerTurn);
                }
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
                    combat_log.push("Not enough stamina!".into());
                }
            }
            // ── Targeted ranged attack ──────────────────────────
            KeyCode::Char('r') if awaiting_input => {
                ranged_intents.write(RangedAttackIntent {
                    attacker: player_entity,
                    range: RANGED_ATTACK_RANGE,
                });
                if let Some(next) = &mut next_turn_state {
                    next.set(TurnState::PlayerTurn);
                }
            }
            // ── Melee wide (cleave) attack ──────────────────────
            KeyCode::Char('e') if awaiting_input => {
                melee_wide_intents.write(MeleeWideIntent {
                    attacker: player_entity,
                });
                if let Some(next) = &mut next_turn_state {
                    next.set(TurnState::PlayerTurn);
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
