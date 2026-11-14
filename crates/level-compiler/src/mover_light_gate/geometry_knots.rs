// Geometry knots: poses placed where a mover's brushes start or stop
// enclosing a near-field probe along its path, by the bake's own enclosing
// rule, one just inside and one clear of the gather's lift band outside
// each end.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use glam::{DVec3, Vec3};

use super::pose::{AnalysedMover, Pose};
use super::travel::{PathTravel, TravelLeg};
use crate::lightmap_bake::RAY_EPSILON;
use crate::map_data::{BrushVolume, MapKinematicMover};
use crate::mover_probe_channel::{NearFieldDomain, enclosure_inset};
use crate::partition::Aabb;
use crate::sh_bake::{ProbeGridLayout, vec3_from};

/// How far past each end of an enclosure interval its outside knot sits, in
/// metres of travel: one map unit, or farther where a face met at a slant
/// keeps the probe in the lift band longer ([`clear_of_band`]). An enclosure
/// run's first and last enclosing poses carry the outside knot's value state
/// under the dark depth tile (the probe channel's edge copy), so across this
/// distance the value holds while the depth goes from the clear tile to the
/// dark one. The 1 u figure comes from a sweep measured before the dark state
/// and edge copy existed. Source: a sweep of 4.4, 2, 1 and 0.2 units against
/// fine-pose truth bakes (`light_poses` 41 on `mover-setpiece`'s rising room,
/// before the map gained its hallway and big room, 57 on `enemy-movers`'
/// em_sweeper), on faces the movers cross square: the
/// room's ceiling residual fell 2.18 → 1.46 → 0.45 → 0.23 luma and
/// em_sweeper's west face came within its 3-level bar from 1 unit down, while
/// 1 unit stays about six times clear of the lift band ([`lift_band`]) on a
/// square crossing and places four fewer room knots than 0.2 units.
pub(crate) const OUTSIDE_KNOT_DISTANCE_M: f64 = 1.0 / 39.37;

/// How far inside each end of an enclosure interval its inside knot sits,
/// in metres of travel, or [`SIDE_GUARD_M`] along the face where that is
/// farther: just inside, so the pose there encloses the probe by the bake's
/// rule and stores the enclosed depth. Knots placed one fine step (4.4
/// units) loose brought the depth spikes back against the truth bakes; this
/// keeps them well inside that slack.
pub(crate) const INSIDE_KNOT_MARGIN_M: f64 = 0.01;

/// The inside knots each end of an enclosure interval places, by their
/// margin inside it in metres of travel: one, [`INSIDE_KNOT_MARGIN_M`]. The
/// probe channel's edge copy (`mover_probe_channel::edge_copy`) gives an
/// enclosure run's first and last enclosing poses the adjoining clear pose's
/// value state and keeps the poses between them dark, so a deeper margin
/// added here (a bounding pose about 1 u inside, should the owner's whole-
/// trip measurement call for one) is a dark interior pose with no change
/// there.
const INSIDE_KNOT_MARGINS_M: [f64; 1] = [INSIDE_KNOT_MARGIN_M];

/// How far, in metres of travel, a knot may move to merge with another, so
/// knots whose targets lie within twice it can share one pose; a merge
/// never moves a knot to the wrong side of its own probe's interval. A
/// quarter of the 4.4-unit (0.11 m) slack the truth bakes show fails.
pub(crate) const KNOT_MERGE_TOLERANCE_M: f64 = 0.025;

/// The least distance, along the face a knot's probe crosses, by which a
/// knot keeps its side: inside the enclosure by this past the enclosing
/// inset, or outside the lift band by this. Far above the `f32` rounding of
/// t and of a posed offset, so the pose placed at a knot reads the side the
/// exact interval gives it.
pub(super) const SIDE_GUARD_M: f64 = 0.001;

