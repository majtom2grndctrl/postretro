// Capture exclusion through the stage: classification feeding the capture, its cache key, and the pipeline around it.
// See: context/lib/resource_management.md §5.2 (Exclusion) · build_pipeline.md §Build Cache

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use glam::DVec3;
use log::Level;
use postretro_level_format::reflection_probes::ReflectionProbesSection;
use postretro_test_log_capture::LogCapture;

use super::capture_exclusion::{BrushFaces, ProbeExclusion};
use super::capture_exclusion_test_fixtures::{
    CEILING_DEPTH, CaptureFixture, PILLAR_TEXTURE, ROOM_MAX, pillar, pool_entity, textured_room,
};
use super::capture_exclusion_tests::{IN_BACK_WALL, IN_CEILING, IN_PILLAR, only_probe};
use super::exclusion::{ExclusionConstants, ExclusionScene, classify_probes_within_boxes};
use super::hit::SceneTracer;
use super::placement_test_fixtures::{
    UNIT, entity, fluid, reflection_volume, sealed_room, textured_box_brush,
};
use super::{
    ReflectionProbeInputs, STAGE_ID, bake_or_load_reflection_probes, bake_or_load_under,
    bake_or_load_unfiltered, cache_key,
};
use crate::bake_control::BakeControl;
use crate::cache::StageCache;
use crate::map_data::ReflectionCapture;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The stage's id 54 for `room`, filtered or with every excluded set forced
/// empty, through `cache` when given.
fn bake(
    room: &CaptureFixture,
    filtered: bool,
    cache: Option<&StageCache>,
) -> ReflectionProbesSection {
    let control = BakeControl::unrestricted();
    room.with_inputs(|inputs| {
        if filtered {
            bake_or_load_reflection_probes(inputs, cache, &control)
        } else {
            bake_or_load_unfiltered(inputs, cache, &control)
        }
    })
    .unwrap()
    .expect("the pool keeps its probe")
}

/// The stage's excluded sets for `room`: placement, then classification
/// within each probe's box, as the stage runs them.
fn stage_sets(room: &CaptureFixture) -> Vec<Vec<u32>> {
    stage_sets_under(room, &ExclusionConstants::PRODUCTION)
}

/// [`stage_sets`] with the rule under `constants`.
fn stage_sets_under(room: &CaptureFixture, constants: &ExclusionConstants) -> Vec<Vec<u32>> {
    let probes = room.world.place().unwrap();
    room.with_inputs(|inputs| classify_probes_within_boxes(&scene(inputs), &probes, constants))
}

fn scene<'a>(inputs: &ReflectionProbeInputs<'a>) -> ExclusionScene<'a> {
    ExclusionScene {
        brush_volumes: inputs.brush_volumes,
        capture_override: inputs.capture_override,
        face_brush: inputs.face_brushes,
        geometry: inputs.geometry,
        environment_volumes: inputs.environment_volumes,
    }
}

/// The capture stage's key for `room` under the rule's `constants`, which
/// classify and key alike, every step before it run as the stage runs it.
fn stage_key(room: &CaptureFixture, constants: &ExclusionConstants) -> String {
    let probes = room.world.place().unwrap();
    room.with_inputs(|inputs| {
        let excluded = classify_probes_within_boxes(&scene(inputs), &probes, constants);
        let tracer = SceneTracer::new(inputs.bvh, inputs.primitives, inputs.geometry);
        let exclusions: Vec<ProbeExclusion<'_>> = probes
            .iter()
            .zip(&excluded)
            .map(|(probe, set)| ProbeExclusion::new(&tracer, room.brushes(), set, probe))
            .collect();
        cache_key(inputs, &probes, &exclusions, constants)
            .unwrap()
            .as_filename()
    })
}

/// The pillar room with its pillar topped at `top` (map units).
fn pillar_room(top: f64) -> CaptureFixture {
    let mut world = textured_room();
    world.push(pillar(top));
    CaptureFixture::compile(&world, &[pool_entity()])
}

// ---------------------------------------------------------------------------
// Stage flow
// ---------------------------------------------------------------------------

