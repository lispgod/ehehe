# Dead Man's Hand

**An 1850s cowboy shootout roguelike**

```
            -*-  DEAD MAN'S HAND  -*-

  You're a cowboy drinking in a saloon
  when bandits raid your town!
```

A turn-based roguelike set in the American frontier. Navigate a
procedurally generated Western town, fight outlaws, vaqueros, lawmen,
and wildlife, and destroy the Outlaw Hideout (Ω) to win.

## Table of Contents

- [Features](#features)
- [Controls](#controls)
- [Special Abilities](#special-abilities)
- [Combat & Weapons](#combat--weapons)
- [Items & Inventory](#items--inventory)
- [Factions](#factions)
- [Enemies](#enemies)
- [Map & World](#map--world)
- [Turn & Energy System](#turn--energy-system)
- [Field of View & Visibility](#field-of-view--visibility)
- [Fire & Explosions](#fire--explosions)
- [Sound Indicators](#sound-indicators)
- [Collectibles & Reloading](#collectibles--reloading)
- [Victory & Death](#victory--death)
- [Building & Running](#building--running)
- [Running Tests](#running-tests)
- [Architecture](#architecture)

## Features

- **Procedural Western town** — 400×280 tile map with desert terrain,
  saloons, stables, sheriff's offices, churches, banks, hotels, jails,
  general stores, and more, all generated from deterministic noise.
- **Cap-and-ball revolvers** — period-accurate .31, .36, .44, .50, .58,
  .577, and .69 caliber firearms with per-gun loaded-round tracking and
  manual reloading.
- **Faction warfare** — Outlaws, Lawmen, Vaqueros, Indians, Civilians,
  Sheriff, and Wildlife fight each other as well as the player. NPCs
  pathfind with A\*, patrol, scavenge items, and flee when wounded.
- **Procedural NPC names** — every humanoid enemy gets a unique 1850s-themed
  name (e.g., "Dusty" Silas Crowley, Ezekiel Boone, Cornelius Shaw).
  Indians get adjective+noun names (e.g., Red Cloud), Vaqueros get
  Spanish names (e.g., Carlos Montoya).
- **Throwable weapons** — knives, tomahawks, dynamite, and molotov cocktails
  with area-of-effect fire spread.
- **Stamina-based special abilities** — dive, war cry, quick draw, dual
  wield, fan shot, throw sand, and throw item abilities, each consuming
  stamina that regenerates over time.
- **Energy-based turn scheduling** — faster entities act more often; excess
  energy carries over for exact long-run fairness.
- **Directional field-of-view** — both the player and enemies have
  cone-based vision tied to facing direction. Aiming narrows the cone
  for long-range tunnel vision.
- **Sound indicators** — off-screen audible events (gunshots, explosions)
  appear as yellow `!` on the map in fog-of-war areas.
- **Combat log filtering** — only events visible to the player are shown,
  with projectile ownership attribution (e.g., "Silas Crowley's bullet
  hits Player").
- **Fire propagation** — molotovs and explosions ignite flammable furniture;
  fire spreads to adjacent objects and burns out over time.
- **Collectible supply system** — percussion caps, black powder, lead bullets
  in multiple calibers, bandages, and dollars.
- **Blood trail system** — entities leave blood stains when wounded that
  fade from bright red to dark red over time.
- **NPC AI personalities** — each NPC has procedurally generated aggression,
  courage, and preferred engagement range values that affect their
  combat behavior.

## Controls

All commands are shown in the command bar at the bottom of the screen.

| Key            | Action                              | Cost         |
| -------------- | ----------------------------------- | ------------ |
| W A S D        | Move                                | 3 ticks      |
| I J K L        | Aim cursor                          | 1 tick       |
| C              | Center cursor on player             | 1 tick       |
| V              | Auto-aim toward nearest enemy       | 1 tick       |
| Space / 1-6    | Fire gun / use item by slot         | 2 ticks      |
| R              | Reload gun                          | 6 ticks      |
| F              | Roundhouse kick (AoE melee)         | 2 ticks      |
| G              | Pick up item                        | 1 tick       |
| T              | Wait / skip turn                    | 1 tick       |
| Q              | Pause menu                          | —            |

## Special Abilities

All special abilities consume stamina. Player stamina is 100 and
regenerates +2 per turn.

| Key | Ability      | Stamina | Description                                        |
| --- | ------------ | ------- | -------------------------------------------------- |
| Z   | Dive         | 20      | Move 3 tiles instantly toward the cursor            |
| X   | War Cry      | 25      | Create a large smoke cloud around you               |
| B   | Quick Draw   | 15      | Instantly fire first loaded gun (0 extra ticks)     |
| 7   | Dual Wield   | 15      | Fire two random revolvers simultaneously            |
| 8   | Fan Shot     | 20      | Fire all loaded rounds from a random revolver       |
| 9   | Throw Sand   | 5       | Create a small sand cloud blocking vision           |
| 0   | Throw Item   | 10      | Throw a random inventory item at enemies            |

### Ability Details

- **Dive (Z)**: Dash 3 tiles in the direction of the cursor. Great for
  closing distance or escaping danger.
- **War Cry (X)**: Creates a radius-3 smoke cloud centered on you, blocking
  enemy vision and giving you time to reposition.
- **Quick Draw (B)**: Lightning-fast draw — fires your first loaded gun
  with zero extra tick cost. Useful when enemies are closing in.
- **Dual Wield (7)**: Fire two revolvers at once. Requires 2+ loaded
  revolvers in inventory.
- **Fan Shot (8)**: Empty all remaining rounds from a revolver in a
  single burst. High damage but uses all ammo.
- **Throw Sand (9)**: Toss a handful of sand to create a vision-blocking
  cloud 2 tiles ahead in the cursor direction.
- **Throw Item (0)**: Hurl a random inventory item at the cursor target.
  Damage varies by item type.

## Combat & Weapons

### Bullet Mechanics

- **Travel speed**: 12 tiles per tick
- **Hit chance**: 98% − distance × 2% (minimum 35%)
- **Headshot chance**: 2% base, 10% at very close range
- **Headshot damage**: Target's max HP (instant kill)
- **Misfire chance**: 5% (ammo wasted, no bullet fired)
- **Damage**: Equal to caliber value (.31 = 31 damage, .44 = 44 damage)

### Weapon Types

**Revolvers** (symbol: `p`)
| Weapon                   | Caliber | Capacity | Damage |
| ------------------------ | ------- | -------- | ------ |
| Colt Pocket              | .31     | 5        | 31     |
| Colt Sheriff             | .36     | 5        | 36     |
| Savage 1856              | .36     | 6        | 36     |
| Colt Army                | .44     | 6        | 44     |
| Remington New Model Army | .44     | 6        | 44     |
| Starr 1858 DA            | .44     | 6        | 44     |
| Adams Revolver           | .44     | 5        | 44     |

**Rifles** (symbol: `r`)
| Weapon            | Caliber | Capacity | Damage |
| ----------------- | ------- | -------- | ------ |
| Hawken Rifle      | .50     | 1        | 50     |
| Springfield 1855  | .58     | 1        | 58     |
| Enfield 1853      | .577    | 1        | 57     |
| Springfield 1842  | .69     | 1        | 69     |

**Melee & Thrown Weapons**
| Weapon     | Damage | Notes                     |
| ---------- | ------ | ------------------------- |
| Bowie Knife| varies | Throwable, 12-tile range   |
| Tomahawk   | varies | Throwable, 12-tile range   |
| Bow        | varies | Used by Indian NPCs        |

**Explosives**
| Item              | Damage | Radius | Notes                                   |
| ----------------- | ------ | ------ | --------------------------------------- |
| Dynamite Stick    | 8      | 2      | Destroys props, 10 stamina to throw     |
| Molotov Cocktail  | 6      | 4      | Sets area on fire, 10 stamina to throw  |

### Melee Combat

- **Roundhouse Kick (F)**: Hits all adjacent hostile entities for your
  base attack damage. Costs 2 ticks.
- **Blunt damage**: All items deal blunt damage when thrown (2-6 HP).

## Items & Inventory

You can carry up to **6 items** in your inventory, accessed with keys 1-6.

**Starting Equipment:**
- Colt Pocket (.31 cal, 5 rounds loaded)
- Bowie Knife
- Whiskey Bottle (heals 10 HP)
- Molotov Cocktail

**Consumables:**
| Item    | Effect               |
| ------- | -------------------- |
| Whiskey | Heals 10 HP          |

**Picking Up Items:**
- Press **G** to pick up items on the ground
- Items dropped by killed enemies can be scavenged
- NPCs also scavenge items when patrolling

## Factions

All human NPCs display as `@` with faction-specific colors.
Only the player is white `@`.

| Faction    | Color         | Notes                              |
| ---------- | ------------- | ---------------------------------- |
| Player     | White         | You                                |
| Outlaws    | Orange-red    | Bandits and gunslingers            |
| Vaqueros   | Lime green    | Mexican cowboys                    |
| Lawmen     | Sky blue      | Cowboys enforcing the law          |
| Civilians  | Light purple  | Unarmed townsfolk                  |
| Indians    | Warm brown    | Native warriors with bows          |
| Sheriff    | Gold          | Sheriff and deputies               |
| Wildlife   | Varies        | Animals (unique symbols: c, s)     |

### Faction Alliances

Factions that won't fight each other:
- **Wildlife** ↔ **Indians**
- **Outlaws** ↔ **Vaqueros**
- **Lawmen** ↔ **Civilians** ↔ **Sheriff**

All other faction pairs are hostile. The player is hostile to everyone.

## Enemies

| Enemy          | HP  | ATK | Speed | Sight | Faction   | Weapon      |
| -------------- | --- | --- | ----- | ----- | --------- | ----------- |
| Coyote         | 100 | 2   | 50    | 6     | Wildlife  | Claws       |
| Rattlesnake    | 100 | 3   | 20    | 8     | Wildlife  | Fangs       |
| Outlaw         | 100 | 4   | 34    | 8     | Outlaws   | Melee       |
| Vaquero        | 100 | 5   | 32    | 10    | Vaqueros  | Melee       |
| Cowboy         | 100 | 6   | 30    | 12    | Lawmen    | Gun         |
| Gunslinger     | 100 | 8   | 38    | 14    | Outlaws   | Gun         |
| Civilian       | 60  | 2   | 28    | 8     | Civilians | Melee       |
| Indian Brave   | 120 | 5   | 40    | 12    | Indians   | Bow         |
| Indian Scout   | 80  | 4   | 45    | 14    | Indians   | Bow         |
| Sheriff        | 150 | 8   | 32    | 14    | Sheriff   | Gun         |
| Deputy         | 100 | 6   | 30    | 12    | Sheriff   | Gun         |

### NPC Behavior

- **Patrolling**: Wander around spawn point within 8-tile radius
- **Chasing**: Pursue visible enemies using A\* pathfinding
- **Fleeing**: Retreat when health drops below 30% (modified by courage)
- **Healing**: Use whiskey items when HP < 50%
- **Hazard avoidance**: NPCs avoid fire, smoke clouds, and cacti
- **Friendly fire prevention**: NPCs won't shoot through allies
- **Memory**: Remember last enemy position for 15 turns after losing sight

### NPC Personalities

Each NPC has procedurally generated personality traits:
- **Aggression** (0.3–0.9): Higher = more likely to pursue enemies
- **Courage** (0.2–0.9): Higher = stays in fights at lower HP
- **Preferred Range**: Melee NPCs close to 1 tile; ranged NPCs kite at 5-11 tiles

Some humanoid NPCs carry throwable items (dynamite or molotov cocktails)
that they will use in combat.

## Map & World

The game takes place on a **400×280 tile** procedurally generated Western town.

### Terrain Types

| Floor        | Symbol/Color                | Notes                      |
| ------------ | --------------------------- | -------------------------- |
| Sand         | `.` tan                     | Desert base terrain        |
| Dirt         | `.` brown                   | Roads and paths            |
| Gravel       | `.` gray                    | Rocky ground               |
| Grass        | `.` green                   | Parks and edges            |
| Tall Grass   | `"` green                   | Dense vegetation           |
| Wood Planks  | `.` warm brown              | Building interiors         |
| Fire         | `^` orange                  | Burns entities (2 dmg/turn)|
| Scorched Earth| `.` dark                   | Burned-out fire            |
| Sand Cloud   | `*` tan                     | Blocks vision, 8-turn life |
| Water        | `~` blue                    | Spilled from water troughs |

### Props (Obstacles)

| Prop         | Symbol | Blocks Move | Blocks Vision | Flammable |
| ------------ | ------ | ----------- | ------------- | --------- |
| Wall         | `#`    | Yes         | Yes           | Yes       |
| Tree         | `T`    | Yes         | Yes           | Yes       |
| Bush         | `%`    | Yes         | No            | Yes       |
| Rock         | `o`    | Yes         | Yes           | No        |
| Dead Tree    | `t`    | Yes         | Yes           | Yes       |
| Cactus       | `Y`    | Yes         | Yes           | No        |
| Barrel       | `0`    | Yes         | Yes           | Yes       |
| Crate        | `B`    | Yes         | Yes           | Yes       |
| Bench        | `H`    | Yes         | No            | Yes       |
| Table        | `n`    | Yes         | Yes           | Yes       |
| Chair        | `h`    | Yes         | No            | Yes       |
| Piano        | `M`    | Yes         | Yes           | Yes       |
| Sign         | `]`    | Yes         | No            | Yes       |
| Hay Bale     | `&`    | Yes         | No            | Yes       |
| Fence        | `=`    | No          | No            | Yes       |
| Water Trough | `~`    | No          | No            | No        |
| Hitching Post| `|`    | Yes         | Yes           | No        |

### Town Generation

1. **Desert base terrain** — noise-driven arid terrain (sand, dirt, gravel)
2. **Street grid** — horizontal avenues (7 tiles wide, spaced 24 apart) and
   vertical cross streets (5 tiles wide, spaced 22 apart)
3. **Buildings** — procedurally placed in city blocks: houses, saloons,
   stables, general stores, sheriff's offices, post offices, churches,
   banks, hotels, jails, undertakers, blacksmiths (12 types total)
4. **Landmarks** — Town Hall (18×12) and Grand Saloon (20×14)
5. **Parks** — 5-8 green areas with trees, benches, and water troughs
6. **Street props** — hitching posts, benches, barrels, water troughs,
   signs, crates placed along sidewalks
7. **Desert decorations** — cacti, dead trees, rocks, bushes in open areas
8. **Spawn clearing** — 6-tile radius around the player's starting position

## Turn & Energy System

The game uses an energy-based discrete turn scheduler:

- Each entity has a **speed** stat (player = 100)
- Each world tick, entities gain energy equal to their speed
- When energy ≥ 100, the entity can take an action (energy −= 100)
- Excess energy carries over for mathematically fair scheduling

### Action Costs

| Action          | World Ticks |
| --------------- | ----------- |
| Move (WASD)     | 3           |
| Aim cursor      | 1           |
| Fire gun        | 2           |
| Reload          | 6           |
| Kick            | 2           |
| Pick up item    | 1           |
| Wait            | 1           |

### Regeneration

- **Stamina**: +2 per world turn
- **Health**: +1 per 30 world turns (very slow natural healing)

## Field of View & Visibility

The game uses **recursive symmetric shadowcasting** for FOV calculation.

### Player Vision

- **Centered cursor**: Full 360° circle with 36-tile range
- **Aimed cursor**: Narrowing cone toward the cursor direction
  - Range increases with distance (up to 120 tiles)
  - Cone narrows with distance (tunnel vision effect)
  - Adjacent tiles are always visible (keyhole effect)

### Fog of War

Tiles have three states:
- **Visible**: Full brightness (currently in FOV)
- **Revealed**: Dimmed (previously seen but not in current FOV)
- **Unseen**: Solid black (never explored)

### Enemy FOV

- Hostile NPC vision cones are tinted red on the map
- Wildlife FOV is not shown (they have very short range)
- NPCs have 40-55° directional cones

## Fire & Explosions

### Fire Spread

- Molotov cocktails and dynamite create fire on impact
- Fire spreads to adjacent **flammable** props every 4 turns
- **2 damage per turn** to entities standing on fire
- Fire burns out after **20 world turns**, leaving scorched earth
- Fire destroys flammable props (tables, chairs, walls, etc.)

### Smoke

- Active fire generates smoke (~30% chance per tick)
- Smoke **blocks line of sight** for 8 turns
- Smoke drifts slowly in random directions
- NPCs avoid walking through smoke

## Sound Indicators

When combat occurs outside your field of view:
- A yellow **`!`** appears at the event location
- Only shown for events within **20 tiles** of the player
- Only shown on previously revealed (explored) tiles
- Persists for **3 world turns**

This lets you track distant gunfights and explosions.

## Collectibles & Reloading

Collectibles are separate from your inventory and don't take up slots.

**Starting Supplies:**
- 10 percussion caps
- 10 black powder
- 10 .31 caliber bullets

**Reload Cost (per round):**
- 1 bullet (matching caliber)
- 1 percussion cap
- 1 black powder

Collectibles are dropped by killed enemies (50% chance) and displayed
in the stats panel.

## Victory & Death

### Victory

Destroy the **Outlaw Hideout** (marked as **Ω** on the map) to win.

### Death

When your HP reaches 0:
- Press **T** to spectate the remaining battle
- Press **Q** to quit
- Press **R** to restart with a new map

## Building & Running

Requires **Rust 1.85+** (nightly features are used).

```bash
cd roguelike
cargo run --release
```

The game renders in your terminal using [ratatui](https://ratatui.rs/) and
runs on any platform with a compatible terminal emulator.

## Running Tests

```bash
cd roguelike
cargo test
```

## Architecture

The game is built on [Bevy ECS](https://bevyengine.org/) with a custom
terminal renderer via `bevy_ratatui`. See
[`roguelike/ECS_ARCHITECTURE.md`](roguelike/ECS_ARCHITECTURE.md) for a
detailed breakdown of systems, components, resources, and data flow.

## License

This project is provided as-is for educational and entertainment purposes.
