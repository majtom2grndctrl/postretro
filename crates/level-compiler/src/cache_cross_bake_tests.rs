//! Cross-stage cache contracts.
//!
//! The individual bakers test their local algorithms. This suite deliberately
//! drives their public controlled entry points through one disk-backed cache so
//! cache identity, CSR locality, and post-bake policy ordering are checked at
//! the compiler seam where regressions otherwise cross module boundaries.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use glam::DVec3;
use log::Level;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::cell_visibility::CellVisibilitySection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::sh_volume::{OctahedralShProbe, OctahedralShVolumeSection};
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_test_log_capture::LogCapture;

use crate::animated_direct_sh_bake::{
    ANIMATED_DIRECT_DELTA_SH_STAGE_ID, ANIMATED_DIRECT_DELTA_SH_STAGE_VERSION,
    AnimatedDirectShBakeInputs, bake_animated_direct_sh_delta_volumes_controlled_with_tally,
};
use crate::bake_control::BakeControl;
use crate::bvh_build::build_bvh;
use crate::cache::StageCache;
use crate::cell_visibility_bake::{
    CELL_VISIBILITY_STAGE_VERSION, cell_visibility_bake_cached, cell_visibility_cache_key,
};
use crate::chunk_light_list_bake::{
    CHUNK_LIGHT_LIST_STAGE_VERSION, ChunkLightListInputs, bake_chunk_light_list_cached,
    chunk_light_list_cache_key, chunk_light_list_cache_key_with_version,
};
use crate::delta_drop_policy::ScriptMutableDescriptorSlots;
use crate::delta_sections::{DeltaSectionConfig, PostBakeDeltaSections};
use crate::delta_sh_bake::{
    DeltaBakeInputs, INDIRECT_DELTA_SH_STAGE_ID, INDIRECT_DELTA_SH_STAGE_VERSION,
    bake_delta_sh_volumes_controlled_with_tally,
};
use crate::delta_sh_cache::{DeltaShEntryKeyInputs, delta_sh_entry_cache_key};
use crate::direct_sh_bake::{
    DIRECT_SH_DELTA_STAGE_ID, DIRECT_SH_DELTA_STAGE_VERSION, DirectBakeInputs,
    bake_direct_sh_delta_volumes_controlled_with_tally,
};
use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::governor::Governor;
use crate::light_namespaces::{AlphaLightsNs, AnimatedBakedLights, StaticBakedLights};
use crate::map_data::{FalloffModel, LightAnimation, LightType, MapLight, ShadowType};
use crate::partition::{Aabb, BspLeaf, BspTree};
use crate::portals::Portal;
use crate::reporter::StageProgress;
use crate::sh_bake::{ShBakeCtx, ShConfig};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn fresh_cache(label: &str) -> (PathBuf, StageCache) {
    let dir = std::env::temp_dir().join(format!(
        "postretro_cross_bake_{label}_{}_{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed),
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let cache = StageCache::new(&dir).expect("create cross-bake cache");
    (dir, cache)
}

fn vertex(position: [f32; 3]) -> Vertex {
    Vertex::new(
        position,
        [0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    )
}

fn cube_geometry_with_extent(extent: f32) -> GeometryResult {
    let corners = [
        [-extent, -extent, -extent],
        [extent, -extent, -extent],
        [extent, extent, -extent],
        [-extent, extent, -extent],
        [-extent, -extent, extent],
        [extent, -extent, extent],
        [extent, extent, extent],
        [-extent, extent, extent],
    ];
    GeometryResult {
        geometry: GeometrySection {
            vertices: corners.map(vertex).to_vec(),
            indices: vec![0, 1, 2],
            faces: vec![FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            }],
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: vec![FaceIndexRange {
            index_offset: 0,
            index_count: 3,
        }],
    }
}

fn cube_geometry() -> GeometryResult {
    cube_geometry_with_extent(2.0)
}

fn empty_tree() -> BspTree {
    BspTree {
        nodes: Vec::new(),
        leaves: vec![BspLeaf {
            face_indices: Vec::new(),
            bounds: Aabb {
                min: DVec3::splat(-100.0),
                max: DVec3::splat(100.0),
            },
            is_solid: false,
            defining_planes: Vec::new(),
        }],
    }
}

fn tree_with_leaves(centres: &[DVec3]) -> BspTree {
    BspTree {
        nodes: Vec::new(),
        leaves: centres
            .iter()
            .map(|&centre| BspLeaf {
                face_indices: Vec::new(),
                bounds: Aabb {
                    min: centre - DVec3::splat(0.5),
                    max: centre + DVec3::splat(0.5),
                },
                is_solid: false,
                defining_planes: Vec::new(),
            })
            .collect(),
    }
}

fn portal(front_leaf: usize, back_leaf: usize, centre: DVec3, width: f64) -> Portal {
    let half = width * 0.5;
    Portal {
        polygon: vec![
            centre + DVec3::new(-half, -half, 0.0),
            centre + DVec3::new(half, -half, 0.0),
            centre + DVec3::new(half, half, 0.0),
            centre + DVec3::new(-half, half, 0.0),
        ],
        front_leaf,
        back_leaf,
    }
}

fn light(origin: DVec3, animated: bool) -> MapLight {
    MapLight {
        origin,
        light_type: LightType::Point,
        carrier: String::new(),
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 3.0,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: animated.then(|| LightAnimation {
            period: 1.0,
            phase: 0.0,
            brightness: Some(vec![0.0, 1.0]),
            color: None,
            direction: None,
            start_active: true,
        }),
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: Vec::new(),
        shadow_type: ShadowType::StaticLightMap,
    }
}

fn delta_lights() -> Vec<MapLight> {
    vec![
        light(DVec3::new(-0.5, 0.0, 0.0), false),
        light(DVec3::new(0.5, 0.0, 0.0), false),
        light(DVec3::new(-0.5, 0.0, 0.0), true),
        light(DVec3::new(0.5, 0.0, 0.0), true),
    ]
}

#[derive(Debug)]
struct DeltaSnapshot {
    indirect: DeltaShVolumesSection,
    direct: DirectShDeltaVolumesSection,
    animated_direct: AnimatedDirectShDeltaVolumesSection,
    indirect_tally: crate::delta_sh_cache::DeltaShCacheTally,
    direct_tally: crate::delta_sh_cache::DeltaShCacheTally,
    animated_direct_tally: crate::delta_sh_cache::DeltaShCacheTally,
    direct_stats: crate::direct_sh_bake::DirectDeltaBakeStats,
}

fn control() -> BakeControl {
    BakeControl::unrestricted()
}

fn bake_deltas(lights: &[MapLight], cache: Option<&StageCache>) -> DeltaSnapshot {
    let indirect_control = control();
    let direct_control = control();
    let animated_control = control();
    bake_deltas_with_controls(
        lights,
        cache,
        &indirect_control,
        &direct_control,
        &animated_control,
    )
}

fn bake_deltas_with_controls(
    lights: &[MapLight],
    cache: Option<&StageCache>,
    indirect_control: &BakeControl,
    direct_control: &BakeControl,
    animated_control: &BakeControl,
) -> DeltaSnapshot {
    let geometry = cube_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).expect("build delta test BVH");
    let tree = empty_tree();
    let exterior = HashSet::new();
    let static_lights = StaticBakedLights::from_lights(lights);
    let animated_lights = AnimatedBakedLights::from_lights(lights);
    let alpha_lights = AlphaLightsNs::from_lights(lights);
    let sh_ctx = ShBakeCtx {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: lights.len(),
    };
    let config = ShConfig { probe_spacing: 1.0 };
    let indirect_inputs = DeltaBakeInputs {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
        tree: &tree,
        exterior_leaves: &exterior,
        portals: &[],
        animated_lights: &animated_lights,
    };
    let animated_inputs = AnimatedDirectShBakeInputs {
        sh_ctx: &sh_ctx,
        portals: &[],
        animated_lights: &animated_lights,
    };
    let direct_inputs = DirectBakeInputs {
        sh_ctx: &sh_ctx,
        portals: &[],
    };
    let selected = EntityShadowLightsSection {
        light_indices: vec![0, 1],
    };
    let (indirect, indirect_tally) = bake_delta_sh_volumes_controlled_with_tally(
        &indirect_inputs,
        &config,
        cache,
        indirect_control,
    );
    let (animated_direct, animated_direct_tally) =
        bake_animated_direct_sh_delta_volumes_controlled_with_tally(
            &animated_inputs,
            &config,
            cache,
            animated_control,
        );
    let (direct, direct_tally) = bake_direct_sh_delta_volumes_controlled_with_tally(
        &direct_inputs,
        &config,
        &alpha_lights,
        &selected,
        cache,
        direct_control,
    );
    let (direct, direct_stats) = direct.expect("selected static lights produce delta entries");
    DeltaSnapshot {
        indirect: indirect.expect("animated lights produce indirect delta entries"),
        direct,
        animated_direct: animated_direct.expect("animated lights produce direct delta entries"),
        indirect_tally,
        direct_tally,
        animated_direct_tally,
        direct_stats,
    }
}

fn bake_animated_delta_pair(
    lights: &[MapLight],
    geometry: &GeometryResult,
    cache: Option<&StageCache>,
) -> (
    DeltaShVolumesSection,
    crate::delta_sh_cache::DeltaShCacheTally,
    AnimatedDirectShDeltaVolumesSection,
    crate::delta_sh_cache::DeltaShCacheTally,
) {
    let (bvh, primitives, _) = build_bvh(geometry).expect("build paired delta test BVH");
    let tree = empty_tree();
    let exterior = HashSet::new();
    let static_lights = StaticBakedLights::from_lights(lights);
    let animated_lights = AnimatedBakedLights::from_lights(lights);
    let sh_ctx = ShBakeCtx {
        bvh: &bvh,
        primitives: &primitives,
        geometry,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: lights.len(),
    };
    let indirect_inputs = DeltaBakeInputs {
        bvh: &bvh,
        primitives: &primitives,
        geometry,
        tree: &tree,
        exterior_leaves: &exterior,
        portals: &[],
        animated_lights: &animated_lights,
    };
    let animated_inputs = AnimatedDirectShBakeInputs {
        sh_ctx: &sh_ctx,
        portals: &[],
        animated_lights: &animated_lights,
    };
    let config = ShConfig { probe_spacing: 1.0 };
    let (indirect, indirect_tally) =
        bake_delta_sh_volumes_controlled_with_tally(&indirect_inputs, &config, cache, &control());
    let (animated_direct, animated_direct_tally) =
        bake_animated_direct_sh_delta_volumes_controlled_with_tally(
            &animated_inputs,
            &config,
            cache,
            &control(),
        );
    (
        indirect.expect("animated fixture produces indirect entries"),
        indirect_tally,
        animated_direct.expect("animated fixture produces direct entries"),
        animated_direct_tally,
    )
}

fn bake_direct_delta_only(
    lights: &[MapLight],
    selected: &EntityShadowLightsSection,
    cache: Option<&StageCache>,
    control: &BakeControl,
) -> (
    Option<(
        DirectShDeltaVolumesSection,
        crate::direct_sh_bake::DirectDeltaBakeStats,
    )>,
    crate::delta_sh_cache::DeltaShCacheTally,
) {
    let geometry = cube_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).expect("build direct-delta test BVH");
    let tree = empty_tree();
    let exterior = HashSet::new();
    let static_lights = StaticBakedLights::from_lights(lights);
    let animated_lights = AnimatedBakedLights::from_lights(lights);
    let alpha_lights = AlphaLightsNs::from_lights(lights);
    let sh_ctx = ShBakeCtx {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: lights.len(),
    };
    bake_direct_sh_delta_volumes_controlled_with_tally(
        &DirectBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
        },
        &ShConfig { probe_spacing: 1.0 },
        &alpha_lights,
        selected,
        cache,
        control,
    )
}

