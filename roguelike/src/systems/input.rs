use bevy::{app::AppExit, ecs::system::SystemParam, prelude::*};
use bevy_ratatui::event::KeyMessage;
use ratatui::crossterm::event::KeyCode;

use crate::components::{Ammo, Hostile, Inventory, ItemKind, Stamina, Player, Position, Viewshed};
use crate::events::{DropItemIntent, MeleeWideIntent, MoveIntent, PickupItemIntent, RangedAttackIntent, SpellCastIntent, ThrowItemIntent, UseItemIntent};
use crate::resources::{CombatLog, CursorPosition, ExtraWorldTicks, GameState, InputMode, InputState, RestartRequested, TurnState};

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
/// Related keys are grouped (WASD, IJKL) to reduce visual clutter.
pub const KEYBINDINGS: &[CommandBinding] = &[
    CommandBinding { key: "WASD / ↑↓←→", name: "Move (2 ticks)", docs: "Move the player one tile. Physical movement costs 2 ticks." },
    CommandBinding { key: "IJKL", name: "Cursor (1 tick)", docs: "Move the cursor one tile for aiming. Costs 1 tick." },
    CommandBinding { key: "C", name: "Center cursor (1 tick)", docs: "Snap cursor onto your position. Costs 1 tick." },
    CommandBinding { key: "N", name: "Auto-aim (1 tick)", docs: "Cursor steps toward nearest enemy. Costs 1 tick." },
    CommandBinding { key: "R", name: "Reload (1 tick)", docs: "Reload gun (1 bullet + 1 cap + 1 powder). Costs 1 tick." },
    CommandBinding { key: "E", name: "Kick (2 ticks)", docs: "Roundhouse kick all adjacent enemies. Costs 2 ticks." },
    CommandBinding { key: ".", name: "Wait (1 tick)", docs: "Skip your turn. Costs 1 tick." },
    CommandBinding { key: "G", name: "Pick up (1 tick)", docs: "Pick up item at your feet. Costs 1 tick." },
    CommandBinding { key: "B", name: "Inventory", docs: "Open inventory. D:Drop R:Reload Enter:Use." },
    CommandBinding { key: "1-9", name: "Fire/Use (2 ticks)", docs: "Use item by slot. Guns/grenades fire toward cursor. Costs 2 ticks." },
    CommandBinding { key: "Esc", name: "Menu", docs: "Pause menu (Resume / Restart / Quit)." },
    CommandBinding { key: "? /", name: "Help", docs: "Toggle this help screen." },
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
    mut extra_world_ticks: ResMut<ExtraWorldTicks>,
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
                KeyCode::Char('b') | KeyCode::Esc => {
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
                        advance_turn(&mut next_turn_state);
                        input_state.mode = InputMode::Game;
                        clamp_inv_selection(&mut input_state.inv_selection, item_count.saturating_sub(1));
                    } else {
                        combat_log.push("No item selected.".into());
                    }
                }
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    // Drop the selected item. Intentionally keeps the inventory
                    // open (no `mode = InputMode::Game`) so the player can drop
                    // multiple items without reopening the menu each time.
                    if item_count > 0 && input_state.inv_selection < item_count {
                        intents.drop_item_intents.write(DropItemIntent {
                            user: player_entity,
                            item_index: input_state.inv_selection,
                        });
                        advance_turn(&mut next_turn_state);
                        clamp_inv_selection(&mut input_state.inv_selection, item_count.saturating_sub(1));
                    }
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    // Reload from inside the inventory — inventory stays open.
                    input_state.reload_pending = true;
                    advance_turn(&mut next_turn_state);
                }
                _ => {}
            }
        }
        return;
    }

    // ── ESC menu input mode ─────────────────────────────────────
    if input_state.mode == InputMode::EscMenu {
        for message in messages.read() {
            // Handle quit confirmation sub-mode within ESC menu.
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

            match message.code {
                KeyCode::Esc => {
                    // Resume game
                    input_state.mode = InputMode::Game;
                    if *game_state.get() == GameState::Paused {
                        next_game_state.set(GameState::Playing);
                    }
                }
                KeyCode::Char('r') => {
                    // Restart
                    input_state.mode = InputMode::Game;
                    restart_requested.0 = true;
                }
                KeyCode::Char('q') => {
                    // Quit — requires Enter confirmation
                    input_state.quit_confirm = true;
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
            KeyCode::Esc => {
                // Open ESC menu and pause the game.
                input_state.mode = InputMode::EscMenu;
                input_state.quit_confirm = false;
                if *game_state.get() == GameState::Playing {
                    next_game_state.set(GameState::Paused);
                }
            }
            KeyCode::Char('?') | KeyCode::Char('/') => {
                input_state.help_visible = !input_state.help_visible;
            }
            // ── Open inventory ───────────────────────────────────
            KeyCode::Char('b') if awaiting_input => {
                input_state.mode = InputMode::Inventory;
                input_state.inv_selection = 0;
            }
            // ── Cursor movement (IJKL) — advances one tick ─────
            KeyCode::Char('i') if awaiting_input => {
                move_cursor(&mut cursor, 0, 1, &mut player_viewshed, &mut next_turn_state);
            }
            KeyCode::Char('k') if awaiting_input => {
                move_cursor(&mut cursor, 0, -1, &mut player_viewshed, &mut next_turn_state);
            }
            KeyCode::Char('j') if awaiting_input => {
                move_cursor(&mut cursor, -1, 0, &mut player_viewshed, &mut next_turn_state);
            }
            KeyCode::Char('l') if awaiting_input => {
                move_cursor(&mut cursor, 1, 0, &mut player_viewshed, &mut next_turn_state);
            }
            // ── Center cursor on player (C) — advances one tick ──
            KeyCode::Char('c') if awaiting_input => {
                cursor.pos = player_pos.as_grid_vec();
                mark_viewshed_dirty(&mut player_viewshed);
                advance_turn(&mut next_turn_state);
            }
            // ── Auto-aim (N): move cursor one step toward nearest hostile — advances one tick ──
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
                    let step = (target - cursor.pos).king_step();
                    cursor.pos += step;
                    mark_viewshed_dirty(&mut player_viewshed);
                    advance_turn(&mut next_turn_state);
                } else {
                    combat_log.push("No enemies visible.".into());
                }
            }
            // ── Movement keys (only while awaiting input) ───────
            // Normal movement — costs 2 ticks (physical movement is slower)
            KeyCode::Char('w') | KeyCode::Up if awaiting_input => {
                extra_world_ticks.0 = 1;
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, 0, 1);
            }
            KeyCode::Char('s') | KeyCode::Down if awaiting_input => {
                extra_world_ticks.0 = 1;
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, 0, -1);
            }
            KeyCode::Char('a') | KeyCode::Left if awaiting_input => {
                extra_world_ticks.0 = 1;
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, -1, 0);
            }
            KeyCode::Char('d') | KeyCode::Right if awaiting_input => {
                extra_world_ticks.0 = 1;
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, 1, 0);
            }
            // ── Wait / skip turn ────────────────────────────────
            KeyCode::Char('.') if awaiting_input => {
                combat_log.push("You wait...".into());
                advance_turn(&mut next_turn_state);
            }
            // ── Reload weapon from inventory magazine ───────────
            KeyCode::Char('r') if awaiting_input => {
                input_state.reload_pending = true;
                advance_turn(&mut next_turn_state);
            }
            // ── Melee wide (cleave) attack — costs 2 ticks ────
            KeyCode::Char('e') if awaiting_input => {
                extra_world_ticks.0 = 1;
                intents.melee_wide_intents.write(MeleeWideIntent {
                    attacker: player_entity,
                });
                advance_turn(&mut next_turn_state);
            }
            // ── Pickup item on ground ───────────────────────────
            KeyCode::Char('g') if awaiting_input => {
                intents.pickup_intents.write(PickupItemIntent {
                    picker: player_entity,
                });
                advance_turn(&mut next_turn_state);
            }
            // ── Use inventory item by slot (1-9) / Fire gun toward cursor / Throw / Grenade ──
            // Combat actions cost 2 ticks.
            KeyCode::Char(c @ '1'..='9') if awaiting_input => {
                let idx = (c as usize) - ('1' as usize);
                let mut handled = false;
                if let Some(inv) = player_inv
                    && let Some(&item_entity) = inv.items.get(idx)
                        && let Ok(kind) = item_kind_query.get(item_entity) {
                            if let ItemKind::Gun { loaded, .. } = kind {
                                if *loaded > 0 {
                                    let delta = cursor.pos - player_pos.as_grid_vec();
                                    if delta != crate::grid_vec::GridVec::ZERO {
                                        extra_world_ticks.0 = 1;
                                        intents.ranged_intents.write(RangedAttackIntent {
                                            attacker: player_entity,
                                            range: RANGED_ATTACK_RANGE,
                                            dx: delta.x,
                                            dy: delta.y,
                                            gun_item: Some(item_entity),
                                        });
                                        advance_turn(&mut next_turn_state);
                                        handled = true;
                                    } else {
                                        combat_log.push("Cursor is on your position!".into());
                                        handled = true;
                                    }
                                } else {
                                    combat_log.push("Gun is empty! Press R to reload.".into());
                                    handled = true;
                                }
                            } else if matches!(kind, ItemKind::Knife { .. } | ItemKind::Tomahawk { .. }) {
                                let delta = cursor.pos - player_pos.as_grid_vec();
                                if delta != crate::grid_vec::GridVec::ZERO {
                                    let (range, damage) = match kind {
                                        ItemKind::Knife { attack } => (8, *attack),
                                        ItemKind::Tomahawk { attack } => (6, *attack),
                                        _ => unreachable!(),
                                    };
                                    extra_world_ticks.0 = 1;
                                    intents.throw_item_intents.write(ThrowItemIntent {
                                        thrower: player_entity,
                                        item_entity,
                                        item_index: idx,
                                        dx: delta.x,
                                        dy: delta.y,
                                        range,
                                        damage,
                                    });
                                    advance_turn(&mut next_turn_state);
                                    handled = true;
                                } else {
                                    combat_log.push("Cursor is on your position!".into());
                                    handled = true;
                                }
                            } else if matches!(kind, ItemKind::Grenade { .. }) {
                                // Throw grenade from this inventory slot toward the cursor.
                                let has_stamina = player_stamina
                                    .map(|m| m.current >= SPELL_STAMINA_COST)
                                    .unwrap_or(false);
                                if !has_stamina {
                                    combat_log.push("Not enough stamina!".into());
                                } else {
                                    extra_world_ticks.0 = 1;
                                    intents.spell_intents.write(SpellCastIntent {
                                        caster: player_entity,
                                        radius: SPELL_RADIUS,
                                        target: cursor.pos,
                                        grenade_index: idx,
                                    });
                                    advance_turn(&mut next_turn_state);
                                }
                                handled = true;
                            }
                        }
                if !handled {
                    // Non-gun items: use normally.
                    intents.use_item_intents.write(UseItemIntent {
                        user: player_entity,
                        item_index: idx,
                    });
                    advance_turn(&mut next_turn_state);
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
    advance_turn(next_turn_state);
}

/// Helper: transitions to `PlayerTurn`, ending the input phase.
#[inline]
fn advance_turn(next_turn_state: &mut Option<ResMut<NextState<TurnState>>>) {
    if let Some(next) = next_turn_state {
        next.set(TurnState::PlayerTurn);
    }
}

/// Helper: marks the player's viewshed as dirty so FOV is recalculated.
#[inline]
fn mark_viewshed_dirty(player_viewshed: &mut Query<&mut Viewshed, With<Player>>) {
    if let Ok(mut vs) = player_viewshed.single_mut() {
        vs.dirty = true;
    }
}

/// Helper: moves the cursor by `(dx, dy)`, marks viewshed dirty, and advances the turn.
#[inline]
fn move_cursor(
    cursor: &mut ResMut<CursorPosition>,
    dx: i32,
    dy: i32,
    player_viewshed: &mut Query<&mut Viewshed, With<Player>>,
    next_turn_state: &mut Option<ResMut<NextState<TurnState>>>,
) {
    cursor.pos.x += dx;
    cursor.pos.y += dy;
    mark_viewshed_dirty(player_viewshed);
    advance_turn(next_turn_state);
}

/// Helper: clamps the inventory selection index after an item is consumed/dropped.
/// `new_count` is the inventory length *after* removal.
#[inline]
fn clamp_inv_selection(selection: &mut usize, new_count: usize) {
    if new_count == 0 {
        *selection = 0;
    } else if *selection >= new_count {
        *selection = new_count - 1;
    }
}
