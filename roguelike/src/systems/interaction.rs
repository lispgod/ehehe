use bevy::prelude::*;

use crate::components::{
    Dead, DrunkStatus, Faction, Hostility, Hostile, InBrawl,
    Name, NpcInteraction, NpcMood, Player, Position, display_name,
};
use crate::events::InteractionIntent;
use crate::resources::{CombatLog, Gold, SaloonEffect, SALOON_MENU};

/// Hostility radius: bystander NPCs within this Chebyshev distance witness
/// an interaction and may have their own hostility affected.
const WITNESS_RADIUS: i32 = 6;

/// Hostility spread to bystanders witnessing a provocation (fraction of delta).
const BYSTANDER_HOSTILITY_FRACTION: i32 = 2; // 1/2 of the delta

/// Processes player interactions with adjacent NPCs.
///
/// When the player interacts with an NPC:
/// 1. Apply the interaction's hostility delta to the target NPC.
/// 2. If the NPC's hostility exceeds their mood threshold, they become hostile.
/// 3. Nearby bystander NPCs also gain a fraction of the hostility change.
/// 4. Bystanders whose hostility crosses threshold also become hostile (cascade).
pub fn interaction_system(
    mut commands: Commands,
    mut intents: MessageReader<InteractionIntent>,
    mut npc_query: Query<(
        Entity,
        &Position,
        &mut Hostility,
        &NpcMood,
        Option<&Name>,
        Option<&Faction>,
    ), (Without<Player>, Without<Dead>)>,
    mut combat_log: ResMut<CombatLog>,
    mut gold: ResMut<Gold>,
) {
    // Collect intents first to avoid borrow conflicts
    let intents_vec: Vec<_> = intents.read().cloned().collect();

    for intent in intents_vec {
        // First pass: get target info and apply direct hostility
        let (target_gv, npc_name_str, delta, became_hostile) = {
            let Ok((_, target_pos, mut hostility, mood, name, _faction)) =
                npc_query.get_mut(intent.target) else { continue; };

            let npc_name = display_name(name).to_string();
            let target_gv = target_pos.as_grid_vec();
            let delta = intent.interaction.hostility_delta();

            // Apply hostility change
            if delta > 0 {
                hostility.increase(delta);
            } else if delta < 0 {
                hostility.decay(-delta);
            }

            // Generate response message based on mood and interaction
            let response = match (intent.interaction, *mood) {
                (NpcInteraction::Greet, NpcMood::Calm) => format!("{npc_name} tips their hat."),
                (NpcInteraction::Greet, NpcMood::Nervous) => format!("{npc_name} nods warily."),
                (NpcInteraction::Greet, NpcMood::Drunk) => format!("{npc_name} slurs a greeting and hiccups."),
                (NpcInteraction::Greet, NpcMood::Angry) => format!("{npc_name} glares at you."),
                (NpcInteraction::Taunt, NpcMood::Calm) => format!("{npc_name} frowns and clenches a fist."),
                (NpcInteraction::Taunt, NpcMood::Nervous) => format!("{npc_name} backs away nervously."),
                (NpcInteraction::Taunt, NpcMood::Drunk) => format!("{npc_name} takes a wild swing!"),
                (NpcInteraction::Taunt, NpcMood::Angry) => format!("{npc_name} snarls: 'You're dead!'"),
                (NpcInteraction::Threaten, NpcMood::Calm) => format!("{npc_name} stiffens and reaches for a weapon."),
                (NpcInteraction::Threaten, NpcMood::Nervous) => format!("{npc_name} trembles and backs away."),
                (NpcInteraction::Threaten, NpcMood::Drunk) => format!("{npc_name} laughs: 'You ain't scarin' me!'"),
                (NpcInteraction::Threaten, NpcMood::Angry) => format!("{npc_name} spits: 'Try it, stranger.'"),
                (NpcInteraction::AskAbout, _) => format!("{npc_name} shares what they know about the town."),
                (NpcInteraction::BuyDrink, _) => format!("You buy {npc_name} a drink."),
            };
            combat_log.push_at(response, target_gv);

            // Handle saloon economy for BuyDrink
            if intent.interaction == NpcInteraction::BuyDrink {
                let whiskey_price = SALOON_MENU.iter()
                    .find(|item| item.effect == SaloonEffect::Whiskey)
                    .map_or(2, |item| item.price);
                if gold.0 >= whiskey_price {
                    gold.0 -= whiskey_price;
                } else {
                    combat_log.push("You can't afford that.".into());
                }
            }

            // Check if NPC crosses hostility threshold → become hostile
            let threshold = mood.hostility_threshold();
            let became_hostile = hostility.exceeds_threshold(threshold);

            (target_gv, npc_name, delta, became_hostile)
        };

        if became_hostile {
            commands.entity(intent.target).insert(Hostile);
            commands.entity(intent.target).insert(InBrawl);
            combat_log.push_at(
                format!("{npc_name_str} has had enough and throws a punch!"),
                target_gv,
            );
        }

        // Second pass: spread hostility to bystanders
        if delta > 0 {
            let bystander_delta = delta / BYSTANDER_HOSTILITY_FRACTION;
            if bystander_delta > 0 {
                // Collect bystander info first, then mutate
                let bystanders: Vec<(Entity, i32)> = npc_query
                    .iter()
                    .filter(|(ent, pos, _, _, _, _)| {
                        *ent != intent.target
                            && pos.as_grid_vec().chebyshev_distance(target_gv) <= WITNESS_RADIUS
                    })
                    .map(|(ent, _, _, mood, _, _)| {
                        (ent, mood.hostility_threshold())
                    })
                    .collect();

                for (ent, threshold) in bystanders {
                    if let Ok((_, _, mut h, _, _, _)) = npc_query.get_mut(ent) {
                        h.increase(bystander_delta);
                        if h.exceeds_threshold(threshold) {
                            commands.entity(ent).insert(Hostile);
                            commands.entity(ent).insert(InBrawl);
                        }
                    }
                }
            }
        }
    }
}

