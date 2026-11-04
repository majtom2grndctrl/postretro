// Capture exclusion: kept solid that holds an excluded hull's exit, passed over for a nearer excluded neighbour, bounds every later read.
// See: context/lib/resource_management.md §5.2 (Exclusion)

use super::capture_bounds::{HELD_DEPTH_SLACK_M, HeldDepth};
use super::capture_exclusion::{CONTACT_TOLERANCE_M, RayOutcome};
use super::capture_exclusion_test_fixtures::{
    CaptureFixture, ROOM_MAX, ROOM_MIN, TRIM_TEXTURE, exclusion, pool_entity, textured_room,
};
use super::capture_exclusion_tests::{
    CONTACT_GAP, IN_BACK_WALL, PLATFORM_HEIGHT, UNITS_PER_METER, boxed, over_the_platform,
    platform_under_a_block_with,
};
use super::capture_fill::Fill;
use super::hit::SurfaceHit;
use super::placement_test_fixtures::textured_box_brush;
use crate::ray_traversal::TRACER_TOLERANCE_METERS;

/// R6's ray: across the block's front 1.1 mm over the platform, 0.3 mm
/// inside the block's bottom edge, descending 0.008 map z per map x.
const R6_RAY: (f64, f64) = (0.0011, 0.008);

/// How far N's top stands over the platform, in metres.
const N_TOP_M: f64 = 0.0005;

/// The map x where the ray of [`over_the_platform`]`(over, slope)` lies
/// `above` metres over the platform's top.
fn at_height(over: f64, slope: f64, above: f64) -> f64 {
    100.0 + (over - above) * UNITS_PER_METER / slope
}

/// The R6 room: the excluded block over map x 100 to 116, [`CONTACT_GAP`]
/// over the platform; under it, with no near brush, an excluded brush N from
/// map x 100 to 104 topped 0.5 mm over the platform, and a kept brush K from
/// 104 to 116 topped at the platform. CSG removes K's front against N and
/// K's whole top under the block.
struct R6 {
    room: CaptureFixture,
    block: u32,
    excluded: u32,
    kept: u32,
}

impl R6 {
    fn new() -> Self {
        Self::with_near(None)
    }

    /// With a kept near brush before the block, topped at `near`.
    fn with_near(near: Option<f64>) -> Self {
        let h = PLATFORM_HEIGHT;
        let room = platform_under_a_block_with(
            near,
            &[
                ([100.0, 104.0], h + N_TOP_M * UNITS_PER_METER),
                ([104.0, 116.0], h),
            ],
            h + CONTACT_GAP,
        );
        let block = room.brush_at([108.0, 0.0, 24.0]);
        let excluded = room.brush_at([102.0, 0.0, h / 2.0]);
        let kept = room.brush_at([110.0, 0.0, h / 2.0]);
        Self {
            room,
            block,
            excluded,
            kept,
        }
    }

    /// The layout's asserted preconditions for a ray `(over, slope)`: CSG
    /// removed K's front and top; the ray crosses the block's front and
    /// leaves its bottom; N and K both hold that exit within contact of the
    /// block, N nearer; and K does not hold N's exit.
    fn assert_preconditions(&self, (over, slope): (f64, f64)) {
        let (room, h) = (&self.room, PLATFORM_HEIGHT);
        assert_eq!(
            room.faces_on_plane(self.kept, 0, [104.0, 0.0, 0.0]),
            0,
            "CSG removes K's front against N"
        );
        assert_eq!(
            room.faces_on_plane(self.kept, 2, [0.0, 0.0, h]),
            0,
            "CSG removes K's top under the block"
        );
        let gap = CONTACT_GAP / UNITS_PER_METER;
        assert!(over > gap, "the ray enters the block's front");
        let exit = at_height(over, slope, gap);
        // Where the ray first lies inside N (its top) and K (its top): each
        // is that brush's depth at the block's exit, measured off the
        // block's bottom plane.
        let n_at = at_height(over, slope, N_TOP_M);
        let k_at = at_height(over, slope, 0.0);
        assert!(exit < n_at && n_at < 104.0, "N's top past the exit: {n_at}");
        assert!(k_at > 104.0 && k_at < 116.0, "K's top past N: {k_at}");
        for (name, top) in [("N", N_TOP_M), ("K", 0.0)] {
            assert!(
                gap - top <= CONTACT_TOLERANCE_M,
                "{name} holds the block's exit, {} m under it",
                gap - top
            );
        }
        // At N's exit (its back, map x = 104) K is first inside at its top,
        // that far off N's back plane: out of contact.
        assert!(
            (k_at - 104.0) / UNITS_PER_METER > CONTACT_TOLERANCE_M,
            "K does not hold N's exit"
        );
    }
}

