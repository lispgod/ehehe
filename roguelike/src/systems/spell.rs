use bevy::prelude::*;

use crate::components::{CombatStats, Inventory, Item, ItemKind, Stamina, Name, Position, Renderable, display_name};
use crate::events::{MolotovCastIntent, SpellCastIntent};
use crate::resources::{CombatLog, GameMapResource, MapSeed, SpellParticles, TurnCounter};
use crate::typeenums::{Floor, Furniture};
use crate::typedefs::RatColor;

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
    mut caster_query: Query<(&CombatStats, Option<&Name>, Option<&mut Stamina>, Option<&mut Inventory>, Option<&Position>)>,
    mut combat_log: ResMut<CombatLog>,
    mut game_map: ResMut<GameMapResource>,
    seed: Res<MapSeed>,
    turn_counter: Res<TurnCounter>,
) {
    for intent in intents.read() {
        let Ok((caster_stats, caster_name, stamina, inventory, caster_pos)) = caster_query.get_mut(intent.caster) else {
            continue;
        };

        // Sand throw (sentinel: grenade_index == usize::MAX) — creates persistent
        // sand cloud tiles on the map that block line of sight.
        // The cloud is circular but biased to spread toward the target
        // direction (plume shape away from the caster).
        if intent.grenade_index == usize::MAX {
            if let Some(mut stamina) = stamina {
                stamina.spend(5); // Sand throw costs 5 stamina
            }
            // Place persistent SandCloud floor tiles on the map.
            let origin = intent.target;
            let radius_f = intent.radius as f64;
            // Compute direction from caster to target for directional bias.
            let caster_vec = caster_pos.map(|p| p.as_grid_vec());
            let dir = caster_vec.map(|cv| {
                let d = origin - cv;
                let len = ((d.x as f64).powi(2) + (d.y as f64).powi(2)).sqrt();
                if len > 0.01 { (d.x as f64 / len, d.y as f64 / len) } else { (0.0, 0.0) }
            }).unwrap_or((0.0, 0.0));

            // First pass: collect tiles and their current floors.
            let mut tiles_to_cloud: Vec<(crate::grid_vec::GridVec, Option<Floor>)> = Vec::new();
            for dx in -(intent.radius + 1)..=(intent.radius + 1) {
                for dy in -(intent.radius + 1)..=(intent.radius + 1) {
                    let fx = dx as f64;
                    let fy = dy as f64;
                    let dist = (fx * fx + fy * fy).sqrt();
                    let dot = if dist > 0.01 {
                        (fx * dir.0 + fy * dir.1) / dist
                    } else {
                        0.0
                    };
                    let effective_radius = radius_f + 0.5 + dot.max(0.0) * 1.0;
                    if dist > effective_radius {
                        continue;
                    }
                    let pos = origin + crate::grid_vec::GridVec::new(dx, dy);
                    if let Some(voxel) = game_map.0.get_voxel_at(&pos) {
                        if !matches!(voxel.furniture, Some(Furniture::Wall)) {
                            tiles_to_cloud.push((pos, voxel.floor.clone()));
                        }
                    }
                }
            }
            // Second pass: apply changes.
            for (pos, prev_floor) in tiles_to_cloud {
                if !game_map.0.sand_cloud_previous_floor.contains_key(&pos) {
                    game_map.0.sand_cloud_previous_floor.insert(pos, prev_floor);
                }
                if let Some(voxel) = game_map.0.get_voxel_at_mut(&pos) {
                    voxel.floor = Some(Floor::SandCloud);
                }
                game_map.0.sand_cloud_turns.insert(pos, turn_counter.0);
            }
            continue;
        }

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
        let c_name = display_name(caster_name);

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
        let mut water_count = 0;
        for dx in -intent.radius..=intent.radius {
            for dy in -intent.radius..=intent.radius {
                let dist = dx.abs().max(dy.abs());
                if dist > 0 && dist <= intent.radius {
                    let target_pos = origin + crate::grid_vec::GridVec::new(dx, dy);
                    if let Some(voxel) = game_map.0.get_voxel_at_mut(&target_pos)
                        && let Some(ref furn) = voxel.furniture {
                            let is_flammable = furn.is_flammable();
                            let is_water_trough = matches!(furn, Furniture::WaterTrough);
                            let is_lootable = matches!(furn, Furniture::Crate | Furniture::Barrel);
                            // Sturdy non-destructible objects survive explosions.
                            let is_indestructible = matches!(
                                furn,
                                Furniture::Wall
                                | Furniture::HitchingPost
                            );
                            if is_water_trough {
                                // Water trough spills water when destroyed
                                voxel.furniture = None;
                                voxel.floor = Some(Floor::Water);
                                water_count += 1;
                            } else if !is_indestructible {
                                if is_lootable {
                                    // Crates/barrels drop loot when destroyed
                                    let loot_roll = crate::noise::value_noise(
                                        target_pos.x, target_pos.y,
                                        seed.0.wrapping_add(88888),
                                    );
                                    spawn_container_loot(&mut commands, target_pos.x, target_pos.y, loot_roll);
                                }
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
        if water_count > 0 {
            combat_log.push(format!("{water_count} water trough(s) spill water!"));
        }
    }
}

/// Resolves molotov cocktail throws.
/// Ignites all flammable furniture within the blast radius, leaving fire on
/// the ground. Deals area damage via shrapnel. Larger fire radius than dynamite.
pub fn molotov_system(
    mut commands: Commands,
    mut intents: MessageReader<MolotovCastIntent>,
    mut caster_query: Query<(&CombatStats, Option<&Name>, Option<&mut Stamina>, Option<&mut Inventory>)>,
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
        let c_name = display_name(caster_name);

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

/// Spawns a random loot item when a container (crate/barrel) is destroyed.
/// Containers have a chance to drop supplies or small items.
fn spawn_container_loot(commands: &mut Commands, x: i32, y: i32, roll: f64) {
    if roll < 0.3 {
        commands.spawn((
            Position { x, y },
            Item,
            Name("Whiskey Bottle".into()),
            Renderable { symbol: "w".into(), fg: RatColor::Rgb(180, 120, 60), bg: RatColor::Black },
            ItemKind::Whiskey { heal: 10 },
        ));
    } else if roll < 0.5 {
        commands.spawn((
            Position { x, y },
            Item,
            Name("Bowie Knife".into()),
            Renderable { symbol: "/".into(), fg: RatColor::Rgb(192, 192, 210), bg: RatColor::Black },
            ItemKind::Knife { attack: 4 },
        ));
    } else if roll < 0.65 {
        commands.spawn((
            Position { x, y },
            Item,
            Name("Dynamite Stick".into()),
            Renderable { symbol: "*".into(), fg: RatColor::Rgb(255, 165, 0), bg: RatColor::Black },
            ItemKind::Grenade { damage: 8, radius: 2 },
        ));
    }
    // else: no drop (35% chance)
}
