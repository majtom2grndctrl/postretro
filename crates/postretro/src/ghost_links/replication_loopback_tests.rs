// Two-peer ghost-link loopback scenarios: the swap, pose stamps, and one-tick arms.
// See: context/lib/networking.md §Current contract · context/lib/entity_model.md §7d

use glam::Vec3;

use super::test_fixtures::*;
use crate::ghost_links::membership::ramp_step;

/// A membership step's float slack.
const EPS: f32 = 1e-5;

/// Copy A at x = 0, copy B at x = 100.
fn in_copy_b(position: Vec3) -> bool {
    position.x > COPY_OFFSET * 0.5
}

/// Every ghost on every frame comes from a source pose inside its own link's
/// volume and draws at that pose's mapping, so nothing ever draws between the
/// copies (P1).
fn assert_every_ghost_maps_an_in_volume_pose(run: &Loopback, frames: &[PresentedFrame], who: &str) {
    for (index, frame) in frames.iter().enumerate() {
        for member in &frame.members {
            assert!(
                inside(run.link_volume(member.link), member.source.position),
                "{who} frame {index}: link {} ghost of {:?} sourced outside its volume at {:?}",
                member.link,
                member.entity,
                member.source.position
            );
            let mapped = member.mapping.point(member.source.position);
            assert!((mapped - member.mapped.position).length() < 1e-4);
            assert!(
                inside(copy_volume(0.0), mapped) || inside(copy_volume(COPY_OFFSET), mapped),
                "{who} frame {index}: a ghost drew between the copies at {mapped:?}"
            );
        }
    }
}

fn first_frame_in_copy_b(frames: &[PresentedFrame], pawn: usize, from_b: bool) -> usize {
    frames
        .iter()
        .position(|frame| frame.pawns[pawn].is_some_and(|at| in_copy_b(at) != from_b))
        .expect("the pawn's presented pose crossed copies")
}

/// The membership of `link`'s ghost of `pawn` on each frame, 0 when absent.
fn memberships(frames: &[PresentedFrame], link: u16, pawn: usize) -> Vec<f32> {
    frames
        .iter()
        .map(|frame| {
            frame
                .member(link, pawn)
                .map_or(0.0, |member| member.membership)
        })
        .collect()
}

/// Every frame-to-frame change moves at most one rate-cap step.
fn assert_rate_capped(series: &[f32], what: &str) {
    let step = ramp_step(FRAME_SECONDS);
    for (index, pair) in series.windows(2).enumerate() {
        assert!(
            (pair[1] - pair[0]).abs() <= step + EPS,
            "{what}: frame {index} stepped {} > {step}",
            pair[1] - pair[0]
        );
    }
}

/// The number of monotone rising and falling runs in `series`.
fn rises_and_falls(series: &[f32]) -> (usize, usize) {
    let mut rises = 0;
    let mut falls = 0;
    let mut direction = 0i8;
    for pair in series.windows(2) {
        let next = if pair[1] > pair[0] + EPS {
            1
        } else if pair[1] < pair[0] - EPS {
            -1
        } else {
            continue;
        };
        if next != direction {
            if next > 0 {
                rises += 1;
            } else {
                falls += 1;
            }
            direction = next;
        }
    }
    (rises, falls)
}

