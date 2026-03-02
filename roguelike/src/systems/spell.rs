use bevy::prelude::*;

use crate::components::{CombatStats, Hostile, Inventory, Stamina, Name, Player, Position};
use crate::events::{DamageEvent, SpellCastIntent};
use crate::resources::{CombatLog, GameMapResource, SpellParticles};
use crate::typeenums::Furniture;

/// Stamina cost for casting the AoE grenade.
const SPELL_STAMINA_COST: i32 = 10;

/// Lifetime (in frames) for shrapnel particle animations.
const PARTICLE_LIFETIME: u32 = 8;

/// Resolves grenade throw intents.
///
/// For each `SpellCastIntent`, the player throws a frag grenade that detonates
/// at the player's position, sending shrapnel in all directions within the
/// specified radius (Chebyshev distance). Damages all entities including
/// potentially the player. Emits `DamageEvent` for each hit.
/// Consumes stamina from the caster and generates shrapnel particle animations.
pub fn spell_system(
    mut commands: Commands,
    mut intents: MessageReader<SpellCastIntent>,
    mut damage_events: MessageWriter<DamageEvent>,
    mut caster_query: Query<(&CombatStats, Option<&Name>, Option<&mut Stamina>, Option<&mut Inventory>), With<Player>>,
    targets: Query<(Entity, &Position, Option<&Name>), With<Hostile>>,
    player_entities: Query<(Entity, &Position), With<Player>>,
    mut combat_log: ResMut<CombatLog>,
    mut spell_particles: ResMut<SpellParticles>,
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
        let mut hit_count = 0;

        // Grenade shrapnel damages all hostile entities within radius.
        for (target_entity, target_pos, target_name) in &targets {
            let target_vec = target_pos.as_grid_vec();
            let dist = origin.chebyshev_distance(target_vec);

            if dist <= intent.radius && dist > 0 {
                let damage = caster_stats.attack;
                let t_name = target_name.map_or("???", |n| &n.0);

                if damage > 0 {
                    damage_events.write(DamageEvent {
                        target: target_entity,
                        amount: damage,
                    });
                    combat_log.push(format!(
                        "{c_name}'s grenade shrapnel hits {t_name} for {damage} damage"
                    ));
                    hit_count += 1;
                }
            }
        }

        // Grenade self-damage: shrapnel hits the thrower only if within blast radius.
        if let Ok((player_entity, player_pos)) = player_entities.single() {
            if player_entity == intent.caster {
                let player_vec = player_pos.as_grid_vec();
                let dist = origin.chebyshev_distance(player_vec);
                if dist <= intent.radius {
                    let self_damage = (caster_stats.attack / 2).max(1);
                    damage_events.write(DamageEvent {
                        target: player_entity,
                        amount: self_damage,
                    });
                    combat_log.push(format!("Grenade shrapnel hits you for {self_damage} damage!"));
                }
            }
        }

        // Generate shrapnel particle animation for the grenade blast.
        spell_particles.add_aoe(origin, PARTICLE_LIFETIME);

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

        if hit_count == 0 {
            combat_log.push(format!("{c_name} throws a grenade but hits nothing"));
        }
    }
}
