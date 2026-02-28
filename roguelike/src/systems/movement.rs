use bevy::prelude::*;

use crate::components::Position;
use crate::events::MoveIntent;
use crate::resources::GameMapResource;

/// Processes `MoveIntent` events: checks the target tile on the `GameMap` for
/// walkability, then updates the entity's `Position` if the move is valid.
pub fn movement_system(
    mut intents: MessageReader<MoveIntent>,
    game_map: Res<GameMapResource>,
    mut positions: Query<&mut Position>,
) {
    for intent in intents.read() {
        let Ok(mut pos) = positions.get_mut(intent.entity) else {
            continue;
        };

        let target_x = pos.x + intent.dx;
        let target_y = pos.y + intent.dy;

        // Check if the target tile is walkable (no blocking furniture)
        if let Some(voxel) = game_map.0.get_voxel_at(&(target_x, target_y))
            && voxel.furniture.is_none()
        {
            pos.x = target_x;
            pos.y = target_y;
        }
    }
}
