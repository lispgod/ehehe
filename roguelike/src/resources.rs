use std::collections::{HashMap, VecDeque};

use bevy::prelude::*;

use crate::gamemap::GameMap;
use crate::noise::NoiseSeed;
use crate::typedefs::MyPoint;

/// Bevy resource wrapping the game map for ECS access.
#[derive(Resource)]
pub struct GameMapResource(pub GameMap);

/// Bevy resource holding the camera position (follows the tracked entity).
#[derive(Resource)]
pub struct CameraPosition(pub MyPoint);

/// Bevy resource holding the seed used for deterministic procedural generation.
///
/// Changing this seed produces a completely different but equally valid map.
/// Keeping the same seed always reproduces exactly the same world, which is
/// essential for debugging, replays, and multiplayer synchronization.
#[derive(Resource, Debug, Clone, Copy)]
pub struct MapSeed(pub NoiseSeed);

/// Top-level game state managed by Bevy's state machine.
/// Systems that should only run during gameplay use
/// `.run_if(in_state(GameState::Playing))`.
#[derive(States, Clone, Copy, Default, Eq, PartialEq, Hash, Debug)]
pub enum GameState {
    #[default]
    Playing,
    Paused,
    Victory,
}

/// Turn-phase sub-state that controls the flow within `GameState::Playing`.
///
/// State machine:
///   AwaitingInput → PlayerTurn → WorldTurn → AwaitingInput
///
/// - **AwaitingInput** – input system is active; game waits for player action.
/// - **PlayerTurn** – player's action is resolved (movement, combat).
/// - **WorldTurn** – NPC AI and world-tick systems run; energy is accumulated.
#[derive(SubStates, Clone, Copy, Default, Eq, PartialEq, Hash, Debug)]
#[source(GameState = GameState::Playing)]
pub enum TurnState {
    #[default]
    AwaitingInput,
    PlayerTurn,
    WorldTurn,
}

/// Spatial index for O(1) entity-at-position lookup.
///
/// Maintained by `spatial_index_system` which runs at the start of every
/// `Update` tick. Any system that needs to know "which entities are at
/// position X" reads this resource instead of iterating all entities.
#[derive(Resource, Debug, Default)]
pub struct SpatialIndex {
    pub map: HashMap<MyPoint, Vec<Entity>>,
}

impl SpatialIndex {
    /// Returns all entities occupying the given tile, or an empty slice.
    pub fn entities_at(&self, point: &MyPoint) -> &[Entity] {
        self.map.get(point).map_or(&[], |v| v.as_slice())
    }
}

/// Counts elapsed world turns. Used by the wave spawning system to
/// determine how many and how often new enemies appear.
#[derive(Resource, Debug, Default)]
pub struct TurnCounter(pub u32);

/// Tracks the total number of hostile entities killed by the player.
/// Displayed in the status bar as the player's score.
#[derive(Resource, Debug, Default)]
pub struct KillCount(pub u32);

/// Accumulator for combat log messages displayed in the status bar.
/// Maintains a rolling history of recent messages using a bounded
/// ring buffer (`VecDeque`) for O(1) enqueue/dequeue — the correct
/// data structure for a bounded FIFO queue.
#[derive(Resource, Debug, Default)]
pub struct CombatLog {
    pub messages: VecDeque<String>,
}

/// Maximum number of messages retained in the combat log.
const MAX_COMBAT_LOG_MESSAGES: usize = 50;

/// Whether the help overlay is currently shown (toggled by `?` or `/`).
#[derive(Resource, Debug, Default)]
pub struct HelpVisible(pub bool);

/// Whether the welcome screen is currently shown (visible at game start).
#[derive(Resource, Debug)]
pub struct WelcomeVisible(pub bool);

impl Default for WelcomeVisible {
    fn default() -> Self {
        Self(true) // shown on first launch
    }
}

/// Active spell particles for rendering AoE animations.
/// Each entry is (position, remaining_lifetime_frames, delay_before_visible).
/// Particles with delay > 0 are not yet visible; they count down each tick.
#[derive(Resource, Debug, Default)]
pub struct SpellParticles {
    pub particles: Vec<(MyPoint, u32, u32)>,
}

/// Maximum number of active spell particles to prevent unbounded growth.
const MAX_PARTICLES: usize = 800;

impl SpellParticles {
    /// Adds an expanding ring of particles for an AoE spell.
    /// Particles at greater distances from the origin appear later, creating
    /// an outward-traveling wave effect.
    pub fn add_aoe(&mut self, origin: MyPoint, lifetime: u32) {
        let radius = 3i32; // visual radius of the particle ring
        let frames_per_ring = 2u32; // ticks of delay per distance unit

        for r in 1..=radius {
            // Generate a ring of particles at Chebyshev distance r
            for dx in -r..=r {
                for dy in -r..=r {
                    // Only the perimeter of the Chebyshev ring
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    if self.particles.len() >= MAX_PARTICLES {
                        return;
                    }
                    let pos = origin + MyPoint::new(dx, dy);
                    let delay = (r as u32 - 1) * frames_per_ring;
                    self.particles.push((pos, lifetime, delay));
                }
            }
        }
    }

