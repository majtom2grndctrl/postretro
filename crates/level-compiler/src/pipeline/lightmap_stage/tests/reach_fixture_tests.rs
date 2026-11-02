//! Reach-cull fixtures through the index and the fused stage: a doorway past
//! R, a slab reaching where texel centres do not, a brush top set into a wall,
//! a mover in a doorway, and an embedded emitter at each floor setting. Each
//! asserts culled or kept, and that bytes with the cull on equal bytes with it
//! off at both floor settings, verify mode on.
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use super::reach_bake::{assert_cull_exact, reach_bake};
use super::*;
use crate::chart_raster::{chart_interior_dims, chart_texel_world_position};
use crate::lightmap_bake::{Chart, RAY_EPSILON};
use crate::lightmap_layer::chart_may_cover;
use crate::lightmap_reach::LightmapReachIndex;
use crate::lightmap_reach::test_fixtures::{
    ReachFixture, Rooms, Wall, chart_inside, point_light, room_a_centre, room_b, two_rooms,
};
use crate::reflection_probes::placement_test_fixtures::{
    UNIT, box_brush, engine, entity, parse_inline,
};

/// The fixtures' lightmap density, as the stage bakes them.
const DENSITY: f32 = 0.25;

/// Assert the cull exact at both floor settings; the counts, floor on then
/// off.
fn assert_exact_at_both_floors(fixture: &ReachFixture, lights: &[MapLight]) -> [ReachCounts; 2] {
    [true, false].map(|floor| assert_cull_exact(fixture, lights, floor))
}

/// The charts inside `bounds` whose texels `light`'s direct term reaches, by
/// atlas index.
fn covered_charts_in<'c>(
    charts: &'c [Chart],
    light: &MapLight,
    bounds: (DVec3, DVec3),
) -> Vec<(usize, &'c Chart)> {
    charts
        .iter()
        .enumerate()
        .filter(|(_, chart)| chart_inside(chart, bounds) && chart_may_cover(light, chart))
        .collect()
}

/// Each interior texel's centre lifted one `RAY_EPSILON` along the chart
/// normal: where its centre shadow ray starts.
fn lifted_centres(chart: &Chart) -> impl Iterator<Item = DVec3> + '_ {
    let (width, height) = chart_interior_dims(chart);
    (0..height).flat_map(move |ty| {
        (0..width).map(move |tx| {
            (chart_texel_world_position(chart, tx, ty) + chart.normal * RAY_EPSILON).as_dvec3()
        })
    })
}

// --- doorway past R --------------------------------------------------------

/// Rooms 768 u long, joined by a doorway at their far end, so a light near
/// the wall reaches room B's floor through solid long before the doorway.
const LONG_ROOMS: Rooms = Rooms {
    wall_x: [256.0, 272.0],
    max: [528.0, 768.0, 128.0],
};
const FAR_DOORWAY: [f64; 4] = [600.0, 664.0, 0.0, 96.0];
/// The light's map-unit x and z; its y sets its distance to the doorway.
const LIGHT_X: f64 = 200.0;
const LIGHT_Z: f64 = 64.0;

fn long_rooms_light(y: f64) -> MapLight {
    point_light(engine([LIGHT_X, y, LIGHT_Z]), 6.0)
}

/// Distance, in metres, from the light at map-unit `y` to the doorway's
/// room-B mouth: the rectangle x = wall_x[1], y and z within the opening.
fn mouth_distance(y: f64) -> f64 {
    let [y0, _, z0, z1] = FAR_DOORWAY;
    assert!(y < y0 && (z0..=z1).contains(&LIGHT_Z));
    (LONG_ROOMS.wall_x[1] - LIGHT_X).hypot(y0 - y) * UNIT
}

/// The map-unit y that puts the light `distance` metres from the mouth.
fn light_y_at(distance: f64) -> f64 {
    let across = LONG_ROOMS.wall_x[1] - LIGHT_X;
    FAR_DOORWAY[0] - ((distance / UNIT).powi(2) - across * across).sqrt()
}

