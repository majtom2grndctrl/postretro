// Two-peer ghost-voice admission: a connected client's fire cue read at its shooter's clock.
// See: context/lib/audio.md §1 (Ghost bus) · context/lib/entity_model.md §7d

use glam::Vec3;
use postretro_foundation::{DescriptorError, GhostStyle, GhostStyleDescriptor, GhostStyleRegistry};

use super::test_fixtures::*;

/// `echo` carries sound, so a member's fire admits a ghost voice.
fn sounding_styles() -> GhostStyleRegistry {
    GhostStyleRegistry::from_declarations(
        "test",
        [Ok::<_, DescriptorError>(GhostStyleDescriptor {
            name: "echo".to_string(),
            style: GhostStyle {
                audio: true,
                ..GhostStyle::FLOOR
            },
        })],
    )
}

// On a connected client, a fire cue from a remote shooter inside link 0,
// received and played on any frame after a host arm, admits a ghost voice by
// armed state as the shooter's ghost presents it on that frame. A cue on a
// frame where host-now is already past the arm but the shooter still presents
// before it admits none; from the frame the shooter's ghost joins on, every
// cue admits.
#[test]
fn a_client_fire_cue_after_an_arm_admits_by_armed_state_as_the_shooters_ghost_presents_it() {
    let mut run = Loopback::new(Scenario {
        links_armed: false,
        pawns: [Vec3::new(-50.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)],
        barrier: false,
    });
    let styles = sounding_styles();
    run.bind_styles("echo", &styles, &styles);
    run.client_fire_probe = Some(1);
    run.run(90);
    let start = run.client_frames.len();
    assert_eq!(
        run.client_frames[start - 1].fire_voices,
        Some(0),
        "fixture: the shooter replicated and no voice admits before the arm"
    );

    // The arm applies during the next tick and is stamped with its end.
    let arm_stamp = f64::from(run.tick + 1);
    run.set_link_armed(0, true);
    run.run(60);

    let frames = &run.client_frames[start..];
    for (index, frame) in frames.iter().enumerate() {
        let presents_armed = frame.member(0, 1).is_some_and(|member| !member.frozen);
        let voices = frame
            .fire_voices
            .expect("the shooter stays replicated on the client");
        assert_eq!(
            voices,
            usize::from(presents_armed),
            "client frame {index}: a cue admits exactly when the shooter's ghost presents armed"
        );
    }
    let first_admitted = frames
        .iter()
        .position(|frame| frame.fire_voices == Some(1))
        .expect("a cue admits once the shooter's ghost presents the arm");
    assert!(
        frames[..first_admitted]
            .iter()
            .any(|frame| frame.host_now >= arm_stamp),
        "fixture: a cue arrived after the arm on host-now, before the shooter presented it"
    );
    assert_eq!(
        frames[first_admitted - 1].fire_voices,
        Some(0),
        "a cue one frame earlier admits none"
    );
    assert!(
        frames[first_admitted..]
            .iter()
            .all(|frame| frame.fire_voices == Some(1)),
        "every later cue admits while the link stays armed"
    );
}
