use bevy::prelude::*;

use crate::components::{CollectibleKind, CombatStats, ExpReward, Experience, Health, HellGate, Hostile, Item, ItemKind, LastDamageSource, Level, LootTable, Stamina, Ammo, Name, Player, Position, Renderable};
use crate::events::{AiRangedAttackIntent, AttackIntent, DamageEvent, MeleeWideIntent, RangedAttackIntent};
use crate::noise::value_noise;
use crate::resources::{CombatLog, GameState, KillCount, MapSeed, PendingExp, PendingNpcExp, SoundEvents};
use crate::systems::inventory::spawn_loot;
use crate::grid_vec::GridVec;
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

/// Resolves attack intents into damage events.
///
/// Damage = max(0, attacker.attack − target.defense).
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
        let Ok((target_stats, target_name, _)) = stats_query.get(intent.target) else {
            continue;
        };

        let damage = attacker_stats.damage_against(target_stats);
        let a_name = attacker_name.map_or("???", |n| &n.0);
        let t_name = target_name.map_or("???", |n| &n.0);
        let pos = attacker_pos.map(|p| p.as_grid_vec());

        if damage > 0 {
            if let Some(p) = pos {
                combat_log.push_at(format!("{a_name} hits {t_name} for {damage} damage"), p);
            } else {
                combat_log.push(format!("{a_name} hits {t_name} for {damage} damage"));
            }
            damage_events.write(DamageEvent {
                target: intent.target,
                amount: damage,
                source: Some(intent.attacker),
            });
        } else if let Some(p) = pos {
            combat_log.push_at(format!("{a_name} attacks {t_name} but deals no damage"), p);
        } else {
            combat_log.push(format!("{a_name} attacks {t_name} but deals no damage"));
        }
    }
}

/// Applies damage events to entity health pools using `Health::apply_damage`.
/// Also records the damage source on the target for kill attribution.
pub fn apply_damage_system(
    mut commands: Commands,
    mut events: MessageReader<DamageEvent>,
    mut health_query: Query<&mut Health>,
) {
    for event in events.read() {
        if let Ok(mut health) = health_query.get_mut(event.target) {
            health.apply_damage(event.amount);
            if let Some(source) = event.source {
                commands.entity(event.target).insert(LastDamageSource(source));
            }
        }
    }
}

