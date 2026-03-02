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
///
/// **Invariant**: `0 ≤ current ≤ max` is maintained by all mutating methods.
/// Direct field mutation is allowed for compatibility but callers should
/// prefer the provided methods to guarantee correctness.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Health {
    pub current: CoordinateUnit,
    pub max: CoordinateUnit,
}

impl Health {
    /// Applies `amount` damage, clamping `current` to `[0, max]`.
    /// Returns the *actual* damage dealt (may be less if current < amount).
    ///
    /// **Post-condition**: `0 ≤ self.current ≤ self.max`.
    #[inline]
    pub fn apply_damage(&mut self, amount: CoordinateUnit) -> CoordinateUnit {
        let actual = amount.min(self.current).max(0);
        self.current -= actual;
        actual
    }

    /// Heals by `amount`, clamping `current` to `[0, max]`.
    /// Returns the *actual* HP restored (may be less if near max).
    ///
    /// **Post-condition**: `0 ≤ self.current ≤ self.max`.
    #[inline]
    pub fn heal(&mut self, amount: CoordinateUnit) -> CoordinateUnit {
        let actual = amount.min(self.max - self.current).max(0);
        self.current += actual;
        actual
    }

    /// Returns `true` when the entity should be considered dead.
    #[inline]
    pub fn is_dead(&self) -> bool {
        self.current <= 0
    }

    /// Returns `true` when health is at maximum.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.current >= self.max
    }

    /// Returns the health fraction in `[0.0, 1.0]`.
    /// Returns `0.0` if `max` is zero (avoids division by zero).
    #[inline]
    pub fn fraction(&self) -> f64 {
        if self.max <= 0 {
            return 0.0;
        }
        (self.current as f64 / self.max as f64).clamp(0.0, 1.0)
    }
}

/// Combat statistics used by the combat system to resolve attacks.
/// Damage dealt = max(0, attacker.attack − defender.defense).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct CombatStats {
    pub attack: CoordinateUnit,
    pub defense: CoordinateUnit,
}

impl CombatStats {
    /// Computes the raw damage this attacker deals against `defender`.
    ///
    /// **Damage model**: `max(0, self.attack − defender.defense)`.
    ///
    /// Mathematical properties:
    /// - **Non-negative**: result ∈ ℤ≥0 (clamped by `max(0, …)`).
    /// - **Monotone in attack**: higher attack → equal or more damage.
    /// - **Monotone in defense**: higher defense → equal or less damage.
    /// - **Idempotent clamping**: `max(0, max(0, x)) = max(0, x)`.
    #[inline]
    pub fn damage_against(&self, defender: &CombatStats) -> CoordinateUnit {
        compute_damage(self.attack, defender.defense)
    }
}

/// Pure function: computes melee/combat damage from attack and defense values.
///
/// `damage(atk, def) = max(0, atk − def)`
///
/// This is the universal damage formula used by:
/// - Melee bump attacks (combat_system)
/// - Roundhouse kick / cleave (melee_wide_system)
/// - Thrown weapons (throw_system)
///
/// **Mathematical properties**:
/// - **Non-negative**: ∀ atk, def: `damage(atk, def) ≥ 0`.
/// - **Monotone increasing in atk**: `atk₁ ≤ atk₂ ⟹ damage(atk₁, def) ≤ damage(atk₂, def)`.
/// - **Monotone decreasing in def**: `def₁ ≤ def₂ ⟹ damage(atk, def₁) ≥ damage(atk, def₂)`.
/// - **Zero threshold**: `damage(atk, def) = 0 ⟺ atk ≤ def`.
/// - **Linearity above threshold**: for `atk > def`, `damage(atk, def) = atk − def`.
#[inline]
pub fn compute_damage(attack: CoordinateUnit, defense: CoordinateUnit) -> CoordinateUnit {
    (attack - defense).max(0)
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

impl Energy {
    /// Returns `true` when this entity has accumulated enough energy to act.
    #[inline]
    pub fn can_act(&self) -> bool {
        self.0 >= ACTION_COST
    }

    /// Deducts `ACTION_COST`, leaving any excess for the next tick.
    ///
    /// **Pre-condition**: `self.can_act()` should be true.
    /// **Post-condition**: `self.0 = old − ACTION_COST`.
    #[inline]
    pub fn spend_action(&mut self) {
        self.0 -= ACTION_COST;
    }

    /// Accumulates energy from the entity's speed.
    ///
    /// This is one tick of the discrete energy scheduler:
    ///   `energy += speed`
    #[inline]
    pub fn accumulate(&mut self, speed: &Speed) {
        self.0 += speed.0;
    }
}

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

/// Faction affiliation for group-based spawning.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub enum Faction {
    Wildlife,
    Outlaws,
    Lawmen,
    Vaqueros,
}

