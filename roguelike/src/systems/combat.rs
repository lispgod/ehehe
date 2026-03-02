use bevy::prelude::*;

use crate::components::{CollectibleKind, CombatStats, ExpReward, Experience, Health, HellGate, Hostile, Item, ItemKind, Level, LootTable, Stamina, Ammo, Name, Player, Position, Renderable};
use crate::events::{AiRangedAttackIntent, AttackIntent, DamageEvent, MeleeWideIntent, RangedAttackIntent};
use crate::noise::value_noise;
use crate::resources::{CombatLog, GameMapResource, GameState, KillCount, MapSeed, PendingExp};
use crate::systems::inventory::spawn_loot;
use crate::typedefs::RatColor;

/// Resolves attack intents into damage events.
///
/// Damage = max(0, attacker.attack − target.defense).
/// Emits a `DamageEvent` for each successful hit and logs combat messages.
pub fn combat_system(
    mut intents: MessageReader<AttackIntent>,
    mut damage_events: MessageWriter<DamageEvent>,
    stats_query: Query<(&CombatStats, Option<&Name>)>,
    mut combat_log: ResMut<CombatLog>,
) {
    for intent in intents.read() {
        let Ok((attacker_stats, attacker_name)) = stats_query.get(intent.attacker) else {
            continue;
        };
        let Ok((target_stats, target_name)) = stats_query.get(intent.target) else {
            continue;
        };

        let damage = (attacker_stats.attack - target_stats.defense).max(0);
        let a_name = attacker_name.map_or("???", |n| &n.0);
        let t_name = target_name.map_or("???", |n| &n.0);

        if damage > 0 {
            combat_log.push(format!("{a_name} hits {t_name} for {damage} damage"));
            damage_events.write(DamageEvent {
                target: intent.target,
                amount: damage,
            });
        } else {
            combat_log.push(format!("{a_name} attacks {t_name} but deals no damage"));
        }
    }
}

/// Applies damage events to entity health pools.
pub fn apply_damage_system(
    mut events: MessageReader<DamageEvent>,
    mut health_query: Query<&mut Health>,
) {
    for event in events.read() {
        if let Ok(mut health) = health_query.get_mut(event.target) {
            health.current = (health.current - event.amount).max(0);
        }
    }
}

