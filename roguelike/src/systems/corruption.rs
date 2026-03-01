use bevy::prelude::*;

use crate::components::HellGate;
use crate::grid_vec::GridVec;
use crate::resources::{GameMapResource, TurnCounter};
use crate::typeenums::Floor;
use crate::typedefs::GATE_POINT;

/// Base corruption radius at turn 0.
const BASE_CORRUPTION_RADIUS: i32 = 2;

/// Additional corruption radius per wave (every 3 turns).
const CORRUPTION_GROWTH_RATE: f64 = 0.4;

/// Spreads corruption around the Hell Gate as turns progress.
///
/// Each world turn, tiles within a growing radius of the gate are converted
/// to lava or scorched earth, creating an expanding hellish landscape.
/// The corruption only spreads while the gate still exists.
pub fn corruption_system(
    gate_query: Query<(), With<HellGate>>,
    mut map: ResMut<GameMapResource>,
    turn_counter: Res<TurnCounter>,
) {
    // Only spread corruption if the gate is still alive.
    if gate_query.is_empty() {
        return;
    }

    let turn = turn_counter.0;
    let corruption_radius =
        BASE_CORRUPTION_RADIUS + (turn as f64 * CORRUPTION_GROWTH_RATE) as i32;

    for dy in -corruption_radius..=corruption_radius {
        for dx in -corruption_radius..=corruption_radius {
            let pos = GATE_POINT + GridVec::new(dx, dy);
            let dist_sq = pos.distance_squared(GATE_POINT);
            let radius_sq = corruption_radius * corruption_radius;

            if dist_sq > radius_sq {
                continue;
            }

            if let Some(voxel) = map.0.get_voxel_at_mut(&pos) {
                // Don't corrupt wall borders.
                if voxel.furniture.as_ref().is_some_and(|f| {
                    matches!(f, crate::typeenums::Furniture::Wall)
                }) {
                    continue;
                }

                // Remove furniture consumed by corruption.
                if voxel.furniture.is_some() && dist_sq < radius_sq / 2 {
                    voxel.furniture = None;
                }

                // Inner ring: lava. Outer ring: scorched earth.
                let inner_sq = (corruption_radius / 2) * (corruption_radius / 2);
                if let Some(ref floor) = voxel.floor {
                    if !matches!(floor, Floor::Lava | Floor::ScorchedEarth) {
                        if dist_sq <= inner_sq {
                            voxel.floor = Some(Floor::Lava);
                        } else {
                            voxel.floor = Some(Floor::ScorchedEarth);
                        }
                    }
                }
            }
        }
    }
}
