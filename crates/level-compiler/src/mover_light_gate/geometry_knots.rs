// Geometry knots: poses placed where a mover's brushes start or stop
// enclosing a near-field probe along its path, by the bake's own enclosing
// rule, one just inside and one clear outside each end.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use glam::Vec3;

use super::pose::{AnalysedMover, Pose};
use super::travel::{PathTravel, TravelLeg};
use crate::map_data::{BrushVolume, MapKinematicMover};
use crate::mover_probe_channel::{NearFieldDomain, enclosure_inset};
use crate::sh_bake::{ProbeGridLayout, vec3_from};

/// How far past each end of an enclosure interval its outside knot sits, in
/// metres of travel: one map unit. An enclosed inside knot blends values
/// across, so between an outside knot and its inside knot the probe is out
/// of the mover yet its values are still mid-lerp; this distance is that
/// window. Source: the task 15b sweep (`mover-light-flow--mid-travel-poses`
/// `captures/t15b-results.md`) of 4.4, 2, 1 and 0.2 units against the
/// fine-pose truth bake: the rising room's ceiling residual fell 2.18 →
/// 1.46 → 0.45 → 0.23 luma and em_sweeper's west face passed from 1 unit
/// down, while 1 unit keeps the knot 8× clear of the gather lift's band
/// ([`OUTSIDE_KNOT_CLEARANCE_M`]) and four fewer room knots than 0.2 units.
pub(crate) const OUTSIDE_KNOT_DISTANCE_M: f64 = 1.0 / 39.37;

/// How far inside each end of an enclosure interval its inside knot sits,
/// in metres of travel: just inside, so the pose there encloses the probe
/// by the bake's rule and stores the enclosed depth. Source: the same
/// evidence, where knots placed one fine step (4.4 units) loose brought the
/// depth spikes back; this keeps them well inside that slack. GPU captures
/// tune it.
pub(crate) const INSIDE_KNOT_MARGIN_M: f64 = 0.01;

/// How far, in metres of travel, a knot may move to merge with another, so
/// knots whose targets lie within twice it can share one pose; a merge
/// never moves a knot to the wrong side of its own probe's interval. A
/// quarter of the 4.4-unit (0.11 m) slack the offline evidence shows fails.
/// GPU captures tune it.
pub(crate) const KNOT_MERGE_TOLERANCE_M: f64 = 0.025;

/// The least travel, in metres, by which an outside knot clears its
/// enclosure interval's end: the enclosing inset (about the gather lift,
/// `RAY_EPSILON`, 1 mm) plus the lift, so every lifted gather ray of the
/// probe starts outside the posed brush, plus [`SIDE_GUARD_M`]. Merging
/// never pulls an outside knot nearer than this.
pub(crate) const OUTSIDE_KNOT_CLEARANCE_M: f64 = 0.003;

/// The least travel, in metres, by which a knot stays on its side of an
/// interval end: far above the `f32` rounding of a posed offset, so the
/// pose placed at a knot reads the side the exact interval gives it.
const SIDE_GUARD_M: f64 = 0.001;

/// A mover's geometry knots: strictly ascending travel fractions strictly
/// between its termini, distinct in `f32`.
#[derive(Debug, Clone, Default)]
pub(super) struct GeometryKnots {
    pub(super) knots: Vec<f32>,
    /// Near-field probes the mover's brushes enclose somewhere along the
    /// path.
    pub(super) enclosed_probes: usize,
    /// Their enclosures, seams joined ([`Enclosure`]), over every such
    /// probe.
    pub(super) intervals: usize,
}

/// Which side of an interval end a knot keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// The pose there encloses the probe.
    Inside,
    /// The pose there does not.
    Outside,
}

/// Where one knot may sit: a travel-fraction window around the knot's
/// target, and the probe and side it must keep.
#[derive(Debug, Clone, Copy)]
struct KnotWindow {
    lo: f64,
    hi: f64,
    target: f64,
    probe: Vec3,
    side: Side,
}

