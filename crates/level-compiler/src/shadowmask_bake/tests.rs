// Shadowmask bake tests: selection, channel assignment, fill, encode, memo, and residency bounds.
// See: context/lib/build_pipeline.md §PRL section IDs

use super::memo::MEMO_OVERLAP_PREFIX_BYTES;
use super::section::{build_raw_shadowmask_from_layers, layers_overlap, overlap_graph_from_layers};
use super::*;
use crate::bake_control::BakeControl;
use crate::bvh_build::build_bvh;
use crate::chart_raster::ChartPlacement;
use crate::entity_shadow_select::{EntityShadowSelectionInputs, select_entity_shadow_lights};
use crate::fixture_pipeline::load_fixture;
use crate::governor::Governor;
use crate::light_namespaces::{AlphaLightsNs, StaticBakedLights};
use crate::lightmap_bake::{BlockLayout, CellBlock, Chart, light_texel_is_covered, prepare_atlas};
use crate::lightmap_layer::{LayerTexel, bake_light_layer};
use crate::map_data::{FalloffModel, LightType, ShadowType};
use crate::reporter::StageProgress;
use glam::{DVec3, Vec3};
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::shadowmask_atlas::SHADOWMASK_CHANNEL_DROPPED;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_test_log_capture::LogCapture;
use rayon::ThreadPoolBuilder;

fn light(intensity: f32) -> MapLight {
    MapLight {
        origin: DVec3::ZERO,
        carrier: String::new(),
        light_type: LightType::Point,
        intensity,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 4.0,
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

fn graph_with_edges(light_count: usize, edges: &[(usize, usize)]) -> OverlapGraph {
    OverlapGraph::from_edges(light_count, edges)
}

fn assert_valid_channel_assignment(graph: &OverlapGraph, channels: &[u8]) {
    assert_eq!(graph.light_count(), channels.len());
    for light in 0..graph.light_count() {
        let channel = channels[light];
        assert!(channel < 4 || channel == SHADOWMASK_CHANNEL_DROPPED);
        if channel == SHADOWMASK_CHANNEL_DROPPED {
            continue;
        }
        for (other, &other_channel) in channels.iter().enumerate().skip(light + 1) {
            if graph.overlaps(light, other) && other_channel != SHADOWMASK_CHANNEL_DROPPED {
                assert_ne!(channel, other_channel);
            }
        }
    }
}

fn quad_geometry() -> GeometryResult {
    let n = [0.0, 1.0, 0.0];
    let t = [1.0, 0.0, 0.0];
    GeometryResult {
        geometry: GeometrySection {
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0], [0.0, 0.0], n, t, true, [0.0, 0.0], 0),
                Vertex::new([1.0, 0.0, 0.0], [1.0, 0.0], n, t, true, [0.0, 0.0], 0),
                Vertex::new([1.0, 0.0, 1.0], [1.0, 1.0], n, t, true, [0.0, 0.0], 0),
                Vertex::new([0.0, 0.0, 1.0], [0.0, 1.0], n, t, true, [0.0, 0.0], 0),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            faces: vec![FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            }],
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: vec![crate::geometry::FaceIndexRange {
            index_offset: 0,
            index_count: 6,
        }],
    }
}

fn layer(width: u32, height: u32, layer_count: u32, texels: &[(u32, u32, f32)]) -> LightmapLayer {
    LightmapLayer {
        atlas_width: width,
        atlas_height: height,
        layer_count,
        target_layer: texels.first().map_or(0, |texel| texel.1),
        texels: texels
            .iter()
            .map(|&(idx, _, visibility)| LayerTexel {
                idx,
                raw_visibility: visibility,
            })
            .collect(),
    }
}

fn fake_layer_hash(seed: u8) -> [u8; 32] {
    let mut hash = [0u8; 32];
    hash[0] = seed;
    hash[17] = seed.wrapping_mul(31);
    hash[31] = seed.wrapping_add(7);
    hash
}

use crate::cache::{CacheKey, StageCache};
use crate::lightmap_bake::PreparedAtlas;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::Duration;

const DENSITY: f32 = 0.25;
const AREA_SAMPLES: u32 = 4;

/// The pre-streaming five-way golden: its slot table is the historical
/// capture, and every texel stays fully visible. Restated for the per-block
/// BC5 wire format: one whole-layer block per 8×8 fixture layer.
fn top_level_multilayer_five_way_golden() -> Vec<u8> {
    ShadowmaskAtlasSection {
        channels: vec![0, 1, SHADOWMASK_CHANNEL_DROPPED, 2, 3],
        blocks: encode::all_visible_blocks(&whole_layer_blocks(8, 8, 2)),
    }
    .to_bytes()
}

/// One block per bake layer, each covering the whole layer.
fn whole_layer_blocks(width: u32, height: u32, layer_count: u32) -> Vec<CellBlock> {
    (0..layer_count)
        .map(|layer| CellBlock {
            cell_id: layer,
            width,
            height,
            layer,
            x: 0,
            y: 0,
        })
        .collect()
}

/// Decode a section back to the layer-major raw fill of `shared`'s bake layers.
fn decode_layers(section: &ShadowmaskAtlasSection, shared: &SharedAtlas<'_>) -> Vec<u8> {
    decode_blocks_to_layers(
        section,
        &shared.layout.blocks,
        shared.atlas_width,
        shared.atlas_height,
        layer_count_from_shared(shared),
    )
}

/// Encoded group bytes across every block.
fn payload_len(section: &ShadowmaskAtlasSection) -> usize {
    section.blocks.iter().map(|[a, b]| a.len() + b.len()).sum()
}

fn test_control() -> BakeControl {
    let progress = StageProgress::indeterminate();
    BakeControl::new(Arc::new(Governor::new(1, false)), &progress)
}

fn one_texel_chart() -> Chart {
    Chart {
        origin: Vec3::ZERO,
        u_axis: Vec3::X,
        v_axis: Vec3::Z,
        uv_min: [0.0, 0.0],
        uv_extent: [1.0, 1.0],
        normal: Vec3::Y,
        width_texels: 5,
        height_texels: 5,
        leaf_index: 0,
    }
}

type TopLevelMultilayerFiveWayInputs = (
    GeometryResult,
    bvh::bvh::Bvh<f32, 3>,
    Vec<BvhPrimitive>,
    Vec<Chart>,
    Vec<ChartPlacement>,
    Vec<MapLight>,
    EntityShadowLightsSection,
);

fn top_level_multilayer_five_way_inputs() -> TopLevelMultilayerFiveWayInputs {
    let geometry = quad_geometry();
    let bvh = bvh::bvh::Bvh { nodes: Vec::new() };
    let primitives = Vec::new();
    let charts = vec![one_texel_chart(), one_texel_chart()];
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
    let mut lights = vec![light(5.0), light(4.0), light(1.0), light(3.0), light(2.0)];
    for test_light in &mut lights {
        test_light.origin = DVec3::new(0.5, 1.0, 0.5);
    }
    let selection = EntityShadowLightsSection {
        light_indices: (0..lights.len() as u32).collect(),
    };
    (
        geometry, bvh, primitives, charts, placements, lights, selection,
    )
}

fn fresh_cache_dir(label: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "postretro_shadowmask_cache_test_{label}_{nonce}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn cache_fixture(
    lights: &[MapLight],
) -> (
    GeometryResult,
    PreparedAtlas,
    bvh::bvh::Bvh<f32, 3>,
    Vec<BvhPrimitive>,
) {
    let mut geo = quad_geometry();
    let static_lights = StaticBakedLights::from_lights(lights);
    let prepared = prepare_atlas(&mut geo, &static_lights, DENSITY, &[]).unwrap();
    let (bvh, primitives, _) = build_bvh(&geo).unwrap();
    (geo, prepared, bvh, primitives)
}

fn shared_from_prepared(prepared: &PreparedAtlas) -> SharedAtlas<'_> {
    SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    }
}

fn layer_key(
    light: &MapLight,
    shared: &SharedAtlas<'_>,
    primitives: &[BvhPrimitive],
    geo: &GeometryResult,
    area_samples: u32,
) -> (CacheKey, [u8; 32]) {
    let input_hash =
        lightmap_layer::layer_input_hash(light, shared, primitives, geo, DENSITY, area_samples, 0);
    (
        CacheKey::new(
            "lightmap_layer",
            lightmap_layer::LAYER_FORMAT_VERSION,
            &input_hash,
        ),
        input_hash,
    )
}

fn cached_partition_with_visibility(
    light: &MapLight,
    shared: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geo: &GeometryResult,
    visibility: f32,
) -> LightmapLayer {
    let mut partition = lightmap_layer::bake_light_layer_controlled(
        light,
        shared,
        bvh,
        primitives,
        geo,
        0,
        AREA_SAMPLES,
        &test_control(),
    );
    for texel in &mut partition.texels {
        texel.raw_visibility = visibility;
    }
    partition
}

fn shadowmask_key(
    selection: &EntityShadowLightsSection,
    shared: &SharedAtlas<'_>,
    layer_input_hashes: &[[u8; 32]],
) -> CacheKey {
    let section_hash = shadowmask_atlas_input_hash(
        selection,
        layer_input_hashes,
        shared.atlas_width,
        shared.atlas_height,
        layer_count_from_shared(shared),
    );
    CacheKey::new(
        SHADOWMASK_ATLAS_STAGE_ID,
        SHADOWMASK_ATLAS_STAGE_VERSION,
        &section_hash,
    )
}

/// A section over `blocks` whose every mask texel is `value`.
fn section_for_blocks(
    blocks: &[CellBlock],
    channels: Vec<u8>,
    value: u8,
) -> ShadowmaskAtlasSection {
    let width = blocks.iter().map(|b| b.x + b.width).max().unwrap_or(4);
    let height = blocks.iter().map(|b| b.y + b.height).max().unwrap_or(4);
    let layer_count = blocks.iter().map(|b| b.layer + 1).max().unwrap_or(1);
    let raw = vec![value; (width * height * layer_count * 4) as usize];
    ShadowmaskAtlasSection {
        channels,
        blocks: encode::encode_blocks_bc5(&raw, width, height, layer_count, blocks),
    }
}

enum BadCachedSection {
    Dimensions,
    BlockCount,
    ChannelCount,
}

fn bad_cached_section(shared: &SharedAtlas<'_>, kind: BadCachedSection) -> ShadowmaskAtlasSection {
    let mut blocks = shared.layout.blocks.clone();
    match kind {
        BadCachedSection::Dimensions => {
            blocks[0].width += 4;
            section_for_blocks(&blocks, vec![0], 0)
        }
        BadCachedSection::BlockCount => {
            blocks.push(blocks[0]);
            section_for_blocks(&blocks, vec![0], 0)
        }
        BadCachedSection::ChannelCount => section_for_blocks(&blocks, vec![0, 1], 0),
    }
}

fn assert_cached_section_rebuilt_from_seeded_layer(label: &str, kind: BadCachedSection) {
    let mut test_light = light(5.0);
    test_light.origin = DVec3::new(0.25, 1.0, 0.25);
    let lights = vec![test_light];
    let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
    let shared = shared_from_prepared(&prepared);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let (layer_key, input_hash) = layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
    let section_key = shadowmask_key(&selection, &shared, &[input_hash]);
    let selected = selected_refs(&selection, &alpha_lights);
    let seeded_layer =
        cached_partition_with_visibility(&lights[0], &shared, &bvh, &primitives, &geo, 0.25);
    let expected = bake_shadowmask_atlas_from_layers(
        &selection,
        shared.atlas_width,
        shared.atlas_height,
        layer_count_from_shared(&shared),
        &shared.layout.blocks,
        &selected,
        std::slice::from_ref(&seeded_layer),
    )
    .unwrap();

    let dir = fresh_cache_dir(label);
    let cache = StageCache::new(&dir).expect("cache dir");
    cache.put(
        &section_key,
        &memo_entry(&bad_cached_section(&shared, kind), 0),
    );
    cache.put(&layer_key, &seeded_layer.to_bytes());

    let result = bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geo,
        DENSITY,
        AREA_SAMPLES,
        Some(&cache),
        &test_control(),
    )
    .expect("rebuilt section");

    assert_eq!(result, expected);
    let overwritten = cache
        .get(&section_key)
        .expect("mismatched section entry overwritten");
    assert_eq!(memo_section(&overwritten, &shared), expected);
    let _ = std::fs::remove_dir_all(&dir);
}

enum BadCachedLayer {
    Metadata,
    TexelBounds,
    OutsideCoveredSet,
    DuplicateTexel,
}

fn bad_cached_layer(
    shared: &SharedAtlas<'_>,
    kind: BadCachedLayer,
    light: &MapLight,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geo: &GeometryResult,
) -> LightmapLayer {
    match kind {
        BadCachedLayer::Metadata => layer(
            shared.atlas_width + 1,
            shared.atlas_height,
            layer_count_from_shared(shared) + 1,
            &[(0, 0, 0.25)],
        ),
        BadCachedLayer::TexelBounds => layer(
            shared.atlas_width,
            shared.atlas_height,
            layer_count_from_shared(shared),
            &[
                (shared.atlas_width * shared.atlas_height, 0, 0.25),
                (0, layer_count_from_shared(shared), 0.5),
            ],
        ),
        BadCachedLayer::OutsideCoveredSet => {
            let mut partition =
                cached_partition_with_visibility(light, shared, bvh, primitives, geo, 0.25);
            assert!(partition.texels.len() > 1, "fixture needs multiple texels");
            partition.texels[0].idx = 0;
            partition
        }
        BadCachedLayer::DuplicateTexel => {
            let mut partition =
                cached_partition_with_visibility(light, shared, bvh, primitives, geo, 0.25);
            assert!(partition.texels.len() > 1, "fixture needs multiple texels");
            partition.texels[1] = partition.texels[0];
            partition
        }
    }
}

fn assert_cached_layer_rejected_and_rebaked(label: &str, kind: BadCachedLayer) {
    let mut test_light = light(5.0);
    test_light.origin = DVec3::new(0.25, 1.0, 0.25);
    let lights = vec![test_light];
    let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
    let shared = shared_from_prepared(&prepared);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let (layer_key, input_hash) = layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
    let section_key = shadowmask_key(&selection, &shared, &[input_hash]);
    let invalid = bad_cached_layer(&shared, kind, &lights[0], &bvh, &primitives, &geo);

    let dir = fresh_cache_dir(label);
    let cache = StageCache::new(&dir).expect("cache dir");
    cache.put(&layer_key, &invalid.to_bytes());

    let result = bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geo,
        DENSITY,
        AREA_SAMPLES,
        Some(&cache),
        &test_control(),
    )
    .expect("rebuilt section");

    let stored_layer = cache
        .get(&layer_key)
        .and_then(|bytes| LightmapLayer::from_bytes(&bytes))
        .expect("invalid layer entry overwritten by rebake");
    assert_ne!(
        stored_layer, invalid,
        "invalid decodable lightmap_layer payload must not be reused"
    );
    lightmap_layer::validate_layer_partition(&stored_layer, &shared, 0)
        .expect("rebaked partition matches current atlas");

    let stored_section = cache.get(&section_key).expect("shadowmask section stored");
    assert_eq!(memo_section(&stored_section, &shared), result);
    let _ = std::fs::remove_dir_all(&dir);
}

