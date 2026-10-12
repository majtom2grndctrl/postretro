// The pure environment resolve: a body's position and capsule plus the volumes give gravity, push and fluid.
// See: context/lib/entity_model.md §7 (Environment Volumes)

use glam::Vec3;

use crate::leaf::ContentsMask;
use crate::line::{BrushInterval, clip_line, union_length};
use crate::query_timer::{EnvironmentQueryTimer, QueryFamily, timed};
use crate::volumes::EnvironmentVolumes;

static NO_VOLUMES: EnvironmentVolumes = EnvironmentVolumes::empty();

/// What a body resolves against on one step: the level default gravity, the
/// installed volumes, and the movement half of the fluid table their fluid
/// indices were resolved against. Built at each call site per tick from the
/// level gravity source, so a mid-level gravity change is seen on the next step.
#[derive(Debug, Clone, Copy)]
pub struct EnvironmentView<'a> {
    pub level_gravity: f32,
    pub volumes: &'a EnvironmentVolumes,
    /// Drag per fluid index, 1/s: index `i` is fluid `i` of the table the
    /// volumes were built against (the host's own registry, or on a connected
    /// client the host's table from the installed tuning). Movement reads the
    /// winner's entry through [`ResolvedEnvironment::viscosity`] and never
    /// queries a registry.
    pub fluid_viscosity: &'a [f32],
    /// CPU-time accumulator for stage timing; `None` unless the caller's
    /// timing gate is on. Never changes a query's result.
    pub timer: Option<&'a EnvironmentQueryTimer>,
}

impl<'a> EnvironmentView<'a> {
    pub const fn new(
        level_gravity: f32,
        volumes: &'a EnvironmentVolumes,
        fluid_viscosity: &'a [f32],
    ) -> Self {
        Self {
            level_gravity,
            volumes,
            fluid_viscosity,
            timer: None,
        }
    }

    /// This view, with its body resolves and fluid-crossing queries timed into
    /// `timer`. Attach only while CPU stage timing is on.
    pub fn timed<'t>(self, timer: &'t EnvironmentQueryTimer) -> EnvironmentView<'t>
    where
        'a: 't,
    {
        EnvironmentView {
            level_gravity: self.level_gravity,
            volumes: self.volumes,
            fluid_viscosity: self.fluid_viscosity,
            timer: Some(timer),
        }
    }
}

impl EnvironmentView<'static> {
    /// A view with no volumes and an empty fluid table: every body resolves the
    /// level default, dry.
    pub const fn level_only(level_gravity: f32) -> Self {
        Self {
            level_gravity,
            volumes: &NO_VOLUMES,
            fluid_viscosity: &[],
            timer: None,
        }
    }
}

/// The body being resolved. `position` is the capsule CENTER, the same point
/// movement integrates (`Transform.position` of a pawn or agent). The capsule's
/// vertical segment runs from `position.y - half_height - radius` (feet) to
/// `position.y + half_height + radius` (head); `half_height` excludes the end
/// caps. The tick eye sits `eye_height` above the center.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyProbe {
    pub position: Vec3,
    pub half_height: f32,
    pub radius: f32,
    pub eye_height: f32,
}

/// One body's environment for one step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedEnvironment {
    /// Signed vertical acceleration, m/s².
    pub gravity: f32,
    /// Extra acceleration in any direction, m/s². Never defines "down".
    pub push: Vec3,
    /// The winning fluid, if any: the overlap-rule winner's fluid among the
    /// fluid volumes in reach. `None` exactly when `immersion` is 0.
    pub fluid: Option<u16>,
    /// The winning fluid's drag while swimming, 1/s, from the view's fluid
    /// table; 0 when dry. A fluid index past the table also reads 0, which a
    /// production view never sees: it builds the volumes and the table together.
    pub viscosity: f32,
    /// Fraction of the capsule's vertical segment, feet to head, covered by the
    /// union of the brushes of every volume carrying the winning fluid, 0..=1.
    /// A pool split into several entities of one fluid counts whole; a
    /// different fluid never adds.
    pub immersion: f32,
    /// Whether the tick eye (`position.y + eye_height` at the probe's x/z) lies
    /// inside the union of the brushes of every volume carrying the winning
    /// fluid, a face counting as inside. Always false when dry. The fluid jump
    /// keys on this, never on the presented camera eye, so host and client
    /// agree on it (`movement.md` §4, Swim).
    pub tick_eye_in_fluid: bool,
}

