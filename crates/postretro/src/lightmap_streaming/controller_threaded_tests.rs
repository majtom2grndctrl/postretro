//! Lightmap controller proofs over the real issuer and held reads (AC 8; P2, P4, P5).
//! See: context/lib/testing_guide.md

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;

use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_loader::{LightmapDrainBatch, LightmapDrainOutcome};
use postretro_visibility::VisibleCells;

use super::*;
use crate::lightmap_streaming::route::{LightmapCompletion, LightmapRouteLedger, lightmap_route};
use crate::lightmap_streaming::test_fixtures::*;
use crate::streaming::issuer::{ReadIssuer, ReadRoutes};
use crate::streaming::shared_drain::SharedDrain;

/// The controller driven as the session drives it, with its route on a real
/// issuer thread. The renderer installs every pair it is handed.
struct ThreadedRig {
    source: Arc<TestBlockSource>,
    set: CellResidencySetSection,
    controller: LightmapResidencyController,
    ledger: Arc<LightmapRouteLedger>,
    completions: Receiver<LightmapCompletion>,
    issuer: Option<ReadIssuer>,
    handle: Option<JoinHandle<()>>,
    drain: SharedDrain,
    /// Every drain's installed blocks, in order.
    batches: Vec<Vec<u32>>,
}

impl ThreadedRig {
    fn new(blocks: Vec<BlockSpec>) -> Self {
        let source = TestBlockSource::new(blocks);
        let set = corridor_set();
        let controller = LightmapResidencyController::new(
            Arc::clone(&source) as Arc<dyn LightmapBlockSource>,
            &set,
            None,
        )
        .unwrap();
        let ledger = Arc::new(LightmapRouteLedger::default());
        let (route, completions) = lightmap_route(
            Arc::clone(&source) as Arc<dyn LightmapBlockSource>,
            Arc::clone(controller.target_bitset()),
            Arc::clone(&ledger),
        );
        let (issuer, handle) = ReadIssuer::spawn(
            ReadRoutes::default().with(StreamResource::LightmapBlock, Box::new(route)),
            MAX_LIGHTMAP_PERMITS,
        )
        .unwrap();
        Self {
            source,
            set,
            controller,
            ledger,
            completions,
            issuer: Some(issuer),
            handle: Some(handle),
            drain: SharedDrain::default(),
            batches: Vec::new(),
        }
    }

    fn frame(&mut self, camera_cell: u32) -> LightmapDrainBatch {
        let visible = VisibleCells::Culled(Vec::new());
        self.controller.update(DemandFrame {
            residency_set: &self.set,
            camera_cell,
            path: PORTAL,
            visible_cells: &visible,
        });
        while let Ok(completion) = self.completions.try_recv() {
            let bytes = completion.result.read_bytes();
            self.controller.admit_completion(completion).unwrap();
            self.ledger.release(bytes);
        }
        self.drain.begin();
        self.controller.offer_ready(&mut self.drain).unwrap();
        self.drain.admit().unwrap();
        let batch = self.controller.finish_drain(&self.drain).unwrap().unwrap();
        let installed: Vec<u32> = batch.ready.iter().map(|prepared| prepared.block).collect();
        self.controller
            .apply_outcome(LightmapDrainOutcome {
                installed: installed.clone(),
                pool: headroom(8),
                ..LightmapDrainOutcome::default()
            })
            .unwrap();
        self.batches.push(installed);
        let issuer = self.issuer.as_ref().unwrap();
        self.controller
            .take_requests(&mut |request| issuer.submit(request))
            .unwrap();
        batch
    }

    fn pump_until(&mut self, camera_cell: u32, what: &str, done: impl Fn(&Self) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done(self) {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            self.frame(camera_cell);
            std::thread::yield_now();
        }
    }

    fn all_installed(&self, blocks: std::ops::Range<u32>) -> bool {
        blocks
            .into_iter()
            .all(|block| self.controller.phase(block) == BlockPhase::Installed)
    }

    fn installs_of(&self, block: u32) -> usize {
        self.batches
            .iter()
            .flatten()
            .filter(|&&installed| installed == block)
            .count()
    }
}

impl Drop for ThreadedRig {
    fn drop(&mut self) {
        self.source.log.gate.release_all();
        drop(self.issuer.take());
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
    }
}

/// Holds block 6's later-read half, then releases it. Neither half becomes
/// sampleable while one is outstanding; the pair installs whole in the first
/// drain after the later half lands.
fn pair_installs_only_after_its_later_half(blocks: Vec<BlockSpec>, held: u64) {
    let mut rig = ThreadedRig::new(blocks);
    rig.source.log.gate.hold(held);
    rig.frame(6);
    rig.source.log.wait_for_reads(4);
    assert_eq!(
        rig.source.log.reads_at(held),
        1,
        "the held half is being read"
    );
    rig.pump_until(6, "block 5", |rig| rig.installs_of(5) == 1);
    for _ in 0..8 {
        rig.frame(6);
        assert_eq!(rig.controller.phase(6), BlockPhase::InFlight);
        assert_eq!(rig.installs_of(6), 0, "no half installs alone");
    }

    rig.source.log.gate.release(held);
    // The completion is admitted and offered in the same frame, so the block
    // is never seen ready between drains: in flight, then installed.
    rig.pump_until(6, "block 6", |rig| {
        let phase = rig.controller.phase(6);
        assert!(
            matches!(phase, BlockPhase::InFlight | BlockPhase::Installed),
            "{phase:?}"
        );
        phase == BlockPhase::Installed
    });
    assert_eq!(rig.installs_of(6), 1);
}

