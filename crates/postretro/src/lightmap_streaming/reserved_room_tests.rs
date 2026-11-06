//! Band reads against reserved room: the controller's half of band headroom (R29).
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_loader::LightmapDrainOutcome;
use postretro_render_cpu::lightmap_pool::LightmapPoolModel;

use super::*;
use crate::lightmap_streaming::test_fixtures::*;

/// A pool of `blocks` 64×64 blocks in one layer, over the corridor's blocks.
fn small_pool(rig: &Rig, edge: u32) -> LightmapPoolModel {
    let extents = (0..rig.source.block_count())
        .map(|block| {
            let spec = rig.source.spec(block);
            (u32::from(spec.width), u32::from(spec.height))
        })
        .collect();
    LightmapPoolModel::new(extents, 4, edge, 1).expect("the blocks fit a layer")
}

/// Run one frame's batch through `model` and back into the controller.
fn drain(rig: &mut Rig, model: &mut LightmapPoolModel) {
    let batch = rig.portal(0, &[]).unwrap();
    let outcome = model_drain(model, batch);
    rig.controller.apply_outcome(outcome).unwrap();
}

#[test]
fn band_headroom_tracks_reservations() {
    // Camera 0 demands 0, 1 and 2, band 3 and 4, in a layer of four blocks.
    let mut rig = Rig::corridor(None);
    let mut model = small_pool(&rig, 128);
    drain(&mut rig, &mut model);
    assert_eq!(rig.requested_blocks(), vec![0, 1, 2]);
    assert_eq!(
        rig.controller.pool_report().band_headroom_texels,
        64 * 64,
        "three reserved of four"
    );

    // Their pairs are in hand with room reserved: charged in the report and
    // not again here, so the one block of room left reads block 3.
    rig.requests.clear();
    drain(&mut rig, &mut model);
    assert_eq!(rig.requested_blocks(), vec![3], "one block of band room");
    rig.requests.clear();
    drain(&mut rig, &mut model);
    assert!(rig.requests.is_empty(), "no second block of room");
}

#[test]
fn a_pair_in_hand_with_no_room_reserved_still_comes_off_band_headroom() {
    let mut rig = Rig::corridor(None);
    rig.portal(0, &[]).unwrap();
    rig.requests.clear();
    // Two of the three mandatory pairs in hand have room; the third has none.
    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            room_reserved: vec![0, 1],
            pool: headroom(1),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    rig.portal(0, &[]).unwrap();
    assert!(rig.requests.is_empty(), "block 2 claims the room left");

    // Block 0 loses its room while in hand: it charges again, and the room it
    // gave back is in the report, so nothing changes.
    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            room_returned: vec![0],
            pool: headroom(2),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    rig.portal(0, &[]).unwrap();
    assert!(rig.requests.is_empty());
}

#[test]
fn a_band_pair_refused_for_reserved_room_is_read_again_when_the_room_returns() {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(1));
    rig.complete_all();
    for _ in 0..2 {
        let batch = rig.portal(0, &[]).unwrap();
        rig.install_all(&batch, headroom(1));
        rig.complete_all();
    }
    let batch = rig.portal(0, &[]).unwrap();
    assert_eq!(batch.ready[0].block, 3);
    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            refused: vec![3],
            pool: headroom(0),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    assert_eq!(rig.controller.phase(3), BlockPhase::Refused);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(0));
    rig.requests.clear();

    // The reservation that shut it out returns: one block of room more.
    let batch = rig.portal(0, &[]).unwrap();
    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            installed: batch.ready.iter().map(|prepared| prepared.block).collect(),
            room_returned: vec![5],
            pool: headroom(1),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    assert_eq!(rig.controller.phase(3), BlockPhase::Absent);
    rig.portal(0, &[]).unwrap();
    assert!(rig.requested_blocks().contains(&3), "read again");
}

#[test]
fn an_outcome_naming_room_past_the_level_is_refused_whole() {
    let mut rig = Rig::corridor(None);
    rig.portal(0, &[]).unwrap();
    let outcome = LightmapDrainOutcome {
        room_reserved: vec![99],
        ..LightmapDrainOutcome::default()
    };
    assert!(rig.controller.apply_outcome(outcome).is_err());
}