fn selected_refs<'a>(
    selection: &EntityShadowLightsSection,
    alpha_lights: &'a AlphaLightsNs<'a>,
) -> Vec<(usize, u32, &'a MapLight)> {
    selection
        .light_indices
        .iter()
        .enumerate()
        .filter_map(|(selection_index, &alpha_index)| {
            alpha_lights
                .entries()
                .get(alpha_index as usize)
                .map(|entry| (selection_index, alpha_index, entry.light))
        })
        .collect()
}

#[test]
fn single_light_mask_writes_raw_visibility_to_assigned_channel() {
    let lights = [(0, light(5.0))];
    let selected: Vec<(usize, u32, &MapLight)> = lights
        .iter()
        .enumerate()
        .map(|(selection_index, (i, l))| (selection_index, *i, l))
        .collect();
    let layers = vec![layer(2, 1, 1, &[(0, 0, 0.25), (1, 0, 1.0)])];

    let section = build_raw_shadowmask_from_layers(2, 1, 1, 1, &selected, &layers);

    assert_eq!(section.channels, vec![0]);
    assert_eq!(section.data[0], 64);
    assert_eq!(section.data[4], 255);
    assert_eq!(
        section.data,
        vec![64, 255, 255, 255, 255, 255, 255, 255],
        "single-layer fill literal captured before the analytic restructure"
    );
}

#[test]
fn nan_raw_visibility_is_retained_with_legacy_inclusion_semantics() {
    let lights = [(0, light(5.0)), (1, light(4.0))];
    let selected: Vec<(usize, u32, &MapLight)> = lights
        .iter()
        .enumerate()
        .map(|(selection_index, (index, light))| (selection_index, *index, light))
        .collect();
    let layers = vec![
        layer(1, 1, 1, &[(0, 0, f32::NAN)]),
        layer(1, 1, 1, &[(0, 0, 1.0)]),
    ];

    let section = build_raw_shadowmask_from_layers(1, 1, 1, 2, &selected, &layers);

    let mut analytic_light = light(5.0);
    analytic_light.origin = DVec3::new(0.5, 1.0, 0.5);
    assert!(
        light_texel_is_covered(&analytic_light, Vec3::new(0.5, 0.0, 0.5), Vec3::Y),
        "NaN visibility is downstream of a positive analytic contribution"
    );
    assert!(raw_visibility_is_covered(f32::NAN));

    // NaN was historically included because the old code skipped only
    // values satisfying `raw_visibility < 0.0`. It must still create the
    // overlap edge (and quantizes with the established Rust cast behavior).
    assert_ne!(section.channels[0], section.channels[1]);
    assert_eq!(section.data[section.channels[0] as usize], 0);
    assert_eq!(section.data[section.channels[1] as usize], 255);
}

#[test]
fn sparse_absence_is_not_membership_and_occluded_presence_writes_zero() {
    let lights = [(0, light(5.0)), (1, light(4.0))];
    let selected: Vec<(usize, u32, &MapLight)> = lights
        .iter()
        .enumerate()
        .map(|(selection_index, (index, light))| (selection_index, *index, light))
        .collect();
    let absent = layer(1, 1, 1, &[]);
    let occluded = layer(1, 1, 1, &[(0, 0, 0.0)]);
    assert!(!layers_overlap(&absent, &occluded));

    let section = build_raw_shadowmask_from_layers(1, 1, 1, 2, &selected, &[absent, occluded]);
    assert_eq!(
        section.channels[0], section.channels[1],
        "an absent record must not create an overlap edge"
    );
    assert_eq!(
        section.data[section.channels[1] as usize], 0,
        "a present fully occluded record must write channel byte zero"
    );
}

#[cfg(target_pointer_width = "64")]
#[test]
fn layer_indices_above_u32_do_not_alias_overlap_or_fill_offsets() {
    let first = 0usize;
    let plane = texel_plane_len(8192, 8192);
    let after_u32 = 64usize * plane;
    assert_eq!(after_u32, u32::MAX as usize + 1);
    let layers = vec![
        layer(8192, 8192, 65, &[(0, 0, 1.0)]),
        layer(8192, 8192, 65, &[(0, 64, 1.0)]),
    ];
    let graph = overlap_graph_from_layers(&layers);

    assert!(
        !graph.overlaps(0, 1),
        "distinct global texels must not gain a false overlap edge after u32"
    );
    assert_ne!(
        shadowmask_data_offset(first, 0),
        shadowmask_data_offset(after_u32, 0),
        "fill offsets must retain the full global texel identity"
    );
}

