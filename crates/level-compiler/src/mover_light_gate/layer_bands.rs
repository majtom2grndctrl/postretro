// Probe-layer enclosure bands along a mover's path: where a pose would hold
// a near-field probe neither enclosed nor clear of the gather's lift band,
// whether a face crossing the probe or one running along it holds it there,
// authored poses moved the least travel out of the crossings, and a warning
// for the grazes.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use glam::Vec3;

use super::geometry_knots::{SIDE_GUARD_M, Span, Sweep, clear_of_band, lift_band};
use super::pose::{AnalysedMover, Pose};
use super::travel::PathTravel;
use crate::map_data::MapKinematicMover;
use crate::mover_probe_channel::{NearFieldDomain, enclosure_inset};
use crate::sh_bake::{ProbeGridLayout, vec3_from};

/// What holds a probe within its enclosure band at some point of travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BandKind {
    /// A face or seam crossing the probe: the brush that holds it in the band
    /// encloses it next to that stretch, so the probe is entering or leaving
    /// the mover's solid. Authored poses move out of these.
    Crossing,
    /// A face running along the probe: no brush that holds it in the band
    /// encloses it next to that stretch. Warned, never moved out of.
    Graze,
}

/// A mid pose that holds a near-field probe within its enclosure band
/// ([`holds_in_band`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BandPose {
    /// The pose's index among the mover's knots.
    pub(crate) pose: usize,
    /// Its travel fraction.
    pub(crate) t: f32,
    /// One probe it holds in the band: a crossed one where there is one.
    pub(crate) probe: Vec3,
    pub(crate) kind: BandKind,
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

/// One near-field probe's band along the path, grown by [`SIDE_GUARD_M`]
/// each way along the face that bounds it: inside a brush grown by the lift
/// band and the guard, and not inside one brush by the enclosing inset plus
/// the guard. Split by what holds it there (owner, 2026-11-13). Each brush's
/// band less its own enclosure leaves pieces; a piece that touches that same
/// brush's enclosure is a crossing, the brush's face passing through the
/// probe. The probe's crossing stretches are those pieces less every brush's
/// enclosure; the rest of its band is graze. A seam is crossings from both
/// sides; a lift wall's outer face running past a probe between a floor and
/// a ceiling that enclose it is graze, even where it meets them. The first
/// and last legs are read on past the path's ends along their lines, so a
/// face that crosses a probe right at a terminus, enclosing it only beyond
/// the path, is a crossing too: a room resting with its roof on a probe
/// layer, then rising away from it. On a straight line a convex brush
/// whose face runs along the probe never encloses it, so the extension
/// never turns a graze into a crossing.
///
/// A brush's spans join across waypoints, so on a bent path a brush that
/// grazes a probe on one leg and encloses it from the joint on reads that
/// graze as a crossing: the conservative side, a nudge or a refusal rather
/// than a silent in-band pose.
struct ProbeBand {
    probe: Vec3,
    /// Ascending, disjoint, closed stretches of travel fraction.
    crossing: Vec<(f64, f64)>,
    graze: Vec<(f64, f64)>,
}

impl ProbeBand {
    fn new(sweep: &Sweep<'_>, probe: Vec3) -> Self {
        let reach = -(lift_band(probe) + SIDE_GUARD_M);
        let near = closed(&sweep.spans(probe, reach, true));
        if near.is_empty() {
            return Self {
                probe,
                crossing: Vec::new(),
                graze: Vec::new(),
            };
        }
        let depth = enclosure_inset(probe) + SIDE_GUARD_M;
        let deep = closed(&sweep.spans(probe, depth, false));
        // Each brush on the lines of the path's first and last legs, so a
        // face crossing the probe right at a terminus, its enclosure just
        // beyond the path, still reads as a crossing.
        let deep_by_brush = sweep.spans_by_brush(probe, depth, false, true);
        let mut crossed = Vec::new();
        for (brush, band) in sweep.spans_by_brush(probe, reach, true, true) {
            let own = deep_by_brush
                .iter()
                .find(|(deep, _)| *deep == brush)
                .map_or_else(Vec::new, |(_, spans)| closed(spans));
            crossed.extend(
                pieces(&closed(&band), &own)
                    .into_iter()
                    .filter(|piece| piece.touches)
                    .map(|piece| (piece.span.0.max(0.0), piece.span.1.min(1.0)))
                    .filter(|&(lo, hi)| lo < hi),
            );
        }
        let crossing = subtract(&merged(crossed), &deep);
        let graze = subtract(&subtract(&near, &deep), &crossing);
        Self {
            probe,
            crossing,
            graze,
        }
    }