/// How far outside the mover's solid, in metres, a gap between two
/// enclosures may lie and still join them as a seam: far above the `f32`
/// rounding of a map's planes, so two brushes meeting on a sloped face
/// whose planes miss each other by a few ulps still join.
const SEAM_TOLERANCE_M: f64 = 1.0e-4;

/// How far outside a posed brush a probe's lifted gather rays can start
/// inside it: one [`RAY_EPSILON`] lift plus the enclosing inset
/// ([`enclosure_inset`]), itself a lift plus the lifted origin's `f32`
/// rounding. The direct half lifts its segments twice, so this covers both
/// halves.
pub(crate) fn lift_band(probe: Vec3) -> f64 {
    f64::from(RAY_EPSILON) + enclosure_inset(probe)
}

/// Whether `pose` leaves `probe` clear of the lift band ([`lift_band`]): for
/// every posed brush, the probe lies past one of its planes by more than the
/// band, or outside its bounds grown by the band. Either way no lifted
/// origin of the probe starts inside that brush, so the pose stores the
/// probe's open-air state. The rule every outside knot is checked against.
pub(crate) fn clear_of_band(pose: &Pose, probe: Vec3) -> bool {
    let band = lift_band(probe);
    let point = probe.as_dvec3();
    !pose.brushes.iter().any(|brush| {
        within_grown(&brush.bounds, point, band)
            && brush
                .planes
                .iter()
                .all(|&(normal, distance)| normal.dot(point) - distance <= band)
    })
}

fn within_grown(bounds: &Aabb, point: DVec3, by: f64) -> bool {
    point.cmpge(bounds.min - by).all() && point.cmple(bounds.max + by).all()
}

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
    /// The pose there leaves the probe clear of the lift band.
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
/// probe in its near-field domain, an inside knot [`INSIDE_KNOT_MARGIN_M`]
/// inside and an outside knot [`OUTSIDE_KNOT_DISTANCE_M`] outside, merged
/// where they lie within [`KNOT_MERGE_TOLERANCE_M`] of travel. Enclosures
/// meeting at a seam of the mover's own solid join ([`Enclosure`]). Outside
/// knots sit only where the pose leaves the probe clear of the lift band of
/// every brush ([`clear_of_band`]), in the clear stretch nearest the end on
/// its side and short of the neighbouring enclosure. An end with no such
/// stretch places no outside knot: a terminus holds that side, or the
/// neighbouring enclosure's edge pose is the next stored state. An end at a terminus
/// places nothing. Every placed knot's pose is checked against
/// the bake's enclosing rule ([`Pose::encloses`]) or the band for each knot
/// it stands for; a disagreement fails by the mover's name, since it would
/// store the wrong state.
///
/// A probe enclosed by less than [`SIDE_GUARD_M`] past the inset either side
/// gets its inside knot at the enclosure's middle only when the pose there
/// encloses it; the enclosure is below what an `f32` pose can resolve
/// otherwise, and logged.
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
    let sweep = Sweep::new(mover, legs, length);
    let tolerance = travel.merge_tolerance();
    let posed_at = |t: f32| Pose::new("", &mover.brush_volumes, travel.mid_offset(t));
    let domain = NearFieldDomain::new(analysed, probe_spacing);
    let mut placed = GeometryKnots::default();
    let mut windows = Vec::new();
    let mut slivers = 0usize;
    for index in domain.valid_probes(probes) {
        let probe = vec3_from(probes.probe_positions[index as usize]);
        let enclosures = sweep.enclosures(probe);
        if enclosures.is_empty() {
            continue;
        }
        placed.enclosed_probes += 1;
        placed.intervals += enclosures.len();
        let clear = sweep.clear_of_band(probe);
        let scale = Scale { length, tolerance };
        for window in end_windows(&enclosures, &clear, probe, &scale) {
            if window.lo < window.hi {
                windows.push(window);
                continue;
            }
            // An enclosure too shallow to hold a window: keep its middle
            // only where a pose there encloses the probe.
            if posed_at(window.lo as f32).encloses(probe) {
                windows.push(window);
            } else {
                slivers += 1;
            }
        }
    }
    if slivers > 0 {
        log::debug!(
            "[MoverLight] {}: {slivers} enclosure(s) shallower than an `f32` pose resolves \
             place no inside knot",
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
        if let Some(window) = members.iter().find(|window| match window.side {
            Side::Inside => !pose.encloses(window.probe),
            Side::Outside => !clear_of_band(pose, window.probe),
        }) {
            let should = match window.side {
                Side::Inside => format!(
                    "enclose the probe at {} by the bake's enclosing rule",
                    window.probe
                ),
                Side::Outside => format!(
                    "leave the probe at {} clear of the gather's lift band",
                    window.probe
                ),
            };
            anyhow::bail!(
                "{}: the geometry knot at t {point:.6} should {should}, and its pose does not",
                analysed.label,
            );
        }
        placed.knots.extend(t);
    }
    placed.knots.dedup();
    Ok(placed)
}

