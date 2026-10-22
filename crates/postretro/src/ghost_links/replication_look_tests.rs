// Two-peer ghost-link look replication: script-set ramps, the host-time rebase, joins and styles.
// See: context/lib/networking.md §Current contract · context/lib/entity_model.md §7d

use glam::Vec3;
use postretro_foundation::{
    ARM_GHOST_LINK, DescriptorError, GhostStyle, GhostStyleDescriptor, GhostStyleRegistry,
    SET_GHOST_LINK_COLOR_FILTER, SET_GHOST_LINK_MEMBERS,
};
use postretro_net::wire::GHOST_LOOK_OPACITY;
use postretro_render_cpu::flash_clamp::GhostLookClamp;
use postretro_render_cpu::flash_limiter::{LimiterFrameInput, pack_limiter_frame};
use serde_json::json;

use super::test_fixtures::*;
use crate::ghost_links::GhostLook;
use crate::scripting_systems::ghost_render::presented_look;

/// One snapshot interval, in presented frames: snapshots go out every second
/// host tick.
const SNAPSHOT_INTERVAL_FRAMES: usize = 2 * FRAMES_PER_TICK;
const EPS: f32 = 1e-5;

/// Both pawns in copy A, inside link 0's volume.
fn pawns_in_copy_a() -> [Vec3; 2] {
    [Vec3::new(1.0, 0.0, 1.0), Vec3::new(-2.0, 0.0, -1.0)]
}

fn style(tint: [f32; 3], opacity: f32, saturation: f32) -> GhostStyle {
    GhostStyle {
        tint,
        opacity,
        saturation,
        audio: false,
        impact_audio: false,
    }
}

fn styles(echo: GhostStyle) -> GhostStyleRegistry {
    GhostStyleRegistry::from_declarations(
        "test",
        [Ok::<_, DescriptorError>(GhostStyleDescriptor {
            name: "echo".to_string(),
            style: echo,
        })],
    )
}

/// The first frame from `from` on where `reached` holds, or `None`.
fn first_frame(
    frames: &[PresentedFrame],
    from: usize,
    reached: impl Fn(&PresentedFrame) -> bool,
) -> Option<usize> {
    frames[from..]
        .iter()
        .position(reached)
        .map(|offset| from + offset)
}

fn close(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance
}

/// The ramp's change per presented frame for a 0.5 → 1 opacity ramp.
fn slope_per_frame(period_ms: f32, delta: f32) -> f32 {
    delta / (period_ms / 1000.0) * FRAME_SECONDS
}

// Host `arm`, `setMembers` and `setColorFilter`, run through the host's
// registered steps, reach a connected client at realistic lag, and the client's
// look ramp ends within one snapshot interval of the host's.
#[test]
fn host_arm_members_and_color_filter_reach_the_client_and_its_ramp_ends_with_the_hosts() {
    let mut run = Loopback::new(Scenario {
        links_armed: false,
        pawns: pawns_in_copy_a(),
        barrier: true,
    });
    run.run(40);
    let spawn = run.client_frames.last().unwrap().link_looks[0];
    let start = run.host_frames.len();
    run.host_step(0, ARM_GHOST_LINK, json!({}));
    run.host_step(0, SET_GHOST_LINK_MEMBERS, json!({ "movers": false }));
    run.host_step(
        0,
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "tint": [1.0, 0.4, 0.3], "opacity": 1.0, "periodMs": 500 }),
    );
    run.run(120);

    let ended = |frame: &PresentedFrame| {
        let look = frame.link_looks[0];
        close(look.opacity, 1.0, EPS) && close(look.tint[1], 0.4, EPS)
    };
    let host_end = first_frame(&run.host_frames, start, ended).expect("the host ramp ends");
    let client_end = first_frame(&run.client_frames, start, ended).expect("the client ramp ends");
    assert!(
        host_end.abs_diff(client_end) <= SNAPSHOT_INTERVAL_FRAMES,
        "client ramp ended at frame {client_end}, host at {host_end}"
    );
    let client_moved = first_frame(&run.client_frames, start, |frame| {
        frame.link_looks[0] != spawn
    })
    .unwrap();
    assert!(
        run.client_frames[client_moved].link_looks[0].opacity < 1.0 - EPS,
        "the client presents the ramp, not a jump to its target"
    );

    // Armed with movers off: both pawns ghost on the client, the barrier not.
    let last = run.client_frames.last().unwrap();
    for pawn in 0..2 {
        let member = last
            .member(0, pawn)
            .expect("the armed link ghosts the pawn");
        assert!(close(member.membership, 1.0, EPS));
        assert!(close(member.look.opacity, 1.0, EPS), "{:?}", member.look);
    }
    assert!(last.barrier_member(0).is_none(), "movers are off");
}

