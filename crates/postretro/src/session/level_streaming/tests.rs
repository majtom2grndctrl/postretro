//! Level-scope streaming proofs: one issuer for SH and lightmaps, session
//! replacement, the spawn preload, renderer drain failures, reload and unload
//! lifetimes. See: context/lib/testing_guide.md · plan AC 11, 13, 19; P3, P9, P12

use std::collections::{BTreeSet, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::thread::ThreadId;

use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_loader::{
    LevelWorld, LightmapDrainOutcome, PrlLoadError, ShStreamManifest, ShStreamingMode,
};
use postretro_renderer::{LightmapResidencyDrainError, ShResidencySnapshot};
use postretro_visibility::VisibleCells;

use super::*;
use crate::lightmap_streaming::controller::BlockPhase;
use crate::lightmap_streaming::prl_test_fixture::{
    StreamedLightmapPrl, eye_in_cell, manifest_pool_model,
};
use crate::lightmap_streaming::source::LightmapBlockSource;
use crate::lightmap_streaming::test_fixtures::*;
use crate::session::lightmap_residency::LightmapLevelView;
use crate::session::sh_async_workers::{ShAsyncWorkers, ShWorkerSource};
use crate::session::sh_residency::sync_manifest_test_fixture;
use crate::sh_streaming::controller::ShClusterRequest;
use crate::streaming::request::StreamResource;

/// SH's id 49, decoded as level scope does.
fn sh_hints(manifest: &ShStreamManifest) -> Arc<ClusterHints> {
    Arc::new(ClusterHints::decode(manifest.cluster_directory()).unwrap())
}

/// An SH session over `manifest` in `mode`, budgeted as the tests' renderer.
fn sh_session(
    world: &LevelWorld,
    manifest: Arc<ShStreamManifest>,
    mode: ShStreamingMode,
) -> ShStreamingSession {
    let hints = sh_hints(&manifest);
    let mut session = ShStreamingSession::from_snapshot(
        manifest,
        ShResidencySnapshot {
            effective_floor_bytes: 1024 * 1024,
            ..ShResidencySnapshot::default()
        },
        world.cell_visibility.as_ref(),
        hints,
    )
    .unwrap();
    session.set_mode_for_test(mode);
    session
}

/// The renderer's lightmap drain, standing in with the pool model: plans
/// the parked batch and applies its outcome through the level owner.
fn model_drains_lightmap(
    level: &mut LevelStreaming,
    model: &mut postretro_render_cpu::lightmap_pool::LightmapPoolModel,
) {
    let Some(batch) = level
        .lightmap_mut()
        .and_then(LightmapStreamingSession::take_drain_batch_for_renderer)
    else {
        return;
    };
    let outcome = model_drain(model, batch);
    level.apply_lightmap_drain(Ok(outcome)).unwrap();
}

fn frame<'a>(
    visible_cells: &'a VisibleCells,
    camera_cell: usize,
    cpu: &'a StageFrame<StreamingStage>,
) -> StreamingFrame<'a> {
    StreamingFrame {
        visible_cells,
        camera_cell: Some(camera_cell),
        path: PORTAL,
        monotonic_seconds: 0.0,
        cpu,
    }
}

