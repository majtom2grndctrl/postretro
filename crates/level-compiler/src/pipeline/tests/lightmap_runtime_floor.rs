//! Atlas pack → loader-backed id 51 → runtime floor → fused memo/bake proofs.
//! No SH or texture bake is needed to exercise these adjacent compiler stages.

use glam::DVec3;
use log::Level;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;
use postretro_level_format::lightmap_pool::place_all_resident;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_test_log_capture::LogCapture;

use crate::bake_control::BakeControl;
use crate::bvh_build::build_bvh;
use crate::cache::StageCache;
use crate::cell_residency_bake::{
    cell_residency_set_bake_cached, test_fixtures::l_corridor_sections,
};
use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::light_namespaces::{AlphaLightsNs, StaticBakedLights};
use crate::lightmap_bake::{self, LightmapConfig};
use crate::map_data::{FalloffModel, LightType, MapLight, ShadowType};
use crate::pipeline::lightmap_pool_gate::gate_runtime_pool_with_floor;
use crate::pipeline::lightmap_stage::{FusedLightingOutput, bake_fused_prepared};

const GATE_NAME: &str = "Lightmap runtime all-resident pool";

fn empty_geometry() -> GeometryResult {
    GeometryResult {
        geometry: GeometrySection {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: Vec::new(),
    }
}

fn push_floor(geometry: &mut GeometryResult, x: f32, cell: u32) {
    let base = geometry.geometry.vertices.len() as u32;
    for [dx, dz] in [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]] {
        geometry.geometry.vertices.push(Vertex::new(
            [x + dx, 0.0, 1.0 + dz],
            [dx, dz],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ));
    }
    let index_offset = geometry.geometry.indices.len() as u32;
    geometry
        .geometry
        .indices
        .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    geometry.geometry.faces.push(FaceMeta {
        leaf_index: cell,
        texture_index: 0,
    });
    geometry.face_index_ranges.push(FaceIndexRange {
        index_offset,
        index_count: 6,
    });
}

fn lights() -> Vec<MapLight> {
    vec![MapLight {
        origin: DVec3::new(2.0, 2.0, 2.0),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0; 3],
        falloff_model: FalloffModel::Linear,
        falloff_range: 20.0,
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
        tags: Vec::new(),
        shadow_type: ShadowType::StaticLightMap,
    }]
}

/// The same geometry and cell world in both modes. Only one portal's vertex
/// count changes, which makes the production loader reject that portal set.
fn compile_adjacent_stages(
    cache: Option<&StageCache>,
    usable_portals: bool,
    no_static_large: bool,
) -> anyhow::Result<FusedLightingOutput> {
    let mut geometry = empty_geometry();
    if no_static_large {
        // Each chart would take its own runtime layer: both dimensions are
        // greater than half the edge. Only chart metadata is materialized.
        for _ in 0..257 {
            push_floor(&mut geometry, 1.0, 0);
        }
    } else {
        push_floor(&mut geometry, 1.0, 0);
        push_floor(&mut geometry, 11.0, 1);
    }
    let lights = if no_static_large {
        Vec::new()
    } else {
        lights()
    };
    let static_lights = StaticBakedLights::from_lights(&lights);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let density = if no_static_large { 0.0005 } else { 0.25 };
    if no_static_large {
        // Size the hypothetical charts independently; no-static charts stay placeholders.
        let witness_lights = self::lights();
        let witness_statics = StaticBakedLights::from_lights(&witness_lights);
        let hypothetical = lightmap_bake::plan_cut_charts(
            &mut geometry.clone(),
            &witness_statics,
            density,
            &[],
            LIGHTMAP_POOL_LAYER_EDGE,
            crate::working_set::DEFAULT_MAX_BYTES,
            false,
        )?;
        let extents: Vec<_> = hypothetical
            .charts
            .iter()
            .map(|c| (c.width_texels, c.height_texels))
            .collect();
        assert_eq!(extents.len(), 257);
        assert!(extents.iter().all(|&(w, h)| {
            w > LIGHTMAP_POOL_LAYER_EDGE / 2
                && h > LIGHTMAP_POOL_LAYER_EDGE / 2
                && w <= LIGHTMAP_POOL_LAYER_EDGE
                && h <= LIGHTMAP_POOL_LAYER_EDGE
        }));
        assert_eq!(
            place_all_resident(&extents, 4, LIGHTMAP_POOL_LAYER_EDGE)
                .unwrap()
                .layer_count,
            257,
        );
    }
    let prepared = lightmap_bake::prepare_atlas(&mut geometry, &static_lights, density, &[])?;
    if no_static_large {
        assert!(prepared.placements.is_empty());
        assert!(prepared.layout.blocks.is_empty());
    } else {
        assert!(!prepared.placements.is_empty());
        assert_eq!(prepared.layout.blocks.len(), 2);
    }

    let (mut cells, mut portals, locator) = l_corridor_sections();
    for (id, cell) in cells.cells.iter_mut().enumerate() {
        cell.face_start = if no_static_large {
            if id == 0 { 0 } else { 257 }
        } else {
            id.min(2) as u32
        };
        cell.face_count = if no_static_large {
            u32::from(id == 0) * 257
        } else {
            u32::from(id < 2)
        };
    }
    if !usable_portals {
        portals.portals[2].vertex_count = 2;
    }
    let control = BakeControl::unrestricted();
    let id51 = cell_residency_set_bake_cached(&cells, &portals, &locator, cache, &control)?;
    assert_eq!(id51.is_some(), usable_portals);
    let layers =
        gate_runtime_pool_with_floor(&prepared, !static_lights.is_empty(), id51.is_some(), 0)?;
    assert_eq!(layers, u32::from(!no_static_large));

    let mut args = crate::parse_args_from(
        ["fixture.map", "--verbose", "--soft-shadow-samples", "4"]
            .into_iter()
            .map(str::to_owned),
    )
    .unwrap();
    if no_static_large {
        args.working_set_max_bytes = 0;
    }
    let config = LightmapConfig {
        lightmap_density: density,
        area_sample_count: 4,
        uncompressed_irradiance: true,
        direction_texel_scale: lightmap_bake::DIRECTION_TEXEL_SCALE,
    };
    let (bvh, primitives, _) = build_bvh(&geometry)?;
    bake_fused_prepared(
        &args,
        cache,
        &control,
        &control,
        &mut geometry,
        &static_lights,
        &alpha_lights,
        None,
        &[],
        &bvh,
        &primitives,
        &config,
        prepared,
    )
}

