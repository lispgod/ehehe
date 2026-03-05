# ECS Architecture for Bevy Roguelike (bevy_ratatui)

This document describes the Entity-Component-System (ECS) architecture used by this
roguelike, built with Bevy and rendered through `bevy_ratatui`.

---

## Core Principles

1. **Entities** represent every discrete game object (player, NPCs, items, projectiles).
2. **Components** are plain data attached to entities — no behavior.
3. **Systems** contain all logic; they query for combinations of components.
4. **Resources** hold global/singleton state (the tile map, camera position, spatial index).
5. **Events (Messages)** decouple intent from execution (e.g., `MoveIntent` → collision check → position update).
6. **States** control which systems run each frame (e.g., `Playing` vs `Paused`).
7. **Sub-states** model turn phases within gameplay (`AwaitingInput` → `PlayerTurn` → `WorldTurn`).
8. **System Sets** enforce a strict pipeline: `Index → Action → Consequence → Render`.

---

## Mathematical Foundations

### GridVec — Algebraic 2D Integer Vector

All grid coordinates use the `GridVec` type, which forms an **Abelian group** (ℤ², +):

| Property | Expression | Meaning |
|---|---|---|
| **Closure** | `GridVec + GridVec → GridVec` | Sum of two vectors is a vector |
| **Associativity** | `(a + b) + c = a + (b + c)` | Grouping doesn't matter |
| **Identity** | `GridVec::ZERO` | Adding zero changes nothing |
| **Inverse** | `-v` for every `v` | Every vector has a negation |
| **Commutativity** | `a + b = b + a` | Order doesn't matter |

Additionally, `GridVec` supports scalar multiplication (ℤ-module structure), four
distance metrics, bilinear operations, and lattice rotations:

| Metric | Formula | Use case |
|---|---|---|
| Manhattan (L₁) | `\|Δx\| + \|Δy\|` | 4-connected grid distance |
| Chebyshev (L∞) | `max(\|Δx\|, \|Δy\|)` | 8-connected (king move) distance |
| Squared Euclidean | `Δx² + Δy²` | Comparison without sqrt (monotonic) |
| Euclidean (L₂) | `√(Δx² + Δy²)` | True straight-line distance (for attenuation, display) |

| Operation | Formula | Use case |
|---|---|---|
| **Dot product** | `aₓbₓ + aᵧbᵧ` | Projection, alignment test (positive → same half-plane, zero → orthogonal) |
| **2D cross product** | `aₓbᵧ − aᵧbₓ` | Signed area / orientation test (det of 2×2 matrix [a\|b]) |
| **Norm²** | `x² + y²` | Squared length (= `v · v`), avoids sqrt for magnitude comparison |
| **King step** | `(signum(x), signum(y))` | Normalize to Chebyshev unit ball — single-step 8-directional movement |
| **is_zero** | `x = 0 ∧ y = 0` | Identity element test |
| **Rotate 90° CW** | `(x, y) ↦ (y, −x)` | SO(2) lattice rotation (cyclic group C₄: R⁴ = I) |
| **Rotate 90° CCW** | `(x, y) ↦ (−y, x)` | Inverse rotation (R_ccw ∘ R_cw = I) |
| **Bresenham line** | Integer rasterization | Exact tile sequence between two points for line-of-sight and ranged attacks |

**Norm² identity**: `‖v‖² = v · v = distance_squared(v, ZERO)`. This connects the
inner product structure to the metric space structure, verified by unit tests.

#### Lattice Rotations (C₄ Cyclic Group)

The `rotate_90_cw` and `rotate_90_ccw` methods apply the SO(2) rotation
matrices restricted to ℤ²:

```
R_cw  = [[0, 1], [−1, 0]]     R_ccw = [[0, −1], [1, 0]]
```

These form a **cyclic group of order 4**: {I, R, R², R³} where R⁴ = I.
Properties verified by unit tests:
- **Closure**: rotation of a `GridVec` produces a `GridVec`.
- **Isometry**: `|Rv|² = |v|²` (preserves squared Euclidean distance).
- **Inverse**: `R_cw ∘ R_ccw = R_ccw ∘ R_cw = I`.
- **Order**: `R⁴ = I` (four rotations return to original).

#### Bresenham Line Algorithm

`bresenham_line(self, other)` computes the integer-only rasterization of a
line segment between two grid points. This is the fundamental primitive for
ranged attack trajectories and line-of-sight checks.

**Properties:**
- **Exact endpoints**: first element is `self`, last is `other`.
- **8-connected**: each successive pair of points differs by at most 1 in each axis.
- **Deterministic**: pure function of endpoints, no floating-point.
- **Time**: O(max(|Δx|, |Δy|)) — visits each tile exactly once.
- **Symmetry**: `a.bresenham_line(b)` and `b.bresenham_line(a)` traverse the same set of points.

All group axioms, rotation properties, and Bresenham invariants are verified by unit tests.

### Energy-Based Turn Scheduling

The turn system uses a **discrete-event energy scheduler**, the mathematically correct
algorithm for multi-actor turn ordering in roguelikes (used by Angband, DCSS, Cogmind):

