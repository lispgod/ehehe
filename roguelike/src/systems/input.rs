use bevy::{app::AppExit, ecs::system::SystemParam, prelude::*};
use bevy_ratatui::event::KeyMessage;
use ratatui::crossterm::event::KeyCode;

use crate::components::{Ammo, Hostile, Inventory, ItemKind, Stamina, Player, Position, Viewshed};
use crate::events::{DropItemIntent, MeleeWideIntent, MoveIntent, PickupItemIntent, RangedAttackIntent, SpellCastIntent, ThrowItemIntent, UseItemIntent};
use crate::resources::{CombatLog, CursorPosition, GameState, InputMode, InputState, RestartRequested, TurnState};

/// Bundles all intent MessageWriters to stay under Bevy's 16-param system limit.
#[derive(SystemParam)]
pub struct IntentWriters<'w> {
    exit: MessageWriter<'w, AppExit>,
    move_intents: MessageWriter<'w, MoveIntent>,
    spell_intents: MessageWriter<'w, SpellCastIntent>,
    use_item_intents: MessageWriter<'w, UseItemIntent>,
    pickup_intents: MessageWriter<'w, PickupItemIntent>,
    ranged_intents: MessageWriter<'w, RangedAttackIntent>,
    melee_wide_intents: MessageWriter<'w, MeleeWideIntent>,
    drop_item_intents: MessageWriter<'w, DropItemIntent>,
    throw_item_intents: MessageWriter<'w, ThrowItemIntent>,
}

/// Default radius for the player's grenade blast.
const SPELL_RADIUS: i32 = 3;

/// Stamina cost for throwing a grenade.
const SPELL_STAMINA_COST: i32 = 10;

