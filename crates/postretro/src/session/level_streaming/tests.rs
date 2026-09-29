//! Level-scope streaming proofs: one issuer for SH and lightmaps, reload and
//! unload lifetimes. See: context/lib/testing_guide.md · plan AC 13, 19; P3, P9, P12

use std::collections::{BTreeSet, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::thread::ThreadId;

use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_loader::{LightmapDrainOutcome, PrlLoadError, ShStreamingMode};
use postretro_renderer::ShResidencySnapshot;
use postretro_visibility::VisibleCells;

use super::*;
use crate::lightmap_streaming::controller::BlockPhase;
use crate::lightmap_streaming::source::LightmapBlockSource;
use crate::lightmap_streaming::test_fixtures::*;
use crate::session::sh_async_workers::{ShAsyncWorkers, ShWorkerSource};
use crate::session::sh_residency::sync_manifest_test_fixture;
use crate::sh_streaming::controller::ShClusterRequest;
use crate::streaming::request::StreamResource;

fn frame(visible_cells: &VisibleCells, camera_cell: usize) -> StreamingFrame<'_> {
    StreamingFrame {
        visible_cells,
        camera_cell: Some(camera_cell),
        path: PORTAL,
        monotonic_seconds: 0.0,
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
    // (24 m) is band and waits for the first outcome's headroom.
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
                pool: headroom(4),
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
    let mut level = LevelStreaming::default();
    level.install_lightmap(lightmap_session(&source));
    let old_ledger = Arc::downgrade(level.lightmap().unwrap().ledger());
    let old_generation = level.lightmap().unwrap().controller().generation();

    // Camera 6 demands blocks 5 and 6; block 6's shadowmask read is held.
    let held = source.spec(6).shadowmask.clone().unwrap().start;
    log.gate.hold(held);
    level
        .prepare_drains(&mut None, Some(&set), frame(&visible, 6))
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
            .prepare_drains(&mut None, Some(&set), frame(&visible, 6))
            .unwrap();
        assert!(level.is_retiring(), "the held read has not returned");
    }
    assert_eq!(log.count(), 4, "no new reads while the old issuer retires");

    log.gate.release(held);
    let mut installed = Vec::new();
    wait_until("the reloaded level's pairs", || {
        installed.extend(renderer_installs_lightmap_batch(&mut level));
        level
            .prepare_drains(&mut None, Some(&set), frame(&visible, 6))
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
    let mut installed = BTreeSet::new();
    wait_until("SH and lightmap installs", || {
        let batch = level
            .prepare_drains(&mut sh, Some(&set), frame(&visible, 0))
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
