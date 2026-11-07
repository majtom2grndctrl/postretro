//! Band reads against reserved room: the controller's half of band headroom.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_loader::{LightmapDrainOutcome, LightmapRefusal};
use postretro_render_cpu::lightmap_pool::{LightmapPoolModel, PerPool, mask_item};

use super::*;
use crate::lightmap_streaming::test_fixtures::*;

/// A pool of `blocks` 64×64 blocks in one layer, over the corridor's blocks.
fn small_pool(rig: &Rig, edge: u32) -> LightmapPoolModel {
    LightmapPoolModel::new(
        source_demand(&rig.source),
        edge,
        PerPool {
            lightmap: 1,
            mask: 2,
        },
    )
    .expect("the blocks fit a layer")
}

fn lightmap_room(items: Vec<u32>) -> PerPool<Vec<u32>> {
    PerPool {
        lightmap: items,
        mask: Vec::new(),
    }
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
        rig.controller.pool_report().lightmap.band_headroom_texels,
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
            room_reserved: lightmap_room(vec![0, 1]),
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
            room_returned: lightmap_room(vec![0]),
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
            refused: vec![LightmapRefusal {
                block: 3,
                short: PerPool::both(true),
            }],
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
            room_returned: lightmap_room(vec![5]),
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
        room_reserved: lightmap_room(vec![99]),
        ..LightmapDrainOutcome::default()
    };
    assert!(rig.controller.apply_outcome(outcome).is_err());
}

// Room for a plane the block does not store would later free more mask room
// than the block ever held.
#[test]
fn an_outcome_reserving_a_plane_the_block_does_not_store_is_refused_whole() {
    let mut blocks = corridor_blocks(64, true);
    blocks[0].planes = [true, false];
    let mut rig = Rig::new(blocks, corridor_set(), None);
    rig.portal(0, &[]).unwrap();
    let outcome = LightmapDrainOutcome {
        room_reserved: PerPool {
            lightmap: vec![0],
            mask: vec![mask_item(0, 0), mask_item(0, 1)],
        },
        ..LightmapDrainOutcome::default()
    };
    let error = rig.controller.apply_outcome(outcome).unwrap_err();
    assert!(
        error.to_string().contains("which the block does not store"),
        "{error}"
    );
}

/// Install camera 0's mandatory pairs, then have the mask pool turn band pair
/// 3 away with lightmap room to spare.
fn refuse_block_3_for_mask_room(rig: &mut Rig) {
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, one_pool_room(1, 2));
    rig.complete_all();
    for _ in 0..2 {
        let batch = rig.portal(0, &[]).unwrap();
        rig.install_all(&batch, one_pool_room(1, 2));
        rig.complete_all();
    }
    let batch = rig.portal(0, &[]).unwrap();
    assert_eq!(batch.ready[0].block, 3);
    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            refused: vec![LightmapRefusal {
                block: 3,
                short: PerPool {
                    lightmap: false,
                    mask: true,
                },
            }],
            pool: one_pool_room(1, 0),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    assert_eq!(rig.controller.phase(3), BlockPhase::Refused);
}

/// `lightmap` and `mask` slots of 64×64 texels of band headroom.
fn one_pool_room(lightmap: u64, mask: u64) -> postretro_level_loader::LightmapPoolReport {
    let mut room = headroom(0);
    room.lightmap.band_headroom_texels = lightmap * 64 * 64;
    room.mask.band_headroom_texels = mask * 64 * 64;
    room
}

// The mask pool turned the pair away, so room in the lightmap pool brings
// it back no sooner, however much of it arrives.
#[test]
fn refused_band_pair_waits_on_the_refusing_pool() {
    let mut rig = Rig::corridor(None);
    refuse_block_3_for_mask_room(&mut rig);

    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, one_pool_room(100, 0));
    assert_eq!(
        rig.controller.phase(3),
        BlockPhase::Refused,
        "lightmap room alone"
    );
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, one_pool_room(100, 1));
    assert_eq!(
        rig.controller.phase(3),
        BlockPhase::Refused,
        "one plane of mask room for a pair of two"
    );

    // The pool that refused it has room, with no more lightmap room than it
    // held at refusal: the pair is requestable.
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, one_pool_room(1, 2));
    assert_eq!(rig.controller.phase(3), BlockPhase::Absent);
}

// Band reads gate on each pool the pair needs: a mask pool out of room stops
// the pairs with planes and lets a pair that stores none through.
#[test]
fn a_full_mask_pool_holds_back_only_the_pairs_with_planes() {
    let mut blocks = corridor_blocks(64, true);
    blocks[4].shadowmask = None;
    blocks[4].planes = [false; 2];
    let mut rig = Rig::new(blocks, corridor_set(), None);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, one_pool_room(8, 0));
    rig.complete_all();
    for _ in 0..2 {
        let batch = rig.portal(0, &[]).unwrap();
        rig.install_all(&batch, one_pool_room(8, 0));
        rig.complete_all();
    }
    assert_eq!(rig.controller.phase(3), BlockPhase::Absent, "stores planes");
    assert_ne!(
        rig.controller.phase(4),
        BlockPhase::Absent,
        "asks nothing of the full pool"
    );
}

// A pair in hand is charged in the pool it holds no room in, plane by plane.
#[test]
fn a_pair_is_charged_only_the_halves_it_holds_no_room_for() {
    let mut rig = Rig::corridor(None);
    rig.portal(0, &[]).unwrap();
    rig.requests.clear();
    // Blocks 0, 1 and 2 are in hand. All three hold lightmap room, and only
    // block 0 holds room for both its planes.
    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            room_reserved: PerPool {
                lightmap: vec![0, 1, 2],
                mask: vec![mask_item(0, 0), mask_item(0, 1)],
            },
            pool: one_pool_room(1, 4),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    rig.portal(0, &[]).unwrap();
    assert!(
        rig.requests.is_empty(),
        "blocks 1 and 2 take the four plane slots of mask room"
    );

    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            pool: one_pool_room(1, 6),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    rig.portal(0, &[]).unwrap();
    assert_eq!(rig.requested_blocks(), vec![3], "two plane slots more");
}
