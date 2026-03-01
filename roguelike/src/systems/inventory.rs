use bevy::prelude::*;

use crate::components::{
    Caliber, CollectibleKind, Health, Hostile, Inventory, Item, ItemKind, Name, Player, Position,
    Renderable, Thrown,
};
use crate::events::{DropItemIntent, PickupItemIntent, ThrowItemIntent, UseItemIntent};
use crate::grid_vec::GridVec;
use crate::resources::{Collectibles, CombatLog, GameMapResource, InputState, SpellParticles, SpatialIndex};
use crate::typedefs::RatColor;

/// Processes pickup intents: player picks up an item on the ground at their position.
pub fn pickup_system(
    mut intents: MessageReader<PickupItemIntent>,
    mut commands: Commands,
    player_query: Query<&Position, With<Player>>,
    items_query: Query<(Entity, &Position, Option<&Name>), With<Item>>,
    spatial: Res<SpatialIndex>,
    mut inventory_query: Query<&mut Inventory, With<Player>>,
    mut combat_log: ResMut<CombatLog>,
) {
    for intent in intents.read() {
        let Ok(player_pos) = player_query.get(intent.picker) else {
            continue;
        };
        let player_vec = player_pos.as_grid_vec();

        // Find items at the player's position using the spatial index.
        let entities_here = spatial.entities_at(&player_vec);
        let mut picked_up = false;

        for &ent in entities_here {
            if items_query.get(ent).is_ok() {
                let item_name = items_query
                    .get(ent)
                    .ok()
                    .and_then(|(_, _, n)| n)
                    .map_or("item", |n| n.0.as_str())
                    .to_string();

                // Add to inventory.
                if let Ok(mut inv) = inventory_query.single_mut() {
                    if inv.items.len() < 9 {
                        // Remove position so it's no longer on the map.
                        commands.entity(ent).remove::<Position>();
                        inv.items.push(ent);
                        combat_log.push(format!("Picked up {item_name}"));
                        picked_up = true;
                        break; // Pick up one item at a time.
                    } else {
                        combat_log.push("Inventory full!".into());
                    }
                }
            }
        }

        if !picked_up {
            combat_log.push("Nothing to pick up here.".into());
        }
    }
}

/// Processes use-item intents: consumes an item from the player's inventory.
pub fn use_item_system(
    mut intents: MessageReader<UseItemIntent>,
    mut commands: Commands,
    mut inventory_query: Query<&mut Inventory, With<Player>>,
    mut health_query: Query<&mut Health, With<Player>>,
    mut item_kind_query: Query<(&mut ItemKind, Option<&Name>)>,
    mut combat_log: ResMut<CombatLog>,
    mut collectibles: ResMut<Collectibles>,
) {
    for intent in intents.read() {
        let Ok(mut inv) = inventory_query.single_mut() else {
            continue;
        };

        let Some(&item_entity) = inv.items.get(intent.item_index) else {
            combat_log.push("No item in that slot.".into());
            continue;
        };

        let Ok((kind, name)) = item_kind_query.get(item_entity) else {
            combat_log.push("Invalid item.".into());
            continue;
        };

        let item_name = name.map_or("item", |n| n.0.as_str()).to_string();

        match &*kind {
            ItemKind::Whiskey { heal } => {
                let heal = *heal;
                if let Ok(mut hp) = health_query.single_mut() {
                    let healed = heal.min(hp.max - hp.current);
                    hp.current = (hp.current + heal).min(hp.max);
                    combat_log.push(format!("Used {item_name}, healed {healed} HP"));
                }
                inv.items.remove(intent.item_index);
                commands.entity(item_entity).despawn();
            }
            ItemKind::Grenade { .. } => {
                combat_log.push(format!("Used {item_name}!"));
                inv.items.remove(intent.item_index);
                commands.entity(item_entity).despawn();
            }
            ItemKind::Hat { defense } => {
                combat_log.push(format!("Equipped {item_name} (+{defense} def)"));
            }
            ItemKind::Gun { loaded, capacity, caliber, name: gun_name, .. } => {
                let gun_name = gun_name.clone();
                let caliber = *caliber;
                let loaded = *loaded;
                let capacity = *capacity;

                if loaded >= capacity {
                    combat_log.push(format!("{gun_name} is fully loaded ({capacity}/{capacity})"));
                } else {
                    let has_bullet = match caliber {
                        Caliber::Cal36 => collectibles.bullets_36 > 0,
                        Caliber::Cal44 => collectibles.bullets_44 > 0,
                    };
                    if has_bullet && collectibles.caps > 0 && collectibles.powder > 0 {
                        match caliber {
                            Caliber::Cal36 => collectibles.bullets_36 -= 1,
                            Caliber::Cal44 => collectibles.bullets_44 -= 1,
                        }
                        collectibles.caps -= 1;
                        collectibles.powder -= 1;
                        if let Ok((mut kind_mut, _)) = item_kind_query.get_mut(item_entity) {
                            if let ItemKind::Gun { ref mut loaded, .. } = *kind_mut {
                                *loaded += 1;
                                combat_log.push(format!(
                                    "Loaded 1 round into {gun_name} ({}/{capacity})",
                                    *loaded
                                ));
                            }
                        }
                    } else {
                        combat_log.push(format!(
                            "Need: 1 {caliber} bullet, 1 cap, 1 powder to reload {gun_name}"
                        ));
                    }
                }
            }
            ItemKind::Knife { .. } => {
                combat_log.push("Readied knife".into());
            }
            ItemKind::Tomahawk { .. } => {
                combat_log.push("Readied tomahawk".into());
            }
        }
    }
}