// The link record repeats its look ramp whenever it rides again,
// here each time `setMembers` toggles a category no member uses. With the ramp
// received, a client whose RTT estimate swings presents exactly what one with
// a steady estimate presents: the ramp rebased once and never again.
#[test]
fn repeated_identical_link_records_do_not_move_the_presented_look_as_rtt_changes() {
    let presented = |swing: bool| {
        let mut run = Loopback::new(Scenario {
            links_armed: true,
            pawns: pawns_in_copy_a(),
            barrier: true,
        });
        run.run(40);
        run.host_step(
            0,
            SET_GHOST_LINK_COLOR_FILTER,
            json!({ "opacity": 1.0, "saturation": 0.8, "periodMs": 1500 }),
        );
        run.run(12);
        assert!(
            run.client_received
                .last()
                .is_some_and(|update| update.state.look_fields & GHOST_LOOK_OPACITY != 0),
            "the ramp record has landed before the RTT swings"
        );
        let from = run.client_frames.len();
        let records = run.client_received.len();
        for tick in 0..100 {
            if swing {
                run.client_rtt_micros = Some(if tick % 2 == 0 { 40_000.0 } else { 300_000.0 });
            }
            if tick % 5 == 0 {
                run.host_step(
                    0,
                    SET_GHOST_LINK_MEMBERS,
                    json!({ "entities": tick % 10 != 0 }),
                );
            }
            run.run(1);
        }
        let repeats = run.client_received[records..]
            .iter()
            .filter(|update| update.state.look_fields & GHOST_LOOK_OPACITY != 0)
            .count();
        assert!(repeats >= 10, "the ramp rode {repeats} more records");
        run.client_frames[from..]
            .iter()
            .map(|frame| {
                let barrier = frame.barrier_member(0).map(|member| member.look);
                (frame.link_looks[0], barrier)
            })
            .collect::<Vec<(GhostLook, Option<GhostLook>)>>()
    };
    let steady = presented(false);
    let swinging = presented(true);
    assert!(steady.iter().any(|(look, _)| look.opacity < 1.0 - EPS));
    assert_eq!(steady, swinging);
}

// A client that joins mid-ramp, after the host armed, narrowed
// and recoloured the link, presents its local spawn state until the first
// record lands, then the host's current armed state, categories and the
// rebased current value of the ramp, which ends with the host's.
#[test]
fn a_late_joiner_mid_ramp_presents_the_hosts_current_state_and_rebased_ramp() {
    let mut run = Loopback::new(Scenario {
        links_armed: false,
        pawns: pawns_in_copy_a(),
        barrier: true,
    });
    run.host_step(0, ARM_GHOST_LINK, json!({}));
    run.host_step(0, SET_GHOST_LINK_MEMBERS, json!({ "movers": false }));
    run.host_step(
        0,
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "opacity": 1.0, "periodMs": 1500 }),
    );
    // Host-only for 0.6 s of the 1.5 s ramp: the client has seen nothing.
    run.run_host_only(36);
    let joined = run.host_frames.len();
    run.run(120);

    let client = &run.client_frames;
    let host = &run.host_frames[joined..];
    let spawn = client[0].link_looks[0];
    assert!(close(spawn.opacity, GhostStyle::FLOOR.opacity, EPS));
    let landed = first_frame(client, 0, |frame| frame.link_looks[0] != spawn)
        .expect("the first record lands");
    for frame in &client[..landed] {
        assert!(frame.members.is_empty(), "local spawn state: disarmed");
    }
    let first = client[landed].link_looks[0].opacity;
    assert!(
        first > GhostStyle::FLOOR.opacity + 0.1 && first < 1.0 - 0.1,
        "the joiner lands mid-ramp, not at the from-value: {first}"
    );
    let tolerance =
        slope_per_frame(1500.0, 1.0 - GhostStyle::FLOOR.opacity) * SNAPSHOT_INTERVAL_FRAMES as f32;
    for (index, (client, host)) in client.iter().zip(host).enumerate().skip(landed) {
        let (client, host) = (client.link_looks[0].opacity, host.link_looks[0].opacity);
        assert!(
            close(client, host, tolerance),
            "frame {index}: client {client} vs host {host}"
        );
    }
    let ended = |frame: &PresentedFrame| close(frame.link_looks[0].opacity, 1.0, EPS);
    let client_end = first_frame(client, 0, ended).expect("the client ramp ends");
    let host_end = first_frame(host, 0, ended).expect("the host ramp ends");
    assert!(client_end.abs_diff(host_end) <= SNAPSHOT_INTERVAL_FRAMES);

    let last = client.last().unwrap();
    assert!(last.member(0, 0).is_some() && last.member(0, 1).is_some());
    assert!(last.barrier_member(0).is_none(), "movers are off");
}