/// A mover's brushes along its path: the legs, and per (leg, brush) the
/// hull its bounds sweep over the leg, which a probe the brush encloses, or
/// holds within the band, lies inside once grown by the band.
pub(super) struct Sweep<'m> {
    brushes: Vec<&'m BrushVolume>,
    legs: Vec<TravelLeg>,
    /// (leg, brush, hull), one per leg and bounded brush.
    sweeps: Vec<(usize, usize, Aabb)>,
    /// The path's length: the rate an end falls back to when no face sets
    /// it.
    length: f64,
}

/// Slack on the sweep prefilter, in metres: far above the rounding of a
/// brush's bounds against its planes.
const SWEEP_SLACK_M: f64 = 1.0e-6;

impl<'m> Sweep<'m> {
    pub(super) fn new(mover: &'m MapKinematicMover, legs: Vec<TravelLeg>, length: f64) -> Self {
        // `Pose` keeps the brushes with bounds, so the spans read the same.
        let brushes: Vec<&BrushVolume> = mover
            .brush_volumes
            .iter()
            .filter(|brush| brush.aabb.is_valid())
            .collect();
        let mut sweeps = Vec::with_capacity(legs.len() * brushes.len());
        for (at, leg) in legs.iter().enumerate() {
            let end = leg.offset0 + leg.per_t * (leg.t1 - leg.t0);
            for (index, brush) in brushes.iter().enumerate() {
                let mut hull = Aabb {
                    min: brush.aabb.min + leg.offset0,
                    max: brush.aabb.max + leg.offset0,
                };
                hull.expand_aabb(&Aabb {
                    min: brush.aabb.min + end,
                    max: brush.aabb.max + end,
                });
                sweeps.push((at, index, hull));
            }
        }
        Self {
            brushes,
            legs,
            sweeps,
            length,
        }
    }

    /// `probe`'s enclosures, ascending and disjoint ([`Enclosure`]). Two
    /// enclosure spans join across a seam: a gap that lies inside the
    /// mover's solid within [`SEAM_TOLERANCE_M`] and is no longer, along the
    /// faces that bound it, than the enclosing inset either side of a shared
    /// face. A longer gap, such as a probe grazing a wall's outer face
    /// between a floor and a ceiling that reach past it, stays two
    /// enclosures, each with its own knots.
    fn enclosures(&self, probe: Vec3) -> Vec<Enclosure> {
        let inset = enclosure_inset(probe);
        let enclosed = self.spans(probe, inset, false);
        if enclosed.is_empty() {
            return Vec::new();
        }
        let solid = self.spans(probe, -SEAM_TOLERANCE_M, false);
        let seam = 2.0 * (inset + SEAM_TOLERANCE_M);
        let mut stretches: Vec<Enclosure> = Vec::with_capacity(enclosed.len());
        for span in enclosed {
            if let Some(stretch) = stretches.last_mut() {
                let gap = (stretch.last.hi, span.lo);
                let in_solid = solid
                    .iter()
                    .any(|whole| whole.lo <= gap.0 && gap.1 <= whole.hi);
                let along = (gap.1 - gap.0) * stretch.last.hi_rate.max(span.lo_rate);
                if in_solid && along <= seam {
                    stretch.last = span;
                    continue;
                }
            }
            stretches.push(Enclosure {
                first: span,
                last: span,
            });
        }
        stretches
    }

