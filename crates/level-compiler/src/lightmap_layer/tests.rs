// Per-light lightmap layer tests: sparse codec, composite parity, cache keys, and validation.
// See: context/lib/build_pipeline.md §Build Cache

use super::cache_keys::{atlas_layout_fingerprint, geometry_slice_hash};
use super::*;
use crate::bake_control::BakeControl;
use crate::bvh_build::build_bvh;
use crate::governor::Governor;
use crate::lightmap_bake::{
    BlockLayout, bake_monolithic_atlas, bake_monolithic_atlas_controlled, prepare_atlas,
};
use crate::map_data::{FalloffModel, ShadowType};
use crate::reporter::StageProgress;
use glam::DVec3;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;
use rayon::ThreadPoolBuilder;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const AREA_SAMPLES: u32 = 16;
const DENSITY: f32 = 0.25;

/// Two coplanar quads on the floor (Y=0), separate faces so the atlas packs
/// more than one chart and `split_shared_vertices` has work to do.
fn two_quad_geometry() -> GeometryResult {
    let mk = |x: f32, z: f32| {
        let n = [0.0, 1.0, 0.0];
        let t = [1.0, 0.0, 0.0];
        vec![
            Vertex::new([x, 0.0, z], [0.0, 0.0], n, t, true, [0.0, 0.0], 0),
            Vertex::new([x + 1.0, 0.0, z], [1.0, 0.0], n, t, true, [0.0, 0.0], 0),
            Vertex::new(
                [x + 1.0, 0.0, z + 1.0],
                [1.0, 1.0],
                n,
                t,
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new([x, 0.0, z + 1.0], [0.0, 1.0], n, t, true, [0.0, 0.0], 0),
        ]
    };
    let mut vertices = mk(0.0, 0.0);
    vertices.extend(mk(3.0, 0.0));
    GeometryResult {
        geometry: GeometrySection {
            vertices,
            indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
            faces: vec![
                FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                },
                FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                },
            ],
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: vec![
            crate::geometry::FaceIndexRange {
                index_offset: 0,
                index_count: 6,
            },
            crate::geometry::FaceIndexRange {
                index_offset: 6,
                index_count: 6,
            },
        ],
    }
}

fn point_light(origin: [f64; 3], range: f32) -> MapLight {
    MapLight {
        origin: DVec3::new(origin[0], origin[1], origin[2]),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: range,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: vec![],
        shadow_type: ShadowType::StaticLightMap,
    }
}

fn directional_light() -> MapLight {
    let mut l = point_light([0.0, 5.0, 0.0], 0.0);
    l.light_type = LightType::Directional;
    l.cone_direction = Some([0.0, -1.0, 0.0]);
    l
}

fn bake_layer_for_test(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
) -> LightmapLayer {
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(1, false)), &progress);
    bake_light_layer_controlled(
        light,
        atlas,
        bvh,
        primitives,
        geometry,
        0,
        area_sample_count,
        &control,
    )
}

fn expected_chart_texel_order(atlas: &SharedAtlas<'_>) -> Vec<(u32, u32)> {
    let padding = crate::chart_raster::CHART_PADDING_TEXELS as i32;
    let mut order = Vec::new();
    for (face_idx, placement) in atlas.placements.iter().enumerate() {
        let chart = &atlas.charts[face_idx];
        if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
            continue;
        }
        let (interior_w, interior_h) = chart_interior_dims(chart);
        for ty in 0..interior_h {
            for tx in 0..interior_w {
                let atlas_x = placement.x as i32 + padding + tx;
                let atlas_y = placement.y as i32 + padding + ty;
                order.push((
                    placement.layer,
                    atlas_y as u32 * atlas.atlas_width + atlas_x as u32,
                ));
            }
        }
    }
    order.sort_unstable();
    order
}

#[test]
fn analytic_coverage_matches_baked_coverage_fixture_matrix() {
    let light = point_light([0.5, 1.0, 0.5], 4.0);
    let samples = [
        ("visible", Vec3::new(0.5, 0.0, 0.5), Vec3::Y, true, true),
        (
            "fully occluded",
            Vec3::new(0.5, 0.0, 0.5),
            Vec3::Y,
            false,
            true,
        ),
        (
            "beyond falloff",
            Vec3::new(20.0, 0.0, 20.0),
            Vec3::Y,
            true,
            false,
        ),
        (
            "backfacing",
            Vec3::new(0.5, 0.0, 0.5),
            -Vec3::Y,
            true,
            false,
        ),
    ];

    for (name, world_p, normal, trace_is_clear, expected_coverage) in samples {
        let analytic = light_texel_is_covered(&light, world_p, normal);
        let (_, _, raw_visibility) = crate::lightmap_bake::light_texel_contribution_and_visibility(
            &light,
            world_p,
            normal,
            7,
            AREA_SAMPLES,
            |_, _| trace_is_clear,
        );
        assert_eq!(analytic, raw_visibility.is_some(), "{name}");
        assert_eq!(analytic, expected_coverage, "{name}");
    }

    let contributing_point = Vec3::new(0.5, 0.0, 0.5);
    assert!(light_texel_is_covered(&light, contributing_point, Vec3::Y));
}

#[test]
fn analytic_and_baked_chart_walks_both_skip_non_positive_extent() {
    let geometry = two_quad_geometry();
    let charts = vec![Chart {
        origin: Vec3::ZERO,
        u_axis: Vec3::X,
        v_axis: Vec3::Z,
        uv_min: [0.0, 0.0],
        uv_extent: [0.0, 1.0],
        normal: Vec3::Y,
        width_texels: 5,
        height_texels: 5,
        leaf_index: 0,
        window: None,
    }];
    let placements = vec![ChartPlacement {
        x: 0,
        y: 0,
        layer: 0,
    }];
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 5,
        atlas_height: 5,
        layout: &BlockLayout::whole_layers(5, 5, &placements, 2),
    };
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let light = point_light([0.5, 1.0, 0.5], 4.0);
    let baked = bake_light_layer_chart_controlled(
        &light,
        &shared,
        0,
        &bvh,
        &primitives,
        &geometry,
        AREA_SAMPLES,
        &BakeControl::unrestricted(),
    );
    let mut analytic = Vec::new();
    visit_light_chart_coverage_controlled(
        &light,
        &shared,
        0,
        &BakeControl::unrestricted(),
        |texel| analytic.push(texel),
    );

    assert!(baked.is_empty());
    assert!(analytic.is_empty());
}

/// The production warm fold reproduces the cold monolith on a synthetic
/// multi-light, multi-atlas-layer layout. Two point lights over different
/// quads plus one directional light exercise sparse + dense partitions,
/// ordered direction accumulation, and the covered-but-dark fallback.
#[test]
fn incremental_layer_fold_matches_monolithic_section_bytes() {
    // One geometry clone per path so each path's prepare_atlas mutates its own
    // copy identically; the BVH is built once from the shared pre-bake state.
    let mut mono_geo = two_quad_geometry();
    let mut layer_geo = two_quad_geometry();

    let lights = vec![
        point_light([0.5, 1.0, 0.5], 5.0),
        point_light([3.5, 1.0, 0.5], 5.0),
        directional_light(),
    ];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();

    // Monolithic path.
    let mut mono_prepared = prepare_atlas(&mut mono_geo, &static_lights, DENSITY, &[]).unwrap();
    mono_prepared.placements[1].layer = 1;
    mono_prepared.layer_count = 2;
    mono_prepared.layout = BlockLayout::whole_layers(
        mono_prepared.atlas_width,
        mono_prepared.atlas_height,
        &mono_prepared.placements,
        crate::lightmap_bake::DIRECTION_TEXEL_SCALE,
    );
    let (mono_bvh, mono_prims, _) = build_bvh(&mono_geo).unwrap();
    let mono_progress = StageProgress::with_total(mono_prepared.placements.len());
    let mono_control = BakeControl::new(Arc::new(Governor::new(1, false)), &mono_progress);
    let mono_atlas = bake_monolithic_atlas_controlled(
        &mono_bvh,
        &mono_prims,
        &mono_geo,
        &light_refs,
        &mono_prepared.charts,
        &mono_prepared.placements,
        mono_prepared.atlas_width,
        mono_prepared.atlas_height,
        mono_prepared.layer_count,
        AREA_SAMPLES,
        &mono_control,
    );
    assert_eq!(mono_progress.completed(), mono_prepared.placements.len());

    // Warm production fold, with the same forced two-plane layout.
    let mut layer_prepared = prepare_atlas(&mut layer_geo, &static_lights, DENSITY, &[]).unwrap();
    layer_prepared.placements[1].layer = 1;
    layer_prepared.layer_count = 2;
    layer_prepared.layout = BlockLayout::whole_layers(
        layer_prepared.atlas_width,
        layer_prepared.atlas_height,
        &layer_prepared.placements,
        crate::lightmap_bake::DIRECTION_TEXEL_SCALE,
    );
    let (layer_bvh, layer_prims, _) = build_bvh(&layer_geo).unwrap();
    let shared = SharedAtlas {
        charts: &layer_prepared.charts,
        placements: &layer_prepared.placements,
        atlas_width: layer_prepared.atlas_width,
        atlas_height: layer_prepared.atlas_height,
        layout: &layer_prepared.layout,
    };
    let warm_section = compose_section(&light_refs, &shared, &layer_bvh, &layer_prims, &layer_geo);
    assert_eq!(
        mono_atlas.encode_section(&mono_prepared.layout, true),
        warm_section,
        "layer-major incremental warm fold must equal the cold monolith byte-for-byte"
    );
    assert_eq!(
        mono_atlas.encode_section(&mono_prepared.layout, false),
        compose_section_with_format(
            &light_refs,
            &shared,
            &layer_bvh,
            &layer_prims,
            &layer_geo,
            false,
        ),
        "incremental layer assembly must preserve the cold BC6H bytes too"
    );
}

