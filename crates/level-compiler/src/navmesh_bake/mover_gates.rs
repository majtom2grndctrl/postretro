// Mover gates: the portals a capsule cannot cross against a posed mover's solid.
// See: context/lib/build_pipeline.md §Navigation bake

use glam::{DVec3, Vec3};
use postretro_level_format::navmesh::{NavMoverGate, NavPortal, NavRegion, sorted_unique_gates};

use super::mover_poses::{NavPose, lane_quad};
use super::{WalkGrid, region_ref_height};
use crate::map_data::NavParams;

/// The gate records for every pose, sorted and unique. A portal is gated at a
/// pose when no lane across it passes: one lane per cell along the portal, each
/// the agent band swept from cell centre to cell centre. Sweeping that far means
/// a door thinner than a cell always meets one of the portals around its
/// footprint, whichever half of the cell it stands in.
pub(super) fn bake_gates(
    grid: &WalkGrid,
    regions: &[NavRegion],
    portals: &[NavPortal],
    poses: &[NavPose],
    params: &NavParams,
) -> Vec<NavMoverGate> {
    let cell = f64::from(grid.cell_size);
    let mut gates = Vec::new();
    for pose in poses {
        for (portal_index, portal) in portals.iter().enumerate() {
            if portal_is_gated(portal, regions, pose, cell, params) {
                gates.push(NavMoverGate {
                    mover_id: pose.mover_id,
                    waypoint: pose.waypoint,
                    portal: portal_index as u32,
                });
            }
        }
    }
    sorted_unique_gates(&gates)
}

fn portal_is_gated(
    portal: &NavPortal,
    regions: &[NavRegion],
    pose: &NavPose,
    cell: f64,
    params: &NavParams,
) -> bool {
    let floor_a = f64::from(region_ref_height(&regions[portal.region_a as usize]));
    let floor_b = f64::from(region_ref_height(&regions[portal.region_b as usize]));
    let left = Vec3::from(portal.left).as_dvec3();
    let right = Vec3::from(portal.right).as_dvec3();
    if !portal_near_pose(left, right, (floor_a, floor_b), pose, cell, params) {
        return false;
    }

    let span = right - left;
    let length = span.x.hypot(span.z);
    if length <= f64::EPSILON {
        return false;
    }
    // The `NavPortal` convention: `left` lies left of `region_a → region_b`
    // travel, so this half step points from region a into region b.
    let half_step = DVec3::new(-span.z, 0.0, span.x) / length * (cell * 0.5);
    let lanes = ((length / cell).round() as usize).max(1);
    (0..lanes).all(|lane| {
        let at = left + span * ((lane as f64 + 0.5) / lanes as f64);
        pose.blocks(&lane_quad(
            at - half_step,
            floor_a,
            at + half_step,
            floor_b,
            params,
        ))
    })
}

/// Whether the pose's bounds can reach any lane of this portal: within the agent
/// radius plus the lane's half step across the floor, and inside the band.
fn portal_near_pose(
    left: DVec3,
    right: DVec3,
    (floor_a, floor_b): (f64, f64),
    pose: &NavPose,
    cell: f64,
    params: &NavParams,
) -> bool {
    let reach = f64::from(params.agent_radius) + cell;
    let bounds = &pose.bounds;
    let band_low = floor_a.min(floor_b) + f64::from(params.step_height);
    let band_high = floor_a.max(floor_b) + f64::from(params.agent_height);
    bounds.max.y > band_low
        && bounds.min.y < band_high
        && left.x.min(right.x) - reach <= bounds.max.x
        && left.x.max(right.x) + reach >= bounds.min.x
        && left.z.min(right.z) - reach <= bounds.max.z
        && left.z.max(right.z) + reach >= bounds.min.z
}
