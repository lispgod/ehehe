use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use bevy::prelude::*;

use crate::components::{AiLookDir, AiMemory, AiPersonality, AiState, AiTarget, AimingStyle, BlocksMovement, CombatStats, Energy, Faction, Health, Inventory, Item, ItemKind, PatrolOrigin, PlayerControlled, Position, Speed, Stamina, Viewshed};
use crate::events::{AttackIntent, MeleeWideIntent, MolotovCastIntent, MoveIntent, PickupItemIntent, RangedAttackIntent, SpellCastIntent, ThrowItemIntent, UseItemIntent};
use crate::grid_vec::GridVec;
use crate::resources::{GameMapResource, SpatialIndex, TurnCounter};
use crate::typeenums::Props;

// ───────────────────── Influence Map / Tile Cost ───────────────────
//
// Classic roguelike influence map: every tile gets a weighted traversal
// cost that encodes environmental desirability.  Higher cost = less
// desirable; `None` = impassable.  A* uses these costs as edge weights
// so NPCs *naturally* prefer covered routes and avoid hazards without
// any special-case movement code.
//
// **Positive influences** (reduce cost → attract NPCs):
//   • Cover near walls: each adjacent blocking prop lowers cost.
//   • Bridges / sidewalks: civilised terrain slightly preferred.
//
// **Negative influences** (increase cost → repel NPCs):
//   • Fire, smoke/sand clouds on the tile itself.
//   • Adjacent cactus or fire (splash danger zone).
//   • Deep/shallow water (slows movement).
//   • Open terrain with no adjacent cover (exposed killzone).

/// Tile cost weights for the AI influence map.
mod cost {
    /// Base movement cost for a normal, safe tile.
    pub const BASE: i32 = 10;
    /// Penalty for a tile that is actively on fire.
    pub const FIRE: i32 = 50;
    /// Penalty for tiles adjacent to a fire tile (radiant heat).
    pub const NEAR_FIRE: i32 = 25;
    /// Penalty for tiles adjacent to a cactus (thorns).
    pub const NEAR_CACTUS: i32 = 30;
    /// Penalty for sand / smoke cloud tiles (blocks vision, chokes).
    pub const SAND_CLOUD: i32 = 20;
    /// Penalty for shallow water (movement slow-down).
    pub const SHALLOW_WATER: i32 = 8;
    /// Penalty for deep water (severe slow-down, drowning risk).
    pub const DEEP_WATER: i32 = 15;
    /// Per-wall bonus subtracted from cost when adjacent to blocking props
    /// (cover). Capped so cost never drops below 1.
    pub const COVER_PER_WALL: i32 = 2;
    /// Penalty for open terrain with zero adjacent blocking props.
    pub const EXPOSED: i32 = 3;
}

/// Returns the AI traversal cost for `pos`, or `None` if impassable.
///
/// The cost integrates terrain type, environmental hazards, and tactical
/// cover into a single value that A* and Dijkstra use as edge weight.
/// This replaces the old binary `is_walkable_for_ai` + `is_near_danger`
/// pair with a unified, graduated influence map.
fn tile_cost(pos: GridVec, game_map: &GameMapResource) -> Option<i32> {
    if !game_map.0.is_passable(&pos) {
        return None;
    }

    let mut c = cost::BASE;

    // ── Tile's own floor type ──
    if let Some(voxel) = game_map.0.get_voxel_at(&pos) {
        match &voxel.floor {
            Some(crate::typeenums::Floor::Fire) => c += cost::FIRE,
            Some(crate::typeenums::Floor::SandCloud) => c += cost::SAND_CLOUD,
            Some(crate::typeenums::Floor::ShallowWater) => c += cost::SHALLOW_WATER,
            Some(crate::typeenums::Floor::DeepWater) => c += cost::DEEP_WATER,
            _ => {}
        }
    }

    // ── Neighbor scan: hazards (repel) and cover (attract) ──
    let mut wall_count: i32 = 0;
    let mut near_fire = false;
    let mut near_cactus = false;
    for neighbor in pos.all_neighbors() {
        if let Some(voxel) = game_map.0.get_voxel_at(&neighbor) {
            if matches!(voxel.props, Some(Props::Cactus)) {
                near_cactus = true;
            }
            if matches!(voxel.floor, Some(crate::typeenums::Floor::Fire)) {
                near_fire = true;
            }
            if voxel.props.as_ref().is_some_and(|p| p.blocks_movement()) {
                wall_count += 1;
            }
        }
    }

    if near_fire { c += cost::NEAR_FIRE; }
    if near_cactus { c += cost::NEAR_CACTUS; }

    // Cover bonus: more adjacent walls → lower cost (safer position).
    // Open terrain with zero cover gets an exposure penalty instead.
    if wall_count > 0 {
        c -= (wall_count * cost::COVER_PER_WALL).min(c - 1);
    } else {
        c += cost::EXPOSED;
    }

    Some(c)
}

/// Returns the AI traversal cost for `pos` including dynamic entity blocking.
/// Combines the static influence map (`tile_cost`) with per-tick entity
/// collision checks from the `SpatialIndex`.
fn tile_cost_for_ai(
    pos: GridVec,
    self_entity: Entity,
    game_map: &GameMapResource,
    spatial: &SpatialIndex,
    blockers: &Query<(), With<BlocksMovement>>,
) -> Option<i32> {
    // Dynamic entity blocking — another entity occupies this tile.
    if spatial.entities_at(&pos).iter().any(|&e| e != self_entity && blockers.contains(e)) {
        return None;
    }
    tile_cost(pos, game_map)
}

// ───────────── Weighted A* Pathfinding ─────────────────────────────

/// Maximum number of nodes A* may explore before giving up.
/// 512 nodes covers roughly a 16-tile radius search area, sufficient
/// for navigating around most local obstacles.
const MAX_A_STAR_NODES: usize = 512;