/// Loot table entries for item drops.
struct LootEntry {
    name: &'static str,
    symbol: &'static str,
    fg: RatColor,
    kind: ItemKind,
    weight: f64,
}

const LOOT_TABLE: &[LootEntry] = &[
    LootEntry {
        name: "Whiskey Bottle",
        symbol: "w",
        fg: RatColor::Rgb(180, 120, 60),
        kind: ItemKind::Whiskey { heal: 10 },
        weight: 0.25,
    },
    LootEntry {
        name: "Dynamite Stick",
        symbol: "*",
        fg: RatColor::Rgb(255, 165, 0),
        kind: ItemKind::Grenade { damage: 8, radius: 2 },
        weight: 0.15,
    },
    LootEntry {
        name: "Bowie Knife",
        symbol: "/",
        fg: RatColor::Rgb(192, 192, 210),
        kind: ItemKind::Knife { attack: 4 },
        weight: 0.20,
    },
    LootEntry {
        name: "Tomahawk",
        symbol: "t",
        fg: RatColor::Rgb(160, 120, 80),
        kind: ItemKind::Tomahawk { attack: 5 },
        weight: 0.20,
    },
    LootEntry {
        name: "Cowboy Hat",
        symbol: "^",
        fg: RatColor::Rgb(210, 180, 140),
        kind: ItemKind::Hat { defense: 1 },
        weight: 0.20,
    },
];

/// Spawns a random loot item at the given position using deterministic noise.
/// Called by the death system when a monster with a LootTable dies.
pub fn spawn_loot(commands: &mut Commands, x: i32, y: i32, roll: f64) {
    // Select item based on weighted roll.
    let mut cumulative = 0.0;
    for entry in LOOT_TABLE {
        cumulative += entry.weight;
        if roll < cumulative {
            commands.spawn((
                Position { x, y },
                Item,
                Name(entry.name.into()),
                Renderable {
                    symbol: entry.symbol.into(),
                    fg: entry.fg,
                    bg: RatColor::Black,
                },
                entry.kind.clone(),
            ));
            return;
        }
    }
    // Fallback: spawn a whiskey bottle.
    commands.spawn((
        Position { x, y },
        Item,
        Name("Whiskey Bottle".into()),
        Renderable {
            symbol: "w".into(),
            fg: RatColor::Rgb(180, 120, 60),
            bg: RatColor::Black,
        },
        ItemKind::Whiskey { heal: 10 },
    ));
}

/// Reload system placeholder. Real reloading will use per-gun loaded rounds.
pub fn reload_system(
    _commands: Commands,
    player_query: Query<&Inventory, With<Player>>,
    item_kind_query: Query<(&ItemKind, Option<&Name>)>,
    mut combat_log: ResMut<CombatLog>,
    mut input_state: ResMut<InputState>,
) {
    if !input_state.reload_pending {
        return;
    }
    input_state.reload_pending = false;

    let Ok(inv) = player_query.single() else {
        return;
    };

    // Find the first Gun in inventory.
    let _gun_index = inv.items.iter().position(|&ent| {
        item_kind_query
            .get(ent)
            .ok()
            .map_or(false, |(k, _)| matches!(k, ItemKind::Gun { .. }))
    });

    combat_log.push("Reload not yet implemented in field - use inventory mode".into());
}

/// Auto-pickup system: automatically picks up any item when the player walks
/// over it. Runs after movement.
pub fn auto_pickup_system(
    mut commands: Commands,
    player_query: Query<&Position, With<Player>>,
    items_query: Query<(Entity, &Position, Option<&Name>, Option<&CollectibleKind>), With<Item>>,
    spatial: Res<SpatialIndex>,
    mut inventory_query: Query<&mut Inventory, With<Player>>,
    mut combat_log: ResMut<CombatLog>,
    mut collectibles: ResMut<Collectibles>,
) {
    let Ok(player_pos) = player_query.single() else {
        return;
    };
    let player_vec = player_pos.as_grid_vec();

    let entities_here = spatial.entities_at(&player_vec);

    for &ent in entities_here {
        let Ok((item_entity, _pos, item_name, coll_kind)) = items_query.get(ent) else {
            continue;
        };

        let name_str = item_name.map_or("item", |n| n.0.as_str()).to_string();

        // Handle collectible items: add to Collectibles resource instead of inventory.
        if let Some(kind) = coll_kind {
            match *kind {
                CollectibleKind::Caps(n) => collectibles.caps += n,
                CollectibleKind::Bullets36(n) => collectibles.bullets_36 += n,
                CollectibleKind::Bullets44(n) => collectibles.bullets_44 += n,
                CollectibleKind::Powder(n) => collectibles.powder += n,
                CollectibleKind::Bandages(n) => collectibles.bandages += n,
                CollectibleKind::Dollars(n) => collectibles.dollars += n,
            }
            combat_log.push(format!("Picked up {name_str}"));
            commands.entity(item_entity).despawn();
            continue;
        }

        if let Ok(mut inv) = inventory_query.single_mut() {
            if inv.items.len() < 9 {
                commands.entity(item_entity).remove::<Position>();
                inv.items.push(item_entity);
                combat_log.push(format!("Picked up {name_str}"));
            }
        }
    }
}

