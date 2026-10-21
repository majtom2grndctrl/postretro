// Reflection-probe capture exclusion: a probe's face mask, excluded-hull exit classification and the leak guard.
// See: context/lib/resource_management.md §5.2 (Exclusion)

use glam::{DVec3, Vec3};

use super::capture_fill::{Fill, KeptCandidate, find_fill};
use super::hit::{FaceMask, MaskedHit, RAY_EPSILON, SceneTracer, SurfaceHit};
use super::placement::{PLANE_TOLERANCE_M, PlacedProbe};
use crate::map_data::BrushVolume;
use crate::partition::Aabb;
use crate::partition::brush_bsp::REGION_CLASSIFICATION_EPSILON;

/// A point this far outside an excluded hull, off its planes, touches it.
/// Brush CSG cuts both facing faces across a gap up to its 1 mm epsilon,
/// measured off a brush's planes, so this contact holds at every ray angle,
/// and the half again above the epsilon keeps such a gap from ever tying.
/// A gap of 1 to 1.5 mm that CSG left whole reads the same: the fill reads
/// the surviving kept face before the exit is classified, and an excluded
/// neighbour is crossed whether entered at once or by a re-trace. Bounds
/// this close are contact candidates.
pub(super) const CONTACT_TOLERANCE_M: f64 = 1.5 * REGION_CLASSIFICATION_EPSILON;

/// A ray parallel to a hull plane lies on its inner side within this. It
/// only absorbs the rounding of a ray running on the plane (flush contact),
/// so a level ray half a millimetre under a ceiling never enters it.
const PARALLEL_INSIDE_TOLERANCE_M: f64 = 1.0e-5;

/// A ray meeting a hull plane at a rate below this runs parallel to it:
/// over any capture distance it drifts off the plane by less than
/// [`PARALLEL_INSIDE_TOLERANCE_M`].
const PARALLEL_RATE: f64 = 1.0e-10;

/// The world brushes a capture may exclude, and the brush each face came from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CaptureBrushes<'a> {
    /// `MapData::brush_volumes`: convex hulls before CSG.
    pub volumes: &'a [BrushVolume],
    /// Post-cut face → brush index into `volumes`, index-aligned with the
    /// geometry's faces; `None` for a face no brush owns.
    pub face_brushes: &'a [Option<u32>],
    /// `face_brushes` inverted.
    pub faces: &'a BrushFaces,
}

/// Each brush's post-cut faces, ascending: the face → brush table inverted
/// once per stage, so a probe's mask and contact faces cost what its brushes
/// own rather than a pass over every face in the world.
#[derive(Debug, Clone, Default)]
pub(crate) struct BrushFaces {
    /// Brush `b`'s faces are `faces[offsets[b]..offsets[b + 1]]`.
    offsets: Vec<u32>,
    faces: Vec<u32>,
}

impl BrushFaces {
    /// `face_brushes` inverted over `brush_count` brushes; a face naming a
    /// brush past them, or none, is in no brush's list.
    pub(crate) fn new(face_brushes: &[Option<u32>], brush_count: usize) -> Self {
        let owner = |brush: &Option<u32>| brush.filter(|&b| (b as usize) < brush_count);
        let mut offsets = vec![0u32; brush_count + 1];
        for brush in face_brushes.iter().filter_map(owner) {
            offsets[brush as usize + 1] += 1;
        }
        for b in 0..brush_count {
            offsets[b + 1] += offsets[b];
        }
        let mut next = offsets.clone();
        let mut faces = vec![0u32; offsets[brush_count] as usize];
        // Faces visited in ascending order land ascending in each list.
        for (face, brush) in face_brushes.iter().enumerate() {
            if let Some(brush) = owner(brush) {
                faces[next[brush as usize] as usize] = face as u32;
                next[brush as usize] += 1;
            }
        }
        Self { offsets, faces }
    }

    /// Brush `brush`'s faces, ascending; empty for a brush past the table.
    pub(crate) fn of(&self, brush: u32) -> &[u32] {
        let b = brush as usize;
        match (self.offsets.get(b), self.offsets.get(b + 1)) {
            (Some(&start), Some(&end)) => &self.faces[start as usize..end as usize],
            _ => &[],
        }
    }
}

