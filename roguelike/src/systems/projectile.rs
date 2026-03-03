use bevy::prelude::*;

use crate::components::{Health, Hostile, Name, Player, Position, Projectile, Renderable, display_name};
use crate::events::DamageEvent;
use crate::grid_vec::GridVec;
use crate::noise::value_noise;
use crate::resources::{CombatLog, GameMapResource, SoundEvents};
use crate::typedefs::RatColor;

/// Bullet travel speed in tiles per game turn.
/// Each time the player or world takes a turn the bullet advances this many
/// tiles along its Bresenham path, then freezes in mid-air (with a blinking
/// render) until the next turn.
pub const BULLET_TILES_PER_TICK: usize = 12;

/// Shrapnel travel speed in tiles per tick.
pub const SHRAPNEL_TILES_PER_TICK: usize = 1;

/// Knife/Tomahawk travel speed in tiles per tick.
pub const THROWN_TILES_PER_TICK: usize = 2;

/// Maximum range for thrown knives and tomahawks (in tiles).
pub const THROWN_RANGE: i32 = 12;

/// Shrapnel self-damage multiplier (fraction of original damage dealt to the caster).
/// Shrapnel that hits the player who threw the grenade deals reduced damage.
const SELF_DAMAGE_DIVISOR: i32 = 2;

/// Result of resolving a bullet hit-chance roll.
enum BulletHitResult {
    Miss,
    Headshot { damage: i32 },
    Hit { damage: i32 },
}

/// Resolves bullet hit-chance, headshot, and miss rolls for a single target.
/// Returns the outcome so callers can handle damage events and logging.
fn resolve_bullet_hit(
    tile: GridVec,
    aim_point: GridVec,
    path_index: usize,
    target_max_hp: i32,
    penetration: i32,
) -> BulletHitResult {
    let dx = (aim_point.x - tile.x) as f64;
    let dy = (aim_point.y - tile.y) as f64;
    let distance = (dx * dx + dy * dy).sqrt();
    let hit_chance = (0.98 - distance * 0.02).clamp(0.35, 0.98);
    let headshot_chance = 0.02 + if distance < 0.5 { 0.08 } else { 0.0 };

    let roll_seed = 7919_u64.wrapping_add(path_index as u64);
    let roll = value_noise(tile.x, tile.y + path_index as i32, roll_seed);

    if roll > hit_chance {
        return BulletHitResult::Miss;
    }

    let hs_roll = value_noise(tile.x, tile.y + path_index as i32, roll_seed.wrapping_add(13));
    if hs_roll < headshot_chance {
        return BulletHitResult::Headshot { damage: target_max_hp };
    }

    BulletHitResult::Hit { damage: penetration }
}

/// Spawns a bullet projectile entity along a Bresenham line from origin to endpoint.
pub fn spawn_bullet(
    commands: &mut Commands,
    origin: GridVec,
    endpoint: GridVec,
    damage: i32,
    source: Entity,
) {
    let path = origin.bresenham_line(endpoint);
    if path.len() <= 1 {
        return;
    }
    let start_pos = path.get(1).copied().unwrap_or(origin);
    commands.spawn((
        Position { x: start_pos.x, y: start_pos.y },
        Renderable {
            symbol: "•".into(),
            fg: RatColor::Rgb(255, 200, 80),
            bg: RatColor::Black,
        },
        Projectile {
            path,
            path_index: 1, // skip origin (index 0)
            tiles_per_tick: BULLET_TILES_PER_TICK,
            damage,
            penetration: damage,
            source,
        },
    ));
}

/// Spawns shrapnel projectile entities for a grenade blast.
/// One projectile per radial direction within the blast radius.
pub fn spawn_shrapnel(
    commands: &mut Commands,
    origin: GridVec,
    radius: i32,
    damage: i32,
    source: Entity,
) {
    for dx in -radius..=radius {
        for dy in -radius..=radius {
            let dist = dx.abs().max(dy.abs());
            if dist == 0 || dist > radius {
                continue;
            }
            let endpoint = origin + GridVec::new(dx, dy);
            let path = origin.bresenham_line(endpoint);
            if path.len() <= 1 {
                continue;
            }
            let start_pos = path.get(1).copied().unwrap_or(origin);
            commands.spawn((
                Position { x: start_pos.x, y: start_pos.y },
                Renderable {
                    symbol: "·".into(),
                    fg: RatColor::Rgb(255, 165, 0),
                    bg: RatColor::Black,
                },
                Projectile {
                    path,
                    path_index: 1,
                    tiles_per_tick: SHRAPNEL_TILES_PER_TICK,
                    damage,
                    penetration: damage,
                    source,
                },
            ));
        }
    }
}

