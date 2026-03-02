use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use bevy::prelude::*;

use crate::components::{AiLookDir, AiState, Ammo, BlocksMovement, Energy, Faction, Inventory, ItemKind, PatrolOrigin, Player, Position, Speed, Viewshed};
use crate::events::{AiRangedAttackIntent, AttackIntent, MolotovCastIntent, MoveIntent, SpellCastIntent};
use crate::grid_vec::GridVec;
use crate::resources::{GameMapResource, SpatialIndex, TurnCounter};

// ───────────────────────── A* Pathfinding ──────────────────────────

/// Maximum number of nodes A* may explore before giving up.
/// Prevents lag when the target is unreachable or far away.
/// 256 nodes covers a ~16-tile radius search area, sufficient
/// for navigating around most local obstacles.
const MAX_A_STAR_NODES: usize = 256;

/// Finds the first step direction from `start` toward `goal` using A*
/// with the **Chebyshev heuristic** (L∞ norm).
///
/// **Mathematical properties:**
/// - **Optimal**: Chebyshev distance is admissible (`h(n) ≤ h*(n)`)
///   and consistent (`h(n) ≤ c(n,n') + h(n')`) for 8-connected grids
///   with uniform cost. A* therefore finds the shortest path.
/// - **Complete**: if a path exists within the exploration budget,
///   it will be found.
/// - **Time**: O(k log k) where k = nodes explored (≤ `MAX_A_STAR_NODES`).
/// - **Space**: O(k) for the open/closed sets and came-from map.
///
/// The `is_walkable` closure abstracts over game map + entity collision
/// checks, making the algorithm reusable and testable in isolation.
///
/// Returns the direction `GridVec` of the first step, or `None` if no
/// path is found within the exploration budget.
fn a_star_first_step(
    start: GridVec,
    goal: GridVec,
    is_walkable: impl Fn(GridVec) -> bool,
) -> Option<GridVec> {
    // Already at the goal — no step needed.
    if start == goal {
        return None;
    }

    // Adjacent and walkable — return the direct step without full search.
    if start.chebyshev_distance(goal) == 1 {
        return Some(goal - start);
    }

    // Min-heap: (f_score, h_score, position). Reverse gives min-first ordering.
    //
    // **Tie-breaking**: when two nodes share the same f-score, we prefer the
    // one with the lower h-score (i.e., higher g-score, meaning closer to the
    // goal along the discovered path). This is a standard A* optimisation that
    // reduces the number of expanded nodes while preserving optimality, since
    // among equal-f nodes, those with smaller h are nearer to the goal.
    let mut open: BinaryHeap<Reverse<(i32, i32, GridVec)>> = BinaryHeap::new();
    let mut came_from: HashMap<GridVec, GridVec> = HashMap::new();
    let mut g_score: HashMap<GridVec, i32> = HashMap::new();
    let mut closed: HashSet<GridVec> = HashSet::new();

    let h_start = start.chebyshev_distance(goal);
    g_score.insert(start, 0);
    open.push(Reverse((h_start, h_start, start)));

    let mut explored = 0usize;

    while let Some(Reverse((_, _, current))) = open.pop() {
        if current == goal {
            // Reconstruct path to extract the first step.
            let mut step = current;
            while let Some(&prev) = came_from.get(&step) {
                if prev == start {
                    return Some(step - start);
                }
                step = prev;
            }
            return None;
        }

        if !closed.insert(current) {
            continue; // Already expanded.
        }

        explored += 1;
        if explored >= MAX_A_STAR_NODES {
            break; // Budget exhausted.
        }

        let current_g = g_score[&current];

        for dir in GridVec::DIRECTIONS_8 {
            let neighbor = current + dir;

            if closed.contains(&neighbor) {
                continue;
            }

            // The goal tile is always "walkable" (we want to reach it).
            if neighbor != goal && !is_walkable(neighbor) {
                continue;
            }

            let new_g = current_g + 1; // Uniform edge cost.
            if new_g < *g_score.get(&neighbor).unwrap_or(&i32::MAX) {
                came_from.insert(neighbor, current);
                g_score.insert(neighbor, new_g);
                let h = neighbor.chebyshev_distance(goal);
                let f = new_g + h;
                open.push(Reverse((f, h, neighbor)));
            }
        }
    }

    None // No path found within budget.
}

// ───────────────────────── AI System ───────────────────────────────

/// AI range for soldier ranged attacks.
const AI_RANGED_ATTACK_RANGE: i32 = 15;