// Placement and the cap never see exclusion. The pillar
// fixture's derived box changes when the pillar is deleted, yet the filtered
// and unfiltered builds place the same probes, boxes, points and bindings;
// only the capture differs.
#[test]
fn placed_probes_are_identical_with_the_filter_on_and_off() {
    let room = pillar_room(ROOM_MAX[2]);
    let deleted = CaptureFixture::pool_room(None);
    let (with, without) = (
        only_probe(&room.world.place().unwrap()).clone(),
        only_probe(&deleted.world.place().unwrap()).clone(),
    );
    assert_ne!(
        (with.box_min, with.box_max),
        (without.box_min, without.box_max),
        "deleting the pillar moves the derived box"
    );
    let pillar = room.brush_at(IN_PILLAR);
    assert_eq!(stage_sets(&room), vec![vec![pillar]]);

    let filtered = bake(&room, true, None);
    let unfiltered = bake(&room, false, None);
    let placement = |section: &ReflectionProbesSection| {
        let probes: Vec<_> = section
            .probes
            .iter()
            .map(|p| {
                (
                    p.source_entity,
                    p.box_min,
                    p.box_max,
                    p.capture_point,
                    p.layer,
                )
            })
            .collect();
        (probes, section.bindings.clone())
    };
    assert_eq!(placement(&filtered), placement(&unfiltered));
    let record = &filtered.probes[0];
    assert_eq!(
        (record.box_min, record.box_max, record.capture_point),
        (with.box_min, with.box_max, with.capture_point)
    );
    assert_ne!(filtered.tiles, unfiltered.tiles, "the filter is on");
    assert_ne!(filtered.distances, unfiltered.distances);
}

// A level whose brushes are all at least every probe's
// threshold excludes nothing, and its id 54 is byte-identical to the build
// with every excluded set forced empty. A block beside the pool, not just
// the room's slabs, sits just past the threshold.
#[test]
fn a_level_whose_brushes_all_reach_the_threshold_captures_byte_identically_unfiltered() {
    let mut world = textured_room();
    world.push(textured_box_brush(
        [-100.0, 104.0, 0.0],
        [60.0, 120.0, 48.0],
        PILLAR_TEXTURE,
    ));
    let room = CaptureFixture::compile(&world, &[pool_entity()]);
    let probe = only_probe(&room.world.place().unwrap()).clone();
    let threshold = super::exclusion::size_threshold(super::exclusion::water_width(&probe));
    assert!(
        160.0 * UNIT > threshold,
        "the block clears t = {threshold} m"
    );
    assert_eq!(stage_sets(&room), vec![Vec::<u32>::new()]);
    assert_eq!(
        bake(&room, true, None).to_bytes(),
        bake(&room, false, None).to_bytes()
    );
}

// A brush wholly off a probe's projection box captures identically kept or
// excluded, which is why the stage's sets hold only brushes touching the box.
// The test uses a crate past an authored box's edge, which the rule alone
// would exclude.
#[test]
fn a_brush_off_the_projection_box_captures_identically_kept_or_excluded() {
    let mut world = textured_room();
    let crate_min = [96.0, -8.0, 0.0];
    world.push(textured_box_brush(
        crate_min,
        [112.0, 8.0, 16.0],
        PILLAR_TEXTURE,
    ));
    let room = CaptureFixture::compile(
        &world,
        &[
            pool_entity(),
            reflection_volume("", [-128.0, -128.0, 0.0], [80.0, 128.0, 128.0]),
        ],
    );
    let crate_ = room.brush_at([104.0, 0.0, 8.0]);
    let probes = room.world.place().unwrap();
    let map_wide =
        room.with_inputs(|inputs| super::exclusion::classify_probes(&scene(inputs), &probes));
    assert_eq!(map_wide, vec![vec![crate_]], "the rule excludes the crate");
    assert_eq!(stage_sets(&room), vec![Vec::<u32>::new()], "off the box");
    let capture = |excluded: Vec<u32>| {
        room.with_scene(|scene, probes| {
            super::capture_section(
                scene,
                probes,
                &[excluded],
                &[],
                &BakeControl::unrestricted(),
            )
            .unwrap()
            .0
            .to_bytes()
        })
    };
    assert_eq!(capture(vec![crate_]), capture(Vec::new()));
}

