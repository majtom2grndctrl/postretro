// Tests: NPC mount composition, the muzzle resolver, and the install checks'
// warn-once diagnostics.
// See: context/lib/entity_model.md §2 (Weapon mounts)

use std::collections::HashMap;
use std::sync::Arc;

use glam::Mat4;
use log::Level;
use postretro_entities::data_descriptors::MountDescriptor;
use postretro_model::ModelHandle;
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::scripting::builtins::data_archetype_test_fixtures::{
    behavior_enemy_descriptor, mesh_descriptor,
};

const BODY: &str = "models/body/model.gltf";
const BAKE: &str = "models/ar_4/body.gltf";
const BAKE_B: &str = "models/ar_4/other.gltf";

fn entry(weapon: &str, pairs: &[(&str, &str)]) -> MountDescriptor {
    MountDescriptor {
        item: weapon.to_string(),
        attachments: Some(
            pairs
                .iter()
                .map(|(socket, model)| (socket.to_string(), model.to_string()))
                .collect(),
        ),
        muzzle_socket: None,
    }
}

fn muzzle_entry(weapon: &str, socket: &str) -> MountDescriptor {
    MountDescriptor {
        item: weapon.to_string(),
        attachments: None,
        muzzle_socket: Some(socket.to_string()),
    }
}

/// An NPC `trooper` on `BODY` whose attacks name `rifle` and `pistol`, with
/// base attachments `back` and `hand_r` and the given table.
fn npc(table: Option<Vec<MountDescriptor>>) -> EntityTypeDescriptor {
    let mut descriptor = behavior_enemy_descriptor("trooper");
    let behavior = descriptor.behavior.as_mut().unwrap();
    let mut claw = behavior.attacks["claw"].clone();
    claw.weapon = Some("rifle".to_string());
    let mut alt = claw.clone();
    alt.weapon = Some("pistol".to_string());
    behavior.attacks.clear();
    behavior.attacks.insert("claw".to_string(), claw);
    behavior.attacks.insert("alt".to_string(), alt);
    let mesh = descriptor.mesh.as_mut().unwrap();
    mesh.model = BODY.to_string();
    mesh.attachments = HashMap::from([
        ("back".to_string(), "models/sling.gltf".to_string()),
        ("hand_r".to_string(), "models/base_rifle.gltf".to_string()),
    ]);
    mesh.mounts = table;
    descriptor
}

fn compose(descriptor: &EntityTypeDescriptor) -> Vec<(String, String)> {
    compose_attachments(descriptor, descriptor.mesh.as_ref().unwrap())
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(socket, model)| (socket.to_string(), model.to_string()))
        .collect()
}

/// A loaded model with the named sockets, each a rigid rest transform.
fn zones(sockets: &[&str]) -> ModelHitZones {
    ModelHitZones::for_test(
        Arc::new(postretro_model::skeleton::Skeleton::default()),
        Arc::new(Vec::new()),
        Vec::new(),
        sockets
            .iter()
            .map(|socket| (socket.to_string(), SocketBinding::RigidRest(Mat4::IDENTITY)))
            .collect(),
        None,
        Vec::new(),
        Arc::new(postretro_model::pose_modifier::PoseModifierStack::default()),
    )
}

fn store_with(models: &[(&str, &[&str])]) -> HitZoneStore {
    let mut store = HitZoneStore::new();
    for (model, sockets) in models {
        store.insert_for_test(ModelHandle::from(*model), zones(sockets));
    }
    store
}

#[test]
fn an_npc_composes_base_then_each_attack_weapons_entry_with_entries_winning() {
    let descriptor = npc(Some(vec![
        entry("rifle", &[("hand_r", BAKE)]),
        entry(
            "pistol",
            &[("hand_r", BAKE), ("hip", "models/holster.gltf")],
        ),
        // No attack names it: it mounts nothing.
        entry("knife", &[("belt", "models/knife.gltf")]),
    ]));
    assert_eq!(
        compose(&descriptor),
        pairs(&[
            ("back", "models/sling.gltf"),
            ("hand_r", BAKE),
            ("hip", "models/holster.gltf"),
        ])
    );
}

#[test]
fn a_muzzle_socket_entry_adds_nothing_and_no_table_leaves_the_base() {
    let muzzle_only = npc(Some(vec![
        muzzle_entry("rifle", "cannon_tip"),
        muzzle_entry("pistol", "cannon_tip"),
    ]));
    let base = pairs(&[
        ("back", "models/sling.gltf"),
        ("hand_r", "models/base_rifle.gltf"),
    ]);
    assert_eq!(compose(&muzzle_only), base);
    assert_eq!(compose(&npc(None)), base);
}

