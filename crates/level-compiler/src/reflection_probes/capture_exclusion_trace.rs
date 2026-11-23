// Reflection-probe capture exclusion's primary-ray trace: a ray's read through a probe's excluded hulls, fills and leak guard.
// See: context/lib/resource_management.md §5.2 (Exclusion)

use glam::Vec3;

use super::crossing::{Crossing, Exit};
use super::{ProbeExclusion, RayOutcome};
use crate::reflection_probes::capture_bounds::HeldDepth;
use crate::reflection_probes::capture_fill::Fill;
use crate::reflection_probes::hit::{FaceMask, MaskedHit, RAY_EPSILON, SceneTracer, SurfaceHit};

impl ProbeExclusion<'_> {
    /// The surface a primary ray from `origin` along unit `direction` reads.
    /// Transparent faces are absent throughout. With nothing excluded this
    /// is the plain closest hit. Otherwise each excluded brush the ray skips
    /// is crossed in turn, its hull exit read by the neighbour the ray is
    /// inside first past it: another excluded brush
    /// continues from that brush's exit, and kept solid takes the leak guard.
    /// Into empty space it traces on from the exit under the same mask.
    /// Before its exit is classified, a ray that crosses into a kept brush
    /// inside the hull or in contact with it, through a side with surviving
    /// faces, fills there. A ray that skipped any excluded brush and then
    /// misses, or ends (traced or filled) outside the projection box, takes
    /// the leak guard. A tolerance hit on an excluded face whose hull the ray
    /// never enters passes beside it. Outside the box that takes the leak
    /// guard, as a crossing there would. Inside it, the pass classifies like
    /// a crossing whose exit is the hit: it fills, takes the leak guard into
    /// kept solid, or continues through another excluded hull. Only when it
    /// does none of these does the trace drop that brush's faces and
    /// re-trace from the same bound, so another brush's face at the same
    /// distance still counts; that pass neither counts as skipping the brush
    /// nor spends a hull pass. Kept solid that held an exit the ray passed
    /// over for a nearer excluded neighbour bounds every read after it: a
    /// kept hit, fill or exit into open space past its depth takes the leak
    /// guard ([`HeldDepth`]). Every distance is from `origin`.
    pub(crate) fn trace(
        &self,
        tracer: &SceneTracer<'_>,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> RayOutcome {
        let mut hull_passes = 0;
        self.trace_counting(tracer, origin, direction, max_distance, &mut hull_passes)
    }

    /// [`Self::trace`], leaving in `hull_passes` the hull passes the ray
    /// spent, so a test can read the budget.
    pub(in crate::reflection_probes) fn trace_counting(
        &self,
        tracer: &SceneTracer<'_>,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        hull_passes: &mut usize,
    ) -> RayOutcome {
        *hull_passes = 0;
        if self.is_empty() {
            let hit = if self.transparent.is_empty() {
                tracer.closest_hit(origin, direction, max_distance)
            } else {
                let none = FaceMask::default();
                tracer
                    .closest_hit_masked(
                        origin,
                        direction,
                        RAY_EPSILON,
                        max_distance,
                        &none,
                        &self.transparent,
                    )
                    .hit
            };
            return hit.map_or(RayOutcome::Miss, RayOutcome::Hit);
        }
        let (origin64, direction64) = (origin.as_dvec3(), direction.as_dvec3());
        let brush_faces = self.brushes.faces;
        let mut min_distance = RAY_EPSILON;
        let mut first_skipped: Option<SurfaceHit> = None;
        // The least depth of kept solid passed over at any exit so far.
        let mut held = HeldDepth::NONE;
        // The excluded brushes the ray passes beside, and their faces, which
        // the trace ignores from then on beside the transparent ones.
        let mut beside: Vec<u32> = Vec::new();
        let mut ignored = self.transparent.clone();
        // Each hull pass moves past one excluded hull, so the excluded set
        // bounds them; a ray re-entering hulls past that takes the guard, as
        // does a grazing ray that spends its passes re-entering the same
        // hull. A pass beside a hull that goes on through another excluded
        // hull spends one, as a crossing does. One that drops the brush
        // spends none and keeps `min_distance`, yet the loop still ends: it
        // adds the brush to `beside`, whose faces are ignored from then on,
        // so no brush is dropped twice and the excluded set bounds those
        // passes too.
        loop {
            if *hull_passes > self.excluded.len() {
                return first_skipped.map_or(RayOutcome::Miss, RayOutcome::LeakGuard);
            }
            let MaskedHit { hit, skipped } = tracer.closest_hit_masked(
                origin,
                direction,
                min_distance,
                max_distance,
                &self.mask,
                &ignored,
            );
            let Some(skipped) = skipped else {
                return self.settle(hit, first_skipped, held, origin, direction);
            };
            debug_assert!(
                skipped.distance > min_distance,
                "a skipped hit lies past the bound"
            );
            // Recorded only on a crossing: a pass beside the hull that drops
            // the brush leaves it unset.
            let first = first_skipped.unwrap_or(skipped);
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
                &mut held,
            ) {
                // Outside the box the ray can only end outside it too, as
                // the box is convex and holds the capture point: the leak
                // guard's case. It reads `first`, the surface the kept brush
                // shows there, so a brush off the box captures the same kept
                // or excluded.
                Crossing::Beside(_)
                    if !self
                        .projection_box
                        .holds(origin64 + direction64 * f64::from(skipped.distance)) =>
                {
                    return RayOutcome::LeakGuard(first);
                }
                // No hull entered, no fill and nothing solid past the hit:
                // drop the brush and trace again from the same bound.
                // Stepping past the tolerance hit's distance instead would
                // step over another brush's real entry at that same `f32`
                // distance.
                Crossing::Beside(None) => {
                    let brush = self.excluded[slot];
                    debug_assert!(
                        !beside.contains(&brush),
                        "an ignored brush's faces are never skipped"
                    );
                    beside.push(brush);
                    let passed = beside
                        .iter()
                        .flat_map(|&b| brush_faces.of(b).iter().copied());
                    ignored = tracer.face_mask(passed).union(&self.transparent);
                }
                // A pass beside the hull reads as a crossing would.
                Crossing::Through(exit) | Crossing::Beside(Some(exit)) => match exit {
                    // A kept side CSG removed against the hull, crossed
                    // inside it or, passing beside it, through their contact
                    // gap.
                    Exit::Fill(fill) => {
                        return self.filled(fill, hit, first, held, origin, direction);
                    }
                    Exit::LeakGuard => return RayOutcome::LeakGuard(first),
                    // Into open space inside kept solid passed over: the
                    // re-trace would read through it.
                    Exit::Open(exit) if !self.admits(held, exit) => {
                        return RayOutcome::LeakGuard(first);
                    }
                    // Never at or before the skipped face itself, so each
                    // pass advances even when f32 rounds the exit below it.
                    Exit::Open(exit) => {
                        first_skipped = Some(first);
                        min_distance = (exit as f32).max(skipped.distance);
                        *hull_passes += 1;
                    }
                },
            }
        }
    }

    /// What a ray reads at `fill`, `hit` its kept front-face hit and
    /// `first` the first excluded surface it skipped: a kept face nearer
    /// than the fill hides it, and a fill past the `held` depth or outside
    /// the projection box takes the leak guard.
    pub(in crate::reflection_probes) fn filled(
        &self,
        fill: Fill,
        hit: Option<SurfaceHit>,
        first: SurfaceHit,
        held: HeldDepth,
        origin: Vec3,
        direction: Vec3,
    ) -> RayOutcome {
        if hit.is_some_and(|hit| hit.distance < fill.distance) {
            return self.settle(hit, Some(first), held, origin, direction);
        }
        let point = origin.as_dvec3() + direction.as_dvec3() * f64::from(fill.distance);
        if self.admits(held, f64::from(fill.distance)) && self.projection_box.holds(point) {
            RayOutcome::Fill(fill)
        } else {
            RayOutcome::LeakGuard(first)
        }
    }

    /// The ray's resolved hit, checked against the leak guard's miss, box
    /// and `held` depth rules once it has skipped an excluded brush.
    pub(in crate::reflection_probes) fn settle(
        &self,
        hit: Option<SurfaceHit>,
        first_skipped: Option<SurfaceHit>,
        held: HeldDepth,
        origin: Vec3,
        direction: Vec3,
    ) -> RayOutcome {
        let Some(first) = first_skipped else {
            return hit.map_or(RayOutcome::Miss, RayOutcome::Hit);
        };
        match hit {
            Some(hit)
                if self.admits(held, f64::from(hit.distance))
                    && self.projection_box.holds(
                        origin.as_dvec3() + direction.as_dvec3() * f64::from(hit.distance),
                    ) =>
            {
                RayOutcome::Hit(hit)
            }
            _ => RayOutcome::LeakGuard(first),
        }
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