/// Asserts `outcome` reads the wall behind, as the trace does without the
/// carried depth on the rows below.
fn assert_reads_the_wall_behind(room: &CaptureFixture, outcome: RayOutcome) {
    let back_wall = room.brush_at(IN_BACK_WALL);
    assert!(
        matches!(outcome, RayOutcome::Hit(hit) if room.face_brush(hit.face) == Some(back_wall)),
        "the uncarried trace reads the wall behind: {outcome:?}"
    );
}

/// The block's front, as the unmasked ray hits it about 1 m out.
fn block_front(r6: &R6, outcome: RayOutcome) -> SurfaceHit {
    let RayOutcome::Hit(front) = outcome else {
        panic!("the block's front faces the ray, not {outcome:?}");
    };
    assert_eq!(r6.room.face_brush(front.face), Some(r6.block), "{front:?}");
    assert!((front.distance - 1.0).abs() < 1e-3, "{front:?}");
    front
}

// Regression: R6 (watertight-tracer plan review, round 5). A ray crossing an
// excluded block whose exit an excluded brush N and kept solid K both held
// went on through N, the nearer, and dropped K's depth. It left N into open
// space beyond K's contact, re-traced into K through the top CSG removed
// under the block, and read the wall behind through it.
#[test]
fn kept_solid_passed_over_at_a_blocks_exit_bounds_the_re_trace_from_a_nearer_neighbour() {
    let r6 = R6::new();
    r6.assert_preconditions(R6_RAY);
    let (origin, direction) = over_the_platform(R6_RAY.0, R6_RAY.1);
    let back_wall = r6.room.brush_at(IN_BACK_WALL);
    r6.room.with_scene(|scene, probes| {
        let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
        let front = block_front(&r6, scene.trace(origin, direction, &exclusion(scene, &probe, &[])));
        let filter = exclusion(scene, &probe, &[r6.block, r6.excluded]);
        let slot = |brush: u32| filter.excluded().binary_search(&brush).unwrap();
        let (kept, excluded) = filter.contacts_of(slot(r6.block));
        assert!(kept.contains(&r6.kept), "{kept:?}");
        assert_eq!(excluded, vec![r6.excluded]);
        let outcome = scene.trace(origin, direction, &filter);
        assert!(
            !matches!(outcome, RayOutcome::Hit(hit) if r6.room.face_brush(hit.face) == Some(back_wall)),
            "reads the wall behind through K: {outcome:?}"
        );
        assert_eq!(outcome, RayOutcome::LeakGuard(front));
        // Without the carried depth the trace reads the wall behind: the
        // depth, not another rule, decides this ray.
        let control = filter.ignoring_held_depth();
        assert_reads_the_wall_behind(&r6.room, scene.trace(origin, direction, &control));
    });
}

