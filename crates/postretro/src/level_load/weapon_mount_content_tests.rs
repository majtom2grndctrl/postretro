//! The dev player's `mesh.weaponMounts` against the committed bakes, read
//! from the table rather than a directory: each listed model is a declared
//! (VERIFIED) fit on exo_red's `hand_r`, sits beside its weapon's viewmodel as
//! `exo_red.gltf` with its own `exo_red.bin`, and is never a viewmodel. The
//! table covers the dev arsenal, so the dev player's install warns about no
//! missing entry.
//! See: context/lib/resource_management.md §7 · docs/weapon-mounts.md

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use postretro_entities::data_descriptors::WeaponMountDescriptor;
use postretro_model::gltf_loader::{LoadedModel, load_model};
use postretro_model::mount::{
    MountThresholds, REFERENCE_MOUNT_CLIP, REFERENCE_MOUNT_TIME, check_declared_weapon_mount,
};
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;

use super::model_tests::{dev_content_root, dev_descriptors, real_sweep, record};

/// The rig every dev player bake is fitted to.
const HOLDER_MODEL: &str = "models/exo_red/model.gltf";
/// The socket the table mounts on, and the one `--mount-joint hand_r` checks.
const MOUNT_SOCKET: &str = "hand_r";
/// A bake is named for its rig, beside the weapon's viewmodel.
const BAKE_FILE: &str = "exo_red.gltf";
const BAKE_BUFFER: &str = "exo_red.bin";

/// The weapons exo_red's table must cover, by canonical name.
const EXO_RED_TABLE_WEAPONS: [&str; 10] = [
    "reference_shotgun",
    "reference_pistol",
    "reference_rifle",
    "reference_plasma_bolt",
    "reference_rocket",
    "incendiary_rifle",
    "wieldable_fixture_auto",
    "wieldable_fixture_press",
    "coop_health_weapon",
    "coop_heat_test",
];

fn player(descriptors: &[EntityTypeDescriptor]) -> &EntityTypeDescriptor {
    descriptors
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some("player"))
        .expect("the dev mod registers the player")
}

fn player_table(descriptors: &[EntityTypeDescriptor]) -> &[WeaponMountDescriptor] {
    player(descriptors)
        .mesh
        .as_ref()
        .expect("the dev player has a mesh")
        .weapon_mounts
        .as_deref()
        .expect("the dev player declares mesh.weaponMounts")
}

fn weapon_viewmodel<'a>(descriptors: &'a [EntityTypeDescriptor], weapon: &str) -> Option<&'a str> {
    descriptors
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some(weapon))?
        .weapon
        .as_ref()?
        .viewmodel
        .as_deref()
}

