// Lightmap bake tests: chart planning, packing, per-texel bake, encode, and cold-path parity.
// See: context/lib/build_pipeline.md §Compiler pipeline

use super::atlas_layout::assign_lightmap_uvs;
use super::charts::{empty_chart_for_leaf, plan_charts, resolved_chart_density};
use super::encode::{encode_direction_rg8, reduce_direction_atlas};
use super::*;
use crate::bake_control::BakeControl;
use crate::bvh_build::build_bvh;
use crate::geometry::FaceIndexRange;
use crate::governor::Governor;
use crate::reporter::StageProgress;
use glam::DVec3;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::lightmap::{LightmapBlock, encode_direction_oct};
use postretro_level_format::texture_names::TextureNamesSection;
use rayon::ThreadPoolBuilder;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn unit_quad_geometry() -> GeometryResult {
    let v0 = Vertex::new(
        [0.0, 0.0, 0.0],
        [0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    );
    let v1 = Vertex::new(
        [1.0, 0.0, 0.0],
        [1.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    );
    let v2 = Vertex::new(
        [1.0, 0.0, 1.0],
        [1.0, 1.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    );
    let v3 = Vertex::new(
        [0.0, 0.0, 1.0],
        [0.0, 1.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    );
    GeometryResult {
        geometry: GeometrySection {
            vertices: vec![v0, v1, v2, v3],
            indices: vec![0, 1, 2, 0, 2, 3],
            faces: vec![FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            }],
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: vec![FaceIndexRange {
            index_offset: 0,
            index_count: 6,
        }],
    }
}

fn two_disjoint_quads_geometry() -> GeometryResult {
    let mut geometry = unit_quad_geometry();
    let mut second = unit_quad_geometry();
    let vertex_offset = geometry.geometry.vertices.len() as u32;
    let index_offset = geometry.geometry.indices.len() as u32;

    for vertex in &mut second.geometry.vertices {
        vertex.position[0] += 2.0;
    }
    geometry.geometry.vertices.extend(second.geometry.vertices);
    geometry.geometry.indices.extend(
        second
            .geometry
            .indices
            .into_iter()
            .map(|index| index + vertex_offset),
    );
    geometry.geometry.faces.extend(second.geometry.faces);
    geometry
        .face_index_ranges
        .extend(second.face_index_ranges.into_iter().map(|mut range| {
            range.index_offset += index_offset;
            range
        }));
    geometry
}

/// Two large, separate leaves that each fit a 64² atlas layer but cannot
/// share one. This keeps the cold-bake byte-identity fixture small while
/// forcing the production path through a non-zero array layer.
fn two_large_disjoint_quads_geometry() -> GeometryResult {
    let mut first = unit_quad_geometry();
    for vertex in &mut first.geometry.vertices {
        vertex.position[0] *= 12.0;
        vertex.position[2] *= 12.0;
    }

    let mut second = unit_quad_geometry();
    for vertex in &mut second.geometry.vertices {
        vertex.position[0] = vertex.position[0] * 12.0 + 16.0;
        vertex.position[2] *= 12.0;
    }
    second.geometry.faces[0].leaf_index = 1;

    let vertex_offset = first.geometry.vertices.len() as u32;
    let index_offset = first.geometry.indices.len() as u32;
    first.geometry.vertices.extend(second.geometry.vertices);
    first.geometry.indices.extend(
        second
            .geometry
            .indices
            .into_iter()
            .map(|index| index + vertex_offset),
    );
    first.geometry.faces.extend(second.geometry.faces);
    first
        .face_index_ranges
        .extend(second.face_index_ranges.into_iter().map(|mut range| {
            range.index_offset += index_offset;
            range
        }));
    first
}

fn point_light_above() -> MapLight {
    MapLight {
        origin: DVec3::new(0.5, 1.0, 0.5),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 5.0,
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
        shadow_type: crate::map_data::ShadowType::StaticLightMap,
    }
}

fn scale_region(min: [f32; 3], max: [f32; 3], scale: f32) -> MapLightmapScaleRegion {
    MapLightmapScaleRegion {
        min,
        max,
        planes: Vec::new(),
        scale,
    }
}

fn bake_scale_region_fixture(regions: &[MapLightmapScaleRegion]) -> Vec<u8> {
    let mut geometry = unit_quad_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let lights = vec![point_light_above()];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &mut geometry,
        lights: &static_lights,
        scale_regions: regions,
    };
    bake_lightmap(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: false,
        },
    )
    .unwrap()
    .section
    .to_bytes()
}

#[test]
fn empty_geometry_bakes_no_blocks() {
    let mut geo = GeometryResult {
        geometry: GeometrySection {
            vertices: vec![],
            indices: vec![],
            faces: vec![],
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: vec![],
    };
    let bvh = bvh::bvh::Bvh { nodes: Vec::new() };
    let prims: Vec<BvhPrimitive> = Vec::new();
    let lights = vec![point_light_above()];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &prims,
        geometry: &mut geo,
        lights: &static_lights,
        scale_regions: &[],
    };
    let section = bake_lightmap(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: DEFAULT_TEXEL_DENSITY_METERS,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: false,
        },
    )
    .unwrap()
    .section;
    assert!(section.blocks.is_empty(), "no static light bakes no block");
}

#[test]
fn no_static_lights_bakes_no_blocks() {
    let mut geo = unit_quad_geometry();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let lights: Vec<MapLight> = Vec::new();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &prims,
        geometry: &mut geo,
        lights: &static_lights,
        scale_regions: &[],
    };
    let section = bake_lightmap(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: DEFAULT_TEXEL_DENSITY_METERS,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: false,
        },
    )
    .unwrap()
    .section;
    assert!(section.blocks.is_empty(), "no static light bakes no block");
}

/// A chart wider than a pool layer rejects the build before block packing,
/// naming the face and the density it was charted at (here a scale region's).
#[test]
fn chart_past_the_pool_layer_edge_rejects_the_build_naming_its_face() {
    use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;

    let lights = vec![point_light_above()];
    let static_lights = StaticBakedLights::from_lights(&lights);
    // The 1 m quad at 0.04 / 200 m per texel needs 5,000 interior texels.
    let region = scale_region([-1.0, -1.0, -1.0], [2.0, 2.0, 2.0], 200.0);
    let mut geo = unit_quad_geometry();
    let error = prepare_atlas(
        &mut geo,
        &static_lights,
        DEFAULT_TEXEL_DENSITY_METERS,
        std::slice::from_ref(&region),
    )
    .expect_err("a chart past the pool layer edge must fail the build");
    match error {
        LightmapBakeError::ChartTooLarge {
            face_index,
            width_texels,
            height_texels,
            max,
            density_m_per_texel,
            ..
        } => {
            assert_eq!(face_index, 0);
            assert_eq!(max, LIGHTMAP_POOL_LAYER_EDGE);
            assert!(width_texels > max && height_texels > max);
            assert_eq!(density_m_per_texel, DEFAULT_TEXEL_DENSITY_METERS / 200.0);
        }
        other => panic!("expected ChartTooLarge, got {other}"),
    }
}

/// Without static light no block is emitted, so a layout the runtime could
/// not hold is no build error: preparation returns the charts with no
/// placements, keeps every vertex on block 0, and keeps the direction scale
/// the placeholder id-22 header carries.
#[test]
fn no_static_light_layout_past_the_runtime_limits_returns_no_placements() {
    let lights: Vec<MapLight> = Vec::new();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut geo = unit_quad_geometry();
    let prepared = prepare_atlas(&mut geo, &static_lights, 1.0 / 4096.0, &[])
        .expect("no static light tolerates an oversize layout");
    assert_eq!(prepared.charts.len(), 1);
    assert!(prepared.placements.is_empty());
    assert!(prepared.layout.blocks.is_empty());
    assert_eq!(prepared.layout.direction_texel_scale, DIRECTION_TEXEL_SCALE);
    assert!(
        geo.geometry
            .vertices
            .iter()
            .all(|vertex| vertex.lightmap_block == 0)
    );
}

#[test]
fn single_static_light_produces_nonzero_irradiance() {
    let mut geo = unit_quad_geometry();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let lights = vec![point_light_above()];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &prims,
        geometry: &mut geo,
        lights: &static_lights,
        scale_regions: &[],
    };
    let section = bake_lightmap(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            // Test reads per-texel bytes assuming the 8-byte RGBA16F stride;
            // request the uncompressed debug bypass so the byte layout stays
            // readable without re-implementing the GPU's BC6H decode here.
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: true,
        },
    )
    .unwrap()
    .section;
    assert!(!section.blocks.is_empty());
    for block in &section.blocks {
        assert_eq!(
            block.irradiance.len(),
            usize::from(block.width) * usize::from(block.height) * 8
        );
    }
    let irradiance = all_irradiance(&section);
    let mut has_nonzero = false;
    for chunk in irradiance.chunks_exact(2).step_by(4) {
        let bits = u16::from_le_bytes([chunk[0], chunk[1]]);
        if bits != 0 {
            has_nonzero = true;
            break;
        }
    }
    assert!(
        has_nonzero,
        "expected at least one non-zero irradiance texel"
    );
}

/// Disjoint-direct contract (sdf-per-light-shadows Task 3): an `sdf`-typed
/// light stays in the `StaticBakedLights` namespace (it's `!is_dynamic`, so
/// SH still bakes its bounce), but the direct lightmap consumer drops it —
/// `lm_irr` carries no direct term for it (the runtime SDF trace resolves
/// that). The atlas preps to a real size (the namespace is non-empty) yet
/// every irradiance texel is zero, in contrast to the `static_light_map`
/// case above which produces non-zero irradiance from the same geometry.
#[test]
fn sdf_typed_light_excluded_from_direct_lightmap() {
    let mut geo = unit_quad_geometry();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let mut sdf_light = point_light_above();
    sdf_light.shadow_type = crate::map_data::ShadowType::Sdf;
    let lights = vec![sdf_light];
    let static_lights = StaticBakedLights::from_lights(&lights);
    // The sdf light is in the namespace (feeds SH); only the direct bake drops it.
    assert_eq!(
        static_lights.len(),
        1,
        "sdf light must remain in StaticBakedLights (keys on position, not shadow type)",
    );
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &prims,
        geometry: &mut geo,
        lights: &static_lights,
        scale_regions: &[],
    };
    let section = bake_lightmap(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            // See `single_static_light_produces_nonzero_irradiance`: this
            // test reads per-texel bytes from the irradiance blob, so it
            // requests the RGBA16F debug bypass rather than the default
            // BC6H production layout.
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: true,
        },
    )
    .unwrap()
    .section;
    let mut has_nonzero = false;
    for chunk in all_irradiance(&section).chunks_exact(2).step_by(4) {
        let bits = u16::from_le_bytes([chunk[0], chunk[1]]);
        if bits != 0 {
            has_nonzero = true;
            break;
        }
    }
    assert!(
        !has_nonzero,
        "an sdf-typed light must contribute no direct lightmap irradiance",
    );
}

#[test]
fn is_dynamic_lights_skipped_by_bake() {
    let mut geo = unit_quad_geometry();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let mut dyn_light = point_light_above();
    dyn_light.is_dynamic = true;
    let lights = vec![dyn_light];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &prims,
        geometry: &mut geo,
        lights: &static_lights,
        scale_regions: &[],
    };
    let section = bake_lightmap(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: DEFAULT_TEXEL_DENSITY_METERS,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: false,
        },
    )
    .unwrap()
    .section;
    assert!(section.blocks.is_empty(), "no static light bakes no block");
}

