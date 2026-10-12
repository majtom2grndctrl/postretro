// Point containment, contents masks, the overlap rule across fields, and the capsule-segment fluid rule.
// See: context/lib/entity_model.md §7 (Environment Volumes)

use glam::Vec3;

use crate::leaf::{ContentsMask, LeafPlane};
use crate::resolve::{
    BodyProbe, EnvironmentView, ResolvedEnvironment, resolve_environment, resolve_gravity,
};
use crate::test_fixtures::{axis_box, volume};
use crate::volumes::{EnvironmentVolume, EnvironmentVolumes};

const LEVEL_GRAVITY: f32 = -9.81;
const EPS: f32 = 1.0e-5;

/// Half height 0.5 plus radius 0.5: the vertical segment runs 1 m below and 1 m
/// above the body point, 2 m in all. The tick eye sits 0.6 m above it.
fn probe_at(position: Vec3) -> BodyProbe {
    BodyProbe {
        position,
        half_height: 0.5,
        radius: 0.5,
        eye_height: 0.6,
    }
}

fn fluid_box(min: Vec3, max: Vec3, fluid: u16, priority: i32) -> EnvironmentVolume {
    EnvironmentVolume {
        fluid: Some(fluid),
        priority,
        ..volume(vec![axis_box(min, max)])
    }
}

/// A 10 x 10 pool spanning `y_min..y_max`.
fn pool(y_min: f32, y_max: f32, fluid: u16) -> EnvironmentVolume {
    fluid_box(
        Vec3::new(-5.0, y_min, -5.0),
        Vec3::new(5.0, y_max, 5.0),
        fluid,
        0,
    )
}

fn resolve(volumes: &EnvironmentVolumes, position: Vec3) -> ResolvedEnvironment {
    resolve_environment(
        EnvironmentView::new(LEVEL_GRAVITY, volumes, &[]),
        &probe_at(position),
    )
}

fn containing(volumes: &EnvironmentVolumes, point: Vec3, mask: ContentsMask) -> Vec<usize> {
    let mut hits = Vec::new();
    volumes.visit_volumes_containing_point(point, mask, |index| hits.push(index));
    hits.sort_unstable();
    hits.dedup();
    hits
}

#[test]
fn volume_point_query_reports_inside_on_face_and_outside_a_tilted_convex_volume() {
    // The unit cube cut by x + y <= 1 (an unnormalized normal): a wedge.
    let mut wedge = axis_box(Vec3::ZERO, Vec3::ONE);
    wedge.planes.push(LeafPlane {
        normal: Vec3::new(1.0, 1.0, 0.0),
        distance: 1.0,
    });
    let volumes = EnvironmentVolumes::build(vec![EnvironmentVolume {
        gravity: Some(-2.0),
        ..volume(vec![wedge])
    }]);
    let all = ContentsMask::GRAVITY | ContentsMask::FLUID | ContentsMask::PUSH;
    assert_eq!(
        containing(&volumes, Vec3::new(0.25, 0.25, 0.5), all),
        vec![0]
    );
    assert_eq!(
        containing(&volumes, Vec3::new(0.5, 0.5, 0.5), all),
        vec![0],
        "exactly on the tilted face is inside"
    );
    assert_eq!(
        containing(&volumes, Vec3::new(1.0, 0.0, 0.5), all),
        vec![0],
        "exactly on an edge is inside"
    );
    assert!(
        containing(&volumes, Vec3::new(0.5, 0.5001, 0.5), all).is_empty(),
        "just outside the tilted face is outside"
    );
}

#[test]
fn fluid_volume_is_reported_under_a_fluid_mask_and_never_under_a_gravity_only_mask() {
    let volumes = EnvironmentVolumes::build(vec![pool(0.0, 4.0, 2)]);
    let point = Vec3::new(0.0, 2.0, 0.0);
    assert!(containing(&volumes, point, ContentsMask::GRAVITY).is_empty());
    assert_eq!(containing(&volumes, point, ContentsMask::FLUID), vec![0]);

    // The resolve's gravity-only entry ignores the fluid; the full resolve sees it.
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    assert!((resolve_gravity(view, point) - LEVEL_GRAVITY).abs() < EPS);
    assert_eq!(resolve(&volumes, point).fluid, Some(2));
}

