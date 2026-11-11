//! Mid-travel poses through the binary: a mover's id-57 record carries its
//! real pose count and knots (path fractions, never ramped), and a PRL whose
//! movers bake mid poses loads, a ramped mover whose mid poses the ramp
//! squeezes out included.
//!
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

mod mover_bounce_support;
mod prl_build_support;

use postretro_level_format::mover_probe_channel::{MoverProbeChannelSection, PoseMark};
use postretro_level_loader::load_prl;

use mover_bounce_support::{Cache, build, channel, dev_map};

/// Every bounce entry's and depth record's marks of `mover` at its mid poses.
fn mid_marks(channel: &MoverProbeChannelSection, mover: u32) -> Vec<PoseMark> {
    let mut marks = Vec::new();
    for view in channel.bounce_entry_views() {
        if view.entry.mover_id != mover {
            continue;
        }
        let last = view.entry.pose_count() - 1;
        for local in 0..64 {
            marks.extend((1..last).map(|pose| view.entry.mark(pose, local)));
        }
    }
    for view in channel.depth_record_views() {
        if view.record.mover_id == mover {
            let last = view.record.pose_count() - 1;
            marks.extend_from_slice(&view.record.marks[1..last]);
        }
    }
    marks
}

// MP11 (bake and load half): the closet door with ramp (0.6, 0.9) and five
// poses. The ramp squeezes its mid poses at t 0.25 and 0.5 onto t′ 0, so
// compose never reads them, yet the bake bakes them and the PRL loads
// through the level loader. The record keeps the knots as path fractions.
#[test]
fn a_ramped_mover_whose_mid_poses_the_ramp_squeezes_out_builds_a_prl_that_loads() {
    let map = dev_map("mover-closet.map").replace(
        "\"name\" \"closet_door\"",
        "\"name\" \"closet_door\"\n\"light_ramp_start\" \"0.6\"\n\"light_ramp_end\" \"0.9\"\n\
         \"light_poses\" \"5\"",
    );
    let built = build(&map, "mover-closet-ramped", Cache::Cold, &[]);
    let channel = channel(&built.prl).expect("the door changes the closet");
    let [record] = channel.mover_records.as_slice() else {
        panic!("one door record");
    };
    assert_eq!(
        (record.ramp_start, record.ramp_end, record.knots.clone()),
        (0.6, 0.9, vec![0.0, 0.25, 0.5, 0.75, 1.0])
    );
    assert!(
        mid_marks(&channel, record.mover_id).contains(&PoseMark::Stored),
        "the squeezed-out mid poses still bake states"
    );
    let world = load_prl(built.prl.to_str().expect("UTF-8 path")).expect("the PRL loads");
    assert_eq!(
        world
            .mover_probe_channel()
            .expect("id 57 loads")
            .mover_records,
        channel.mover_records
    );
}

// Automatic placement on `enemy-movers`: every mover's record carries
// min(16, ⌈L / s⌉ + 1) knots at even arc length, mid poses store states,
// and the PRL loads.
#[test]
fn enemy_movers_bake_their_placed_poses_and_load() {
    let built = build(
        &dev_map("enemy-movers.map"),
        "enemy-movers",
        Cache::Cold,
        &[],
    );
    let channel = channel(&built.prl).expect("the movers change the probes");
    for record in &channel.mover_records {
        let n = record.knots.len();
        assert!(n > 2, "mover {} places mid poses", record.mover_id);
        for (k, &knot) in record.knots.iter().enumerate() {
            assert!(
                (knot - k as f32 / (n - 1) as f32).abs() < 1.0e-6,
                "mover {} knot {k}: {knot}",
                record.mover_id
            );
        }
        assert!(
            mid_marks(&channel, record.mover_id).contains(&PoseMark::Stored),
            "mover {}: a mid pose stores a state",
            record.mover_id
        );
    }
    load_prl(built.prl.to_str().expect("UTF-8 path")).expect("the PRL loads");
}