/// Resolve `probe`'s environment. Gravity and push are point rules at
/// `probe.position`: for each field, the winner among volumes that set it and
/// contain the point (`EnvironmentVolumes::outranks`); with none, the level
/// default gravity and zero push. The fluid follows the capsule-segment rule
/// (`resolve_fluid`). A non-finite position resolves the level defaults, dry.
/// Deterministic and allocation-free: the overlap order is total and every
/// reduction is order-independent, so the result never depends on tree shape
/// or visit order.
pub fn resolve_environment(view: EnvironmentView<'_>, probe: &BodyProbe) -> ResolvedEnvironment {
    timed(view.timer, QueryFamily::BodyResolve, || {
        resolve_environment_untimed(view, probe)
    })
}

fn resolve_environment_untimed(
    view: EnvironmentView<'_>,
    probe: &BodyProbe,
) -> ResolvedEnvironment {
    if !probe.position.is_finite() {
        return level_defaults(view.level_gravity);
    }
    let volumes = view.volumes;
    let mut gravity_winner: Option<usize> = None;
    let mut push_winner: Option<usize> = None;
    volumes.visit_volumes_containing_point(
        probe.position,
        ContentsMask::GRAVITY | ContentsMask::PUSH,
        |index| {
            let Some(volume) = volumes.volume(index) else {
                return;
            };
            if volume.gravity.is_some() && beats(volumes, index, gravity_winner) {
                gravity_winner = Some(index);
            }
            if volume.push.is_some() && beats(volumes, index, push_winner) {
                push_winner = Some(index);
            }
        },
    );

    let gravity = gravity_winner
        .and_then(|index| volumes.volume(index))
        .and_then(|volume| volume.gravity)
        .unwrap_or(view.level_gravity);
    let push = push_winner
        .and_then(|index| volumes.volume(index))
        .and_then(|volume| volume.push)
        .unwrap_or(Vec3::ZERO);
    let fluid = resolve_fluid(view, probe);
    let viscosity = fluid
        .fluid
        .and_then(|index| view.fluid_viscosity.get(usize::from(index)))
        .copied()
        .unwrap_or(0.0);

    ResolvedEnvironment {
        gravity,
        push,
        fluid: fluid.fluid,
        viscosity,
        immersion: fluid.immersion,
        tick_eye_in_fluid: fluid.tick_eye_in_fluid,
    }
}

/// Gravity alone at `position`, by the same point rule as `resolve_environment`.
/// For bodies that read only gravity (AI agents), so they never pay for the push
/// reduction or the fluid's segment query. A non-finite position resolves the
/// level default.
pub fn resolve_gravity(view: EnvironmentView<'_>, position: Vec3) -> f32 {
    timed(view.timer, QueryFamily::BodyResolve, || {
        resolve_gravity_untimed(view, position)
    })
}

fn resolve_gravity_untimed(view: EnvironmentView<'_>, position: Vec3) -> f32 {
    if !position.is_finite() {
        return view.level_gravity;
    }
    let volumes = view.volumes;
    let mut winner: Option<usize> = None;
    volumes.visit_volumes_containing_point(position, ContentsMask::GRAVITY, |index| {
        if volumes.volume(index).is_some_and(|v| v.gravity.is_some())
            && beats(volumes, index, winner)
        {
            winner = Some(index);
        }
    });
    winner
        .and_then(|index| volumes.volume(index))
        .and_then(|volume| volume.gravity)
        .unwrap_or(view.level_gravity)
}

fn level_defaults(level_gravity: f32) -> ResolvedEnvironment {
    ResolvedEnvironment {
        gravity: level_gravity,
        push: Vec3::ZERO,
        fluid: None,
        viscosity: 0.0,
        immersion: 0.0,
        tick_eye_in_fluid: false,
    }
}

pub(crate) fn beats(
    volumes: &EnvironmentVolumes,
    candidate: usize,
    incumbent: Option<usize>,
) -> bool {
    match incumbent {
        None => true,
        Some(incumbent) => volumes.outranks(candidate, incumbent),
    }
}

struct FluidResolve {
    fluid: Option<u16>,
    immersion: f32,
    tick_eye_in_fluid: bool,
}

const DRY: FluidResolve = FluidResolve {
    fluid: None,
    immersion: 0.0,
    tick_eye_in_fluid: false,
};

/// Fluid brush intervals one resolve keeps inline, so the winner's union is
/// measured from the clips already made. A segment meeting more falls back to
/// re-walking the tree for them; both paths measure the same union.
const INLINE_FLUID_INTERVALS: usize = 8;

