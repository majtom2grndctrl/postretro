// Proofs that the staged parse hands out the monolithic parse's final data in
// order, rejects after its hand-outs as the monolithic parse does, honors
// cancellation between stages, and that load progress spans every phase.
// See: context/lib/testing_guide.md · context/lib/boot_sequence.md §2

use std::cell::Cell;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use postretro_level_format::SectionBlob;
use postretro_level_format::SectionId;
use postretro_level_format::cell_residency_set::{CellResidencySetSection, ResidencyEntry};
use postretro_level_format::fog_cell_masks::FogCellMasksSection;
use postretro_level_format::lightmap::{
    DIRECTION_TEXEL_BYTES, IRRADIANCE_FORMAT_RGBA16F, IRRADIANCE_TEXEL_BYTES, LightmapBlock,
    LightmapMode as WireLightmapMode, LightmapSection,
};
use postretro_level_format::lightmap_pool_bound::LightmapPoolBoundSection;

use super::tests::{cluster_directory_blob, sample_kinematic_section, zero_grid_cluster_directory};
use super::{HandOut, HandOutCollector, HandOutStage, LoadCancel, StagedParseOutcome, assemble};
use crate::lightmap_stream::{LightmapStorage, LightmapStreamingMode};
use crate::prl_load_test_fixtures::{write_portal_prl_load_fixture, write_prl_load_fixture};
use crate::prl_streaming::{ModeSource, load_prl_staged_with_modes, load_prl_with_modes};
use crate::sh_stream::{ShStorage, ShStreamingMode};
use crate::sh_stream_tests::{Id50Body, write_stream_fixture};
use crate::{LevelLoadProgress, LevelWorld, LoadPhase, LoadProgress, PrlLoadError};

const DELTA_LIMIT: u64 = super::MAX_DELTA_SECTION_BINDING_BYTES;
const SCATTER_LIMIT: u64 = postretro_level_format::animated_billboard_direct_scatter_delta_volumes::MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES;

const ALL_STAGES: [HandOutStage; 5] = [
    HandOutStage::ShManifest,
    HandOutStage::Geometry,
    HandOutStage::Spatial,
    HandOutStage::Entities,
    HandOutStage::Final,
];

// ---------------------------------------------------------------------------
// Field-by-field comparison.
//
// Every type these fields hold derives `Debug` (level-loader, level-format and
// render-data have no hand-written `Debug`), so a field's `Debug` text is
// complete and stricter than `PartialEq` on floats (it separates -0.0 and
// keeps NaN equal to itself). The two fields that hold a retained file handle,
// whose `Debug` names an OS handle, compare their manifests' content instead.
// ---------------------------------------------------------------------------

fn same_debug(field: &str, staged: &impl Debug, monolithic: &impl Debug) {
    let (staged, monolithic) = (format!("{staged:?}"), format!("{monolithic:?}"));
    if staged != monolithic {
        let at = staged
            .bytes()
            .zip(monolithic.bytes())
            .position(|(a, b)| a != b)
            .unwrap_or(staged.len().min(monolithic.len()));
        let window = |text: &str| text[at.saturating_sub(80)..(at + 80).min(text.len())].to_owned();
        panic!(
            "staged `{field}` differs from the monolithic parse's at byte {at}:\n  staged:     …{}…\n  monolithic: …{}…",
            window(&staged),
            window(&monolithic)
        );
    }
}

macro_rules! same_fields {
    ($label:literal, $staged:expr, $monolithic:expr, [$($field:ident),* $(,)?]) => {
        {
            $(same_debug(concat!($label, ".", stringify!($field)), &$staged.$field, &$monolithic.$field);)*
        }
    };
}

fn same_sh_storage(staged: &ShStorage, monolithic: &ShStorage) {
    match (staged.manifest(), monolithic.manifest()) {
        (None, None) => {}
        (Some(staged), Some(monolithic)) => same_debug(
            "sh_storage",
            &staged.content_debug(),
            &monolithic.content_debug(),
        ),
        _ => panic!("staged sh_storage {staged:?} differs in mode from {monolithic:?}"),
    }
}

fn same_lightmap_storage(staged: &LightmapStorage, monolithic: &LightmapStorage) {
    match (staged.manifest(), monolithic.manifest()) {
        (None, None) => {}
        (Some(staged), Some(monolithic)) => same_debug(
            "lightmap_storage",
            &staged.content_debug(),
            &monolithic.content_debug(),
        ),
        _ => panic!("staged lightmap_storage differs in mode from the monolithic parse's"),
    }
}

