// Received link state through history and membership: stamps, idempotence, joins and resets.
// See: context/lib/networking.md §Current contract · context/lib/entity_model.md §7d

use glam::Vec3;
use postretro_entities::EntityRegistry;
use postretro_entities::provenance::DescriptorSpawnPath;
use postretro_net::wire::WireGhostLinkState;

use super::test_fixtures::{FRAME_SECONDS, install_ghosts, level, spawn_pawn};
use super::*;
use crate::ghost_links::clocks::{ClockSource, PoseStamps, PresentationClocks};
use crate::ghost_links::eligibility::CandidateView;
use crate::ghost_links::membership::ramp_step;
use crate::ghost_links::{GhostFrame, GhostLinks};

fn update(server_tick: u32, armed: bool, members: u8) -> GhostLinkUpdate {
    GhostLinkUpdate {
        server_tick,
        state: WireGhostLinkState::new(0, 20, armed, members),
    }
}

/// A connected client's view: link 0 over copy A, disarmed at spawn, and a
/// remote pawn presented inside it on the delay-buffered lane.
struct Client {
    registry: EntityRegistry,
    ghosts: GhostLinks,
    pawn: postretro_entities::EntityId,
}

impl Client {
    fn new() -> Self {
        let world = level(false);
        let mut registry = EntityRegistry::new();
        crate::ghost_links::spawn_loaded_ghost_links(&mut registry, &world);
        // The viewer, outside every volume: it never ghosts for itself.
        let viewer = spawn_pawn(
            &mut registry,
            Vec3::new(-50.0, 0.0, 0.0),
            DescriptorSpawnPath::PlayerSpawn,
        );
        registry.mark_local_player_pawn(viewer).unwrap();
        let pawn = spawn_pawn(
            &mut registry,
            Vec3::new(1.0, 0.0, 0.0),
            DescriptorSpawnPath::NetworkSlot,
        );
        let ghosts = install_ghosts(&world, &registry);
        Self {
            registry,
            ghosts,
            pawn,
        }
    }

    /// One presented frame with remotes at `render_tick`; the pawn's membership.
    fn frame(&mut self, render_tick: f64) -> f32 {
        self.ghosts.update(
            &self.registry,
            GhostFrame {
                clocks: PresentationClocks::compute(
                    ClockSource::Client {
                        render_server_tick: Some(render_tick),
                        estimated_server_tick: Some(render_tick + 6.0),
                        latest_snapshot_tick: None,
                    },
                    0.0,
                ),
                alpha: 0.0,
                elapsed_seconds: FRAME_SECONDS,
                view: CandidateView::Authoritative,
                stamps: PoseStamps::None,
            },
        );
        self.ghosts
            .members()
            .iter()
            .find(|member| member.link == 0 && member.entity == self.pawn)
            .map_or(0.0, |member| member.membership)
    }

    fn armed_at(&self, clock: f64) -> bool {
        self.ghosts.links[0].history.state_at(clock).armed
    }
}

// Every received state lands at its snapshot stamp, so a state that lasted only
// between two presented frames still presents at its own host time.
#[test]
fn received_link_states_land_at_their_snapshot_stamps_between_frames() {
    let mut client = Client::new();
    client.ghosts.apply_received(update(40, true, 0b1111));
    client.ghosts.apply_received(update(42, false, 0b1111));
    client.ghosts.apply_received(update(44, true, 0b0001));
    assert!(
        !client.armed_at(39.0),
        "spawn state before the first record"
    );
    assert!(
        client.armed_at(39.01),
        "ceil: motion toward 40 presents its state"
    );
    assert!(!client.armed_at(41.5));
    assert!(client.armed_at(44.0));
    assert_eq!(
        client.ghosts.links[0].history.state_at(44.0).members.bits(),
        0b0001
    );
}

// P8 at the history level: an always-on record repeating the current state at
// later stamps changes nothing a frame can read, so the presented value never
// moves as the RTT estimate, and with it the render tick, changes.
#[test]
fn repeated_identical_link_records_leave_every_presented_state_unchanged() {
    let mut client = Client::new();
    client.ghosts.apply_received(update(40, true, 0b1111));
    let clocks: Vec<f64> = (0..200).map(|step| 30.0 + f64::from(step) * 0.17).collect();
    let before: Vec<_> = clocks
        .iter()
        .map(|&clock| client.ghosts.links[0].history.state_at(clock))
        .collect();
    for stamp in [42, 44, 44, 46, 60] {
        client.ghosts.apply_received(update(stamp, true, 0b1111));
    }
    let after: Vec<_> = clocks
        .iter()
        .map(|&clock| client.ghosts.links[0].history.state_at(clock))
        .collect();
    assert_eq!(before, after);

    // And the presented membership is the same at any render tick past the arm.
    let mut membership = 0.0;
    for frame in 0..400 {
        membership = client.frame(41.0 + f64::from(frame % 7) * 0.9);
    }
    assert_eq!(membership, 1.0);
}

// P9: until a link's first record lands, a joiner presents its local spawn
// state; a disagreeing armed state then moves membership at the rate cap from
// 0, never cutting. P38 / AC41: dropping received state on demotion returns
// the link to spawn state through the same ramp.
#[test]
fn a_joiner_ramps_from_spawn_state_on_its_first_record_and_back_on_demotion() {
    let mut client = Client::new();
    let step = ramp_step(FRAME_SECONDS);
    for _ in 0..30 {
        assert_eq!(client.frame(100.0), 0.0, "local spawn state: disarmed");
    }
    client.ghosts.apply_received(update(101, true, 0b1111));
    let mut series = Vec::new();
    for frame in 0..200 {
        series.push(client.frame(100.5 + f64::from(frame) * 0.25));
    }
    assert!(
        series[0] <= f32::EPSILON,
        "a new ghost starts at 0, never cut in"
    );
    assert!(
        series
            .windows(2)
            .all(|pair| (pair[1] - pair[0]).abs() <= step + 1e-5)
    );
    assert_eq!(*series.last().unwrap(), 1.0);

    client.ghosts.reset_received_state();
    let mut series = vec![1.0];
    for _ in 0..200 {
        series.push(client.frame(200.0));
    }
    assert!(
        series
            .windows(2)
            .all(|pair| pair[1] <= pair[0] && pair[0] - pair[1] <= step + 1e-5)
    );
    assert!(
        series[1] > 0.0,
        "spawn state returns through the ramp, never a cut"
    );
    assert_eq!(*series.last().unwrap(), 0.0);
    assert!(!client.armed_at(1e9), "received state is gone");
}