// Standing posts, all excluded, ring the capture point and
// hide nearly all of the pool from it. With every excluded set forced empty
// the hidden-water warning fires, on the cold build and the warm one; the
// filtered capture sees through the posts and warns on neither.
#[test]
fn hidden_water_warns_unfiltered_and_not_filtered_cold_and_warm() {
    let mut world = sealed_room([-160.0, -160.0, 0.0], [160.0, 160.0, 128.0]);
    for (min, max) in [
        ([-48.0, 32.0, 0.0], [48.0, 40.0, 96.0]),
        ([-48.0, -40.0, 0.0], [48.0, -32.0, 96.0]),
        ([32.0, -32.0, 0.0], [40.0, 32.0, 96.0]),
        ([-40.0, -32.0, 0.0], [-32.0, 32.0, 96.0]),
    ] {
        world.push(textured_box_brush(min, max, PILLAR_TEXTURE));
    }
    let water = fluid(
        "water",
        &[textured_box_brush(
            [-144.0, -144.0, 0.0],
            [144.0, 144.0, 16.0],
            "test/water",
        )],
    );
    let room = CaptureFixture::compile(
        &world,
        &[
            water,
            reflection_volume("", [-160.0, -160.0, 0.0], [160.0, 160.0, 128.0]),
        ],
    );
    let posts: Vec<u32> = [
        [0.0, 36.0, 48.0],
        [0.0, -36.0, 48.0],
        [36.0, 0.0, 48.0],
        [-36.0, 0.0, 48.0],
    ]
    .into_iter()
    .map(|q| room.brush_at(q))
    .collect::<std::collections::BTreeSet<_>>()
    .into_iter()
    .collect();
    assert_eq!(stage_sets(&room), vec![posts], "every post is excluded");

    let dir = tempfile::tempdir().unwrap();
    let cache = StageCache::new(dir.path()).unwrap();
    let logs = LogCapture::start();
    let warning = "of the water it binds";
    for build in ["cold", "warm"] {
        logs.clear();
        bake(&room, false, Some(&cache));
        logs.assert_logged_once(Level::Warn, warning);
        logs.clear();
        bake(&room, true, Some(&cache));
        logs.assert_not_logged(Level::Warn, warning);
        let access = cache.test_access(STAGE_ID);
        let expected = if build == "cold" { (2, 0) } else { (2, 2) };
        assert_eq!((access.writes, access.read_hits), expected, "{build}");
    }
}

// The capture with an exclusion in play is byte-identical at one
// worker and at four.
#[test]
fn an_excluding_capture_is_byte_identical_across_worker_counts() {
    let room = pillar_room(ROOM_MAX[2]);
    assert_eq!(stage_sets(&room), vec![vec![room.brush_at(IN_PILLAR)]]);
    let run = |threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| bake(&room, true, None))
            .to_bytes()
    };
    let one = run(1);
    assert_eq!(one, run(4));
    assert_eq!(one, run(1));
}

#[test]
fn brush_faces_list_each_brushs_faces_in_ascending_order() {
    let table = [Some(2), None, Some(0), Some(2), Some(9), Some(0)];
    let faces = BrushFaces::new(&table, 3);
    assert_eq!(faces.of(0), [2, 5]);
    assert_eq!(faces.of(1), [] as [u32; 0]);
    assert_eq!(faces.of(2), [0, 3]);
    assert_eq!(faces.of(3), [] as [u32; 0], "past the table");
}

// ---------------------------------------------------------------------------
// Cache key
// ---------------------------------------------------------------------------

// A changed resolved override value, or any curve or floor
// constant, misses the capture stage, even where the excluded sets stay the
// same; an unchanged rebuild hits.
#[test]
fn capture_key_misses_on_a_resolved_override_or_any_rule_constant_and_hits_otherwise() {
    let mut room = pillar_room(ROOM_MAX[2]);
    let production = ExclusionConstants::PRODUCTION;
    let base = stage_key(&room, &production);
    assert_eq!(base, stage_key(&room, &production), "unchanged input hits");
    let sets = stage_sets(&room);

    // The pillar `never` (the rule excludes it already) and a wall `always`
    // (the rule keeps it already): the sets hold, the table changes.
    let pillar = room.brush_at(IN_PILLAR) as usize;
    let wall = room.brush_at(IN_BACK_WALL) as usize;
    for (brush, value) in [
        (pillar, ReflectionCapture::Never),
        (wall, ReflectionCapture::Always),
    ] {
        room.world.capture_override[brush] = value;
        assert_eq!(stage_sets(&room), sets);
        assert_ne!(base, stage_key(&room, &production), "{brush} → {value:?}");
        room.world.capture_override[brush] = ReflectionCapture::Auto;
    }
    assert_eq!(base, stage_key(&room, &production));

    let perturbed: [(&str, ExclusionConstants); 8] = [
        (
            "near share",
            ExclusionConstants {
                near_share: 0.8,
                ..production
            },
        ),
        (
            "knee width",
            ExclusionConstants {
                knee_width_m: 12.0,
                ..production
            },
        ),
        (
            "far share",
            ExclusionConstants {
                far_share: 0.3,
                ..production
            },
        ),
        (
            "far width",
            ExclusionConstants {
                far_width_m: 60.0,
                ..production
            },
        ),
        (
            "stride divisor",
            ExclusionConstants {
                stride_divisor: 135,
                ..production
            },
        ),
        (
            "field of view",
            ExclusionConstants {
                hfov_degrees: 90.0,
                ..production
            },
        ),
        (
            "aspect",
            ExclusionConstants {
                floor_aspect: 4.0 / 3.0,
                ..production
            },
        ),
        (
            "underside slope",
            ExclusionConstants {
                underside_max_normal_dot: -0.02,
                ..production
            },
        ),
    ];
    for (what, constants) in perturbed {
        assert_ne!(base, stage_key(&room, &constants), "{what}");
    }
}