    /// The stretches of travel over which every posed brush leaves `probe`
    /// clear of its lift band by [`SIDE_GUARD_M`] ([`clear_of_band`]):
    /// the complement in [0, 1] of every brush's band, ascending and
    /// disjoint, whether or not that brush ever encloses the probe.
    fn clear_of_band(&self, probe: Vec3) -> Vec<(f64, f64)> {
        let band = self.spans(probe, -(lift_band(probe) + SIDE_GUARD_M), true);
        let mut clear = Vec::with_capacity(band.len() + 1);
        let mut from = 0.0;
        for span in &band {
            if span.lo > from {
                clear.push((from, span.lo));
            }
            from = span.hi.max(from);
        }
        if from < 1.0 {
            clear.push((from, 1.0));
        }
        clear
    }

    /// The travel fractions at which the mover's brushes hold `probe` inside
    /// one brush by at least `inset` on every plane, the half-space test
    /// [`Pose::contains_by`] reads; at [`enclosure_inset`] it is the bake's
    /// enclosing rule ([`Pose::encloses`]). A negative inset grows each brush
    /// by its magnitude; with `clip`, each grown brush is also clipped to its
    /// bounds grown as much, as [`clear_of_band`] reads the band. Disjoint
    /// closed spans, ascending. A posed brush's planes move by the offset,
    /// so on one leg each plane's test is linear in t: a brush's span is
    /// the intersection over its planes, and the pose's is their union over
    /// brushes, leg by leg. Spans that meet, at a waypoint or where two
    /// brushes touch, join.
    pub(super) fn spans(&self, probe: Vec3, inset: f64, clip: bool) -> Vec<Span> {
        join_spans(
            self.leg_spans(probe, inset, clip, false)
                .into_iter()
                .map(|(_, span)| span)
                .collect(),
        )
    }

    /// [`Self::spans`] brush by brush: each bounded brush that holds `probe`
    /// somewhere along the path, by its index among the bounded brushes,
    /// with its own spans joined across the legs. Ascending by brush. With
    /// `past_ends`, the first leg runs on before t 0 and the last past t 1
    /// along their lines, so a span the path's end cuts short reads whole.
    pub(super) fn spans_by_brush(
        &self,
        probe: Vec3,
        inset: f64,
        clip: bool,
        past_ends: bool,
    ) -> Vec<(usize, Vec<Span>)> {
        let mut spans = self.leg_spans(probe, inset, clip, past_ends);
        // Stable, so each brush keeps its legs in order.
        spans.sort_by_key(|&(brush, _)| brush);
        let mut by_brush: Vec<(usize, Vec<Span>)> = Vec::new();
        for (brush, span) in spans {
            match by_brush.last_mut() {
                Some((last, own)) if *last == brush => own.push(span),
                _ => by_brush.push((brush, vec![span])),
            }
        }
        by_brush
            .into_iter()
            .map(|(brush, own)| (brush, join_spans(own)))
            .collect()
    }

