// Probe-layer enclosure bands along a mover's path: where a pose would hold
// a near-field probe neither enclosed nor clear of the gather's lift band,
// and authored poses moved the least travel out of them.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use glam::Vec3;

use super::geometry_knots::{SIDE_GUARD_M, Sweep, clear_of_band, lift_band};
use super::pose::{AnalysedMover, Pose};
use super::travel::PathTravel;
use crate::map_data::MapKinematicMover;
use crate::mover_probe_channel::{NearFieldDomain, enclosure_inset};
use crate::sh_bake::{ProbeGridLayout, vec3_from};

/// A mid pose that holds a near-field probe within its enclosure band
/// ([`holds_in_band`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BandPose {
    /// The pose's index among the mover's knots.
    pub(crate) pose: usize,
    /// Its travel fraction.
    pub(crate) t: f32,
    /// One probe it holds in the band.
    pub(crate) probe: Vec3,
}

/// Whether `pose` holds `probe` within the enclosure band: neither enclosed
/// by the bake's rule ([`Pose::encloses`]) nor clear of the lift band
/// ([`clear_of_band`]). Some of the probe's lifted gather rays then start in
/// the posed solid, so a mid pose there stores a partly dark in-solid state,
/// neither the enclosed pose's zero bounce nor a clear one's open air.
/// A probe within the inset of a seam between two touching brushes is in
/// the band too: inside the solid, but enclosed by neither brush.
pub(crate) fn holds_in_band(pose: &Pose, probe: Vec3) -> bool {
    !pose.encloses(probe) && !clear_of_band(pose, probe)
}

/// One mover's near-field probes and where along its path any of them lies
/// within the enclosure band, grown by [`SIDE_GUARD_M`] each way along the
/// face that bounds it.
struct LayerBands<'m> {
    mover: &'m MapKinematicMover,
    travel: &'m PathTravel,
    /// The valid probes of the near-field domain, at the positions the
    /// gather traces from. No probe outside it comes within a band.
    probes: Vec<Vec3>,
    /// Ascending, disjoint, closed stretches of travel fraction over which
    /// some probe lies within its band grown by the guard: inside the brush
    /// grown by the lift band and the guard, and not inside one brush by the
    /// enclosing inset plus the guard. A superset of every pose
    /// [`holds_in_band`] judges in the band, since the guard is far above the
    /// `f32` rounding of a pose's t and offset; outside it, every pose is
    /// clean by the bake's own rule.
    guarded: Vec<(f64, f64)>,
}

impl<'m> LayerBands<'m> {
    fn new(
        analysed: &AnalysedMover,
        mover: &'m MapKinematicMover,
        travel: &'m PathTravel,
        layout: &ProbeGridLayout,
        probe_spacing: f32,
    ) -> Self {
        let legs = travel.legs();
        if legs.is_empty() || layout.is_empty() {
            return Self {
                mover,
                travel,
                probes: Vec::new(),
                guarded: Vec::new(),
            };
        }
        let probes: Vec<Vec3> = NearFieldDomain::new(analysed, probe_spacing)
            .valid_probes(layout)
            .into_iter()
            .map(|index| vec3_from(layout.probe_positions[index as usize]))
            .collect();
        let sweep = Sweep::new(mover, legs, f64::from(travel.length()));
        let mut stretches = Vec::new();
        for &probe in &probes {
            let near = sweep.spans(probe, -(lift_band(probe) + SIDE_GUARD_M), true);
            if near.is_empty() {
                continue;
            }
            let deep = sweep.spans(probe, enclosure_inset(probe) + SIDE_GUARD_M, false);
            let near: Vec<(f64, f64)> = near.iter().map(|span| (span.lo, span.hi)).collect();
            let deep: Vec<(f64, f64)> = deep.iter().map(|span| (span.lo, span.hi)).collect();
            stretches.extend(subtract(&near, &deep));
        }
        stretches.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut guarded: Vec<(f64, f64)> = Vec::with_capacity(stretches.len());
        for (lo, hi) in stretches {
            match guarded.last_mut() {
                Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
                _ => guarded.push((lo, hi)),
            }
        }
        Self {
            mover,
            travel,
            probes,
            guarded,
        }
    }

    /// The guarded stretch holding travel fraction `t`, if any.
    fn stretch_at(&self, t: f64) -> Option<(f64, f64)> {
        let at = self.guarded.partition_point(|&(lo, _)| lo <= t);
        let &(lo, hi) = self.guarded.get(at.checked_sub(1)?)?;
        (t <= hi).then_some((lo, hi))
    }

    fn posed_at(&self, t: f32) -> Pose {
        Pose::new("", &self.mover.brush_volumes, self.travel.mid_offset(t))
    }

    /// A probe the mid pose at `t` holds within its band, by the bake's own
    /// predicates; `None` where every near-field probe is clean.
    fn probe_in_band(&self, t: f32) -> Option<Vec3> {
        self.stretch_at(f64::from(t))?;
        let pose = self.posed_at(t);
        self.probes
            .iter()
            .copied()
            .find(|&probe| holds_in_band(&pose, probe))
    }