/// Despawns entities whose health has reached zero.
/// Logs a death message, increments the kill counter for hostile entities,
/// awards EXP to the PendingExp resource, spawns loot from entities with a LootTable,
/// and removes the entity from the world.
/// If the Hell Gate is destroyed, transitions to the Victory state.
/// If the player dies, transitions to the Dead state.
pub fn death_system(
    mut commands: Commands,
    query: Query<(Entity, &Health, Option<&Name>, Option<&Hostile>, Option<&HellGate>, Option<&Position>, Option<&LootTable>, Option<&Player>, Option<&ExpReward>)>,
    mut combat_log: ResMut<CombatLog>,
    mut kill_count: ResMut<KillCount>,
    mut next_game_state: ResMut<NextState<GameState>>,
    seed: Res<MapSeed>,
    mut pending_exp: ResMut<PendingExp>,
) {
    for (entity, health, name, hostile, hell_gate, pos, loot_table, is_player, exp_reward) in &query {
        if health.current <= 0 {
            let label = name.map_or("Something", |n| &n.0);
            combat_log.push(format!("{label} has been slain!"));

            // If the player died, transition to Dead state (don't despawn so UI can read stats).
            if is_player.is_some() {
                combat_log.push("You have fallen... Press Q to quit or R to restart.".into());
                next_game_state.set(GameState::Dead);
                continue; // don't despawn the player
            }

            if hostile.is_some() {
                kill_count.0 += 1;
                // Award EXP from killed hostile.
                if let Some(reward) = exp_reward {
                    pending_exp.0 += reward.0;
                }
            }
            if hell_gate.is_some() {
                combat_log.push("The Enemy Stronghold crumbles! You are victorious!".into());
                next_game_state.set(GameState::Victory);
            }

            // Loot drop: if the entity has a LootTable, roll for item drop.
            if let (Some(lt), Some(p)) = (loot_table, pos) {
                let drop_roll = value_noise(p.x.wrapping_add(kill_count.0 as i32), p.y, seed.0.wrapping_add(55555));
                if drop_roll < lt.drop_chance {
                    let item_roll = value_noise(p.y, p.x.wrapping_add(kill_count.0 as i32), seed.0.wrapping_add(77777));
                    spawn_loot(&mut commands, p.x, p.y, item_roll);
                    combat_log.push(format!("{label} dropped an item!"));
                }

                // Also drop collectible supplies (caps + random ammo).
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
}

/// Applies pending EXP to the player and handles level-ups.
/// Runs after the death system each frame.
pub fn level_up_system(
    mut player_query: Query<(&mut Experience, &mut Level, &mut CombatStats, &mut Health, &mut Stamina), With<Player>>,
    mut pending_exp: ResMut<PendingExp>,
    mut combat_log: ResMut<CombatLog>,
) {
    if pending_exp.0 <= 0 {
        return;
    }

    let Ok((mut exp, mut level, mut stats, mut hp, mut stamina)) = player_query.single_mut() else {
        pending_exp.0 = 0;
        return;
    };

    exp.current += pending_exp.0;
    combat_log.push(format!("+{} EXP", pending_exp.0));
    pending_exp.0 = 0;

    // Check for level-up(s).
    while exp.current >= exp.next_level {
        exp.current -= exp.next_level;
        level.0 += 1;
        // Scale next-level requirement.
        exp.next_level = 20 + (level.0 - 1) * 10;
        // Stat bonuses per level.
        stats.attack += 1;
        stats.defense += 1;
        hp.max += 5;
        hp.current = hp.max; // full heal on level up
        stamina.max += 5;
        stamina.current = stamina.max;
        combat_log.push(format!(
            "LEVEL UP! Now level {}! ATK {} DEF {} HP {} STA {}",
            level.0, stats.attack, stats.defense, hp.max, stamina.max
        ));
    }
}

/// Resolves targeted ranged attack intents using player-chosen trajectory.
///
/// The bullet path is computed using **Bresenham's line algorithm** from the
/// caster's position to the maximum range endpoint. This provides a
/// mathematically correct, integer-only trajectory that visits each tile
/// exactly once. Targets are hit if and only if the Bresenham line passes
/// through their tile, eliminating the previous fuzzy directional heuristic.
///
/// The bullet penetrates through multiple enemies: remaining penetration
/// decreases by each target's defense value. Consumes 1 ammo per shot.
/// Also spawns bullet travel particles for visual feedback.
pub fn ranged_attack_system(
    mut intents: MessageReader<RangedAttackIntent>,
    mut damage_events: MessageWriter<DamageEvent>,
    mut caster_query: Query<(&Position, &mut Ammo, &CombatStats, Option<&Name>), With<Player>>,
    targets: Query<(Entity, &Position, &CombatStats, Option<&Name>), With<Hostile>>,
    mut combat_log: ResMut<CombatLog>,
    mut spell_particles: ResMut<crate::resources::SpellParticles>,
    game_map: Res<GameMapResource>,
    mut item_kind_query: Query<&mut ItemKind>,
) {
    for intent in intents.read() {
        let Ok((caster_pos, mut ammo, caster_stats, caster_name)) = caster_query.get_mut(intent.attacker) else {
            continue;
        };
        let origin = caster_pos.as_grid_vec();
        let c_name = caster_name.map_or("???", |n| &n.0);

        // Determine damage and consume ammo from either a gun item or the global Ammo pool.
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
            // Legacy path: use global Ammo pool.
            if ammo.current <= 0 {
                combat_log.push("Out of ammo!".into());
                continue;
            }
            ammo.current -= 1;
            damage = caster_stats.attack;
        }

        let dx = intent.dx;
        let dy = intent.dy;

        if dx == 0 && dy == 0 {
            combat_log.push("Invalid aim direction!".into());
            continue;
        }

        let mut penetration = damage;
        let mut hit_count = 0;

        // Compute the bullet endpoint: scale the direction vector so the
        // Bresenham line extends approximately `range` tiles along the
        // dominant axis, preserving the exact trajectory angle.
        let max_comp = dx.abs().max(dy.abs());
        let scale = intent.range.div_euclid(max_comp).max(1);
        let endpoint = origin + crate::grid_vec::GridVec::new(dx * scale, dy * scale);

        // Generate the bullet path using Bresenham's line algorithm.
        // This produces the mathematically correct sequence of tiles the
        // bullet traverses — integer-only, no floating-point, O(range).
        let bullet_path = origin.bresenham_line(endpoint);

        // Build a lookup of hostile entities by position for O(1) hit detection.
        let mut target_by_pos: std::collections::HashMap<crate::grid_vec::GridVec, (Entity, i32, String)> =
            std::collections::HashMap::new();
        for (target_entity, target_pos, target_stats, target_name) in &targets {
            let target_vec = target_pos.as_grid_vec();
            let t_name = target_name.map_or("???".to_string(), |n| n.0.clone());
            target_by_pos.insert(target_vec, (target_entity, target_stats.defense, t_name));
        }

        // Walk the bullet path (skip index 0 which is the caster's tile).
        for (step_idx, &tile) in bullet_path.iter().enumerate().skip(1) {
            // Spawn particle for visual feedback.
            spell_particles.particles.push((tile, 3, (step_idx as u32).saturating_sub(1)));

            // Stop the bullet if it hits an impassable wall.
            if !game_map.0.is_passable(&tile) {
                break;
            }

            // Check if a hostile entity is at this tile.
            if let Some((target_entity, target_def, t_name)) = target_by_pos.get(&tile) {
                if penetration <= 0 {
                    break;
                }
                let hit_damage = penetration;
                damage_events.write(DamageEvent {
                    target: *target_entity,
                    amount: hit_damage,
                });
                combat_log.push(format!("{c_name} shoots {t_name} for {hit_damage} damage!"));
                hit_count += 1;
                penetration -= target_def;
            }
        }

        if hit_count == 0 {
            combat_log.push(format!("{c_name} fires but the bullet misses!"));
        } else if hit_count > 1 {
            combat_log.push(format!("Bullet penetrated through {hit_count} targets!"));
        }
    }
}

/// Resolves AI ranged attack intents (used by soldier enemies).
/// Fires a bullet from the attacker toward the target entity.
pub fn ai_ranged_attack_system(
    mut intents: MessageReader<AiRangedAttackIntent>,
    mut damage_events: MessageWriter<DamageEvent>,
    attacker_query: Query<(&Position, &CombatStats, Option<&Name>)>,
    target_query: Query<(&Position, Option<&Name>)>,
    mut combat_log: ResMut<CombatLog>,
    mut spell_particles: ResMut<crate::resources::SpellParticles>,
) {
    for intent in intents.read() {
        let Ok((attacker_pos, attacker_stats, attacker_name)) = attacker_query.get(intent.attacker) else {
            continue;
        };
        let Ok((target_pos, target_name)) = target_query.get(intent.target) else {
            continue;
        };

        let origin = attacker_pos.as_grid_vec();
        let target_vec = target_pos.as_grid_vec();
        let a_name = attacker_name.map_or("???", |n| &n.0);
        let t_name = target_name.map_or("???", |n| &n.0);

        let dx = (target_vec.x - origin.x).signum();
        let dy = (target_vec.y - origin.y).signum();

        if dx == 0 && dy == 0 {
            continue;
        }

        let damage = attacker_stats.attack;

        // Spawn bullet travel particles.
        let dist = origin.chebyshev_distance(target_vec);
        let travel = dist.min(intent.range);
        for step in 1..=travel {
            let bullet_pos = origin + crate::grid_vec::GridVec::new(dx * step, dy * step);
            spell_particles.particles.push((bullet_pos, 3, (step as u32).saturating_sub(1)));
        }

        if damage > 0 {
            damage_events.write(DamageEvent {
                target: intent.target,
                amount: damage,
            });
            combat_log.push(format!("{a_name} shoots {t_name} for {damage} damage!"));
        }
    }
}

/// Resolves roundhouse kick attack intents.
/// Hits all adjacent hostile entities (Chebyshev distance 1).
/// This is a powerful melee attack that sweeps all enemies around the player.
pub fn melee_wide_system(
    mut intents: MessageReader<MeleeWideIntent>,
    mut damage_events: MessageWriter<DamageEvent>,
    attacker_query: Query<(&Position, &CombatStats, Option<&Name>)>,
    targets: Query<(Entity, &Position, &CombatStats, Option<&Name>), With<Hostile>>,
    mut combat_log: ResMut<CombatLog>,
) {
    for intent in intents.read() {
        let Ok((attacker_pos, attacker_stats, attacker_name)) = attacker_query.get(intent.attacker) else {
            continue;
        };
        let origin = attacker_pos.as_grid_vec();
        let a_name = attacker_name.map_or("???", |n| &n.0);
        let mut hit_count = 0;

        for (target_entity, target_pos, target_stats, target_name) in &targets {
            let dist = origin.chebyshev_distance(target_pos.as_grid_vec());
            if dist == 1 {
                let damage = (attacker_stats.attack - target_stats.defense).max(0);
                let t_name = target_name.map_or("???", |n| &n.0);
                if damage > 0 {
                    damage_events.write(DamageEvent {
                        target: target_entity,
                        amount: damage,
                    });
                    combat_log.push(format!("{a_name} roundhouse kicks {t_name} for {damage} damage!"));
                    hit_count += 1;
                } else {
                    combat_log.push(format!("{a_name} roundhouse kicks at {t_name} but deals no damage"));
                }
            }
        }

        if hit_count == 0 {
            combat_log.push(format!("{a_name} roundhouse kicks but hits nothing!"));
        }
    }
}
