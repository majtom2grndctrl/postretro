//! Lightmap reach cull through the fused stage: culled bytes equal unculled
//! bytes, verify mode's planted fault, and the verify switch.
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use super::reach_bake::{reach_bake, reach_off};
use super::*;
use crate::lightmap_reach::test_fixtures::{ReachFixture, Wall, point_light, room_a_centre};

use super::super::reach_verify::resolve_reach_verify;

// Two rooms split by a solid wall, the light in room A: the stage culls
// room B's pairs, skips texels and, floored, traces the ones whose footprint
// touches solid, with verify mode re-walking every culled pair, and its bytes equal
// the bake with only the reach cull off, at each floor setting and at one
// worker and many.
#[test]
fn reach_cull_two_rooms_bytes_equal_the_unculled_bake_at_each_floor_setting() {
    let fixture = ReachFixture::two_rooms(Wall::Solid);
    let lights = [point_light(room_a_centre(), 6.0)];
    for penumbra_floor in [true, false] {
        let culled = reach_bake(
            &fixture,
            &lights,
            penumbra_floor,
            lightmap_layer::ExactSkips::ON,
            4,
        )
        .expect("culled bake");
        let counts = culled.counts;
        assert!(counts.verify, "verify mode is on in tests");
        assert!(counts.culled > 0, "floor {penumbra_floor}: {counts:?}");
        assert_eq!(counts.rewalked, counts.culled, "floor {penumbra_floor}");
        assert!(counts.skipped > 0, "floor {penumbra_floor}: {counts:?}");
        // A floored texel's footprint reaches a texel pitch, so the border
        // texels beside room B's walls trace; the centre sampler traces from
        // the centre alone, half a pitch clear of every wall.
        assert_eq!(
            counts.traced_in_culled > 0,
            penumbra_floor,
            "floor {penumbra_floor}: {counts:?}"
        );
        let unculled =
            reach_bake(&fixture, &lights, penumbra_floor, reach_off(), 4).expect("unculled bake");
        assert_eq!(unculled.counts.culled, 0);
        assert_eq!(unculled.counts.walked, counts.walked);
        assert!(
            culled.lightmap == unculled.lightmap,
            "floor {penumbra_floor}: lightmap bytes differ with the reach cull off"
        );
        assert!(
            culled.shadowmask == unculled.shadowmask,
            "floor {penumbra_floor}: shadowmask bytes differ with the reach cull off"
        );
        let serial = reach_bake(
            &fixture,
            &lights,
            penumbra_floor,
            lightmap_layer::ExactSkips::ON,
            1,
        )
        .expect("serial bake");
        assert_eq!(
            serial.counts, counts,
            "floor {penumbra_floor}: tallies by workers"
        );
        assert!(serial.lightmap == culled.lightmap && serial.shadowmask == culled.shadowmask);
    }
}

// A planted fault: every pair forced culled, so room A's lit texels away
// from its walls store 0. Verify mode fails the bake, naming one with both
// values.
#[test]
fn reach_verify_fails_a_planted_cull_naming_a_lit_texel_and_both_values() {
    let fixture = ReachFixture::two_rooms(Wall::Solid);
    let lights = [point_light(room_a_centre(), 6.0)];
    let forced = lightmap_layer::ExactSkips {
        force_reach_cull: true,
        ..lightmap_layer::ExactSkips::ON
    };
    let error = match reach_bake(&fixture, &lights, true, forced, 4) {
        Ok(_) => panic!("verify mode passes a planted cull"),
        Err(error) => format!("{error:#}"),
    };
    assert!(error.contains("lightmap reach verify"), "{error}");
    assert!(error.contains("texel ("), "{error}");
    let kept = error
        .rsplit("the kept walk ")
        .next()
        .expect("the kept walk's value");
    assert!(
        error.contains("the culled walk stored 0.0,") && kept != "0.0" && kept != "no record",
        "the named texel is lit: {error}"
    );
}

#[test]
fn reach_verify_switch_resolves_one_zero_unset_and_malformed() {
    assert_eq!(resolve_reach_verify(Some("1"), false), Ok(true));
    assert_eq!(resolve_reach_verify(Some(" 0 "), true), Ok(false));
    assert_eq!(resolve_reach_verify(None, false), Ok(false));
    assert_eq!(resolve_reach_verify(None, true), Ok(true));
    assert_eq!(resolve_reach_verify(Some("yes"), false), Err(false));
    assert_eq!(resolve_reach_verify(Some(""), false), Err(false));
}
