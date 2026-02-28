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

Additionally, `GridVec` supports scalar multiplication (ℤ-module structure) and three
distance metrics:

| Metric | Formula | Use case |
|---|---|---|
| Manhattan (L₁) | `\|Δx\| + \|Δy\|` | 4-connected grid distance |
| Chebyshev (L∞) | `max(\|Δx\|, \|Δy\|)` | 8-connected (king move) distance |
| Squared Euclidean | `Δx² + Δy²` | Comparison without sqrt (monotonic) |

All group axioms are verified by unit tests.

### Energy-Based Turn Scheduling

The turn system uses a **discrete-event energy scheduler**, the mathematically correct
algorithm for multi-actor turn ordering in roguelikes (used by Angband, DCSS, Cogmind):

```
Each world tick:
  for each actor:
    energy += speed

  for each actor where energy ≥ ACTION_COST:
    perform action
    energy -= ACTION_COST
```

**Properties:**
- **Exact fairness**: over N ticks, an entity with speed S takes exactly ⌊N × S / ACTION_COST⌋ actions.
- **Integer-only**: no floating-point, no rounding errors.
- **Deterministic**: same inputs → same scheduling order.
- **Speed ratios**: Speed(100) = 1 action/tick; Speed(50) = 1 action per 2 ticks; Speed(200) = 2 actions/tick.

### Symmetric Shadowcasting

Field-of-view uses recursive symmetric shadowcasting (Albert Ford, 2017) with
rational slopes (integer y/x) to avoid floating-point:

- **Symmetry**: A sees B ⟺ B sees A.
- **Completeness**: no visible tile is missed.
- **Efficiency**: O(visible tiles) — each tile visited at most once per octant.

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
| `CombatStats` | `{ attack: i32, defense: i32 }` | Offensive/defensive power for combat resolution |
| `Speed` | `Speed(i32)` | Energy gained per world tick (100 = normal) |
| `Energy` | `Energy(i32)` | Accumulated action points; act when ≥ ACTION_COST |
| `AiState` | `Idle \| Chasing` | AI behaviour state for non-player entities |

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
 ├─ CombatStats { attack: 5, defense: 2 }
 ├─ Speed(100)      (normal speed)
 ├─ Energy(0)
 └─ Viewshed { range: 15, dirty: true }
```

### Monster (Goblin / Orc / Rat)

```text
Entity
 ├─ Position { x, y }
 ├─ Name("Goblin")
 ├─ Renderable { symbol: "g", fg: Green, … }
 ├─ BlocksMovement  (marker)
 ├─ Hostile         (marker)
 ├─ Health { current: 8, max: 8 }
 ├─ CombatStats { attack: 3, defense: 1 }
 ├─ Speed(80)       (slower than player)
 ├─ Energy(0)
 ├─ AiState::Idle
 └─ Viewshed { range: 8 }
```

### Future: Item

```text
Entity
 ├─ Position { x, y }
 └─ Renderable { symbol: "!", fg: Yellow, … }
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
| `SpatialIndex` | `HashMap<GridVec, Vec<Entity>>` rebuilt every tick by `spatial_index_system`. Provides O(1) entity-at-position queries used by movement and combat. |
| `MapSeed` | `u64` seed for deterministic procedural generation. Same seed always produces the same world; different seed gives a different world. |
| `CombatLog` | Accumulator for combat messages displayed in the status bar. Cleared after each render frame. |

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
- Each type has distinct stats: health, attack, defense, speed, and sight range.

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
| `AwaitingInput` (default) | `input_system` accepts movement/action keys | Player presses a key → `PlayerTurn` |
| `PlayerTurn` | movement, combat, visibility, camera | After consequence systems → `WorldTurn` |
| `WorldTurn` | energy accumulation, AI, movement, combat | After AI systems → `AwaitingInput` |

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
| `DamageEvent` | `target, amount` | `combat_system` | `apply_damage_system` |

Events decouple intent from physics. AI systems emit the same `MoveIntent`
as the player input system — the resolution pipeline is completely shared.

### Bump-to-Attack Flow

```text
Player presses 'd' (move right)
  → input_system emits MoveIntent { entity: player, dx: 1, dy: 0 }
  → movement_system detects Hostile entity at target tile
  → movement_system emits AttackIntent { attacker: player, target: goblin }
  → combat_system resolves damage = max(0, attack - defense)
  → combat_system emits DamageEvent { target: goblin, amount: 3 }
  → apply_damage_system reduces goblin health
  → death_system despawns goblin if health ≤ 0
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
  └─ input_system            reads KeyMessage → emits MoveIntent / toggles states

Update
  ├─ [Index]
  │   └─ spatial_index_system    rebuilds HashMap<GridVec, Vec<Entity>>
  │
  ├─ [Action]  (gated on GameState::Playing)
  │   ├─ movement_system         reads MoveIntent → collision (map + spatial) → bump-to-attack or mutate Position
  │   ├─ combat_system           reads AttackIntent → computes damage → emits DamageEvent → logs messages
  │   ├─ apply_damage_system     reads DamageEvent → mutates Health
  │   └─ death_system            despawns entities with health ≤ 0
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
  │   └─ end_world_turn           → transitions to AwaitingInput
  │
  └─ [Render]
      └─ draw_system             renders map + entities + fog-of-war + health + combat log + status bar
```

