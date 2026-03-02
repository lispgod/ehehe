use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use bevy::prelude::*;

use crate::components::{AiLookDir, AiState, Ammo, BlocksMovement, Energy, Faction, Player, Position, Speed, Viewshed};
use crate::events::{AiRangedAttackIntent, MoveIntent};
use crate::grid_vec::GridVec;
use crate::resources::{GameMapResource, SpatialIndex};

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

/// AI system: runs during `WorldTurn` for every entity with an `AiState`.
///
/// **Behaviour**:
/// - **Idle**: if the player is within the entity's Viewshed, switch to `Chasing`.
/// - **Chasing**: use **A\* pathfinding** (Chebyshev heuristic) to find the
///   optimal route to the player, navigating around walls and other blocking
///   entities. Falls back to greedy best-first (king-step toward player) when
///   A* cannot find a path within its exploration budget.
///   Military faction entities with ammo will attempt ranged attacks when they
///   can see the player but are not adjacent.
///
/// Emits `MoveIntent` just like the player's input system, so the same
/// movement/collision/bump-to-attack pipeline resolves NPC actions. This is
/// the core ECS composability guarantee: AI and player share identical
/// intent→action→consequence data flow.
pub fn ai_system(
    mut ai_query: Query<
        (Entity, &Position, &mut AiState, Option<&mut Viewshed>, &mut Energy, Option<&Faction>, Option<&mut Ammo>, Option<&mut AiLookDir>),
        Without<Player>,
    >,
    player_query: Query<(Entity, &Position), With<Player>>,
    game_map: Res<GameMapResource>,
    spatial: Res<SpatialIndex>,
    blockers: Query<(), With<BlocksMovement>>,
    mut move_intents: MessageWriter<MoveIntent>,
    mut ranged_intents: MessageWriter<AiRangedAttackIntent>,
) {
    let Ok((player_entity, player_pos)) = player_query.single() else {
        return;
    };
    let player_vec = player_pos.as_grid_vec();

    for (entity, pos, mut ai, mut viewshed, mut energy, faction, ammo, mut ai_look_dir) in &mut ai_query {
        // Only act if enough energy has accumulated.
        if !energy.can_act() {
            continue;
        }

        let my_pos = pos.as_grid_vec();

        match *ai {
            AiState::Idle => {
                // Check if player is visible — no energy cost for looking.
                // Idle enemies periodically scan by rotating their look direction.
                let player_visible = viewshed.as_ref()
                    .is_some_and(|vs| vs.visible_tiles.contains(&player_vec));
                if player_visible {
                    *ai = AiState::Chasing;
                    // Point look direction at the player immediately.
                    if let Some(ref mut look) = ai_look_dir {
                        let toward = (player_vec - my_pos).king_step();
                        if !toward.is_zero() {
                            look.0 = toward;
                        }
                    }
                } else {
                    // Idle enemies slowly scan: rotate look direction each tick.
                    // This costs energy so they don't act for free forever.
                    if let Some(ref mut look) = ai_look_dir {
                        // Rotate 45° clockwise by cycling through DIRECTIONS_8.
                        // If current direction isn't in the array, normalize via king_step first.
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
            AiState::Chasing => {
                // If the enemy has an AiLookDir, check if they need to rotate
                // toward the player first (costs a tick).
                let toward_player = (player_vec - my_pos).king_step();
                let needs_rotation = !toward_player.is_zero()
                    && ai_look_dir.as_ref().is_some_and(|look| look.0 != toward_player);

                if needs_rotation {
                    if let Some(ref mut look) = ai_look_dir {
                        look.0 = toward_player;
                        if let Some(ref mut vs) = viewshed {
                            vs.dirty = true;
                        }
                    }
                    energy.spend_action();
                    continue;
                }

                let dist = my_pos.chebyshev_distance(player_vec);

                // Military faction entities attempt ranged attacks when they can see
                // the player, have ammo, and are not adjacent.
                let is_military = faction.is_some_and(|f| *f == Faction::Lawmen);
                let can_shoot = is_military
                    && ammo.is_some()
                    && dist > 1
                    && dist <= AI_RANGED_ATTACK_RANGE
                    && viewshed.as_ref().is_some_and(|vs| vs.visible_tiles.contains(&player_vec));

                if can_shoot
                    && let Some(mut ammo_pool) = ammo
                        && ammo_pool.spend_one() {
                            ranged_intents.write(AiRangedAttackIntent {
                                attacker: entity,
                                target: player_entity,
                                range: AI_RANGED_ATTACK_RANGE,
                            });
                            energy.spend_action();
                            continue;
                        }

                // A* pathfinding: find optimal route around obstacles.
                // Falls back to greedy king-step if no path is found.
                let step = a_star_first_step(my_pos, player_vec, |pos| {
                    game_map.0.is_passable(&pos)
                        && !spatial
                            .entities_at(&pos)
                            .iter()
                            .any(|&e| e != entity && blockers.contains(e))
                })
                .unwrap_or_else(|| (player_vec - my_pos).king_step());

                if !step.is_zero() {
                    move_intents.write(MoveIntent {
                        entity,
                        dx: step.x,
                        dy: step.y,
                    });
                    // Update look direction to match movement.
                    if let Some(ref mut look) = ai_look_dir {
                        look.0 = step.king_step();
                        if let Some(ref mut vs) = viewshed {
                            vs.dirty = true;
                        }
                    }
                    // Only deduct energy when an action is actually emitted.
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