/// Processes drop-item intents: removes an item from inventory and places it on the ground.
pub fn drop_item_system(
    mut intents: MessageReader<DropItemIntent>,
    mut commands: Commands,
    mut inventory_query: Query<(&mut Inventory, &Position), With<Player>>,
    name_query: Query<Option<&Name>>,
    mut combat_log: ResMut<CombatLog>,
) {
    for intent in intents.read() {
        let Ok((mut inv, player_pos)) = inventory_query.single_mut() else {
            continue;
        };

        if intent.item_index >= inv.items.len() {
            combat_log.push("No item in that slot.".into());
            continue;
        }

        let item_entity = inv.items.remove(intent.item_index);
        let item_name = name_query
            .get(item_entity)
            .ok()
            .flatten()
            .map_or("item".to_string(), |n| n.0.clone());

        commands
            .entity(item_entity)
            .insert(Position { x: player_pos.x, y: player_pos.y });
        combat_log.push(format!("Dropped {item_name}"));
    }
}

/// Processes throw-item intents: removes a knife/tomahawk from inventory and
/// traces a Bresenham line toward the target. Damages the first hostile hit,
/// then places the item at the landing position with a Thrown marker.
pub fn throw_system(
    mut intents: MessageReader<ThrowItemIntent>,
    mut commands: Commands,
    mut damage_events: MessageWriter<crate::events::DamageEvent>,
    mut inventory_query: Query<(&mut Inventory, &Position), With<Player>>,
    targets: Query<(Entity, &Position, &crate::components::CombatStats, Option<&Name>), With<Hostile>>,
    mut combat_log: ResMut<CombatLog>,
    mut spell_particles: ResMut<SpellParticles>,
    game_map: Res<GameMapResource>,
    name_query: Query<Option<&Name>>,
) {
    for intent in intents.read() {
        let Ok((mut inv, player_pos)) = inventory_query.single_mut() else {
            continue;
        };

        // Remove from inventory
        if let Some(idx) = inv.items.iter().position(|&e| e == intent.item_entity) {
            inv.items.remove(idx);
        } else {
            continue;
        }

        let item_name = name_query
            .get(intent.item_entity)
            .ok()
            .flatten()
            .map_or("item".to_string(), |n| n.0.clone());

        let origin = player_pos.as_grid_vec();
        let endpoint = origin + GridVec::new(intent.dx * intent.range, intent.dy * intent.range);
        let path = origin.bresenham_line(endpoint);

        // Build hostile lookup
        let mut target_by_pos: std::collections::HashMap<GridVec, (Entity, i32, String)> =
            std::collections::HashMap::new();
        for (target_entity, target_pos, target_stats, target_name) in &targets {
            let t_name = target_name.map_or("???".to_string(), |n| n.0.clone());
            target_by_pos.insert(target_pos.as_grid_vec(), (target_entity, target_stats.defense, t_name));
        }

        let mut landing = origin;
        let mut hit = false;

        for (step_idx, &tile) in path.iter().enumerate().skip(1) {
            spell_particles.particles.push((tile, 3, (step_idx as u32).saturating_sub(1)));

            if !game_map.0.is_passable(&tile) {
                break;
            }

            landing = tile;

            if let Some((target_entity, target_def, t_name)) = target_by_pos.get(&tile) {
                let dmg = (intent.damage - target_def).max(0);
                if dmg > 0 {
                    damage_events.write(crate::events::DamageEvent {
                        target: *target_entity,
                        amount: dmg,
                    });
                    combat_log.push(format!("Threw {item_name} at {t_name} for {dmg} damage!"));
                } else {
                    combat_log.push(format!("Threw {item_name} at {t_name} but dealt no damage"));
                }
                hit = true;
                break;
            }
        }

        if !hit {
            combat_log.push(format!("Threw {item_name} but hit nothing"));
        }

        // Place the item at the landing position with Thrown marker
        commands.entity(intent.item_entity).insert((
            Position { x: landing.x, y: landing.y },
            Thrown,
        ));
    }
}
