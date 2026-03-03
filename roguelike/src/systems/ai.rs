use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use bevy::prelude::*;

use crate::components::{AiLookDir, AiMemory, AiPersonality, AiState, BlocksMovement, CombatStats, Energy, Faction, Health, Hostile, Inventory, Item, ItemKind, PatrolOrigin, Player, Position, Speed, Stamina, Viewshed};
use crate::events::{AttackIntent, MeleeWideIntent, MolotovCastIntent, MoveIntent, PickupItemIntent, RangedAttackIntent, SpellCastIntent, ThrowItemIntent, UseItemIntent};
use crate::grid_vec::GridVec;
use crate::resources::{GameMapResource, SpatialIndex, TurnCounter};
use crate::typeenums::Props;

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

/// Public wrapper for `a_star_first_step` to allow integration testing.
/// See [`a_star_first_step`] for full documentation.
pub fn a_star_first_step_pub(
    start: GridVec,
    goal: GridVec,
    is_walkable: impl Fn(GridVec) -> bool,
) -> Option<GridVec> {
    a_star_first_step(start, goal, is_walkable)
}

// ───────────────────────── AI System ───────────────────────────────

/// AI range for soldier ranged attacks.
const AI_RANGED_ATTACK_RANGE: i32 = 15;

/// Returns `true` if `pos` or any neighbor of `pos` contains a dangerous
/// hazard: cactus, fire, or smoke/sand clouds. NPCs avoid walking into or
/// adjacent to these hazards during pathfinding.
fn is_near_danger(pos: GridVec, game_map: &GameMapResource) -> bool {
    // Check the tile itself for fire or sand/smoke clouds.
    if let Some(voxel) = game_map.0.get_voxel_at(&pos) {
        if matches!(voxel.floor, Some(crate::typeenums::Floor::Fire) | Some(crate::typeenums::Floor::SandCloud)) {
            return true;
        }
    }
    // Check all 8 neighbors for cactus or fire.
    for neighbor in pos.all_neighbors() {
        if let Some(voxel) = game_map.0.get_voxel_at(&neighbor) {
            if matches!(voxel.props, Some(Props::Cactus)) {
                return true;
            }
            if matches!(voxel.floor, Some(crate::typeenums::Floor::Fire)) {
                return true;
            }
        }
    }
    false
}

/// Returns `true` if `pos` is walkable for AI pathfinding: the tile is passable
/// on the game map, not occupied by a blocking entity other than `self_entity`,
/// and not near any dangerous hazards (cactus, fire, smoke).
fn is_walkable_for_ai(
    pos: GridVec,
    self_entity: Entity,
    game_map: &GameMapResource,
    spatial: &SpatialIndex,
    blockers: &Query<(), With<BlocksMovement>>,
) -> bool {
    game_map.0.is_passable(&pos)
        && !spatial.entities_at(&pos).iter().any(|&e| e != self_entity && blockers.contains(e))
        && !is_near_danger(pos, game_map)
}

/// Updates the NPC's look direction and marks the viewshed dirty.
/// Used after movement or rotation to ensure FOV is recalculated.
fn update_look_dir(
    dir: GridVec,
    ai_look_dir: &mut Option<Mut<AiLookDir>>,
    viewshed: &mut Option<Mut<Viewshed>>,
) {
    if let Some(look) = ai_look_dir {
        look.0 = dir.king_step();
        if let Some(vs) = viewshed {
            vs.dirty = true;
        }
    }
}

/// Rotates the NPC's look direction one step clockwise through the 8 cardinal
/// and diagonal directions. Marks the viewshed dirty.
fn rotate_look_dir(
    ai_look_dir: &mut Option<Mut<AiLookDir>>,
    viewshed: &mut Option<Mut<Viewshed>>,
) {
    if let Some(look) = ai_look_dir {
        let current_normalized = look.0.king_step();
        let idx = GridVec::DIRECTIONS_8.iter()
            .position(|&d| d == current_normalized)
            .map(|i| (i + 1) % 8)
            .unwrap_or(0);
        look.0 = GridVec::DIRECTIONS_8[idx];
        if let Some(vs) = viewshed {
            vs.dirty = true;
        }
    }
}

/// Checks line-of-sight between two points using Bresenham, ignoring
/// the directional FOV cone.  Returns `true` when no vision-blocking
/// props or sand cloud exists on the path (the endpoints are excluded from the
/// obstruction check so the attacker can fire from / into a doorway).
fn has_clear_line_of_sight(origin: GridVec, target: GridVec, game_map: &GameMapResource, sand_clouds: &HashSet<GridVec>) -> bool {
    let path = origin.bresenham_line(target);
    for &tile in &path[1..] {
        if tile == target {
            return true;
        }
        if sand_clouds.contains(&tile) {
            return false;
        }
        match game_map.0.get_voxel_at(&tile) {
            Some(v) => {
                if matches!(v.floor, Some(crate::typeenums::Floor::SandCloud)) {
                    return false;
                }
                if v.props.as_ref().is_some_and(|f| f.blocks_vision()) {
                    return false;
                }
            }
            None => return false,
        }
    }
    true
}