/// The renderer installs every pair of the parked lightmap batch.
fn renderer_installs_lightmap_batch(level: &mut LevelStreaming) -> Vec<u32> {
    let lightmap = level.lightmap_mut().expect("lightmap session");
    let Some(batch) = lightmap.take_drain_batch_for_renderer() else {
        return Vec::new();
    };
    let installed: Vec<u32> = batch.ready.iter().map(|prepared| prepared.block).collect();
    lightmap
        .apply_outcome(LightmapDrainOutcome {
            installed: installed.clone(),
            pool: headroom(8),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    installed
}

fn lightmap_session(source: &Arc<TestBlockSource>) -> LightmapStreamingSession {
    LightmapStreamingSession::with_source(
        Arc::clone(source) as Arc<dyn LightmapBlockSource>,
        &corridor_set(),
        None,
    )
    .unwrap()
}

/// SH chunks at chosen offsets, read through the shared log and gate.
struct ShTestSource {
    ranges: Vec<Range<u64>>,
    log: Arc<ReadLog>,
}

impl ShWorkerSource for ShTestSource {
    fn cluster_count(&self) -> u32 {
        self.ranges.len() as u32
    }

    fn chunk_file_range(&self, cluster_id: u32) -> Result<Range<u64>, PrlLoadError> {
        Ok(self.ranges[cluster_id as usize].clone())
    }

    fn decoded_bytes(&self, cluster_id: u32) -> u64 {
        let range = &self.ranges[cluster_id as usize];
        range.end - range.start
    }

    fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        self.log.record(StreamResource::Sh, &range);
        self.log.gate.wait_while_held(range.start);
        Ok(pattern(&range))
    }

    fn decode(
        &self,
        cluster_id: u32,
        bytes: Vec<u8>,
    ) -> Result<DecodedClusterShPayload, PrlLoadError> {
        Ok(DecodedClusterShPayload {
            cluster_id,
            bytes,
            blocks: Vec::new(),
        })
    }
}

fn sh_request(cluster_id: u32, mandatory: bool) -> ShClusterRequest {
    ShClusterRequest {
        generation: 9,
        content_tag: [9; 32],
        cluster_id,
        chunk_hash: [cluster_id as u8; 32],
        mandatory,
    }
}

fn at(slot: u64) -> Range<u64> {
    slot * FAR..slot * FAR + 64
}

// AC 19: with SH and lightmap demand both pending through the level-scope
// issuer, one thread performs every read, mandatory before optional across
// both resources, each tier in ascending file offset.
#[test]
fn level_issuer_reads_sh_and_lightmap_mandatory_first_in_ascending_offset_on_one_thread() {
    let log = Arc::new(ReadLog::default());
    // SH cluster k at the slots below; lightmap block b at slot 2b + 1.
    let sh_source = Arc::new(ShTestSource {
        ranges: vec![at(100), at(2), at(6), at(10), at(0)],
        log: Arc::clone(&log),
    });
    let (mut workers, sh_route) = ShAsyncWorkers::prepare_with_source(sh_source, 1).unwrap();
    let lightmap_source = TestBlockSource::with_log(corridor_blocks(64, false), Arc::clone(&log));
    let set = corridor_set();
    let mut lightmap = lightmap_session(&lightmap_source);
    let reads = LevelReadIssuer::spawn(Some(Box::new(sh_route)), lightmap.take_route()).unwrap();
    workers.attach_issuer(reads.issuer().clone());
    workers.publish_targets(&(0..5).collect::<BTreeSet<_>>());

    // Pin the issuer inside one read while both resources queue work.
    log.gate.hold(at(100).start);
    workers.submit(sh_request(0, true)).unwrap();
    log.wait_for_reads(1);

    // Camera 1: blocks 0, 1, 2 (lead 0) and 3 (10 m) are mandatory; block 4
    // (24 m) is band and waits for the first outcome's headroom, which must
    // hold the four mandatory pairs in flight as well.
    let visible = VisibleCells::Culled(Vec::new());
    let mut drain = SharedDrain::default();
    for _ in 0..2 {
        drain.begin();
        lightmap
            .begin_drain(
                DemandFrame {
                    residency_set: &set,
                    camera_cell: 1,
                    path: PORTAL,
                    visible_cells: &visible,
                },
                &mut drain,
            )
            .unwrap();
        drain.admit().unwrap();
        lightmap.finish_drain(&drain, Some(reads.issuer())).unwrap();
        lightmap.take_drain_batch_for_renderer().unwrap();
        lightmap
            .apply_outcome(LightmapDrainOutcome {
                pool: headroom(5),
                ..LightmapDrainOutcome::default()
            })
            .unwrap();
    }
    for (cluster, mandatory) in [(1, false), (2, true), (3, false), (4, true)] {
        workers.submit(sh_request(cluster, mandatory)).unwrap();
    }
    log.gate.release(at(100).start);
    log.wait_for_reads(10);

    use StreamResource::{LightmapBlock as Lm, Sh};
    assert_eq!(
        log.spans(),
        vec![
            (Sh, at(100)),
            // Mandatory tier across both resources, ascending.
            (Sh, at(0)),
            (Lm, at(1)),
            (Lm, at(3)),
            (Lm, at(5)),
            (Sh, at(6)),
            (Lm, at(7)),
            // Optional tier across both resources, ascending.
            (Sh, at(2)),
            (Lm, at(9)),
            (Sh, at(10)),
        ]
    );
    let threads: HashSet<ThreadId> = log
        .reads
        .lock()
        .unwrap()
        .iter()
        .map(|(_, _, thread)| *thread)
        .collect();
    assert_eq!(threads.len(), 1, "one issuer thread performs every read");
    assert!(!threads.contains(&std::thread::current().id()));

    // SH's workers drop their handle before the owner joins the thread.
    drop(workers);
    drop(lightmap);
    drop(reads);
}

// AC 13 / P3: a reload while a pair's second half is being read discards the
// old generation's pair and releases its buffers; the new generation reads
// the block afresh and never sees an old completion.
#[test]
fn reload_discards_the_old_generations_pair_and_rereads_it() {
    let log = Arc::new(ReadLog::default());
    let source = TestBlockSource::with_log(corridor_blocks(64, true), Arc::clone(&log));
    let set = corridor_set();
    let visible = VisibleCells::Culled(Vec::new());
    let cpu = StageFrame::default();
    let mut level = LevelStreaming::default();
    level.install_lightmap(lightmap_session(&source));
    let old_ledger = Arc::downgrade(level.lightmap().unwrap().ledger());
    let old_generation = level.lightmap().unwrap().controller().generation();

    // Camera 6 demands blocks 5 and 6; block 6's shadowmask read is held.
    let held = source.spec(6).shadowmask.clone().unwrap().start;
    log.gate.hold(held);
    level
        .prepare_drains(&mut None, Some(&set), frame(&visible, 6, &cpu))
        .unwrap();
    log.wait_for_reads(4);

    level.retire(&mut None);
    level.install_lightmap(lightmap_session(&source));
    assert_ne!(
        level.lightmap().unwrap().controller().generation(),
        old_generation
    );
    for _ in 0..3 {
        renderer_installs_lightmap_batch(&mut level);
        level
            .prepare_drains(&mut None, Some(&set), frame(&visible, 6, &cpu))
            .unwrap();
        assert!(level.is_retiring(), "the held read has not returned");
    }
    assert_eq!(log.count(), 4, "no new reads while the old issuer retires");

    log.gate.release(held);
    let mut installed = Vec::new();
    wait_until("the reloaded level's pairs", || {
        installed.extend(renderer_installs_lightmap_batch(&mut level));
        level
            .prepare_drains(&mut None, Some(&set), frame(&visible, 6, &cpu))
            .unwrap();
        installed.len() == 2
    });
    installed.sort_unstable();
    assert_eq!(installed, vec![5, 6]);
    assert!(
        old_ledger.upgrade().is_none(),
        "the old route and its read buffers are gone"
    );
    let counters = level.lightmap().unwrap().controller().counters();
    assert_eq!(counters.stale_completions, 0);
    assert_eq!(counters.installs, 2);
    assert_eq!(log.reads_at(held), 2, "held once, then read afresh");
}

// AC 13 / P9 and P12: SH and lightmap blocks stream through one level issuer
// and both install; unload then releases the workers, the issuer, every
// manifest clone, and the sessions.
#[test]
fn sh_and_lightmap_stream_through_one_issuer_and_unload_releases_everything() {
    let (_temp, path) = sync_manifest_test_fixture::write_one_cluster_prl();
    let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    let manifest = Arc::clone(world.sh_stream_manifest().expect("id 50 selects streaming"));
    let sh_baseline = Arc::strong_count(&manifest);
    let mut sh = ShStreamingSession::from_snapshot(
        Arc::clone(&manifest),
        ShResidencySnapshot {
            effective_floor_bytes: 1024 * 1024,
            ..ShResidencySnapshot::default()
        },
        world.cell_visibility.as_ref(),
        sh_hints(&manifest),
    )
    .unwrap();
    sh.set_mode_for_test(ShStreamingMode::Async);
    let mut sh = Some(sh);

    let source = TestBlockSource::new(corridor_blocks(64, true));
    let set = corridor_set();
    let mut level = LevelStreaming::default();
    level.install_lightmap(lightmap_session(&source));
    let ledger = Arc::downgrade(level.lightmap().unwrap().ledger());

    // Cell 0 is SH's one cell and the corridor's camera 0.
    let visible = VisibleCells::Culled(vec![0]);
    let cpu = StageFrame::default();
    let mut installed = BTreeSet::new();
    wait_until("SH and lightmap installs", || {
        let batch = level
            .prepare_drains(&mut sh, Some(&set), frame(&visible, 0, &cpu))
            .unwrap();
        sh.as_mut().unwrap().accept_drain_for_test(&batch).unwrap();
        installed.extend(renderer_installs_lightmap_batch(&mut level));
        sh.as_ref().unwrap().all_targets_sampleable()
            && [0, 1, 2].iter().all(|block| installed.contains(block))
    });
    let controller = level.lightmap().unwrap().controller();
    assert_eq!(controller.phase(0), BlockPhase::Installed);
    assert!(Arc::strong_count(&manifest) > sh_baseline);
    assert!(Arc::strong_count(&source) > 1);

    level.retire(&mut sh);
    assert!(sh.is_none() && level.lightmap().is_none());
    wait_until("retired threads", || {
        level.poll_retirement();
        !level.is_retiring()
    });
    assert_eq!(
        Arc::strong_count(&manifest),
        sh_baseline,
        "SH manifest clones released"
    );
    assert_eq!(
        Arc::strong_count(&source),
        1,
        "lightmap source clones released"
    );
    assert!(ledger.upgrade().is_none(), "lightmap route released");
    assert!(
        level.drain_capacity() > 0,
        "the drain buffer is kept for reuse"
    );
}

/// A session factory for levels that stream no SH.
fn no_sh(
    _: Arc<ShStreamManifest>,
    _: ShStreamingMode,
    _: Option<Arc<ClusterHints>>,
) -> anyhow::Result<ShStreamingSession> {
    unreachable!("the level streams no SH")
}

// SH alone through the production drain step: the level issuer spawns over
// SH's route, the cluster installs, and retirement then joins every thread
// and releases every manifest clone.
#[test]
fn sh_only_level_streams_through_the_level_drain_step_and_retires_cleanly() {
    let (_temp, path) = sync_manifest_test_fixture::write_one_cluster_prl();
    let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    let manifest = Arc::clone(world.sh_stream_manifest().expect("id 50 selects streaming"));
    let baseline = Arc::strong_count(&manifest);
    let mut sh = Some(sh_session(
        &world,
        Arc::clone(&manifest),
        ShStreamingMode::Async,
    ));
    let mut level = LevelStreaming::default();
    let visible = VisibleCells::Culled(vec![0]);
    let cpu = StageFrame::default();

    wait_until("the SH install", || {
        let batch = level
            .prepare_drains(&mut sh, None, frame(&visible, 0, &cpu))
            .unwrap();
        sh.as_mut().unwrap().accept_drain_for_test(&batch).unwrap();
        sh.as_ref().unwrap().all_targets_sampleable()
    });
    assert!(level.lightmap().is_none());
    assert!(Arc::strong_count(&manifest) > baseline);

    level.retire(&mut sh);
    assert!(sh.is_none());
    wait_until("retired threads", || {
        level.poll_retirement();
        !level.is_retiring()
    });
    assert_eq!(
        Arc::strong_count(&manifest),
        baseline,
        "workers, issuer and session released"
    );
}

// AC 11 through the level owner the install path drives: the spawn eye's
// camera cell's mandatory set is resident in the renderer's pool before the
// first drain step, and the first frame keeps the preloaded session.
#[test]
fn spawn_preload_makes_the_spawn_set_resident_and_the_first_frame_keeps_the_session() {
    let prl = StreamedLightmapPrl::write();
    let world = prl.load();
    let view = LightmapLevelView::of(&world).unwrap();
    let mut model = manifest_pool_model(view.manifest);
    let wanted = WantedStreaming {
        sh: None,
        lightmap: Some(view),
        cluster_directory: world.cluster_directory(),
    };
    let mut level = LevelStreaming::default();
    let mut sh = None;
    assert!(level.ensure_sessions(&mut sh, wanted, no_sh).unwrap());

    // Spawn in cell 0: cells 0 and 1 are mandatory at the default lead, and
    // cell 2 is the prefetch band.
    level
        .install_spawn_lightmap(&world, eye_in_cell(0), |batch| {
            Ok(model_drain(&mut model, batch))
        })
        .unwrap();
    let session = level.lightmap().unwrap();
    for block in [0, 1] {
        assert_eq!(
            session.controller().phase(block),
            BlockPhase::Installed,
            "spawn block {block}"
        );
        assert!(model.is_resident(block), "spawn block {block} in the pool");
    }
    assert!(
        !model.is_resident(2),
        "band blocks wait for in-play prefetch"
    );
    assert!(session.settled());
    let generation = session.controller().generation();

    // The first frame: the same session and controller, no reset, nothing
    // mandatory read again.
    assert!(level.ensure_sessions(&mut sh, wanted, no_sh).unwrap());
    let cpu = StageFrame::default();
    let visible = VisibleCells::Culled(vec![0, 1]);
    level
        .prepare_drains(&mut sh, Some(view.residency_set), frame(&visible, 0, &cpu))
        .unwrap();
    let session = level.lightmap().unwrap();
    assert_eq!(session.controller().generation(), generation);
    let batch = session
        .parked_batch()
        .expect("the first frame drains after the preload");
    assert!(batch.target_reset.is_none(), "not a fresh controller");
    assert!(batch.ready.is_empty());
}

// Regression: an SH mode change mid-level replaced the lightmap session too,
// and the fresh controller's outcome validation then failed fatally against
// the renderer's pool, still full of the level's blocks.
#[test]
fn sh_mode_change_mid_level_keeps_the_lightmap_session_and_its_resident_blocks() {
    let prl = StreamedLightmapPrl::write();
    let world = prl.load();
    let view = LightmapLevelView::of(&world).unwrap();
    let mut model = manifest_pool_model(view.manifest);
    let (_temp, sh_path) = sync_manifest_test_fixture::write_one_cluster_prl();
    let sh_world = postretro_level_loader::load_prl(sh_path.to_str().unwrap()).unwrap();
    let sh_manifest = Arc::clone(
        sh_world
            .sh_stream_manifest()
            .expect("id 50 selects streaming"),
    );
    // The SH fixture is a level of its own, so its session decodes its own
    // hints rather than the lightmap level's.
    let make_sh = |manifest, mode, _: Option<Arc<ClusterHints>>| {
        anyhow::Ok(sh_session(&sh_world, manifest, mode))
    };
    let wanted = |mode| WantedStreaming {
        sh: Some((&sh_manifest, mode)),
        lightmap: Some(view),
        cluster_directory: world.cluster_directory(),
    };
    let mut level = LevelStreaming::default();
    let mut sh = None;
    assert!(
        level
            .ensure_sessions(&mut sh, wanted(ShStreamingMode::SyncProof), make_sh)
            .unwrap()
    );
    level
        .install_spawn_lightmap(&world, eye_in_cell(0), |batch| {
            Ok(model_drain(&mut model, batch))
        })
        .unwrap();
    let generation = level.lightmap().unwrap().controller().generation();

    // One frame as the app runs it: both drains, the renderer accepting all.
    let cpu = StageFrame::default();
    let visible = VisibleCells::Culled(vec![0]);
    let mut run_frame = |level: &mut LevelStreaming, sh: &mut Option<ShStreamingSession>| {
        let batch = level
            .prepare_drains(sh, Some(view.residency_set), frame(&visible, 0, &cpu))
            .unwrap();
        sh.as_mut().unwrap().accept_drain_for_test(&batch).unwrap();
        model_drains_lightmap(level, &mut model);
    };
    run_frame(&mut level, &mut sh);
    run_frame(&mut level, &mut sh);

    assert!(
        level
            .ensure_sessions(&mut sh, wanted(ShStreamingMode::Async), make_sh)
            .unwrap()
    );
    assert!(
        sh.as_ref()
            .unwrap()
            .is_for(&sh_manifest, ShStreamingMode::Async)
    );
    let lightmap = level.lightmap().expect("the lightmap session is kept");
    assert_eq!(lightmap.controller().generation(), generation);
    for block in [0, 1] {
        assert_eq!(lightmap.controller().phase(block), BlockPhase::Installed);
    }

    // SH now streams through the next issuer, and every lightmap outcome
    // still applies against the same pool.
    wait_until("the async SH install and the band block", || {
        run_frame(&mut level, &mut sh);
        sh.as_ref().unwrap().all_targets_sampleable()
            && level.lightmap().unwrap().controller().phase(2) == BlockPhase::Installed
    });
    assert_eq!(
        level.lightmap().unwrap().controller().generation(),
        generation
    );
}

// A renderer that fell back to the placeholder (device limits) does not
// stream the lightmap: the level drops its lightmap session with one warning
// rather than exiting, and later frames never recreate it.
#[test]
fn a_renderer_that_does_not_stream_the_lightmap_declines_it_for_the_level() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let prl = StreamedLightmapPrl::write();
    let world = prl.load();
    let wanted = WantedStreaming {
        sh: None,
        lightmap: LightmapLevelView::of(&world),
        cluster_directory: world.cluster_directory(),
    };
    let mut level = LevelStreaming::default();
    let mut sh = None;
    assert!(level.ensure_sessions(&mut sh, wanted, no_sh).unwrap());

    level
        .install_spawn_lightmap(&world, eye_in_cell(0), |_| {
            Err(LightmapResidencyDrainError::NotStreaming)
        })
        .unwrap();
    assert!(level.lightmap().is_none());
    for _ in 0..3 {
        assert!(
            !level.ensure_sessions(&mut sh, wanted, no_sh).unwrap(),
            "nothing streams"
        );
        assert!(level.lightmap().is_none());
    }
    capture.assert_logged_once(log::Level::Warn, "does not stream this level's lightmap");
}

