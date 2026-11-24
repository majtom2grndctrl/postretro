// NPC weapon mounts: composition of an NPC's attachments from `mesh.mounts`,
// the muzzle a fired weapon resolves to, and the load-time diagnostics for both.
// See: context/lib/entity_model.md §2 (Weapon mounts) · context/lib/resource_management.md §7

use std::collections::BTreeMap;

use postretro_entities::EntityTypeDescriptor;
use postretro_entities::data_descriptors::MeshDescriptor;
use postretro_model::gltf_loader::SocketBinding;

use crate::scripting_systems::hit_zones::{HitZoneStore, ModelHitZones};
use crate::scripting_systems::muzzle_pose::{BAKE_MUZZLE_SOCKET, MuzzleMount};
use crate::scripting_systems::weapon_mounts::{
    warn_weapon_mount_model_missing, warn_weapon_mount_socket_missing,
};

/// The canonical weapon names an NPC descriptor's attacks name, sorted. Empty
/// for a descriptor without a brain.
fn attack_weapons(descriptor: &EntityTypeDescriptor) -> Vec<&str> {
    descriptor
        .behavior
        .as_ref()
        .map(|behavior| behavior.attack_weapons())
        .unwrap_or_default()
}

/// An entry's (socket, bake) pairs in socket-name order. Empty for a
/// `muzzleSocket` entry or a weapon with no entry.
fn sorted_attachments<'a>(mesh: &'a MeshDescriptor, weapon: &str) -> Vec<(&'a str, &'a str)> {
    let mut pairs: Vec<(&str, &str)> = mesh
        .mount(weapon)
        .and_then(|entry| entry.attachments.as_ref())
        .map(|attachments| {
            attachments
                .iter()
                .map(|(socket, model)| (socket.as_str(), model.as_str()))
                .collect()
        })
        .unwrap_or_default();
    pairs.sort_unstable();
    pairs
}

/// The attachment list a mesh spawns with, in socket order: the base
/// `mesh.attachments`, then, for an NPC, each attack weapon's entry, the entry
/// winning on a socket both name. A `muzzleSocket` entry adds nothing, an entry
/// for a weapon no attack names mounts nothing, and no table leaves the base
/// alone. Host attach and client materialize both build the mesh here, so every
/// peer composes the same list from the descriptor with no wire change.
pub fn compose_attachments(
    descriptor: &EntityTypeDescriptor,
    mesh: &MeshDescriptor,
) -> Vec<(String, String)> {
    // Descriptor maps are unordered; the BTreeMap gives the component a stable
    // attachment order so later presentation collection is deterministic.
    let mut mounted: BTreeMap<&str, &str> = mesh
        .attachments
        .iter()
        .map(|(socket, model)| (socket.as_str(), model.as_str()))
        .collect();
    for weapon in attack_weapons(descriptor) {
        for (socket, model) in sorted_attachments(mesh, weapon) {
            mounted.insert(socket, model);
        }
    }
    mounted
        .into_iter()
        .map(|(socket, model)| (socket.to_string(), model.to_string()))
        .collect()
}

/// Whether `model` is loaded and carries a rigid `muzzle` node socket.
fn has_rigid_muzzle(model: Option<&ModelHitZones>) -> bool {
    model.is_some_and(|model| {
        matches!(
            model.sockets.get(BAKE_MUZZLE_SOCKET),
            Some(SocketBinding::RigidRest(_))
        )
    })
}

/// Where `weapon`'s shots leave the NPC `descriptor`, from its mount entry and
/// the loaded models:
/// - a `muzzleSocket` entry resolves to that socket when the body model has it;
/// - an attachments entry resolves to the first of its (socket, bake) pairs, in
///   socket-name order, whose holder socket the body has and whose bake is
///   loaded with a rigid `muzzle` socket.
///
/// `None` for no table, no entry, an unloaded body, or no usable mount; the
/// caller treats that as a missing muzzle.
pub fn npc_muzzle_mount<'a>(
    descriptor: &'a EntityTypeDescriptor,
    weapon: &str,
    store: &HitZoneStore,
) -> Option<MuzzleMount<'a>> {
    let mesh = descriptor.mesh.as_ref()?;
    let entry = mesh.mount(weapon)?;
    let body = store.get_by_name(&mesh.model)?;
    if let Some(socket) = entry.muzzle_socket.as_deref() {
        return body
            .has_socket(socket)
            .then_some(MuzzleMount::BodySocket(socket));
    }
    sorted_attachments(mesh, weapon)
        .into_iter()
        .find(|(socket, bake)| body.has_socket(socket) && has_rigid_muzzle(store.get_by_name(bake)))
        .map(|(holder_socket, bake)| MuzzleMount::Mounted {
            holder_socket,
            bake,
        })
}