    /// What holds the probe in its band at `t`, should a pose there hold it:
    /// a graze only where `t` lies in a graze stretch and no crossing one.
    /// Any other `t` reads as a crossing, the side that moves a pose.
    fn kind_at(&self, t: f64) -> BandKind {
        if !within(&self.crossing, t) && within(&self.graze, t) {
            BandKind::Graze
        } else {
            BandKind::Crossing
        }
    }
}

/// A probe some face of the mover runs along, and where along the path it
/// holds the probe within the band by the bake's own rule, guard left out.
struct Grazed {
    probe: Vec3,
    stretches: Vec<(f64, f64)>,
}

/// One mover's near-field probes and where along its path any of them lies
/// within the enclosure band, by what holds it there ([`ProbeBand`]).
pub(super) struct LayerBands<'m> {
    mover: &'m MapKinematicMover,
    travel: &'m PathTravel,
    /// Every valid probe of the near-field domain, at the position the
    /// gather traces from. No probe outside it comes within a band.
    probes: Vec<ProbeBand>,
    /// The probes' crossing stretches, merged: a superset of every pose
    /// [`holds_in_band`] judges in a crossing's band, since the guard is far
    /// above the `f32` rounding of a pose's t and offset; outside it, every
    /// pose is clean by the bake's own rule or holds probes only where a
    /// face runs along them.
    guarded: Vec<(f64, f64)>,
    /// The probes some face runs along within the band, by the bake's rule.
    grazed: Vec<Grazed>,
}

impl<'m> LayerBands<'m> {
    pub(super) fn new(
        analysed: &AnalysedMover,
        mover: &'m MapKinematicMover,
        travel: &'m PathTravel,
        layout: &ProbeGridLayout,
        probe_spacing: f32,
    ) -> Self {
        let mut bands = Self {
            mover,
            travel,
            probes: Vec::new(),
            guarded: Vec::new(),
            grazed: Vec::new(),
        };
        let legs = travel.legs();
        if legs.is_empty() || layout.is_empty() {
            return bands;
        }
        let sweep = Sweep::new(mover, legs, f64::from(travel.length()));
        let mut crossings = Vec::new();
        for index in NearFieldDomain::new(analysed, probe_spacing).valid_probes(layout) {
            let probe = vec3_from(layout.probe_positions[index as usize]);
            let band = ProbeBand::new(&sweep, probe);
            crossings.extend_from_slice(&band.crossing);
            if !band.graze.is_empty() {
                // The guard widens the graze past where the probe sits in
                // the band; only the stretch the bake's rule holds warns.
                let held = subtract(
                    &closed(&sweep.spans(probe, -lift_band(probe), true)),
                    &closed(&sweep.spans(probe, enclosure_inset(probe), false)),
                );
                let stretches = intersect(&band.graze, &held);
                if !stretches.is_empty() {
                    bands.grazed.push(Grazed { probe, stretches });
                }
            }
            bands.probes.push(band);
        }
        bands.guarded = merged(crossings);
        bands
    }

    /// The guarded crossing stretch holding travel fraction `t`, if any.
    fn stretch_at(&self, t: f64) -> Option<(f64, f64)> {
        let at = self.guarded.partition_point(|&(lo, _)| lo <= t);
        let &(lo, hi) = self.guarded.get(at.checked_sub(1)?)?;
        (t <= hi).then_some((lo, hi))
    }

    fn posed_at(&self, t: f32) -> Pose {
        Pose::new("", &self.mover.brush_volumes, self.travel.mid_offset(t))
    }

    /// A probe `pose`, at `t`, holds within a crossing's band, by the bake's
    /// own predicates; `None` where every near-field probe is clean or held
    /// only where a face runs along it.
    fn crossed_probe(&self, pose: &Pose, t: f32) -> Option<Vec3> {
        self.probes
            .iter()
            .find(|band| {
                band.kind_at(f64::from(t)) == BandKind::Crossing && holds_in_band(pose, band.probe)
            })
            .map(|band| band.probe)
    }

    /// The mid poses among `knots` that hold some near-field probe within a
    /// crossing's band. The termini are not judged: they never move.
    fn crossing_poses(&self, knots: &[f32]) -> Vec<BandPose> {
        let mid = 1..knots.len().saturating_sub(1);
        mid.filter_map(|pose| {
            let t = knots[pose];
            self.stretch_at(f64::from(t))?;
            self.crossed_probe(&self.posed_at(t), t)
                .map(|probe| BandPose {
                    pose,
                    t,
                    probe,
                    kind: BandKind::Crossing,
                })
        })
        .collect()
    }

