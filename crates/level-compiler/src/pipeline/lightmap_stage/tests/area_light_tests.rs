//! Area lights through the fused stage and its keys: every exact skip on equals
//! every skip off, the reach cull culls past a solid wall and the shaft cull
//! clears tiles under a flush panel; a moved polygon vertex, a moved wall in
//! reach of a panel's edge and a raised sample count re-key what they should.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12, Area lights)

use super::reach_bake::{ReachRun, reach_bake, reach_bake_with};
use super::*;
use crate::area_light::EmitterPolygon;
use crate::area_light::test_lights::{area_light, ceiling_panel, pillar_sides};
use crate::delta_sh_cache::encoded_key_light;
use crate::lightmap_layer::{SkipStats, layer_input_hash};
use crate::lightmap_reach::test_fixtures::{
    ReachFixture, Rooms, Wall, room_a_centre, room_b_centre,
};
use crate::ray_traversal::TRACER_EPOCH;
use crate::reflection_probes::placement_test_fixtures::{UNIT, box_brush};
use crate::surface_radiance::LightDigest;

/// Room A's ceiling height, engine metres.
const CEILING: f64 = 128.0 * UNIT;

/// A 1 m panel flush in room A's ceiling and a light wrapping room A's
/// pillar, each reaching 5 m: far enough to reach room B past the wall.
fn room_a_lights() -> Vec<MapLight> {
    let panel = area_light(
        vec![ceiling_panel([-4.5, -3.5], [-4.5, -3.5], CEILING)],
        1.0,
        Some(5.0),
    );
    let axis = -72.0 * UNIT;
    let pillar = area_light(
        pillar_sides(axis, axis, 8.0 * UNIT, [0.5, 1.5]),
        1.0,
        Some(5.0),
    );
    vec![panel, pillar]
}

/// The two rooms split by a solid wall, with a 16-unit pillar in room A
/// whose axis is the pillar light's.
fn rooms_with_pillar() -> ReachFixture {
    let mut world = Rooms::STANDARD.world(Wall::Solid);
    world.push(box_brush([64.0, 64.0, 0.0], [80.0, 80.0, 128.0]));
    ReachFixture::compile(&world, &[])
}

/// Every layer key `light` takes over `fixture`'s atlas at `samples`.
fn layer_keys(fixture: &ReachFixture, light: &MapLight, samples: u32) -> Vec<[u8; 32]> {
    let config = config(false);
    let mut geometry = fixture.compiled.geometry.clone();
    let prepared = lightmap_bake::prepare_atlas(
        &mut geometry,
        &StaticBakedLights::from_lights(std::slice::from_ref(light)),
        config.lightmap_density,
        &[],
    )
    .expect("fixture atlas");
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    (0..prepared.layer_count)
        .map(|layer| {
            layer_input_hash(
                light,
                &shared,
                &fixture.compiled.primitives,
                &geometry,
                config.lightmap_density,
                samples,
                config.penumbra_floor,
                TRACER_EPOCH,
                layer,
            )
        })
        .collect()
}

// AC 21: on a map lit by a flush ceiling panel and a light wrapping a
// pillar, every lightmap and shadowmask byte is the same with every exact
// skip on and off, at both floor settings. With them on, the reach cull
// culls room B's pairs past the solid wall (verify mode re-walks each), and
// the shaft cull clears tiles of the panel's floor.
#[test]
fn area_lights_bake_byte_identically_with_every_exact_skip_on_and_off() {
    let fixture = rooms_with_pillar();
    let lights = room_a_lights();
    let panel_origin = lights[0].origin.as_vec3().to_array();
    for penumbra_floor in [true, false] {
        let stats = SkipStats::leak();
        let on = lightmap_layer::ExactSkips::ON.tallied(stats);
        let culled = reach_bake(&fixture, &lights, penumbra_floor, on, 4).expect("culled bake");
        let unculled = reach_bake(
            &fixture,
            &lights,
            penumbra_floor,
            lightmap_layer::ExactSkips::OFF,
            4,
        )
        .expect("unculled bake");
        assert!(
            culled.lightmap == unculled.lightmap,
            "floor {penumbra_floor}: lightmap bytes differ with the exact skips off"
        );
        assert!(
            culled.shadowmask == unculled.shadowmask,
            "floor {penumbra_floor}: shadowmask bytes differ with the exact skips off"
        );
        let counts = culled.counts;
        assert!(counts.culled > 0, "floor {penumbra_floor}: {counts:?}");
        assert_eq!(counts.rewalked, counts.culled, "floor {penumbra_floor}");
        let cleared: usize = stats
            .charts()
            .iter()
            .filter(|chart| chart.light_origin == panel_origin)
            .map(|chart| chart.clear_tiles().count())
            .sum();
        assert!(cleared > 0, "floor {penumbra_floor}: no panel tile cleared");
    }
}

