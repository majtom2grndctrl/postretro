// Mover termini and the brushes posed at each, from the authored path.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::collections::HashMap;

use glam::DVec3;

use super::segment::{segment_may_touch_box, segment_touches_brush};
use crate::map_data::{BrushVolume, MapKinematicMover, MapKinematicWaypoint};
pub(crate) use crate::mover_pose::PosedBrush;
use crate::mover_pose::{BROKEN_PATH_REASON, path_waypoints};
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
    /// A gate mask with both termini's bits: gated at both, so shared.
    pub(crate) const BOTH_BITS: u8 = Terminus::First.bit() | Terminus::Last.bit();

    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    /// This terminus's bit in a texel's two-bit gate mask.
    pub(crate) const fn bit(self) -> u8 {
        1 << self.index()
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Terminus::First => "first",
            Terminus::Last => "last",
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
    /// Union of the brushes' bounds; always valid for an analysed mover,
    /// since one with no bounded brush is not analysed.
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
    /// only the termini.
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
    /// What the side count reads: the bounds its brushes sweep over every
    /// waypoint at any rotation about the spin pivot; invalid for a broken
    /// path or a mover with no brush, which gets no side count
    /// ([`MoverPoses::swept`]).
    pub(crate) swept: Aabb,
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
pub(crate) const NO_BOUNDS_REASON: &str =
    "no brush has bounds, so it blocks no light at either terminus";

impl MoverPoses {
    /// Pose each mover at its path's first and last waypoint: its authored
    /// brushes, which sit at the first waypoint, translated by
    /// (waypoint k − waypoint 0). A mover the gate cannot pose still keeps
    /// its swept bounds, so a spinner gets a side count.
    pub(crate) fn resolve(
        movers: &[MapKinematicMover],
        waypoints: &[MapKinematicWaypoint],
    ) -> Self {
        let by_name: HashMap<&str, &MapKinematicWaypoint> = waypoints
            .iter()
            .map(|waypoint| (waypoint.name.as_str(), waypoint))
            .collect();
        let base_labels: Vec<String> = movers.iter().map(base_label).collect();
        let mut label_uses: HashMap<&str, usize> = HashMap::new();
        for label in &base_labels {
            *label_uses.entry(label.as_str()).or_default() += 1;
        }
        let mut poses = Self::default();
        for (mover, base) in movers.iter().zip(&base_labels) {
            // Two movers sharing a name, or unnamed ones sharing a centre,
            // are told apart by id.
            let label = if label_uses[base.as_str()] > 1 {
                format!("{base} (id {})", mover.mover_id)
            } else {
                base.clone()
            };
            // A spin axis admits scripted spin even at zero authored speed.
            let spins = mover.spin_axis != [0.0; 3];
            let path = path_waypoints(&mover.path, &by_name);
            let swept = path.as_ref().map_or_else(
                |_| Aabb::empty(),
                |path| swept_bounds(&mover.brush_volumes, path, spins),
            );
            // A mover with no bounded brush poses empty: it gates nothing,
            // and analysing it would only cost the probe channel two posed
            // bakes.
            let bounded = mover
                .brush_volumes
                .iter()
                .any(|brush| brush.aabb.is_valid());
            let reason = match &path {
                _ if spins => Some(SPIN_REASON),
                Err(reason) => Some(*reason),
                Ok(path) if path.len() < 2 => Some(ONE_WAYPOINT_REASON),
                Ok(_) if !bounded => Some(NO_BOUNDS_REASON),
                Ok(_) => None,
            };
            let (Ok(path), None) = (path, reason) else {
                poses.not_analysed.push(NotAnalysedMover {
                    mover_id: mover.mover_id,
                    label,
                    reason: reason.unwrap_or(BROKEN_PATH_REASON),
                    swept,
                });
                continue;
            };
            let (first, last) = (path[0], path[path.len() - 1]);
            poses.analysed.push(AnalysedMover {
                mover_id: mover.mover_id,
                label,
                poses: [
                    Pose::new(&first.name, &mover.brush_volumes, DVec3::ZERO),
                    Pose::new(&last.name, &mover.brush_volumes, last.origin - first.origin),
                ],
                swept,
            });
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

    /// Mover `mover_id`'s label, analysed or not.
    pub(crate) fn label(&self, mover_id: u32) -> Option<&str> {
        self.mover(mover_id)
            .map(|mover| mover.label.as_str())
            .or_else(|| {
                self.not_analysed
                    .iter()
                    .find(|mover| mover.mover_id == mover_id)
                    .map(|mover| mover.label.as_str())
            })
    }

    /// The movers the side count covers, with their swept bounds, ascending
    /// by `mover_id`: each mover, analysed or not, whose path and brushes
    /// give it bounds. One without bounds covers nothing, so it gates no
    /// texel and gets no side count.
    pub(crate) fn swept(&self) -> Vec<(u32, &Aabb)> {
        let analysed = self
            .analysed
            .iter()
            .map(|mover| (mover.mover_id, &mover.swept));
        let not_analysed = self
            .not_analysed
            .iter()
            .map(|mover| (mover.mover_id, &mover.swept));
        let mut swept: Vec<(u32, &Aabb)> = analysed
            .chain(not_analysed)
            .filter(|(_, swept)| swept.is_valid())
            .collect();
        swept.sort_by_key(|(mover_id, _)| *mover_id);
        swept
    }
}

/// The bounds `brushes` sweep over every waypoint of `path`, translated by
/// (waypoint − first waypoint); invalid when no brush has bounds.
///
/// A mover that `spins` may park at any rotation, so each waypoint
/// contributes the box around the sphere its brushes sweep about the spin
/// pivot. The pivot is the mover's origin, the first waypoint, where its
/// geometry's local frame is rooted; the radius is the farthest brush-bounds
/// corner from it, which encloses every brush at every rotation.
fn swept_bounds(brushes: &[BrushVolume], path: &[&MapKinematicWaypoint], spins: bool) -> Aabb {
    let Some(first) = path.first() else {
        return Aabb::empty();
    };
    let mut authored = Aabb::empty();
    for brush in brushes.iter().filter(|brush| brush.aabb.is_valid()) {
        authored.expand_aabb(&brush.aabb);
    }
    if !authored.is_valid() {
        return authored;
    }
    let pivot = first.origin;
    if spins {
        let radius = brushes
            .iter()
            .filter(|brush| brush.aabb.is_valid())
            .flat_map(|brush| {
                let (min, max) = (brush.aabb.min, brush.aabb.max);
                (0..8).map(move |corner: u32| {
                    DVec3::new(
                        if corner & 1 == 0 { min.x } else { max.x },
                        if corner & 2 == 0 { min.y } else { max.y },
                        if corner & 4 == 0 { min.z } else { max.z },
                    )
                })
            })
            .map(|corner| corner.distance(pivot))
            .fold(0.0_f64, f64::max);
        authored = Aabb {
            min: pivot - DVec3::splat(radius),
            max: pivot + DVec3::splat(radius),
        };
    }
    let mut swept = Aabb::empty();
    for waypoint in path {
        let offset = waypoint.origin - pivot;
        swept.expand_aabb(&Aabb {
            min: authored.min + offset,
            max: authored.max + offset,
        });
    }
    swept
}

/// The mover's name, or for an unnamed mover the centre of its authored
/// brushes' bounds. [`MoverPoses::resolve`] appends the id when another
/// mover's label is the same.
fn base_label(mover: &MapKinematicMover) -> String {
    let name = mover.name.trim();
    if !name.is_empty() {
        return format!("kinematic_mover '{name}'");
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
