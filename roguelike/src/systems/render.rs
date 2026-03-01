use std::collections::HashSet;

use bevy::prelude::*;
use bevy_ratatui::RatatuiContext;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph, Wrap};

use crate::components::{Health, Inventory, Mana, Name, Player, Position, Renderable, Viewshed};
use crate::grid_vec::GridVec;
use crate::resources::{
    CameraPosition, CombatLog, GameMapResource, GameState, HelpVisible, KillCount,
    SpellParticles, TurnCounter, WelcomeVisible,
};
use crate::systems::input::KEYBINDINGS;
use crate::typedefs::{CoordinateUnit, MyPoint, RatColor};

/// Lifetime (in frames) for spell particle animations.
/// Must match the lifetime used in spell.rs when creating particles.
const PARTICLE_LIFETIME: f32 = 8.0;

/// Number of recent combat log messages shown in the status bar.
const STATUS_BAR_MESSAGE_COUNT: usize = 2;

/// Ticks and renders spell particles each frame.
pub fn particle_tick_system(mut particles: ResMut<SpellParticles>) {
    particles.tick();
}

/// Renders the game map and all `Renderable` entities to the terminal.
/// Uses the player's `Viewshed` to determine tile visibility, and the
/// `revealed_tiles` set for fog-of-war memory (dimmed rendering).
///
/// Layout:
/// ┌─────────────────────────────┬──────────────┐
/// │         Game Area           │  Side Panel   │
/// │                             │  (HP/Mana     │
/// │                             │   Inventory   │
/// │                             │   Visible)    │
/// ├─────────────────────────────┴──────────────┤
/// │  Status Bar                                 │
/// └─────────────────────────────────────────────┘
pub fn draw_system(
    mut context: ResMut<RatatuiContext>,
    game_map: Res<GameMapResource>,
    camera: Res<CameraPosition>,
    renderables: Query<(&Position, &Renderable, Option<&Name>)>,
    player_query: Query<
        (&Position, Option<&Viewshed>, Option<&Health>, Option<&Mana>, Option<&Inventory>),
        With<Player>,
    >,
    item_names: Query<Option<&Name>, With<crate::components::Item>>,
    state: Res<State<GameState>>,
    combat_log: Res<CombatLog>,
    turn_counter: Res<TurnCounter>,
    kill_count: Res<KillCount>,
    help_visible: Res<HelpVisible>,
    spell_particles: Res<SpellParticles>,
    welcome_visible: Res<WelcomeVisible>,
) -> Result {
    context.draw(|frame| {
        let area = frame.area();

        // ── Top-level layout: main area + status bar (1 row) ────
        let vert_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(area);

        let main_area = vert_chunks[0];
        let status_area = vert_chunks[1];

        // ── Main area: game viewport + side panel ───────────────
        let side_panel_width = 22u16;
        let horiz_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(side_panel_width),
            ])
            .split(main_area);

        let game_area = horiz_chunks[0];
        let side_area = horiz_chunks[1];

        let render_width = game_area.width;
        let render_height = game_area.height;

        // Collect the player's visible and revealed tiles.
        let (visible_tiles, revealed_tiles, player_hp, player_mana, player_inv): (
            Option<&HashSet<MyPoint>>,
            Option<&HashSet<MyPoint>>,
            Option<&Health>,
            Option<&Mana>,
            Option<&Inventory>,
        ) = player_query
            .single()
            .ok()
            .map(|(_, vs, hp, mp, inv)| {
                let (vis, rev) = vs
                    .map(|vs| (Some(&vs.visible_tiles), Some(&vs.revealed_tiles)))
                    .unwrap_or((None, None));
                (vis, rev, hp, mp, inv)
            })
            .unwrap_or((None, None, None, None, None));

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

        // Collect visible entities for the side panel.
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

        // Overlay spell particles on the render packet.
        // Only show particles whose delay has reached 0 (already visible).
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

        // Collect inventory item names for the side panel.
        let inv_item_names: Vec<String> = player_inv
            .map(|inv| {
                inv.items
                    .iter()
                    .map(|&ent| {
                        item_names
                            .get(ent)
                            .ok()
                            .flatten()
                            .map_or("item".to_string(), |n| n.0.clone())
                    })
                    .collect()
            })
            .unwrap_or_default();

        // ── Side Panel ──────────────────────────────────────────
        render_side_panel(
            frame,
            side_area,
            player_hp,
            player_mana,
            player_inv,
            &inv_item_names,
            &visible_entity_infos,
            &combat_log,
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
            let label = " VICTORY! The Gate of Hell has been destroyed! Press Q to quit. ";
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

        // Show help overlay when toggled
        if help_visible.0 {
            render_help_overlay(frame, game_area);
        }

        // Show welcome screen at game start
        if welcome_visible.0 {
            render_welcome_overlay(frame, game_area);
        }

        // ── Status bar ──────────────────────────────────────────
        let player_info = player_query
            .single()
            .map(|(p, _, _, _, _)| format!("({}, {})", p.x, p.y))
            .unwrap_or_default();

        let recent_msgs = combat_log.recent(STATUS_BAR_MESSAGE_COUNT);
        let last_msg = recent_msgs.join(" | ");

        let status = Line::from(format!(
            " Gate of Hell | {player_info} | Turn:{} Kills:{} | {last_msg} | ?/: help",
            turn_counter.0, kill_count.0,
        ));
        frame.render_widget(Paragraph::new(status).on_dark_gray(), status_area);
    })?;

    Ok(())
}