fn gate_warning(capture: &LogCapture) -> String {
    capture.assert_logged_once(Level::Warn, GATE_NAME);
    capture
        .records()
        .into_iter()
        .find(|record| record.level == Level::Warn && record.message.contains(GATE_NAME))
        .unwrap()
        .message
}

#[test]
fn packed_compile_reads_actual_id51_and_warm_memo_keeps_the_cold_pool_warning() {
    let directory = tempfile::tempdir().unwrap();
    let cache = StageCache::new(directory.path()).unwrap();
    let capture = LogCapture::start();
    let cold = compile_adjacent_stages(Some(&cache), true, false).unwrap();
    let cold_warning = gate_warning(&capture);
    assert!(!cold.lightmap.section.blocks.is_empty());
    assert_eq!(cache.test_access("lightmap_section").read_hits, 0);
    assert_eq!(cache.test_access("lightmap_section").writes, 1);
    assert_eq!(cache.test_access("shadowmask_atlas").writes, 1);

    capture.clear();
    cache.clear_test_accesses();
    let warm = compile_adjacent_stages(Some(&cache), true, false).unwrap();
    assert_eq!(gate_warning(&capture), cold_warning);
    capture.assert_logged_once(Level::Info, "[cache] lightmap_section hit");
    assert_eq!(cache.test_access("cell_residency_set").read_hits, 1);
    assert_eq!(cache.test_access("lightmap_section").read_hits, 1);
    assert_eq!(cache.test_access("shadowmask_atlas").read_hits, 1);
    assert_eq!(warm.lightmap.section, cold.lightmap.section);
    assert_eq!(warm.shadowmask, cold.shadowmask);

    // The same packed geometry with a loader-rejected portal set emits no
    // id 51. Even populated lightmap/shadowmask memos must not be read.
    capture.clear();
    cache.clear_test_accesses();
    let warm_error = match compile_adjacent_stages(Some(&cache), false, false) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("a level without usable portals must fail the pool floor"),
    };
    assert!(warm_error.contains(GATE_NAME), "{warm_error}");
    assert!(warm_error.contains("cannot stream"), "{warm_error}");
    assert_eq!(cache.test_access("lightmap_section").read_attempts, 0);
    assert_eq!(cache.test_access("shadowmask_atlas").read_attempts, 0);
    capture.assert_not_logged(Level::Warn, GATE_NAME);
    let cold_error = match compile_adjacent_stages(None, false, false) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("an uncached level without usable portals must fail the same floor"),
    };
    assert_eq!(warm_error, cold_error);
}

#[test]
fn no_static_compile_skips_257_layers_of_charts_with_and_without_actual_id51() {
    let capture = LogCapture::start();
    for usable_portals in [false, true] {
        let output = compile_adjacent_stages(None, usable_portals, true).unwrap();
        assert!(output.lightmap.section.blocks.is_empty());
        assert!(output.lightmap.placements.is_empty());
        assert!(output.shadowmask.is_none());
        capture.assert_not_logged(Level::Warn, GATE_NAME);
    }
}
