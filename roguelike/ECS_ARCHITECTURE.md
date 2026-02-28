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

## Components

| Component | Type | Purpose |
|---|---|---|
| `Position` | `{ x: i32, y: i32 }` | World-grid coordinate for any entity |
| `Player` | marker (unit struct) | Tags the player-controlled entity |
| `Renderable` | `{ symbol: String, fg: Color, bg: Color }` | Visual representation of an entity |
| `CameraFollow` | marker | Tags the entity the camera tracks |
| `BlocksMovement` | marker | Marks an entity as impassable (enforced via `SpatialIndex`) |
| `Viewshed` | `{ range, visible_tiles, revealed_tiles, dirty }` | Field-of-view + fog-of-war memory |
| `Health` | `{ current: i32, max: i32 }` | Hit-point pool for damageable entities |
| `CombatStats` | `{ attack: i32, defense: i32 }` | Offensive/defensive power for combat resolution |

### Why markers?

Bevy queries use component presence for filtering. `Player` lets any system
find the player with `Query<&Position, With<Player>>` without coupling to a
concrete "player struct."

---

## Entities

### Player

```text
Entity
 ├─ Position { x, y }
 ├─ Player          (marker)
 ├─ Renderable { symbol: "@", fg: White, bg: Black }
 ├─ CameraFollow    (marker)
 ├─ Health { current: 30, max: 30 }
 ├─ CombatStats { attack: 5, defense: 2 }
 └─ Viewshed { range: 15, dirty: true }
```

### Future: NPC / Monster

```text
Entity
 ├─ Position { x, y }
 ├─ Renderable { symbol: "g", fg: Red, … }
 ├─ BlocksMovement
 ├─ Health { current: 10, max: 10 }
 ├─ CombatStats { attack: 3, defense: 1 }
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
| `SpatialIndex` | `HashMap<(i32,i32), Vec<Entity>>` rebuilt every tick by `spatial_index_system`. Provides O(1) entity-at-position queries used by movement and combat. |

> **Design note:** Tiles are *not* individual entities. A 120×80 map would
> create 9,600 entities — expensive to query every frame. Storing the grid in
> a resource with O(1) coordinate look-up is the standard ECS roguelike
> approach (used by Bracket-lib, RLTK tutorials, and Cogmind).

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
| `WorldTurn` | (future: NPC AI, world tick) | After world systems → `AwaitingInput` |

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
| `MoveIntent` | `entity, dx, dy` | `input_system` (future: AI) | `movement_system` |
| `AttackIntent` | `attacker, target` | (future: input/AI) | `combat_system` |
| `DamageEvent` | `target, amount` | `combat_system` | `apply_damage_system` |

Events decouple intent from physics. Tomorrow you can add AI systems that
also emit `MoveIntent` or `AttackIntent` without touching the resolution code.

---

## System Sets & Schedule

### RoguelikeSet Pipeline

```text
  Index → Action → Consequence → Render
```

| Set | Purpose | State gate |
|---|---|---|
| `Index` | Rebuild `SpatialIndex` | Always |
| `Action` | Process intents (movement, combat, damage) | `GameState::Playing` |
| `Consequence` | Recalculate FOV, update camera | `GameState::Playing` |
| `Render` | Draw frame to terminal | Always |

### Full System Layout

```text
PreUpdate
  └─ input_system            reads KeyMessage → emits MoveIntent / toggles states

Update
  ├─ [Index]
  │   └─ spatial_index_system    rebuilds HashMap<Point, Vec<Entity>>
  │
  ├─ [Action]  (gated on GameState::Playing)
  │   ├─ movement_system         reads MoveIntent → collision (map + spatial) → mutates Position
  │   ├─ combat_system           reads AttackIntent → computes damage → emits DamageEvent
  │   └─ apply_damage_system     reads DamageEvent → mutates Health
  │
  ├─ [Consequence]  (gated on GameState::Playing)
  │   ├─ visibility_system       symmetric shadowcasting → updates visible_tiles + revealed_tiles
  │   └─ camera_follow_system    copies Position+CameraFollow → CameraPosition
  │
  ├─ end_player_turn             (gated on TurnState::PlayerTurn) → transitions to WorldTurn
  ├─ end_world_turn              (gated on TurnState::WorldTurn) → transitions to AwaitingInput
  │
  └─ [Render]
      └─ draw_system             renders map + entities + fog-of-war + status bar
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
- **Reads:** `MessageReader<MoveIntent>`, `Res<GameMapResource>`, `Res<SpatialIndex>`, `Query<(), With<BlocksMovement>>`
- **Writes:** `Query<(&mut Position, Option<&mut Viewshed>)>`
- For each `MoveIntent`, checks:
  1. Map tile walkability (no blocking furniture).
  2. Spatial index for entities with `BlocksMovement` at the target.