/// The swap of one pawn as one peer presents it: the old ghost fades at its last
/// in-volume pose from the cut frame on, and the new ghost starts on the cut
/// frame at 0 and ramps in at the rate cap.
fn assert_swap(frames: &[PresentedFrame], pawn: usize, from: u16, who: &str) {
    let to = 1 - from;
    let cut = first_frame_in_copy_b(frames, pawn, from == 1);
    let before = &frames[cut - 1];
    let old = before
        .member(from, pawn)
        .unwrap_or_else(|| panic!("{who}: pawn {pawn} ghosts through link {from} before the swap"));
    assert!(!old.frozen && (old.membership - 1.0).abs() < EPS);
    let last_in_volume = old.source;
    for frame in &frames[..cut] {
        assert!(
            frame.member(to, pawn).is_none(),
            "{who}: no link-{to} ghost of pawn {pawn} before its cut frame"
        );
    }
    let new = frames[cut]
        .member(to, pawn)
        .unwrap_or_else(|| panic!("{who}: the new ghost starts on the cut frame"));
    assert!(new.membership <= EPS, "{who}: a new ghost starts at 0");
    let fading = frames[cut]
        .member(from, pawn)
        .expect("the old ghost is still fading on the cut frame");
    assert!(fading.frozen);
    for frame in &frames[cut..] {
        if let Some(old) = frame.member(from, pawn) {
            assert!(old.frozen, "{who}: the old ghost never retargets");
            assert!(
                (old.source.position - last_in_volume.position).length() < 1e-5,
                "{who}: the old ghost fades at its last in-volume pose"
            );
        }
    }
    let rising = memberships(&frames[cut..], to, pawn);
    let falling = memberships(&frames[cut - 1..], from, pawn);
    assert_rate_capped(&rising, &format!("{who} new ghost"));
    assert_rate_capped(&falling, &format!("{who} old ghost"));
    assert!(rising.windows(2).all(|pair| pair[1] >= pair[0] - EPS));
    assert!(falling.windows(2).all(|pair| pair[1] <= pair[0] + EPS));
    assert!(
        (rising.last().unwrap() - 1.0).abs() < EPS,
        "{who}: fades in fully"
    );
    assert!(*falling.last().unwrap() == 0.0, "{who}: fades out fully");
}

// AC15 / P1: two pawns swapped on one host tick by arming both seamless
// teleporters, presented by a connected client over a conditioned link and by
// the host seat for the client pawn it draws delayed.
#[test]
fn ghost_swap_on_one_tick_fades_each_old_ghost_in_volume_and_starts_each_new_on_the_cut_frame() {
    let mut run = Loopback::new(Scenario {
        links_armed: true,
        pawns: [Vec3::new(1.0, 0.0, 1.0), Vec3::new(101.0, 0.0, -1.0)],
        barrier: false,
    });
    run.run(90);
    let settled = run.client_frames.last().unwrap();
    for pawn in 0..2 {
        assert_eq!(
            settled
                .member(pawn as u16, pawn)
                .map(|member| member.membership),
            Some(1.0),
            "fixture: pawn {pawn} ghosts in full through its own copy's link"
        );
    }
    let (client_start, host_start) = (run.client_frames.len(), run.host_frames.len());

    run.arm_teleporter(0);
    run.arm_teleporter(1);
    run.run(90);

    let client = &run.client_frames[client_start..];
    let host = &run.host_frames[host_start..];
    let arrived = client.last().unwrap();
    assert!(in_copy_b(arrived.pawns[0].unwrap()) && !in_copy_b(arrived.pawns[1].unwrap()));
    assert_eq!(
        first_frame_in_copy_b(client, 0, false),
        first_frame_in_copy_b(client, 1, true),
        "fixture: both pawns swap on one presented frame"
    );
    assert_every_ghost_maps_an_in_volume_pose(&run, client, "client");
    assert_every_ghost_maps_an_in_volume_pose(&run, host, "host seat");
    assert_swap(client, 0, 0, "client");
    assert_swap(client, 1, 1, "client");
    // The host seat never ghosts its own pawn; it draws the client pawn delayed.
    assert!(
        host.iter()
            .all(|frame| frame.member(0, 0).is_none() && frame.member(1, 0).is_none())
    );
    assert_swap(host, 1, 1, "host seat");
}