#[test]
fn paused_assignment_barrier_waits_before_shadowmask_fill() {
    let governor = Arc::new(Governor::new(1, true));
    let progress = StageProgress::with_total(3);
    progress
        .completed_handle()
        .store(1, std::sync::atomic::Ordering::Relaxed);
    let (waiting_tx, waiting_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker_governor = Arc::clone(&governor);
    let worker_progress = progress.clone();
    let worker = thread::spawn(move || {
        let control = BakeControl::new(worker_governor, &worker_progress);
        let light = light(1.0);
        let selected = vec![(0, 0, &light)];
        let graph = OverlapGraph::new(1);
        let fill = ShadowmaskFill::new_with_assignment_checkpoint(
            1,
            1,
            1,
            &[],
            1,
            &selected,
            &graph,
            Some(&control),
            None,
            || {
                control.governor().checkpoint_with_wait_observer(|| {
                    waiting_tx.send(()).expect("test coordinator is waiting");
                });
            },
        );
        let section = fill.finish_raw();
        control.advance(1);
        finished_tx
            .send(section)
            .expect("test coordinator is waiting");
    });

    waiting_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("assignment checkpoint did not observe the paused governor");
    assert!(
        matches!(finished_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "assignment must remain blocked after its pause checkpoint reports waiting"
    );
    assert_eq!(progress.completed(), 1);
    assert!(progress.completed() < progress.total().unwrap());

    governor.set_paused(false);
    let section = finished_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("assignment did not resume after unpausing");
    worker.join().expect("assignment worker must not panic");
    assert_eq!(section.data, vec![255, 255, 255, 255]);
    assert_eq!(progress.completed(), progress.total().unwrap());
}

#[test]
fn overlapping_lights_use_different_channels() {
    let lights = [(0, light(5.0)), (1, light(4.0))];
    let selected: Vec<(usize, u32, &MapLight)> = lights
        .iter()
        .enumerate()
        .map(|(selection_index, (i, l))| (selection_index, *i, l))
        .collect();
    let layers = vec![
        layer(1, 1, 1, &[(0, 0, 0.25)]),
        layer(1, 1, 1, &[(0, 0, 0.5)]),
    ];

    let section = build_raw_shadowmask_from_layers(1, 1, 1, 2, &selected, &layers);

    assert_ne!(section.channels[0], section.channels[1]);
    assert_eq!(section.data[section.channels[0] as usize], 64);
    assert_eq!(section.data[section.channels[1] as usize], 128);
}

#[test]
fn five_way_overlap_drops_lowest_intensity_globally() {
    let lights = [
        (0, light(5.0)),
        (1, light(4.0)),
        (2, light(1.0)),
        (3, light(3.0)),
        (4, light(2.0)),
    ];
    let selected: Vec<(usize, u32, &MapLight)> = lights
        .iter()
        .enumerate()
        .map(|(selection_index, (i, l))| (selection_index, *i, l))
        .collect();
    let layers: Vec<LightmapLayer> = (0..5).map(|_| layer(1, 1, 1, &[(0, 0, 1.0)])).collect();

    let section = build_raw_shadowmask_from_layers(1, 1, 1, 5, &selected, &layers);

    assert_eq!(section.channels[2], SHADOWMASK_CHANNEL_DROPPED);
    assert_eq!(
        section
            .channels
            .iter()
            .filter(|&&c| c != SHADOWMASK_CHANNEL_DROPPED)
            .count(),
        4
    );
}

#[test]
fn bounded_exact_search_preserves_legacy_backtracking_assignment() {
    let lights: Vec<MapLight> = (0..10).map(|_| light(1.0)).collect();
    let selected: Vec<(usize, u32, &MapLight)> = lights
        .iter()
        .enumerate()
        .map(|(index, light)| (index, index as u32, light))
        .collect();
    // A five-pair crown graph in alternating pair order makes the legacy
    // static-order greedy branch request a fifth color, then succeeds by
    // backtracking to the graph's two-color solution.
    let mut edges = Vec::new();
    for pair_a in 0..5 {
        for pair_b in 0..5 {
            if pair_a != pair_b {
                edges.push((pair_a * 2, pair_b * 2 + 1));
            }
        }
    }
    let graph = graph_with_edges(lights.len(), &edges);

    let assignment = assign_channels_with_drops_controlled(
        &graph,
        &selected,
        SHADOWMASK_COLOR_SEARCH_NODE_BUDGET,
        || {},
    );

    assert!(!assignment.used_fallback);
    assert!(assignment.exact_nodes_visited > lights.len());
    assert!(
        assignment
            .channels
            .iter()
            .all(|&channel| channel != SHADOWMASK_CHANNEL_DROPPED)
    );
    assert_valid_channel_assignment(&graph, &assignment.channels);
}

#[test]
fn exhausted_search_uses_deterministic_bounded_fallback() {
    let lights: Vec<MapLight> = [0.5, 1.0, 2.0, 3.0, 4.0, 5.0]
        .into_iter()
        .map(light)
        .collect();
    let selected: Vec<(usize, u32, &MapLight)> = lights
        .iter()
        .enumerate()
        .map(|(index, light)| (index, index as u32, light))
        .collect();
    // Vertex 0 is an unrelated low-priority mask. Vertices 1..=5 form K5,
    // so the one-pass fallback keeps the four highest-priority clique
    // members, drops vertex 1, and later retains unrelated vertex 0.
    let mut edges = Vec::new();
    for a in 1..6 {
        for b in a + 1..6 {
            edges.push((a, b));
        }
    }
    let graph = graph_with_edges(lights.len(), &edges);
    let mut checkpoint_count = 0;
    let first = assign_channels_with_drops_controlled(&graph, &selected, 0, || {
        checkpoint_count += 1;
    });
    let second = assign_channels_with_drops_controlled(&graph, &selected, 0, || {});

    assert!(first.used_fallback);
    assert_eq!(first.exact_nodes_visited, 0);
    assert_eq!(first.fallback_lights_considered, graph.light_count());
    assert_eq!(
        first.fallback_operations,
        graph.light_count() * (graph.light_count() + 1)
    );
    assert!(checkpoint_count > 0, "fallback must remain cooperative");
    assert_eq!(first.channels, second.channels);
    assert_ne!(first.channels[0], SHADOWMASK_CHANNEL_DROPPED);
    assert_eq!(first.channels[1], SHADOWMASK_CHANNEL_DROPPED);
    assert_valid_channel_assignment(&graph, &first.channels);
}

#[test]
fn fallback_retention_ties_reverse_legacy_drop_order() {
    let lights: Vec<MapLight> = (0..5).map(|_| light(1.0)).collect();
    let selection_indexes = [4usize, 3, 2, 1, 0];
    let selected: Vec<(usize, u32, &MapLight)> = lights
        .iter()
        .enumerate()
        .map(|(compact_index, light)| {
            (
                selection_indexes[compact_index],
                compact_index as u32,
                light,
            )
        })
        .collect();
    let mut edges = Vec::new();
    for a in 0..5 {
        for b in a + 1..5 {
            edges.push((a, b));
        }
    }
    let graph = graph_with_edges(lights.len(), &edges);

    let assignment = assign_channels_with_drops_controlled(&graph, &selected, 0, || {});

    assert_eq!(assignment.channels[4], SHADOWMASK_CHANNEL_DROPPED);
    assert_eq!(
        assignment
            .channels
            .iter()
            .filter(|&&channel| channel == SHADOWMASK_CHANNEL_DROPPED)
            .count(),
        1
    );
    assert_valid_channel_assignment(&graph, &assignment.channels);
}

#[test]
fn exact_search_never_visits_more_than_injected_budget() {
    let lights: Vec<MapLight> = (0..5).map(|index| light(index as f32 + 1.0)).collect();
    let selected: Vec<(usize, u32, &MapLight)> = lights
        .iter()
        .enumerate()
        .map(|(index, light)| (index, index as u32, light))
        .collect();
    let mut edges = Vec::new();
    for a in 0..5 {
        for b in a + 1..5 {
            edges.push((a, b));
        }
    }
    let graph = graph_with_edges(lights.len(), &edges);

    let assignment = assign_channels_with_drops_controlled(&graph, &selected, 1, || {});

    assert!(assignment.used_fallback);
    assert_eq!(assignment.exact_nodes_visited, 1);
    assert_eq!(assignment.fallback_lights_considered, graph.light_count());
    assert_eq!(
        assignment.fallback_operations,
        graph.light_count() * (graph.light_count() + 1)
    );
    assert_valid_channel_assignment(&graph, &assignment.channels);
}

#[test]
fn iterative_exact_search_handles_large_colorable_order_without_call_stack_depth() {
    const LIGHT_COUNT: usize = 4096;
    let graph = OverlapGraph::new(LIGHT_COUNT);
    let active = vec![true; LIGHT_COUNT];
    let mut budget = ExactColorBudget::new(LIGHT_COUNT + 1);
    let mut operation_count = 0;
    let mut checkpoint = || {};

    let (result, channels) = color_graph_exact_bounded(
        &graph,
        &active,
        &mut budget,
        &mut operation_count,
        &mut checkpoint,
    );

    assert_eq!(result, ExactColorResult::Colored);
    assert_eq!(budget.visited, LIGHT_COUNT + 1);
    assert!(channels.iter().all(|&channel| channel == 0));
}

#[test]
fn dense_exact_and_fallback_neighbor_scans_checkpoint_mid_work() {
    const LIGHT_COUNT: usize = 48;
    let mut edges = Vec::new();
    for a in 0..LIGHT_COUNT {
        for b in a + 1..LIGHT_COUNT {
            edges.push((a, b));
        }
    }
    let graph = graph_with_edges(LIGHT_COUNT, &edges);
    let active = vec![true; LIGHT_COUNT];
    let mut budget = ExactColorBudget::new(1);
    let mut exact_operations = 0;
    let mut exact_checkpoints = 0;
    let mut exact_checkpoint = || exact_checkpoints += 1;

    let (result, _) = color_graph_exact_bounded(
        &graph,
        &active,
        &mut budget,
        &mut exact_operations,
        &mut exact_checkpoint,
    );

    assert_eq!(result, ExactColorResult::BudgetExhausted);
    assert!(
        exact_checkpoints >= 2,
        "dense exact degree and neighbor scans must checkpoint within inner work"
    );

    let lights: Vec<MapLight> = (0..LIGHT_COUNT)
        .map(|index| light(index as f32 + 1.0))
        .collect();
    let selected: Vec<(usize, u32, &MapLight)> = lights
        .iter()
        .enumerate()
        .map(|(index, light)| (index, index as u32, light))
        .collect();
    let mut fallback_active = vec![true; LIGHT_COUNT];
    let mut fallback_operations = 0;
    let mut fallback_checkpoints = 0;
    let mut fallback_checkpoint = || fallback_checkpoints += 1;

    let (channels, considered) = color_graph_priority_greedy(
        &graph,
        &selected,
        &mut fallback_active,
        &mut fallback_operations,
        &mut fallback_checkpoint,
    );

    assert_eq!(considered, LIGHT_COUNT);
    assert_eq!(fallback_operations, LIGHT_COUNT * (LIGHT_COUNT + 1));
    assert!(
        fallback_checkpoints >= 2,
        "dense fallback neighbor scans must checkpoint within inner work"
    );
    assert_eq!(
        channels
            .iter()
            .filter(|&&channel| channel != SHADOWMASK_CHANNEL_DROPPED)
            .count(),
        4
    );
    assert_valid_channel_assignment(&graph, &channels);
}

#[test]
fn analytic_coverage_matches_baked_coverage_on_multilayer_golden() {
    let (geometry, bvh, primitives, charts, placements, lights, _) =
        top_level_multilayer_five_way_inputs();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };

    for test_light in &lights {
        let baked = lightmap_layer::bake_light_layer(
            test_light,
            &shared,
            &bvh,
            &primitives,
            &geometry,
            AREA_SAMPLES,
            &test_control(),
        );
        let mut baked_coverage: Vec<_> = baked
            .iter()
            .flat_map(|partition| {
                partition
                    .texels
                    .iter()
                    .map(|texel| (partition.target_layer, texel.idx))
            })
            .collect();
        let mut analytic_coverage = Vec::new();
        for chart_index in 0..shared.placements.len() {
            lightmap_layer::visit_light_chart_coverage_controlled(
                test_light,
                &shared,
                chart_index,
                &test_control(),
                |texel| analytic_coverage.push((texel.layer, texel.idx)),
            );
        }

        baked_coverage.sort_unstable();
        analytic_coverage.sort_unstable();
        assert_eq!(analytic_coverage, baked_coverage);
    }
}

#[test]
fn shadowmask_graph_storage_is_one_byte_per_light_pair() {
    let graph = OverlapGraph::new(338);

    assert_eq!(graph.storage_bytes(), 338 * 338);
    assert_eq!(graph.storage_bytes(), 114_244);
}

#[test]
fn shadowmask_graph_storage_is_layer_count_independent() {
    let one_layer_graph = OverlapGraph::new(37);
    let many_layer_graph = OverlapGraph::new(37);

    assert_eq!(one_layer_graph.storage_bytes(), 37 * 37);
    assert_eq!(many_layer_graph.storage_bytes(), 37 * 37);
}

#[test]
fn shadowmask_chart_prune_is_coverage_superset() {
    let mut near = light(5.0);
    near.origin = DVec3::new(0.5, 1.0, 0.5);
    let mut remote = light(4.0);
    remote.origin = DVec3::new(100.5, 1.0, 0.5);
    let mut unreachable = light(3.0);
    unreachable.origin = DVec3::new(500.5, 1.0, 0.5);
    let lights = vec![near, remote, unreachable];

    let mut remote_chart = one_texel_chart();
    remote_chart.origin = Vec3::new(100.0, 0.0, 0.0);
    let charts = vec![one_texel_chart(), remote_chart];
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
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let geometry = quad_geometry();
    let world_aabb = lightmap_layer::geometry_world_aabb(&geometry);
    let mut rejected_pairs = 0;

    for (chart_index, chart) in charts.iter().enumerate() {
        let chart_aabb = chart_world_aabb(chart);
        for test_light in &lights {
            if chart_may_receive_light(test_light, chart_aabb, world_aabb) {
                continue;
            }
            rejected_pairs += 1;
            let mut covered_texels = 0;
            lightmap_layer::for_each_light_layer_chart_texel(&shared, chart_index, |sample| {
                covered_texels += usize::from(light_texel_is_covered(
                    test_light,
                    sample.world_p,
                    sample.surface_normal,
                ));
            });
            assert_eq!(
                covered_texels, 0,
                "a rejected light/chart pair contained analytic coverage"
            );
        }
    }

    assert!(rejected_pairs > 0, "fixture must exercise prune rejection");
}

// Regression: pruning used the f64 source origin while coverage rounded it
// to f32, dropping a finite high-coordinate overlap edge and its masks.
#[test]
fn shadowmask_chart_prune_preserves_high_coordinate_coverage_edge_and_masks() {
    let mut chart = one_texel_chart();
    chart.origin = Vec3::new(16_777_216.0, 0.0, 0.0);
    let charts = vec![chart];
    let placements = vec![ChartPlacement {
        x: 0,
        y: 0,
        layer: 0,
    }];
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let mut rounded_origin = light(5.0);
    rounded_origin.origin = DVec3::new(16_777_217.0, 0.05, 0.5);
    rounded_origin.falloff_range = 0.1;
    let mut exact_origin = light(4.0);
    exact_origin.origin = DVec3::new(16_777_216.0, 0.05, 0.5);
    exact_origin.falloff_range = 0.1;
    let lights = [rounded_origin, exact_origin];
    let selected: Vec<_> = lights
        .iter()
        .enumerate()
        .map(|(index, test_light)| (index, index as u32, test_light))
        .collect();
    let geometry = quad_geometry();
    let world_aabb = lightmap_layer::geometry_world_aabb(&geometry);

    assert!(chart_may_receive_light(
        &lights[0],
        chart_world_aabb(&charts[0]),
        world_aabb,
    ));

    let mut shared_covered_texel = None;
    lightmap_layer::for_each_light_layer_chart_texel(&shared, 0, |sample| {
        if light_texel_is_covered(&lights[0], sample.world_p, sample.surface_normal)
            && light_texel_is_covered(&lights[1], sample.world_p, sample.surface_normal)
        {
            shared_covered_texel = Some((sample.idx, shared.placements[0].layer));
        }
    });
    let (covered_idx, covered_layer) =
        shared_covered_texel.expect("both lights must cover one chart texel");

    let chart_order = [0];
    let (pruned_graph, _) = build_analytic_overlap_graph_in_order(
        &selected,
        &shared,
        &geometry,
        &test_control(),
        &chart_order,
        true,
    );
    let (unpruned_graph, _) = build_analytic_overlap_graph_in_order(
        &selected,
        &shared,
        &geometry,
        &test_control(),
        &chart_order,
        false,
    );

    assert!(pruned_graph.overlaps(0, 1));
    assert_eq!(pruned_graph.snapshot(), unpruned_graph.snapshot());
    assert_eq!(
        pruned_graph.peak_texel_overlap(),
        unpruned_graph.peak_texel_overlap()
    );

    let layers = [
        layer(8, 8, 1, &[(covered_idx, covered_layer, 0.25)]),
        layer(8, 8, 1, &[(covered_idx, covered_layer, 0.5)]),
    ];
    let mut fill = ShadowmaskFill::new(8, 8, 1, &[], 2, &selected, &pruned_graph, None, None);
    for (compact_index, layer) in layers.iter().enumerate() {
        fill.write_partition(compact_index, layer);
    }
    let section = fill.finish_raw();
    let first_channel = section.channels[0];
    let second_channel = section.channels[1];
    let global_texel_index = covered_idx as usize + covered_layer as usize * 64;

    assert_ne!(first_channel, SHADOWMASK_CHANNEL_DROPPED);
    assert_ne!(second_channel, SHADOWMASK_CHANNEL_DROPPED);
    assert_ne!(first_channel, second_channel);
    assert_eq!(
        section.data[shadowmask_data_offset(global_texel_index, first_channel).unwrap()],
        64
    );
    assert_eq!(
        section.data[shadowmask_data_offset(global_texel_index, second_channel).unwrap()],
        128
    );
}

#[test]
fn pruned_zero_coverage_light_keeps_node_and_channel_table() {
    let (geometry, bvh, primitives, charts, placements, mut lights, _) =
        top_level_multilayer_five_way_inputs();
    lights.truncate(2);
    let mut remote = light(4.5);
    remote.origin = DVec3::new(100.5, 1.0, 0.5);
    lights.insert(1, remote);
    let selected: Vec<_> = lights
        .iter()
        .enumerate()
        .map(|(index, test_light)| (index, index as u32, test_light))
        .collect();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let chart_order: Vec<_> = (0..charts.len()).collect();
    let (pruned_graph, _) = build_analytic_overlap_graph_in_order(
        &selected,
        &shared,
        &geometry,
        &test_control(),
        &chart_order,
        true,
    );
    let (unpruned_graph, _) = build_analytic_overlap_graph_in_order(
        &selected,
        &shared,
        &geometry,
        &test_control(),
        &chart_order,
        false,
    );

    assert_eq!(pruned_graph.light_count(), 3);
    assert_eq!(pruned_graph.snapshot(), unpruned_graph.snapshot());
    assert_eq!(
        pruned_graph.peak_texel_overlap(),
        unpruned_graph.peak_texel_overlap()
    );
    assert!(charts.iter().all(|chart| !chart_may_receive_light(
        lights.get(1).expect("remote selected light"),
        chart_world_aabb(chart),
        lightmap_layer::geometry_world_aabb(&geometry),
    )));

    let layers: Vec<_> = lights
        .iter()
        .map(|test_light| {
            lightmap_layer::bake_light_layer(
                test_light,
                &shared,
                &bvh,
                &primitives,
                &geometry,
                AREA_SAMPLES,
                &test_control(),
            )
        })
        .collect();
    let mut pruned_fill = ShadowmaskFill::new(
        shared.atlas_width,
        shared.atlas_height,
        layer_count_from_shared(&shared),
        &shared.layout.blocks,
        lights.len(),
        &selected,
        &pruned_graph,
        None,
        None,
    );
    let mut unpruned_fill = ShadowmaskFill::new(
        shared.atlas_width,
        shared.atlas_height,
        layer_count_from_shared(&shared),
        &shared.layout.blocks,
        lights.len(),
        &selected,
        &unpruned_graph,
        None,
        None,
    );
    for (compact_index, partitions) in layers.iter().enumerate() {
        for partition in partitions {
            pruned_fill.write_partition(compact_index, partition);
            unpruned_fill.write_partition(compact_index, partition);
        }
    }
    let pruned_section = pruned_fill.finish();
    let unpruned_section = unpruned_fill.finish();

    assert_ne!(pruned_section.channels[1], SHADOWMASK_CHANNEL_DROPPED);
    assert_eq!(pruned_section.channels, unpruned_section.channels);
    assert_eq!(pruned_section.to_bytes(), unpruned_section.to_bytes());
}

fn analytic_graph_fixture() -> (
    GeometryResult,
    Vec<Chart>,
    Vec<ChartPlacement>,
    Vec<MapLight>,
) {
    let mut remote_chart = one_texel_chart();
    remote_chart.origin = Vec3::new(100.0, 0.0, 0.0);
    let charts = vec![one_texel_chart(), one_texel_chart(), remote_chart];
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
        ChartPlacement {
            x: 0,
            y: 0,
            layer: 2,
        },
    ];
    let mut lights = vec![light(5.0), light(4.0), light(3.0)];
    lights[0].origin = DVec3::new(0.5, 1.0, 0.5);
    lights[1].origin = DVec3::new(0.5, 1.0, 0.5);
    lights[2].origin = DVec3::new(100.5, 1.0, 0.5);
    (quad_geometry(), charts, placements, lights)
}

#[test]
fn analytic_graph_respects_cross_layer_overlap_and_disjoint_reuse() {
    let (geometry, charts, placements, lights) = analytic_graph_fixture();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let selected: Vec<_> = lights
        .iter()
        .enumerate()
        .map(|(index, test_light)| (index, index as u32, test_light))
        .collect();
    let graph = build_analytic_overlap_graph(&selected, &shared, &geometry, &test_control());
    let assignment = assign_channels_with_drops_controlled(
        &graph,
        &selected,
        SHADOWMASK_COLOR_SEARCH_NODE_BUDGET,
        || {},
    );

    assert!(graph.overlaps(0, 1));
    assert!(!graph.overlaps(0, 2));
    assert!(!graph.overlaps(1, 2));
    assert_ne!(assignment.channels[0], assignment.channels[1]);
    assert!(assignment.channels[..2].contains(&assignment.channels[2]));
}

#[test]
fn analytic_graph_is_order_and_worker_count_independent() {
    let (geometry, charts, placements, lights) = analytic_graph_fixture();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let selected: Vec<_> = lights
        .iter()
        .enumerate()
        .map(|(index, test_light)| (index, index as u32, test_light))
        .collect();
    let forward: Vec<_> = (0..charts.len()).collect();
    let reverse: Vec<_> = forward.iter().copied().rev().collect();
    let one_worker = ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let four_workers = ThreadPoolBuilder::new().num_threads(4).build().unwrap();
    let (forward_graph, _) = one_worker.install(|| {
        build_analytic_overlap_graph_in_order(
            &selected,
            &shared,
            &geometry,
            &test_control(),
            &forward,
            true,
        )
    });
    let (reverse_graph, _) = four_workers.install(|| {
        build_analytic_overlap_graph_in_order(
            &selected,
            &shared,
            &geometry,
            &test_control(),
            &reverse,
            true,
        )
    });

    assert_eq!(forward_graph.snapshot(), reverse_graph.snapshot());
    let forward_channels = assign_channels_with_drops_controlled(
        &forward_graph,
        &selected,
        SHADOWMASK_COLOR_SEARCH_NODE_BUDGET,
        || {},
    )
    .channels;
    let reverse_channels = assign_channels_with_drops_controlled(
        &reverse_graph,
        &selected,
        SHADOWMASK_COLOR_SEARCH_NODE_BUDGET,
        || {},
    )
    .channels;
    assert_eq!(forward_channels, reverse_channels);
}