/// The light's R, and whether its flood reaches room B beside it.
fn reach_of(fixture: &ReachFixture, light: &MapLight) -> (f64, bool) {
    let atlas = fixture.atlas(std::slice::from_ref(light), DENSITY);
    let index = fixture.index(std::slice::from_ref(light), &atlas.charts, true);
    let beside = engine([400.0, light_y_of(light), LIGHT_Z]);
    (
        index.reach_radius(0).expect("the light is eligible"),
        index
            .reaches(0, fixture.leaf_at(beside))
            .expect("the light is eligible"),
    )
}

fn light_y_of(light: &MapLight) -> f64 {
    // engine(q) = (-q.y, q.z, -q.x) * UNIT.
    -light.origin.x / UNIT
}

// The same rooms joined by a doorway whose room-B mouth lies 2 cm past the
// light's R: the flood stops short of room B and the stage culls room B's
// pairs. 2 cm inside R, the flood reaches room B.
#[test]
fn reach_culls_the_far_room_behind_a_doorway_just_past_reach() {
    let fixture = ReachFixture::compile(&LONG_ROOMS.world(Wall::Opening(FAR_DOORWAY)), &[]);
    // R depends on the footprints of the charts the light's range admits,
    // so place the light from a first guess's R, then check the R it has.
    let (guess, _) = reach_of(&fixture, &long_rooms_light(360.0));
    let past = long_rooms_light(light_y_at(guess + 0.02));
    let (reach, reached) = reach_of(&fixture, &past);
    let distance = mouth_distance(light_y_of(&past));
    assert!(
        distance > reach && distance < reach + 0.03,
        "mouth {distance} m, R {reach} m"
    );
    assert!(!reached, "the flood stops at the doorway past R");
    let within = long_rooms_light(light_y_at(reach - 0.02));
    let (reach_within, reached_within) = reach_of(&fixture, &within);
    assert!(mouth_distance(light_y_of(&within)) < reach_within);
    assert!(reached_within, "a doorway within R joins the rooms");

    let lights = [past];
    let atlas = fixture.atlas(&lights, DENSITY);
    assert!(
        !covered_charts_in(&atlas.charts, &lights[0], LONG_ROOMS.room_b()).is_empty(),
        "the light covers a room-B chart through the wall"
    );
    for counts in assert_exact_at_both_floors(&fixture, &lights) {
        assert!(counts.culled > 0, "{counts:?}");
    }
}

// --- centres vs slab ---------------------------------------------------------

// A 10 cm wall: room B's charts beside it have every texel centre in room B,
// which the flood never reaches, while their floored slab, padded a texel
// footprint past the edge texels, reaches through the wall into room A. The
// pair is kept. Floor off, the slab pads by the centre sampler's zero
// footprint and stays in room B.
#[test]
fn reach_keeps_a_chart_whose_slab_reaches_a_reached_leaf_its_centres_do_not() {
    let rooms = Rooms {
        wall_x: [256.0, 260.0],
        max: [516.0, 256.0, 128.0],
    };
    let fixture = ReachFixture::compile(&rooms.world(Wall::Solid), &[]);
    let lights = [point_light(room_a_centre(), 6.0)];
    let atlas = fixture.atlas(&lights, DENSITY);
    let index = fixture.index(&lights, &atlas.charts, true);
    let reached = |point: DVec3| index.reaches(0, fixture.leaf_at(point)) == Some(true);
    let mut kept = 0;
    for (chart_idx, chart) in covered_charts_in(&atlas.charts, &lights[0], rooms.room_b()) {
        if lifted_centres(chart).any(reached) {
            continue;
        }
        let slab = index
            .forced_chart(chart_idx, chart)
            .expect("an orthonormal chart frame");
        if !slab
            .open_leaves()
            .iter()
            .any(|&leaf| index.reaches(0, leaf as usize) == Some(true))
        {
            assert!(index.culled_chart(0, chart_idx, chart).is_some());
            continue;
        }
        assert!(
            index.culled_chart(0, chart_idx, chart).is_none(),
            "chart {chart_idx}: its slab touches a reached leaf"
        );
        kept += 1;
    }
    assert!(kept > 0, "a room-B chart's slab reaches through the wall");
    let [on, off] = assert_exact_at_both_floors(&fixture, &lights);
    assert!(off.culled > on.culled, "on {on:?}, off {off:?}");
}