```
Each world tick:
  for each actor:
    energy.accumulate(&speed)     // energy += speed

  for each actor where energy.can_act():   // energy ≥ ACTION_COST
    perform action
    energy.spend_action()         // energy -= ACTION_COST
```

The `Energy` component provides three methods that encapsulate the scheduling invariants:
- `accumulate(&Speed)` — one tick of energy gain.
- `can_act() -> bool` — predicate: `energy ≥ ACTION_COST`.
- `spend_action()` — deducts `ACTION_COST`, preserving excess for next tick.

**Properties:**
- **Exact fairness**: over N ticks, an entity with speed S takes exactly ⌊N × S / ACTION_COST⌋ actions. Verified by a property test that checks this identity for multiple speed values over 100 ticks.
- **Integer-only**: no floating-point, no rounding errors.
- **Deterministic**: same inputs → same scheduling order.
- **Speed ratios**: Speed(100) = 1 action/tick; Speed(50) = 1 action per 2 ticks; Speed(200) = 2 actions/tick.

### Symmetric Shadowcasting

Field-of-view uses recursive symmetric shadowcasting (Albert Ford, 2017) with
rational slopes (integer y/x) to avoid floating-point:

- **Symmetry**: A sees B ⟺ B sees A.
- **Completeness**: no visible tile is missed.
- **Efficiency**: O(visible tiles) — each tile visited at most once per octant.

### A* Pathfinding (AI Navigation)

AI entities use **A\* search** with the **Chebyshev heuristic** (L∞ norm) and
**lexicographic tie-breaking** to find optimal paths to the player, navigating
around walls, furniture, and other blocking entities.

**Mathematical guarantees:**

| Property | Explanation |
|---|---|
| **Optimality** | Chebyshev distance is *admissible* (`h(n) ≤ h*(n)` — never overestimates) and *consistent* (`h(n) ≤ c(n,n') + h(n')` — satisfies triangle inequality). Therefore A* finds the shortest path. |
| **Tie-breaking** | When multiple open nodes share the same f-score, we prefer the node with the lower h-score (i.e., higher g-score, meaning further along the path). This lexicographic ordering `(f, h)` reduces the number of expanded nodes without affecting optimality — among equal-f nodes, those nearer the goal are expanded first, pruning the search frontier. |
| **Completeness** | If a path exists within the 256-node exploration budget, it will be found. |
| **Time** | O(k log k) where k = nodes explored (≤ 256). Each node visits ≤ 8 neighbours. |
| **Space** | O(k) for the open set (binary heap), closed set, g-score map, and came-from map. |
| **Fallback** | If A* exhausts its budget (target unreachable or too far), the AI falls back to greedy king-step (component-wise `signum` toward the player). |

The heuristic is the Chebyshev distance because the grid is 8-connected with
uniform edge cost — diagonal and cardinal moves cost the same. This makes
Chebyshev the tightest admissible heuristic, minimising node expansions.

### Spatial Index Atomicity

The `SpatialIndex` resource provides three encapsulated operations for
maintaining the position→entity mapping:

| Method | Signature | Purpose |
|---|---|---|
| `add_entity` | `(point, entity)` | Register an entity at a tile |
| `remove_entity` | `(point, entity)` | Unregister an entity from a tile |
| `move_entity` | `(old, new, entity)` | Atomic remove + add (single call) |

The `movement_system` uses `move_entity` to maintain the **spatial index
invariant** after each successful move. This ensures that when multiple
`MoveIntent`s are processed in a single frame (e.g., AI moves during WorldTurn),
each subsequent intent sees the up-to-date occupancy map.

**Without this invariant**, two entities could move to the same tile
simultaneously because the index would still show their original positions
(stale read). The atomic `move_entity` prevents this race condition entirely,
and encapsulating it as a method (rather than ad-hoc field manipulation)
ensures all callers maintain the invariant correctly.

### Damage Model — `compute_damage(atk, def)`

All combat damage in the game is resolved through a single pure function:

```
damage(atk, def) = max(0, atk − def)
```

This function is used by melee bump attacks (`combat_system`), roundhouse kicks
(`melee_wide_system`), thrown weapons (`throw_system`), and any other damage
source that uses attack power. Having a single canonical formula
eliminates divergence between systems.

**Mathematical properties** (verified by property tests):

| Property | Statement |
|---|---|
| **Non-negative** | ∀ atk: `damage(atk) ≥ 0` |
| **Monotone ↑ in attack** | `atk₁ ≤ atk₂ ⟹ damage(atk₁) ≤ damage(atk₂)` |
| **Linearity** | `damage(atk) = max(0, atk)` |
| **Idempotent clamping** | `max(0, max(0, x)) = max(0, x)` |

The `CombatStats` component provides a convenience method `damage_against(&defender)`
that calls `compute_damage(self.attack)`.

### Pool Invariants — Health, Stamina

The `Health` and `Stamina` components represent **clamped integer pools**
with the fundamental invariant:

```
0 ≤ current ≤ max
```

Each pool provides methods that maintain this invariant:

| Component | Method | Behaviour |
|---|---|---|
| `Health` | `apply_damage(amount) → actual` | Reduces current, clamps to 0, returns actual damage dealt |
| `Health` | `heal(amount) → actual` | Increases current, clamps to max, returns actual HP restored |
| `Health` | `is_dead() → bool` | True when `current ≤ 0` |
| `Health` | `is_full() → bool` | True when `current ≥ max` |
| `Health` | `fraction() → f64` | Returns `current / max ∈ [0, 1]` (0 if max = 0) |
| `Stamina` | `spend(cost) → bool` | Atomic check-and-deduct: returns false (no mutation) if insufficient |
| `Stamina` | `recover(amount)` | Increases current, clamps to max |

Using these methods instead of raw field mutation guarantees that:
1. Health never goes below 0 (no negative HP bugs).
2. Healing never exceeds max (no over-heal exploits).
3. Stamina spending is atomic (check + deduct in one operation).
4. The `actual` return value from `apply_damage` / `heal` enables accurate combat logging.

---

## Components

| Component | Type | Purpose |
|---|---|---|
| `Position` | `{ x: i32, y: i32 }` | World-grid coordinate for any entity |
| `Player` | marker (unit struct) | Tags the player-controlled entity |
| `Name` | `Name(String)` | Display name for combat messages, UI, and logs |
| `Renderable` | `{ symbol: String, fg: Color, bg: Color }` | Visual representation of an entity |
| `CameraFollow` | marker | Tags the entity the camera tracks |
| `BlocksMovement` | marker | Marks an entity as impassable (enforced via `SpatialIndex`) |
| `Hostile` | marker | Tags entities hostile to the player (triggers bump-to-attack) |
| `Viewshed` | `{ range, visible_tiles, revealed_tiles, dirty }` | Field-of-view + fog-of-war memory |
| `Health` | `{ current: i32, max: i32 }` | Hit-point pool for damageable entities |
| `CombatStats` | `{ attack: i32 }` | Offensive power for combat resolution |
| `Speed` | `Speed(i32)` | Energy gained per world tick (100 = normal) |
| `Energy` | `Energy(i32)` | Accumulated action points; act when ≥ ACTION_COST |
| `AiState` | `Idle \| Chasing` | AI behaviour state for non-player entities |
| `Faction` | `Wildlife \| Outlaws \| Lawmen \| Vaqueros` | Group affiliation for spawning tiers |
| `Stamina` | `{ current: i32, max: i32 }` | Pool for special actions (grenades, etc.) |
| `Item` | marker | Tags an entity as a pickable item |
| `ItemKind` | `Gun \| Knife \| Tomahawk \| Grenade \| Whiskey \| Hat` | Item type and associated stats |
| `Inventory` | `{ items: Vec<Entity> }` | Holds item entities belonging to an entity |
| `Projectile` | `{ path, path_index, tiles_per_tick, damage, penetration, source }` | Traveling bullet/shrapnel |
| `CollectibleKind` | `Caps \| Bullets \| Powder \| Bandages \| Dollars` | Supply drop type |
| `Experience` | `{ current: i32, next_level: i32 }` | EXP tracking for the player |
| `Level` | `Level(i32)` | Player level for stat scaling |
| `LootTable` | `{ drop_chance: f64 }` | Controls item drops on entity death |
| `ExpReward` | `ExpReward(i32)` | EXP awarded when this entity is killed |

### Why markers?

Bevy queries use component presence for filtering. `Player` lets any system
find the player with `Query<&Position, With<Player>>` without coupling to a
concrete "player struct." `Hostile` enables bump-to-attack without the movement
system knowing anything about combat rules.

---

## Entities

### Player

```text
Entity
 ├─ Position { x, y }
 ├─ Player          (marker)
 ├─ Name("Player")
 ├─ Renderable { symbol: "@", fg: White, bg: Black }
 ├─ CameraFollow    (marker)
 ├─ Health { current: 30, max: 30 }
 ├─ CombatStats { attack: 5 }
 ├─ Speed(100)      (normal speed)
 ├─ Energy(0)
 └─ Viewshed { range: 15, dirty: true }
```

### Monster (Coyote / Outlaw / Cowboy / …)

```text
Entity
 ├─ Position { x, y }
 ├─ Name("Coyote")
 ├─ Renderable { symbol: "c", fg: Rgb(160,120,80), … }
 ├─ BlocksMovement  (marker)
 ├─ Hostile         (marker)
 ├─ Faction::Wildlife
 ├─ Health { current: 4, max: 4 }
 ├─ CombatStats { attack: 2 }
 ├─ Speed(110)
 ├─ Energy(0)
 ├─ AiState::Idle
 ├─ LootTable { drop_chance: 0.25 }
 ├─ ExpReward(3)
 └─ Viewshed { range: 6 }
```

All hostile entities share the same component bundle, constructed via
`spawn::spawn_monster()`. This ensures consistent spawning across
initial placement and wave spawning.

### Item