/// Bullet caliber for period-accurate cap-and-ball revolvers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Caliber {
    /// .31 caliber (Colt Pocket)
    Cal31,
    /// .36 caliber (Colt Navy, Colt Sheriff)
    Cal36,
    /// .44 caliber (Colt Army, Remington New Model Army)
    Cal44,
}

impl std::fmt::Display for Caliber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Caliber::Cal31 => write!(f, ".31"),
            Caliber::Cal36 => write!(f, ".36"),
            Caliber::Cal44 => write!(f, ".44"),
        }
    }
}

/// Stamina pool for entities that can perform special actions.
/// Special actions consume stamina; stamina regenerates slowly each turn.
///
/// **Invariant**: `0 ≤ current ≤ max`.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Stamina {
    pub current: CoordinateUnit,
    pub max: CoordinateUnit,
}

impl Stamina {
    /// Attempts to spend `cost` stamina. Returns `true` and deducts if
    /// sufficient stamina is available, `false` otherwise (no mutation).
    ///
    /// **Post-condition**: if `true`, `self.current` decreased by `cost`.
    #[inline]
    pub fn spend(&mut self, cost: CoordinateUnit) -> bool {
        if self.current >= cost {
            self.current -= cost;
            true
        } else {
            false
        }
    }

    /// Recovers `amount` stamina, clamped to `max`.
    ///
    /// **Post-condition**: `self.current ≤ self.max`.
    #[inline]
    pub fn recover(&mut self, amount: CoordinateUnit) {
        self.current = (self.current + amount).min(self.max);
    }
}

/// Ammo supply for AI ranged attacks (enemies only).
/// Player weapons use per-gun `ItemKind::Gun { loaded, .. }` instead.
///
/// **Invariant**: `0 ≤ current ≤ max`.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Ammo {
    pub current: CoordinateUnit,
    pub max: CoordinateUnit,
}

impl Ammo {
    /// Attempts to spend one round. Returns `true` on success.
    #[inline]
    pub fn spend_one(&mut self) -> bool {
        if self.current > 0 {
            self.current -= 1;
            true
        } else {
            false
        }
    }

    /// Returns `true` when no ammo remains.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.current <= 0
    }
}

/// A projectile entity (bullet or shrapnel) that travels along a path over ticks.
/// Each tick the projectile advances `tiles_per_tick` steps along its precomputed
/// Bresenham path. When it reaches a hostile entity, it applies damage and despawns.
/// Bullets and shrapnel can move multiple tiles in one tick.
#[derive(Component, Debug)]
pub struct Projectile {
    /// Precomputed path tiles (Bresenham line from origin to endpoint).
    pub path: Vec<GridVec>,
    /// Current index along the path (how far the projectile has traveled).
    pub path_index: usize,
    /// Number of tiles the projectile advances per tick.
    pub tiles_per_tick: usize,
    /// Damage to apply on hit.
    pub damage: i32,
    /// Remaining penetration power. Decreases by target defense on each hit.
    pub penetration: i32,
    /// Entity that fired the projectile (to avoid self-damage for bullets).
    pub source: Entity,
}

