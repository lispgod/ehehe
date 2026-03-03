use bevy::prelude::*;

use crate::components::{Health, Stamina, Player, Position};
use crate::grid_vec::GridVec;
use crate::resources::{CombatLog, DynamicRng, ExtraWorldTicks, GameMapResource, SoundEvents, SpectatingAfterDeath, TurnCounter, TurnState};
use crate::typeenums::Floor;

/// Stamina regenerated per world turn.
const STAMINA_REGEN_PER_TURN: i32 = 2;

/// Health regenerated per turn (passive, slower than stamina).
const HEALTH_REGEN_PER_TURN: i32 = 1;

/// Health regeneration only triggers every N turns to keep it slower than stamina.
const HEALTH_REGEN_INTERVAL: u32 = 30;

/// Fire spreads every N world turns.
const FIRE_SPREAD_INTERVAL: u32 = 4;

/// Damage dealt to entities standing on fire per turn.
const FIRE_DAMAGE: i32 = 2;

/// Maximum number of world turns a fire tile persists before burning out.
const FIRE_BURNOUT_TURNS: u32 = 20;

/// Number of world turns before a sand cloud tile dissipates.
const SAND_CLOUD_LIFETIME: u32 = 8;

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
    mut sound_events: ResMut<SoundEvents>,
    spectating: Res<SpectatingAfterDeath>,
    mut dynamic_rng: ResMut<DynamicRng>,
) {
    turn_counter.0 += 1;
    dynamic_rng.advance();
    sound_events.tick();

    if let Ok((mut stamina, mut health)) = player_query.single_mut() {
        // Regenerate player stamina using the pool's recover method.
        stamina.recover(STAMINA_REGEN_PER_TURN);

        // Regenerate player health (slower than stamina — every N turns).
        if turn_counter.0.is_multiple_of(HEALTH_REGEN_INTERVAL) {
            health.heal(HEALTH_REGEN_PER_TURN);
        }
    }

    // If spectating after death, stay in Playing and go to AwaitingInput
    // so the input system can auto-advance the next turn.
    if spectating.0 {
        extra_ticks.0 = 0;
        next_state.set(TurnState::AwaitingInput);
        return;
    }

    if extra_ticks.0 > 0 {
        extra_ticks.0 -= 1;
        // Stay in WorldTurn for the extra tick — don't transition yet.
    } else {
        next_state.set(TurnState::AwaitingInput);
    }
}

/// Fire system: spreads fire to adjacent flammable tiles and damages entities
/// standing on fire. Fire tiles burn out deterministically after
/// `FIRE_BURNOUT_TURNS` world turns, destroying any furniture and leaving
/// scorched earth.
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
        if let Some(voxel) = game_map.0.get_voxel_at(&p)
            && matches!(voxel.floor, Some(Floor::Fire))
                && let Ok(mut hp) = health_query.get_mut(entity) {
                    let actual = hp.apply_damage(FIRE_DAMAGE);
                    if actual > 0 {
                        combat_log.push(format!("Fire burns for {actual} damage!"));
                    }
                }
    }

    // Register any new fire tiles that the tracker doesn't know about yet.
    for y in 1..map_height - 1 {
        for x in 1..map_width - 1 {
            let pos = GridVec::new(x, y);
            if let Some(voxel) = game_map.0.get_voxel_at(&pos)
                && matches!(voxel.floor, Some(Floor::Fire)) {
                    game_map.0.fire_turns.entry(pos).or_insert(turn_counter.0);
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

                // Deterministic burnout: fire burns out after FIRE_BURNOUT_TURNS world turns.
                if let Some(&ignited_at) = game_map.0.fire_turns.get(&pos)
                    && turn_counter.0.saturating_sub(ignited_at) >= FIRE_BURNOUT_TURNS {
                        burnout_tiles.push(pos);
                    }

                // Spread fire to adjacent flammable furniture and wooden floors.
                for neighbor in pos.cardinal_neighbors() {
                    if let Some(n_voxel) = game_map.0.get_voxel_at(&neighbor) {
                        // Skip tiles already on fire
                        if matches!(n_voxel.floor, Some(Floor::Fire)) {
                            continue;
                        }
                        let has_flammable_furniture = n_voxel.furniture.as_ref()
                            .is_some_and(|f| f.is_flammable());
                        let has_wood_floor = matches!(n_voxel.floor, Some(Floor::WoodPlanks));
                        if has_flammable_furniture || has_wood_floor {
                            new_fire_tiles.push(neighbor);
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

    // Apply burnout: destroy any remaining furniture and leave scorched earth.
    for tile in &burnout_tiles {
        if let Some(voxel) = game_map.0.get_voxel_at_mut(tile) {
            voxel.furniture = None;
            voxel.floor = Some(Floor::ScorchedEarth);
        }
        game_map.0.fire_turns.remove(tile);
    }

    // ── Sand cloud dissipation ──────────────────────────────────────
    // Remove sand cloud tiles that have exceeded their lifetime.
    let expired_clouds: Vec<GridVec> = game_map.0.sand_cloud_turns.iter()
        .filter(|entry| turn_counter.0.saturating_sub(*entry.1) >= SAND_CLOUD_LIFETIME)
        .map(|entry| *entry.0)
        .collect();

    for tile in &expired_clouds {
        if let Some(voxel) = game_map.0.get_voxel_at_mut(tile) {
            if matches!(voxel.floor, Some(Floor::SandCloud)) {
                // Restore to sand (desert default).
                voxel.floor = Some(Floor::Sand);
            }
        }
        game_map.0.sand_cloud_turns.remove(tile);
    }
}
