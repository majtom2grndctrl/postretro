// Reflection-probe excluded-hull crossing: how a primary ray leaves, or passes beside, an excluded hull.
// See: context/lib/resource_management.md §5.2 (Exclusion: Exit, Tolerance hits)

use glam::DVec3;

use super::ProbeExclusion;
use crate::reflection_probes::capture_bounds::{Held, HeldDepth, held_at, nearest_held};
use crate::reflection_probes::capture_fill::{Fill, find_fill};
use crate::reflection_probes::hit::SceneTracer;

/// A ray parallel to a hull plane lies on its inner side within this. It
/// only absorbs the rounding of a ray running on the plane (flush contact),
/// so a level ray half a millimetre under a ceiling never enters it.
const PARALLEL_INSIDE_TOLERANCE_M: f64 = 1.0e-5;

/// A ray meeting a hull plane at a rate below this runs parallel to it:
/// over any capture distance it drifts off the plane by less than
/// [`PARALLEL_INSIDE_TOLERANCE_M`].
const PARALLEL_RATE: f64 = 1.0e-10;

/// How a ray passes an excluded hull it skipped.
pub(super) enum Crossing {
    /// Into the hull, leaving it as the exit reads.
    Through(Exit),
    /// Never into the hull: the skipped face took a tolerance hit, within τ
    /// of it in its plane, from a ray that passes beside the hull
    /// (`ray_traversal::TRACER_TOLERANCE_METERS`). It classifies like a
    /// crossing whose exit is that hit, so a ray through the contact gap
    /// between the hull and a kept brush reads what a ray just inside the
    /// hull would: CSG removed the kept faces in that gap, so passing on
    /// would go through the hole. `None` when it neither fills nor leaves
    /// into kept solid or another excluded hull: a ray that misses a convex
    /// hull misses all of it, so inside the projection box it traces on as
    /// if the brush were absent.
    Beside(Option<Exit>),
}

/// What a ray reads where it leaves an excluded hull, or passes beside one.
pub(super) enum Exit {
    /// Through a kept brush's covering side, inside the hull or in contact.
    Fill(Fill),
    /// Into empty space at this distance: trace on from there.
    Open(f64),
    /// Into kept solid, or past the crossing bound.
    LeakGuard,
}

impl ProbeExclusion<'_> {
    /// Resolve the excluded hull in `slot`, entered at `entry`: a covering
    /// side crossed inside it or in contact with it fills, else its exit is
    /// classified. A neighbour holds the exit when the first point at or
    /// past the exit where the ray is inside it lies within
    /// [`CONTACT_TOLERANCE_M`] of the hull's planes ([`held_at`]): CSG cut
    /// the facing faces across a gap that small, so no entry face would stop
    /// a re-trace. Of the neighbours that hold it, the one the ray is inside
    /// first decides, kept solid winning a tie ([`nearest_held`]): kept solid
    /// takes the leak guard, and an excluded hull is gone on through, its
    /// own exit classified in turn, and the nearest kept solid that also held
    /// the exit is carried in `held`. A level or gently rising ray leaving a
    /// pillar's side just under the ceiling enters the ceiling, if at all,
    /// far from the pillar, so it re-traces onto the intact ceiling. A ray
    /// that never enters the hull, its entry a tolerance hit, passes beside
    /// it and runs the same steps with that hit as its exit: through a
    /// contact gap, CSG removed the kept faces it would meet, so a kept
    /// brush it enters there is solid behind the gap.
    pub(super) fn cross(
        &self,
        tracer: &SceneTracer<'_>,
        mut slot: usize,
        origin: DVec3,
        direction: DVec3,
        entry: f64,
        held: &mut HeldDepth,
    ) -> Crossing {
        let volumes = self.brushes.volumes;
        let mut reached = entry;
        let (enters, mut leaves) = self.span(self.excluded[slot], origin, direction);
        // The ray misses the hull: its hit on the face was a tolerance hit,
        // and the exit is classified there.
        let beside = enters > leaves;
        if beside {
            leaves = entry;
        }
        // Whether the ray has entered an excluded hull. A pass beside this
        // one enters one only by going on through an excluded neighbour it
        // is inside at or past the hit, within contact of this hull.
        let mut entered = !beside;
        let passed = |exit: Exit| {
            if beside {
                Crossing::Beside(Some(exit))
            } else {
                Crossing::Through(exit)
            }
        };
        for _ in 0..self.excluded.len() {
            let exit = leaves.max(reached);
            if !exit.is_finite() {
                return passed(Exit::LeakGuard);
            }
            let hull = &volumes[self.excluded[slot] as usize];
            let contacts = &self.contacts[slot];
            if let Some(fill) = self.fill(tracer, slot, (origin, direction), reached) {
                return passed(Exit::Fill(fill));
            }
            // Where the ray is first inside `brush` at or past the exit, and
            // where it leaves it, when it holds the exit.
            let holds = |brush: u32| {
                let span = self.span(brush, origin, direction);
                held_at(hull, span, exit, (origin, direction)).map(|at| (at, span.1))
            };
            // The nearest kept depth decides the tie rule and, passed over,
            // is the one carried.
            let kept = contacts
                .kept
                .iter()
                .filter_map(|&kept| holds(self.kept[kept].brush))
                .map(|(at, _)| at)
                .min_by(f64::total_cmp);
            let excluded = contacts.excluded.iter().filter_map(|&next| {
                let (at, next_leaves) = holds(self.excluded[next])?;
                Some((at, next, next_leaves))
            });
            match nearest_held(kept, excluded) {
                Some(Held::Kept) => return passed(Exit::LeakGuard),
                // Going on moves the exit strictly on: the next hull holds
                // the exit only when the ray leaves it past that exit.
                Some(Held::Excluded {
                    slot: next,
                    leaves: next_leaves,
                }) => {
                    if let Some(depth) = kept {
                        held.pass_over(depth);
                    }
                    slot = next;
                    reached = exit;
                    leaves = next_leaves;
                    entered = true;
                }
                None => {
                    return if entered {
                        passed(Exit::Open(exit))
                    } else {
                        Crossing::Beside(None)
                    };
                }
            }
        }
        passed(Exit::LeakGuard)
    }

    /// The covering side of a kept contact of the excluded hull in `slot`
    /// the ray first crosses, from `reached` on ([`find_fill`]).
    pub(super) fn fill(
        &self,
        tracer: &SceneTracer<'_>,
        slot: usize,
        ray: (DVec3, DVec3),
        reached: f64,
    ) -> Option<Fill> {
        let volumes = self.brushes.volumes;
        find_fill(
            tracer,
            volumes,
            &self.kept,
            &self.contacts[slot].kept,
            ray,
            (&volumes[self.excluded[slot] as usize], reached),
        )
    }

    /// Where the ray enters and leaves brush `brush`'s convex hull: the
    /// latest plane it heads into and the earliest it heads out of. A plane
    /// the ray runs parallel to ([`PARALLEL_RATE`]), outside it, leaves no
    /// entry (`+∞`).
    pub(super) fn span(&self, brush: u32, origin: DVec3, direction: DVec3) -> (f64, f64) {
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
}
