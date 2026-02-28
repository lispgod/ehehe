# ECS Architecture for Bevy Roguelike (bevy_ratatui)

This document describes the Entity-Component-System (ECS) architecture used by this
roguelike, built with Bevy and rendered through `bevy_ratatui`.

---

## Core Principles

1. **Entities** represent every discrete game object (player, NPCs, items, projectiles).
2. **Components** are plain data attached to entities — no behavior.
3. **Systems** contain all logic; they query for combinations of components.
4. **Resources** hold global/singleton state (the tile map, camera position).
5. **Events (Messages)** decouple intent from execution (e.g., `MoveIntent` → collision check → position update).
6. **States** control which systems run each frame (e.g., `Playing` vs `Paused`).

---

## Components

| Component | Type | Purpose |
|---|---|---|
| `Position` | `{ x: i32, y: i32 }` | World-grid coordinate for any entity |
| `Player` | marker (unit struct) | Tags the player-controlled entity |
| `Renderable` | `{ symbol: String, fg: Color, bg: Color }` | Visual representation of an entity |
| `CameraFollow` | marker | Tags the entity the camera tracks |
| `BlocksMovement` | marker | Marks an entity as impassable |
| `Viewshed` | `{ range: i32, visible_tiles: HashSet, dirty: bool }` | Field-of-view data, recomputed when dirty |

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
 └─ Viewshed { range: 15, dirty: true }
```

### Future: NPC / Monster

```text
Entity
 ├─ Position { x, y }
 ├─ Renderable { symbol: "g", fg: Red, … }
 ├─ BlocksMovement
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

> **Design note:** Tiles are *not* individual entities. A 120×80 map would
> create 9,600 entities — expensive to query every frame. Storing the grid in
> a resource with O(1) coordinate look-up is the standard ECS roguelike
> approach (used by Bracket-lib, RLTK tutorials, and Cogmind).

---

## States

| State | Variant | Effect |
|---|---|---|
| `GameState` | `Playing` (default) | Movement, visibility, and camera systems run normally |
| `GameState` | `Paused` | Movement and visibility systems are skipped; draw system shows PAUSED overlay |

States use Bevy's `States` derive macro and `in_state()` run conditions.
The input system always runs so the player can unpause or quit.

---

## Events (Messages)

| Message | Fields | Emitted by | Consumed by |
|---|---|---|---|
| `MoveIntent` | `entity: Entity, dx: i32, dy: i32` | `input_system` | `movement_system` |

Events decouple input from physics. Tomorrow you can add AI systems that
also emit `MoveIntent` without touching the movement code.

---

## Systems & Schedule

```text
PreUpdate
  └─ input_system            reads KeyMessage → emits MoveIntent / toggles GameState

Update  (chained, gated on GameState::Playing)
  ├─ movement_system         reads MoveIntent → collision check → mutates Position, marks Viewshed dirty
  ├─ visibility_system       recalculates Viewshed.visible_tiles via Bresenham ray casting
  └─ camera_follow_system    queries Position + CameraFollow → updates CameraPosition

Update  (always runs, after camera_follow_system)
  └─ draw_system             queries Renderable + Position + Viewshed → renders frame
```

### System Details

#### `input_system`
- **Reads:** `MessageReader<KeyMessage>`, `Res<State<GameState>>`
- **Writes:** `MessageWriter<MoveIntent>`, `MessageWriter<AppExit>`, `ResMut<NextState<GameState>>`
- Translates key presses into `MoveIntent` events (only when `Playing`).
  Always handles quit (`q`/`Esc`) and pause toggle (`p`).

#### `movement_system`
- **Reads:** `MessageReader<MoveIntent>`, `Res<GameMapResource>`
- **Writes:** `Query<(&mut Position, Option<&mut Viewshed>)>`
- For each `MoveIntent`, computes the target tile and checks the `GameMap`
  for walkability (walls, furniture that blocks). Updates `Position` only
  if the tile is passable. Marks the entity's `Viewshed` as dirty.

#### `visibility_system`
- **Reads:** `Res<GameMapResource>`
- **Writes:** `Query<(&Position, &mut Viewshed)>`
- For each entity with a dirty `Viewshed`, casts Bresenham rays from the
  entity's position to the perimeter of its sight range. Tiles with
  furniture (walls, trees) block line-of-sight but are themselves visible.

#### `camera_follow_system`
- **Reads:** `Query<&Position, With<CameraFollow>>`
- **Writes:** `ResMut<CameraPosition>`
- Copies the followed entity's position into the camera resource.

#### `draw_system`
- **Reads:** `Res<GameMapResource>`, `Res<CameraPosition>`,
  `Query<(&Position, &Renderable)>`, `Query<(&Position, Option<&Viewshed>), With<Player>>`,
  `Res<State<GameState>>`
- **Writes:** `ResMut<RatatuiContext>`
- Builds a render packet from the map using per-tile visibility from the
  player's `Viewshed`. Tiles outside the FOV are dimmed. Overlays all
  visible `Renderable` entities and draws the frame. Shows a PAUSED
  overlay when the game is paused.

---

## Plugin

`RoguelikePlugin` is the single entry point. It:

1. Adds `StatesPlugin` (required for `MinimalPlugins` setups)
2. Inserts resources (`GameMapResource`, `CameraPosition`)
3. Initializes `GameState`
4. Registers the `spawn_player` startup system
5. Registers all gameplay systems with correct ordering and state gating

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
├── main.rs            App entry point (minimal — just plugin registration)
├── lib.rs             Module declarations
├── components.rs      All ECS components (Position, Player, Renderable, Viewshed, …)
├── events.rs          MoveIntent and future game events
├── resources.rs       GameMapResource, CameraPosition, GameState
├── plugins.rs         RoguelikePlugin (groups systems + resources + states)
├── systems/
│   ├── mod.rs         Re-exports
│   ├── input.rs       input_system
│   ├── movement.rs    movement_system
│   ├── visibility.rs  visibility_system (Bresenham FOV)
│   ├── camera.rs      camera_follow_system
│   └── render.rs      draw_system
├── gamemap.rs         GameMap struct & tile grid
├── voxel.rs           Voxel struct & rendering helpers
├── typeenums.rs       Floor / Furniture enums
├── typedefs.rs        Type aliases, constants, helpers
└── graphic_trait.rs   GraphicElement trait implementations
```

---

## Extensibility Roadmap

Because every game object is an entity with composable components, future
features slot in naturally:

| Feature | Implementation |
|---|---|
| NPCs / Monsters | Spawn entities with `Position + Renderable + AI + BlocksMovement + Viewshed` |
| Turn system | Add `TurnOrder` resource; gate `MoveIntent` processing on turn state |
| Items & inventory | `Item` marker + `InBackpack(Entity)` component |
| Combat | `CombatStats` component + `AttackIntent` event + `combat_system` |
| Procedural generation | Replace `GameMap::new()` with a generation plugin |
| Fog of war (memory) | Add `revealed_tiles: HashSet` to track explored tiles |

---

*This document is the single source of truth for the game's ECS design.*