// The reach index floods an area light from the leaves in front of its
// polygons: the light wrapping the pillar, whose centroid lies inside the
// pillar, is culled as a point light in open air is, and its flood stops at
// the solid wall.
#[test]
fn a_pillar_wrapped_light_floods_from_its_faces_not_its_centroid() {
    let fixture = rooms_with_pillar();
    let lights = room_a_lights();
    let pillar = std::slice::from_ref(&lights[1]);
    let centroid_leaf = fixture.leaf_at(lights[1].origin);
    assert!(fixture.tree().leaves[centroid_leaf].is_solid);
    let atlas = fixture.atlas(pillar, config(false).lightmap_density);
    let index = fixture.index(pillar, &atlas.charts, true);
    assert_eq!(index.culling_light_count(), 1);
    assert_eq!(
        index.reaches(0, fixture.leaf_at(room_a_centre())),
        Some(true)
    );
    assert_eq!(
        index.reaches(0, fixture.leaf_at(room_b_centre())),
        Some(false)
    );
}

// AC 22: moving one vertex of the panel's polygon re-keys its lightmap
// layers, its surface-radiance digest and the key bytes the SH-family memos
// fold; the pillar light's keys do not move.
#[test]
fn moving_one_polygon_vertex_rekeys_only_that_lights_entries() {
    let fixture = rooms_with_pillar();
    let lights = room_a_lights();
    let samples = test_args().soft_shadow_samples;
    let pillar_before = (
        layer_keys(&fixture, &lights[1], samples),
        LightDigest::of(&lights[1]),
        encoded_key_light(&lights[1]),
    );
    let panel = &lights[0].area.as_ref().unwrap().polygons[0];
    let mut vertices = panel.vertices.clone();
    vertices[2] += DVec3::new(0.05, 0.0, 0.05);
    let moved = area_light(
        vec![EmitterPolygon::new(vertices, panel.normal)],
        1.0,
        Some(5.0),
    );
    let (panel, pillar) = (&lights[0], &lights[1]);
    assert_ne!(
        layer_keys(&fixture, panel, samples),
        layer_keys(&fixture, &moved, samples)
    );
    assert_ne!(LightDigest::of(panel), LightDigest::of(&moved));
    assert_ne!(encoded_key_light(panel), encoded_key_light(&moved));
    // Each key folds its own light alone, so the pillar light's entries,
    // over the same geometry and atlas, stay where they were.
    let pillar_after = (
        layer_keys(&fixture, pillar, samples),
        LightDigest::of(pillar),
        encoded_key_light(pillar),
    );
    assert!(
        pillar_after == pillar_before,
        "the pillar light's keys moved"
    );
}

/// The two rooms with a 16 × 8 × 16-unit box against room A's east wall,
/// 80 to 96 units up, from `qx` along the wall.
fn rooms_with_box(qx: f64) -> ReachFixture {
    let mut world = Rooms::STANDARD.world(Wall::Solid);
    world.push(box_brush([qx, 0.0, 80.0], [qx + 16.0, 8.0, 96.0]));
    ReachFixture::compile(&world, &[])
}

