//! Multi-block cell proofs: a cell's contiguous blocks share its demand and
//! install independently (P10, P12, P13, P15, P16).
//! See: context/lib/testing_guide.md

use postretro_level_loader::{LightmapBlockClass, LightmapTarget};
use postretro_render_cpu::lightmap_pool::LightmapPoolModel;
use postretro_visibility::VisibleCells;

use super::*;
use crate::lightmap_streaming::block_map::LevelBlockMap;
use crate::lightmap_streaming::demand::DemandFrame;
use crate::lightmap_streaming::test_fixtures::*;

/// Cell 0 owns blocks 0 and 1, cell 1 owns block 2, cell 2 owns none.
fn two_block_cell(half_bytes: u64) -> Vec<BlockSpec> {
    vec![
        BlockSpec::standard(0, 0, half_bytes, true),
        BlockSpec::standard(1, 0, half_bytes, true),
        BlockSpec::standard(2, 1, half_bytes, true),
    ]
}

/// Each camera cell's baked set is itself alone.
fn own_cell_set() -> postretro_level_format::cell_residency_set::CellResidencySetSection {
    residency_set(3, &[(0, 0, 0), (1, 1, 0), (2, 2, 0)], 32)
}

fn two_block_rig(half_bytes: u64) -> (Rig, LightmapPoolModel) {
    let rig = Rig::new(two_block_cell(half_bytes), own_cell_set(), None);
    let model = pool_model(&rig.source);
    (rig, model)
}

/// One portal frame whose batch the pool model drains and applies.
fn frame(rig: &mut Rig, model: &mut LightmapPoolModel, camera: u32, drawn: &[u32]) -> Vec<u32> {
    let batch = rig.portal(camera, drawn).expect("previous outcome applied");
    let ready = batch.ready.iter().map(|prepared| prepared.block).collect();
    let outcome = model_drain(model, batch);
    rig.controller.apply_outcome(outcome).unwrap();
    ready
}

fn target(block: u32, class: LightmapBlockClass) -> Option<LightmapTarget> {
    Some(LightmapTarget {
        block,
        class,
        lead: 0,
    })
}

// P10: a cell resolves to exactly its contiguous blocks; a cell without
// charts to none. Blocks of one cell interleaved with another's are rejected.
#[test]
fn block_map_resolves_each_cell_to_its_contiguous_blocks_and_rejects_interleaving() {
    let source = TestBlockSource::new(two_block_cell(64));
    let map = LevelBlockMap::build(source.as_ref(), 3, None).expect("contiguous blocks build");
    assert_eq!(map.blocks_of_cell(0), 0..2);
    assert_eq!(map.blocks_of_cell(1), 2..3);
    assert_eq!(
        map.blocks_of_cell(2),
        0..0,
        "a cell without charts owns none"
    );

    let interleaved = TestBlockSource::new(vec![
        BlockSpec::standard(0, 0, 64, true),
        BlockSpec::standard(1, 1, 64, true),
        BlockSpec::standard(2, 0, 64, true),
    ]);
    let error = LevelBlockMap::build(interleaved.as_ref(), 3, None)
        .expect_err("interleaved cell blocks must be rejected")
        .to_string();
    assert!(
        error.contains("cell 0's blocks are not contiguous"),
        "{error}"
    );
}

// A mandatory or visible cell demands every one of its blocks, at the cell's
// class and lead.
#[test]
fn a_cell_with_several_blocks_demands_all_of_them_at_its_class() {
    let (mut rig, mut model) = two_block_rig(64);
    frame(&mut rig, &mut model, 0, &[]);
    assert_eq!(
        rig.controller.target(0),
        target(0, LightmapBlockClass::Mandatory)
    );
    assert_eq!(
        rig.controller.target(1),
        target(1, LightmapBlockClass::Mandatory)
    );
    assert_eq!(rig.controller.target(2), None);
    assert_eq!(rig.requested_blocks(), vec![0, 1]);

    let (mut rig, mut model) = two_block_rig(64);
    frame(&mut rig, &mut model, 1, &[0]);
    assert_eq!(
        rig.controller.target(0),
        target(0, LightmapBlockClass::Visible)
    );
    assert_eq!(
        rig.controller.target(1),
        target(1, LightmapBlockClass::Visible)
    );
    assert_eq!(
        rig.controller.target(2),
        target(2, LightmapBlockClass::Mandatory)
    );
}

