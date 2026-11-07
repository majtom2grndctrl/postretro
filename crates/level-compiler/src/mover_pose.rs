// Mover path walking and brush posing, for the mover light gate and the navmesh bake.
// See: context/lib/build_pipeline.md §Navigation bake

use std::collections::{HashMap, HashSet};

use glam::DVec3;

use crate::map_data::{BrushVolume, MapKinematicWaypoint};
use crate::partition::Aabb;

pub(crate) const BROKEN_PATH_REASON: &str = "path names a waypoint the map lacks";

/// One authored brush moved to a pose. Inside is where every
/// `normal · p − distance` is at most zero, as for [`BrushVolume`].
#[derive(Debug, Clone)]
pub(crate) struct PosedBrush {
    pub(crate) planes: Vec<(DVec3, f64)>,
    pub(crate) bounds: Aabb,
}

impl PosedBrush {
    pub(crate) fn translated(brush: &BrushVolume, offset: DVec3) -> Self {
        Self {
            planes: brush
                .planes
                .iter()
                .map(|plane| (plane.normal, plane.distance + plane.normal.dot(offset)))
                .collect(),
            bounds: Aabb {
                min: brush.aabb.min + offset,
                max: brush.aabb.max + offset,
            },
        }
    }
}

/// The waypoints of the `next` chain starting at `start`, in path order,
/// at least one. Parse keeps only the chain's names, so it is walked again
/// here. An entry's index is the runtime's `segment_index`.
pub(crate) fn path_waypoints<'w>(
    start: &str,
    by_name: &HashMap<&str, &'w MapKinematicWaypoint>,
) -> Result<Vec<&'w MapKinematicWaypoint>, &'static str> {
    let first = *by_name.get(start).ok_or(BROKEN_PATH_REASON)?;
    let mut path = vec![first];
    let mut seen = HashSet::from([first.name.as_str()]);
    // Parse rejects cycles; the seen set only bounds the walk.
    let mut last = first;
    while !last.next.trim().is_empty() {
        let next = *by_name.get(last.next.as_str()).ok_or(BROKEN_PATH_REASON)?;
        if !seen.insert(next.name.as_str()) {
            break;
        }
        path.push(next);
        last = next;
    }
    Ok(path)
}