    /// Ticks all particles: counts down delays, then lifetimes. Removes expired ones.
    pub fn tick(&mut self) {
        self.particles.retain_mut(|(_, life, delay)| {
            if *delay > 0 {
                *delay -= 1;
                true // still waiting to appear
            } else {
                *life = life.saturating_sub(1);
                *life > 0
            }
        });
    }
}

impl CombatLog {
    /// Adds a message and trims the oldest entry to keep the log bounded.
    /// O(1) amortised via `VecDeque::pop_front` (no element shifting).
    pub fn push(&mut self, message: String) {
        self.messages.push_back(message);
        if self.messages.len() > MAX_COMBAT_LOG_MESSAGES {
            self.messages.pop_front();
        }
    }

    /// Returns the most recent `n` messages as owned references.
    pub fn recent(&self, n: usize) -> Vec<&str> {
        let len = self.messages.len();
        let start = len.saturating_sub(n);
        self.messages.iter().skip(start).map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid_vec::GridVec;

    // ─── CombatLog tests ─────────────────────────────────────────

    #[test]
    fn combat_log_default_is_empty() {
        let log = CombatLog::default();
        assert!(log.messages.is_empty());
    }

    #[test]
    fn combat_log_push_adds_message() {
        let mut log = CombatLog::default();
        log.push("Hello".into());
        assert_eq!(log.messages.len(), 1);
        assert_eq!(log.messages[0], "Hello");
    }

    #[test]
    fn combat_log_push_multiple_preserves_order() {
        let mut log = CombatLog::default();
        log.push("First".into());
        log.push("Second".into());
        log.push("Third".into());
        let msgs: Vec<&str> = log.messages.iter().map(|s| s.as_str()).collect();
        assert_eq!(msgs, vec!["First", "Second", "Third"]);
    }

    #[test]
    fn combat_log_rolling_window_trims_oldest() {
        let mut log = CombatLog::default();
        for i in 0..60 {
            log.push(format!("msg-{i}"));
        }
        assert_eq!(log.messages.len(), MAX_COMBAT_LOG_MESSAGES);
        // Oldest messages (0..9) were trimmed; first remaining is msg-10.
        assert_eq!(log.messages[0], "msg-10");
        assert_eq!(
            log.messages[MAX_COMBAT_LOG_MESSAGES - 1],
            "msg-59"
        );
    }

    #[test]
    fn combat_log_recent_returns_last_n() {
        let mut log = CombatLog::default();
        log.push("A".into());
        log.push("B".into());
        log.push("C".into());
        log.push("D".into());
        let recent = log.recent(2);
        assert_eq!(recent, vec!["C", "D"]);
    }

    #[test]
    fn combat_log_recent_more_than_available() {
        let mut log = CombatLog::default();
        log.push("Only".into());
        let recent = log.recent(10);
        assert_eq!(recent, vec!["Only"]);
    }

    #[test]
    fn combat_log_recent_zero() {
        let mut log = CombatLog::default();
        log.push("Something".into());
        let recent = log.recent(0);
        assert!(recent.is_empty());
    }

    #[test]
    fn combat_log_recent_on_empty() {
        let log = CombatLog::default();
        let recent = log.recent(5);
        assert!(recent.is_empty());
    }

    // ─── SpatialIndex tests ──────────────────────────────────────

    #[test]
    fn spatial_index_default_empty() {
        let index = SpatialIndex::default();
        assert!(index.map.is_empty());
    }

    #[test]
    fn spatial_index_entities_at_empty_tile() {
        let index = SpatialIndex::default();
        let point = GridVec::new(5, 5);
        assert!(index.entities_at(&point).is_empty());
    }

    #[test]
    fn spatial_index_entities_at_populated_tile() {
        let mut index = SpatialIndex::default();
        let point = GridVec::new(3, 7);
        let entity = Entity::from_bits(42);
        index.map.entry(point).or_default().push(entity);
        let result = index.entities_at(&point);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], entity);
    }

    #[test]
    fn spatial_index_multiple_entities_at_same_tile() {
        let mut index = SpatialIndex::default();
        let point = GridVec::new(0, 0);
        let e1 = Entity::from_bits(1);
        let e2 = Entity::from_bits(2);
        index.map.entry(point).or_default().push(e1);
        index.map.entry(point).or_default().push(e2);
        let result = index.entities_at(&point);
        assert_eq!(result.len(), 2);
        assert!(result.contains(&e1));
        assert!(result.contains(&e2));
    }
}
