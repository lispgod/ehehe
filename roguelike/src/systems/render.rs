use std::collections::HashSet;

use bevy::prelude::*;
use bevy_ratatui::RatatuiContext;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph, Row, Table, Wrap};

use crate::components::{Ammo, Experience, Health, Hostile, Inventory, ItemKind, Level, Stamina, Name, Player, Position, Renderable, Viewshed};
use crate::grid_vec::GridVec;
use crate::resources::{
    CameraPosition, Collectibles, CombatLog, CursorPosition, GameMapResource, GameState, InputMode,
    InputState, KillCount, SpellParticles, TurnCounter,
};
use crate::systems::input::KEYBINDINGS;
use crate::typedefs::{CoordinateUnit, MyPoint, RatColor};

/// Lifetime (in frames) for combat particle animations.
/// Must match the lifetime used in spell.rs when creating particles.
const PARTICLE_LIFETIME: f32 = 8.0;


/// Ticks and renders combat particles each frame.
pub fn particle_tick_system(mut particles: ResMut<SpellParticles>) {
    particles.tick();
}

/// Renders the game map and all `Renderable` entities to the terminal.
/// Uses the player's `Viewshed` to determine tile visibility, and the
/// `revealed_tiles` set for fog-of-war memory (dimmed rendering).
///
/// Layout:
/// ┌─────────────────────────────────────────────┐
/// │              Game Area (full width)          │
/// ├────────┬────────────────────────┬───────────┤
/// │ Stats  │    Central Log         │  Info     │
/// │ HP/STA │    (combat log)        │  Inv/Vis  │
/// │ Ammo   │                       │           │
/// └────────┴────────────────────────┴───────────┘
pub fn draw_system(
    mut context: ResMut<RatatuiContext>,
    game_map: Res<GameMapResource>,
    camera: Res<CameraPosition>,
    renderables: Query<(&Position, &Renderable, Option<&Name>)>,
    player_query: Query<
        (&Position, Option<&Viewshed>, Option<&Health>, Option<&Stamina>, Option<&Ammo>, Option<&Inventory>, Option<&Level>, Option<&Experience>),
        With<Player>,
    >,
    item_query: Query<(Option<&Name>, Option<&ItemKind>), With<crate::components::Item>>,
    hostile_viewsheds: Query<&Viewshed, With<Hostile>>,
    state: Res<State<GameState>>,
    combat_log: Res<CombatLog>,
    turn_counter: Res<TurnCounter>,
    kill_count: Res<KillCount>,
    spell_particles: Res<SpellParticles>,
    input_state: Res<InputState>,
    cursor: Res<CursorPosition>,
    collectibles: Res<Collectibles>,
) -> Result {
    context.draw(|frame| {
        let area = frame.area();

        // ── Top-level layout: game area + bottom panel ──────────
        let bottom_panel_height = 10u16;
        let vert_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(bottom_panel_height)])
            .split(area);

        let game_area = vert_chunks[0];
        let bottom_area = vert_chunks[1];

        let render_width = game_area.width;
        let render_height = game_area.height;

        // Collect the player's visible and revealed tiles.
        let (visible_tiles, revealed_tiles, player_hp, player_stamina, player_ammo, player_inv, player_level, player_exp): (
            Option<&HashSet<MyPoint>>,
            Option<&HashSet<MyPoint>>,
            Option<&Health>,
            Option<&Stamina>,
            Option<&Ammo>,
            Option<&Inventory>,
            Option<&Level>,
            Option<&Experience>,
        ) = player_query
            .single()
            .ok()
            .map(|(_, vs, hp, sta, ammo, inv, lvl, exp)| {
                let (vis, rev) = vs
                    .map(|vs| (Some(&vs.visible_tiles), Some(&vs.revealed_tiles)))
                    .unwrap_or((None, None));
                (vis, rev, hp, sta, ammo, inv, lvl, exp)
            })
            .unwrap_or((None, None, None, None, None, None, None, None));

        let mut render_packet = game_map.0.create_render_packet_with_fog(
            &camera.0,
            render_width,
            render_height,
            visible_tiles,
            revealed_tiles,
        );

        // Overlay all renderable entities at their screen-relative positions
        let w_radius = render_width as CoordinateUnit / 2;
        let h_radius = render_height as CoordinateUnit / 2;
        let bottom_left = camera.0 - GridVec::new(w_radius, h_radius);

        // Brighten tiles visible to hostile entities (enemy FOV visualization).
        {
            let mut enemy_visible: HashSet<MyPoint> = HashSet::new();
            for vs in &hostile_viewsheds {
                enemy_visible.extend(&vs.visible_tiles);
            }
            for (screen_y, row) in render_packet.iter_mut().enumerate() {
                for (screen_x, cell) in row.iter_mut().enumerate() {
                    let world = bottom_left + GridVec::new(screen_x as i32, screen_y as i32);
                    let in_player_view = visible_tiles
                        .map(|vt| vt.contains(&world))
                        .unwrap_or(false);
                    if in_player_view && enemy_visible.contains(&world) {
                        if let RatColor::Rgb(r, g, b) = cell.2 {
                            cell.2 = RatColor::Rgb(
                                r.saturating_add(15),
                                g.saturating_add(10),
                                b.saturating_add(5),
                            );
                        }
                    }
                }
            }
        }

        // Collect visible entities for the info panel.
        let mut visible_entity_infos: Vec<(String, RatColor, RatColor, String)> = Vec::new();

        for (pos, renderable, name) in &renderables {
            let screen = pos.as_grid_vec() - bottom_left;

            if screen.x >= 0
                && screen.x < render_width as CoordinateUnit
                && screen.y >= 0
                && screen.y < render_height as CoordinateUnit
            {
                // Only draw entities that are currently visible (not merely revealed)
                let entity_visible = visible_tiles
                    .map(|vt| vt.contains(&pos.as_grid_vec()))
                    .unwrap_or(true);
                if entity_visible {
                    let bg = render_packet[screen.y as usize][screen.x as usize].2;
                    render_packet[screen.y as usize][screen.x as usize] =
                        (renderable.symbol.clone(), renderable.fg, bg);

                    // Collect for visible entities panel.
                    let full_name = name.map_or("???".to_string(), |n| n.0.clone());
                    visible_entity_infos.push((
                        renderable.symbol.clone(),
                        renderable.fg,
                        renderable.bg,
                        full_name,
                    ));
                }
            }
        }

        // Overlay combat particles on the render packet.
        for (particle_pos, lifetime, delay) in &spell_particles.particles {
            if *delay > 0 {
                continue; // not yet visible
            }
            let screen = *particle_pos - bottom_left;
            if screen.x >= 0
                && screen.x < render_width as CoordinateUnit
                && screen.y >= 0
                && screen.y < render_height as CoordinateUnit
            {
                let visible = visible_tiles
                    .map(|vt| vt.contains(particle_pos))
                    .unwrap_or(true);
                if visible {
                    // Particle symbol and color fade with lifetime.
                    let intensity = (*lifetime as f32 / PARTICLE_LIFETIME).min(1.0);
                    let r = (255.0 * intensity) as u8;
                    let g = (165.0 * intensity) as u8;
                    let symbol = if *lifetime > 4 { "✦" } else if *lifetime > 2 { "*" } else { "·" };
                    let bg = render_packet[screen.y as usize][screen.x as usize].2;
                    render_packet[screen.y as usize][screen.x as usize] =
                        (symbol.into(), RatColor::Rgb(r, g, 0), bg);
                }
            }
        }

        // Overlay cursor position with blink effect.
        {
            let cursor_screen = cursor.0 - bottom_left;
            if cursor_screen.x >= 0
                && cursor_screen.x < render_width as CoordinateUnit
                && cursor_screen.y >= 0
                && cursor_screen.y < render_height as CoordinateUnit
            {
                let bg = render_packet[cursor_screen.y as usize][cursor_screen.x as usize].2;
                // Blink: alternate symbol/color every 2 ticks.
                let blink_on = (turn_counter.0 / 2) % 2 == 0;
                let (sym, fg) = if blink_on {
                    ("X", RatColor::Rgb(255, 255, 0))
                } else {
                    ("+", RatColor::Rgb(180, 180, 0))
                };
                render_packet[cursor_screen.y as usize][cursor_screen.x as usize] =
                    (sym.into(), fg, bg);
            }
        }

        let mut render_lines = Vec::new();

        for y in 0..render_height as usize {
            if y < render_packet.len() {
                let spans: Vec<Span> = render_packet[y]
                    .iter()
                    .map(|gt| Span::from(gt.0.clone()).fg(gt.1).bg(gt.2))
                    .collect();
                render_lines.push(Line::from(spans));
            }
        }

        // Reverse so that higher Y values are at the top (standard roguelike convention)
        render_lines.reverse();

        frame.render_widget(Paragraph::new(Text::from(render_lines)).on_black(), game_area);

        // Collect inventory item names and kinds for the bottom panel and inventory overlay.
        let inv_item_info: Vec<(String, String)> = player_inv
            .map(|inv| {
                inv.items
                    .iter()
                    .map(|&ent| {
                        let name = item_query
                            .get(ent)
                            .ok()
                            .and_then(|(n, _)| n)
                            .map_or("item".to_string(), |n| n.0.clone());
                        let desc = item_query
                            .get(ent)
                            .ok()
                            .and_then(|(_, k)| k)
                            .map_or("".to_string(), |k| match k {
                                ItemKind::Gun { loaded, capacity, caliber, .. } => format!("{loaded}/{capacity} {caliber}"),
                                ItemKind::Knife { attack } => format!("+{attack} atk"),
                                ItemKind::Tomahawk { attack } => format!("+{attack} atk"),
                                ItemKind::Grenade { damage, radius } => format!("{damage} dmg r{radius}"),
                                ItemKind::Whiskey { heal } => format!("Heal {heal} HP"),
                                ItemKind::Hat { defense } => format!("+{defense} def"),
                            });
                        (name, desc)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let inv_item_names: Vec<String> = inv_item_info.iter().map(|(n, _)| n.clone()).collect();

        // ── Bottom Panel ────────────────────────────────────────
        render_bottom_panel(
            frame,
            bottom_area,
            player_hp,
            player_stamina,
            player_ammo,
            player_inv,
            &inv_item_names,
            &visible_entity_infos,
            &combat_log,
            player_level,
            player_exp,
            &turn_counter,
            &kill_count,
            &collectibles,
        );

        // ── Overlays ────────────────────────────────────────────

        // Show "PAUSED" overlay centered on game area when paused
        if *state.get() == GameState::Paused {
            let label = " PAUSED — press P to resume ";
            let label_width = label.len() as u16;
            if render_width >= label_width && render_height >= 1 {
                let cx = game_area.x + (render_width - label_width) / 2;
                let cy = game_area.y + render_height / 2;
                let pause_area = Rect {
                    x: cx,
                    y: cy,
                    width: label_width,
                    height: 1,
                };
                frame.render_widget(
                    Paragraph::new(Line::from(label).bold()).on_dark_gray(),
                    pause_area,
                );
            }
        }

        // Show "VICTORY" overlay centered on game area when the gate is destroyed
        if *state.get() == GameState::Victory {
            let label = " VICTORY! The Outlaw Hideout has been destroyed! Press Q to quit, R to restart. ";
            let label_width = label.len() as u16;
            if render_width >= label_width && render_height >= 1 {
                let cx = game_area.x + (render_width - label_width) / 2;
                let cy = game_area.y + render_height / 2;
                let victory_area = Rect {
                    x: cx,
                    y: cy,
                    width: label_width,
                    height: 1,
                };
                frame.render_widget(
                    Paragraph::new(Line::from(label).bold()).on_yellow(),
                    victory_area,
                );
            }
        }

        // Show "YOU DIED" overlay when the player has fallen
        if *state.get() == GameState::Dead {
            let label = " YOU DIED — Press Q to quit, R to restart ";
            let label_width = label.len() as u16;
            if render_width >= label_width && render_height >= 1 {
                let cx = game_area.x + (render_width - label_width) / 2;
                let cy = game_area.y + render_height / 2;
                let death_area = Rect {
                    x: cx,
                    y: cy,
                    width: label_width,
                    height: 1,
                };
                frame.render_widget(
                    Paragraph::new(Line::from(label).bold()).on_red(),
                    death_area,
                );
            }
        }

        // Show quit confirmation overlay
        if input_state.quit_confirm {
            let label = " Quit? Press Enter to confirm, any other key to cancel ";
            let label_width = label.len() as u16;
            if render_width >= label_width && render_height >= 1 {
                let cx = game_area.x + (render_width - label_width) / 2;
                let cy = game_area.y + render_height / 2;
                let quit_area = Rect {
                    x: cx,
                    y: cy,
                    width: label_width,
                    height: 1,
                };
                frame.render_widget(
                    Paragraph::new(Line::from(label).bold()).on_dark_gray(),
                    quit_area,
                );
            }
        }

        // Show help overlay when toggled
        if input_state.help_visible {
            render_help_overlay(frame, game_area);
        }

        // Show welcome screen at game start
        if input_state.welcome_visible {
            render_welcome_overlay(frame, game_area);
        }

        // Show inventory overlay when in Inventory input mode
        if input_state.mode == InputMode::Inventory {
            render_inventory_overlay(frame, game_area, &inv_item_info, input_state.inv_selection);
        }
    })?;

    Ok(())
}

/// Renders the bottom panel with stats, central combat log, and info.
/// Layout: [Stats | Central Log | Info]
fn render_bottom_panel(
    frame: &mut ratatui::Frame,
    area: Rect,
    player_hp: Option<&Health>,
    player_stamina: Option<&Stamina>,
    player_ammo: Option<&Ammo>,
    player_inv: Option<&Inventory>,
    inv_item_names: &[String],
    visible_entities: &[(String, RatColor, RatColor, String)],
    combat_log: &CombatLog,
    player_level: Option<&Level>,
    player_exp: Option<&Experience>,
    turn_counter: &TurnCounter,
    kill_count: &KillCount,
    collectibles: &Collectibles,
) {
    // Split bottom panel into three horizontal columns: stats | log | info
    let horiz_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(22),   // Stats column (HP, Stamina, Ammo, Level)
            Constraint::Min(1),       // Central log (wide, fills remaining space)
            Constraint::Length(22),   // Info column (Inventory, Visible entities)
        ])
        .split(area);

    let stats_area = horiz_chunks[0];
    let log_area = horiz_chunks[1];
    let info_area = horiz_chunks[2];

    // ── Stats Column (left) ─────────────────────────────────────
    render_stats_column(frame, stats_area, player_hp, player_stamina, player_ammo, player_level, player_exp, collectibles);

    // ── Central Log (middle) ────────────────────────────────────
    let log_height = log_area.height.saturating_sub(2) as usize; // subtract border
    let log_lines: Vec<Line> = combat_log
        .recent(log_height.max(1))
        .into_iter()
        .map(|s| Line::from(format!(" {s}")).dark_gray())
        .collect();

    let title = format!(" Log | Tick:{} Kills:{} | ?:help ", turn_counter.0, kill_count.0);
    frame.render_widget(
        Paragraph::new(if log_lines.is_empty() {
            vec![Line::from(" (no events)".dark_gray())]
        } else {
            log_lines
        })
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: true }),
        log_area,
    );

    // ── Info Column (right) ─────────────────────────────────────
    render_info_column(frame, info_area, player_inv, inv_item_names, visible_entities);
}

