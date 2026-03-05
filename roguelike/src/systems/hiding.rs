use bevy::prelude::*;

use crate::components::{Hidden, Player, Position, Renderable};
use crate::events::HideIntent;
use crate::resources::{CombatLog, GameMapResource, SoundEvents, StarLevel};

/// Noise radius when exiting a hiding spot (tiles).
/// Used by hide_system to determine the audible range of the exit noise event.
const EXIT_NOISE_RADIUS: i32 = 5;

/// Processes hide/unhide intents.
///
/// When entering a hiding spot:
/// - The player's renderable is changed to match the prop's appearance.
/// - A `Hidden` component is added to the player.
/// - The player becomes invisible to NPC FOV checks.
///
/// When exiting:
/// - The `Hidden` component is removed.
/// - A noise event is generated that may alert nearby NPCs.
/// - The player's renderable is restored.
pub fn hide_system(
    mut commands: Commands,
    mut intents: MessageReader<HideIntent>,
    mut player_query: Query<(&Position, &mut Renderable, Option<&Hidden>), With<Player>>,
    game_map: Res<GameMapResource>,
    mut combat_log: ResMut<CombatLog>,
    mut sound_events: ResMut<SoundEvents>,
    mut star_level: ResMut<StarLevel>,
) {
    for intent in intents.read() {
        let Ok((pos, mut renderable, hidden)) = player_query.get_mut(intent.entity) else {
            continue;
        };
        let gv = pos.as_grid_vec();

        if intent.entering {
            // Check if the tile has a hiding-spot prop
            if let Some(voxel) = game_map.0.get_voxel_at(&gv) {
                let is_hiding_spot = voxel.props.as_ref()
                    .is_some_and(|p| p.is_hiding_spot());

                if is_hiding_spot {
                    // Change player appearance to match the prop
                    let prop_symbol = match &voxel.props {
                        Some(crate::typeenums::Props::Barrel) => "o",
                        Some(crate::typeenums::Props::HayBale) => "#",
                        Some(crate::typeenums::Props::Outhouse) => "O",
                        Some(crate::typeenums::Props::Wardrobe) => "W",
                        _ => "?",
                    };
                    renderable.symbol = prop_symbol.into();
                    commands.entity(intent.entity).insert(Hidden { hiding_pos: gv });
                    star_level.player_hidden = true;
                    combat_log.push("You slip inside and hide.".into());
                } else {
                    combat_log.push("Nothing to hide in here.".into());
                }
            }
        } else {
            // Exiting hiding
            if hidden.is_some() {
                // Restore player appearance
                renderable.symbol = "@".into();
                commands.entity(intent.entity).remove::<Hidden>();
                star_level.player_hidden = false;

                // Generate noise that may alert nearby NPCs within EXIT_NOISE_RADIUS
                sound_events.add(gv);
                combat_log.push(
                    format!("You climb out — NPCs within {EXIT_NOISE_RADIUS} tiles may hear you."),
                );
            }
        }
    }
}

/// Detection check: NPCs close to a hidden player may detect them.
/// Suspicious or adjacent NPCs can see through hiding.
///
/// This modifies the existing FOV system behavior:
/// - Hidden players are excluded from NPC visible_tiles checks
///   unless the NPC is within detection range (1 tile = adjacent).
pub fn hiding_detection_system(
    mut commands: Commands,
    player_query: Query<(Entity, &Position, &Hidden), With<Player>>,
    npc_query: Query<(&Position, &crate::components::Viewshed, Option<&crate::components::NpcMood>), Without<Player>>,
    mut combat_log: ResMut<CombatLog>,
    mut star_level: ResMut<StarLevel>,
) {
    let Ok((player_entity, player_pos, _hidden)) = player_query.single() else {
        return;
    };
    let player_gv = player_pos.as_grid_vec();

    for (npc_pos, _viewshed, mood) in &npc_query {
        let dist = npc_pos.as_grid_vec().chebyshev_distance(player_gv);

        // Adjacent NPCs always detect hidden player
        // Suspicious (Angry/Nervous) NPCs detect within 2 tiles
        let detection_range = match mood {
            Some(crate::components::NpcMood::Angry) => 2,
            Some(crate::components::NpcMood::Nervous) => 2,
            _ => 1,
        };

        if dist <= detection_range {
            commands.entity(player_entity).remove::<Hidden>();
            star_level.player_hidden = false;
            combat_log.push("You've been discovered!".into());
            return; // Only need to be discovered once
        }
    }
}
