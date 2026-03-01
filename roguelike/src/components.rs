use std::collections::HashSet;

use bevy::prelude::*;

use crate::grid_vec::GridVec;
use crate::typedefs::{CoordinateUnit, MyPoint, RatColor};

/// World-grid position for any entity.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Position {
    pub x: CoordinateUnit,
    pub y: CoordinateUnit,
}

impl Position {
    /// Convert to a `GridVec` for vector arithmetic and distance calculations.
    #[inline]
    pub fn as_grid_vec(self) -> GridVec {
        GridVec::new(self.x, self.y)
    }
}

impl From<GridVec> for Position {
    #[inline]
    fn from(v: GridVec) -> Self {
        Self { x: v.x, y: v.y }
    }
}

impl From<Position> for GridVec {
    #[inline]
    fn from(p: Position) -> Self {
        GridVec::new(p.x, p.y)
    }
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

/// Field-of-view component. Attached to entities that "see" the world.
/// `visible_tiles` is recomputed by `visibility_system` when dirty.
/// `revealed_tiles` accumulates all tiles ever seen (fog of war memory).
#[derive(Component, Debug)]
pub struct Viewshed {
    /// Maximum sight range (in tiles).
    pub range: CoordinateUnit,
    /// Set of world-grid coordinates currently visible.
    pub visible_tiles: HashSet<MyPoint>,
    /// Set of world-grid coordinates that have been seen at least once.
    /// Used for fog-of-war: revealed tiles are drawn dimmed when not visible.
    pub revealed_tiles: HashSet<MyPoint>,
    /// Whether the viewshed needs recalculation (dirty flag).
    pub dirty: bool,
}

/// Health pool for any entity that can take damage or be healed.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Health {
    pub current: CoordinateUnit,
    pub max: CoordinateUnit,
}

/// Combat statistics used by the combat system to resolve attacks.
/// Damage dealt = max(0, attacker.attack − defender.defense).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct CombatStats {
    pub attack: CoordinateUnit,
    pub defense: CoordinateUnit,
}

/// Display name for any entity. Used in combat messages, UI, and logs.
#[derive(Component, Clone, Debug)]
pub struct Name(pub String);

/// Movement speed: determines how much energy an entity gains each world tick.
///
/// In the energy-based turn model, an entity acts when its accumulated energy
/// reaches `ACTION_COST`. Higher speed → more energy per tick → more frequent
/// actions. A speed of 100 is the "normal" baseline (one action per tick).
///
/// The energy model is a discrete event scheduler:
///   turns_between_actions = ⌈ACTION_COST / speed⌉
///
/// This is the standard roguelike scheduling algorithm used by Angband, DCSS,
/// and Cogmind. It avoids floating-point entirely and provides exact fairness.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Speed(pub CoordinateUnit);

/// Accumulated action energy. When `energy >= ACTION_COST`, the entity may act.
///
/// After acting, energy is reduced by `ACTION_COST`. Excess energy carries
/// over, ensuring long-run fairness: over N ticks, an entity with speed S
/// takes exactly ⌊N × S / ACTION_COST⌋ actions.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Energy(pub CoordinateUnit);

/// The energy threshold required to perform one action.
/// Entities accumulate energy each tick equal to their `Speed` value.
/// When energy ≥ ACTION_COST, they may act and energy is reduced by ACTION_COST.
pub const ACTION_COST: CoordinateUnit = 100;

/// AI behaviour state for non-player entities.
///
/// The AI system reads this to decide what action to emit:
/// - `Idle`: stand still, wait for the player to enter sight range.
/// - `Chasing`: move toward the last known player position.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub enum AiState {
    /// Entity is stationary — has not seen the player yet.
    Idle,
    /// Entity is actively pursuing the player.
    Chasing,
}

/// Marker component: tags entities hostile to the player.
/// Used by bump-to-attack: moving into a hostile entity's tile triggers combat.
#[derive(Component, Debug)]
pub struct Hostile;

/// Marker component: tags the Hell Gate entity.
/// When destroyed, the player wins the game.
#[derive(Component, Debug)]
pub struct HellGate;

