use std::collections::HashSet;

use bevy::prelude::*;
use bevy_ratatui::RatatuiContext;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::components::{Health, Player, Position, Renderable, Viewshed};
use crate::grid_vec::GridVec;
use crate::resources::{CameraPosition, CombatLog, GameMapResource, GameState, KillCount, TurnCounter};
use crate::typedefs::{CoordinateUnit, MyPoint};

/// Renders the game map and all `Renderable` entities to the terminal.
/// Uses the player's `Viewshed` to determine tile visibility, and the
/// `revealed_tiles` set for fog-of-war memory (dimmed rendering).
pub fn draw_system(
    mut context: ResMut<RatatuiContext>,
    game_map: Res<GameMapResource>,
    camera: Res<CameraPosition>,
    renderables: Query<(&Position, &Renderable)>,
    player_query: Query<(&Position, Option<&Viewshed>, Option<&Health>), With<Player>>,
    state: Res<State<GameState>>,
    combat_log: Res<CombatLog>,
    turn_counter: Res<TurnCounter>,
    kill_count: Res<KillCount>,
) -> Result {
    context.draw(|frame| {
        let area = frame.area();
        let render_width = area.width;
        let render_height = area.height.saturating_sub(1); // reserve 1 row for status

        // Collect the player's visible and revealed tiles.
        let (visible_tiles, revealed_tiles): (
            Option<&HashSet<MyPoint>>,
            Option<&HashSet<MyPoint>>,
        ) = player_query
            .single()
            .ok()
            .and_then(|(_, vs, _)| vs)
            .map(|vs| (&vs.visible_tiles, &vs.revealed_tiles))
            .map(|(vis, rev)| (Some(vis), Some(rev)))
            .unwrap_or((None, None));

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
        for (pos, renderable) in &renderables {
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

        let game_area = ratatui::layout::Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: render_height,
        };
        frame.render_widget(Paragraph::new(Text::from(render_lines)).on_black(), game_area);

        // Show "PAUSED" overlay centered on screen when paused
        if *state.get() == GameState::Paused {
            let label = " PAUSED — press P to resume ";
            let label_width = label.len() as u16;
            if render_width >= label_width && render_height >= 1 {
                let cx = area.x + (render_width - label_width) / 2;
                let cy = area.y + render_height / 2;
                let pause_area = ratatui::layout::Rect {
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

        // Status bar — show player position, health, and last combat message
        let player_info = player_query
            .single()
            .map(|(p, _, h)| {
                let hp_str = h.map_or(String::new(), |h| format!(" HP:{}/{}", h.current, h.max));
                format!("({}, {}){hp_str}", p.x, p.y)
            })
            .unwrap_or_default();

        let recent_msgs = combat_log.recent(3);
        let last_msg = recent_msgs.join(" | ");

        let status_area = ratatui::layout::Rect {
            x: area.x,
            y: area.y + render_height,
            width: area.width,
            height: 1,
        };
        let status = Line::from(format!(
            " Survivor | {player_info} | Turn:{} Kills:{} | {last_msg} | WASD: move | F/Space: spell | P: pause | Q: quit",
            turn_counter.0, kill_count.0,
        ));
        frame.render_widget(Paragraph::new(status).on_dark_gray(), status_area);
    })?;

    Ok(())
}