#[test]
fn static_nonanimated_bakes_but_dynamic_and_animated_do_not() {
    // The static atlas carries only non-animated static lights. Dynamic and animated lights
    // are owned by the runtime direct-lighting and weight-map compose passes respectively;
    // baking them here would double-count.
    use crate::map_data::LightAnimation;

    let mut geo_static = unit_quad_geometry();
    let (bvh, prims, _) = build_bvh(&geo_static).unwrap();
    let base = point_light_above();
    let static_base = StaticBakedLights::from_lights(std::slice::from_ref(&base));
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &prims,
        geometry: &mut geo_static,
        lights: &static_base,
        scale_regions: &[],
    };
    let section_static = bake_lightmap(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: false,
        },
    )
    .unwrap()
    .section;
    assert!(
        !section_static.blocks.is_empty(),
        "non-animated static light must bake into real blocks",
    );

    let mut dyn_light = point_light_above();
    dyn_light.is_dynamic = true;
    let mut geo_dyn = unit_quad_geometry();
    let (bvh_d, prims_d, _) = build_bvh(&geo_dyn).unwrap();
    let static_dyn = StaticBakedLights::from_lights(std::slice::from_ref(&dyn_light));
    let mut inputs_d = LightmapBakeCtx {
        bvh: &bvh_d,
        primitives: &prims_d,
        geometry: &mut geo_dyn,
        lights: &static_dyn,
        scale_regions: &[],
    };
    let section_dyn = bake_lightmap(
        &mut inputs_d,
        &LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: false,
        },
    )
    .unwrap()
    .section;
    assert!(
        section_dyn.blocks.is_empty(),
        "is_dynamic light must not bake"
    );

    let mut anim_light = point_light_above();
    anim_light.animation = Some(LightAnimation {
        period: 1.0,
        phase: 0.0,
        brightness: Some(vec![1.0, 0.5]),
        color: None,
        direction: None,
        start_active: true,
    });
    let mut geo_anim = unit_quad_geometry();
    let (bvh_a, prims_a, _) = build_bvh(&geo_anim).unwrap();
    let static_anim = StaticBakedLights::from_lights(std::slice::from_ref(&anim_light));
    let mut inputs_a = LightmapBakeCtx {
        bvh: &bvh_a,
        primitives: &prims_a,
        geometry: &mut geo_anim,
        lights: &static_anim,
        scale_regions: &[],
    };
    let section_anim = bake_lightmap(
        &mut inputs_a,
        &LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: false,
        },
    )
    .unwrap()
    .section;
    assert!(
        section_anim.blocks.is_empty(),
        "animated light must not contribute to the static lightmap",
    );

    let mut bake_only_anim = anim_light.clone();
    bake_only_anim.bake_only = true;
    let mut geo_bo = unit_quad_geometry();
    let (bvh_b, prims_b, _) = build_bvh(&geo_bo).unwrap();
    let static_bo = StaticBakedLights::from_lights(std::slice::from_ref(&bake_only_anim));
    let mut inputs_b = LightmapBakeCtx {
        bvh: &bvh_b,
        primitives: &prims_b,
        geometry: &mut geo_bo,
        lights: &static_bo,
        scale_regions: &[],
    };
    let section_bo = bake_lightmap(
        &mut inputs_b,
        &LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: false,
        },
    )
    .unwrap()
    .section;
    assert!(
        section_bo.blocks.is_empty(),
        "bake_only animated light must not contribute to the static lightmap",
    );
}

#[test]
fn chart_planning_produces_positive_extents() {
    let geo = unit_quad_geometry();
    let charts = plan_charts(&geo, 0.25, &[]).unwrap();
    assert_eq!(charts.len(), 1);
    assert!(charts[0].uv_extent[0] > 0.0);
    assert!(charts[0].uv_extent[1] > 0.0);
    assert!(charts[0].width_texels >= 1);
    assert!(charts[0].height_texels >= 1);
}

#[test]
fn scale_regions_override_chart_density_by_origin_in_definition_order() {
    let geo = unit_quad_geometry();
    // The unit quad's p0 is at the origin while its centroid is at
    // (0.5, 0.0, 0.5). This thin box therefore proves membership uses p0,
    // not the centroid (P9).
    let straddled_region = scale_region([-0.1, -1.0, -0.1], [0.1, 1.0, 0.1], 0.5);
    let outside = scale_region([2.0, -1.0, 2.0], [3.0, 1.0, 3.0], 0.25);
    let baseline = plan_charts(&geo, 0.25, &[]).unwrap();
    let straddled = plan_charts(&geo, 0.25, std::slice::from_ref(&straddled_region)).unwrap();
    let outside_only = plan_charts(&geo, 0.25, &[outside]).unwrap();
    assert!(
        straddled[0].width_texels < baseline[0].width_texels,
        "p0 inside a coarse region must make the whole chart coarser"
    );
    assert_eq!(
        outside_only[0].width_texels, baseline[0].width_texels,
        "a region missing p0 must leave global density unchanged"
    );

    let overlapping = [
        scale_region([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0], 0.5),
        scale_region([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0], 0.25),
    ];
    let planned = plan_charts(&geo, 0.25, &overlapping).unwrap();
    assert!(
        planned[0].width_texels < straddled[0].width_texels,
        "the last containing region must win and apply its scale"
    );
    assert!((resolved_chart_density(Vec3::ZERO, 0.25, &overlapping) - 1.0).abs() < 1e-6);
}

#[test]
fn chart_planning_rejects_nonfinite_density_from_extreme_finite_scale() {
    let geo = unit_quad_geometry();
    // The smallest positive finite `f32` overflows the resolved density.
    let regions = [scale_region(
        [-1.0, -1.0, -1.0],
        [1.0, 1.0, 1.0],
        f32::from_bits(1),
    )];

    let err = plan_charts(&geo, 0.25, &regions).unwrap_err();
    assert!(matches!(
        err,
        LightmapBakeError::InvalidChartDensity {
            face_index: 0,
            density_m_per_texel,
        } if density_m_per_texel.is_infinite()
    ));
}

#[test]
fn chart_planning_rejects_dimension_overflow_from_extreme_finite_scale() {
    let geo = unit_quad_geometry();
    let regions = [scale_region([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0], f32::MAX)];

    let err = plan_charts(&geo, 0.25, &regions).unwrap_err();
    assert!(matches!(
        err,
        LightmapBakeError::ChartDimensionOverflow {
            face_index: 0,
            axis: "width",
            ..
        }
    ));
}

#[test]
fn overlapping_scale_regions_plan_deterministically() {
    let geo = unit_quad_geometry();
    let regions = [
        scale_region([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0], 0.5),
        scale_region([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0], 0.25),
    ];
    let first = plan_charts(&geo, 0.25, &regions).unwrap();
    let second = plan_charts(&geo, 0.25, &regions).unwrap();
    let first_dims: Vec<_> = first
        .iter()
        .map(|chart| (chart.width_texels, chart.height_texels))
        .collect();
    let second_dims: Vec<_> = second
        .iter()
        .map(|chart| (chart.width_texels, chart.height_texels))
        .collect();
    assert_eq!(
        first_dims, second_dims,
        "P4: source-order resolution is stable"
    );
    assert_eq!(
        bake_scale_region_fixture(&regions),
        bake_scale_region_fixture(&regions),
        "P4: re-baking the same overlapping-region map must emit identical PRL bytes"
    );
}

#[test]
fn warm_and_cold_atlas_preparation_share_scale_regions() {
    let regions = [scale_region([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0], 0.5)];
    let lights = vec![point_light_above()];
    let static_lights = StaticBakedLights::from_lights(&lights);

    let mut warm_geo = unit_quad_geometry();
    let warm = prepare_atlas(&mut warm_geo, &static_lights, 0.25, &regions).unwrap();

    let mut cold_geo = unit_quad_geometry();
    let (bvh, primitives, _) = build_bvh(&cold_geo).unwrap();
    let mut cold_inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &mut cold_geo,
        lights: &static_lights,
        scale_regions: &regions,
    };
    let cold = bake_lightmap(
        &mut cold_inputs,
        &LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: false,
        },
    )
    .unwrap();

    assert_eq!(cold.atlas_width, warm.atlas_width, "P13: atlas width");
    assert_eq!(cold.atlas_height, warm.atlas_height, "P13: atlas height");
    assert_eq!(cold.layer_count, warm.layer_count, "P13: atlas layers");
    let warm_layout: Vec<_> = warm
        .charts
        .iter()
        .zip(&warm.placements)
        .map(|(chart, placement)| {
            (
                chart.width_texels,
                chart.height_texels,
                placement.x,
                placement.y,
                placement.layer,
            )
        })
        .collect();
    let cold_layout: Vec<_> = cold
        .charts
        .iter()
        .zip(&cold.placements)
        .map(|(chart, placement)| {
            (
                chart.width_texels,
                chart.height_texels,
                placement.x,
                placement.y,
                placement.layer,
            )
        })
        .collect();
    assert_eq!(
        cold_layout, warm_layout,
        "P13: warm/cold scale regions diverged"
    );
}

#[test]
fn pack_layers_is_deterministic() {
    let geo = unit_quad_geometry();
    let charts = plan_charts(&geo, 0.25, &[]).unwrap();
    let p1 = pack_layers(&charts, MAX_ATLAS_DIMENSION, 0.25).unwrap();
    let p2 = pack_layers(&charts, MAX_ATLAS_DIMENSION, 0.25).unwrap();
    assert_eq!(p1.atlas_width, p2.atlas_width);
    assert_eq!(p1.atlas_height, p2.atlas_height);
    assert_eq!(p1.layer_count, p2.layer_count);
    assert_eq!(p1.placements.len(), p2.placements.len());
    for (a, b) in p1.placements.iter().zip(p2.placements.iter()) {
        assert_eq!(a.x, b.x);
        assert_eq!(a.y, b.y);
        assert_eq!(a.layer, b.layer);
    }
}

/// Pins the production ceiling: the layer-limit parameter must not have
/// loosened `pack_layers` itself.
#[test]
fn pack_layers_overflows_past_max_atlas_layers() {
    let dim = MIN_ATLAS_DIMENSION;
    let leaves = |count: u32| -> Vec<Chart> {
        (0..count)
            .map(|leaf| synthetic_chart_leaf(dim, dim, leaf))
            .collect()
    };
    let full = pack_layers(&leaves(MAX_ATLAS_LAYERS), dim, 1.0).unwrap();
    assert_eq!(full.layer_count, MAX_ATLAS_LAYERS);
    match pack_layers(&leaves(MAX_ATLAS_LAYERS + 1), dim, 1.0) {
        Err(LightmapBakeError::LayerOverflow { max, .. }) => assert_eq!(max, MAX_ATLAS_LAYERS),
        other => panic!("expected LayerOverflow, got {other:?}"),
    }
}

fn synthetic_chart(w: u32, h: u32) -> Chart {
    synthetic_chart_leaf(w, h, 0)
}

fn synthetic_chart_leaf(w: u32, h: u32, leaf_index: u32) -> Chart {
    Chart {
        origin: Vec3::ZERO,
        u_axis: Vec3::X,
        v_axis: Vec3::Z,
        uv_min: [0.0, 0.0],
        uv_extent: [1.0, 1.0],
        normal: Vec3::Y,
        width_texels: w,
        height_texels: h,
        leaf_index,
        window: None,
    }
}

/// Leaf-aware multi-bin packing (Task 3b): when two BVH leaves' charts can't
/// all share one layer, the packer opens a second layer and keeps each leaf
/// whole on its own layer. Forces two layers by sizing each leaf to fill an
/// entire small `max_dim` atlas, so the second leaf cannot share the first
/// leaf's layer.
#[test]
fn pack_layers_opens_second_layer_keeping_each_leaf_cohesive() {
    let max_dim = 64;
    // Leaf 0: one 64×64 chart that fills the whole 64² layer. Leaf 1: two
    // 64×32 charts that together fill a second 64² layer. Neither leaf can
    // share a layer with the other.
    let charts = vec![
        synthetic_chart_leaf(64, 64, 0),
        synthetic_chart_leaf(64, 32, 1),
        synthetic_chart_leaf(64, 32, 1),
    ];

    let pack = pack_layers(&charts, max_dim, 0.25).expect("must pack into two layers");

    assert_eq!(pack.layer_count, 2, "two full leaves must span two layers");
    assert_eq!(pack.atlas_width, 64);
    assert_eq!(pack.atlas_height, 64);

    // Every chart of a given leaf must land on exactly one layer.
    let mut leaf_layers: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for (chart, placement) in charts.iter().zip(pack.placements.iter()) {
        assert!(
            placement.layer < pack.layer_count,
            "layer {} out of range (count {})",
            placement.layer,
            pack.layer_count,
        );
        match leaf_layers.get(&chart.leaf_index) {
            Some(&existing) => assert_eq!(
                existing, placement.layer,
                "leaf {} straddled layers {existing} and {}",
                chart.leaf_index, placement.layer,
            ),
            None => {
                leaf_layers.insert(chart.leaf_index, placement.layer);
            }
        }
    }
    // The two leaves landed on different layers.
    assert_ne!(
        leaf_layers[&0], leaf_layers[&1],
        "the two leaves must occupy distinct layers",
    );
}