fn base_for(section: &DeltaShVolumesSection) -> OctahedralShVolumeSection {
    let mut base = OctahedralShVolumeSection::placeholder();
    base.grid_dimensions = section.affinity_dims.map(|dimension| dimension * 4);
    base.probes = vec![
        OctahedralShProbe {
            validity: 1,
            ..OctahedralShProbe::default()
        };
        base.grid_dimensions.iter().product::<u32>() as usize
    ];
    base
}

fn finalized_delta_bytes(snapshot: DeltaSnapshot) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let base = base_for(&snapshot.indirect);
    let mut sections = PostBakeDeltaSections::new(
        DeltaSectionConfig::default(),
        Some(snapshot.indirect),
        Some(EntityShadowLightsSection {
            light_indices: vec![0, 1],
        }),
        Some(snapshot.direct),
        Some(snapshot.animated_direct),
    );
    sections
        .apply_exact_zero_drop_policy(&ScriptMutableDescriptorSlots::empty(2))
        .expect("drop policy");
    sections.enforce_id41_only_coarsening_policy();
    sections
        .apply_valid_probe_compaction(&base)
        .expect("valid-probe compaction");
    sections.enforce_payload_cap().expect("payload cap");
    (
        sections.indirect.expect("indirect section").to_bytes(),
        sections.direct.expect("direct section").to_bytes(),
        sections
            .animated_direct
            .expect("animated direct section")
            .try_to_bytes()
            .expect("fallible animated-direct codec"),
    )
}