// ─── Inventory & Item system ─────────────────────────────────────

/// Marker component: tags an entity as an item that can be picked up.
#[derive(Component, Debug)]
pub struct Item;

/// The kind of item and its associated effect.
#[derive(Component, Clone, Debug, PartialEq)]
pub enum ItemKind {
    /// A cap-and-ball revolver. Tracks loaded rounds and caliber.
    Gun {
        loaded: i32,
        capacity: i32,
        caliber: Caliber,
        attack: i32,
        name: String,
    },
    /// A throwing knife. Can be recovered after landing.
    Knife { attack: i32 },
    /// A throwing tomahawk. Can be recovered after landing.
    Tomahawk { attack: i32 },
    /// A grenade (dynamite stick). Deals area damage.
    Grenade { damage: i32, radius: i32 },
    /// Whiskey bottle. Restores health when consumed.
    Whiskey { heal: i32 },
    /// A hat. Provides defense when equipped.
    Hat { defense: i32 },
}

/// Marker component for a thrown item (knife/tomahawk) that has landed
/// and can be recovered by walking over it.
#[derive(Component, Debug)]
pub struct Thrown;

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
/// Each level grants stat bonuses (attack, defense, max HP, max stamina).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Level(pub i32);

/// Type of collectible supply drop.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub enum CollectibleKind {
    Caps(i32),
    Bullets31(i32),
    Bullets36(i32),
    Bullets44(i32),
    Powder(i32),
    Bandages(i32),
    Dollars(i32),
}

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
        let actual = h.apply_damage(5);
        assert_eq!(actual, 5);
        assert_eq!(h.current, 25);
        assert_eq!(h.max, 30);
    }

    #[test]
    fn health_damage_clamps_to_zero() {
        let mut h = Health {
            current: 3,
            max: 30,
        };
        let actual = h.apply_damage(10);
        assert_eq!(actual, 3, "Actual damage should be clamped to current HP");
        assert_eq!(h.current, 0);
    }

    #[test]
    fn health_apply_damage_returns_actual() {
        let mut h = Health { current: 5, max: 30 };
        assert_eq!(h.apply_damage(3), 3);
        assert_eq!(h.current, 2);
        assert_eq!(h.apply_damage(10), 2);
        assert_eq!(h.current, 0);
    }

    #[test]
    fn health_heal_clamps_to_max() {
        let mut h = Health { current: 25, max: 30 };
        let healed = h.heal(10);
        assert_eq!(healed, 5, "Should only heal the deficit");
        assert_eq!(h.current, 30);
    }

    #[test]
    fn health_heal_returns_actual() {
        let mut h = Health { current: 20, max: 30 };
        assert_eq!(h.heal(5), 5);
        assert_eq!(h.current, 25);
    }

    #[test]
    fn health_heal_at_full_returns_zero() {
        let mut h = Health { current: 30, max: 30 };
        assert_eq!(h.heal(5), 0);
        assert_eq!(h.current, 30);
    }

    #[test]
    fn health_is_dead() {
        assert!(Health { current: 0, max: 30 }.is_dead());
        assert!(!Health { current: 1, max: 30 }.is_dead());
    }

    #[test]
    fn health_is_full() {
        assert!(Health { current: 30, max: 30 }.is_full());
        assert!(!Health { current: 29, max: 30 }.is_full());
    }

    #[test]
    fn health_fraction() {
        let h = Health { current: 15, max: 30 };
        assert!((h.fraction() - 0.5).abs() < 1e-10);
    }

    #[test]
    fn health_fraction_zero_max() {
        let h = Health { current: 0, max: 0 };
        assert_eq!(h.fraction(), 0.0);
    }

    #[test]
    fn health_invariant_maintained() {
        let mut h = Health { current: 10, max: 30 };
        h.apply_damage(100);
        assert!(h.current >= 0 && h.current <= h.max);
        h.heal(100);
        assert!(h.current >= 0 && h.current <= h.max);
    }

    // ─── CombatStats damage formula tests ────────────────────────

    #[test]
    fn damage_formula_positive() {
        let attacker = CombatStats { attack: 5, defense: 0 };
        let defender = CombatStats { attack: 0, defense: 2 };
        assert_eq!(attacker.damage_against(&defender), 3);
        assert_eq!(compute_damage(5, 2), 3);
    }

    #[test]
    fn damage_formula_zero_when_defense_equals_attack() {
        let attacker = CombatStats { attack: 3, defense: 0 };
        let defender = CombatStats { attack: 0, defense: 3 };
        assert_eq!(attacker.damage_against(&defender), 0);
        assert_eq!(compute_damage(3, 3), 0);
    }

    #[test]
    fn damage_formula_zero_when_defense_exceeds_attack() {
        let attacker = CombatStats { attack: 2, defense: 0 };
        let defender = CombatStats { attack: 0, defense: 10 };
        assert_eq!(attacker.damage_against(&defender), 0);
        assert_eq!(compute_damage(2, 10), 0);
    }

    #[test]
    fn compute_damage_non_negative() {
        // Property: ∀ atk, def: compute_damage(atk, def) ≥ 0
        for atk in 0..20 {
            for def in 0..20 {
                assert!(compute_damage(atk, def) >= 0);
            }
        }
    }

    #[test]
    fn compute_damage_monotone_in_attack() {
        // Property: atk₁ ≤ atk₂ ⟹ damage(atk₁, def) ≤ damage(atk₂, def)
        let def = 3;
        for atk in 0..19 {
            assert!(compute_damage(atk, def) <= compute_damage(atk + 1, def));
        }
    }

    #[test]
    fn compute_damage_monotone_in_defense() {
        // Property: def₁ ≤ def₂ ⟹ damage(atk, def₁) ≥ damage(atk, def₂)
        let atk = 10;
        for def in 0..19 {
            assert!(compute_damage(atk, def) >= compute_damage(atk, def + 1));
        }
    }

    #[test]
    fn compute_damage_zero_threshold() {
        // Property: damage(atk, def) = 0 ⟺ atk ≤ def
        for atk in 0..15 {
            for def in 0..15 {
                let d = compute_damage(atk, def);
                if atk <= def {
                    assert_eq!(d, 0);
                } else {
                    assert!(d > 0);
                }
            }
        }
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
        energy.accumulate(&speed);
        assert_eq!(energy.0, 100);
        assert!(energy.can_act());
    }

    #[test]
    fn energy_accumulation_slow_speed() {
        let speed = Speed(50);
        let mut energy = Energy(0);
        // After 1 tick: not enough to act
        energy.accumulate(&speed);
        assert_eq!(energy.0, 50);
        assert!(!energy.can_act());
        // After 2 ticks: enough to act
        energy.accumulate(&speed);
        assert_eq!(energy.0, 100);
        assert!(energy.can_act());
    }

    #[test]
    fn energy_accumulation_fast_speed() {
        let speed = Speed(200);
        let mut energy = Energy(0);
        energy.accumulate(&speed);
        assert_eq!(energy.0, 200);
        // Fast entity can act twice
        assert!(energy.can_act());
        energy.spend_action();
        assert!(energy.can_act());
    }

    #[test]
    fn energy_deduction_leaves_excess() {
        let speed = Speed(120);
        let mut energy = Energy(0);
        energy.accumulate(&speed);
        energy.spend_action();
        assert_eq!(energy.0, 20); // Excess carries over
    }

    #[test]
    fn energy_scheduling_fairness() {
        // Property: over N ticks, entity with speed S takes
        // exactly ⌊N × S / ACTION_COST⌋ actions.
        for &spd in &[50, 75, 100, 120, 150, 200] {
            let speed = Speed(spd);
            let mut energy = Energy(0);
            let mut actions = 0i64;
            let n = 100;
            for _ in 0..n {
                energy.accumulate(&speed);
                while energy.can_act() {
                    energy.spend_action();
                    actions += 1;
                }
            }
            let expected = (n as i64 * spd as i64) / ACTION_COST as i64;
            assert_eq!(
                actions, expected,
                "Speed {spd}: expected {expected} actions over {n} ticks, got {actions}"
            );
        }
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

    // ─── Caliber display tests ──────────────────────────────────────

    #[test]
    fn caliber_display_formatting() {
        assert_eq!(format!("{}", Caliber::Cal31), ".31");
        assert_eq!(format!("{}", Caliber::Cal36), ".36");
        assert_eq!(format!("{}", Caliber::Cal44), ".44");
    }

    // ─── Gun ItemKind tests ─────────────────────────────────────────

    #[test]
    fn gun_loaded_rounds_decrement() {
        let mut gun = ItemKind::Gun {
            loaded: 6,
            capacity: 6,
            caliber: Caliber::Cal36,
            attack: 5,
            name: "Test Gun".into(),
        };
        if let ItemKind::Gun { ref mut loaded, .. } = gun {
            *loaded -= 1;
            assert_eq!(*loaded, 5);
        }
    }

    #[test]
    fn gun_cannot_fire_when_empty() {
        let gun = ItemKind::Gun {
            loaded: 0,
            capacity: 6,
            caliber: Caliber::Cal36,
            attack: 5,
            name: "Test Gun".into(),
        };
        if let ItemKind::Gun { loaded, .. } = gun {
            assert_eq!(loaded, 0);
            assert!(loaded <= 0, "Gun should not be able to fire");
        }
    }

    // ─── Stamina pool tests ─────────────────────────────────────────

    #[test]
    fn stamina_spend_success() {
        let mut s = Stamina { current: 50, max: 50 };
        assert!(s.spend(10));
        assert_eq!(s.current, 40);
    }

    #[test]
    fn stamina_spend_insufficient() {
        let mut s = Stamina { current: 5, max: 50 };
        assert!(!s.spend(10));
        assert_eq!(s.current, 5, "Should not mutate on failed spend");
    }

    #[test]
    fn stamina_spend_exact() {
        let mut s = Stamina { current: 10, max: 50 };
        assert!(s.spend(10));
        assert_eq!(s.current, 0);
    }

    #[test]
    fn stamina_recover_clamps_to_max() {
        let mut s = Stamina { current: 45, max: 50 };
        s.recover(10);
        assert_eq!(s.current, 50);
    }

    #[test]
    fn stamina_recover_partial() {
        let mut s = Stamina { current: 30, max: 50 };
        s.recover(5);
        assert_eq!(s.current, 35);
    }

    #[test]
    fn stamina_invariant_maintained() {
        let mut s = Stamina { current: 25, max: 50 };
        s.spend(100); // Should fail (not enough), no mutation
        assert!(s.current >= 0 && s.current <= s.max);
        s.recover(100);
        assert!(s.current >= 0 && s.current <= s.max);
    }

    // ─── Ammo pool tests ────────────────────────────────────────────

    #[test]
    fn ammo_spend_one_success() {
        let mut a = Ammo { current: 5, max: 10 };
        assert!(a.spend_one());
        assert_eq!(a.current, 4);
    }

    #[test]
    fn ammo_spend_one_empty() {
        let mut a = Ammo { current: 0, max: 10 };
        assert!(!a.spend_one());
        assert_eq!(a.current, 0);
    }

    #[test]
    fn ammo_is_empty() {
        assert!(Ammo { current: 0, max: 10 }.is_empty());
        assert!(!Ammo { current: 1, max: 10 }.is_empty());
    }
}
