use std::collections::HashMap;

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
}

/// Turn-phase sub-state that controls the flow within `GameState::Playing`.
///
/// State machine:
///   AwaitingInput → PlayerTurn → WorldTurn → AwaitingInput
///
/// - **AwaitingInput** – input system is active; game waits for player action.
/// - **PlayerTurn** – player's action is resolved (movement, combat).
/// - **WorldTurn** – NPC AI and world-tick systems run.
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
