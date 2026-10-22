// Presentation clocks per role and lane, and how they meet the history's ceil rule.
// See: context/lib/entity_model.md §7d · context/lib/networking.md §Host-side remote-pawn presentation

use std::collections::BTreeSet;

use postretro_level_format::ghost_links::GhostMembers;

use super::*;
use crate::ghost_links::history::{LinkHistory, LinkState};
use crate::netcode::ReplicatedDescriptorClass;

fn assert_close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[test]
fn host_clocks_present_simulated_members_at_tick_minus_one_plus_alpha() {
    let clocks = PresentationClocks::compute(ClockSource::Host { tick: 101 }, 0.25);
    // The registry blends end-of-tick 99 (stamp 100) toward end-of-tick 100
    // (stamp 101) by alpha.
    assert_close(clocks.lane(MemberLane::Simulated), 100.25);
    assert_close(clocks.lane(MemberLane::Mover), 100.25);
}

#[test]
fn host_clocks_present_client_pawns_at_the_host_seat_delay() {
    let clocks = PresentationClocks::compute(ClockSource::Host { tick: 101 }, 0.25);
    // The buffer trails its newest sample (end of tick 100, stamp 101) by the
    // two-tick delay: one tick behind simulated members, in stamp units.
    assert_close(clocks.lane(MemberLane::Remote), 99.25);
    assert_close(
        clocks.lane(MemberLane::Remote),
        crate::netcode::host_presented_client_pawn_tick(101, 0.25),
    );
    assert_close(clocks.oldest(), 99.25);
}

#[test]
fn single_player_clock_is_the_completed_tick_count_minus_one_plus_alpha() {
    let clocks = PresentationClocks::compute(ClockSource::SinglePlayer { completed_ticks: 7 }, 0.5);
    for lane in [MemberLane::Simulated, MemberLane::Remote, MemberLane::Mover] {
        assert_close(clocks.lane(lane), 6.5);
    }
}

#[test]
fn client_clocks_read_the_sampled_render_tick_and_the_estimated_host_tick() {
    let synced = PresentationClocks::compute(
        ClockSource::Client {
            render_server_tick: Some(50.3),
            estimated_server_tick: Some(55.7),
            latest_snapshot_tick: Some(54),
        },
        // The render tick is already fractional; alpha never shifts it.
        0.9,
    );
    assert_close(synced.lane(MemberLane::Remote), 50.3);
    assert_close(synced.lane(MemberLane::Mover), 55.7);
    assert_close(synced.oldest(), 50.3);

    let unsynced = PresentationClocks::compute(
        ClockSource::Client {
            render_server_tick: None,
            estimated_server_tick: None,
            latest_snapshot_tick: Some(54),
        },
        0.9,
    );
    // Remotes hold their newest applied snapshot until time sync names a tick.
    assert_close(unsynced.lane(MemberLane::Remote), 54.0);
    assert_close(unsynced.lane(MemberLane::Mover), 54.0);
}

#[test]
fn arm_during_tick_n_presents_from_the_first_frame_blending_past_tick_n() {
    // Armed while the host ran the tick labelled 100: stamped 101.
    let mut history = LinkHistory::new(LinkState::new(false, GhostMembers::all()));
    history.push(101, LinkState::new(true, GhostMembers::all()));
    let armed_at = |alpha| {
        let clocks = PresentationClocks::compute(ClockSource::Host { tick: 101 }, alpha);
        history.state_at(clocks.lane(MemberLane::Simulated)).armed
    };
    assert!(!armed_at(0.0), "alpha 0 presents end-of-tick 99 exactly");
    assert!(
        armed_at(0.01),
        "motion toward the tick's end presents with it"
    );
    assert!(armed_at(0.99));
}

#[test]
fn member_lane_delays_slot_pawns_on_the_host_and_every_remote_on_a_client() {
    let mut registry = EntityRegistry::new();
    let pawn = |registry: &mut EntityRegistry, spawn_path| {
        let id = registry.spawn(Default::default());
        registry
            .set_component(
                id,
                DescriptorProvenance {
                    canonical_name: "player".into(),
                    owned_components: BTreeSet::new(),
                    map_overrides: BTreeSet::new(),
                    spawn_path,
                },
            )
            .unwrap();
        id
    };
    let host_pawn = pawn(&mut registry, DescriptorSpawnPath::PlayerSpawn);
    let slot_pawn = pawn(&mut registry, DescriptorSpawnPath::NetworkSlot);
    let players = GhostCategory::Players;
    let local = CandidateOrigin::Local;
    assert_eq!(
        member_lane(&registry, host_pawn, local, players),
        MemberLane::Simulated
    );
    assert_eq!(
        member_lane(&registry, slot_pawn, local, players),
        MemberLane::Remote
    );
    let copy = registry.spawn(Default::default());
    let replicated = CandidateOrigin::Replicated(ReplicatedDescriptorClass::Pawn("player"));
    assert_eq!(
        member_lane(&registry, copy, replicated, players),
        MemberLane::Remote
    );
    assert_eq!(
        member_lane(&registry, copy, local, GhostCategory::Movers),
        MemberLane::Mover
    );
}

// On a connected client, a link arm and a mover phase change applied on one
// host tick ride one snapshot, stamped S. The mover re-simulates from that
// phase to the estimated host tick on the frame the snapshot applies; the
// mover lane reads link state at that same estimated tick, already at or past
// S, so the arm presents with the motion on that frame and not before it.
// Checked on the lane clock and link history alone.
#[test]
fn mover_lane_reads_a_link_arm_on_the_frame_its_snapshot_applies() {
    let spawn = LinkState::new(false, GhostMembers::all());
    let mut history = LinkHistory::new(spawn);
    let snapshot_stamp = 120;
    let frame = |latest| {
        PresentationClocks::compute(
            ClockSource::Client {
                render_server_tick: Some(112.25),
                estimated_server_tick: Some(123.4),
                latest_snapshot_tick: latest,
            },
            0.25,
        )
    };
    // The frame before the snapshot lands: no arm, and the mover still plays
    // its old phase.
    assert!(
        !history
            .state_at(frame(Some(118)).lane(MemberLane::Mover))
            .armed
    );
    history.push(
        snapshot_stamp,
        LinkState {
            armed: true,
            ..spawn
        },
    );
    let applied = frame(Some(snapshot_stamp));
    assert!(applied.lane(MemberLane::Mover) >= f64::from(snapshot_stamp));
    assert!(history.state_at(applied.lane(MemberLane::Mover)).armed);
    // Remotes trail: a pawn presented at the render tick still reads disarmed.
    assert!(!history.state_at(applied.lane(MemberLane::Remote)).armed);
}

// Pose stamps cover delay-buffered members only: movers and simulated members
// read their lane clock, and a remote with no sampled pose falls back to it.
#[test]
fn pose_stamps_apply_to_the_remote_lane_only_and_fall_back_to_the_lane_clock() {
    let mut registry = EntityRegistry::new();
    let entity = registry.spawn(Default::default());
    let replication = crate::netcode::ClientReplication::default();
    for stamps in [PoseStamps::None, PoseStamps::Client(&replication)] {
        for lane in [MemberLane::Simulated, MemberLane::Remote, MemberLane::Mover] {
            assert_eq!(stamps.state_tick(entity, lane), None);
        }
    }
    let clocks = PresentationClocks::compute(ClockSource::Host { tick: 101 }, 0.25);
    assert_close(
        clocks.oldest_readable(),
        clocks.oldest() - POSE_STAMP_LAG_TICKS,
    );
}