/// `assign_lightmap_uvs` names each face's cell block as `id + 1` and writes
/// the UV over that block's extent. Three charts in two cells → two blocks;
/// the cell-1 faces share a block, and every UV stays in `[0, 1]`.
#[test]
fn assign_lightmap_uvs_writes_block_id_plus_one_and_block_local_uvs() {
    let charts = vec![
        synthetic_chart_leaf(64, 64, 0),
        synthetic_chart_leaf(64, 32, 1),
        synthetic_chart_leaf(64, 32, 1),
    ];
    let pack = block_layout::pack_cell_blocks(
        &charts,
        BlockOrdering::by_cell_id(DIRECTION_TEXEL_SCALE),
        &BakeControl::unrestricted(),
    )
    .expect("synthetic cells must pack");
    assert_eq!(pack.layout.blocks.len(), 2, "one block per cell");

    // Minimal geometry: one triangle per chart, each face owning three
    // vertices (no sharing), so face index `i` maps to chart `i`.
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut faces = Vec::new();
    let mut face_index_ranges = Vec::new();
    for chart in &charts {
        let base = vertices.len() as u32;
        for p in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] {
            vertices.push(Vertex::new(
                p,
                [0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ));
        }
        let offset = indices.len() as u32;
        indices.extend_from_slice(&[base, base + 1, base + 2]);
        faces.push(FaceMeta {
            leaf_index: chart.leaf_index,
            texture_index: 0,
        });
        face_index_ranges.push(FaceIndexRange {
            index_offset: offset,
            index_count: 3,
        });
    }
    let mut geom = GeometryResult {
        geometry: GeometrySection {
            vertices,
            indices,
            faces,
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges,
    };

    assign_lightmap_uvs(&mut geom, &charts, &pack.placements, &pack.layout);

    let first_vertex_block = |face: usize| {
        let r = geom.face_index_ranges[face];
        geom.geometry.vertices[geom.geometry.indices[r.index_offset as usize] as usize]
            .lightmap_block
    };
    for (face_index, range) in geom.face_index_ranges.iter().enumerate() {
        let expected = pack.layout.chart_blocks[face_index] as u16 + 1;
        let start = range.index_offset as usize;
        let end = start + range.index_count as usize;
        for &idx in &geom.geometry.indices[start..end] {
            let v = &geom.geometry.vertices[idx as usize];
            assert_eq!(
                v.lightmap_block, expected,
                "face {face_index} vertex {idx}: lightmap_block must be its block id + 1",
            );
            let uv = v.decode_lightmap_uv();
            assert!(
                (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]),
                "face {face_index} vertex {idx}: lightmap_uv out of [0,1]: {uv:?}",
            );
        }
    }
    assert_ne!(first_vertex_block(0), first_vertex_block(1));
    assert_eq!(
        first_vertex_block(1),
        first_vertex_block(2),
        "one cell's faces share its block"
    );
    assert!(first_vertex_block(0) > 0 && first_vertex_block(1) > 0);
}

/// `count` 12 m floor quads in cell 0, side by side along x. At 0.25 m/texel
/// each charts at 52² texels, so no two share a 64-texel test pool layer.
fn quads_in_one_cell(count: usize) -> GeometryResult {
    let mut geometry = unit_quad_geometry();
    geometry.geometry.vertices.clear();
    geometry.geometry.indices.clear();
    geometry.geometry.faces.clear();
    geometry.face_index_ranges.clear();
    for quad in 0..count {
        let mut next = unit_quad_geometry();
        for vertex in &mut next.geometry.vertices {
            vertex.position[0] = vertex.position[0] * 12.0 + 14.0 * quad as f32;
            vertex.position[2] *= 12.0;
        }
        let vertex_offset = geometry.geometry.vertices.len() as u32;
        let index_offset = geometry.geometry.indices.len() as u32;
        geometry.geometry.vertices.extend(next.geometry.vertices);
        geometry.geometry.indices.extend(
            next.geometry
                .indices
                .into_iter()
                .map(|index| index + vertex_offset),
        );
        geometry.geometry.faces.extend(next.geometry.faces);
        geometry
            .face_index_ranges
            .extend(next.face_index_ranges.into_iter().map(|mut range| {
                range.index_offset += index_offset;
                range
            }));
    }
    geometry
}

/// A cell too large for one pool layer splits into several blocks, and every
/// vertex names the block its own face's chart landed in.
#[test]
fn oversized_cell_vertices_name_their_own_charts_block() {
    let lights = vec![point_light_above()];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut geom = quads_in_one_cell(3);
    let prepared = prepare_atlas_within(
        &mut geom,
        &static_lights,
        0.25,
        &[],
        BlockOrdering::by_cell_id(DIRECTION_TEXEL_SCALE),
        64,
        &BakeControl::unrestricted(),
    )
    .expect("oversized cell prepares");
    assert_eq!(prepared.layout.blocks.len(), 3, "one block per 52² chart");
    assert!(prepared.layout.blocks.iter().all(|b| b.cell_id == 0));
    assert!(
        prepared
            .layout
            .blocks
            .iter()
            .all(|b| b.width <= 64 && b.height <= 64)
    );
    for (face, range) in geom.face_index_ranges.iter().enumerate() {
        let expected = prepared.layout.chart_blocks[face] as u16 + 1;
        let start = range.index_offset as usize;
        for &index in &geom.geometry.indices[start..start + range.index_count as usize] {
            assert_eq!(
                geom.geometry.vertices[index as usize].lightmap_block, expected,
                "face {face} vertex {index} must name its chart's block"
            );
        }
    }
}

/// Fix 1 robustness — a single BVH leaf whose charts can't fit even an empty
/// `max_dim²` layer must return `LeafTooLarge`, not silently corrupt
/// placements. Leaf cohesion forbids splitting the leaf across layers, so the
/// packer has no escape: several near-`max_dim` charts on one leaf overflow a
/// single layer regardless of how many layers it opens.
#[test]
fn pack_layers_single_oversized_leaf_errors() {
    let max_dim = 64;
    // One leaf, four 64×64 charts. Each fills a whole 64² layer, so they
    // cannot coexist on one layer — and the leaf cannot be split.
    let charts = vec![
        synthetic_chart_leaf(64, 64, 7),
        synthetic_chart_leaf(64, 64, 7),
        synthetic_chart_leaf(64, 64, 7),
        synthetic_chart_leaf(64, 64, 7),
    ];
    let err = pack_layers(&charts, max_dim, 0.25)
        .expect_err("an oversized single leaf must error, not pack");
    match err {
        LightmapBakeError::LeafTooLarge {
            leaf_index,
            chart_count,
            max_dim: reported_max,
        } => {
            assert_eq!(leaf_index, 7, "error must carry the BVH leaf index");
            assert_eq!(chart_count, 4, "error must report the leaf's chart count");
            assert_eq!(reported_max, max_dim, "error must report the max dimension");
        }
        other => panic!("expected LeafTooLarge, got {other:?}"),
    }
}

// Regression: `CompositedAtlas::zeroed` computed `atlas_w * atlas_h *
// layer_count` in u32 before casting, so a many-layer 8192² atlas wrapped
// past u32::MAX and under-allocated the buffers — a release-build buffer
// overrun at bake time. The texel count must be computed in usize.
#[test]
fn composited_atlas_texel_count_does_not_overflow_u32() {
    // 8192² × 100 layers = 6_710_886_400, which overflows u32 (max
    // 4_294_967_295) but is exact in usize. No allocation — pure arithmetic.
    let n = composited_atlas_texel_count(8192, 8192, 100);
    assert_eq!(n, 8192usize * 8192 * 100);
    assert!(
        n > u32::MAX as usize,
        "the guarded product must exceed u32::MAX to exercise the wrap path"
    );
}

fn bake_monolithic_with_workers(worker_count: usize) -> CompositedAtlas {
    let mut geometry = two_disjoint_quads_geometry();
    let lights = vec![point_light_above()];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let light_refs: Vec<&MapLight> = static_lights
        .entries()
        .iter()
        .map(|entry| entry.light)
        .collect();
    let prepared = prepare_atlas(&mut geometry, &static_lights, 0.25, &[]).unwrap();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let progress = StageProgress::with_total(prepared.placements.len());
    let control = BakeControl::new(Arc::new(Governor::new(worker_count, false)), &progress);

    let atlas = ThreadPoolBuilder::new()
        .num_threads(worker_count)
        .build()
        .unwrap()
        .install(|| {
            bake_monolithic_atlas_controlled(
                &bvh,
                &primitives,
                &geometry,
                &light_refs,
                &prepared.charts,
                &prepared.placements,
                prepared.atlas_width,
                prepared.atlas_height,
                prepared.layer_count,
                DEFAULT_AREA_SAMPLE_COUNT,
                &control,
            )
        });

    assert_eq!(
        progress.completed(),
        prepared.placements.len(),
        "every chart must advance progress exactly once"
    );
    assert_eq!(
        progress.total(),
        Some(progress.completed()),
        "at bake return the published total must equal completed chart advances"
    );
    atlas
}

#[test]
fn monolithic_atlas_is_byte_identical_with_one_or_many_workers() {
    let serial = bake_monolithic_with_workers(1);
    let parallel = bake_monolithic_with_workers(4);

    assert_eq!(
        serial, parallel,
        "chart completion order must not change the pre-BC6H atlas"
    );
}

fn single_layer_reference_section(uncompressed_irradiance: bool) -> LightmapSection {
    let mut geometry = unit_quad_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let lights = vec![point_light_above()];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let light_refs: Vec<&MapLight> = static_lights
        .entries()
        .iter()
        .map(|entry| entry.light)
        .collect();
    let prepared = prepare_atlas(&mut geometry, &static_lights, 0.25, &[]).unwrap();
    assert_eq!(prepared.layer_count, 1);

    bake_monolithic_atlas(
        &bvh,
        &primitives,
        &geometry,
        &light_refs,
        &prepared.charts,
        &prepared.placements,
        prepared.atlas_width,
        prepared.atlas_height,
        prepared.layer_count,
        DEFAULT_AREA_SAMPLE_COUNT,
    )
    .encode_section(&prepared.layout, uncompressed_irradiance)
}

fn single_layer_cold_section(uncompressed_irradiance: bool) -> LightmapSection {
    let mut geometry = unit_quad_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let lights = vec![point_light_above()];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &mut geometry,
        lights: &static_lights,
        scale_regions: &[],
    };

    bake_lightmap_controlled(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance,
        },
        &BakeControl::unrestricted(),
    )
    .unwrap()
    .section
}

fn multi_layer_reference_section(uncompressed_irradiance: bool) -> LightmapSection {
    let mut geometry = two_large_disjoint_quads_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let mut light = point_light_above();
    light.origin = DVec3::new(10.0, 8.0, 6.0);
    light.falloff_range = 40.0;
    let lights = vec![light];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let light_refs: Vec<&MapLight> = static_lights
        .entries()
        .iter()
        .map(|entry| entry.light)
        .collect();
    let prepared = prepare_atlas(&mut geometry, &static_lights, 0.25, &[]).unwrap();
    assert_eq!(
        prepared.layer_count, 2,
        "fixture must force a non-zero atlas layer"
    );

    bake_monolithic_atlas(
        &bvh,
        &primitives,
        &geometry,
        &light_refs,
        &prepared.charts,
        &prepared.placements,
        prepared.atlas_width,
        prepared.atlas_height,
        prepared.layer_count,
        DEFAULT_AREA_SAMPLE_COUNT,
    )
    .encode_section(&prepared.layout, uncompressed_irradiance)
}

fn multi_layer_cold_section(uncompressed_irradiance: bool) -> LightmapSection {
    let mut geometry = two_large_disjoint_quads_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let mut light = point_light_above();
    light.origin = DVec3::new(10.0, 8.0, 6.0);
    light.falloff_range = 40.0;
    let lights = vec![light];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &mut geometry,
        lights: &static_lights,
        scale_regions: &[],
    };

    bake_lightmap_controlled(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance,
        },
        &BakeControl::unrestricted(),
    )
    .unwrap()
    .section
}

/// OP1: the shipping cold path encodes every bake layer in ascending order and
/// slices its blocks exactly as the retained whole-atlas reference kernel.
#[test]
fn layered_cold_bake_matches_reference_and_repeats_byte_identically() {
    for uncompressed_irradiance in [true, false] {
        let single_reference = single_layer_reference_section(uncompressed_irradiance);
        let single_cold = single_layer_cold_section(uncompressed_irradiance);
        assert_eq!(
            single_cold.to_bytes(),
            single_reference.to_bytes(),
            "single-layer cold stage must equal the frozen whole-atlas reference"
        );

        let reference = multi_layer_reference_section(uncompressed_irradiance);
        let first = multi_layer_cold_section(uncompressed_irradiance);
        let second = multi_layer_cold_section(uncompressed_irradiance);

        assert_eq!(first.blocks.len(), 2, "fixture must exercise bake layer 1");
        assert_eq!(
            first.to_bytes(),
            reference.to_bytes(),
            "multi-layer cold stage must equal the frozen whole-atlas reference"
        );
        assert_eq!(
            first.to_bytes(),
            second.to_bytes(),
            "serial layer traversal and per-chart parallel scatter must not reorder output"
        );
    }
}

