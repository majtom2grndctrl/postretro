// Two-peer committed reloads of descriptor ghost opt-ins: each peer re-resolves its own ghosts.
// See: context/lib/networking.md §Current contract · context/lib/entity_model.md §7d

use glam::Vec3;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{ComponentKind, EntityId, EntityTypeDescriptor};
use postretro_foundation::{ActivationLane, ShotId};

use super::test_fixtures::*;
use crate::ghost_links::membership::ramp_step;
use crate::ghost_links::opt_ins_test_fixtures::{data_registry, descriptor, projectile_weapon};
use crate::netcode;

/// A membership step's float slack.
const EPS: f32 = 1e-5;

/// Link 0's ghost of host pawn `pawn` on each frame, 0 when absent.
fn pawn_series(frames: &[PresentedFrame], pawn: usize) -> Vec<f32> {
    frames
        .iter()
        .map(|frame| {
            frame
                .member(0, pawn)
                .map_or(0.0, |member| member.membership)
        })
        .collect()
}

/// Link 0's ghost of this peer's `entity` on each frame, 0 when absent.
fn entity_series(frames: &[PresentedFrame], entity: EntityId) -> Vec<f32> {
    frames
        .iter()
        .map(|frame| {
            frame
                .members
                .iter()
                .find(|member| member.link == 0 && member.entity == entity)
                .map_or(0.0, |member| member.membership)
        })
        .collect()
}

/// A re-resolve through the membership ramp, from the last frame before a
/// reload on: `from` there, then every frame at most one rate-cap step toward
/// `to`, never away from it, ending at `to`.
fn assert_ramps(series: &[f32], from: f32, to: f32, who: &str) {
    let step = ramp_step(FRAME_SECONDS);
    assert!(
        (series[0] - from).abs() < EPS,
        "{who}: presents {from} before the reload, not {}",
        series[0]
    );
    for (index, pair) in series.windows(2).enumerate() {
        let delta = pair[1] - pair[0];
        assert!(
            delta.abs() <= step + EPS,
            "{who}: frame {index} stepped {delta}, past the rate cap {step}"
        );
        assert!(
            delta * (to - from) >= -EPS,
            "{who}: frame {index} moved away from {to}"
        );
    }
    assert!(
        (series[series.len() - 1] - to).abs() < EPS,
        "{who}: ends at {to}"
    );
}

/// Link 0's ghost of this peer's `entity` on the latest frame, 0 when absent.
fn last_membership(frames: &[PresentedFrame], entity: EntityId) -> f32 {
    entity_series(&frames[frames.len() - 1..], entity)[0]
}

/// The shared content both peers load: the player opts in, and the rocket
/// launcher's projectiles ghost per `projectile_ghost`.
fn content(projectile_ghost: bool) -> Vec<EntityTypeDescriptor> {
    vec![
        descriptor("player", true),
        projectile_weapon("rocket_launcher", projectile_ghost),
    ]
}

fn digest(descriptors: Vec<EntityTypeDescriptor>) -> [u8; 32] {
    crate::mod_digest::mod_compatibility_digest_from_registry(&data_registry(descriptors))
}

/// Fly a remote client's rocket on the host at `at`, through the host's
/// production remote-fire presentation: the measured client receives it under
/// its weapon's class. Returns the host's presentation entity.
fn fly_rocket(run: &mut Loopback, at: Vec3) -> EntityId {
    // The host's flight table lives in its endpoint, so a host endpoint lends
    // one. The flight is never advanced: the rocket holds at `at`.
    let Some(netcode::NetEndpoint::Host {
        mut projectile_presentations,
        ..
    }) = netcode::NetEndpoint::from_role(&netcode::NetRole::Host { port: 0 }, None)
        .expect("a host endpoint binds")
    else {
        unreachable!("a host role builds a host endpoint");
    };
    let projectile = projectile_weapon("rocket_launcher", true)
        .weapon
        .and_then(|weapon| weapon.projectile)
        .expect("a projectile weapon");
    let launch = postretro_sim::sim::RemoteProjectilePresentationLaunch {
        action: None,
        model_scale: 1.0,
        // Fired by the slot that never connects, so the measured client
        // receives the presentation.
        owner_client_id: CLIENT_B,
        shot_id: ShotId::from_parts(1, run.tick, ActivationLane::Primary, 0),
        origin: at,
        direction: Vec3::X,
        range: 64.0,
        descriptor_class: "rocket_launcher".to_string(),
        projectile,
    };
    run.with_host_replication(|registry, allocator, replicable, replication, tick| {
        projectile_presentations.spawn_remote(
            registry,
            allocator,
            replicable,
            replication,
            &launch,
            tick + 1,
        );
        registry
            .iter_with_kind(ComponentKind::DescriptorProvenance)
            .map(|(id, _)| id)
            .find(|id| {
                registry
                    .get_component::<DescriptorProvenance>(*id)
                    .is_ok_and(|provenance| {
                        provenance.spawn_path == DescriptorSpawnPath::ProjectilePresentation
                    })
            })
            .expect("the host flies the rocket's presentation")
    })
}

