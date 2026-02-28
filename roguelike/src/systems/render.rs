use bevy::prelude::*;
use bevy_ratatui::RatatuiContext;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::components::{Player, Position, Renderable};
use crate::systems::camera::CameraPosition;
use crate::systems::movement::GameMapResource;
use crate::typedefs::CoordinateUnit;

/// Renders the game map and all `Renderable` entities to the terminal.
pub fn draw_system(
    mut context: ResMut<RatatuiContext>,
    game_map: Res<GameMapResource>,
    camera: Res<CameraPosition>,
    renderables: Query<(&Position, &Renderable)>,
    player_query: Query<&Position, With<Player>>,
) -> Result {
    context.draw(|frame| {
        let area = frame.area();
        let render_width = area.width;
        let render_height = area.height.saturating_sub(1); // reserve 1 row for status

        let mut render_packet =
            game_map.0.create_render_packet(&camera.0, render_width, render_height);

        // Overlay all renderable entities at their screen-relative positions
        let w_radius = render_width as CoordinateUnit / 2;
        let h_radius = render_height as CoordinateUnit / 2;

        for (pos, renderable) in &renderables {
            let screen_x = pos.x - (camera.0 .0 - w_radius);
            let screen_y = pos.y - (camera.0 .1 - h_radius);

            if screen_x >= 0
                && screen_x < render_width as CoordinateUnit
                && screen_y >= 0
                && screen_y < render_height as CoordinateUnit
            {
                let bg = render_packet[screen_y as usize][screen_x as usize].2;
                render_packet[screen_y as usize][screen_x as usize] =
                    (renderable.symbol.clone(), renderable.fg, bg);
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

        // Status bar — show player position (gracefully handles missing player)
        let player_info = player_query
            .single()
            .map(|p| format!("({}, {})", p.x, p.y))
            .unwrap_or_default();

        let status_area = ratatui::layout::Rect {
            x: area.x,
            y: area.y + render_height,
            width: area.width,
            height: 1,
        };
        let status = Line::from(format!(
            " Roguelike | Player: {} | WASD/Arrows: move | Q: quit",
            player_info
        ));
        frame.render_widget(Paragraph::new(status).on_dark_gray(), status_area);
    })?;

    Ok(())
}