/// OP2: a global placement on layer 1 must scatter into layer 0 of the
/// one-layer temporary atlas. Compare against layer 1 of the retained
/// whole-atlas reference to catch both an out-of-bounds layer address and
/// a silent write into the wrong plane.
#[test]
fn per_layer_bake_rebases_nonzero_layer_scatter_to_temporary_plane() {
    let geometry = unit_quad_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let lights = [point_light_above()];
    let light_refs: Vec<&MapLight> = lights.iter().collect();
    let charts = vec![synthetic_chart_leaf(64, 64, 1)];
    let placements = vec![ChartPlacement {
        x: 0,
        y: 0,
        layer: 1,
    }];

    let whole = bake_monolithic_atlas(
        &bvh,
        &primitives,
        &geometry,
        &light_refs,
        &charts,
        &placements,
        64,
        64,
        2,
        DEFAULT_AREA_SAMPLE_COUNT,
    );
    let layer = bake_atlas_layer_controlled(
        &bvh,
        &primitives,
        &geometry,
        &light_refs,
        &charts,
        &placements,
        64,
        64,
        1,
        DEFAULT_AREA_SAMPLE_COUNT,
        &BakeControl::unrestricted(),
    );
    let plane = 64usize * 64;

    assert_eq!(layer.layer_count, 1);
    assert_eq!(layer.irradiance, whole.irradiance[plane * 4..plane * 8]);
    assert_eq!(layer.direction, whole.direction[plane..plane * 2]);
    assert_eq!(layer.coverage, whole.coverage[plane..plane * 2]);
}

/// OP7: the per-layer chart fan-out must join its scatter work before the
/// serial dilation pass reads the temporary plane. A two-worker scatter of
/// disjoint chart rectangles must therefore equal the reference whole-atlas
/// plane and report both chart completions on return.
#[test]
fn per_layer_parallel_scatter_joins_before_dilation() {
    let geometry = two_disjoint_quads_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let lights = [point_light_above()];
    let light_refs: Vec<&MapLight> = lights.iter().collect();
    let charts = vec![
        synthetic_chart_leaf(32, 64, 0),
        synthetic_chart_leaf(32, 64, 1),
    ];
    let placements = vec![
        ChartPlacement {
            x: 0,
            y: 0,
            layer: 0,
        },
        ChartPlacement {
            x: 32,
            y: 0,
            layer: 0,
        },
    ];
    let expected = bake_monolithic_atlas(
        &bvh,
        &primitives,
        &geometry,
        &light_refs,
        &charts,
        &placements,
        64,
        64,
        1,
        DEFAULT_AREA_SAMPLE_COUNT,
    );
    let progress = StageProgress::with_total(placements.len());
    let control = BakeControl::new(Arc::new(Governor::new(2, false)), &progress);
    let actual = ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap()
        .install(|| {
            bake_atlas_layer_controlled(
                &bvh,
                &primitives,
                &geometry,
                &light_refs,
                &charts,
                &placements,
                64,
                64,
                0,
                DEFAULT_AREA_SAMPLE_COUNT,
                &control,
            )
        });

    assert_eq!(actual, expected);
    assert_eq!(progress.completed(), placements.len());
    assert_eq!(progress.total(), Some(progress.completed()));
}

/// OP8: even a block containing only degenerate charts is encoded. Skipping
/// its bake layer would leave the block without blobs despite leaving no
/// covered texels behind.
#[test]
fn layered_cold_encode_retains_degenerate_layer_blob() {
    let geometry = unit_quad_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let lights = [point_light_above()];
    let light_refs: Vec<&MapLight> = lights.iter().collect();
    let charts = vec![synthetic_chart_leaf(64, 64, 0), empty_chart_for_leaf(1)];
    let placements = vec![
        ChartPlacement {
            x: 0,
            y: 0,
            layer: 0,
        },
        ChartPlacement {
            x: 0,
            y: 0,
            layer: 1,
        },
    ];
    let layout = BlockLayout::whole_layers(64, 64, &placements, DIRECTION_TEXEL_SCALE);
    let section = bake_layered_section_controlled(
        &bvh,
        &primitives,
        &geometry,
        &light_refs,
        &charts,
        &placements,
        &layout,
        64,
        64,
        2,
        DEFAULT_AREA_SAMPLE_COUNT,
        true,
        &BakeControl::unrestricted(),
    );
    let reference = bake_monolithic_atlas(
        &bvh,
        &primitives,
        &geometry,
        &light_refs,
        &charts,
        &placements,
        64,
        64,
        2,
        DEFAULT_AREA_SAMPLE_COUNT,
    )
    .encode_section(&layout, true);

    assert_eq!(section.blocks.len(), 2);
    for block in &section.blocks {
        assert_eq!(block.irradiance.len(), 64 * 64 * 8);
        assert_eq!(block.direction.len(), 32 * 32 * 2);
    }
    assert_eq!(
        section.to_bytes(),
        reference.to_bytes(),
        "the uncovered degenerate block must still be encoded"
    );
}

#[test]
fn monolithic_atlas_counts_degenerate_chart_progress_without_scatter() {
    let geometry = unit_quad_geometry();
    let bvh = Bvh { nodes: Vec::new() };
    let primitives = Vec::new();
    let lights = [point_light_above()];
    let light_refs: Vec<&MapLight> = lights.iter().collect();
    let charts = vec![empty_chart_for_leaf(0)];
    let placements = vec![ChartPlacement {
        x: 0,
        y: 0,
        layer: 0,
    }];
    let progress = StageProgress::with_total(placements.len());
    let control = BakeControl::new(Arc::new(Governor::new(1, false)), &progress);

    let atlas = bake_monolithic_atlas_controlled(
        &bvh,
        &primitives,
        &geometry,
        &light_refs,
        &charts,
        &placements,
        MIN_ATLAS_DIMENSION,
        MIN_ATLAS_DIMENSION,
        1,
        DEFAULT_AREA_SAMPLE_COUNT,
        &control,
    );

    assert_eq!(
        progress.completed(),
        1,
        "degenerate chart must still advance"
    );
    assert_eq!(
        progress.total(),
        Some(progress.completed()),
        "at bake return the published total must include the degenerate chart"
    );
    assert!(
        atlas.coverage.iter().all(|&covered| !covered),
        "a degenerate chart must leave the atlas untouched"
    );
    assert!(
        atlas.irradiance.iter().all(|&value| value == 0.0),
        "a degenerate chart must not write irradiance"
    );
}

#[test]
fn monolithic_bake_paused_before_permit_release_starts_no_chart() {
    let mut geometry = two_disjoint_quads_geometry();
    let lights = vec![point_light_above()];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut geometry, &static_lights, 0.25, &[]).unwrap();
    assert!(
        prepared.placements.len() > 1,
        "fixture must have enough charts to park multiple parallel workers"
    );
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let chart_count = prepared.placements.len();
    let governor = Arc::new(Governor::new(1, false));
    let held_permit = governor.enter();
    let progress = StageProgress::with_total(chart_count);
    let control = BakeControl::new(Arc::clone(&governor), &progress);
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();

    let worker = thread::spawn(move || {
        let light_refs: Vec<&MapLight> = lights.iter().collect();
        started_tx.send(()).expect("test coordinator is waiting");
        ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .expect("multi-worker rayon pool")
            .install(|| {
                bake_monolithic_atlas_controlled(
                    &bvh,
                    &primitives,
                    &geometry,
                    &light_refs,
                    &prepared.charts,
                    &prepared.placements,
                    prepared.atlas_width,
                    prepared.atlas_height,
                    prepared.layer_count,
                    DEFAULT_AREA_SAMPLE_COUNT,
                    &control,
                )
            });
        finished_tx.send(()).expect("test coordinator is waiting");
    });

    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("bake worker did not start");
    // Holding the only permit ensures there is no in-flight chart. Once the
    // pause is set, each queued `enter` must observe it before admission.
    governor.set_paused(true);
    drop(held_permit);
    assert_eq!(
        progress.completed(),
        0,
        "no monolithic chart may advance after pause and before admission resumes"
    );

    governor.set_paused(false);
    finished_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("monolithic bake did not resume after unpausing");
    worker.join().expect("bake worker must not panic");
    assert_eq!(progress.completed(), chart_count);
    assert_eq!(
        progress.total(),
        Some(progress.completed()),
        "resumed monolithic bake must advance its published chart total exactly"
    );
}

/// Cache-determinism guard: the lightmap bake is consumed by the build-stage
/// cache, which keys cache entries on input hash and reuses stored output
/// verbatim. Any run-to-run drift in the encoded section (HashMap iteration
/// order leaking into output, parallel-reduce sums with variable ordering,
/// RNG without a fixed seed) would defeat the cache. This test fails fast
/// if any such drift is reintroduced into the bake path.
#[test]
fn lightmap_bake_produces_byte_identical_output_on_repeated_runs() {
    fn run_bake() -> Vec<u8> {
        let mut geo = unit_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light_above()];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
            scale_regions: &[],
        };
        let out = bake_lightmap(
            &mut inputs,
            &LightmapConfig {
                lightmap_density: 0.25,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                direction_texel_scale: DIRECTION_TEXEL_SCALE,
                uncompressed_irradiance: false,
            },
        )
        .unwrap();
        // LightmapSection has a defined on-disk byte layout — comparing
        // those bytes is the same check the cache substrate applies.
        out.section.to_bytes()
    }

    let bytes_a = run_bake();
    let bytes_b = run_bake();
    assert_eq!(
        bytes_a, bytes_b,
        "lightmap bake output drifted between runs; the build-stage cache requires \
             byte-identical output for identical inputs",
    );
}

/// Task 4a — determinism via decode. The plan AC reads: "Two `--no-cache`
/// bakes of the same input each decode within tolerance — byte-equality
/// not required." This test models that contract directly: run the bake
/// twice on identical inputs (the in-process equivalent of `--no-cache`,
/// which would force the cache substrate to re-invoke the bake), decode
/// each BC6H block back to f16 through the same hardware-matching
/// reference decoder the round-trip test uses, and assert per-channel
/// per-texel agreement within the same frozen `BC6H_ROUNDTRIP_TOLERANCE_REL`
/// the round-trip AC gates against.
///
/// This is *not* redundant with `lightmap_bake_produces_byte_identical_output_on_repeated_runs`:
/// that sibling locks down byte-equality (a stricter cache-friendliness
/// invariant). This one is the plan-stated decoded-tolerance gate, so a
/// future encoder swap that produces non-byte-identical-but-still-correct
/// output (e.g. a cluster-fit refinement) keeps the determinism AC green
/// while the byte-identity test is updated alongside the encoder.
#[test]
fn two_bakes_decode_within_frozen_tolerance() {
    fn run_bake() -> LightmapSection {
        let mut geo = unit_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light_above()];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
            scale_regions: &[],
        };
        bake_lightmap(
            &mut inputs,
            &LightmapConfig {
                lightmap_density: 0.25,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                direction_texel_scale: DIRECTION_TEXEL_SCALE,
                uncompressed_irradiance: false,
            },
        )
        .unwrap()
        .section
    }

    let a = run_bake();
    let b = run_bake();

    // The bake's `unit_quad_geometry` exercises the BC6H path (default
    // config, `uncompressed_irradiance = false`). The two sections must
    // agree on dimensions and irradiance format before we can do block-
    // wise decode comparison.
    let extents = |s: &LightmapSection| {
        s.blocks
            .iter()
            .map(|b| (b.cell_id, b.width, b.height))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        extents(&a),
        extents(&b),
        "block layout drifted between runs"
    );
    assert_eq!(
        a.irradiance_format,
        postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H,
        "determinism gate is BC6H-specific; bake unexpectedly emitted a different format",
    );
    assert_eq!(
        a.irradiance_format, b.irradiance_format,
        "irradiance format drifted between runs",
    );
    let (irradiance_a, irradiance_b) = (all_irradiance(&a), all_irradiance(&b));
    assert_eq!(
        irradiance_a.len(),
        irradiance_b.len(),
        "irradiance blob length drifted; block math diverged across runs",
    );

    // Block-wise decode + relative-error comparison. The decoded f16 bits
    // are first lifted to f32 so the relative-error metric matches the
    // round-trip AC's gauge (a per-channel epsilon-floored ratio).
    let blocks = irradiance_a.len() / 16;
    for bi in 0..blocks {
        let off = bi * 16;
        let block_a: [u8; 16] = irradiance_a[off..off + 16].try_into().unwrap();
        let block_b: [u8; 16] = irradiance_b[off..off + 16].try_into().unwrap();
        let decoded_a = crate::bc6h::decode_bc6h_block_for_tests(&block_a);
        let decoded_b = crate::bc6h::decode_bc6h_block_for_tests(&block_b);
        for t in 0..16 {
            for c in 0..3 {
                let va = crate::bc6h::f16_bits_to_f32(decoded_a[t][c]);
                let vb = crate::bc6h::f16_bits_to_f32(decoded_b[t][c]);
                let denom = va.abs().max(vb.abs()).max(1.0e-3);
                let rel = (va - vb).abs() / denom;
                assert!(
                    rel <= crate::bc6h::BC6H_ROUNDTRIP_TOLERANCE_REL,
                    "block {bi} texel {t} c{c}: two-bake relative drift {rel} > frozen \
                         tolerance {} (a={va}, b={vb})",
                    crate::bc6h::BC6H_ROUNDTRIP_TOLERANCE_REL,
                );
            }
        }
    }
}

