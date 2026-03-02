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

/// Fired when the player throws a grenade (area-of-effect attack).
/// Damages all hostile entities within `radius` tiles of the target.
#[derive(Message, Debug, Clone)]
pub struct SpellCastIntent {
    pub caster: Entity,
    pub radius: CoordinateUnit,
    /// World position where the grenade detonates (cursor position).
    pub target: crate::grid_vec::GridVec,
    /// Inventory index of the grenade item to consume.
    pub grenade_index: usize,
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

/// Fired when the player uses a targeted ranged attack in a chosen direction.
/// The bullet travels along (dx, dy) direction for up to `range` tiles.
#[derive(Message, Debug, Clone)]
pub struct RangedAttackIntent {
    pub attacker: Entity,
    pub range: CoordinateUnit,
    /// Trajectory direction (normalized to -1/0/1 per axis).
    pub dx: CoordinateUnit,
    pub dy: CoordinateUnit,
    /// Optional gun item entity. If present, decrements loaded rounds instead of Ammo.
    pub gun_item: Option<Entity>,
}

/// Fired when an AI entity performs a ranged attack toward a target position.
#[derive(Message, Debug, Clone)]
pub struct AiRangedAttackIntent {
    pub attacker: Entity,
    pub target: Entity,
    pub range: CoordinateUnit,
}

/// Fired when the player performs a melee wide (cleave) attack hitting all adjacent enemies.
#[derive(Message, Debug, Clone)]
pub struct MeleeWideIntent {
    pub attacker: Entity,
}

/// Fired when the player drops an item from inventory onto the ground.
#[derive(Message, Debug, Clone)]
pub struct DropItemIntent {
    pub user: Entity,
    pub item_index: usize,
}

/// Fired when the player throws a knife or tomahawk toward the cursor.
#[derive(Message, Debug, Clone)]
pub struct ThrowItemIntent {
    pub thrower: Entity,
    pub item_entity: Entity,
    pub item_index: usize,
    pub dx: CoordinateUnit,
    pub dy: CoordinateUnit,
    pub range: CoordinateUnit,
    pub damage: CoordinateUnit,
}
