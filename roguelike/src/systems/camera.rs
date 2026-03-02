use bevy::prelude::*;

use crate::components::{CameraFollow, Player, Position};
use crate::resources::{CameraPosition, CursorPosition};

/// Centers the camera between the player and cursor positions.
/// This gives the player a wider view of the area they're aiming at.
pub fn camera_follow_system(
    query: Query<&Position, (With<CameraFollow>, With<Player>)>,
    cursor: Res<CursorPosition>,
    mut camera: ResMut<CameraPosition>,
) {
    if let Ok(pos) = query.single() {
        let player_pos = pos.as_grid_vec();
        // Center camera between player and cursor
        camera.0 = crate::grid_vec::GridVec::new(
            (player_pos.x + cursor.pos.x) / 2,
            (player_pos.y + cursor.pos.y) / 2,
        );
    }
}