#[test]
fn shadowmask_coloring_waits_for_complete_graph() {
    let (geometry, charts, placements, lights) = analytic_graph_fixture();
    let shared = SharedAtlas {
        charts: &charts[..2],
        placements: &placements[..2],
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements[..2], 2),
    };
    let selected: Vec<_> = lights[..2]
        .iter()
        .enumerate()
        .map(|(index, test_light)| (index, index as u32, test_light))
        .collect();
    let progress = StageProgress::indeterminate();
    let governor = Arc::new(Governor::new(1, false));
    let control = BakeControl::new(Arc::clone(&governor), &progress);
    let pool = ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let entries = Arc::new(AtomicUsize::new(0));
    let (second_tx, second_rx) = mpsc::channel();
    let released = Arc::new((Mutex::new(false), Condvar::new()));
    let hook_entries = Arc::clone(&entries);
    let hook_released = Arc::clone(&released);
    governor.set_enter_hook(Arc::new(move || {
        if hook_entries.fetch_add(1, Ordering::Relaxed) == 1 {
            second_tx.send(()).unwrap();
            let (lock, changed) = &*hook_released;
            let released = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            drop(
                changed
                    .wait_while(released, |released| !*released)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
        }
    }));
    let (coloring_tx, coloring_rx) = mpsc::channel();

    thread::scope(|scope| {
        scope.spawn(|| {
            let graph = pool
                .install(|| build_analytic_overlap_graph(&selected, &shared, &geometry, &control));
            coloring_tx.send(()).unwrap();
            assign_channels_with_drops_controlled(
                &graph,
                &selected,
                SHADOWMASK_COLOR_SEARCH_NODE_BUDGET,
                || {},
            )
        });

        second_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("second graph item did not reach its deterministic barrier");
        assert_eq!(progress.completed(), 1);
        assert!(matches!(
            coloring_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        let (lock, changed) = &*released;
        *lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        changed.notify_all();
        coloring_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("coloring did not start after the graph joined");
    });
}

#[test]
fn shadowmask_progress_advances_during_graph_pass() {
    let (geometry, charts, placements, lights) = analytic_graph_fixture();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let selected: Vec<_> = lights
        .iter()
        .enumerate()
        .map(|(index, test_light)| (index, index as u32, test_light))
        .collect();
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(1, false)), &progress);

    build_analytic_overlap_graph(&selected, &shared, &geometry, &control);

    assert_eq!(progress.completed(), charts.len());
}

#[test]
fn shadowmask_graph_pause_and_permit_retarget_preserve_output() {
    let (geometry, charts, placements, lights) = analytic_graph_fixture();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let selected: Vec<_> = lights
        .iter()
        .enumerate()
        .map(|(index, test_light)| (index, index as u32, test_light))
        .collect();
    let baseline =
        build_analytic_overlap_graph(&selected, &shared, &geometry, &BakeControl::unrestricted());
    let progress = StageProgress::indeterminate();
    let governor = Arc::new(Governor::new(2, false));
    let control = BakeControl::new(Arc::clone(&governor), &progress);
    let pool = ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let (admitted_tx, admitted_rx) = mpsc::channel();
    let released = Arc::new((Mutex::new(false), Condvar::new()));
    let hook_released = Arc::clone(&released);
    governor.set_enter_hook(Arc::new(move || {
        admitted_tx.send(()).unwrap();
        let (lock, changed) = &*hook_released;
        let released = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        drop(
            changed
                .wait_while(released, |released| !*released)
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
    }));
    let (done_tx, done_rx) = mpsc::channel();

    thread::scope(|scope| {
        scope.spawn(|| {
            let graph = pool
                .install(|| build_analytic_overlap_graph(&selected, &shared, &geometry, &control));
            done_tx.send(graph.snapshot()).unwrap();
        });

        admitted_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        admitted_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        governor.set_paused(true);
        governor.set_permits(1);
        let (lock, changed) = &*released;
        *lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        changed.notify_all();

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while progress.completed() < 2 && std::time::Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!(progress.completed(), 2);
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        governor.set_paused(false);
        let retargeted = done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("retargeted graph pass deadlocked after resume");
        assert_eq!(retargeted, baseline.snapshot());
    });
}

#[test]
#[ignore = "explicit mini-warren graph-prune measurement"]
fn measure_mini_warren_shadowmask_graph_reach_fraction() {
    let mut fixture = load_fixture("stress-warren-hallway-inspection-mini");
    let lights = fixture.lights.clone();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let selection = select_entity_shadow_lights(&EntityShadowSelectionInputs {
        bvh: &fixture.bvh,
        primitives: &fixture.primitives,
        geometry: &fixture.geometry,
        static_lights: &static_lights,
        alpha_lights: &alpha_lights,
        params: crate::map_data::EntityShadowParams::default(),
    });
    let prepared = prepare_atlas(&mut fixture.geometry, &static_lights, 0.04, &[])
        .expect("mini-warren atlas planning");
    let shared = shared_from_prepared(&prepared);
    let selected: Vec<_> = selection
        .light_indices
        .iter()
        .enumerate()
        .filter_map(|(selection_index, &alpha_index)| {
            alpha_lights
                .entries()
                .get(alpha_index as usize)
                .map(|entry| (selection_index, alpha_index, entry.light))
        })
        .collect();
    let chart_order: Vec<_> = (0..shared.charts.len()).collect();
    let (_, stats) = build_analytic_overlap_graph_in_order(
        &selected,
        &shared,
        &fixture.geometry,
        &BakeControl::unrestricted(),
        &chart_order,
        true,
    );
    let kept_fraction =
        stats.candidate_light_chart_pairs as f64 / stats.light_chart_pairs.max(1) as f64;

    eprintln!(
        "mini-warren analytic graph prune kept {}/{} light-chart pairs ({:.2}%) across {} selected lights and {} charts",
        stats.candidate_light_chart_pairs,
        stats.light_chart_pairs,
        kept_fraction * 100.0,
        selected.len(),
        shared.charts.len(),
    );
    assert!(stats.light_chart_pairs > 0);
    assert!(stats.candidate_light_chart_pairs < stats.light_chart_pairs);
}

#[test]
fn shadowmask_fixture_is_deterministic_across_rebuilds_and_workers() {
    let mut fixture = load_fixture("gate-heavily-lit");
    let lights = fixture.lights.clone();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let selection = select_entity_shadow_lights(&EntityShadowSelectionInputs {
        bvh: &fixture.bvh,
        primitives: &fixture.primitives,
        geometry: &fixture.geometry,
        static_lights: &static_lights,
        alpha_lights: &alpha_lights,
        params: crate::map_data::EntityShadowParams::default(),
    });
    assert!(!selection.light_indices.is_empty());
    let prepared = prepare_atlas(&mut fixture.geometry, &static_lights, 0.25, &[])
        .expect("gate-heavily-lit atlas planning");
    let (bvh, primitives, _) = build_bvh(&fixture.geometry).unwrap();
    let shared = shared_from_prepared(&prepared);
    let one_worker = ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let many_workers = ThreadPoolBuilder::new().num_threads(4).build().unwrap();
    let one_progress = StageProgress::indeterminate();
    let one_control = BakeControl::new(Arc::new(Governor::new(1, false)), &one_progress);
    let many_progress = StageProgress::indeterminate();
    let many_control = BakeControl::new(Arc::new(Governor::new(4, false)), &many_progress);

    let one = one_worker
        .install(|| {
            bake_shadowmask_atlas(
                Some(&selection),
                &alpha_lights,
                &shared,
                &bvh,
                &primitives,
                &fixture.geometry,
                1,
                &one_control,
            )
        })
        .expect("one-worker named-fixture shadowmask");
    let many = many_workers
        .install(|| {
            bake_shadowmask_atlas(
                Some(&selection),
                &alpha_lights,
                &shared,
                &bvh,
                &primitives,
                &fixture.geometry,
                1,
                &many_control,
            )
        })
        .expect("many-worker named-fixture shadowmask");

    assert_eq!(one.to_bytes(), many.to_bytes());
    assert_eq!(one_progress.completed(), one_progress.total().unwrap());
    assert_eq!(many_progress.completed(), many_progress.total().unwrap());
}

#[test]
fn pre_streaming_multilayer_five_way_overlap_golden_bytes_are_preserved() {
    let lights = [
        (0, light(5.0)),
        (1, light(4.0)),
        (2, light(1.0)),
        (3, light(3.0)),
        (4, light(2.0)),
    ];
    let selected: Vec<(usize, u32, &MapLight)> = lights
        .iter()
        .enumerate()
        .map(|(selection_index, (alpha_index, light))| (selection_index, *alpha_index, light))
        .collect();
    let layers = vec![
        layer(2, 1, 2, &[(0, 0, 0.25)]),
        layer(2, 1, 2, &[(0, 0, 0.5)]),
        layer(2, 1, 2, &[(0, 0, 0.75)]),
        layer(2, 1, 2, &[(0, 0, 1.0)]),
        layer(2, 1, 2, &[(0, 0, 0.0)]),
    ];

    let graph = overlap_graph_from_layers(&layers);
    let mut fill = ShadowmaskFill::new(2, 1, 2, &[], 5, &selected, &graph, None, None);
    for (compact_index, partition) in layers.iter().enumerate() {
        fill.write_partition(compact_index, partition);
    }
    fill.write_partition(0, &layer(2, 1, 2, &[(1, 1, 0.6)]));
    let raw = fill.finish_raw();

    // Captured from the pre-streaming layer composite: two atlas layers,
    // five selected masks sharing a texel (the lowest intensity drops), and
    // an additional layer-1 texel proving layer-major addressing.
    assert_eq!(raw.channels, [0, 1, 0xFF, 2, 3]);
    assert_eq!(
        raw.data,
        [
            64, 128, 255, 0, 255, 255, 255, 255, 255, 255, 255, 255, 153, 255, 255, 255
        ]
    );
}

#[test]
fn top_level_cached_and_no_cache_paths_match_multilayer_five_way_golden() {
    let (geometry, bvh, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let alpha_lights = AlphaLightsNs::from_lights(&lights);

    let no_cache = bake_shadowmask_atlas(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        AREA_SAMPLES,
        &test_control(),
    )
    .expect("no-cache shadowmask section");
    assert_eq!(
        no_cache.to_bytes().as_slice(),
        top_level_multilayer_five_way_golden().as_slice(),
        "no-cache streaming bake must match the pre-streaming golden"
    );

    let mut input_hashes = Vec::new();
    for test_light in &lights {
        for target_layer in 0..layer_count_from_shared(&shared) {
            input_hashes.push(lightmap_layer::layer_input_hash(
                test_light,
                &shared,
                &primitives,
                &geometry,
                DENSITY,
                AREA_SAMPLES,
                target_layer,
            ));
        }
    }
    let cold_dir = fresh_cache_dir("top_level_golden_cold");
    let cold_cache = StageCache::new(&cold_dir).expect("cold cache dir");
    let cached_miss = bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        DENSITY,
        AREA_SAMPLES,
        Some(&cold_cache),
        &test_control(),
    )
    .expect("cached shadowmask section after cold per-light misses");
    assert_eq!(
        cached_miss.to_bytes().as_slice(),
        top_level_multilayer_five_way_golden().as_slice(),
        "cache-backed streaming bake must match the pre-streaming golden"
    );
    let cold_section_key = shadowmask_key(&selection, &shared, &input_hashes);
    let stored_section = cold_cache
        .get(&cold_section_key)
        .expect("cache miss stores the streamed whole section");
    assert_eq!(
        &stored_section[MEMO_OVERLAP_PREFIX_BYTES..],
        top_level_multilayer_five_way_golden().as_slice(),
        "streamed whole-section cache bytes must match the section wire format"
    );

    // Copy every warm light/layer partition to a fresh cache so the next
    // call must take a section miss while reusing every raw-visibility
    // source without accepting a stale neighbouring layer.
    let warm_dir = fresh_cache_dir("top_level_golden_warm_layers");
    let warm_cache = StageCache::new(&warm_dir).expect("warm cache dir");
    for input_hash in &input_hashes {
        let layer_key = CacheKey::new(
            "lightmap_layer",
            lightmap_layer::LAYER_FORMAT_VERSION,
            input_hash,
        );
        let bytes = cold_cache
            .get(&layer_key)
            .expect("cold cache stores each baked layer");
        warm_cache.put(&layer_key, &bytes);
    }
    let cached_warm_layers = bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        DENSITY,
        AREA_SAMPLES,
        Some(&warm_cache),
        &test_control(),
    )
    .expect("cached shadowmask section from warm layers");
    assert_eq!(
        cached_warm_layers.to_bytes().as_slice(),
        top_level_multilayer_five_way_golden().as_slice(),
        "per-light warm cache section miss must match the pre-streaming golden"
    );

    let cached_section_hit = bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        DENSITY,
        AREA_SAMPLES,
        Some(&warm_cache),
        &test_control(),
    )
    .expect("whole shadowmask cache hit");
    assert_eq!(
        cached_section_hit.to_bytes().as_slice(),
        top_level_multilayer_five_way_golden().as_slice(),
        "whole-section cache hit must return the pre-streaming golden"
    );

    let _ = std::fs::remove_dir_all(&cold_dir);
    let _ = std::fs::remove_dir_all(&warm_dir);
}

#[test]
fn fused_plan_colors_before_walk_and_matches_multilayer_golden() {
    let (geometry, bvh, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(4, false)), &progress);
    let mut plan = prepare_fused_shadowmask(
        Some(&selection),
        &alpha_lights,
        &shared,
        &primitives,
        &geometry,
        DENSITY,
        AREA_SAMPLES,
        None,
        &control,
    )
    .expect("aligned fixture atlas");

    for target_layer in 0..layer_count_from_shared(&shared) {
        for (source_index, light) in lights.iter().enumerate() {
            let partition = lightmap_layer::bake_light_layer_controlled(
                light,
                &shared,
                &bvh,
                &primitives,
                &geometry,
                target_layer,
                AREA_SAMPLES,
                &BakeControl::unrestricted(),
            );
            plan.consume_partition(source_index, &partition);
        }
    }
    let section = plan.finish().section;
    assert_eq!(
        section.expect("fused shadowmask").to_bytes(),
        top_level_multilayer_five_way_golden(),
    );
    assert_eq!(progress.completed(), progress.total().unwrap());
}

fn memo_entry(section: &ShadowmaskAtlasSection, peak_texel_overlap: u32) -> Vec<u8> {
    let mut entry = peak_texel_overlap.to_le_bytes().to_vec();
    entry.extend_from_slice(&section.to_bytes());
    entry
}

fn memo_section(entry: &[u8], shared: &SharedAtlas<'_>) -> ShadowmaskAtlasSection {
    ShadowmaskAtlasSection::from_bytes(
        &entry[MEMO_OVERLAP_PREFIX_BYTES..],
        &super::memo::extent_index(shared),
    )
    .expect("memo entry holds a section after its overlap count")
}

