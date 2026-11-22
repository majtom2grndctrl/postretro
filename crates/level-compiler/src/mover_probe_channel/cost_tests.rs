// The mover channel's price at its edges: one mover's states, with no bounce
// state at `light_bounces` 0 and no later-bounce buffer at fill 0.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Memory)

use super::*;

/// A cost over `valid` valid probes with one mover of `poses` poses.
fn one_mover(valid: u64, poses: u64, radiance: bool, posed_later: bool) -> MoverChannelCost {
    MoverChannelCost {
        movers: 1,
        radiance_movers: u64::from(radiance),
        valid_probes: valid,
        total_probes: valid,
        depth_cap: 32_768,
        max_mover_states_bytes: mover_states_bytes(valid, valid, poses, 0, radiance),
        posed_later,
        payload_cap: 1 << 20,
        ..MoverChannelCost::default()
    }
}

// No analysed mover prices the channel at zero: no states, no finish, no
// pose.
#[test]
fn no_analysed_mover_prices_nothing() {
    let cost = MoverChannelCost::default();
    assert_eq!(cost.states_bytes(), 0);
    assert_eq!(cost.raw_mover_bytes(), 0);
    assert_eq!(cost.finish_bytes(), 0);
    assert_eq!(cost.pose_bytes(1_000, 1_000), 0);
}

// A mover at `light_bounces` 0 stores no bounce state, so its states are
// priced at its depth records alone: a record per valid probe, its marks
// and both termini's depth tiles.
#[test]
fn a_mover_at_light_bounces_0_prices_no_bounce_state() {
    let (valid, poses) = (500u64, 2u64);
    let depth_only = one_mover(valid, poses, false, false).states_bytes();
    let with_bounce = one_mover(valid, poses, true, false).states_bytes();
    let depth =
        valid * (2 * DEPTH_TILE_BYTES + size_of::<DepthProbe>() as u64 + poses * MARK_BYTES);
    assert_eq!(depth_only, depth);
    assert_eq!(
        with_bounce - depth_only,
        valid * (2 * STATE_TILE_BYTES + size_of::<BounceProbe>() as u64 + poses * MARK_BYTES)
    );
}

// At fill 0 no pose re-bakes a later bounce, and the bake allocates none of
// their buffers: the pose prices none.
#[test]
fn fill_0_prices_no_later_bounce_buffer() {
    let patches = 10_000u64;
    let side = size_of::<[f32; 3]>() as u64;
    let with = one_mover(500, 2, true, true).pose_bytes(patches, 0);
    let without = one_mover(500, 2, true, false).pose_bytes(patches, 0);
    assert_eq!(with - without, 3 * side * patches + 64 * side);
}
