use bevy::prelude::*;
use bevy_ratatui::RatatuiContext;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::components::{Player, Position, Renderable, Viewshed};
use crate::resources::{CameraPosition, GameMapResource, GameState};
use crate::typedefs::CoordinateUnit;

/// Renders the game map and all `Renderable` entities to the terminal.
/// Uses the player's `Viewshed` to determine tile visibility.
pub fn draw_system(
    mut context: ResMut<RatatuiContext>,
    game_map: Res<GameMapResource>,
    camera: Res<CameraPosition>,
    renderables: Query<(&Position, &Renderable)>,
    player_query: Query<(&Position, Option<&Viewshed>), With<Player>>,
    state: Res<State<GameState>>,
) -> Result {
    context.draw(|frame| {
        let area = frame.area();
        let render_width = area.width;
        let render_height = area.height.saturating_sub(1); // reserve 1 row for status

        // Collect the player's visible tiles (if they have a Viewshed)
        let visible_tiles = player_query
            .single()
            .ok()
            .and_then(|(_, vs)| vs)
            .map(|vs| &vs.visible_tiles);

        let mut render_packet = game_map.0.create_render_packet_with_visibility(
            &camera.0,
            render_width,
            render_height,
            visible_tiles,
        );

        // Overlay all renderable entities at their screen-relative positions
        let w_radius = render_width as CoordinateUnit / 2;
        let h_radius = render_height as CoordinateUnit / 2;
        let bottom_left_x = camera.0 .0 - w_radius;
        let bottom_left_y = camera.0 .1 - h_radius;
        for (pos, renderable) in &renderables {
            let screen_x = pos.x - bottom_left_x;
            let screen_y = pos.y - bottom_left_y;

            if screen_x >= 0
                && screen_x < render_width as CoordinateUnit
                && screen_y >= 0
                && screen_y < render_height as CoordinateUnit
            {
                // Only draw entities that are visible (or if no viewshed exists)
                let entity_visible = visible_tiles
                    .map(|vt| vt.contains(&(pos.x, pos.y)))
                    .unwrap_or(true);
                if entity_visible {
                    let bg = render_packet[screen_y as usize][screen_x as usize].2;
                    render_packet[screen_y as usize][screen_x as usize] =
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

        // Status bar — show player position (gracefully handles missing player)
        let player_info = player_query
            .single()
            .map(|(p, _)| format!("({}, {})", p.x, p.y))
            .unwrap_or_default();

        let status_area = ratatui::layout::Rect {
            x: area.x,
            y: area.y + render_height,
            width: area.width,
            height: 1,
        };
        let status = Line::from(format!(
            " Roguelike | Player: {} | WASD/Arrows: move | P: pause/resume | Q: quit",
            player_info
        ));
        frame.render_widget(Paragraph::new(status).on_dark_gray(), status_area);
    })?;

    Ok(())
}