    /// The mid poses among `knots` that hold some near-field probe within
    /// its band. The termini are not judged: they never move.
    fn poses_in_band(&self, knots: &[f32]) -> Vec<BandPose> {
        let mid = 1..knots.len().saturating_sub(1);
        mid.filter_map(|pose| {
            let t = knots[pose];
            self.probe_in_band(t)
                .map(|probe| BandPose { pose, t, probe })
        })
        .collect()
    }
}

/// `from` less `cut`, both ascending and disjoint.
fn subtract(from: &[(f64, f64)], cut: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(start, end) in from {
        let mut lo = start;
        for &(c0, c1) in cut {
            if c1 < lo {
                continue;
            }
            if c0 > end {
                break;
            }
            if c0 > lo {
                out.push((lo, c0));
            }
            lo = lo.max(c1);
        }
        if lo < end {
            out.push((lo, end));
        }
    }
    out
}

/// The mid poses among `knots` (the mover's travel fractions, termini first
/// and last) that hold a near-field probe of `layout` within its enclosure
/// band ([`holds_in_band`]), each with one such probe. The termini are not
/// judged: they never move.
#[cfg(test)]
pub(crate) fn poses_in_band(
    analysed: &AnalysedMover,
    mover: &MapKinematicMover,
    knots: &[f32],
    layout: &ProbeGridLayout,
    probe_spacing: f32,
) -> Vec<BandPose> {
    let travel = PathTravel::new(&analysed.path);
    LayerBands::new(analysed, mover, &travel, layout, probe_spacing).poses_in_band(knots)
}

/// An authored `light_poses` count's knots, `knots`, with every mid pose
/// that holds a near-field probe within its enclosure band moved the least
/// travel out of every probe's band (owner, 2026-11-12). It goes to the
/// nearer end of the guarded stretch holding it ([`LayerBands::guarded`]),
/// past the guard, or to the farther end where the nearer would reach or
/// pass a neighbouring pose. The termini never move, poses keep their order
/// and no two share a knot, so the count is the author's. Every other pose
/// keeps its knot bit for bit.
///
/// Fails by the mover's name where neither end lies strictly between the
/// pose's neighbours: a count so dense that several poses fall in one band,
/// or one band reaching a terminus with a pose beside it. Never drops a pose.
pub(super) fn clear_authored_knots(
    analysed: &AnalysedMover,
    mover: &MapKinematicMover,
    travel: &PathTravel,
    layout: &ProbeGridLayout,
    probe_spacing: f32,
    knots: Vec<f32>,
) -> anyhow::Result<Vec<f32>> {
    if knots.len() <= 2 {
        return Ok(knots);
    }
    let bands = LayerBands::new(analysed, mover, travel, layout, probe_spacing);
    let in_band = bands.poses_in_band(&knots);
    let length = f64::from(travel.length());
    let mut placed = knots.clone();
    for BandPose { pose, t, probe } in in_band {
        let (lo, hi) = bands
            .stretch_at(f64::from(t))
            .expect("a pose in the band lies in a guarded stretch");
        let from = f64::from(t);
        let mut ends = [at_most(lo), at_least(hi)];
        // Stable: on a tie the earlier end goes first.
        ends.sort_by(|a, b| {
            (f64::from(*a) - from)
                .abs()
                .total_cmp(&(f64::from(*b) - from).abs())
        });
        let (left, right) = (placed[pose - 1], knots[pose + 1]);
        let Some(&to) = ends.iter().find(|&&end| left < end && end < right) else {
            anyhow::bail!(
                "{}: `light_poses` pose {pose} at t {t:.6} holds the probe at {probe} within the \
                 band of a face or seam of the mover, where its stored state would be the solid's, \
                 and neither end of that band (t {lo:.6} to {hi:.6}, {:.2} mm of travel) lies \
                 between its neighbouring poses at t {left:.6} and {right:.6}. Choose another \
                 `light_poses`, or remove it for automatic placement",
                analysed.label,
                (hi - lo) * length * 1000.0,
            );
        };
        // The guard keeps the moved pose clean; check it by the bake's own
        // predicates, since a pose left in the band would store the solid.
        let moved = bands.posed_at(to);
        if let Some(stuck) = bands
            .probes
            .iter()
            .copied()
            .find(|&probe| holds_in_band(&moved, probe))
        {
            anyhow::bail!(
                "{}: `light_poses` pose {pose}, moved from t {t:.6} to {to:.6} out of the \
                 enclosure band, still holds the probe at {stuck} within it",
                analysed.label,
            );
        }
        log::info!(
            "[MoverLight] {}: `light_poses` pose {pose} at t {t:.6} held the probe at {probe} \
             within the band of a face or seam; moved {:.2} mm of travel to t {to:.6}",
            analysed.label,
            (f64::from(to) - from).abs() * length * 1000.0,
        );
        placed[pose] = to;
    }
    Ok(placed)
}

/// The largest `f32` at or below `t`.
fn at_most(t: f64) -> f32 {
    let rounded = t as f32;
    if f64::from(rounded) > t {
        rounded.next_down()
    } else {
        rounded
    }
}

/// The smallest `f32` at or above `t`.
fn at_least(t: f64) -> f32 {
    let rounded = t as f32;
    if f64::from(rounded) < t {
        rounded.next_up()
    } else {
        rounded
    }
}