/// The mover's geometry knots: at each end of each enclosure of each valid
/// probe in its near-field domain, an inside knot just inside and an
/// outside knot [`OUTSIDE_KNOT_DISTANCE_M`] of travel outside, merged where
/// they lie within [`KNOT_MERGE_TOLERANCE_M`]. Enclosure intervals the
/// probe passes between through a seam of the mover's own solid join into
/// one enclosure ([`Enclosure`]). An end at a terminus places nothing: the
/// terminus already holds that side. Every placed knot's pose is checked
/// against the bake's enclosing rule ([`Pose::encloses`]) for each knot it
/// stands for; a disagreement fails by the mover's name, since it would
/// store the wrong state.
///
/// A probe enclosed for less than twice [`SIDE_GUARD_M`] of travel, or clear
/// between two enclosures for less, gets its knot at that sliver's middle
/// only when the pose there reads the sliver's side; the sliver is below
/// what an `f32` pose can resolve otherwise, and logged.
pub(super) fn geometry_knots(
    analysed: &AnalysedMover,
    mover: &MapKinematicMover,
    travel: &PathTravel,
    probes: &ProbeGridLayout,
    probe_spacing: f32,
) -> anyhow::Result<GeometryKnots> {
    let legs = travel.legs();
    if legs.is_empty() || probes.is_empty() {
        return Ok(GeometryKnots::default());
    }
    let length = f64::from(travel.length());
    let scale = Scale {
        outside: OUTSIDE_KNOT_DISTANCE_M / length,
        inside: INSIDE_KNOT_MARGIN_M / length,
        tolerance: travel.merge_tolerance(),
        guard: SIDE_GUARD_M / length,
        clearance: OUTSIDE_KNOT_CLEARANCE_M / length,
    };
    // `Pose` keeps the brushes with bounds, so the intervals read the same.
    let brushes: Vec<&BrushVolume> = mover
        .brush_volumes
        .iter()
        .filter(|brush| brush.aabb.is_valid())
        .collect();
    let posed_at = |t: f32| Pose::new("", &mover.brush_volumes, travel.mid_offset(t));
    let domain = NearFieldDomain::new(analysed, probe_spacing);
    let mut placed = GeometryKnots::default();
    let mut windows = Vec::new();
    let mut slivers = 0usize;
    for index in domain.valid_probes(probes) {
        let probe = vec3_from(probes.probe_positions[index as usize]);
        let enclosures = enclosures(&brushes, &legs, probe);
        if enclosures.is_empty() {
            continue;
        }
        placed.enclosed_probes += 1;
        placed.intervals += enclosures.len();
        for window in end_windows(&enclosures, probe, &scale) {
            if window.lo < window.hi {
                windows.push(window);
                continue;
            }
            // A sliver: keep its middle only where a pose there reads its
            // side.
            let reads = posed_at(window.lo as f32).encloses(probe) == (window.side == Side::Inside);
            if reads {
                windows.push(window);
            } else {
                slivers += 1;
            }
        }
    }
    if slivers > 0 {
        log::debug!(
            "[MoverLight] {}: {slivers} enclosure end(s) narrower than an `f32` pose resolves \
             place no knot",
            analysed.label,
        );
    }
    let termini = [
        &analysed.poses[0],
        &analysed.poses[analysed.poses.len() - 1],
    ];
    for (point, members) in stab(windows) {
        // A window reaching a terminus is held by the terminus's own pose.
        let t = point as f32;
        let mid;
        let (t, pose) = if t <= 0.0 {
            (None, termini[0])
        } else if t >= 1.0 {
            (None, termini[1])
        } else {
            mid = posed_at(t);
            (Some(t), &mid)
        };
        if let Some(window) = members
            .iter()
            .find(|window| pose.encloses(window.probe) != (window.side == Side::Inside))
        {
            anyhow::bail!(
                "{}: the geometry knot at t {point:.6} should {} the probe at {} by the \
                 bake's enclosing rule, and its pose does not",
                analysed.label,
                match window.side {
                    Side::Inside => "enclose",
                    Side::Outside => "leave clear",
                },
                window.probe,
            );
        }
        placed.knots.extend(t);
    }
    placed.knots.dedup();
    Ok(placed)
}

/// The placement constants as travel fractions of one path.
struct Scale {
    outside: f64,
    inside: f64,
    tolerance: f64,
    guard: f64,
    clearance: f64,
}