// Regression: a renderer that stopped streaming the lightmap mid-level retired
// SH between SH's drain batch and its outcome, and applying that outcome
// without an SH session exited the game.
#[test]
fn a_mid_level_lightmap_decline_keeps_sh_for_its_pending_outcome() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let prl = StreamedLightmapPrl::write();
    let world = prl.load();
    let view = LightmapLevelView::of(&world).unwrap();
    let mut model = manifest_pool_model(view.manifest);
    let (_temp, sh_path) = sync_manifest_test_fixture::write_one_cluster_prl();
    let sh_world = postretro_level_loader::load_prl(sh_path.to_str().unwrap()).unwrap();
    let sh_manifest = Arc::clone(
        sh_world
            .sh_stream_manifest()
            .expect("id 50 selects streaming"),
    );
    let make_sh = |manifest, mode, _: Option<Arc<ClusterHints>>| {
        anyhow::Ok(sh_session(&sh_world, manifest, mode))
    };
    let wanted = WantedStreaming {
        sh: Some((&sh_manifest, ShStreamingMode::SyncProof)),
        lightmap: Some(view),
        cluster_directory: world.cluster_directory(),
    };
    let mut level = LevelStreaming::default();
    let mut sh = None;
    assert!(level.ensure_sessions(&mut sh, wanted, make_sh).unwrap());
    level
        .install_spawn_lightmap(&world, eye_in_cell(0), |batch| {
            Ok(model_drain(&mut model, batch))
        })
        .unwrap();

    // SH's batch is out when the renderer declines the lightmap.
    let cpu = StageFrame::default();
    let visible = VisibleCells::Culled(vec![0]);
    let sh_batch = level
        .prepare_drains(&mut sh, Some(view.residency_set), frame(&visible, 0, &cpu))
        .unwrap();
    level
        .lightmap_mut()
        .unwrap()
        .take_drain_batch_for_renderer()
        .expect("the frame parks a lightmap batch");
    level
        .apply_lightmap_drain(Err(LightmapResidencyDrainError::NotStreaming))
        .unwrap();
    assert!(
        level.lightmap().is_none(),
        "no lightmap work after the decline"
    );
    let parked = level.lightmap.as_ref().expect("the issuer keeps it parked");
    assert!(parked.is_declined());
    let controller = parked.controller();
    assert_eq!(controller.in_hand_bytes(), 0, "parking holds no pair");
    assert_eq!(controller.permits_in_use(), 0);
    sh.as_mut()
        .expect("SH outlives the decline")
        .accept_drain_for_test(&sh_batch)
        .unwrap();

    // Later frames keep SH's session (and its residency): the declined
    // lightmap session stays parked, and the lightmap never returns.
    let sh_generation = sh.as_ref().unwrap().generation();
    for _ in 0..3 {
        assert!(level.ensure_sessions(&mut sh, wanted, make_sh).unwrap());
        assert!(level.lightmap().is_none());
        assert_eq!(
            sh.as_ref().unwrap().generation(),
            sh_generation,
            "SH is kept, not recreated"
        );
        let sh_batch = level
            .prepare_drains(&mut sh, Some(view.residency_set), frame(&visible, 0, &cpu))
            .unwrap();
        sh.as_mut()
            .unwrap()
            .accept_drain_for_test(&sh_batch)
            .unwrap();
    }
    capture.assert_logged_once(log::Level::Warn, "does not stream this level's lightmap");
}