/// Run the production fused path: prepare (memo probe, graph, coloring),
/// feed every selected light's baked partitions, finish.
#[allow(clippy::too_many_arguments)]
fn fused_section(
    selection: &EntityShadowLightsSection,
    lights: &[MapLight],
    shared: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    cache: Option<&StageCache>,
) -> Option<ShadowmaskAtlasSection> {
    let alpha_lights = AlphaLightsNs::from_lights(lights);
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(4, false)), &progress);
    let mut plan = prepare_fused_shadowmask(
        Some(selection),
        &alpha_lights,
        shared,
        primitives,
        geometry,
        DENSITY,
        AREA_SAMPLES,
        cache,
        &control,
    )
    .expect("aligned fixture atlas");
    for target_layer in 0..layer_count_from_shared(shared) {
        for (source_index, light) in lights.iter().enumerate() {
            if !plan.needs_source(source_index) {
                continue;
            }
            let partition = lightmap_layer::bake_light_layer_controlled(
                light,
                shared,
                bvh,
                primitives,
                geometry,
                target_layer,
                AREA_SAMPLES,
                &BakeControl::unrestricted(),
            );
            plan.consume_partition(source_index, &partition);
        }
    }
    plan.finish().section
}

/// The retired untagged raw layout over `shared`'s bake layers.
fn pre_bc5_raw_section_bytes(
    section: &ShadowmaskAtlasSection,
    shared: &SharedAtlas<'_>,
) -> Vec<u8> {
    let layer_count = layer_count_from_shared(shared);
    let mut bytes = Vec::new();
    for word in [
        shared.atlas_width,
        shared.atlas_height,
        layer_count,
        section.channels.len() as u32,
    ] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(&section.channels);
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    bytes.resize(
        bytes.len() + (shared.atlas_width * shared.atlas_height * layer_count * 4) as usize,
        255,
    );
    bytes
}

#[test]
fn fused_overlap_report_is_the_graph_peak_on_miss_and_the_memo_value_on_hit() {
    let (geometry, _, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let cache_dir = fresh_cache_dir("overlap_report");
    let cache = StageCache::new(&cache_dir).expect("cache dir");
    let report = |cache: Option<&StageCache>, shared: &SharedAtlas<'_>| {
        let control = test_control();
        prepare_fused_shadowmask(
            Some(&selection),
            &alpha_lights,
            shared,
            &primitives,
            &geometry,
            DENSITY,
            AREA_SAMPLES,
            cache,
            &control,
        )
        .expect("aligned fixture atlas")
        .finish()
        .overlap
    };

    // All five fixture lights cover the one shared texel.
    assert_eq!(report(None, &shared), ShadowmaskOverlapReport::Peak(5));
    assert_eq!(
        report(Some(&cache), &shared),
        ShadowmaskOverlapReport::Peak(5)
    );
    let capture = LogCapture::start();
    assert_eq!(
        report(Some(&cache), &shared),
        ShadowmaskOverlapReport::Peak(5)
    );
    capture.assert_logged_once(log::Level::Info, "[cache] shadowmask_atlas hit");
    let _ = std::fs::remove_dir_all(cache_dir);
}