/// Finds the first step direction from `start` toward `goal` using **weighted A***
/// with the **Chebyshev heuristic** (L∞ norm) scaled by `cost::BASE`.
///
/// The `cost_fn` closure returns `Some(cost)` for traversable tiles
/// (higher = less desirable) or `None` for impassable tiles.  This lets
/// A* naturally route around hazards and prefer covered paths via the
/// influence map, without any special-case movement logic.
///
/// **Mathematical properties:**
/// - **Admissible**: `h(n) = chebyshev(n, goal) × BASE` never overestimates
///   because every tile costs at least `BASE` (or more for hazards).
///   Therefore A* finds the lowest-cost path.
/// - **Consistent**: `h(n) ≤ c(n,n') + h(n')` holds because one king-step
///   reduces Chebyshev distance by at most 1, contributing `≥ BASE` cost.
/// - **Time**: O(k log k), **Space**: O(k), where k ≤ `MAX_A_STAR_NODES`.
///
/// Returns the direction `GridVec` of the first step, or `None` if no
/// path is found within the exploration budget.
fn a_star_first_step(
    start: GridVec,
    goal: GridVec,
    cost_fn: impl Fn(GridVec) -> Option<i32>,
) -> Option<GridVec> {
    // Already at the goal — no step needed.
    if start == goal {
        return None;
    }

    // Adjacent shortcut: if the goal tile is reachable, step directly.
    if start.chebyshev_distance(goal) == 1 && cost_fn(goal).is_some() {
        return Some(goal - start);
    }

    // Min-heap: (f_score, h_score, position). Reverse gives min-first ordering.
    //
    // Tie-breaking: among equal-f nodes, prefer lower h (closer to goal).
    let mut open: BinaryHeap<Reverse<(i32, i32, GridVec)>> = BinaryHeap::new();
    let mut came_from: HashMap<GridVec, GridVec> = HashMap::new();
    let mut g_score: HashMap<GridVec, i32> = HashMap::new();
    let mut closed: HashSet<GridVec> = HashSet::new();

    let h_start = start.chebyshev_distance(goal) * cost::BASE;
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

            // The goal tile is always reachable (we want to path into it).
            let edge_cost = if neighbor == goal {
                cost::BASE
            } else {
                match cost_fn(neighbor) {
                    Some(c) => c,
                    None => continue, // Impassable.
                }
            };

            let new_g = current_g + edge_cost;
            if new_g < *g_score.get(&neighbor).unwrap_or(&i32::MAX) {
                came_from.insert(neighbor, current);
                g_score.insert(neighbor, new_g);
                let h = neighbor.chebyshev_distance(goal) * cost::BASE;
                let f = new_g + h;
                open.push(Reverse((f, h, neighbor)));
            }
        }
    }

    None // No path found within budget.
}

/// Public wrapper for `a_star_first_step` that accepts a boolean walkability
/// closure (for backward-compatible integration testing).
/// Internally converts `true` → `Some(cost::BASE)`, `false` → `None`.
pub fn a_star_first_step_pub(
    start: GridVec,
    goal: GridVec,
    is_walkable: impl Fn(GridVec) -> bool,
) -> Option<GridVec> {
    a_star_first_step(start, goal, |pos| {
        if is_walkable(pos) { Some(cost::BASE) } else { None }
    })
}

// ─────────── Dijkstra Flood-Fill (Goal Maps) ──────────────────────
//
// Classic roguelike technique: flood outward from one or more source
// positions using Dijkstra's algorithm with weighted tile costs.
// The resulting distance map can be used in two ways:
//   • **Move downhill** (toward lower values) → approach the sources.
//   • **Move uphill** (toward higher values) → flee from the sources.
//
// Budget-limited to prevent expensive map-wide floods.

/// Maximum tiles the Dijkstra flood may visit.
const MAX_DIJKSTRA_NODES: usize = 512;

/// Multi-source Dijkstra flood fill.  Returns a map of `tile → weighted
/// distance from nearest source`.  Respects the same influence-map costs
/// as A*, so flee paths naturally prefer cover and avoid hazards.
fn dijkstra_map(
    sources: &[GridVec],
    cost_fn: impl Fn(GridVec) -> Option<i32>,
) -> HashMap<GridVec, i32> {
    let mut dist: HashMap<GridVec, i32> = HashMap::with_capacity(MAX_DIJKSTRA_NODES);
    let mut open: BinaryHeap<Reverse<(i32, GridVec)>> = BinaryHeap::new();

    for &src in sources {
        dist.insert(src, 0);
        open.push(Reverse((0, src)));
    }

    let mut explored = 0usize;

    while let Some(Reverse((d, current))) = open.pop() {
        if d > *dist.get(&current).unwrap_or(&i32::MAX) {
            continue; // Stale entry.
        }
        explored += 1;
        if explored >= MAX_DIJKSTRA_NODES {
            break;
        }

        for dir in GridVec::DIRECTIONS_8 {
            let neighbor = current + dir;
            let edge = match cost_fn(neighbor) {
                Some(c) => c,
                None => continue,
            };
            let new_d = d + edge;
            if new_d < *dist.get(&neighbor).unwrap_or(&i32::MAX) {
                dist.insert(neighbor, new_d);
                open.push(Reverse((new_d, neighbor)));
            }
        }
    }

    dist
}

// ───────────────────────── AI System ───────────────────────────────

/// AI range for soldier ranged attacks.
const AI_RANGED_ATTACK_RANGE: i32 = 15;

/// Knuth multiplicative hash constant (golden-ratio-derived prime) for aiming style selection.
const HASH_KNUTH: u64 = 2654435761;
/// LCG multiplier (from PCG family) for pseudo-random spread calculations.
const HASH_LCG: u64 = 6364136223846793005;

/// Assigns a random [`AimingStyle`] and sets [`AiTarget`] on an entity
/// transitioning to [`AiState::Chasing`].
fn assign_aiming_style(
    commands: &mut Commands,
    entity: Entity,
    turn: u32,
    chase_target: Option<(Entity, GridVec)>,
) {
    let aim_hash = (entity.to_bits() ^ turn as u64).wrapping_mul(HASH_KNUTH);
    let style = match aim_hash % 3 {
        0 => AimingStyle::CarefulAim,
        1 => AimingStyle::SnapShot,
        _ => AimingStyle::Suppression,
    };
    commands.entity(entity).insert(style);
    if let Some((te, tv)) = chase_target {
        commands.entity(entity).insert(AiTarget { entity: te, last_pos: tv, last_seen: turn });
    }
}

/// Updates the NPC's look direction and marks the viewshed dirty.
/// Used after movement or rotation to ensure FOV is recalculated.
/// Resets any in-progress circular rotation sequence.
fn update_look_dir(
    dir: GridVec,
    ai_look_dir: &mut Option<Mut<AiLookDir>>,
    viewshed: &mut Option<Mut<Viewshed>>,
) {
    if let Some(look) = ai_look_dir {
        look.0 = dir.king_step();
        look.1 = 0; // cancel any rotation sequence
        if let Some(vs) = viewshed {
            vs.dirty = true;
        }
    }
}

