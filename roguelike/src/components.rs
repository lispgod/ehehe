use bevy::prelude::*;

use crate::typedefs::{CoordinateUnit, RatColor};

/// World-grid position for any entity.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Position {
    pub x: CoordinateUnit,
    pub y: CoordinateUnit,
}

/// Marker component: tags the player-controlled entity.
#[derive(Component, Debug)]
pub struct Player;

/// Visual representation used when rendering an entity on the grid.
#[derive(Component, Clone, Debug)]
pub struct Renderable {
    pub symbol: String,
    pub fg: RatColor,
    pub bg: RatColor,
}

/// Marker component: the camera will follow entities that have this.
#[derive(Component, Debug)]
pub struct CameraFollow;

/// Marker component: entity occupies its tile and blocks movement.
#[derive(Component, Debug)]
pub struct BlocksMovement;