#[test]
fn memo_entry_without_an_overlap_count_is_a_miss() {
    let (geometry, bvh, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let cache_dir = fresh_cache_dir("overlap_count_missing");
    let cache = StageCache::new(&cache_dir).expect("cache dir");
    let expected = fused_section(
        &selection,
        &lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        Some(&cache),
    )
    .expect("seeding section");

    // Re-store the same key with the section alone: its first four bytes
    // are then read as a count and the rest no longer parses.
    let input_hashes: Vec<_> = lights
        .iter()
        .flat_map(|light| {
            (0..layer_count_from_shared(&shared)).map(|target_layer| {
                lightmap_layer::layer_input_hash(
                    light,
                    &shared,
                    &primitives,
                    &geometry,
                    DENSITY,
                    AREA_SAMPLES,
                    target_layer,
                )
            })
        })
        .collect();
    let section_key = shadowmask_key(&selection, &shared, &input_hashes);
    cache.put(&section_key, &expected.to_bytes());
    let capture = LogCapture::start();
    let rebuilt = fused_section(
        &selection,
        &lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        Some(&cache),
    )
    .expect("rebuilt section");
    capture.assert_logged_once(log::Level::Info, "[cache] shadowmask_atlas miss");
    assert_eq!(rebuilt, expected);
    let _ = std::fs::remove_dir_all(cache_dir);
}

#[test]
fn overlap_report_logs_peak_with_blocks_and_format() {
    let section = section_for_blocks(&whole_layer_blocks(8, 8, 3), vec![0, 1], 255);
    let capture = LogCapture::start();
    log_overlap_report(ShadowmaskOverlapReport::Peak(3), Some(&section));
    log_overlap_report(ShadowmaskOverlapReport::NoSelection, None);
    capture.assert_logged_once(
        log::Level::Info,
        "[ShadowmaskAtlas] peak per-texel overlap: 3 selected light(s) at one texel; \
             3 cell block(s), BC5 .rg in 2 groups (4 slots)",
    );
    assert_eq!(
        capture
            .records()
            .iter()
            .filter(|record| record.message.contains("peak per-texel overlap"))
            .count(),
        1,
        "no selection reports nothing"
    );
}

#[test]
fn fused_prepare_rejects_a_misaligned_atlas_naming_its_dimensions() {
    let (geometry, _, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    // Width-only and height-only misalignment.
    for (width, height) in [(5, 8), (8, 6)] {
        let shared = SharedAtlas {
            charts: &charts,
            placements: &placements,
            atlas_width: width,
            atlas_height: height,
            layout: &BlockLayout::whole_layers(width, height, &placements, 2),
        };
        reset_shadowmask_output_allocation_count();
        let error = prepare_fused_shadowmask(
            Some(&selection),
            &alpha_lights,
            &shared,
            &primitives,
            &geometry,
            DENSITY,
            AREA_SAMPLES,
            None,
            &test_control(),
        )
        .err()
        .unwrap_or_else(|| panic!("a {width}x{height} atlas must fail the bake"));
        assert_eq!(
            error,
            ShadowmaskBakeError::MisalignedAtlas { width, height }
        );
        assert!(error.to_string().contains(&format!("{width}x{height}")));
        assert_eq!(
            shadowmask_output_allocation_count(),
            0,
            "no raw fill may exist for a section that cannot encode"
        );
    }
}

// Regression: the empty-geometry atlas is a 1×1 placeholder with no
// placements; a non-empty selection failed the alignment check on it.
#[test]
fn fused_prepare_treats_an_atlas_without_placements_as_no_section() {
    let (geometry, _, primitives, _, _, lights, selection) = top_level_multilayer_five_way_inputs();
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let shared = SharedAtlas {
        charts: &[],
        placements: &[],
        atlas_width: 1,
        atlas_height: 1,
        layout: &BlockLayout::whole_layers(1, 1, &[], 2),
    };
    let cache_dir = fresh_cache_dir("no_placements");
    let cache = StageCache::new(&cache_dir).expect("cache dir");
    reset_shadowmask_output_allocation_count();
    let output = prepare_fused_shadowmask(
        Some(&selection),
        &alpha_lights,
        &shared,
        &primitives,
        &geometry,
        DENSITY,
        AREA_SAMPLES,
        Some(&cache),
        &test_control(),
    )
    .expect("an atlas without placements is not an error")
    .finish();
    assert_eq!(output.section, None);
    assert_eq!(output.overlap, ShadowmaskOverlapReport::NoSelection);
    assert_eq!(shadowmask_output_allocation_count(), 0, "no raw fill");
    assert_eq!(
        std::fs::read_dir(&cache_dir).map_or(0, |entries| entries.count()),
        0,
        "no memo entry"
    );
    let _ = std::fs::remove_dir_all(cache_dir);
}

// A pool layer holds both mask groups side by side, and the build rejects a
// block wider than a pool layer, so no block is too wide for id 42: a bake
// layer as wide as the old omission threshold still emits the section.
#[test]
fn wide_bake_layers_still_emit_the_shadowmask_for_pool_sized_blocks() {
    let (geometry, _, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let layout = BlockLayout {
        direction_texel_scale: 2,
        blocks: whole_layer_blocks(8, 8, 2),
        chart_blocks: placements.iter().map(|p| p.layer).collect(),
    };
    let wide = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8192,
        atlas_height: 8,
        layout: &layout,
    };
    let capture = LogCapture::start();
    let section = prepare_fused_shadowmask(
        Some(&selection),
        &alpha_lights,
        &wide,
        &primitives,
        &geometry,
        DENSITY,
        AREA_SAMPLES,
        None,
        &test_control(),
    )
    .expect("an aligned wide bake layer is not an error")
    .finish()
    .section
    .expect("8192-wide bake layers still emit id 42");
    assert_eq!(section.blocks.len(), 2);
    capture.assert_not_logged(log::Level::Warn, "texels wide");
}

// Pin: stale-memo. The version bump misses the old key; a raw entry under
// the new key fails `from_bytes` and re-bakes.
#[test]
fn pre_bc5_memo_entries_are_never_served_and_the_rebuild_matches_uncached() {
    let (geometry, bvh, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let uncached = fused_section(
        &selection,
        &lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        None,
    )
    .expect("uncached section");

    let input_hashes: Vec<_> = lights
        .iter()
        .flat_map(|light| {
            (0..layer_count_from_shared(&shared)).map(|target_layer| {
                lightmap_layer::layer_input_hash(
                    light,
                    &shared,
                    &primitives,
                    &geometry,
                    DENSITY,
                    AREA_SAMPLES,
                    target_layer,
                )
            })
        })
        .collect();
    let input_hash = shadowmask_atlas_input_hash(
        &selection,
        &input_hashes,
        shared.atlas_width,
        shared.atlas_height,
        layer_count_from_shared(&shared),
    );
    let stale = pre_bc5_raw_section_bytes(&uncached, &shared);
    let cache_dir = fresh_cache_dir("stale_pre_bc5_memo");
    let cache = StageCache::new(&cache_dir).expect("cache dir");
    for version in [2, SHADOWMASK_ATLAS_STAGE_VERSION] {
        cache.put(
            &CacheKey::new(SHADOWMASK_ATLAS_STAGE_ID, version, &input_hash),
            &stale,
        );
    }

    let capture = LogCapture::start();
    let rebuilt = fused_section(
        &selection,
        &lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        Some(&cache),
    )
    .expect("rebuilt section");
    capture.assert_logged_once(log::Level::Warn, "corrupt shadowmask atlas, re-baking");
    capture.assert_logged_once(log::Level::Info, "[cache] shadowmask_atlas miss");
    assert_eq!(rebuilt.to_bytes(), uncached.to_bytes());
    let _ = std::fs::remove_dir_all(cache_dir);
}

// Pin: warm-equals-cold. A drifted streamed header would fail `from_bytes`
// and silently re-bake, so the second build must log a hit.
#[test]
fn second_fused_build_hits_the_memo_and_warm_equals_uncached_on_real_masks() {
    let mut fixture = load_fixture("soft_shadow_test");
    let lights = fixture.lights.clone();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let selection = select_entity_shadow_lights(&EntityShadowSelectionInputs {
        bvh: &fixture.bvh,
        primitives: &fixture.primitives,
        geometry: &fixture.geometry,
        static_lights: &static_lights,
        alpha_lights: &alpha_lights,
        params: crate::map_data::EntityShadowParams::default(),
    });
    let prepared = prepare_atlas(&mut fixture.geometry, &static_lights, DENSITY, &[])
        .expect("soft_shadow_test atlas planning");
    let (bvh, primitives, _) = build_bvh(&fixture.geometry).unwrap();
    let shared = shared_from_prepared(&prepared);

    let uncached = fused_section(
        &selection,
        &lights,
        &shared,
        &bvh,
        &primitives,
        &fixture.geometry,
        None,
    )
    .expect("uncached section");
    let raw = take_last_raw_fill();
    assert!(
        raw.iter().any(|&mask| mask != 0 && mask != 255),
        "fixture must exercise interior mask values, not only endpoints"
    );

    let cache_dir = fresh_cache_dir("warm_hit_real_masks");
    let cache = StageCache::new(&cache_dir).expect("cache dir");
    for (build, expected_log) in [
        ("cold", "[cache] shadowmask_atlas miss"),
        ("warm", "[cache] shadowmask_atlas hit"),
    ] {
        let capture = LogCapture::start();
        let section = fused_section(
            &selection,
            &lights,
            &shared,
            &bvh,
            &primitives,
            &fixture.geometry,
            Some(&cache),
        )
        .expect("cached section");
        capture.assert_logged_once(log::Level::Info, expected_log);
        capture.assert_not_logged(log::Level::Warn, "shadowmask");
        assert_eq!(section.to_bytes(), uncached.to_bytes(), "{build} bytes");
    }
    let _ = std::fs::remove_dir_all(cache_dir);
}

#[test]
fn cache_miss_holds_one_raw_fill_one_output_and_bounded_encode_scratch() {
    let (geometry, bvh, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let cache_dir = fresh_cache_dir("encode_residency");
    let cache = StageCache::new(&cache_dir).expect("cache dir");
    reset_shadowmask_output_allocation_count();
    let _ = encode::take_peak_encode_residency();
    let _ = take_raw_fill_live_bytes_at_cache_write();

    let section = fused_section(
        &selection,
        &lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        Some(&cache),
    )
    .expect("cache-miss section");

    // Proves the `RawFillBuffer` release and the encoder's scratch. The
    // raw-fill count comes from the allocation counter; the encoder's
    // `raw_fill` field only echoes the slice it was handed, and the
    // test-only `record_raw_fill` copy sits outside both.
    let raw_layer = (shared.atlas_width * shared.atlas_height * 4) as usize;
    let raw_fill = raw_layer * layer_count_from_shared(&shared) as usize;
    assert_eq!(shadowmask_output_allocation_count(), 1, "one raw fill");
    let peak = encode::take_peak_encode_residency().expect("the miss encodes");
    assert_eq!(peak.raw_fill, raw_fill);
    assert_eq!(peak.output_capacity, raw_fill / 2, "one compressed output");
    assert!(
        peak.scratch <= 3 * raw_layer,
        "encode scratch {} exceeds three raw layers ({})",
        peak.scratch,
        3 * raw_layer
    );
    assert_eq!(
        take_raw_fill_live_bytes_at_cache_write(),
        Some(0),
        "the raw fill must be gone before the section is cached"
    );
    assert_eq!(raw_fill_live_bytes(), 0, "and before it is returned");
    assert_eq!(payload_len(&section), raw_fill / 2);
    let _ = std::fs::remove_dir_all(cache_dir);
}

// Measured, not gated: BC5 error against the raw masks on real bakes.
// `cargo test -p postretro-level-compiler --bin prl-build shadowmask_bc5_encode_error -- --ignored --nocapture`
#[test]
#[ignore = "measurement: bakes content/dev/maps fixtures at default density"]
fn shadowmask_bc5_encode_error_on_fixture_bakes() {
    for name in [
        "shadowmask-groups-capture",
        "soft_shadow_test",
        "gate-heavily-lit",
    ] {
        let mut fixture = load_fixture(name);
        let lights = fixture.lights.clone();
        let static_lights = StaticBakedLights::from_lights(&lights);
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let selection = select_entity_shadow_lights(&EntityShadowSelectionInputs {
            bvh: &fixture.bvh,
            primitives: &fixture.primitives,
            geometry: &fixture.geometry,
            static_lights: &static_lights,
            alpha_lights: &alpha_lights,
            params: crate::map_data::EntityShadowParams::default(),
        });
        let prepared = prepare_atlas(
            &mut fixture.geometry,
            &static_lights,
            lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS,
            &[],
        )
        .expect("fixture atlas planning");
        let (bvh, primitives, _) = build_bvh(&fixture.geometry).unwrap();
        let shared = shared_from_prepared(&prepared);
        let Some(section) = bake_shadowmask_atlas(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &fixture.geometry,
            lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT,
            &test_control(),
        ) else {
            eprintln!("{name}: no selected lights, no shadowmask");
            continue;
        };
        let raw = take_last_raw_fill();
        let decoded = decode_layers(&section, &shared);
        let mut used_channels: Vec<usize> = section
            .channels
            .iter()
            .filter(|&&slot| slot != SHADOWMASK_CHANNEL_DROPPED)
            .map(|&slot| slot as usize)
            .collect();
        used_channels.sort_unstable();
        used_channels.dedup();
        let errors: Vec<u8> = raw
            .chunks_exact(4)
            .zip(decoded.chunks_exact(4))
            .flat_map(|(raw, decoded)| {
                used_channels
                    .iter()
                    .map(|&c| raw[c].abs_diff(decoded[c]))
                    .collect::<Vec<_>>()
            })
            .collect();
        let max = errors.iter().copied().max().unwrap_or(0);
        let mean = errors.iter().map(|&e| f64::from(e)).sum::<f64>() / errors.len().max(1) as f64;
        eprintln!(
            "{name}: {}x{}x{} bake layers, {} used slot(s), {} samples: max abs error {max}/255, mean {mean:.4}/255",
            shared.atlas_width,
            shared.atlas_height,
            layer_count_from_shared(&shared),
            used_channels.len(),
            errors.len(),
        );
    }
}

#[test]
fn shadowmask_cache_miss_allocates_one_output_and_streams_without_a_second_payload() {
    let (geometry, bvh, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let alpha_lights = AlphaLightsNs::from_lights(&lights);

    reset_shadowmask_output_allocation_count();
    reset_shadowmask_streamed_cache_write_count();
    let cold = bake_shadowmask_atlas(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        AREA_SAMPLES,
        &test_control(),
    );
    assert!(cold.is_some());
    assert_eq!(shadowmask_output_allocation_count(), 1);
    assert_eq!(shadowmask_streamed_cache_write_count(), 0);

    let cache_dir = fresh_cache_dir("one_output_allocation");
    let cache = StageCache::new(&cache_dir).unwrap();
    reset_shadowmask_output_allocation_count();
    reset_shadowmask_streamed_cache_write_count();
    let cached_miss = bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        DENSITY,
        AREA_SAMPLES,
        Some(&cache),
        &test_control(),
    );
    assert!(cached_miss.is_some());
    assert_eq!(shadowmask_output_allocation_count(), 1);
    assert_eq!(shadowmask_streamed_cache_write_count(), 1);

    let no_alpha_lights = AlphaLightsNs::from_lights(&[]);
    let filtered_selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let filtered_cache_dir = fresh_cache_dir("filtered_one_output_allocation");
    let filtered_cache = StageCache::new(&filtered_cache_dir).unwrap();
    reset_shadowmask_output_allocation_count();
    reset_shadowmask_streamed_cache_write_count();
    let filtered = bake_shadowmask_atlas_cached(
        Some(&filtered_selection),
        &no_alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        DENSITY,
        AREA_SAMPLES,
        Some(&filtered_cache),
        &test_control(),
    );
    assert!(filtered.is_some());
    assert_eq!(shadowmask_output_allocation_count(), 1);
    assert_eq!(shadowmask_streamed_cache_write_count(), 1);

    let _ = std::fs::remove_dir_all(cache_dir);
    let _ = std::fs::remove_dir_all(filtered_cache_dir);
}

#[test]
fn dropped_light_partitions_are_cached_then_hit_next_compile() {
    let (geometry, bvh, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let cold_dir = fresh_cache_dir("dropped_partition_cold");
    let cold_cache = StageCache::new(&cold_dir).unwrap();
    let cold = bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        DENSITY,
        AREA_SAMPLES,
        Some(&cold_cache),
        &test_control(),
    )
    .unwrap();
    let dropped = cold
        .channels
        .iter()
        .position(|&channel| channel == SHADOWMASK_CHANNEL_DROPPED)
        .expect("five-way overlap drops one selected light");

    let warm_dir = fresh_cache_dir("dropped_partition_warm");
    let warm_cache = StageCache::new(&warm_dir).unwrap();
    for (compact_index, test_light) in lights.iter().enumerate() {
        for target_layer in 0..layer_count_from_shared(&shared) {
            let hash = lightmap_layer::layer_input_hash(
                test_light,
                &shared,
                &primitives,
                &geometry,
                DENSITY,
                AREA_SAMPLES,
                target_layer,
            );
            let key = CacheKey::new(
                "lightmap_layer",
                lightmap_layer::LAYER_FORMAT_VERSION,
                &hash,
            );
            let bytes = cold_cache.get(&key).unwrap_or_else(|| {
                    panic!(
                        "selected light {compact_index} layer {target_layer} was not cached; dropped={}",
                        compact_index == dropped
                    )
                });
            warm_cache.put(&key, &bytes);
        }
    }

    let admissions = Arc::new(AtomicUsize::new(0));
    let hook_admissions = Arc::clone(&admissions);
    let governor = Arc::new(Governor::new(4, false));
    governor.set_enter_hook(Arc::new(move || {
        hook_admissions.fetch_add(1, Ordering::Relaxed);
    }));
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(governor, &progress);
    let warm = bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        DENSITY,
        AREA_SAMPLES,
        Some(&warm_cache),
        &control,
    )
    .unwrap();

    assert_eq!(warm.to_bytes(), cold.to_bytes());
    assert_eq!(
        admissions.load(Ordering::Relaxed),
        shared.placements.len(),
        "only analytic graph charts should enter; every selected partition, including the dropped light, must hit"
    );
    let _ = std::fs::remove_dir_all(cold_dir);
    let _ = std::fs::remove_dir_all(warm_dir);
}

// Regression: assembling a partition while its full cold chart batch was
// retained made a four-partition window physically hold five payloads.
#[test]
fn window_sizes_physically_bound_resident_layers_and_preserve_shadowmask_bytes() {
    let (geometry, bvh, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let alpha_lights = AlphaLightsNs::from_lights(&lights);

    for window in [1, 2, 4] {
        let progress = StageProgress::indeterminate();
        let control = BakeControl::new(Arc::new(Governor::new(4, false)), &progress);
        let (section, resident_high_water) = bake_shadowmask_atlas_with_test_window(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geometry,
            AREA_SAMPLES,
            &control,
            window,
        );
        let section = section.expect("selected lights produce a shadowmask section");

        assert_eq!(
            resident_high_water, window,
            "W={window} must be the physical high-water mark, including assembled partitions"
        );
        assert_eq!(
            section.to_bytes().as_slice(),
            top_level_multilayer_five_way_golden().as_slice(),
            "W={window} must not change shipped shadowmask bytes"
        );
    }
}

#[test]
fn high_permit_low_chart_bake_saturates_all_permits_when_window_exposes_eight_tasks() {
    let geometry = quad_geometry();
    let bvh = bvh::bvh::Bvh { nodes: Vec::new() };
    let primitives = Vec::new();
    let charts = vec![one_texel_chart()];
    let placements = vec![ChartPlacement {
        x: 0,
        y: 0,
        layer: 0,
    }];
    let mut lights: Vec<MapLight> = (0..8).map(|index| light(8.0 - index as f32)).collect();
    for test_light in &mut lights {
        test_light.origin = DVec3::new(0.5, 1.0, 0.5);
    }
    let selection = EntityShadowLightsSection {
        light_indices: (0..lights.len() as u32).collect(),
    };
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let progress = StageProgress::indeterminate();
    let governor = Arc::new(Governor::new(8, false));
    let control = BakeControl::new(Arc::clone(&governor), &progress);
    let pool = ThreadPoolBuilder::new()
        .num_threads(8)
        .build()
        .expect("eight-worker pool");

    let (admitted_tx, admitted_rx) = mpsc::channel();
    let admission_sequence = Arc::new(AtomicUsize::new(0));
    let released = Arc::new((Mutex::new(false), Condvar::new()));
    let hook_admission_sequence = Arc::clone(&admission_sequence);
    let hook_released = Arc::clone(&released);
    governor.set_enter_hook(Arc::new(move || {
        // The analytic graph's lone chart item precedes the light/chart
        // batch this regression is measuring.
        if hook_admission_sequence.fetch_add(1, Ordering::Relaxed) == 0 {
            return;
        }
        admitted_tx
            .send(())
            .expect("admission coordinator is waiting");
        let (lock, changed) = &*hook_released;
        let released = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        drop(
            changed
                .wait_while(released, |released| !*released)
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
    }));

    let ((section, resident_high_water), admitted_count) = thread::scope(|scope| {
        let worker = scope.spawn(|| {
            pool.install(|| {
                bake_shadowmask_atlas_with_test_window(
                    Some(&selection),
                    &alpha_lights,
                    &shared,
                    &bvh,
                    &primitives,
                    &geometry,
                    AREA_SAMPLES,
                    &control,
                    8,
                )
            })
        });

        let mut admitted_count = 0;
        while admitted_count < 8 && admitted_rx.recv_timeout(Duration::from_secs(2)).is_ok() {
            admitted_count += 1;
        }
        let (lock, changed) = &*released;
        *lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        changed.notify_all();

        (
            worker
                .join()
                .expect("shadowmask saturation worker panicked"),
            admitted_count,
        )
    });

    assert_eq!(
        admitted_count, 8,
        "one-chart selected-light work must expose the full -j 8 permit cap"
    );
    assert!(section.is_some());
    assert!(
        resident_high_water <= 8,
        "the chart batch must stay within its full-layer residency window"
    );
    let expected_total = shadowmask_progress_total(lights.len(), &shared);
    assert_eq!(progress.total(), Some(expected_total));
    assert_eq!(progress.completed(), expected_total);
}

// Regression: permit-driven batch widening retained eight raw full-layer payloads at W=1.
#[test]
fn one_layer_window_bounds_cold_one_chart_payloads_below_eight_permits() {
    let geometry = quad_geometry();
    let bvh = bvh::bvh::Bvh { nodes: Vec::new() };
    let primitives = Vec::new();
    let charts = vec![one_texel_chart()];
    let placements = vec![ChartPlacement {
        x: 0,
        y: 0,
        layer: 0,
    }];
    let mut lights: Vec<MapLight> = (0..8).map(|index| light(8.0 - index as f32)).collect();
    for test_light in &mut lights {
        test_light.origin = DVec3::new(0.5, 1.0, 0.5);
    }
    let selection = EntityShadowLightsSection {
        light_indices: (0..lights.len() as u32).collect(),
    };
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(8, false)), &progress);
    let pool = ThreadPoolBuilder::new()
        .num_threads(8)
        .build()
        .expect("eight-worker pool");

    let (section, resident_high_water) = pool.install(|| {
        bake_shadowmask_atlas_with_test_window(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geometry,
            AREA_SAMPLES,
            &control,
            1,
        )
    });

    assert!(section.is_some());
    assert_eq!(resident_high_water, 1);
    let expected_total = shadowmask_progress_total(lights.len(), &shared);
    assert_eq!(progress.total(), Some(expected_total));
    assert_eq!(progress.completed(), expected_total);
}

#[test]
fn reversed_partition_fill_keeps_compact_selection_bytes() {
    let lights = [(0, light(5.0)), (1, light(4.0)), (2, light(3.0))];
    let selected: Vec<(usize, u32, &MapLight)> = lights
        .iter()
        .enumerate()
        .map(|(selection_index, (alpha_index, light))| (selection_index, *alpha_index, light))
        .collect();
    let layers = vec![
        layer(1, 1, 1, &[(0, 0, 0.25)]),
        layer(1, 1, 1, &[(0, 0, 0.5)]),
        layer(1, 1, 1, &[(0, 0, 0.75)]),
    ];
    let submission_order = build_raw_shadowmask_from_layers(1, 1, 1, 3, &selected, &layers);

    let graph = overlap_graph_from_layers(&layers);
    let mut reversed_fill = ShadowmaskFill::new(1, 1, 1, &[], 3, &selected, &graph, None, None);
    for compact_light_index in [2, 1, 0] {
        reversed_fill.write_partition(compact_light_index, &layers[compact_light_index]);
    }
    let reversed = reversed_fill.finish_raw();

    assert_eq!(reversed, submission_order);
}

#[test]
fn one_permit_with_window_four_completes_and_reports_all_chart_work() {
    let (geometry, bvh, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let total = shadowmask_progress_total(selection.light_indices.len(), &shared);
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(1, false)), &progress);
    let pool = ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .expect("two-worker pool");
    let (section, resident_high_water) = pool.install(|| {
        bake_shadowmask_atlas_with_test_window(
            Some(&selection),
            &alpha_lights,
            &shared,
            &bvh,
            &primitives,
            &geometry,
            AREA_SAMPLES,
            &control,
            4,
        )
    });

    assert!(section.is_some(), "-j 1 windowed bake must complete");
    assert_eq!(progress.total(), Some(total));
    assert_eq!(progress.completed(), total);
    assert!(resident_high_water <= 4);
}

#[test]
fn last_partial_batch_assigns_cross_batch_overlap_once_globally() {
    let (geometry, bvh, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let (section, _) = bake_shadowmask_atlas_with_test_window(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        AREA_SAMPLES,
        &test_control(),
        2,
    );
    let section = section.expect("last partial batch still emits a section");

    // Five lights cover the fixture texel. The lowest-intensity third light
    // is dropped, while A (first batch) and E (last partial batch) must be
    // colored differently by the one global post-batch assignment.
    assert_ne!(section.channels[0], SHADOWMASK_CHANNEL_DROPPED);
    assert_ne!(section.channels[4], SHADOWMASK_CHANNEL_DROPPED);
    assert_ne!(section.channels[0], section.channels[4]);
}

#[test]
fn zero_selection_keeps_the_none_section_path() {
    let selection = EntityShadowLightsSection {
        light_indices: Vec::new(),
    };
    assert_eq!(
        bake_shadowmask_atlas_from_layers(&selection, 1, 1, 1, &[], &[], &[]),
        None
    );
}

#[test]
fn all_filtered_selection_keeps_empty_bytes_and_indeterminate_progress() {
    let source_lights = vec![light(5.0)];
    let (geo, prepared, bvh, primitives) = cache_fixture(&source_lights);
    let shared = shared_from_prepared(&prepared);
    let no_alpha_lights = AlphaLightsNs::from_lights(&[]);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(1, false)), &progress);

    let section = bake_shadowmask_atlas(
        Some(&selection),
        &no_alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geo,
        AREA_SAMPLES,
        &control,
    )
    .expect("all-filtered selection still emits an empty section");

    assert_eq!(section.channels, vec![SHADOWMASK_CHANNEL_DROPPED]);
    // Pin: all-sentinel. The section still ships, one block per cell block,
    // at half the raw bytes of those blocks.
    assert_eq!(section.blocks.len(), shared.layout.blocks.len());
    let block_raw: usize = shared
        .layout
        .blocks
        .iter()
        .map(|b| (b.width * b.height * 4) as usize)
        .sum();
    assert_eq!(payload_len(&section), block_raw / 2);
    assert!(
        decode_layers(&section, &shared)
            .iter()
            .all(|&mask| mask == 255)
    );
    assert_eq!(progress.total(), None);
    assert_eq!(progress.completed(), 0);

    let cache_dir = fresh_cache_dir("all_filtered_progress");
    let cache = StageCache::new(&cache_dir).expect("cache dir");
    let cached_progress = StageProgress::indeterminate();
    let cached_control = BakeControl::new(Arc::new(Governor::new(1, false)), &cached_progress);
    let cached_section = bake_shadowmask_atlas_cached(
        Some(&selection),
        &no_alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geo,
        DENSITY,
        AREA_SAMPLES,
        Some(&cache),
        &cached_control,
    )
    .expect("cached all-filtered selection still emits an empty section");

    assert_eq!(cached_section, section);
    let invalid_hashes: Vec<_> = (0..layer_count_from_shared(&shared))
        .map(|target_layer| invalid_selected_light_hash(0, target_layer))
        .collect();
    let section_key = shadowmask_key(&selection, &shared, &invalid_hashes);
    assert_eq!(
        cache
            .get(&section_key)
            .expect("all-filtered route stores the streamed whole section"),
        memo_entry(&section, 0),
        "all-filtered cache bytes must match the section wire format"
    );
    assert_eq!(cached_progress.total(), None);
    assert_eq!(cached_progress.completed(), 0);
    let _ = std::fs::remove_dir_all(cache_dir);
}

#[test]
fn multi_layer_payload_uses_layer_major_texel_indexing() {
    let lights = [(0, light(5.0))];
    let selected: Vec<(usize, u32, &MapLight)> = lights
        .iter()
        .enumerate()
        .map(|(selection_index, (i, l))| (selection_index, *i, l))
        .collect();
    let layers = vec![layer(1, 1, 2, &[(0, 1, 0.5)])];

    let section = build_raw_shadowmask_from_layers(1, 1, 2, 1, &selected, &layers);

    assert_eq!(section.data.len(), 8);
    assert_eq!(section.data[0], 255);
    assert_eq!(section.data[4], 128);
}

// Regression: an invalid earlier AlphaLights selection compacted the valid
// light left, so the channel table no longer matched selection indices.
#[test]
fn invalid_selected_alpha_light_preserves_original_channel_slot() {
    let valid_light = light(5.0);
    let selected = vec![(1usize, 0u32, &valid_light)];
    let selection = EntityShadowLightsSection {
        light_indices: vec![99, 0],
    };
    let layers = vec![layer(4, 4, 1, &[(0, 0, 0.25)])];

    let section = bake_shadowmask_atlas_from_layers(
        &selection,
        4,
        4,
        1,
        &whole_layer_blocks(4, 4, 1),
        &selected,
        &layers,
    )
    .unwrap();

    assert_eq!(section.channels[0], SHADOWMASK_CHANNEL_DROPPED);
    assert_ne!(section.channels[1], SHADOWMASK_CHANNEL_DROPPED);
    assert_eq!(take_last_raw_fill()[section.channels[1] as usize], 64);
}

// Regression: the compatibility route only checked this alignment in debug
// builds, so release could emit a section with inconsistent channel data.
#[test]
#[should_panic(expected = "shadowmask selected light/layer slices must align")]
fn preloaded_layer_bake_rejects_missing_layer_for_selected_light() {
    let first_light = light(5.0);
    let second_light = light(4.0);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0, 1],
    };
    let selected = vec![(0usize, 0u32, &first_light), (1usize, 1u32, &second_light)];
    let layers = vec![layer(1, 1, 1, &[(0, 0, 0.25)])];

    let _ = bake_shadowmask_atlas_from_layers(&selection, 1, 1, 1, &[], &selected, &layers);
}

