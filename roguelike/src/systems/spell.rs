use bevy::prelude::*;

use crate::components::{CombatStats, Hostile, Mana, Name, Player, Position};
use crate::events::{DamageEvent, SpellCastIntent};
use crate::resources::{CombatLog, GameMapResource, SpellParticles};
use crate::typeenums::Furniture;

/// Mana cost for casting the AoE spell.
const SPELL_MANA_COST: i32 = 10;

/// Lifetime (in frames) for spell particle animations.
const PARTICLE_LIFETIME: u32 = 8;

/// Resolves area-of-effect spell casts.
///
/// For each `SpellCastIntent`, finds all `Hostile` entities within the
/// specified radius (Chebyshev distance) of the caster and emits a
/// `DamageEvent` for each. Damage equals the caster's attack stat.
/// Consumes mana from the caster and generates particle animations.
pub fn spell_system(
    mut intents: MessageReader<SpellCastIntent>,
    mut damage_events: MessageWriter<DamageEvent>,
    mut caster_query: Query<(&Position, &CombatStats, Option<&Name>, Option<&mut Mana>), With<Player>>,
    targets: Query<(Entity, &Position, Option<&Name>), With<Hostile>>,
    mut combat_log: ResMut<CombatLog>,
    mut spell_particles: ResMut<SpellParticles>,
    mut game_map: ResMut<GameMapResource>,
) {
    for intent in intents.read() {
        let Ok((caster_pos, caster_stats, caster_name, mana)) = caster_query.get_mut(intent.caster) else {
            continue;
        };

        // Consume mana.
        if let Some(mut mana) = mana {
            if mana.current < SPELL_MANA_COST {
                combat_log.push("Not enough mana!".into());
                continue;
            }
            mana.current -= SPELL_MANA_COST;
        }

        let origin = caster_pos.as_grid_vec();
        let c_name = caster_name.map_or("???", |n| &n.0);
        let mut hit_count = 0;

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
                        "{c_name}'s spell hits {t_name} for {damage} damage"
                    ));
                    hit_count += 1;
                }
            }
        }

        // Generate particle animation for the AoE effect.
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
            combat_log.push(format!("The spell destroys {destroyed_count} obstacle(s)!"));
        }

        if hit_count == 0 {
            combat_log.push(format!("{c_name} casts a spell but hits nothing"));
        }
    }
}