/// Returns `true` if two factions are hostile to each other.
/// - Outlaws and Lawmen fight each other.
/// - Wildlife attacks everyone.
/// - Vaqueros fight Outlaws and Wildlife.
pub fn factions_are_hostile(a: Faction, b: Faction) -> bool {
    if a == b {
        return false;
    }
    matches!(
        (a, b),
        (Faction::Outlaws, Faction::Lawmen)
        | (Faction::Lawmen, Faction::Outlaws)
        | (Faction::Wildlife, _)
        | (_, Faction::Wildlife)
        | (Faction::Vaqueros, Faction::Outlaws)
        | (Faction::Outlaws, Faction::Vaqueros)
    )
}

/// Patrol radius: how far an NPC will wander from its spawn point.
const PATROL_RADIUS: i32 = 8;

/// AI system: runs during `WorldTurn` for every entity with an `AiState`.
///
/// **Behaviour**:
/// - **Idle**: if the player is within the entity's Viewshed, switch to `Chasing`.
/// - **Patrolling**: wander around patrol origin. If player or enemy faction
///   entity is spotted, switch to `Chasing`.
///   - Lawmen: patrol in a structured pattern around their origin.
///   - Wildlife: random wandering with occasional idle pauses.
///   - Outlaws: skulk (move slowly, stay near buildings).
/// - **Chasing**: use **A\* pathfinding** to pursue the player or nearest
///   hostile faction entity. NPCs with guns fire them; NPCs with throwables
///   (dynamite, molotovs) use them at range.
///
/// NPCs also fight entities of hostile factions (outlaws vs lawmen vs wildlife).
pub fn ai_system(
    mut ai_query: Query<
        (Entity, &Position, &mut AiState, Option<&mut Viewshed>, &mut Energy, Option<&Faction>, Option<&mut Ammo>, Option<&mut AiLookDir>, Option<&PatrolOrigin>, Option<&mut Inventory>),
        Without<Player>,
    >,
    player_query: Query<(Entity, &Position), With<Player>>,
    // Query all NPCs with position + faction for inter-faction combat targeting
    npc_positions: Query<(Entity, &Position, Option<&Faction>), Without<Player>>,
    game_map: Res<GameMapResource>,
    spatial: Res<SpatialIndex>,
    turn_counter: Res<TurnCounter>,
    blockers: Query<(), With<BlocksMovement>>,
    mut item_kinds: Query<&mut ItemKind>,
    mut move_intents: MessageWriter<MoveIntent>,
    mut ranged_intents: MessageWriter<AiRangedAttackIntent>,
    mut attack_intents: MessageWriter<AttackIntent>,
    mut spell_intents: MessageWriter<SpellCastIntent>,
    mut molotov_intents: MessageWriter<MolotovCastIntent>,
) {
    let player_info = player_query.single().ok();
    let player_vec = player_info.map(|(_, p)| p.as_grid_vec());

    for (entity, pos, mut ai, mut viewshed, mut energy, faction, ammo, mut ai_look_dir, patrol_origin, mut inventory) in &mut ai_query {
        // Only act if enough energy has accumulated.
        if !energy.can_act() {
            continue;
        }

        let my_pos = pos.as_grid_vec();
        let my_faction = faction.copied();

        // Find the nearest hostile faction entity visible to this NPC.
        let faction_target: Option<(Entity, GridVec)> = if let Some(my_f) = my_faction {
            let mut best_dist = i32::MAX;
            let mut best = None;
            for (other_ent, other_pos, other_faction) in &npc_positions {
                if other_ent == entity { continue; }
                if let Some(&of) = other_faction {
                    if factions_are_hostile(my_f, of) {
                        let ov = other_pos.as_grid_vec();
                        let dist = my_pos.chebyshev_distance(ov);
                        if dist < best_dist {
                            // Only target if visible
                            if viewshed.as_ref().is_some_and(|vs| vs.visible_tiles.contains(&ov)) {
                                best_dist = dist;
                                best = Some((other_ent, ov));
                            }
                        }
                    }
                }
            }
            best
        } else {
            None
        };

        // Determine chase target: prefer player if visible, else faction target
        let player_visible = player_vec.is_some_and(|pv|
            viewshed.as_ref().is_some_and(|vs| vs.visible_tiles.contains(&pv))
        );

        let chase_target: Option<(Entity, GridVec)> = if player_visible {
            player_info.map(|(e, p)| (e, p.as_grid_vec()))
        } else {
            faction_target
        };

        match *ai {
            AiState::Idle => {
                // Check if any target is visible — transition to Chasing.
                if chase_target.is_some() {
                    *ai = AiState::Chasing;
                    if let Some((_, tv)) = chase_target {
                        if let Some(ref mut look) = ai_look_dir {
                            let toward = (tv - my_pos).king_step();
                            if !toward.is_zero() {
                                look.0 = toward;
                            }
                        }
                    }
                } else {
                    // Idle: slowly rotate look direction.
                    if let Some(ref mut look) = ai_look_dir {
                        let current_normalized = look.0.king_step();
                        let idx = GridVec::DIRECTIONS_8.iter()
                            .position(|&d| d == current_normalized)
                            .map(|i| (i + 1) % 8)
                            .unwrap_or(0);
                        look.0 = GridVec::DIRECTIONS_8[idx];
                        if let Some(ref mut vs) = viewshed {
                            vs.dirty = true;
                        }
                    }
                    energy.spend_action();
                }
            }
            AiState::Patrolling => {
                // If we spot a target, transition to Chasing.
                if chase_target.is_some() {
                    *ai = AiState::Chasing;
                    continue;
                }

                // Patrol behavior depends on faction.
                let origin = patrol_origin.map(|po| po.0).unwrap_or(my_pos);

                // Natural movement: occasional idle pauses based on position + turn.
                // Using turn counter creates less frequent, more natural pauses.
                let pause_hash = (my_pos.x.wrapping_mul(13) ^ my_pos.y.wrapping_mul(7))
                    .wrapping_add(turn_counter.0 as i32) as u32;
                if pause_hash % 7 == 0 {
                    // Skip this turn (idle pause for natural movement).
                    if let Some(ref mut look) = ai_look_dir {
                        // Slowly look around during pause.
                        let current_normalized = look.0.king_step();
                        let idx = GridVec::DIRECTIONS_8.iter()
                            .position(|&d| d == current_normalized)
                            .map(|i| (i + 1) % 8)
                            .unwrap_or(0);
                        look.0 = GridVec::DIRECTIONS_8[idx];
                        if let Some(ref mut vs) = viewshed {
                            vs.dirty = true;
                        }
                    }
                    energy.spend_action();
                    continue;
                }

                let patrol_step = match my_faction {
                    Some(Faction::Lawmen) => {
                        // Sheriff/Lawmen: structured patrol around origin
                        // Walk toward the origin if too far, else circle around it.
                        if my_pos.chebyshev_distance(origin) > PATROL_RADIUS {
                            Some((origin - my_pos).king_step())
                        } else {
                            // Rotate clockwise around origin
                            let offset = my_pos - origin;
                            let rotated = offset.rotate_45_cw();
                            let target = origin + rotated;
                            Some((target - my_pos).king_step())
                        }
                    }
                    Some(Faction::Wildlife) => {
                        // Animals: random wandering using prime multipliers for
                        // pseudo-random direction based on position and energy.
                        // Occasionally change direction for more natural movement.
                        let dir_seed = energy.0.wrapping_add(my_pos.x.wrapping_mul(7) ^ my_pos.y.wrapping_mul(13));
                        let dir_idx = dir_seed.unsigned_abs() as usize % 8;
                        Some(GridVec::DIRECTIONS_8[dir_idx])
                    }
                    Some(Faction::Outlaws) => {
                        // Outlaws: skulk — stay near origin, move slowly and randomly.
                        // Uses different prime multipliers (3, 11) for varied patterns.
                        if my_pos.chebyshev_distance(origin) > PATROL_RADIUS / 2 {
                            Some((origin - my_pos).king_step())
                        } else {
                            let dir_seed = energy.0.wrapping_add(my_pos.x.wrapping_mul(3) ^ my_pos.y.wrapping_mul(11));
                            let dir_idx = dir_seed.unsigned_abs() as usize % 8;
                            Some(GridVec::DIRECTIONS_8[dir_idx])
                        }
                    }
                    _ => {
                        // Vaqueros and others: wander using prime multipliers (5, 9)
                        // for pseudo-random direction.
                        let dir_seed = energy.0.wrapping_add(my_pos.x.wrapping_mul(5) ^ my_pos.y.wrapping_mul(9));
                        let dir_idx = dir_seed.unsigned_abs() as usize % 8;
                        Some(GridVec::DIRECTIONS_8[dir_idx])
                    }
                };

                if let Some(step) = patrol_step {
                    if !step.is_zero() {
                        let target = my_pos + step;
                        if game_map.0.is_passable(&target)
                            && !spatial.entities_at(&target).iter().any(|&e| e != entity && blockers.contains(e))
                        {
                            move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                            if let Some(ref mut look) = ai_look_dir {
                                look.0 = step.king_step();
                                if let Some(ref mut vs) = viewshed {
                                    vs.dirty = true;
                                }
                            }
                        }
                    }
                }

                energy.spend_action();
            }
            AiState::Chasing => {
                // Determine the target to chase.
                let target_info = if player_visible {
                    player_info.map(|(e, p)| (e, p.as_grid_vec()))
                } else if let Some(ft) = faction_target {
                    Some(ft)
                } else {
                    // Lost sight of all targets — return to patrolling.
                    *ai = if patrol_origin.is_some() { AiState::Patrolling } else { AiState::Idle };
                    energy.spend_action();
                    continue;
                };

                let Some((target_entity, target_vec)) = target_info else {
                    energy.spend_action();
                    continue;
                };

                // If the enemy has an AiLookDir, check if they need to rotate
                // toward the target first (costs a tick).
                let toward_target = (target_vec - my_pos).king_step();
                let needs_rotation = !toward_target.is_zero()
                    && ai_look_dir.as_ref().is_some_and(|look| look.0 != toward_target);

                if needs_rotation {
                    if let Some(ref mut look) = ai_look_dir {
                        look.0 = toward_target;
                        if let Some(ref mut vs) = viewshed {
                            vs.dirty = true;
                        }
                    }
                    energy.spend_action();
                    continue;
                }

                let dist = my_pos.chebyshev_distance(target_vec);

                // Check inventory for throwable items (grenade/molotov) at medium range.
                let mut used_throwable = false;
                if dist >= 3 && dist <= 6 {
                    if let Some(ref mut inv) = inventory {
                        // Find a throwable item in inventory.
                        let throwable_idx = inv.items.iter().position(|&ent| {
                            item_kinds.get(ent).ok().is_some_and(|k|
                                matches!(*k, ItemKind::Grenade { .. } | ItemKind::Molotov { .. })
                            )
                        });
                        if let Some(idx) = throwable_idx {
                            let item_ent = inv.items[idx];
                            if let Ok(kind) = item_kinds.get(item_ent) {
                                match *kind {
                                    ItemKind::Grenade { damage: _, radius } => {
                                        spell_intents.write(SpellCastIntent {
                                            caster: entity,
                                            radius,
                                            target: target_vec,
                                            grenade_index: idx,
                                        });
                                        used_throwable = true;
                                    }
                                    ItemKind::Molotov { damage, radius } => {
                                        molotov_intents.write(MolotovCastIntent {
                                            caster: entity,
                                            radius,
                                            damage,
                                            target: target_vec,
                                            item_index: idx,
                                        });
                                        used_throwable = true;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                if used_throwable {
                    energy.spend_action();
                    continue;
                }

                // Check inventory for a loaded gun to fire. Decrements the gun's
                // loaded rounds directly, keeping inventory-based ammo in sync.
                let mut used_gun = false;
                if dist > 1 && dist <= AI_RANGED_ATTACK_RANGE
                    && viewshed.as_ref().is_some_and(|vs| vs.visible_tiles.contains(&target_vec))
                {
                    if let Some(ref mut inv) = inventory {
                        let gun_ent = inv.items.iter().copied().find(|&ent| {
                            item_kinds.get(ent).ok().is_some_and(|k|
                                matches!(k, ItemKind::Gun { loaded, .. } if *loaded > 0)
                            )
                        });
                        if let Some(gun_entity) = gun_ent {
                            if let Ok(mut kind) = item_kinds.get_mut(gun_entity)
                                && let ItemKind::Gun { ref mut loaded, .. } = *kind {
                                    *loaded -= 1;
                                    ranged_intents.write(AiRangedAttackIntent {
                                        attacker: entity,
                                        target: target_entity,
                                        range: AI_RANGED_ATTACK_RANGE,
                                    });
                                    used_gun = true;
                                }
                        }
                    } else if let Some(mut ammo_pool) = ammo
                        && ammo_pool.spend_one() {
                            // Fallback for NPCs without inventory but with Ammo component.
                            ranged_intents.write(AiRangedAttackIntent {
                                attacker: entity,
                                target: target_entity,
                                range: AI_RANGED_ATTACK_RANGE,
                            });
                            used_gun = true;
                        }
                }
                if used_gun {
                    energy.spend_action();
                    continue;
                }

                // Adjacent to target? Attack directly (faction fight or player).
                if dist == 1 {
                    attack_intents.write(AttackIntent {
                        attacker: entity,
                        target: target_entity,
                    });
                    energy.spend_action();
                    continue;
                }

                // A* pathfinding toward target.
                let step = a_star_first_step(my_pos, target_vec, |pos| {
                    game_map.0.is_passable(&pos)
                        && !spatial
                            .entities_at(&pos)
                            .iter()
                            .any(|&e| e != entity && blockers.contains(e))
                })
                .unwrap_or_else(|| (target_vec - my_pos).king_step());

                if !step.is_zero() {
                    move_intents.write(MoveIntent {
                        entity,
                        dx: step.x,
                        dy: step.y,
                    });
                    if let Some(ref mut look) = ai_look_dir {
                        look.0 = step.king_step();
                        if let Some(ref mut vs) = viewshed {
                            vs.dirty = true;
                        }
                    }
                    energy.spend_action();
                }
            }
        }
    }
}

/// Accumulates energy for all actors each world tick.
///
/// Energy accumulation follows the standard roguelike scheduling formula:
///   energy += speed
///
/// An entity with Speed(100) gains exactly `ACTION_COST` per tick (acts every
/// tick). Speed(50) → acts every 2 ticks. Speed(200) → acts twice per tick
/// (if the system processes multiple actions per tick).
///
/// This is a discrete-event scheduler that provides exact long-run fairness:
///   actions_over_N_ticks = ⌊N × speed / ACTION_COST⌋
pub fn energy_accumulate_system(mut query: Query<(&Speed, &mut Energy)>) {
    for (speed, mut energy) in &mut query {
        energy.accumulate(speed);
    }
}

// ───────────────────────── A* Tests ────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_star_adjacent_returns_direct_step() {
        let start = GridVec::new(5, 5);
        let goal = GridVec::new(6, 5);
        let step = a_star_first_step(start, goal, |_| true);
        assert_eq!(step, Some(GridVec::new(1, 0)));
    }

    #[test]
    fn a_star_straight_line_path() {
        let start = GridVec::new(0, 0);
        let goal = GridVec::new(5, 0);
        let step = a_star_first_step(start, goal, |_| true);
        assert!(step.is_some(), "A* should find a step toward the goal");
        let s = step.unwrap();
        // The step should move closer to the goal in Chebyshev distance.
        let new_pos = start + s;
        assert!(
            new_pos.chebyshev_distance(goal) < start.chebyshev_distance(goal),
            "First step should reduce Chebyshev distance to goal"
        );
    }

    #[test]
    fn a_star_diagonal_path() {
        let start = GridVec::new(0, 0);
        let goal = GridVec::new(3, 3);
        let step = a_star_first_step(start, goal, |_| true);
        assert_eq!(step, Some(GridVec::new(1, 1)));
    }

    #[test]
    fn a_star_navigates_around_wall() {
        // Wall blocks direct east path: tiles (3,2), (3,3), (3,4)
        let start = GridVec::new(2, 3);
        let goal = GridVec::new(5, 3);
        let wall: HashSet<GridVec> = [
            GridVec::new(3, 2),
            GridVec::new(3, 3),
            GridVec::new(3, 4),
        ]
        .into_iter()
        .collect();

        let step = a_star_first_step(start, goal, |pos| !wall.contains(&pos));
        // A* should find a path around the wall (step should not be directly east)
        assert!(step.is_some(), "A* should find a path around the wall");
        let s = step.unwrap();
        // Should not step into the wall
        let next = start + s;
        assert!(!wall.contains(&next), "First step should not be into a wall");
    }

    #[test]
    fn a_star_returns_none_when_unreachable() {
        // Completely surround the goal
        let start = GridVec::new(0, 0);
        let goal = GridVec::new(10, 10);
        let step = a_star_first_step(start, goal, |pos| {
            // Block a complete ring around the goal
            let d = pos.chebyshev_distance(goal);
            d == 0 || d > 1 // Block exactly the ring at distance 1
        });
        // Might return None (blocked) or find a path to adjacent tile
        // that's reachable. The goal itself is walkable but ring at d=1 isn't.
        // With our implementation, the goal is always walkable, so we need
        // the ring to truly block. Since the neighbor check skips non-walkable
        // tiles (except goal), and all d=1 tiles from goal are blocked,
        // no path should be found from (0,0).
        assert!(step.is_none(), "Should return None when goal is surrounded");
    }

    #[test]
    fn a_star_zero_distance_returns_none() {
        let pos = GridVec::new(5, 5);
        let step = a_star_first_step(pos, pos, |_| true);
        assert_eq!(step, None, "No step needed when already at goal");
    }
}