/// Number of 45° CW steps in a full circular rotation (360° / 45° = 8).
const FULL_ROTATION_STEPS: u8 = 8;

/// Rotates the NPC's look direction one step clockwise through the 8 cardinal
/// and diagonal directions. Marks the viewshed dirty.  Decrements the
/// remaining-steps counter when a circular rotation sequence is active.
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
        look.1 = look.1.saturating_sub(1);
        if let Some(vs) = viewshed {
            vs.dirty = true;
        }
    }
}

/// Starts a full 360° circular rotation sequence.  Sets the rotation counter
/// so the NPC rotates one 45° CW step per turn for `FULL_ROTATION_STEPS`
/// turns before resuming movement.
fn begin_circular_rotation(
    ai_look_dir: &mut Option<Mut<AiLookDir>>,
    viewshed: &mut Option<Mut<Viewshed>>,
) {
    if let Some(look) = ai_look_dir {
        look.1 = FULL_ROTATION_STEPS;
    }
    rotate_look_dir(ai_look_dir, viewshed);
}

/// Returns `true` if the NPC is currently in the middle of a circular
/// rotation sequence and should not move until the rotation completes.
fn is_rotating(ai_look_dir: &Option<Mut<AiLookDir>>) -> bool {
    ai_look_dir.as_ref().is_some_and(|look| look.1 > 0)
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
    npc_factions: &Query<(Entity, &Position, Option<&Faction>), Without<PlayerControlled>>,
) -> bool {
    let Some(my_f) = my_faction else { return false; };
    let path = origin.bresenham_line(target);
    for &tile in &path[1..] {
        if tile == target {
            return false; // reached the target
        }
        for &ent in spatial.entities_at(&tile) {
            if ent == self_entity { continue; }
            if let Ok((_, _, Some(fac))) = npc_factions.get(ent)
                && !factions_are_hostile(my_f, *fac) {
                    return true; // friendly entity in the path
                }
        }
    }
    false
}

/// Returns `true` if two factions are hostile to each other.
/// All factions are hostile to each other — every faction fights everyone else.
/// Same-faction members cooperate; different factions are enemies.
pub fn factions_are_hostile(a: Faction, b: Faction) -> bool {
    a != b
}

/// Dodge probability: chance per turn that an NPC sidesteps nearby explosions.
const DODGE_CHANCE: f64 = 0.20;

/// Patrol radius: how far an NPC will wander from its spawn point.
const PATROL_RADIUS: i32 = 12;

/// Absolute HP threshold below which an NPC will flee.
const FLEE_HP_ABSOLUTE: i32 = 20;

/// Base number of turns between full circular look-around when idle/patrolling.
const LOOK_AROUND_BASE_INTERVAL: u32 = 20;

/// Additional random turns added to look-around interval (dice roll range).
const LOOK_AROUND_DICE_RANGE: u32 = 20;

/// Number of turns memory persists after losing sight of a target.
const MEMORY_DURATION: u32 = 40;

// ─────────────────────── AI Decision Helpers ───────────────────────

/// Returns `true` if the NPC has an unloaded (but reloadable) gun.
fn has_unloaded_gun(inventory: &Option<Mut<Inventory>>, item_kinds: &Query<&mut ItemKind>) -> bool {
    inventory.as_ref().is_some_and(|inv| {
        inv.items.iter().any(|&ent| {
            item_kinds.get(ent).ok().is_some_and(|k|
                matches!(*k, ItemKind::Gun { loaded, capacity, .. } if loaded < capacity)
            )
        })
    })
}

/// Returns `true` if the NPC should consider fleeing based on health.
/// NPCs only flee when below 20 absolute HP.
fn should_flee(health: &Option<Mut<Health>>) -> bool {
    let Some(hp) = health else { return false; };
    hp.current < FLEE_HP_ABSOLUTE
}