/// Renders the stats column (HP, Stamina, Ammo, Level gauges stacked vertically).
fn render_stats_column(
    frame: &mut ratatui::Frame,
    area: Rect,
    player_hp: Option<&Health>,
    player_stamina: Option<&Stamina>,
    player_ammo: Option<&Ammo>,
    player_level: Option<&Level>,
    player_exp: Option<&Experience>,
    collectibles: &Collectibles,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // HP gauge (compact, no border)
            Constraint::Length(1), // Stamina gauge
            Constraint::Length(1), // Ammo gauge
            Constraint::Length(1), // EXP gauge
            Constraint::Length(1), // Collectibles line
            Constraint::Min(0),   // padding
        ])
        .split(Block::default().borders(Borders::ALL).title("Stats").inner(area));

    frame.render_widget(
        Block::default().borders(Borders::ALL).title("Stats"),
        area,
    );

    // HP
    if let Some(hp) = player_hp {
        let ratio = if hp.max > 0 { (hp.current as f64 / hp.max as f64).clamp(0.0, 1.0) } else { 0.0 };
        let gauge = Gauge::default()
            .gauge_style(ratatui::style::Style::default().fg(ratatui::style::Color::Red).bg(ratatui::style::Color::DarkGray))
            .ratio(ratio)
            .label(Span::from(format!("HP {}/{}", hp.current, hp.max)).style(ratatui::style::Style::default().fg(ratatui::style::Color::White)));
        frame.render_widget(gauge, chunks[0]);
    }

    // Stamina
    if let Some(stamina) = player_stamina {
        let ratio = if stamina.max > 0 { (stamina.current as f64 / stamina.max as f64).clamp(0.0, 1.0) } else { 0.0 };
        let gauge = Gauge::default()
            .gauge_style(ratatui::style::Style::default().fg(ratatui::style::Color::Blue).bg(ratatui::style::Color::DarkGray))
            .ratio(ratio)
            .label(Span::from(format!("STA {}/{}", stamina.current, stamina.max)).style(ratatui::style::Style::default().fg(ratatui::style::Color::White)));
        frame.render_widget(gauge, chunks[1]);
    }

    // Ammo
    if let Some(ammo) = player_ammo {
        let ratio = if ammo.max > 0 { (ammo.current as f64 / ammo.max as f64).clamp(0.0, 1.0) } else { 0.0 };
        let gauge = Gauge::default()
            .gauge_style(ratatui::style::Style::default().fg(ratatui::style::Color::Yellow).bg(ratatui::style::Color::DarkGray))
            .ratio(ratio)
            .label(Span::from(format!("AMMO {}/{}", ammo.current, ammo.max)).style(ratatui::style::Style::default().fg(ratatui::style::Color::White)));
        frame.render_widget(gauge, chunks[2]);
    }

    // EXP
    if let (Some(exp), Some(level)) = (player_exp, player_level) {
        let ratio = if exp.next_level > 0 { (exp.current as f64 / exp.next_level as f64).clamp(0.0, 1.0) } else { 0.0 };
        let gauge = Gauge::default()
            .gauge_style(ratatui::style::Style::default().fg(ratatui::style::Color::Green).bg(ratatui::style::Color::DarkGray))
            .ratio(ratio)
            .label(Span::from(format!("Lv.{} {}/{}", level.0, exp.current, exp.next_level)).style(ratatui::style::Style::default().fg(ratatui::style::Color::White)));
        frame.render_widget(gauge, chunks[3]);
    }

    // Collectibles
    let coll_text = format!(
        "Cap:{} Pdr:{} .36:{} .44:{}",
        collectibles.caps, collectibles.powder, collectibles.bullets_36, collectibles.bullets_44
    );
    frame.render_widget(
        Paragraph::new(Line::from(coll_text).dark_gray()),
        chunks[4],
    );
}