/// One stretch of travel over which a probe stays inside the mover: its
/// enclosure intervals by the bake's rule, joined across any gap the probe
/// spends inside the mover's solid. Such a gap is a seam where two of the
/// mover's brushes meet: the inset leaves the probe unenclosed within it,
/// but no face passes the probe there, so no outside knot belongs in it,
/// and one would store the solid's dark values mid-enclosure.
#[derive(Debug, Clone, Copy)]
struct Enclosure {
    /// The first and last enclosure intervals of the stretch; one interval
    /// where no seam splits it. Inside knots stay within these.
    first: (f64, f64),
    last: (f64, f64),
}

impl Enclosure {
    fn start(&self) -> f64 {
        self.first.0
    }

    fn end(&self) -> f64 {
        self.last.1
    }
}

/// `probe`'s enclosures by the mover's `brushes` posed along `legs`,
/// ascending and disjoint ([`Enclosure`]).
fn enclosures(brushes: &[&BrushVolume], legs: &[TravelLeg], probe: Vec3) -> Vec<Enclosure> {
    let enclosed = inside_intervals(brushes, legs, probe, enclosure_inset(probe));
    if enclosed.is_empty() {
        return Vec::new();
    }
    let solid = inside_intervals(brushes, legs, probe, 0.0);
    let mut stretches: Vec<Enclosure> = Vec::with_capacity(enclosed.len());
    for interval in enclosed {
        if let Some(stretch) = stretches.last_mut() {
            let gap = (stretch.end(), interval.0);
            if solid.iter().any(|&(lo, hi)| lo <= gap.0 && gap.1 <= hi) {
                stretch.last = interval;
                continue;
            }
        }
        stretches.push(Enclosure {
            first: interval,
            last: interval,
        });
    }
    stretches
}

/// The travel fractions at which the mover's `brushes`, posed along `legs`,
/// hold `probe` inside one brush by at least `inset` on every plane, the
/// half-space test [`Pose::contains_by`] reads; at
/// [`enclosure_inset`] it is the bake's enclosing rule
/// ([`Pose::encloses`]). Disjoint closed intervals, ascending. A posed
/// brush's planes move by the offset, so on one leg each plane's test is
/// linear in t: a brush's interval is the intersection over its planes, and
/// the pose's is their union over brushes, leg by leg. Intervals that meet,
/// at a waypoint or where two brushes touch, join.
fn inside_intervals(
    brushes: &[&BrushVolume],
    legs: &[TravelLeg],
    probe: Vec3,
    inset: f64,
) -> Vec<(f64, f64)> {
    let point = probe.as_dvec3();
    let mut intervals = Vec::new();
    for leg in legs {
        for brush in brushes {
            let (mut lo, mut hi) = (leg.t0, leg.t1);
            for plane in &brush.planes {
                // Posed at offset o, the plane is (n, d + n · o); the probe
                // is inside it by the inset where n · o ≥ n · p − d + inset.
                let need = plane.normal.dot(point) - plane.distance + inset;
                let at_start = plane.normal.dot(leg.offset0);
                let rate = plane.normal.dot(leg.per_t);
                if rate > 0.0 {
                    lo = lo.max(leg.t0 + (need - at_start) / rate);
                } else if rate < 0.0 {
                    hi = hi.min(leg.t0 + (need - at_start) / rate);
                } else if at_start < need {
                    hi = f64::NEG_INFINITY;
                }
                if lo > hi {
                    break;
                }
            }
            if lo <= hi {
                intervals.push((lo, hi));
            }
        }
    }
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut joined: Vec<(f64, f64)> = Vec::with_capacity(intervals.len());
    for (lo, hi) in intervals {
        match joined.last_mut() {
            Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
            _ => joined.push((lo, hi)),
        }
    }
    joined
}

