// Mid-travel pose placement: how many poses a mover's probe channel bakes,
// and where along its path they sit.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use postretro_level_format::texture_names::TextureNamesSection;

use super::geometry_knots::{GeometryKnots, geometry_knots};
use super::pose::{AnalysedMover, MoverPoses, Pose, Terminus};
use super::travel::PathTravel;
use crate::map_data::MapKinematicMover;
use crate::sh_bake::ProbeGridLayout;

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

/// How a mover's knots were placed, for the Build Summary: its even poses,
/// termini included, and its geometry knots. They sum to its pose count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KnotCounts {
    pub(crate) even: u32,
    pub(crate) geometry: u32,
}

impl KnotCounts {
    /// A mover posed at its termini alone.
    pub(crate) const TERMINI: Self = Self {
        even: 2,
        geometry: 0,
    };
}

/// One knot of a mover's union, and whether geometry placed it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PlacedKnot {
    t: f32,
    geometry: bool,
}

/// The even knots and the geometry knots as one strictly ascending set.
/// Termini are always kept, as the first and last knots. An even mid knot
/// within `tolerance` (a travel fraction) of a geometry knot merges into
/// it: the geometry knot sits where the bake's enclosure changes, and the
/// even one has no such reason to stay. Geometry knots lie strictly between
/// the termini and never merge away.
fn union_knots(even: &[f32], geometry: &[f32], tolerance: f64) -> Vec<PlacedKnot> {
    let near_geometry = |t: f32| {
        // The nearest geometry knots on either side of t.
        let at = geometry.partition_point(|&g| g < t);
        [at.checked_sub(1), Some(at)]
            .into_iter()
            .flatten()
            .filter_map(|index| geometry.get(index))
            .any(|&g| (f64::from(g) - f64::from(t)).abs() <= tolerance)
    };
    let last = even.len() - 1;
    let mut knots: Vec<PlacedKnot> = even
        .iter()
        .enumerate()
        .filter(|&(index, &t)| index == 0 || index == last || !near_geometry(t))
        .map(|(_, &t)| PlacedKnot { t, geometry: false })
        .chain(geometry.iter().map(|&t| PlacedKnot { t, geometry: true }))
        .collect();
    knots.sort_by(|a, b| a.t.total_cmp(&b.t));
    knots
}

/// The `-v` line per mover with geometry knots: its knots by kind.
fn log_knots(analysed: &AnalysedMover, knots: &[PlacedKnot], geometry: &GeometryKnots) {
    if geometry.knots.is_empty() {
        return;
    }
    let list = |kind: bool| {
        knots
            .iter()
            .filter(|knot| knot.geometry == kind)
            .map(|knot| format!("{:.4}", knot.t))
            .collect::<Vec<_>>()
            .join(", ")
    };
    log::debug!(
        "[MoverLight] {}: {} enclosure(s) over {} near-field probe(s); even knots \
         [{}]; geometry knots [{}]",
        analysed.label,
        geometry.intervals,
        geometry.enclosed_probes,
        list(false),
        list(true),
    );
}

/// One authored `light_poses` count and what its poses hold, for the
/// refusal's list.
struct AuthoredPoses<'a> {
    label: &'a str,
    count: u32,
    bytes: u64,
}