// A record landing before the presentation clock anchors (no RTT
// sample yet) applies armed state at once and defers the look ramp; once the
// clock anchors, the ramp rebases and presents the host's current value.
#[test]
fn a_record_before_the_presentation_clock_applies_armed_state_and_defers_the_ramp() {
    let mut run = Loopback::new(Scenario {
        links_armed: false,
        pawns: pawns_in_copy_a(),
        barrier: false,
    });
    run.client_rtt_micros = None;
    run.run(20);
    let style = run.client_frames.last().unwrap().link_looks[0];
    run.host_step(0, ARM_GHOST_LINK, json!({}));
    run.host_step(
        0,
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "opacity": 0.9, "periodMs": 2000 }),
    );
    run.run(40);
    let waiting = run.client_frames.last().unwrap();
    let member = waiting
        .member(0, 0)
        .expect("armed state applied before the clock");
    assert!(close(member.membership, 1.0, EPS));
    assert_eq!(waiting.link_looks[0], style, "the ramp waits for the clock");
    assert_eq!(member.look, style);

    run.client_rtt_micros = Some(120_000.0);
    let anchored = run.client_frames.len();
    run.run(10);
    let tolerance = slope_per_frame(2000.0, 0.4) * SNAPSHOT_INTERVAL_FRAMES as f32;
    for index in anchored + FRAMES_PER_TICK..run.client_frames.len() {
        let (client, host) = (
            run.client_frames[index].link_looks[0].opacity,
            run.host_frames[index].link_looks[0].opacity,
        );
        assert!(
            client > style.opacity + 0.05 && close(client, host, tolerance),
            "frame {index}: client {client} vs host {host}"
        );
    }
}