/// One fluid brush's positive-length interval along the capsule segment.
#[derive(Clone, Copy)]
struct FluidInterval {
    fluid: u16,
    interval: BrushInterval,
}

/// The fluid rule's one home. Fluid reach is the capsule's vertical segment at
/// the probe's x/z, feet (`y - half_height - radius`) to head
/// (`y + half_height + radius`). A fluid volume is in reach when its brushes
/// cover a positive length of the segment, so a segment that only touches a
/// face (feet exactly on the surface) stays dry. The fluid is the overlap-rule
/// winner's among volumes in reach. Immersion is the segment's coverage by the
/// union of the brushes of every volume carrying that fluid, so leaving through
/// any face, bottom included, drops it, overlapping brushes count once, a pool
/// split into stacked entities of one fluid reads whole, and a different fluid
/// never adds. A degenerate segment (zero or negative length) falls back to the
/// point rule at `position`: immersion 1 inside a fluid, 0 outside.
fn resolve_fluid(view: EnvironmentView<'_>, probe: &BodyProbe) -> FluidResolve {
    let volumes = view.volumes;
    if volumes.is_empty() || !probe.position.is_finite() {
        return DRY;
    }
    let reach = probe.half_height + probe.radius;
    let feet = Vec3::new(probe.position.x, probe.position.y - reach, probe.position.z);
    let length = 2.0 * reach;
    let eye = probe.position + Vec3::new(0.0, probe.eye_height, 0.0);

    if !(length > 0.0 && length.is_finite()) {
        return point_fluid(volumes, probe.position, eye);
    }

    // One pass clips every fluid brush on the segment once: it picks the
    // winner and keeps the intervals the union is measured from.
    let mut inline = [None::<FluidInterval>; INLINE_FLUID_INTERVALS];
    let mut kept = 0;
    let mut overflowed = false;
    let mut winner: Option<usize> = None;
    volumes.visit_brushes_on_line(
        feet,
        Vec3::Y,
        0.0,
        length,
        ContentsMask::FLUID,
        |index, brush, leaf| {
            let Some(fluid) = volumes.volume(index).and_then(|v| v.fluid) else {
                return;
            };
            let Some(interval) = clip_line(&leaf.planes, feet, Vec3::Y, 0.0, length)
                .filter(|interval| interval.has_length())
            else {
                return;
            };
            if kept < INLINE_FLUID_INTERVALS {
                inline[kept] = Some(FluidInterval {
                    fluid,
                    interval: BrushInterval { brush, interval },
                });
                kept += 1;
            } else {
                overflowed = true;
            }
            if beats(volumes, index, winner) {
                winner = Some(index);
            }
        },
    );
    let Some(fluid) = winner.and_then(|index| volumes.volume(index)?.fluid) else {
        return DRY;
    };
    let covered = if overflowed {
        union_length(|sink| {
            volumes.visit_brushes_on_line(
                feet,
                Vec3::Y,
                0.0,
                length,
                ContentsMask::FLUID,
                |index, brush, leaf| {
                    if volumes.volume(index).and_then(|v| v.fluid) != Some(fluid) {
                        return;
                    }
                    if let Some(interval) = clip_line(&leaf.planes, feet, Vec3::Y, 0.0, length) {
                        sink(BrushInterval { brush, interval });
                    }
                },
            )
        })
    } else {
        union_length(|sink| {
            inline[..kept]
                .iter()
                .flatten()
                .filter(|kept| kept.fluid == fluid)
                .for_each(|kept| sink(kept.interval))
        })
    };
    FluidResolve {
        fluid: Some(fluid),
        immersion: (covered / length).clamp(0.0, 1.0),
        tick_eye_in_fluid: volumes.fluid_contains_point(fluid, eye),
    }
}

/// The point rule for a body with no vertical extent.
fn point_fluid(volumes: &EnvironmentVolumes, point: Vec3, eye: Vec3) -> FluidResolve {
    let Some(fluid) =
        fluid_winner_at_point(volumes, point).and_then(|winner| volumes.volume(winner)?.fluid)
    else {
        return DRY;
    };
    FluidResolve {
        fluid: Some(fluid),
        immersion: 1.0,
        tick_eye_in_fluid: volumes.fluid_contains_point(fluid, eye),
    }
}