### System Details

#### `spatial_index_system`
- **Reads:** `Query<(Entity, &Position)>`
- **Writes:** `ResMut<SpatialIndex>`
- Clears and rebuilds the spatial index every tick. O(n) where n = entity count.

#### `input_system`
- **Reads:** `MessageReader<KeyMessage>`, `Res<State<GameState>>`, `Res<State<TurnState>>`
- **Writes:** `MessageWriter<MoveIntent>`, `MessageWriter<AppExit>`, `ResMut<NextState<GameState>>`, `ResMut<NextState<TurnState>>`
- Translates key presses into `MoveIntent` events (only when `AwaitingInput`).
  Always handles quit (`q`/`Esc`) and pause toggle (`p`).
  Transitions to `PlayerTurn` when a movement key is accepted.

#### `movement_system`
- **Reads:** `MessageReader<MoveIntent>`, `Res<GameMapResource>`, `Res<SpatialIndex>`, `Query<(), With<BlocksMovement>>`, `Query<(), With<Hostile>>`
- **Writes:** `MessageWriter<AttackIntent>`, `Query<(&mut Position, Option<&mut Viewshed>)>`
- For each `MoveIntent`:
  1. **Bump-to-attack**: if a `Hostile` entity occupies the target, emit `AttackIntent`.
  2. Check map tile walkability (no blocking furniture).
  3. Check spatial index for entities with `BlocksMovement` at the target.
- Updates `Position` only if both checks pass. Marks `Viewshed` dirty.

#### `combat_system`
- **Reads:** `MessageReader<AttackIntent>`, `Query<(&CombatStats, Option<&Name>)>`
- **Writes:** `MessageWriter<DamageEvent>`, `ResMut<CombatLog>`
- Resolves damage = max(0, attacker.attack − target.defense).
- Logs combat messages to `CombatLog`.

#### `apply_damage_system`
- **Reads:** `MessageReader<DamageEvent>`
- **Writes:** `Query<&mut Health>`
- Subtracts damage from health, clamping at zero.

#### `death_system`
- **Reads:** `Query<(Entity, &Health, Option<&Name>)>`
- **Writes:** `Commands` (despawn), `ResMut<CombatLog>`
- Despawns entities with `health.current <= 0` and logs death messages.

#### `energy_accumulate_system`
- **Reads:** `Query<(&Speed, &mut Energy)>`
- Adds `speed` to `energy` for every actor. Runs during `WorldTurn`.

#### `ai_system`
- **Reads:** `Query<(Entity, &Position, &mut AiState, Option<&Viewshed>, &mut Energy)>`
- **Writes:** `MessageWriter<MoveIntent>`
- For entities with enough energy (≥ ACTION_COST):
  - **Idle**: check if player is visible → transition to `Chasing`.
  - **Chasing**: emit `MoveIntent` toward player (greedy best-first, minimise Chebyshev distance).
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

#### `end_player_turn` / `end_world_turn`
- Advance the `TurnState` state machine after each phase completes.

---

## Plugin

`RoguelikePlugin` is the single entry point. It:

1. Adds `StatesPlugin` (required for `MinimalPlugins` setups)
2. Registers all message types (`MoveIntent`, `AttackIntent`, `DamageEvent`)
3. Inserts resources (`GameMapResource`, `CameraPosition`, `SpatialIndex`, `CombatLog`)
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
├── grid_vec.rs          GridVec: algebraic 2D integer vector (Abelian group, distances, tests)
├── components.rs        All ECS components (Position, Player, Name, Renderable, Viewshed, Health, CombatStats, Speed, Energy, AiState, Hostile, …)
├── events.rs            MoveIntent, AttackIntent, DamageEvent
├── noise.rs             Deterministic noise (Squirrel3 hash, smooth value noise, fBm) for procedural generation
├── resources.rs         GameMapResource, CameraPosition, MapSeed, SpatialIndex, CombatLog, GameState, TurnState
├── plugins.rs           RoguelikePlugin + RoguelikeSet + spawn_player + spawn_monsters + monster templates
├── systems/
│   ├── mod.rs           Re-exports
│   ├── input.rs         input_system
│   ├── spatial_index.rs spatial_index_system
│   ├── movement.rs      movement_system (uses SpatialIndex + BlocksMovement + bump-to-attack)
│   ├── combat.rs        combat_system + apply_damage_system + death_system
│   ├── ai.rs            ai_system + energy_accumulate_system
│   ├── visibility.rs    visibility_system (recursive symmetric shadowcasting)
│   ├── camera.rs        camera_follow_system
│   ├── turn.rs          end_player_turn + end_world_turn (state transitions)
│   └── render.rs        draw_system (three-state fog of war + health + combat log)
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
| Ranged combat | `RangedAttackIntent { attacker, target_pos }` + line-of-sight check via `SpatialIndex` |
| Items & inventory | `Item` marker + `InBackpack(Entity)` component |
| ~~Procedural generation~~ | ✅ Implemented via `noise.rs` (Squirrel3 hash + fBm). Insert a custom `MapSeed` resource before `RoguelikePlugin` for different worlds. |
| Pathfinding AI | Replace greedy best-first with A* using `GridVec::manhattan_distance` as heuristic |

---

*This document is the single source of truth for the game's ECS design.*