/// Mana pool for entities that can cast spells.
/// Spells consume mana; mana regenerates slowly each turn.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Mana {
    pub current: CoordinateUnit,
    pub max: CoordinateUnit,
}

/// A visual particle used for spell animations.
/// The particle has a limited lifetime (in frames) before despawning.
#[derive(Component, Debug)]
pub struct Particle {
    pub lifetime: u32,
}

// ─── Inventory & Item system ─────────────────────────────────────

/// Marker component: tags an entity as an item that can be picked up.
#[derive(Component, Debug)]
pub struct Item;

/// Links an item entity to the entity carrying it.
#[derive(Component, Debug)]
pub struct InBackpack {
    pub owner: Entity,
}

/// The kind of item and its associated effect.
#[derive(Component, Clone, Debug, PartialEq)]
pub enum ItemKind {
    /// Restores `amount` health when used.
    HealingPotion { amount: CoordinateUnit },
    /// A spell scroll: deals `damage` in a radius when used.
    Scroll { damage: CoordinateUnit, radius: CoordinateUnit },
    /// Armor: provides `defense` bonus when equipped.
    Armor { defense: CoordinateUnit },
    /// Weapon: provides `attack` bonus when equipped.
    Weapon { attack: CoordinateUnit },
}

/// Marker component indicating the item is currently equipped.
#[derive(Component, Debug)]
pub struct Equipped;

/// Inventory component: holds item entities belonging to an entity.
#[derive(Component, Debug, Default)]
pub struct Inventory {
    pub items: Vec<Entity>,
}

/// Loot table component: when this entity dies, it may drop items.
#[derive(Component, Debug)]
pub struct LootTable {
    /// Probability (0.0–1.0) that this entity drops an item on death.
    pub drop_chance: f64,
}

/// Experience points component for the player.
/// Killing hostile entities awards EXP. Accumulating enough EXP triggers a level-up.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Experience {
    pub current: i32,
    pub next_level: i32,
}

/// Player level. Increases when enough EXP is accumulated.
/// Each level grants stat bonuses (attack, defense, max HP, max mana).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Level(pub i32);