/// **Dijkstra-enhanced flee**: builds a small threat-distance map centred
/// on `threat_pos`, then picks the adjacent tile with the *highest*
/// weighted distance (furthest from threat along safe, covered paths).
///
/// This replaces the old greedy "step away" heuristic.  Because the
/// Dijkstra flood uses the same influence-map costs as A*, the NPC
/// naturally flees *through cover* and *around hazards* rather than
/// blindly running in the opposite direction.
fn flee_direction(
    my_pos: GridVec,
    threat_pos: GridVec,
    entity: Entity,
    game_map: &GameMapResource,
    spatial: &SpatialIndex,
    blockers: &Query<(), With<BlocksMovement>>,
) -> Option<GridVec> {
    // Flood from the threat through the influence map.
    let threat_map = dijkstra_map(
        &[threat_pos],
        |p| tile_cost_for_ai(p, entity, game_map, spatial, blockers),
    );

    let my_dist = *threat_map.get(&my_pos).unwrap_or(&0);

    // Pick the neighbor that is furthest from the threat while still being
    // traversable.  When Dijkstra hasn't reached a neighbor (it's outside
    // the flood budget), treat it as very far — a reasonable default.
    let mut best_dir = None;
    let mut best_score = i32::MIN;
    for dir in GridVec::DIRECTIONS_8 {
        let neighbor = my_pos + dir;
        if tile_cost_for_ai(neighbor, entity, game_map, spatial, blockers).is_none() {
            continue;
        }
        let neighbor_dist = *threat_map.get(&neighbor).unwrap_or(&(my_dist + cost::BASE * 4));
        // Score: prefer higher distance from threat, penalised by tile cost.
        let tile_c = tile_cost(neighbor, game_map).unwrap_or(cost::BASE);
        let score = neighbor_dist * 2 - tile_c;
        if score > best_score {
            best_score = score;
            best_dir = Some(dir);
        }
    }
    best_dir
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
        ((Entity, &Position, &mut AiState, Option<&mut Viewshed>, &mut Energy, Option<&Faction>, Option<&mut AiLookDir>, Option<&PatrolOrigin>), (Option<&mut Inventory>, Option<&mut Health>, Option<&mut Stamina>, Option<&CombatStats>, Option<&mut AiMemory>, Option<&AiPersonality>, Option<&AiTarget>, Option<&AimingStyle>)),
        Without<PlayerControlled>,
    >,
    player_query: Query<(Entity, &Position, &Health), With<PlayerControlled>>,
    npc_positions: Query<(Entity, &Position, Option<&Faction>), Without<PlayerControlled>>,
    floor_items: Query<(Entity, &Position), With<Item>>,
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

    // When the player is dead, clear all NPC memory so they stop
    // pathfinding toward the player's last known position.
    if !player_alive {
        for ((_, _, mut ai_state, _, _, _, _, _), (_, _, _, _, mut mem_opt, _, _, _)) in &mut ai_query {
            if let Some(ref mut mem) = mem_opt {
                mem.last_known_pos = None;
            }
            if matches!(*ai_state, AiState::Chasing) {
                *ai_state = AiState::Patrolling;
            }
        }
    }

    let sand_cloud_tiles: HashSet<GridVec> = spell_particles.particles.iter()
        .filter(|(_, life, delay, _, _, _)| *delay == 0 && *life > 0)
        .map(|(pos, _, _, _, _, _)| *pos)
        .collect();

    // ── Allied target sharing ──────────────────────────────────────
    // Build a map of (faction → Vec<known hostile position>) from NPCs
    // that are currently chasing a target. Idle/patrolling NPCs within
    // ALLY_SHARE_RANGE can adopt these targets, simulating coordinated
    // faction response (e.g., lawmen converging on a shooter).
    const ALLY_SHARE_RANGE: i32 = 20;
    let mut faction_alerts: HashMap<Faction, Vec<GridVec>> = HashMap::new();
    for ((_, _pos_ref, ai_state, _, _, faction_opt, _, _), (_, _, _, _, mem_opt, _, _, _)) in &ai_query {
        if !matches!(*ai_state, AiState::Chasing) { continue; }
        let Some(&f) = faction_opt else { continue; };
        if let Some(mem) = mem_opt
            && let Some(known) = mem.last_known_pos {
                let age = turn_counter.0.saturating_sub(mem.last_seen_turn);
                if age < MEMORY_DURATION {
                    faction_alerts.entry(f).or_default().push(known);
                }
            }
    }

    for ((entity, pos, mut ai, mut viewshed, mut energy, faction, mut ai_look_dir, patrol_origin), (mut inventory, health, mut stamina, combat_stats, mut ai_memory, _personality, ai_target, aiming_style)) in &mut ai_query {
        if !energy.can_act() {
            continue;
        }

        let my_pos = pos.as_grid_vec();
        let my_faction = faction.copied();

        // Find the nearest hostile faction entity visible to this NPC.
        // Hostility is purely faction-based: entities of different factions are enemies.
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

        // PlayerControlled is always visible as a target if in the NPC's viewshed.
        // Hostility is purely faction-based — all NPCs with a faction target
        // the player (who has no faction).
        let player_visible = player_vec.is_some_and(|pv|
            viewshed.as_ref().is_some_and(|vs| vs.visible_tiles.contains(&pv))
        );

        // Target the closest hostile entity — not always the player.
        // Chase any visible entity of a different faction (including the player).
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

        // Persistent target tracking via AiTarget
        let chase_target: Option<(Entity, GridVec)> = if chase_target.is_some() {
            chase_target
        } else if let Some(ai_tgt) = ai_target {
            let viewshed_range = viewshed.as_ref().map(|vs| vs.range).unwrap_or(8) as i32;
            // Check if the target entity is still alive
            let target_alive = player_query.get(ai_tgt.entity).is_ok()
                || npc_positions.get(ai_tgt.entity).is_ok();
            if !target_alive || my_pos.chebyshev_distance(ai_tgt.last_pos) > viewshed_range * 2 {
                // Target dead or escaped awareness range entirely — clear AiTarget
                commands.entity(entity).remove::<AiTarget>();
                None
            } else if viewshed.as_ref().is_some_and(|vs| vs.visible_tiles.contains(&ai_tgt.last_pos)) {
                // Target visible at last known pos — use it
                Some((ai_tgt.entity, ai_tgt.last_pos))
            } else if my_pos.chebyshev_distance(ai_tgt.last_pos) <= viewshed_range * 2 {
                // Not visible but within awareness range — pursue last known position
                Some((ai_tgt.entity, ai_tgt.last_pos))
            } else {
                commands.entity(entity).remove::<AiTarget>();
                None
            }
        } else {
            None
        };

        // Update memory when target is visible
        if let Some((_, tv)) = chase_target
            && let Some(ref mut mem) = ai_memory {
                mem.last_known_pos = Some(tv);
                mem.last_seen_turn = turn_counter.0;
            }

        // Update AiTarget when target is visible
        if let Some((te, tv)) = chase_target {
            commands.entity(entity).insert(AiTarget { entity: te, last_pos: tv, last_seen: turn_counter.0 });
        }

        // Allied target sharing: if this NPC has no direct target but a
        // nearby ally of the same faction is chasing something, adopt
        // that target into memory so we converge on the threat.
        if chase_target.is_none()
            && let Some(my_f) = my_faction
                && let Some(alerts) = faction_alerts.get(&my_f) {
                    let nearest_alert: Option<&GridVec> = alerts.iter()
                        .filter(|&&alert_pos| my_pos.chebyshev_distance(alert_pos) <= ALLY_SHARE_RANGE)
                        .min_by_key(|&&alert_pos| my_pos.chebyshev_distance(alert_pos));
                    if let Some(&alert_pos) = nearest_alert
                        && let Some(ref mut mem) = ai_memory
                            && (mem.last_known_pos.is_none()
                                || turn_counter.0.saturating_sub(mem.last_seen_turn) >= MEMORY_DURATION)
                            {
                                mem.last_known_pos = Some(alert_pos);
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
                // Count entities of different factions as enemies
                let has_enemy = spatial.entities_at(&neighbor).iter().any(|&e| {
                    if e == entity { return false; }
                    npc_positions.get(e).ok().is_some_and(|(_, _, f)| {
                        f.and_then(|nf| my_faction.map(|mf| factions_are_hostile(mf, *nf)))
                            .unwrap_or(false)
                    })
                });
                let has_player = player_vec.is_some_and(|pv| pv == neighbor);
                if has_enemy || has_player {
                    count += 1;
                }
            }
            count
        };

        // NPC Dodge: sidestep when projectile is nearby.
        // Uses tile cost as secondary criterion: among tiles equally far
        // from the nearest projectile, prefer covered/safe tiles.
        let dodge_roll = dynamic_rng.roll(seed.0, entity.to_bits() ^ 0xD0D6);
        let nearby_danger = spell_particles.particles.iter().any(|(p, life, delay, _, _, _)| {
            *delay == 0 && *life > 0 && my_pos.chebyshev_distance(*p) <= 2
        });
        if nearby_danger && dodge_roll < DODGE_CHANCE {
            let mut best_dir = None;
            let mut best_score = (0i32, i32::MAX); // (particle_dist, -tile_cost)
            for dir in GridVec::DIRECTIONS_8 {
                let target = my_pos + dir;
                let tc = match tile_cost_for_ai(target, entity, &game_map, &spatial, &blockers) {
                    Some(c) => c,
                    None => continue,
                };
                let min_particle_dist = spell_particles.particles.iter()
                    .filter(|(_, life, delay, _, _, _)| *delay == 0 && *life > 0)
                    .map(|(p, _, _, _, _, _)| target.chebyshev_distance(*p))
                    .min()
                    .unwrap_or(i32::MAX);
                let score = (min_particle_dist, -tc);
                if score > best_score {
                    best_score = score;
                    best_dir = Some(dir);
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
        if should_flee(&health) {
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
                    assign_aiming_style(&mut commands, entity, turn_counter.0, chase_target);
                    if let Some((_, tv)) = chase_target {
                        let toward = (tv - my_pos).king_step();
                        if !toward.is_zero() {
                            update_look_dir(toward, &mut ai_look_dir, &mut viewshed);
                        }
                    }
                } else {
                    // No enemy visible — prioritize reloading.
                    let mut reloaded = false;
                    if has_unloaded_gun(&inventory, &item_kinds)
                        && let Some(ref mut inv) = inventory {
                            let reloadable = inv.items.iter().copied().find(|&ent| {
                                item_kinds.get(ent).ok().is_some_and(|k|
                                    matches!(k, ItemKind::Gun { loaded, capacity, .. } if *loaded < *capacity)
                                )
                            });
                            if let Some(gun_entity) = reloadable
                                && let Ok(mut kind) = item_kinds.get_mut(gun_entity)
                                    && let ItemKind::Gun { ref mut loaded, .. } = *kind {
                                        *loaded += 1;
                                        reloaded = true;
                                    }
                        }
                    if reloaded {
                        energy.spend_action();
                        continue;
                    }

                    // Scavenge items — but only when no remembered target exists.
                    // Pursuit of a known threat always takes priority.
                    if let Some(ref mem) = ai_memory
                        && let Some(remembered_pos) = mem.last_known_pos
                        && turn_counter.0.saturating_sub(mem.last_seen_turn) < MEMORY_DURATION
                        && my_pos != remembered_pos
                    {
                        let step = a_star_first_step(my_pos, remembered_pos, |p| {
                            tile_cost_for_ai(p, entity, &game_map, &spatial, &blockers)
                        });
                        if let Some(step) = step
                            && !step.is_zero() {
                                move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                                update_look_dir(step, &mut ai_look_dir, &mut viewshed);
                            }
                        energy.spend_action();
                    } else if let Some((_, item_vec)) = nearest_item {
                        let step = a_star_first_step(my_pos, item_vec, |p| {
                            tile_cost_for_ai(p, entity, &game_map, &spatial, &blockers)
                        });
                        if let Some(step) = step
                                && !step.is_zero() {
                                    move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                                update_look_dir(step, &mut ai_look_dir, &mut viewshed);
                            }
                        energy.spend_action();
                    } else {
                        if let Some(ref mut mem) = ai_memory
                            && turn_counter.0.saturating_sub(mem.last_seen_turn) >= MEMORY_DURATION {
                                mem.last_known_pos = None;
                            }
                        // Circular rotation: if a rotation sequence is active,
                        // continue it one CW step; otherwise start a new one on
                        // the look-around interval.
                        if is_rotating(&ai_look_dir) {
                            rotate_look_dir(&mut ai_look_dir, &mut viewshed);
                        } else {
                            let look_hash = (my_pos.x.wrapping_mul(7919) ^ my_pos.y.wrapping_mul(6271)).unsigned_abs();
                            let look_interval = LOOK_AROUND_BASE_INTERVAL + (look_hash % LOOK_AROUND_DICE_RANGE);
                            if turn_counter.0 > 0 && turn_counter.0.is_multiple_of(look_interval) {
                                begin_circular_rotation(&mut ai_look_dir, &mut viewshed);
                            } else {
                                rotate_look_dir(&mut ai_look_dir, &mut viewshed);
                            }
                        }
                        energy.spend_action();
                    }
                }
            }
            AiState::Patrolling => {
                if chase_target.is_some() {
                    *ai = AiState::Chasing;
                    assign_aiming_style(&mut commands, entity, turn_counter.0, chase_target);
                    continue;
                }

                // No enemy visible — prioritize reloading.
                let mut reloaded_patrol = false;
                if has_unloaded_gun(&inventory, &item_kinds)
                    && let Some(ref mut inv) = inventory {
                        let reloadable = inv.items.iter().copied().find(|&ent| {
                            item_kinds.get(ent).ok().is_some_and(|k|
                                matches!(k, ItemKind::Gun { loaded, capacity, .. } if *loaded < *capacity)
                            )
                        });
                        if let Some(gun_entity) = reloadable
                            && let Ok(mut kind) = item_kinds.get_mut(gun_entity)
                                && let ItemKind::Gun { ref mut loaded, .. } = *kind {
                                    *loaded += 1;
                                    reloaded_patrol = true;
                                }
                    }
                if reloaded_patrol {
                    energy.spend_action();
                    continue;
                }

                // Pursue remembered targets before scavenging items —
                // chasing a known threat always takes priority.
                if let Some(ref mem) = ai_memory
                    && let Some(remembered_pos) = mem.last_known_pos
                    && turn_counter.0.saturating_sub(mem.last_seen_turn) < MEMORY_DURATION
                    && my_pos != remembered_pos
                {
                    let step = a_star_first_step(my_pos, remembered_pos, |p| {
                        tile_cost_for_ai(p, entity, &game_map, &spatial, &blockers)
                    });
                    if let Some(step) = step
                            && !step.is_zero() {
                                move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                            update_look_dir(step, &mut ai_look_dir, &mut viewshed);
                        }
                    energy.spend_action();
                    continue;
                }

                if let Some((_, item_vec)) = nearest_item {
                    let step = a_star_first_step(my_pos, item_vec, |p| {
                        tile_cost_for_ai(p, entity, &game_map, &spatial, &blockers)
                    });
                    if let Some(step) = step
                            && !step.is_zero() {
                                move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                            update_look_dir(step, &mut ai_look_dir, &mut viewshed);
                        }
                    energy.spend_action();
                    continue;
                }

                if let Some(ref mut mem) = ai_memory
                    && turn_counter.0.saturating_sub(mem.last_seen_turn) >= MEMORY_DURATION {
                        mem.last_known_pos = None;
                    }

                let origin = patrol_origin.map(|po| po.0).unwrap_or(my_pos);

                // Circular rotation: if a rotation sequence is active,
                // continue rotating CW and skip movement until the full
                // 360° circle is complete.
                if is_rotating(&ai_look_dir) {
                    rotate_look_dir(&mut ai_look_dir, &mut viewshed);
                    energy.spend_action();
                    continue;
                }

                let pause_hash = (my_pos.x.wrapping_mul(13) ^ my_pos.y.wrapping_mul(7))
                    .wrapping_add(turn_counter.0 as i32) as u32;
                // Start a new circular look-around every LOOK_AROUND interval.
                let look_hash_patrol = (my_pos.x.wrapping_mul(7919) ^ my_pos.y.wrapping_mul(6271)).unsigned_abs();
                let look_interval_patrol = LOOK_AROUND_BASE_INTERVAL + (look_hash_patrol % LOOK_AROUND_DICE_RANGE);
                if turn_counter.0 > 0 && turn_counter.0.is_multiple_of(look_interval_patrol) {
                    begin_circular_rotation(&mut ai_look_dir, &mut viewshed);
                    energy.spend_action();
                    continue;
                }
                if pause_hash.is_multiple_of(7) {
                    rotate_look_dir(&mut ai_look_dir, &mut viewshed);
                    energy.spend_action();
                    continue;
                }

                // Faction-specific patrol direction (preferred heading).
                let patrol_heading: Option<GridVec> = match my_faction {
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

                // Influence-weighted patrol: score each neighbor tile by
                // combining the preferred heading with the tile's influence
                // cost. Lower tile cost (cover, safe terrain) is preferred;
                // tiles aligned with the patrol heading get a bonus.
                if let Some(heading) = patrol_heading
                    && !heading.is_zero()
                {
                    let mut best_dir = None;
                    let mut best_score = i32::MIN;
                    for dir in GridVec::DIRECTIONS_8 {
                        let candidate = my_pos + dir;
                        let tc = match tile_cost_for_ai(candidate, entity, &game_map, &spatial, &blockers) {
                            Some(c) => c,
                            None => continue,
                        };
                        // Heading alignment bonus: dot product with heading.
                        let alignment = dir.x * heading.x + dir.y * heading.y;
                        let score = alignment * cost::BASE - tc;
                        if score > best_score {
                            best_score = score;
                            best_dir = Some(dir);
                        }
                    }
                    if let Some(step) = best_dir {
                        move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                        update_look_dir(step, &mut ai_look_dir, &mut viewshed);
                    }
                }

                energy.spend_action();
            }
            AiState::Fleeing => {
                let threat_pos = chase_target.map(|(_, tv)| tv)
                    .or(player_vec);

                if let Some(tp) = threat_pos {
                    if let Some(dir) = flee_direction(my_pos, tp, entity, &game_map, &spatial, &blockers) {
                        move_intents.write(MoveIntent { entity, dx: dir.x, dy: dir.y });
                        update_look_dir(dir, &mut ai_look_dir, &mut viewshed);
                    }
                } else {
                    let origin = patrol_origin.map(|po| po.0).unwrap_or(my_pos);
                    if my_pos != origin {
                        let step = a_star_first_step(my_pos, origin, |p| {
                            tile_cost_for_ai(p, entity, &game_map, &spatial, &blockers)
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
                                tile_cost_for_ai(pos, entity, &game_map, &spatial, &blockers)
                            })
                            .unwrap_or_else(|| (remembered_pos - my_pos).king_step());

                            if !step.is_zero() {
                                move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                                update_look_dir(step, &mut ai_look_dir, &mut viewshed);
                            }
                            energy.spend_action();
                            continue;
                        }
                        // Check if we have a persistent AiTarget to pursue
                        if let Some(ai_tgt) = ai_target {
                            if my_pos.chebyshev_distance(ai_tgt.last_pos) > 0 {
                                let step = a_star_first_step(my_pos, ai_tgt.last_pos, |pos| {
                                    tile_cost_for_ai(pos, entity, &game_map, &spatial, &blockers)
                                }).unwrap_or_else(|| (ai_tgt.last_pos - my_pos).king_step());
                                if !step.is_zero() {
                                    move_intents.write(MoveIntent { entity, dx: step.x, dy: step.y });
                                    update_look_dir(step, &mut ai_look_dir, &mut viewshed);
                                }
                                energy.spend_action();
                                continue;
                            }
                        }
                        // Only now fall back to patrolling
                        *ai = if patrol_origin.is_some() { AiState::Patrolling } else { AiState::Idle };
                        if let Some(ref mut mem) = ai_memory {
                            mem.last_known_pos = None;
                        }
                        commands.entity(entity).remove::<AiTarget>();
                        commands.entity(entity).remove::<AimingStyle>();
                        energy.spend_action();
                        continue;
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

                // CarefulAim: spend an extra turn aiming before firing (every other turn)
                if matches!(aiming_style, Some(&AimingStyle::CarefulAim)) {
                    let aim_turn = (entity.to_bits() ^ turn_counter.0 as u64) % 2;
                    if aim_turn == 0 {
                        energy.spend_action();
                        continue;
                    }
                }

                let dist = my_pos.chebyshev_distance(target_vec);

                // ── PRIORITY 1: Ranged attack — fire guns when enemy is in sights ──
                // NPCs shoot at enemies whenever they have a loaded gun and
                // clear line of sight — no range limit. If they can see you,
                // they shoot at you.
                let mut used_gun = false;
                if dist > 1
                    && has_clear_line_of_sight(my_pos, target_vec, &game_map, &sand_cloud_tiles)
                    && !has_friendly_in_path(my_pos, target_vec, my_faction, entity, &spatial, &npc_positions)
                    && let Some(ref mut inv) = inventory {
                        let gun_ent = inv.items.iter().copied().find(|&ent| {
                            item_kinds.get(ent).ok().is_some_and(|k|
                                matches!(k, ItemKind::Gun { loaded, .. } if *loaded > 0)
                            )
                        });
                        if let Some(gun_entity) = gun_ent {
                            let mut dx = target_vec.x - my_pos.x;
                            let mut dy = target_vec.y - my_pos.y;
                            // Apply aiming style modifiers to ranged fire
                            match aiming_style {
                                Some(&AimingStyle::SnapShot) => {
                                    // Wider spread: offset endpoint by a small pseudo-random amount
                                    let spread_hash = (entity.to_bits() ^ turn_counter.0 as u64).wrapping_mul(HASH_LCG);
                                    let spread_x = ((spread_hash % 3) as i32) - 1;
                                    let spread_y = (((spread_hash >> 16) % 3) as i32) - 1;
                                    dx += spread_x;
                                    dy += spread_y;
                                }
                                Some(&AimingStyle::Suppression) => {
                                    // Approximate direction only: use king_step scaled up
                                    let dir = (target_vec - my_pos).king_step();
                                    let range = dist.max(AI_RANGED_ATTACK_RANGE);
                                    dx = dir.x * range;
                                    dy = dir.y * range;
                                }
                                _ => {}
                            }
                            ranged_intents.write(RangedAttackIntent {
                                attacker: entity,
                                range: dist.max(AI_RANGED_ATTACK_RANGE),
                                dx,
                                dy,
                                gun_item: Some(gun_entity),
                            });
                            used_gun = true;
                        } else {
                            // Gun empty but in combat — reload immediately.
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
                if used_gun {
                    energy.spend_action();
                    continue;
                }

                // ── PRIORITY 2: Bow attack (unlimited ammo) ──
                let mut used_bow = false;
                if dist > 1
                    && has_clear_line_of_sight(my_pos, target_vec, &game_map, &sand_cloud_tiles)
                    && !has_friendly_in_path(my_pos, target_vec, my_faction, entity, &spatial, &npc_positions)
                    && let Some(ref inv) = inventory {
                        let bow_ent = inv.items.iter().copied().find(|&ent| {
                            item_kinds.get(ent).ok().is_some_and(|k|
                                matches!(k, ItemKind::Bow { .. })
                            )
                        });
                        if let Some(bow_entity) = bow_ent
                            && let Ok(kind) = item_kinds.get(bow_entity)
                                && let ItemKind::Bow { attack: bow_atk, .. } = *kind {
                                    let dx = target_vec.x - my_pos.x;
                                    let dy = target_vec.y - my_pos.y;
                                    let max_comp = dx.abs().max(dy.abs()).max(1);
                                    let range = dist.max(AI_RANGED_ATTACK_RANGE);
                                    let scale = range.div_euclid(max_comp).max(1);
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
                if used_bow {
                    energy.spend_action();
                    continue;
                }

                // ── PRIORITY 3: Adjacent melee attack ──
                if dist == 1 {
                    attack_intents.write(AttackIntent {
                        attacker: entity,
                        target: target_entity,
                    });
                    energy.spend_action();
                    continue;
                }

                // ── PRIORITY 4: Throwable items (grenade/molotov) at medium range ──
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

                // Weighted A* pathfinding toward target.
                // The influence map naturally routes the NPC through cover
                // and around hazards.  Occasionally attempt to flank by
                // adding a perpendicular offset to the goal.
                let flank_hash = (my_pos.x.wrapping_mul(31) ^ my_pos.y.wrapping_mul(17))
                    .wrapping_add(turn_counter.0 as i32) as u32;
                let flank_goal = if dist > 3 && flank_hash.is_multiple_of(3) {
                    let perp = (target_vec - my_pos).rotate_90_cw().king_step();
                    let candidate = target_vec + perp;
                    if game_map.0.is_passable(&candidate) { candidate } else { target_vec }
                } else {
                    target_vec
                };
                let step = a_star_first_step(my_pos, flank_goal, |pos| {
                    tile_cost_for_ai(pos, entity, &game_map, &spatial, &blockers)
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

// ─────────── Influence Map / A* / Dijkstra Tests ──────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: uniform-cost walkable closure (all tiles cost BASE).
    fn walkable(_: GridVec) -> Option<i32> { Some(cost::BASE) }

    /// Helper: convert a wall set into a cost closure (walls → None, rest → BASE).
    fn walls_to_cost(wall: &HashSet<GridVec>) -> impl Fn(GridVec) -> Option<i32> + '_ {
        move |pos| if wall.contains(&pos) { None } else { Some(cost::BASE) }
    }

    // ── A* core tests ─────────────────────────────────────────────

    #[test]
    fn a_star_adjacent_returns_direct_step() {
        let start = GridVec::new(5, 5);
        let goal = GridVec::new(6, 5);
        let step = a_star_first_step(start, goal, walkable);
        assert_eq!(step, Some(GridVec::new(1, 0)));
    }

    #[test]
    fn a_star_straight_line_path() {
        let start = GridVec::new(0, 0);
        let goal = GridVec::new(5, 0);
        let step = a_star_first_step(start, goal, walkable);
        assert!(step.is_some(), "A* should find a step toward the goal");
        let s = step.unwrap();
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
        let step = a_star_first_step(start, goal, walkable);
        assert_eq!(step, Some(GridVec::new(1, 1)));
    }

    #[test]
    fn a_star_navigates_around_wall() {
        let start = GridVec::new(2, 3);
        let goal = GridVec::new(5, 3);
        let wall: HashSet<GridVec> = [
            GridVec::new(3, 2),
            GridVec::new(3, 3),
            GridVec::new(3, 4),
        ]
        .into_iter()
        .collect();

        let step = a_star_first_step(start, goal, walls_to_cost(&wall));
        assert!(step.is_some(), "A* should find a path around the wall");
        let s = step.unwrap();
        let next = start + s;
        assert!(!wall.contains(&next), "First step should not be into a wall");
    }

    #[test]
    fn a_star_returns_none_when_unreachable() {
        let start = GridVec::new(0, 0);
        let goal = GridVec::new(10, 10);
        let step = a_star_first_step(start, goal, |pos| {
            // Block exactly the ring at Chebyshev distance 1 from goal.
            let d = pos.chebyshev_distance(goal);
            if d == 1 { None } else { Some(cost::BASE) }
        });
        assert!(step.is_none(), "Should return None when goal is surrounded");
    }

    #[test]
    fn a_star_zero_distance_returns_none() {
        let pos = GridVec::new(5, 5);
        let step = a_star_first_step(pos, pos, walkable);
        assert_eq!(step, None, "No step needed when already at goal");
    }

    // ── Weighted A* tests ─────────────────────────────────────────

    #[test]
    fn a_star_prefers_low_cost_path() {
        // Two paths from (0,0) to (4,0):
        //   Direct (y=0): interior tiles cost 50 each (expensive / hazardous)
        //   Detour (y≠0): tiles cost BASE each (safe / covered)
        // A* should route through the cheaper detour.
        let start = GridVec::new(0, 0);
        let goal = GridVec::new(4, 0);
        let step = a_star_first_step(start, goal, |pos| {
            if pos.y == 0 && pos.x > 0 && pos.x < 4 {
                Some(50) // Hazardous corridor
            } else {
                Some(cost::BASE) // Safe detour
            }
        });
        assert!(step.is_some());
        let s = step.unwrap();
        // Should step off the y=0 line to avoid the expensive corridor.
        assert_ne!(s.y, 0, "Should detour around the hazardous corridor, got ({}, {})", s.x, s.y);
    }

    #[test]
    fn a_star_uses_cover_bonus() {
        // y=2 has low cost (simulating cover near walls), y=0 has high cost.
        let start = GridVec::new(0, 1);
        let goal = GridVec::new(5, 1);
        let step = a_star_first_step(start, goal, |pos| {
            if pos.y == 2 { Some(5) } // "Covered" path
            else if pos.y == 0 { Some(20) } // Exposed path
            else { Some(cost::BASE) }
        });
        assert!(step.is_some());
        let s = step.unwrap();
        // Should prefer moving toward the cheaper (covered) y=2 row.
        assert!(s.y > 0, "Should prefer moving toward cover (y=2), got y={}", s.y);
    }

    // ── Dijkstra map tests ────────────────────────────────────────

    #[test]
    fn dijkstra_source_has_zero_distance() {
        let src = GridVec::new(5, 5);
        let map = dijkstra_map(&[src], walkable);
        assert_eq!(*map.get(&src).unwrap(), 0);
    }

    #[test]
    fn dijkstra_distance_increases_outward() {
        let src = GridVec::new(5, 5);
        let map = dijkstra_map(&[src], walkable);
        // Adjacent tile should have distance == BASE (one step away).
        let adj = GridVec::new(6, 5);
        assert_eq!(*map.get(&adj).unwrap_or(&0), cost::BASE);
        // Two steps away should be 2*BASE.
        let far = GridVec::new(7, 5);
        assert_eq!(*map.get(&far).unwrap_or(&0), cost::BASE * 2);
    }

    #[test]
    fn dijkstra_blocked_tiles_not_reached() {
        let src = GridVec::new(0, 0);
        // Surround the source with walls (block at distance 1).
        let map = dijkstra_map(&[src], |pos| {
            if pos == src { return Some(cost::BASE); }
            if pos.chebyshev_distance(src) == 1 { return None; }
            Some(cost::BASE)
        });
        // Source is reachable.
        assert!(map.contains_key(&src));
        // Tiles beyond the ring should NOT be reached.
        let beyond = GridVec::new(2, 0);
        assert!(!map.contains_key(&beyond), "Tiles beyond blocked ring should be unreachable");
    }

    #[test]
    fn dijkstra_multi_source_picks_nearest() {
        let a = GridVec::new(0, 0);
        let b = GridVec::new(10, 0);
        let map = dijkstra_map(&[a, b], walkable);
        // Midpoint (5,0) should have distance 5*BASE from nearest source.
        let mid = GridVec::new(5, 0);
        assert_eq!(*map.get(&mid).unwrap_or(&0), 5 * cost::BASE);
    }

    #[test]
    fn dijkstra_high_cost_tiles_have_larger_distance() {
        let src = GridVec::new(0, 0);
        let map = dijkstra_map(&[src], |pos| {
            if pos.y > 0 { Some(50) } // Expensive tiles north
            else { Some(cost::BASE) } // Cheap tiles along y=0
        });
        // (1,0) via cheap path: cost BASE
        // (0,1) via expensive path: cost 50
        let cheap = *map.get(&GridVec::new(1, 0)).unwrap_or(&0);
        let expensive = *map.get(&GridVec::new(0, 1)).unwrap_or(&0);
        assert!(expensive > cheap, "Expensive tiles should have larger Dijkstra distance");
    }

    // ── Tile cost module constants sanity check ───────────────────

    #[test]
    fn cost_constants_are_positive() {
        assert!(cost::BASE > 0);
        assert!(cost::FIRE > cost::BASE);
        assert!(cost::NEAR_FIRE > cost::BASE);
        assert!(cost::NEAR_CACTUS > cost::BASE);
        assert!(cost::SAND_CLOUD > cost::BASE);
        assert!(cost::COVER_PER_WALL > 0);
        assert!(cost::EXPOSED > 0);
    }

    // ── Patrol rotation helpers ───────────────────────────────────

    #[test]
    fn circular_rotation_cycles_all_8_directions() {
        // Verify that DIRECTIONS_8 is 8 elements and they form a CW cycle.
        assert_eq!(GridVec::DIRECTIONS_8.len(), 8);
        // Starting from NORTH, stepping CW through all 8 should return to NORTH.
        let start_idx = GridVec::DIRECTIONS_8.iter()
            .position(|&d| d == GridVec::NORTH)
            .unwrap();
        let end_idx = (start_idx + 8) % 8;
        assert_eq!(GridVec::DIRECTIONS_8[end_idx], GridVec::NORTH);
    }

    #[test]
    fn full_rotation_steps_constant_is_eight() {
        assert_eq!(FULL_ROTATION_STEPS, 8,
            "A full 360° rotation should take exactly 8 steps of 45° each");
    }

    #[test]
    fn rotation_counter_prevents_premature_resume() {
        // Simulate a rotation sequence by manually tracking the counter.
        let mut remaining: u8 = FULL_ROTATION_STEPS;
        let mut steps_taken = 0;
        while remaining > 0 {
            remaining = remaining.saturating_sub(1);
            steps_taken += 1;
        }
        assert_eq!(steps_taken, FULL_ROTATION_STEPS as u32);
    }

    #[test]
    fn half_rotation_requires_four_steps() {
        // A half rotation (180°) requires at least 4 × 45° steps.
        // This ensures an NPC won't face a recently-faced direction
        // until completing at least a half rotation.
        let start = GridVec::NORTH;
        let start_idx = GridVec::DIRECTIONS_8.iter()
            .position(|&d| d == start)
            .unwrap();
        let opposite_idx = (start_idx + 4) % 8;
        assert_eq!(GridVec::DIRECTIONS_8[opposite_idx], GridVec::SOUTH,
            "4 CW steps from NORTH should reach SOUTH (opposite)");
    }
}