// AC 26 (O10): a box within reach of a 3 m panel's edge, but farther than
// its reach (and its padded influence box) from the centroid, moves between
// two builds. The panel's layers re-key, and the warm rebuild over the first
// build's cache matches a cold build byte for byte. (A point light at the
// centroid is no control here: the box's planes re-split faces its own
// influence box holds.)
#[test]
fn moving_a_wall_in_reach_of_a_panels_edge_rekeys_it_and_warm_equals_cold() {
    let reach = 2.0;
    let edge = -1.7;
    let panel = area_light(
        vec![ceiling_panel([-4.7, edge], [-4.7, edge], CEILING)],
        1.0,
        Some(reach),
    );
    let (before, after) = (rooms_with_box(112.0), rooms_with_box(96.0));
    // The box's near face lies 8 units from the wall at x 0, its top 96
    // units up; the panel's +x edge and the centroid lie west of it. The
    // centroid's influence box pads its reach by 0.5 m and a texel (0.25 m).
    let box_x = -8.0 * UNIT;
    let box_top = 96.0 * UNIT;
    let to_edge = DVec3::new(box_x - edge, box_top - CEILING, 0.0).length();
    assert!(
        to_edge < f64::from(reach),
        "within reach of the panel: {to_edge}"
    );
    let padded = f64::from(reach) + 0.5 + 0.25;
    assert!(
        (box_x - panel.origin.x).abs() > padded,
        "beyond the centroid's padded reach"
    );

    let samples = test_args().soft_shadow_samples;
    assert_ne!(
        layer_keys(&before, &panel, samples),
        layer_keys(&after, &panel, samples),
        "the panel's layers re-key"
    );

    let lights = [panel];
    let dir = fresh_cache_dir("area_wall_move");
    let cache = StageCache::new(&dir).expect("cache");
    let bake = |fixture: &ReachFixture, cache: Option<&StageCache>| {
        let mut run = ReachRun::new(true, lightmap_layer::ExactSkips::ON, 4);
        run.cache = cache;
        run.verify = Some(false);
        reach_bake_with(fixture, &lights, &run).expect("bake")
    };
    let _ = bake(&before, Some(&cache));
    let warm = bake(&after, Some(&cache));
    let cold = bake(&after, None);
    assert!(
        warm.counts.baked > 0,
        "the moved box re-bakes: {:?}",
        warm.counts
    );
    assert!(
        warm.lightmap == cold.lightmap && warm.shadowmask == cold.shadowmask,
        "warm bytes differ from cold"
    );
    drop(cache);
    let _ = std::fs::remove_dir_all(&dir);
}

// AC 37 (texel paths): raising the soft-shadow sample count re-keys an area
// light's lightmap layers, and the penumbra its texels store changes with
// it. The probe-path bakes take the fixed default lattice: none of their
// keys reads the count.
#[test]
fn raising_the_sample_count_rekeys_and_rebakes_area_texels() {
    let fixture = rooms_with_pillar();
    let lights = room_a_lights();
    let panel = &lights[0];
    assert_ne!(
        layer_keys(&fixture, panel, 4),
        layer_keys(&fixture, panel, 16)
    );
    let config = config(false);
    let bake = |samples: u32| {
        let mut geometry = fixture.compiled.geometry.clone();
        let prepared = lightmap_bake::prepare_atlas(
            &mut geometry,
            &StaticBakedLights::from_lights(std::slice::from_ref(panel)),
            config.lightmap_density,
            &[],
        )
        .expect("fixture atlas");
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
            layout: &prepared.layout,
        };
        lightmap_layer::bake_light_layer(
            panel,
            &shared,
            &fixture.compiled.bvh,
            &fixture.compiled.primitives,
            &geometry,
            samples,
            true,
            lightmap_layer::ExactSkips::ON,
            &BakeControl::unrestricted(),
        )
        .into_iter()
        .flat_map(|layer| layer.texels)
        .map(|texel| (texel.idx, texel.raw_visibility.to_bits()))
        .collect::<Vec<_>>()
    };
    let (coarse, fine) = (bake(4), bake(16));
    assert_eq!(
        coarse.iter().map(|&(idx, _)| idx).collect::<Vec<_>>(),
        fine.iter().map(|&(idx, _)| idx).collect::<Vec<_>>(),
        "the same texels are lit"
    );
    assert_ne!(coarse, fine, "the pillar's penumbra moves with the count");
}