- Updates `Position` only if both checks pass. Marks `Viewshed` dirty.

#### `combat_system`
- **Reads:** `MessageReader<AttackIntent>`, `Query<&CombatStats>`
- **Writes:** `MessageWriter<DamageEvent>`
- Resolves damage = max(0, attacker.attack − target.defense).

#### `apply_damage_system`
- **Reads:** `MessageReader<DamageEvent>`
- **Writes:** `Query<&mut Health>`
- Subtracts damage from health, clamping at zero.

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
  `Query<(&Position, &Renderable)>`, `Query<(&Position, Option<&Viewshed>), With<Player>>`,
  `Res<State<GameState>>`
- **Writes:** `ResMut<RatatuiContext>`
- Renders the map with three-state fog of war:
  - **Visible tiles**: full brightness.
  - **Revealed tiles** (seen before but not now): dimmed.
  - **Unseen tiles**: solid black.
- Overlays `Renderable` entities (only if currently visible).
- Shows PAUSED overlay and status bar.

#### `end_player_turn` / `end_world_turn`
- Advance the `TurnState` state machine after each phase completes.

---

## Plugin

`RoguelikePlugin` is the single entry point. It:

1. Adds `StatesPlugin` (required for `MinimalPlugins` setups)
2. Registers all message types (`MoveIntent`, `AttackIntent`, `DamageEvent`)
3. Inserts resources (`GameMapResource`, `CameraPosition`, `SpatialIndex`)
4. Initializes `GameState` and `TurnState`
5. Configures `RoguelikeSet` ordering (`Index → Action → Consequence → Render`)
6. Registers the `spawn_player` startup system
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
├── components.rs        All ECS components (Position, Player, Renderable, Viewshed, Health, CombatStats, …)
├── events.rs            MoveIntent, AttackIntent, DamageEvent
├── resources.rs         GameMapResource, CameraPosition, SpatialIndex, GameState, TurnState
├── plugins.rs           RoguelikePlugin + RoguelikeSet (groups systems + resources + states)
├── systems/
│   ├── mod.rs           Re-exports
│   ├── input.rs         input_system
│   ├── spatial_index.rs spatial_index_system
│   ├── movement.rs      movement_system (uses SpatialIndex + BlocksMovement)
│   ├── combat.rs        combat_system + apply_damage_system
│   ├── visibility.rs    visibility_system (recursive symmetric shadowcasting)
│   ├── camera.rs        camera_follow_system
│   ├── turn.rs          end_player_turn + end_world_turn (state transitions)
│   └── render.rs        draw_system (three-state fog of war)
├── gamemap.rs           GameMap struct & tile grid
├── voxel.rs             Voxel struct & rendering helpers
├── typeenums.rs         Floor / Furniture enums
├── typedefs.rs          Type aliases, constants, helpers
└── graphic_trait.rs     GraphicElement trait implementations
```

---

## Extensibility Roadmap

Because every game object is an entity with composable components, future
features slot in naturally:

| Feature | Implementation |
|---|---|
| NPCs / Monsters | Spawn entities with `Position + Renderable + AI + BlocksMovement + Health + CombatStats + Viewshed` |
| Melee combat | Input system emits `AttackIntent` when moving into a tile occupied by a hostile. `combat_system` already resolves damage. |
| Ranged combat | `RangedAttackIntent { attacker, target_pos }` + line-of-sight check via `SpatialIndex` |
| Turn system (NPC AI) | Add `AiComponent` + `ai_system` gated on `TurnState::WorldTurn`; emit `MoveIntent`/`AttackIntent` |
| Items & inventory | `Item` marker + `InBackpack(Entity)` component |
| Procedural generation | Replace `GameMap::new()` with a generation plugin |
| Death / despawn | System that despawns entities when `health.current <= 0` |

---

*This document is the single source of truth for the game's ECS design.*