// --- brush top set into a wall ---------------------------------------------

// A crate in room B set 16 u into the side wall: its top chart's slab pads
// past the wall face into solid. The relaxed rule ignores that solid leaf,
// so the pair is culled, and the top's texels against the wall trace.
#[test]
fn reach_culls_a_far_room_brush_top_whose_slab_overhangs_into_solid() {
    let mut world = two_rooms(Wall::Solid);
    world.push(box_brush([288.0, 224.0, 0.0], [352.0, 272.0, 32.0]));
    let fixture = ReachFixture::compile(&world, &[]);
    let lights = [point_light(room_a_centre(), 6.0)];
    let atlas = fixture.atlas(&lights, DENSITY);
    let index = fixture.index(&lights, &atlas.charts, true);
    let top_height = engine([0.0, 0.0, 32.0]).y;
    let tops: Vec<_> = covered_charts_in(&atlas.charts, &lights[0], room_b())
        .into_iter()
        .filter(|(_, chart)| {
            chart.normal.y > 0.99
                && (chart_texel_world_position(chart, 0, 0).y as f64 - top_height).abs() < 1.0e-3
        })
        .collect();
    assert_eq!(tops.len(), 1, "one crate top chart");
    let (chart_idx, chart) = tops[0];
    // Above the crate top, past the wall face: the solid the slab overhangs.
    let overhang = engine([320.0, 262.0, 32.0]) + DVec3::Y * 1.5 * f64::from(RAY_EPSILON);
    assert!(fixture.tree().leaves[fixture.leaf_at(overhang)].is_solid);
    let reach = index
        .culled_chart(0, chart_idx, chart)
        .expect("the crate top is culled");
    let (width, height) = chart_interior_dims(chart);
    let traced: Vec<(i32, i32)> = (0..height)
        .flat_map(|ty| (0..width).map(move |tx| (tx, ty)))
        .filter(|&(tx, ty)| !reach.skips(tx, ty))
        .collect();
    assert!(
        !traced.is_empty() && traced.len() < (width * height) as usize,
        "the texels against the wall trace, the rest skip: {traced:?} of {width}x{height}"
    );
    for counts in assert_exact_at_both_floors(&fixture, &lights) {
        assert!(counts.culled > 0, "{counts:?}");
    }
}

// --- far from the world origin ---------------------------------------------

// The two rooms moved 300 m and 1 km along one axis, the light in room A:
// room B's pairs are culled and skip as many texels as at the world origin,
// with bytes equal to the unculled bake's at both floor settings, verify
// mode on. The slab's rounding slack grows with the coordinates; a bound
// loose enough to reach the brush under each face traces every texel.
#[test]
fn reach_skips_far_room_texels_a_kilometre_from_the_world_origin() {
    let counts_at = |metres: f64| {
        let offset = [metres / UNIT, 0.0, 0.0];
        let fixture = ReachFixture::compile(&Rooms::STANDARD.world_at(Wall::Solid, offset), &[]);
        let lights = [point_light(engine([128.0 + offset[0], 128.0, 64.0]), 6.0)];
        assert_exact_at_both_floors(&fixture, &lights)
    };
    let near = counts_at(0.0);
    for metres in [300.0, 1000.0] {
        for ((floor, far), near) in [true, false].into_iter().zip(counts_at(metres)).zip(near) {
            eprintln!("{metres} m, floor {floor}: {far:?}");
            assert!(
                far.culled > 0
                    && far.skipped > 0
                    && (far.culled, far.skipped, far.traced_in_culled)
                        == (near.culled, near.skipped, near.traced_in_culled),
                "{metres} m, floor {floor}: {far:?}; at the origin {near:?}"
            );
        }
    }
}

