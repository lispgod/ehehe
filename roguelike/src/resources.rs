use bevy::prelude::*;

use crate::gamemap::GameMap;
use crate::typedefs::MyPoint;

/// Bevy resource wrapping the game map for ECS access.
#[derive(Resource)]
pub struct GameMapResource(pub GameMap);

/// Bevy resource holding the camera position (follows the tracked entity).
#[derive(Resource)]
pub struct CameraPosition(pub MyPoint);

/// Top-level game state managed by Bevy's state machine.
/// Systems that should only run during gameplay use
/// `.run_if(in_state(GameState::Playing))`.
#[derive(States, Clone, Copy, Default, Eq, PartialEq, Hash, Debug)]
pub enum GameState {
    #[default]
    Playing,
    Paused,
}
