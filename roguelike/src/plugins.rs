use bevy::prelude::*;

use crate::systems::{camera, input, movement, render};

/// Bevy plugin that registers all roguelike ECS systems.
pub struct RoguelikePlugin;

impl Plugin for RoguelikePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, input::input_system)
            .add_systems(
                Update,
                (
                    movement::movement_system,
                    camera::camera_follow_system,
                    render::draw_system,
                )
                    .chain(),
            );
    }
}
