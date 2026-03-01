use bevy::prelude::*;

use crate::components::{CombatStats, ExpReward, Experience, Health, HellGate, Hostile, Level, LootTable, Mana, Name, Player, Position};
use crate::events::{AttackIntent, DamageEvent, MeleeWideIntent, RangedAttackIntent};
use crate::noise::value_noise;
use crate::resources::{CombatLog, GameState, KillCount, MapSeed, PendingExp};
use crate::systems::inventory::spawn_loot;

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
            }

            commands.entity(entity).despawn();
        }
    }
}

/// Applies pending EXP to the player and handles level-ups.
/// Runs after the death system each frame.
pub fn level_up_system(
    mut player_query: Query<(&mut Experience, &mut Level, &mut CombatStats, &mut Health, &mut Mana), With<Player>>,
    mut pending_exp: ResMut<PendingExp>,
    mut combat_log: ResMut<CombatLog>,
) {
    if pending_exp.0 <= 0 {
        return;
    }

    let Ok((mut exp, mut level, mut stats, mut hp, mut mana)) = player_query.single_mut() else {
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
        mana.max += 5;
        mana.current = mana.max;
        combat_log.push(format!(
            "LEVEL UP! Now level {}! ATK {} DEF {} HP {} MP {}",
            level.0, stats.attack, stats.defense, hp.max, mana.max
        ));
    }
}

/// Resolves targeted ranged attack intents.
/// Finds the nearest visible hostile within range and deals damage.
pub fn ranged_attack_system(
    mut intents: MessageReader<RangedAttackIntent>,
    mut damage_events: MessageWriter<DamageEvent>,
    caster_query: Query<(&Position, &CombatStats, Option<&Name>), With<Player>>,
    targets: Query<(Entity, &Position, Option<&Name>), With<Hostile>>,
    mut combat_log: ResMut<CombatLog>,
) {
    for intent in intents.read() {
        let Ok((caster_pos, caster_stats, caster_name)) = caster_query.get(intent.attacker) else {
            continue;
        };
        let origin = caster_pos.as_grid_vec();
        let c_name = caster_name.map_or("???", |n| &n.0);

        // Find the nearest hostile within range.
        let mut best: Option<(Entity, i32, String)> = None;
        for (target_entity, target_pos, target_name) in &targets {
            let dist = origin.chebyshev_distance(target_pos.as_grid_vec());
            if dist > 0 && dist <= intent.range {
                if best.as_ref().map_or(true, |(_, best_dist, _)| dist < *best_dist) {
                    let t_name = target_name.map_or("???".to_string(), |n| n.0.clone());
                    best = Some((target_entity, dist, t_name));
                }
            }
        }

        if let Some((target_entity, _dist, t_name)) = best {
            let damage = caster_stats.attack;
            if damage > 0 {
                damage_events.write(DamageEvent {
                    target: target_entity,
                    amount: damage,
                });
                combat_log.push(format!("{c_name} shoots {t_name} for {damage} damage!"));
            }
        } else {
            combat_log.push(format!("{c_name} aims but finds no target in range!"));
        }
    }
}

/// Resolves melee wide (cleave) attack intents.
/// Hits all adjacent hostile entities (Chebyshev distance 1).
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
                    combat_log.push(format!("{a_name} cleaves {t_name} for {damage} damage!"));
                    hit_count += 1;
                } else {
                    combat_log.push(format!("{a_name} cleaves at {t_name} but deals no damage"));
                }
            }
        }

        if hit_count == 0 {
            combat_log.push(format!("{a_name} swings wide but hits nothing!"));
        }
    }
}