/// Ticks down drunk status for all entities each world turn.
/// Removes the DrunkStatus component when the duration expires.
pub fn drunk_tick_system(
    mut commands: Commands,
    mut drunk_query: Query<(Entity, &mut DrunkStatus)>,
    mut combat_log: ResMut<CombatLog>,
    player_query: Query<(), With<Player>>,
) {
    for (entity, mut drunk) in &mut drunk_query {
        if !drunk.tick() {
            commands.entity(entity).remove::<DrunkStatus>();
            if player_query.contains(entity) {
                combat_log.push("You sober up. Your aim steadies.".into());
            }
        }
    }
}

/// NPC mood update system: drunk NPCs occasionally stagger (random movement).
/// Nervous NPCs may calm down over time. Angry NPCs stay angry.
pub fn mood_system(
    mut npc_query: Query<(
        &mut NpcMood,
        &mut Hostility,
        Option<&DrunkStatus>,
    ), Without<Player>>,
) {
    for (mut mood, mut hostility, drunk) in &mut npc_query {
        // Drunk status overrides mood
        if drunk.is_some() && *mood != NpcMood::Drunk {
            *mood = NpcMood::Drunk;
        }

        // Passive hostility decay each turn
        if !hostility.exceeds_threshold(mood.hostility_threshold()) {
            hostility.decay(1);
        }
    }
}

/// Applies pending saloon purchase effects (heal from food, drunk from whiskey).
/// Runs each frame; checks InputState flags set by the saloon buy menu.
pub fn saloon_effect_system(
    mut commands: Commands,
    mut input_state: ResMut<crate::resources::InputState>,
    mut player_query: Query<(Entity, &mut crate::components::Health), With<Player>>,
) {
    if let Ok((player_entity, mut hp)) = player_query.single_mut() {
        // Apply pending food heal
        if input_state.saloon_heal_pending > 0 {
            hp.heal(input_state.saloon_heal_pending);
            input_state.saloon_heal_pending = 0;
        }

        // Apply pending drunk status
        if input_state.saloon_drunk_pending {
            commands.entity(player_entity).insert(DrunkStatus::new());
            input_state.saloon_drunk_pending = false;
        }
    }
}