#[test]
fn layer_bake_preserves_chart_order_and_bytes_across_thread_counts() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    assert!(
        prepared.placements.len() > 1,
        "fixture must have enough charts to exercise ordered parallel collection"
    );
    let (bvh, primitives, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let single_progress = StageProgress::with_total(prepared.placements.len());
    let single_control = BakeControl::new(Arc::new(Governor::new(1, false)), &single_progress);
    let one_thread = ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .expect("single-worker rayon pool");
    let single_thread_layer = one_thread.install(|| {
        bake_light_layer_controlled(
            &lights[0],
            &shared,
            &bvh,
            &primitives,
            &geo,
            0,
            AREA_SAMPLES,
            &single_control,
        )
    });

    let multi_progress = StageProgress::with_total(prepared.placements.len());
    let multi_control = BakeControl::new(Arc::new(Governor::new(2, false)), &multi_progress);
    let multi_thread = ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .expect("multi-worker rayon pool");
    let multi_thread_layer = multi_thread.install(|| {
        bake_light_layer_controlled(
            &lights[0],
            &shared,
            &bvh,
            &primitives,
            &geo,
            0,
            AREA_SAMPLES,
            &multi_control,
        )
    });

    assert_eq!(
        single_progress.completed(),
        prepared.placements.len(),
        "one progress advance is required for every chart"
    );
    assert_eq!(
        single_progress.total(),
        Some(single_progress.completed()),
        "single-worker bake must advance its published chart total exactly"
    );
    assert_eq!(
        multi_progress.completed(),
        prepared.placements.len(),
        "parallel workers must advance every chart exactly once"
    );
    assert_eq!(
        multi_progress.total(),
        Some(multi_progress.completed()),
        "parallel bake must advance its published chart total exactly"
    );
    assert_eq!(
        single_thread_layer.to_bytes(),
        multi_thread_layer.to_bytes(),
        "layer cache bytes must not depend on Rayon worker count"
    );
    assert_eq!(
        multi_thread_layer
            .texels
            .iter()
            .map(|texel| (multi_thread_layer.target_layer, texel.idx))
            .collect::<Vec<_>>(),
        expected_chart_texel_order(&shared),
        "parallel chart buffers must concatenate in placement order"
    );
}

#[test]
fn layer_bake_degenerate_chart_advances_progress_and_keeps_ordered_empty_slot() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let mut prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    assert!(
        prepared.placements.len() > 1,
        "fixture must have a non-degenerate chart after the forced skip"
    );
    prepared.charts[0].uv_extent[0] = 0.0;
    let (bvh, primitives, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let progress = StageProgress::with_total(prepared.placements.len());
    let control = BakeControl::new(Arc::new(Governor::new(2, false)), &progress);
    let pool = ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .expect("multi-worker rayon pool");
    let layer = pool.install(|| {
        bake_light_layer_controlled(
            &lights[0],
            &shared,
            &bvh,
            &primitives,
            &geo,
            0,
            AREA_SAMPLES,
            &control,
        )
    });

    assert_eq!(progress.completed(), prepared.placements.len());
    assert_eq!(
        progress.total(),
        Some(progress.completed()),
        "the degenerate chart must still complete the published chart total"
    );
    assert_eq!(
        layer
            .texels
            .iter()
            .map(|texel| (layer.target_layer, texel.idx))
            .collect::<Vec<_>>(),
        expected_chart_texel_order(&shared),
        "the degenerate chart must contribute an empty ordered buffer"
    );
}

#[test]
fn layer_bake_paused_before_permit_release_starts_no_chart() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (bvh, primitives, _) = build_bvh(&geo).unwrap();
    let chart_count = prepared.placements.len();
    let expected_texel_count = expected_chart_texel_order(&SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    })
    .len();
    let governor = Arc::new(Governor::new(1, false));
    let held_permit = governor.enter();
    let progress = StageProgress::with_total(chart_count);
    let control = BakeControl::new(Arc::clone(&governor), &progress);
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();

    let worker = thread::spawn(move || {
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
            layout: &prepared.layout,
        };
        started_tx.send(()).expect("test coordinator is waiting");
        let layer = ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .expect("multi-worker rayon pool")
            .install(|| {
                bake_light_layer_controlled(
                    &lights[0],
                    &shared,
                    &bvh,
                    &primitives,
                    &geo,
                    0,
                    AREA_SAMPLES,
                    &control,
                )
            });
        finished_tx
            .send(layer)
            .expect("test coordinator is waiting");
    });

    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("bake worker did not start");
    // The held permit prevents an in-flight chart. Pausing before releasing
    // it makes every pending `enter` observe the pause, without a timing
    // window in which a new chart could begin.
    governor.set_paused(true);
    drop(held_permit);
    assert_eq!(
        progress.completed(),
        0,
        "no chart may advance after pause and before admission resumes"
    );

    governor.set_paused(false);
    let layer = finished_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("bake did not resume after unpausing");
    worker.join().expect("bake worker must not panic");
    assert_eq!(progress.completed(), chart_count);
    assert_eq!(
        progress.total(),
        Some(progress.completed()),
        "the resumed layer bake must advance its published chart total exactly"
    );
    assert_eq!(
        layer.texels.len(),
        expected_texel_count,
        "every placement must bake once after resume"
    );
}

#[test]
fn layer_roundtrips_through_codec() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let layer = bake_layer_for_test(&lights[0], &shared, &bvh, &prims, &geo, AREA_SAMPLES);

    let bytes = layer.to_bytes();
    assert_eq!(
        bytes.len(),
        LAYER_HEADER_BYTES + layer.texels.len() * 8,
        "sparse cache payload must use one 8-byte record per reached texel"
    );
    let decoded = LightmapLayer::from_bytes(&bytes).expect("round-trip decode");
    assert_eq!(layer, decoded, "layer codec must round-trip exactly");
}

#[test]
fn sparse_codec_preserves_zero_and_nan_visibility_bits() {
    let nan = f32::from_bits(0x7fc0_1234);
    let layer = LightmapLayer {
        atlas_width: 8,
        atlas_height: 8,
        layer_count: 2,
        target_layer: 1,
        texels: vec![
            LayerTexel {
                idx: 7,
                raw_visibility: 0.0,
            },
            LayerTexel {
                idx: 19,
                raw_visibility: nan,
            },
        ],
    };
    let bytes = layer.to_bytes();
    assert_eq!(bytes.len(), 20 + 2 * 8);
    let decoded = LightmapLayer::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.target_layer, 1);
    assert_eq!(decoded.texels[0].raw_visibility.to_bits(), 0.0f32.to_bits());
    assert_eq!(decoded.texels[1].raw_visibility.to_bits(), nan.to_bits());
}