// AC 8 / P2 (CPU side): with the shadowmask read held back, neither half of
// the pair is ready; releasing it makes the pair ready in one drain.
#[test]
fn held_shadowmask_read_keeps_the_pair_unready_until_released() {
    let blocks = corridor_blocks(64, true);
    let held = blocks[6].shadowmask.clone().unwrap().start;
    pair_installs_only_after_its_later_half(blocks, held);
}

// P2, the other ordering: the shadowmask half lands first and the lightmap
// half is held.
#[test]
fn held_lightmap_read_keeps_the_pair_unready_when_the_shadowmask_lands_first() {
    let mut blocks = corridor_blocks(64, true);
    let shadow = blocks[6].lightmap.clone();
    let late = SHADOWMASK_BASE + 100 * FAR;
    blocks[6].lightmap = late..late + 64;
    blocks[6].shadowmask = Some(shadow);
    pair_installs_only_after_its_later_half(blocks, late);
}

// P4: a block leaving demand while its one-range read is in flight is
// discarded when the read lands, and its buffers are released.
#[test]
fn block_leaving_demand_mid_read_is_discarded_and_released() {
    let mut rig = ThreadedRig::new(corridor_blocks(64, false));
    let held = rig.source.spec(6).lightmap.start;
    rig.source.log.gate.hold(held);
    rig.frame(6);
    rig.source.log.wait_for_reads(2);
    rig.frame(0);
    assert_eq!(rig.controller.target(6), None);
    rig.source.log.gate.release(held);
    rig.pump_until(0, "the departed read", |rig| {
        rig.controller.counters().departed_reads >= 1
    });
    rig.pump_until(0, "camera 0's targets", |rig| rig.all_installed(0..5));
    assert_eq!(rig.controller.phase(6), BlockPhase::Absent);
    assert_eq!(rig.installs_of(6), 0);
    assert_eq!(rig.controller.in_hand_bytes(), 0, "every pair consumed");
    assert_eq!(rig.ledger.in_memory_bytes(), 0, "read buffers released");
}

// P4: a pair whose block leaves demand after its first half is read is
// cancelled whole before its second half, and its first half is released.
#[test]
fn pair_leaving_demand_between_halves_is_cancelled_whole() {
    let mut rig = ThreadedRig::new(corridor_blocks(64, true));
    let held = rig.source.spec(6).lightmap.start;
    rig.source.log.gate.hold(held);
    rig.frame(6);
    rig.source.log.wait_for_reads(2);
    rig.frame(0);
    rig.source.log.gate.release(held);
    rig.pump_until(0, "the cancelled pair", |rig| {
        rig.controller.phase(6) == BlockPhase::Absent
    });
    rig.pump_until(0, "camera 0's targets", |rig| rig.all_installed(0..5));
    let shadowmask = rig.source.spec(6).shadowmask.clone().unwrap();
    assert_eq!(rig.source.log.reads_at(shadowmask.start), 0);
    assert_eq!(rig.installs_of(6), 0);
    assert_eq!(rig.ledger.in_memory_bytes(), 0);
}

// P4: a block that leaves demand and re-enters it before its read lands
// installs exactly once, from the one read.
#[test]
fn block_reentering_demand_before_its_read_lands_installs_once() {
    let mut rig = ThreadedRig::new(corridor_blocks(64, false));
    let held = rig.source.spec(6).lightmap.start;
    rig.source.log.gate.hold(held);
    rig.frame(6);
    rig.source.log.wait_for_reads(2);
    rig.frame(0);
    rig.frame(6);
    rig.source.log.gate.release(held);
    rig.pump_until(6, "block 6", |rig| rig.installs_of(6) == 1);
    for _ in 0..8 {
        rig.frame(6);
    }
    assert_eq!(rig.installs_of(6), 1);
    assert_eq!(rig.source.log.reads_at(held), 1);
}

// P5: many frames of demand for one in-flight block, then more after it is
// delivered and installed, produce one read and one install.
#[test]
fn repeated_demand_for_an_in_flight_block_reads_and_installs_once() {
    let mut rig = ThreadedRig::new(corridor_blocks(64, true));
    let held = rig.source.spec(6).lightmap.start;
    rig.source.log.gate.hold(held);
    for _ in 0..10 {
        rig.frame(6);
    }
    rig.source.log.wait_for_reads(2);
    rig.source.log.gate.release(held);
    rig.pump_until(6, "block 6", |rig| rig.installs_of(6) == 1);
    for _ in 0..10 {
        rig.frame(6);
    }
    assert_eq!(rig.source.log.reads_at(held), 1);
    assert_eq!(rig.installs_of(6), 1);
    assert_eq!(
        rig.controller.counters().reads_requested,
        2,
        "blocks 5 and 6"
    );
}
