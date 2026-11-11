// Mid-travel pose placement: how many poses a mover's probe channel bakes,
// and where along its path they sit.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use glam::{DVec3, Vec3};
use postretro_physics::kinematic_mover::MoverPathArcLengths;

use super::pose::{MoverPoses, Pose};
use crate::map_data::MapKinematicMover;

/// The most poses automatic placement bakes. A longer path spreads them over
/// the whole trip and warns; `light_poses` goes past it. Owner value: every
/// `--release` build bakes cold, and long lifts are ordinary in the genre.
pub(crate) const AUTO_POSE_CAP: u32 = 16;

/// A mover's pose count, and what automatic placement wanted when the cap
/// cut it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoseCount {
    pub(crate) count: u32,
    /// The uncapped automatic count, only where it passed the cap.
    pub(crate) capped_from: Option<u64>,
}

/// min(cap, max(2, ⌈L / s⌉ + 1)) poses for a path of length `path_length`
/// at probe spacing `probe_spacing` (both metres), or the authored
/// `light_poses` as given.
pub(crate) fn pose_count(path_length: f32, probe_spacing: f32, authored: Option<u32>) -> PoseCount {
    if let Some(count) = authored {
        return PoseCount {
            count,
            capped_from: None,
        };
    }
    let steps = f64::from(path_length) / f64::from(probe_spacing);
    // `as` saturates, so an absurd path still reads as past the cap.
    let wanted = if steps.is_finite() && steps > 0.0 {
        (steps.ceil() as u64).saturating_add(1)
    } else {
        2
    }
    .max(2);
    if wanted > u64::from(AUTO_POSE_CAP) {
        PoseCount {
            count: AUTO_POSE_CAP,
            capped_from: Some(wanted),
        }
    } else {
        PoseCount {
            count: wanted as u32,
            capped_from: None,
        }
    }
}

/// `count` travel fractions at even arc length: k / (count − 1), exactly 0
/// first and 1 last.
pub(crate) fn even_knots(count: u32) -> Vec<f32> {
    let last = count.max(2) - 1;
    (0..=last).map(|k| k as f32 / last as f32).collect()
}

impl MoverPoses {
    /// Add each analysed mover's mid poses between its termini, at even arc
    /// length by the runtime's own rule ([`MoverPathArcLengths::point_at`]),
    /// on the path as the runtime holds it (`f32` waypoints, as id 43 packs
    /// them). Termini keep their exact poses. Warns once per mover whose
    /// automatic count passed [`AUTO_POSE_CAP`].
    pub(crate) fn place_poses(&mut self, movers: &[MapKinematicMover], probe_spacing: f32) {
        for analysed in &mut self.analysed {
            let Some(mover) = movers.iter().find(|m| m.mover_id == analysed.mover_id) else {
                continue;
            };
            let path: Vec<Vec3> = analysed.path.iter().map(|o| o.as_vec3()).collect();
            let arc_lengths = MoverPathArcLengths::from_waypoints(&path);
            let length = arc_lengths.total_length();
            let placed = pose_count(length, probe_spacing, mover.light_poses);
            if let Some(wanted) = placed.capped_from {
                log::warn!(
                    "[MoverLight] {}: its {length:.2} m path wants {wanted} poses at the {probe_spacing} m \
                     probe spacing, past the cap of {AUTO_POSE_CAP}; baking {AUTO_POSE_CAP} poses \
                     {:.2} m apart instead. Set `light_poses` on the mover to bake more",
                    analysed.label,
                    length / (AUTO_POSE_CAP - 1) as f32,
                );
            }
            if placed.count <= 2 {
                continue;
            }
            let knots = even_knots(placed.count);
            let first = path[0];
            let mut poses = Vec::with_capacity(knots.len());
            poses.push(analysed.poses[0].clone());
            for &t in &knots[1..knots.len() - 1] {
                // A path with no length has nothing between its termini, so
                // an authored count stacks its mid poses on the first.
                let position = arc_lengths
                    .point_at(t)
                    .map_or(first, |point| point.position);
                let offset: DVec3 = (position - first).as_dvec3();
                poses.push(Pose::new(
                    &format!("t {t:.2}"),
                    &mover.brush_volumes,
                    offset,
                ));
            }
            poses.push(analysed.poses[analysed.poses.len() - 1].clone());
            analysed.poses = poses;
            analysed.knots = knots;
        }
    }
}
