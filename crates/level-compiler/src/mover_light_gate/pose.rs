// Mover termini and the brushes posed at each, from the authored path.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::collections::{HashMap, HashSet};

use glam::DVec3;

use super::segment::{segment_may_touch_box, segment_touches_brush};
use crate::map_data::{BrushVolume, MapKinematicMover, MapKinematicWaypoint};
use crate::partition::Aabb;

/// One end of a mover's travel: its path's first or last waypoint. Ordered
/// first before last, the report's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Terminus {
    First,
    Last,
}

impl Terminus {
    pub(crate) const BOTH: [Terminus; 2] = [Terminus::First, Terminus::Last];

    pub(crate) fn index(self) -> usize {
        self as usize
    }

    /// This terminus's bit in a texel's two-bit gate mask.
    pub(crate) fn bit(self) -> u8 {
        1 << self.index()
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Terminus::First => "first",
            Terminus::Last => "last",
        }
    }
}

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

/// A mover parked at one terminus. Its brushes are tested one by one, never
/// as their hull, so a hollow mover keeps its doorways open.
#[derive(Debug, Clone)]
pub(crate) struct Pose {
    /// The terminus waypoint's authored name.
    pub(crate) waypoint: String,
    pub(crate) brushes: Vec<PosedBrush>,
    /// Union of the brushes' bounds; invalid for a mover with no brush.
    pub(crate) bounds: Aabb,
}

impl Pose {
    /// Whether the closed segment `start..=end` touches one of the posed
    /// brushes.
    pub(crate) fn segment_touches(&self, start: DVec3, end: DVec3) -> bool {
        segment_may_touch_box(start, end, &self.bounds)
            && self
                .brushes
                .iter()
                .any(|brush| segment_touches_brush(start, end, brush))
    }

    fn new(waypoint: &str, brushes: &[BrushVolume], offset: DVec3) -> Self {
        // A brush without bounds has no face, so it covers nothing.
        let brushes: Vec<PosedBrush> = brushes
            .iter()
            .filter(|brush| brush.aabb.is_valid())
            .map(|brush| PosedBrush::translated(brush, offset))
            .collect();
        let mut bounds = Aabb::empty();
        for brush in &brushes {
            bounds.expand_aabb(&brush.bounds);
        }
        Self {
            waypoint: waypoint.to_string(),
            brushes,
            bounds,
        }
    }
}

/// A mover whose termini the gate tests, posed alone: every other mover is
/// absent, as in the bake.
#[derive(Debug, Clone)]
pub(crate) struct AnalysedMover {
    pub(crate) mover_id: u32,
    pub(crate) label: String,
    /// Indexed by [`Terminus::index`].
    pub(crate) poses: [Pose; 2],
    /// Union of the posed brush bounds at every waypoint on the path, not
    /// only the termini; invalid for a mover with no brush.
    pub(crate) swept: Aabb,
}

impl AnalysedMover {
    pub(crate) fn pose(&self, terminus: Terminus) -> &Pose {
        &self.poses[terminus.index()]
    }
}

/// A mover the gate cannot pose at compile time, and why.
#[derive(Debug, Clone)]
pub(crate) struct NotAnalysedMover {
    pub(crate) mover_id: u32,
    pub(crate) label: String,
    pub(crate) reason: &'static str,
}

/// Every mover, analysed or not, each list ascending by `mover_id`.
#[derive(Debug, Clone, Default)]
pub(crate) struct MoverPoses {
    pub(crate) analysed: Vec<AnalysedMover>,
    pub(crate) not_analysed: Vec<NotAnalysedMover>,
}

pub(crate) const SPIN_REASON: &str =
    "spin axis set, so its parked rotation is unknowable at compile time";
pub(crate) const ONE_WAYPOINT_REASON: &str =
    "path holds one waypoint, so it has no second terminus";
pub(crate) const BROKEN_PATH_REASON: &str = "path names a waypoint the map lacks";

impl MoverPoses {
    /// Pose each mover at its path's first and last waypoint: its authored
    /// brushes, which sit at the first waypoint, translated by
    /// (waypoint k − waypoint 0).
    pub(crate) fn resolve(
        movers: &[MapKinematicMover],
        waypoints: &[MapKinematicWaypoint],
    ) -> Self {
        let by_name: HashMap<&str, &MapKinematicWaypoint> = waypoints
            .iter()
            .map(|waypoint| (waypoint.name.as_str(), waypoint))
            .collect();
        let mut poses = Self::default();
        for mover in movers {
            let label = mover_label(mover);
            // A spin axis admits scripted spin even at zero authored speed.
            let path = if mover.spin_axis != [0.0; 3] {
                Err(SPIN_REASON)
            } else {
                path_waypoints(&mover.path, &by_name)
            };
            match path {
                Ok(path) => {
                    let (first, last) = (path[0], path[path.len() - 1]);
                    let poses_at = [
                        Pose::new(&first.name, &mover.brush_volumes, DVec3::ZERO),
                        Pose::new(&last.name, &mover.brush_volumes, last.origin - first.origin),
                    ];
                    let authored = &poses_at[Terminus::First.index()].bounds;
                    let mut swept = Aabb::empty();
                    if authored.is_valid() {
                        for waypoint in &path {
                            let offset = waypoint.origin - first.origin;
                            swept.expand_aabb(&Aabb {
                                min: authored.min + offset,
                                max: authored.max + offset,
                            });
                        }
                    }
                    poses.analysed.push(AnalysedMover {
                        mover_id: mover.mover_id,
                        label,
                        poses: poses_at,
                        swept,
                    });
                }
                Err(reason) => poses.not_analysed.push(NotAnalysedMover {
                    mover_id: mover.mover_id,
                    label,
                    reason,
                }),
            }
        }
        poses.analysed.sort_by_key(|mover| mover.mover_id);
        poses.not_analysed.sort_by_key(|mover| mover.mover_id);
        poses
    }

    pub(crate) fn mover(&self, mover_id: u32) -> Option<&AnalysedMover> {
        self.analysed
            .binary_search_by_key(&mover_id, |mover| mover.mover_id)
            .ok()
            .map(|at| &self.analysed[at])
    }
}

/// The waypoints of the `next` chain starting at `start`, in path order,
/// at least two. Parse keeps only the chain's names, so it is walked again
/// here.
fn path_waypoints<'w>(
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
    if path.len() < 2 {
        return Err(ONE_WAYPOINT_REASON);
    }
    Ok(path)
}

/// The mover's name, or for an unnamed mover the centre of its authored
/// brushes' bounds.
fn mover_label(mover: &MapKinematicMover) -> String {
    if !mover.name.trim().is_empty() {
        return format!("kinematic_mover '{}'", mover.name);
    }
    let mut bounds = Aabb::empty();
    for brush in mover.brush_volumes.iter().filter(|b| b.aabb.is_valid()) {
        bounds.expand_aabb(&brush.aabb);
    }
    let centre = if bounds.is_valid() {
        bounds.centroid()
    } else {
        mover.origin
    };
    format!(
        "kinematic_mover at ({:.3}, {:.3}, {:.3}) m",
        centre.x, centre.y, centre.z
    )
}
