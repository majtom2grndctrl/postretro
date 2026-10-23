use super::*;
use crate::map_data::{LightType, MapLight};
use crate::texel_footprint::PENUMBRA_FLOOR_DEFAULT;
use glam::DVec3;
use glam::Vec3;
use log::Level;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::shadowmask_atlas::{SHADOWMASK_OWNER_EMPTY, ShadowmaskOwnerBounds};
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_test_log_capture::LogCapture;
use rayon::ThreadPoolBuilder;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::bvh_build::build_bvh;
use crate::geometry::FaceIndexRange;
use crate::governor::Governor;
use crate::map_data::FalloffModel;
use crate::reporter::StageProgress;

fn fresh_cache_dir(label: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "postretro_fused_lighting_cache_{label}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn quad_geometry() -> GeometryResult {
    let normal = [0.0, 1.0, 0.0];
    let tangent = [1.0, 0.0, 0.0];
    GeometryResult {
        geometry: GeometrySection {
            vertices: vec![
                Vertex::new(
                    [0.0, 0.0, 0.0],
                    [0.0, 0.0],
                    normal,
                    tangent,
                    true,
                    [0.0, 0.0],
                    0,
                ),
                Vertex::new(
                    [1.0, 0.0, 0.0],
                    [1.0, 0.0],
                    normal,
                    tangent,
                    true,
                    [0.0, 0.0],
                    0,
                ),
                Vertex::new(
                    [1.0, 0.0, 1.0],
                    [1.0, 1.0],
                    normal,
                    tangent,
                    true,
                    [0.0, 0.0],
                    0,
                ),
                Vertex::new(
                    [0.0, 0.0, 1.0],
                    [0.0, 1.0],
                    normal,
                    tangent,
                    true,
                    [0.0, 0.0],
                    0,
                ),
            ],
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

/// Two large, separate leaves that each fit one 64² atlas layer but do
/// not fit together. The fused-path matrix relies on production atlas
/// preparation assigning the second face to layer 1.
fn two_layer_geometry() -> GeometryResult {
    let mut first = quad_geometry();
    for vertex in &mut first.geometry.vertices {
        vertex.position[0] *= 12.0;
        vertex.position[2] *= 12.0;
    }

    let mut second = quad_geometry();
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

fn point_light(origin: DVec3, color: [f32; 3]) -> MapLight {
    MapLight {
        origin,
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color,
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
        tags: Vec::new(),
        shadow_type: ShadowType::StaticLightMap,
    }
}

fn test_args() -> Args {
    crate::parse_args_from(
        ["fixture.map", "--verbose", "--soft-shadow-samples", "4"]
            .into_iter()
            .map(str::to_owned),
    )
    .expect("test arguments must parse")
}

fn config(uncompressed_irradiance: bool) -> LightmapConfig {
    LightmapConfig {
        lightmap_density: 0.25,
        area_sample_count: 4,
        uncompressed_irradiance,
        direction_texel_scale: lightmap_bake::DIRECTION_TEXEL_SCALE,
        penumbra_floor: PENUMBRA_FLOOR_DEFAULT,
    }
}

fn run_fused(
    args: &Args,
    cache: Option<&StageCache>,
    lights: &[MapLight],
    selection: Option<&EntityShadowLightsSection>,
    config: &LightmapConfig,
    lightmap_control: &BakeControl,
    shadowmask_control: &BakeControl,
) -> FusedLightingOutput {
    run_fused_with_geometry(
        args,
        cache,
        lights,
        selection,
        config,
        lightmap_control,
        shadowmask_control,
        quad_geometry(),
    )
}

#[allow(clippy::too_many_arguments)]
fn run_fused_with_geometry(
    args: &Args,
    cache: Option<&StageCache>,
    lights: &[MapLight],
    selection: Option<&EntityShadowLightsSection>,
    config: &LightmapConfig,
    lightmap_control: &BakeControl,
    shadowmask_control: &BakeControl,
    mut geometry: GeometryResult,
) -> FusedLightingOutput {
    run_fused_labeled(
        args,
        cache,
        lights,
        selection,
        &[],
        config,
        lightmap_control,
        shadowmask_control,
        &mut geometry,
    )
}

/// As [`run_fused_with_geometry`], naming each light by `labels` in the drop
/// report.
#[allow(clippy::too_many_arguments)]
fn run_fused_labeled(
    args: &Args,
    cache: Option<&StageCache>,
    lights: &[MapLight],
    selection: Option<&EntityShadowLightsSection>,
    labels: &[String],
    config: &LightmapConfig,
    lightmap_control: &BakeControl,
    shadowmask_control: &BakeControl,
    geometry: &mut GeometryResult,
) -> FusedLightingOutput {
    let (bvh, primitives, _) = build_bvh(geometry).expect("fixture BVH must build");
    let static_lights = StaticBakedLights::from_lights(lights);
    let alpha_lights = AlphaLightsNs::from_lights(lights);
    let prepared =
        lightmap_bake::prepare_atlas(geometry, &static_lights, config.lightmap_density, &[])
            .expect("fixture atlas must prepare");
    bake_fused_prepared(
        args,
        cache,
        lightmap_control,
        shadowmask_control,
        geometry,
        &static_lights,
        &alpha_lights,
        selection,
        labels,
        &bvh,
        &primitives,
        config,
        prepared,
    )
    .expect("fused fixture bake must succeed")
}

fn fused_outputs(
    args: &Args,
    cache: Option<&StageCache>,
    lights: &[MapLight],
    selection: &EntityShadowLightsSection,
    config: &LightmapConfig,
) -> (Vec<u8>, Vec<u8>, u32) {
    let output = run_fused_with_geometry(
        args,
        cache,
        lights,
        Some(selection),
        config,
        &BakeControl::unrestricted(),
        &BakeControl::unrestricted(),
        two_layer_geometry(),
    );
    let shadowmask = output
        .shadowmask
        .expect("lit fixture faces must emit id 42");
    (
        output.lightmap.section.to_bytes(),
        shadowmask.to_bytes(),
        output.lightmap.layer_count,
    )
}

/// (lightmap bytes, shadowmask bytes, layer count), chart total, checkpoint count.
type FusedOutputsWithWorkers = ((Vec<u8>, Vec<u8>, u32), Option<usize>, usize);

fn fused_outputs_with_workers(
    workers: usize,
    args: &Args,
    cache: Option<&StageCache>,
    lights: &[MapLight],
    selection: &EntityShadowLightsSection,
    config: &LightmapConfig,
) -> FusedOutputsWithWorkers {
    let progress = StageProgress::indeterminate();
    let governor = Arc::new(Governor::new(workers, false));
    let output = ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .expect("build fused fixture pool")
        .install(|| {
            run_fused_with_geometry(
                args,
                cache,
                lights,
                Some(selection),
                config,
                &BakeControl::new(governor, &progress),
                &BakeControl::unrestricted(),
                two_layer_geometry(),
            )
        });
    let shadowmask = output
        .shadowmask
        .expect("lit fixture faces must emit id 42");
    (
        (
            output.lightmap.section.to_bytes(),
            shadowmask.to_bytes(),
            output.lightmap.layer_count,
        ),
        progress.total(),
        progress.completed(),
    )
}

/// The uncached lightmap reference. The shadowmask has no separate reference:
/// a cold single-worker uncached bake is its source of truth.
fn reference_lightmap(lights: &[MapLight], config: &LightmapConfig) -> Vec<u8> {
    let mut geometry = two_layer_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH must build");
    let static_lights = StaticBakedLights::from_lights(lights);
    let prepared =
        lightmap_bake::prepare_atlas(&mut geometry, &static_lights, config.lightmap_density, &[])
            .expect("fixture atlas must prepare");
    let mut ctx = lightmap_bake::LightmapBakeCtx {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &mut geometry,
        lights: &static_lights,
        scale_regions: &[],
    };
    lightmap_bake::bake_prepared_lightmap_controlled(
        &mut ctx,
        config,
        prepared,
        &BakeControl::unrestricted(),
    )
    .expect("reference lightmap must bake")
    .section
    .to_bytes()
}

/// Per-texel mask channels `0..4` of one block: group 0's `.rg`, then group
/// 1's.
fn decode_block_channels(groups: &[Vec<u8>; 2], width: u32, height: u32) -> Vec<[u8; 4]> {
    let a = crate::bc5::decode_bc5_rg(&groups[0], width, height);
    let b = crate::bc5::decode_bc5_rg(&groups[1], width, height);
    a.as_chunks::<2>()
        .0
        .iter()
        .zip(b.as_chunks::<2>().0.iter())
        .map(|(a, b)| [a[0], a[1], b[0], b[1]])
        .collect()
}

/// Lights 0 and 1 (spec indices 0 and 1) both own one face of block 1, and
/// both their mask values are non-zero at one of its texels.
fn assert_two_lights_own_a_face_in_block_one(lightmap: &[u8], bytes: &[u8]) {
    let lightmap = postretro_level_format::lightmap::LightmapSection::from_bytes(lightmap)
        .expect("fused lightmap bytes must decode");
    let bounds = ShadowmaskOwnerBounds::from_section_header(bytes).expect("id 42 header");
    let shadowmask = ShadowmaskAtlasSection::from_bytes(bytes, &lightmap.index(), bounds)
        .expect("fused shadowmask bytes must decode");
    assert_eq!(shadowmask.blocks.len(), 2);
    assert!(
        shadowmask
            .owners
            .iter()
            .any(|owners| owners[0] == 0 && owners[1] == 1),
        "overlapping lights must own one face together, ascending: {:?}",
        shadowmask.owners
    );
    assert!(
        shadowmask
            .owners
            .iter()
            .flatten()
            .all(|&owner| owner == SHADOWMASK_OWNER_EMPTY || owner < 3)
    );
    let block = &lightmap.blocks[1];
    let texels = decode_block_channels(
        &shadowmask.blocks[1],
        u32::from(block.width),
        u32::from(block.height),
    );
    assert!(
        texels.iter().any(|texel| texel[0] != 0 && texel[1] != 0),
        "both owners must write their own channels at one overlapping block-1 texel"
    );
}

/// The cold single-worker uncached bake of `lights` under `selection`.
fn cold_outputs(
    lights: &[MapLight],
    selection: &EntityShadowLightsSection,
    config: &LightmapConfig,
) -> (Vec<u8>, Vec<u8>, u32) {
    fused_outputs_with_workers(1, &test_args(), None, lights, selection, config).0
}

// Id 42 is byte-identical across worker counts and between cold, warm, and
// lightmap-reuse builds; editing a light outside id 40, or only id-40
// membership, rebuilds it; a reuse rebuild reads each eligible partition once
// per layer and bakes none.
fn metadata_counts(prepared: &PreparedAtlas, lights: &[MapLight]) -> MetadataCounts {
    MetadataCounts::new(
        &StaticBakedLights::from_lights(lights),
        &AlphaLightsNs::from_lights(lights),
        prepared.charts.len(),
    )
}

#[test]
fn fused_cold_warm_and_lightmap_reuse_paths_match_cold_bytes() {
    let args = test_args();
    let mut lights = vec![
        point_light(DVec3::new(13.0, 8.0, 5.0), [1.0, 0.25, 0.1]),
        point_light(DVec3::new(15.0, 9.0, 7.0), [0.1, 0.35, 1.0]),
        point_light(DVec3::new(14.0, 7.0, 6.0), [0.2, 1.0, 0.3]),
    ];
    for light in &mut lights {
        light.falloff_range = 40.0;
    }
    let initial_selection = EntityShadowLightsSection {
        light_indices: vec![0, 1],
    };
    // Membership alone changes: light 2 joins, light 1 leaves.
    let edited_selection = EntityShadowLightsSection {
        light_indices: vec![0, 2],
    };

    for uncompressed in [false, true] {
        let config = config(uncompressed);
        let (cold, cold_total, cold_completed) =
            fused_outputs_with_workers(1, &args, None, &lights, &initial_selection, &config);
        let (parallel_cold, parallel_total, parallel_completed) =
            fused_outputs_with_workers(4, &args, None, &lights, &initial_selection, &config);
        assert_eq!(cold.2, 2, "fixture must exercise atlas layer 1");
        let expected_partitions = lights.len() * cold.2 as usize;
        assert_eq!(cold_total, Some(expected_partitions));
        assert_eq!(cold_completed, expected_partitions);
        assert_eq!(parallel_total, Some(expected_partitions));
        assert_eq!(parallel_completed, expected_partitions);
        assert_eq!(
            cold.0,
            reference_lightmap(&lights, &config),
            "cold fused lightmap bytes changed"
        );
        assert_eq!(parallel_cold, cold, "id 42 must not depend on worker count");

        assert_two_lights_own_a_face_in_block_one(&cold.0, &cold.1);

        let dir = fresh_cache_dir(if uncompressed { "rgba16f" } else { "bc6h" });
        let cache = StageCache::new(&dir).expect("create fused test cache");
        let warm_miss_logs = LogCapture::start();
        let warm_miss = fused_outputs(&args, Some(&cache), &lights, &initial_selection, &config);
        assert_eq!(warm_miss, cold, "warm miss must equal the cold build");
        warm_miss_logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas miss");
        warm_miss_logs.assert_logged_once(Level::Info, "[cache] lightmap_section miss");
        // Partition loads run on Rayon workers, where log capture does not
        // reach, so partition counts come from the cache itself.
        let layers = cache.test_access("lightmap_layer");
        assert_eq!(
            layers.read_attempts - layers.read_hits,
            expected_partitions,
            "an empty warm cache must bake each light/layer partition exactly once"
        );
        assert_eq!(
            layers.read_hits, 0,
            "an empty warm cache cannot load a lightmap partition"
        );
        drop(warm_miss_logs);
        cache.clear_test_accesses();

        let no_edit_logs = LogCapture::start();
        let warm_hit = fused_outputs(&args, Some(&cache), &lights, &initial_selection, &config);
        assert_eq!(warm_hit, cold, "warm hit must preserve exact bytes");
        no_edit_logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas hit");
        no_edit_logs.assert_logged_once(Level::Info, "[cache] lightmap_section hit");
        assert_eq!(
            cache.test_access("lightmap_layer").read_attempts,
            0,
            "both memo hits must read no lightmap partition"
        );
        drop(no_edit_logs);
        cache.clear_test_accesses();

        // Light 2 is outside id 40 but owns channels, so its edit rebuilds id 42.
        let mut one_light_edit = lights.clone();
        one_light_edit[2].intensity = 0.875;
        let edit_cold = cold_outputs(&one_light_edit, &initial_selection, &config);
        let edit_logs = LogCapture::start();
        let edited = fused_outputs(
            &args,
            Some(&cache),
            &one_light_edit,
            &initial_selection,
            &config,
        );
        assert_eq!(
            edited, edit_cold,
            "a one-light edit must equal its cold build"
        );
        edit_logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas miss");
        edit_logs.assert_logged_once(Level::Info, "[cache] lightmap_section miss");
        let layers = cache.test_access("lightmap_layer");
        let layer_hits = layers.read_hits;
        let layer_misses = layers.read_attempts - layers.read_hits;
        assert_eq!(
            layer_hits,
            (lights.len() - 1) * edited.2 as usize,
            "the unedited partitions must each load exactly once"
        );
        assert_eq!(layer_misses, edited.2 as usize);
        drop(edit_logs);
        cache.clear_test_accesses();

        // Membership alone: the lightmap memo hits and id 42 rebuilds from the
        // cached partitions of every eligible light, each read once per layer,
        // through per-layer raw fill and no trace.
        let membership_cold = cold_outputs(&lights, &edited_selection, &config);
        let selection_logs = LogCapture::start();
        let selection_only =
            fused_outputs(&args, Some(&cache), &lights, &edited_selection, &config);
        assert_eq!(
            selection_only, membership_cold,
            "a membership-only rebuild must equal its cold build"
        );
        selection_logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas miss");
        selection_logs.assert_logged_once(Level::Info, "[cache] lightmap_section hit");
        let layers = cache.test_access("lightmap_layer");
        assert_eq!(
            layers.read_attempts, layers.read_hits,
            "a reuse rebuild must miss, and so bake, no lightmap partition"
        );
        assert_eq!(
            layers.read_hits,
            lights.len() * selection_only.2 as usize,
            "a reuse rebuild must read each eligible partition exactly once per layer"
        );
        drop(selection_logs);
        drop(cache);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// A static light that cannot own (here SDF-shadowed) inserted before the
// eligible lights changes no lightmap layer key, so the lightmap memo hits; it
// shifts every owner's compact spec index, so id 42 must rebuild from the
// cached partitions and equal a cold build of the edited level.
#[test]
fn an_earlier_unownable_static_light_rebuilds_id_42_over_a_reused_lightmap() {
    let args = test_args();
    let config = config(false);
    let mut lights = vec![
        point_light(DVec3::new(13.0, 8.0, 5.0), [1.0, 0.25, 0.1]),
        point_light(DVec3::new(15.0, 9.0, 7.0), [0.1, 0.35, 1.0]),
        point_light(DVec3::new(14.0, 7.0, 6.0), [0.2, 1.0, 0.3]),
    ];
    for light in &mut lights {
        light.falloff_range = 40.0;
    }
    let selection = EntityShadowLightsSection {
        light_indices: vec![0, 1],
    };
    let dir = fresh_cache_dir("spec_index_shift");
    let cache = StageCache::new(&dir).expect("create spec-index shift cache");
    let before = fused_outputs(&args, Some(&cache), &lights, &selection, &config);
    cache.clear_test_accesses();

    let mut sdf = point_light(DVec3::new(14.0, 8.0, 6.0), [1.0; 3]);
    sdf.shadow_type = ShadowType::Sdf;
    let mut shifted = lights.clone();
    shifted.insert(0, sdf);
    // Id 40 indexes AlphaLights, so the same lights stay promoted.
    let shifted_selection = EntityShadowLightsSection {
        light_indices: vec![1, 2],
    };
    let shifted_cold = cold_outputs(&shifted, &shifted_selection, &config);
    let logs = LogCapture::start();
    let warm = fused_outputs(&args, Some(&cache), &shifted, &shifted_selection, &config);
    logs.assert_logged_once(Level::Info, "[cache] lightmap_section hit");
    logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas miss");
    assert_eq!(
        warm, shifted_cold,
        "a spec-index shift must rebuild id 42 equal to its cold build"
    );
    assert_eq!(warm.0, before.0, "the lightmap is reused unchanged");
    assert_ne!(
        warm.1, before.1,
        "the owner table names the shifted indices"
    );
    let layers = cache.test_access("lightmap_layer");
    assert_eq!(
        layers.read_attempts, layers.read_hits,
        "the rebuild must bake no lightmap partition"
    );
    assert_eq!(
        layers.read_hits,
        lights.len() * warm.2 as usize,
        "the rebuild must read each eligible partition exactly once per layer"
    );
    drop(logs);
    drop(cache);
    let _ = std::fs::remove_dir_all(&dir);
}

// Id 42 does not depend on id 40.
#[test]
fn fused_stage_progress_is_exact_and_a_cleared_selection_keeps_the_shadowmask() {
    let args = test_args();
    let config = config(false);
    let lights = vec![
        point_light(DVec3::new(0.2, 1.0, 0.35), [1.0, 0.25, 0.1]),
        point_light(DVec3::new(0.85, 1.5, 0.75), [0.1, 0.35, 1.0]),
    ];
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let lightmap_progress = StageProgress::indeterminate();
    let shadowmask_progress = StageProgress::indeterminate();
    let governor = Arc::new(Governor::new(4, false));
    let output = run_fused(
        &args,
        None,
        &lights,
        Some(&selection),
        &config,
        &BakeControl::new(Arc::clone(&governor), &lightmap_progress),
        &BakeControl::new(governor, &shadowmask_progress),
    );
    let expected_lightmap = output
        .lightmap
        .placements
        .len()
        .saturating_mul(lights.len());
    // One unit per eligible partition, plus the encode.
    let expected_shadowmask = lights.len() * output.lightmap.layer_count as usize + 1;
    assert_eq!(lightmap_progress.total(), Some(expected_lightmap));
    assert_eq!(lightmap_progress.completed(), expected_lightmap);
    assert_eq!(shadowmask_progress.total(), Some(expected_shadowmask));
    assert_eq!(shadowmask_progress.completed(), expected_shadowmask);

    let cleared = run_fused(
        &args,
        None,
        &lights,
        None,
        &config,
        &BakeControl::unrestricted(),
        &BakeControl::unrestricted(),
    );
    assert!(
        cleared.shadowmask.is_some(),
        "a level whose eligible lights include none in id 40 keeps id 42"
    );
    assert_eq!(
        cleared.lightmap.section.to_bytes(),
        output.lightmap.section.to_bytes(),
        "clearing entity-shadow selection must not change lightmap bytes"
    );
}

#[test]
fn fused_stage_preserves_all_sdf_one_plane_fallback_on_cache_hit() {
    let args = test_args();
    let config = config(false);
    let mut sdf_light = point_light(DVec3::new(0.5, 1.0, 0.5), [1.0; 3]);
    sdf_light.shadow_type = ShadowType::Sdf;
    let lights = vec![sdf_light];
    let dir = fresh_cache_dir("all_sdf");
    let cache = StageCache::new(&dir).expect("create all-SDF test cache");

    let first_progress = StageProgress::indeterminate();
    let first = run_fused(
        &args,
        Some(&cache),
        &lights,
        None,
        &config,
        &BakeControl::new(Arc::new(Governor::new(1, false)), &first_progress),
        &BakeControl::unrestricted(),
    );
    let second_progress = StageProgress::indeterminate();
    let second = run_fused(
        &args,
        Some(&cache),
        &lights,
        None,
        &config,
        &BakeControl::new(Arc::new(Governor::new(1, false)), &second_progress),
        &BakeControl::unrestricted(),
    );
    assert_eq!(first.lightmap.section.blocks.len(), 1);
    assert!(
        first.lightmap.section.blocks[0]
            .direction
            .as_chunks::<2>()
            .0
            .iter()
            .all(|texel| *texel == [128, 255]),
        "an all-SDF bake keeps its block with neutral direction"
    );
    assert_eq!(
        first.lightmap.section.to_bytes(),
        second.lightmap.section.to_bytes()
    );
    assert!(first.shadowmask.is_none());
    assert!(second.shadowmask.is_none());
    assert_eq!(first_progress.total(), Some(0));
    assert_eq!(first_progress.completed(), 0);
    assert_eq!(second_progress.total(), Some(0));
    assert_eq!(second_progress.completed(), 0);

    drop(cache);
    let _ = std::fs::remove_dir_all(dir);
}

/// `count` 12 m quads in one cell. At the fixture's 0.25 m/texel each
/// charts at 52², so under a 64-texel test pool edge the cell takes one
/// block per quad.
fn quads_in_one_cell(count: usize) -> GeometryResult {
    let mut geometry = quad_geometry();
    geometry.geometry.vertices.clear();
    geometry.geometry.indices.clear();
    geometry.geometry.faces.clear();
    geometry.face_index_ranges.clear();
    for quad in 0..count {
        let mut next = quad_geometry();
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

/// (lightmap bytes, shadowmask bytes, block count) of a fused cold bake of
/// one multi-block cell on `workers` threads.
fn multi_block_fused_outputs(workers: usize, uncompressed: bool) -> (Vec<u8>, Vec<u8>, usize) {
    let args = test_args();
    let config = config(uncompressed);
    let mut light = point_light(DVec3::new(20.0, 6.0, 6.0), [1.0, 0.5, 0.2]);
    light.falloff_range = 40.0;
    let lights = vec![light];
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let mut geometry = quads_in_one_cell(3);
    let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH must build");
    let static_lights = StaticBakedLights::from_lights(&lights);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(workers, false)), &progress);
    ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .expect("build fused fixture pool")
        .install(|| {
            let prepared = lightmap_bake::prepare_atlas_within(
                &mut geometry,
                &static_lights,
                config.lightmap_density,
                &[],
                lightmap_bake::BlockOrdering::by_cell_id(config.direction_texel_scale),
                64,
                &control,
            )
            .expect("multi-block fixture must prepare");
            let blocks = prepared.layout.blocks.len();
            let output = bake_fused_prepared(
                &args,
                None,
                &control,
                &BakeControl::unrestricted(),
                &mut geometry,
                &static_lights,
                &alpha_lights,
                Some(&selection),
                &[],
                &bvh,
                &primitives,
                &config,
                prepared,
            )
            .expect("multi-block fused bake must succeed");
            (
                output.lightmap.section.to_bytes(),
                output
                    .shadowmask
                    .expect("the lit cell must emit id 42")
                    .to_bytes(),
                blocks,
            )
        })
}

#[test]
fn multi_block_cell_section_bytes_are_identical_with_one_worker_and_many() {
    for uncompressed in [false, true] {
        let one = multi_block_fused_outputs(1, uncompressed);
        let many = multi_block_fused_outputs(4, uncompressed);
        assert_eq!(one.2, 3, "the fixture cell splits into three blocks");
        let section = postretro_level_format::lightmap::LightmapSection::from_bytes(&one.0)
            .expect("multi-block lightmap decodes");
        assert!(section.blocks.iter().all(|block| block.cell_id == 0));
        assert_eq!(one, many, "uncompressed = {uncompressed}");
    }
}

#[test]
fn predicted_peak_charges_one_layer_fill_scratch_and_each_section_once() {
    let mut geometry = quads_in_one_cell(3);
    let lights = vec![point_light(DVec3::new(20.0, 6.0, 6.0), [1.0; 3])];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let prepared = lightmap_bake::prepare_atlas_within(
        &mut geometry,
        &static_lights,
        0.25,
        &[],
        lightmap_bake::BlockOrdering::by_cell_id(2),
        64,
        &BakeControl::unrestricted(),
    )
    .expect("multi-block fixture must prepare");
    let layer_texels = u64::from(prepared.atlas_width) * u64::from(prepared.atlas_height);
    let peak = predicted_peak(&prepared, metadata_counts(&prepared, &lights), false, 1);
    assert_eq!(peak.shadowmask_fill, 4 * layer_texels);
    assert_eq!(peak.layer_plane, 57 * layer_texels);
    let block_texels: u64 = prepared
        .layout
        .blocks
        .iter()
        .map(|b| u64::from(b.width) * u64::from(b.height))
        .sum();
    // BC6H 1 B + direction 2 B / scale² + shadowmask 2 B per texel, once each.
    assert_eq!(peak.sections, block_texels * 3 + block_texels / 4 * 2);
    assert_eq!(peak.shadowmask_face_map, 4 * layer_texels);
    assert_eq!(
        peak.total(),
        peak.shadowmask_fill
            + peak.shadowmask_encode_scratch
            + peak.shadowmask_face_map
            + peak.layer_plane
            + peak.partitions
            + peak.sections
            + peak.metadata
            + peak.cache_metadata
            + peak.ownership_metadata
    );
}

// The window's resident partitions are charged, growing with the window;
// a window of one adds exactly one partition to the old terms.
#[test]
fn predicted_peak_charges_window_partitions() {
    let mut geometry = quads_in_one_cell(3);
    let lights = vec![point_light(DVec3::new(20.0, 6.0, 6.0), [1.0; 3])];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let prepared = lightmap_bake::prepare_atlas(&mut geometry, &static_lights, 0.25, &[]).unwrap();
    let layer_texels = u64::from(prepared.atlas_width) * u64::from(prepared.atlas_height);
    let one_partition = 2 * 8 * layer_texels;

    let one = predicted_peak(&prepared, metadata_counts(&prepared, &lights), false, 1);
    let without_window = one.shadowmask_fill
        + one.shadowmask_encode_scratch
        + one.shadowmask_face_map
        + one.layer_plane
        + one.sections
        + one.metadata
        + one.cache_metadata
        + one.ownership_metadata;
    assert_eq!(one.total(), without_window + one_partition);
    let mut previous = one.total();
    for window in [2, LIGHTMAP_PARTITION_WINDOW, 4 * LIGHTMAP_PARTITION_WINDOW] {
        let peak = predicted_peak(
            &prepared,
            metadata_counts(&prepared, &lights),
            false,
            window,
        );
        assert!(peak.total() > previous, "window {window}");
        assert_eq!(peak.partitions, window as u64 * one_partition);
        previous = peak.total();
    }
}

/// Append a horizontal quad `[x, x + size] × [z, z + size]` at height
/// `y` as one face of `leaf`.
fn push_quad(geometry: &mut GeometryResult, x: f32, z: f32, size: f32, y: f32, leaf: u32) {
    let mut quad = quad_geometry();
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

/// A 6 m floor under a 2 m occluder in cell 1, so an area light casts a
/// penumbra. With `far_quad`, an unlit 3 m quad in cell 0 precedes them:
/// it renumbers both faces and takes the bake layer's corner, so cell 1's
/// block lands beside it at other bake-layer coordinates.
fn penumbra_cell_geometry(far_quad: bool) -> GeometryResult {
    let mut geometry = quad_geometry();
    geometry.geometry.vertices.clear();
    geometry.geometry.indices.clear();
    geometry.geometry.faces.clear();
    geometry.face_index_ranges.clear();
    if far_quad {
        push_quad(&mut geometry, 500.0, 500.0, 3.0, 0.0, 0);
    }
    push_quad(&mut geometry, 0.0, 0.0, 6.0, 0.0, 1);
    push_quad(&mut geometry, 2.0, 2.0, 2.0, 1.0, 1);
    geometry
}

/// Soft-visibility seeds key on the chart through the shipping per-light
/// layer walk too: the fused stage bakes a moved, renumbered floor's
/// penumbra into identical lightmap and shadowmask block bytes.
#[test]
fn fused_bake_of_a_moved_and_renumbered_chart_is_byte_identical() {
    let args = test_args();
    let config = config(true);
    let mut light = point_light(DVec3::new(3.0, 3.0, 3.0), [1.0, 0.9, 0.8]);
    light.falloff_range = 8.0;
    light.light_size = 1.0;
    let lights = vec![light];
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let bake = |far_quad: bool| {
        let output = run_fused_with_geometry(
            &args,
            None,
            &lights,
            Some(&selection),
            &config,
            &BakeControl::unrestricted(),
            &BakeControl::unrestricted(),
            penumbra_cell_geometry(far_quad),
        );
        let floor = output.lightmap.placements[usize::from(far_quad)];
        let block = output
            .lightmap
            .section
            .blocks
            .iter()
            .position(|block| block.cell_id == 1)
            .expect("cell 1 bakes a block");
        let shadowmask = output.shadowmask.expect("the lit floor emits id 42");
        (
            (floor.x, floor.y),
            output.lightmap.section.blocks[block].clone(),
            shadowmask.blocks[block].clone(),
        )
    };
    let (alone_at, alone, alone_mask) = bake(false);
    let (shifted_at, shifted, shifted_mask) = bake(true);
    assert_ne!(alone_at, shifted_at, "the far quad must move the floor");

    // The penumbra must reach the floor, or seeds never matter.
    let red: Vec<u16> = alone
        .irradiance
        .as_chunks::<8>()
        .0
        .iter()
        .map(|texel| u16::from_le_bytes([texel[0], texel[1]]))
        .collect();
    let (lo, hi) = (*red.iter().min().unwrap(), *red.iter().max().unwrap());
    let penumbra = red
        .iter()
        .filter(|&&r| r > lo + (hi - lo) / 20 && r < hi - (hi - lo) / 20)
        .count();
    assert!(
        penumbra > 10,
        "fixture must bake a penumbra, got {penumbra}"
    );

    assert_eq!(alone.irradiance, shifted.irradiance);
    assert_eq!(alone.direction, shifted.direction);
    assert_eq!(alone_mask, shifted_mask, "shadowmask moved with the chart");
}

/// A floor under a roof that overhangs it on every side.
fn floor_under_roof() -> GeometryResult {
    let mut geometry = quad_geometry();
    geometry.geometry.vertices.clear();
    geometry.geometry.indices.clear();
    geometry.geometry.faces.clear();
    geometry.face_index_ranges.clear();
    push_quad(&mut geometry, 0.0, 0.0, 6.0, 0.0, 0);
    push_quad(&mut geometry, -4.0, -4.0, 14.0, 2.0, 0);
    geometry
}

fn owners_of(output: &FusedLightingOutput) -> Vec<[u16; 4]> {
    output
        .shadowmask
        .as_ref()
        .expect("lit faces emit id 42")
        .owners
        .clone()
}

// A light walled off from a receiver inside its range takes no channel there
// while one that reaches it does; a directional light owns the face it lights
// and none under a roof; lights outside id 40 own channels.
#[test]
fn occluded_lights_and_suns_own_only_the_faces_they_reach() {
    let args = test_args();
    let config = config(false);
    let mut above_roof = point_light(DVec3::new(3.0, 4.0, 3.0), [1.0; 3]);
    above_roof.falloff_range = 10.0;
    let mut sun = point_light(DVec3::new(-30.0, 50.0, 12.0), [1.0; 3]);
    sun.light_type = LightType::Directional;
    sun.cone_direction = Some([0.0, -1.0, 0.0]);
    let mut under_roof = point_light(DVec3::new(3.0, 1.0, 3.0), [1.0; 3]);
    under_roof.falloff_range = 10.0;
    let lights = vec![above_roof, sun, under_roof];
    let output = run_fused_with_geometry(
        &args,
        None,
        &lights,
        None,
        &config,
        &BakeControl::unrestricted(),
        &BakeControl::unrestricted(),
        floor_under_roof(),
    );
    let owners = owners_of(&output);
    const E: u16 = SHADOWMASK_OWNER_EMPTY;
    assert_eq!(
        owners[0],
        [2, E, E, E],
        "only the light under the roof owns the floor"
    );
    assert_eq!(
        owners[1],
        [0, 1, E, E],
        "the light above the roof and the sun own its top"
    );
}

// The drop warning names the light and its face, and `--verbose` the peak
// demand; a warm build that reuses the cached id 42 repeats the cold build's
// warnings and peak.
#[test]
fn the_drop_report_names_light_and_face_and_repeats_on_a_warm_memo_hit() {
    let args = test_args();
    let config = config(false);
    let mut geometry = quad_geometry();
    for vertex in &mut geometry.geometry.vertices {
        vertex.position[0] *= 6.0;
        vertex.position[2] *= 6.0;
    }
    let lights: Vec<MapLight> = (0..5)
        .map(|n| {
            let mut light = point_light(DVec3::new(3.0, 2.0, 3.0), [1.0; 3]);
            light.falloff_range = 10.0;
            light.intensity = 1.0 + n as f32;
            light
        })
        .collect();
    let labels: Vec<String> = (0..5).map(|n| format!("light #{n} (fixture)")).collect();
    let dir = fresh_cache_dir("drop_report");
    let cache = StageCache::new(&dir).expect("drop report cache");
    let mut runs = Vec::new();
    for expected in [
        "[cache] shadowmask_atlas miss",
        "[cache] shadowmask_atlas hit",
    ] {
        let logs = LogCapture::start();
        let mut fixture = geometry.clone();
        let output = run_fused_labeled(
            &args,
            Some(&cache),
            &lights,
            None,
            &labels,
            &config,
            &BakeControl::unrestricted(),
            &BakeControl::unrestricted(),
            &mut fixture,
        );
        logs.assert_logged_once(Level::Info, expected);
        logs.assert_logged_once(
            Level::Warn,
            "[ShadowmaskAtlas] light #0 (fixture) lost its specular mask channel on face 0",
        );
        logs.assert_logged_once(
            Level::Info,
            "[ShadowmaskAtlas] peak per-face owner demand: 5 candidate light(s) on one face",
        );
        for kept in 1..5 {
            logs.assert_not_logged(Level::Warn, &format!("light #{kept} (fixture) lost"));
        }
        runs.push(owners_of(&output));
    }
    assert_eq!(runs[0], runs[1]);
    assert_eq!(runs[0][0], [1, 2, 3, 4]);
    drop(cache);
    let _ = std::fs::remove_dir_all(dir);
}

// More static spec lights than an owner index can name fail the stage by name
// before any lightmap or estimate work. The pipeline runs the same check before
// its first bake stage; this guards callers that reach the stage directly.
#[test]
fn too_many_static_spec_lights_fail_before_any_lightmap_work() {
    let args = test_args();
    let config = config(false);
    let lights: Vec<MapLight> = (0..=usize::from(u16::MAX))
        .map(|_| point_light(DVec3::new(0.5, 1.0, 0.5), [1.0; 3]))
        .collect();
    let mut geometry = quad_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH must build");
    let static_lights = StaticBakedLights::from_lights(&lights);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let prepared =
        lightmap_bake::prepare_atlas(&mut geometry, &static_lights, config.lightmap_density, &[])
            .expect("fixture atlas must prepare");
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(1, false)), &progress);
    let error = match bake_fused_prepared(
        &args,
        None,
        &control,
        &BakeControl::unrestricted(),
        &mut geometry,
        &static_lights,
        &alpha_lights,
        None,
        &[],
        &bvh,
        &primitives,
        &config,
        prepared,
    ) {
        Ok(_) => panic!("an owner index past u16 must fail the stage"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("65536 static spec lights"), "{error}");
    assert_eq!(progress.total(), None, "no lightmap work was published");
    assert_eq!(progress.completed(), 0);
}

/// Pool edge for cut fixtures: a 24 m floor at 0.25 m/texel (96 interior
/// texels) cuts 2 × 2 into 56-texel blocks, and a bake layer (64 at least)
/// holds one of them, so the sub-faces span four bake layers.
const CUT_POOL_EDGE: u32 = 64;

/// Horizontal quads `(x, z, size, y)`; the first is the floor, face 0.
fn horizontal_quads(quads: &[(f32, f32, f32, f32)]) -> GeometryResult {
    let mut geometry = quad_geometry();
    geometry.geometry.vertices.clear();
    geometry.geometry.indices.clear();
    geometry.geometry.faces.clear();
    geometry.face_index_ranges.clear();
    for &(x, z, size, y) in quads {
        push_quad(&mut geometry, x, z, size, y, 0);
    }
    geometry
}

const FLOOR: (f32, f32, f32, f32) = (0.0, 0.0, 24.0, 0.0);

struct CutBake {
    owners: Vec<[u16; 4]>,
    section: Vec<u8>,
    raw: Vec<u8>,
    cut: crate::lightmap_bake::CutParent,
    charts: Vec<crate::lightmap_bake::Chart>,
    placements: Vec<crate::chart_raster::ChartPlacement>,
    atlas_width: u32,
    atlas_height: u32,
    layer_count: u32,
}

impl CutBake {
    /// Raw-fill channel `channel` over sub-face `face`'s padded chart.
    fn channel(&self, face: usize, channel: usize) -> Vec<u8> {
        let (chart, placement) = (&self.charts[face], self.placements[face]);
        let plane = (self.atlas_width * self.atlas_height) as usize;
        (0..chart.height_texels)
            .flat_map(|row| (0..chart.width_texels).map(move |col| (row, col)))
            .map(|(row, col)| {
                let idx = (placement.y + row) * self.atlas_width + placement.x + col;
                self.raw[(placement.layer as usize * plane + idx as usize) * 4 + channel]
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
struct CutRun<'a> {
    workers: usize,
    window: usize,
    cache: Option<&'a StageCache>,
    selection: Option<&'a EntityShadowLightsSection>,
}

impl Default for CutRun<'_> {
    fn default() -> Self {
        Self {
            workers: 1,
            window: LIGHTMAP_PARTITION_WINDOW,
            cache: None,
            selection: None,
        }
    }
}

fn bake_cut(quads: &[(f32, f32, f32, f32)], lights: &[MapLight], run: CutRun<'_>) -> CutBake {
    bake_cut_geometry(horizontal_quads(quads), lights, run)
}

/// [`bake_cut`] over `geometry`, whose face 0 must be cut.
fn bake_cut_geometry(
    mut geometry: GeometryResult,
    lights: &[MapLight],
    run: CutRun<'_>,
) -> CutBake {
    let args = test_args();
    let config = config(false);
    let static_lights = StaticBakedLights::from_lights(lights);
    let alpha_lights = AlphaLightsNs::from_lights(lights);
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(run.workers, false)), &progress);
    let mut bake = || {
        let prepared = lightmap_bake::prepare_atlas_within(
            &mut geometry,
            &static_lights,
            config.lightmap_density,
            &[],
            lightmap_bake::BlockOrdering::by_cell_id(config.direction_texel_scale),
            CUT_POOL_EDGE,
            &control,
        )
        .expect("cut fixture prepares");
        let cut = prepared
            .cut_parents
            .iter()
            .find(|cut| cut.parent == 0)
            .expect("the floor, face 0, is cut")
            .clone();
        let (bvh, primitives, _) = build_bvh(&geometry).expect("cut fixture BVH");
        let window = PartitionWindow {
            size: run.window,
            ..PartitionWindow::default()
        };
        let output = bake_fused_windowed(
            &args,
            run.cache,
            &control,
            &BakeControl::unrestricted(),
            &mut geometry,
            &static_lights,
            &alpha_lights,
            run.selection,
            &[],
            &bvh,
            &primitives,
            &config,
            prepared,
            &window,
        )
        .expect("cut fixture bakes");
        let section = output.shadowmask.expect("the cut floor is lit");
        let raw = crate::shadowmask_bake::take_last_raw_fill_if_any();
        CutBake {
            owners: section.owners.clone(),
            section: section.to_bytes(),
            raw,
            cut,
            charts: output.lightmap.charts,
            placements: output.lightmap.placements,
            atlas_width: output.lightmap.atlas_width,
            atlas_height: output.lightmap.atlas_height,
            layer_count: output.lightmap.layer_count,
        }
    };
    // One worker bakes on the test thread, where log capture reaches.
    if run.workers == 1 {
        return bake();
    }
    ThreadPoolBuilder::new()
        .num_threads(run.workers)
        .build()
        .expect("cut fixture pool")
        .install(bake)
}

fn over(origin: [f64; 3], range: f32, intensity: f32) -> MapLight {
    let mut light = point_light(DVec3::from_array(origin), [1.0; 3]);
    light.falloff_range = range;
    light.intensity = intensity;
    light
}

/// Four broad lights over the whole floor, dimmest first, then a bright light
/// close over one corner that lights only the first sub-face.
fn broad_and_corner() -> Vec<MapLight> {
    vec![
        over([12.0, 8.0, 12.0], 40.0, 2.0),
        over([11.0, 8.0, 13.0], 40.0, 3.0),
        over([13.0, 8.0, 11.0], 40.0, 4.0),
        over([12.0, 9.0, 12.0], 40.0, 5.0),
        over([1.5, 1.0, 1.5], 2.5, 60.0),
    ]
}

// The sub-faces of one cut face carry identical owner sets, even
// where they sit in different bake layers and one sub-face alone would have
// ranked a different fourth light; each owner's mask holds its own baked
// values in every sub-face.
#[test]
fn cut_sub_faces_share_one_owner_set_ranked_over_the_whole_face() {
    const E: u16 = SHADOWMASK_OWNER_EMPTY;
    let lights = broad_and_corner();
    let bake = bake_cut(&[FLOOR], &lights, CutRun::default());
    assert!(bake.layer_count > 1, "the sub-faces span bake layers");
    let layers: std::collections::BTreeSet<u32> = bake
        .cut
        .faces
        .clone()
        .map(|face| bake.placements[face].layer)
        .collect();
    assert!(layers.len() > 1, "sub-faces on {layers:?}");
    for face in bake.cut.faces.clone() {
        assert_eq!(bake.owners[face], [0, 1, 2, 3], "sub-face {face}");
    }

    // Ranked alone, the corner quadrant would keep the corner light.
    let quadrants = [
        (0.0, 0.0, 12.0, 0.0),
        (12.0, 0.0, 12.0, 0.0),
        (0.0, 12.0, 12.0, 0.0),
        (12.0, 12.0, 12.0, 0.0),
    ];
    let uncut = run_fused_with_geometry(
        &test_args(),
        None,
        &lights,
        None,
        &config(false),
        &BakeControl::unrestricted(),
        &BakeControl::unrestricted(),
        horizontal_quads(&quadrants),
    );
    let corner = owners_of(&uncut)[0];
    assert!(
        corner.contains(&4),
        "a lone quadrant ranks the corner light: {corner:?}"
    );
    assert_ne!(corner[3], E);

    // Each owner alone writes the values it writes beside the others.
    for owner in 0..4 {
        let alone = bake_cut(&[FLOOR], &lights[owner..=owner], CutRun::default());
        for face in bake.cut.faces.clone() {
            assert_eq!(
                bake.channel(face, owner),
                alone.channel(face, 0),
                "owner {owner} in sub-face {face}"
            );
        }
    }
}

// With a cut face whose sub-faces span bake layers, id 42 and the drop report
// are identical across worker counts, window sizes, and cold, warm and
// lightmap-reuse builds; two lights tied on the estimate resolve to the lower
// index.
#[test]
fn cut_face_owners_are_identical_across_workers_windows_and_cache_paths() {
    let lights = broad_and_corner();
    let cold = bake_cut(&[FLOOR], &lights, CutRun::default());
    for (workers, window) in [(4, LIGHTMAP_PARTITION_WINDOW), (1, 1), (4, 2)] {
        let other = bake_cut(
            &[FLOOR],
            &lights,
            CutRun {
                workers,
                window,
                ..CutRun::default()
            },
        );
        assert_eq!(
            other.section, cold.section,
            "workers {workers}, window {window}"
        );
    }
    let dir = fresh_cache_dir("cut_face_paths");
    let cache = StageCache::new(&dir).expect("cut cache");
    let cached = CutRun {
        cache: Some(&cache),
        ..CutRun::default()
    };
    // One worker bakes on the test thread, so the estimate's trace count,
    // recorded on the thread that ran it, is visible here.
    crate::shadowmask_bake::reset_estimate_trace_count();
    let logs = LogCapture::start();
    assert_eq!(bake_cut(&[FLOOR], &lights, cached).section, cold.section);
    logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas miss");
    logs.assert_logged(
        Level::Info,
        "[ShadowmaskAtlas] cut-face estimate: 1 cut face(s)",
    );
    logs.assert_logged_once(Level::Warn, "of pre-cut face 0 (now faces 0..=3)");
    drop(logs);
    assert!(
        crate::shadowmask_bake::estimate_trace_count() > 0,
        "a memo miss on a cut face traces the estimate"
    );
    crate::shadowmask_bake::reset_estimate_trace_count();
    let logs = LogCapture::start();
    assert_eq!(bake_cut(&[FLOOR], &lights, cached).section, cold.section);
    logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas hit");
    assert_eq!(
        crate::shadowmask_bake::estimate_trace_count(),
        0,
        "a memo hit traces no estimate sample"
    );
    logs.assert_not_logged(Level::Info, "cut-face estimate");
    logs.assert_logged_once(Level::Warn, "of pre-cut face 0 (now faces 0..=3)");
    drop(logs);
    // Membership alone: the lightmap memo hits and id 42 rebuilds from cache.
    let selection = EntityShadowLightsSection {
        light_indices: vec![4],
    };
    let promoted_cold = bake_cut(
        &[FLOOR],
        &lights,
        CutRun {
            selection: Some(&selection),
            ..CutRun::default()
        },
    );
    let logs = LogCapture::start();
    let reuse = bake_cut(
        &[FLOOR],
        &lights,
        CutRun {
            selection: Some(&selection),
            ..cached
        },
    );
    logs.assert_logged_once(Level::Info, "[cache] lightmap_section hit");
    logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas miss");
    assert_eq!(reuse.section, promoted_cold.section);
    drop(logs);
    drop(cache);
    let _ = std::fs::remove_dir_all(dir);

    // Five identical lights tie on the estimate: the lower four indices own.
    let tied: Vec<MapLight> = (0..5).map(|_| over([12.0, 8.0, 12.0], 40.0, 3.0)).collect();
    let tie = bake_cut(&[FLOOR], &tied, CutRun::default());
    for face in tie.cut.faces.clone() {
        assert_eq!(tie.owners[face], [0, 1, 2, 3]);
    }
}

// On a cut face, a light walled off from the whole face
// takes no channel even in range, a light lighting part of it does, and a light
// that reaches only past the face's edge takes none.
#[test]
fn cut_face_admission_needs_a_lit_interior_texel() {
    const E: u16 = SHADOWMASK_OWNER_EMPTY;
    let roof = (-4.0, -4.0, 32.0, 2.0);
    let lights = vec![
        // Above the roof, in range of the whole floor.
        over([12.0, 4.0, 12.0], 20.0, 4.0),
        // Under the roof, over one corner.
        over([3.0, 1.0, 3.0], 2.5, 2.0),
        // Just past the floor's edge, reaching only its padding.
        over([-0.2, 0.05, 12.0], 0.3, 50.0),
    ];
    let bake = bake_cut(&[FLOOR, roof], &lights, CutRun::default());
    for face in bake.cut.faces.clone() {
        assert_eq!(bake.owners[face], [1, E, E, E], "sub-face {face}");
    }
    // On real geometry: the corner light lights one sub-face, and
    // its channel reads zero across the others, padding included.
    let lit: Vec<usize> = bake
        .cut
        .faces
        .clone()
        .filter(|&face| bake.channel(face, 0).iter().any(|&v| v != 0))
        .collect();
    assert_eq!(
        lit.len(),
        1,
        "the corner light lights one sub-face: {lit:?}"
    );
}

// On a cut face, a promoted light lighting fewer texels than the
// floor takes no channel from an above-floor specular-only light, and is not
// warned.
#[test]
fn a_small_promoted_light_on_a_cut_face_never_displaces_an_above_floor_light() {
    let mut lights: Vec<MapLight> = (0..4)
        .map(|n| over([12.0, 8.0, 12.0], 40.0, 2.0 + n as f32))
        .collect();
    // A tiny bright light 5 cm above the floor at a parent-grid texel whose
    // coordinates are multiples of the estimate stride: it lights that texel
    // and its four neighbours, and no estimate sample.
    let probe = bake_cut(&[FLOOR], &lights, CutRun::default());
    let face = probe.cut.faces.start;
    let chart = &probe.charts[face];
    let window = chart.window.expect("sub-chart");
    let (gx, gy) = (8u32, 8u32);
    let at = crate::chart_raster::chart_texel_world_position(
        chart,
        (gx - window.origin[0]) as i32,
        (gy - window.origin[1]) as i32,
    );
    lights.push(over([f64::from(at.x), 0.05, f64::from(at.z)], 0.3, 500.0));
    let selection = EntityShadowLightsSection {
        light_indices: vec![4],
    };
    let logs = LogCapture::start();
    let bake = bake_cut(
        &[FLOOR],
        &lights,
        CutRun {
            selection: Some(&selection),
            ..CutRun::default()
        },
    );
    for face in bake.cut.faces.clone() {
        assert_eq!(bake.owners[face], [0, 1, 2, 3], "sub-face {face}");
    }
    logs.assert_not_logged(Level::Warn, "no world highlight there");
}

// A cut face only two texels across, cut along its length, still gets estimate
// samples: its thin own segments hold no stride-lattice row, so each is
// sampled at its centre. One light over it owns every sub-face, undropped.
#[test]
fn a_thin_cut_face_gets_estimate_samples_and_an_owner() {
    const E: u16 = SHADOWMASK_OWNER_EMPTY;
    // 24 m × 0.5 m: 96 × 2 interior texels at 0.25 m/texel.
    let mut strip = horizontal_quads(&[FLOOR]);
    // Set exactly: a scaled 0.5 m can round up past two texels.
    for vertex in &mut strip.geometry.vertices {
        if vertex.position[2] > 0.0 {
            vertex.position[2] = 0.5;
        }
    }
    let lights = vec![over([12.0, 2.0, 0.25], 40.0, 3.0)];
    let logs = LogCapture::start();
    let bake = bake_cut_geometry(strip, &lights, CutRun::default());
    assert!(
        bake.cut.faces.len() > 1,
        "the strip is cut along its length"
    );
    assert!(
        bake.cut.own.iter().all(|[u, v]| u.len().min(v.len()) <= 2),
        "every own segment is at most two texels across: {:?}",
        bake.cut.own
    );
    for face in bake.cut.faces.clone() {
        assert_eq!(bake.owners[face], [0, E, E, E], "sub-face {face}");
    }
    logs.assert_not_logged(Level::Warn, "no world highlight there");
}

// A level with no cut face traces no estimate sample, and on a
// cut face the estimate traces only lights whose unshadowed term reaches it.
#[test]
fn the_estimate_traces_only_cut_faces_and_lights_that_reach_them() {
    let near = over([12.0, 8.0, 12.0], 40.0, 2.0);
    let far = over([200.0, 8.0, 200.0], 5.0, 2.0);
    let logs = LogCapture::start();
    let _uncut = run_fused_with_geometry(
        &test_args(),
        None,
        &[near.clone(), far.clone()],
        None,
        &config(false),
        &BakeControl::unrestricted(),
        &BakeControl::unrestricted(),
        horizontal_quads(&[(0.0, 0.0, 6.0, 0.0)]),
    );
    logs.assert_not_logged(Level::Info, "cut-face estimate");
    drop(logs);

    let logs = LogCapture::start();
    let _cut = bake_cut(&[FLOOR], &[near, far], CutRun::default());
    let line = logs
        .records()
        .into_iter()
        .map(|record| record.message)
        .find(|message| message.contains("cut-face estimate"))
        .expect("the estimate reports");
    let number = |label: &str| -> usize {
        let before = line.split(label).next().expect(label);
        before
            .rsplit(' ')
            .find(|word| !word.is_empty())
            .and_then(|word| word.trim_end_matches(',').parse().ok())
            .unwrap_or_else(|| panic!("no count before {label} in {line}"))
    };
    let samples = number(" sample(s)");
    let traces = number(" light trace(s)");
    assert!(samples > 100, "{line}");
    // The 24 m floor sampled every 4 texels of 0.25 m: 24 × 24 samples.
    assert_eq!(samples, 576, "{line}");
    assert_eq!(traces, samples, "only the near light is traced: {line}");
}

mod window_tests;

mod cache_tests;

mod budget_regression;

mod raw_fill_tests;

mod budget_gate_tests;

mod metadata_tests;