/// Maximum number of inventory items shown in the compact bottom panel.
const MAX_DISPLAYED_INVENTORY_ITEMS: usize = 4;

/// Renders the info column (Inventory + Visible entities stacked vertically).
fn render_info_column(
    frame: &mut ratatui::Frame,
    area: Rect,
    player_inv: Option<&Inventory>,
    inv_item_names: &[String],
    visible_entities: &[(String, RatColor, RatColor, String)],
) {
    let inv_item_count = player_inv.map_or(0, |inv| inv.items.len());
    let inv_display_count = inv_item_count.max(1).min(MAX_DISPLAYED_INVENTORY_ITEMS);
    let inv_height = (inv_display_count as u16) + 2; // +2 for borders

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(inv_height), // Inventory
            Constraint::Min(1),            // Visible entities
        ])
        .split(area);

    // ── Inventory ───────────────────────────────────────────────
    let mut inv_lines: Vec<Line> = Vec::new();
    if let Some(inv) = player_inv {
        if inv.items.is_empty() {
            inv_lines.push(Line::from(" (empty)".dark_gray()));
        } else {
            for (i, name) in inv_item_names.iter().enumerate().take(MAX_DISPLAYED_INVENTORY_ITEMS) {
                inv_lines.push(Line::from(format!(" {}: {name}", i + 1)));
            }
            if inv_item_names.len() > MAX_DISPLAYED_INVENTORY_ITEMS {
                inv_lines.push(Line::from(format!(" +{} more [Tab]", inv_item_names.len() - MAX_DISPLAYED_INVENTORY_ITEMS)).dark_gray());
            }
        }
    } else {
        inv_lines.push(Line::from(" (none)".dark_gray()));
    }
    frame.render_widget(
        Paragraph::new(inv_lines)
            .block(Block::default().borders(Borders::ALL).title("Bag [Tab]")),
        chunks[0],
    );

    // ── Visible Entities ────────────────────────────────────────
    let max_visible = (chunks[1].height.saturating_sub(2)) as usize;
    let mut vis_lines: Vec<Line> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    for (sym, fg, _bg, name) in visible_entities {
        if seen_names.insert(name.clone()) {
            vis_lines.push(Line::from(vec![
                Span::from(format!(" {sym}")).fg(*fg),
                Span::from(format!(" {name}")).white(),
            ]));
            if vis_lines.len() >= max_visible {
                break;
            }
        }
    }
    if vis_lines.is_empty() {
        vis_lines.push(Line::from(" (nothing)".dark_gray()));
    }

    frame.render_widget(
        Paragraph::new(vis_lines)
            .block(Block::default().borders(Borders::ALL).title("Visible")),
        chunks[1],
    );
}

