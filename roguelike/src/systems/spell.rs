use bevy::prelude::*;

use crate::components::{CombatStats, Inventory, Stamina, Name, Player};
use crate::events::{MolotovCastIntent, SpellCastIntent};
use crate::resources::{CombatLog, GameMapResource, SpellParticles};
use crate::typeenums::{Floor, Furniture};

/// Stamina cost for casting the AoE grenade.
const SPELL_STAMINA_COST: i32 = 10;

/// Resolves grenade throw intents by spawning shrapnel projectile entities.
///
/// For each `SpellCastIntent`, the player throws a frag grenade that detonates
/// at the target position, spawning shrapnel projectile entities in all
/// directions within the specified radius. Shrapnel entities travel outward
/// over ticks and apply damage when they reach hostile entities.
/// Dynamite also sets some flammable objects on fire.
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
        if let Some(mut stamina) = stamina
            && !stamina.spend(SPELL_STAMINA_COST) {
                combat_log.push("Not enough stamina!".into());
                continue;
            }

        // Consume the grenade item from inventory.
        if let Some(mut inv) = inventory
            && intent.grenade_index < inv.items.len() {
                let grenade_entity = inv.items.remove(intent.grenade_index);
                commands.entity(grenade_entity).despawn();
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

        // Environmental destruction: destroy obstacles and set some on fire.
        let mut destroyed_count = 0;
        let mut fire_count = 0;
        for dx in -intent.radius..=intent.radius {
            for dy in -intent.radius..=intent.radius {
                let dist = dx.abs().max(dy.abs());
                if dist > 0 && dist <= intent.radius {
                    let target_pos = origin + crate::grid_vec::GridVec::new(dx, dy);
                    if let Some(voxel) = game_map.0.get_voxel_at_mut(&target_pos)
                        && let Some(ref furn) = voxel.furniture {
                            let is_flammable = furn.is_flammable();
                            // Sturdy non-destructible objects survive explosions.
                            let is_indestructible = matches!(
                                furn,
                                Furniture::Wall | Furniture::LampPost
                                | Furniture::HitchingPost | Furniture::WaterTrough
                            );
                            if !is_indestructible {
                                // Dynamite sets flammable things on fire in the inner radius.
                                if is_flammable && dist <= 1 {
                                    voxel.furniture = None;
                                    voxel.floor = Some(Floor::Fire);
                                    fire_count += 1;
                                } else {
                                    voxel.furniture = None;
                                    destroyed_count += 1;
                                }
                            }
                        }
                }
            }
        }
        if destroyed_count > 0 {
            combat_log.push(format!("The grenade destroys {destroyed_count} obstacle(s)!"));
        }
        if fire_count > 0 {
            combat_log.push(format!("{fire_count} object(s) catch fire!"));
        }
    }
}

/// Resolves molotov cocktail throws.
/// Ignites all flammable furniture within the blast radius, leaving fire on
/// the ground. Deals area damage via shrapnel. Larger fire radius than dynamite.
pub fn molotov_system(
    mut commands: Commands,
    mut intents: MessageReader<MolotovCastIntent>,
    mut caster_query: Query<(&CombatStats, Option<&Name>, Option<&mut Stamina>, Option<&mut Inventory>), With<Player>>,
    mut combat_log: ResMut<CombatLog>,
    mut game_map: ResMut<GameMapResource>,
    mut spell_particles: ResMut<SpellParticles>,
) {
    for intent in intents.read() {
        let Ok((caster_stats, caster_name, stamina, inventory)) = caster_query.get_mut(intent.caster) else {
            continue;
        };

        // Consume stamina.
        if let Some(mut stamina) = stamina
            && !stamina.spend(SPELL_STAMINA_COST) {
                combat_log.push("Not enough stamina!".into());
                continue;
            }

        // Consume the molotov item from inventory.
        if let Some(mut inv) = inventory
            && intent.item_index < inv.items.len() {
                let molotov_entity = inv.items.remove(intent.item_index);
                commands.entity(molotov_entity).despawn();
            }

        let origin = intent.target;
        let c_name = caster_name.map_or("???", |n| &n.0);

        combat_log.push(format!("{c_name} hurls a Molotov cocktail!"));

        // Spawn fire shrapnel (slightly weaker than grenade, but more fire).
        crate::systems::projectile::spawn_shrapnel(
            &mut commands,
            origin,
            intent.radius.min(2), // shrapnel only in smaller radius
            caster_stats.attack / 2 + intent.damage,
            intent.caster,
        );

        // Add fire particle effects
        spell_particles.add_aoe(origin, 6);

        // Set everything flammable on fire within the full radius.
        let mut fire_count = 0;
        for dx in -intent.radius..=intent.radius {
            for dy in -intent.radius..=intent.radius {
                let dist = dx.abs().max(dy.abs());
                if dist <= intent.radius {
                    let target_pos = origin + crate::grid_vec::GridVec::new(dx, dy);
                    if let Some(voxel) = game_map.0.get_voxel_at_mut(&target_pos) {
                        if let Some(ref furn) = voxel.furniture {
                            if furn.is_flammable() {
                                voxel.furniture = None;
                                voxel.floor = Some(Floor::Fire);
                                fire_count += 1;
                            }
                        } else if dist <= 1 {
                            // Center tiles catch fire on the ground
                            voxel.floor = Some(Floor::Fire);
                            fire_count += 1;
                        }
                    }
                }
            }
        }
        if fire_count > 0 {
            combat_log.push(format!("A blazing inferno! {fire_count} tile(s) set ablaze!"));
        }
    }
}
