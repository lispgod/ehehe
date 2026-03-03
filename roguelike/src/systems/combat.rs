use bevy::prelude::*;

use crate::components::{CollectibleKind, CombatStats, Faction, Health, Hostile, Inventory, Item, ItemKind, LastDamageSource, LootTable, Name, Player, Position, Renderable, display_name};
use crate::events::{AiRangedAttackIntent, AttackIntent, DamageEvent, MeleeWideIntent, RangedAttackIntent};
use crate::noise::value_noise;
use crate::resources::{CombatLog, DynamicRng, GameMapResource, GameState, KillCount, MapSeed, SoundEvents, TurnCounter};
use crate::grid_vec::GridVec;
use crate::typeenums::{Floor, Furniture};
use crate::typedefs::{CoordinateUnit, RatColor};

/// Computes the bullet endpoint by scaling a direction vector so the
/// Bresenham line extends approximately `range` tiles along the
/// dominant axis, preserving the exact trajectory angle.
#[inline]
fn bullet_endpoint(origin: GridVec, dx: CoordinateUnit, dy: CoordinateUnit, range: CoordinateUnit) -> GridVec {
    let max_comp = dx.abs().max(dy.abs());
    let scale = range.div_euclid(max_comp).max(1);
    origin + GridVec::new(dx * scale, dy * scale)
}

