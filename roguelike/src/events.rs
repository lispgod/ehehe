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

/// Fired when an entity wants to use an inventory item.
#[derive(Message, Debug, Clone)]
pub struct UseItemIntent {
    pub user: Entity,
    pub item_index: usize,
}

/// Fired when an entity wants to pick up an item on the ground.
#[derive(Message, Debug, Clone)]
pub struct PickupItemIntent {
    pub picker: Entity,
}

/// Fired when the player uses a targeted ranged attack on the nearest visible enemy.
#[derive(Message, Debug, Clone)]
pub struct RangedAttackIntent {
    pub attacker: Entity,
    pub range: CoordinateUnit,
}

/// Fired when the player performs a melee wide (cleave) attack hitting all adjacent enemies.
#[derive(Message, Debug, Clone)]
pub struct MeleeWideIntent {
    pub attacker: Entity,
}