/// What a capture ray reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RayOutcome {
    /// Nothing within reach: no distance (the tile stores 0).
    Miss,
    /// A kept front face, as traced, behind any excluded brush the ray crossed.
    Hit(SurfaceHit),
    /// The first excluded surface the ray skipped, read as if it were kept:
    /// the ray found nothing behind the excluded brushes it may show.
    LeakGuard(SurfaceHit),
    /// A kept brush's side, crossed inside an excluded hull or in contact
    /// with it, where CSG removed it against that brush.
    Fill(Fill),
}

impl RayOutcome {
    /// The traced surface the ray reads, with its distance from the ray
    /// origin. A fill's surface is synthesized instead
    /// ([`super::capture_fill::fill_surface`]).
    #[cfg(test)]
    pub(crate) fn surface(&self) -> Option<&SurfaceHit> {
        match self {
            Self::Miss | Self::Fill(_) => None,
            Self::Hit(hit) | Self::LeakGuard(hit) => Some(hit),
        }
    }

    /// How far along the ray its surface lies; `None` on a miss.
    #[cfg(test)]
    pub(crate) fn distance(&self) -> Option<f32> {
        match self {
            Self::Miss => None,
            Self::Hit(hit) | Self::LeakGuard(hit) => Some(hit.distance),
            Self::Fill(fill) => Some(fill.distance),
        }
    }
}

/// One probe's excluded set as its capture rays read it.
#[derive(Debug, Clone)]
pub(crate) struct ProbeExclusion<'a> {
    brushes: CaptureBrushes<'a>,
    /// Excluded brush indices, sorted and deduplicated.
    excluded: Vec<u32>,
    /// Their surviving faces.
    mask: FaceMask,
    /// Per excluded brush, parallel to `excluded`: the brushes whose bounds
    /// touch its own, among those touching the projection box. An exit point
    /// or a covering side's crossing lies within [`CONTACT_TOLERANCE_M`] of
    /// the hull, so any brush holding one is among these; one off the box
    /// could only place the ray outside it, where the leak guard reads the
    /// same surface whatever that brush is.
    contacts: Vec<Contacts>,
    /// Every kept brush some excluded brush touches, sorted by brush.
    kept: Vec<KeptCandidate>,
    projection_box: ProjectionBox,
}

#[derive(Debug, Clone, Default)]
struct Contacts {
    /// Brushes this probe keeps, its kept solid, as slots of `kept`.
    kept: Vec<usize>,
    /// Other brushes this probe excludes, as slots of `excluded`.
    excluded: Vec<usize>,
}

/// How a ray leaves an excluded hull it skipped.
enum Crossing {
    /// Through a kept brush's covering side, inside the hull or in contact.
    Fill(Fill),
    /// Into empty space at this distance: trace on from there.
    Exit(f64),
    /// Into kept solid, or past the crossing bound.
    LeakGuard,
}

impl<'a> ProbeExclusion<'a> {
    /// `excluded`: brush indices into `brushes.volumes`. Contacts are drawn
    /// per probe, so a brush another probe excludes is kept solid here when
    /// this one keeps it.
    pub(crate) fn new(
        tracer: &SceneTracer<'_>,
        brushes: CaptureBrushes<'a>,
        excluded: &[u32],
        probe: &PlacedProbe,
    ) -> Self {
        debug_assert!(
            excluded
                .iter()
                .all(|&brush| (brush as usize) < brushes.volumes.len()),
            "excluded brushes index the brush volumes"
        );
        let projection_box = ProjectionBox::of(probe);
        // Leaving out brushes wholly off the box captures the same only
        // while the capture point is inside it.
        debug_assert!(
            projection_box.holds(probe.capture_point_vec3().as_dvec3()),
            "the projection box holds the capture point: {probe:?}"
        );
        let mut excluded: Vec<u32> = excluded
            .iter()
            .copied()
            .filter(|&brush| (brush as usize) < brushes.volumes.len())
            .collect();
        // Slots and contact lookups binary-search it.
        excluded.sort_unstable();
        excluded.dedup();
        let mask = if excluded.is_empty() {
            FaceMask::default()
        } else {
            tracer.face_mask(
                excluded
                    .iter()
                    .flat_map(|&brush| brushes.faces.of(brush).iter().copied()),
            )
        };
        // One pass over the hulls per probe; every contact scan after it is
        // bounded by the brushes near the box.
        let near: Vec<u32> = if excluded.is_empty() {
            Vec::new()
        } else {
            (0..brushes.volumes.len() as u32)
                .filter(|&brush| projection_box.touches(&brushes.volumes[brush as usize].aabb))
                .collect()
        };
        let mut touching: Vec<(Vec<u32>, Vec<usize>)> = Vec::with_capacity(excluded.len());
        for &brush in &excluded {
            let bounds = &brushes.volumes[brush as usize].aabb;
            let (mut kept, mut others) = (Vec::new(), Vec::new());
            for &other in &near {
                if other == brush || !bounds_touch(bounds, &brushes.volumes[other as usize].aabb) {
                    continue;
                }
                match excluded.binary_search(&other) {
                    Ok(slot) => others.push(slot),
                    Err(_) => kept.push(other),
                }
            }
            touching.push((kept, others));
        }
        let mut kept_brushes: Vec<u32> = touching
            .iter()
            .flat_map(|(kept, _)| kept)
            .copied()
            .collect();
        kept_brushes.sort_unstable();
        kept_brushes.dedup();
        let kept = KeptCandidate::build(
            brushes.volumes,
            brushes.faces,
            tracer.geometry(),
            &kept_brushes,
        );
        let contacts = touching
            .into_iter()
            .map(|(kept, excluded)| Contacts {
                kept: kept
                    .iter()
                    .map(|brush| kept_brushes.binary_search(brush).expect("collected above"))
                    .collect(),
                excluded,
            })
            .collect();
        Self {
            brushes,
            excluded,
            mask,
            contacts,
            kept,
            projection_box,
        }
    }

