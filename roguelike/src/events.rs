use bevy::prelude::*;

use crate::typedefs::CoordinateUnit;

/// Fired when an entity intends to move by a relative offset.
#[derive(Message, Debug, Clone)]
pub struct MoveIntent {
    pub entity: Entity,
    pub dx: CoordinateUnit,
    pub dy: CoordinateUnit,
}

/// Fired when an entity intends to attack another entity.
#[derive(Message, Debug, Clone)]
pub struct AttackIntent {
    pub attacker: Entity,
    pub target: Entity,
}

/// Fired after damage has been resolved and applied to an entity.
#[derive(Message, Debug, Clone)]
pub struct DamageEvent {
    pub target: Entity,
    pub amount: CoordinateUnit,
}

/// Fired when the player casts an area-of-effect spell.
/// Damages all hostile entities within `radius` tiles of the caster.
#[derive(Message, Debug, Clone)]
pub struct SpellCastIntent {
    pub caster: Entity,
    pub radius: CoordinateUnit,
}
