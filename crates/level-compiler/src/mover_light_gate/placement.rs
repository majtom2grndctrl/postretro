// Mid-travel pose placement: how many poses a mover's probe channel bakes,
// and where along its path they sit.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use glam::{DVec3, Vec3};
use postretro_physics::kinematic_mover::MoverPathArcLengths;

use super::pose::{MoverPoses, Pose, PosedBrush};
use crate::map_data::MapKinematicMover;

/// The most poses automatic placement bakes. A longer path spreads them over
/// the whole trip and warns; `light_poses` goes past it. Owner value: every
/// `--release` build bakes cold, and long lifts are ordinary in the genre.
pub(crate) const AUTO_POSE_CAP: u32 = 16;

/// The most poses one mover places; an authored `light_poses` past it fails
/// the build by name. Knot k is k / (count − 1) in `f32`. Up to
/// count − 1 = 2^24, every k and count − 1 is exact in `f32`: at 2^24 every
/// quotient is exact, and below it neighbouring quotients differ by more
/// than 2^-24, the widest `f32` spacing in [0, 1], so the rounded knots stay
/// strictly ascending, as id 57 requires. One more pose and count − 1 rounds
/// to 2^24, so the last two knots both read 1.
pub(crate) const MAX_POSES: u32 = (1 << 24) + 1;

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
    (0..=last).map(|k| knot(k, last)).collect()
}

/// Knot `k` of a mover whose last pose is `last`: k / last in `f32`.
pub(crate) fn knot(k: u32, last: u32) -> f32 {
    k as f32 / last as f32
}

/// The least bytes one placed pose of `mover` holds before the SH base gate
/// prices the channel: its knot, and twice its [`Pose`] with each bounded
/// brush's posed copy and planes, once as placed and once as the posed
/// solid's own copy (`mover_probe_channel::PosedSolid`), which resolves for
/// every pose ahead of the gate. A floor: the solid's sides and geometry, the
/// bounce plan's per-pose scenes, and every later state and payload come on
/// top, so a count whose floor passes the budget cannot fit the gate's peak.
pub(super) fn pose_bytes_floor(mover: &MapKinematicMover) -> u64 {
    let brushes: u64 = mover
        .brush_volumes
        .iter()
        .filter(|brush| brush.aabb.is_valid())
        .map(|brush| {
            (std::mem::size_of::<PosedBrush>()
                + brush.planes.len() * std::mem::size_of::<(DVec3, f64)>()) as u64
        })
        .sum();
    let pose = std::mem::size_of::<Pose>() as u64 + brushes;
    std::mem::size_of::<f32>() as u64 + 2 * pose
}

/// One authored `light_poses` count and its floor, for the refusal's list.
struct AuthoredFloor<'a> {
    label: &'a str,
    count: u32,
    bytes: u64,
}

impl MoverPoses {
    /// Refuse authored `light_poses` counts placement cannot honour, naming
    /// the movers, before [`Self::place_poses`] allocates anything in
    /// proportion to them: a count past [`MAX_POSES`], or counts whose poses
    /// alone, priced at their floor and summed over every authored mover,
    /// pass `working_set_max_bytes`. Every mover's poses live together until
    /// the gate, so the floors sum; the refusal is the SH base gate's, in its
    /// words, and lists each contributing mover, largest first. Never places
    /// fewer poses. An automatic count is at most [`AUTO_POSE_CAP`] and is
    /// priced by the SH base gate with the rest of the channel.
    pub(crate) fn admit_pose_counts(
        &self,
        movers: &[MapKinematicMover],
        working_set_max_bytes: u64,
    ) -> anyhow::Result<()> {
        let mut floors: Vec<AuthoredFloor<'_>> = Vec::new();
        for analysed in &self.analysed {
            let Some(mover) = movers.iter().find(|m| m.mover_id == analysed.mover_id) else {
                continue;
            };
            let Some(count) = mover.light_poses else {
                continue;
            };
            if count > MAX_POSES {
                anyhow::bail!(
                    "{}: `light_poses` {count} is past the most poses a path can place, \
                     {MAX_POSES}: past it, neighbouring poses fall on one point of travel. \
                     Lower `light_poses`",
                    analysed.label,
                );
            }
            floors.push(AuthoredFloor {
                label: &analysed.label,
                count,
                bytes: pose_bytes_floor(mover).saturating_mul(u64::from(count)),
            });
        }
        let total = floors
            .iter()
            .fold(0u64, |sum, floor| sum.saturating_add(floor.bytes));
        if total <= working_set_max_bytes {
            return Ok(());
        }
        // Stable, so equal floors keep mover-id order.
        floors.sort_by_key(|floor| std::cmp::Reverse(floor.bytes));
        let named = floors
            .iter()
            .map(|floor| {
                format!(
                    "{} ({} poses, at least {} bytes)",
                    floor.label, floor.count, floor.bytes
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "SH base working-set gate refused before patch shading: estimated peak at least \
             {total} bytes exceeds budget {working_set_max_bytes} bytes, from the authored \
             `light_poses` poses alone: {named}. Lower `light_poses`, or raise \
             `--working-set-max-size`"
        );
    }

    /// Add each analysed mover's mid poses between its termini, at even arc
    /// length by the runtime's own rule ([`MoverPathArcLengths::point_at`]),
    /// on the path as the runtime holds it (`f32` waypoints, as id 43 packs
    /// them). Termini keep their exact poses. Warns once per mover whose
    /// automatic count passed [`AUTO_POSE_CAP`]. Authored counts are admitted
    /// first ([`Self::admit_pose_counts`]).
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
            assert!(
                placed.count <= MAX_POSES,
                "{}: `light_poses` {} is past the most poses, which admission refuses first",
                analysed.label,
                placed.count,
            );
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