```text
Entity
 ├─ Position { x, y }
 ├─ Item            (marker)
 ├─ Name("Colt Army")
 ├─ Renderable { symbol: "P", fg: …, … }
 └─ ItemKind::Gun { loaded: 6, capacity: 6, caliber: Cal44, attack: 6 }
```

Entities are composed at spawn time by bundling the components they need.
Adding new entity types requires zero changes to existing systems — just
new component combinations.

---

## Resources

| Resource | Purpose |
|---|---|
| `GameMapResource` | Wraps the 2D tile grid (`GameMap`). Kept as a resource because tiles are static spatial data best accessed by coordinate, not by query. |
| `CameraPosition` | Cached viewport center, updated each frame by `camera_follow_system`. |
| `SpatialIndex` | `HashMap<GridVec, Vec<Entity>>` rebuilt every tick by `spatial_index_system`. Provides O(1) entity-at-position queries. Exposes `add_entity`, `remove_entity`, and `move_entity` for atomic inline updates during movement. |
| `MapSeed` | `u64` seed for deterministic procedural generation. Same seed always produces the same world; different seed gives a different world. |
| `CombatLog` | Accumulator for combat messages displayed in the status bar. Cleared after each render frame. |
| `TurnCounter` | `TurnCounter(u32)` — Counts elapsed world turns. Incremented by `end_world_turn`. Used by `wave_spawn_system` to determine when to spawn new waves. |
| `KillCount` | `KillCount(u32)` — Tracks total hostile entities killed. Incremented by `death_system`. Displayed in the status bar as the player's score. |

> **Design note:** Tiles are *not* individual entities. A 120×80 map would
> create 9,600 entities — expensive to query every frame. Storing the grid in
> a resource with O(1) coordinate look-up is the standard ECS roguelike
> approach (used by Bracket-lib, RLTK tutorials, and Cogmind).

---

## Procedural Generation

Map generation uses layered deterministic noise from `noise.rs` (no external
dependencies). The pipeline runs once at startup when `GameMap::new()` is
called with the `MapSeed`.

### Noise Primitives (`noise.rs`)

| Function | Purpose |
|---|---|
| `squirrel3` | Positional hash (Squirrel3, GDC 2017). Maps `(position, seed)` → `u32` with full avalanche. |
| `value_noise` | 2D lattice noise in [0, 1) from hashed grid points. |
| `smooth_noise` | Bilinear interpolation with smoothstep (3t² − 2t³) over four lattice corners. Eliminates grid-axis artifacts. |
| `fbm` | Fractal Brownian Motion — sums `n` octaves of `smooth_noise` with lacunarity 2 and configurable persistence. Produces 1/f self-similar patterns. |

### Generation Layers

```text
  Biome layer (fBm, freq 0.03) ──▶ dominant terrain region
  Detail layer (fBm, freq 0.10) ──▶ local floor variation within biome
  Tree layer   (fBm, freq 0.05) ──▶ forest cluster density
  Undergrowth  (fBm, freq 0.08) ──▶ bush / rock scatter
```

Each layer uses a decorrelated seed offset so layers are statistically independent.

### Floor Selection

The biome value (0–1) partitions the map into four terrain bands:

| Biome range | Dominant terrain |
|---|---|
| 0.00 – 0.30 | Sandy / gravelly (Sand, Gravel, Dirt) |
| 0.30 – 0.50 | Transition (Dirt, Grass, Gravel) |
| 0.50 – 0.75 | Forest (Grass, TallGrass, Flowers, Moss, Dirt) |
| 0.75 – 1.00 | Dense forest (Moss, TallGrass, Grass, Flowers) |

The detail noise sub-selects within each band.

### Furniture Placement

- **Border walls** surround the map unconditionally.
- **Spawn clearing** — Euclidean distance < 6 tiles from spawn: no furniture.
  Distance 6–10: smooth density ramp via linear transition factor.
- **Trees** — placed where `tree_fBm > (1 − biome × 0.5 − 0.1) × transition`
  *and* per-tile jitter > 0.3. ~12% chance of `DeadTree` variant.
- **Undergrowth** — bushes and rocks in medium-density pockets where
  `undergrowth_fBm > 0.62` and jitter > 0.6.

### Monster Spawning

Monsters are placed deterministically using noise-based spawn probability:
- ~2% of passable tiles spawn a monster.
- Minimum distance of 12 tiles from player spawn.
- Monster type (Goblin, Orc, Rat) selected by a separate noise layer.
- Each type has distinct stats: health, attack, speed, and sight range.

### Determinism

All noise functions are pure: `f(x, y, seed) → value`. No global state,
no floating-point rounding issues (smoothstep is computed in `f64`), and
no dependency on evaluation order. Two runs with the same `MapSeed`
produce bit-identical maps and monster placements.

---

## States

### GameState (top-level)

| Variant | Effect |
|---|---|
| `Playing` (default) | All gameplay systems run; `TurnState` sub-state is active |
| `Paused` | Action and consequence systems are skipped; draw shows PAUSED overlay |

