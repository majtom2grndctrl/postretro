// Capture exclusion: kept solid that holds an excluded hull's exit, passed over for a nearer excluded neighbour, bounds every later read.
// See: context/lib/resource_management.md §5.2 (Exclusion)

use super::capture_bounds::{HELD_DEPTH_SLACK_M, HeldDepth};
use super::capture_exclusion::{CONTACT_TOLERANCE_M, RayOutcome};
use super::capture_exclusion_test_fixtures::{CaptureFixture, ROOM_MAX, ROOM_MIN, exclusion};
use super::capture_exclusion_tests::{
    CONTACT_GAP, IN_BACK_WALL, PLATFORM_HEIGHT, UNITS_PER_METER, boxed, over_the_platform,
    platform_under_a_block_with,
};
use super::capture_fill::Fill;
use super::hit::SurfaceHit;

/// Map units per millimetre.
const MM: f64 = 0.001 * UNITS_PER_METER;

/// R6's ray (`research.md` §R6 by reading): across the block's front
/// 1.1 mm over the platform, 0.3 mm inside the block's bottom edge,
/// descending 0.008 map z per map x.
const R6_RAY: (f64, f64) = (0.0011, 0.008);

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
        let h = PLATFORM_HEIGHT;
        let room = platform_under_a_block_with(
            None,
            &[([100.0, 104.0], h + 0.5 * MM), ([104.0, 116.0], h)],
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
        let n_at = at_height(over, slope, 0.0005);
        let k_at = at_height(over, slope, 0.0);
        assert!(exit < n_at && n_at < 104.0, "N's top past the exit: {n_at}");
        assert!(k_at > 104.0 && k_at < 116.0, "K's top past N: {k_at}");
        for (name, top) in [("N", 0.0005), ("K", 0.0)] {
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
// that depth within distance rounding and takes the leak guard past it. Each
// goes through the read point the trace itself calls, on the R6 layout's
// exclusion, at a depth whose point lies in the projection box.
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
        // Past it: just beyond the slack.
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