/// The knot windows at each end of `enclosures`, one probe's ascending
/// disjoint enclosures. At an end strictly inside the path: an inside
/// window around [`INSIDE_KNOT_MARGIN_M`] inside, within the enclosure
/// interval at that end, and an outside window around
/// [`OUTSIDE_KNOT_DISTANCE_M`] outside, but no farther than the middle of
/// the gap to a neighbouring enclosure. Each spans the merge tolerance
/// either side of its target, cut to stay [`SIDE_GUARD_M`] on its side, and
/// an outside window [`OUTSIDE_KNOT_CLEARANCE_M`] clear of the end. A
/// side narrower than that guard leaves a zero-width window at its middle,
/// which the caller checks.
fn end_windows(enclosures: &[Enclosure], probe: Vec3, scale: &Scale) -> Vec<KnotWindow> {
    let mut windows = Vec::new();
    let mut push = |target: f64, (lo, hi): (f64, f64), sliver: f64, side: Side| {
        let (lo, hi) = (
            lo.max(target - scale.tolerance),
            hi.min(target + scale.tolerance),
        );
        let (lo, hi, target) = if lo < hi {
            (lo, hi, target.clamp(lo, hi))
        } else {
            (sliver, sliver, sliver)
        };
        windows.push(KnotWindow {
            lo,
            hi,
            target,
            probe,
            side,
        });
    };
    let inside = |(a, b): (f64, f64)| ((a + scale.guard, b - scale.guard), 0.5 * (a + b));
    for (index, enclosure) in enclosures.iter().enumerate() {
        let (a, b) = (enclosure.start(), enclosure.end());
        if a > scale.guard {
            let before = index.checked_sub(1).map_or(0.0, |i| enclosures[i].end());
            let clear = (
                if index == 0 {
                    0.0
                } else {
                    before + scale.guard
                },
                a - scale.clearance,
            );
            let gap_middle = 0.5 * (before + a);
            let (span, middle) = inside(enclosure.first);
            push(a + scale.inside, span, middle, Side::Inside);
            // With no enclosure before, the clear side runs back to t = 0.
            let floor = if index == 0 { 0.0 } else { gap_middle };
            push(
                (a - scale.outside).max(floor),
                clear,
                gap_middle,
                Side::Outside,
            );
        }
        if b < 1.0 - scale.guard {
            let after = enclosures.get(index + 1).map_or(1.0, Enclosure::start);
            let last = index + 1 == enclosures.len();
            let clear = (
                b + scale.clearance,
                if last { 1.0 } else { after - scale.guard },
            );
            let gap_middle = 0.5 * (b + after);
            let (span, middle) = inside(enclosure.last);
            push(b - scale.inside, span, middle, Side::Inside);
            let ceiling = if last { 1.0 } else { gap_middle };
            push(
                (b + scale.outside).min(ceiling),
                clear,
                gap_middle,
                Side::Outside,
            );
        }
    }
    windows
}

/// Merge knot windows into the fewest points, each point inside every
/// window it stands for: termini first, which hold any window reaching 0 or
/// 1, then greedily by right end, each point the mean of its group's
/// targets held to the group's common span. Every window is at most twice
/// the merge tolerance wide, so knots merge only within it, and each stays
/// inside its own window, on its side of its probe's interval. Probes whose
/// ends coincide, as a lattice layer's do, merge onto their shared target.
fn stab(mut windows: Vec<KnotWindow>) -> Vec<(f64, Vec<KnotWindow>)> {
    let mut groups: Vec<(f64, Vec<KnotWindow>)> = Vec::new();
    for terminus in [0.0, 1.0] {
        let (held, rest): (Vec<_>, Vec<_>) = windows
            .into_iter()
            .partition(|window| window.lo <= terminus && terminus <= window.hi);
        if !held.is_empty() {
            groups.push((terminus, held));
        }
        windows = rest;
    }
    windows.sort_by(|a, b| a.hi.total_cmp(&b.hi));
    let widest = windows
        .iter()
        .map(|window| window.hi - window.lo)
        .fold(0.0, f64::max);
    let mut taken = vec![false; windows.len()];
    for first in 0..windows.len() {
        if taken[first] {
            continue;
        }
        let end = windows[first].hi;
        let mut members = Vec::new();
        // Sorted by right end, every later window whose left end is at or
        // before `end` holds `end`; none past `end` plus the widest window
        // can.
        for (index, window) in windows.iter().enumerate().skip(first) {
            if window.hi > end + widest {
                break;
            }
            if !taken[index] && window.lo <= end {
                taken[index] = true;
                members.push(*window);
            }
        }
        let start = members
            .iter()
            .map(|window| window.lo)
            .fold(f64::MIN, f64::max);
        let mean = members.iter().map(|window| window.target).sum::<f64>() / members.len() as f64;
        groups.push((mean.clamp(start, end), members));
    }
    groups.sort_by(|a, b| a.0.total_cmp(&b.0));
    groups
}