/// The overlap-rule winner among fluid volumes containing `point`.
pub(crate) fn fluid_winner_at_point(volumes: &EnvironmentVolumes, point: Vec3) -> Option<usize> {
    let mut winner: Option<usize> = None;
    volumes.visit_volumes_containing_point(point, ContentsMask::FLUID, |index| {
        if volumes.volume(index).is_some_and(|v| v.fluid.is_some()) && beats(volumes, index, winner)
        {
            winner = Some(index);
        }
    });
    winner
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{axis_box, volume};
    use crate::volumes::{EnvironmentVolume, EnvironmentVolumes};

    const LEVEL_GRAVITY: f32 = -9.81;
    const EPS: f32 = 1.0e-6;

    fn probe_at(position: Vec3) -> BodyProbe {
        BodyProbe {
            position,
            half_height: 0.8,
            radius: 0.4,
            eye_height: 0.5,
        }
    }

    fn gravity_volume(min: Vec3, max: Vec3, priority: i32, gravity: f32) -> EnvironmentVolume {
        EnvironmentVolume {
            gravity: Some(gravity),
            priority,
            ..volume(vec![axis_box(min, max)])
        }
    }

    fn resolve_at(volumes: &EnvironmentVolumes, point: Vec3) -> ResolvedEnvironment {
        resolve_environment(
            EnvironmentView::new(LEVEL_GRAVITY, volumes, &[]),
            &probe_at(point),
        )
    }

    #[test]
    fn resolve_with_no_volume_returns_level_defaults() {
        let resolved = resolve_environment(
            EnvironmentView::level_only(LEVEL_GRAVITY),
            &probe_at(Vec3::new(3.0, 1.0, -2.0)),
        );
        assert!((resolved.gravity - LEVEL_GRAVITY).abs() < EPS);
        assert_eq!(resolved.push, Vec3::ZERO);
        assert_eq!(resolved.fluid, None);
        assert!(resolved.immersion.abs() < EPS);

        // A built set whose volumes miss the point resolves defaults too.
        let volumes = EnvironmentVolumes::build(vec![gravity_volume(
            Vec3::splat(10.0),
            Vec3::splat(12.0),
            0,
            -2.0,
        )]);
        let resolved = resolve_at(&volumes, Vec3::ZERO);
        assert!((resolved.gravity - LEVEL_GRAVITY).abs() < EPS);
        assert_eq!(resolved.push, Vec3::ZERO);
    }

    #[test]
    fn resolve_gravity_applies_inside_on_face_and_not_outside() {
        let volumes =
            EnvironmentVolumes::build(vec![gravity_volume(Vec3::ZERO, Vec3::splat(4.0), 0, -2.0)]);
        assert!((resolve_at(&volumes, Vec3::splat(2.0)).gravity + 2.0).abs() < EPS);
        assert!(
            (resolve_at(&volumes, Vec3::new(4.0, 2.0, 2.0)).gravity + 2.0).abs() < EPS,
            "a body exactly on a face is inside"
        );
        assert!(
            (resolve_at(&volumes, Vec3::new(4.001, 2.0, 2.0)).gravity - LEVEL_GRAVITY).abs() < EPS,
            "just outside takes the level default"
        );
    }

    #[test]
    fn resolve_gravity_prefers_higher_priority_over_smaller_volume() {
        let volumes = EnvironmentVolumes::build(vec![
            gravity_volume(Vec3::splat(1.0), Vec3::splat(2.0), 0, -1.0),
            gravity_volume(Vec3::splat(-10.0), Vec3::splat(10.0), 5, -4.0),
        ]);
        let resolved = resolve_at(&volumes, Vec3::splat(1.5));
        assert!((resolved.gravity + 4.0).abs() < EPS);
    }

    #[test]
    fn resolve_gravity_prefers_smaller_volume_at_equal_priority_regardless_of_map_order() {
        let small = gravity_volume(Vec3::splat(1.0), Vec3::splat(2.0), 0, -1.0);
        let large = gravity_volume(Vec3::splat(-10.0), Vec3::splat(10.0), 0, -4.0);
        for volumes in [
            EnvironmentVolumes::build(vec![small.clone(), large.clone()]),
            EnvironmentVolumes::build(vec![large, small]),
        ] {
            let resolved = resolve_at(&volumes, Vec3::splat(1.5));
            assert!((resolved.gravity + 1.0).abs() < EPS);
        }
    }

    #[test]
    fn resolve_gravity_breaks_exact_tie_by_earlier_map_order() {
        let volumes = EnvironmentVolumes::build(vec![
            gravity_volume(Vec3::ZERO, Vec3::ONE, 0, -1.0),
            gravity_volume(Vec3::ZERO, Vec3::ONE, 0, -3.0),
        ]);
        let resolved = resolve_at(&volumes, Vec3::splat(0.5));
        assert!((resolved.gravity + 1.0).abs() < EPS);
    }

    // Regression: identical copies at different positions round their summed
    // volumes differently, so an exact comparison let float noise pick the later one.
    #[test]
    fn resolve_gravity_ties_identical_copies_at_different_positions_by_map_order() {
        const MAP_SCALE: f32 = 0.0254;
        let copy = |x_units: f32, gravity: f32| {
            gravity_volume(
                Vec3::new(x_units, 16.0, -48.0) * MAP_SCALE,
                Vec3::new(x_units + 64.0, 64.0, 48.0) * MAP_SCALE,
                0,
                gravity,
            )
        };
        let point = Vec3::new(52.0, 40.0, 0.0) * MAP_SCALE;
        let volumes = EnvironmentVolumes::build(vec![copy(0.0, -1.0), copy(40.0, -3.0)]);
        assert!(
            volumes.summed_brush_volume(1).unwrap() < volumes.summed_brush_volume(0).unwrap(),
            "fixture: the later copy rounds smaller"
        );
        assert!((resolve_at(&volumes, point).gravity + 1.0).abs() < EPS);
        let swapped = EnvironmentVolumes::build(vec![copy(40.0, -3.0), copy(0.0, -1.0)]);
        assert!((resolve_at(&swapped, point).gravity + 3.0).abs() < EPS);
    }

    #[test]
    fn resolve_gravity_ranks_by_summed_brush_volume_across_brushes() {
        // A two-brush entity: each brush (1 m³) is smaller than the single-brush
        // entity (1.5 m³), but its summed volume (2 m³) is larger, so it loses.
        let two_brushes = EnvironmentVolume {
            gravity: Some(-1.0),
            ..volume(vec![
                axis_box(Vec3::ZERO, Vec3::ONE),
                axis_box(Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 1.0)),
            ])
        };
        let single = gravity_volume(Vec3::ZERO, Vec3::new(1.5, 1.0, 1.0), 0, -5.0);
        let volumes = EnvironmentVolumes::build(vec![two_brushes, single]);
        assert!((volumes.summed_brush_volume(0).unwrap() - 2.0).abs() < 1.0e-4);
        let resolved = resolve_at(&volumes, Vec3::new(0.5, 0.5, 0.5));
        assert!((resolved.gravity + 5.0).abs() < EPS);
    }

    #[test]
    fn resolve_fields_take_independent_winners() {
        // A small push pocket inside a large low-gravity room: the point takes
        // the room's gravity and the pocket's push; a gravity-less volume never
        // wins gravity however small it is.
        let room = gravity_volume(Vec3::splat(-10.0), Vec3::splat(10.0), 0, -3.0);
        let pocket = EnvironmentVolume {
            push: Some(Vec3::new(0.0, 15.0, 0.0)),
            ..volume(vec![axis_box(Vec3::ZERO, Vec3::ONE)])
        };
        let volumes = EnvironmentVolumes::build(vec![room, pocket]);
        let resolved = resolve_at(&volumes, Vec3::splat(0.5));
        assert!((resolved.gravity + 3.0).abs() < EPS);
        assert!((resolved.push - Vec3::new(0.0, 15.0, 0.0)).length() < EPS);

        let outside_pocket = resolve_at(&volumes, Vec3::splat(5.0));
        assert!((outside_pocket.gravity + 3.0).abs() < EPS);
        assert_eq!(outside_pocket.push, Vec3::ZERO);
    }

    #[test]
    fn resolve_push_is_one_winner_never_a_sum() {
        let wind = EnvironmentVolume {
            push: Some(Vec3::new(8.0, 0.0, 0.0)),
            ..volume(vec![axis_box(Vec3::splat(-10.0), Vec3::splat(10.0))])
        };
        let calm = EnvironmentVolume {
            push: Some(Vec3::ZERO),
            ..volume(vec![axis_box(Vec3::ZERO, Vec3::ONE)])
        };
        let volumes = EnvironmentVolumes::build(vec![wind, calm]);
        assert_eq!(resolve_at(&volumes, Vec3::splat(0.5)).push, Vec3::ZERO);
        assert!(
            (resolve_at(&volumes, Vec3::splat(5.0)).push - Vec3::new(8.0, 0.0, 0.0)).length() < EPS
        );
    }
}