#[test]
fn a_descriptor_without_a_brain_mounts_no_table_entry() {
    let mut descriptor = mesh_descriptor("prop", false);
    let mesh = descriptor.mesh.as_mut().unwrap();
    mesh.mounts = Some(vec![entry("rifle", &[("hand_r", BAKE)])]);
    let base: Vec<(String, String)> = descriptor
        .mesh
        .as_ref()
        .unwrap()
        .attachments
        .iter()
        .map(|(socket, model)| (socket.clone(), model.clone()))
        .collect();
    assert_eq!(compose(&descriptor), base);
}

#[test]
fn the_muzzle_resolver_follows_the_entry_shape_and_the_loaded_models() {
    let store = store_with(&[
        (BODY, &["hand_r", "back", "cannon_tip"]),
        (BAKE, &["muzzle"]),
        (BAKE_B, &["grip"]),
    ]);
    let descriptor = npc(Some(vec![
        entry("rifle", &[("hand_r", BAKE)]),
        muzzle_entry("pistol", "cannon_tip"),
        muzzle_entry("shotgun", "absent"),
        entry("blunderbuss", &[("hand_r", BAKE_B)]),
        entry("carbine", &[("hip", BAKE)]),
    ]));
    let resolve = |weapon: &str| npc_muzzle_mount(&descriptor, weapon, &store);
    assert_eq!(
        resolve("rifle"),
        Some(MuzzleMount::Mounted {
            holder_socket: "hand_r",
            bake: BAKE
        })
    );
    assert_eq!(
        resolve("pistol"),
        Some(MuzzleMount::BodySocket("cannon_tip"))
    );
    assert_eq!(resolve("shotgun"), None, "the body lacks the socket");
    assert_eq!(resolve("blunderbuss"), None, "the bake has no muzzle");
    assert_eq!(resolve("carbine"), None, "the body lacks the holder socket");
    assert_eq!(resolve("unlisted"), None, "no entry");

    assert_eq!(
        npc_muzzle_mount(&npc(None), "rifle", &store),
        None,
        "no table"
    );
    let unloaded_body = store_with(&[(BAKE, &["muzzle"])]);
    assert_eq!(npc_muzzle_mount(&descriptor, "rifle", &unloaded_body), None);
}

#[test]
fn with_several_muzzle_bakes_the_resolver_takes_the_first_in_socket_order() {
    let store = store_with(&[
        (BODY, &["hand_l", "hand_r"]),
        (BAKE, &["muzzle"]),
        (BAKE_B, &["muzzle"]),
    ]);
    let descriptor = npc(Some(vec![entry(
        "rifle",
        &[("hand_r", BAKE), ("hand_l", BAKE_B)],
    )]));
    assert_eq!(
        npc_muzzle_mount(&descriptor, "rifle", &store),
        Some(MuzzleMount::Mounted {
            holder_socket: "hand_l",
            bake: BAKE_B
        })
    );
}

const RIFLE_MISSING: &str =
    "[Weapon mounts] entity `trooper` has no mounts entry for weapon `rifle`; it mounts nothing";

#[test]
fn the_pre_sweep_check_warns_once_for_each_attack_weapon_an_npc_table_lacks() {
    let logs = LogCapture::start();
    let store = HitZoneStore::new();
    let descriptors = [npc(Some(vec![entry("pistol", &[("hand_r", BAKE)])]))];

    crate::scripting_systems::weapon_mounts::check_weapon_mount_tables(&descriptors, &store);
    crate::scripting_systems::weapon_mounts::check_weapon_mount_tables(&descriptors, &store);

    logs.assert_logged_once(Level::Warn, RIFLE_MISSING);
    logs.assert_not_logged(Level::Warn, "weapon `pistol`");
}

#[test]
fn the_pre_sweep_check_is_silent_for_an_npc_without_a_table() {
    let logs = LogCapture::start();
    let store = HitZoneStore::new();

    crate::scripting_systems::weapon_mounts::check_weapon_mount_tables(&[npc(None)], &store);

    logs.assert_nothing_above(Level::Info);
}

fn warns(logs: &LogCapture, needle: &str) -> usize {
    logs.records()
        .iter()
        .filter(|record| record.level == Level::Warn && record.message.contains(needle))
        .count()
}

#[test]
fn the_post_sweep_check_warns_once_for_a_missing_socket_and_marks_the_binding_key() {
    let logs = LogCapture::start();
    let store = store_with(&[(BODY, &["back"]), (BAKE, &["muzzle"])]);
    let descriptors = [npc(Some(vec![
        entry("rifle", &[("hand_r", BAKE)]),
        entry("pistol", &[("hand_r", BAKE)]),
    ]))];

    check_npc_mount_models(&descriptors, &store);
    check_npc_mount_models(&descriptors, &store);

    logs.assert_logged_once(
        Level::Warn,
        "entity `trooper` mounts weapon `rifle` on socket `hand_r`, which its model `models/body/model.gltf` lacks",
    );
    // The mesh-binding resolver's key is taken, so it never doubles the warning.
    let key = format!("attachment-socket:{BODY}:hand_r:{BAKE}");
    assert!(!store.mark_attachment_resolution_warning(key));
}