// The carried depth survives a re-trace and a second hull. R6 with N split
// along the ray, a 12.7 mm gap between the pieces: the ray leaves N1 into
// open space, re-traces, takes a tolerance hit on N2's front just over its
// top edge and crosses N2 before it reaches K, then leaves N2 into open space
// short of K's depth. Only the depth K held at the block's exit, carried
// from the first crossing, bounds the re-trace from N2.
#[test]
fn kept_depth_carried_from_one_hull_bounds_the_re_trace_past_a_later_hull() {
    let h = PLATFORM_HEIGHT;
    let (over, slope) = R6_RAY;
    let n1_top = 0.0007;
    let room = platform_under_a_block_with(
        None,
        &[
            ([100.0, 102.0], h + n1_top * UNITS_PER_METER),
            ([102.5, 104.0], h + N_TOP_M * UNITS_PER_METER),
            ([104.0, 116.0], h),
        ],
        h + CONTACT_GAP,
    );
    let block = room.brush_at([108.0, 0.0, 24.0]);
    let first = room.brush_at([101.0, 0.0, h / 2.0]);
    let second = room.brush_at([103.0, 0.0, h / 2.0]);
    let kept = room.brush_at([110.0, 0.0, h / 2.0]);
    assert_eq!(
        room.faces_on_plane(kept, 0, [104.0, 0.0, 0.0]),
        0,
        "CSG removes K's front against N2"
    );
    assert_eq!(
        room.faces_on_plane(kept, 2, [0.0, 0.0, h]),
        0,
        "CSG removes K's top under the block"
    );
    assert!(
        room.faces_on_plane(second, 0, [102.5, 0.0, 0.0]) > 0,
        "N2's front faces the gap"
    );
    let gap = CONTACT_GAP / UNITS_PER_METER;
    let exit = at_height(over, slope, gap);
    let n1_at = at_height(over, slope, n1_top);
    let k_at = at_height(over, slope, 0.0);
    assert!(
        exit < n1_at && n1_at < 102.0,
        "N1's top past the exit: {n1_at}"
    );
    assert!(gap - n1_top <= CONTACT_TOLERANCE_M && gap <= CONTACT_TOLERANCE_M);
    assert!(n1_at < k_at, "N1 is nearer than K at the block's exit");
    // At N2's front the ray lies within τ over N2's top edge, and it enters
    // N2 through its top before N2's back, short of K's depth.
    let over_n2 = over - slope * 2.5 / UNITS_PER_METER - N_TOP_M;
    let tolerance = f64::from(TRACER_TOLERANCE_METERS);
    assert!(over_n2 > 0.0 && over_n2 < tolerance, "{over_n2}");
    let n2_at = at_height(over, slope, N_TOP_M);
    assert!(
        n2_at > 102.5 && n2_at < 104.0 && k_at > 104.0,
        "{n2_at}, {k_at}"
    );
    assert!(
        (k_at - 104.0) / UNITS_PER_METER > CONTACT_TOLERANCE_M,
        "K does not hold N2's exit"
    );
    let (origin, direction) = over_the_platform(over, slope);
    room.with_scene(|scene, probes| {
        let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
        let unmasked = scene.trace(origin, direction, &exclusion(scene, &probe, &[]));
        let RayOutcome::Hit(front) = unmasked else {
            panic!("the block's front faces the ray, not {unmasked:?}");
        };
        assert_eq!(room.face_brush(front.face), Some(block), "{front:?}");
        let filter = exclusion(scene, &probe, &[block, first, second]);
        let slot = |brush: u32| filter.excluded().binary_search(&brush).unwrap();
        let (kept_of_block, excluded_of_block) = filter.contacts_of(slot(block));
        assert!(kept_of_block.contains(&kept), "{kept_of_block:?}");
        assert!(excluded_of_block.contains(&first), "{excluded_of_block:?}");
        let (kept_of_first, excluded_of_first) = filter.contacts_of(slot(first));
        assert!(
            !kept_of_first.contains(&kept) && !excluded_of_first.contains(&second),
            "N1 leaves into open space: {kept_of_first:?}, {excluded_of_first:?}"
        );
        let mut hull_passes = 0;
        let outcome = filter.trace_counting(
            &scene.tracer,
            origin,
            direction,
            scene.max_distance,
            &mut hull_passes,
        );
        assert_eq!(outcome, RayOutcome::LeakGuard(front));
        assert_eq!(hull_passes, 2, "the ray re-traced past N1 and N2");
        let control = filter.ignoring_held_depth();
        assert_reads_the_wall_behind(&room, scene.trace(origin, direction, &control));
    });
}

// The class at a pass beside. R6 with a kept near brush level with N, which
// takes N's front with CSG, and a ray passing just under the block's bottom
// edge: its only hit there is a tolerance hit on the block's front. N and K
// both hold that pass, N nearer, and the ray reaches K past N's back as in R6.
#[test]
fn kept_solid_passed_over_on_a_pass_beside_the_block_bounds_the_re_trace() {
    let h = PLATFORM_HEIGHT;
    let (over, slope) = (0.0006, 0.004);
    let r6 = R6::with_near(Some(h + N_TOP_M * UNITS_PER_METER));
    let gap = CONTACT_GAP / UNITS_PER_METER;
    assert!(
        over < gap && gap - over < f64::from(TRACER_TOLERANCE_METERS),
        "the ray's line misses the block, within τ of its front"
    );
    assert_eq!(
        r6.room.faces_on_plane(r6.excluded, 0, [100.0, 0.0, 0.0]),
        0,
        "CSG removes N's front against the near brush"
    );
    let n_at = at_height(over, slope, N_TOP_M);
    let k_at = at_height(over, slope, 0.0);
    assert!(
        n_at < 104.0 && k_at > 104.0 && k_at < 116.0,
        "{n_at}, {k_at}"
    );
    assert!(
        (k_at - 104.0) / UNITS_PER_METER > CONTACT_TOLERANCE_M,
        "K does not hold N's exit"
    );
    let (origin, direction) = over_the_platform(over, slope);
    r6.room.with_scene(|scene, probes| {
        let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
        let front = block_front(
            &r6,
            scene.trace(origin, direction, &exclusion(scene, &probe, &[])),
        );
        let filter = exclusion(scene, &probe, &[r6.block, r6.excluded]);
        assert_eq!(
            scene.trace(origin, direction, &filter),
            RayOutcome::LeakGuard(front)
        );
        let control = filter.ignoring_held_depth();
        assert_reads_the_wall_behind(&r6.room, scene.trace(origin, direction, &control));
    });
}

