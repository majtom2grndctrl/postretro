// The pure environment resolve: a body's position and capsule plus the volumes give gravity, push and fluid.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md

use glam::Vec3;

use crate::leaf::ContentsMask;
use crate::volumes::EnvironmentVolumes;

static NO_VOLUMES: EnvironmentVolumes = EnvironmentVolumes::empty();

/// What a body resolves against on one step: the level default gravity and the
/// installed volumes. Built at each call site per tick from the level gravity
/// source, so a mid-level gravity change is seen on the next step.
#[derive(Debug, Clone, Copy)]
pub struct EnvironmentView<'a> {
    pub level_gravity: f32,
    pub volumes: &'a EnvironmentVolumes,
}

impl<'a> EnvironmentView<'a> {
    pub const fn new(level_gravity: f32, volumes: &'a EnvironmentVolumes) -> Self {
        Self {
            level_gravity,
            volumes,
        }
    }
}

impl EnvironmentView<'static> {
    /// A view with no volumes: every body resolves the level default.
    pub const fn level_only(level_gravity: f32) -> Self {
        Self {
            level_gravity,
            volumes: &NO_VOLUMES,
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
    /// The winning fluid, if any.
    pub fluid: Option<u16>,
    /// Coverage of the capsule's vertical segment by the winning fluid, 0..=1.
    pub immersion: f32,
}

/// Resolve `probe`'s environment. Gravity and push are point rules at
/// `probe.position`: for each field, the winner among volumes that set it and
/// contain the point (`EnvironmentVolumes::outranks`); with none, the level
/// default gravity and zero push. Deterministic and allocation-free: the result
/// never depends on visit order, and the only float math is the volumes' own
/// containment tests.
pub fn resolve_environment(view: EnvironmentView<'_>, probe: &BodyProbe) -> ResolvedEnvironment {
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
    let (fluid, immersion) = resolve_fluid(view, probe);

    ResolvedEnvironment {
        gravity,
        push,
        fluid,
        immersion,
    }
}

fn beats(volumes: &EnvironmentVolumes, candidate: usize, incumbent: Option<usize>) -> bool {
    match incumbent {
        None => true,
        Some(incumbent) => volumes.outranks(candidate, incumbent),
    }
}

/// The fluid rule's one home: fluid reach is the capsule's vertical segment, and
/// immersion is that segment's coverage by the winner's brushes. This slice
/// carries no capsule-segment query yet, so every body resolves dry.
fn resolve_fluid(_view: EnvironmentView<'_>, _probe: &BodyProbe) -> (Option<u16>, f32) {
    (None, 0.0)
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
            EnvironmentView::new(LEVEL_GRAVITY, volumes),
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
