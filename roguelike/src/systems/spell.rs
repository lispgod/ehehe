use bevy::prelude::*;

use crate::components::{CombatStats, Hostile, Name, Player, Position};
use crate::events::{DamageEvent, SpellCastIntent};
use crate::resources::CombatLog;

/// Resolves area-of-effect spell casts.
///
/// For each `SpellCastIntent`, finds all `Hostile` entities within the
/// specified radius (Chebyshev distance) of the caster and emits a
/// `DamageEvent` for each. Damage equals the caster's attack stat.
pub fn spell_system(
    mut intents: MessageReader<SpellCastIntent>,
    mut damage_events: MessageWriter<DamageEvent>,
    caster_query: Query<(&Position, &CombatStats, Option<&Name>), With<Player>>,
    targets: Query<(Entity, &Position, Option<&Name>), With<Hostile>>,
    mut combat_log: ResMut<CombatLog>,
) {
    for intent in intents.read() {
        let Ok((caster_pos, caster_stats, caster_name)) = caster_query.get(intent.caster) else {
            continue;
        };

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

        if hit_count == 0 {
            combat_log.push(format!("{c_name} casts a spell but hits nothing"));
        }
    }
}