// Regression: the compatibility route only checked this alignment in debug
// builds, so release could retain a layer with no selected-light channel.
#[test]
#[should_panic(expected = "shadowmask selected light/layer slices must align")]
fn preloaded_layer_bake_rejects_layer_without_selected_light() {
    let valid_light = light(5.0);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let selected = vec![(0usize, 0u32, &valid_light)];
    let layers = vec![
        layer(1, 1, 1, &[(0, 0, 0.25)]),
        layer(1, 1, 1, &[(0, 0, 0.5)]),
    ];

    let _ = bake_shadowmask_atlas_from_layers(&selection, 1, 1, 1, &[], &selected, &layers);
}

// Regression: a preloaded texel in a later layer could overwrite layer one.
#[test]
#[should_panic(expected = "preloaded shadowmask texel index")]
fn preloaded_layer_bake_rejects_texel_index_outside_its_plane() {
    let valid_light = light(5.0);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let selected = vec![(0usize, 0u32, &valid_light)];
    let layers = vec![layer(1, 1, 2, &[(1, 0, 0.25)])];

    let _ = bake_shadowmask_atlas_from_layers(&selection, 1, 1, 2, &[], &selected, &layers);
}

#[test]
#[should_panic(expected = "preloaded shadowmask partition layer")]
fn preloaded_layer_bake_rejects_texel_layer_outside_atlas() {
    let valid_light = light(5.0);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let selected = vec![(0usize, 0u32, &valid_light)];
    let layers = vec![layer(1, 1, 1, &[(0, 1, 0.25)])];

    let _ = bake_shadowmask_atlas_from_layers(&selection, 1, 1, 1, &[], &selected, &layers);
}

#[test]
fn preloaded_layer_bake_matches_uncached_shadowmask_section_bytes() {
    let mut geo = quad_geometry();
    let mut light_a = light(5.0);
    light_a.origin = DVec3::new(0.25, 1.0, 0.25);
    let mut light_b = light(3.0);
    light_b.origin = DVec3::new(0.75, 1.0, 0.75);
    let lights = vec![light_a, light_b];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let prepared = prepare_atlas(&mut geo, &static_lights, 0.25, &[]).unwrap();
    let (bvh, primitives, _) = build_bvh(&geo).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0, 1],
    };

    let uncached = bake_shadowmask_atlas(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geo,
        4,
        &test_control(),
    )
    .unwrap();

    let selected: Vec<(usize, u32, &MapLight)> = selection
        .light_indices
        .iter()
        .enumerate()
        .filter_map(|(selection_index, &alpha_index)| {
            alpha_lights
                .entries()
                .get(alpha_index as usize)
                .map(|entry| (selection_index, alpha_index, entry.light))
        })
        .collect();
    let layers: Vec<LightmapLayer> = selected
        .iter()
        .map(|(_, _, light)| {
            bake_light_layer(light, &shared, &bvh, &primitives, &geo, 4, &test_control())
                .into_iter()
                .next()
                .expect("fixture has one atlas layer")
        })
        .collect();

    let preloaded = bake_shadowmask_atlas_from_layers(
        &selection,
        shared.atlas_width,
        shared.atlas_height,
        layer_count_from_shared(&shared),
        &shared.layout.blocks,
        &selected,
        &layers,
    )
    .unwrap();

    assert_eq!(
        preloaded.to_bytes(),
        uncached.to_bytes(),
        "cache-facing preloaded layer path must preserve uncached section bytes"
    );
}

#[test]
fn shadowmask_atlas_cache_hit_returns_section_without_layer_entries() {
    let mut test_light = light(5.0);
    test_light.origin = DVec3::new(0.25, 1.0, 0.25);
    let lights = vec![test_light];
    let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
    let shared = shared_from_prepared(&prepared);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let (_, input_hash) = layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
    let section_key = shadowmask_key(&selection, &shared, &[input_hash]);
    let cached = section_for_blocks(&shared.layout.blocks, vec![3], 0);

    let dir = fresh_cache_dir("whole_section_hit");
    let cache = StageCache::new(&dir).expect("cache dir");
    cache.put(&section_key, &memo_entry(&cached, 0));

    let result = bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geo,
        DENSITY,
        AREA_SAMPLES,
        Some(&cache),
        &test_control(),
    )
    .expect("cached section");

    assert_eq!(
        result, cached,
        "whole-section hit must return cached bytes without requiring lightmap_layer entries"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shadowmask_atlas_cache_hit_with_wrong_dimensions_is_rebuilt() {
    assert_cached_section_rebuilt_from_seeded_layer(
        "wrong_section_dimensions",
        BadCachedSection::Dimensions,
    );
}

#[test]
fn shadowmask_atlas_cache_hit_with_wrong_layer_count_is_rebuilt() {
    assert_cached_section_rebuilt_from_seeded_layer(
        "wrong_section_layer_count",
        BadCachedSection::BlockCount,
    );
}

#[test]
fn shadowmask_atlas_cache_hit_with_wrong_channel_count_is_rebuilt() {
    assert_cached_section_rebuilt_from_seeded_layer(
        "wrong_section_channel_count",
        BadCachedSection::ChannelCount,
    );
}

#[test]
fn pre_analytic_layer_cache_is_reused_without_rebake() {
    let mut test_light = light(5.0);
    test_light.origin = DVec3::new(0.25, 1.0, 0.25);
    let lights = vec![test_light];
    let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
    let shared = shared_from_prepared(&prepared);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let (layer_key, input_hash) = layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
    let section_key = shadowmask_key(&selection, &shared, &[input_hash]);
    let selected = selected_refs(&selection, &alpha_lights);
    let seeded_layer =
        cached_partition_with_visibility(&lights[0], &shared, &bvh, &primitives, &geo, 0.25);
    let expected = bake_shadowmask_atlas_from_layers(
        &selection,
        shared.atlas_width,
        shared.atlas_height,
        layer_count_from_shared(&shared),
        &shared.layout.blocks,
        &selected,
        std::slice::from_ref(&seeded_layer),
    )
    .unwrap();

    let dir = fresh_cache_dir("section_miss_reuse_layer");
    let cache = StageCache::new(&dir).expect("cache dir");
    cache.put(&layer_key, &seeded_layer.to_bytes());

    let result = bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geo,
        DENSITY,
        AREA_SAMPLES,
        Some(&cache),
        &test_control(),
    )
    .expect("rebuilt section");

    assert_eq!(
        result, expected,
        "section miss must build from the existing lightmap_layer payload"
    );
    let stored = cache.get(&section_key).expect("shadowmask section stored");
    assert_eq!(memo_section(&stored, &shared), expected);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shadowmask_cache_epochs_pin_sparse_layer_values() {
    assert_eq!(SHADOWMASK_ATLAS_STAGE_VERSION, 6);
    assert_eq!(lightmap_layer::LAYER_FORMAT_VERSION, 8);
    assert_eq!(lightmap_layer::LIGHTMAP_SECTION_VERSION, 4);
}

#[test]
fn shadowmask_final_progress_unit_follows_section_memo_write() {
    let dir = fresh_cache_dir("final_progress_order");
    let cache = StageCache::new(&dir).unwrap();
    let key = CacheKey::new(
        SHADOWMASK_ATLAS_STAGE_ID,
        SHADOWMASK_ATLAS_STAGE_VERSION,
        &[7; 32],
    );
    let section = section_for_blocks(&whole_layer_blocks(4, 4, 1), vec![0], 255);
    let progress = StageProgress::with_total(1);
    let control = BakeControl::new(Arc::new(Governor::new(1, false)), &progress);

    cache_shadowmask_section_then_complete(&cache, &key, &section, 0, &control, true, || {
        assert!(cache.get(&key).is_some());
        assert_eq!(progress.completed(), 0);
    });

    assert_eq!(progress.completed(), 1);
    let _ = std::fs::remove_dir_all(dir);

    let light = light(1.0);
    let selected = vec![(0, 0, &light)];
    let graph = OverlapGraph::new(1);
    let uncached_progress = StageProgress::with_total(2);
    let uncached_control = BakeControl::new(Arc::new(Governor::new(1, false)), &uncached_progress);
    let fill = ShadowmaskFill::new(
        4,
        4,
        1,
        &whole_layer_blocks(4, 4, 1),
        1,
        &selected,
        &graph,
        Some(&uncached_control),
        None,
    );
    assert_eq!(uncached_progress.completed(), 1);
    let uncached_section = fill.finish();
    assert_eq!(uncached_progress.completed(), 1);
    assert_eq!(payload_len(&uncached_section), 2 * 16);
    uncached_control.advance(1);
    assert_eq!(uncached_progress.completed(), 2);
}

#[test]
fn shadowmask_publishes_one_total_and_completes_with_section() {
    let (geometry, bvh, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(4, false)), &progress);

    let section = bake_shadowmask_atlas(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        AREA_SAMPLES,
        &control,
    );

    assert!(section.is_some());
    let total = progress
        .total()
        .expect("stage publishes a determinate total");
    assert_eq!(total, shadowmask_progress_total(lights.len(), &shared));
    assert_eq!(progress.completed(), total);
}