impl MoverPoses {
    /// Refuse authored `light_poses` counts placement cannot honour, naming
    /// the movers, before [`Self::place_poses`] allocates anything in
    /// proportion to them: a count past [`MAX_POSES`], or counts whose poses'
    /// own working set, summed over every authored mover, passes
    /// `working_set_max_bytes`. Each pose is priced from the mover's first
    /// terminus ([`crate::mover_probe_channel::pose_held_bytes`]): its knot,
    /// the pose as placed, its posed solid and the bounce plan's posed scene,
    /// all held before the SH base gate runs. Every mover's poses live
    /// together until then, so the movers' bytes sum; the refusal lists each
    /// contributing mover, largest first. Never places fewer poses. An
    /// automatic count is at most [`AUTO_POSE_CAP`] and is priced by the SH
    /// base gate with the rest of the channel.
    pub(crate) fn admit_pose_counts(
        &self,
        movers: &[MapKinematicMover],
        texture_names: &TextureNamesSection,
        working_set_max_bytes: u64,
    ) -> anyhow::Result<()> {
        let mut authored: Vec<AuthoredPoses<'_>> = Vec::new();
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
            let per_pose = crate::mover_probe_channel::pose_held_bytes(
                mover,
                analysed.pose(Terminus::First),
                texture_names,
            );
            authored.push(AuthoredPoses {
                label: &analysed.label,
                count,
                bytes: per_pose.saturating_mul(u64::from(count)),
            });
        }
        let total = authored
            .iter()
            .fold(0u64, |sum, poses| sum.saturating_add(poses.bytes));
        if total <= working_set_max_bytes {
            return Ok(());
        }
        // Stable, so equal costs keep mover-id order.
        authored.sort_by_key(|poses| std::cmp::Reverse(poses.bytes));
        let named = authored
            .iter()
            .map(|poses| {
                format!(
                    "{} ({} poses, {} bytes)",
                    poses.label, poses.count, poses.bytes
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "mover poses' working set refused before the SH base gate: the authored \
             `light_poses` poses hold an estimated {total} bytes, past the \
             `--working-set-max-size` budget of {working_set_max_bytes} bytes: {named}. \
             Lower `light_poses`, or raise `--working-set-max-size`"
        );
    }

    /// The one placement chokepoint: add each analysed mover's mid poses
    /// between its termini. With no `light_poses`, its knots are its even
    /// poses, at even arc length by the runtime's own rule
    /// ([`postretro_physics::kinematic_mover::MoverPathArcLengths::point_at`]) on the path as the runtime holds
    /// it (`f32` waypoints, as id 43 packs them), plus its geometry knots
    /// ([`super::geometry_knots`]), placed where its brushes start or stop
    /// enclosing a near-field probe of `probes`. An authored `light_poses`
    /// places exactly that many even poses and no geometry knot: the
    /// author's trade of exactness for cost. Termini keep their exact poses
    /// as knots 0 and 1. Warns once per mover whose automatic even count
    /// passed [`AUTO_POSE_CAP`]; geometry knots never count against it, and
    /// none is dropped to meet a count. Authored counts are admitted first
    /// ([`Self::admit_pose_counts`]).
    ///
    /// Fails by the mover's name only where a placed geometry knot's pose
    /// disagrees with the bake's enclosing rule, which placement checks at
    /// every knot.
    pub(crate) fn place_poses(
        &mut self,
        movers: &[MapKinematicMover],
        probe_spacing: f32,
        probes: &ProbeGridLayout,
    ) -> anyhow::Result<()> {
        for analysed in &mut self.analysed {
            let Some(mover) = movers.iter().find(|m| m.mover_id == analysed.mover_id) else {
                continue;
            };
            let travel = PathTravel::new(&analysed.path);
            let length = travel.length();
            let placed = pose_count(length, probe_spacing, mover.light_poses);
            if let Some(wanted) = placed.capped_from {
                log::warn!(
                    "[MoverLight] {}: its {length:.2} m path wants {wanted} even poses at the \
                     {probe_spacing} m probe spacing, past the cap of {AUTO_POSE_CAP}; baking \
                     {AUTO_POSE_CAP} even poses {:.2} m apart instead. Set `light_poses` on the \
                     mover to bake more",
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
            let even = even_knots(placed.count);
            let geometry = match mover.light_poses {
                Some(_) => GeometryKnots::default(),
                None => geometry_knots(analysed, mover, &travel, probes, probe_spacing)?,
            };
            let knots = union_knots(&even, &geometry.knots, travel.merge_tolerance());
            analysed.knot_counts = KnotCounts {
                even: knots.iter().filter(|knot| !knot.geometry).count() as u32,
                geometry: geometry.knots.len() as u32,
            };
            log_knots(analysed, &knots, &geometry);
            if knots.len() <= 2 {
                continue;
            }
            let mut poses = Vec::with_capacity(knots.len());
            poses.push(analysed.poses[0].clone());
            for knot in &knots[1..knots.len() - 1] {
                let t = knot.t;
                // Geometry knots sit closer than a hundredth apart.
                let name = if knot.geometry {
                    format!("t {t:.4}")
                } else {
                    format!("t {t:.2}")
                };
                poses.push(Pose::new(&name, &mover.brush_volumes, travel.mid_offset(t)));
            }
            poses.push(analysed.poses[analysed.poses.len() - 1].clone());
            analysed.poses = poses;
            analysed.knots = knots.iter().map(|knot| knot.t).collect();
        }
        Ok(())
    }
}