fn corrupt_cache_entries(dir: &PathBuf) {
    for entry in std::fs::read_dir(dir).expect("read cache dir") {
        let path = entry.expect("cache dir entry").path();
        if path.is_file() {
            std::fs::write(path, b"corrupt cross-bake cache entry").expect("corrupt cache entry");
        }
    }
}

#[test]
fn p1_p2_cell_visibility_portal_edit_misses_while_light_only_edit_hits() {
    let tree = tree_with_leaves(&[DVec3::ZERO, DVec3::new(2.0, 0.0, 0.0)]);
    let portals = vec![portal(0, 1, DVec3::X, 2.0)];
    let (dir, cache) = fresh_cache("cell_visibility_pins");
    let uncached = cell_visibility_bake_cached(&tree, &portals, None, &control())
        .expect("uncached CellVisibility bake");
    let baseline = cell_visibility_bake_cached(&tree, &portals, Some(&cache), &control())
        .expect("initial CellVisibility bake");
    assert_eq!(
        baseline, uncached,
        "CellVisibility warm output matches --no-cache"
    );
    // P2: no light parameter appears in the structural-only API/key, so the
    // same stage entry is hit after a light-only authoring edit.
    let light_only = cell_visibility_bake_cached(&tree, &portals, Some(&cache), &control())
        .expect("light-only CellVisibility hit");
    assert_eq!(baseline, light_only);
    cache.put(
        &cell_visibility_cache_key(&tree, &portals, CELL_VISIBILITY_STAGE_VERSION),
        b"invalid CellVisibility section",
    );
    assert_eq!(
        cell_visibility_bake_cached(&tree, &portals, Some(&cache), &control())
            .expect("CellVisibility corruption is a soft miss"),
        baseline
    );

    // P1: leaf count and portal endpoints stay fixed while the portal polygon
    // changes. Render geometry is not an input to either bake call.
    let narrow = vec![portal(0, 1, DVec3::X, 0.25)];
    let baseline_key = cell_visibility_cache_key(&tree, &portals, CELL_VISIBILITY_STAGE_VERSION);
    let narrow_key = cell_visibility_cache_key(&tree, &narrow, CELL_VISIBILITY_STAGE_VERSION);
    assert_ne!(baseline_key.as_filename(), narrow_key.as_filename());
    let rebaked = cell_visibility_bake_cached(&tree, &narrow, Some(&cache), &control())
        .expect("structural CellVisibility miss");
    let baseline_section =
        CellVisibilitySection::from_bytes(&baseline, 2).expect("decode baseline");
    let rebaked_section = CellVisibilitySection::from_bytes(&rebaked, 2).expect("decode rebake");
    assert_ne!(
        baseline_section.coupled_pairs,
        rebaked_section.coupled_pairs
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn p10_cell_visibility_one_leaf_aperture_edit_creates_a_distinct_cache_entry() {
    let tree = tree_with_leaves(&[DVec3::ZERO]);
    // A portal cannot connect two distinct cells in a one-leaf grid. A
    // self-loop is the smallest structurally accepted Portal value and leaves
    // the one-cell baked relation unchanged. That makes cache identity, rather
    // than changed output, the observable proof that aperture is folded.
    let wide = vec![portal(0, 0, DVec3::ZERO, 1.0)];
    let narrow = vec![portal(0, 0, DVec3::ZERO, 0.25)];
    let wide_key = cell_visibility_cache_key(&tree, &wide, CELL_VISIBILITY_STAGE_VERSION);
    let narrow_key = cell_visibility_cache_key(&tree, &narrow, CELL_VISIBILITY_STAGE_VERSION);
    assert_ne!(wide_key.as_filename(), narrow_key.as_filename());

    let (dir, cache) = fresh_cache("cell_visibility_one_leaf_aperture");
    let wide_bytes = cell_visibility_bake_cached(&tree, &wide, Some(&cache), &control())
        .expect("cache one-leaf wide aperture");
    assert!(cache.get(&wide_key).is_some());
    assert_eq!(cache.get(&narrow_key), None, "changed aperture must miss");

    let narrow_bytes = cell_visibility_bake_cached(&tree, &narrow, Some(&cache), &control())
        .expect("rebake one-leaf narrow aperture");
    assert!(
        cache.get(&narrow_key).is_some(),
        "miss path must write its entry"
    );
    assert_eq!(
        wide_bytes, narrow_bytes,
        "one cell has no distinct coupled pair regardless of self-loop aperture"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn p4_p5_p9_chunk_light_dynamic_edits_and_empty_static_cache_contract() {
    let geometry = cube_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).expect("chunk test BVH");
    let tree = empty_tree();
    let exterior = HashSet::new();
    let static_light = light(DVec3::ZERO, false);
    let dynamic = MapLight {
        is_dynamic: true,
        ..light(DVec3::new(10.0, 0.0, 0.0), false)
    };
    let base_lights = vec![static_light.clone(), dynamic.clone()];
    let inserted_lights = vec![
        MapLight {
            is_dynamic: true,
            ..light(DVec3::new(-10.0, 0.0, 0.0), false)
        },
        static_light,
        dynamic.clone(),
    ];
    let mut dynamic_edit = dynamic;
    dynamic_edit.intensity = 3.0;
    let edited_lights = vec![base_lights[0].clone(), dynamic_edit];
    let base_ns = AlphaLightsNs::from_lights(&base_lights);
    let inserted_ns = AlphaLightsNs::from_lights(&inserted_lights);
    let edited_ns = AlphaLightsNs::from_lights(&edited_lights);
    let inputs = |lights| ChunkLightListInputs {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
        lights,
        tree: &tree,
        portals: &[],
        exterior_leaves: &exterior,
    };
    let base_inputs = inputs(&base_ns);
    let inserted_inputs = inputs(&inserted_ns);
    let edited_inputs = inputs(&edited_ns);
    let (dir, cache) = fresh_cache("chunk_pins");
    let uncached = bake_chunk_light_list_cached(&base_inputs, 8.0, 64, None)
        .expect("uncached chunk-light bake")
        .to_bytes();
    let first = bake_chunk_light_list_cached(&base_inputs, 8.0, 64, Some(&cache))
        .expect("base chunk-light bake")
        .to_bytes();
    assert_eq!(
        first, uncached,
        "ChunkLightList warm output matches --no-cache"
    );
    assert_eq!(
        chunk_light_list_cache_key(&base_inputs, 8.0, 64).as_filename(),
        chunk_light_list_cache_key(&inserted_inputs, 8.0, 64).as_filename(),
        "P4: dynamic insertion cannot perturb compacted static slots"
    );
    assert_eq!(
        first,
        bake_chunk_light_list_cached(&inserted_inputs, 8.0, 64, Some(&cache))
            .expect("P4 cache hit")
            .to_bytes()
    );
    assert_eq!(
        chunk_light_list_cache_key(&base_inputs, 8.0, 64).as_filename(),
        chunk_light_list_cache_key(&edited_inputs, 8.0, 64).as_filename(),
        "P5: dynamic parameters cannot perturb static-only cache identity"
    );
    assert_eq!(
        first,
        bake_chunk_light_list_cached(&edited_inputs, 8.0, 64, Some(&cache))
            .expect("P5 cache hit")
            .to_bytes(),
        "P5 dynamic parameter edit must return the cached static-only payload"
    );

    let only_dynamic_lights = [MapLight {
        is_dynamic: true,
        ..light(DVec3::ZERO, false)
    }];
    let only_dynamic = AlphaLightsNs::from_lights(&only_dynamic_lights);
    let empty_inputs = inputs(&only_dynamic);
    let placeholder = bake_chunk_light_list_cached(&empty_inputs, 8.0, 64, Some(&cache))
        .expect("P9 empty static placeholder");
    assert_eq!(placeholder.has_grid, 0);
    assert_eq!(
        bake_chunk_light_list_cached(&empty_inputs, 8.0, 64, Some(&cache))
            .expect("P9 warm empty-static hit"),
        placeholder
    );
    let empty_key = chunk_light_list_cache_key(&empty_inputs, 8.0, 64);
    cache.put(&empty_key, b"invalid chunk list");
    assert_eq!(
        bake_chunk_light_list_cached(&empty_inputs, 8.0, 64, Some(&cache))
            .expect("P9 corrupt placeholder cache is a miss"),
        placeholder
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn p3_p6_p7_p8_p12_delta_cache_locality_progress_stats_and_policy_identity() {
    let lights = delta_lights();
    let (dir, cache) = fresh_cache("delta_pipeline_pins");
    let cold = bake_deltas(&lights, None);
    let first = bake_deltas(&lights, Some(&cache));
    let indirect_progress = StageProgress::indeterminate();
    let direct_progress = StageProgress::indeterminate();
    let animated_progress = StageProgress::indeterminate();
    let indirect_control = BakeControl::new(Arc::new(Governor::new(1, false)), &indirect_progress);
    let direct_control = BakeControl::new(Arc::new(Governor::new(1, false)), &direct_progress);
    let animated_control = BakeControl::new(Arc::new(Governor::new(1, false)), &animated_progress);
    let warm = bake_deltas_with_controls(
        &lights,
        Some(&cache),
        &indirect_control,
        &direct_control,
        &animated_control,
    );
    assert_eq!(cold.indirect.to_bytes(), first.indirect.to_bytes());
    assert_eq!(cold.direct.to_bytes(), first.direct.to_bytes());
    assert_eq!(
        cold.animated_direct
            .try_to_bytes()
            .expect("cold animated codec"),
        first
            .animated_direct
            .try_to_bytes()
            .expect("warm animated codec")
    );
    assert_eq!(
        first.direct_stats, cold.direct_stats,
        "P3 cache-miss stats match --no-cache stats"
    );
    assert_eq!(
        warm.direct_stats, cold.direct_stats,
        "P3 all-hit stats reconstruct from the raw cached section"
    );
    assert_eq!(warm.indirect_tally.misses, 0);
    assert_eq!(warm.direct_tally.misses, 0);
    assert_eq!(warm.animated_direct_tally.misses, 0);
    assert_eq!(
        indirect_progress.total(),
        Some(warm.indirect.affinity_lights.len())
    );
    assert_eq!(
        indirect_progress.completed(),
        warm.indirect.affinity_lights.len()
    );
    assert_eq!(
        direct_progress.total(),
        Some(warm.direct.affinity_lights.len())
    );
    assert_eq!(
        direct_progress.completed(),
        warm.direct.affinity_lights.len()
    );
    assert_eq!(
        animated_progress.total(),
        Some(warm.animated_direct.affinity_lights.len())
    );
    assert_eq!(
        animated_progress.completed(),
        warm.animated_direct.affinity_lights.len()
    );

    let mut animated_edit = lights.clone();
    animated_edit[2].color = [0.2, 0.4, 0.6];
    animated_edit[2].intensity = 7.0;
    let recolored = bake_deltas(&animated_edit, Some(&cache));
    assert_eq!(
        recolored.indirect_tally.misses, 0,
        "P14 unitized indirect hit"
    );
    assert_eq!(
        recolored.animated_direct_tally.misses, 0,
        "P14 unitized animated-direct hit"
    );

    let mut animated_transport_edit = lights.clone();
    animated_transport_edit[2].origin.x -= 0.25;
    let animated_partial = bake_deltas(&animated_transport_edit, Some(&cache));
    let animated_partial_cold = bake_deltas(&animated_transport_edit, None);
    let changed_animated_entries = cold
        .indirect
        .affinity_lights
        .iter()
        .filter(|&&animated_slot| animated_slot == 0)
        .count();
    assert_eq!(
        animated_partial.indirect_tally.misses,
        changed_animated_entries
    );
    let changed_animated_direct_entries = cold
        .animated_direct
        .affinity_lights
        .iter()
        .filter(|&&animated_slot| animated_slot == 0)
        .count();
    assert_eq!(
        animated_partial.animated_direct_tally.misses,
        changed_animated_direct_entries
    );
    assert_eq!(
        animated_partial.indirect_tally.hits + animated_partial.indirect_tally.misses,
        animated_partial.indirect.affinity_lights.len()
    );
    assert_eq!(
        animated_partial.animated_direct_tally.hits + animated_partial.animated_direct_tally.misses,
        animated_partial.animated_direct.affinity_lights.len()
    );
    assert_eq!(
        animated_partial.indirect.to_bytes(),
        animated_partial_cold.indirect.to_bytes(),
        "P7 edited indirect pre-drop bytes match --no-cache"
    );
    assert_eq!(
        animated_partial
            .animated_direct
            .try_to_bytes()
            .expect("partial warm animated-direct codec"),
        animated_partial_cold
            .animated_direct
            .try_to_bytes()
            .expect("partial cold animated-direct codec"),
        "P7 edited animated-direct pre-drop bytes match --no-cache"
    );
    assert_eq!(
        finalized_delta_bytes(animated_partial),
        finalized_delta_bytes(animated_partial_cold),
        "P7 animated-light partial hit stays byte-identical through the downstream pipeline"
    );

    let mut static_edit = lights.clone();
    static_edit[0].intensity = 2.0;
    let partial = bake_deltas(&static_edit, Some(&cache));
    let partial_cold = bake_deltas(&static_edit, None);
    let changed_direct_entries = cold
        .direct
        .affinity_lights
        .iter()
        .filter(|&&selection_slot| selection_slot == 0)
        .count();
    assert_eq!(
        partial.direct_tally.misses, changed_direct_entries,
        "P7 direct locality"
    );
    assert_eq!(
        partial.direct_tally.hits + partial.direct_tally.misses,
        cold.direct.affinity_lights.len()
    );
    assert_eq!(
        partial.direct.to_bytes(),
        partial_cold.direct.to_bytes(),
        "P7 static-light partial warm direct bytes match --no-cache"
    );
    assert_eq!(
        finalized_delta_bytes(partial),
        finalized_delta_bytes(partial_cold),
        "P7 static-light partial hit stays byte-identical through the downstream pipeline"
    );
    assert_eq!(
        finalized_delta_bytes(cold),
        finalized_delta_bytes(bake_deltas(&lights, Some(&cache))),
        "P12 exact-zero drop, coarsening, compaction, and cap remain downstream of raw cache"
    );

    // P8: an empty animated namespace returns no section and performs no entry work.
    let static_only = vec![light(DVec3::ZERO, false)];
    let empty = bake_deltas_empty_animated(&static_only, Some(&cache));
    assert_eq!(empty.0, None);
    assert_eq!(empty.1.hits + empty.1.misses, 0);
    let _ = std::fs::remove_dir_all(dir);
}

fn bake_deltas_empty_animated(
    lights: &[MapLight],
    cache: Option<&StageCache>,
) -> (
    Option<DeltaShVolumesSection>,
    crate::delta_sh_cache::DeltaShCacheTally,
) {
    let geometry = cube_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).expect("empty animated BVH");
    let tree = empty_tree();
    let exterior = HashSet::new();
    let animated_lights = AnimatedBakedLights::from_lights(lights);
    let inputs = DeltaBakeInputs {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
        tree: &tree,
        exterior_leaves: &exterior,
        portals: &[],
        animated_lights: &animated_lights,
    };
    bake_delta_sh_volumes_controlled_with_tally(
        &inputs,
        &ShConfig { probe_spacing: 1.0 },
        cache,
        &control(),
    )
}

#[test]
fn p11_direct_stats_observe_raw_zero_entries_before_selection_retention_drop() {
    let mut zero_light = light(DVec3::ZERO, false);
    zero_light.intensity = 0.0;
    let lights = vec![zero_light];
    let selected = EntityShadowLightsSection {
        light_indices: vec![0],
    };
    let (dir, cache) = fresh_cache("direct_zero_stats");
    let (first, _) = bake_direct_delta_only(&lights, &selected, Some(&cache), &control());
    let (warm, warm_tally) = bake_direct_delta_only(&lights, &selected, Some(&cache), &control());
    let (raw, stats) = warm.expect("zero-radiance selected light still has reached cells");
    let (_, first_stats) = first.expect("initial zero-radiance direct bake");

    assert_eq!(warm_tally.misses, 0, "zero payloads must be cacheable");
    assert_eq!(
        stats, first_stats,
        "warm stats reconstruct from raw cached CSR"
    );
    assert!(
        raw.affinity_lights.len() > 1,
        "fixture needs entries to drop"
    );
    assert!(
        raw.delta_subblocks
            .chunks_exact(4)
            .all(|rgba| rgba[..3].iter().all(|value| *value & 0x7fff == 0)),
        "zero authored intensity must produce zero RGB direct deltas"
    );
    let raw_entry_count = raw.affinity_lights.len();
    let raw_bytes = raw.delta_subblocks.len() * std::mem::size_of::<u16>();
    assert_eq!(stats.rows.len(), 1);
    assert_eq!(stats.rows[0].csr_entry_count, raw_entry_count);
    assert_eq!(stats.rows[0].byte_total, raw_bytes);
    assert_eq!(stats.total_bytes, raw_bytes);

    let mut sections = PostBakeDeltaSections::new(
        DeltaSectionConfig::default(),
        None,
        Some(selected),
        Some(raw),
        None,
    );
    sections
        .apply_exact_zero_drop_policy(&ScriptMutableDescriptorSlots::empty(0))
        .expect("direct zero drop");
    let retained = sections.direct.expect("selected direct section retained");
    assert_eq!(retained.affinity_lights, vec![0]);
    assert!(raw_entry_count > retained.affinity_lights.len());

    let capture = LogCapture::start();
    crate::pipeline::log_direct_sh_delta_stats_for_test(Some(&stats), false);
    capture.assert_logged_once(Level::Info, "DirectShDeltaVolumes:");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn p13_animated_direct_source_index_shift_misses_seeded_entries() {
    let lights = delta_lights();
    let (dir, cache) = fresh_cache("animated_direct_source_index");
    bake_deltas(&lights, Some(&cache));
    let mut shifted = lights.clone();
    shifted.insert(0, light(DVec3::new(20.0, 0.0, 0.0), false));
    let shifted_result = bake_deltas(&shifted, Some(&cache));
    assert!(
        shifted_result.animated_direct_tally.misses > 0,
        "P13 source-index seed shifts must miss animated-direct entries"
    );
    let repeat = bake_deltas(&lights, Some(&cache));
    assert_eq!(repeat.indirect_tally.misses, 0);
    assert_eq!(repeat.animated_direct_tally.misses, 0);
    assert_eq!(repeat.direct_tally.misses, 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn p15_seed_zero_indirect_and_animated_direct_do_not_cross_serve_one_shared_cache() {
    let lights = vec![light(DVec3::ZERO, true)];
    let animated = AnimatedBakedLights::from_lights(&lights);
    assert_eq!(animated.entries()[0].source_index, 0);
    let geometry = cube_geometry_with_extent(0.25);
    let (cold_indirect, _, cold_animated, _) = bake_animated_delta_pair(&lights, &geometry, None);

    assert_eq!(cold_indirect.affinity_dims, [1, 1, 1]);
    assert_eq!(cold_animated.affinity_dims, [1, 1, 1]);
    assert_eq!(cold_indirect.affinity_lights, vec![0]);
    assert_eq!(cold_animated.affinity_lights, vec![0]);
    assert_ne!(
        cold_indirect.delta_subblocks, cold_animated.delta_subblocks,
        "fixture must distinguish indirect transport from animated direct transport"
    );

    // Both real bake entries fold the same geometry, one affinity cell,
    // spacing, validity mask, unitized light postcard, and seed zero. The
    // second stage must still miss because stage_id is the remaining axis.
    let (dir, cache) = fresh_cache("seed_zero_cross_bake");
    let (first_indirect, first_indirect_tally, first_animated, first_animated_tally) =
        bake_animated_delta_pair(&lights, &geometry, Some(&cache));
    assert_eq!(first_indirect_tally.hits, 0);
    assert_eq!(first_indirect_tally.misses, 1);
    assert_eq!(first_animated_tally.hits, 0);
    assert_eq!(first_animated_tally.misses, 1);
    assert_eq!(
        first_indirect.delta_subblocks,
        cold_indirect.delta_subblocks
    );
    assert_eq!(
        first_animated.delta_subblocks,
        cold_animated.delta_subblocks
    );

    let (warm_indirect, warm_indirect_tally, warm_animated, warm_animated_tally) =
        bake_animated_delta_pair(&lights, &geometry, Some(&cache));
    assert_eq!(warm_indirect_tally.misses, 0);
    assert_eq!(warm_indirect_tally.hits, 1);
    assert_eq!(warm_animated_tally.misses, 0);
    assert_eq!(warm_animated_tally.hits, 1);
    assert_eq!(warm_indirect.delta_subblocks, cold_indirect.delta_subblocks);
    assert_eq!(warm_animated.delta_subblocks, cold_animated.delta_subblocks);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn five_sections_corruption_is_a_soft_miss_and_stage_versions_change_keys() {
    let lights = delta_lights();
    let (dir, cache) = fresh_cache("all_corruption");
    let baseline = bake_deltas(&lights, Some(&cache));
    corrupt_cache_entries(&dir);
    let rebaked = bake_deltas(&lights, Some(&cache));
    assert_eq!(baseline.indirect.to_bytes(), rebaked.indirect.to_bytes());
    assert_eq!(baseline.direct.to_bytes(), rebaked.direct.to_bytes());
    assert_eq!(
        baseline
            .animated_direct
            .try_to_bytes()
            .expect("baseline codec"),
        rebaked
            .animated_direct
            .try_to_bytes()
            .expect("rebaked codec")
    );
    assert!(rebaked.indirect_tally.misses > 0);
    assert!(rebaked.direct_tally.misses > 0);
    assert!(rebaked.animated_direct_tally.misses > 0);

    let tree = tree_with_leaves(&[DVec3::ZERO]);
    let cell_key = cell_visibility_cache_key(&tree, &[], CELL_VISIBILITY_STAGE_VERSION);
    let bumped_cell_key = cell_visibility_cache_key(&tree, &[], CELL_VISIBILITY_STAGE_VERSION + 1);
    assert_ne!(cell_key.as_filename(), bumped_cell_key.as_filename());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn five_stage_version_bumps_miss_then_hit() {
    let key_light = light(DVec3::ZERO, true);
    let (dir, cache) = fresh_cache("stage_version_contracts");
    let delta_keys = [
        (INDIRECT_DELTA_SH_STAGE_ID, INDIRECT_DELTA_SH_STAGE_VERSION),
        (
            ANIMATED_DIRECT_DELTA_SH_STAGE_ID,
            ANIMATED_DIRECT_DELTA_SH_STAGE_VERSION,
        ),
        (DIRECT_SH_DELTA_STAGE_ID, DIRECT_SH_DELTA_STAGE_VERSION),
    ];
    for (stage_id, version) in delta_keys {
        let current = delta_sh_entry_cache_key(&DeltaShEntryKeyInputs {
            stage_id,
            stage_version: version,
            geometry_hash: &[9; 32],
            affinity_dims: [1, 1, 1],
            cell: 0,
            probe_spacing: 1.0,
            valid_probe_mask: u64::MAX,
            seed_axis: 0,
            light: &key_light,
        });
        let bumped = delta_sh_entry_cache_key(&DeltaShEntryKeyInputs {
            stage_id,
            stage_version: version + 1,
            geometry_hash: &[9; 32],
            affinity_dims: [1, 1, 1],
            cell: 0,
            probe_spacing: 1.0,
            valid_probe_mask: u64::MAX,
            seed_axis: 0,
            light: &key_light,
        });
        cache.put(&current, b"current");
        assert_eq!(
            cache.get(&bumped),
            None,
            "version bump must miss {stage_id}"
        );
        cache.put(&bumped, b"bumped");
        assert_eq!(cache.get(&bumped), Some(b"bumped".to_vec()));
    }

    let tree = tree_with_leaves(&[DVec3::ZERO]);
    let current_cell = cell_visibility_cache_key(&tree, &[], CELL_VISIBILITY_STAGE_VERSION);
    let bumped_cell = cell_visibility_cache_key(&tree, &[], CELL_VISIBILITY_STAGE_VERSION + 1);
    cache.put(&current_cell, b"current");
    assert_eq!(cache.get(&bumped_cell), None);
    cache.put(&bumped_cell, b"bumped");
    assert_eq!(cache.get(&bumped_cell), Some(b"bumped".to_vec()));

    let geometry = cube_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).expect("chunk version BVH");
    let tree = empty_tree();
    let exterior = HashSet::new();
    let lights = vec![light(DVec3::ZERO, false)];
    let alpha = AlphaLightsNs::from_lights(&lights);
    let inputs = ChunkLightListInputs {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
        lights: &alpha,
        tree: &tree,
        portals: &[],
        exterior_leaves: &exterior,
    };
    let current_chunk = chunk_light_list_cache_key(&inputs, 8.0, 64);
    let bumped_chunk = chunk_light_list_cache_key_with_version(
        &inputs,
        8.0,
        64,
        CHUNK_LIGHT_LIST_STAGE_VERSION + 1,
    );
    cache.put(&current_chunk, b"current");
    assert_eq!(cache.get(&bumped_chunk), None);
    cache.put(&bumped_chunk, b"bumped");
    assert_eq!(cache.get(&bumped_chunk), Some(b"bumped".to_vec()));
    let _ = std::fs::remove_dir_all(dir);
}
