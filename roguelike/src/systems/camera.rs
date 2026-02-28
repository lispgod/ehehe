use bevy::prelude::*;

use crate::components::{CameraFollow, Position};
use crate::resources::CameraPosition;

/// Copies the position of the entity tagged with `CameraFollow` into the
/// `CameraPosition` resource so the renderer can use it for viewport math.
pub fn camera_follow_system(
    query: Query<&Position, With<CameraFollow>>,
    mut camera: ResMut<CameraPosition>,
) {
    if let Ok(pos) = query.single() {
        camera.0 = (pos.x, pos.y);
    }
}