/// Returns `true` if a friendly (same-faction) entity lies on the Bresenham
/// path between `origin` and `target`. Used to prevent NPCs from shooting
/// through their allies.
fn has_friendly_in_path(
    origin: GridVec,
    target: GridVec,
    my_faction: Option<Faction>,
    self_entity: Entity,
    spatial: &SpatialIndex,
    npc_factions: &Query<(Entity, &Position, Option<&Faction>), Without<Player>>,
) -> bool {
    let Some(my_f) = my_faction else { return false; };
    let path = origin.bresenham_line(target);
    for &tile in &path[1..] {
        if tile == target {
            return false; // reached the target
        }
        for &ent in spatial.entities_at(&tile) {
            if ent == self_entity { continue; }
            if let Ok((_, _, Some(fac))) = npc_factions.get(ent) {
                if !factions_are_hostile(my_f, *fac) {
                    return true; // friendly entity in the path
                }
            }
        }
    }
    false
}

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

/// Dodge probability: chance per turn that an NPC sidesteps nearby explosions.
const DODGE_CHANCE: f64 = 0.20;

/// Patrol radius: how far an NPC will wander from its spawn point.
const PATROL_RADIUS: i32 = 8;

/// HP fraction below which an NPC considers fleeing (if courage is low enough).
const FLEE_HP_THRESHOLD: f64 = 0.3;

/// Number of turns memory persists after losing sight of a target.
const MEMORY_DURATION: u32 = 15;

// ─────────────────────── AI Decision Helpers ───────────────────────

/// Returns `true` if the NPC has any healing item (whiskey) in inventory.
fn has_healing_item(inventory: &Option<Mut<Inventory>>, item_kinds: &Query<&mut ItemKind>) -> bool {
    inventory.as_ref().is_some_and(|inv| {
        inv.items.iter().any(|&ent| {
            item_kinds.get(ent).ok().is_some_and(|k|
                matches!(*k, ItemKind::Whiskey { .. })
            )
        })
    })
}

/// Returns `true` if the NPC has a loaded gun in inventory.
fn has_loaded_gun(inventory: &Option<Mut<Inventory>>, item_kinds: &Query<&mut ItemKind>) -> bool {
    inventory.as_ref().is_some_and(|inv| {
        inv.items.iter().any(|&ent| {
            item_kinds.get(ent).ok().is_some_and(|k|
                matches!(*k, ItemKind::Gun { loaded, .. } if loaded > 0)
            )
        })
    })
}

/// Returns `true` if the NPC has a bow in inventory.
fn has_bow(inventory: &Option<Mut<Inventory>>, item_kinds: &Query<&mut ItemKind>) -> bool {
    inventory.as_ref().is_some_and(|inv| {
        inv.items.iter().any(|&ent| {
            item_kinds.get(ent).ok().is_some_and(|k|
                matches!(*k, ItemKind::Bow { .. })
            )
        })
    })
}

/// Returns `true` if the NPC has a ranged weapon (loaded gun or bow).
fn has_ranged_weapon(inventory: &Option<Mut<Inventory>>, item_kinds: &Query<&mut ItemKind>) -> bool {
    has_loaded_gun(inventory, item_kinds) || has_bow(inventory, item_kinds)
}

/// Returns `true` if the NPC should consider fleeing based on health and personality.
fn should_flee(health: &Option<Mut<Health>>, personality: Option<&AiPersonality>, inventory: &Option<Mut<Inventory>>, item_kinds: &Query<&mut ItemKind>) -> bool {
    let Some(hp) = health else { return false; };
    if hp.fraction() >= FLEE_HP_THRESHOLD { return false; }
    if has_healing_item(inventory, item_kinds) { return false; }
    let courage = personality.map(|p| p.courage).unwrap_or(0.5);
    hp.fraction() < courage * FLEE_HP_THRESHOLD
}

/// Find a direction to flee away from a threat position.
fn flee_direction(
    my_pos: GridVec,
    threat_pos: GridVec,
    entity: Entity,
    game_map: &GameMapResource,
    spatial: &SpatialIndex,
    blockers: &Query<(), With<BlocksMovement>>,
) -> Option<GridVec> {
    let away = (my_pos - threat_pos).king_step();
    if !away.is_zero() {
        let target = my_pos + away;
        if is_walkable_for_ai(target, entity, game_map, spatial, blockers) {
            return Some(away);
        }
    }
    for dir in GridVec::DIRECTIONS_8 {
        let dot_toward_threat = dir.x * (threat_pos.x - my_pos.x).signum()
            + dir.y * (threat_pos.y - my_pos.y).signum();
        if dot_toward_threat <= 0 {
            let target = my_pos + dir;
            if is_walkable_for_ai(target, entity, game_map, spatial, blockers) {
                return Some(dir);
            }
        }
    }
    None
}