/// Every field of `staged` equals `monolithic`'s. The exhaustive destructure
/// makes a new `LevelWorld` field fail to compile here until it is compared.
fn assert_worlds_match(staged: &LevelWorld, monolithic: &LevelWorld) {
    let LevelWorld {
        vertices,
        indices,
        face_meta,
        cells,
        cell_portal_refs,
        cell_locator_root,
        cell_locator_nodes,
        portals,
        has_portals,
        portal_planes,
        cell_visibility,
        texture_names,
        texture_cache_keys,
        bvh,
        lights,
        light_influences,
        sh_volume,
        sh_storage,
        lightmap,
        lightmap_mode,
        sdf_atlas,
        chunk_light_list,
        animated_light_chunks,
        animated_light_weight_maps,
        delta_sh_volumes,
        direct_sh_volume,
        direct_sh_delta_volumes,
        animated_direct_sh_delta_volumes,
        billboard_direct_scatter_volume,
        animated_billboard_direct_scatter_delta_volumes,
        entity_shadow_lights,
        shadowmask_atlas,
        vertex_spec_owners,
        gpu_lighting_payloads,
        lightmap_storage,
        data_script,
        map_entities,
        kinematic_geometry,
        trigger_volumes,
        teleporters,
        ghost_links,
        fog_volumes,
        fog_pixel_scale,
        environment_volumes,
        environment_push_forces,
        reflection_probes,
        fog_cell_masks,
        navmesh,
        cell_draw_index,
        cluster_directory,
        cell_residency_set,
        prl_read_counters,
    } = staged;
    let m = monolithic;
    same_debug("vertices", vertices, &m.vertices);
    same_debug("indices", indices, &m.indices);
    same_debug("face_meta", face_meta, &m.face_meta);
    same_debug("cells", cells, &m.cells);
    same_debug("cell_portal_refs", cell_portal_refs, &m.cell_portal_refs);
    same_debug("cell_locator_root", cell_locator_root, &m.cell_locator_root);
    same_debug(
        "cell_locator_nodes",
        cell_locator_nodes,
        &m.cell_locator_nodes,
    );
    same_debug("portals", portals, &m.portals);
    same_debug("has_portals", has_portals, &m.has_portals);
    same_debug("portal_planes", portal_planes, &m.portal_planes);
    same_debug("cell_visibility", cell_visibility, &m.cell_visibility);
    same_debug("texture_names", texture_names, &m.texture_names);
    same_debug(
        "texture_cache_keys",
        texture_cache_keys,
        &m.texture_cache_keys,
    );
    same_debug("bvh", bvh, &m.bvh);
    same_debug("lights", lights, &m.lights);
    same_debug("light_influences", light_influences, &m.light_influences);
    same_debug("sh_volume", sh_volume, &m.sh_volume);
    same_sh_storage(sh_storage, &m.sh_storage);
    same_debug("lightmap", lightmap, &m.lightmap);
    same_debug("lightmap_mode", lightmap_mode, &m.lightmap_mode);
    same_debug("sdf_atlas", sdf_atlas, &m.sdf_atlas);
    same_debug("chunk_light_list", chunk_light_list, &m.chunk_light_list);
    same_debug(
        "animated_light_chunks",
        animated_light_chunks,
        &m.animated_light_chunks,
    );
    same_debug(
        "animated_light_weight_maps",
        animated_light_weight_maps,
        &m.animated_light_weight_maps,
    );
    same_debug("delta_sh_volumes", delta_sh_volumes, &m.delta_sh_volumes);
    same_debug("direct_sh_volume", direct_sh_volume, &m.direct_sh_volume);
    same_debug(
        "direct_sh_delta_volumes",
        direct_sh_delta_volumes,
        &m.direct_sh_delta_volumes,
    );
    same_debug(
        "animated_direct_sh_delta_volumes",
        animated_direct_sh_delta_volumes,
        &m.animated_direct_sh_delta_volumes,
    );
    same_debug(
        "billboard_direct_scatter_volume",
        billboard_direct_scatter_volume,
        &m.billboard_direct_scatter_volume,
    );
    same_debug(
        "animated_billboard_direct_scatter_delta_volumes",
        animated_billboard_direct_scatter_delta_volumes,
        &m.animated_billboard_direct_scatter_delta_volumes,
    );
    same_debug(
        "entity_shadow_lights",
        entity_shadow_lights,
        &m.entity_shadow_lights,
    );
    same_debug("shadowmask_atlas", shadowmask_atlas, &m.shadowmask_atlas);
    same_debug(
        "vertex_spec_owners",
        vertex_spec_owners,
        &m.vertex_spec_owners,
    );
    same_debug(
        "gpu_lighting_payloads",
        gpu_lighting_payloads,
        &m.gpu_lighting_payloads,
    );
    same_lightmap_storage(lightmap_storage, &m.lightmap_storage);
    same_debug("data_script", data_script, &m.data_script);
    same_debug("map_entities", map_entities, &m.map_entities);
    same_debug(
        "kinematic_geometry",
        kinematic_geometry,
        &m.kinematic_geometry,
    );
    same_debug("trigger_volumes", trigger_volumes, &m.trigger_volumes);
    same_debug("teleporters", teleporters, &m.teleporters);
    same_debug("ghost_links", ghost_links, &m.ghost_links);
    same_debug("fog_volumes", fog_volumes, &m.fog_volumes);
    same_debug("fog_pixel_scale", fog_pixel_scale, &m.fog_pixel_scale);
    same_debug(
        "environment_volumes",
        environment_volumes,
        &m.environment_volumes,
    );
    same_debug(
        "environment_push_forces",
        environment_push_forces,
        &m.environment_push_forces,
    );
    same_debug("reflection_probes", reflection_probes, &m.reflection_probes);
    same_debug("fog_cell_masks", fog_cell_masks, &m.fog_cell_masks);
    same_debug("navmesh", navmesh, &m.navmesh);
    same_debug("cell_draw_index", cell_draw_index, &m.cell_draw_index);
    same_debug("cluster_directory", cluster_directory, &m.cluster_directory);
    same_debug(
        "cell_residency_set",
        cell_residency_set,
        &m.cell_residency_set,
    );
    same_debug("prl_read_counters", prl_read_counters, &m.prl_read_counters);
}