/// Range for the targeted ranged attack (bullet max travel distance).
const RANGED_ATTACK_RANGE: i32 = 100;

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
    CommandBinding { key: "I/K/J/L", name: "Cursor ↑↓←→", docs: "Move the cursor one tile (used for aiming guns with 1-9)." },
    CommandBinding { key: "N", name: "Auto-aim", docs: "Move cursor one step toward the nearest enemy." },
    CommandBinding { key: "F / Space", name: "Throw grenade", docs: "Throw a grenade from inventory toward the cursor. Warning: can damage you too!" },
    CommandBinding { key: "T", name: "Reload", docs: "Reload weapon from a magazine in your inventory. Current partial magazine is saved to inventory." },
    CommandBinding { key: "E", name: "Roundhouse kick", docs: "Roundhouse kick hitting all adjacent enemies in melee range." },
    CommandBinding { key: "Shift+WASD", name: "Sprint", docs: "Hold Shift while moving to sprint (move 2 tiles per turn)." },
    CommandBinding { key: ". / 5", name: "Wait", docs: "Skip the current turn without acting." },
    CommandBinding { key: "G", name: "Pick up item", docs: "Pick up an item on the ground at your position. Magazines and grenades are auto-picked up on contact." },
    CommandBinding { key: "Tab", name: "Open inventory", docs: "Open the inventory screen to view and use items. Magazines can be used to reload." },
    CommandBinding { key: "1-9", name: "Use/Fire item", docs: "Quickly use an inventory item by slot number. Guns fire toward the cursor." },
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
    mut intents: IntentWriters,
    player_query: Query<(Entity, &Position, Option<&Stamina>, Option<&Ammo>, Option<&Inventory>), With<Player>>,
    mut player_viewshed: Query<&mut Viewshed, With<Player>>,
    item_kind_query: Query<&ItemKind>,
    hostiles_query: Query<&Position, With<Hostile>>,
    game_state: Res<State<GameState>>,
    mut next_game_state: ResMut<NextState<GameState>>,
    turn_state: Option<Res<State<TurnState>>>,
    mut next_turn_state: Option<ResMut<NextState<TurnState>>>,
    mut combat_log: ResMut<CombatLog>,
    mut input_state: ResMut<InputState>,
    mut restart_requested: ResMut<RestartRequested>,
    mut cursor: ResMut<CursorPosition>,
) {
    // Handle Dead and Victory states: only Q/Esc to quit, R to restart.
    if *game_state.get() == GameState::Dead || *game_state.get() == GameState::Victory {
        for message in messages.read() {
            match message.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    intents.exit.write_default();
                }
                KeyCode::Char('r') => {
                    restart_requested.0 = true;
                }
                _ => {}
            }
        }
        return;
    }

    let Ok((player_entity, player_pos, player_stamina, _player_ammo, player_inv)) = player_query.single() else {
        // Player entity is gone (should only happen transiently).
        for message in messages.read() {
            if matches!(message.code, KeyCode::Char('q') | KeyCode::Esc) {
                intents.exit.write_default();
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
                KeyCode::Tab | KeyCode::Esc => {
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
                        intents.use_item_intents.write(UseItemIntent {
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
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    if item_count > 0 && input_state.inv_selection < item_count {
                        intents.drop_item_intents.write(DropItemIntent {
                            user: player_entity,
                            item_index: input_state.inv_selection,
                        });
                        if let Some(next) = &mut next_turn_state {
                            next.set(TurnState::PlayerTurn);
                        }
                        input_state.mode = InputMode::Game;
                        let new_count = item_count.saturating_sub(1);
                        if new_count > 0 && input_state.inv_selection >= new_count {
                            input_state.inv_selection = new_count - 1;
                        } else if new_count == 0 {
                            input_state.inv_selection = 0;
                        }
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
                    intents.exit.write_default();
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
            KeyCode::Tab if awaiting_input => {
                input_state.mode = InputMode::Inventory;
                input_state.inv_selection = 0;
            }
            // ── Cursor movement (IJKL) ──────────────────────────
            KeyCode::Char('i') if awaiting_input => {
                cursor.0.y += 1;
                if let Ok(mut vs) = player_viewshed.single_mut() { vs.dirty = true; }
            }
            KeyCode::Char('k') if awaiting_input => {
                cursor.0.y -= 1;
                if let Ok(mut vs) = player_viewshed.single_mut() { vs.dirty = true; }
            }
            KeyCode::Char('j') if awaiting_input => {
                cursor.0.x -= 1;
                if let Ok(mut vs) = player_viewshed.single_mut() { vs.dirty = true; }
            }
            KeyCode::Char('l') if awaiting_input => {
                cursor.0.x += 1;
                if let Ok(mut vs) = player_viewshed.single_mut() { vs.dirty = true; }
            }
            // ── Auto-aim (N): move cursor one step toward nearest hostile ──
            KeyCode::Char('n') if awaiting_input => {
                let player_vec = player_pos.as_grid_vec();
                let mut best_dist = i32::MAX;
                let mut best_pos = None;
                for hostile_pos in &hostiles_query {
                    let hv = hostile_pos.as_grid_vec();
                    let dist = player_vec.chebyshev_distance(hv);
                    if dist < best_dist {
                        best_dist = dist;
                        best_pos = Some(hv);
                    }
                }
                if let Some(target) = best_pos {
                    let step = (target - cursor.0).king_step();
                    cursor.0 = cursor.0 + step;
                    if let Ok(mut vs) = player_viewshed.single_mut() { vs.dirty = true; }
                } else {
                    combat_log.push("No enemies visible.".into());
                }
            }
            // ── Movement keys (only while awaiting input) ───────
            // Shift+direction = sprint (move 2 tiles at once).
            // Crossterm sends uppercase chars when Shift is held.
            KeyCode::Char('W') if awaiting_input => {
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, 0, 2);
            }
            KeyCode::Char('S') if awaiting_input => {
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, 0, -2);
            }
            KeyCode::Char('A') if awaiting_input => {
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, -2, 0);
            }
            KeyCode::Char('D') if awaiting_input => {
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, 2, 0);
            }
            // Normal movement
            KeyCode::Char('w') | KeyCode::Up if awaiting_input => {
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, 0, 1);
            }
            KeyCode::Char('s') | KeyCode::Down if awaiting_input => {
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, 0, -1);
            }
            KeyCode::Char('a') | KeyCode::Left if awaiting_input => {
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, -1, 0);
            }
            KeyCode::Char('d') | KeyCode::Right if awaiting_input => {
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, 1, 0);
            }
            // ── Wait / skip turn ────────────────────────────────
            KeyCode::Char('.') | KeyCode::Char('5') if awaiting_input => {
                combat_log.push("You wait...".into());
                if let Some(next) = &mut next_turn_state {
                    next.set(TurnState::PlayerTurn);
                }
            }
            // ── Grenade throw: throw a grenade from inventory toward the cursor ──
            KeyCode::Char('f') | KeyCode::Char(' ') if awaiting_input => {
                // Check stamina before throwing grenade.
                let has_stamina = player_stamina
                    .map(|m| m.current >= SPELL_STAMINA_COST)
                    .unwrap_or(false);
                if !has_stamina {
                    combat_log.push("Not enough stamina!".into());
                } else {
                    // Find a grenade in inventory.
                    let grenade_idx = player_inv.and_then(|inv| {
                        inv.items.iter().enumerate().find_map(|(i, &ent)| {
                            if let Ok(kind) = item_kind_query.get(ent) {
                                if matches!(kind, ItemKind::Grenade { .. }) {
                                    return Some(i);
                                }
                            }
                            None
                        })
                    });
                    if let Some(idx) = grenade_idx {
                        intents.spell_intents.write(SpellCastIntent {
                            caster: player_entity,
                            radius: SPELL_RADIUS,
                            target: cursor.0,
                            grenade_index: idx,
                        });
                        if let Some(next) = &mut next_turn_state {
                            next.set(TurnState::PlayerTurn);
                        }
                    } else {
                        combat_log.push("No grenades in inventory!".into());
                    }
                }
            }
            // ── Reload weapon from inventory magazine ───────────
            KeyCode::Char('t') if awaiting_input => {
                input_state.reload_pending = true;
                if let Some(next) = &mut next_turn_state {
                    next.set(TurnState::PlayerTurn);
                }
            }
            // ── Melee wide (cleave) attack ──────────────────────
            KeyCode::Char('e') if awaiting_input => {
                intents.melee_wide_intents.write(MeleeWideIntent {
                    attacker: player_entity,
                });
                if let Some(next) = &mut next_turn_state {
                    next.set(TurnState::PlayerTurn);
                }
            }
            // ── Pickup item on ground ───────────────────────────
            KeyCode::Char('g') if awaiting_input => {
                intents.pickup_intents.write(PickupItemIntent {
                    picker: player_entity,
                });
                if let Some(next) = &mut next_turn_state {
                    next.set(TurnState::PlayerTurn);
                }
            }
            // ── Use inventory item by slot (1-9) / Fire gun toward cursor / Throw ──
            KeyCode::Char(c @ '1'..='9') if awaiting_input => {
                let idx = (c as usize) - ('1' as usize);
                let mut handled = false;
                if let Some(inv) = player_inv {
                    if let Some(&item_entity) = inv.items.get(idx) {
                        if let Ok(kind) = item_kind_query.get(item_entity) {
                            if let ItemKind::Gun { loaded, .. } = kind {
                                if *loaded > 0 {
                                    let delta = cursor.0 - player_pos.as_grid_vec();
                                    if delta != crate::grid_vec::GridVec::ZERO {
                                        intents.ranged_intents.write(RangedAttackIntent {
                                            attacker: player_entity,
                                            range: RANGED_ATTACK_RANGE,
                                            dx: delta.x,
                                            dy: delta.y,
                                            gun_item: Some(item_entity),
                                        });
                                        if let Some(next) = &mut next_turn_state {
                                            next.set(TurnState::PlayerTurn);
                                        }
                                        handled = true;
                                    } else {
                                        combat_log.push("Cursor is on your position!".into());
                                        handled = true;
                                    }
                                } else {
                                    combat_log.push("Gun is empty! Reload in inventory mode.".into());
                                    handled = true;
                                }
                            } else if matches!(kind, ItemKind::Knife { .. } | ItemKind::Tomahawk { .. }) {
                                let delta = cursor.0 - player_pos.as_grid_vec();
                                if delta != crate::grid_vec::GridVec::ZERO {
                                    let (range, damage) = match kind {
                                        ItemKind::Knife { attack } => (8, *attack),
                                        ItemKind::Tomahawk { attack } => (6, *attack),
                                        _ => unreachable!(),
                                    };
                                    intents.throw_item_intents.write(ThrowItemIntent {
                                        thrower: player_entity,
                                        item_entity,
                                        item_index: idx,
                                        dx: delta.x,
                                        dy: delta.y,
                                        range,
                                        damage,
                                    });
                                    if let Some(next) = &mut next_turn_state {
                                        next.set(TurnState::PlayerTurn);
                                    }
                                    handled = true;
                                } else {
                                    combat_log.push("Cursor is on your position!".into());
                                    handled = true;
                                }
                            }
                        }
                    }
                }
                if !handled {
                    // Non-gun items: use normally.
                    intents.use_item_intents.write(UseItemIntent {
                        user: player_entity,
                        item_index: idx,
                    });
                    if let Some(next) = &mut next_turn_state {
                        next.set(TurnState::PlayerTurn);
                    }
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