// --- a mover in the doorway --------------------------------------------------

/// A sliding door: a `kinematic_mover` brush filling the standard doorway at
/// waypoint `door_a`, slid 64 u into the wall at `door_b`, authored at the
/// pose `slide` map units along y.
fn sliding_door(slide: f64) -> Vec<String> {
    let brush = box_brush([256.0, 96.0 + slide, 0.0], [272.0, 160.0 + slide, 96.0]);
    vec![
        entity(
            "kinematic_mover",
            "\"name\" \"door\"\n\"path\" \"door_a\"\n\"speed\" \"1\"\n\"move_mode\" \"ping_pong\"",
            &[brush],
        ),
        entity(
            "kinematic_waypoint",
            "\"name\" \"door_a\"\n\"next\" \"door_b\"\n\"origin\" \"264 128 48\"",
            &[],
        ),
        entity(
            "kinematic_waypoint",
            "\"name\" \"door_b\"\n\"origin\" \"264 192 48\"",
            &[],
        ),
    ]
}

// A mover brush between the light and room B, at either waypoint pose: the
// mover is in neither the static tree nor the BVH, so the flood passes the
// doorway it closes and every room-B pair is kept.
#[test]
fn reach_keeps_pairs_behind_a_kinematic_mover_at_every_waypoint_pose() {
    for slide in [0.0, 32.0, 64.0] {
        let world = two_rooms(Wall::doorway());
        let map = parse_inline(&format!(
            "{}\n{}\n",
            entity("worldspawn", "", &world),
            sliding_door(slide).join("\n")
        ));
        assert_eq!(map.kinematic_movers.len(), 1, "pose {slide}: one mover");
        let fixture = ReachFixture::compile(&world, &sliding_door(slide));
        let doorway = engine([264.0, 128.0, 48.0]);
        assert!(
            !fixture.tree().leaves[fixture.leaf_at(doorway)].is_solid,
            "pose {slide}: the mover is not in the static tree"
        );
        let lights = [point_light(room_a_centre(), 6.0)];
        let atlas = fixture.atlas(&lights, DENSITY);
        let index: LightmapReachIndex<'_> = fixture.index(&lights, &atlas.charts, true);
        let charts = covered_charts_in(&atlas.charts, &lights[0], room_b());
        assert!(!charts.is_empty());
        for (chart_idx, chart) in charts {
            assert!(
                index.culled_chart(0, chart_idx, chart).is_none(),
                "pose {slide}: room-B chart {chart_idx} is kept"
            );
        }
        if slide == 0.0 {
            for counts in assert_exact_at_both_floors(&fixture, &lights) {
                assert_eq!(counts.culled, 0, "{counts:?}");
            }
        }
    }
}

// --- embedded emitter --------------------------------------------------------

// A light whose 0.6 m emitter sphere reaches 0.2 m into the wall. Floored,
// the stage flags it, so its pairs are never culled; floor off, the stage
// runs no emitter test and flags none, so room B's pairs are culled.
#[test]
fn reach_follows_the_stage_embedded_emitter_flag_at_each_floor_setting() {
    let fixture = ReachFixture::two_rooms(Wall::Solid);
    let mut light = point_light(engine([240.0, 128.0, 64.0]), 6.0);
    light.light_size = 0.6;
    let lights = [light];
    let flagged = |records: &[postretro_test_log_capture::CapturedRecord]| {
        records.iter().any(|record| {
            record.level == Level::Warn && record.message.contains("emitter inside geometry")
        })
    };
    let [on, off] = assert_exact_at_both_floors(&fixture, &lights);
    assert_eq!(on.culled, 0, "floored: flagged, never culled: {on:?}");
    assert!(on.walked > 0);
    assert!(off.culled > 0, "floor off: flags none, culled: {off:?}");
    for (floor, expect) in [(true, true), (false, false)] {
        let bake =
            reach_bake(&fixture, &lights, floor, lightmap_layer::ExactSkips::ON, 1).expect("bake");
        assert_eq!(flagged(&bake.records), expect, "floor {floor}");
    }
}