/// Each hand-out's sections equal the monolithic parse's final data for
/// those sections, the parse-end rewritten ones included (pin P25).
fn assert_hand_outs_match(hand_outs: &[HandOut], monolithic: &LevelWorld) {
    let stages: Vec<_> = hand_outs.iter().map(HandOut::stage).collect();
    assert_eq!(
        stages, ALL_STAGES,
        "an accepted parse sends every hand-out in order"
    );
    for hand_out in hand_outs {
        match hand_out {
            HandOut::ShManifest(sh) => match (&sh.manifest, monolithic.sh_stream_manifest()) {
                (None, None) => {}
                (Some(staged), Some(monolithic)) => same_debug(
                    "sh_manifest",
                    &staged.manifest_for_test().content_debug(),
                    &monolithic.content_debug(),
                ),
                _ => panic!("hand-out 1 carries SH metadata exactly when the level streams SH"),
            },
            HandOut::Geometry(geometry) => same_fields!(
                "geometry",
                geometry,
                monolithic,
                [
                    vertices,
                    indices,
                    face_meta,
                    texture_names,
                    texture_cache_keys
                ]
            ),
            HandOut::Spatial(spatial) => same_fields!(
                "spatial",
                spatial,
                monolithic,
                [
                    bvh,
                    cells,
                    cell_visibility,
                    cell_residency_set,
                    cell_locator_root,
                    cell_locator_nodes,
                ]
            ),
            HandOut::Entities(entities) => same_fields!(
                "entities",
                entities,
                monolithic,
                [
                    data_script,
                    map_entities,
                    kinematic_geometry,
                    trigger_volumes,
                    teleporters,
                    ghost_links,
                    environment_volumes,
                    environment_push_forces,
                    reflection_probes,
                    fog_volumes,
                    fog_pixel_scale,
                    navmesh,
                    cell_draw_index,
                ]
            ),
            HandOut::Final(final_hand_out) => {
                same_fields!(
                    "final",
                    final_hand_out,
                    monolithic,
                    [
                        portals,
                        has_portals,
                        cell_portal_refs,
                        fog_cell_masks,
                        prl_read_counters
                    ]
                );
                let lighting = &final_hand_out.lighting;
                same_fields!(
                    "lighting",
                    lighting,
                    monolithic,
                    [
                        lights,
                        light_influences,
                        sh_volume,
                        lightmap,
                        lightmap_mode,
                        sdf_atlas,
                        chunk_light_list,
                        animated_light_chunks,
                        animated_light_weight_maps,
                        delta_sh_volumes,
                        direct_sh_volume,
                        direct_sh_delta_volumes,
                        animated_direct_sh_delta_volumes,
                        billboard_direct_scatter_volume,
                        animated_billboard_direct_scatter_delta_volumes,
                        entity_shadow_lights,
                        shadowmask_atlas,
                        vertex_spec_owners,
                        gpu_lighting_payloads,
                        cluster_directory,
                    ]
                );
                same_sh_storage(&lighting.sh_storage, &monolithic.sh_storage);
                same_lightmap_storage(&lighting.lightmap_storage, &monolithic.lightmap_storage);
            }
        }
    }
}

/// One staged run's observations.
struct StagedRun {
    result: Result<StagedParseOutcome, PrlLoadError>,
    hand_outs: Vec<HandOut>,
    /// Whether hand-out 1's manifest already had its cluster topology when
    /// hand-out 1 arrived.
    topology_at_first_hand_out: bool,
}

fn collect_staged(
    run: impl FnOnce(&mut dyn FnMut(HandOut)) -> Result<StagedParseOutcome, PrlLoadError>,
) -> StagedRun {
    let mut hand_outs = Vec::new();
    let mut topology_at_first_hand_out = false;
    let result = run(&mut |hand_out: HandOut| {
        if let HandOut::ShManifest(sh) = &hand_out
            && let Some(metadata) = &sh.manifest
        {
            topology_at_first_hand_out = metadata
                .manifest_for_test()
                .cluster_portal_topology_installed();
        }
        hand_outs.push(hand_out);
    });
    StagedRun {
        result,
        hand_outs,
        topology_at_first_hand_out,
    }
}