/// Advances all projectile entities along their paths each tick.
/// When a projectile reaches a tile with a hostile entity, it applies damage.
/// Projectiles are despawned when they reach the end of their path, hit a wall,
/// or run out of penetration power.
pub fn projectile_system(
    mut commands: Commands,
    mut projectiles: Query<(Entity, &mut Position, &mut Projectile, &mut Renderable), Without<crate::components::ThrownExplosive>>,
    mut damage_events: MessageWriter<DamageEvent>,
    targets: Query<(Entity, &Position, &Health, Option<&Name>), (With<Hostile>, Without<Projectile>)>,
    player_query: Query<(Entity, &Position, &Health, Option<&Name>), (With<Player>, Without<Projectile>)>,
    source_names: Query<Option<&Name>>,
    game_map: Res<GameMapResource>,
    mut combat_log: ResMut<CombatLog>,
    mut sound_events: ResMut<SoundEvents>,
    turn_state: Option<Res<State<crate::resources::TurnState>>>,
) {
    // Projectiles only advance during actual game turns (PlayerTurn / WorldTurn).
    // During AwaitingInput they freeze in mid-air with the blinking render.
    let is_awaiting = turn_state.as_ref()
        .is_some_and(|ts| *ts.get() == crate::resources::TurnState::AwaitingInput);
    if is_awaiting {
        return;
    }
    // Build a lookup of hostile entities by position for O(1) hit detection.
    let mut target_by_pos: std::collections::HashMap<GridVec, Vec<(Entity, String, i32)>> =
        std::collections::HashMap::new();
    for (target_entity, target_pos, target_health, target_name) in &targets {
        let t_name = display_name(target_name).to_string();
        target_by_pos
            .entry(target_pos.as_grid_vec())
            .or_default()
            .push((target_entity, t_name, target_health.max));
    }

    // Player position for NPC bullet hits and shrapnel self-damage.
    let player_info = player_query.single().ok();

    for (proj_entity, mut proj_pos, mut proj, mut renderable) in &mut projectiles {
        let mut despawn = false;
        let steps = proj.tiles_per_tick;

        // Look up the name of the entity that fired this projectile.
        let source_name = display_name(source_names.get(proj.source).ok().flatten());

        for _ in 0..steps {
            // Check current tile for damage before advancing.
            let tile = proj.path[proj.path_index];
            proj_pos.x = tile.x;
            proj_pos.y = tile.y;

            // Stop if hitting an impassable wall.
            if !game_map.0.is_passable(&tile) {
                sound_events.add(tile);
                despawn = true;
                break;
            }

            // Check for hostile entities at this tile.
            // Penetration model: the first hit deals full penetration damage.
            // Penetration is not reduced on hit.
            if let Some(entities_here) = target_by_pos.get(&tile) {
                for (target_entity, t_name, t_max_hp) in entities_here {
                    if proj.penetration <= 0 {
                        break;
                    }

                    // Chance-to-hit for bullets (shrapnel always hits).
                    let is_bullet = proj.tiles_per_tick == BULLET_TILES_PER_TICK;
                    if is_bullet {
                        let aim_point = proj.path.last().copied().unwrap_or(tile);
                        match resolve_bullet_hit(tile, aim_point, proj.path_index, *t_max_hp, proj.penetration) {
                            BulletHitResult::Miss => {
                                combat_log.push_at(
                                    format!("{source_name}'s bullet barely misses {t_name}!"),
                                    tile,
                                );
                                continue;
                            }
                            BulletHitResult::Headshot { damage: hs_damage } => {
                                damage_events.write(DamageEvent {
                                    target: *target_entity,
                                    amount: hs_damage,
                                    source: Some(proj.source),
                                });
                                combat_log.push_at(
                                    format!("{source_name} headshots {t_name}!"),
                                    tile,
                                );
                                sound_events.add(tile);
                                continue;
                            }
                            BulletHitResult::Hit { .. } => {
                                // Fall through to normal hit handling below,
                                // which uses proj.penetration (same value).
                            }
                        }
                    }

                    let hit_damage = proj.penetration;
                    damage_events.write(DamageEvent {
                        target: *target_entity,
                        amount: hit_damage,
                        source: Some(proj.source),
                    });
                    combat_log.push_at(
                        format!("{source_name}'s bullet hits {t_name} for {hit_damage} damage!"),
                        tile,
                    );
                    sound_events.add(tile);
                }
                if proj.penetration <= 0 {
                    despawn = true;
                    break;
                }
            }

            // NPC bullet hitting the player: check if the projectile source
            // is NOT the player and it landed on the player's tile.
            // Always stop the bullet after hitting the player to prevent
            // any possibility of double damage.
            if let Some((player_entity, player_pos, player_health, player_name)) = &player_info
                && proj.source != *player_entity
                && tile == player_pos.as_grid_vec()
                && proj.penetration > 0
            {
                // Chance-to-hit for bullets (shrapnel always hits).
                let is_bullet = proj.tiles_per_tick == BULLET_TILES_PER_TICK;
                if is_bullet {
                    let aim_point = proj.path.last().copied().unwrap_or(tile);
                    let p_name = display_name(*player_name);
                    match resolve_bullet_hit(tile, aim_point, proj.path_index, player_health.max, proj.penetration) {
                        BulletHitResult::Miss => {
                            combat_log.push(format!("{source_name}'s bullet barely misses {p_name}!"));
                            // Bullet continues through on miss — don't despawn.
                        }
                        BulletHitResult::Headshot { damage: hs_damage } => {
                            damage_events.write(DamageEvent {
                                target: *player_entity,
                                amount: hs_damage,
                                source: Some(proj.source),
                            });
                            combat_log.push(format!("{source_name} headshots {p_name}!"));
                            sound_events.add(tile);
                            despawn = true;
                            break;
                        }
                        BulletHitResult::Hit { damage: hit_damage } => {
                            damage_events.write(DamageEvent {
                                target: *player_entity,
                                amount: hit_damage,
                                source: Some(proj.source),
                            });
                            combat_log.push(format!("{source_name}'s bullet hits {p_name} for {hit_damage} damage!"));
                            sound_events.add(tile);
                            despawn = true;
                            break;
                        }
                    }
                } else {
                    // Non-bullet (shrapnel) always hits.
                    let hit_damage = proj.penetration;
                    damage_events.write(DamageEvent {
                        target: *player_entity,
                        amount: hit_damage,
                        source: Some(proj.source),
                    });
                    let p_name = display_name(*player_name);
                    combat_log.push(format!("Shrapnel hits {p_name} for {hit_damage} damage!"));
                    sound_events.add(tile);
                    despawn = true;
                    break;
                }
            }

            // Shrapnel self-damage: if the projectile's source is the player
            // and the projectile lands on the player's tile.
            if let Some((player_entity, player_pos, _, _)) = &player_info
                && proj.source == *player_entity && tile == player_pos.as_grid_vec() {
                    let self_damage = (proj.damage / SELF_DAMAGE_DIVISOR).max(1);
                    damage_events.write(DamageEvent {
                        target: *player_entity,
                        amount: self_damage,
                        source: Some(proj.source),
                    });
                    combat_log.push(format!("Shrapnel hits you for {self_damage} damage!"));
                    despawn = true;
                    break;
                }

            // Advance to next tile.
            proj.path_index += 1;
            if proj.path_index >= proj.path.len() {
                despawn = true;
                break;
            }
        }

        // Fade the renderable as projectile nears end of path.
        let remaining = proj.path.len().saturating_sub(proj.path_index);
        if remaining <= 2 {
            renderable.symbol = "·".into();
            renderable.fg = RatColor::Rgb(180, 120, 0);
        } else if remaining <= 4 {
            renderable.symbol = "·".into();
            renderable.fg = RatColor::Rgb(255, 180, 40);
        }

        if despawn {
            commands.entity(proj_entity).despawn();
        }
    }
}
