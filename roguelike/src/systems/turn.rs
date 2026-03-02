use bevy::prelude::*;

use crate::components::{Health, Stamina, Player, Position};
use crate::grid_vec::GridVec;
use crate::resources::{CombatLog, ExtraWorldTicks, GameMapResource, TurnCounter, TurnState};
use crate::typeenums::Floor;

/// Stamina regenerated per world turn.
const STAMINA_REGEN_PER_TURN: i32 = 2;

/// Health regenerated per turn (passive, slower than stamina).
const HEALTH_REGEN_PER_TURN: i32 = 1;

/// Health regeneration only triggers every N turns to keep it slower than stamina.
const HEALTH_REGEN_INTERVAL: u32 = 3;

/// Fire spreads every N world turns.
const FIRE_SPREAD_INTERVAL: u32 = 4;

/// Damage dealt to entities standing on fire per turn.
const FIRE_DAMAGE: i32 = 2;

/// Maximum number of world turns a fire tile persists before burning out.
const FIRE_BURNOUT_TURNS: u32 = 20;

/// Advances the turn state from `PlayerTurn` → `WorldTurn`.
/// Runs only during `TurnState::PlayerTurn` after all player-phase systems.
pub fn end_player_turn(mut next_state: ResMut<NextState<TurnState>>) {
    next_state.set(TurnState::WorldTurn);
}

/// Advances the turn state from `WorldTurn` → `AwaitingInput`, or stays in
/// `WorldTurn` if `ExtraWorldTicks` has remaining ticks (physical movement
/// costs 2 ticks). Increments the turn counter and regenerates player stats.
pub fn end_world_turn(
    mut next_state: ResMut<NextState<TurnState>>,
    mut turn_counter: ResMut<TurnCounter>,
    mut player_query: Query<(&mut Stamina, &mut Health), With<Player>>,
    mut extra_ticks: ResMut<ExtraWorldTicks>,
) {
    turn_counter.0 += 1;

    if let Ok((mut stamina, mut health)) = player_query.single_mut() {
        // Regenerate player stamina using the pool's recover method.
        stamina.recover(STAMINA_REGEN_PER_TURN);

        // Regenerate player health (slower than stamina — every N turns).
        if turn_counter.0.is_multiple_of(HEALTH_REGEN_INTERVAL) {
            health.heal(HEALTH_REGEN_PER_TURN);
        }
    }

    if extra_ticks.0 > 0 {
        extra_ticks.0 -= 1;
        // Stay in WorldTurn for the extra tick — don't transition yet.
    } else {
        next_state.set(TurnState::AwaitingInput);
    }
}

/// Fire system: spreads fire to adjacent flammable tiles and damages entities
/// standing on fire. Fire tiles eventually burn out to scorched earth.
///
/// Runs every world turn during `WorldTurn`.
pub fn fire_system(
    mut game_map: ResMut<GameMapResource>,
    turn_counter: Res<TurnCounter>,
    mut health_query: Query<&mut Health>,
    position_query: Query<(Entity, &Position)>,
    mut combat_log: ResMut<CombatLog>,
) {
    let map_width = game_map.0.width;
    let map_height = game_map.0.height;

    // Damage entities standing on fire tiles.
    for (entity, pos) in &position_query {
        let p = pos.as_grid_vec();
        if let Some(voxel) = game_map.0.get_voxel_at(&p) {
            if matches!(voxel.floor, Some(Floor::Fire)) {
                if let Ok(mut hp) = health_query.get_mut(entity) {
                    let actual = hp.apply_damage(FIRE_DAMAGE);
                    if actual > 0 {
                        combat_log.push(format!("Fire burns for {actual} damage!"));
                    }
                }
            }
        }
    }

    // Spread fire and burn out old fire tiles every FIRE_SPREAD_INTERVAL turns.
    if !turn_counter.0.is_multiple_of(FIRE_SPREAD_INTERVAL) {
        return;
    }

    // Collect fire tile positions and tiles to spread fire to.
    let mut new_fire_tiles: Vec<GridVec> = Vec::new();
    let mut burnout_tiles: Vec<GridVec> = Vec::new();

    for y in 1..map_height - 1 {
        for x in 1..map_width - 1 {
            let pos = GridVec::new(x, y);
            if let Some(voxel) = game_map.0.get_voxel_at(&pos) {
                if !matches!(voxel.floor, Some(Floor::Fire)) {
                    continue;
                }

                // Probabilistic burnout: each spread interval, each fire tile has
                // a 1/FIRE_BURNOUT_TURNS chance of burning out. Over time this
                // guarantees all fires eventually expire with an expected lifetime
                // of ~FIRE_BURNOUT_TURNS * FIRE_SPREAD_INTERVAL turns.
                let burnout_hash = (x.wrapping_mul(37) ^ y.wrapping_mul(53))
                    .wrapping_add(turn_counter.0 as i32);
                if burnout_hash.unsigned_abs() % FIRE_BURNOUT_TURNS == 0 {
                    burnout_tiles.push(pos);
                }

                // Spread fire to adjacent flammable furniture.
                for neighbor in pos.cardinal_neighbors() {
                    if let Some(n_voxel) = game_map.0.get_voxel_at(&neighbor) {
                        if let Some(ref furn) = n_voxel.furniture {
                            if furn.is_flammable() {
                                new_fire_tiles.push(neighbor);
                            }
                        }
                    }
                }
            }
        }
    }

    // Apply fire spread.
    for tile in new_fire_tiles {
        if let Some(voxel) = game_map.0.get_voxel_at_mut(&tile) {
            voxel.furniture = None;
            voxel.floor = Some(Floor::Fire);
        }
    }

    // Apply burnout.
    for tile in burnout_tiles {
        if let Some(voxel) = game_map.0.get_voxel_at_mut(&tile) {
            voxel.floor = Some(Floor::ScorchedEarth);
        }
    }
}