/// Spawns a small cloud of gun smoke at the firing position.
/// Places persistent SandCloud floor tiles on the map that block sight.
/// Saves the previous floor type for restoration when smoke dissipates.
/// The smoke is biased in the firing direction for a more natural plume.
fn spawn_gun_smoke(game_map: &mut GameMapResource, origin: GridVec, turn: u32, fire_dx: i32, fire_dy: i32) {
    // Normalize firing direction for directional bias.
    let flen = ((fire_dx as f64).powi(2) + (fire_dy as f64).powi(2)).sqrt();
    let (ndx, ndy) = if flen > 0.01 {
        (fire_dx as f64 / flen, fire_dy as f64 / flen)
    } else {
        (0.0, 0.0)
    };

    // First pass: collect positions and their current floor types.
    let mut tiles_to_cloud: Vec<(GridVec, Option<Floor>)> = Vec::new();
    for dx in -2..=2i32 {
        for dy in -2..=2i32 {
            let fx = dx as f64;
            let fy = dy as f64;
            let dist = (fx * fx + fy * fy).sqrt();
            let dot = if dist > 0.01 {
                (fx * ndx + fy * ndy) / dist
            } else {
                0.0
            };
            let effective_radius = 1.5 + dot.max(0.0) * 1.0;
            if dist > effective_radius {
                continue;
            }
            let pos = origin + GridVec::new(dx, dy);
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
        game_map.0.sand_cloud_turns.insert(pos, turn);
    }
}

/// Resolves attack intents into damage events.
///
/// Damage = attacker.attack.
/// Uses `CombatStats::damage_against` for the formal damage model.
/// Emits a `DamageEvent` for each successful hit and logs combat messages.
pub fn combat_system(
    mut intents: MessageReader<AttackIntent>,
    mut damage_events: MessageWriter<DamageEvent>,
    stats_query: Query<(&CombatStats, Option<&Name>, Option<&Position>)>,
    mut combat_log: ResMut<CombatLog>,
) {
    for intent in intents.read() {
        let Ok((attacker_stats, attacker_name, attacker_pos)) = stats_query.get(intent.attacker) else {
            continue;
        };
        let Ok((_target_stats, target_name, _)) = stats_query.get(intent.target) else {
            continue;
        };

        let damage = attacker_stats.damage_against();
        let a_name = display_name(attacker_name);
        let t_name = display_name(target_name);
        let pos = attacker_pos.map(|p| p.as_grid_vec());

        if damage > 0 {
            combat_log.push_opt(format!("{a_name} hits {t_name} for {damage} damage"), pos);
            damage_events.write(DamageEvent {
                target: intent.target,
                amount: damage,
                source: Some(intent.attacker),
            });
        } else {
            combat_log.push_opt(format!("{a_name} attacks {t_name} but deals no damage"), pos);
        }
    }
}

/// Applies damage events to entity health pools using `Health::apply_damage`.
/// Also records the damage source on the target for kill attribution.
/// When god mode is active, damage to the player is ignored.
pub fn apply_damage_system(
    mut commands: Commands,
    mut events: MessageReader<DamageEvent>,
    mut health_query: Query<&mut Health>,
    player_query: Query<Entity, With<Player>>,
    god_mode: Res<crate::resources::GodMode>,
) {
    let player_entity = player_query.single().ok();
    for event in events.read() {
        // God mode: skip damage to the player.
        if god_mode.0 && player_entity == Some(event.target) {
            continue;
        }
        if let Ok(mut health) = health_query.get_mut(event.target) {
            health.apply_damage(event.amount);
            if let Some(source) = event.source {
                commands.entity(event.target).insert(LastDamageSource(source));
            }
        }
    }
}

/// Despawns entities whose health has reached zero.
/// Logs a death message, increments the kill counter for hostile entities
/// killed by the player, drops the entity's entire inventory on the ground,
/// and removes the entity.
/// Animals (Wildlife faction) drop nothing. Non-wildlife NPCs drop their full
/// inventory including guns with ammo.
/// If the player dies, transitions to the Dead state.
pub fn death_system(
    mut commands: Commands,
    query: Query<(Entity, &Health, Option<&Name>, Option<&Hostile>, Option<&Position>, Option<&LootTable>, Option<&Player>, Option<&LastDamageSource>, Option<&Inventory>, Option<&Faction>)>,
    player_entities: Query<Entity, With<Player>>,
    mut combat_log: ResMut<CombatLog>,
    mut kill_count: ResMut<KillCount>,
    mut next_game_state: ResMut<NextState<GameState>>,
    seed: Res<MapSeed>,
) {
    let player_entity = player_entities.single().ok();

    for (entity, health, name, hostile, pos, loot_table, is_player, last_damage_source, inventory, faction) in &query {
        if !health.is_dead() {
            continue;
        }

        let label = name.map_or("Something", |n| &n.0);
        combat_log.push_opt(format!("{label} has been slain!"), pos.map(|p| p.as_grid_vec()));

        // If the player died, transition to Dead state (don't despawn so UI can read stats).
        if is_player.is_some() {
            combat_log.push("You have fallen... Press T to continue watching, Q to quit, or R to restart.".into());
            next_game_state.set(GameState::Dead);
            continue; // don't despawn the player
        }

        if hostile.is_some() {
            let player_killed = player_entity.is_some_and(|pe|
                last_damage_source.is_some_and(|lds| lds.0 == pe)
            );
            if player_killed {
                kill_count.0 += 1;
            }
        }
        let is_wildlife = faction.is_some_and(|f| matches!(f, Faction::Wildlife));

        // Drop entire NPC inventory on the ground (animals drop nothing).
        if !is_wildlife
            && let (Some(inv), Some(p)) = (inventory, pos) {
                for &item_entity in &inv.items {
                    commands.entity(item_entity).insert(Position { x: p.x, y: p.y });
                }
                if !inv.items.is_empty() {
                    combat_log.push_at(format!("{label} dropped their gear!"), p.as_grid_vec());
                }
            }

        // Loot drop: non-wildlife entities with a LootTable may also drop collectible supplies.
        if !is_wildlife
            && let (Some(_lt), Some(p)) = (loot_table, pos) {
                // Drop collectible supplies (caps + random ammo).
                let coll_roll = value_noise(p.x.wrapping_add(kill_count.0 as i32 + 1), p.y, seed.0.wrapping_add(33333));
                if coll_roll < 0.5 {
                    let caps_amount = ((coll_roll * 20.0) as i32).max(1);
                    commands.spawn((
                        Position { x: p.x, y: p.y },
                        Item,
                        Name(format!("{caps_amount} Caps")),
                        Renderable { symbol: "$".into(), fg: RatColor::Rgb(255, 215, 0), bg: RatColor::Black },
                        CollectibleKind::Caps(caps_amount),
                    ));
                }
                let ammo_roll = value_noise(p.y.wrapping_add(kill_count.0 as i32 + 1), p.x, seed.0.wrapping_add(44444));
                if ammo_roll < 0.3 {
                    let amount = ((ammo_roll * 15.0) as i32).max(1);
                    commands.spawn((
                        Position { x: p.x, y: p.y },
                        Item,
                        Name(format!("{amount}x .36 Bullets")),
                        Renderable { symbol: "·".into(), fg: RatColor::Rgb(180, 180, 180), bg: RatColor::Black },
                        CollectibleKind::Bullets36(amount),
                    ));
                }
            }

        commands.entity(entity).despawn();
    }
}

/// Small chance for a gun to misfire (ammo consumed but no bullet fires).
const MISFIRE_CHANCE: f64 = 0.05;

/// Resolves targeted ranged attack intents by spawning bullet projectile entities.
///
/// The bullet path is computed using **Bresenham's line algorithm** from the
/// caster's position to the maximum range endpoint. Instead of applying damage
/// instantly, a bullet entity is spawned that travels along the path over
/// multiple ticks. Damage is applied when the projectile reaches a hostile.
///
/// Works for both the player and NPCs — the attacker entity is taken from the
/// intent. Consumes 1 loaded round from the gun item.
/// There is a small chance for the gun to misfire.
pub fn ranged_attack_system(
    mut commands: Commands,
    mut intents: MessageReader<RangedAttackIntent>,
    mut caster_query: Query<(&Position, Option<&Name>)>,
    mut combat_log: ResMut<CombatLog>,
    mut item_kind_query: Query<&mut ItemKind>,
    mut sound_events: ResMut<SoundEvents>,
    dynamic_rng: Res<DynamicRng>,
    seed: Res<MapSeed>,
    mut game_map: ResMut<GameMapResource>,
    turn_counter: Res<TurnCounter>,
) {
    for intent in intents.read() {
        let Ok((caster_pos, caster_name)) = caster_query.get_mut(intent.attacker) else {
            continue;
        };
        let origin = caster_pos.as_grid_vec();
        let c_name = display_name(caster_name);

        // Determine damage and consume a loaded round from the gun item.
        let damage;
        if let Some(gun_entity) = intent.gun_item {
            if let Ok(mut kind) = item_kind_query.get_mut(gun_entity) {
                if let ItemKind::Gun { loaded, attack, .. } = kind.as_mut() {
                    if *loaded <= 0 {
                        combat_log.push("Gun is empty!".into());
                        continue;
                    }
                    *loaded -= 1;
                    damage = *attack;
                } else {
                    continue;
                }
            } else {
                continue;
            }
        } else {
            combat_log.push("No weapon available!".into());
            continue;
        }

        let dx = intent.dx;
        let dy = intent.dy;

        if dx == 0 && dy == 0 {
            combat_log.push("Invalid aim direction!".into());
            continue;
        }

        // Misfire check: small chance the gun misfires (ammo wasted, no bullet).
        let misfire_roll = dynamic_rng.roll(seed.0, (origin.x as u64) << 32 | (origin.y as u64 & 0xFFFFFFFF) ^ 0xDEAD);
        if misfire_roll < MISFIRE_CHANCE {
            combat_log.push(format!("{c_name}'s gun misfires! *click*"));
            sound_events.add(origin);
            continue;
        }

        // Compute the bullet endpoint.
        let endpoint = bullet_endpoint(origin, dx, dy, intent.range);

        combat_log.push(format!("{c_name} fires!"));
        sound_events.add(origin);

        // Spawn gun smoke at the firing position (persists and blocks sight).
        spawn_gun_smoke(&mut game_map, origin, turn_counter.0, dx, dy);

        // Spawn a bullet projectile entity that will travel along the path.
        crate::systems::projectile::spawn_bullet(
            &mut commands,
            origin,
            endpoint,
            damage,
            intent.attacker,
        );
    }
}

/// Resolves AI ranged attack intents by spawning bullet projectile entities.
/// Fires a bullet from the attacker toward the target entity.
pub fn ai_ranged_attack_system(
    mut commands: Commands,
    mut intents: MessageReader<AiRangedAttackIntent>,
    attacker_query: Query<(&Position, &CombatStats, Option<&Name>)>,
    target_query: Query<&Position>,
    mut combat_log: ResMut<CombatLog>,
    mut sound_events: ResMut<SoundEvents>,
    mut game_map: ResMut<GameMapResource>,
    turn_counter: Res<TurnCounter>,
) {
    for intent in intents.read() {
        let Ok((attacker_pos, attacker_stats, attacker_name)) = attacker_query.get(intent.attacker) else {
            continue;
        };
        let Ok(target_pos) = target_query.get(intent.target) else {
            continue;
        };

        let origin = attacker_pos.as_grid_vec();
        let target_vec = target_pos.as_grid_vec();
        let a_name = display_name(attacker_name);

        // Use the actual direction delta for accurate aiming (not just signum).
        let dx = target_vec.x - origin.x;
        let dy = target_vec.y - origin.y;

        if dx == 0 && dy == 0 {
            continue;
        }

        let damage = attacker_stats.attack;

        // Compute bullet endpoint.
        let endpoint = bullet_endpoint(origin, dx, dy, intent.range);

        combat_log.push_at(format!("{a_name} fires!"), origin);
        sound_events.add(origin);

        // Spawn gun smoke at the firing position (persists and blocks sight).
        spawn_gun_smoke(&mut game_map, origin, turn_counter.0, dx, dy);

        // Spawn a bullet projectile entity.
        crate::systems::projectile::spawn_bullet(
            &mut commands,
            origin,
            endpoint,
            damage,
            intent.attacker,
        );
    }
}

/// Resolves roundhouse kick attack intents.
/// Hits all adjacent hostile entities (Chebyshev distance 1).
/// This is a powerful melee attack that sweeps all enemies around the player.
/// Uses `CombatStats::damage_against` for the formal damage model.
pub fn melee_wide_system(
    mut intents: MessageReader<MeleeWideIntent>,
    mut damage_events: MessageWriter<DamageEvent>,
    attacker_query: Query<(&Position, &CombatStats, Option<&Name>)>,
    targets: Query<(Entity, &Position, Option<&Name>), With<Hostile>>,
    mut combat_log: ResMut<CombatLog>,
    mut game_map: ResMut<GameMapResource>,
) {
    for intent in intents.read() {
        let Ok((attacker_pos, attacker_stats, attacker_name)) = attacker_query.get(intent.attacker) else {
            continue;
        };
        let origin = attacker_pos.as_grid_vec();
        let a_name = display_name(attacker_name);
        let mut hit_count = 0;

        for (target_entity, target_pos, target_name) in &targets {
            let dist = origin.chebyshev_distance(target_pos.as_grid_vec());
            if dist == 1 {
                let damage = attacker_stats.damage_against();
                let t_name = display_name(target_name);
                if damage > 0 {
                    damage_events.write(DamageEvent {
                        target: target_entity,
                        amount: damage,
                        source: Some(intent.attacker),
                    });
                    combat_log.push(format!("{a_name} roundhouse kicks {t_name} for {damage} damage!"));
                    hit_count += 1;
                } else {
                    combat_log.push(format!("{a_name} roundhouse kicks at {t_name} but deals no damage"));
                }
            }
        }

        // Destroy adjacent destructible furniture (Chebyshev distance 1).
        let mut furn_destroyed = 0;
        for dx in -1..=1i32 {
            for dy in -1..=1i32 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let tile = origin + GridVec::new(dx, dy);
                if let Some(voxel) = game_map.0.get_voxel_at_mut(&tile)
                    && let Some(ref furn) = voxel.furniture {
                        let is_indestructible = matches!(
                            furn,
                            Furniture::Wall
                            | Furniture::HitchingPost | Furniture::Rock
                        );
                        if !is_indestructible {
                            voxel.furniture = None;
                            furn_destroyed += 1;
                        }
                    }
            }
        }
        if furn_destroyed > 0 {
            combat_log.push(format!("{a_name} smashes {furn_destroyed} piece(s) of furniture!"));
        }

        if hit_count == 0 && furn_destroyed == 0 {
            combat_log.push(format!("{a_name} roundhouse kicks but hits nothing!"));
        }
    }
}