#[test]
fn pool_inside_lower_priority_low_gravity_room_resolves_pool_fluid_and_room_gravity() {
    let room = EnvironmentVolume {
        gravity: Some(-3.0),
        priority: -1,
        ..volume(vec![axis_box(Vec3::splat(-20.0), Vec3::splat(20.0))])
    };
    let volumes = EnvironmentVolumes::build(vec![room, pool(0.0, 4.0, 1)]);
    let resolved = resolve(&volumes, Vec3::new(0.0, 2.0, 0.0));
    assert_eq!(resolved.fluid, Some(1));
    assert!((resolved.gravity + 3.0).abs() < EPS);
    assert!((resolved.immersion - 1.0).abs() < EPS);
    assert_eq!(resolved.push, Vec3::ZERO);
}

#[test]
fn resolve_gravity_matches_the_full_resolve_gravity_at_every_point() {
    let volumes = EnvironmentVolumes::build(vec![
        EnvironmentVolume {
            gravity: Some(-3.0),
            ..volume(vec![axis_box(Vec3::splat(-10.0), Vec3::splat(10.0))])
        },
        EnvironmentVolume {
            gravity: Some(4.0),
            priority: 2,
            ..volume(vec![axis_box(Vec3::splat(5.0), Vec3::splat(15.0))])
        },
        pool(0.0, 2.0, 0),
    ]);
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    for point in [
        Vec3::ZERO,
        Vec3::splat(7.0),
        Vec3::splat(12.0),
        Vec3::splat(30.0),
        Vec3::new(0.0, 1.0, 0.0),
    ] {
        let full = resolve(&volumes, point).gravity;
        assert_eq!(resolve_gravity(view, point), full, "at {point:?}");
    }
}

#[test]
fn immersion_is_zero_with_feet_at_fluid_top_one_with_head_below_and_proportional_between() {
    let volumes = EnvironmentVolumes::build(vec![pool(0.0, 4.0, 5)]);

    let feet_on_top = resolve(&volumes, Vec3::new(0.0, 5.0, 0.0));
    assert!(feet_on_top.immersion.abs() < EPS);
    assert_eq!(
        feet_on_top.fluid, None,
        "a segment only touching the surface is dry"
    );

    let submerged = resolve(&volumes, Vec3::new(0.0, 2.5, 0.0));
    assert!((submerged.immersion - 1.0).abs() < EPS);
    assert_eq!(submerged.fluid, Some(5));

    let head_on_top = resolve(&volumes, Vec3::new(0.0, 3.0, 0.0));
    assert!(
        (head_on_top.immersion - 1.0).abs() < EPS,
        "head exactly at the surface"
    );

    for (y, expected) in [(4.0, 0.5), (3.5, 0.75), (4.5, 0.25), (4.9, 0.05)] {
        let resolved = resolve(&volumes, Vec3::new(0.0, y, 0.0));
        assert!(
            (resolved.immersion - expected).abs() < EPS,
            "body at y={y}: immersion {} expected {expected}",
            resolved.immersion
        );
        assert_eq!(resolved.fluid, Some(5));
    }
}

#[test]
fn floating_fluid_block_counts_only_the_contained_fraction_when_the_capsule_pokes_out_the_bottom() {
    // A block floating in mid-air, y 2..5. The capsule (1.5..3.5) hangs out of
    // its bottom face: only 2..3.5 counts.
    let volumes = EnvironmentVolumes::build(vec![pool(2.0, 5.0, 0)]);
    let resolved = resolve(&volumes, Vec3::new(0.0, 2.5, 0.0));
    assert_eq!(resolved.fluid, Some(0));
    assert!((resolved.immersion - 0.75).abs() < EPS);

    // Fully below the block: dry, the bottom face drops it like any other.
    let below = resolve(&volumes, Vec3::new(0.0, 0.5, 0.0));
    assert_eq!(below.fluid, None);
    assert!(below.immersion.abs() < EPS);
}

