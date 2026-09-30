//! Preload proofs: the spawn cell's mandatory set resident before a first
//! frame (AC 11, P10), forced misses, and the settled query.
//! See: context/lib/testing_guide.md

use std::ops::Range;
use std::sync::Arc;

use postretro_level_format::lightmap::LightmapBlockPayload;
use postretro_level_loader::{LightmapBlockFileRanges, LightmapTarget, PrlLoadError};
use postretro_render_cpu::lightmap_pool::LightmapPoolModel;

use super::*;
use crate::lightmap_streaming::source::BlockSummary;
use crate::lightmap_streaming::test_fixtures::*;

/// Corridor camera cell 2 at the default 16 m lead: blocks 0–4 within lead,
/// block 5 in the band (28 m), block 6 pinned (cluster 3).
const SPAWN_CELL: u32 = 2;
const SPAWN_MANDATORY: [u32; 6] = [0, 1, 2, 3, 4, 6];
const SPAWN_BAND: u32 = 5;
/// Cell 7 has no charts and an empty baked range.
const EMPTY_CELL: u32 = 7;

fn pinned_corridor() -> (Rig, LightmapPoolModel) {
    let rig = Rig::corridor(Some(&corridor_hints(&[3], &[])));
    let model = pool_model(&rig.source);
    (rig, model)
}

/// Level install's preload: spawn-cell demand, one batch, the renderer's
/// outcome applied.
fn preload_spawn(
    rig: &mut Rig,
    model: &mut LightmapPoolModel,
    camera_cell: u32,
) -> LightmapPreloadReads {
    rig.controller.update_camera_set(&rig.set, camera_cell);
    let (batch, reads) = rig.controller.preload_batch(&[]).unwrap();
    let outcome = model_drain(model, batch);
    rig.controller.apply_outcome(outcome).unwrap();
    reads
}

// AC 11: before the first rendered frame, every block in the spawn cell's
// mandatory set (lead <= L, plus pins) is resident in the renderer's pool.
#[test]
fn spawn_preload_makes_the_spawn_cells_mandatory_set_and_pins_resident_before_any_frame() {
    let (mut rig, mut model) = pinned_corridor();

    let reads = preload_spawn(&mut rig, &mut model, SPAWN_CELL);

    assert_eq!(reads.pairs, SPAWN_MANDATORY.len() as u32);
    assert_eq!(reads.failed, 0);
    for block in SPAWN_MANDATORY {
        assert!(model.is_resident(block), "mandatory block {block} resident");
        assert_eq!(rig.controller.phase(block), BlockPhase::Installed);
    }
    assert!(
        !model.is_resident(SPAWN_BAND),
        "band blocks are left to in-play prefetch"
    );
    assert!(rig.controller.settled());
    // Both halves of each pair, read synchronously on this thread, with no
    // issuer involved.
    let reads = rig.source.log.reads.lock().unwrap().clone();
    assert_eq!(reads.len(), 2 * SPAWN_MANDATORY.len());
    assert!(
        reads
            .iter()
            .all(|(_, _, thread)| *thread == std::thread::current().id())
    );
    assert_eq!(rig.controller.permits_in_use(), 0);
    assert_eq!(rig.controller.in_hand_bytes(), 0);
}

// Gameplay never waits on a block after install: the first frame at the
// spawn cell re-reads nothing mandatory and sends no target change; only the
// band prefetch is requested.
#[test]
fn first_frame_after_spawn_preload_requests_only_band_prefetch() {
    let (mut rig, mut model) = pinned_corridor();
    preload_spawn(&mut rig, &mut model, SPAWN_CELL);

    let batch = rig
        .portal(SPAWN_CELL, &[1, 2, 3])
        .expect("the preload outcome was applied");
    assert!(batch.target_reset.is_none());
    assert!(batch.target_set.is_empty() && batch.target_remove.is_empty());
    assert!(batch.ready.is_empty());
    assert_eq!(rig.requested_blocks(), vec![SPAWN_BAND]);
    rig.controller
        .apply_outcome(model_drain(&mut model, batch))
        .unwrap();
    assert!(rig.controller.settled());
}

