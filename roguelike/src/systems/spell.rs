use bevy::prelude::*;

use crate::components::{CombatStats, Inventory, Stamina, Name, Player};
use crate::events::SpellCastIntent;
use crate::resources::{CombatLog, GameMapResource};
use crate::typeenums::Furniture;

/// Stamina cost for casting the AoE grenade.
const SPELL_STAMINA_COST: i32 = 10;

/// Resolves grenade throw intents by spawning shrapnel projectile entities.
///
/// For each `SpellCastIntent`, the player throws a frag grenade that detonates
/// at the target position, spawning shrapnel projectile entities in all
/// directions within the specified radius. Shrapnel entities travel outward
/// over ticks and apply damage when they reach hostile entities.
/// Consumes stamina from the caster.
pub fn spell_system(
    mut commands: Commands,
    mut intents: MessageReader<SpellCastIntent>,
    mut caster_query: Query<(&CombatStats, Option<&Name>, Option<&mut Stamina>, Option<&mut Inventory>), With<Player>>,
    mut combat_log: ResMut<CombatLog>,
    mut game_map: ResMut<GameMapResource>,
) {
    for intent in intents.read() {
        let Ok((caster_stats, caster_name, stamina, inventory)) = caster_query.get_mut(intent.caster) else {
            continue;
        };

        // Consume stamina.
        if let Some(mut stamina) = stamina {
            if stamina.current < SPELL_STAMINA_COST {
                combat_log.push("Not enough stamina!".into());
                continue;
            }
            stamina.current -= SPELL_STAMINA_COST;
        }

        // Consume the grenade item from inventory.
        if let Some(mut inv) = inventory {
            if intent.grenade_index < inv.items.len() {
                let grenade_entity = inv.items.remove(intent.grenade_index);
                commands.entity(grenade_entity).despawn();
            }
        }

        let origin = intent.target;
        let c_name = caster_name.map_or("???", |n| &n.0);

        combat_log.push(format!("{c_name} throws a grenade!"));

        // Spawn shrapnel projectile entities that radiate from the detonation point.
        crate::systems::projectile::spawn_shrapnel(
            &mut commands,
            origin,
            intent.radius,
            caster_stats.attack,
            intent.caster,
        );

        // Environmental destruction: destroy trees, bushes, rocks within radius.
        let mut destroyed_count = 0;
        for dx in -intent.radius..=intent.radius {
            for dy in -intent.radius..=intent.radius {
                let dist = dx.abs().max(dy.abs());
                if dist > 0 && dist <= intent.radius {
                    let target_pos = origin + crate::grid_vec::GridVec::new(dx, dy);
                    if let Some(voxel) = game_map.0.get_voxel_at_mut(&target_pos) {
                        if let Some(ref furn) = voxel.furniture {
                            match furn {
                                Furniture::Tree | Furniture::DeadTree | Furniture::Bush | Furniture::Rock => {
                                    voxel.furniture = None;
                                    destroyed_count += 1;
                                }
                                Furniture::Wall => {} // Walls are indestructible.
                            }
                        }
                    }
                }
            }
        }
        if destroyed_count > 0 {
            combat_log.push(format!("The grenade destroys {destroyed_count} obstacle(s)!"));
        }
    }
}