/// Renders the side panel with HP gauge, Mana gauge, inventory, visible entities, and combat log.
fn render_side_panel(
    frame: &mut ratatui::Frame,
    area: Rect,
    player_hp: Option<&Health>,
    player_mana: Option<&Mana>,
    player_inv: Option<&Inventory>,
    inv_item_names: &[String],
    visible_entities: &[(String, RatColor, RatColor, String)],
    _combat_log: &CombatLog,
) {
    // Dynamic inventory height: 2 (border) + max(1, item_count)
    let item_count = player_inv.map_or(1, |inv| inv.items.len().max(1));
    let inv_height = (item_count as u16) + 2; // +2 for borders

    // Divide the side panel into sections.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),          // HP gauge
            Constraint::Length(3),          // Mana gauge
            Constraint::Length(inv_height), // Inventory (dynamic)
            Constraint::Length(5),          // Combat log
            Constraint::Min(1),            // Visible entities
        ])
        .split(area);

    // ── HP Gauge ────────────────────────────────────────────────
    if let Some(hp) = player_hp {
        let ratio = if hp.max > 0 {
            (hp.current as f64 / hp.max as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("HP"))
            .gauge_style(
                ratatui::style::Style::default()
                    .fg(ratatui::style::Color::Red)
                    .bg(ratatui::style::Color::DarkGray),
            )
            .ratio(ratio)
            .label(Span::from(format!("{}/{}", hp.current, hp.max)).style(ratatui::style::Style::default().fg(ratatui::style::Color::White)));
        frame.render_widget(gauge, chunks[0]);
    } else {
        frame.render_widget(
            Block::default().borders(Borders::ALL).title("HP"),
            chunks[0],
        );
    }

    // ── Mana Gauge ──────────────────────────────────────────────
    if let Some(mana) = player_mana {
        let ratio = if mana.max > 0 {
            (mana.current as f64 / mana.max as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("Mana"))
            .gauge_style(
                ratatui::style::Style::default()
                    .fg(ratatui::style::Color::Blue)
                    .bg(ratatui::style::Color::DarkGray),
            )
            .ratio(ratio)
            .label(Span::from(format!("{}/{}", mana.current, mana.max)).style(ratatui::style::Style::default().fg(ratatui::style::Color::White)));
        frame.render_widget(gauge, chunks[1]);
    } else {
        frame.render_widget(
            Block::default().borders(Borders::ALL).title("Mana"),
            chunks[1],
        );
    }

    // ── Inventory ───────────────────────────────────────────────
    let mut inv_lines: Vec<Line> = Vec::new();
    if let Some(inv) = player_inv {
        if inv.items.is_empty() {
            inv_lines.push(Line::from(" (empty)".dark_gray()));
        } else {
            for (i, name) in inv_item_names.iter().enumerate().take(9) {
                inv_lines.push(Line::from(format!(" {}: {name}", i + 1)));
            }
        }
    } else {
        inv_lines.push(Line::from(" (none)".dark_gray()));
    }
    frame.render_widget(
        Paragraph::new(inv_lines)
            .block(Block::default().borders(Borders::ALL).title("Bag [1-9]")),
        chunks[2],
    );

    // ── Combat Log ──────────────────────────────────────────────
    let log_lines: Vec<Line> = _combat_log
        .recent(3)
        .into_iter()
        .map(|s| Line::from(format!(" {s}")).dark_gray())
        .collect();
    frame.render_widget(
        Paragraph::new(if log_lines.is_empty() {
            vec![Line::from(" (no events)".dark_gray())]
        } else {
            log_lines
        })
        .block(Block::default().borders(Borders::ALL).title("Log"))
        .wrap(Wrap { trim: true }),
        chunks[3],
    );

    // ── Visible Entities ────────────────────────────────────────
    let max_visible = (chunks[4].height.saturating_sub(2)) as usize;
    let mut vis_lines: Vec<Line> = Vec::new();
    // Deduplicate: show each unique name only once.
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
        chunks[4],
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
    for (key, desc) in KEYBINDINGS {
        lines.push(Line::from(vec![
            Span::from(format!(" {key:<14}")).bold().yellow(),
            Span::from(format!("{desc}")).white(),
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
    let w = 56u16.min(game_area.width.saturating_sub(4));
    let h = 16u16.min(game_area.height.saturating_sub(4));

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

    let lines = vec![
        Line::from(""),
        Line::from("  ⚔  GATE OF HELL  ⚔").bold().yellow(),
        Line::from(""),
        Line::from("  Destroy the Gate of Hell (Ω) to win!").white(),
        Line::from("  Monsters will keep spawning from it.").white(),
        Line::from(""),
        Line::from(vec![
            Span::from("  WASD / Arrows").bold().yellow(),
            Span::from("  Move").white(),
        ]),
        Line::from(vec![
            Span::from("  F / Space    ").bold().yellow(),
            Span::from("  Cast AoE spell").white(),
        ]),
        Line::from(vec![
            Span::from("  G            ").bold().yellow(),
            Span::from("  Pick up items").white(),
        ]),
        Line::from(vec![
            Span::from("  1-9          ").bold().yellow(),
            Span::from("  Use inventory item").white(),
        ]),
        Line::from(vec![
            Span::from("  ? or /       ").bold().yellow(),
            Span::from("  Help screen").white(),
        ]),
        Line::from(""),
        Line::from("  Press any key to begin...").dark_gray(),
    ];

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