// P10: a spawn cell with an empty baked range completes install with nothing
// resident, and the first frame needs no outcome from the preload.
#[test]
fn spawn_preload_of_an_empty_baked_range_installs_nothing() {
    let mut rig = Rig::corridor(None);
    let mut model = pool_model(&rig.source);

    let reads = preload_spawn(&mut rig, &mut model, EMPTY_CELL);

    assert_eq!(reads, LightmapPreloadReads::default());
    assert!(model.resident_blocks().is_empty());
    assert_eq!(rig.source.log.count(), 0);
    assert!(rig.controller.settled(), "nothing is mandatory");
    assert!(
        rig.portal(EMPTY_CELL, &[]).is_some(),
        "the first frame drains without waiting"
    );
}

// The reset batch names the whole target set, so the renderer holds the band
// target too; the preload's ready pairs are exactly the mandatory set.
#[test]
fn spawn_preload_batch_resets_every_target_and_carries_only_mandatory_pairs() {
    let (mut rig, _) = pinned_corridor();
    rig.controller.update_camera_set(&rig.set, SPAWN_CELL);

    let (batch, _) = rig.controller.preload_batch(&[]).unwrap();

    let reset = batch.target_reset.expect("the generation's first batch");
    let band = reset
        .iter()
        .find(|target| target.block == SPAWN_BAND)
        .expect("band target");
    assert_eq!(band.class, LightmapBlockClass::Band);
    assert_eq!(
        reset.iter().find(|target| target.block == 6),
        Some(&LightmapTarget {
            block: 6,
            class: LightmapBlockClass::Mandatory,
            lead: 0,
        }),
        "pinned"
    );
    let mut ready: Vec<u32> = batch.ready.iter().map(|prepared| prepared.block).collect();
    ready.sort_unstable();
    assert_eq!(ready, SPAWN_MANDATORY);
}

// Capture preloads its view: mandatory plus visible blocks, while a forced
// miss stays targeted, unread and non-resident.
#[test]
fn view_preload_reads_visible_blocks_and_keeps_forced_misses_unread() {
    let mut rig = Rig::corridor(None);
    let mut model = pool_model(&rig.source);
    // Camera 0: blocks 0 and 1 at lead 0, block 2 at 8 m; cell 5 is drawn but
    // outside the baked set.
    rig.controller.update(DemandFrame {
        residency_set: &rig.set,
        camera_cell: 0,
        path: PORTAL,
        visible_cells: &postretro_visibility::VisibleCells::Culled(vec![0, 1, 5]),
    });

    let (batch, reads) = rig.controller.preload_batch(&[1]).unwrap();
    assert_eq!(reads.pairs, 3);
    let outcome = model_drain(&mut model, batch);
    rig.controller.apply_outcome(outcome).unwrap();

    for block in [0, 2, 5] {
        assert!(model.is_resident(block), "block {block} resident");
    }
    assert!(!model.is_resident(1), "the forced miss stays non-resident");
    assert_eq!(
        rig.source.log.reads_at(rig.source.spec(1).lightmap.start),
        0
    );
    assert!(
        rig.controller.target(1).is_some(),
        "the forced miss stays a target, so the renderer samples it as a miss"
    );
    assert!(!rig.controller.settled(), "a mandatory block is missing");
}

// Capture renders its full view synchronously, so on a non-portal path its
// preload still targets every drawn block as visible, not only the camera
// cell's baked set.
#[test]
fn capture_view_on_a_step_limit_path_preloads_every_drawn_block() {
    let mut rig = Rig::corridor(None);
    let mut model = pool_model(&rig.source);
    rig.controller.update_capture_view(DemandFrame {
        residency_set: &rig.set,
        camera_cell: 0,
        path: postretro_visibility::VisibilityPath::PortalStepLimitFallback {
            considered: 10,
            accepted: 3,
        },
        visible_cells: &postretro_visibility::VisibleCells::Culled(vec![0, 5, 6]),
    });
    for block in [5, 6] {
        assert_eq!(
            rig.controller.target(block).map(|target| target.class),
            Some(LightmapBlockClass::Visible),
            "drawn block {block}"
        );
    }

    let (batch, reads) = rig.controller.preload_batch(&[]).unwrap();
    assert_eq!(reads.pairs, 5, "blocks 0, 1 and 2, and the drawn 5 and 6");
    rig.controller
        .apply_outcome(model_drain(&mut model, batch))
        .unwrap();
    for block in [0, 1, 2, 5, 6] {
        assert!(model.is_resident(block), "block {block} resident");
    }
}