/// Despawns entities whose health has reached zero.
/// Logs a death message, increments the kill counter for hostile entities,
/// awards EXP to the PendingExp resource only when the player dealt the killing
/// blow, spawns loot from entities with a LootTable, and removes the entity.
/// If the Hell Gate is destroyed, transitions to the Victory state.
/// If the player dies, transitions to the Dead state.
/// NPCs that kill other entities also gain stat bonuses (enemy level-up).
pub fn death_system(
    mut commands: Commands,
    query: Query<(Entity, &Health, Option<&Name>, Option<&Hostile>, Option<&HellGate>, Option<&Position>, Option<&LootTable>, Option<&Player>, Option<&ExpReward>, Option<&LastDamageSource>)>,
    player_entities: Query<Entity, With<Player>>,
    mut combat_log: ResMut<CombatLog>,
    mut kill_count: ResMut<KillCount>,
    mut next_game_state: ResMut<NextState<GameState>>,
    seed: Res<MapSeed>,
    mut pending_exp: ResMut<PendingExp>,
    mut pending_npc_exp: ResMut<PendingNpcExp>,
) {
    let player_entity = player_entities.single().ok();

    for (entity, health, name, hostile, hell_gate, pos, loot_table, is_player, exp_reward, last_damage_source) in &query {
        if !health.is_dead() {
            continue;
        }

        let label = name.map_or("Something", |n| &n.0);
        if let Some(p) = pos {
            combat_log.push_at(format!("{label} has been slain!"), p.as_grid_vec());
        } else {
            combat_log.push(format!("{label} has been slain!"));
        }

        // If the player died, transition to Dead state (don't despawn so UI can read stats).
        if is_player.is_some() {
            combat_log.push("You have fallen... Press Esc to quit or R to restart.".into());
            next_game_state.set(GameState::Dead);
            continue; // don't despawn the player
        }

        if hostile.is_some() {
            let player_killed = player_entity.is_some_and(|pe|
                last_damage_source.is_some_and(|lds| lds.0 == pe)
            );
            if player_killed {
                kill_count.0 += 1;
                if let Some(reward) = exp_reward {
                    pending_exp.0 += reward.0;
                }
            } else if let Some(lds) = last_damage_source {
                // NPC killed this entity — queue EXP for enemy level-up.
                let reward = exp_reward.map(|er| er.0).unwrap_or(5);
                pending_npc_exp.entries.push((lds.0, reward));
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
                combat_log.push_at(format!("{label} dropped an item!"), p.as_grid_vec());
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

/// Applies pending NPC EXP and handles NPC level-ups.
/// Runs after the death system each frame.
pub fn npc_level_up_system(
    mut npc_query: Query<(&mut Experience, &mut Level, &mut CombatStats, &mut Health, Option<&Name>, Option<&Position>), (With<Hostile>, Without<Player>)>,
    mut pending_npc_exp: ResMut<PendingNpcExp>,
    mut combat_log: ResMut<CombatLog>,
) {
    for (killer_entity, reward) in pending_npc_exp.entries.drain(..) {
        if let Ok((mut exp, mut level, mut stats, mut hp, killer_name, killer_pos)) = npc_query.get_mut(killer_entity) {
            exp.current += reward;
            while exp.current >= exp.next_level {
                exp.current -= exp.next_level;
                level.0 += 1;
                exp.next_level = 20 + (level.0 - 1) * 10;
                stats.attack += 1;
                stats.defense += 1;
                hp.max += 3;
                hp.current = hp.max;
                let k_name = killer_name.map_or("???", |n| &n.0);
                if let Some(p) = killer_pos {
                    combat_log.push_at(format!("{k_name} levels up to {}!", level.0), p.as_grid_vec());
                }
            }
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

/// Resolves targeted ranged attack intents by spawning bullet projectile entities.
///
/// The bullet path is computed using **Bresenham's line algorithm** from the
/// caster's position to the maximum range endpoint. Instead of applying damage
/// instantly, a bullet entity is spawned that travels along the path over
/// multiple ticks. Damage is applied when the projectile reaches a hostile.
///
/// Consumes 1 ammo per shot from the gun item or global Ammo pool.
pub fn ranged_attack_system(
    mut commands: Commands,
    mut intents: MessageReader<RangedAttackIntent>,
    mut caster_query: Query<(&Position, &mut Ammo, &CombatStats, Option<&Name>), With<Player>>,
    mut combat_log: ResMut<CombatLog>,
    mut item_kind_query: Query<&mut ItemKind>,
    mut sound_events: ResMut<SoundEvents>,
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
            if ammo.is_empty() {
                combat_log.push("Out of ammo!".into());
                continue;
            }
            ammo.spend_one();
            damage = caster_stats.attack;
        }

        let dx = intent.dx;
        let dy = intent.dy;

        if dx == 0 && dy == 0 {
            combat_log.push("Invalid aim direction!".into());
            continue;
        }

        // Compute the bullet endpoint.
        let endpoint = bullet_endpoint(origin, dx, dy, intent.range);

        combat_log.push(format!("{c_name} fires!"));
        sound_events.add(origin);

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
    target_query: Query<(&Position, Option<&Name>)>,
    mut combat_log: ResMut<CombatLog>,
    mut sound_events: ResMut<SoundEvents>,
) {
    for intent in intents.read() {
        let Ok((attacker_pos, attacker_stats, attacker_name)) = attacker_query.get(intent.attacker) else {
            continue;
        };
        let Ok((target_pos, _target_name)) = target_query.get(intent.target) else {
            continue;
        };

        let origin = attacker_pos.as_grid_vec();
        let target_vec = target_pos.as_grid_vec();
        let a_name = attacker_name.map_or("???", |n| &n.0);

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
                let damage = attacker_stats.damage_against(target_stats);
                let t_name = target_name.map_or("???", |n| &n.0);
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

        if hit_count == 0 {
            combat_log.push(format!("{a_name} roundhouse kicks but hits nothing!"));
        }
    }
}
