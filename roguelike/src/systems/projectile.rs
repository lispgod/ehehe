use bevy::prelude::*;

use crate::components::{CombatStats, Hostile, Name, Player, Position, Projectile, Renderable};
use crate::events::DamageEvent;
use crate::grid_vec::GridVec;
use crate::resources::{CombatLog, GameMapResource, SoundEvents};
use crate::typedefs::RatColor;

/// Bullet travel speed in tiles per tick.
pub const BULLET_TILES_PER_TICK: usize = 5;

/// Shrapnel travel speed in tiles per tick.
pub const SHRAPNEL_TILES_PER_TICK: usize = 2;

/// Shrapnel self-damage multiplier (fraction of original damage dealt to the caster).
/// Shrapnel that hits the player who threw the grenade deals reduced damage.
const SELF_DAMAGE_DIVISOR: i32 = 2;

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
                    symbol: "✦".into(),
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
    mut projectiles: Query<(Entity, &mut Position, &mut Projectile, &mut Renderable)>,
    mut damage_events: MessageWriter<DamageEvent>,
    targets: Query<(Entity, &Position, &CombatStats, Option<&Name>), (With<Hostile>, Without<Projectile>)>,
    player_query: Query<(Entity, &Position, &CombatStats, Option<&Name>), (With<Player>, Without<Projectile>)>,
    source_names: Query<Option<&Name>>,
    game_map: Res<GameMapResource>,
    mut combat_log: ResMut<CombatLog>,
    mut sound_events: ResMut<SoundEvents>,
) {
    // Build a lookup of hostile entities by position for O(1) hit detection.
    let mut target_by_pos: std::collections::HashMap<GridVec, Vec<(Entity, i32, String)>> =
        std::collections::HashMap::new();
    for (target_entity, target_pos, target_stats, target_name) in &targets {
        let t_name = target_name.map_or("???".to_string(), |n| n.0.clone());
        target_by_pos
            .entry(target_pos.as_grid_vec())
            .or_default()
            .push((target_entity, target_stats.defense, t_name));
    }

    // Player position for NPC bullet hits and shrapnel self-damage.
    let player_info = player_query.single().ok();

    for (proj_entity, mut proj_pos, mut proj, mut renderable) in &mut projectiles {
        let mut despawn = false;
        let steps = proj.tiles_per_tick;

        // Look up the name of the entity that fired this projectile.
        let source_name: String = source_names
            .get(proj.source)
            .ok()
            .flatten()
            .map_or("???".into(), |n| n.0.clone());

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
            // Each hit reduces remaining penetration by the target's defense,
            // so subsequent targets take less damage — matching standard
            // roguelike bullet-through-armor mechanics.
            if let Some(entities_here) = target_by_pos.get(&tile) {
                for (target_entity, target_def, t_name) in entities_here {
                    if proj.penetration <= 0 {
                        break;
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
                    proj.penetration -= target_def;
                }
                if proj.penetration <= 0 {
                    despawn = true;
                    break;
                }
            }

            // NPC bullet hitting the player: check if the projectile source
            // is NOT the player and it landed on the player's tile.
            if let Some((player_entity, player_pos, player_stats, player_name)) = &player_info
                && proj.source != *player_entity
                && tile == player_pos.as_grid_vec()
                && proj.penetration > 0
            {
                let hit_damage = proj.penetration;
                damage_events.write(DamageEvent {
                    target: *player_entity,
                    amount: hit_damage,
                    source: Some(proj.source),
                });
                let p_name = player_name.map_or("???", |n| &n.0);
                combat_log.push(format!("{source_name}'s bullet hits {p_name} for {hit_damage} damage!"));
                sound_events.add(tile);
                proj.penetration -= player_stats.defense;
                if proj.penetration <= 0 {
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
            renderable.symbol = "*".into();
            renderable.fg = RatColor::Rgb(255, 180, 40);
        }

        if despawn {
            commands.entity(proj_entity).despawn();
        }
    }
}
