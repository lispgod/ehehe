use std::time::Duration;

use bevy::prelude::*;
use bevy_ratatui::RatatuiPlugins;

use roguelike::plugins::RoguelikePlugin;

fn main() {
    App::new()
        .add_plugins((
            MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(
                Duration::from_secs_f32(1. / 30.),
            )),
            RatatuiPlugins::default(),
            RoguelikePlugin,
        ))
        .run();
}
