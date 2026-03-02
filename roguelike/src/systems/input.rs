use bevy::{app::AppExit, ecs::system::SystemParam, prelude::*};
use bevy_ratatui::event::KeyMessage;
use ratatui::crossterm::event::KeyCode;

use crate::components::{Ammo, Hostile, Inventory, ItemKind, Stamina, Player, Position, Viewshed};
use crate::events::{MeleeWideIntent, MolotovCastIntent, MoveIntent, PickupItemIntent, RangedAttackIntent, SpellCastIntent, ThrowItemIntent, UseItemIntent};
use crate::resources::{CombatLog, CursorPosition, DynamicRng, ExtraWorldTicks, GameState, InputMode, InputState, MapSeed, RestartRequested, SpectatingAfterDeath, TurnState};

/// Bundles all intent MessageWriters to stay under Bevy's 16-param system limit.
#[derive(SystemParam)]
pub struct IntentWriters<'w> {
    exit: MessageWriter<'w, AppExit>,
    move_intents: MessageWriter<'w, MoveIntent>,
    spell_intents: MessageWriter<'w, SpellCastIntent>,
    molotov_intents: MessageWriter<'w, MolotovCastIntent>,
    use_item_intents: MessageWriter<'w, UseItemIntent>,
    pickup_intents: MessageWriter<'w, PickupItemIntent>,
    ranged_intents: MessageWriter<'w, RangedAttackIntent>,
    melee_wide_intents: MessageWriter<'w, MeleeWideIntent>,
    throw_item_intents: MessageWriter<'w, ThrowItemIntent>,
}

/// Default radius for the player's grenade blast.
const SPELL_RADIUS: i32 = 3;

/// Stamina cost for throwing a grenade.
const SPELL_STAMINA_COST: i32 = 10;

/// Range for the targeted ranged attack (bullet max travel distance).
const RANGED_ATTACK_RANGE: i32 = 100;

/// Maximum inventory slots for the player.
pub const MAX_INVENTORY_SIZE: usize = 6;

/// Stamina cost for the Dive ability (Z key).
const DIVE_STAMINA_COST: i32 = 20;

/// Number of tiles the Dive ability moves.
const DIVE_DISTANCE: i32 = 3;

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
    CommandBinding { key: "WASD / ↑↓←→", name: "Move (3 ticks)", docs: "Move the player one tile. Physical movement costs 3 ticks." },
    CommandBinding { key: "IJKL", name: "Cursor (1 tick)", docs: "Move the cursor one tile for aiming. Costs 1 tick." },
    CommandBinding { key: "C", name: "Center cursor (1 tick)", docs: "Snap cursor onto your position. Costs 1 tick." },
    CommandBinding { key: "N", name: "Auto-aim (1 tick)", docs: "Cursor steps toward nearest enemy. Costs 1 tick." },
    CommandBinding { key: "R", name: "Reload (6 ticks)", docs: "Reload gun (1 bullet + 1 cap + 1 powder). Cap and ball is slow. Costs 6 ticks." },
    CommandBinding { key: "F", name: "Kick (2 ticks)", docs: "Roundhouse kick all adjacent enemies. Costs 2 ticks." },
    CommandBinding { key: "T", name: "Wait (1 tick)", docs: "Skip your turn. Costs 1 tick." },
    CommandBinding { key: "G", name: "Pick up (1 tick)", docs: "Pick up item at your feet. Costs 1 tick." },
    CommandBinding { key: "Z", name: "Dive (20 sta)", docs: "Move 3 tiles toward cursor instantly. Costs 20 stamina." },
    CommandBinding { key: "1-6", name: "Fire/Use (2 ticks)", docs: "Use item by slot. Guns/grenades fire toward cursor. Costs 2 ticks." },
    CommandBinding { key: "7", name: "Dual wield", docs: "Fire two random revolvers at once." },
    CommandBinding { key: "8", name: "Fan shot", docs: "Fire all rounds from a random revolver." },
    CommandBinding { key: "9", name: "Throw sand", docs: "Create sand cloud blocking vision toward cursor." },
    CommandBinding { key: "0", name: "Throw item", docs: "Throw a random inventory item toward cursor." },
    CommandBinding { key: "Q", name: "Menu / Close", docs: "Close help, toggle pause menu, E then Y to quit." },
    CommandBinding { key: "? /", name: "Help", docs: "Toggle this help screen." },
];

