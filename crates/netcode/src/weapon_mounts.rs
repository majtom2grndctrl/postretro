// Third-person weapon mounts: the one writer of a pawn's mounted attachments,
// composed from the wielder's own descriptor and the replicated active weapon.
// See: context/lib/networking.md §Weapon placement is content

use std::collections::BTreeMap;

use postretro_entities::components::mesh::{MeshAttachment, MeshComponent};
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{EntityId, EntityRegistry, EntityTypeDescriptor, MeshDescriptor};

use crate::command_queue::{WeaponOwners, active_wieldable_for_pawn};
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::scripting_systems::weapon_mounts::{
    warn_missing_weapon_mount_entry, warn_weapon_mount_model_missing,
    warn_weapon_mount_socket_missing,
};

/// The class a pawn's descriptor-less record resolves as, matching body
/// materialization's default.
const DEFAULT_WIELDER_CLASS: &str = "player";

/// Recompose `pawn`'s mounted attachments from the wielder descriptor named
/// `wielder_class`: its base `mesh.attachments`, then its `mesh.weaponMounts`
/// entry for the active weapon, the entry winning per socket. Only the shared
/// active-weapon identity and the pawn's class feed it, so every peer resolves
/// the same mount from its own descriptors and nothing crosses the wire.
///
/// A character without a table mounts its base attachments for any weapon,
/// silently. With a table, a weapon lacking an entry, an entry socket the
/// holder's rig lacks, and an entry model the level did not load each warn once
/// per level load and mount nothing for that weapon or socket.
///
/// Replaces the attachments only when the (socket, model) set changes and
/// returns whether it did; the caller then runs the binding resolver.
pub(crate) fn update_weapon_mount_attachments(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    wielder_class: &str,
    descriptors: &[EntityTypeDescriptor],
    active_weapon_archetype: Option<&str>,
    hit_zone_store: &HitZoneStore,
) -> bool {
    let Ok(mesh) = registry.get_component::<MeshComponent>(pawn) else {
        return false;
    };
    let Some(wielder_mesh) = descriptors
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some(wielder_class))
        .and_then(|descriptor| descriptor.mesh.as_ref())
    else {
        return false;
    };

    let desired = compose_mounted_attachments(
        &mesh.model,
        wielder_class,
        wielder_mesh,
        active_weapon_archetype,
        hit_zone_store,
    );
    let mut current: Vec<(&str, &str)> = mesh
        .attachments
        .iter()
        .map(|attachment| (attachment.socket.as_str(), attachment.model.as_str()))
        .collect();
    current.sort_unstable();
    if current
        .iter()
        .copied()
        .eq(desired.iter().map(|(s, m)| (*s, *m)))
    {
        return false;
    }

    // Keep the resolved binding of every attachment that stays, so only the
    // changed sockets wait on the caller's binding pass.
    let attachments = desired
        .iter()
        .map(|(socket, model)| {
            mesh.attachments
                .iter()
                .find(|attachment| attachment.socket == *socket && attachment.model == *model)
                .cloned()
                .unwrap_or_else(|| {
                    MeshAttachment::unresolved((*socket).to_string(), (*model).to_string())
                })
        })
        .collect();
    let mut mesh = mesh.clone();
    mesh.attachments = attachments;
    let _ = registry.set_component(pawn, mesh);
    true
}

/// The mounted (socket, model) list in socket order, the order a descriptor
/// spawn gives base attachments.
fn compose_mounted_attachments<'a>(
    holder_model: &str,
    wielder_class: &str,
    wielder_mesh: &'a MeshDescriptor,
    active_weapon_archetype: Option<&str>,
    hit_zone_store: &HitZoneStore,
) -> BTreeMap<&'a str, &'a str> {
    let mut mounted: BTreeMap<&str, &str> = wielder_mesh
        .attachments
        .iter()
        .map(|(socket, model)| (socket.as_str(), model.as_str()))
        .collect();
    // No active weapon, or no table: the base attachments alone, silently.
    let (Some(weapon), Some(_)) = (active_weapon_archetype, wielder_mesh.weapon_mounts.as_ref())
    else {
        return mounted;
    };
    let Some(entry) = wielder_mesh.weapon_mount(weapon) else {
        warn_missing_weapon_mount_entry(hit_zone_store, wielder_class, weapon);
        return mounted;
    };

    let holder = hit_zone_store.get_by_name(holder_model);
    let mut sockets: Vec<(&String, &String)> = entry.attachments.iter().collect();
    sockets.sort_unstable();
    for (socket, model) in sockets {
        if !holder.is_some_and(|holder| holder.has_socket(socket)) {
            warn_weapon_mount_socket_missing(
                hit_zone_store,
                wielder_class,
                weapon,
                socket,
                holder_model,
            );
            mounted.remove(socket.as_str());
        } else if hit_zone_store.get_by_name(model).is_none() {
            warn_weapon_mount_model_missing(hit_zone_store, wielder_class, weapon, socket, model);
            mounted.remove(socket.as_str());
        } else {
            mounted.insert(socket.as_str(), model.as_str());
        }
    }
    mounted
}

