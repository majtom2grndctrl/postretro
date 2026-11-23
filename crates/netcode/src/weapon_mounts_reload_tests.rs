// Tests: weapon mounts across a hot reload, whose `mesh` deferral keeps the
// pre-edit table live until the next level load.
// See: context/lib/networking.md §Weapon placement is content

use log::Level;
use postretro_entities::data_descriptors::InventoryDescriptor;
use postretro_entities::provenance::DescriptorSpawnPath;
use postretro_test_log_capture::LogCapture;

use super::tests::{
    BACKPACK, GRIP_TAPE, PICKUP_MISSING, PISTOL_EXO, PISTOL_TROOPER, descriptors, entry, mounted,
    pairs, select, spawn_pawn, store,
};
use super::*;

/// The active descriptor set a hot reload leaves behind: the reload's new
/// inventory with the mesh, table included, deferred to the pre-reload value.
fn deferred_reload(
    active: &[EntityTypeDescriptor],
    staged: &[EntityTypeDescriptor],
) -> Vec<EntityTypeDescriptor> {
    staged
        .iter()
        .map(|next| {
            let mut next = next.clone();
            if let Some(old) = active
                .iter()
                .find(|old| old.canonical_name == next.canonical_name)
            {
                next.mesh = old.mesh.clone();
            }
            next
        })
        .collect()
}

// A switch after a deferred table edit mounts the pre-edit entry.
#[test]
fn a_switch_after_a_deferred_table_edit_mounts_the_pre_edit_entry_until_the_next_level_load() {
    let before = descriptors();
    let mut edited = before.clone();
    edited[0].mesh.as_mut().unwrap().weapon_mounts =
        Some(vec![entry("pistol", &[("hand_r", PISTOL_TROOPER)])]);
    let active = deferred_reload(&before, &edited);
    let store = store();
    let mut registry = EntityRegistry::new();
    let (pawn, _) = spawn_pawn(
        &mut registry,
        &before,
        "player",
        DescriptorSpawnPath::PlayerSpawn,
        &["rifle", "pistol"],
        Some(0),
    );
    synchronize_weapon_attachment_for_pawn(&mut registry, pawn, &before, &store);

    select(&mut registry, pawn, Some(1));
    synchronize_weapon_attachment_for_pawn(&mut registry, pawn, &active, &store);
    assert_eq!(
        mounted(&registry, pawn),
        pairs(&[("back", BACKPACK), ("hand_r", PISTOL_EXO)]),
        "the pre-edit entry"
    );

    // The next level load installs the edited descriptors.
    let store = self::store();
    let mut next_level = EntityRegistry::new();
    let (pawn, _) = spawn_pawn(
        &mut next_level,
        &edited,
        "player",
        DescriptorSpawnPath::PlayerSpawn,
        &["rifle", "pistol"],
        Some(1),
    );
    synchronize_weapon_attachment_for_pawn(&mut next_level, pawn, &edited, &store);
    assert_eq!(
        mounted(&next_level, pawn),
        pairs(&[("back", BACKPACK), ("hand_r", PISTOL_TROOPER)])
    );
}

#[test]
fn a_loadout_weapon_added_with_its_entry_in_one_reload_mounts_nothing_until_the_next_level_load() {
    let logs = LogCapture::start();
    let before = descriptors();
    let mut staged = before.clone();
    staged[0].inventory = Some(InventoryDescriptor {
        loadout: vec!["rifle".into(), "pistol".into(), "pickup".into()],
    });
    staged[0]
        .mesh
        .as_mut()
        .unwrap()
        .weapon_mounts
        .as_mut()
        .unwrap()
        .push(entry("pickup", &[("hand_r", PISTOL_TROOPER)]));
    let active = deferred_reload(&before, &staged);
    let store = store();
    let mut registry = EntityRegistry::new();
    let (pawn, _) = spawn_pawn(
        &mut registry,
        &active,
        "player",
        DescriptorSpawnPath::PlayerSpawn,
        &["rifle", "pistol", "pickup"],
        Some(2),
    );
    for slot in [2, 0, 2] {
        select(&mut registry, pawn, Some(slot));
        synchronize_weapon_attachment_for_pawn(&mut registry, pawn, &active, &store);
    }
    assert_eq!(
        mounted(&registry, pawn),
        pairs(&[("back", BACKPACK), ("hand_r", GRIP_TAPE)])
    );
    logs.assert_logged_once(Level::Warn, PICKUP_MISSING);

    let store = self::store();
    crate::scripting_systems::weapon_mounts::check_weapon_mount_tables(&staged, &store);
    synchronize_weapon_attachment_for_pawn(&mut registry, pawn, &staged, &store);
    assert_eq!(
        mounted(&registry, pawn),
        pairs(&[("back", BACKPACK), ("hand_r", PISTOL_TROOPER)]),
        "the next level load mounts the added entry"
    );
    logs.assert_logged_once(Level::Warn, PICKUP_MISSING);
}
