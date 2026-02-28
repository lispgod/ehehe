# ECS Architecture for Bevy Roguelike (bevy_ratatui)

This document describes the Entity-Component-System (ECS) architecture used by this
roguelike, built with Bevy and rendered through `bevy_ratatui`.

---

## Core Principles

1. **Entities** represent every discrete game object (player, NPCs, items, projectiles).
2. **Components** are plain data attached to entities — no behavior.
3. **Systems** contain all logic; they query for combinations of components.
4. **Resources** hold global/singleton state (the tile map, turn counter, RNG).
5. **Events** decouple intent from execution (e.g., `MoveIntent` → collision check → position update).

---

## Components

| Component | Type | Purpose |
|---|---|---|
| `Position` | `{ x: i32, y: i32 }` | World-grid coordinate for any entity |
| `Player` | marker (unit struct) | Tags the player-controlled entity |
| `Renderable` | `{ symbol: String, fg: Color, bg: Color }` | Visual representation of an entity |
| `CameraFollow` | marker | Tags the entity the camera tracks |
| `BlocksMovement` | marker | Marks an entity as impassable |

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
 ├─ Renderable { symbol: "@", fg: White, bg: transparent }
 └─ CameraFollow    (marker)
```

### Future: NPC / Monster

```text
Entity
 ├─ Position { x, y }
 ├─ Renderable { symbol: "g", fg: Red, … }
 └─ BlocksMovement
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
| `GameMap` | 2D tile grid (floor + furniture). Kept as a resource because tiles are static spatial data best accessed by coordinate, not by query. |
| `CameraPosition` | Cached viewport center, updated each frame by `camera_follow_system`. |

> **Design note:** Tiles are *not* individual entities. A 120×80 map would
> create 9 600 entities — expensive to query every frame. Storing the grid in
> a resource with O(1) coordinate look-up is the standard ECS roguelike
> approach (used by Bracket-lib, RLTK tutorials, and Cogmind).

---

## Events

| Event | Fields | Emitted by | Consumed by |
|---|---|---|---|
| `MoveIntent` | `entity: Entity, dx: i32, dy: i32` | `input_system` | `movement_system` |

Events decouple input from physics. Tomorrow you can add AI systems that
also emit `MoveIntent` without touching the movement code.

---

## Systems & Schedule

```text
PreUpdate
  └─ input_system          reads KeyMessage → emits MoveIntent

Update  (chained)
  ├─ movement_system       reads MoveIntent → collision check → mutates Position
  ├─ camera_follow_system  queries Position + CameraFollow → updates CameraPosition
  └─ draw_system           queries all Renderable + Position → renders frame
```

### System Details

#### `input_system`
- **Reads:** `MessageReader<KeyMessage>`
- **Writes:** `MessageWriter<MoveIntent>`, `MessageWriter<AppExit>`
- Translates key presses into `MoveIntent` events. Does *not* mutate any
  position directly.

#### `movement_system`
- **Reads:** `MessageReader<MoveIntent>`, `Res<GameMap>`
- **Writes:** `Query<&mut Position>`
- For each `MoveIntent`, computes the target tile and checks the `GameMap`
  for walkability (walls, furniture that blocks). Updates `Position` only
  if the tile is passable.

#### `camera_follow_system`
- **Reads:** `Query<&Position, With<CameraFollow>>`
- **Writes:** `ResMut<CameraPosition>`
- Copies the followed entity's position into the camera resource.

#### `draw_system`
- **Reads:** `Res<GameMap>`, `Res<CameraPosition>`,
  `Query<(&Position, &Renderable)>`
- **Writes:** `ResMut<RatatuiContext>`
- Builds a render packet from the map, overlays all `Renderable` entities
  at their screen-relative positions, and draws the frame.

---

## Directory Layout

```
roguelike/src/
├── main.rs            App entry point, plugin registration
├── lib.rs             Module declarations
├── components.rs      All ECS components (Position, Player, Renderable, …)
├── events.rs          MoveIntent and future game events
├── systems/
│   ├── mod.rs         Re-exports
│   ├── input.rs       input_system
│   ├── movement.rs    movement_system
│   ├── camera.rs      camera_follow_system
│   └── render.rs      draw_system
├── plugins.rs         RoguelikePlugin (groups systems + events)
├── gamemap.rs         GameMap resource & tile grid
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
| NPCs / Monsters | Spawn entities with `Position + Renderable + AI + BlocksMovement` |
| Turn system | Add `TurnOrder` resource; gate `MoveIntent` processing on turn state |
| Items & inventory | `Item` marker + `InBackpack(Entity)` component |
| Field of view | `Viewshed` component + `visibility_system` |
| Combat | `CombatStats` component + `AttackIntent` event + `combat_system` |
| Procedural generation | Replace `GameMap::new()` with a generation plugin |

---

*This document is the single source of truth for the game's ECS design.*
