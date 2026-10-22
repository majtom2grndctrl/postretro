// Two-peer ghost-link loopback with a tagged mover: an arm and the motion it reveals.
// See: context/lib/networking.md §Current contract · context/lib/entity_model.md §7d

use glam::Vec3;

use super::test_fixtures::*;

/// Per frame: the barrier's ghost through link 0 (its presented membership,
/// if it is a ghost), and whether the barrier presents away from its rest pose.
fn barrier_trace(frames: &[PresentedFrame]) -> Vec<(Option<f32>, bool)> {
    let rest = frames[0].barrier.expect("barrier placed").1;
    frames
        .iter()
        .map(|frame| {
            let moved = frame
                .barrier
                .is_some_and(|(_, presented)| presented.distance(rest) > 1e-4);
            (
                frame.barrier_member(0).map(|member| member.membership),
                moved,
            )
        })
        .collect()
}

fn first(trace: &[(Option<f32>, bool)], test: impl Fn(&(Option<f32>, bool)) -> bool) -> usize {
    trace
        .iter()
        .position(test)
        .expect("the barrier ghosts and moves")
}

// A link arm and a member mover's motion (its phase change) on one
// host tick present on one frame, on a connected client and on the host seat.
// Run with the tick on and off the snapshot cadence: when no snapshot carries
// the armed tick, the next carries both the arm and the moved phase together.
//
// Host seat: the ghost joins on the very frame the barrier starts to move. A
// client applies the arm and the re-simulated phase from one snapshot; the
// ghost's first drawn frame (membership above 0) is the barrier's first moved
// frame. Its membership target may flip one frame earlier, on the snapshot's
// alpha-0 frame, where the mover still presents its pre-snapshot interpolation
// endpoint; a new ghost presents membership 0 there and draws nothing.
#[test]
fn ghost_link_arm_and_member_mover_motion_on_one_tick_present_on_one_client_frame() {
    for snapshot_carries_tick in [true, false] {
        let mut run = Loopback::new(Scenario {
            links_armed: false,
            pawns: [Vec3::new(-50.0, 0.0, 0.0), Vec3::new(50.0, 0.0, 0.0)],
            barrier: true,
        });
        run.run(90);
        // Snapshots carry even stamps; the arm is stamped with the next tick.
        if (run.tick + 1).is_multiple_of(2) != snapshot_carries_tick {
            run.run(1);
        }
        let (client_start, host_start) = (run.client_frames.len(), run.host_frames.len());
        let label = format!("snapshot carries the tick: {snapshot_carries_tick}");
        for frames in [&run.client_frames, &run.host_frames] {
            let last = frames.last().unwrap();
            assert!(
                last.barrier_member(0).is_none(),
                "{label}: disarmed at rest"
            );
        }

        run.set_link_armed(0, true);
        run.start_barrier();
        run.run(60);

        let host = barrier_trace(&run.host_frames[host_start..]);
        let joined = first(&host, |(ghost, _)| ghost.is_some());
        let moved = first(&host, |(_, moved)| *moved);
        assert_eq!(
            joined, moved,
            "host seat, {label}: the ghost joins with the motion"
        );

        let client = barrier_trace(&run.client_frames[client_start..]);
        let drawn = first(&client, |(ghost, _)| ghost.is_some_and(|m| m > 0.0));
        let joined = first(&client, |(ghost, _)| ghost.is_some());
        let moved = first(&client, |(_, moved)| *moved);
        assert_eq!(
            drawn, moved,
            "client, {label}: first drawn ghost frame moves"
        );
        assert!(
            moved - joined <= 1,
            "client, {label}: the target leads the motion by at most the alpha-0 frame"
        );
        for (who, frames) in [
            ("client", &run.client_frames[client_start..]),
            ("host seat", &run.host_frames[host_start..]),
        ] {
            assert!(
                frames.iter().all(|frame| frame.barrier_member(1).is_none()),
                "{who}: link 1's volume holds no barrier"
            );
        }
    }
}
