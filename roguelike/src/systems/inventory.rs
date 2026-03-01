use bevy::prelude::*;

use crate::components::{
    Health, Inventory, Item, ItemKind, Name, Player, Position, Renderable,
};
use crate::events::{PickupItemIntent, UseItemIntent};
use crate::resources::{CombatLog, SpatialIndex};
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
    item_kind_query: Query<(&ItemKind, Option<&Name>)>,
    mut combat_log: ResMut<CombatLog>,
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

        match kind {
            ItemKind::HealingPotion { amount } => {
                if let Ok(mut hp) = health_query.single_mut() {
                    let healed = (*amount).min(hp.max - hp.current);
                    hp.current = (hp.current + amount).min(hp.max);
                    combat_log.push(format!("Used {item_name}, healed {healed} HP"));
                }
                inv.items.remove(intent.item_index);
                commands.entity(item_entity).despawn();
            }
            ItemKind::Scroll { damage: _, radius: _ } => {
                // Scrolls trigger a spell effect — for now just log and consume.
                combat_log.push(format!("Used {item_name}!"));
                inv.items.remove(intent.item_index);
                commands.entity(item_entity).despawn();
            }
            ItemKind::Armor { defense } => {
                combat_log.push(format!("Equipped {item_name} (+{defense} def)"));
                // Equip handled elsewhere; for now just log.
            }
            ItemKind::Weapon { attack } => {
                combat_log.push(format!("Equipped {item_name} (+{attack} atk)"));
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
        name: "Medkit",
        symbol: "+",
        fg: RatColor::Rgb(255, 50, 50),
        kind: ItemKind::HealingPotion { amount: 10 },
        weight: 0.45,
    },
    LootEntry {
        name: "Frag Grenade",
        symbol: "*",
        fg: RatColor::Rgb(255, 165, 0),
        kind: ItemKind::Scroll { damage: 8, radius: 2 },
        weight: 0.20,
    },
    LootEntry {
        name: "Body Armor",
        symbol: "[",
        fg: RatColor::Rgb(100, 130, 100),
        kind: ItemKind::Armor { defense: 1 },
        weight: 0.15,
    },
    LootEntry {
        name: "Combat Rifle",
        symbol: "/",
        fg: RatColor::Rgb(180, 180, 200),
        kind: ItemKind::Weapon { attack: 2 },
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
    // Fallback: spawn a medkit.
    commands.spawn((
        Position { x, y },
        Item,
        Name("Medkit".into()),
        Renderable {
            symbol: "+".into(),
            fg: RatColor::Rgb(255, 50, 50),
            bg: RatColor::Black,
        },
        ItemKind::HealingPotion { amount: 10 },
    ));
}
