// Reach index: the flood over full-tree leaves, chart culls and per-texel
// masks on the two-rooms fixtures.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use super::closed_leaves;
use super::slab::ChartFrame;
use super::test_fixtures::{
    ReachFixture, Wall, chart_inside, point_light, room_a_centre, room_b, room_b_centre,
};
use crate::chart_raster::{chart_interior_dims, chart_texel_world_position};
use crate::lightmap_bake::Chart;
use crate::lightmap_layer::chart_may_cover;
use crate::map_data::MapLight;
use crate::partition::BspTree;
use crate::texel_footprint::TexelFrame;

/// The fixtures' lightmap density: coarse, so charts stay small.
const DENSITY: f32 = 0.25;

/// A light at room A's centre whose range reaches well into room B.
fn room_a_light() -> MapLight {
    point_light(room_a_centre(), 6.0)
}

/// Room B's charts the light's direct term reaches, by atlas index.
fn far_room_charts<'c>(charts: &'c [Chart], light: &MapLight) -> Vec<(usize, &'c Chart)> {
    charts
        .iter()
        .enumerate()
        .filter(|(_, chart)| chart_inside(chart, room_b()) && chart_may_cover(light, chart))
        .collect()
}

#[test]
fn reach_flood_stops_at_a_solid_wall() {
    let fixture = ReachFixture::two_rooms(Wall::Solid);
    let light = room_a_light();
    let atlas = fixture.atlas(std::slice::from_ref(&light), DENSITY);
    let index = fixture.index(std::slice::from_ref(&light), &atlas.charts, true);
    assert_eq!(
        index.reaches(0, fixture.leaf_at(room_a_centre())),
        Some(true)
    );
    assert_eq!(
        index.reaches(0, fixture.leaf_at(room_b_centre())),
        Some(false)
    );
}

// Two rooms split by a solid wall, the light in room A: every room-B chart
// the light covers is culled. Inside one, a texel whose footprint reaches a
// wall's solid traces, and a texel clear of every wall by more than its
// footprint skips.
#[test]
fn reach_culls_the_far_room_and_traces_only_texels_whose_footprint_touches_solid() {
    let fixture = ReachFixture::two_rooms(Wall::Solid);
    let light = room_a_light();
    let atlas = fixture.atlas(std::slice::from_ref(&light), DENSITY);
    let index = fixture.index(std::slice::from_ref(&light), &atlas.charts, true);
    let charts = far_room_charts(&atlas.charts, &light);
    assert!(!charts.is_empty(), "the light covers a room-B chart");
    let (min, max) = room_b();
    let (mut traced, mut skipped) = (0, 0);
    for (chart_idx, chart) in charts {
        let reach = index
            .culled_chart(0, chart_idx, chart)
            .unwrap_or_else(|| panic!("room-B chart {chart_idx} is culled"));
        // Floor charts: their texels' in-plane distance to room B's walls.
        if chart.normal.y < 0.99 {
            continue;
        }
        let radius = f64::from(TexelFrame::for_chart(chart).radius());
        let (width, height) = chart_interior_dims(chart);
        for ty in 0..height {
            for tx in 0..width {
                let p = chart_texel_world_position(chart, tx, ty).as_dvec3();
                let to_wall = [p.x - min.x, max.x - p.x, p.z - min.z, max.z - p.z]
                    .into_iter()
                    .fold(f64::INFINITY, f64::min);
                if to_wall < radius {
                    assert!(
                        !reach.skips(tx, ty),
                        "texel ({tx}, {ty}) {to_wall:.3} m from a wall, footprint {radius:.3} m, \
                         must trace"
                    );
                    traced += 1;
                } else if to_wall > radius + 0.01 {
                    assert!(
                        reach.skips(tx, ty),
                        "texel ({tx}, {ty}) {to_wall:.3} m from every wall must skip"
                    );
                    skipped += 1;
                }
            }
        }
    }
    assert!(
        traced > 0 && skipped > 0,
        "traced {traced}, skipped {skipped}"
    );
}