### TurnState (sub-state of `Playing`)

| Variant | Active Systems | Transition |
|---|---|---|
| `AwaitingInput` (default) | `input_system` accepts movement/spell/action keys | Player presses a key → `PlayerTurn` |
| `PlayerTurn` | movement, spell, combat, visibility, camera | After consequence systems → `WorldTurn` |
| `WorldTurn` | energy accumulation, AI, wave spawning, movement, combat | After AI + wave systems → `AwaitingInput` |

```text
┌──────────────┐  key press  ┌────────────┐  end_player_turn  ┌───────────┐  end_world_turn
│AwaitingInput │────────────▶│ PlayerTurn │──────────────────▶│ WorldTurn │──────────────────▶ ↻
└──────────────┘             └────────────┘                   └───────────┘
```

States use Bevy's `States`/`SubStates` derive macros and `in_state()` run conditions.
The input system always runs so the player can unpause or quit.

---

## Events (Messages)

| Message | Fields | Emitted by | Consumed by |
|---|---|---|---|
| `MoveIntent` | `entity, dx, dy` | `input_system`, `ai_system` | `movement_system` |
| `AttackIntent` | `attacker, target` | `movement_system` (bump-to-attack) | `combat_system` |
| `DamageEvent` | `target, amount` | `combat_system`, `spell_system`, `projectile_system`, `throw_system` | `apply_damage_system` |
| `SpellCastIntent` | `caster, radius, target, grenade_index` | `input_system` | `spell_system` |
| `RangedAttackIntent` | `attacker, range, dx, dy, gun_item` | `input_system` | `ranged_attack_system` |
| `AiRangedAttackIntent` | `attacker, target, range` | `ai_system` | `ai_ranged_attack_system` |
| `MeleeWideIntent` | `attacker` | `input_system` | `melee_wide_system` |
| `PickupItemIntent` | `picker` | `input_system` | `pickup_system` |
| `UseItemIntent` | `user, item_index` | `input_system` | `use_item_system` |
| `DropItemIntent` | `user, item_index` | `input_system` | `drop_item_system` |
| `ThrowItemIntent` | `thrower, item_entity, item_index, dx, dy, range, damage` | `input_system` | `throw_system` |

Events decouple intent from physics. AI systems emit the same `MoveIntent`
as the player input system — the resolution pipeline is completely shared.

### Bump-to-Attack Flow

```text
Player presses 'd' (move right)
  → input_system emits MoveIntent { entity: player, dx: 1, dy: 0 }
  → movement_system detects Hostile entity at target tile
  → movement_system emits AttackIntent { attacker: player, target: goblin }
  → combat_system resolves damage = max(0, attack)
  → combat_system emits DamageEvent { target: goblin, amount: 3 }
  → apply_damage_system reduces goblin health
  → death_system despawns goblin if health ≤ 0
```

### AoE Spell Flow

```text
Player presses 'f' or Space (cast spell)
  → input_system emits SpellCastIntent { caster: player, radius: 5 }
  → spell_system finds all Hostile entities within Chebyshev radius
  → spell_system emits DamageEvent { target: goblin, amount: 5 } for each
  → apply_damage_system reduces each target's health
  → death_system despawns killed hostiles, increments KillCount
```

---

## System Sets & Schedule

### RoguelikeSet Pipeline

```text
  Index → Action → Consequence → Render
```

| Set | Purpose | State gate |
|---|---|---|
| `Index` | Rebuild `SpatialIndex` | Always |
| `Action` | Process intents (movement, combat, damage, death) | `GameState::Playing` |
| `Consequence` | Recalculate FOV, update camera | `GameState::Playing` |
| `Render` | Draw frame to terminal | Always |

### Full System Layout

```text
PreUpdate
  └─ input_system            reads KeyMessage → emits MoveIntent or SpellCastIntent / toggles states

Update
  ├─ [Index]
  │   └─ spatial_index_system    rebuilds HashMap<GridVec, Vec<Entity>>
  │
  ├─ [Action]  (gated on GameState::Playing)
  │   ├─ movement_system         reads MoveIntent → collision (map + spatial) → bump-to-attack or mutate Position
  │   ├─ spell_system            reads SpellCastIntent → finds Hostile in radius → emits DamageEvent for each
  │   ├─ combat_system           reads AttackIntent → computes damage → emits DamageEvent → logs messages
  │   ├─ apply_damage_system     reads DamageEvent → mutates Health
  │   └─ death_system            despawns entities with health ≤ 0, increments KillCount for Hostile kills
  │
  ├─ [Consequence]  (gated on GameState::Playing)
  │   ├─ visibility_system       symmetric shadowcasting → updates visible_tiles + revealed_tiles
  │   └─ camera_follow_system    copies Position+CameraFollow → CameraPosition
  │
  ├─ end_player_turn             (gated on TurnState::PlayerTurn) → transitions to WorldTurn
  │
  ├─ [WorldTurn phase]  (gated on TurnState::WorldTurn)
  │   ├─ energy_accumulate_system  energy += speed for all actors
  │   ├─ ai_system                 AI decisions → emits MoveIntent
  │   ├─ wave_spawn_system         spawns enemy waves every 5 turns based on TurnCounter
  │   └─ end_world_turn           increments TurnCounter → transitions to AwaitingInput
  │
  └─ [Render]
      └─ draw_system             renders map + entities + fog-of-war + health + combat log + status bar (turn count, kill count, spell keybind)
```