/// Renders the help overlay listing all keybindings.
fn render_help_overlay(frame: &mut ratatui::Frame, game_area: Rect) {
    let help_width = 50u16;
    let help_height = (KEYBINDINGS.len() as u16) + 6; // +6 for border + title + padding

    // Clamp to game area to avoid overflow
    let w = help_width.min(game_area.width.saturating_sub(2));
    let h = help_height.min(game_area.height.saturating_sub(2));

    if w < 10 || h < 5 {
        return;
    }

    let cx = game_area.x + (game_area.width.saturating_sub(w)) / 2;
    let cy = game_area.y + (game_area.height.saturating_sub(h)) / 2;
    let help_area = Rect {
        x: cx,
        y: cy,
        width: w,
        height: h,
    };

    // Clear the area first so no map ASCII bleeds through.
    frame.render_widget(Clear, help_area);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    for binding in KEYBINDINGS {
        lines.push(Line::from(vec![
            Span::from(format!(" {:<14}", binding.key)).bold().yellow(),
            Span::from(binding.name.to_string()).white(),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(" Press ? or / to close".dark_gray()));

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Controls ")
                    .border_style(ratatui::style::Style::default().fg(ratatui::style::Color::Yellow)),
            )
            .wrap(Wrap { trim: false })
            .on_black(),
        help_area,
    );
}

/// Renders the welcome screen shown at game start.
fn render_welcome_overlay(frame: &mut ratatui::Frame, game_area: Rect) {
    let binding_count = KEYBINDINGS.len() as u16;
    let w = 62u16.min(game_area.width.saturating_sub(4));
    // title(1) + blank(1) + objective(4) + blank(1) + bindings + blank(1) + press-any(1) + border(2)
    let h = (binding_count + 13).min(game_area.height.saturating_sub(4));

    if w < 20 || h < 10 {
        return;
    }

    let cx = game_area.x + (game_area.width.saturating_sub(w)) / 2;
    let cy = game_area.y + (game_area.height.saturating_sub(h)) / 2;
    let welcome_area = Rect {
        x: cx,
        y: cy,
        width: w,
        height: h,
    };

    frame.render_widget(Clear, welcome_area);

    let mut lines = vec![
        Line::from(""),
        Line::from("  -*-  DEAD MAN'S HAND  -*-").bold().yellow(),
        Line::from(""),
        Line::from("  Destroy the Outlaw Hideout (Ω) to win!").white(),
        Line::from("  Enemies will keep spawning from it.").white(),
        Line::from("  Enemies drop items that are").white(),
        Line::from("  auto-picked up on contact.").white(),
        Line::from("  Cowboys can shoot at you from range!").white(),
        Line::from(""),
    ];
    for binding in KEYBINDINGS {
        lines.push(Line::from(vec![
            Span::from(format!("  {:<14}", binding.key)).bold().yellow(),
            Span::from(format!("  {}", binding.name)).white(),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("  Press any key to begin...").dark_gray());

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Welcome ")
                    .border_style(ratatui::style::Style::default().fg(ratatui::style::Color::Yellow)),
            )
            .wrap(Wrap { trim: false })
            .on_black(),
        welcome_area,
    );
}

/// Renders the inventory overlay as a scrollable ratatui Table widget.
fn render_inventory_overlay(
    frame: &mut ratatui::Frame,
    game_area: Rect,
    items: &[(String, String)],
    selected: usize,
) {
    let w = 44u16.min(game_area.width.saturating_sub(4));
    let h = (items.len() as u16 + 6).min(game_area.height.saturating_sub(4)).max(8);

    if w < 20 || h < 5 {
        return;
    }

    let cx = game_area.x + (game_area.width.saturating_sub(w)) / 2;
    let cy = game_area.y + (game_area.height.saturating_sub(h)) / 2;
    let inv_area = Rect {
        x: cx,
        y: cy,
        width: w,
        height: h,
    };

    frame.render_widget(Clear, inv_area);

    if items.is_empty() {
        let lines = vec![
            Line::from(""),
            Line::from("  Inventory is empty.").dark_gray(),
            Line::from(""),
            Line::from("  Press Tab or Esc to close").dark_gray(),
        ];
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Inventory [Tab] ")
                        .border_style(ratatui::style::Style::default().fg(ratatui::style::Color::Cyan)),
                )
                .on_black(),
            inv_area,
        );
        return;
    }

    let rows: Vec<Row> = items
        .iter()
        .enumerate()
        .map(|(i, (name, desc))| {
            let style = if i == selected {
                ratatui::style::Style::default()
                    .fg(ratatui::style::Color::Black)
                    .bg(ratatui::style::Color::Cyan)
                    .bold()
            } else {
                ratatui::style::Style::default().fg(ratatui::style::Color::White)
            };
            Row::new(vec![format!("{}", i + 1), name.clone(), desc.clone()]).style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(3),
        Constraint::Min(16),
        Constraint::Length(14),
    ];

    let table = Table::new(rows, widths)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Inventory [Tab] — ↑↓ Navigate, Enter Use, Esc Close ")
                .border_style(ratatui::style::Style::default().fg(ratatui::style::Color::Cyan)),
        )
        .header(
            Row::new(vec!["#", "Item", "Effect"])
                .style(ratatui::style::Style::default().bold().fg(ratatui::style::Color::Yellow)),
        );

    frame.render_widget(table, inv_area);
}
