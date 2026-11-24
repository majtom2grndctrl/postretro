// Tests: the level-install weapon-mount table check and its warn-once keys.
// See: context/lib/entity_model.md §2 (Weapon vocabulary)

use std::collections::HashMap;

use log::Level;
use postretro_entities::data_descriptors::{InventoryDescriptor, MountDescriptor};
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::scripting::builtins::data_archetype_test_fixtures::mesh_descriptor;
use crate::traps::trap_test_fixtures::{bolt, entity};

const RIFLE_MISSING: &str =
    "[Weapon mounts] entity `player` has no mounts entry for weapon `rifle`; it mounts nothing";

fn entry(weapon: &str, model: &str) -> MountDescriptor {
    MountDescriptor {
        item: weapon.to_string(),
        attachments: Some(HashMap::from([("hand_r".to_string(), model.to_string())])),
        muzzle_socket: None,
    }
}

/// A player carrying `loadout`, with `table` as its weapon-mount table, plus
/// the `rifle` and `pistol` weapons.
fn descriptors(loadout: &[&str], table: Option<Vec<MountDescriptor>>) -> Vec<EntityTypeDescriptor> {
    let mut player = mesh_descriptor("player", false);
    player.mesh.as_mut().unwrap().mounts = table;
    player.inventory = Some(InventoryDescriptor {
        loadout: loadout.iter().map(|name| name.to_string()).collect(),
    });
    vec![
        player,
        entity("rifle", bolt(100.0)),
        entity("pistol", bolt(100.0)),
    ]
}

#[test]
fn install_check_warns_once_for_each_loadout_weapon_the_table_lacks_naming_both() {
    let logs = LogCapture::start();
    let store = HitZoneStore::new();
    let descriptors = descriptors(
        &["rifle", "pistol"],
        Some(vec![entry("pistol", "models/pistol/player.gltf")]),
    );

    check_weapon_mount_tables(&descriptors, &store);
    check_weapon_mount_tables(&descriptors, &store);

    logs.assert_logged_once(Level::Warn, RIFLE_MISSING);
    logs.assert_not_logged(Level::Warn, "weapon `pistol`");
}

#[test]
fn a_loadout_weapon_warned_at_install_does_not_warn_again_at_first_wield() {
    let logs = LogCapture::start();
    let store = HitZoneStore::new();
    let descriptors = descriptors(&["rifle"], Some(Vec::new()));

    check_weapon_mount_tables(&descriptors, &store);
    // The writer's first wield of the same weapon marks the same key.
    warn_missing_weapon_mount_entry(&store, "player", "rifle");

    logs.assert_logged_once(Level::Warn, RIFLE_MISSING);
}

#[test]
fn install_check_logs_nothing_for_a_character_without_a_table() {
    let logs = LogCapture::start();
    let store = HitZoneStore::new();

    check_weapon_mount_tables(&descriptors(&["rifle", "pistol"], None), &store);

    logs.assert_nothing_above(Level::Info);
}

#[test]
fn an_entry_naming_no_weapon_warns_once_per_level_load() {
    let logs = LogCapture::start();
    let mut store = HitZoneStore::new();
    let descriptors = descriptors(
        &["rifle"],
        Some(vec![
            entry("rifle", "models/rifle/player.gltf"),
            entry("retired_rifle", "models/rifle/player.gltf"),
        ]),
    );
    let unknown = "[Weapon mounts] entity `player` has a mounts entry for `retired_rifle`, which names no weapon; the entry never mounts";

    check_weapon_mount_tables(&descriptors, &store);
    check_weapon_mount_tables(&descriptors, &store);
    logs.assert_logged_once(Level::Warn, unknown);
    logs.assert_not_logged(Level::Warn, "no mounts entry");

    // The next level load clears the per-level set; the check may warn again.
    store.clear();
    check_weapon_mount_tables(&descriptors, &store);
    assert_eq!(
        logs.records()
            .iter()
            .filter(|record| record.level == Level::Warn && record.message.contains(unknown))
            .count(),
        2
    );
}

#[test]
fn a_missing_entry_warns_again_after_the_next_level_load() {
    let logs = LogCapture::start();
    let mut store = HitZoneStore::new();

    warn_missing_weapon_mount_entry(&store, "player", "rifle");
    warn_missing_weapon_mount_entry(&store, "player", "rifle");
    logs.assert_logged_once(Level::Warn, RIFLE_MISSING);

    store.clear();
    warn_missing_weapon_mount_entry(&store, "player", "rifle");
    assert_eq!(
        logs.records()
            .iter()
            .filter(|record| record.message.contains(RIFLE_MISSING))
            .count(),
        2
    );
}