/// Assert the opening joins the rooms in the flood and keeps every room-B
/// chart the light covers.
fn assert_far_room_kept(wall: Wall) {
    let fixture = ReachFixture::two_rooms(wall);
    let light = room_a_light();
    let atlas = fixture.atlas(std::slice::from_ref(&light), DENSITY);
    let index = fixture.index(std::slice::from_ref(&light), &atlas.charts, true);
    assert_eq!(
        index.reaches(0, fixture.leaf_at(room_b_centre())),
        Some(true),
        "{wall:?}: the flood reaches room B"
    );
    let charts = far_room_charts(&atlas.charts, &light);
    assert!(!charts.is_empty());
    for (chart_idx, chart) in charts {
        assert!(
            index.culled_chart(0, chart_idx, chart).is_none(),
            "{wall:?}: room-B chart {chart_idx} is kept"
        );
    }
}

#[test]
fn reach_keeps_the_far_room_through_a_doorway_within_reach() {
    assert_far_room_kept(Wall::doorway());
}

// Regression: the shipped full-tree portals drop openings under 0.1 m², so a
// 30 cm vent left the rooms unconnected and the flood culled light that
// passes through it.
#[test]
fn reach_keeps_the_far_room_through_a_30_cm_vent_within_reach() {
    assert_far_room_kept(Wall::vent(12.0));
}

// Regression: the shipped full-tree portals also drop pieces narrower than
// their 1 cm clip tolerance, which a ray crosses.
#[test]
fn reach_flood_crosses_a_2_5_mm_slit() {
    let fixture = ReachFixture::two_rooms(Wall::slit(0.1));
    let light = room_a_light();
    let atlas = fixture.atlas(std::slice::from_ref(&light), DENSITY);
    let index = fixture.index(std::slice::from_ref(&light), &atlas.charts, true);
    assert_eq!(
        index.reaches(0, fixture.leaf_at(room_b_centre())),
        Some(true)
    );
}

#[test]
fn reach_index_never_culls_a_sun_or_a_light_in_solid() {
    let fixture = ReachFixture::two_rooms(Wall::Solid);
    let mut sun = room_a_light();
    sun.light_type = crate::map_data::LightType::Directional;
    sun.cone_direction = Some([0.0, -1.0, 0.0]);
    // Inside the wall between the rooms.
    let buried = point_light(
        crate::reflection_probes::placement_test_fixtures::engine([264.0, 128.0, 64.0]),
        6.0,
    );
    let lights = [sun, buried];
    let atlas = fixture.atlas(&lights, DENSITY);
    let index = fixture.index(&lights, &atlas.charts, true);
    assert_eq!(index.culling_light_count(), 0);
    let leaf = fixture.leaf_at(room_b_centre());
    assert_eq!(index.reaches(0, leaf), None);
    assert_eq!(index.reaches(1, leaf), None);
    assert!(
        (0..atlas.charts.len())
            .all(|c| (0..2).all(|light| index.culled_chart(light, c, &atlas.charts[c]).is_none()))
    );
}

// A slab whose descent touches no leaf keeps its pairs: an empty leaf set
// would otherwise read as unreached and skip every texel. A tree with no
// nodes stands in for a descent that finds none.
#[test]
fn reach_keeps_a_chart_whose_slab_touches_no_leaf() {
    let fixture = ReachFixture::two_rooms(Wall::Solid);
    let light = room_a_light();
    let atlas = fixture.atlas(std::slice::from_ref(&light), DENSITY);
    let (_, chart) = far_room_charts(&atlas.charts, &light)[0];
    let frame = ChartFrame::new(chart, true).expect("an orthonormal frame");
    let leafless = BspTree {
        nodes: Vec::new(),
        leaves: Vec::new(),
    };
    assert!(frame.reach(&leafless, &[]).is_none());
    let closed = closed_leaves(fixture.tree(), fixture.exterior());
    assert!(frame.reach(fixture.tree(), &closed).is_some());
}