/// One frame of a lightmap-only level: its renderer drain either fails and
/// rolls back, or the pool model installs the batch.
fn lightmap_drain_frame(
    level: &mut LevelStreaming,
    wanted: WantedStreaming<'_>,
    model: &mut postretro_render_cpu::lightmap_pool::LightmapPoolModel,
    rolled_back: bool,
) {
    let mut sh = None;
    assert!(level.ensure_sessions(&mut sh, wanted, no_sh).unwrap());
    let cpu = StageFrame::default();
    let visible = VisibleCells::Culled(vec![0]);
    let residency_set = wanted.lightmap.unwrap().residency_set;
    level
        .prepare_drains(&mut sh, Some(residency_set), frame(&visible, 0, &cpu))
        .unwrap();
    let batch = level
        .lightmap_mut()
        .and_then(LightmapStreamingSession::take_drain_batch_for_renderer)
        .expect("every frame parks a batch");
    let result = if rolled_back {
        Err(LightmapResidencyDrainError::Upload("staging failed".into()))
    } else {
        Ok(model_drain(model, batch))
    };
    level.apply_lightmap_drain(result).unwrap();
}

// A rollback that recurs on every drain declines the lightmap for the level
// with one error, rather than re-reading every pair forever. A shorter run,
// or one a successful drain breaks, keeps streaming.
#[test]
fn a_rollback_recurring_on_every_drain_declines_the_lightmap_with_one_error() {
    use super::sessions::MAX_CONSECUTIVE_ROLLED_BACK_DRAINS;

    let capture = postretro_test_log_capture::LogCapture::start();
    let prl = StreamedLightmapPrl::write();
    let world = prl.load();
    let view = LightmapLevelView::of(&world).unwrap();
    let mut model = manifest_pool_model(view.manifest);
    let wanted = WantedStreaming {
        sh: None,
        lightmap: Some(view),
        cluster_directory: world.cluster_directory(),
    };
    let mut level = LevelStreaming::default();
    let short_run = MAX_CONSECUTIVE_ROLLED_BACK_DRAINS - 1;

    for _ in 0..short_run {
        lightmap_drain_frame(&mut level, wanted, &mut model, true);
    }
    lightmap_drain_frame(&mut level, wanted, &mut model, false);
    for _ in 0..short_run {
        lightmap_drain_frame(&mut level, wanted, &mut model, true);
    }
    assert!(level.lightmap().is_some(), "no run reached the bound");
    capture.assert_not_logged(log::Level::Error, "in a row failed");

    lightmap_drain_frame(&mut level, wanted, &mut model, true);
    assert!(level.lightmap().is_none(), "declined for the level");
    let mut sh = None;
    for _ in 0..3 {
        assert!(
            !level.ensure_sessions(&mut sh, wanted, no_sh).unwrap(),
            "nothing streams"
        );
    }
    capture.assert_logged_once(log::Level::Error, "in a row failed and were rolled back");
}