    /// The excluded brushes, ascending.
    pub(crate) fn excluded(&self) -> &[u32] {
        &self.excluded
    }

    /// Every kept brush some excluded brush touches, ascending: the
    /// candidates whose sides a fill reads and whose hulls are kept solid.
    pub(crate) fn kept_contacts(&self) -> impl Iterator<Item = u32> + '_ {
        self.kept.iter().map(|candidate| candidate.brush)
    }

    /// The post-cut faces of the excluded brush in `slot` of
    /// [`Self::excluded`]: its share of the mask, ascending.
    pub(crate) fn faces_of(&self, slot: usize) -> &[u32] {
        self.brush_faces(self.excluded[slot])
    }

    /// The post-cut faces of `brush`, ascending: for a kept contact
    /// candidate, the faces its fill pools.
    pub(crate) fn brush_faces(&self, brush: u32) -> &[u32] {
        self.brushes.faces.of(brush)
    }

    /// The contacts of the excluded brush in `slot` of [`Self::excluded`]:
    /// its kept brushes, then its other excluded brushes, each ascending.
    pub(crate) fn contacts_of(&self, slot: usize) -> (Vec<u32>, Vec<u32>) {
        let contacts = &self.contacts[slot];
        (
            contacts.kept.iter().map(|&k| self.kept[k].brush).collect(),
            contacts
                .excluded
                .iter()
                .map(|&e| self.excluded[e])
                .collect(),
        )
    }

    /// Whether the probe excludes nothing: its capture is the unfiltered one.
    pub(crate) fn is_empty(&self) -> bool {
        self.mask.is_empty()
    }

    /// The surface a primary ray from `origin` along unit `direction` reads.
    /// With nothing excluded this is the plain closest hit. Otherwise each
    /// excluded brush the ray skips is crossed in turn: its hull exit into
    /// another excluded brush continues from that brush's exit, into kept
    /// solid takes the leak guard, and into empty space traces on from the
    /// exit under the same mask. Before its exit is classified, a ray that
    /// crosses into a kept brush inside the hull or in contact with it,
    /// through a side with surviving faces, fills there. A ray that skipped
    /// any excluded brush and then misses, or ends (traced or filled) outside
    /// the projection box, takes the leak guard. Every distance is from
    /// `origin`.
    pub(crate) fn trace(
        &self,
        tracer: &SceneTracer<'_>,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> RayOutcome {
        if self.is_empty() {
            return tracer
                .closest_hit(origin, direction, max_distance)
                .map_or(RayOutcome::Miss, RayOutcome::Hit);
        }
        let (origin64, direction64) = (origin.as_dvec3(), direction.as_dvec3());
        let mut min_distance = RAY_EPSILON;
        let mut first_skipped: Option<SurfaceHit> = None;
        // Each pass moves past one excluded hull, so the excluded set bounds
        // them; a ray re-entering hulls past that takes the guard, as does a
        // grazing ray that spends its passes re-entering the same hull.
        for _ in 0..=self.excluded.len() {
            let MaskedHit { hit, skipped } = tracer.closest_hit_masked(
                origin,
                direction,
                min_distance,
                max_distance,
                &self.mask,
            );
            let Some(skipped) = skipped else {
                return self.settle(hit, first_skipped, origin, direction);
            };
            let first = *first_skipped.get_or_insert(skipped);
            let slot = self.slot_of_face(skipped.face);
            debug_assert!(
                slot.is_some(),
                "the mask holds only excluded brushes' faces"
            );
            let Some(slot) = slot else {
                return RayOutcome::LeakGuard(first);
            };
            match self.cross(
                tracer,
                slot,
                origin64,
                direction64,
                f64::from(skipped.distance),
            ) {
                // A kept front face nearer than the fill hides it.
                Crossing::Fill(fill) if hit.is_some_and(|hit| hit.distance < fill.distance) => {
                    return self.settle(hit, first_skipped, origin, direction);
                }
                Crossing::Fill(fill) => {
                    let point = origin64 + direction64 * f64::from(fill.distance);
                    return if self.projection_box.holds(point) {
                        RayOutcome::Fill(fill)
                    } else {
                        RayOutcome::LeakGuard(first)
                    };
                }
                Crossing::LeakGuard => return RayOutcome::LeakGuard(first),
                // Never at or before the skipped face itself, so each pass
                // advances even when f32 rounds the exit below it.
                Crossing::Exit(exit) => min_distance = (exit as f32).max(skipped.distance),
            }
        }
        first_skipped.map_or(RayOutcome::Miss, RayOutcome::LeakGuard)
    }

    /// The ray's resolved hit, checked against the leak guard's miss and box
    /// rules once it has skipped an excluded brush.
    fn settle(
        &self,
        hit: Option<SurfaceHit>,
        first_skipped: Option<SurfaceHit>,
        origin: Vec3,
        direction: Vec3,
    ) -> RayOutcome {
        let Some(first) = first_skipped else {
            return hit.map_or(RayOutcome::Miss, RayOutcome::Hit);
        };
        match hit {
            Some(hit)
                if self
                    .projection_box
                    .holds(origin.as_dvec3() + direction.as_dvec3() * f64::from(hit.distance)) =>
            {
                RayOutcome::Hit(hit)
            }
            _ => RayOutcome::LeakGuard(first),
        }
    }

    /// Resolve the excluded hull in `slot`, entered at `entry`: a covering
    /// side crossed inside it or in contact with it fills, else its exit is
    /// classified, continuing through every excluded hull that exit lies
    /// in. A neighbour holds the exit when the ray is inside it just past
    /// the exit, within [`CONTACT_TOLERANCE_M`] of the hull ([`holds_past`]):
    /// CSG cut the facing faces across a gap that small, so no entry face
    /// would stop a re-trace. A level or gently rising ray leaving a
    /// pillar's side just under the ceiling enters the ceiling, if at all,
    /// far from the pillar, so it re-traces onto the intact ceiling.
    fn cross(
        &self,
        tracer: &SceneTracer<'_>,
        mut slot: usize,
        origin: DVec3,
        direction: DVec3,
        entry: f64,
    ) -> Crossing {
        let volumes = self.brushes.volumes;
        let mut reached = entry;
        let mut leaves = self.span(self.excluded[slot], origin, direction).1;
        for _ in 0..self.excluded.len() {
            let exit = leaves.max(reached);
            if !exit.is_finite() {
                return Crossing::LeakGuard;
            }
            let hull = &volumes[self.excluded[slot] as usize];
            let contacts = &self.contacts[slot];
            if let Some(fill) = find_fill(
                tracer,
                volumes,
                &self.kept,
                &contacts.kept,
                (origin, direction),
                (hull, reached),
            ) {
                return Crossing::Fill(fill);
            }
            // Where the ray leaves `brush`, when it holds the exit.
            let holds = |brush: u32| {
                let span = self.span(brush, origin, direction);
                holds_past(hull, span, exit, (origin, direction)).then_some(span.1)
            };
            // An excluded hull that holds the exit and reaches past it:
            // continue through it.
            let next = contacts
                .excluded
                .iter()
                .find_map(|&next| Some((next, holds(self.excluded[next])?)));
            if let Some((next, next_leaves)) = next {
                slot = next;
                reached = exit;
                leaves = next_leaves;
                continue;
            }
            // Kept solid behind the exit: leak guard.
            if contacts
                .kept
                .iter()
                .any(|&kept| holds(self.kept[kept].brush).is_some())
            {
                return Crossing::LeakGuard;
            }
            return Crossing::Exit(exit);
        }
        Crossing::LeakGuard
    }

    /// Where the ray enters and leaves brush `brush`'s convex hull: the
    /// latest plane it heads into and the earliest it heads out of. A plane
    /// the ray runs parallel to ([`PARALLEL_RATE`]), outside it, leaves no
    /// entry (`+∞`).
    fn span(&self, brush: u32, origin: DVec3, direction: DVec3) -> (f64, f64) {
        let (mut enters, mut leaves) = (f64::NEG_INFINITY, f64::INFINITY);
        for plane in &self.brushes.volumes[brush as usize].planes {
            let rate = plane.normal.dot(direction);
            // Positive while the origin lies inside the plane.
            let inside = plane.distance - plane.normal.dot(origin);
            if rate.abs() < PARALLEL_RATE {
                if inside < -PARALLEL_INSIDE_TOLERANCE_M {
                    enters = f64::INFINITY;
                }
            } else if rate > 0.0 {
                leaves = leaves.min(inside / rate);
            } else {
                enters = enters.max(inside / rate);
            }
        }
        (enters, leaves)
    }

    fn slot_of_face(&self, face: u32) -> Option<usize> {
        let brush = self
            .brushes
            .face_brushes
            .get(face as usize)
            .copied()
            .flatten()?;
        self.excluded.binary_search(&brush).ok()
    }
}

