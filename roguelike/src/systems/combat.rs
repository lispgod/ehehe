use bevy::prelude::*;

use crate::components::{AiState, Caliber, CollectibleKind, CombatStats, Dead, Faction, Health, Hostile, Inventory, Item, ItemKind, LastDamageSource, LootTable, Name, Player, Position, Renderable, display_name};
use crate::events::{AiRangedAttackIntent, AttackIntent, DamageEvent, MeleeWideIntent, RangedAttackIntent};
use crate::noise::value_noise;
use crate::resources::{CombatLog, DynamicRng, GameMapResource, GameState, KillCount, MapSeed, SoundEvents, TurnCounter};
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

/// Gun smoke scan radius (Chebyshev distance from the firing position).
const SMOKE_SCAN_RADIUS: i32 = 2;
/// Base effective radius for gun smoke (before directional bias).
const SMOKE_BASE_RADIUS: f64 = 0.8;
/// Additional radius in the firing direction (directional plume stretch).
const SMOKE_DIRECTIONAL_SCALE: f64 = 2.0;

/// Spawns a small cloud of gun smoke at the firing position.
/// Places persistent SandCloud floor tiles on the map that block sight.
/// Saves the previous floor type for restoration when smoke dissipates.
/// The smoke is biased in the firing direction for a more natural plume.
fn spawn_gun_smoke(game_map: &mut GameMapResource, origin: GridVec, turn: u32, fire_dx: i32, fire_dy: i32) {
    let flen = ((fire_dx as f64).powi(2) + (fire_dy as f64).powi(2)).sqrt();
    let dir = if flen > 0.01 {
        (fire_dx as f64 / flen, fire_dy as f64 / flen)
    } else {
        (0.0, 0.0)
    };
    game_map.place_sand_cloud(origin, turn, dir, SMOKE_SCAN_RADIUS, SMOKE_BASE_RADIUS, SMOKE_DIRECTIONAL_SCALE);
}