// A drain the renderer rolled back returns its pairs to the controller, which
// reads them again under a target reset; a drain-contract violation is fatal.
#[test]
fn a_rolled_back_renderer_drain_returns_its_pairs_and_a_contract_violation_is_fatal() {
    let source = TestBlockSource::new(corridor_blocks(64, true));
    let set = corridor_set();
    let visible = VisibleCells::Culled(Vec::new());
    let cpu = StageFrame::default();
    let mut level = LevelStreaming::default();
    level.install_lightmap(lightmap_session(&source));
    let mut sh = None;

    // Camera 6 demands blocks 5 and 6; frames run until a batch carries them.
    let mut drained = Vec::new();
    wait_until("a batch with ready pairs", || {
        level
            .prepare_drains(&mut sh, Some(&set), frame(&visible, 6, &cpu))
            .unwrap();
        let batch = level
            .lightmap_mut()
            .unwrap()
            .take_drain_batch_for_renderer()
            .expect("no outcome is outstanding");
        if batch.ready.is_empty() {
            level
                .apply_lightmap_drain(Ok(LightmapDrainOutcome {
                    pool: headroom(8),
                    ..LightmapDrainOutcome::default()
                }))
                .unwrap();
            return false;
        }
        drained = batch.ready.iter().map(|prepared| prepared.block).collect();
        true
    });

    level
        .apply_lightmap_drain(Err(LightmapResidencyDrainError::GpuCapacity {
            required_layers: 9,
            max_layers: 8,
        }))
        .unwrap();
    let controller = level.lightmap().unwrap().controller();
    for &block in &drained {
        assert_eq!(controller.phase(block), BlockPhase::Absent, "block {block}");
    }
    assert_eq!(controller.counters().aborted_drains, 1);

    level
        .prepare_drains(&mut sh, Some(&set), frame(&visible, 6, &cpu))
        .unwrap();
    let reset = level
        .lightmap()
        .unwrap()
        .parked_batch()
        .expect("the controller is not wedged");
    assert!(
        reset.target_reset.is_some(),
        "the renderer rolled the deltas back"
    );
    let mut installed = Vec::new();
    wait_until("the pairs read again", || {
        installed.extend(renderer_installs_lightmap_batch(&mut level));
        level
            .prepare_drains(&mut sh, Some(&set), frame(&visible, 6, &cpu))
            .unwrap();
        drained.iter().all(|block| installed.contains(block))
    });

    let error = level
        .apply_lightmap_drain(Err(LightmapResidencyDrainError::InvalidBatch(
            "a block past the table".into(),
        )))
        .unwrap_err();
    assert!(error.to_string().contains("renderer drain"), "{error}");
}