#[test]
fn one_light_change_reruns_graph_and_reuses_unchanged_partitions() {
    let (geometry, bvh, primitives, charts, placements, lights, selection) =
        top_level_multilayer_five_way_inputs();
    let shared = SharedAtlas {
        charts: &charts,
        placements: &placements,
        atlas_width: 8,
        atlas_height: 8,
        layout: &BlockLayout::whole_layers(8, 8, &placements, 2),
    };
    let cache_dir = fresh_cache_dir("one_light_change");
    let cache = StageCache::new(&cache_dir).unwrap();
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        DENSITY,
        AREA_SAMPLES,
        Some(&cache),
        &test_control(),
    )
    .unwrap();

    let mut changed_lights = lights.clone();
    changed_lights[2].intensity += 0.5;
    let changed_alpha_lights = AlphaLightsNs::from_lights(&changed_lights);
    let changed_hashes: Vec<_> = (0..layer_count_from_shared(&shared))
        .map(|target_layer| {
            lightmap_layer::layer_input_hash(
                &changed_lights[2],
                &shared,
                &primitives,
                &geometry,
                DENSITY,
                AREA_SAMPLES,
                target_layer,
            )
        })
        .collect();
    for hash in &changed_hashes {
        let key = CacheKey::new("lightmap_layer", lightmap_layer::LAYER_FORMAT_VERSION, hash);
        assert!(cache.get(&key).is_none());
    }

    let admissions = Arc::new(AtomicUsize::new(0));
    let hook_admissions = Arc::clone(&admissions);
    let governor = Arc::new(Governor::new(4, false));
    governor.set_enter_hook(Arc::new(move || {
        hook_admissions.fetch_add(1, Ordering::Relaxed);
    }));
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(governor, &progress);
    let changed = bake_shadowmask_atlas_cached(
        Some(&selection),
        &changed_alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geometry,
        DENSITY,
        AREA_SAMPLES,
        Some(&cache),
        &control,
    );

    assert!(changed.is_some());
    assert_eq!(progress.completed(), progress.total().unwrap());
    assert_eq!(
        admissions.load(Ordering::Relaxed),
        shared.placements.len() * 2,
        "the changed section must rerun one graph chart item and one changed-light bake item per chart; unchanged partitions must hit"
    );
    for hash in &changed_hashes {
        let key = CacheKey::new("lightmap_layer", lightmap_layer::LAYER_FORMAT_VERSION, hash);
        assert!(cache.get(&key).is_some());
    }
    let _ = std::fs::remove_dir_all(cache_dir);
}

#[test]
fn cached_streaming_layers_stay_bounded_and_warm_progress_completes() {
    let mut lights: Vec<MapLight> = (1..=5).map(|intensity| light(intensity as f32)).collect();
    for test_light in &mut lights {
        test_light.origin = DVec3::new(0.25, 1.0, 0.25);
    }
    let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
    let shared = shared_from_prepared(&prepared);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let selection = EntityShadowLightsSection {
        light_indices: (0..lights.len() as u32).collect(),
    };
    let hashes: Vec<[u8; 32]> = lights
        .iter()
        .map(|test_light| {
            lightmap_layer::layer_input_hash(
                test_light,
                &shared,
                &primitives,
                &geo,
                DENSITY,
                AREA_SAMPLES,
                0,
            )
        })
        .collect();
    let dir = fresh_cache_dir("streaming_bound_and_warm_progress");
    let cache = StageCache::new(&dir).expect("cache dir");
    for (index, hash) in hashes.iter().enumerate() {
        let key = CacheKey::new("lightmap_layer", lightmap_layer::LAYER_FORMAT_VERSION, hash);
        let partition = cached_partition_with_visibility(
            &lights[index],
            &shared,
            &bvh,
            &primitives,
            &geo,
            index as f32 / 4.0,
        );
        cache.put(&key, &partition.to_bytes());
    }

    let total = shadowmask_progress_total(lights.len(), &shared);
    assert_ne!(total, 0, "fixture must expose chart work");
    let cold_progress = StageProgress::indeterminate();
    let cold_control = BakeControl::new(Arc::new(Governor::new(1, false)), &cold_progress);
    let (cold, resident_high_water) = bake_shadowmask_atlas_cached_with_test_window(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geo,
        DENSITY,
        AREA_SAMPLES,
        Some(&cache),
        &cold_control,
        SHADOWMASK_RESIDENT_LAYER_WINDOW,
    );
    let cold = cold.expect("cached section miss rebuilt from streamed layers");

    assert_eq!(cold_progress.total(), Some(total));
    assert_eq!(cold_progress.completed(), total);
    assert!(
        resident_high_water <= SHADOWMASK_RESIDENT_LAYER_WINDOW,
        "streaming path must retain no more than its fixed layer window"
    );

    let warm_progress = StageProgress::indeterminate();
    let warm_control = BakeControl::new(Arc::new(Governor::new(1, false)), &warm_progress);
    let warm = bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geo,
        DENSITY,
        AREA_SAMPLES,
        Some(&cache),
        &warm_control,
    )
    .expect("whole shadowmask memo hit");

    assert_eq!(warm, cold);
    assert_eq!(warm_progress.total(), Some(total));
    assert_eq!(warm_progress.completed(), total);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mixed_layer_cache_hits_and_misses_complete_shadowmask_progress() {
    let mut lights = vec![light(5.0), light(4.0), light(3.0)];
    for (index, test_light) in lights.iter_mut().enumerate() {
        test_light.origin = DVec3::new(0.25 + index as f64 * 0.1, 1.0, 0.25);
    }
    let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
    let shared = shared_from_prepared(&prepared);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0, 1, 2],
    };
    let (first_layer_key, _) = layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
    let dir = fresh_cache_dir("mixed_layer_cache_progress");
    let cache = StageCache::new(&dir).expect("cache dir");
    let first_partition =
        cached_partition_with_visibility(&lights[0], &shared, &bvh, &primitives, &geo, 0.5);
    cache.put(&first_layer_key, &first_partition.to_bytes());

    let total = shadowmask_progress_total(selection.light_indices.len(), &shared);
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(2, false)), &progress);
    let (section, resident_high_water) = bake_shadowmask_atlas_cached_with_test_window(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geo,
        DENSITY,
        AREA_SAMPLES,
        Some(&cache),
        &control,
        2,
    );

    assert!(section.is_some());
    assert_eq!(progress.total(), Some(total));
    assert_eq!(progress.completed(), total);
    assert!(resident_high_water <= 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shadowmask_atlas_layer_cache_rejects_wrong_atlas_metadata() {
    assert_cached_layer_rejected_and_rebaked("wrong_layer_metadata", BadCachedLayer::Metadata);
}

#[test]
fn shadowmask_atlas_layer_cache_rejects_out_of_bounds_texels() {
    assert_cached_layer_rejected_and_rebaked(
        "out_of_bounds_layer_texel",
        BadCachedLayer::TexelBounds,
    );
}

#[test]
fn shadowmask_atlas_layer_cache_rejects_outside_covered_set() {
    assert_cached_layer_rejected_and_rebaked(
        "outside_covered_set",
        BadCachedLayer::OutsideCoveredSet,
    );
}

#[test]
fn shadowmask_atlas_layer_cache_rejects_duplicate_texel() {
    assert_cached_layer_rejected_and_rebaked(
        "duplicate_layer_texel",
        BadCachedLayer::DuplicateTexel,
    );
}

#[test]
fn shadowmask_atlas_cache_miss_bakes_and_stores_missing_layer() {
    let mut test_light = light(5.0);
    test_light.origin = DVec3::new(0.25, 1.0, 0.25);
    let lights = vec![test_light];
    let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
    let shared = shared_from_prepared(&prepared);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let (layer_key, input_hash) = layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
    let section_key = shadowmask_key(&selection, &shared, &[input_hash]);

    let dir = fresh_cache_dir("missing_layer_store");
    let cache = StageCache::new(&dir).expect("cache dir");

    let result = bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geo,
        DENSITY,
        AREA_SAMPLES,
        Some(&cache),
        &test_control(),
    )
    .expect("rebuilt section");

    assert!(
        cache
            .get(&layer_key)
            .and_then(|bytes| LightmapLayer::from_bytes(&bytes))
            .is_some(),
        "missing selected lightmap_layer must be baked and stored"
    );
    let stored = cache.get(&section_key).expect("shadowmask section stored");
    assert_eq!(memo_section(&stored, &shared), result);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corrupt_shadowmask_atlas_cache_entry_is_overwritten_from_layers() {
    let mut test_light = light(5.0);
    test_light.origin = DVec3::new(0.25, 1.0, 0.25);
    let lights = vec![test_light];
    let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
    let shared = shared_from_prepared(&prepared);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let (layer_key, input_hash) = layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
    let section_key = shadowmask_key(&selection, &shared, &[input_hash]);
    let selected = selected_refs(&selection, &alpha_lights);
    let seeded_layer =
        cached_partition_with_visibility(&lights[0], &shared, &bvh, &primitives, &geo, 0.75);
    let expected = bake_shadowmask_atlas_from_layers(
        &selection,
        shared.atlas_width,
        shared.atlas_height,
        layer_count_from_shared(&shared),
        &shared.layout.blocks,
        &selected,
        std::slice::from_ref(&seeded_layer),
    )
    .unwrap();

    let dir = fresh_cache_dir("corrupt_section");
    let cache = StageCache::new(&dir).expect("cache dir");
    cache.put(&section_key, b"not a shadowmask section");
    cache.put(&layer_key, &seeded_layer.to_bytes());

    let result = bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geo,
        DENSITY,
        AREA_SAMPLES,
        Some(&cache),
        &test_control(),
    )
    .expect("rebuilt section");

    assert_eq!(result, expected);
    let overwritten = cache
        .get(&section_key)
        .expect("corrupt section entry overwritten");
    assert_eq!(memo_section(&overwritten, &shared), expected);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shadowmask_atlas_no_cache_ignores_and_does_not_overwrite_entry() {
    let mut test_light = light(5.0);
    test_light.origin = DVec3::new(0.25, 1.0, 0.25);
    let lights = vec![test_light];
    let (geo, prepared, bvh, primitives) = cache_fixture(&lights);
    let shared = shared_from_prepared(&prepared);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let (_, input_hash) = layer_key(&lights[0], &shared, &primitives, &geo, AREA_SAMPLES);
    let section_key = shadowmask_key(&selection, &shared, &[input_hash]);
    let cached = section_for_blocks(&shared.layout.blocks, vec![3], 0);

    let dir = fresh_cache_dir("no_cache");
    let cache = StageCache::new(&dir).expect("cache dir");
    cache.put(&section_key, &memo_entry(&cached, 0));

    let result = bake_shadowmask_atlas_cached(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geo,
        DENSITY,
        AREA_SAMPLES,
        None,
        &test_control(),
    )
    .expect("uncached section");
    let uncached = bake_shadowmask_atlas(
        Some(&selection),
        &alpha_lights,
        &shared,
        &bvh,
        &primitives,
        &geo,
        AREA_SAMPLES,
        &test_control(),
    )
    .expect("direct uncached section");

    assert_eq!(
        result, uncached,
        "stage_cache == None must delegate to the current recompute path"
    );
    assert_ne!(
        result, cached,
        "stage_cache == None must ignore an existing shadowmask_atlas entry"
    );
    let still_cached = cache.get(&section_key).expect("seeded cache entry remains");
    assert_eq!(
        memo_section(&still_cached, &shared),
        cached,
        "stage_cache == None must not overwrite shadowmask_atlas entries"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shadowmask_atlas_input_hash_is_membership_and_order_sensitive() {
    let selection = EntityShadowLightsSection {
        light_indices: vec![1, 2],
    };
    let hashes = vec![fake_layer_hash(10), fake_layer_hash(20)];
    let base = shadowmask_atlas_input_hash(&selection, &hashes, 4, 8, 1);

    let reordered_selection = EntityShadowLightsSection {
        light_indices: vec![2, 1],
    };
    let reordered_hashes = vec![fake_layer_hash(20), fake_layer_hash(10)];
    assert_ne!(
        base,
        shadowmask_atlas_input_hash(&reordered_selection, &reordered_hashes, 4, 8, 1),
        "selection order must affect the section cache key"
    );

    let changed_membership = EntityShadowLightsSection {
        light_indices: vec![1, 3],
    };
    assert_ne!(
        base,
        shadowmask_atlas_input_hash(&changed_membership, &hashes, 4, 8, 1),
        "selection membership must affect the section cache key"
    );
}

#[test]
fn shadowmask_atlas_input_hash_includes_atlas_dimensions_and_layer_count() {
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let hashes = vec![fake_layer_hash(7)];
    let base = shadowmask_atlas_input_hash(&selection, &hashes, 4, 8, 1);

    assert_ne!(
        base,
        shadowmask_atlas_input_hash(&selection, &hashes, 5, 8, 1)
    );
    assert_ne!(
        base,
        shadowmask_atlas_input_hash(&selection, &hashes, 4, 9, 1)
    );
    assert_ne!(
        base,
        shadowmask_atlas_input_hash(&selection, &[hashes[0], fake_layer_hash(8)], 4, 8, 2)
    );
}

#[test]
fn shadowmask_atlas_input_hash_includes_selected_layer_hashes() {
    let selection = EntityShadowLightsSection {
        light_indices: vec![0, 1],
    };
    let base_hashes = vec![fake_layer_hash(1), fake_layer_hash(2)];
    let changed_hashes = vec![fake_layer_hash(1), fake_layer_hash(3)];

    assert_ne!(
        shadowmask_atlas_input_hash(&selection, &base_hashes, 4, 8, 1),
        shadowmask_atlas_input_hash(&selection, &changed_hashes, 4, 8, 1),
        "changing a selected lightmap layer input hash must affect the section cache key"
    );
}

#[test]
fn soft_shadow_samples_change_selected_layer_and_shadowmask_keys() {
    let mut test_light = light(5.0);
    test_light.origin = DVec3::new(0.25, 1.0, 0.25);
    let lights = vec![test_light];
    let (geo, prepared, _, primitives) = cache_fixture(&lights);
    let shared = shared_from_prepared(&prepared);
    let selection = EntityShadowLightsSection {
        light_indices: vec![0],
    };

    let layer_hash_4 =
        lightmap_layer::layer_input_hash(&lights[0], &shared, &primitives, &geo, DENSITY, 4, 0);
    let layer_hash_8 =
        lightmap_layer::layer_input_hash(&lights[0], &shared, &primitives, &geo, DENSITY, 8, 0);
    assert_ne!(
        layer_hash_4, layer_hash_8,
        "--soft-shadow-samples must affect the selected layer input hash"
    );

    assert_ne!(
        shadowmask_atlas_input_hash(
            &selection,
            &[layer_hash_4],
            shared.atlas_width,
            shared.atlas_height,
            layer_count_from_shared(&shared),
        ),
        shadowmask_atlas_input_hash(
            &selection,
            &[layer_hash_8],
            shared.atlas_width,
            shared.atlas_height,
            layer_count_from_shared(&shared),
        ),
        "--soft-shadow-samples must flow through to the shadowmask_atlas key"
    );
}