// The class at a second hull of one classification. R6 with an excluded brush
// F butted to the block's front: the ray crosses F and goes on through the
// block, its neighbour, before K is passed over at the block's exit, all
// within one classification.
#[test]
fn kept_solid_passed_over_at_a_second_hull_of_one_classification_bounds_the_re_trace() {
    let h = PLATFORM_HEIGHT;
    let mut world = textured_room();
    for (min, max) in [
        (
            [100.0, -8.0, 0.0],
            [104.0, 8.0, h + N_TOP_M * UNITS_PER_METER],
        ),
        ([104.0, -8.0, 0.0], [116.0, 8.0, h]),
        ([100.0, -8.0, h + CONTACT_GAP], [116.0, 8.0, 40.0]),
        ([84.0, -8.0, h + CONTACT_GAP], [100.0, 8.0, 40.0]),
    ] {
        world.push(textured_box_brush(min, max, TRIM_TEXTURE));
    }
    let room = CaptureFixture::compile(&world, &[pool_entity()]);
    let block = room.brush_at([108.0, 0.0, 24.0]);
    let excluded = room.brush_at([102.0, 0.0, h / 2.0]);
    let kept = room.brush_at([110.0, 0.0, h / 2.0]);
    let fore = room.brush_at([92.0, 0.0, 24.0]);
    assert_eq!(
        room.faces_on_plane(fore, 0, [100.0, 0.0, 0.0]),
        0,
        "CSG removes F's back against the block"
    );
    let (origin, direction) = over_the_platform(R6_RAY.0, R6_RAY.1);
    room.with_scene(|scene, probes| {
        let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
        let unmasked = scene.trace(origin, direction, &exclusion(scene, &probe, &[]));
        let RayOutcome::Hit(front) = unmasked else {
            panic!("F's front faces the ray, not {unmasked:?}");
        };
        assert_eq!(room.face_brush(front.face), Some(fore), "{front:?}");
        let filter = exclusion(scene, &probe, &[block, excluded, fore]);
        let slot = |brush: u32| filter.excluded().binary_search(&brush).unwrap();
        assert!(filter.contacts_of(slot(fore)).1.contains(&block));
        assert!(filter.contacts_of(slot(block)).0.contains(&kept));
        assert_eq!(
            scene.trace(origin, direction, &filter),
            RayOutcome::LeakGuard(front)
        );
        let control = filter.ignoring_held_depth();
        assert_reads_the_wall_behind(&room, scene.trace(origin, direction, &control));
    });
}

// An exit into open space past the carried depth. K nested along the ray
// inside N, so N's back lies past K's depth and past K itself: the ray takes
// the leak guard at that exit, with no re-trace.
#[test]
fn an_exit_into_open_space_past_the_carried_depth_takes_the_leak_guard_there() {
    let h = PLATFORM_HEIGHT;
    let (over, slope) = R6_RAY;
    let room = platform_under_a_block_with(
        None,
        &[
            ([100.0, 110.0], h + N_TOP_M * UNITS_PER_METER),
            ([104.0, 108.0], h),
        ],
        h + CONTACT_GAP,
    );
    let block = room.brush_at([112.0, 0.0, 24.0]);
    let excluded = room.brush_at([102.0, 0.0, h / 2.0]);
    let k_at = at_height(over, slope, 0.0);
    assert!(
        k_at > 104.0 && k_at < 108.0,
        "K's depth short of N's back: {k_at}"
    );
    let (origin, direction) = over_the_platform(over, slope);
    room.with_scene(|scene, probes| {
        let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
        let filter = exclusion(scene, &probe, &[block, excluded]);
        let mut hull_passes = 0;
        let outcome = filter.trace_counting(
            &scene.tracer,
            origin,
            direction,
            scene.max_distance,
            &mut hull_passes,
        );
        assert!(matches!(outcome, RayOutcome::LeakGuard(_)), "{outcome:?}");
        assert_eq!(
            hull_passes, 0,
            "the guard falls at the exit, not on a re-trace"
        );
        let control = filter.ignoring_held_depth();
        assert_reads_the_wall_behind(&room, scene.trace(origin, direction, &control));
    });
}

