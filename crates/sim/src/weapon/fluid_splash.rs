// Hitscan fluid splash facts: each pellet's fluid-face crossings before its blocking hit.
// See: context/lib/entity_model.md §7 (Environment Volumes: Hitscan splash facts)

use glam::Vec3;
use postretro_collision::{CROSSING_START_EPSILON, EnvironmentView, visit_fluid_crossings};

/// A crossing within this distance, in metres, of the pellet's blocking hit is
/// not a splash. A pool's bottom face buried in the floor coincides with the
/// floor hit, and the two are measured by different queries (brush planes
/// against the static trimesh), so they agree only to float noise; a buried
/// face never splashes. A real surface this close in front of a wall is not
/// worth a splash either. The same face-noise tolerance the crossing query
/// applies at the ray's start, here at its blocking end, so it is that constant.
pub(crate) const SPLASH_BLOCKING_EPSILON: f32 = CROSSING_START_EPSILON;

/// A hitscan pellet crossed a fluid volume's surface before its blocking hit.
/// A presentation fact: the engine reports where and which fluid; the look and
/// sound attach above it, as for impacts. Never replicated, never hashed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidSplashFact {
    pub point: Vec3,
    /// Unit outward normal of the crossed face: the face entered on the way
    /// in, the face left on the way out.
    pub normal: Vec3,
    /// The crossed volume's session fluid index.
    pub fluid: u16,
}

/// Append one pellet's splash facts to `out`, nearest first.
///
/// `blocking_toi` is the pellet's nearest world or entity hit, in units of
/// `direction`; `None` when nothing blocks, so crossings out to `range` count.
/// A crossing at the blocking hit, within [`SPLASH_BLOCKING_EPSILON`], never
/// splashes; one strictly before it does. Allocation-free beyond `out`'s growth.
pub(crate) fn collect_pellet_splashes(
    environment: EnvironmentView<'_>,
    origin: Vec3,
    direction: Vec3,
    blocking_toi: Option<f32>,
    range: f32,
    out: &mut Vec<FluidSplashFact>,
) {
    let (max_t, cutoff) = match blocking_toi {
        Some(toi) => (toi, toi - SPLASH_BLOCKING_EPSILON / direction.length()),
        None => (range, f32::INFINITY),
    };
    let first = out.len();
    visit_fluid_crossings(environment, origin, direction, max_t, |crossing| {
        if crossing.t < cutoff {
            out.push(FluidSplashFact {
                point: crossing.point,
                normal: crossing.outward_normal,
                fluid: crossing.fluid,
            });
        }
    });
    // The query orders crossings within one volume only. Sort the pellet's
    // facts by distance along the ray, with a total tiebreak so an exit and
    // an entry at one shared face land in the same order on every peer.
    // Unstable sort never allocates.
    let along = |fact: &FluidSplashFact| (fact.point - origin).dot(direction);
    out[first..].sort_unstable_by(|a, b| {
        along(a)
            .total_cmp(&along(b))
            .then(a.fluid.cmp(&b.fluid))
            .then(a.normal.x.total_cmp(&b.normal.x))
            .then(a.normal.y.total_cmp(&b.normal.y))
            .then(a.normal.z.total_cmp(&b.normal.z))
    });
}