#[test]
fn wading_capsule_with_body_point_above_a_shallow_pool_resolves_the_pool_fluid() {
    let volumes = EnvironmentVolumes::build(vec![pool(0.0, 0.3, 4)]);
    let body = Vec3::new(0.0, 1.0, 0.0);
    assert!(
        containing(&volumes, body, ContentsMask::FLUID).is_empty(),
        "fixture: the body point itself is above the water"
    );
    let resolved = resolve(&volumes, body);
    assert_eq!(resolved.fluid, Some(4));
    assert!((resolved.immersion - 0.15).abs() < EPS);
    assert!(!resolved.tick_eye_in_fluid);
}

#[test]
fn capsule_spanning_two_fluids_resolves_the_overlap_winner_and_counts_only_its_brushes() {
    // Equal priority: the smaller volume (fluid 1, y 0..1) wins over the larger
    // one (fluid 2, y 0..3) wherever the segment meets both.
    let volumes = EnvironmentVolumes::build(vec![pool(0.0, 3.0, 2), pool(0.0, 1.0, 1)]);
    let resolved = resolve(&volumes, Vec3::new(0.0, 1.0, 0.0));
    assert_eq!(resolved.fluid, Some(1));
    assert!(
        (resolved.immersion - 0.5).abs() < EPS,
        "only the winner's 0..1 counts, not the loser's 0..2"
    );

    // Priority beats size.
    let volumes = EnvironmentVolumes::build(vec![
        fluid_box(Vec3::splat(-5.0), Vec3::new(5.0, 3.0, 5.0), 2, 1),
        pool(0.0, 1.0, 1),
    ]);
    let resolved = resolve(&volumes, Vec3::new(0.0, 1.0, 0.0));
    assert_eq!(resolved.fluid, Some(2));
    assert!((resolved.immersion - 1.0).abs() < EPS);
}

#[test]
fn multi_brush_fluid_counts_overlapping_brushes_once() {
    // Two brushes of one fluid overlapping on y 1..2: their union is 0..3.
    let fluid = EnvironmentVolume {
        fluid: Some(0),
        ..volume(vec![
            axis_box(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 2.0, 5.0)),
            axis_box(Vec3::new(-5.0, 1.0, -5.0), Vec3::new(5.0, 3.0, 5.0)),
        ])
    };
    let volumes = EnvironmentVolumes::build(vec![fluid]);
    let resolved = resolve(&volumes, Vec3::new(0.0, 3.0, 0.0));
    assert!(
        (resolved.immersion - 0.5).abs() < EPS,
        "segment 2..4 covered on 2..3"
    );
    let resolved = resolve(&volumes, Vec3::new(0.0, 1.5, 0.0));
    assert!((resolved.immersion - 1.0).abs() < EPS);
}

#[test]
fn capsule_whose_side_overlaps_a_fluid_while_its_vertical_segment_misses_resolves_dry() {
    let volumes = EnvironmentVolumes::build(vec![fluid_box(
        Vec3::new(0.0, 0.0, -5.0),
        Vec3::new(5.0, 4.0, 5.0),
        0,
        0,
    )]);
    // Radius 0.5 at x = -0.3: the capsule's side reaches x = 0.2, inside the
    // fluid, but its segment at x = -0.3 never enters it.
    let resolved = resolve(&volumes, Vec3::new(-0.3, 2.0, 0.0));
    assert_eq!(resolved.fluid, None);
    assert!(resolved.immersion.abs() < EPS);
    assert!(!resolved.tick_eye_in_fluid);
}