// A committed reload on both peers that flips the player descriptor's `ghost`
// re-resolves membership through the membership ramp on the host seat and on a
// connected client: an opt-out fades each ghost one rate-cap step per frame
// from the next frame, never a cut, and an opt-in ramps each in from 0.
#[test]
fn committed_ghost_reload_on_both_peers_ramps_host_seat_and_client_membership() {
    let mut run = Loopback::new(Scenario {
        links_armed: true,
        pawns: [Vec3::new(1.0, 0.0, 1.0), Vec3::new(-2.0, 0.0, -1.0)],
        barrier: false,
    });
    run.run(90);
    let step = ramp_step(FRAME_SECONDS);
    for (opted_in, from, to) in [(false, 1.0, 0.0), (true, 0.0, 1.0)] {
        let host_start = run.host_frames.len() - 1;
        let client_start = run.client_frames.len() - 1;
        run.commit_descriptors(
            Some(vec![descriptor("player", opted_in)]),
            Some(vec![descriptor("player", opted_in)]),
        );
        run.run(90);

        // The host seat ghosts the slot pawn, never its own; the client, which
        // has no pawn of its own here, ghosts both.
        let mut watched = vec![(
            "host seat, slot pawn".to_string(),
            &run.host_frames[host_start..],
            1,
        )];
        for pawn in 0..2 {
            watched.push((
                format!("client, pawn {pawn}"),
                &run.client_frames[client_start..],
                pawn,
            ));
        }
        for (who, frames, pawn) in watched {
            assert_ramps(&pawn_series(frames, pawn), from, to, &who);
            let next = frames[1]
                .member(0, pawn)
                .unwrap_or_else(|| panic!("{who}: a ghost presents on the frame after the reload"));
            if opted_in {
                assert!(
                    !next.frozen && next.membership <= EPS,
                    "{who}: an opt-in ramps in from 0"
                );
            } else {
                assert!(
                    next.frozen && (next.membership - (1.0 - step)).abs() < EPS,
                    "{who}: an opt-out fades one step, never a cut"
                );
            }
        }
    }
}

// A host reload flipping only a weapon's `projectile.ghost` changes ghosting on
// a connected client without rebuilding its pawn. Each peer resolves the opt-in
// from its own descriptors: the host's reload fades the host seat's ghost of a
// remote client's rocket and leaves the client's untouched; the client's own
// reload of the same content then fades its ghost through the ramp. Neither
// reload resends the client's tuning (the host retunes the live weapon, and the
// payload strips the opt-in) or changes either admission digest, so the client
// is never demoted. A control reload that does change the weapon's tuning
// resends it.
#[test]
fn projectile_ghost_reload_changes_client_ghosting_without_tuning_resend_or_demotion() {
    let mut run = Loopback::new(Scenario {
        links_armed: true,
        pawns: [Vec3::new(-50.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)],
        barrier: false,
    });
    run.commit_descriptors(Some(content(true)), Some(content(true)));
    run.host_hold_client_weapon("rocket_launcher");
    run.run(90);
    assert!(
        run.client_participating(),
        "fixture: the client participates"
    );
    // The send on the client's participation entry.
    run.host_send_client_tuning();
    let rocket = fly_rocket(&mut run, Vec3::new(3.0, 0.0, 0.0));
    run.run(90);
    assert_eq!(
        run.client_tunings_received, 1,
        "fixture: the entry tuning reached the client"
    );
    let client_rocket = run
        .client_entity_of(rocket)
        .expect("the rocket reaches the client");
    assert_eq!(
        last_membership(&run.host_frames, rocket),
        1.0,
        "fixture: the host seat ghosts the rocket"
    );
    assert_eq!(
        last_membership(&run.client_frames, client_rocket),
        1.0,
        "fixture: the client ghosts the rocket"
    );
    let divergences = run.client_divergences_received;
    let host_start = run.host_frames.len() - 1;
    let client_start = run.client_frames.len() - 1;

    // The host reloads: its own ghost of the rocket fades. Nothing about the
    // opt-in reaches the client, whose ghost holds.
    run.commit_descriptors(Some(content(false)), None);
    run.run(60);
    assert_ramps(
        &entity_series(&run.host_frames[host_start..], rocket),
        1.0,
        0.0,
        "host seat rocket",
    );
    assert!(
        entity_series(&run.client_frames[client_start..], client_rocket)
            .iter()
            .all(|membership| *membership == 1.0),
        "the client's rocket ghost holds through the host's reload"
    );
    assert!(
        run.client_participating(),
        "the host's reload demotes no one"
    );

    // The client reloads the same content: its own ghosting changes.
    let client_reload = run.client_frames.len() - 1;
    run.commit_descriptors(None, Some(content(false)));
    run.run(60);
    assert_ramps(
        &entity_series(&run.client_frames[client_reload..], client_rocket),
        1.0,
        0.0,
        "client rocket",
    );

    // Across both reloads.
    assert!(
        pawn_series(&run.client_frames[client_start..], 1)
            .iter()
            .all(|membership| *membership == 1.0),
        "the client's ghost of the slot pawn holds"
    );
    assert_eq!(run.client_tunings_received, 1, "no tuning resend");
    assert_eq!(
        run.client_divergences_received, divergences,
        "no demotion notice"
    );
    assert!(run.client_participating(), "the client still participates");
    assert_eq!(
        digest(content(true)),
        digest(content(false)),
        "the opt-in leaves the admission digest unchanged"
    );

    // Control: a host reload that changes the weapon's tuning resends it.
    let mut retuned = content(false);
    retuned[1].weapon.as_mut().expect("a weapon").damage = 25.0;
    run.commit_descriptors(Some(retuned), None);
    run.run(30);
    assert_eq!(
        run.client_tunings_received, 2,
        "fixture: a tuning change reaches the client"
    );
}