/// Whether a neighbour the ray spans over `(enters, leaves)` holds what lies
/// just past `exit` from the excluded `hull`: the ray leaves it after the
/// exit, and the first point at or past the exit where the ray is inside it
/// lies within [`CONTACT_TOLERANCE_M`] of `hull`. Measured off the hull's
/// planes, as CSG measures contact, so a gap it cut holds at any ray angle.
fn holds_past(
    hull: &BrushVolume,
    (enters, leaves): (f64, f64),
    exit: f64,
    (origin, direction): (DVec3, DVec3),
) -> bool {
    leaves > exit
        && enters < leaves
        && outside_by(hull, origin + direction * enters.max(exit)) <= CONTACT_TOLERANCE_M
}

/// How far `point` lies outside `hull`: its greatest distance past any of
/// the hull's planes, negative inside.
pub(super) fn outside_by(hull: &BrushVolume, point: DVec3) -> f64 {
    hull.planes
        .iter()
        .map(|plane| plane.normal.dot(point) - plane.distance)
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Bounds that overlap or touch within [`CONTACT_TOLERANCE_M`], so every
/// hull a contact test can reach is a candidate.
fn bounds_touch(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|axis| {
        a.min[axis] <= b.max[axis] + CONTACT_TOLERANCE_M
            && b.min[axis] <= a.max[axis] + CONTACT_TOLERANCE_M
    })
}

