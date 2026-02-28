use bevy::prelude::*;

use crate::typedefs::CoordinateUnit;

/// Fired when an entity intends to move by a relative offset.
#[derive(Message, Debug, Clone)]
pub struct MoveIntent {
    pub entity: Entity,
    pub dx: CoordinateUnit,
    pub dy: CoordinateUnit,
}
