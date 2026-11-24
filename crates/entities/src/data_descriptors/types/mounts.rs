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
    "mounts",
];

/// Every key one `components.mesh.mounts` entry accepts.
pub const MOUNT_KEYS: &[&str] = &["item", "attachments", "muzzleSocket"];

/// One entry of a character's weapon-mount table: the props this character
/// mounts while it wields `item`, each baked for its own rig.
#[derive(Debug, Clone, PartialEq)]
pub struct MountDescriptor {
    /// Canonical name of the weapon descriptor. The SDK builders lower the
    /// authored weapon handle to this name. Rust does not check that it names
    /// a registered weapon; an unresolved name is a runtime warning.
    pub item: String,
    /// Holder socket → content-relative prop model path, the same shape as
    /// [`MeshDescriptor::attachments`]. `None` for a `muzzleSocket` entry;
    /// present, it is never empty.
    pub attachments: Option<HashMap<String, String>>,
    /// A socket on the character's own body model that this item's shots leave
    /// from, for a weapon built into the body. The entry mounts nothing. Exactly
    /// one of `attachments` and `muzzle_socket` is set; a non-empty name.
    pub muzzle_socket: Option<String>,
}

impl MeshDescriptor {
    /// The table entry for `item` (a canonical weapon name), if this
    /// character declares one.
    pub fn mount(&self, item: &str) -> Option<&MountDescriptor> {
        self.mounts
            .as_deref()?
            .iter()
            .find(|entry| entry.item == item)
    }

    /// Every prop model the weapon-mount table lists, across all entries, for
    /// preload sweeps. A model named by several entries repeats.
    pub fn mount_models(&self) -> impl Iterator<Item = &str> {
        self.mounts
            .iter()
            .flatten()
            .filter_map(|entry| entry.attachments.as_ref())
            .flat_map(|attachments| attachments.values().map(String::as_str))
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

/// Reject a weapon-mount entry key outside [`MOUNT_KEYS`], naming the
/// key and the character.
pub fn check_mount_key(owner: &str, index: usize, key: &str) -> Result<(), DescriptorError> {
    if MOUNT_KEYS.contains(&key) {
        return Ok(());
    }
    Err(DescriptorError::InvalidShape {
        reason: format!(
            "{owner}: `components.mesh.mounts[{index}].{key}` is not a weapon-mount field; expected `item`, `attachments` or `muzzleSocket`"
        ),
    })
}

/// A shape error inside `components.mesh.mounts`, naming the character.
pub fn mount_shape_error(owner: &str, detail: impl std::fmt::Display) -> DescriptorError {
    DescriptorError::InvalidShape {
        reason: format!("{owner}: {detail}"),
    }
}

/// The load rules both readers share once they have gathered a table: each
/// weapon has a non-empty name and appears once, each entry mounts at least
/// one socket, socket names are non-empty, and each model path follows the
/// content-relative rule weapon models follow (`components.weapon.viewmodel`).
pub fn validate_mounts(owner: &str, entries: &[MountDescriptor]) -> Result<(), DescriptorError> {
    let mut seen = HashSet::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let path = format!("components.mesh.mounts[{index}]");
        if entry.item.is_empty() {
            return Err(mount_shape_error(
                owner,
                format_args!("`{path}.item` must be a non-empty weapon name"),
            ));
        }
        if !seen.insert(entry.item.as_str()) {
            return Err(mount_shape_error(
                owner,
                format_args!(
                    "`{path}.item` repeats weapon `{}`; each weapon takes one entry",
                    entry.item
                ),
            ));
        }
        let attachments = match (&entry.attachments, &entry.muzzle_socket) {
            (Some(_), Some(_)) => {
                return Err(mount_shape_error(
                    owner,
                    format_args!(
                        "`{path}` names both `attachments` and `muzzleSocket`; an entry takes exactly one"
                    ),
                ));
            }
            (None, None) => {
                return Err(mount_shape_error(
                    owner,
                    format_args!(
                        "`{path}` names neither `attachments` nor `muzzleSocket`; an entry takes exactly one"
                    ),
                ));
            }
            (None, Some(socket)) => {
                if socket.is_empty() {
                    return Err(mount_shape_error(
                        owner,
                        format_args!("`{path}.muzzleSocket` must be a non-empty socket name"),
                    ));
                }
                continue;
            }
            (Some(attachments), None) => attachments,
        };
        if attachments.is_empty() {
            return Err(mount_shape_error(
                owner,
                format_args!(
                    "`{path}.attachments` for weapon `{}` is empty; an entry mounts at least one socket",
                    entry.item
                ),
            ));
        }
        // Sorted so a table with several bad sockets reports the same one in
        // both runtimes.
        let mut sockets: Vec<_> = attachments.iter().collect();
        sockets.sort();
        for (socket, model) in sockets {
            if socket.is_empty() {
                return Err(mount_shape_error(
                    owner,
                    format_args!("`{path}.attachments` must not contain an empty socket name"),
                ));
            }
            if !is_portable_content_relative_asset_path(model) {
                return Err(mount_shape_error(
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
