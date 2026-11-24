// Weapon-mount diagnostics: the level-install table check and the warn-once
// messages it shares with the third-person mount writer.
// See: context/lib/entity_model.md §2 (Weapon vocabulary)

use postretro_entities::EntityTypeDescriptor;

use super::hit_zones::HitZoneStore;

/// Warn once per level load that `character` declares a weapon-mount table
/// without an entry for `weapon`. The install-time loadout check, the NPC
/// attack-weapon check and the writer's first wield share this key, so a
/// weapon warns at load and not again when wielded or spawned.
pub fn warn_missing_weapon_mount_entry(store: &HitZoneStore, character: &str, weapon: &str) {
    if store.mark_attachment_resolution_warning(format!("weapon-mount-entry:{character}:{weapon}"))
    {
        log::warn!(
            "[Weapon mounts] entity `{character}` has no mounts entry for weapon `{weapon}`; it mounts nothing"
        );
    }
}

/// Warn once per level load that an entry names a socket the holder's rig
/// lacks. The writer mounts nothing there, so the generic holder-socket
/// warning never sees the mount.
pub fn warn_weapon_mount_socket_missing(
    store: &HitZoneStore,
    character: &str,
    weapon: &str,
    socket: &str,
    holder_model: &str,
) {
    if store.mark_attachment_resolution_warning(format!(
        "weapon-mount-socket:{character}:{weapon}:{socket}"
    )) {
        log::warn!(
            "[Weapon mounts] entity `{character}` mounts weapon `{weapon}` on socket `{socket}`, which its model `{holder_model}` lacks; nothing mounts there"
        );
    }
}

/// Warn once per level load that the wielder's own model is not in the level's
/// loaded model set, so no entry socket can be checked and the entry mounts
/// nothing. Distinct from a missing socket, which needs a loaded rig to judge.
pub fn warn_weapon_mount_holder_missing(
    store: &HitZoneStore,
    character: &str,
    weapon: &str,
    holder_model: &str,
) {
    if store.mark_attachment_resolution_warning(format!(
        "weapon-mount-holder:{character}:{weapon}:{holder_model}"
    )) {
        log::warn!(
            "[Weapon mounts] entity `{character}` cannot mount weapon `{weapon}`: its model `{holder_model}` is not loaded; nothing mounts"
        );
    }
}

/// Warn once per level load that an entry's prop model is not in the level's
/// loaded model set (it failed to load or was never preloaded).
pub fn warn_weapon_mount_model_missing(
    store: &HitZoneStore,
    character: &str,
    weapon: &str,
    socket: &str,
    model: &str,
) {
    if store.mark_attachment_resolution_warning(format!(
        "weapon-mount-model:{character}:{weapon}:{socket}:{model}"
    )) {
        log::warn!(
            "[Weapon mounts] entity `{character}` mounts model `{model}` on socket `{socket}` for weapon `{weapon}`, but the model is not loaded; nothing mounts there"
        );
    }
}

/// Level-install check of every declared weapon-mount table against the
/// descriptor set: a loadout weapon the table lacks warns now, as does a
/// weapon an NPC's attacks name (whether or not it spawns), and an entry
/// naming no weapon descriptor warns that it never mounts. Runs after the
/// per-level warn-once set clears, so each warning repeats at most once per
/// level load. A character without `mesh.mounts` logs nothing.
pub fn check_weapon_mount_tables(descriptors: &[EntityTypeDescriptor], store: &HitZoneStore) {
    let is_weapon = |name: &str| {
        descriptors.iter().any(|descriptor| {
            descriptor.canonical_name.as_deref() == Some(name) && descriptor.weapon.is_some()
        })
    };
    for descriptor in descriptors {
        let Some(character) = descriptor.canonical_name.as_deref() else {
            continue;
        };
        let Some(mesh) = descriptor.mesh.as_ref() else {
            continue;
        };
        let Some(table) = mesh.mounts.as_deref() else {
            continue;
        };
        for entry in table {
            if !is_weapon(&entry.item)
                && store.mark_attachment_resolution_warning(format!(
                    "weapon-mount-unknown:{character}:{}",
                    entry.item
                ))
            {
                log::warn!(
                    "[Weapon mounts] entity `{character}` has a mounts entry for `{}`, which names no weapon; the entry never mounts",
                    entry.item
                );
            }
        }
        // An NPC mounts every weapon its attacks name, whether or not it ever
        // spawns, so each one the table lacks warns now. A pawn's missing entry
        // shares the key, so the same text never doubles.
        if let Some(behavior) = descriptor.behavior.as_ref() {
            for weapon in behavior.attack_weapons() {
                if mesh.mount(weapon).is_none() {
                    warn_missing_weapon_mount_entry(store, character, weapon);
                }
            }
        }
        // A loadout name that resolves to no weapon is never wielded; the
        // spawn-time loadout check already reports it.
        let loadout = descriptor
            .inventory
            .as_ref()
            .map(|inventory| inventory.loadout.as_slice())
            .unwrap_or_default();
        for weapon in loadout {
            if is_weapon(weapon) && mesh.mount(weapon).is_none() {
                warn_missing_weapon_mount_entry(store, character, weapon);
            }
        }
    }
}

#[cfg(test)]
#[path = "weapon_mounts_tests.rs"]
mod tests;