/// Resolves attack intents into damage events.
///
/// Damage = attacker.attack + bonus from a random inventory item's blunt_damage.
/// When an entity melee attacks (bump attack), a random item in their inventory
/// provides bonus blunt damage (e.g., pistol-whipping with a gun, smashing with
/// a bottle). Both players and NPCs use this system.
/// Uses `CombatStats::damage_against` for the base damage model.
/// Emits a `DamageEvent` for each successful hit and logs combat messages.
///
/// After dealing damage, adds `Hostile` to both attacker and target.
/// Nearby same-faction NPCs within 8 tiles also become hostile;
/// other-faction NPCs within 8 tiles start fleeing.
pub fn combat_system(
    mut commands: Commands,
    mut intents: MessageReader<AttackIntent>,
    mut damage_events: MessageWriter<DamageEvent>,
    stats_query: Query<(&CombatStats, Option<&Name>, Option<&Position>)>,
    inventory_query: Query<&Inventory>,
    item_kind_query: Query<&ItemKind>,
    dynamic_rng: Res<crate::resources::DynamicRng>,
    seed: Res<crate::resources::MapSeed>,
    mut combat_log: ResMut<CombatLog>,
    faction_query: Query<(Option<&Faction>, Option<&Position>)>,
    npc_query: Query<(Entity, &Position, Option<&Faction>), Without<Player>>,
    player_query: Query<Entity, With<Player>>,
    mut star_level: ResMut<crate::resources::StarLevel>,
) {
    // Collect aggro events to apply after processing all intents
    let mut aggro_targets: Vec<(Entity, Entity, Option<GridVec>)> = Vec::new(); // (attacker, target, target_pos)

    for intent in intents.read() {
        let Ok((attacker_stats, attacker_name, attacker_pos)) = stats_query.get(intent.attacker) else {
            continue;
        };
        let Ok((_target_stats, target_name, _)) = stats_query.get(intent.target) else {
            continue;
        };

        let base_damage = attacker_stats.damage_against();
        let a_name = display_name(attacker_name);
        let t_name = display_name(target_name);
        let pos = attacker_pos.map(|p| p.as_grid_vec());

        // Add bonus damage from a random inventory item's blunt_damage.
        let mut bonus = 0;
        let mut bonus_item_name: Option<String> = None;
        if let Ok(inv) = inventory_query.get(intent.attacker)
            && !inv.items.is_empty() {
                let idx = dynamic_rng.random_index(
                    seed.0,
                    intent.attacker.to_bits() ^ 0xB1A7,
                    inv.items.len(),
                );
                if let Ok(kind) = item_kind_query.get(inv.items[idx]) {
                    let bd = kind.blunt_damage();
                    if bd > 0 {
                        bonus = bd;
                        bonus_item_name = Some(kind.display_name());
                    }
                }
            }

        let damage = base_damage + bonus;

        if damage > 0 {
            if let Some(item_name) = bonus_item_name {
                combat_log.push_opt(format!("{a_name} hits {t_name} with {item_name} for {damage} damage"), pos);
            } else {
                combat_log.push_opt(format!("{a_name} hits {t_name} for {damage} damage"), pos);
            }
            damage_events.write(DamageEvent {
                target: intent.target,
                amount: damage,
                source: Some(intent.attacker),
            });
        } else {
            combat_log.push_opt(format!("{a_name} attacks {t_name} but deals no damage"), pos);
        }

        // Collect aggro info
        let target_pos = if let Ok((_, _, tp)) = stats_query.get(intent.target) {
            tp.map(|p| p.as_grid_vec())
        } else {
            None
        };
        aggro_targets.push((intent.attacker, intent.target, target_pos));
    }

    // Apply aggro: make attacker and target Hostile, propagate to nearby NPCs
    let player_entity = player_query.single().ok();
    for (attacker, target, target_pos) in aggro_targets {
        commands.entity(target).insert(Hostile);
        commands.entity(attacker).insert(Hostile);

        // Increase star level when the player attacks someone
        if player_entity == Some(attacker) {
            star_level.level = (star_level.level + 1).min(5);
            star_level.unseen_turns = 0;
        }

        // Get target's faction for propagation
        let target_faction = faction_query.get(target).ok().and_then(|(f, _)| f.copied());

        if let Some(t_pos) = target_pos {
            const AGGRO_RADIUS: i32 = 8;
            for (nearby_ent, nearby_pos, nearby_fac) in &npc_query {
                if nearby_ent == target || nearby_ent == attacker { continue; }
                let nv = nearby_pos.as_grid_vec();
                if nv.chebyshev_distance(t_pos) > AGGRO_RADIUS { continue; }

                if let Some(&nf) = nearby_fac {
                    if target_faction.is_some_and(|tf| tf == nf) {
                        // Same faction as target: become hostile too
                        commands.entity(nearby_ent).insert(Hostile);
                    } else {
                        // Different faction: start fleeing
                        commands.entity(nearby_ent).insert(AiState::Fleeing);
                    }
                }
            }
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
    item_query: Query<&ItemKind>,
    mut combat_log: ResMut<CombatLog>,
    mut kill_count: ResMut<KillCount>,
    mut next_game_state: ResMut<NextState<GameState>>,
    seed: Res<MapSeed>,
    spectating: Res<crate::resources::SpectatingAfterDeath>,
) {
    let player_entity = player_entities.single().ok();

    for (entity, health, name, hostile, pos, loot_table, is_player, last_damage_source, inventory, faction) in &query {
        if !health.is_dead() {
            continue;
        }

        // If the player died and we already processed their death (spectating),
        // skip entirely — don't re-log messages or re-trigger state changes.
        if is_player.is_some() && spectating.0 {
            continue;
        }

        let label = name.map_or("Something", |n| &n.0);
        combat_log.push_opt(format!("{label} has been slain!"), pos.map(|p| p.as_grid_vec()));

        // If the player died, transition to Dead state.
        // Spawn a corpse marker (X) at the player's position — same visual
        // treatment as NPC deaths. The player entity itself is NOT despawned
        // so the UI can continue reading stats (HP, inventory, etc.).
        if is_player.is_some() {
            combat_log.push("You have fallen... Press T to continue watching, or R to restart.".into());
            next_game_state.set(GameState::Dead);
            commands.entity(entity).insert(Dead);
            if let Some(p) = pos {
                commands.spawn((
                    Position { x: p.x, y: p.y },
                    Name(format!("{label}'s corpse")),
                    Renderable {
                        symbol: "X".into(),
                        fg: RatColor::Rgb(120, 60, 60),
                        bg: RatColor::Black,
                    },
                ));
            }
            continue; // don't despawn the player (UI reads stats from it)
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
        // Ammo drops now match the caliber of any gun the NPC was carrying.
        if !is_wildlife
            && let (Some(_lt), Some(p)) = (loot_table, pos) {
                // Drop collectible supplies (caps + powder + matching ammo).
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
                // Drop powder (needed for reloading).
                let powder_roll = value_noise(p.x.wrapping_add(kill_count.0 as i32 + 2), p.y, seed.0.wrapping_add(55555));
                if powder_roll < 0.4 {
                    let amount = ((powder_roll * 10.0) as i32).max(1);
                    commands.spawn((
                        Position { x: p.x, y: p.y },
                        Item,
                        Name(format!("{amount}x Powder")),
                        Renderable { symbol: "·".into(), fg: RatColor::Rgb(140, 140, 140), bg: RatColor::Black },
                        CollectibleKind::Powder(amount),
                    ));
                }
                // Drop ammo matching the NPC's gun caliber, if they had one.
                let ammo_roll = value_noise(p.y.wrapping_add(kill_count.0 as i32 + 1), p.x, seed.0.wrapping_add(44444));
                if ammo_roll < 0.4 {
                    let amount = ((ammo_roll * 15.0) as i32).max(1);
                    // Find the caliber of the NPC's gun from their inventory.
                    let npc_caliber = inventory.and_then(|inv| {
                        inv.items.iter().find_map(|&ent| {
                            item_query.get(ent).ok().and_then(|k| {
                                if let ItemKind::Gun { caliber, .. } = k { Some(*caliber) } else { None }
                            })
                        })
                    });
                    let (cal_name, collectible) = match npc_caliber {
                        Some(Caliber::Cal31) => (".31", CollectibleKind::Bullets31(amount)),
                        Some(Caliber::Cal36) => (".36", CollectibleKind::Bullets36(amount)),
                        Some(Caliber::Cal44) => (".44", CollectibleKind::Bullets44(amount)),
                        Some(Caliber::Cal50) => (".50", CollectibleKind::Bullets50(amount)),
                        Some(Caliber::Cal58) => (".58", CollectibleKind::Bullets58(amount)),
                        Some(Caliber::Cal577) => (".577", CollectibleKind::Bullets577(amount)),
                        Some(Caliber::Cal69) => (".69", CollectibleKind::Bullets69(amount)),
                        None => (".36", CollectibleKind::Bullets36(amount)),
                    };
                    commands.spawn((
                        Position { x: p.x, y: p.y },
                        Item,
                        Name(format!("{amount}x {cal_name} Bullets")),
                        Renderable { symbol: "·".into(), fg: RatColor::Rgb(180, 180, 180), bg: RatColor::Black },
                        collectible,
                    ));
                }
            }

        // Spawn a corpse marker for human (non-wildlife) NPCs.
        if !is_wildlife
            && let Some(p) = pos {
                commands.spawn((
                    Position { x: p.x, y: p.y },
                    Name(format!("{label}'s corpse")),
                    Renderable {
                        symbol: "X".into(),
                        fg: RatColor::Rgb(120, 60, 60),
                        bg: RatColor::Black,
                    },
                ));
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
    attacker_query: Query<(&Position, &CombatStats, Option<&Name>, Option<&Inventory>)>,
    target_query: Query<&Position>,
    mut item_kind_query: Query<&mut ItemKind>,
    mut combat_log: ResMut<CombatLog>,
    mut sound_events: ResMut<SoundEvents>,
    mut game_map: ResMut<GameMapResource>,
    turn_counter: Res<TurnCounter>,
) {
    for intent in intents.read() {
        let Ok((attacker_pos, attacker_stats, attacker_name, inventory)) = attacker_query.get(intent.attacker) else {
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

        // NPC consumes a loaded round from their gun, just like the player.
        let mut damage = attacker_stats.attack;
        if let Some(inv) = inventory {
            let mut fired = false;
            for &item_ent in &inv.items {
                if let Ok(mut kind) = item_kind_query.get_mut(item_ent)
                    && let ItemKind::Gun { ref mut loaded, attack, .. } = *kind
                        && *loaded > 0 {
                            *loaded -= 1;
                            damage = attack;
                            fired = true;
                            break;
                        }
            }
            if !fired {
                // No loaded gun — NPC can't fire this turn.
                continue;
            }
        }

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

/// Resolves roundhouse kick attack intents as an AoE push attack.
/// Attempts to push all adjacent entities (Chebyshev distance 1) away from the
/// attacker by one tile. Only pushes if the destination tile is unoccupied and passable.
/// Deals damage to all adjacent entities (not just hostile ones).
/// Also damages adjacent props; props are only destroyed when their health reaches zero.
///
/// Minimum prop damage per kick to ensure weak kicks still damage props.
const MIN_PROP_DAMAGE: i32 = 5;
pub fn melee_wide_system(
    mut commands: Commands,
    mut intents: MessageReader<MeleeWideIntent>,
    mut damage_events: MessageWriter<DamageEvent>,
    attacker_query: Query<(&Position, &CombatStats, Option<&Name>), With<Player>>,
    mut targets: Query<(Entity, &mut Position, Option<&Name>, Option<&Health>), Without<Player>>,
    blockers: Query<(), With<crate::components::BlocksMovement>>,
    mut combat_log: ResMut<CombatLog>,
    mut game_map: ResMut<GameMapResource>,
    spatial: Res<crate::resources::SpatialIndex>,
    mut prop_health: ResMut<crate::resources::PropHealth>,
) {
    for intent in intents.read() {
        let Ok((attacker_pos, attacker_stats, attacker_name)) = attacker_query.get(intent.attacker) else {
            continue;
        };
        let origin = attacker_pos.as_grid_vec();
        let a_name = display_name(attacker_name);
        let damage = attacker_stats.damage_against();
        let mut hit_count = 0;
        let mut push_count = 0;

        // Collect entities to push (can't modify positions while iterating spatially)
        let mut push_list: Vec<(Entity, GridVec, GridVec)> = Vec::new(); // (entity, current, push_to)

        for (target_entity, target_pos, target_name, _target_hp) in &targets {
            let tv = target_pos.as_grid_vec();
            let dist = origin.chebyshev_distance(tv);
            if dist != 1 || target_entity == intent.attacker {
                continue;
            }

            // Deal damage
            let t_name = display_name(target_name);
            if damage > 0 {
                damage_events.write(DamageEvent {
                    target: target_entity,
                    amount: damage,
                    source: Some(intent.attacker),
                });
                combat_log.push(format!("{a_name} kicks {t_name} for {damage} damage!"));
                hit_count += 1;
            }

            // Aggro the target
            commands.entity(target_entity).insert(Hostile);

            // Calculate push direction (away from attacker)
            let push_dir = (tv - origin).king_step();
            let push_to = tv + push_dir;

            // Check if push destination is passable and unoccupied
            let tile_passable = game_map.0.is_passable(&push_to);
            let entity_blocked = spatial.entities_at(&push_to).iter().any(|&e| {
                e != target_entity && blockers.contains(e)
            });

            if tile_passable && !entity_blocked {
                push_list.push((target_entity, tv, push_to));
            }
        }

        // Apply pushes
        for (entity, _from, to) in &push_list {
            if let Ok((_, mut pos, _, _)) = targets.get_mut(*entity) {
                pos.x = to.x;
                pos.y = to.y;
                push_count += 1;
            }
        }

        // Damage adjacent props (using prop health system)
        let mut props_damaged = 0;
        for dx in -1..=1i32 {
            for dy in -1..=1i32 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let tile = origin + GridVec::new(dx, dy);
                if let Some(voxel) = game_map.0.get_voxel_at(&tile) {
                    if let Some(ref prop) = voxel.props {
                        let max_hp = prop.max_health();
                        if max_hp == i32::MAX {
                            continue; // indestructible
                        }
                        // Initialize prop health if not yet tracked
                        let current_hp = prop_health.hp.entry(tile).or_insert(max_hp);
                        *current_hp -= damage.max(MIN_PROP_DAMAGE);
                        props_damaged += 1;
                        if *current_hp <= 0 {
                            // Destroy the prop
                            if let Some(voxel) = game_map.0.get_voxel_at_mut(&tile) {
                                voxel.props = None;
                            }
                            prop_health.hp.remove(&tile);
                            combat_log.push(format!("{a_name} destroys a prop!"));
                        }
                    }
                }
            }
        }

        if hit_count == 0 && props_damaged == 0 {
            combat_log.push(format!("{a_name} roundhouse kicks but hits nothing!"));
        } else if push_count > 0 {
            combat_log.push(format!("{a_name} pushes {push_count} away!"));
        }
    }
}