/// Level-install check of every NPC's mounts against the loaded models, run
/// right after the model sweep. Each warning fires once per level load and
/// judges only loaded models: a body that never loaded logs no body-socket
/// warning (its own load failure is reported where it fails).
pub fn check_npc_mount_models(descriptors: &[EntityTypeDescriptor], store: &HitZoneStore) {
    for descriptor in descriptors {
        let (Some(character), Some(mesh)) = (
            descriptor.canonical_name.as_deref(),
            descriptor.mesh.as_ref(),
        ) else {
            continue;
        };
        if mesh.mounts.is_none() {
            continue;
        }
        let body = store.get_by_name(&mesh.model);
        for weapon in attack_weapons(descriptor) {
            let Some(entry) = mesh.mount(weapon) else {
                continue;
            };
            if let Some(socket) = entry.muzzle_socket.as_deref() {
                if body.is_some_and(|body| !body.has_socket(socket))
                    && store.mark_attachment_resolution_warning(format!(
                        "npc-muzzle-socket:{character}:{weapon}:{socket}"
                    ))
                {
                    log::warn!(
                        "[Model] archetype `{character}` weapon `{weapon}`: its body model `{}` lacks muzzle socket `{socket}` — its shots launch from the eye",
                        mesh.model
                    );
                }
                continue;
            }
            check_attachment_entry(store, character, weapon, mesh, body);
        }
    }
}

/// One attachments entry: its sockets against the body, its bakes against the
/// loaded set, and the `muzzle` sockets its loaded bakes carry.
fn check_attachment_entry(
    store: &HitZoneStore,
    character: &str,
    weapon: &str,
    mesh: &MeshDescriptor,
    body: Option<&ModelHitZones>,
) {
    let mut loaded_bakes = 0;
    let mut muzzle_bakes: Vec<&str> = Vec::new();
    for (socket, bake) in sorted_attachments(mesh, weapon) {
        let bake_model = store.get_by_name(bake);
        if bake_model.is_some() {
            loaded_bakes += 1;
            // Only a bake on a socket the body has can fire: the one the
            // warning names below must be the one `npc_muzzle_mount` picks.
            let mountable = body.is_none_or(|body| body.has_socket(socket));
            if mountable && has_rigid_muzzle(bake_model) && !muzzle_bakes.contains(&bake) {
                muzzle_bakes.push(bake);
            }
        }
        if body.is_some_and(|body| !body.has_socket(socket)) {
            warn_weapon_mount_socket_missing(store, character, weapon, socket, &mesh.model);
            // The binding resolver would warn about the same socket on the
            // spawned mesh; the warning above already named it.
            store.mark_attachment_resolution_warning(format!(
                "attachment-socket:{}:{socket}:{bake}",
                mesh.model
            ));
        } else if bake_model.is_none() {
            warn_weapon_mount_model_missing(store, character, weapon, socket, bake);
        }
    }
    if loaded_bakes > 0 && muzzle_bakes.is_empty() {
        if store.mark_attachment_resolution_warning(format!("npc-muzzle-none:{character}:{weapon}"))
        {
            log::warn!(
                "[Model] archetype `{character}` weapon `{weapon}`: no mounted bake carries a `{BAKE_MUZZLE_SOCKET}` socket — its shots launch from the eye"
            );
        }
    } else if muzzle_bakes.len() > 1
        && store.mark_attachment_resolution_warning(format!("npc-muzzle-many:{character}:{weapon}"))
    {
        log::warn!(
            "[Model] archetype `{character}` weapon `{weapon}`: {} mounted bakes carry a `{BAKE_MUZZLE_SOCKET}` socket; its shots launch from `{}`, the first in socket-name order",
            muzzle_bakes.len(),
            muzzle_bakes[0]
        );
    }
}

#[cfg(test)]
#[path = "npc_mounts_tests.rs"]
mod tests;