// A strobe then a mid-ramp `setColorFilter` on a host whose limiter
// is on, presented by a client whose limiter is off. The record carries the
// host's unclamped value; each peer runs the same unclamped ramp through its
// own limiter, which holds the strobe on the host and passes it on the client.
#[test]
fn a_mid_ramp_color_filter_carries_the_unclamped_host_value_through_each_peers_own_limiter() {
    let mut run = Loopback::new(Scenario {
        links_armed: true,
        pawns: [Vec3::new(-50.0, 0.0, 0.0), Vec3::new(50.0, 0.0, 0.0)],
        barrier: true,
    });
    run.run(40);
    let strobe_from = run.host_frames.len();
    for flip in 0..20 {
        let opacity = if flip % 2 == 0 { 1.0 } else { 0.0 };
        run.host_step(
            0,
            SET_GHOST_LINK_COLOR_FILTER,
            json!({ "opacity": opacity, "periodMs": 0 }),
        );
        run.run(3);
    }
    run.host_step(
        0,
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "opacity": 1.0, "periodMs": 1500 }),
    );
    run.run(45);
    let retarget_frame = run.host_frames.len();
    run.host_step(
        0,
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "opacity": 0.2, "periodMs": 1500 }),
    );
    run.run(150);

    // The record carries the host's unclamped value at the applying tick.
    let retarget = run
        .client_received
        .iter()
        .filter_map(|update| update.state.opacity)
        .find(|ramp| ramp.to == 0.2)
        .expect("the retarget reached the client");
    let unclamped = run.host_frames[retarget_frame].link_looks[0].opacity;
    assert!(
        retarget.from > 0.3 && retarget.from < 0.9,
        "{}",
        retarget.from
    );
    assert!(close(
        retarget.from,
        unclamped,
        slope_per_frame(1500.0, 0.5) * 2.0
    ));

    // Each peer limits the barrier ghost it presents through its own clamp.
    let limited = |frames: &[PresentedFrame], enabled: bool| {
        let mut clamp = GhostLookClamp::default();
        let frame_input = pack_limiter_frame(
            LimiterFrameInput {
                elapsed_seconds: FRAME_SECONDS,
            },
            enabled,
            false,
        );
        frames
            .iter()
            .map(|frame| {
                clamp.begin_frame(Some(&frame_input));
                let presented = frame.barrier_member(0).map(|member| {
                    let raw = member.look.opacity * member.membership;
                    (raw, presented_look(&mut clamp, member).opacity)
                });
                clamp.end_frame();
                presented
            })
            .collect::<Vec<_>>()
    };
    let host = limited(&run.host_frames[strobe_from..], true);
    let client = limited(&run.client_frames[strobe_from..], false);
    assert!(
        host.iter()
            .flatten()
            .any(|(raw, presented)| (raw - presented).abs() > 0.1),
        "the host's limiter holds its strobe"
    );
    assert!(
        client
            .iter()
            .flatten()
            .all(|(raw, presented)| raw == presented),
        "the client's limiter is off: it presents the unclamped ramp"
    );
    // After the strobe the two unclamped ramps agree within a snapshot
    // interval's progress. The client learns of the retarget a one-way trip
    // late and runs the old ramp until then, so that window is skipped; once
    // the record lands it presents the new ramp's rebased current value.
    let ramp_from = retarget_frame - strobe_from - 30 * FRAMES_PER_TICK;
    let retarget = retarget_frame - strobe_from;
    let in_flight = retarget..retarget + 8 * FRAMES_PER_TICK;
    let tolerance = slope_per_frame(1500.0, 0.8) * SNAPSHOT_INTERVAL_FRAMES as f32;
    for index in (ramp_from..host.len()).filter(|index| !in_flight.contains(index)) {
        let (Some((host, _)), Some((client, _))) = (host[index], client[index]) else {
            panic!("frame {index}: the barrier ghosts on both peers");
        };
        assert!(
            close(host, client, tolerance),
            "frame {index}: host {host} vs client {client}"
        );
    }
}

// Peers whose manifests declare `echo` differently each
// present their own style, and the mod digest admits both. An opacity-only
// `setColorFilter` gives both the host's opacity, and each keeps its own tint
// and saturation. A committed reload on either peer re-resolves its own style
// fields while the script-set opacity keeps its ramp.
#[test]
fn peers_present_their_own_style_and_the_hosts_opacity_through_reloads() {
    let host_echo = style([0.6, 0.8, 1.0], 0.45, 0.2);
    let client_echo = style([1.0, 0.5, 0.2], 0.7, 0.9);
    let digest = |echo| {
        let mut data = crate::ghost_links::opt_ins_test_fixtures::player_opted_in();
        data.ghost_styles = styles(echo);
        crate::mod_digest::mod_compatibility_digest_from_registry(&data)
    };
    assert_eq!(
        digest(host_echo),
        digest(client_echo),
        "the session admits both"
    );

    let mut run = Loopback::new(Scenario {
        links_armed: true,
        pawns: pawns_in_copy_a(),
        barrier: true,
    });
    run.bind_styles("echo", &styles(host_echo), &styles(client_echo));
    run.run(30);
    let look = |frames: &[PresentedFrame]| frames.last().unwrap().link_looks[0];
    assert_eq!(look(&run.host_frames), host_echo.look());
    assert_eq!(look(&run.client_frames), client_echo.look());

    run.host_step(
        0,
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "opacity": 0.9, "periodMs": 0 }),
    );
    run.run(30);
    assert!(
        run.client_received
            .iter()
            .all(|update| update.state.look_fields & !GHOST_LOOK_OPACITY == 0),
        "only opacity crosses the wire"
    );
    for (frames, own) in [
        (&run.host_frames, host_echo),
        (&run.client_frames, client_echo),
    ] {
        let presented = look(frames);
        assert_eq!(presented.opacity, 0.9);
        assert_eq!(
            (presented.tint, presented.saturation),
            (own.tint, own.saturation)
        );
        let barrier = frames.last().unwrap().barrier_member(0).unwrap().look;
        assert_eq!(barrier, presented, "the member presents its link's look");
    }

    // A slow fade, then a reload on each peer mid-ramp.
    run.host_step(
        0,
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "opacity": 0.1, "periodMs": 1500 }),
    );
    run.run(30);
    let host_reloaded = style([0.3, 0.3, 0.9], 0.05, 0.6);
    let client_reloaded = style([0.1, 0.2, 0.3], 0.95, 0.4);
    let before = [look(&run.host_frames), look(&run.client_frames)];
    run.reload_styles(Some(&styles(host_reloaded)), Some(&styles(client_reloaded)));
    run.run(1);
    for ((frames, own), before) in [
        (&run.host_frames, host_reloaded),
        (&run.client_frames, client_reloaded),
    ]
    .into_iter()
    .zip(before)
    {
        let presented = look(frames);
        assert_eq!(
            (presented.tint, presented.saturation),
            (own.tint, own.saturation)
        );
        assert!(
            presented.opacity < before.opacity && presented.opacity > 0.1,
            "the opacity ramp runs on: {} after {}",
            presented.opacity,
            before.opacity
        );
    }
    run.run(90);
    assert!(close(look(&run.host_frames).opacity, 0.1, EPS));
    assert!(close(look(&run.client_frames).opacity, 0.1, EPS));
}