// The stage classifies under the instance its key hashes. A block just past
// the threshold stays in the probe; raising the near share past its size
// excludes it, misses the cache and captures as the block excluded would.
#[test]
fn a_perturbed_rule_constant_moves_a_brush_near_the_threshold_and_misses_the_capture() {
    let mut world = textured_room();
    world.push(textured_box_brush(
        [-100.0, 104.0, 0.0],
        [60.0, 120.0, 48.0],
        PILLAR_TEXTURE,
    ));
    let room = CaptureFixture::compile(&world, &[pool_entity()]);
    let block = room.brush_at([0.0, 112.0, 24.0]);
    let production = ExclusionConstants::PRODUCTION;
    let raised = ExclusionConstants {
        near_share: 0.95,
        ..production
    };
    let width = super::exclusion::water_width(only_probe(&room.world.place().unwrap()));
    let block_size = 160.0 * UNIT;
    assert!(
        production.size_threshold(width) < block_size && block_size < raised.size_threshold(width),
        "the block sits between t = {} m and t = {} m",
        production.size_threshold(width),
        raised.size_threshold(width)
    );
    assert_eq!(
        stage_sets_under(&room, &production),
        vec![Vec::<u32>::new()]
    );
    assert_eq!(stage_sets_under(&room, &raised), vec![vec![block]]);
    assert_ne!(stage_key(&room, &production), stage_key(&room, &raised));

    let dir = tempfile::tempdir().unwrap();
    let cache = StageCache::new(dir.path()).unwrap();
    let control = BakeControl::unrestricted();
    let bake_under = |constants: &ExclusionConstants| {
        room.with_inputs(|inputs| bake_or_load_under(inputs, Some(&cache), &control, constants))
            .unwrap()
            .expect("the pool keeps its probe")
            .to_bytes()
    };
    let (kept, excluded) = (bake_under(&production), bake_under(&raised));
    let access = cache.test_access(STAGE_ID);
    assert_eq!(
        (access.writes, access.read_hits),
        (2, 0),
        "both builds miss"
    );
    assert_eq!(kept, bake(&room, true, None).to_bytes());
    let capture = |excluded: Vec<u32>| {
        room.with_scene(|scene, probes| {
            super::capture_section(
                scene,
                probes,
                &[excluded],
                &[],
                &BakeControl::unrestricted(),
            )
            .unwrap()
            .0
            .to_bytes()
        })
    };
    assert_eq!(excluded, capture(vec![block]));
    assert_ne!(kept, excluded);
}

// An excluded pillar flush with the ceiling and one sunk into
// it leave the same surfaces, since CSG hides the sunk part, yet key the
// capture stage differently.
#[test]
fn flush_and_sunk_excluded_pillars_key_the_capture_differently() {
    let flush = pillar_room(ROOM_MAX[2]);
    let surfaces = |room: &CaptureFixture| {
        let geometry = &room.world.geometry.geometry;
        let positions: Vec<[f32; 3]> = geometry.vertices.iter().map(|v| v.position).collect();
        (positions, geometry.indices.clone())
    };
    // Sunk half a metre, CSG hides every sunk point; half a millimetre, f32
    // rounding of the clipped sides may tell them apart, and the key must
    // differ either way.
    for (sink_m, same_surfaces) in [(0.5, true), (0.0005, false)] {
        assert!(sink_m / UNIT < CEILING_DEPTH);
        let sunk = pillar_room(ROOM_MAX[2] + sink_m / UNIT);
        if same_surfaces {
            assert!(
                surfaces(&flush) == surfaces(&sunk),
                "{sink_m} m: CSG hides the sunk part"
            );
        }
        assert_eq!(stage_sets(&sunk), vec![vec![sunk.brush_at(IN_PILLAR)]]);
        assert_ne!(
            stage_key(&flush, &ExclusionConstants::PRODUCTION),
            stage_key(&sunk, &ExclusionConstants::PRODUCTION),
            "{sink_m} m"
        );
    }
}