    /// Each (leg, brush) span of [`Self::spans`], unjoined, with its brush's
    /// index among the bounded brushes; with `past_ends`, the first and last
    /// legs unbounded before t 0 and past t 1 ([`Self::spans_by_brush`]).
    fn leg_spans(
        &self,
        probe: Vec3,
        inset: f64,
        clip: bool,
        past_ends: bool,
    ) -> Vec<(usize, Span)> {
        let point = probe.as_dvec3();
        // A grown brush left unclipped reaches past its grown bounds near a
        // sharp edge; the seam test's growth is small, so a wide margin
        // keeps the prefilter conservative there.
        let grow = match (inset < 0.0, clip) {
            (false, _) => 0.0,
            (true, true) => -inset,
            (true, false) => -100.0 * inset,
        } + SWEEP_SLACK_M;
        let mut spans = Vec::new();
        for &(at, index, ref hull) in &self.sweeps {
            // The hull bounds the leg as travelled, not its line.
            let (first, last) = (past_ends && at == 0, past_ends && at + 1 == self.legs.len());
            if !(first || last) && !within_grown(hull, point, grow) {
                continue;
            }
            let (leg, brush) = (&self.legs[at], self.brushes[index]);
            let mut span = Span {
                lo: if first { f64::NEG_INFINITY } else { leg.t0 },
                hi: if last { f64::INFINITY } else { leg.t1 },
                lo_rate: self.length,
                hi_rate: self.length,
            };
            let planes = brush
                .planes
                .iter()
                .map(|plane| (plane.normal, plane.distance));
            let bounds = (inset < 0.0 && clip)
                .then(|| bound_planes(&brush.aabb))
                .into_iter()
                .flatten();
            for (normal, distance) in planes.chain(bounds) {
                // Posed at offset o, the plane is (n, d + n · o); the probe
                // is inside it by the inset where n · o ≥ n · p − d + inset.
                let need = normal.dot(point) - distance + inset;
                let at_start = normal.dot(leg.offset0);
                let rate = normal.dot(leg.per_t);
                if rate > 0.0 {
                    let t = leg.t0 + (need - at_start) / rate;
                    if t > span.lo {
                        (span.lo, span.lo_rate) = (t, rate);
                    }
                } else if rate < 0.0 {
                    let t = leg.t0 + (need - at_start) / rate;
                    if t < span.hi {
                        (span.hi, span.hi_rate) = (t, -rate);
                    }
                } else if at_start < need {
                    span.hi = f64::NEG_INFINITY;
                }
                if span.lo > span.hi {
                    break;
                }
            }
            if span.lo <= span.hi {
                spans.push((index, span));
            }
        }
        spans
    }
}

/// `spans` sorted and joined where they meet: ascending, disjoint, closed.
fn join_spans(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by(|a, b| a.lo.total_cmp(&b.lo));
    let mut joined: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans {
        match joined.last_mut() {
            Some(last) if span.lo <= last.hi => {
                if span.hi > last.hi {
                    (last.hi, last.hi_rate) = (span.hi, span.hi_rate);
                }
            }
            _ => joined.push(span),
        }
    }
    joined
}

/// A brush's bounds as six half-spaces `n · p ≤ d`.
fn bound_planes(bounds: &Aabb) -> [(DVec3, f64); 6] {
    let (min, max) = (bounds.min, bounds.max);
    [
        (DVec3::X, max.x),
        (DVec3::NEG_X, -min.x),
        (DVec3::Y, max.y),
        (DVec3::NEG_Y, -min.y),
        (DVec3::Z, max.z),
        (DVec3::NEG_Z, -min.z),
    ]
}

/// One closed span of travel fractions, and at each end the rate, in metres
/// per unit of t, at which the probe's distance to the face that ends the
/// span there changes: |n · d(offset)/dt| for that face's plane. Turns a
/// distance along the face into travel. An end no face sets (a leg's start
/// or end) falls back to the path's length, a square crossing.
#[derive(Debug, Clone, Copy)]
pub(super) struct Span {
    pub(super) lo: f64,
    pub(super) hi: f64,
    lo_rate: f64,
    hi_rate: f64,
}

/// One stretch of travel over which a probe stays inside the mover: its
/// enclosure spans by the bake's rule, joined across any seam where two of
/// the mover's brushes meet. The inset leaves the probe unenclosed for a
/// sliver at the seam, but no face passes the probe there, so no outside
/// knot belongs in it, and one would store the solid's dark values
/// mid-enclosure.
#[derive(Debug, Clone, Copy)]
struct Enclosure {
    /// The first and last enclosure spans of the stretch; one span where no
    /// seam splits it. Inside knots stay within these.
    first: Span,
    last: Span,
}