/// A content-relative path's folder and file name, split on `/` (content
/// paths always use forward slashes, whatever the host).
fn split_content_path(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn content_path(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// The `buffers[].uri` values of a glTF document.
fn buffer_uris(gltf: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(gltf).map_err(|error| format!("read: {error}"))?;
    let document: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("parse JSON: {error}"))?;
    Ok(document["buffers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|buffer| buffer["uri"].as_str().unwrap_or_default().to_string())
        .collect())
}

/// Every way `table` breaks the Content contract, one line per violation:
/// each model listed for `socket` resolves under `content_root`, loads,
/// declares `extras.mount` with its applied euler, passes the declared check
/// on `holder` (named `holder_name` in each line) at the reference pose, sits
/// beside its entry weapon's viewmodel as [`BAKE_FILE`] over [`BAKE_BUFFER`],
/// and is no weapon's viewmodel.
fn table_violations(
    table: &[WeaponMountDescriptor],
    descriptors: &[EntityTypeDescriptor],
    holder_name: &str,
    holder: &LoadedModel,
    content_root: &Path,
) -> Vec<String> {
    let viewmodels: BTreeSet<&str> = descriptors
        .iter()
        .filter_map(|descriptor| descriptor.weapon.as_ref()?.viewmodel.as_deref())
        .collect();
    let mut violations = Vec::new();
    for entry in table {
        let weapon = entry.weapon.as_str();
        let mut sockets: Vec<_> = entry.attachments.iter().collect();
        sockets.sort();
        for (socket, model) in sockets {
            let mut fail =
                |what: String| violations.push(format!("{weapon} {socket} {model}: {what}"));
            if socket != MOUNT_SOCKET {
                fail(format!(
                    "mounts on {socket:?}; {holder_name}'s bakes are fitted to {MOUNT_SOCKET:?}"
                ));
                continue;
            }
            if viewmodels.contains(model.as_str()) {
                fail("is a weapon viewmodel, not a rig bake".to_string());
            }
            let (folder, file) = split_content_path(model);
            if file != BAKE_FILE {
                fail(format!("is named {file:?}, not {BAKE_FILE:?}"));
            }
            match weapon_viewmodel(descriptors, weapon) {
                Some(viewmodel) if split_content_path(viewmodel).0 == folder => {}
                Some(viewmodel) => {
                    fail(format!("is not in the folder of its viewmodel {viewmodel}"))
                }
                None => fail("its weapon names no registered weapon with a viewmodel".to_string()),
            }
            let path = content_path(content_root, model);
            match buffer_uris(&path) {
                Ok(uris) if uris == [BAKE_BUFFER] => {}
                Ok(uris) => fail(format!(
                    "references buffers {uris:?}, not its own {BAKE_BUFFER}"
                )),
                Err(error) => fail(format!("glTF JSON: {error}")),
            }
            let loaded = match load_model(&path) {
                Ok(loaded) => loaded,
                Err(error) => {
                    fail(format!("does not load: {error}"));
                    continue;
                }
            };
            if !loaded.mount.is_some_and(|mount| mount.euler.is_some()) {
                fail(
                    "declares no extras.mount with its applied euler, so its check would be the UNVERIFIED geometric assist"
                        .to_string(),
                );
                continue;
            }
            match check_declared_weapon_mount(
                holder,
                &loaded,
                REFERENCE_MOUNT_CLIP,
                MOUNT_SOCKET,
                REFERENCE_MOUNT_TIME,
                MountThresholds::default(),
            ) {
                Ok(check) if check.passed() => {}
                Ok(check) => fail(format!(
                    "declared mount check failed on {holder_name}: {}",
                    check.failures.join(", ")
                )),
                Err(error) => fail(format!("declared mount check errored: {error}")),
            }
        }
    }
    violations
}

fn exo_red(content_root: &Path) -> LoadedModel {
    load_model(&content_path(content_root, HOLDER_MODEL))
        .expect("exo_red loads and binds its clips")
}

// Every model the dev player's table lists is a VERIFIED exo_red fit, beside
// its weapon's viewmodel, named for the rig, and no viewmodel.
#[test]
fn every_dev_player_weapon_mount_is_a_verified_exo_red_bake_beside_its_viewmodel() {
    let descriptors = dev_descriptors();
    let content_root = dev_content_root();
    let table = player_table(&descriptors);
    assert!(!table.is_empty(), "the dev player's table lists bakes");
    let violations = table_violations(
        table,
        &descriptors,
        HOLDER_MODEL,
        &exo_red(&content_root),
        &content_root,
    );
    assert!(
        violations.is_empty(),
        "table violations:\n{}",
        violations.join("\n")
    );
}

// The table covers the dev arsenal, one entry per weapon.
#[test]
fn dev_player_weapon_mount_table_covers_exactly_the_dev_arsenal() {
    let descriptors = dev_descriptors();
    let table = player_table(&descriptors);
    let named: Vec<&str> = table.iter().map(|entry| entry.weapon.as_str()).collect();
    let covered: BTreeSet<&str> = named.iter().copied().collect();
    assert_eq!(
        named.len(),
        covered.len(),
        "one entry per weapon: {named:?}"
    );
    assert_eq!(covered, BTreeSet::from(EXO_RED_TABLE_WEAPONS));
}

// With the dev player's real table, its level install logs no missing-entry
// warning on either net role.
#[test]
fn dev_player_level_install_warns_about_no_missing_weapon_mount_entry() {
    let descriptors = dev_descriptors();
    let loadout = player(&descriptors)
        .inventory
        .as_ref()
        .map(|inventory| inventory.loadout.clone())
        .unwrap_or_default();
    assert!(
        !loadout.is_empty(),
        "the dev player carries a loadout the install checks"
    );
    for connected_client in [false, true] {
        let logs = postretro_test_log_capture::LogCapture::start();
        let _ = real_sweep(
            vec![record("player_spawn", &[], 0)],
            &descriptors,
            connected_client,
        );
        logs.assert_not_logged(
            log::Level::Warn,
            "[Weapon mounts] entity `player` has no weaponMounts entry",
        );
    }
}

// An entry repointed at its weapon's viewmodel fails the table check, so the
// guard cannot pass on a viewmodel standing in for a bake.
#[test]
fn a_weapon_mount_entry_repointed_at_a_viewmodel_fails_the_table_check() {
    let descriptors = dev_descriptors();
    let content_root = dev_content_root();
    let rifle_viewmodel = weapon_viewmodel(&descriptors, "reference_rifle")
        .expect("the reference rifle has a viewmodel")
        .to_string();
    let table = [WeaponMountDescriptor {
        weapon: "reference_rifle".to_string(),
        attachments: [(MOUNT_SOCKET.to_string(), rifle_viewmodel)].into(),
    }];
    let violations = table_violations(
        &table,
        &descriptors,
        HOLDER_MODEL,
        &exo_red(&content_root),
        &content_root,
    );
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("is a weapon viewmodel")),
        "{violations:?}"
    );
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("UNVERIFIED")),
        "the viewmodel declares no mount: {violations:?}"
    );
}

