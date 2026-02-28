use std::collections::HashSet;

use bevy::prelude::*;

use crate::components::{CameraFollow, Player, Position, Renderable, Viewshed};
use crate::events::MoveIntent;
use crate::gamemap::GameMap;
use crate::resources::{CameraPosition, GameMapResource, GameState};
use crate::systems::{camera, input, movement, render, visibility};
use crate::typedefs::{RatColor, SPAWN_X, SPAWN_Y};

/// Bevy plugin that registers all roguelike ECS systems, resources, and
/// startup logic. Adding this plugin is the only step needed to wire up the
/// game — `main.rs` stays minimal.
pub struct RoguelikePlugin;

impl Plugin for RoguelikePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_message::<MoveIntent>()
            .insert_resource(GameMapResource(GameMap::new(120, 80)))
            .insert_resource(CameraPosition((SPAWN_X, SPAWN_Y)))
            .init_state::<GameState>()
            .add_systems(Startup, spawn_player)
            // input_system runs in PreUpdate so movement intents are ready
            // before Update systems process them.
            .add_systems(PreUpdate, input::input_system)
            .add_systems(
                Update,
                (
                    movement::movement_system,
                    visibility::visibility_system,
                    camera::camera_follow_system,
                )
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            )
            // draw_system always runs (renders "PAUSED" overlay when paused)
            .add_systems(Update, render::draw_system.after(camera::camera_follow_system));
    }
}

/// Spawns the player entity with all required ECS components.
fn spawn_player(mut commands: Commands) {
    commands.spawn((
        Position {
            x: SPAWN_X,
            y: SPAWN_Y,
        },
        Player,
        Renderable {
            symbol: "@".into(),
            fg: RatColor::White,
            bg: RatColor::Black,
        },
        CameraFollow,
        Viewshed {
            range: 15,
            visible_tiles: HashSet::new(),
            dirty: true, // compute on first frame
        },
    ));
}
