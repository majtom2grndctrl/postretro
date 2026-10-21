// Reflection-probe capture exclusion: a probe's face mask, excluded-hull exit classification and the leak guard.
// See: context/lib/resource_management.md §5.2 (Exclusion)

use glam::{DVec3, Vec3};

use super::capture_fill::{Fill, KeptCandidate, find_fill};
use super::hit::{FaceMask, MaskedHit, RAY_EPSILON, SceneTracer, SurfaceHit};
use super::placement::{PLANE_TOLERANCE_M, PlacedProbe};
use crate::map_data::BrushVolume;
use crate::partition::Aabb;

/// A point within this of a hull's planes is inside it. It only absorbs the
/// rounding of a ray point computed on a coplanar plane (flush contact), so
/// it stays far below the 1 mm hit epsilon: an exit half a millimetre short
/// of a kept face lies in empty space and re-traces onto that face.
const HULL_INSIDE_TOLERANCE_M: f64 = 1.0e-5;

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
    /// Nothing within reach: distance 0.
    Miss,
    /// A kept front face, as traced, behind any excluded brush the ray crossed.
    Hit(SurfaceHit),
    /// The first excluded surface the ray skipped, read as if it were kept:
    /// the ray found nothing behind the excluded brushes it may show.
    LeakGuard(SurfaceHit),
    /// A kept brush's side, crossed inside an excluded hull, where CSG
    /// removed it against that brush.
    Fill(Fill),
}

impl RayOutcome {
    #[cfg(test)]
    /// The traced surface the ray reads, with its distance from the ray
    /// origin. A fill's surface is synthesized instead
    /// ([`super::capture_fill::fill_surface`]).
    pub(crate) fn surface(&self) -> Option<&SurfaceHit> {
        match self {
            Self::Miss | Self::Fill(_) => None,
            Self::Hit(hit) | Self::LeakGuard(hit) => Some(hit),
        }
    }

    #[cfg(test)]
    /// How far along the ray its surface lies; `None` on a miss.
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
    /// or a covering side lies on the hull, so any brush holding one is among
    /// these; one off the box could only place the ray outside it, where the
    /// leak guard reads the same surface whatever that brush is.
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
    /// Through a kept brush's covering side inside the hull.
    Fill(Fill),
    /// Into empty space at this distance: trace on from there.
    Exit(f64),
    /// Into kept solid, or past the crossing bound.
    LeakGuard,
}

impl<'a> ProbeExclusion<'a> {
    /// `excluded`: brush indices into `brushes.volumes`, sorted and
    /// deduplicated (the per-probe contract). Contacts are drawn per probe,
    /// so a brush another probe excludes is kept solid here when this one
    /// keeps it.
    pub(crate) fn new(
        tracer: &SceneTracer<'_>,
        brushes: CaptureBrushes<'a>,
        excluded: &[u32],
        probe: &PlacedProbe,
    ) -> Self {
        debug_assert!(
            excluded.windows(2).all(|pair| pair[0] < pair[1]),
            "excluded brushes are sorted and deduplicated"
        );
        let projection_box = ProjectionBox::of(probe);
        let excluded: Vec<u32> = excluded
            .iter()
            .copied()
            .filter(|&brush| (brush as usize) < brushes.volumes.len())
            .collect();
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
        self.brushes.faces.of(self.excluded[slot])
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
    /// exit under the same mask. Before its exit, a ray that crosses into a
    /// kept brush inside the hull, through a side with surviving faces,
    /// fills there. A ray that skipped any excluded brush and then misses,
    /// or ends (traced or filled) outside the projection box, takes the leak
    /// guard. Every distance is from `origin`.
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
        // them; a ray re-entering hulls past that takes the guard.
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
            let Some(slot) = self.slot_of_face(skipped.face) else {
                return RayOutcome::LeakGuard(first);
            };
            match self.cross(
                tracer,
                slot,
                origin64,
                direction64,
                f64::from(skipped.distance),
            ) {
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
    /// side crossed inside it fills, else its exit is classified, continuing
    /// through every excluded hull that exit lies in.
    fn cross(
        &self,
        tracer: &SceneTracer<'_>,
        mut slot: usize,
        origin: DVec3,
        direction: DVec3,
        entry: f64,
    ) -> Crossing {
        let mut reached = entry;
        for _ in 0..self.excluded.len() {
            let exit = self.hull_exit(slot, origin, direction).max(reached);
            if !exit.is_finite() {
                return Crossing::LeakGuard;
            }
            let contacts = &self.contacts[slot];
            if let Some(fill) = find_fill(
                tracer,
                self.brushes.volumes,
                &self.kept,
                &contacts.kept,
                (origin, direction),
                [reached, exit],
            ) {
                return Crossing::Fill(fill);
            }
            let point = origin + direction * exit;
            // Rule 1: a hull that holds the exit and reaches past it.
            if let Some(&next) = contacts.excluded.iter().find(|&&next| {
                self.holds(self.excluded[next], point)
                    && self.hull_exit(next, origin, direction) > exit
            }) {
                slot = next;
                reached = exit;
                continue;
            }
            // Rule 3: kept solid behind the exit.
            if contacts
                .kept
                .iter()
                .any(|&kept| self.holds(self.kept[kept].brush, point))
            {
                return Crossing::LeakGuard;
            }
            return Crossing::Exit(exit);
        }
        Crossing::LeakGuard
    }

    /// The least distance at which the ray leaves the excluded hull in
    /// `slot`: the hull is convex, so over the planes the ray heads out of.
    fn hull_exit(&self, slot: usize, origin: DVec3, direction: DVec3) -> f64 {
        let volume = &self.brushes.volumes[self.excluded[slot] as usize];
        volume
            .planes
            .iter()
            .filter_map(|plane| {
                let rate = plane.normal.dot(direction);
                (rate > 0.0).then(|| (plane.distance - plane.normal.dot(origin)) / rate)
            })
            .fold(f64::INFINITY, f64::min)
    }

    /// Whether `point` lies in brush `brush`'s hull, its boundary included.
    fn holds(&self, brush: u32, point: DVec3) -> bool {
        self.brushes.volumes[brush as usize]
            .planes
            .iter()
            .all(|plane| plane.normal.dot(point) - plane.distance <= HULL_INSIDE_TOLERANCE_M)
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

/// Bounds that overlap or touch within [`PLANE_TOLERANCE_M`].
fn bounds_touch(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|axis| {
        a.min[axis] <= b.max[axis] + PLANE_TOLERANCE_M
            && b.min[axis] <= a.max[axis] + PLANE_TOLERANCE_M
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
