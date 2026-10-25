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
        let world = level(false, false);
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
    client
        .ghosts
        .apply_received(update(40, true, 0b1111), Some(0.0));
    client
        .ghosts
        .apply_received(update(42, false, 0b1111), Some(0.0));
    client
        .ghosts
        .apply_received(update(44, true, 0b0001), Some(0.0));
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

// In a client's received link history, an always-on record repeating the
// current state at later stamps changes nothing a frame can read, so the
// presented value never moves as the RTT estimate, and with it the render
// tick, changes.
#[test]
fn repeated_identical_link_records_leave_every_presented_state_unchanged() {
    let mut client = Client::new();
    client
        .ghosts
        .apply_received(update(40, true, 0b1111), Some(0.0));
    let clocks: Vec<f64> = (0..200).map(|step| 30.0 + f64::from(step) * 0.17).collect();
    let before: Vec<_> = clocks
        .iter()
        .map(|&clock| client.ghosts.links[0].history.state_at(clock))
        .collect();
    for stamp in [42, 44, 44, 46, 60] {
        client
            .ghosts
            .apply_received(update(stamp, true, 0b1111), Some(0.0));
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

// Until a link's first record lands, a joiner presents its local spawn
// state; a disagreeing armed state then moves membership at the rate cap from
// 0, never cutting. Dropping received state on demotion returns
// the link to spawn state through the same ramp.
#[test]
fn a_joiner_ramps_from_spawn_state_on_its_first_record_and_back_on_demotion() {
    let mut client = Client::new();
    let step = ramp_step(FRAME_SECONDS);
    for _ in 0..30 {
        assert_eq!(client.frame(100.0), 0.0, "local spawn state: disarmed");
    }
    client
        .ghosts
        .apply_received(update(101, true, 0b1111), Some(0.0));
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

const LOOK_START: f64 = 3.0;

/// A record for link 0 carrying an opacity ramp to `to` starting at host
/// script time `start`, and optionally a tint ramp.
fn look_update(server_tick: u32, start: f64, to: f32, tint: bool) -> GhostLinkUpdate {
    use postretro_net::wire::{
        GHOST_LOOK_OPACITY, GHOST_LOOK_TINT, WireGhostScalarRamp, WireGhostTintRamp,
    };
    let mut state = WireGhostLinkState {
        look_fields: GHOST_LOOK_OPACITY,
        opacity: Some(WireGhostScalarRamp {
            from: 0.5,
            to,
            start_time: start,
            duration_ms: 1000.0,
        }),
        ..WireGhostLinkState::new(0, 20, true, 0b1111)
    };
    if tint {
        state.look_fields |= GHOST_LOOK_TINT;
        state.tint = Some(WireGhostTintRamp {
            from: [1.0; 3],
            to: [1.0, 0.4, 0.3],
            start_time: LOOK_START,
            duration_ms: 500.0,
        });
    }
    GhostLinkUpdate { server_tick, state }
}

fn latest_filter(client: &Client) -> postretro_foundation::GhostColorFilter {
    client.ghosts.links[0].history.latest().filter
}

// A received ramp rebases once, when its host ramp changes. The record
// repeats every snapshot while the host-clock offset drifts with the RTT
// estimate; the presented ramp never moves. A later opacity-only change
// rebases opacity alone, and the tint keeps its first rebase.
#[test]
fn a_received_look_ramp_rebases_once_and_repeats_never_move_it_as_the_offset_drifts() {
    let mut client = Client::new();
    client
        .ghosts
        .apply_received(look_update(40, LOOK_START, 1.0, true), Some(0.25));
    let first = latest_filter(&client);
    assert_eq!(first.opacity.unwrap().start_time, LOOK_START + 0.25);
    assert_eq!(first.tint.unwrap().start_time, LOOK_START + 0.25);
    for (stamp, offset) in [(42, 0.4), (44, -0.1), (46, 0.0)] {
        client
            .ghosts
            .apply_received(look_update(stamp, LOOK_START, 1.0, true), Some(offset));
        assert_eq!(latest_filter(&client), first, "stamp {stamp}");
    }

    client
        .ghosts
        .apply_received(look_update(48, 3.5, 0.1, true), Some(0.6));
    let retargeted = latest_filter(&client);
    assert_eq!(retargeted.opacity.unwrap().start_time, 3.5 + 0.6);
    assert_eq!(
        retargeted.tint, first.tint,
        "an unchanged field keeps its rebase"
    );
}

// A record landing before the presentation clock anchors applies its
// armed state and categories now and defers the look ramp; once the clock
// anchors the ramp rebases and is in force from the carrying record's stamp,
// through every later change.
#[test]
fn a_look_ramp_received_before_the_clock_anchors_defers_then_lands_at_its_stamp() {
    let mut client = Client::new();
    client
        .ghosts
        .intake_received([look_update(50, LOOK_START, 1.0, false)].into_iter(), None);
    assert!(client.armed_at(50.0), "armed state applies at once");
    assert!(
        latest_filter(&client).opacity.is_none(),
        "the ramp waits for the clock: the style presents"
    );
    // The disarm a later record carries lands too; the ramp still waits.
    client.ghosts.intake_received(
        [GhostLinkUpdate {
            state: WireGhostLinkState {
                armed: false,
                ..look_update(52, LOOK_START, 1.0, false).state
            },
            server_tick: 52,
        }]
        .into_iter(),
        None,
    );
    assert!(!client.armed_at(52.0));

    client
        .ghosts
        .intake_received(std::iter::empty(), Some(0.125));
    let history = &client.ghosts.links[0].history;
    let rebased = LOOK_START + 0.125;
    assert!(history.state_at(49.0).filter.opacity.is_none());
    for clock in [49.5, 50.0, 51.0, 52.0, 60.0] {
        let opacity = history.state_at(clock).filter.opacity;
        assert_eq!(
            opacity.map(|ramp| ramp.start_time),
            Some(rebased),
            "{clock}"
        );
    }
    assert!(!history.state_at(52.0).armed, "the amend keeps later state");
    // Released once: a later frame finds nothing waiting.
    client.ghosts.intake_received(std::iter::empty(), Some(9.0));
    assert_eq!(latest_filter(&client).opacity.unwrap().start_time, rebased);
}

// Demotion drops received look ramps with the rest of received state, and
// the link presents its local style again.
#[test]
fn demotion_drops_received_look_ramps_and_the_link_presents_its_style() {
    let mut client = Client::new();
    client.ghosts.set_look_time(LOOK_START + 10.0);
    let style = client.ghosts.link_look(0);
    client
        .ghosts
        .apply_received(look_update(40, LOOK_START, 1.0, true), Some(0.0));
    assert_eq!(client.ghosts.link_look(0).opacity, 1.0);
    client.ghosts.reset_received_state();
    assert_eq!(client.ghosts.link_look(0), style);
    // A record after the reset rebases afresh rather than matching the
    // dropped one.
    client
        .ghosts
        .apply_received(look_update(60, LOOK_START, 1.0, true), Some(2.0));
    assert_eq!(
        latest_filter(&client).opacity.unwrap().start_time,
        LOOK_START + 2.0
    );
}

// Regression: a received look value outside 0..=1 reached presentation as is.
#[test]
fn received_out_of_range_look_values_clamp_into_range_and_warn_once_per_record() {
    use postretro_net::wire::{
        GHOST_LOOK_OPACITY, GHOST_LOOK_TINT, WireGhostScalarRamp, WireGhostTintRamp,
    };
    let capture = postretro_test_log_capture::LogCapture::start();
    let mut client = Client::new();
    let state = WireGhostLinkState {
        look_fields: GHOST_LOOK_TINT | GHOST_LOOK_OPACITY,
        tint: Some(WireGhostTintRamp {
            from: [1.0, 1.0, 1.0],
            to: [2.0, -1.0, 0.5],
            start_time: 0.0,
            duration_ms: 0.0,
        }),
        opacity: Some(WireGhostScalarRamp {
            from: 0.5,
            to: 3.0,
            start_time: 0.0,
            duration_ms: 0.0,
        }),
        ..WireGhostLinkState::new(0, 20, true, 0b1111)
    };
    // The always-on record repeats unchanged on the next snapshot.
    for server_tick in [40, 42] {
        client
            .ghosts
            .apply_received(GhostLinkUpdate { server_tick, state }, Some(0.0));
    }
    let filter = client.ghosts.links[0].history.state_at(44.0).filter;
    assert_eq!(filter.tint.map(|ramp| ramp.to), Some([1.0, 0.0, 0.5]));
    assert_eq!(filter.opacity.map(|ramp| ramp.to), Some(1.0));
    capture.assert_logged_once(log::Level::Warn, "outside 0..=1; clamped");
}

// Regression: the drain ran inside the membership update, after ghost-voice
// admission had already read the previous frame's history. A client's
// received link states now enter history once per frame, in the audio stage
// before ghost-voice admission, and the membership update reads that history.
#[test]
fn received_link_state_drains_before_ghost_voice_admission_and_not_in_membership_update() {
    let frame_loop = include_str!("../frame_loop/mod.rs");
    let redraw = frame_loop
        .split("pub(crate) fn redraw(")
        .nth(1)
        .expect("the frame loop keeps `redraw`");
    let drain = redraw
        .find("app.drain_received_ghost_link_state();")
        .expect("redraw drains received link state");
    let voices = redraw
        .find("app.plan_ghost_voices(")
        .expect("redraw plans ghost voices");
    assert!(
        drain < voices,
        "received link state enters history before ghost-voice admission reads it"
    );
    assert_eq!(
        frame_loop
            .matches("drain_received_ghost_link_state(")
            .count(),
        1,
        "the frame loop drains received link state once per frame"
    );

    let frame = include_str!("frame.rs");
    let membership_update = frame
        .split("fn update_ghost_links(")
        .nth(1)
        .expect("ghost_links keeps `update_ghost_links`")
        .split("\n    }\n")
        .next()
        .unwrap();
    assert!(
        !membership_update.contains("drain_received_ghost_link_state("),
        "the membership update reads history the audio stage already filled"
    );
}