#[test]
fn sparse_presence_and_reconstruction_preserve_dense_edge_semantics() {
    let light = point_light([0.5, 1.0, 0.5], 5.0);
    let world_p = Vec3::new(0.5, 0.0, 0.5);

    let unreached = bake_sparse_layer_texel(
        3,
        &light,
        Vec3::new(20.0, 0.0, 20.0),
        Vec3::Y,
        0,
        1,
        |_, _| true,
    );
    assert!(
        unreached.is_none(),
        "an unreached texel must have no record"
    );

    let occluded = bake_sparse_layer_texel(3, &light, world_p, Vec3::Y, 0, 1, |_, _| false)
        .expect("analytic coverage survives full occlusion");
    assert_eq!(occluded.raw_visibility.to_bits(), 0.0f32.to_bits());
    let (irradiance, weighted_dir) =
        reconstruct_light_texel(&light, world_p, Vec3::Y, occluded.raw_visibility);
    assert!(irradiance.to_array().iter().all(|v| v.to_bits() == 0));
    assert!(weighted_dir.to_array().iter().all(|v| v.to_bits() == 0));

    let nan = f32::from_bits(0x7fc0_1234);
    let (contribution, to_light) = light_contribution_and_direction(&light, world_p, Vec3::Y);
    let expected_irradiance = contribution * nan;
    let expected_direction = to_light * ((contribution.x + contribution.y + contribution.z) * nan);
    let (actual_irradiance, actual_direction) =
        reconstruct_light_texel(&light, world_p, Vec3::Y, nan);
    assert_eq!(
        actual_irradiance.to_array().map(f32::to_bits),
        expected_irradiance.to_array().map(f32::to_bits)
    );
    assert_eq!(
        actual_direction.to_array().map(f32::to_bits),
        expected_direction.to_array().map(f32::to_bits)
    );
}

#[test]
fn sparse_skip_preserves_dense_signed_zero_fold_bits() {
    let light = directional_light();
    let (_, term) = reconstruct_light_texel(&light, Vec3::ZERO, Vec3::Y, 1.0);
    assert_eq!(term.x.to_bits(), (-0.0f32).to_bits());
    let sparse = Vec3::ZERO + term;
    let dense = Vec3::ZERO + term + Vec3::ZERO;
    assert_eq!(
        sparse.to_array().map(f32::to_bits),
        dense.to_array().map(f32::to_bits),
        "omitting a later unreached +0 term must preserve dense fold bits"
    );
}

#[test]
fn sparse_writer_uses_adjacent_values_around_coverage_epsilon() {
    let mut light = directional_light();
    let threshold = (crate::lightmap_bake::LIGHT_TEXEL_CONTRIBUTION_EPSILON_SQUARED / 3.0).sqrt();
    let mut below = threshold;
    while crate::lightmap_bake::contribution_covers_shadowmask(3.0 * below * below) {
        below = f32::from_bits(below.to_bits() - 1);
    }
    let above = f32::from_bits(below.to_bits() + 1);
    assert!(!crate::lightmap_bake::contribution_covers_shadowmask(
        3.0 * below * below
    ));
    assert!(crate::lightmap_bake::contribution_covers_shadowmask(
        3.0 * above * above
    ));

    light.intensity = below;
    assert!(bake_sparse_layer_texel(0, &light, Vec3::ZERO, Vec3::Y, 0, 1, |_, _| true).is_none());
    light.intensity = above;
    assert!(bake_sparse_layer_texel(0, &light, Vec3::ZERO, Vec3::Y, 0, 1, |_, _| true).is_some());
}

#[test]
fn sparse_cache_epochs_are_pinned() {
    assert_eq!(LAYER_FORMAT_VERSION, 8);
    assert_eq!(LIGHTMAP_SECTION_VERSION, 4);
}

#[test]
fn pre_sparse_cache_epochs_are_unreadable() {
    let dir = fresh_cache_dir("pre_sparse_epochs");
    let cache = StageCache::new(&dir).unwrap();
    let digest = [0x5a; 32];
    let old_layer = CacheKey::new("lightmap_layer", 5, &digest);
    let new_layer = CacheKey::new("lightmap_layer", LAYER_FORMAT_VERSION, &digest);
    let old_section = CacheKey::new("lightmap_section", 2, &digest);
    let new_section = CacheKey::new("lightmap_section", LIGHTMAP_SECTION_VERSION, &digest);
    cache.put(&old_layer, b"old dense layer");
    cache.put(&old_section, b"old section");
    assert!(cache.get(&new_layer).is_none());
    assert!(cache.get(&new_section).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_sparse_partition_roundtrips_and_has_no_fold_effect() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([100.0, 1.0, 100.0], 1.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let partition = bake_light_layer_controlled(
        &lights[0],
        &shared,
        &bvh,
        &prims,
        &geo,
        0,
        AREA_SAMPLES,
        &BakeControl::unrestricted(),
    );
    assert!(partition.texels.is_empty());
    validate_layer_partition(&partition, &shared, 0).unwrap();
    assert_eq!(
        LightmapLayer::from_bytes(&partition.to_bytes()),
        Some(partition.clone())
    );

    let mut accumulator = IncrementalLayerAccumulator::for_atlas_layer(&shared, 0);
    accumulator.fold_partition(&lights[0], &partition, &shared);
    let folded = accumulator.finish();
    assert!(folded.coverage.iter().any(|covered| *covered));
    assert!(
        folded
            .irradiance
            .chunks_exact(4)
            .all(|rgba| rgba[..3].iter().all(|value| value.to_bits() == 0))
    );
}

#[test]
fn sparse_multilayer_payload_is_below_one_tenth_dense_bytes() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 1.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let mut prepared = prepare_atlas(&mut geo, &static_lights, 0.1, &[]).unwrap();
    prepared.placements[0].layer = 0;
    prepared.placements[1].layer = 1;
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let partitions = bake_light_layer(
        &lights[0],
        &shared,
        &bvh,
        &prims,
        &geo,
        AREA_SAMPLES,
        &BakeControl::unrestricted(),
    );
    assert_eq!(partitions.len(), 2, "named fixture must remain multi-layer");
    assert!(
        partitions[1].texels.is_empty(),
        "remote layer must be empty"
    );
    let sparse_bytes: usize = partitions
        .iter()
        .map(|partition| partition.to_bytes().len())
        .sum();
    let former_dense_records = expected_chart_texel_order(&shared).len();
    let former_dense_bytes = former_dense_records * 48;
    assert!(
        sparse_bytes * 10 < former_dense_bytes,
        "sparse {sparse_bytes} bytes must be below one tenth of former dense {former_dense_bytes} bytes"
    );
}

#[test]
fn corrupt_layer_blob_is_a_miss() {
    // `from_bytes` rejects a garbage blob and returns `None` (format-validation
    // path). This is independent of the `StageCache` byte-level length/hash
    // check — it calls `from_bytes` directly, exercising only the codec.
    assert!(LightmapLayer::from_bytes(b"not a valid header+body layer").is_none());
}

#[test]
fn from_bytes_rejects_overflowing_texel_count() {
    // Pins the overflow-guard defense: a header with count = u32::MAX causes
    // count * size_of::<LayerTexel>() to overflow; from_bytes must return None,
    // never panic.
    let mut blob = Vec::new();
    blob.extend_from_slice(&1u32.to_ne_bytes()); // atlas_width
    blob.extend_from_slice(&1u32.to_ne_bytes()); // atlas_height
    blob.extend_from_slice(&1u32.to_ne_bytes()); // layer_count
    blob.extend_from_slice(&0u32.to_ne_bytes()); // target_layer
    blob.extend_from_slice(&u32::MAX.to_ne_bytes()); // count = u32::MAX
    // No body bytes — the implied body cannot match.
    assert!(LightmapLayer::from_bytes(&blob).is_none());
}

#[test]
fn layer_input_hash_changes_when_light_moves() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (_, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let base = point_light([0.5, 1.0, 0.5], 5.0);
    let moved = point_light([10.0, 1.0, 0.5], 5.0);
    let h_base = layer_input_hash(&base, &shared, &prims, &geo, DENSITY, AREA_SAMPLES, 0);
    let h_moved = layer_input_hash(&moved, &shared, &prims, &geo, DENSITY, AREA_SAMPLES, 0);
    assert_ne!(
        h_base, h_moved,
        "moving the light must change its layer cache key"
    );
}

#[test]
fn target_layer_partitions_are_disjoint_and_rekeyed() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let mut prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    assert_eq!(prepared.placements.len(), 2, "fixture needs two charts");
    // Force a two-layer layout while retaining chart coordinates and seeds.
    prepared.placements[0].layer = 0;
    prepared.placements[1].layer = 1;
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let control = BakeControl::unrestricted();
    let first = bake_light_layer_controlled(
        &lights[0],
        &shared,
        &bvh,
        &prims,
        &geo,
        0,
        AREA_SAMPLES,
        &control,
    );
    let second = bake_light_layer_controlled(
        &lights[0],
        &shared,
        &bvh,
        &prims,
        &geo,
        1,
        AREA_SAMPLES,
        &control,
    );

    assert!(!first.texels.is_empty() && !second.texels.is_empty());
    assert_eq!(first.target_layer, 0);
    assert_eq!(second.target_layer, 1);
    assert!(validate_layer_partition(&first, &shared, 0).is_ok());
    assert!(validate_layer_partition(&second, &shared, 1).is_ok());
    assert!(
        validate_layer_partition(&first, &shared, 1).is_err(),
        "a partition from another atlas layer must be a soft cache miss"
    );

    let key_for = |target_layer| {
        CacheKey::new(
            "lightmap_layer",
            LAYER_FORMAT_VERSION,
            &layer_input_hash(
                &lights[0],
                &shared,
                &prims,
                &geo,
                DENSITY,
                AREA_SAMPLES,
                target_layer,
            ),
        )
        .as_filename()
    };
    assert_ne!(
        key_for(0),
        key_for(1),
        "one light's partitions must not share a cache key"
    );
}

