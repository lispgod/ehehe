use bevy::prelude::*;

use crate::components::{BlocksMovement, Health, Hostile, Player, Position, Stamina, Viewshed};
use crate::events::{AttackIntent, MoveIntent};
use crate::grid_vec::GridVec;
use crate::resources::{BloodMap, CombatLog, CursorPosition, GameMapResource, InputState, SpatialIndex, TurnCounter, TurnState};
use crate::typeenums::Props;

/// Processes `MoveIntent` events: checks the target tile on the `GameMap` for
/// walkability *and* the `SpatialIndex` for entities that block movement.
///
/// **Bump-to-attack**: if the target tile contains an entity the mover would
/// attack, emits an `AttackIntent` instead of moving. For the player, this
/// means walking into a `Hostile` entity. For `Hostile` entities, this means
/// walking into the `Player`. This is the standard roguelike mechanic where
/// walking into an enemy initiates melee combat.
///
/// **Spatial index atomicity**: after each successful move, the spatial index
/// is updated inline (entity removed from old tile, added to new tile). This
/// ensures that when processing multiple `MoveIntent`s in a single frame
/// (e.g., from AI during WorldTurn), subsequent intents see accurate
/// occupancy data. Without this, two entities could move to the same tile
/// simultaneously because the index would still show their original positions.
///
/// Also marks the entity's `Viewshed` as dirty so FOV is recalculated.
pub fn movement_system(
    mut intents: MessageReader<MoveIntent>,
    game_map: Res<GameMapResource>,
    mut spatial: ResMut<SpatialIndex>,
    mut cursor: ResMut<CursorPosition>,
    mut blood_map: ResMut<BloodMap>,
    turn_counter: Res<TurnCounter>,
    blockers: Query<(), With<BlocksMovement>>,
    hostiles: Query<(), With<Hostile>>,
    players: Query<(), With<Player>>,
    healths: Query<&Health>,
    mut attack_intents: MessageWriter<AttackIntent>,
    mut movers: Query<(&mut Position, Option<&mut Viewshed>)>,
) {
    for intent in intents.read() {
        let Ok((mut pos, viewshed)) = movers.get_mut(intent.entity) else {
            continue;
        };

        let target = pos.as_grid_vec() + GridVec::new(intent.dx, intent.dy);

        // ── Bump-to-attack ──────────────────────────────────────
        // Check if a hostile entity occupies the target tile (player attacks monster).
        let hostile_at_target = spatial.entities_at(&target).iter().find(|&&e| {
            e != intent.entity && hostiles.contains(e)
        });
        if let Some(&target_entity) = hostile_at_target {
            attack_intents.write(AttackIntent {
                attacker: intent.entity,
                target: target_entity,
            });
            continue;
        }

        // Check if the player occupies the target tile (monster attacks player).
        let player_at_target = spatial.entities_at(&target).iter().find(|&&e| {
            e != intent.entity && players.contains(e)
        });
        if let Some(&target_entity) = player_at_target
            && hostiles.contains(intent.entity) {
                attack_intents.write(AttackIntent {
                    attacker: intent.entity,
                    target: target_entity,
                });
                continue;
            }

        // 1. Check map tile walkability (no blocking props).
        let tile_passable = game_map.0.is_passable(&target);

        // 2. Check spatial index for blocking entities at the target.
        let entity_blocked = spatial.entities_at(&target).iter().any(|&e| {
            e != intent.entity && blockers.contains(e)
        });

        let is_player = players.contains(intent.entity);

        if tile_passable && !entity_blocked {
            let old_pos = pos.as_grid_vec();

            // ── Blood trail: wounded entities leave blood behind ─
            if let Ok(hp) = healths.get(intent.entity)
                && hp.current < hp.max {
                    blood_map.stains.insert(old_pos, turn_counter.0);
                }

            let delta = GridVec::new(intent.dx, intent.dy);
            pos.x = target.x;
            pos.y = target.y;

            // ── Maintain spatial index invariant ─────────────────
            // Atomically move the entity in the index so subsequent
            // intents in this frame see accurate occupancy.
            spatial.move_entity(&old_pos, target, intent.entity);

            // ── Cursor follows player movement ──────────────────
            // When the player moves, the cursor moves by the same delta
            // so the player keeps looking in the same relative direction.
            if is_player {
                cursor.pos += delta;
            }

            // Mark viewshed dirty so visibility is recalculated.
            if let Some(mut vs) = viewshed {
                vs.dirty = true;
            }
        }
    }

    // Periodically prune old blood stains to prevent unbounded growth.
    blood_map.prune(turn_counter.0);
}

/// Damage dealt by walking into a cactus (adjacent tile).
const CACTUS_DAMAGE: i32 = 1;

/// Applies cactus contact damage: any entity standing on a tile adjacent to a
/// cactus takes `CACTUS_DAMAGE` each turn. Runs after movement.
///
/// **Turn-gated**: only applies damage during `PlayerTurn` or `WorldTurn`,
/// not during `AwaitingInput` (which runs every frame at 30 FPS). Without
/// this gate the system would deal 30 damage/second, instantly killing
/// any entity near a cactus.
pub fn cactus_damage_system(
    game_map: Res<GameMapResource>,
    mut health_query: Query<(&Position, &mut Health, Option<&crate::components::Name>)>,
    mut combat_log: ResMut<CombatLog>,
    turn_state: Option<Res<State<TurnState>>>,
) {
    // Only apply cactus damage during actual turns, not every frame.
    let is_active_turn = turn_state.as_ref().is_some_and(|ts| {
        matches!(ts.get(), TurnState::PlayerTurn | TurnState::WorldTurn)
    });
    if !is_active_turn {
        return;
    }

    for (pos, mut hp, name) in &mut health_query {
        let p = pos.as_grid_vec();
        let entity_name = crate::components::display_name(name);
        // Check if standing adjacent to (or on) a cactus
        for neighbor in p.cardinal_neighbors() {
            if let Some(voxel) = game_map.0.get_voxel_at(&neighbor)
                && matches!(voxel.props, Some(Props::Cactus)) {
                    let actual = hp.apply_damage(CACTUS_DAMAGE);
                    if actual > 0 {
                        combat_log.push_at(
                            format!("{entity_name} is pricked by a cactus for {actual} damage!"),
                            p,
                        );
                    }
                    break; // Only one cactus damage per turn even if multiple adjacent
                }
        }
    }
}

/// Consumes pending dive stamina after movement is processed.
/// The input system sets `dive_stamina_pending` and the movement system
/// processes the move intents. This system deducts the stamina.
pub fn dive_stamina_system(
    mut input_state: ResMut<InputState>,
    mut player_query: Query<&mut Stamina, With<Player>>,
) {
    if input_state.dive_stamina_pending > 0 {
        if let Ok(mut stamina) = player_query.single_mut() {
            stamina.spend(input_state.dive_stamina_pending);
        }
        input_state.dive_stamina_pending = 0;
    }
    if input_state.ability_stamina_pending > 0 {
        if let Ok(mut stamina) = player_query.single_mut() {
            stamina.spend(input_state.ability_stamina_pending);
        }
        input_state.ability_stamina_pending = 0;
    }
}