/// The equivalence verdict for one file: the staged run rejects with the
/// monolithic error, or accepts with hand-outs and an assembled world equal to
/// the monolithic world.
fn assert_staged_matches(monolithic: &Result<LevelWorld, PrlLoadError>, staged: StagedRun) {
    let stages: Vec<_> = staged.hand_outs.iter().map(HandOut::stage).collect();
    assert!(
        ALL_STAGES.starts_with(&stages),
        "hand-outs arrive in stage order, each once: {stages:?}"
    );
    match (monolithic, staged.result) {
        (Err(monolithic), Err(staged_error)) => {
            assert_eq!(
                format!("{staged_error:?}"),
                format!("{monolithic:?}"),
                "the staged parse rejects with the monolithic parse's error"
            );
            assert!(
                !stages.contains(&HandOutStage::Final),
                "a rejected parse never sends hand-out 5"
            );
        }
        (Ok(world), Ok(StagedParseOutcome::Complete)) => {
            assert_hand_outs_match(&staged.hand_outs, world);
            assert!(
                !staged.topology_at_first_hand_out,
                "hand-out 1 goes out before the cluster topology is installed"
            );
            let mut collector = HandOutCollector::new();
            for hand_out in staged.hand_outs {
                collector.accept(hand_out);
            }
            let assembled = assemble(collector.complete().expect("all five hand-outs"));
            assert_worlds_match(&assembled, world);
        }
        (monolithic, staged) => panic!(
            "staged result {staged:?} disagrees with the monolithic result {:?}",
            monolithic.as_ref().map(|_| "Ok(LevelWorld)")
        ),
    }
}

/// Run the staged parse beside a monolithic load the test loaders just made,
/// and assert the two agree. Every test load through `load_prl`,
/// `load_prl_with_progress` and the mode- and limit-injecting test loaders
/// passes through here, so the equivalence holds on every fixture the
/// loader's tests build or load. The staged run gets its own thread so its
/// logs never reach the caller's per-thread log capture.
pub(crate) fn cross_check_staged<F>(monolithic: &Result<LevelWorld, PrlLoadError>, run_staged: F)
where
    F: FnOnce(&mut dyn FnMut(HandOut)) -> Result<StagedParseOutcome, PrlLoadError> + Send,
{
    let staged = std::thread::scope(|scope| {
        scope
            .spawn(move || collect_staged(run_staged))
            .join()
            .expect("the staged parse never panics")
    });
    assert_staged_matches(monolithic, staged);
}

// ---------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------

/// A temp PRL removed on drop.
struct TempPrl(PathBuf);

