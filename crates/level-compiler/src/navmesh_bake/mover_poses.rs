// Movers posed at each waypoint for the nav bake: solid brushes and walkable tops.
// See: context/lib/build_pipeline.md §Navigation bake

use std::collections::HashMap;

use glam::DVec3;

use crate::geometry_utils::{clip_winding_to_half_spaces, polygon_area};
use crate::map_data::{BrushVolume, MapKinematicMover, MapKinematicWaypoint, NavParams};
use crate::mover_pose::{PosedBrush, path_waypoints};
use crate::partition::Aabb;

/// A lane overlapping a brush by less area than this only touches it.
const MIN_LANE_OVERLAP_M2: f64 = 1.0e-9;

/// One mover docked at one waypoint, posed alone: other movers are absent, as
/// each record's dock is switched on its own at runtime.
pub(super) struct NavPose {
    pub(super) mover_id: u32,
    /// Index along the mover's resolved path, the runtime's `segment_index`.
    pub(super) waypoint: u32,
    /// Tested one by one, never as their hull, so a hollow mover's doorway stays
    /// open.
    pub(super) brushes: Vec<NavPoseBrush>,
    pub(super) surfaces: Vec<WalkableSurface>,
    /// Union of the brushes' bounds.
    pub(super) bounds: Aabb,
}

pub(super) struct NavPoseBrush {
    pub(super) posed: PosedBrush,
    /// Half-spaces, in the form `clip_winding_to_half_spaces` keeps, of this
    /// brush grown by the agent radius across the floor plane only. The vertical
    /// allowance is the agent band itself: inflating a lift's top by the radius
    /// would lift it into a band a stepping agent clears.
    inflated: Vec<(DVec3, f64)>,
}

/// An upward walkable brush face at this pose: one convex polygon.
pub(super) struct WalkableSurface {
    pub(super) vertices: Vec<DVec3>,
    pub(super) centroid: DVec3,
    pub(super) min: DVec3,
    pub(super) max: DVec3,
}

impl NavPose {
    /// Whether the agent band swept over `lane` meets any brush.
    pub(super) fn blocks(&self, lane: &[DVec3; 4]) -> bool {
        self.brushes.iter().any(|brush| {
            clip_winding_to_half_spaces(lane.to_vec(), &brush.inflated, 0.0)
                .is_some_and(|overlap| polygon_area(&overlap) > MIN_LANE_OVERLAP_M2)
        })
    }

    /// Whether `point` lies strictly inside a brush.
    pub(super) fn covers(&self, point: DVec3) -> bool {
        self.brushes.iter().any(|brush| {
            brush
                .posed
                .planes
                .iter()
                .all(|&(normal, distance)| normal.dot(point) - distance < 0.0)
        })
    }
}

/// The vertical quad the agent band sweeps from `from` to `to`, each at its own
/// floor height: `[floor + step height, floor + agent height]`. Solid below the
/// band is a step the agent climbs, so a lift top at floor level never blocks.
pub(super) fn lane_quad(
    from: DVec3,
    from_floor: f64,
    to: DVec3,
    to_floor: f64,
    params: &NavParams,
) -> [DVec3; 4] {
    let step = f64::from(params.step_height);
    let height = f64::from(params.agent_height);
    [
        DVec3::new(from.x, from_floor + step, from.z),
        DVec3::new(to.x, to_floor + step, to.z),
        DVec3::new(to.x, to_floor + height, to.z),
        DVec3::new(from.x, from_floor + height, from.z),
    ]
}

/// Pose every mover at every waypoint of its path, ascending by mover id then
/// waypoint. Authored brushes sit at waypoint 0, so pose k is a translate by
/// (waypoint k − waypoint 0).
///
/// A spinner-capable mover has no single pose to bake, so it gets none: the test
/// is the mover light gate's, a non-zero axis admitting scripted spin even at
/// zero authored speed. Parse rejects a broken path; one that reaches here is
/// skipped like a spinner, leaving the mover to collision.
pub(super) fn nav_poses(
    movers: &[MapKinematicMover],
    waypoints: &[MapKinematicWaypoint],
    params: &NavParams,
) -> Vec<NavPose> {
    let by_name: HashMap<&str, &MapKinematicWaypoint> = waypoints
        .iter()
        .map(|waypoint| (waypoint.name.as_str(), waypoint))
        .collect();
    let mut ordered: Vec<&MapKinematicMover> = movers.iter().collect();
    ordered.sort_by_key(|mover| mover.mover_id);

    let radius = f64::from(params.agent_radius);
    let cos_max_slope = f64::from(params.max_slope_deg).to_radians().cos();
    let mut poses = Vec::new();
    for mover in ordered {
        if mover.spin_axis != [0.0; 3] {
            continue;
        }
        let Ok(path) = path_waypoints(&mover.path, &by_name) else {
            continue;
        };
        for (waypoint, stop) in path.iter().enumerate() {
            let offset = stop.origin - path[0].origin;
            let brushes = posed_brushes(&mover.brush_volumes, offset, radius);
            if brushes.is_empty() {
                continue;
            }
            let mut bounds = Aabb::empty();
            for brush in &brushes {
                bounds.expand_aabb(&brush.posed.bounds);
            }
            poses.push(NavPose {
                mover_id: mover.mover_id,
                waypoint: waypoint as u32,
                brushes,
                surfaces: walkable_surfaces(&mover.brush_volumes, offset, cos_max_slope),
                bounds,
            });
        }
    }
    poses
}

fn posed_brushes(brushes: &[BrushVolume], offset: DVec3, radius: f64) -> Vec<NavPoseBrush> {
    brushes
        .iter()
        .filter(|brush| brush.aabb.is_valid())
        .map(|brush| {
            let posed = PosedBrush::translated(brush, offset);
            let inflated = posed
                .planes
                .iter()
                .map(|&(normal, distance)| {
                    // Support of a horizontal disk along this normal.
                    let grown = distance + radius * normal.x.hypot(normal.z);
                    (-normal, -grown)
                })
                .collect();
            NavPoseBrush { posed, inflated }
        })
        .collect()
}

fn walkable_surfaces(
    brushes: &[BrushVolume],
    offset: DVec3,
    cos_max_slope: f64,
) -> Vec<WalkableSurface> {
    let mut surfaces = Vec::new();
    for brush in brushes.iter().filter(|brush| brush.aabb.is_valid()) {
        for side in &brush.sides {
            if side.normal.y < cos_max_slope || side.vertices.len() < 3 {
                continue;
            }
            let vertices: Vec<DVec3> = side.vertices.iter().map(|v| *v + offset).collect();
            let mut bounds = Aabb::empty();
            let mut sum = DVec3::ZERO;
            for &vertex in &vertices {
                bounds.expand_point(vertex);
                sum += vertex;
            }
            surfaces.push(WalkableSurface {
                centroid: sum / vertices.len() as f64,
                min: bounds.min,
                max: bounds.max,
                vertices,
            });
        }
    }
    surfaces
}