/// The canonical class an authoritative pawn wields as: its descriptor
/// provenance, or the default player class for a pawn without one.
fn pawn_wielder_class(registry: &EntityRegistry, pawn: EntityId) -> String {
    registry
        .get_component::<DescriptorProvenance>(pawn)
        .map(|provenance| provenance.canonical_name.clone())
        .unwrap_or_else(|_| DEFAULT_WIELDER_CLASS.to_string())
}

/// The canonical archetype of an inventory weapon instance.
fn weapon_archetype(registry: &EntityRegistry, weapon: Option<EntityId>) -> Option<String> {
    weapon.and_then(|weapon| {
        registry
            .get_component::<DescriptorProvenance>(weapon)
            .ok()
            .map(|provenance| provenance.canonical_name.clone())
    })
}

/// Synchronize dirty pawn inventory changes into third-person presentation
/// attachments immediately before snapshot production. `WeaponOwners` is only the
/// dirty attachment queue; each drain resolves the active instance from live pawn
/// inventory, so two switches in one tick mount only the final weapon.
pub fn synchronize_weapon_owner_attachments(
    registry: &mut EntityRegistry,
    weapon_owners: &mut WeaponOwners,
    descriptors: &[EntityTypeDescriptor],
    hit_zone_store: &HitZoneStore,
) -> Vec<EntityId> {
    weapon_owners
        .take_attachment_changes(registry)
        .into_iter()
        .filter_map(|(pawn, weapon)| {
            let active_weapon_archetype = weapon_archetype(registry, weapon);
            let wielder_class = pawn_wielder_class(registry, pawn);
            update_weapon_mount_attachments(
                registry,
                pawn,
                &wielder_class,
                descriptors,
                active_weapon_archetype.as_deref(),
                hit_zone_store,
            )
            .then_some(pawn)
        })
        .collect()
}

/// Synchronize one local pawn's third-person attachments from its active inventory
/// weapon. Single-player has no dirty attachment queue, and the listen host installs
/// its own pawn before the regular pre-snapshot synchronization can run.
pub fn synchronize_weapon_attachment_for_pawn(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    descriptors: &[EntityTypeDescriptor],
    hit_zone_store: &HitZoneStore,
) -> bool {
    let active_weapon_archetype =
        weapon_archetype(registry, active_wieldable_for_pawn(registry, pawn));
    let wielder_class = pawn_wielder_class(registry, pawn);
    update_weapon_mount_attachments(
        registry,
        pawn,
        &wielder_class,
        descriptors,
        active_weapon_archetype.as_deref(),
        hit_zone_store,
    )
}

/// Apply a snapshot's active-weapon updates to the connected client's own pawn.
/// These run after the snapshot armed the pawn, so the armed class is the
/// arming snapshot's `pawn:` class; a record without one resolves as the
/// default player, as body materialization does.
pub(crate) fn apply_local_weapon_attachments(
    updates: &[crate::client::LocalWeaponAttachmentUpdate],
    replication: &crate::client::ClientReplication,
    registry: &mut EntityRegistry,
    descriptors: &[EntityTypeDescriptor],
    hit_zone_store: &HitZoneStore,
) -> bool {
    let armed = replication.armed_local_pawn();
    let mut changed = false;
    for update in updates {
        let replicated_class = armed
            .as_ref()
            .filter(|armed| armed.entity_id == update.entity_id)
            .and_then(|armed| armed.entity_class.as_deref())
            .unwrap_or(DEFAULT_WIELDER_CLASS);
        let wielder_class =
            crate::descriptor_class::decode_replicated_descriptor_class(replicated_class)
                .canonical_name();
        changed |= update_weapon_mount_attachments(
            registry,
            update.entity_id,
            wielder_class,
            descriptors,
            update.active_weapon_archetype.as_deref(),
            hit_zone_store,
        );
    }
    changed
}

#[cfg(test)]
#[path = "weapon_mounts_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "weapon_mounts_path_tests.rs"]
mod path_tests;
