// Character-owned weapon mounts on the mesh descriptor, and the mesh key
// vocabulary both descriptor readers enforce.
// See: context/lib/entity_model.md §2 (MeshComponent, Weapon vocabulary)

use std::collections::{HashMap, HashSet};

use crate::data_descriptors::{DescriptorError, is_portable_content_relative_asset_path};

use super::entity::MeshDescriptor;

/// Every key `components.mesh` accepts. Both readers reject any other key, so
/// a misspelled field (`weaponMount`) fails load instead of silently opting
/// out. Keep in step with the `MeshDescriptor` SDK type.
pub const MESH_DESCRIPTOR_KEYS: &[&str] = &[
    "model",
    "attachments",
    "shadowBiasScale",
    "shadowOnly",
    "animations",
    "defaultState",
    "locomotion",
    "weaponMounts",
];

/// Every key one `components.mesh.weaponMounts` entry accepts.
pub const WEAPON_MOUNT_KEYS: &[&str] = &["weapon", "attachments"];

/// One entry of a character's weapon-mount table: the props this character
/// mounts while it wields `weapon`, each baked for its own rig.
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponMountDescriptor {
    /// Canonical name of the weapon descriptor. The SDK builders lower the
    /// authored weapon handle to this name. Rust does not check that it names
    /// a registered weapon; an unresolved name is a runtime warning.
    pub weapon: String,
    /// Holder socket → content-relative prop model path, the same shape as
    /// [`MeshDescriptor::attachments`]. Never empty.
    pub attachments: HashMap<String, String>,
}

impl MeshDescriptor {
    /// The table entry for `weapon` (a canonical weapon name), if this
    /// character declares one.
    pub fn weapon_mount(&self, weapon: &str) -> Option<&WeaponMountDescriptor> {
        self.weapon_mounts
            .as_deref()?
            .iter()
            .find(|entry| entry.weapon == weapon)
    }

    /// Every prop model the weapon-mount table lists, across all entries, for
    /// preload sweeps. A model named by several entries repeats.
    pub fn weapon_mount_models(&self) -> impl Iterator<Item = &str> {
        self.weapon_mounts
            .iter()
            .flatten()
            .flat_map(|entry| entry.attachments.values().map(String::as_str))
    }
}

/// How load errors name the character that owns a mesh: its canonical name,
/// or a stand-in for a descriptor that has none.
pub fn mesh_owner_label(canonical_name: Option<&str>) -> String {
    match canonical_name {
        Some(name) => format!("entity `{name}`"),
        None => "an entity without a canonicalName".to_string(),
    }
}

/// Reject a `components.mesh` key outside [`MESH_DESCRIPTOR_KEYS`], naming the
/// key and the character.
pub fn check_mesh_key(owner: &str, key: &str) -> Result<(), DescriptorError> {
    if MESH_DESCRIPTOR_KEYS.contains(&key) {
        return Ok(());
    }
    Err(DescriptorError::InvalidShape {
        reason: format!(
            "{owner}: `components.mesh.{key}` is not a mesh field; expected one of {}",
            MESH_DESCRIPTOR_KEYS.join(", ")
        ),
    })
}

/// Reject a weapon-mount entry key outside [`WEAPON_MOUNT_KEYS`], naming the
/// key and the character.
pub fn check_weapon_mount_key(owner: &str, index: usize, key: &str) -> Result<(), DescriptorError> {
    if WEAPON_MOUNT_KEYS.contains(&key) {
        return Ok(());
    }
    Err(DescriptorError::InvalidShape {
        reason: format!(
            "{owner}: `components.mesh.weaponMounts[{index}].{key}` is not a weapon-mount field; expected `weapon` and `attachments`"
        ),
    })
}

/// A shape error inside `components.mesh.weaponMounts`, naming the character.
pub fn weapon_mount_shape_error(owner: &str, detail: impl std::fmt::Display) -> DescriptorError {
    DescriptorError::InvalidShape {
        reason: format!("{owner}: {detail}"),
    }
}

/// The load rules both readers share once they have gathered a table: each
/// weapon has a non-empty name and appears once, each entry mounts at least
/// one socket, socket names are non-empty, and each model path follows the
/// content-relative rule weapon models follow (`components.weapon.viewmodel`).
pub fn validate_weapon_mounts(
    owner: &str,
    entries: &[WeaponMountDescriptor],
) -> Result<(), DescriptorError> {
    let mut seen = HashSet::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let path = format!("components.mesh.weaponMounts[{index}]");
        if entry.weapon.is_empty() {
            return Err(weapon_mount_shape_error(
                owner,
                format_args!("`{path}.weapon` must be a non-empty weapon name"),
            ));
        }
        if !seen.insert(entry.weapon.as_str()) {
            return Err(weapon_mount_shape_error(
                owner,
                format_args!(
                    "`{path}.weapon` repeats weapon `{}`; each weapon takes one entry",
                    entry.weapon
                ),
            ));
        }
        if entry.attachments.is_empty() {
            return Err(weapon_mount_shape_error(
                owner,
                format_args!(
                    "`{path}.attachments` for weapon `{}` is empty; an entry mounts at least one socket",
                    entry.weapon
                ),
            ));
        }
        // Sorted so a table with several bad sockets reports the same one in
        // both runtimes.
        let mut sockets: Vec<_> = entry.attachments.iter().collect();
        sockets.sort();
        for (socket, model) in sockets {
            if socket.is_empty() {
                return Err(weapon_mount_shape_error(
                    owner,
                    format_args!("`{path}.attachments` must not contain an empty socket name"),
                ));
            }
            if !is_portable_content_relative_asset_path(model) {
                return Err(weapon_mount_shape_error(
                    owner,
                    format_args!(
                        "`{path}.attachments.{socket}` must be a non-empty, content-relative model path using forward slashes with no parent traversal"
                    ),
                ));
            }
        }
    }
    Ok(())
}