// P12, P16: two blocks of one cell ready in one drain with budget for one.
// The first (lower file offset) installs this drain and turns resident on
// its own; the second defers and is the only one of the pair missing.
#[test]
fn a_cells_blocks_install_one_by_one_and_only_the_missing_one_is_non_resident() {
    // 6 MiB pairs against the 8 MiB per-drain budget: one per drain.
    let (mut rig, mut model) = two_block_rig(3 * MIB);
    frame(&mut rig, &mut model, 0, &[]);
    rig.complete_all();

    assert_eq!(frame(&mut rig, &mut model, 0, &[]), vec![0]);
    assert!(model.is_resident(0), "the first block samples this frame");
    assert!(
        !model.is_resident(1),
        "only the missing block lacks residency"
    );
    assert_eq!(rig.controller.phase(1), BlockPhase::Ready);
    assert!(!rig.controller.settled());

    assert_eq!(frame(&mut rig, &mut model, 0, &[]), vec![1]);
    assert!(model.is_resident(0) && model.is_resident(1));
    assert!(rig.controller.settled());
}

// P13: a cell leaving demand with block 0 resident and block 1 mid-read
// releases both: 0 is freed at the next drain, and 1's completion is
// discarded with its buffers.
#[test]
fn a_cell_leaving_demand_releases_every_block_including_one_in_flight() {
    let (mut rig, mut model) = two_block_rig(64);
    frame(&mut rig, &mut model, 0, &[]);
    let requests = std::mem::take(&mut rig.requests);
    let (first, second): (Vec<_>, Vec<_>) = requests.into_iter().partition(|r| r.key == 0);
    for request in first {
        let completion = rig.source.completion(request);
        rig.controller.admit_completion(completion).unwrap();
    }
    assert_eq!(frame(&mut rig, &mut model, 0, &[]), vec![0]);
    assert!(model.is_resident(0));
    assert_eq!(rig.controller.phase(1), BlockPhase::InFlight);

    // Cell 2 owns no block, so nothing new is read while cell 0 releases.
    let batch = rig.portal(2, &[]).expect("previous outcome applied");
    assert!(
        batch.target_remove.contains(&0) && batch.target_remove.contains(&1),
        "both of the cell's blocks leave every target: {:?}",
        batch.target_remove
    );
    let outcome = model_drain(&mut model, batch);
    rig.controller.apply_outcome(outcome).unwrap();
    assert!(
        !model.is_resident(0),
        "the resident block is freed at the drain"
    );

    for request in second {
        let completion = rig.source.completion(request);
        rig.controller.admit_completion(completion).unwrap();
    }
    assert_eq!(rig.controller.counters().departed_reads, 1);
    assert_eq!(rig.controller.phase(1), BlockPhase::Absent);
    assert_eq!(rig.controller.in_hand_bytes(), 0);
    assert_eq!(rig.controller.permits_in_use(), 0);
}

// P15: level install and capture preload read every block of a multi-block
// mandatory cell before the first frame; settled holds only once all are
// resident.
#[test]
fn preload_installs_every_block_of_a_multi_block_mandatory_cell_before_settling() {
    for capture in [false, true] {
        let (mut rig, mut model) = two_block_rig(64);
        if capture {
            let visible = VisibleCells::Culled(vec![0]);
            rig.controller.update_capture_view(DemandFrame {
                residency_set: &rig.set,
                camera_cell: 0,
                path: PORTAL,
                visible_cells: &visible,
            });
        } else {
            rig.controller.update_camera_set(&rig.set, 0);
        }
        let (batch, reads) = rig.controller.preload_batch(&[]).unwrap();
        assert_eq!(reads.pairs, 2, "capture = {capture}");
        let outcome = model_drain(&mut model, batch);
        rig.controller.apply_outcome(outcome).unwrap();
        assert!(model.is_resident(0) && model.is_resident(1));
        assert!(rig.controller.settled());
    }

    // One block withheld: the cell is not settled.
    let (mut rig, mut model) = two_block_rig(64);
    rig.controller.update_camera_set(&rig.set, 0);
    let (batch, reads) = rig.controller.preload_batch(&[1]).unwrap();
    assert_eq!(reads.pairs, 1);
    let outcome = model_drain(&mut model, batch);
    rig.controller.apply_outcome(outcome).unwrap();
    assert!(model.is_resident(0) && !model.is_resident(1));
    assert!(!rig.controller.settled());
}
