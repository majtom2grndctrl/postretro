use std::collections::HashMap;
use std::path::PathBuf;

use glam::{Mat4, Vec3};

use postretro_entities::components::animation::{AnimationState, InterruptPolicy, MeshAnimation};
use postretro_entities::components::mesh::MeshComponent;

use super::*;
use crate::scripting_systems::hit_zones::ModelLoadWarningOwner;

const BODY: &str = "models/limitator/model.gltf";
const BAKE: &str = "models/ar_4/limitator.gltf";
const BODY_CANNON: &str = "models/mutant_arm_cannon/model.gltf";

fn content_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/dev")
}

fn store_with(models: &[&str]) -> HitZoneStore {
    let mut store = HitZoneStore::new();
    for model in models {
        store.insert_from_load(model, &content_root(), ModelLoadWarningOwner::GameSide);
        assert!(store.get_by_name(model).is_some(), "{model} loads");
    }
    store
}

fn state(clip: &str, looping: bool) -> AnimationState {
    AnimationState {
        clip: clip.into(),
        looping,
        crossfade_ms: 0.0,
        interrupt: InterruptPolicy::Smooth,
        travel_speed: None,
        max_playback_rate: None,
        scale_with_speed: true,
        playback_rate: None,
        // Unresolved, as on a client: the pose resolves the clip by name.
        clip_index: None,
    }
}

fn mesh(model: &str, states: &[(&str, &str, bool)]) -> MeshComponent {
    let map: HashMap<String, AnimationState> = states
        .iter()
        .map(|(name, clip, looping)| (name.to_string(), state(clip, *looping)))
        .collect();
    MeshComponent::animated(
        model.to_string(),
        MeshAnimation::new(map, states[0].0.to_string()),
    )
}

fn limitator_mesh() -> MeshComponent {
    mesh(
        BODY,
        &[
            ("idle_aiming", "idle_aiming", true),
            ("shoot", "firing_rifle", true),
            ("reload", "reloading", false),
        ],
    )
}

const MOUNTED: MuzzleMount<'static> = MuzzleMount::Mounted {
    holder_socket: "hand_r",
    bake: BAKE,
};

fn muzzle(
    store: &HitZoneStore,
    mesh: &MeshComponent,
    model_to_world: Mat4,
    state: &str,
    time: f64,
    mount: MuzzleMount<'_>,
) -> Option<Vec3> {
    let mut scratch = Vec::new();
    posed_muzzle_world(
        store,
        model_to_world,
        mesh,
        ClipPose { state, time },
        mount,
        &mut scratch,
    )
}

fn bake_muzzle_offset(store: &HitZoneStore) -> f32 {
    let bake = store.get_by_name(BAKE).expect("bake loaded");
    let SocketBinding::RigidRest(rest) = bake.sockets[BAKE_MUZZLE_SOCKET] else {
        panic!("the bake's muzzle is a rigid node socket");
    };
    rest.w_axis.truncate().length()
}

// The riskiest premise of E10--npc-weapon-mounts: the host can pose the tracked
// state at clip time and compose the bake's muzzle on the posed hand.
#[test]
fn limitator_muzzle_sits_at_the_mounted_barrel_below_the_head() {
    let store = store_with(&[BODY, BAKE]);
    let mesh = limitator_mesh();
    let offset = bake_muzzle_offset(&store);
    assert!(
        offset > 0.15,
        "the barrel tip sits well off the grip: {offset}"
    );

    for (state, time) in [("idle_aiming", 0.0), ("idle_aiming", 0.7), ("shoot", 0.1)] {
        let at = muzzle(&store, &mesh, Mat4::IDENTITY, state, time, MOUNTED)
            .unwrap_or_else(|| panic!("{state} at {time} poses a muzzle"));
        let hand = muzzle(
            &store,
            &mesh,
            Mat4::IDENTITY,
            state,
            time,
            MuzzleMount::BodySocket("hand_r"),
        )
        .expect("hand_r poses");
        // The limitator's head top is ~1.74 m (research §NPC shot origin);
        // its eye, today's origin, 1.82 m.
        assert!(
            (0.8..1.6).contains(&at.y),
            "{state} at {time}: muzzle {at} sits at gun height, below the head top",
        );
        assert!(
            ((at - hand).length() - offset).abs() < 1e-3,
            "{state} at {time}: the muzzle rides the posed hand at the bake's offset",
        );
    }
}

#[test]
fn muzzle_moves_with_clip_time_and_wraps_a_looping_state() {
    let store = store_with(&[BODY, BAKE]);
    let mesh = limitator_mesh();
    let duration = store
        .get_by_name(BODY)
        .unwrap()
        .state_clip(None, "idle_aiming")
        .expect("idle_aiming resolves")
        .duration as f64;
    let early = muzzle(&store, &mesh, Mat4::IDENTITY, "idle_aiming", 0.0, MOUNTED).unwrap();
    let mid = muzzle(
        &store,
        &mesh,
        Mat4::IDENTITY,
        "idle_aiming",
        duration / 2.0,
        MOUNTED,
    )
    .unwrap();
    assert!(
        early.distance(mid) > 1e-3,
        "the pose moves the muzzle: {early} vs {mid}"
    );
    let wrapped = muzzle(
        &store,
        &mesh,
        Mat4::IDENTITY,
        "idle_aiming",
        duration + duration / 2.0,
        MOUNTED,
    )
    .unwrap();
    assert!(
        mid.distance(wrapped) < 1e-4,
        "unwrapped clip time wraps a looping state"
    );

    let shoot = muzzle(&store, &mesh, Mat4::IDENTITY, "shoot", 0.0, MOUNTED).unwrap();
    assert!(
        early.distance(shoot) > 1e-3,
        "the tracked state picks the clip: {early} vs {shoot}"
    );
}