// A trace carries the least depth it passed over, whatever the order.
#[test]
fn a_carried_depth_is_the_least_passed_over() {
    let mut held = HeldDepth::NONE;
    for depth in [2.0, 1.1, 1.5] {
        held.pass_over(depth);
    }
    assert!(held.admits(1.1));
    assert!(!held.admits(1.1 + 2.0 * HELD_DEPTH_SLACK_M));
}

// A brush wholly off the projection box is never a contact, so it adds no
// depth: the R6 layout with the box ending short of K captures the same on
// every ray, K kept or excluded.
#[test]
fn the_r6_layout_with_the_kept_brush_off_the_box_captures_the_same_kept_or_excluded() {
    let r6 = R6::new();
    r6.room.with_scene(|scene, probes| {
        let probe = boxed(&probes[0], ROOM_MIN, [103.0, ROOM_MAX[1], ROOM_MAX[2]]);
        let kept = exclusion(scene, &probe, &[r6.block, r6.excluded]);
        let excluded = exclusion(scene, &probe, &[r6.block, r6.excluded, r6.kept]);
        let slot = kept.excluded().binary_search(&r6.block).unwrap();
        assert!(
            !kept.contacts_of(slot).0.contains(&r6.kept),
            "K is off the box"
        );
        for over in [0.0006, 0.0009, 0.0011, 0.0013] {
            for slope in [0.004, 0.006, 0.008, 0.012] {
                let (origin, direction) = over_the_platform(over, slope);
                assert_eq!(
                    scene.trace(origin, direction, &kept),
                    scene.trace(origin, direction, &excluded),
                    "{over}, {slope}"
                );
            }
        }
    });
}

/// A traced surface at `distance` along the ray, on the mesh's first face.
fn surface_at(distance: f32) -> SurfaceHit {
    SurfaceHit {
        distance,
        face: 0,
        triangle: [0, 1, 2],
        barycentric: [1.0, 0.0, 0.0],
    }
}

// Once kept solid has been passed over at a depth, every kind of read the
// trace settles on, a kept hit, a fill and an exit into open space, reads at
// that depth within distance rounding and takes the leak guard past it. A kept
// hit and a fill go through the read point the trace itself calls, on the R6
// layout's exclusion, at a depth whose point lies in the projection box. The
// exit is the predicate its arm tests; the nested-K row drives that arm.
#[test]
fn reads_past_a_carried_kept_depth_take_the_leak_guard_and_reads_at_it_stand() {
    let r6 = R6::new();
    let (origin, direction) = over_the_platform(R6_RAY.0, R6_RAY.1);
    r6.room.with_scene(|scene, probes| {
        let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
        let filter = exclusion(scene, &probe, &[r6.block, r6.excluded]);
        let first = surface_at(1.0);
        let depth = 1.1_f64;
        let mut held = HeldDepth::NONE;
        held.pass_over(depth);
        // At the depth: as `f32` rounds it, and a rounding's width past it.
        // Past it: twice the slack.
        let at = [depth as f32, (depth + 0.1 * HELD_DEPTH_SLACK_M) as f32];
        let past = (depth + 2.0 * HELD_DEPTH_SLACK_M) as f32;
        let fill_at = |distance: f32| Fill {
            distance,
            brush: r6.kept,
            side: 0,
            texture_index: 0,
            lit: surface_at(distance),
        };
        for distance in at {
            let hit = surface_at(distance);
            assert_eq!(
                filter.settle(Some(hit), Some(first), held, origin, direction),
                RayOutcome::Hit(hit),
                "a kept hit at the depth reads"
            );
            let fill = fill_at(distance);
            assert_eq!(
                filter.filled(fill, None, first, held, origin, direction),
                RayOutcome::Fill(fill),
                "a fill at the depth reads"
            );
            assert!(
                filter.admits(held, f64::from(distance)),
                "an exit into open space at the depth traces on"
            );
        }
        assert_eq!(
            filter.settle(Some(surface_at(past)), Some(first), held, origin, direction),
            RayOutcome::LeakGuard(first),
            "a kept hit past the depth"
        );
        assert_eq!(
            filter.filled(fill_at(past), None, first, held, origin, direction),
            RayOutcome::LeakGuard(first),
            "a fill past the depth"
        );
        assert!(
            !filter.admits(held, f64::from(past)),
            "an exit into open space past the depth"
        );
        // With nothing passed over, the same reads stand.
        let hit = surface_at(past);
        assert_eq!(
            filter.settle(Some(hit), Some(first), HeldDepth::NONE, origin, direction),
            RayOutcome::Hit(hit)
        );
    });
}