/// Find the best direction to maintain preferred range from a target.
fn kite_direction(
    my_pos: GridVec,
    target_pos: GridVec,
    preferred_range: i32,
    entity: Entity,
    game_map: &GameMapResource,
    spatial: &SpatialIndex,
    blockers: &Query<(), With<BlocksMovement>>,
) -> Option<GridVec> {
    let dist = my_pos.chebyshev_distance(target_pos);
    if dist == preferred_range {
        return None;
    }
    if dist < preferred_range {
        flee_direction(my_pos, target_pos, entity, game_map, spatial, blockers)
    } else {
        let toward = (target_pos - my_pos).king_step();
        if !toward.is_zero() {
            let target = my_pos + toward;
            if is_walkable_for_ai(target, entity, game_map, spatial, blockers) {
                return Some(toward);
            }
        }
        a_star_first_step(my_pos, target_pos, |p| {
            is_walkable_for_ai(p, entity, game_map, spatial, blockers)
        })
    }
}

/// AI system: runs during `WorldTurn` for every entity with an `AiState`.
///
/// **Architecture**: NPC AI is a decision layer on top of player capabilities.
/// NPCs emit the same intent events as the player (MoveIntent, RangedAttackIntent,
/// UseItemIntent, PickupItemIntent, etc.) — the action resolution systems are
/// unified for both players and NPCs.
///
/// **Behaviour**:
/// - **Idle**: if the player or hostile faction entity is visible, switch to `Chasing`.
///   Otherwise, slowly rotate look direction and scavenge nearby items.
/// - **Patrolling**: wander around patrol origin. If player or enemy faction
///   entity is spotted, switch to `Chasing`. Investigates remembered positions.
/// - **Chasing**: use A* pathfinding to pursue the target. NPCs use the
///   same combat abilities as the player: fire guns (RangedAttackIntent), throw
///   grenades/molotovs, throw knives, heal with whiskey (UseItemIntent),
///   roundhouse kick (MeleeWideIntent). Ranged NPCs kite to maintain range.
/// - **Fleeing**: retreat from threats when health is critical and no healing
///   items are available.
///
/// **Memory**: NPCs remember the last known position of their target. When
/// sight is lost, they navigate to the remembered position before giving up.
pub fn ai_system(
    mut commands: Commands,
    mut ai_query: Query<
        (Entity, &Position, &mut AiState, Option<&mut Viewshed>, &mut Energy, Option<&Faction>, Option<&mut AiLookDir>, Option<&PatrolOrigin>, Option<&mut Inventory>, Option<&mut Health>, Option<&mut Stamina>, Option<&CombatStats>, Option<&mut AiMemory>, Option<&AiPersonality>),
        Without<Player>,
    >,
    player_query: Query<(Entity, &Position, &Health), With<Player>>,
    npc_positions: Query<(Entity, &Position, Option<&Faction>), Without<Player>>,
    floor_items: Query<(Entity, &Position), With<Item>>,
    hostile_positions: Query<(Entity, &Position), With<Hostile>>,
    game_map: Res<GameMapResource>,
    spatial: Res<SpatialIndex>,
    turn_counter: Res<TurnCounter>,
    blockers: Query<(), With<BlocksMovement>>,
    mut item_kinds: Query<&mut ItemKind>,
    mut move_intents: MessageWriter<MoveIntent>,
    (mut ranged_intents, mut attack_intents, mut spell_intents): (MessageWriter<RangedAttackIntent>, MessageWriter<AttackIntent>, MessageWriter<SpellCastIntent>),
    (mut molotov_intents, mut melee_wide_intents, mut throw_intents): (MessageWriter<MolotovCastIntent>, MessageWriter<MeleeWideIntent>, MessageWriter<ThrowItemIntent>),
    (mut use_item_intents, mut pickup_intents): (MessageWriter<UseItemIntent>, MessageWriter<PickupItemIntent>),
    (dynamic_rng, seed, spell_particles): (Res<crate::resources::DynamicRng>, Res<crate::resources::MapSeed>, Res<crate::resources::SpellParticles>),
) {
    let player_info = player_query.single().ok();
    // When the player is dead, NPCs should no longer target them.
    let player_alive = player_info.as_ref().is_some_and(|(_, _, hp)| !hp.is_dead());
    let player_vec = if player_alive {
        player_info.as_ref().map(|(_, p, _)| p.as_grid_vec())
    } else {
        None
    };

    let sand_cloud_tiles: HashSet<GridVec> = spell_particles.particles.iter()
        .filter(|(_, life, delay, _, _, _)| *delay == 0 && *life > 0)
        .map(|(pos, _, _, _, _, _)| *pos)
        .collect();

    for (entity, pos, mut ai, mut viewshed, mut energy, faction, mut ai_look_dir, patrol_origin, mut inventory, health, mut stamina, combat_stats, mut ai_memory, personality) in &mut ai_query {
        if !energy.can_act() {
            continue;
        }

        let my_pos = pos.as_grid_vec();
        let my_faction = faction.copied();

        // Find the nearest hostile faction entity visible to this NPC.
        // This includes the player if they are hostile (which they always are to Hostile entities).
        let faction_target: Option<(Entity, GridVec)> = if let Some(my_f) = my_faction {
            let mut best_dist = i32::MAX;
            let mut best = None;
            for (other_ent, other_pos, other_faction) in &npc_positions {
                if other_ent == entity { continue; }
                if let Some(&of) = other_faction
                    && factions_are_hostile(my_f, of) {
                        let ov = other_pos.as_grid_vec();
                        let dist = my_pos.chebyshev_distance(ov);
                        if dist < best_dist
                            && viewshed.as_ref().is_some_and(|vs| vs.visible_tiles.contains(&ov))
                        {
                            best_dist = dist;
                            best = Some((other_ent, ov));
                        }
                    }
            }
            best
        } else {
            None
        };

        let player_visible = player_vec.is_some_and(|pv|
            viewshed.as_ref().is_some_and(|vs| vs.visible_tiles.contains(&pv))
        );

        // Target the closest hostile entity — not always the player.
        // If the player is visible, compare distance to player vs nearest faction target.
        // Faction targets are preferred only when strictly closer than the player.
        let chase_target: Option<(Entity, GridVec)> = {
            let player_option = if player_visible {
                player_info.map(|(e, p, _)| (e, p.as_grid_vec()))
            } else {
                None
            };
            match (player_option, faction_target) {
                (Some((pe, pv)), Some((fe, fv))) => {
                    let pd = my_pos.chebyshev_distance(pv);
                    let fd = my_pos.chebyshev_distance(fv);
                    if fd < pd { Some((fe, fv)) } else { Some((pe, pv)) }
                }
                (Some(pt), None) => Some(pt),
                (None, Some(ft)) => Some(ft),
                (None, None) => None,
            }
        };

        // Update memory when target is visible
        if let Some((_, tv)) = chase_target {
            if let Some(ref mut mem) = ai_memory {
                mem.last_known_pos = Some(tv);
                mem.last_seen_turn = turn_counter.0;
            }
        }

        // Find the nearest visible floor item for scavenging.
        let nearest_item: Option<(Entity, GridVec)> = {
            let mut best_dist = i32::MAX;
            let mut best = None;
            for (item_ent, item_pos) in &floor_items {
                let iv = item_pos.as_grid_vec();
                let dist = my_pos.chebyshev_distance(iv);
                if dist < best_dist
                    && viewshed.as_ref().is_some_and(|vs| vs.visible_tiles.contains(&iv))
                    && inventory.as_ref().is_some_and(|inv| inv.items.len() < 9)
                {
                    best_dist = dist;
                    best = Some((item_ent, iv));
                }
            }
            best
        };

        // NPC auto-pickup via unified PickupItemIntent
        if let Some((_, item_vec)) = nearest_item
            && item_vec == my_pos
            && inventory.as_ref().is_some_and(|inv| inv.items.len() < 9)
        {
            pickup_intents.write(PickupItemIntent { picker: entity });
        }

        // NPC Healing via unified UseItemIntent
        let mut healed_this_turn = false;
        if let Some(ref hp) = health
            && hp.fraction() < 0.5
            && let Some(ref inv) = inventory
        {
            let whiskey_idx = inv.items.iter().position(|&ent| {
                item_kinds.get(ent).ok().is_some_and(|k|
                    matches!(*k, ItemKind::Whiskey { .. })
                )
            });
            if let Some(idx) = whiskey_idx {
                use_item_intents.write(UseItemIntent {
                    user: entity,
                    item_index: idx,
                });
                healed_this_turn = true;
            }
        }
        if healed_this_turn {
            energy.spend_action();
            continue;
        }

        // Count adjacent hostile entities (for melee wide decision)
        let adjacent_enemy_count = {
            let mut count = 0;
            for dir in GridVec::DIRECTIONS_8 {
                let neighbor = my_pos + dir;
                let has_enemy = spatial.entities_at(&neighbor).iter().any(|&e| {
                    e != entity && hostile_positions.contains(e)
                });
                let has_player = player_vec.is_some_and(|pv| pv == neighbor);
                if has_enemy || has_player {
                    count += 1;
                }
            }
            count
        };

        // NPC Dodge: sidestep when projectile is nearby
        let dodge_roll = dynamic_rng.roll(seed.0, entity.to_bits() ^ 0xD0D6);
        let nearby_danger = spell_particles.particles.iter().any(|(p, life, delay, _, _, _)| {
            *delay == 0 && *life > 0 && my_pos.chebyshev_distance(*p) <= 2
        });
        if nearby_danger && dodge_roll < DODGE_CHANCE {
            let mut best_dir = None;
            let mut best_dist = 0;
            for dir in GridVec::DIRECTIONS_8 {
                let target = my_pos + dir;
                if is_walkable_for_ai(target, entity, &game_map, &spatial, &blockers) {
                    let min_particle_dist = spell_particles.particles.iter()
                        .filter(|(_, life, delay, _, _, _)| *delay == 0 && *life > 0)
                        .map(|(p, _, _, _, _, _)| target.chebyshev_distance(*p))
                        .min()
                        .unwrap_or(i32::MAX);
                    if min_particle_dist > best_dist {
                        best_dist = min_particle_dist;
                        best_dir = Some(dir);
                    }
                }
            }
            if let Some(dir) = best_dir {
                move_intents.write(MoveIntent { entity, dx: dir.x, dy: dir.y });
                update_look_dir(dir, &mut ai_look_dir, &mut viewshed);
                energy.spend_action();
                continue;
            }
        }

        // Check if NPC should flee
        if should_flee(&health, personality, &inventory, &item_kinds) {
            if !matches!(*ai, AiState::Fleeing) {
                *ai = AiState::Fleeing;
            }
        } else if matches!(*ai, AiState::Fleeing) {
            *ai = if chase_target.is_some() { AiState::Chasing } else { AiState::Patrolling };
        }

        match *ai {
            AiState::Idle => {
                if chase_target.is_some() {
                    *ai = AiState::Chasing;
                    if let Some((_, tv)) = chase_target {
                        let toward = (tv - my_pos).king_step();
                        if !toward.is_zero() {
                            update_look_dir(toward, &mut ai_look_dir, &mut viewshed);
                        }
                    }
                } else if let Some((_, item_vec)) = nearest_item {
                    let step = a_star_first_step(my_pos, item_vec, |p| {
                        is_walkable_for_ai(p, entity, &game_map, &spatial, &blockers)
                    });
                    if let Some(step) = step
                            && !step.is_zero() {
                                move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                            update_look_dir(step, &mut ai_look_dir, &mut viewshed);
                        }
                    energy.spend_action();
                } else if let Some(ref mem) = ai_memory
                    && let Some(remembered_pos) = mem.last_known_pos
                    && turn_counter.0.saturating_sub(mem.last_seen_turn) < MEMORY_DURATION
                    && my_pos != remembered_pos
                {
                    let step = a_star_first_step(my_pos, remembered_pos, |p| {
                        is_walkable_for_ai(p, entity, &game_map, &spatial, &blockers)
                    });
                    if let Some(step) = step
                        && !step.is_zero() {
                            move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                            update_look_dir(step, &mut ai_look_dir, &mut viewshed);
                        }
                    energy.spend_action();
                } else {
                    if let Some(ref mut mem) = ai_memory {
                        if turn_counter.0.saturating_sub(mem.last_seen_turn) >= MEMORY_DURATION {
                            mem.last_known_pos = None;
                        }
                    }
                    rotate_look_dir(&mut ai_look_dir, &mut viewshed);
                    energy.spend_action();
                }
            }
            AiState::Patrolling => {
                if chase_target.is_some() {
                    *ai = AiState::Chasing;
                    continue;
                }

                if let Some((_, item_vec)) = nearest_item {
                    let step = a_star_first_step(my_pos, item_vec, |p| {
                        is_walkable_for_ai(p, entity, &game_map, &spatial, &blockers)
                    });
                    if let Some(step) = step
                            && !step.is_zero() {
                                move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                            update_look_dir(step, &mut ai_look_dir, &mut viewshed);
                        }
                    energy.spend_action();
                    continue;
                }

                // Check memory for remembered positions
                if let Some(ref mem) = ai_memory
                    && let Some(remembered_pos) = mem.last_known_pos
                    && turn_counter.0.saturating_sub(mem.last_seen_turn) < MEMORY_DURATION
                    && my_pos != remembered_pos
                {
                    let step = a_star_first_step(my_pos, remembered_pos, |p| {
                        is_walkable_for_ai(p, entity, &game_map, &spatial, &blockers)
                    });
                    if let Some(step) = step
                            && !step.is_zero() {
                                move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                            update_look_dir(step, &mut ai_look_dir, &mut viewshed);
                        }
                    energy.spend_action();
                    continue;
                }

                if let Some(ref mut mem) = ai_memory {
                    if turn_counter.0.saturating_sub(mem.last_seen_turn) >= MEMORY_DURATION {
                        mem.last_known_pos = None;
                    }
                }

                let origin = patrol_origin.map(|po| po.0).unwrap_or(my_pos);

                let pause_hash = (my_pos.x.wrapping_mul(13) ^ my_pos.y.wrapping_mul(7))
                    .wrapping_add(turn_counter.0 as i32) as u32;
                if pause_hash.is_multiple_of(7) {
                    rotate_look_dir(&mut ai_look_dir, &mut viewshed);
                    energy.spend_action();
                    continue;
                }

                let patrol_step = match my_faction {
                    Some(Faction::Lawmen) => {
                        if my_pos.chebyshev_distance(origin) > PATROL_RADIUS {
                            Some((origin - my_pos).king_step())
                        } else {
                            let offset = my_pos - origin;
                            let rotated = offset.rotate_45_cw();
                            let target = origin + rotated;
                            Some((target - my_pos).king_step())
                        }
                    }
                    Some(Faction::Wildlife) => {
                        let dir_seed = energy.0.wrapping_add(my_pos.x.wrapping_mul(7) ^ my_pos.y.wrapping_mul(13));
                        let dir_idx = dir_seed.unsigned_abs() as usize % 8;
                        Some(GridVec::DIRECTIONS_8[dir_idx])
                    }
                    Some(Faction::Outlaws) => {
                        if my_pos.chebyshev_distance(origin) > PATROL_RADIUS / 2 {
                            Some((origin - my_pos).king_step())
                        } else {
                            let dir_seed = energy.0.wrapping_add(my_pos.x.wrapping_mul(3) ^ my_pos.y.wrapping_mul(11));
                            let dir_idx = dir_seed.unsigned_abs() as usize % 8;
                            Some(GridVec::DIRECTIONS_8[dir_idx])
                        }
                    }
                    _ => {
                        let dir_seed = energy.0.wrapping_add(my_pos.x.wrapping_mul(5) ^ my_pos.y.wrapping_mul(9));
                        let dir_idx = dir_seed.unsigned_abs() as usize % 8;
                        Some(GridVec::DIRECTIONS_8[dir_idx])
                    }
                };

                if let Some(step) = patrol_step
                    && !step.is_zero() {
                        let target = my_pos + step;
                        if game_map.0.is_passable(&target)
                            && !spatial.entities_at(&target).iter().any(|&e| e != entity && blockers.contains(e))
                        {
                            move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                            update_look_dir(step, &mut ai_look_dir, &mut viewshed);
                        }
                    }

                energy.spend_action();
            }
            AiState::Fleeing => {
                let threat_pos = chase_target.map(|(_, tv)| tv)
                    .or_else(|| player_vec);

                if let Some(tp) = threat_pos {
                    if let Some(dir) = flee_direction(my_pos, tp, entity, &game_map, &spatial, &blockers) {
                        move_intents.write(MoveIntent { entity, dx: dir.x, dy: dir.y });
                        update_look_dir(dir, &mut ai_look_dir, &mut viewshed);
                    }
                } else {
                    let origin = patrol_origin.map(|po| po.0).unwrap_or(my_pos);
                    if my_pos != origin {
                        let step = a_star_first_step(my_pos, origin, |p| {
                            is_walkable_for_ai(p, entity, &game_map, &spatial, &blockers)
                        });
                        if let Some(step) = step
                                && !step.is_zero() {
                                    move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                                update_look_dir(step, &mut ai_look_dir, &mut viewshed);
                            }
                    }
                    *ai = if patrol_origin.is_some() { AiState::Patrolling } else { AiState::Idle };
                }
                energy.spend_action();
            }
            AiState::Chasing => {
                // Pick the closest hostile target (player or faction enemy).
                // Only use viewshed-based visibility (which respects walls via
                // shadowcasting) — never raw distance alone. This prevents
                // NPC line-of-sight from bleeding through walls.
                let player_option = if player_visible {
                    player_info.map(|(e, p, _)| (e, p.as_grid_vec()))
                } else {
                    None
                };
                let target_info = match (player_option, faction_target) {
                    (Some((pe, pv)), Some((fe, fv))) => {
                        let pd = my_pos.chebyshev_distance(pv);
                        let fd = my_pos.chebyshev_distance(fv);
                        if fd < pd { Some((fe, fv)) } else { Some((pe, pv)) }
                    }
                    (Some(pt), None) => Some(pt),
                    (None, Some(ft)) => Some(ft),
                    (None, None) => {
                        if let Some(ref mem) = ai_memory
                            && let Some(remembered_pos) = mem.last_known_pos
                            && turn_counter.0.saturating_sub(mem.last_seen_turn) < MEMORY_DURATION
                            && my_pos != remembered_pos
                        {
                            // Memory pursuit: navigate to remembered position.
                            let step = a_star_first_step(my_pos, remembered_pos, |pos| {
                                is_walkable_for_ai(pos, entity, &game_map, &spatial, &blockers)
                            })
                            .unwrap_or_else(|| (remembered_pos - my_pos).king_step());

                            if !step.is_zero() {
                                move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                                update_look_dir(step, &mut ai_look_dir, &mut viewshed);
                            }
                            energy.spend_action();
                            continue;
                        } else {
                            *ai = if patrol_origin.is_some() { AiState::Patrolling } else { AiState::Idle };
                            if let Some(ref mut mem) = ai_memory {
                                mem.last_known_pos = None;
                            }
                            energy.spend_action();
                            continue;
                        }
                    }
                };

                let Some((target_entity, target_vec)) = target_info else {
                    energy.spend_action();
                    continue;
                };

                // Update memory with current target position
                if let Some(ref mut mem) = ai_memory {
                    mem.last_known_pos = Some(target_vec);
                    mem.last_seen_turn = turn_counter.0;
                }

                let toward_target = (target_vec - my_pos).king_step();
                let needs_rotation = !toward_target.is_zero()
                    && ai_look_dir.as_ref().is_some_and(|look| look.0 != toward_target);

                if needs_rotation {
                    update_look_dir(toward_target, &mut ai_look_dir, &mut viewshed);
                    energy.spend_action();
                    continue;
                }

                let dist = my_pos.chebyshev_distance(target_vec);

                // Throwable items (grenade/molotov) at medium range
                let mut used_throwable = false;
                if (3..=6).contains(&dist)
                    && let Some(ref mut inv) = inventory {
                        let throwable_idx = inv.items.iter().position(|&ent| {
                            item_kinds.get(ent).ok().is_some_and(|k|
                                matches!(*k, ItemKind::Grenade { .. } | ItemKind::Molotov { .. })
                            )
                        });
                        if let Some(idx) = throwable_idx {
                            let item_ent = inv.items[idx];
                            if let Ok(kind) = item_kinds.get(item_ent) {
                                match *kind {
                                    ItemKind::Grenade { damage: _, radius, .. } => {
                                        spell_intents.write(SpellCastIntent {
                                            caster: entity,
                                            radius,
                                            target: target_vec,
                                            grenade_index: idx,
                                        });
                                        used_throwable = true;
                                    }
                                    ItemKind::Molotov { damage, radius, .. } => {
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
                if used_throwable {
                    energy.spend_action();
                    continue;
                }

                // Throw knives/tomahawks at medium range
                let mut used_thrown_weapon = false;
                if (2..=8).contains(&dist)
                    && has_clear_line_of_sight(my_pos, target_vec, &game_map, &sand_cloud_tiles)
                    && let Some(ref mut inv) = inventory {
                        let knife_idx = inv.items.iter().position(|&ent| {
                            item_kinds.get(ent).ok().is_some_and(|k|
                                matches!(*k, ItemKind::Knife { .. } | ItemKind::Tomahawk { .. })
                            )
                        });
                        if let Some(idx) = knife_idx {
                            let item_ent = inv.items[idx];
                            if let Ok(kind) = item_kinds.get(item_ent) {
                                let (dmg, range) = match *kind {
                                    ItemKind::Knife { attack, .. } => (attack, 12),
                                    ItemKind::Tomahawk { attack, .. } => (attack, 10),
                                    _ => (0, 0),
                                };
                                if dmg > 0 {
                                    let toward = (target_vec - my_pos).king_step();
                                    throw_intents.write(ThrowItemIntent {
                                        thrower: entity,
                                        item_entity: item_ent,
                                        item_index: idx,
                                        dx: toward.x,
                                        dy: toward.y,
                                        range,
                                        damage: dmg,
                                    });
                                    used_thrown_weapon = true;
                                }
                            }
                        }
                    }
                if used_thrown_weapon {
                    energy.spend_action();
                    continue;
                }

                // Melee wide (roundhouse kick) when surrounded
                if adjacent_enemy_count >= 2
                    && stamina.as_ref().is_some_and(|s| s.current >= 15)
                    && combat_stats.is_some()
                {
                    if let Some(ref mut sta) = stamina {
                        sta.spend(15);
                    }
                    melee_wide_intents.write(MeleeWideIntent {
                        attacker: entity,
                    });
                    energy.spend_action();
                    continue;
                }

                // Sand throw (1% chance)
                let sand_roll = dynamic_rng.roll(seed.0, entity.to_bits() ^ 0x5A4D);
                if sand_roll < 0.01 && (2..=5).contains(&dist) {
                    let toward = (target_vec - my_pos).king_step();
                    if !toward.is_zero() {
                        let sand_center = my_pos + toward * 2;
                        spell_intents.write(SpellCastIntent {
                            caster: entity,
                            radius: 2,
                            target: sand_center,
                            grenade_index: usize::MAX,
                        });
                        energy.spend_action();
                        continue;
                    }
                }

                // Ranged attack: fire guns via unified RangedAttackIntent
                // Skip if a friendly entity is in the line of fire.
                let mut used_gun = false;
                if dist > 1 && dist <= AI_RANGED_ATTACK_RANGE
                    && has_clear_line_of_sight(my_pos, target_vec, &game_map, &sand_cloud_tiles)
                    && !has_friendly_in_path(my_pos, target_vec, my_faction, entity, &spatial, &npc_positions)
                {
                    if let Some(ref mut inv) = inventory {
                        let gun_ent = inv.items.iter().copied().find(|&ent| {
                            item_kinds.get(ent).ok().is_some_and(|k|
                                matches!(k, ItemKind::Gun { loaded, .. } if *loaded > 0)
                            )
                        });
                        if let Some(gun_entity) = gun_ent {
                            let dx = target_vec.x - my_pos.x;
                            let dy = target_vec.y - my_pos.y;
                            ranged_intents.write(RangedAttackIntent {
                                attacker: entity,
                                range: AI_RANGED_ATTACK_RANGE,
                                dx,
                                dy,
                                gun_item: Some(gun_entity),
                            });
                            used_gun = true;
                        } else {
                            let reloadable_gun = inv.items.iter().copied().find(|&ent| {
                                item_kinds.get(ent).ok().is_some_and(|k|
                                    matches!(k, ItemKind::Gun { loaded, capacity, .. } if *loaded < *capacity)
                                )
                            });
                            if let Some(gun_entity) = reloadable_gun
                                && let Ok(mut kind) = item_kinds.get_mut(gun_entity)
                                    && let ItemKind::Gun { ref mut loaded, .. } = *kind {
                                        *loaded += 1;
                                        used_gun = true;
                                    }
                        }
                    }
                }
                if used_gun {
                    energy.spend_action();
                    continue;
                }

                // Bow attack: fire an arrow projectile (unlimited ammo).
                let mut used_bow = false;
                if dist > 1 && dist <= AI_RANGED_ATTACK_RANGE
                    && has_clear_line_of_sight(my_pos, target_vec, &game_map, &sand_cloud_tiles)
                    && !has_friendly_in_path(my_pos, target_vec, my_faction, entity, &spatial, &npc_positions)
                {
                    if let Some(ref inv) = inventory {
                        let bow_ent = inv.items.iter().copied().find(|&ent| {
                            item_kinds.get(ent).ok().is_some_and(|k|
                                matches!(k, ItemKind::Bow { .. })
                            )
                        });
                        if let Some(bow_entity) = bow_ent {
                            if let Ok(kind) = item_kinds.get(bow_entity) {
                                if let ItemKind::Bow { attack: bow_atk, .. } = *kind {
                                    let dx = target_vec.x - my_pos.x;
                                    let dy = target_vec.y - my_pos.y;
                                    let max_comp = dx.abs().max(dy.abs()).max(1);
                                    let scale = AI_RANGED_ATTACK_RANGE.div_euclid(max_comp).max(1);
                                    let endpoint = my_pos + GridVec::new(dx * scale, dy * scale);
                                    crate::systems::projectile::spawn_arrow(
                                        &mut commands,
                                        my_pos,
                                        endpoint,
                                        bow_atk,
                                        entity,
                                    );
                                    used_bow = true;
                                }
                            }
                        }
                    }
                }
                if used_bow {
                    energy.spend_action();
                    continue;
                }

                // Adjacent to target? Melee attack.
                if dist == 1 {
                    attack_intents.write(AttackIntent {
                        attacker: entity,
                        target: target_entity,
                    });
                    energy.spend_action();
                    continue;
                }

                // Tactical range management (kiting) for ranged NPCs
                let pref_range = personality.map(|p| p.preferred_range).unwrap_or(1);
                let is_ranged_npc = pref_range > 1 && has_ranged_weapon(&inventory, &item_kinds);

                if is_ranged_npc && dist < pref_range {
                    if let Some(dir) = kite_direction(my_pos, target_vec, pref_range, entity, &game_map, &spatial, &blockers) {
                        move_intents.write(MoveIntent { entity, dx: dir.x, dy: dir.y });
                        update_look_dir(dir, &mut ai_look_dir, &mut viewshed);
                        energy.spend_action();
                        continue;
                    }
                }

                // A* pathfinding toward target.
                let step = a_star_first_step(my_pos, target_vec, |pos| {
                    is_walkable_for_ai(pos, entity, &game_map, &spatial, &blockers)
                })
                .unwrap_or_else(|| (target_vec - my_pos).king_step());

                if !step.is_zero() {
                    move_intents.write(MoveIntent {
                        entity,
                        dx: step.x,
                        dy: step.y,
                    });
                    update_look_dir(step, &mut ai_look_dir, &mut viewshed);
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

/// When a group leader dies, their followers become more erratic and cowardly.
/// Reduces courage to 0.1 and changes state to Fleeing for leaderless followers.
pub fn leader_death_system(
    mut commands: Commands,
    mut followers: Query<(Entity, &crate::components::GroupFollower, &mut AiPersonality, &mut AiState)>,
    leaders: Query<Entity, bevy::prelude::With<crate::components::GroupLeader>>,
) {
    /// Aggression multiplier when the group leader dies (50% reduction).
    const LEADERLESS_AGGRESSION_MULTIPLIER: f64 = 0.5;

    for (entity, follower, mut personality, mut ai_state) in &mut followers {
        // Check if the leader entity still exists
        if leaders.get(follower.leader).is_err() {
            // Leader is dead — reduce courage and become erratic
            personality.courage = 0.1;
            personality.aggression *= LEADERLESS_AGGRESSION_MULTIPLIER;
            *ai_state = AiState::Fleeing;
            commands.entity(entity).remove::<crate::components::GroupFollower>();
        }
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