### System Details

#### `spatial_index_system`
- **Reads:** `Query<(Entity, &Position)>`
- **Writes:** `ResMut<SpatialIndex>`
- Clears and rebuilds the spatial index every tick. O(n) where n = entity count.

#### `input_system`
- **Reads:** `MessageReader<KeyMessage>`, `Res<State<GameState>>`, `Res<State<TurnState>>`
- **Writes:** `MessageWriter<MoveIntent>`, `MessageWriter<SpellCastIntent>`, `MessageWriter<AppExit>`, `ResMut<NextState<GameState>>`, `ResMut<NextState<TurnState>>`
- Translates key presses into `MoveIntent` or `SpellCastIntent` events (only when `AwaitingInput`).
  Movement keys: WASD. Spell keys: F/Space (emits `SpellCastIntent`).
  Always handles quit (`q`/`Esc`) and pause toggle (`p`).
  Transitions to `PlayerTurn` when a movement or spell key is accepted.

#### `movement_system`
- **Reads:** `MessageReader<MoveIntent>`, `Res<GameMapResource>`, `Query<(), With<BlocksMovement>>`, `Query<(), With<Hostile>>`
- **Writes:** `ResMut<SpatialIndex>`, `MessageWriter<AttackIntent>`, `Query<(&mut Position, Option<&mut Viewshed>)>`
- For each `MoveIntent`:
  1. **Bump-to-attack**: if a `Hostile` entity occupies the target, emit `AttackIntent`.
  2. Check map tile walkability (no blocking furniture).
  3. Check spatial index for entities with `BlocksMovement` at the target.
- Updates `Position` only if both checks pass. Marks `Viewshed` dirty.
- **Spatial index atomicity**: after each successful move, calls `SpatialIndex::move_entity()` to atomically update the index so subsequent intents in the same frame see accurate occupancy data.

#### `combat_system`
- **Reads:** `MessageReader<AttackIntent>`, `Query<(&CombatStats, Option<&Name>)>`
- **Writes:** `MessageWriter<DamageEvent>`, `ResMut<CombatLog>`
- Resolves damage = max(0, attacker.attack).
- Logs combat messages to `CombatLog`.

#### `ranged_attack_system` (Bresenham Line-of-Sight)
- **Reads:** `MessageReader<RangedAttackIntent>`, `Query<(&Position, &CombatStats)>`, `Query<(Entity, &Position, &CombatStats), With<Hostile>>`, `Res<GameMapResource>`
- **Writes:** `MessageWriter<DamageEvent>`, `ResMut<CombatLog>`, `ResMut<SpellParticles>`
- Computes the bullet trajectory using **Bresenham's line algorithm** from the
  caster's position to the maximum range endpoint. The bullet path is the
  mathematically correct sequence of integer grid tiles — no floating-point,
  no directional heuristics. Bullets stop at impassable walls and can penetrate
  through multiple enemies (penetration carries through until exhausted).
- Spawns visual particle effects along the trajectory.

#### `apply_damage_system`
- **Reads:** `MessageReader<DamageEvent>`
- **Writes:** `Query<&mut Health>`
- Subtracts damage from health, clamping at zero.

#### `death_system`
- **Reads:** `Query<(Entity, &Health, Option<&Name>, Option<&Hostile>)>`
- **Writes:** `Commands` (despawn), `ResMut<CombatLog>`, `ResMut<KillCount>`
- Despawns entities with `health.current <= 0` and logs death messages.
- When a despawned entity has the `Hostile` component, increments `KillCount`.

#### `spell_system`
- **Reads:** `MessageReader<SpellCastIntent>`, `Query<&Position>`, `Query<(Entity, &Position), With<Hostile>>`, `Query<&CombatStats>`
- **Writes:** `MessageWriter<DamageEvent>`
- For each `SpellCastIntent`, finds all `Hostile` entities within the specified radius
  (Chebyshev distance) of the caster and emits a `DamageEvent` for each.
  Damage equals the caster's attack stat. Runs in the Action set.

#### `wave_spawn_system`
- **Reads:** `Res<TurnCounter>`, `Res<GameMapResource>`, `Res<SpatialIndex>`, `Query<&Position, With<Player>>`
- **Writes:** `Commands` (spawn entities)
- Spawns waves of enemies as turns progress. Every 5 turns, spawns a batch of
  enemies near the player. Batch size grows over time (base 2 + 1 per wave cycle).
  Enemies spawn 10–30 tiles from the player on passable, unoccupied tiles.
  Runs during WorldTurn.

#### `energy_accumulate_system`
- **Reads:** `Query<(&Speed, &mut Energy)>`
- Adds `speed` to `energy` for every actor. Runs during `WorldTurn`.