#[test]
fn tick_eye_in_fluid_tracks_the_tick_eye_against_the_winner_only() {
    let volumes = EnvironmentVolumes::build(vec![pool(0.0, 2.0, 0)]);
    // Body at 1.5: segment 0.5..2.5 is mostly wet, but the eye at 2.1 is out.
    let eye_out = resolve(&volumes, Vec3::new(0.0, 1.5, 0.0));
    assert_eq!(eye_out.fluid, Some(0));
    assert!((eye_out.immersion - 0.75).abs() < EPS);
    assert!(!eye_out.tick_eye_in_fluid);
    // Body at 1.3: the eye at 1.9 is under.
    assert!(resolve(&volumes, Vec3::new(0.0, 1.3, 0.0)).tick_eye_in_fluid);
    // An eye exactly on the surface counts as in.
    let eye_on_surface = BodyProbe {
        eye_height: 0.5,
        ..probe_at(Vec3::new(0.0, 1.5, 0.0))
    };
    assert!(
        resolve_environment(
            EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]),
            &eye_on_surface
        )
        .tick_eye_in_fluid
    );

    // The eye is tested against the winner's brushes, not any fluid: a
    // higher-priority shallow fluid wins while the eye sits only in the deep one.
    let volumes = EnvironmentVolumes::build(vec![
        pool(0.0, 10.0, 1),
        fluid_box(Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 1.8, 5.0), 2, 5),
    ]);
    let resolved = resolve(&volumes, Vec3::new(0.0, 1.3, 0.0));
    assert_eq!(resolved.fluid, Some(2));
    assert!(!resolved.tick_eye_in_fluid);
}

#[test]
fn body_with_no_vertical_extent_resolves_fluid_by_its_point() {
    let volumes = EnvironmentVolumes::build(vec![pool(0.0, 2.0, 3)]);
    let point_probe = |y: f32| BodyProbe {
        position: Vec3::new(0.0, y, 0.0),
        half_height: 0.0,
        radius: 0.0,
        eye_height: 0.0,
    };
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let inside = resolve_environment(view, &point_probe(1.0));
    assert_eq!(inside.fluid, Some(3));
    assert!((inside.immersion - 1.0).abs() < EPS);
    assert!(inside.tick_eye_in_fluid);
    let outside = resolve_environment(view, &point_probe(2.5));
    assert_eq!(outside.fluid, None);
    assert!(outside.immersion.abs() < EPS);
}

// Movement reads drag from the resolve, never from a registry: the winner's
// entry in the view's fluid table while wet, 0 while dry or with no table.
#[test]
fn resolved_viscosity_is_the_winning_fluid_entry_of_the_view_table() {
    // Fluid 0 is a large low-priority lake; fluid 1 a pool inside it that wins.
    let volumes = EnvironmentVolumes::build(vec![
        fluid_box(Vec3::splat(-20.0), Vec3::splat(20.0), 0, 0),
        fluid_box(
            Vec3::new(-2.0, -20.0, -2.0),
            Vec3::new(2.0, 20.0, 2.0),
            1,
            3,
        ),
    ]);
    let table = [2.0, 6.5];
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &table);

    let in_pool = resolve_environment(view, &probe_at(Vec3::ZERO));
    assert_eq!((in_pool.fluid, in_pool.viscosity), (Some(1), 6.5));
    let in_lake = resolve_environment(view, &probe_at(Vec3::new(10.0, 0.0, 0.0)));
    assert_eq!((in_lake.fluid, in_lake.viscosity), (Some(0), 2.0));
    let dry = resolve_environment(view, &probe_at(Vec3::new(30.0, 0.0, 0.0)));
    assert_eq!((dry.fluid, dry.viscosity), (None, 0.0));

    // The same volumes under an empty table, or a level-only view, carry no drag.
    let untabled = resolve(&volumes, Vec3::ZERO);
    assert_eq!((untabled.fluid, untabled.viscosity), (Some(1), 0.0));
    let level_only = resolve_environment(
        EnvironmentView::level_only(LEVEL_GRAVITY),
        &probe_at(Vec3::ZERO),
    );
    assert_eq!((level_only.fluid, level_only.viscosity), (None, 0.0));
}