// Regression: a retiring issuer blocked delivering into a kept lightmap
// session's full old completion queue never finished, because retirement
// emptied that queue only once the issuer had.
#[test]
fn retirement_empties_the_old_completion_queue_its_issuer_is_blocked_on() {
    use crate::lightmap_streaming::route::{
        LightmapCompletion, LightmapReadResult, LightmapRouteLedger,
    };
    use crate::streaming::request::{ReadIdentity, ReadRanges, ReadRequest, ReadTier};

    let (completions, queue) = std::sync::mpsc::sync_channel(1);
    let issuer = std::thread::spawn(move || {
        for key in 0..3 {
            let request = ReadRequest {
                resource: StreamResource::LightmapBlock,
                key,
                tier: ReadTier::Mandatory,
                identity: ReadIdentity {
                    generation: 1,
                    content_tag: [0; 32],
                    item_hash: [0; 32],
                },
                ranges: ReadRanges::one(0..64),
            };
            let _ = completions.send(LightmapCompletion {
                request,
                result: LightmapReadResult::Cancelled,
            });
        }
    });
    let mut retirement = StreamingRetirement::default();
    retirement.add_issuer(issuer);
    retirement.drain_lightmap_completions(queue, Arc::new(LightmapRouteLedger::default()));
    wait_until("the blocked issuer to finish", || retirement.try_finish());
}