#[test]
fn muzzle_follows_the_holder_model_matrix() {
    let store = store_with(&[BODY, BAKE]);
    let mesh = limitator_mesh();
    let local = muzzle(&store, &mesh, Mat4::IDENTITY, "shoot", 0.2, MOUNTED).unwrap();
    let model_to_world = Mat4::from_rotation_translation(
        glam::Quat::from_rotation_y(1.1),
        Vec3::new(3.0, 0.5, -2.0),
    );
    let world = muzzle(&store, &mesh, model_to_world, "shoot", 0.2, MOUNTED).unwrap();
    assert!(world.distance(model_to_world.transform_point3(local)) < 1e-4);
}

#[test]
fn built_in_muzzle_reads_a_body_socket_that_moves_with_the_clip() {
    let store = store_with(&[BODY_CANNON]);
    let mesh = mesh(
        BODY_CANNON,
        &[("idle", "idle", true), ("punch", "punch", false)],
    );
    let tip = MuzzleMount::BodySocket("cannon_tip");
    let idle = muzzle(&store, &mesh, Mat4::IDENTITY, "idle", 0.0, tip).expect("cannon_tip poses");
    let punch = muzzle(&store, &mesh, Mat4::IDENTITY, "punch", 0.4, tip).expect("cannon_tip poses");
    assert!(
        idle.distance(punch) > 0.1,
        "the forearm carries the tip: {idle} vs {punch}"
    );
}

#[test]
fn muzzle_is_missing_when_the_state_socket_or_bake_does_not_resolve() {
    let store = store_with(&[BODY, BAKE]);
    let mesh = limitator_mesh();
    let at = |state: &str, mount| muzzle(&store, &mesh, Mat4::IDENTITY, state, 0.0, mount);

    assert_eq!(
        at("no_such_state", MOUNTED),
        None,
        "an untracked state has no pose"
    );
    assert_eq!(
        at(
            "shoot",
            MuzzleMount::Mounted {
                holder_socket: "no_such_socket",
                bake: BAKE
            }
        ),
        None,
        "a socket the rig lacks"
    );
    assert_eq!(
        at(
            "shoot",
            MuzzleMount::Mounted {
                holder_socket: "hand_r",
                bake: "models/ar_4/not_loaded.gltf"
            }
        ),
        None,
        "a bake the level did not load"
    );
    assert_eq!(
        at("shoot", MuzzleMount::BodySocket("cannon_tip")),
        None,
        "a body socket the model lacks"
    );

    let unresolved = mesh_with_unknown_clip();
    assert_eq!(
        muzzle(&store, &unresolved, Mat4::IDENTITY, "shoot", 0.0, MOUNTED),
        None,
        "a state whose clip does not resolve"
    );
}

#[test]
fn a_bake_without_a_muzzle_socket_has_no_muzzle() {
    let mut store = store_with(&[BODY, BAKE]);
    let bake = "models/ar_4/no_muzzle.gltf";
    let mut built = store.get_by_name(BAKE).cloned().unwrap();
    built.sockets.remove(BAKE_MUZZLE_SOCKET);
    store.insert_for_test(postretro_model::ModelHandle::from(bake), built);
    let mesh = limitator_mesh();
    let mount = MuzzleMount::Mounted {
        holder_socket: "hand_r",
        bake,
    };
    assert_eq!(
        muzzle(&store, &mesh, Mat4::IDENTITY, "shoot", 0.0, mount),
        None
    );
}

fn mesh_with_unknown_clip() -> MeshComponent {
    mesh(BODY, &[("shoot", "no_such_clip", true)])
}

// Regression: a long-held loop's unwrapped clip time lost its phase in the
// sampler's f32, so the muzzle drifted off the drawn gun.
#[test]
fn a_loop_held_a_million_passes_poses_the_muzzle_where_its_first_pass_does() {
    let store = store_with(&[BODY, BAKE]);
    let mesh = limitator_mesh();
    let duration = store
        .get_by_name(BODY)
        .unwrap()
        .state_clip(None, "firing_rifle")
        .expect("firing_rifle resolves")
        .duration as f64;
    for time in [0.03, duration * 0.6] {
        let first = muzzle(&store, &mesh, Mat4::IDENTITY, "shoot", time, MOUNTED).unwrap();
        let held = muzzle(
            &store,
            &mesh,
            Mat4::IDENTITY,
            "shoot",
            1.0e6 * duration + time,
            MOUNTED,
        )
        .unwrap();
        assert!(
            first.distance(held) < 1.0e-3,
            "at {time} s into the pass: {first} vs {held}"
        );
    }
}