/// A path's length and the knot merge tolerance as a travel fraction.
struct Scale {
    length: f64,
    tolerance: f64,
}

/// The knot windows at each end of `enclosures`, one probe's ascending
/// disjoint enclosures, strictly inside the path. An inside window around
/// each of [`INSIDE_KNOT_MARGINS_M`] of travel inside, within the enclosure
/// span at that end less [`SIDE_GUARD_M`] along the face that ends it. An outside
/// window around [`OUTSIDE_KNOT_DISTANCE_M`] of travel outside, no farther
/// than the middle of the gap to a neighbouring enclosure, within the
/// `clear` stretch nearest the end on its side, between it and that
/// neighbour (or the terminus): a face met at a slant keeps the probe in the
/// band longer, and the clear stretch pushes the knot out to where it
/// leaves. Where no clear stretch lies on that side, no outside window is
/// placed. Each window spans the merge tolerance either side of its target,
/// cut to its side. An enclosure too shallow for an inside window leaves a
/// zero-width one at its middle, which the caller checks.
fn end_windows(
    enclosures: &[Enclosure],
    clear: &[(f64, f64)],
    probe: Vec3,
    scale: &Scale,
) -> Vec<KnotWindow> {
    let mut windows = Vec::new();
    let tolerance = scale.tolerance;
    let mut push = |target: f64, (lo, hi): (f64, f64), side: Side| {
        let target = target.clamp(lo, hi);
        windows.push(KnotWindow {
            lo: lo.max(target - tolerance),
            hi: hi.min(target + tolerance),
            target,
            probe,
            side,
        });
    };
    let outside = OUTSIDE_KNOT_DISTANCE_M / scale.length;
    let inward = INSIDE_KNOT_MARGINS_M.map(|margin| margin / scale.length);
    for (index, enclosure) in enclosures.iter().enumerate() {
        let (start, end) = (enclosure.first, enclosure.last);
        let previous = index.checked_sub(1).map(|i| &enclosures[i]);
        let next = enclosures.get(index + 1);
        if start.lo > 0.0 {
            for inward in inward {
                let (target, deep) = inside(start, start.lo + inward);
                push(target, deep, Side::Inside);
            }
            let after = previous.map_or(0.0, |p| p.last.hi);
            if let Some(&piece) = clear
                .iter()
                .rev()
                .find(|piece| piece.1 <= start.lo && piece.0 >= after)
            {
                let floor = previous.map_or(0.0, |p| 0.5 * (p.last.hi + start.lo));
                push((start.lo - outside).max(floor), piece, Side::Outside);
            }
        }
        if end.hi < 1.0 {
            for inward in inward {
                let (target, deep) = inside(end, end.hi - inward);
                push(target, deep, Side::Inside);
            }
            let before = next.map_or(1.0, |n| n.first.lo);
            if let Some(&piece) = clear
                .iter()
                .find(|piece| piece.0 >= end.hi && piece.1 <= before)
            {
                let ceiling = next.map_or(1.0, |n| 0.5 * (end.hi + n.first.lo));
                push((end.hi + outside).min(ceiling), piece, Side::Outside);
            }
        }
    }
    windows
}

/// An inside knot's target and the stretch of `span` it may take: the
/// span less [`SIDE_GUARD_M`] of each face, or, where that leaves nothing,
/// the span's middle alone.
fn inside(span: Span, target: f64) -> (f64, (f64, f64)) {
    let deep = (
        span.lo + SIDE_GUARD_M / span.lo_rate,
        span.hi - SIDE_GUARD_M / span.hi_rate,
    );
    if deep.0 < deep.1 {
        (target, deep)
    } else {
        let middle = 0.5 * (span.lo + span.hi);
        (middle, (middle, middle))
    }
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