impl TempPrl {
    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for TempPrl {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

fn blob(section_id: SectionId, version: u16, data: Vec<u8>) -> SectionBlob {
    SectionBlob {
        section_id: section_id as u32,
        version,
        data,
    }
}

/// A two-cell portal level with a residency set, a lightmap that can stream,
/// and a mover whose sealed portal ids name the one real portal and one past
/// it, so entity decode's drop is visible in hand-out 4.
fn portal_lightmap_mover_fixture(name: &str) -> TempPrl {
    let block = |cell_id: u32| {
        let (width, height) = (16u16, 16u16);
        let texels = usize::from(width) * usize::from(height);
        LightmapBlock {
            cell_id,
            width,
            height,
            irradiance: vec![cell_id as u8 + 1; texels * IRRADIANCE_TEXEL_BYTES],
            direction: vec![7; texels / 4 * DIRECTION_TEXEL_BYTES],
        }
    };
    let lightmap = LightmapSection {
        direction_texel_scale: 2,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        mode: WireLightmapMode::Shadowed,
        blocks: vec![block(0), block(1)],
    };
    let entry = |cell_id| ResidencyEntry { lead: 0, cell_id };
    let residency = CellResidencySetSection {
        max_lead: 32 * 1024,
        offsets: vec![0, 2, 4],
        entries: vec![entry(0), entry(1), entry(0), entry(1)],
    };
    let mut movers = sample_kinematic_section();
    movers.movers[0].sealed_portal_ids = vec![0, 3];
    TempPrl(write_portal_prl_load_fixture(
        [
            blob(SectionId::CellResidencySet, 1, residency.to_bytes()),
            blob(SectionId::Lightmap, 1, lightmap.to_bytes()),
            blob(
                SectionId::LightmapPoolBound,
                1,
                LightmapPoolBoundSection {
                    worst_lightmap_layers: 1,
                    worst_mask_layers: 0,
                }
                .to_bytes(),
            ),
            blob(SectionId::KinematicGeometry, 1, movers.to_bytes()),
        ],
        name,
    ))
}

/// Run both parses of `path` under injected modes, with no cross-check in
/// between, and assert they agree. Returns the monolithic result.
fn assert_equivalent(
    path: &str,
    sh: ShStreamingMode,
    lightmap: LightmapStreamingMode,
) -> Result<LevelWorld, PrlLoadError> {
    let modes = ModeSource::Injected { sh, lightmap };
    let monolithic = load_prl_with_modes(
        path,
        modes,
        DELTA_LIMIT,
        SCATTER_LIMIT,
        &Arc::new(LoadProgress::new()),
    );
    let staged = collect_staged(|sink| {
        load_prl_staged_with_modes(
            path,
            modes,
            DELTA_LIMIT,
            SCATTER_LIMIT,
            &Arc::new(LoadProgress::new()),
            &AtomicBool::new(false),
            sink,
        )
    });
    assert_staged_matches(&monolithic, staged);
    monolithic
}

const MODE_MATRIX: [(ShStreamingMode, LightmapStreamingMode); 4] = [
    (ShStreamingMode::Off, LightmapStreamingMode::AllResident),
    (ShStreamingMode::Off, LightmapStreamingMode::Stream),
    (
        ShStreamingMode::SyncProof,
        LightmapStreamingMode::AllResident,
    ),
    (ShStreamingMode::Async, LightmapStreamingMode::Stream),
];

fn run_staged(
    path: &str,
    sh: ShStreamingMode,
    lightmap: LightmapStreamingMode,
    progress: &Arc<LoadProgress>,
    cancel: &(impl LoadCancel + ?Sized),
    sink: &mut dyn FnMut(HandOut),
) -> Result<StagedParseOutcome, PrlLoadError> {
    load_prl_staged_with_modes(
        path,
        ModeSource::Injected { sh, lightmap },
        DELTA_LIMIT,
        SCATTER_LIMIT,
        progress,
        cancel,
        sink,
    )
}

// ---------------------------------------------------------------------------
// Equivalence.
// ---------------------------------------------------------------------------

#[test]
fn staged_parse_hand_outs_equal_monolithic_parse() {
    let minimal = TempPrl(write_prl_load_fixture(
        [],
        "postretro_test_staged_equivalence_minimal.prl",
    ));
    let portal = portal_lightmap_mover_fixture("postretro_test_staged_equivalence_portal.prl");
    let directory = TempPrl(write_prl_load_fixture(
        [cluster_directory_blob(&zero_grid_cluster_directory(&[
            SectionId::OctahedralShVolume,
        ]))],
        "postretro_test_staged_equivalence_directory.prl",
    ));
    let streamed = write_stream_fixture(Id50Body::Valid);
    let streamed_path = streamed.path.to_str().unwrap();

    let mut saw_streamed_sh = false;
    let mut saw_streamed_lightmap = false;
    let mut saw_whole_file = false;
    for path in [
        minimal.path(),
        portal.path(),
        directory.path(),
        streamed_path,
    ] {
        for (sh, lightmap) in MODE_MATRIX {
            let world = assert_equivalent(path, sh, lightmap)
                .unwrap_or_else(|error| panic!("{path} under {sh:?}/{lightmap:?}: {error}"));
            saw_streamed_sh |= world.sh_storage.is_streaming();
            saw_streamed_lightmap |= world.lightmap_storage.is_streaming();
            // The whole-file backing keeps no retained file to read through.
            saw_whole_file |= world.lightmap_storage().mode() == LightmapStreamingMode::AllResident
                && !world.sh_storage.is_streaming()
                && lightmap == LightmapStreamingMode::AllResident;
        }
    }
    assert!(saw_streamed_sh, "the streamed-SH backing was covered");
    assert!(
        saw_streamed_lightmap,
        "the lightmap-streaming backing was covered"
    );
    assert!(saw_whole_file, "the whole-file backing was covered");

    // The mover drop is a parse-end rewrite in the old order; hand-out 4
    // already carries its result.
    let mut hand_outs = Vec::new();
    run_staged(
        portal.path(),
        ShStreamingMode::Off,
        LightmapStreamingMode::Stream,
        &Arc::new(LoadProgress::new()),
        &AtomicBool::new(false),
        &mut |hand_out| hand_outs.push(hand_out),
    )
    .unwrap();
    let Some(HandOut::Entities(entities)) = hand_outs
        .iter()
        .find(|hand_out| hand_out.stage() == HandOutStage::Entities)
    else {
        panic!("hand-out 4 arrived");
    };
    assert_eq!(
        entities.kinematic_geometry.movers[0].sealed_portal_ids,
        vec![0]
    );
}

/// On-demand sweep over the dev maps: `cargo test -p postretro-level-loader
/// staged_parse_matches_monolithic_on_dev_maps -- --ignored --nocapture`.
/// `POSTRETRO_STAGED_SWEEP_MAX_BYTES` skips maps larger than its value.
#[test]
#[ignore = "reads every baked dev map; run on demand"]
fn staged_parse_matches_monolithic_on_dev_maps() {
    let maps = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev/maps");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&maps)
        .unwrap_or_else(|error| panic!("{}: {error}", maps.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "prl"))
        .collect();
    paths.sort();
    // Optional size cap, for a quick pass that skips the multi-GiB stress bakes.
    let max_bytes: Option<u64> =
        std::env::var("POSTRETRO_STAGED_SWEEP_MAX_BYTES")
            .ok()
            .map(|value| {
                value
                    .parse()
                    .expect("POSTRETRO_STAGED_SWEEP_MAX_BYTES is a byte count")
            });
    let mut compared = 0;
    for path in &paths {
        let size = std::fs::metadata(path).map_or(0, |metadata| metadata.len());
        if max_bytes.is_some_and(|max| size > max) {
            println!(
                "skipping {}: {size} bytes is over the sweep cap",
                path.display()
            );
            continue;
        }
        let path = path.to_str().unwrap();
        for (sh, lightmap) in [
            (ShStreamingMode::Off, LightmapStreamingMode::AllResident),
            (ShStreamingMode::SyncProof, LightmapStreamingMode::Stream),
        ] {
            // A stale bake fails the monolithic parse; that is not this
            // test's concern, so say why and move on.
            if let Err(error) = load_prl_with_modes(
                path,
                ModeSource::Injected { sh, lightmap },
                DELTA_LIMIT,
                SCATTER_LIMIT,
                &Arc::new(LoadProgress::new()),
            ) {
                println!(
                    "skipping {path} ({sh:?}/{lightmap:?}): stale or unloadable bake: {error}"
                );
                continue;
            }
            assert_equivalent(path, sh, lightmap).unwrap();
            println!("{path} ({sh:?}/{lightmap:?}): staged parse matches");
            compared += 1;
        }
    }
    println!("{compared} map/mode pair(s) compared");
}

// ---------------------------------------------------------------------------
// Late validation.
// ---------------------------------------------------------------------------

/// Hand-outs 1–4 go out, then the monolithic parse's error, with no hand-out 5.
fn assert_rejects_after_hand_outs(path: &str) -> PrlLoadError {
    let monolithic = load_prl_with_modes(
        path,
        ModeSource::Injected {
            sh: ShStreamingMode::Off,
            lightmap: LightmapStreamingMode::AllResident,
        },
        DELTA_LIMIT,
        SCATTER_LIMIT,
        &Arc::new(LoadProgress::new()),
    )
    .expect_err("the fixture violates a parse-end check");
    let mut stages = Vec::new();
    let staged = run_staged(
        path,
        ShStreamingMode::Off,
        LightmapStreamingMode::AllResident,
        &Arc::new(LoadProgress::new()),
        &AtomicBool::new(false),
        &mut |hand_out| stages.push(hand_out.stage()),
    )
    .expect_err("the staged parse rejects the same file");
    assert_eq!(stages, ALL_STAGES[..4], "hand-outs 1–4 went out first");
    assert_eq!(format!("{staged:?}"), format!("{monolithic:?}"));
    staged
}

#[test]
fn late_validation_rejects_after_hand_outs() {
    // Cross-section: three fog masks for a two-cell level.
    let masks = TempPrl(write_prl_load_fixture(
        [blob(
            SectionId::FogCellMasks,
            1,
            FogCellMasksSection {
                masks: vec![0, 0, 0],
            }
            .to_bytes(),
        )],
        "postretro_test_staged_late_cross_section.prl",
    ));
    let error = assert_rejects_after_hand_outs(masks.path());
    assert!(
        matches!(
            error,
            PrlLoadError::SectionValidation {
                section: "FogCellMasks",
                ..
            }
        ),
        "{error:?}"
    );

    // Cluster directory naming a cell past the two-cell table.
    let mut directory = zero_grid_cluster_directory(&[SectionId::OctahedralShVolume]);
    directory.runtime_cell_count = 3;
    directory.clusters[1].member_count = 2;
    directory.members = vec![0, 1, 2];
    let past_table = TempPrl(write_prl_load_fixture(
        [cluster_directory_blob(&directory)],
        "postretro_test_staged_late_cluster_directory.prl",
    ));
    let error = assert_rejects_after_hand_outs(past_table.path());
    assert!(
        format!("{error}").contains("runtime_cell_count"),
        "the semantic check names the cell count: {error}"
    );
}

// ---------------------------------------------------------------------------
// Cancellation and order.
// ---------------------------------------------------------------------------

/// Cancels from the `limit + 1`-th poll on, and remembers whether it has.
struct CancelAfterPolls {
    polls: Cell<usize>,
    limit: usize,
    seen: Cell<bool>,
}

impl CancelAfterPolls {
    fn new(limit: usize) -> Self {
        Self {
            polls: Cell::new(0),
            limit,
            seen: Cell::new(false),
        }
    }
}

impl LoadCancel for CancelAfterPolls {
    fn is_cancelled(&self) -> bool {
        self.polls.set(self.polls.get() + 1);
        let cancelled = self.polls.get() > self.limit;
        self.seen.set(self.seen.get() || cancelled);
        cancelled
    }
}

#[test]
fn cancelled_staged_parse_stops_between_stages() {
    let streamed = write_stream_fixture(Id50Body::Valid);
    let path = streamed.path.to_str().unwrap();
    let modes = (
        ShStreamingMode::SyncProof,
        LightmapStreamingMode::AllResident,
    );

    let uncancelled = CancelAfterPolls::new(usize::MAX);
    run_staged(
        path,
        modes.0,
        modes.1,
        &Arc::new(LoadProgress::new()),
        &uncancelled,
        &mut |_| {},
    )
    .unwrap();
    let boundaries = uncancelled.polls.get();
    assert!(boundaries > ALL_STAGES.len(), "every stage has a boundary");

    // Cancel at each boundary in turn: the parse stops there, sends nothing
    // after the flag was seen, and leaves the counter unfinished.
    for limit in 0..boundaries {
        let cancel = CancelAfterPolls::new(limit);
        let progress = Arc::new(LoadProgress::new());
        let mut stages = Vec::new();
        let outcome = run_staged(
            path,
            modes.0,
            modes.1,
            &progress,
            &cancel,
            &mut |hand_out| {
                assert!(!cancel.seen.get(), "no hand-out after the cancel was seen");
                stages.push(hand_out.stage());
            },
        )
        .unwrap();
        assert_eq!(
            outcome,
            StagedParseOutcome::Cancelled,
            "cancelled at boundary {limit}"
        );
        assert_eq!(
            cancel.polls.get(),
            limit + 1,
            "the parse stopped at the first cancelled poll"
        );
        assert!(ALL_STAGES[..4].starts_with(&stages), "{stages:?}");
        assert!(
            !progress.is_finished(),
            "a cancelled parse does not finish its counter"
        );
    }

    // A flag raised by a consumer of hand-out k stops the parse after k.
    for k in 1..=4 {
        let cancel = AtomicBool::new(false);
        let mut stages = Vec::new();
        let outcome = run_staged(
            path,
            modes.0,
            modes.1,
            &Arc::new(LoadProgress::new()),
            &cancel,
            &mut |hand_out| {
                stages.push(hand_out.stage());
                if stages.len() == k {
                    cancel.store(true, Ordering::Release);
                }
            },
        )
        .unwrap();
        assert_eq!(outcome, StagedParseOutcome::Cancelled);
        assert_eq!(stages, ALL_STAGES[..k]);
    }
}

#[test]
fn hand_outs_arrive_in_order() {
    let portal = portal_lightmap_mover_fixture("postretro_test_staged_order_portal.prl");
    let streamed = write_stream_fixture(Id50Body::Valid);
    for path in [portal.path(), streamed.path.to_str().unwrap()] {
        for (sh, lightmap) in MODE_MATRIX {
            let progress = Arc::new(LoadProgress::new());
            let mut seen = Vec::new();
            let outcome = run_staged(
                path,
                sh,
                lightmap,
                &progress,
                &AtomicBool::new(false),
                &mut |hand_out| seen.push((hand_out.stage(), progress.fraction())),
            )
            .unwrap();
            assert_eq!(outcome, StagedParseOutcome::Complete);
            let stages: Vec<_> = seen.iter().map(|(stage, _)| *stage).collect();
            assert_eq!(stages, ALL_STAGES, "{path} under {sh:?}/{lightmap:?}");
            assert!(
                seen.windows(2).all(|pair| pair[0].1 <= pair[1].1),
                "parse progress rises across the hand-outs: {seen:?}"
            );
            assert_eq!(progress.fraction(), 1.0);
        }
    }
}

#[test]
#[should_panic(expected = "arrived after")]
fn hand_out_collector_refuses_out_of_order_hand_outs() {
    let minimal = TempPrl(write_prl_load_fixture(
        [],
        "postretro_test_staged_collector_order.prl",
    ));
    let mut hand_outs = Vec::new();
    run_staged(
        minimal.path(),
        ShStreamingMode::Off,
        LightmapStreamingMode::AllResident,
        &Arc::new(LoadProgress::new()),
        &AtomicBool::new(false),
        &mut |hand_out| hand_outs.push(hand_out),
    )
    .unwrap();
    let mut collector = HandOutCollector::new();
    let geometry = hand_outs.remove(1);
    collector.accept(hand_outs.remove(0));
    collector.accept(hand_outs.remove(0)); // spatial before geometry
    collector.accept(geometry);
}

// ---------------------------------------------------------------------------
// Progress.
// ---------------------------------------------------------------------------

#[test]
fn staged_parse_progress_moves_before_the_first_section_read() {
    let streamed = write_stream_fixture(Id50Body::Valid);
    let path = streamed.path.to_str().unwrap();
    let progress = Arc::new(LoadProgress::new());
    let mut fraction_at_first_hand_out = None;
    let mut read_counters = None;
    run_staged(
        path,
        ShStreamingMode::SyncProof,
        LightmapStreamingMode::AllResident,
        &progress,
        &AtomicBool::new(false),
        &mut |hand_out| match hand_out {
            HandOut::ShManifest(_) => fraction_at_first_hand_out = Some(progress.fraction()),
            HandOut::Final(final_hand_out) => read_counters = final_hand_out.prl_read_counters,
            _ => {}
        },
    )
    .unwrap();

    // The first credit after the plan is the header and table alone: bytes
    // read outside every section, before the manifest or any section.
    let history = progress.history();
    let (first_credit, _) = history[1];
    let read_counters = read_counters.expect("a positional load keeps its read counters");
    assert!(first_credit > 0, "the table read is credited: {history:?}");
    assert_eq!(first_credit, read_counters.bytes_outside_sections());
    // Hand-out 1 goes out before the first section read, with the table and
    // the manifest's reads already credited.
    let at_first = fraction_at_first_hand_out.expect("hand-out 1 arrived");
    assert!(
        at_first > 0.0,
        "progress moved before the first section read"
    );
    assert!(at_first < 1.0);
}

/// Every sample of the whole-load counter is at least the previous one,
/// below 1.0 until settling finishes, and exactly 1.0 after.
fn assert_load_samples(samples: &[(String, f32)]) {
    for pair in samples.windows(2) {
        assert!(
            pair[1].1 >= pair[0].1,
            "progress fell from {:?} to {:?}",
            pair[0],
            pair[1]
        );
    }
    let (last, rest) = samples.split_last().unwrap();
    assert_eq!(last.1, 1.0, "exactly 1.0 once every phase finished");
    assert!(
        rest.iter().all(|(_, fraction)| *fraction < 1.0),
        "below 1.0 until settling finishes: {samples:?}"
    );
}

#[test]
fn level_load_progress_spans_every_phase() {
    let streamed = write_stream_fixture(Id50Body::Valid);
    let path = streamed.path.to_str().unwrap();
    let load = LevelLoadProgress::new();
    let mut samples = vec![("start".to_owned(), load.fraction())];
    let sample = |samples: &mut Vec<(String, f32)>, label: &str| {
        samples.push((label.to_owned(), load.fraction()));
    };

    let mut parse_samples = Vec::new();
    run_staged(
        path,
        ShStreamingMode::SyncProof,
        LightmapStreamingMode::AllResident,
        load.phase(LoadPhase::Parse),
        &AtomicBool::new(false),
        &mut |hand_out| parse_samples.push((format!("{:?}", hand_out.stage()), load.fraction())),
    )
    .unwrap();
    samples.extend(parse_samples);
    sample(&mut samples, "parse finished");
    let after_parse = load.fraction();
    assert!(
        (after_parse - crate::LOAD_PHASE_WEIGHT_PARSE as f32).abs() < 1e-6,
        "a finished parse earns its share and no more (pin P15): {after_parse}"
    );

    let prep = load.phase(LoadPhase::Prep);
    prep.begin(3);
    for task in 0..3 {
        prep.advance(1);
        sample(&mut samples, &format!("prep task {task}"));
    }
    prep.finish();
    sample(&mut samples, "prep finished");

    let upload = load.phase(LoadPhase::Upload);
    upload.begin(4096);
    upload.advance(1024);
    sample(&mut samples, "upload 1 KiB");
    upload.advance(3072);
    upload.finish();
    sample(&mut samples, "upload finished");

    let settling = load.phase(LoadPhase::Settling);
    for remaining in [5, 3, 4, 1, 0] {
        settling.credit_remaining(remaining);
        sample(&mut samples, &format!("settling, {remaining} unsettled"));
    }
    assert!(
        load.fraction() < 1.0,
        "a settled shortfall is not a finished phase"
    );
    settling.finish();
    sample(&mut samples, "settling finished");

    assert_load_samples(&samples);
    // Each phase raised the bar with its own units, before it finished.
    let at = |label: &str| {
        samples
            .iter()
            .find(|(sampled, _)| sampled == label)
            .map(|(_, fraction)| *fraction)
            .unwrap()
    };
    assert!(at("prep task 0") > at("parse finished"));
    assert!(at("upload 1 KiB") > at("prep finished"));
    assert!(at("settling, 3 unsettled") > at("settling, 5 unsettled"));
    assert_eq!(
        at("settling, 4 unsettled"),
        at("settling, 3 unsettled"),
        "a growing shortfall stalls the bar"
    );
}

#[test]
fn near_empty_level_and_empty_phases_reach_one() {
    let minimal = TempPrl(write_prl_load_fixture(
        [],
        "postretro_test_staged_progress_near_empty.prl",
    ));
    let load = LevelLoadProgress::new();
    run_staged(
        minimal.path(),
        ShStreamingMode::Off,
        LightmapStreamingMode::AllResident,
        load.phase(LoadPhase::Parse),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    let mut samples = vec![("parse finished".to_owned(), load.fraction())];
    // An empty prep set, nothing to upload, nothing to settle: each phase
    // finishes without planning anything.
    for phase in [LoadPhase::Prep, LoadPhase::Upload, LoadPhase::Settling] {
        load.phase(phase).finish();
        samples.push((format!("{phase:?} finished"), load.fraction()));
    }
    assert_load_samples(&samples);
    assert!(load.is_finished());
}