#[test]
fn the_post_sweep_check_warns_once_for_an_unloaded_bake() {
    let logs = LogCapture::start();
    let store = store_with(&[(BODY, &["hand_r"])]);
    let descriptors = [npc(Some(vec![
        entry("rifle", &[("hand_r", BAKE)]),
        entry("pistol", &[("hand_r", BAKE)]),
    ]))];

    check_npc_mount_models(&descriptors, &store);
    check_npc_mount_models(&descriptors, &store);

    logs.assert_logged_once(
        Level::Warn,
        "entity `trooper` mounts model `models/ar_4/body.gltf` on socket `hand_r` for weapon `rifle`, but the model is not loaded",
    );
    // No bake loaded, so no muzzle verdict is possible.
    assert_eq!(warns(&logs, "muzzle"), 0);
}

#[test]
fn the_post_sweep_check_warns_once_per_weapon_when_no_loaded_bake_has_a_muzzle() {
    let logs = LogCapture::start();
    let store = store_with(&[(BODY, &["hand_r"]), (BAKE, &["grip"])]);
    let descriptors = [npc(Some(vec![
        entry("rifle", &[("hand_r", BAKE)]),
        entry("pistol", &[("hand_r", BAKE)]),
    ]))];

    check_npc_mount_models(&descriptors, &store);
    check_npc_mount_models(&descriptors, &store);

    for weapon in ["rifle", "pistol"] {
        logs.assert_logged_once(
            Level::Warn,
            &format!(
                "archetype `trooper` weapon `{weapon}`: no mounted bake carries a `muzzle` socket"
            ),
        );
    }
}

#[test]
fn the_post_sweep_check_warns_once_naming_the_bake_used_when_several_carry_a_muzzle() {
    let logs = LogCapture::start();
    let store = store_with(&[
        (BODY, &["hand_l", "hand_r"]),
        (BAKE, &["muzzle"]),
        (BAKE_B, &["muzzle"]),
    ]);
    let descriptors = [npc(Some(vec![entry(
        "rifle",
        &[("hand_r", BAKE), ("hand_l", BAKE_B)],
    )]))];

    check_npc_mount_models(&descriptors, &store);
    check_npc_mount_models(&descriptors, &store);

    logs.assert_logged_once(
        Level::Warn,
        &format!(
            "archetype `trooper` weapon `rifle`: 2 mounted bakes carry a `muzzle` socket; its shots launch from `{BAKE_B}`"
        ),
    );
}

#[test]
fn a_bake_on_two_sockets_is_one_muzzle_bake() {
    let logs = LogCapture::start();
    let store = store_with(&[(BODY, &["hand_l", "hand_r"]), (BAKE, &["muzzle"])]);
    let descriptors = [npc(Some(vec![entry(
        "rifle",
        &[("hand_r", BAKE), ("hand_l", BAKE)],
    )]))];

    check_npc_mount_models(&descriptors, &store);

    assert_eq!(warns(&logs, "muzzle"), 0);
}

#[test]
fn a_muzzle_socket_entry_warns_once_when_the_loaded_body_lacks_it() {
    let logs = LogCapture::start();
    let store = store_with(&[(BODY, &["hand_r"])]);
    let descriptors = [npc(Some(vec![
        muzzle_entry("rifle", "cannon_tip"),
        muzzle_entry("pistol", "hand_r"),
    ]))];

    check_npc_mount_models(&descriptors, &store);
    check_npc_mount_models(&descriptors, &store);

    logs.assert_logged_once(
        Level::Warn,
        "archetype `trooper` weapon `rifle`: its body model `models/body/model.gltf` lacks muzzle socket `cannon_tip`",
    );
    assert_eq!(warns(&logs, "weapon `pistol`"), 0);
}

#[test]
fn the_post_sweep_check_is_silent_without_a_table_for_an_unloaded_body_and_for_unused_entries() {
    let logs = LogCapture::start();
    // No table.
    check_npc_mount_models(&[npc(None)], &store_with(&[(BODY, &[])]));
    // A body that never loaded: no body-socket and no muzzle-socket warning.
    let unloaded_body = store_with(&[(BAKE, &["muzzle"])]);
    check_npc_mount_models(
        &[npc(Some(vec![
            entry("rifle", &[("hand_r", BAKE)]),
            muzzle_entry("pistol", "cannon_tip"),
        ]))],
        &unloaded_body,
    );
    // An entry no attack names.
    check_npc_mount_models(
        &[npc(Some(vec![
            entry("rifle", &[("hand_r", BAKE)]),
            entry("pistol", &[("hand_r", BAKE)]),
            entry("knife", &[("belt", "models/missing.gltf")]),
        ]))],
        &store_with(&[(BODY, &["hand_r"]), (BAKE, &["muzzle"])]),
    );

    logs.assert_nothing_above(Level::Info);
}