/// A probe's projection box as the leak guard tests it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProjectionBox {
    min: DVec3,
    max: DVec3,
}

impl ProjectionBox {
    pub(crate) fn of(probe: &PlacedProbe) -> Self {
        Self {
            min: Vec3::from(probe.box_min).as_dvec3(),
            max: Vec3::from(probe.box_max).as_dvec3(),
        }
    }

    /// Whether `point` lies in the box within [`PLANE_TOLERANCE_M`], so a
    /// wall or ceiling the box was fitted to holds on rounding. The floor
    /// along the reflector normal (up, for water) is ignored: a derived box
    /// sets it at the capture point's height, so a ray below the horizon
    /// takes the guard only on a miss.
    fn holds(&self, point: DVec3) -> bool {
        let tolerance = PLANE_TOLERANCE_M;
        point.x >= self.min.x - tolerance
            && point.x <= self.max.x + tolerance
            && point.z >= self.min.z - tolerance
            && point.z <= self.max.z + tolerance
            && point.y <= self.max.y + tolerance
    }

    /// Whether `aabb` touches the box under [`Self::holds`]'s rule: within
    /// [`PLANE_TOLERANCE_M`], its floor ignored. A hull that does not touch
    /// it holds no point the box holds.
    pub(crate) fn touches(&self, aabb: &Aabb) -> bool {
        let tolerance = PLANE_TOLERANCE_M;
        aabb.min.x <= self.max.x + tolerance
            && aabb.max.x >= self.min.x - tolerance
            && aabb.min.z <= self.max.z + tolerance
            && aabb.max.z >= self.min.z - tolerance
            && aabb.min.y <= self.max.y + tolerance
    }
}