// A pair whose read fails stays non-resident; the spawn set reads as
// unsettled rather than settled over a hole.
#[test]
fn a_failed_preload_read_leaves_its_block_absent_and_the_set_unsettled() {
    let inner = TestBlockSource::new(corridor_blocks(64, true));
    let source = Arc::new(FailingSource {
        inner: Arc::clone(&inner),
        fail: 3,
    });
    let set = corridor_set();
    let mut controller = LightmapResidencyController::new(
        Arc::clone(&source) as Arc<dyn LightmapBlockSource>,
        &set,
        None,
    )
    .unwrap();
    let mut model = pool_model(&inner);
    controller.update_camera_set(&set, SPAWN_CELL);

    let (batch, reads) = controller.preload_batch(&[]).unwrap();
    assert_eq!((reads.pairs, reads.failed), (4, 1));
    controller
        .apply_outcome(model_drain(&mut model, batch))
        .unwrap();

    assert!(!model.is_resident(3));
    assert_eq!(controller.phase(3), BlockPhase::Failed);
    assert_eq!(controller.permits_in_use(), 0);
    assert!(!controller.settled());
}

#[test]
fn preload_after_the_first_drain_is_refused() {
    let (mut rig, _) = pinned_corridor();
    rig.portal(SPAWN_CELL, &[]).expect("first batch");

    assert!(matches!(
        rig.controller.preload_batch(&[]),
        Err(LightmapResidencyError::PreloadAfterStart)
    ));
}

// The settled query follows the camera cell: a move to a cell whose mandatory
// set is not yet resident unsettles it until those pairs install.
#[test]
fn settled_follows_the_camera_cells_mandatory_set_across_a_move() {
    let (mut rig, mut model) = pinned_corridor();
    preload_spawn(&mut rig, &mut model, SPAWN_CELL);
    assert!(rig.controller.settled());

    // Camera 4: block 5 at lead 0 is new; it was only band from cell 2.
    let batch = rig.portal(4, &[4]).unwrap();
    rig.controller
        .apply_outcome(model_drain(&mut model, batch))
        .unwrap();
    assert!(!rig.controller.settled());

    for _ in 0..4 {
        rig.complete_all();
        let batch = rig.portal(4, &[4]).unwrap();
        rig.controller
            .apply_outcome(model_drain(&mut model, batch))
            .unwrap();
    }
    assert!(rig.controller.settled());
    assert!(model.is_resident(5));
}

/// Fails `fail`'s pair read; every other block reads through `inner`.
#[derive(Debug)]
struct FailingSource {
    inner: Arc<TestBlockSource>,
    fail: u32,
}

impl LightmapBlockSource for FailingSource {
    fn block_count(&self) -> u32 {
        self.inner.block_count()
    }

    fn block_summary(&self, block: u32) -> Option<BlockSummary> {
        self.inner.block_summary(block)
    }

    fn block_alignment(&self) -> u32 {
        self.inner.block_alignment()
    }

    fn block_file_ranges(&self, block: u32) -> Result<LightmapBlockFileRanges, PrlLoadError> {
        self.inner.block_file_ranges(block)
    }

    fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        self.inner.read_file_span(range)
    }

    fn payload_from_pair_bytes(
        &self,
        block: u32,
        lightmap: Vec<u8>,
        shadowmask: Option<Vec<u8>>,
    ) -> Result<LightmapBlockPayload, PrlLoadError> {
        self.inner
            .payload_from_pair_bytes(block, lightmap, shadowmask)
    }

    fn content_tag(&self) -> [u8; 32] {
        self.inner.content_tag()
    }

    fn read_block_pair(&self, block: u32) -> Result<LightmapBlockPayload, PrlLoadError> {
        if block == self.fail {
            return Err(PrlLoadError::SectionValidation {
                section: "test source",
                message: format!("block {block} read failed"),
            });
        }
        self.inner.read_block_pair(block)
    }
}