/// Task 4a — atlas sizing. Across a representative chart set, the packed
/// atlas dimensions must be (a) power-of-two, (b) 4-aligned (BC6H block
/// requirement — satisfied for free when dimensions are power-of-two ≥ 4,
/// but pinned here as a contract because the bake's BC6H sizing math
/// (`ceil(w/4)·ceil(h/4)·16`) silently misreports if either axis is < 4),
/// and (c) ≤ `MAX_ATLAS_DIMENSION`. All charts here share one leaf, so the
/// packer resolves them into a single layer; this table-driven test extends
/// dimension coverage across a representative breadth so a regression that
/// only triggers on, e.g., narrow-tall sets is still caught.
#[test]
fn pack_layers_dims_are_pow2_4aligned_under_cap_across_chart_sets() {
    // Table of chart sets, each producing an atlas that the packer should
    // resolve under the 8192 cap. Mixed shapes: square small, square
    // large, narrow-tall, wide-short, and a single-chart degenerate.
    let cases: &[(&str, Vec<Chart>)] = &[
        ("single small square", vec![synthetic_chart(64, 64)]),
        (
            "many small squares",
            (0..16).map(|_| synthetic_chart(128, 128)).collect(),
        ),
        (
            "narrow-tall column",
            (0..4).map(|_| synthetic_chart(256, 2048)).collect(),
        ),
        (
            "wide-short row",
            (0..4).map(|_| synthetic_chart(2048, 256)).collect(),
        ),
        (
            "mixed shapes",
            vec![
                synthetic_chart(512, 256),
                synthetic_chart(256, 512),
                synthetic_chart(1024, 128),
                synthetic_chart(128, 1024),
                synthetic_chart(256, 256),
            ],
        ),
    ];
    for (label, charts) in cases {
        let pack = pack_layers(charts, MAX_ATLAS_DIMENSION, 0.25)
            .unwrap_or_else(|e| panic!("{label}: pack_layers failed: {e:?}"));
        let (w, h) = (pack.atlas_width, pack.atlas_height);
        assert_eq!(
            pack.layer_count, 1,
            "{label}: single-leaf charts must pack into one layer, got {}",
            pack.layer_count,
        );
        assert!(
            w.is_power_of_two(),
            "{label}: atlas width must be power-of-two, got {w}",
        );
        assert!(
            h.is_power_of_two(),
            "{label}: atlas height must be power-of-two, got {h}",
        );
        assert!(w >= 4, "{label}: width must be ≥4 (BC6H block), got {w}");
        assert!(h >= 4, "{label}: height must be ≥4 (BC6H block), got {h}");
        assert_eq!(
            w % 4,
            0,
            "{label}: width must be 4-aligned (BC6H block), got {w}",
        );
        assert_eq!(
            h % 4,
            0,
            "{label}: height must be 4-aligned (BC6H block), got {h}",
        );
        assert!(
            w <= MAX_ATLAS_DIMENSION,
            "{label}: width must be ≤cap ({MAX_ATLAS_DIMENSION}), got {w}",
        );
        assert!(
            h <= MAX_ATLAS_DIMENSION,
            "{label}: height must be ≤cap ({MAX_ATLAS_DIMENSION}), got {h}",
        );
    }
}

#[test]
fn lightmap_uvs_in_zero_one_range_after_bake() {
    let mut geo = unit_quad_geometry();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let lights = vec![point_light_above()];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &prims,
        geometry: &mut geo,
        lights: &static_lights,
        scale_regions: &[],
    };
    let _ = bake_lightmap(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: false,
        },
    )
    .unwrap();
    for v in &geo.geometry.vertices {
        let uv = v.decode_lightmap_uv();
        assert!(
            uv[0] >= 0.0 && uv[0] <= 1.0,
            "lightmap u out of range: {}",
            uv[0]
        );
        assert!(
            uv[1] >= 0.0 && uv[1] <= 1.0,
            "lightmap v out of range: {}",
            uv[1]
        );
    }
}