/// How much EXP a hostile entity is worth when killed.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct ExpReward(pub i32);

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Position / GridVec conversion tests ─────────────────────

    #[test]
    fn position_as_grid_vec() {
        let pos = Position { x: 10, y: 20 };
        let gv = pos.as_grid_vec();
        assert_eq!(gv, GridVec::new(10, 20));
    }

    #[test]
    fn position_from_grid_vec() {
        let gv = GridVec::new(5, -3);
        let pos = Position::from(gv);
        assert_eq!(pos.x, 5);
        assert_eq!(pos.y, -3);
    }

    #[test]
    fn grid_vec_from_position() {
        let pos = Position { x: 7, y: 13 };
        let gv: GridVec = pos.into();
        assert_eq!(gv, GridVec::new(7, 13));
    }

    #[test]
    fn position_round_trip() {
        let original = Position { x: -42, y: 99 };
        let gv = original.as_grid_vec();
        let back = Position::from(gv);
        assert_eq!(original, back);
    }

    // ─── Health tests ────────────────────────────────────────────

    #[test]
    fn health_full() {
        let h = Health {
            current: 30,
            max: 30,
        };
        assert_eq!(h.current, h.max);
    }

    #[test]
    fn health_damage_reduces_current() {
        let mut h = Health {
            current: 30,
            max: 30,
        };
        h.current = (h.current - 5).max(0);
        assert_eq!(h.current, 25);
        assert_eq!(h.max, 30);
    }

    #[test]
    fn health_damage_clamps_to_zero() {
        let mut h = Health {
            current: 3,
            max: 30,
        };
        h.current = (h.current - 10).max(0);
        assert_eq!(h.current, 0);
    }

    // ─── CombatStats damage formula tests ────────────────────────

    #[test]
    fn damage_formula_positive() {
        let attacker = CombatStats {
            attack: 5,
            defense: 0,
        };
        let defender = CombatStats {
            attack: 0,
            defense: 2,
        };
        let damage = (attacker.attack - defender.defense).max(0);
        assert_eq!(damage, 3);
    }

    #[test]
    fn damage_formula_zero_when_defense_equals_attack() {
        let attacker = CombatStats {
            attack: 3,
            defense: 0,
        };
        let defender = CombatStats {
            attack: 0,
            defense: 3,
        };
        let damage = (attacker.attack - defender.defense).max(0);
        assert_eq!(damage, 0);
    }

    #[test]
    fn damage_formula_zero_when_defense_exceeds_attack() {
        let attacker = CombatStats {
            attack: 2,
            defense: 0,
        };
        let defender = CombatStats {
            attack: 0,
            defense: 10,
        };
        let damage = (attacker.attack - defender.defense).max(0);
        assert_eq!(damage, 0);
    }

    // ─── Energy / Speed tests ────────────────────────────────────

    #[test]
    fn action_cost_is_100() {
        assert_eq!(ACTION_COST, 100);
    }

    #[test]
    fn energy_accumulation_normal_speed() {
        let speed = Speed(100);
        let mut energy = Energy(0);
        energy.0 += speed.0;
        assert_eq!(energy.0, 100);
        assert!(energy.0 >= ACTION_COST);
    }

    #[test]
    fn energy_accumulation_slow_speed() {
        let speed = Speed(50);
        let mut energy = Energy(0);
        // After 1 tick: not enough to act
        energy.0 += speed.0;
        assert_eq!(energy.0, 50);
        assert!(energy.0 < ACTION_COST);
        // After 2 ticks: enough to act
        energy.0 += speed.0;
        assert_eq!(energy.0, 100);
        assert!(energy.0 >= ACTION_COST);
    }

    #[test]
    fn energy_accumulation_fast_speed() {
        let speed = Speed(200);
        let mut energy = Energy(0);
        energy.0 += speed.0;
        assert_eq!(energy.0, 200);
        // Fast entity can act twice
        assert!(energy.0 >= ACTION_COST);
        energy.0 -= ACTION_COST;
        assert!(energy.0 >= ACTION_COST);
    }

    #[test]
    fn energy_deduction_leaves_excess() {
        let speed = Speed(120);
        let mut energy = Energy(0);
        energy.0 += speed.0;
        energy.0 -= ACTION_COST;
        assert_eq!(energy.0, 20); // Excess carries over
    }

    // ─── AiState tests ──────────────────────────────────────────

    #[test]
    fn ai_state_idle_default() {
        let state = AiState::Idle;
        assert_eq!(state, AiState::Idle);
    }

    #[test]
    fn ai_state_transitions() {
        let state = AiState::Idle;
        assert_eq!(state, AiState::Idle);
        let state = AiState::Chasing;
        assert_eq!(state, AiState::Chasing);
    }

    // ─── Viewshed tests ─────────────────────────────────────────

    #[test]
    fn viewshed_dirty_flag() {
        let mut vs = Viewshed {
            range: 10,
            visible_tiles: HashSet::new(),
            revealed_tiles: HashSet::new(),
            dirty: true,
        };
        assert!(vs.dirty);
        vs.dirty = false;
        assert!(!vs.dirty);
    }

    #[test]
    fn viewshed_visible_tiles_insert() {
        let mut vs = Viewshed {
            range: 10,
            visible_tiles: HashSet::new(),
            revealed_tiles: HashSet::new(),
            dirty: true,
        };
        let point = GridVec::new(5, 5);
        vs.visible_tiles.insert(point);
        assert!(vs.visible_tiles.contains(&point));
    }

    #[test]
    fn viewshed_revealed_accumulates() {
        let mut vs = Viewshed {
            range: 10,
            visible_tiles: HashSet::new(),
            revealed_tiles: HashSet::new(),
            dirty: true,
        };
        let p1 = GridVec::new(1, 1);
        let p2 = GridVec::new(2, 2);
        vs.revealed_tiles.insert(p1);
        vs.revealed_tiles.insert(p2);
        // Clearing visible doesn't affect revealed
        vs.visible_tiles.clear();
        assert!(vs.revealed_tiles.contains(&p1));
        assert!(vs.revealed_tiles.contains(&p2));
    }
}