// With geometry, primitives and every other input fixed, moving
// one plane of an excluded hull, or one side of a kept brush it touches, or
// dropping one of that kept brush's faces from the face → brush table,
// misses; the same edit to a brush neither excluded nor touched hits.
#[test]
fn moving_an_excluded_hull_plane_or_a_contact_side_misses_the_capture() {
    let mut room = pillar_room(ROOM_MAX[2]);
    let production = ExclusionConstants::PRODUCTION;
    let base = stage_key(&room, &production);
    let pillar = room.brush_at(IN_PILLAR) as usize;
    let ceiling = room.brush_at(IN_CEILING) as usize;
    // The wall behind the pillar the capture reads; the pillar never
    // touches it.
    let wall = room.brush_at(IN_BACK_WALL) as usize;

    room.world.brushes[pillar].planes[0].distance += 1.0e-3;
    assert_ne!(
        base,
        stage_key(&room, &production),
        "an excluded hull plane"
    );
    room.world.brushes[pillar].planes[0].distance -= 1.0e-3;
    assert_eq!(base, stage_key(&room, &production));

    let shifted = |room: &mut CaptureFixture, brush: usize, by: f64| {
        room.world.brushes[brush].sides[0].vertices[0] += DVec3::splat(by);
    };
    shifted(&mut room, ceiling, 1.0e-3);
    assert_ne!(base, stage_key(&room, &production), "a contact side");
    shifted(&mut room, ceiling, -1.0e-3);
    assert_eq!(base, stage_key(&room, &production));

    // The fill pools a kept contact's faces; the wall's are never read.
    let sets = stage_sets(&room);
    let retag = |room: &mut CaptureFixture, face: usize, brush: Option<u32>| {
        room.world.face_brushes[face] = brush;
        room.brush_faces = BrushFaces::new(&room.world.face_brushes, room.world.brushes.len());
    };
    for (brush, misses) in [(ceiling, true), (wall, false)] {
        let face = room.brush_faces.of(brush as u32)[0] as usize;
        retag(&mut room, face, None);
        assert_eq!(stage_sets(&room), sets);
        assert_eq!(
            misses,
            base != stage_key(&room, &production),
            "dropping face {face} of brush {brush}"
        );
        retag(&mut room, face, Some(brush as u32));
    }
    assert_eq!(base, stage_key(&room, &production));

    shifted(&mut room, wall, 1.0e-3);
    room.world.brushes[wall].planes[0].distance += 1.0e-3;
    assert_eq!(
        base,
        stage_key(&room, &production),
        "a brush the exclusion never reads"
    );
}

// ---------------------------------------------------------------------------
// Through the pipeline
// ---------------------------------------------------------------------------

/// A full prl-build pipeline run of `map_text` at `map` with a stage cache in
/// `root`, as prl-build runs it. Returns the cache handle the build used.
fn pipeline_build(root: &Path, map: &Path, map_text: &str) -> StageCache {
    std::fs::write(map, map_text).unwrap();
    let path = |name: &str| root.join(name).to_string_lossy().into_owned();
    let args = crate::parse_args_from(
        [
            map.to_string_lossy().into_owned(),
            "--cache-dir".to_owned(),
            path("cache"),
            "--baked-root".to_owned(),
            path("baked"),
            "-o".to_owned(),
            path("out.prl"),
        ]
        .into_iter(),
    )
    .expect("bake arguments parse");
    let cache = crate::construct_stage_cache(&args).expect("cache is enabled");
    let started = Instant::now();
    let reporter: Arc<dyn crate::reporter::Reporter> = Arc::new(
        crate::reporter::PlainReporter::new(started, crate::logger::LogSink::default()),
    );
    crate::pipeline::run(
        &args,
        Some(cache.clone()),
        started,
        reporter,
        Arc::new(crate::governor::Governor::new(1, false)),
    )
    .expect("fixture compiles");
    cache
}