/// P18 on one peer: link 0's arm, link 1's arm and pawn 1's swap from copy A to
/// copy B all apply on one host tick. The pawn is never drawn in copy A while
/// link 0 shows armed, so link 0 never ghosts it; link 1's ghost of it starts on
/// the cut frame. Returns whether the lane clock alone would have presented the
/// arm while the pawn was still drawn in copy A.
fn assert_arm_presents_with_the_swap(frames: &[PresentedFrame], arm_stamp: u32, who: &str) -> bool {
    let cut = first_frame_in_copy_b(frames, 1, false);
    assert!(
        frames.iter().all(|frame| frame.member(0, 1).is_none()),
        "{who}: the arm never presents against the source-side pose"
    );
    for frame in &frames[..cut] {
        assert!(frame.member(1, 1).is_none());
    }
    assert!(
        frames[cut].member(1, 1).is_some(),
        "{who}: the arm and the swap present on one frame"
    );
    frames[..cut]
        .iter()
        .any(|frame| frame.remote_clock.ceil() >= f64::from(arm_stamp))
}

// AC16 / P18: a link arm and a remote pawn's seamless swap on one host tick
// present on one frame, on a connected client and on the host seat. Each ghost
// reads link state at the stamp of the pose actually presented: across the cut
// the buffers hold the source-side sample, which the lane clock alone would
// pair with the arm one tick early.
#[test]
fn ghost_link_arm_and_seamless_swap_on_one_tick_present_on_one_frame_on_client_and_host_seat() {
    let mut run = Loopback::new(Scenario {
        links_armed: false,
        pawns: [Vec3::new(-50.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)],
        barrier: false,
    });
    run.run(90);
    assert!(run.client_frames.last().unwrap().members.is_empty());
    let (client_start, host_start) = (run.client_frames.len(), run.host_frames.len());

    let arm_stamp = run.tick + 1;
    run.set_link_armed(0, true);
    run.set_link_armed(1, true);
    run.arm_teleporter(0);
    run.run(90);

    let client = &run.client_frames[client_start..];
    let host = &run.host_frames[host_start..];
    let client_window = assert_arm_presents_with_the_swap(client, arm_stamp, "client");
    let host_window = assert_arm_presents_with_the_swap(host, arm_stamp, "host seat");
    assert!(
        client_window && host_window,
        "fixture: each peer presents frames where the lane clock alone shows the arm early"
    );
    assert_every_ghost_maps_an_in_volume_pose(&run, client, "client");
    assert_every_ghost_maps_an_in_volume_pose(&run, host, "host seat");
}

// AC13 / P35 (and P3 on the host seat): `arm` on tick N and `disarm` on N + 1
// over a conditioned link. A connected client presents at most one rise and
// fall, each at the rate cap, and none when no snapshot carries tick N's end
// state; the host seat, which records every tick, presents one rise and fall
// either way. Also AC37's thin part: the host's arm reaches the client.
#[test]
fn ghost_arm_then_disarm_on_consecutive_ticks_presents_at_most_one_rise_and_fall_on_a_client() {
    for snapshot_carries_arm in [true, false] {
        let mut run = Loopback::new(Scenario {
            links_armed: false,
            pawns: [Vec3::new(-50.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)],
            barrier: false,
        });
        run.run(90);
        // Snapshots carry even stamps; the arm is stamped with the next tick.
        if (run.tick + 1).is_multiple_of(2) != snapshot_carries_arm {
            run.run(1);
        }
        let (client_start, host_start) = (run.client_frames.len(), run.host_frames.len());
        run.set_link_armed(0, true);
        run.run(1);
        run.set_link_armed(0, false);
        run.run(90);

        let client = memberships(&run.client_frames[client_start..], 0, 1);
        let host = memberships(&run.host_frames[host_start..], 0, 1);
        let label = format!("snapshot carries the arm: {snapshot_carries_arm}");
        assert_rate_capped(&client, &format!("client, {label}"));
        assert_rate_capped(&host, &format!("host seat, {label}"));
        assert_eq!(rises_and_falls(&host), (1, 1), "host seat, {label}");
        assert_eq!(*host.last().unwrap(), 0.0);
        if snapshot_carries_arm {
            assert_eq!(rises_and_falls(&client), (1, 1), "client, {label}");
        } else {
            assert!(
                client.iter().all(|membership| *membership == 0.0),
                "client, {label}: no ghost when no snapshot carries the armed tick"
            );
        }
        assert_eq!(*client.last().unwrap(), 0.0);
    }
}