#### `ai_system`
- **Reads:** `Query<(Entity, &Position, &mut AiState, Option<&Viewshed>, &mut Energy)>`, `Res<GameMapResource>`, `Res<SpatialIndex>`, `Query<(), With<BlocksMovement>>`
- **Writes:** `MessageWriter<MoveIntent>`
- For entities with enough energy (≥ ACTION_COST):
  - **Idle**: check if player is visible → transition to `Chasing`.
  - **Chasing**: use **A\* pathfinding** (Chebyshev heuristic, 256-node budget,
    lexicographic `(f, h)` tie-breaking) to find optimal route around walls and
    blocking entities. Falls back to greedy king-step if no path found.
- Deducts ACTION_COST from energy after acting.

#### `visibility_system`  (Symmetric Shadowcasting)
- **Reads:** `Res<GameMapResource>`
- **Writes:** `Query<(&Position, &mut Viewshed)>`
- For each entity with a dirty `Viewshed`, runs **recursive symmetric
  shadowcasting** (Albert Ford, 2017) in all 8 octants.
- **Guarantees:**
  - **Symmetry**: if tile A is visible from B, then B is visible from A.
  - **Completeness**: no visible tile is missed.
  - **Efficiency**: O(visible tiles) — each tile visited at most once per octant.
- Uses rational slopes (integer y/x) to avoid floating-point rounding.
- Merges `visible_tiles` into `revealed_tiles` for fog-of-war persistence.

#### `camera_follow_system`
- **Reads:** `Query<&Position, With<CameraFollow>>`
- **Writes:** `ResMut<CameraPosition>`
- Copies the followed entity's position into the camera resource.

#### `draw_system`
- **Reads:** `Res<GameMapResource>`, `Res<CameraPosition>`,
  `Query<(&Position, &Renderable)>`, `Query<(&Position, Option<&Viewshed>, Option<&Health>), With<Player>>`,
  `Res<State<GameState>>`, `ResMut<CombatLog>`
- **Writes:** `ResMut<RatatuiContext>`
- Renders the map with three-state fog of war:
  - **Visible tiles**: full brightness.
  - **Revealed tiles** (seen before but not now): dimmed.
  - **Unseen tiles**: solid black.
- Overlays `Renderable` entities (only if currently visible).
- Shows PAUSED overlay, health display, combat log, and status bar.
- Status bar now shows: Turn count (`TurnCounter`), Kill count (`KillCount`), and spell keybind (F/Space: spell).

#### `end_player_turn` / `end_world_turn`
- Advance the `TurnState` state machine after each phase completes.
- `end_world_turn` also increments `TurnCounter` each world turn.

---

## Plugin

`RoguelikePlugin` is the single entry point. It:

1. Adds `StatesPlugin` (required for `MinimalPlugins` setups)
2. Registers all message types (`MoveIntent`, `AttackIntent`, `DamageEvent`, `SpellCastIntent`)
3. Inserts resources (`GameMapResource`, `CameraPosition`, `SpatialIndex`, `CombatLog`, `TurnCounter`, `KillCount`)
4. Initializes `GameState` and `TurnState`
5. Configures `RoguelikeSet` ordering (`Index → Action → Consequence → Render`)
6. Registers startup systems (`spawn_player`, `spawn_monsters`)
7. Registers all gameplay systems with correct set membership and state gating

`main.rs` only needs:
```rust
App::new()
    .add_plugins((MinimalPlugins, RatatuiPlugins::default(), RoguelikePlugin))
    .run();
```

---

## Directory Layout

```
roguelike/src/
├── main.rs              App entry point (minimal — just plugin registration)
├── lib.rs               Module declarations
├── grid_vec.rs          GridVec: algebraic 2D integer vector (Abelian group, distances, rotations, Bresenham, tests)
├── components.rs        All ECS components (Position, Player, Name, Renderable, Viewshed, Health, CombatStats, Speed, Energy, AiState, Hostile, …)
├── events.rs            MoveIntent, AttackIntent, DamageEvent, SpellCastIntent, RangedAttackIntent, …
├── noise.rs             Deterministic noise (Squirrel3 hash, smooth value noise, fBm) for procedural generation
├── resources.rs         GameMapResource, CameraPosition, MapSeed, SpatialIndex, CombatLog, TurnCounter, KillCount, GameState, TurnState
├── plugins.rs           RoguelikePlugin + RoguelikeSet + spawn_player + spawn_monsters
├── systems/
│   ├── mod.rs           Re-exports
│   ├── input.rs         input_system (keyboard handling + intent emission)
│   ├── spatial_index.rs spatial_index_system
│   ├── movement.rs      movement_system (uses SpatialIndex + BlocksMovement + bump-to-attack)
│   ├── combat.rs        combat_system + ranged_attack_system + ai_ranged_attack_system + apply_damage_system + death_system + level_up_system + melee_wide_system
│   ├── ai.rs            ai_system + energy_accumulate_system (A* pathfinding)
│   ├── visibility.rs    visibility_system (recursive symmetric shadowcasting)
│   ├── camera.rs        camera_follow_system
│   ├── spell.rs         spell_system (grenade AoE via shrapnel projectiles)
│   ├── spawn.rs         MonsterTemplate + MONSTER_TEMPLATES + spawn_monster helper (shared by plugins.rs and wave_spawn.rs)
│   ├── projectile.rs    projectile_system + spawn_bullet + spawn_shrapnel
│   ├── inventory.rs     pickup_system + use_item_system + drop_item_system + reload_system + auto_pickup_system + throw_system
│   ├── wave_spawn.rs    wave_spawn_system (escalating enemy wave spawning from gate)
│   ├── turn.rs          end_player_turn + end_world_turn (state transitions + TurnCounter + regen)
│   └── render.rs        draw_system (three-state fog of war + health + combat log + turn/kill counters + inventory UI)
├── gamemap.rs           GameMap struct & noise-based procedural generation
├── voxel.rs             Voxel struct & rendering helpers
├── typeenums.rs         Floor / Furniture enums
├── typedefs.rs          Type aliases (GridVec-based MyPoint), constants, helpers
└── graphic_trait.rs     GraphicElement trait implementations
```

