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
    /// Entity that dealt this damage. Used to attribute kills for EXP.
    pub source: Option<Entity>,
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
    /// Optional gun item entity. If present, decrements its loaded rounds.
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

/// Fired when the player throws a molotov cocktail.
/// Ignites all flammable props within `radius` tiles of the target.
#[derive(Message, Debug, Clone)]
pub struct MolotovCastIntent {
    pub caster: Entity,
    pub radius: CoordinateUnit,
    pub damage: CoordinateUnit,
    /// World position where the molotov detonates.
    pub target: crate::grid_vec::GridVec,
    /// Inventory index of the molotov item to consume.
    pub item_index: usize,
}

/// Fired when the player interacts with an adjacent NPC.
#[derive(Message, Debug, Clone)]
pub struct InteractionIntent {
    /// The player entity performing the interaction.
    pub player: Entity,
    /// The NPC entity being interacted with.
    pub target: Entity,
    /// The type of interaction.
    pub interaction: crate::components::NpcInteraction,
}

/// Fired when a crime is committed (witnessed or detected).
/// Processed by the wanted system to update star level.
#[derive(Message, Debug, Clone)]
pub struct CrimeEvent {
    /// The entity that committed the crime.
    pub perpetrator: Entity,
    /// The type of crime committed.
    pub crime: crate::components::CrimeType,
    /// World position where the crime occurred.
    pub position: crate::grid_vec::GridVec,
}

/// Fired when the player enters or exits a hiding spot.
#[derive(Message, Debug, Clone)]
pub struct HideIntent {
    /// The player entity.
    pub entity: Entity,
    /// True = entering hiding, False = exiting hiding.
    pub entering: bool,
}

/// Fired when an NPC draws a weapon during a brawl, escalating to lethal combat.
#[derive(Message, Debug, Clone)]
pub struct BrawlEscalation {
    /// The entity that drew a weapon.
    pub entity: Entity,
    /// World position where the escalation occurs.
    pub position: crate::grid_vec::GridVec,
}