/// Reads keyboard input. Global keys (quit, pause, help) are always handled.
/// Movement keys are only processed while `TurnState::AwaitingInput`,
/// which transitions the game into `PlayerTurn` so that the action is
/// resolved before the next input is accepted.
///
/// When the game is in `GameState::Dead`, only quit (Q) and restart (R) work.
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
    (mut extra_world_ticks, mut spectating, dynamic_rng, seed): (ResMut<ExtraWorldTicks>, ResMut<SpectatingAfterDeath>, Res<DynamicRng>, Res<MapSeed>),
    mut god_mode: ResMut<crate::resources::GodMode>,
) {
    // Handle Dead and Victory states: Q to quit, R to restart, T to spectate.
    if *game_state.get() == GameState::Dead || *game_state.get() == GameState::Victory {
        for message in messages.read() {
            match message.code {
                KeyCode::Char('q') => {
                    intents.exit.write_default();
                }
                KeyCode::Char('r') => {
                    restart_requested.0 = true;
                }
                // Allow watching the game continue after death by pressing wait key (T).
                KeyCode::Char('t') if *game_state.get() == GameState::Dead => {
                    spectating.0 = true;
                    next_game_state.set(GameState::Playing);
                }
                _ => {}
            }
        }
        return;
    }

    let Ok((player_entity, player_pos, player_stamina, _player_ammo, player_inv)) = player_query.single() else {
        // Player entity is gone (should only happen transiently).
        for message in messages.read() {
            if matches!(message.code, KeyCode::Char('q')) {
                intents.exit.write_default();
            }
        }
        return;
    };

    let awaiting_input = turn_state
        .as_ref()
        .is_some_and(|s| *s.get() == TurnState::AwaitingInput);

    // When spectating after death, automatically advance to WorldTurn
    // without waiting for player input.
    if spectating.0 && awaiting_input {
        advance_turn(&mut next_turn_state);
        // Drain pending key messages so they aren't processed as game input.
        let _ = messages.read().count();
        return;
    }

    // ── ESC menu input mode ─────────────────────────────────────
    if input_state.mode == InputMode::EscMenu {
        for message in messages.read() {
            // Handle exit confirmation sub-mode within ESC menu.
            if input_state.quit_confirm {
                match message.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        intents.exit.write_default();
                    }
                    _ => {
                        input_state.quit_confirm = false;
                    }
                }
                continue;
            }

            match message.code {
                KeyCode::Char('q') => {
                    // Resume game (Q toggles ESC menu)
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
                KeyCode::Char('e') | KeyCode::Char('E') => {
                    // Exit — requires Y confirmation
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

        // Handle exit confirmation mode.
        if input_state.quit_confirm {
            match message.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
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
            // ── Q key: close help, toggle ESC menu ──────────────
            KeyCode::Char('q') => {
                if input_state.help_visible {
                    // Close help screen
                    input_state.help_visible = false;
                } else {
                    // Open ESC menu and pause the game.
                    input_state.mode = InputMode::EscMenu;
                    input_state.quit_confirm = false;
                    if *game_state.get() == GameState::Playing {
                        next_game_state.set(GameState::Paused);
                    }
                }
            }
            KeyCode::Char('?') | KeyCode::Char('/') => {
                input_state.help_visible = !input_state.help_visible;
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
            // Normal movement — costs 3 ticks (physical movement is slower)
            KeyCode::Char('w') | KeyCode::Up if awaiting_input => {
                extra_world_ticks.0 = 2;
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, 0, 1);
            }
            KeyCode::Char('s') | KeyCode::Down if awaiting_input => {
                extra_world_ticks.0 = 2;
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, 0, -1);
            }
            KeyCode::Char('a') | KeyCode::Left if awaiting_input => {
                extra_world_ticks.0 = 2;
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, -1, 0);
            }
            KeyCode::Char('d') | KeyCode::Right if awaiting_input => {
                extra_world_ticks.0 = 2;
                emit_move(&mut intents.move_intents, &mut next_turn_state, player_entity, 1, 0);
            }
            // ── Wait / skip turn (T) ────────────────────────────
            KeyCode::Char('t') if awaiting_input => {
                combat_log.push("You wait...".into());
                advance_turn(&mut next_turn_state);
            }
            // ── Reload weapon from inventory magazine — costs 6 ticks ──
            KeyCode::Char('r') if awaiting_input => {
                extra_world_ticks.0 = 5;
                input_state.reload_pending = true;
                advance_turn(&mut next_turn_state);
            }
            // ── Melee wide (roundhouse kick) attack — costs 2 ticks (F key) ────
            KeyCode::Char('f') if awaiting_input => {
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
            // ── Toggle God Mode (Shift+G) ───────────────────────
            KeyCode::Char('G') if awaiting_input => {
                god_mode.0 = !god_mode.0;
                if god_mode.0 {
                    combat_log.push("God mode ENABLED — you are invincible.".into());
                } else {
                    combat_log.push("God mode DISABLED.".into());
                }
            }
            // ── Dive (Z): move 3 tiles toward cursor, costs 20 stamina ──
            KeyCode::Char('z') if awaiting_input => {
                let has_stamina = player_stamina
                    .map(|m| m.current >= DIVE_STAMINA_COST)
                    .unwrap_or(false);
                if !has_stamina {
                    combat_log.push("Not enough stamina to dive!".into());
                } else {
                    let delta = cursor.pos - player_pos.as_grid_vec();
                    if delta == crate::grid_vec::GridVec::ZERO {
                        combat_log.push("Cursor is on your position!".into());
                    } else {
                        let step = delta.king_step();
                        // Emit 3 consecutive move intents in the dive direction
                        for _ in 0..DIVE_DISTANCE {
                            intents.move_intents.write(MoveIntent {
                                entity: player_entity,
                                dx: step.x,
                                dy: step.y,
                            });
                        }
                        input_state.dive_stamina_pending = DIVE_STAMINA_COST;
                        extra_world_ticks.0 = 0;
                        combat_log.push("You dive!".into());
                        advance_turn(&mut next_turn_state);
                    }
                }
            }
            // ── Use inventory item by slot (1-6) / Fire gun toward cursor / Throw / Grenade ──
            // Combat actions cost 2 ticks.
            KeyCode::Char(c @ '1'..='6') if awaiting_input => {
                let idx = (c as usize) - ('1' as usize);
                let mut handled = false;
                if let Some(inv) = player_inv
                    && let Some(&item_entity) = inv.items.get(idx)
                        && let Ok(kind) = item_kind_query.get(item_entity) {
                            if let ItemKind::Gun { loaded, name, .. } = kind {
                                if *loaded > 0 {
                                    let delta = cursor.pos - player_pos.as_grid_vec();
                                    if delta != crate::grid_vec::GridVec::ZERO {
                                        // Double-action revolvers (Starr 1858) cost only 1 tick.
                                        extra_world_ticks.0 = if name.contains("Starr") { 0 } else { 1 };
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
                            } else if let ItemKind::Knife { attack } | ItemKind::Tomahawk { attack } = kind {
                                let delta = cursor.pos - player_pos.as_grid_vec();
                                if delta != crate::grid_vec::GridVec::ZERO {
                                    extra_world_ticks.0 = 1;
                                    intents.throw_item_intents.write(ThrowItemIntent {
                                        thrower: player_entity,
                                        item_entity,
                                        item_index: idx,
                                        dx: delta.x,
                                        dy: delta.y,
                                        range: crate::systems::projectile::THROWN_RANGE,
                                        damage: *attack,
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
                            } else if let ItemKind::Molotov { damage, radius } = kind {
                                // Throw molotov from this inventory slot toward the cursor.
                                let has_stamina = player_stamina
                                    .map(|m| m.current >= SPELL_STAMINA_COST)
                                    .unwrap_or(false);
                                if !has_stamina {
                                    combat_log.push("Not enough stamina!".into());
                                } else {
                                    extra_world_ticks.0 = 1;
                                    intents.molotov_intents.write(MolotovCastIntent {
                                        caster: player_entity,
                                        radius: *radius,
                                        damage: *damage,
                                        target: cursor.pos,
                                        item_index: idx,
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
            // ── Special abilities (7-0) ─────────────────────────
            // 7: Dual wield shot — shoot once using two random revolvers
            KeyCode::Char('7') if awaiting_input => {
                handle_dual_wield(
                    player_entity, player_pos, player_inv, &item_kind_query,
                    &cursor, &mut intents, &mut extra_world_ticks,
                    &mut next_turn_state, &mut combat_log, &dynamic_rng, &seed,
                );
            }
            // 8: Fan shot — fire all rounds from a random revolver
            KeyCode::Char('8') if awaiting_input => {
                handle_fan_shot(
                    player_entity, player_pos, player_inv, &item_kind_query,
                    &cursor, &mut intents, &mut extra_world_ticks,
                    &mut next_turn_state, &mut combat_log, &dynamic_rng, &seed,
                );
            }
            // 9: Throw sand — create sand cloud blocking vision
            KeyCode::Char('9') if awaiting_input => {
                let has_stamina = player_stamina
                    .map(|m| m.current >= 5)
                    .unwrap_or(false);
                if !has_stamina {
                    combat_log.push("Not enough stamina!".into());
                } else {
                    let delta = cursor.pos - player_pos.as_grid_vec();
                    if delta == crate::grid_vec::GridVec::ZERO {
                        combat_log.push("Cursor is on your position!".into());
                    } else {
                        let step = delta.king_step();
                        let sand_center = player_pos.as_grid_vec() + step * 2;
                        // Create sand cloud as spell particles (visual obstruction)
                        intents.spell_intents.write(SpellCastIntent {
                            caster: player_entity,
                            radius: 2,
                            target: sand_center,
                            grenade_index: usize::MAX, // sentinel: no grenade consumed
                        });
                        combat_log.push("You throw a handful of sand!".into());
                        extra_world_ticks.0 = 0;
                        advance_turn(&mut next_turn_state);
                    }
                }
            }
            // 0: Throw random item from inventory toward cursor
            KeyCode::Char('0') if awaiting_input => {
                handle_throw_random(
                    player_entity, player_pos, player_inv, &item_kind_query,
                    &cursor, &mut intents, &mut extra_world_ticks,
                    &mut next_turn_state, &mut combat_log, &dynamic_rng, &seed,
                );
            }
            _ => {}
        }
    }
}

/// Special ability 7: Dual wield shot — fire two random revolvers at once.
#[allow(clippy::too_many_arguments)]
fn handle_dual_wield(
    player_entity: Entity,
    player_pos: &Position,
    player_inv: Option<&Inventory>,
    item_kind_query: &Query<&ItemKind>,
    cursor: &CursorPosition,
    intents: &mut IntentWriters,
    extra_world_ticks: &mut ExtraWorldTicks,
    next_turn_state: &mut Option<ResMut<NextState<TurnState>>>,
    combat_log: &mut CombatLog,
    dynamic_rng: &DynamicRng,
    seed: &MapSeed,
) {
    let delta = cursor.pos - player_pos.as_grid_vec();
    if delta == crate::grid_vec::GridVec::ZERO {
        combat_log.push("Cursor is on your position!".into());
        return;
    }

    let Some(inv) = player_inv else {
        combat_log.push("No inventory!".into());
        return;
    };

    // Find all loaded revolvers in inventory
    let loaded_guns: Vec<Entity> = inv.items.iter()
        .filter(|&&ent| {
            item_kind_query.get(ent).ok().is_some_and(|k| {
                matches!(k, ItemKind::Gun { loaded, .. } if *loaded > 0)
            })
        })
        .copied()
        .collect();

    if loaded_guns.len() < 2 {
        combat_log.push("Need at least 2 loaded revolvers for dual wield!".into());
        return;
    }

    // Pick two random guns using dynamic RNG
    let idx1 = dynamic_rng.random_index(seed.0, 0xDA01, loaded_guns.len());
    let mut idx2 = dynamic_rng.random_index(seed.0, 0xDA02, loaded_guns.len().saturating_sub(1));
    if idx2 >= idx1 { idx2 += 1; }
    idx2 = idx2.min(loaded_guns.len() - 1);

    for &gun in &[loaded_guns[idx1], loaded_guns[idx2]] {
        intents.ranged_intents.write(RangedAttackIntent {
            attacker: player_entity,
            range: RANGED_ATTACK_RANGE,
            dx: delta.x,
            dy: delta.y,
            gun_item: Some(gun),
        });
    }
    extra_world_ticks.0 = 1;
    advance_turn(next_turn_state);
    combat_log.push("Dual wield shot!".into());
}

/// Special ability 8: Fan shot — fire all rounds from a random revolver.
#[allow(clippy::too_many_arguments)]
fn handle_fan_shot(
    player_entity: Entity,
    player_pos: &Position,
    player_inv: Option<&Inventory>,
    item_kind_query: &Query<&ItemKind>,
    cursor: &CursorPosition,
    intents: &mut IntentWriters,
    extra_world_ticks: &mut ExtraWorldTicks,
    next_turn_state: &mut Option<ResMut<NextState<TurnState>>>,
    combat_log: &mut CombatLog,
    dynamic_rng: &DynamicRng,
    seed: &MapSeed,
) {
    let delta = cursor.pos - player_pos.as_grid_vec();
    if delta == crate::grid_vec::GridVec::ZERO {
        combat_log.push("Cursor is on your position!".into());
        return;
    }

    let Some(inv) = player_inv else {
        combat_log.push("No inventory!".into());
        return;
    };

    // Find all loaded revolvers
    let loaded_guns: Vec<(Entity, i32)> = inv.items.iter()
        .filter_map(|&ent| {
            item_kind_query.get(ent).ok().and_then(|k| {
                if let ItemKind::Gun { loaded, .. } = k {
                    if *loaded > 0 { Some((ent, *loaded)) } else { None }
                } else { None }
            })
        })
        .collect();

    if loaded_guns.is_empty() {
        combat_log.push("No loaded revolvers!".into());
        return;
    }

    // Pick random gun
    let idx = dynamic_rng.random_index(seed.0, 0xFA00, loaded_guns.len());
    let (gun, rounds) = loaded_guns[idx];

    // Fire all loaded rounds
    for _ in 0..rounds {
        intents.ranged_intents.write(RangedAttackIntent {
            attacker: player_entity,
            range: RANGED_ATTACK_RANGE,
            dx: delta.x,
            dy: delta.y,
            gun_item: Some(gun),
        });
    }
    extra_world_ticks.0 = 2;
    advance_turn(next_turn_state);
    combat_log.push(format!("Fan shot! {} rounds!", rounds));
}

/// Special ability 0: Throw random inventory item toward cursor.
#[allow(clippy::too_many_arguments)]
fn handle_throw_random(
    player_entity: Entity,
    player_pos: &Position,
    player_inv: Option<&Inventory>,
    item_kind_query: &Query<&ItemKind>,
    cursor: &CursorPosition,
    intents: &mut IntentWriters,
    extra_world_ticks: &mut ExtraWorldTicks,
    next_turn_state: &mut Option<ResMut<NextState<TurnState>>>,
    combat_log: &mut CombatLog,
    dynamic_rng: &DynamicRng,
    seed: &MapSeed,
) {
    let delta = cursor.pos - player_pos.as_grid_vec();
    if delta == crate::grid_vec::GridVec::ZERO {
        combat_log.push("Cursor is on your position!".into());
        return;
    }

    let Some(inv) = player_inv else {
        combat_log.push("No inventory!".into());
        return;
    };

    if inv.items.is_empty() {
        combat_log.push("Inventory is empty!".into());
        return;
    }

    // Pick random item
    let idx = dynamic_rng.random_index(seed.0, 0x7000, inv.items.len());
    let item_entity = inv.items[idx];

    // Determine damage based on item type
    let damage = item_kind_query.get(item_entity).ok().map_or(2, |k| match k {
        ItemKind::Knife { attack } | ItemKind::Tomahawk { attack } => *attack,
        ItemKind::Gun { attack, .. } => *attack / 2,
        _ => 2,
    });

    intents.throw_item_intents.write(ThrowItemIntent {
        thrower: player_entity,
        item_entity,
        item_index: idx,
        dx: delta.x,
        dy: delta.y,
        range: crate::systems::projectile::THROWN_RANGE,
        damage,
    });
    extra_world_ticks.0 = 1;
    advance_turn(next_turn_state);
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