#[test]
fn occluder_produces_dark_texel() {
    // Build two parallel quads: a floor and a ceiling blocker.
    // Light is above the ceiling, so the floor should see zero irradiance.
    let floor = vec![
        Vertex::new(
            [0.0, 0.0, 0.0],
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
        Vertex::new(
            [2.0, 0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
        Vertex::new(
            [2.0, 0.0, 2.0],
            [1.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
        Vertex::new(
            [0.0, 0.0, 2.0],
            [0.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
    ];
    let ceiling = vec![
        Vertex::new(
            [-2.0, 1.0, -2.0],
            [0.0, 0.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
        Vertex::new(
            [4.0, 1.0, -2.0],
            [1.0, 0.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
        Vertex::new(
            [4.0, 1.0, 4.0],
            [1.0, 1.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
        Vertex::new(
            [-2.0, 1.0, 4.0],
            [0.0, 1.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ),
    ];
    let mut vertices = floor;
    vertices.extend(ceiling);
    let indices = vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
    let faces = vec![
        FaceMeta {
            leaf_index: 0,
            texture_index: 0,
        },
        FaceMeta {
            leaf_index: 0,
            texture_index: 0,
        },
    ];
    let face_index_ranges = vec![
        FaceIndexRange {
            index_offset: 0,
            index_count: 6,
        },
        FaceIndexRange {
            index_offset: 6,
            index_count: 6,
        },
    ];
    let mut geo = GeometryResult {
        geometry: GeometrySection {
            vertices,
            indices,
            faces,
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges,
    };

    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let light = MapLight {
        origin: DVec3::new(1.0, 2.0, 1.0),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 10.0,
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
        shadow_type: crate::map_data::ShadowType::StaticLightMap,
    };
    let lights = vec![light];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &prims,
        geometry: &mut geo,
        lights: &static_lights,
        scale_regions: &[],
    };
    let section = bake_lightmap(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            // Reads per-texel bytes — request the RGBA16F debug bypass
            // so the 8-byte stride matches what this loop expects.
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: true,
        },
    )
    .unwrap()
    .section;

    let irradiance = all_irradiance(&section);
    let mut zero_count = 0;
    for t in 0..irradiance.len() / 8 {
        let r_bits = u16::from_le_bytes([irradiance[t * 8], irradiance[t * 8 + 1]]);
        if r_bits == 0 {
            zero_count += 1;
        }
    }
    assert!(
        zero_count > 0,
        "expected at least one occluded (zero-irradiance) texel",
    );
}

#[test]
fn oversize_face_returns_error_rather_than_panicking() {
    // Regression: the old path clamped atlas_h but left chart placements at pre-clamp
    // coordinates, causing out-of-bounds writes during bake and dilation.
    let size = 400.0; // 10000 texels at 0.04 m/texel, beyond one pool layer
    let v0 = Vertex::new(
        [0.0, 0.0, 0.0],
        [0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    );
    let v1 = Vertex::new(
        [size, 0.0, 0.0],
        [1.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    );
    let v2 = Vertex::new(
        [size, 0.0, size],
        [1.0, 1.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    );
    let v3 = Vertex::new(
        [0.0, 0.0, size],
        [0.0, 1.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    );
    let mut geo = GeometryResult {
        geometry: GeometrySection {
            vertices: vec![v0, v1, v2, v3],
            indices: vec![0, 1, 2, 0, 2, 3],
            faces: vec![FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            }],
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: vec![FaceIndexRange {
            index_offset: 0,
            index_count: 6,
        }],
    };
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let lights = vec![point_light_above()];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &prims,
        geometry: &mut geo,
        lights: &static_lights,
        scale_regions: &[],
    };
    let result = bake_lightmap(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: DEFAULT_TEXEL_DENSITY_METERS,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: false,
        },
    );
    // A 400 m face is 10000 texels at 0.04 m/texel: its chart cannot fit one
    // runtime pool layer, so the build fails naming the face before packing.
    match result {
        Err(LightmapBakeError::ChartTooLarge {
            face_index: 0, max, ..
        }) if max == postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE => {}
        other => panic!("expected ChartTooLarge error, got {other:?}"),
    }
}

#[test]
fn shared_world_vertex_produces_two_distinct_records() {
    let shared = Vertex::new(
        [0.0, 0.0, 0.0],
        [0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    );
    let a1 = Vertex::new(
        [1.0, 0.0, 0.0],
        [1.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    );
    let a2 = Vertex::new(
        [1.0, 0.0, 1.0],
        [1.0, 1.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    );
    let b1 = Vertex::new(
        [0.0, 0.0, 1.0],
        [0.0, 1.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    );
    let b2 = Vertex::new(
        [-1.0, 0.0, 1.0],
        [-1.0, 1.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    );
    let mut geo = GeometryResult {
        geometry: GeometrySection {
            vertices: vec![shared, a1, a2, b1, b2],
            indices: vec![0, 1, 2, 0, 3, 4],
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
            FaceIndexRange {
                index_offset: 0,
                index_count: 3,
            },
            FaceIndexRange {
                index_offset: 3,
                index_count: 3,
            },
        ],
    };
    let original_vertex_count = geo.geometry.vertices.len();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let lights = vec![point_light_above()];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &prims,
        geometry: &mut geo,
        lights: &static_lights,
        scale_regions: &[],
    };
    let _ = bake_lightmap(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: 0.25,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: false,
        },
    )
    .unwrap();

    assert!(
        geo.geometry.vertices.len() > original_vertex_count,
        "expected at least one duplicated vertex after split, got {} (was {})",
        geo.geometry.vertices.len(),
        original_vertex_count,
    );
    let face_b_start = geo.face_index_ranges[1].index_offset as usize;
    let face_b_first_vi = geo.geometry.indices[face_b_start];
    assert_ne!(
        face_b_first_vi, 0,
        "face B should reference a duplicated vertex, not the original shared one",
    );
    let original = &geo.geometry.vertices[0];
    let duplicate = &geo.geometry.vertices[face_b_first_vi as usize];
    assert_eq!(original.position, duplicate.position);
    assert_eq!(original.uv, duplicate.uv);
    assert_eq!(original.normal_oct, duplicate.normal_oct);
    assert_eq!(original.tangent_packed, duplicate.tangent_packed);
}

// --- soft_visibility -------------------------------------------------
//
// These exercise the trace-context-agnostic helper with mock `trace`
// closures, so they need no BVH/geometry — the closure stands in for
// each caller's `segment_clear`.

use crate::map_data::{DEFAULT_ANGULAR_DIAMETER_DEG, DEFAULT_LIGHT_SIZE};

fn soft_point_light(size: f32) -> MapLight {
    let mut l = point_light_above();
    l.light_size = size;
    l
}

fn soft_directional_light(angular_diameter: f32) -> MapLight {
    let mut l = point_light_above();
    l.light_type = LightType::Directional;
    l.cone_direction = Some([0.0, -1.0, 0.0]);
    l.angular_diameter = angular_diameter;
    l
}

const EPS: f32 = 1.0e-5;

#[test]
fn soft_visibility_zero_light_size_matches_hard_ray() {
    // An authored `0` must reproduce the single hard ray exactly: 1.0 clear,
    // 0.0 blocked — preserving an explicit hard edge.
    let light = soft_point_light(0.0);
    let clear = soft_visibility(
        Vec3::ZERO,
        Vec3::Y,
        &light,
        1,
        DEFAULT_AREA_SAMPLE_COUNT,
        |_, _| true,
    );
    let blocked = soft_visibility(
        Vec3::ZERO,
        Vec3::Y,
        &light,
        1,
        DEFAULT_AREA_SAMPLE_COUNT,
        |_, _| false,
    );
    assert!((clear - 1.0).abs() < EPS, "clear got {clear}");
    assert!(blocked.abs() < EPS, "blocked got {blocked}");
}

#[test]
fn soft_visibility_zero_angular_diameter_matches_hard_ray() {
    let light = soft_directional_light(0.0);
    let clear = soft_visibility(
        Vec3::ZERO,
        Vec3::Y,
        &light,
        1,
        DEFAULT_AREA_SAMPLE_COUNT,
        |_, _| true,
    );
    let blocked = soft_visibility(
        Vec3::ZERO,
        Vec3::Y,
        &light,
        1,
        DEFAULT_AREA_SAMPLE_COUNT,
        |_, _| false,
    );
    assert!((clear - 1.0).abs() < EPS, "clear got {clear}");
    assert!(blocked.abs() < EPS, "blocked got {blocked}");
}

#[test]
fn soft_visibility_zero_size_single_ray_equals_shadow_visible_target() {
    // The hard-ray short-circuit must hit the same target shadow_visible uses,
    // so a closure that only passes that exact target returns fully visible.
    let light = soft_point_light(0.0);
    let surface = Vec3::new(0.5, 0.0, 0.5);
    let expected_target = Vec3::new(
        light.origin.x as f32,
        light.origin.y as f32,
        light.origin.z as f32,
    );
    let v = soft_visibility(
        surface,
        Vec3::Y,
        &light,
        0,
        DEFAULT_AREA_SAMPLE_COUNT,
        |_, to| (to - expected_target).length() < EPS,
    );
    assert!((v - 1.0).abs() < EPS, "got {v}");
}

#[test]
fn soft_visibility_full_clear_is_one() {
    let light = soft_point_light(DEFAULT_LIGHT_SIZE);
    let v = soft_visibility(
        Vec3::ZERO,
        Vec3::Y,
        &light,
        42,
        DEFAULT_AREA_SAMPLE_COUNT,
        |_, _| true,
    );
    assert!((v - 1.0).abs() < EPS, "got {v}");
}

#[test]
fn soft_visibility_full_block_is_zero() {
    let light = soft_point_light(DEFAULT_LIGHT_SIZE);
    let v = soft_visibility(
        Vec3::ZERO,
        Vec3::Y,
        &light,
        42,
        DEFAULT_AREA_SAMPLE_COUNT,
        |_, _| false,
    );
    assert!(v.abs() < EPS, "got {v}");
}

#[test]
fn soft_visibility_partial_occluder_is_fractional() {
    // Mock occluder: a half-plane through the light origin. Samples on the +X
    // half of the emitter sphere are blocked, the -X half clear — guaranteeing
    // a penumbra with the escalated sample set.
    let light = soft_point_light(0.5);
    let center = Vec3::new(
        light.origin.x as f32,
        light.origin.y as f32,
        light.origin.z as f32,
    );
    let v = soft_visibility(
        Vec3::ZERO,
        Vec3::Y,
        &light,
        9,
        DEFAULT_AREA_SAMPLE_COUNT,
        |_, to| (to - center).x <= 0.0,
    );
    assert!(v > 0.0 && v < 1.0, "expected penumbra fraction, got {v}");
}

#[test]
fn soft_visibility_is_deterministic_across_repeated_calls() {
    // Same inputs incl. seed → identical output, no RNG, no hash-order
    // dependence. A position-dependent occluder forces the escalated path.
    let light = soft_point_light(0.5);
    let surface = Vec3::new(0.5, 0.0, 0.5);
    let occluder = |_from: Vec3, to: Vec3| to.x <= 0.5;
    let a = soft_visibility(
        surface,
        Vec3::Y,
        &light,
        0xABCD,
        DEFAULT_AREA_SAMPLE_COUNT,
        occluder,
    );
    let b = soft_visibility(
        surface,
        Vec3::Y,
        &light,
        0xABCD,
        DEFAULT_AREA_SAMPLE_COUNT,
        occluder,
    );
    let c = soft_visibility(
        surface,
        Vec3::Y,
        &light,
        0xABCD,
        DEFAULT_AREA_SAMPLE_COUNT,
        occluder,
    );
    assert_eq!(a.to_bits(), b.to_bits(), "repeat 1 diverged: {a} vs {b}");
    assert_eq!(a.to_bits(), c.to_bits(), "repeat 2 diverged: {a} vs {c}");
}

/// F1 regression: an occluder clipping the emitter's *lower* hemisphere — the
/// orientation the old contiguous probe set (`0,1,2,3`, all clustered at the
/// emitter's top cap) missed — must now yield a fractional `soft_visibility`,
/// not a hard 0/1. The strided probe subset (`{0,8,16,24}` for full=32) places
/// probes in both the upper and lower halves, so the split forces escalation.
/// Block rays toward the lower half of the emitter sphere (sample `y` below the
/// light center), pass the upper half.
#[test]
fn soft_visibility_lower_hemisphere_occluder_is_fractional() {
    let light = soft_point_light(0.5);
    let center = Vec3::new(
        light.origin.x as f32,
        light.origin.y as f32,
        light.origin.z as f32,
    );
    // Clear (true) for the upper half of the emitter, blocked for the lower.
    let v = soft_visibility(
        Vec3::ZERO,
        Vec3::Y,
        &light,
        13,
        DEFAULT_AREA_SAMPLE_COUNT,
        |_, to| (to - center).y >= 0.0,
    );
    assert!(
        v > 0.0 && v < 1.0,
        "lower-hemisphere occluder should be a penumbra, got {v} \
             (clustered top-cap probes would have early-out to a hard edge)"
    );
}

/// Regression: a sub-0.05° directional emitter baked at the minimum sample
/// count (`full_samples == SOFT_PROBE_SAMPLES == 4`) used to collapse its four
/// probe indices (`probe_indices` snapped multiple probes to index 0), so a
/// partially-occluded texel double-counted a clear sample and early-out to a
/// hard 1.0 instead of a penumbra. With distinct probe indices the genuine
/// split now bakes a fraction. At 0.03° / full=4 the four cone samples have
/// x-offsets `{0, 0, -3.4, +2.8}`, so `to.x < 0.0` blocks exactly one sample.
#[test]
fn soft_visibility_narrow_directional_at_min_samples_is_fractional() {
    let light = soft_directional_light(0.03);
    let full_samples = SOFT_PROBE_SAMPLES; // 4 — the minimum, where escalation
    // has no extra samples to recover a collapsed probe set.
    let v = soft_visibility(Vec3::ZERO, Vec3::Y, &light, 13, full_samples, |_, to| {
        to.x >= 0.0
    });
    assert!(
        v > 0.0 && v < 1.0,
        "narrow directional at min samples should be a penumbra, got {v} \
             (a duplicated probe index would have double-counted to a hard 0/1)"
    );
    // Exactly one of the four distinct samples is blocked → 3/4 clear. A
    // duplicated index would skew this off the 4-sample grid.
    assert!((v - 0.75).abs() < EPS, "expected 3/4 clear, got {v}");
}

#[test]
fn soft_visibility_directional_partial_occluder_is_fractional() {
    // Wide cone so jittered directions spread; occluder splits the cone.
    let light = soft_directional_light(20.0);
    let v = soft_visibility(
        Vec3::ZERO,
        Vec3::Y,
        &light,
        3,
        DEFAULT_AREA_SAMPLE_COUNT,
        |_, to| to.x <= 0.0,
    );
    assert!(v > 0.0 && v < 1.0, "expected penumbra fraction, got {v}");
}

proptest::proptest! {
    // The unoccluded fraction is always a valid probability for any seed and
    // any deterministic occluder pattern keyed on the sample index.
    #[test]
    fn soft_visibility_always_in_unit_interval(
        seed in proptest::prelude::any::<u64>(),
        size in 0.0f32..2.0,
        pattern in proptest::prelude::any::<u32>(),
    ) {
        let light = soft_point_light(size);
        // Pseudo-arbitrary but deterministic clear/block decision per call:
        // `Cell` gives the `Fn` closure a per-sample counter without `FnMut`.
        let counter = std::cell::Cell::new(0u32);
        let occluder = |_from: Vec3, _to: Vec3| {
            let i = counter.get();
            counter.set(i + 1);
            (pattern >> (i % 32)) & 1 == 0
        };
        let v = soft_visibility(Vec3::ZERO, Vec3::Y, &light, seed, DEFAULT_AREA_SAMPLE_COUNT, occluder);
        proptest::prop_assert!((0.0..=1.0).contains(&v), "v out of range: {v}");
    }

    #[test]
    fn soft_visibility_directional_always_in_unit_interval(
        seed in proptest::prelude::any::<u64>(),
        angular in 0.0f32..45.0,
        all_clear in proptest::prelude::any::<bool>(),
    ) {
        let light = soft_directional_light(angular);
        let v = soft_visibility(Vec3::ZERO, Vec3::Y, &light, seed, DEFAULT_AREA_SAMPLE_COUNT, |_, _| all_clear);
        proptest::prop_assert!((0.0..=1.0).contains(&v), "v out of range: {v}");
    }
}

#[test]
fn soft_visibility_default_sizes_softpath_disagreeing_probes_escalate() {
    // Sanity: documented nonzero defaults take the soft path (not the hard
    // short-circuit), so a split occluder yields a fraction, not 0/1.
    let point = soft_point_light(DEFAULT_LIGHT_SIZE);
    let center = Vec3::new(
        point.origin.x as f32,
        point.origin.y as f32,
        point.origin.z as f32,
    );
    let pv = soft_visibility(
        Vec3::ZERO,
        Vec3::Y,
        &point,
        11,
        DEFAULT_AREA_SAMPLE_COUNT,
        |_, to| (to - center).x <= 0.0,
    );
    assert!(pv > 0.0 && pv < 1.0, "point default not soft: {pv}");

    let dir = soft_directional_light(DEFAULT_ANGULAR_DIAMETER_DEG);
    // Directional default is a narrow 0.5°; a split still yields a fraction.
    let dv = soft_visibility(
        Vec3::ZERO,
        Vec3::Y,
        &dir,
        11,
        DEFAULT_AREA_SAMPLE_COUNT,
        |_, to| to.x <= 0.0,
    );
    assert!(
        (0.0..=1.0).contains(&dv),
        "directional default out of range: {dv}"
    );
}

// --- Task 6: sub-texel-penumbra author hint --------------------------
//
// Test the pure predicate `penumbra_below_one_texel` (and the
// `warn_sub_texel_penumbra_lights` count via it), not the `log::warn!`
// macro — the warning is gated entirely by this predicate, so its branch
// is the testable seam.

#[test]
fn penumbra_below_one_texel_flags_tiny_point_emitter() {
    // A near-zero (but nonzero) emitter over a long reach subtends a
    // sub-texel penumbra at the default atlas density → flagged.
    let mut light = soft_point_light(0.001);
    light.falloff_range = 5.0;
    assert!(
        penumbra_below_one_texel(&light, DEFAULT_TEXEL_DENSITY_METERS),
        "tiny emitter should be flagged sub-texel"
    );
}

#[test]
fn penumbra_below_one_texel_passes_default_sized_point_emitter() {
    // The documented default `_light_size` is sized to span ~multiple
    // texels at the default density → not flagged.
    let mut light = soft_point_light(DEFAULT_LIGHT_SIZE);
    light.falloff_range = 5.0;
    assert!(
        !penumbra_below_one_texel(&light, DEFAULT_TEXEL_DENSITY_METERS),
        "default-sized emitter must not be flagged"
    );
}

#[test]
fn penumbra_below_one_texel_does_not_flag_explicit_hard_light() {
    // An explicitly-authored hard light (size 0) opted into a hard edge —
    // a "too soft" hint would be noise, so it is never flagged.
    let mut light = soft_point_light(0.0);
    light.falloff_range = 5.0;
    assert!(
        !penumbra_below_one_texel(&light, DEFAULT_TEXEL_DENSITY_METERS),
        "explicit hard light (size 0) must not be flagged"
    );
    let dir = soft_directional_light(0.0);
    assert!(
        !penumbra_below_one_texel(&dir, DEFAULT_TEXEL_DENSITY_METERS),
        "explicit hard directional (angular 0) must not be flagged"
    );
}

#[test]
fn penumbra_below_one_texel_flags_narrow_directional() {
    // A 0.001° sun over a unit reach is well below one texel → flagged;
    // a wide 5° sun is not.
    let mut narrow = soft_directional_light(0.001);
    narrow.falloff_range = 1.0;
    assert!(
        penumbra_below_one_texel(&narrow, DEFAULT_TEXEL_DENSITY_METERS),
        "narrow directional should be flagged sub-texel"
    );
    let mut wide = soft_directional_light(5.0);
    wide.falloff_range = 1.0;
    assert!(
        !penumbra_below_one_texel(&wide, DEFAULT_TEXEL_DENSITY_METERS),
        "wide directional must not be flagged"
    );
}

#[test]
fn warn_sub_texel_penumbra_counts_only_below_threshold_lights() {
    // Mixed set: one flagged (tiny), one not (default). The predicate that
    // gates the per-light `log::warn!` must fire for exactly one of them.
    let mut tiny = soft_point_light(0.001);
    tiny.falloff_range = 5.0;
    let mut ok = soft_point_light(DEFAULT_LIGHT_SIZE);
    ok.falloff_range = 5.0;
    let lights = [&tiny, &ok];
    let flagged = lights
        .iter()
        .filter(|l| penumbra_below_one_texel(l, DEFAULT_TEXEL_DENSITY_METERS))
        .count();
    assert_eq!(flagged, 1, "exactly one light should warn");
    // Smoke: the warning emitter runs without panicking on the same set.
    warn_sub_texel_penumbra_lights(&lights, DEFAULT_TEXEL_DENSITY_METERS);
}

// --- Task 6: area-sample-count knob ----------------------------------

#[test]
fn soft_visibility_knob_below_probe_floor_is_clamped() {
    // A knob below the fixed probe count must not panic or invert the
    // prefix invariant — it clamps up to the probe floor. Full-clear and
    // full-block still resolve to 1.0 / 0.0.
    let light = soft_point_light(0.5);
    let clear = soft_visibility(Vec3::ZERO, Vec3::Y, &light, 1, 1, |_, _| true);
    let blocked = soft_visibility(Vec3::ZERO, Vec3::Y, &light, 1, 1, |_, _| false);
    assert!((clear - 1.0).abs() < EPS, "clamped clear got {clear}");
    assert!(blocked.abs() < EPS, "clamped blocked got {blocked}");
}

#[test]
fn soft_visibility_higher_knob_changes_penumbra_fraction_resolution() {
    // Raising the knob raises the stratification denominator, so a penumbra
    // fraction is quantized more finely. The two counts trace different
    // sample sets, so the returned fractions differ — proving the knob
    // reaches `soft_visibility`'s full-sample target.
    let light = soft_point_light(0.5);
    let center = Vec3::new(
        light.origin.x as f32,
        light.origin.y as f32,
        light.origin.z as f32,
    );
    let occluder = |_from: Vec3, to: Vec3| (to - center).x <= 0.0;
    let low = soft_visibility(Vec3::ZERO, Vec3::Y, &light, 9, 16, occluder);
    let high = soft_visibility(Vec3::ZERO, Vec3::Y, &light, 9, 64, occluder);
    assert!(low > 0.0 && low < 1.0, "low knob not a penumbra: {low}");
    assert!(high > 0.0 && high < 1.0, "high knob not a penumbra: {high}");
    assert_ne!(
        low.to_bits(),
        high.to_bits(),
        "knob did not change full-sample resolution: {low} vs {high}"
    );
}

// --- Task 3: static lightmap soft sum (full bake) --------------------
//
// These drive the real per-texel bake (BVH + `segment_clear`), unlike the
// mock-closure tests above which exercise `soft_visibility` in isolation.

/// A large floor at y=0 plus a small horizontal occluder quad floating above
/// it, positioned so a light above the occluder casts a shadow with a
/// penumbra band onto the floor. The occluder is small relative to the floor
/// so the shadow edge falls on the floor's interior texels.
fn box_on_floor_geometry() -> GeometryResult {
    // Floor: 4x4 m quad centered at origin, upward normal.
    let floor = [
        ([-2.0, 0.0, -2.0], [0.0, 0.0]),
        ([2.0, 0.0, -2.0], [1.0, 0.0]),
        ([2.0, 0.0, 2.0], [1.0, 1.0]),
        ([-2.0, 0.0, 2.0], [0.0, 1.0]),
    ];
    // Occluder: 1x1 m quad at y=1, downward normal (faces the floor). Sits
    // between the light (high above) and the floor center.
    let occluder = [
        ([-0.5, 1.0, -0.5], [0.0, 0.0]),
        ([0.5, 1.0, -0.5], [1.0, 0.0]),
        ([0.5, 1.0, 0.5], [1.0, 1.0]),
        ([-0.5, 1.0, 0.5], [0.0, 1.0]),
    ];
    let mut vertices = Vec::new();
    for (pos, uv) in floor {
        vertices.push(Vertex::new(
            pos,
            uv,
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ));
    }
    for (pos, uv) in occluder {
        vertices.push(Vertex::new(
            pos,
            uv,
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ));
    }
    let indices = vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
    let faces = vec![
        FaceMeta {
            leaf_index: 0,
            texture_index: 0,
        },
        FaceMeta {
            leaf_index: 0,
            texture_index: 0,
        },
    ];
    let face_index_ranges = vec![
        FaceIndexRange {
            index_offset: 0,
            index_count: 6,
        },
        FaceIndexRange {
            index_offset: 6,
            index_count: 6,
        },
    ];
    GeometryResult {
        geometry: GeometrySection {
            vertices,
            indices,
            faces,
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges,
    }
}

/// Point light high above the occluder center, default soft size.
fn soft_overhead_light() -> MapLight {
    let mut l = point_light_above();
    l.origin = DVec3::new(0.0, 4.0, 0.0);
    l.falloff_range = 20.0;
    l.light_size = DEFAULT_LIGHT_SIZE;
    l
}

/// Decode every block's irradiance texels (R channel) from an RGBA16F section.
fn floor_irradiance_r(section: &LightmapSection) -> Vec<f32> {
    all_irradiance(section)
        .chunks_exact(8)
        .map(|texel| f16_bits_to_f32(u16::from_le_bytes([texel[0], texel[1]])))
        .collect()
}

/// Every block's irradiance blob, in block order.
fn all_irradiance(section: &LightmapSection) -> Vec<u8> {
    section
        .blocks
        .iter()
        .flat_map(|block: &LightmapBlock| block.irradiance.iter().copied())
        .collect()
}

/// IEEE-754 half → f32 decode for reading baked irradiance back in tests.
/// Mirrors `sh_bake.rs`'s private decoder (the inverse of
/// `level-format`'s `f32_to_f16_bits`); duplicated here because that one is
/// a sibling-module private and level-format exposes no public decoder.
fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = (bits >> 15) & 0x1;
    let exp = (bits >> 10) & 0x1f;
    let mant = bits & 0x3ff;
    let value = if exp == 0 {
        (mant as f32) * 2.0f32.powi(-24)
    } else if exp == 0x1f {
        if mant == 0 { f32::INFINITY } else { f32::NAN }
    } else {
        let m = 1.0 + (mant as f32) / 1024.0;
        m * 2.0f32.powi(exp as i32 - 15)
    };
    if sign == 1 { -value } else { value }
}

/// Soft-sum penumbra: a default-sized area light over a box-on-floor scene
/// must bake a *gradient* of intermediate irradiance across the shadow
/// boundary, not the binary lit/dark step a hard `shadow_visible` gate
/// produces. We assert the floor has texels strictly between fully dark and
/// fully lit — the fractional `v` band that defines a penumbra.
#[test]
fn soft_sum_bakes_penumbra_gradient_not_hard_step() {
    let mut geo = box_on_floor_geometry();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let lights = vec![soft_overhead_light()];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &prims,
        geometry: &mut geo,
        lights: &static_lights,
        scale_regions: &[],
    };
    let section = bake_lightmap(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: 0.05,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            // `floor_irradiance_r` reads per-texel bytes from the
            // irradiance blob — request the RGBA16F debug bypass so the
            // 8-byte stride stays valid.
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: true,
        },
    )
    .unwrap()
    .section;

    let r = floor_irradiance_r(&section);
    let max_lit = r.iter().cloned().fold(0.0f32, f32::max);
    assert!(max_lit > 0.0, "expected some lit floor texels");

    // A penumbra texel: partially occluded, so its irradiance lands strictly
    // between full dark and (near-)full light. Fully-lit and fully-shadowed
    // texels are excluded by the margins.
    let lit_threshold = max_lit * 0.95;
    let dark_threshold = max_lit * 0.05;
    let penumbra_count = r
        .iter()
        .filter(|&&v| v > dark_threshold && v < lit_threshold)
        .count();
    assert!(
        penumbra_count > 1,
        "expected a multi-texel penumbra gradient, found {penumbra_count} intermediate texels \
             (max_lit={max_lit}); a hard shadow step would yield ~0",
    );
}

/// Soft-shadow determinism: re-baking the identical box-on-floor + soft-light
/// scene must produce byte-identical output. The per-texel seed is a fixed
/// `(x, y)` hash, so escalated penumbra sampling stays reproducible across
/// processes — the property the build cache relies on.
#[test]
fn soft_sum_bake_is_byte_identical_on_repeat() {
    fn run() -> Vec<u8> {
        let mut geo = box_on_floor_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![soft_overhead_light()];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let mut inputs = LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
            scale_regions: &[],
        };
        bake_lightmap(
            &mut inputs,
            &LightmapConfig {
                lightmap_density: 0.05,
                area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
                direction_texel_scale: DIRECTION_TEXEL_SCALE,
                uncompressed_irradiance: false,
            },
        )
        .unwrap()
        .section
        .to_bytes()
    }
    let a = run();
    let b = run();
    assert_eq!(
        a, b,
        "soft-shadow bake drifted between runs; the area-sample seed must be a fixed \
             (x, y) hash with no RNG or hash-order dependence",
    );
}

#[test]
fn direction_scale_one_emits_rg8_bytes() {
    let direction = vec![Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(-1.0, 0.0, 0.0)];
    let coverage = vec![true, true, false, true];
    let atlas = CompositedAtlas {
        irradiance: vec![0.0; 4 * 4],
        direction: direction.clone(),
        coverage: coverage.clone(),
        atlas_width: 2,
        atlas_height: 2,
        layer_count: 1,
    };
    let placements = [ChartPlacement {
        x: 0,
        y: 0,
        layer: 0,
    }];

    let section = atlas.encode_section(&BlockLayout::whole_layers(2, 2, &placements, 1), true);
    assert_eq!(section.direction_texel_scale, 1);
    assert_eq!(
        section.blocks[0].direction,
        encode_direction_rg8(&direction, &coverage),
        "factor 1 must encode only the octahedral channels"
    );

    let coarse = atlas.encode_section(&BlockLayout::whole_layers(2, 2, &placements, 2), true);
    assert_eq!(coarse.direction_texel_scale, 2);
    assert_eq!(coarse.blocks[0].irradiance, section.blocks[0].irradiance);
    assert_eq!(
        coarse.blocks[0].direction.len(),
        section.blocks[0].direction.len() / 4
    );
}

/// The CLI rejects these scales, but direct callers must still never produce
/// a zero or oversized direction scale in the header or the block alignment.
#[test]
fn block_layout_normalizes_out_of_range_direction_scales() {
    let charts = [synthetic_chart_leaf(8, 8, 0)];
    for (requested, expected) in [(0, 1), (3, 4), (128, 64)] {
        let pack = block_layout::pack_cell_blocks(
            &charts,
            BlockOrdering::by_cell_id(requested),
            &BakeControl::unrestricted(),
        )
        .unwrap();
        assert_eq!(pack.layout.direction_texel_scale, expected, "{requested}");
        let block = pack.layout.blocks[0];
        assert_eq!(block.width % pack.layout.alignment(), 0);
    }
}

#[test]
fn direction_reduction_uses_neutral_up_for_covered_cancelling_block() {
    let direction = vec![Vec3::X, -Vec3::X, Vec3::X, -Vec3::X];
    let coverage = vec![true; 4];

    let (reduced_direction, reduced_coverage) =
        reduce_direction_atlas(&direction, &coverage, 2, 2, 1, 2);

    assert_eq!(reduced_coverage, vec![true]);
    assert_eq!(
        reduced_direction,
        vec![Vec3::Y],
        "covered cancelling vectors must not normalize zero"
    );
    assert_eq!(
        encode_direction_rg8(&reduced_direction, &reduced_coverage),
        encode_direction_oct(Vec3::Y.to_array()).to_vec(),
        "the degenerate covered block must encode as neutral up deterministically"
    );
}

#[test]
fn direction_reduction_keeps_array_layers_separate() {
    let direction = vec![Vec3::X; 8]
        .into_iter()
        .chain(vec![-Vec3::X; 8])
        .collect::<Vec<_>>();
    let coverage = vec![true; direction.len()];
    let (reduced_direction, reduced_coverage) =
        reduce_direction_atlas(&direction, &coverage, 4, 2, 2, 2);

    assert_eq!(reduced_coverage, vec![true; 4]);
    assert_eq!(reduced_direction[..2], [Vec3::X; 2]);
    assert_eq!(reduced_direction[2..], [-Vec3::X; 2]);

    let atlas = CompositedAtlas {
        irradiance: vec![0.0; direction.len() * 4],
        direction,
        coverage,
        atlas_width: 4,
        atlas_height: 2,
        layer_count: 2,
    };
    let placements = [0, 1].map(|layer| ChartPlacement { x: 0, y: 0, layer });
    let section = atlas.encode_section(&BlockLayout::whole_layers(4, 2, &placements, 2), true);
    assert_eq!(section.blocks.len(), 2);
    for (block, expected) in section.blocks.iter().zip([Vec3::X, -Vec3::X]) {
        assert_eq!(block.direction.len(), 2 * 2);
        assert_eq!(
            block.direction[..2],
            encode_direction_oct(expected.to_array())
        );
    }
}

#[test]
fn direction_reduction_is_deterministic_in_fixed_row_major_order() {
    let direction = vec![Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X];
    let coverage = vec![true; 4];

    let first = reduce_direction_atlas(&direction, &coverage, 2, 2, 1, 2);
    let second = reduce_direction_atlas(&direction, &coverage, 2, 2, 1, 2);
    assert_eq!(first, second);

    let expected = (Vec3::X + Vec3::Y + Vec3::Z - Vec3::X).normalize();
    assert!(
        (first.0[0] - expected).length() <= 1.0e-6,
        "the block sum must follow the source row-major order"
    );
}

/// Append an axis-aligned horizontal quad `[x, x + size] × [z, z + size]` at
/// height `y`, facing `+Y`, as one face of `leaf`.
fn push_floor_quad(geometry: &mut GeometryResult, x: f32, z: f32, size: f32, y: f32, leaf: u32) {
    let mut quad = unit_quad_geometry();
    for vertex in &mut quad.geometry.vertices {
        vertex.position = [
            x + vertex.position[0] * size,
            y,
            z + vertex.position[2] * size,
        ];
    }
    quad.geometry.faces[0].leaf_index = leaf;
    let vertex_offset = geometry.geometry.vertices.len() as u32;
    let index_offset = geometry.geometry.indices.len() as u32;
    geometry.geometry.vertices.extend(quad.geometry.vertices);
    geometry.geometry.indices.extend(
        quad.geometry
            .indices
            .into_iter()
            .map(|index| index + vertex_offset),
    );
    geometry.geometry.faces.extend(quad.geometry.faces);
    geometry
        .face_index_ranges
        .extend(quad.face_index_ranges.into_iter().map(|mut range| {
            range.index_offset += index_offset;
            range
        }));
}

/// A 6 m floor in cell 1 under a 2 m occluder, lit by an area light so the
/// occluder casts a soft penumbra across the floor. With `far_quad`, an unlit
/// 3 m quad in cell 0 precedes them: it renumbers both faces and takes the
/// bake layer's corner, so cell 1's block lands beside it.
fn penumbra_cell_geometry(far_quad: bool) -> GeometryResult {
    let mut geometry = unit_quad_geometry();
    geometry.geometry.vertices.clear();
    geometry.geometry.indices.clear();
    geometry.geometry.faces.clear();
    geometry.face_index_ranges.clear();
    if far_quad {
        push_floor_quad(&mut geometry, 500.0, 500.0, 3.0, 0.0, 0);
    }
    push_floor_quad(&mut geometry, 0.0, 0.0, 6.0, 0.0, 1);
    push_floor_quad(&mut geometry, 2.0, 2.0, 2.0, 1.0, 1);
    geometry
}

fn penumbra_cell_bake(far_quad: bool) -> LightmapBakeOutput {
    let mut geometry = penumbra_cell_geometry(far_quad);
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let mut light = point_light_above();
    light.origin = DVec3::new(3.0, 3.0, 3.0);
    light.falloff_range = 8.0;
    light.light_size = 1.0;
    let lights = vec![light];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let mut inputs = LightmapBakeCtx {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &mut geometry,
        lights: &static_lights,
        scale_regions: &[],
    };
    bake_lightmap_controlled(
        &mut inputs,
        &LightmapConfig {
            lightmap_density: 0.1,
            area_sample_count: DEFAULT_AREA_SAMPLE_COUNT,
            direction_texel_scale: DIRECTION_TEXEL_SCALE,
            uncompressed_irradiance: true,
        },
        &BakeControl::unrestricted(),
    )
    .unwrap()
}

/// Soft-visibility seeds key on the chart, not its bake-layer coordinates or
/// face index: a chart that moves in the atlas because an unrelated quad
/// joined the map ahead of it bakes exactly the same texels.
#[test]
fn moved_and_renumbered_chart_bakes_identical_texels() {
    let alone = penumbra_cell_bake(false);
    let shifted = penumbra_cell_bake(true);

    // The floor is face 0 alone and face 1 once the far quad precedes it,
    // and its bake-layer placement moves.
    let (floor_alone, floor_shifted) = (alone.placements[0], shifted.placements[1]);
    assert_ne!(
        (floor_alone.x, floor_alone.y),
        (floor_shifted.x, floor_shifted.y),
        "the far quad must move the floor's bake-layer texel coordinates"
    );

    let cell_block = |section: &LightmapSection| {
        let blocks: Vec<_> = section.blocks.iter().filter(|b| b.cell_id == 1).collect();
        assert_eq!(blocks.len(), 1);
        blocks[0].clone()
    };
    let (before, after) = (cell_block(&alone.section), cell_block(&shifted.section));

    // The occluder's penumbra must reach the floor, or seeds never matter:
    // some irradiance texels sit strictly between the darkest and brightest.
    let red: Vec<f32> = before
        .irradiance
        .chunks_exact(8)
        .map(|texel| f16_bits_to_f32(u16::from_le_bytes([texel[0], texel[1]])))
        .collect();
    let (lo, hi) = red
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &r| (lo.min(r), hi.max(r)));
    let penumbra = red
        .iter()
        .filter(|&&r| r > lo + 0.05 * (hi - lo) && r < hi - 0.05 * (hi - lo))
        .count();
    assert!(
        penumbra > 10,
        "fixture must bake a penumbra, got {penumbra} texels"
    );

    assert_eq!((before.width, before.height), (after.width, after.height));
    assert_eq!(
        before.irradiance, after.irradiance,
        "irradiance moved with the chart"
    );
    assert_eq!(
        before.direction, after.direction,
        "direction moved with the chart"
    );
}

/// A sub-chart window is a view onto its parent's grid: each of its texels
/// has the parent texel's exact world position and seed, and a vertex maps to
/// the parent-grid position shifted by the window origin.
#[test]
fn chart_window_texels_and_vertices_are_the_parents_bit_for_bit() {
    use crate::chart_raster::{CHART_PADDING_TEXELS, chart_texel_seed, chart_texel_world_position};

    let mut geometry = unit_quad_geometry();
    for vertex in &mut geometry.geometry.vertices {
        vertex.position[0] *= 7.3;
        vertex.position[2] *= 3.1;
    }
    let parent = plan_charts(&geometry, 0.0137, &[]).unwrap().remove(0);
    let padding = CHART_PADDING_TEXELS;
    let (parent_w, parent_h) = (
        parent.width_texels - 2 * padding,
        parent.height_texels - 2 * padding,
    );
    let origin = [parent_w / 3, parent_h / 4];
    let (window_w, window_h) = (parent_w / 2, parent_h / 2);
    let pitch = [
        parent.uv_extent[0] / parent_w as f32,
        parent.uv_extent[1] / parent_h as f32,
    ];
    let sub = Chart {
        uv_min: [
            parent.uv_min[0] + origin[0] as f32 * pitch[0],
            parent.uv_min[1] + origin[1] as f32 * pitch[1],
        ],
        uv_extent: [window_w as f32 * pitch[0], window_h as f32 * pitch[1]],
        width_texels: window_w + 2 * padding,
        height_texels: window_h + 2 * padding,
        window: Some(ChartWindow {
            grid_uv_min: parent.uv_min,
            grid_uv_extent: parent.uv_extent,
            grid_interior: [parent_w, parent_h],
            origin,
        }),
        ..parent.clone()
    };

    for (tx, ty) in [(0, 0), (5, 9), (window_w as i32 - 1, window_h as i32 - 1)] {
        let (px, py) = (origin[0] as i32 + tx, origin[1] as i32 + ty);
        assert_eq!(
            chart_texel_world_position(&sub, tx, ty)
                .to_array()
                .map(f32::to_bits),
            chart_texel_world_position(&parent, px, py)
                .to_array()
                .map(f32::to_bits),
            "texel ({tx}, {ty})"
        );
        assert_eq!(
            chart_texel_seed(&sub, tx, ty),
            chart_texel_seed(&parent, px, py)
        );
    }

    let vertex = Vec3::new(2.9, 0.0, 1.7);
    let (sx, sy) = chart_texel_position(&sub, 40, 60, vertex);
    let (px, py) = chart_texel_position(&parent, 40, 60, vertex);
    assert!(
        (sx - (px - origin[0] as f32)).abs() < 1.0e-3,
        "{sx} vs {px}"
    );
    assert!(
        (sy - (py - origin[1] as f32)).abs() < 1.0e-3,
        "{sy} vs {py}"
    );
}

/// `chart_texel_seed` is a pure function of the chart's frame and texel:
/// the same texel of the same frame always draws the same samples, wherever
/// the chart is placed, while neighbouring texels and other frames
/// decorrelate. Guards against reintroducing process-varying hashing.
#[test]
fn chart_texel_seed_depends_on_frame_and_texel_only() {
    use crate::chart_raster::chart_texel_seed;

    let chart = synthetic_chart_leaf(16, 16, 0);
    let mut elsewhere = chart.clone();
    elsewhere.leaf_index = 9;
    elsewhere.width_texels = 40;
    assert_eq!(
        chart_texel_seed(&chart, 3, 7),
        chart_texel_seed(&elsewhere, 3, 7)
    );
    assert_ne!(
        chart_texel_seed(&chart, 3, 7),
        chart_texel_seed(&chart, 7, 3)
    );
    assert_ne!(
        chart_texel_seed(&chart, 0, 0),
        chart_texel_seed(&chart, 0, 1)
    );
    assert_ne!(
        chart_texel_seed(&chart, 0, 0),
        chart_texel_seed(&chart, 1, 0)
    );
    let mut moved = chart.clone();
    moved.origin += glam::Vec3::new(0.0, 0.0, 0.25);
    assert_ne!(
        chart_texel_seed(&chart, 3, 7),
        chart_texel_seed(&moved, 3, 7)
    );
}

#[test]
fn shadowmask_coverage_threshold_uses_one_shared_comparison() {
    let threshold = LIGHT_TEXEL_CONTRIBUTION_EPSILON_SQUARED;
    let below = f32::from_bits(threshold.to_bits() - 1);
    let above = f32::from_bits(threshold.to_bits() + 1);

    assert!(!contribution_covers_shadowmask(below));
    assert!(contribution_covers_shadowmask(above));
}

/// For every lightmapped vertex of a fixture, the block id plus the block-local
/// UV, resolved through the block's bake-layer origin, addresses the texel the
/// bake-layer UV addressed before the rebase, within the two quantizations.
/// The bake-layer UV is recomputed exactly as the pre-block packer wrote it:
/// the chart's continuous texel over the layer extent.
#[test]
fn block_local_vertex_uv_addresses_the_same_chart_texel_as_the_bake_layer_uv() {
    let mut fixture = crate::fixture_pipeline::load_fixture("soft_shadow_test");
    let static_lights = StaticBakedLights::from_lights(&fixture.lights);
    assert!(!static_lights.is_empty(), "fixture must bake static light");
    let prepared = prepare_atlas(&mut fixture.geometry, &static_lights, 0.25, &[]).unwrap();
    let layout = &prepared.layout;
    assert!(
        layout.blocks.len() > 1,
        "fixture must exercise several cell blocks"
    );

    let geometry = &fixture.geometry;
    let mut checked = 0usize;
    let mut nonzero_origin = false;
    for (face, chart) in prepared.charts.iter().enumerate() {
        if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
            continue;
        }
        let placement = prepared.placements[face];
        let range = geometry.face_index_ranges[face];
        let start = range.index_offset as usize;
        for &index in &geometry.geometry.indices[start..start + range.index_count as usize] {
            let vertex = &geometry.geometry.vertices[index as usize];
            assert!(
                vertex.lightmap_block > 0,
                "face {face} vertex {index} names no block"
            );
            let block = &layout.blocks[usize::from(vertex.lightmap_block) - 1];
            assert_eq!(
                block.cell_id, chart.leaf_index,
                "vertex names its cell's block"
            );
            assert_eq!(block.layer, placement.layer);
            nonzero_origin |= block.x > 0 || block.y > 0;

            let (bake_x, bake_y) =
                chart_texel_position(chart, placement.x, placement.y, Vec3::from(vertex.position));
            let resolve =
                |quantized: u16, extent: u32| f32::from(quantized) / 65535.0 * extent as f32;
            let pre_rebase = [
                resolve(
                    quantize_lightmap_uv(bake_x, prepared.atlas_width),
                    prepared.atlas_width,
                ),
                resolve(
                    quantize_lightmap_uv(bake_y, prepared.atlas_height),
                    prepared.atlas_height,
                ),
            ];
            let block_frame = [
                block.x as f32 + resolve(vertex.lightmap_uv[0], block.width),
                block.y as f32 + resolve(vertex.lightmap_uv[1], block.height),
            ];
            let tolerance = [
                (prepared.atlas_width + block.width) as f32 / 65535.0 / 2.0 + 1.0e-3,
                (prepared.atlas_height + block.height) as f32 / 65535.0 / 2.0 + 1.0e-3,
            ];
            for axis in 0..2 {
                assert!(
                    (block_frame[axis] - pre_rebase[axis]).abs() <= tolerance[axis],
                    "face {face} vertex {index} axis {axis}: block frame {} vs bake layer {}",
                    block_frame[axis],
                    pre_rebase[axis]
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 0);
    assert!(
        nonzero_origin,
        "fixture must rebase through a nonzero block origin"
    );
}