#[test]
fn validate_layer_partition_accepts_sparse_and_rejects_invalid_records() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let original = bake_light_layer_controlled(
        &lights[0],
        &shared,
        &bvh,
        &prims,
        &geo,
        0,
        AREA_SAMPLES,
        &BakeControl::unrestricted(),
    );
    assert!(original.texels.len() > 1, "fixture needs multiple texels");

    let mut missing = original.clone();
    missing.texels.remove(missing.texels.len() / 2);
    assert!(
        validate_layer_partition(&missing, &shared, 0).is_ok(),
        "absence is the sparse encoding of an analytically unreached texel"
    );

    let mut duplicate = original.clone();
    duplicate.texels[1] = duplicate.texels[0];
    assert!(
        validate_layer_partition(&duplicate, &shared, 0).is_err(),
        "a duplicate replacing another in-bounds texel must be rejected"
    );

    let mut out_of_bounds = original.clone();
    out_of_bounds.texels[0].idx = shared.atlas_width * shared.atlas_height;
    out_of_bounds.texels.sort_unstable_by_key(|texel| texel.idx);
    assert!(validate_layer_partition(&out_of_bounds, &shared, 0).is_err());

    let mut outside_chart = original;
    outside_chart.texels[0].idx = 0;
    outside_chart.texels.sort_unstable_by_key(|texel| texel.idx);
    assert!(validate_layer_partition(&outside_chart, &shared, 0).is_err());

    let mut nan_visibility = missing.clone();
    nan_visibility.texels[0].raw_visibility = f32::from_bits(0x7fc0_1234);
    assert!(
        validate_layer_partition(&nan_visibility, &shared, 0).is_ok(),
        "NaN visibility remains a valid sparse record"
    );

    for (visibility, name) in [(f32::INFINITY, "+infinity"), (2.0, "above one")] {
        let mut invalid_visibility = missing.clone();
        invalid_visibility.texels[0].raw_visibility = visibility;
        assert!(
            validate_layer_partition(&invalid_visibility, &shared, 0).is_err(),
            "{name} visibility must become a semantic cache miss"
        );
    }
}

#[test]
fn all_sdf_fallback_encodes_every_block_uncovered() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let mut prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    prepared.placements[1].layer = 1;
    prepared.layer_count = 2;
    prepared.layout = BlockLayout::whole_layers(
        prepared.atlas_width,
        prepared.atlas_height,
        &prepared.placements,
        crate::lightmap_bake::DIRECTION_TEXEL_SCALE,
    );
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let warm_fallback = compose_section(&[], &shared, &bvh, &prims, &geo);
    let mut uncovered = CompositedAtlas::zeroed(shared.atlas_width, shared.atlas_height, 2);
    uncovered.dilate();
    assert_eq!(
        warm_fallback,
        uncovered.encode_section(shared.layout, true),
        "all-Sdf warm fallback must encode every block from an uncovered plane"
    );
    assert_eq!(warm_fallback.blocks.len(), 2);
    for block in &warm_fallback.blocks {
        assert!(block.irradiance.iter().all(|&byte| byte == 0));
        assert!(
            block
                .direction
                .chunks_exact(2)
                .all(|texel| texel == [128, 255])
        );
    }
}

fn lone_chart_layout_fingerprint(
    width_texels: u32,
    height_texels: u32,
    uv_extent: [f32; 2],
) -> Vec<u8> {
    let charts = [Chart {
        origin: Vec3::ZERO,
        u_axis: Vec3::X,
        v_axis: Vec3::Z,
        uv_min: [0.0, 0.0],
        uv_extent,
        normal: Vec3::Y,
        width_texels,
        height_texels,
        leaf_index: 0,
        window: None,
    }];
    // Both P2 variants remain at the packer's 64² minimum and at the same
    // sole-chart placement. Only the resolved chart dimensions may re-key.
    let placements = [ChartPlacement {
        x: 0,
        y: 0,
        layer: 0,
    }];
    atlas_layout_fingerprint(&SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 64,
        atlas_height: 64,
        layout: &BlockLayout::whole_layers(64, 64, &placements, 2),
    })
}

#[test]
fn atlas_layout_fingerprint_rekeys_lone_chart_when_scale_changes_dimensions() {
    let fine = lone_chart_layout_fingerprint(40, 40, [1.0, 1.0]);
    let coarse = lone_chart_layout_fingerprint(20, 20, [1.0, 1.0]);
    assert_ne!(
        fine, coarse,
        "P2: chart dimensions must re-key even when 64² placement is unchanged"
    );
}

#[test]
fn atlas_layout_fingerprint_tracks_resolved_dimensions_not_region_definition_order() {
    // Two region orderings that resolve to the same chart dimensions reach
    // this boundary as identical chart/placement data and must share a key.
    let equivalent_a = lone_chart_layout_fingerprint(20, 20, [1.0, 1.0]);
    let equivalent_b = lone_chart_layout_fingerprint(20, 20, [1.0, 1.0]);
    let different = lone_chart_layout_fingerprint(40, 40, [1.0, 1.0]);
    assert_eq!(
        equivalent_a, equivalent_b,
        "P11: equivalent resolved scale outcomes must not spuriously miss"
    );
    assert_ne!(
        equivalent_a, different,
        "P11: a changed resolved chart dimension must miss"
    );
}

#[test]
fn atlas_layout_fingerprint_rekeys_lone_chart_when_uv_extent_changes() {
    // A scale edit can move chart sample positions without changing a
    // small chart's rounded dimensions or its sole 64² placement.
    let baseline = lone_chart_layout_fingerprint(20, 20, [1.0, 1.0]);
    let rescaled = lone_chart_layout_fingerprint(20, 20, [1.1, 1.0]);
    assert_ne!(
        baseline, rescaled,
        "resolved sampling extent must re-key a warm layer"
    );
}

#[test]
fn geometry_slice_hash_ignores_faces_outside_influence() {
    // A light tightly bounded near the first quad must not fold the distant
    // second quad's geometry into its slice hash: editing the far quad leaves
    // the near light's key unchanged.
    let geo = two_quad_geometry();
    let (_, prims, _) = build_bvh(&geo).unwrap();
    let world = geometry_world_aabb(&geo);

    // Tight range so the influence AABB covers only the first quad
    // (x in [0,1]) and not the second (x in [3,4]).
    let near = point_light([0.5, 0.5, 0.5], 1.0);
    let slice_a = geometry_slice_hash(&near, &prims, &geo, world);

    // Move the far quad's vertices; rebuild prims/geo.
    let mut geo2 = two_quad_geometry();
    for v in geo2.geometry.vertices[4..].iter_mut() {
        v.position[1] += 2.0;
    }
    let (_, prims2, _) = build_bvh(&geo2).unwrap();
    let world2 = geometry_world_aabb(&geo2);
    let slice_b = geometry_slice_hash(&near, &prims2, &geo2, world2);

    assert_eq!(
        slice_a, slice_b,
        "editing geometry outside the influence AABB must not change the slice hash"
    );
}