/// `document` with every `extras.mount` removed, as a re-export that drops
/// the fit would leave it.
fn without_extras_mount(document: &str) -> String {
    let mut json: serde_json::Value = serde_json::from_str(document).expect("bake JSON parses");
    let mut removed = 0;
    for list in ["nodes", "meshes"] {
        for item in json[list].as_array_mut().into_iter().flatten() {
            if let Some(extras) = item
                .get_mut("extras")
                .and_then(|extras| extras.as_object_mut())
                && extras.remove("mount").is_some()
            {
                removed += 1;
            }
        }
    }
    assert!(
        removed > 0,
        "the committed bake carries extras.mount to drop"
    );
    serde_json::to_string_pretty(&json).expect("bake JSON writes")
}

// A re-export of a listed bake that drops `extras.mount` fails the table
// check rather than falling back to the geometric assist.
#[test]
fn a_listed_bake_re_exported_without_extras_mount_fails_the_table_check() {
    let descriptors = dev_descriptors();
    let dev_root = dev_content_root();
    let model = "models/cyberpunk_weapons/rifle/exo_red.gltf";
    let scratch = tempfile::tempdir().expect("scratch content root");
    let folder = content_path(scratch.path(), split_content_path(model).0);
    std::fs::create_dir_all(&folder).expect("scratch weapon folder");
    let source = std::fs::read_to_string(content_path(&dev_root, model)).expect("bake reads");
    std::fs::write(
        content_path(scratch.path(), model),
        without_extras_mount(&source),
    )
    .expect("stripped bake writes");
    let source_folder = content_path(&dev_root, split_content_path(model).0);
    for file in [BAKE_BUFFER, "texture.png"] {
        std::fs::copy(source_folder.join(file), folder.join(file))
            .unwrap_or_else(|error| panic!("{file} copies: {error}"));
    }
    let table = [WeaponMountDescriptor {
        weapon: "reference_rifle".to_string(),
        attachments: [(MOUNT_SOCKET.to_string(), model.to_string())].into(),
    }];

    let violations = table_violations(
        &table,
        &descriptors,
        HOLDER_MODEL,
        &exo_red(&dev_root),
        scratch.path(),
    );
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("declares no extras.mount"),
        "{violations:?}"
    );
}
