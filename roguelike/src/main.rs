use std::time::Duration;

use bevy::prelude::*;
use bevy_ratatui::RatatuiPlugins;

use roguelike::components::{CameraFollow, Player, Position, Renderable};
use roguelike::gamemap::GameMap;
use roguelike::plugins::RoguelikePlugin;
use roguelike::systems::camera::CameraPosition;
use roguelike::systems::movement::GameMapResource;
use roguelike::typedefs::{RatColor, SPAWN_X, SPAWN_Y};

fn main() {
    App::new()
        .add_plugins((
            MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(
                Duration::from_secs_f32(1. / 30.),
            )),
            RatatuiPlugins::default(),
            RoguelikePlugin,
        ))
        .insert_resource(GameMapResource(GameMap::new(120, 80)))
        .insert_resource(CameraPosition((SPAWN_X, SPAWN_Y)))
        .add_systems(Startup, spawn_player)
        .run();
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
    ));
}