    /// The mid poses among `knots` that hold some near-field probe within
    /// its band, of either kind: a crossed probe where the pose holds one.
    #[cfg(test)]
    fn poses_in_band(&self, knots: &[f32]) -> Vec<BandPose> {
        let mid = 1..knots.len().saturating_sub(1);
        mid.filter_map(|pose| {
            let t = knots[pose];
            let posed = self.posed_at(t);
            let mut held = self
                .probes
                .iter()
                .filter(|band| holds_in_band(&posed, band.probe))
                .map(|band| (band.probe, band.kind_at(f64::from(t))));
            let first = held.next()?;
            let (probe, kind) = std::iter::once(first)
                .chain(held)
                .find(|&(_, kind)| kind == BandKind::Crossing)
                .unwrap_or(first);
            Some(BandPose {
                pose,
                t,
                probe,
                kind,
            })
        })
        .collect()
    }

    /// Warn once, naming the mover, where a face of it runs along near-field
    /// probes within their band: how many, the nearest to its face, where
    /// along the path, and how far to move the face. Poses there store part
    /// of the solid in those probes; placement leaves them where they are.
    pub(super) fn warn_grazes(&self, label: &str) {
        let Some((lo, hi)) = self
            .grazed
            .iter()
            .flat_map(|grazed| grazed.stretches.iter().copied())
            .reduce(|(lo, hi), (a, b)| (lo.min(a), hi.max(b)))
        else {
            return;
        };
        let (probe, off) = self
            .grazed
            .iter()
            .map(|grazed| {
                // Where the face runs along the probe longest.
                let (a, b) = grazed
                    .stretches
                    .iter()
                    .copied()
                    .max_by(|x, y| (x.1 - x.0).total_cmp(&(y.1 - y.0)))
                    .expect("a grazed probe has a stretch");
                let pose = self.posed_at((0.5 * (a + b)) as f32);
                (grazed.probe, face_distance(&pose, grazed.probe))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .expect("some probe is grazed");
        let length = f64::from(self.travel.length());
        log::warn!(
            "[MoverLight] {label}: a face of the mover runs along {} lighting probe(s) without \
             enclosing them, the nearest at ({:.3}, {:.3}, {:.3}) m, {:.1} mm off the face, over \
             t {lo:.3} to {hi:.3} ({:.2} m of travel); poses there bake part of the mover's \
             solid into those probes. Move that face at least {:.1} mm off the probe layer, \
             either way, to clear it",
            self.grazed.len(),
            probe.x,
            probe.y,
            probe.z,
            off * 1000.0,
            (hi - lo) * length,
            (lift_band(probe) + SIDE_GUARD_M) * 1000.0,
        );
    }

    /// An authored `light_poses` count's knots, `knots`, with every mid pose
    /// that holds a near-field probe within a crossing's band moved the least
    /// travel out of every probe's crossing (owner, 2026-11-12; crossings
    /// only, 2026-11-13). It goes to the nearer end of the guarded stretch
    /// holding it ([`Self::guarded`]), past the guard, or to the farther end
    /// where the nearer would reach or pass a neighbouring pose. A pose held
    /// in the band only where a face runs along the probe stays: moving it
    /// clear could take the whole trip ([`Self::warn_grazes`] names the
    /// face). The termini never move, poses keep their order and no two
    /// share a knot, so the count is the author's. Every other pose keeps its
    /// knot bit for bit.
    ///
    /// Fails by the mover's name where neither end lies strictly between the
    /// pose's neighbours: a count so dense that several poses fall in one
    /// crossing, or one crossing reaching a terminus with a pose beside it.
    /// Never drops a pose.
    pub(super) fn clear_authored(&self, label: &str, knots: Vec<f32>) -> anyhow::Result<Vec<f32>> {
        if knots.len() <= 2 {
            return Ok(knots);
        }
        let crossed = self.crossing_poses(&knots);
        let length = f64::from(self.travel.length());
        let mut placed = knots.clone();
        for BandPose { pose, t, probe, .. } in crossed {
            let (lo, hi) = self
                .stretch_at(f64::from(t))
                .expect("a pose in a crossing's band lies in a guarded stretch");
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
                    "{label}: `light_poses` pose {pose} at t {t:.6} holds the probe at {probe} \
                     within the band of a face or seam of the mover crossing it, where its stored \
                     state would be the solid's, and neither end of that crossing (t {lo:.6} to \
                     {hi:.6}, {:.2} mm of travel) lies between its neighbouring poses at t \
                     {left:.6} and {right:.6}. Set a lower `light_poses` that leaves this pose \
                     room outside that stretch, set it to 2 to bake the ends alone, or remove it \
                     for automatic placement",
                    (hi - lo) * length * 1000.0,
                );
            };
            // The guard keeps the moved pose clear of every crossing; check
            // it by the bake's own predicates, since a pose left in a
            // crossing's band would store the solid.
            if let Some(stuck) = self.crossed_probe(&self.posed_at(to), to) {
                anyhow::bail!(
                    "{label}: `light_poses` pose {pose}, moved from t {t:.6} to {to:.6} out of the \
                     enclosure band, still holds the probe at {stuck} within it",
                );
            }
            log::info!(
                "[MoverLight] {label}: `light_poses` pose {pose} at t {t:.6} held the probe at \
                 {probe} within the band of a face or seam; moved {:.2} mm of travel to t {to:.6}",
                (f64::from(to) - from).abs() * length * 1000.0,
            );
            placed[pose] = to;
        }
        Ok(placed)
    }
}