// Look: a `setColorFilter` and the motion of the same host tick present
// on one frame per member, each at its own clock: the barrier at the mover
// clock, the slot pawn through the host seat's delay, and both pawns at the
// client's remote render tick.
#[test]
fn a_color_filter_presents_with_its_members_motion_at_each_members_clock() {
    let mut run = Loopback::new(Scenario {
        links_armed: true,
        pawns: pawns_in_copy_a(),
        barrier: true,
    });
    run.run(40);
    let start = run.host_frames.len();
    let style = run.host_frames.last().unwrap().link_looks[0];
    run.host_step(
        0,
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "opacity": 0.9, "periodMs": 0 }),
    );
    run.start_barrier();
    run.displace_pawn(0, Vec3::new(4.0, 0.0, 0.0));
    run.displace_pawn(1, Vec3::new(4.0, 0.0, 0.0));
    run.run(40);

    let recoloured =
        |look: Option<GhostLook>| look.is_some_and(|look| close(look.opacity, 0.9, EPS));
    // `mover_lead`: frames a mover's state may lead its drawn motion. A client
    // reads mover state at the estimated host tick while its registry blends
    // toward the re-simulated pose by alpha, so a mover's link state, its look
    // included, lands one frame before the first drawn motion: the gap,
    // measured and accepted as imperceptible (one frame, under 17 ms).
    let check = |frames: &[PresentedFrame], who: &str, pawns: &[usize], mover_lead: usize| {
        let rest = frames[start - 1].barrier.unwrap().1;
        let moved = first_frame(frames, start, |frame| {
            frame.barrier.unwrap().1.distance(rest) > 1e-4
        })
        .unwrap_or_else(|| panic!("{who}: the barrier moves"));
        let looked = first_frame(frames, start, |frame| {
            recoloured(frame.barrier_member(0).map(|member| member.look))
        })
        .unwrap_or_else(|| panic!("{who}: the barrier ghost recolours"));
        assert!(
            looked <= moved && moved - looked <= mover_lead,
            "{who}: barrier recoloured at frame {looked}, moved at {moved}"
        );
        for &pawn in pawns {
            let rest = frames[start - 1].pawns[pawn].unwrap().x;
            let moved = first_frame(frames, start, |frame| {
                frame.pawns[pawn].is_some_and(|at| at.x > rest + 1e-3)
            })
            .unwrap_or_else(|| panic!("{who}: pawn {pawn} moves"));
            let looked = first_frame(frames, start, |frame| {
                recoloured(frame.member(0, pawn).map(|member| member.look))
            })
            .unwrap_or_else(|| panic!("{who}: pawn {pawn}'s ghost recolours"));
            assert_eq!(looked, moved, "{who}: pawn {pawn} look and motion");
            assert!(
                frames[looked - 1]
                    .member(0, pawn)
                    .is_some_and(|member| member.look == style),
                "{who}: pawn {pawn} presents the old look up to its motion frame"
            );
        }
        moved
    };
    let host_barrier = check(&run.host_frames, "host seat", &[1], 0);
    let client_barrier = check(&run.client_frames, "client", &[0, 1], 1);
    assert!(client_barrier > host_barrier, "the client presents later");
}