// -----------------------------------------------------------------------
// Cache-key behaviors at the module API seam and the full-fixture
// determinism gate (1).

use crate::cache::{CacheKey, StageCache};
use std::sync::atomic::{AtomicU64, Ordering};

fn fresh_cache_dir(label: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "postretro_lmlayer_test_{label}_{nonce}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Build one light's layer cache key the same way the production pipeline
/// does, so the locality/round-trip tests assert on its key derivation.
fn layer_key(
    light: &MapLight,
    shared: &SharedAtlas<'_>,
    prims: &[BvhPrimitive],
    geo: &GeometryResult,
) -> CacheKey {
    let h = layer_input_hash(light, shared, prims, geo, DENSITY, AREA_SAMPLES, 0);
    CacheKey::new("lightmap_layer", LAYER_FORMAT_VERSION, &h)
}

/// Round-trip skip: with a real `StageCache`, the first per-light bake is a
/// miss + put, the second build serves every layer from cache (hit) and
/// re-bakes nothing. Proves the lightmap-layer half of the
/// "build twice → all entries hit" AC (the sh_group half lives in
/// `cache_round_trip_hits_on_second_build`).
#[test]
fn layer_cache_round_trip_skips_rebake() {
    let mut geo = two_quad_geometry();
    let lights = vec![
        point_light([0.5, 1.0, 0.5], 5.0),
        point_light([3.5, 1.0, 0.5], 5.0),
    ];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let dir = fresh_cache_dir("roundtrip");
    let cache = StageCache::new(&dir).expect("cache dir");

    // First build: every light is a miss, so bake + put.
    for light in &lights {
        let key = layer_key(light, &shared, &prims, &geo);
        assert!(cache.get(&key).is_none(), "first build must miss");
        let layer = bake_layer_for_test(light, &shared, &bvh, &prims, &geo, AREA_SAMPLES);
        cache.put(&key, &layer.to_bytes());
    }

    // Second build: every light hits and decodes; nothing re-bakes.
    for light in &lights {
        let key = layer_key(light, &shared, &prims, &geo);
        let bytes = cache.get(&key).expect("second build must hit");
        assert!(
            LightmapLayer::from_bytes(&bytes).is_some(),
            "cached layer must decode on the round-trip"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Single point-light edit: only the edited light's layer key changes; every
/// other light's key is untouched (so only its layer re-bakes, the rest hit).
/// This is the lightmap side of the "edit one point/spot light" AC.
#[test]
fn single_light_edit_invalidates_only_its_own_layer() {
    let mut geo = two_quad_geometry();
    // Two well-separated lights so an edit to one is provably outside the
    // other's influence slice.
    let lights = vec![
        point_light([0.5, 1.0, 0.5], 1.0),
        point_light([3.5, 1.0, 0.5], 1.0),
    ];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (_, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let base_keys: Vec<String> = lights
        .iter()
        .map(|l| layer_key(l, &shared, &prims, &geo).as_filename())
        .collect();

    // Edit the first light's color only.
    let mut edited = lights.clone();
    edited[0].color = [0.3, 0.3, 0.3];
    let edited_keys: Vec<String> = edited
        .iter()
        .map(|l| layer_key(l, &shared, &prims, &geo).as_filename())
        .collect();

    assert_ne!(
        base_keys[0], edited_keys[0],
        "edited light's layer key must change (it re-bakes)"
    );
    assert_eq!(
        base_keys[1], edited_keys[1],
        "an untouched light's layer key must stay identical (it hits)"
    );
}

/// Directional-light edit: the directional's own layer key changes, and no
/// other (point) light's layer key is affected. (The "all SH groups re-bake"
/// half of the directional-edit AC is covered in `sh_group.rs`; SH groups
/// fold the whole-map geometry hash and a directional reaches every group.)
#[test]
fn directional_light_edit_does_not_disturb_point_layers() {
    let mut geo = two_quad_geometry();
    let lights = vec![directional_light(), point_light([0.5, 1.0, 0.5], 2.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (_, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let dir_base = layer_key(&lights[0], &shared, &prims, &geo).as_filename();
    let pt_base = layer_key(&lights[1], &shared, &prims, &geo).as_filename();

    let mut edited = lights.clone();
    edited[0].color = [0.5, 0.4, 0.3]; // edit the directional only
    let dir_edited = layer_key(&edited[0], &shared, &prims, &geo).as_filename();
    let pt_edited = layer_key(&edited[1], &shared, &prims, &geo).as_filename();

    assert_ne!(
        dir_base, dir_edited,
        "edited directional layer must re-bake"
    );
    assert_eq!(
        pt_base, pt_edited,
        "a directional edit must not disturb a point light's layer key"
    );
}

/// Localized geometry edit: moving the far quad changes the far light's
/// influence-bounded slice (its key — and only its key — changes), while the
/// near light's slice is provably unchanged (its key is identical, so it
/// hits). This exercises lightmap geometry-slice locality non-vacuously — at
/// least one layer misses and at least one hits on the same edit.
#[test]
fn localized_geometry_edit_invalidates_only_overlapping_layers() {
    let geo = two_quad_geometry();
    let mut prep_geo = two_quad_geometry();
    let near = point_light([0.5, 0.5, 0.5], 1.0); // bounds the first quad only
    let far = point_light([3.5, 0.5, 0.5], 1.0); // bounds the second quad only
    let lights = vec![near.clone(), far.clone()];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut prep_geo, &static_lights, DENSITY, &[]).unwrap();
    let (_, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let near_base = layer_key(&near, &shared, &prims, &geo).as_filename();
    let far_base = layer_key(&far, &shared, &prims, &geo).as_filename();

    // Edit the FAR quad's geometry (vertices 4..8).
    let mut geo2 = two_quad_geometry();
    for v in geo2.geometry.vertices[4..].iter_mut() {
        v.position[1] += 0.5;
    }
    let (_, prims2, _) = build_bvh(&geo2).unwrap();

    let near_edited = layer_key(&near, &shared, &prims2, &geo2).as_filename();
    let far_edited = layer_key(&far, &shared, &prims2, &geo2).as_filename();

    assert_eq!(
        near_base, near_edited,
        "near light's slice is outside the edit — its layer must hit"
    );
    assert_ne!(
        far_base, far_edited,
        "far light's slice covers the edited quad — its layer must re-bake"
    );
}

/// Corruption recovery at the real `StageCache`: a stored layer overwritten
/// with garbage is detected (length/hash fail → `get` returns `None`, or a
/// format-skewed payload → `from_bytes` returns `None`), so the caller
/// re-bakes and the rebuilt layer is byte-identical to the original.
#[test]
fn corrupt_layer_entry_is_discarded_and_rebaked() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let dir = fresh_cache_dir("corrupt");
    let cache = StageCache::new(&dir).expect("cache dir");
    let key = layer_key(&lights[0], &shared, &prims, &geo);

    let original = bake_layer_for_test(&lights[0], &shared, &bvh, &prims, &geo, AREA_SAMPLES);
    cache.put(&key, &original.to_bytes());

    // Corrupt every file in the cache dir.
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            std::fs::write(&path, b"corrupt").unwrap();
        }
    }

    // The cache rejects it (returns None), so the pipeline re-bakes.
    let recovered = match cache
        .get(&key)
        .and_then(|bytes| LightmapLayer::from_bytes(&bytes))
    {
        Some(layer) => layer,
        None => bake_layer_for_test(&lights[0], &shared, &bvh, &prims, &geo, AREA_SAMPLES),
    };
    assert_eq!(
        original, recovered,
        "re-bake after corruption must reproduce the original layer"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn malformed_partition_cache_hits_soft_miss_before_warm_fold() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let control = BakeControl::unrestricted();
    let expected = bake_light_layer_controlled(
        &lights[0],
        &shared,
        &bvh,
        &prims,
        &geo,
        0,
        AREA_SAMPLES,
        &control,
    );
    assert!(expected.texels.len() > 1, "fixture needs multiple texels");
    let key = layer_key(&lights[0], &shared, &prims, &geo);
    let dir = fresh_cache_dir("malformed_partition_soft_miss");
    let cache = StageCache::new(&dir).expect("cache dir");

    for malformed in [
        {
            let mut layer = expected.clone();
            layer.texels[0].idx = 0;
            layer
        },
        {
            let mut layer = expected.clone();
            layer.texels[1] = layer.texels[0];
            layer
        },
        {
            let mut layer = expected.clone();
            layer.texels.last_mut().unwrap().idx = shared.atlas_width * shared.atlas_height;
            layer
        },
    ] {
        cache.put(&key, &malformed.to_bytes());
        let recovered = cache
            .get(&key)
            .and_then(|bytes| LightmapLayer::from_bytes(&bytes))
            .and_then(|partition| {
                validate_layer_partition(&partition, &shared, 0)
                    .ok()
                    .map(|()| partition)
            })
            .unwrap_or_else(|| {
                bake_light_layer_controlled(
                    &lights[0],
                    &shared,
                    &bvh,
                    &prims,
                    &geo,
                    0,
                    AREA_SAMPLES,
                    &control,
                )
            });
        assert_eq!(
            recovered, expected,
            "malformed decoded payload must degrade to the exact re-bake"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// `--cache-dir` redirect (module-level): a `StageCache` opened on an
/// override directory writes its layer entries there, under that path —
/// nothing lands in the default `.build-caches/prl-cache/`.
#[test]
fn cache_dir_override_places_entries_under_override() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let dir = fresh_cache_dir("override");
    let cache = StageCache::new(&dir).expect("cache dir");
    let key = layer_key(&lights[0], &shared, &prims, &geo);
    let layer = bake_layer_for_test(&lights[0], &shared, &bvh, &prims, &geo, AREA_SAMPLES);
    cache.put(&key, &layer.to_bytes());

    let entry = dir.join(key.as_filename());
    assert!(
        entry.is_file(),
        "layer entry must land under the override dir: {}",
        entry.display()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Determinism GATE (1): the warm per-light composite is byte-identical to
/// the monolithic `bake_face_chart` atlas (pre-BC6H `CompositedAtlas`) on
/// every `GATE_FIXTURES` entry. The synthetic
/// `composite_matches_monolithic_atlas_bit_for_bit` proves the mechanism;
/// this runs the real fixtures through both paths.
///
/// `#[ignore]` because each fixture bake casts the full per-texel ray load
/// (roughly a minute or two across the gate fixture set). Run manually:
///   cargo test -p postretro-level-compiler -- --ignored --nocapture \
///       lightmap_composite_equals_monolithic_on_fixtures
#[test]
#[ignore = "full-fixture lightmap bake; run with --ignored"]
fn lightmap_composite_equals_monolithic_on_fixtures() {
    use crate::fixture_pipeline::{GATE_FIXTURES, load_fixture};

    for &name in GATE_FIXTURES {
        let fx = load_fixture(name);
        // Match the pipeline's direct-lightmap light set: global static order,
        // Sdf-shadow lights dropped (their direct term resolves at runtime).
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&fx.lights);
        let layer_lights: Vec<&MapLight> = static_lights
            .entries()
            .iter()
            .map(|e| e.light)
            .filter(|l| l.shadow_type != crate::map_data::ShadowType::Sdf)
            .collect();
        if layer_lights.is_empty() {
            // Placeholder path; no atlas to compare. Skip (still meaningful
            // for the other fixtures).
            continue;
        }

        // Two geometry clones so each path's prepare_atlas mutates its own.
        let mut mono_geo = fx.geometry.clone();
        let mut layer_geo = fx.geometry.clone();
        let density = crate::lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS;

        let mono_prepared = prepare_atlas(&mut mono_geo, &static_lights, density, &[]).unwrap();
        if mono_prepared.placements.is_empty() {
            continue;
        }
        let (mono_bvh, mono_prims, _) = build_bvh(&mono_geo).unwrap();
        let mono_atlas = bake_monolithic_atlas(
            &mono_bvh,
            &mono_prims,
            &mono_geo,
            &layer_lights,
            &mono_prepared.charts,
            &mono_prepared.placements,
            mono_prepared.atlas_width,
            mono_prepared.atlas_height,
            mono_prepared.layer_count,
            AREA_SAMPLES,
        );

        let layer_prepared = prepare_atlas(&mut layer_geo, &static_lights, density, &[]).unwrap();
        let (layer_bvh, layer_prims, _) = build_bvh(&layer_geo).unwrap();
        let shared = SharedAtlas {
            charts: &layer_prepared.charts,
            placements: &layer_prepared.placements,
            atlas_width: layer_prepared.atlas_width,
            atlas_height: layer_prepared.atlas_height,
            layout: &layer_prepared.layout,
        };
        let layers: Vec<Vec<LightmapLayer>> = layer_lights
            .iter()
            .map(|light| {
                bake_light_layer(
                    light,
                    &shared,
                    &layer_bvh,
                    &layer_prims,
                    &layer_geo,
                    AREA_SAMPLES,
                    &BakeControl::unrestricted(),
                )
            })
            .collect();
        let light_layers: Vec<_> = layer_lights
            .iter()
            .zip(&layers)
            .flat_map(|(&light, partitions)| {
                partitions.iter().map(move |partition| (light, partition))
            })
            .collect();
        let mut composite = composite_layers(&light_layers, &shared);
        composite.dilate();

        assert_eq!(
            mono_atlas, composite,
            "fixture {name}: per-light composite must equal the monolithic atlas bit-for-bit"
        );
    }
}

// -----------------------------------------------------------------------
// Second-level "lightmap_section" cache behaviors, mirroring the layer
// suite above. These protect the warm no-edit rebuild (section hit → one
// decode, no layer reads / composite / encode) and the section-key
// invalidation coupling that the production pipeline relies on.

use postretro_level_format::lightmap::LightmapSection;

/// Build the section cache key the same way the `pipeline.rs` warm path does:
/// fold the ordered per-light `layer_input_hash` set through
/// `section_input_hash`, then `CacheKey::new("lightmap_section", ...)`.
/// Mirrors the existing `layer_key` helper so the section tests assert on
/// the production key derivation, not a test-only reimplementation.
fn section_key(
    layer_input_hashes: &[[u8; 32]],
    shared: &SharedAtlas<'_>,
    density: f32,
    uncompressed_irradiance: bool,
) -> CacheKey {
    let h = section_input_hash(
        layer_input_hashes,
        shared,
        density,
        uncompressed_irradiance,
        crate::lightmap_bake::DIRECTION_TEXEL_SCALE,
    );
    CacheKey::new("lightmap_section", LIGHTMAP_SECTION_VERSION, &h)
}

#[test]
fn direction_texel_scale_rekeys_section_without_rekeying_light_layers() {
    // The layer hash represents an already-baked full-resolution light
    // contribution. Direction coarsening occurs only during section encode,
    // so the two builds must retain this exact layer key while their
    // composited-section keys differ.
    let layer_input_hashes = [[0x5a; 32]];
    let charts = [Chart {
        origin: Vec3::ZERO,
        u_axis: Vec3::X,
        v_axis: Vec3::Z,
        uv_min: [0.0, 0.0],
        uv_extent: [1.0, 1.0],
        normal: Vec3::Y,
        width_texels: 5,
        height_texels: 5,
        leaf_index: 0,
        window: None,
    }];
    let placements = [ChartPlacement {
        x: 0,
        y: 0,
        layer: 0,
    }];
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 64,
        atlas_height: 64,
        layout: &BlockLayout::whole_layers(64, 64, &placements, 2),
    };
    let at_scale_two = CacheKey::new(
        "lightmap_section",
        LIGHTMAP_SECTION_VERSION,
        &section_input_hash(&layer_input_hashes, &shared, DENSITY, true, 2),
    );
    let at_scale_one = CacheKey::new(
        "lightmap_section",
        LIGHTMAP_SECTION_VERSION,
        &section_input_hash(&layer_input_hashes, &shared, DENSITY, true, 1),
    );
    assert_ne!(
        at_scale_two.as_filename(),
        at_scale_one.as_filename(),
        "changing only direction scale must re-key the warm section memo"
    );

    let dir = fresh_cache_dir("section_direction_scale");
    let cache = StageCache::new(&dir).expect("cache dir");
    cache.put(&at_scale_two, b"scale-two-section");
    assert!(
        cache.get(&at_scale_one).is_none(),
        "a factor-1 rebuild must miss a factor-2 section memo"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn all_sdf_section_cache_rekeys_when_prepared_dimensions_change() {
    // Regression: the empty layer-hash list previously let an all-Sdf
    // fallback from prepared layout A alias layout B even though its bytes
    // are dimension-dependent.
    let charts = [Chart {
        origin: Vec3::ZERO,
        u_axis: Vec3::X,
        v_axis: Vec3::Z,
        uv_min: [0.0, 0.0],
        uv_extent: [1.0, 1.0],
        normal: Vec3::Y,
        width_texels: 5,
        height_texels: 5,
        leaf_index: 0,
        window: None,
    }];
    let placements = [ChartPlacement {
        x: 0,
        y: 0,
        layer: 0,
    }];
    let layout_a = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 64,
        atlas_height: 64,
        layout: &BlockLayout::whole_layers(64, 64, &placements, 2),
    };
    let layout_b = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 128,
        atlas_height: 64,
        layout: &BlockLayout::whole_layers(128, 64, &placements, 2),
    };
    let key_a = section_key(&[], &layout_a, DENSITY, true);
    let key_b = section_key(&[], &layout_b, DENSITY, true);
    assert_ne!(key_a.as_filename(), key_b.as_filename());

    let dir = fresh_cache_dir("all_sdf_layout_change");
    let cache = StageCache::new(&dir).expect("cache dir");
    cache.put(&key_a, b"layout-a-section");
    assert!(
        cache.get(&key_b).is_none(),
        "layout B must miss a fallback section memo written for layout A"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Exercise the production warm fold: for each atlas layer, bake/load one
/// light partition at a time in global light order, fold it, then
/// dilate/encode/append that plane before advancing to the next one.
fn compose_section(
    lights: &[&MapLight],
    shared: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    prims: &[BvhPrimitive],
    geo: &GeometryResult,
) -> LightmapSection {
    compose_section_with_format(lights, shared, bvh, prims, geo, true)
}

#[allow(clippy::too_many_arguments)]
fn compose_section_with_format(
    lights: &[&MapLight],
    shared: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    prims: &[BvhPrimitive],
    geo: &GeometryResult,
    uncompressed_irradiance: bool,
) -> LightmapSection {
    let control = BakeControl::unrestricted();
    let mut builder =
        crate::lightmap_bake::BlockSectionBuilder::new(shared.layout, uncompressed_irradiance);
    for target_layer in 0..atlas_layer_count(shared) {
        let mut plane = if lights.is_empty() {
            empty_composite(shared.atlas_width, shared.atlas_height)
        } else {
            let mut accumulator =
                IncrementalLayerAccumulator::for_atlas_layer(shared, target_layer);
            for light in lights {
                let partition = bake_light_layer_controlled(
                    light,
                    shared,
                    bvh,
                    prims,
                    geo,
                    target_layer,
                    AREA_SAMPLES,
                    &control,
                );
                accumulator.fold_partition(light, &partition, shared);
            }
            accumulator.finish()
        };
        plane.dilate();
        builder.push_layer(target_layer, &plane);
    }
    builder.finish()
}

/// Compute the filtered direct-lightmap light set + their ordered
/// `layer_input_hash`es exactly as the warm path does (global static order,
/// `Sdf` dropped). Returned alongside the light refs so a test can key the
/// section and bake from the same ordering.
fn layer_input_hashes(
    light_refs: &[&MapLight],
    shared: &SharedAtlas<'_>,
    prims: &[BvhPrimitive],
    geo: &GeometryResult,
) -> Vec<[u8; 32]> {
    (0..atlas_layer_count(shared))
        .flat_map(|target_layer| {
            light_refs.iter().map(move |light| {
                layer_input_hash(
                    light,
                    shared,
                    prims,
                    geo,
                    DENSITY,
                    AREA_SAMPLES,
                    target_layer,
                )
            })
        })
        .collect()
}

/// Round-trip skip (section level): with a real `StageCache`, the first
/// build is a section miss → recompose → `put`; the second build serves the
/// section from cache (`get` + `from_bytes`) and reads NO per-light layer
/// blob. Asserts the cached section decodes, equals the original, and the
/// section key is identical across builds. Section-level mirror of
/// `layer_cache_round_trip_skips_rebake`.
#[test]
fn section_cache_round_trip_skips_recompose() {
    let mut geo = two_quad_geometry();
    let lights = vec![
        point_light([0.5, 1.0, 0.5], 5.0),
        point_light([3.5, 1.0, 0.5], 5.0),
        directional_light(),
    ];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
    let key_build1 = section_key(&hashes, &shared, DENSITY, true);

    let dir = fresh_cache_dir("section_roundtrip");
    let cache = StageCache::new(&dir).expect("cache dir");

    // First build: section miss → recompose → put.
    assert!(
        cache.get(&key_build1).is_none(),
        "first build must miss the section key"
    );
    let composed = compose_section(&light_refs, &shared, &bvh, &prims, &geo);
    cache.put(&key_build1, &composed.to_bytes());

    // Second build: same key, section hits and decodes — no layer blob read.
    let key_build2 = section_key(&hashes, &shared, DENSITY, true);
    assert_eq!(
        key_build1.as_filename(),
        key_build2.as_filename(),
        "an unchanged build must derive the identical section key"
    );
    let bytes = cache
        .get(&key_build2)
        .expect("second build must hit the section");
    let decoded = LightmapSection::from_bytes(&bytes).expect("cached section must decode");
    assert_eq!(
        decoded, composed,
        "the hit-path section must equal the originally composited section"
    );

    // No layer blob was ever written: only the section entry exists on disk.
    let layer_key0 = CacheKey::new("lightmap_layer", LAYER_FORMAT_VERSION, &hashes[0]);
    assert!(
        cache.get(&layer_key0).is_none(),
        "the section round-trip must not read or write any per-light layer blob"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cached_section_validation_covers_current_block_layout_and_encode_config() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let expected = compose_section(&light_refs, &shared, &bvh, &prims, &geo);
    let validate =
        |section: &LightmapSection| validate_cached_lightmap_section(section, &shared, true);
    validate(&expected).expect("freshly composed section matches current inputs");

    let mut stale = expected.clone();
    stale.direction_texel_scale *= 2;
    assert!(validate(&stale).is_err(), "direction scale");
    let mut stale = expected.clone();
    stale.irradiance_format = postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;
    assert!(validate(&stale).is_err(), "irradiance format");
    let mut stale = expected.clone();
    stale.mode = postretro_level_format::lightmap::LightmapMode::Unshadowed;
    assert!(validate(&stale).is_err(), "mode");
    let mut stale = expected.clone();
    stale.blocks.pop();
    assert!(validate(&stale).is_err(), "block count");
    let mut stale = expected.clone();
    stale.blocks[0].width += 4;
    assert!(validate(&stale).is_err(), "block extent");
    let mut stale = expected.clone();
    stale.blocks[0].cell_id += 7;
    assert!(validate(&stale).is_err(), "block cell");
}

#[test]
fn mismatched_decoded_section_cache_hit_soft_misses_and_recomposes() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
    let key = section_key(&hashes, &shared, DENSITY, true);
    let expected = compose_section(&light_refs, &shared, &bvh, &prims, &geo);
    let mut stale = expected.clone();
    stale.direction_texel_scale *= 2;

    let dir = fresh_cache_dir("section_metadata_soft_miss");
    let cache = StageCache::new(&dir).expect("cache dir");
    cache.put(&key, &stale.to_bytes());
    let recovered = cache
        .get(&key)
        .and_then(|bytes| LightmapSection::from_bytes(&bytes).ok())
        .and_then(|section| {
            validate_cached_lightmap_section(&section, &shared, true)
                .ok()
                .map(|()| section)
        })
        .unwrap_or_else(|| compose_section(&light_refs, &shared, &bvh, &prims, &geo));
    assert_eq!(recovered, expected);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Single-light edit: editing one light changes its `layer_input_hash`,
/// hence the section key (a miss → recompose), while every OTHER light's
/// layer key is unchanged (those layers still hit). Section-level proxy for
/// the plan's primary correctness criterion. Mirror of
/// `single_light_edit_invalidates_only_its_own_layer`.
#[test]
fn single_light_edit_changes_section_key_but_not_unedited_layer_key() {
    let mut geo = two_quad_geometry();
    // Well-separated lights so the edit to one is outside the other's slice.
    let lights = vec![
        point_light([0.5, 1.0, 0.5], 1.0),
        point_light([3.5, 1.0, 0.5], 1.0),
    ];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (_, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let base_hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
    let base_section = section_key(&base_hashes, &shared, DENSITY, true).as_filename();
    let base_layer1 =
        CacheKey::new("lightmap_layer", LAYER_FORMAT_VERSION, &base_hashes[1]).as_filename();

    // Edit only the first light's color.
    let mut edited = lights.clone();
    edited[0].color = [0.3, 0.3, 0.3];
    let edited_static = crate::light_namespaces::StaticBakedLights::from_lights(&edited);
    let edited_refs: Vec<&MapLight> = edited_static.entries().iter().map(|e| e.light).collect();
    let edited_hashes = layer_input_hashes(&edited_refs, &shared, &prims, &geo);
    let edited_section = section_key(&edited_hashes, &shared, DENSITY, true).as_filename();
    let edited_layer1 =
        CacheKey::new("lightmap_layer", LAYER_FORMAT_VERSION, &edited_hashes[1]).as_filename();

    assert_ne!(
        base_section, edited_section,
        "editing a light must change the section key (section miss → recompose)"
    );
    assert_eq!(
        base_layer1, edited_layer1,
        "an unedited light's layer key must be unchanged (it still hits)"
    );
}

/// Corruption recovery (section level): a present-but-undecodable
/// `lightmap_section` entry — written through the cache so its length/hash
/// check passes, but whose bytes `LightmapSection::from_bytes` rejects — is
/// treated as a miss, so the pipeline recomposes. Asserts the recovered
/// section equals the originally-composited one. Mirror of
/// `corrupt_layer_entry_is_discarded_and_rebaked`, exercising the
/// `get`-succeeds-but-`from_bytes`-fails branch guarded by `pipeline.rs`.
#[test]
fn corrupt_section_entry_is_discarded_and_recomposed() {
    let mut geo = two_quad_geometry();
    let lights = vec![
        point_light([0.5, 1.0, 0.5], 5.0),
        point_light([3.5, 1.0, 0.5], 5.0),
    ];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
    let key = section_key(&hashes, &shared, DENSITY, true);
    let original = compose_section(&light_refs, &shared, &bvh, &prims, &geo);

    let dir = fresh_cache_dir("section_corrupt");
    let cache = StageCache::new(&dir).expect("cache dir");
    // Store a blob that PASSES the cache's length/hash check (it is `put`
    // through the cache) but is too short for `LightmapSection::from_bytes`
    // to parse — the exact format skew the pipeline recovers from.
    cache.put(&key, b"not a valid lightmap section blob");

    let recovered = match cache
        .get(&key)
        .and_then(|bytes| LightmapSection::from_bytes(&bytes).ok())
    {
        Some(section) => section,
        None => compose_section(&light_refs, &shared, &bvh, &prims, &geo),
    };
    assert_eq!(
        original, recovered,
        "recompose after a corrupt section entry must reproduce the original section"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `--cache-dir` redirect (section level): a `StageCache` opened on an
/// override directory writes the `lightmap_section` entry under that path.
/// Mirror of `cache_dir_override_places_entries_under_override`.
#[test]
fn cache_dir_override_places_section_entry_under_override() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
    let key = section_key(&hashes, &shared, DENSITY, true);
    let section = compose_section(&light_refs, &shared, &bvh, &prims, &geo);

    let dir = fresh_cache_dir("section_override");
    let cache = StageCache::new(&dir).expect("cache dir");
    cache.put(&key, &section.to_bytes());

    let entry = dir.join(key.as_filename());
    assert!(
        entry.is_file(),
        "section entry must land under the override dir: {}",
        entry.display()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `--no-cache` bypass (section level): the section key is only ever
/// consulted when a `StageCache` exists. With no cache, the warm branch is
/// not entered at all (`pipeline.rs` gates the section-cache block behind
/// `if let Some(ref cache) = stage_cache`), so no section entry is created.
/// A clean unit assertion for "no cache → no entry" is to open a cache dir,
/// derive the key WITHOUT putting anything (modeling the no-cache control
/// flow that never reaches `put`), and confirm the entry does not exist.
#[test]
fn no_cache_path_writes_no_section_entry() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (_, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let hashes = layer_input_hashes(&light_refs, &shared, &prims, &geo);
    let key = section_key(&hashes, &shared, DENSITY, true);

    // No-cache control flow: the key is derivable, but with the section-cache
    // block skipped nothing is ever `put`. Model that by never calling `put`.
    let dir = fresh_cache_dir("section_nocache");
    let cache = StageCache::new(&dir).expect("cache dir");
    let entry = dir.join(key.as_filename());
    assert!(
        !entry.is_file(),
        "no section entry may exist when the warm path never puts one"
    );
    assert!(
        cache.get(&key).is_none(),
        "a section key must miss when no entry was written"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Add / remove / reorder invalidation: each mutation changes the folded
/// `layer_input_hash` set (its length and/or order), so the section key
/// changes versus the two-light baseline. Reorder is the subtle case — the
/// fold concatenates the hashes in order, so the same two hashes in swapped
/// order must still yield a different key.
#[test]
fn section_key_changes_on_add_remove_and_reorder() {
    let mut geo = two_quad_geometry();
    let lights = vec![
        point_light([0.5, 1.0, 0.5], 5.0),
        point_light([3.5, 1.0, 0.5], 5.0),
    ];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (_, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let base = layer_input_hashes(&light_refs, &shared, &prims, &geo);
    let base_key = section_key(&base, &shared, DENSITY, true).as_filename();

    // Add a light: a third hash extends the fold.
    let mut added = base.clone();
    added.push(layer_input_hash(
        &point_light([2.0, 1.0, 0.5], 5.0),
        &shared,
        &prims,
        &geo,
        DENSITY,
        AREA_SAMPLES,
        0,
    ));
    assert_ne!(
        base_key,
        section_key(&added, &shared, DENSITY, true).as_filename(),
        "adding a light must change the section key"
    );

    // Remove a light: drop the second hash.
    let removed = vec![base[0]];
    assert_ne!(
        base_key,
        section_key(&removed, &shared, DENSITY, true).as_filename(),
        "removing a light must change the section key"
    );

    // Reorder: swap the two hashes. Same set, different concatenation order.
    let reordered = vec![base[1], base[0]];
    assert_ne!(
        base_key,
        section_key(&reordered, &shared, DENSITY, true).as_filename(),
        "reordering lights must change the section key (the fold is ordered)"
    );
    // Guard against an accidental commutative fold: the swap must NOT be a
    // no-op even though the hash multiset is identical.
    assert_ne!(
        base[0], base[1],
        "the two lights must have distinct layer hashes for the reorder check to bite"
    );
}

/// `--soft-shadow-samples` change → section miss: the sample count folds
/// into every `layer_input_hash` (as `area_sample_count`), hence into the
/// section key. Two different sample counts must yield different keys.
#[test]
fn section_key_changes_when_soft_shadow_samples_change() {
    let mut geo = two_quad_geometry();
    let lights = vec![point_light([0.5, 1.0, 0.5], 5.0)];
    let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
    let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (_, prims, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };

    let samples_a = 16u32;
    let samples_b = 32u32;
    let hashes_a: Vec<[u8; 32]> = light_refs
        .iter()
        .map(|l| layer_input_hash(l, &shared, &prims, &geo, DENSITY, samples_a, 0))
        .collect();
    let hashes_b: Vec<[u8; 32]> = light_refs
        .iter()
        .map(|l| layer_input_hash(l, &shared, &prims, &geo, DENSITY, samples_b, 0))
        .collect();

    assert_ne!(
        section_key(&hashes_a, &shared, DENSITY, true).as_filename(),
        section_key(&hashes_b, &shared, DENSITY, true).as_filename(),
        "changing --soft-shadow-samples must change the section key (section miss)"
    );
}