/// How far `probe` lies from the nearest face of `pose` whose brush holds
/// it within the lift band, in metres: the largest plane distance of that
/// brush, inside or out.
fn face_distance(pose: &Pose, probe: Vec3) -> f64 {
    let (point, band) = (probe.as_dvec3(), lift_band(probe));
    pose.brushes
        .iter()
        .filter(|brush| {
            point.cmpge(brush.bounds.min - band).all() && point.cmple(brush.bounds.max + band).all()
        })
        .map(|brush| {
            brush
                .planes
                .iter()
                .map(|&(normal, distance)| normal.dot(point) - distance)
                .fold(f64::NEG_INFINITY, f64::max)
                .abs()
        })
        .fold(f64::INFINITY, f64::min)
}

fn closed(spans: &[Span]) -> Vec<(f64, f64)> {
    spans.iter().map(|span| (span.lo, span.hi)).collect()
}

/// `stretches` sorted, with those that meet or overlap merged.
fn merged(mut stretches: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    stretches.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(stretches.len());
    for (lo, hi) in stretches {
        match out.last_mut() {
            Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
            _ => out.push((lo, hi)),
        }
    }
    out
}

/// Whether closed, ascending, disjoint `stretches` hold `t`.
fn within(stretches: &[(f64, f64)], t: f64) -> bool {
    let at = stretches.partition_point(|&(lo, _)| lo <= t);
    at.checked_sub(1)
        .and_then(|at| stretches.get(at))
        .is_some_and(|&(_, hi)| t <= hi)
}

/// One piece of a stretch less a cut, and whether a cut stretch bounds it
/// at either end.
struct Piece {
    span: (f64, f64),
    touches: bool,
}

/// `from` less `cut`, both ascending and disjoint, piece by piece. A piece
/// that ends where a cut begins, or begins where one ends, touches it.
fn pieces(from: &[(f64, f64)], cut: &[(f64, f64)]) -> Vec<Piece> {
    let mut out = Vec::new();
    for &(start, end) in from {
        let mut lo = start;
        let mut after_cut = false;
        for &(c0, c1) in cut {
            if c1 < lo {
                continue;
            }
            if c0 > end {
                break;
            }
            if c0 > lo {
                out.push(Piece {
                    span: (lo, c0),
                    touches: true,
                });
            }
            lo = lo.max(c1);
            after_cut = true;
        }
        if lo < end {
            out.push(Piece {
                span: (lo, end),
                touches: after_cut,
            });
        }
    }
    out
}

/// `from` less `cut`, both ascending and disjoint.
fn subtract(from: &[(f64, f64)], cut: &[(f64, f64)]) -> Vec<(f64, f64)> {
    pieces(from, cut)
        .into_iter()
        .map(|piece| piece.span)
        .collect()
}

/// Where `a` and `b`, both ascending and disjoint, overlap by more than a
/// point.
fn intersect(a: &[(f64, f64)], b: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let (lo, hi) = (a[i].0.max(b[j].0), a[i].1.min(b[j].1));
        if lo < hi {
            out.push((lo, hi));
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// The mid poses among `knots` (the mover's travel fractions, termini first
/// and last) that hold a near-field probe of `layout` within its enclosure
/// band ([`holds_in_band`]), of either kind, each with one such probe: a
/// crossed one where the pose holds one. The termini are not judged: they
/// never move.
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

/// [`LayerBands::clear_authored`] on any ascending `knots`, termini first
/// and last: even authored knots reach some of its branches only within an
/// `f32` step.
#[cfg(test)]
pub(super) fn clear_authored_knots(
    analysed: &AnalysedMover,
    mover: &MapKinematicMover,
    layout: &ProbeGridLayout,
    probe_spacing: f32,
    knots: Vec<f32>,
) -> anyhow::Result<Vec<f32>> {
    let travel = PathTravel::new(&analysed.path);
    LayerBands::new(analysed, mover, &travel, layout, probe_spacing)
        .clear_authored(&analysed.label, knots)
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