---

## Extensibility Roadmap

Because every game object is an entity with composable components, future
features slot in naturally:

| Feature | Implementation |
|---|---|
| ~~NPCs / Monsters~~ | ✅ Implemented: Goblin, Orc, Rat with distinct stats, AI, and energy-based scheduling |
| ~~Melee combat~~ | ✅ Implemented: bump-to-attack triggers `AttackIntent` → `combat_system` → `DamageEvent` → `death_system` |
| ~~Turn system (NPC AI)~~ | ✅ Implemented: energy-based scheduling + `AiState` (Idle/Chasing) + greedy best-first movement |
| ~~Death / despawn~~ | ✅ Implemented: `death_system` despawns at 0 HP with combat log |
| ~~AoE spell system~~ | ✅ Implemented: `SpellCastIntent` → `spell_system` finds `Hostile` in Chebyshev radius → `DamageEvent` per target |
| ~~Enemy wave spawning~~ | ✅ Implemented: `wave_spawn_system` spawns escalating waves every 5 turns near the player |
| ~~Kill tracking / scoring~~ | ✅ Implemented: `KillCount` resource incremented by `death_system`, displayed in status bar |
| ~~Turn counter~~ | ✅ Implemented: `TurnCounter` resource incremented by `end_world_turn`, displayed in status bar |
| ~~Ranged combat~~ | ✅ Implemented: `RangedAttackIntent` with Bresenham line trajectory, wall obstruction, and bullet penetration mechanics |
| ~~Items & inventory~~ | ✅ Implemented: `Item` marker + `Inventory { items: Vec<Entity> }` + auto-pickup + drop + use + reload |
| ~~Procedural generation~~ | ✅ Implemented via `noise.rs` (Squirrel3 hash + fBm). Insert a custom `MapSeed` resource before `RoguelikePlugin` for different worlds. |
| ~~Pathfinding AI~~ | ✅ Implemented: A* with Chebyshev heuristic (admissible + consistent for 8-connected grids), lexicographic (f, h) tie-breaking for reduced node expansion. 256-node exploration budget with greedy fallback. |

---

## Vampire Survivors Gameplay Design

The roguelike now incorporates Vampire Survivors-style mechanics layered on top of
the existing turn-based ECS architecture:

### Core Loop

```text
AwaitingInput
  ├─ Player moves (WASD)        → MoveIntent  → PlayerTurn
  └─ Player casts spell (F/Space) → SpellCastIntent → PlayerTurn
         ↓
    PlayerTurn resolves actions (movement, combat, spell damage)
         ↓
    WorldTurn: enemies act, new waves may spawn, TurnCounter increments
         ↓
    AwaitingInput (cycle repeats)
```

### Escalating Waves

- **`wave_spawn_system`** checks `TurnCounter` every world turn.
- Every 5 turns, a new wave spawns near the player (10–30 tiles away).
- Batch size grows over time: **base 2 + 1 per wave cycle**, creating escalating pressure.
- Enemies are placed on passable, unoccupied tiles using `GameMapResource` and `SpatialIndex`.

### AoE Spell

- Pressing **F** or **Space** during `AwaitingInput` emits a `SpellCastIntent`.
- `spell_system` finds all `Hostile` entities within the spell's Chebyshev-distance radius.
- Each target receives a `DamageEvent` equal to the caster's attack stat.
- This gives the player a way to deal with surrounding hordes without moving.

### Scoring

- `KillCount` tracks every hostile entity killed (incremented by `death_system`).
- `TurnCounter` tracks elapsed world turns (incremented by `end_world_turn`).
- Both are displayed in the status bar, providing survival-style scoring feedback.

### Arena Design

- The large procedurally generated forest map serves as the arena.
- Each action (move or spell) consumes one turn, during which enemies also act.
- The player must balance movement and spell usage to survive escalating waves.

---

*This document is the single source of truth for the game's ECS design.*