// A pool split into two stacked entities of one fluid reads as one pool,
// whichever entity wins the overlap rule.
#[test]
fn pool_split_into_two_stacked_entities_of_one_fluid_reads_full_immersion_and_eye_in() {
    // The upper layer is smaller, so it wins wherever the segment meets both.
    let volumes = EnvironmentVolumes::build(vec![pool(0.0, 2.0, 0), pool(2.0, 3.0, 0)]);

    // Segment 0.5..2.5 straddles the seam: the winner's 2..2.5 alone would read 0.25.
    let straddling = resolve(&volumes, Vec3::new(0.0, 1.5, 0.0));
    assert_eq!(straddling.fluid, Some(0));
    assert!(
        (straddling.immersion - 1.0).abs() < EPS,
        "immersion {}",
        straddling.immersion
    );
    assert!(
        straddling.tick_eye_in_fluid,
        "the eye at 2.1 is in the upper layer"
    );

    // The eye at 1.9 sits in the lower, losing layer: still in the fluid.
    let eye_in_loser = resolve(&volumes, Vec3::new(0.0, 1.3, 0.0));
    assert!((eye_in_loser.immersion - 1.0).abs() < EPS);
    assert!(eye_in_loser.tick_eye_in_fluid);

    // Head out of the top of the stack: segment 1.5..3.5 is covered on 1.5..3.
    let surfacing = resolve(&volumes, Vec3::new(0.0, 2.5, 0.0));
    assert!((surfacing.immersion - 0.75).abs() < EPS);
    assert!(!surfacing.tick_eye_in_fluid);
}

// A narrow stream volume of water dipping into a water pool adds to it; a
// stream of another fluid wins the fluid and counts alone.
#[test]
fn stream_of_one_fluid_dipping_into_a_pool_of_the_same_fluid_adds_up() {
    let stream = |fluid: u16| {
        fluid_box(
            Vec3::new(-0.5, 1.5, -0.5),
            Vec3::new(0.5, 4.0, 0.5),
            fluid,
            0,
        )
    };
    // Body at 2: segment 1..3, the pool covers 1..2 and the stream 1.5..3.
    let water = EnvironmentVolumes::build(vec![pool(0.0, 2.0, 0), stream(0)]);
    let resolved = resolve(&water, Vec3::new(0.0, 2.0, 0.0));
    assert_eq!(resolved.fluid, Some(0));
    assert!(
        (resolved.immersion - 1.0).abs() < EPS,
        "{}",
        resolved.immersion
    );

    let sludge = EnvironmentVolumes::build(vec![pool(0.0, 2.0, 0), stream(3)]);
    let resolved = resolve(&sludge, Vec3::new(0.0, 2.0, 0.0));
    assert_eq!(resolved.fluid, Some(3), "the smaller stream wins the fluid");
    assert!(
        (resolved.immersion - 0.75).abs() < EPS,
        "different fluids never add: {}",
        resolved.immersion
    );
}

// Regression: a NaN probe resolved fully immersed in any fluid its NaN axis
// made the segment test pass.
#[test]
fn non_finite_probe_resolves_level_defaults_dry() {
    let everywhere = EnvironmentVolume {
        gravity: Some(-2.0),
        push: Some(Vec3::new(3.0, 0.0, 0.0)),
        fluid: Some(1),
        ..volume(vec![axis_box(Vec3::splat(-50.0), Vec3::splat(50.0))])
    };
    let volumes = EnvironmentVolumes::build(vec![everywhere]);
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[4.0, 4.0]);
    let defaults = ResolvedEnvironment {
        gravity: LEVEL_GRAVITY,
        push: Vec3::ZERO,
        fluid: None,
        viscosity: 0.0,
        immersion: 0.0,
        tick_eye_in_fluid: false,
    };
    for position in [
        Vec3::new(f32::NAN, 0.0, 0.0),
        Vec3::new(0.0, f32::NAN, 0.0),
        Vec3::new(0.0, 0.0, f32::INFINITY),
        Vec3::new(f32::NEG_INFINITY, 0.0, 0.0),
    ] {
        assert_eq!(
            resolve_environment(view, &probe_at(position)),
            defaults,
            "{position:?}"
        );
        assert_eq!(
            resolve_gravity(view, position),
            LEVEL_GRAVITY,
            "{position:?}"
        );
    }
    // A finite body with a non-finite eye is still wet, its eye out.
    let nan_eye = BodyProbe {
        eye_height: f32::NAN,
        ..probe_at(Vec3::ZERO)
    };
    let resolved = resolve_environment(view, &nan_eye);
    assert_eq!(resolved.fluid, Some(1));
    assert!(!resolved.tick_eye_in_fluid);
}
