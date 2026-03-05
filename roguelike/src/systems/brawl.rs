use bevy::prelude::*;

use crate::components::{
    CrimeType, Faction, Hostile, InBrawl,
    Name, Player, Position, display_name,
};
use crate::events::{BrawlEscalation, CrimeEvent};
use crate::grid_vec::GridVec;
use crate::resources::CombatLog;

/// Radius within which NPCs witness a brawl and may join in.
const BRAWL_JOIN_RADIUS: i32 = 6;

/// Processes brawl escalation events.
///
/// When a player draws a weapon (uses a Gun item) during a fistfight:
/// 1. All hostile NPCs in the brawl escalate to lethal combat.
/// 2. Law NPCs (Sheriff faction) are alerted and become hostile.
/// 3. A crime event is generated for the weapon draw.
///
/// Nearby NPCs witnessing a brawl may join in if they are already
/// hostile or share a faction with someone in the fight.
pub fn brawl_escalation_system(
    mut commands: Commands,
    mut escalation_events: MessageReader<BrawlEscalation>,
    mut crime_events: MessageWriter<CrimeEvent>,
    npc_query: Query<(Entity, &Position, Option<&Faction>, Option<&Name>), Without<Player>>,
    player_query: Query<Entity, With<Player>>,
    mut combat_log: ResMut<CombatLog>,
) {
    for event in escalation_events.read() {
        let player_entity = player_query.single().ok();

        combat_log.push_at(
            "A weapon is drawn! The fight turns deadly!".into(),
            event.position,
        );

        // Remove InBrawl from all entities — fight is now lethal
        // Make all nearby NPCs hostile if they are law or already in brawl
        for (ent, pos, faction, name) in &npc_query {
            let dist = pos.as_grid_vec().chebyshev_distance(event.position);
            if dist > BRAWL_JOIN_RADIUS {
                continue;
            }

            // Law NPCs always respond to weapon draws
            if matches!(faction, Some(Faction::Sheriff)) {
                commands.entity(ent).insert(Hostile);
                commands.entity(ent).remove::<InBrawl>();
                let npc_name = display_name(name);
                combat_log.push_at(
                    format!("{npc_name} reaches for their gun!"),
                    pos.as_grid_vec(),
                );
            }
        }

        // Emit a crime event for the weapon draw
        if player_entity == Some(event.entity) {
            crime_events.write(CrimeEvent {
                perpetrator: event.entity,
                crime: CrimeType::Assault,
                position: event.position,
            });
        }
    }
}

/// Brawl witness system: NPCs near an active brawl may join in.
///
/// Each world turn, NPCs within `BRAWL_JOIN_RADIUS` of a brawling entity
/// have a chance to join the fistfight if they share a faction with
/// someone already fighting.
pub fn brawl_witness_system(
    mut commands: Commands,
    brawl_query: Query<(Entity, &Position, Option<&Faction>), With<InBrawl>>,
    nearby_npc_query: Query<(Entity, &Position, Option<&Faction>, Option<&Name>), (Without<InBrawl>, Without<Player>)>,
    mut combat_log: ResMut<CombatLog>,
) {
    // Collect brawl positions and factions
    let brawl_sites: Vec<(GridVec, Option<Faction>)> = brawl_query
        .iter()
        .map(|(_, pos, faction)| (pos.as_grid_vec(), faction.copied()))
        .collect();

    if brawl_sites.is_empty() {
        return;
    }

    for (ent, pos, faction, name) in &nearby_npc_query {
        let npc_gv = pos.as_grid_vec();

        for (brawl_pos, brawl_faction) in &brawl_sites {
            if npc_gv.chebyshev_distance(*brawl_pos) <= BRAWL_JOIN_RADIUS {
                // Same faction as someone in the brawl → join in
                if let (Some(nf), Some(bf)) = (faction, brawl_faction) {
                    if nf == bf {
                        commands.entity(ent).insert(InBrawl);
                        commands.entity(ent).insert(Hostile);
                        let npc_name = display_name(name);
                        combat_log.push_at(
                            format!("{npc_name} joins the brawl!"),
                            npc_gv,
                        );
                        break;
                    }
                }
            }
        }
    }
}