/// The pillar room as map text: the pillar in a `func_group` carrying
/// `properties`, a light over the pool, and `pools` fluid entities.
fn pillar_map(properties: &str, pools: &[String]) -> String {
    let light = entity(
        "light",
        "\"origin\" \"0 96 112\"\n\"light\" \"300\"\n\"_falloff_range\" \"256\"\n\"_light_size\" \"0\"",
        &[],
    );
    let group = entity("func_group", properties, &[pillar(ROOM_MAX[2])]);
    let mut entities = vec![entity("worldspawn", "", &textured_room()), group, light];
    entities.extend(pools.iter().cloned());
    entities.join("\n") + "\n"
}

// Changing only a brush's `_reflection_capture` value hits every
// cached stage before the capture stage and misses the capture stage, though
// no brush moves; an unchanged rebuild then hits the capture too.
#[test]
fn an_override_only_edit_misses_only_the_capture_stage() {
    let root = tempfile::tempdir().unwrap();
    let map = root.path().join("exclusion.map");
    let auto = pillar_map("", &[pool_entity()]);
    let always = pillar_map("\"_reflection_capture\" \"always\"", &[pool_entity()]);
    pipeline_build(root.path(), &map, &auto);

    let edited = pipeline_build(root.path(), &map, &always);
    let accesses = edited.test_accesses_by_stage();
    let capture = accesses.get(STAGE_ID).copied().unwrap_or_default();
    assert_eq!(
        (capture.read_hits, capture.writes),
        (0, 1),
        "the capture stage misses"
    );
    assert!(
        accesses.len() > 1,
        "earlier stages are cached: {accesses:?}"
    );
    for (stage, access) in accesses.iter().filter(|(stage, _)| *stage != STAGE_ID) {
        assert_eq!(access.read_hits, access.read_attempts, "{stage} hits");
        assert_eq!(access.writes, 0, "{stage} re-bakes nothing");
    }

    let unchanged = pipeline_build(root.path(), &map, &always);
    let capture = unchanged.test_access(STAGE_ID);
    assert_eq!((capture.read_hits, capture.writes), (1, 0));
}

// An invalid value on a three-brush `func_group` warns
// exactly once, naming the entity, on a cold build, on a warm build whose
// capture stage hits, and in a map with no water.
#[test]
fn an_invalid_override_warns_once_cold_warm_and_without_water() {
    let decor = |n: f64| {
        let x = -100.0 + 24.0 * n;
        textured_box_brush([x, 104.0, 0.0], [x + 16.0, 120.0, 32.0], PILLAR_TEXTURE)
    };
    let map_text = |pools: &[String]| {
        let group = entity(
            "func_group",
            "\"_reflection_capture\" \"bogus\"",
            &[decor(0.0), decor(1.0), decor(2.0)],
        );
        let mut entities = vec![entity("worldspawn", "", &textured_room()), group];
        entities.extend(pools.iter().cloned());
        entities.join("\n") + "\n"
    };
    let pools = [
        super::capture_exclusion_test_fixtures::pool_between(-96.0, -16.0),
        super::capture_exclusion_test_fixtures::pool_between(16.0, 96.0),
    ];
    let warning = "entity 1 'func_group' `_reflection_capture` value `bogus` is not `auto`";
    let warnings = |logs: &LogCapture| {
        logs.records()
            .into_iter()
            .filter(|r| r.level == Level::Warn && r.message.contains("_reflection_capture"))
            .count()
    };

    let root = tempfile::tempdir().unwrap();
    let map = root.path().join("bogus.map");
    let logs = LogCapture::start();
    for build in ["cold", "warm"] {
        logs.clear();
        let cache = pipeline_build(root.path(), &map, &map_text(&pools));
        logs.assert_logged_once(Level::Warn, warning);
        assert_eq!(warnings(&logs), 1, "{build}");
        let capture = cache.test_access(STAGE_ID);
        let expected = if build == "cold" { (0, 1) } else { (1, 0) };
        assert_eq!((capture.read_hits, capture.writes), expected, "{build}");
    }

    let dry = tempfile::tempdir().unwrap();
    logs.clear();
    pipeline_build(dry.path(), &dry.path().join("dry.map"), &map_text(&[]));
    logs.assert_logged_once(Level::Warn, warning);
    assert_eq!(warnings(&logs), 1, "no water");
}
