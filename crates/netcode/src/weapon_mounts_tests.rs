// Tests: the weapon-mount writer, its host and single-player synchronizers, and
// the missing-data warnings it shares with the level-install check.
// See: context/lib/networking.md §Weapon placement is content

use std::collections::HashMap;
use std::sync::Arc;

use glam::Vec3;
use log::Level;
use postretro_entities::Transform;
use postretro_entities::components::inventory::{Inventory, WIELDABLE_SLOT_CAPACITY};
use postretro_entities::components::mesh::AttachmentBinding;
use postretro_entities::data_descriptors::{InventoryDescriptor, WeaponMountDescriptor};
use postretro_entities::provenance::{DescriptorComponentKind, DescriptorSpawnPath};
use postretro_foundation::{
    ActivationTrigger, ProjectileBodyVisual, ProjectileDescriptor, ProjectileVisual,
    ResolutionMode, WeaponActivationDescriptor, WeaponDescriptor,
};
use postretro_model::ModelHandle;
use postretro_model::gltf_loader::SocketBinding;
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::scripting_systems::hit_zones::ModelHitZones;

pub(super) const EXO_RIG: &str = "models/exo_red/model.gltf";
pub(super) const TROOPER_RIG: &str = "models/trooper/model.gltf";
pub(super) const GRIP_TAPE: &str = "models/grip_tape.gltf";
pub(super) const BACKPACK: &str = "models/backpack.gltf";
pub(super) const TRIMMER: &str = "models/trimmer.gltf";
pub(super) const RIFLE_EXO: &str = "models/rifle/exo_red.gltf";
pub(super) const RIFLE_TROOPER: &str = "models/rifle/trooper.gltf";
pub(super) const SLING: &str = "models/rifle/sling.gltf";
pub(super) const PISTOL_EXO: &str = "models/pistol/exo_red.gltf";
pub(super) const PISTOL_TROOPER: &str = "models/pistol/trooper.gltf";
pub(super) const UNLOADED: &str = "models/never_loaded.gltf";

const PICKUP_MISSING: &str = "[Weapon mounts] entity `player` has no weaponMounts entry for weapon `pickup`; it mounts nothing";

pub(super) fn weapon(resolution: ResolutionMode) -> WeaponDescriptor {
    WeaponDescriptor {
        sounds: None,
        knockback: None,
        damage: 10.0,
        pellet_count: 1,
        spread_degrees: 0.0,
        bloom_per_shot_degrees: 0.0,
        bloom_max_degrees: 0.0,
        bloom_decay_degrees_per_second: 0.0,
        bloom_decay_delay_ms: 0.0,
        movement_spread_degrees: 0.0,
        spread_vertical_bias: 0.0,
        range: 50.0,
        primary: WeaponActivationDescriptor::single(ActivationTrigger::Press, 100.0),
        secondary: None,
        resolution,
        projectile: (resolution == ResolutionMode::Projectile).then(|| ProjectileDescriptor {
            ghost: false,
            speed: 20.0,
            radius: 0.1,
            lifetime_ms: 1_000.0,
            visual: ProjectileVisual {
                body: ProjectileBodyVisual::Sprite {
                    sprite: "sprites/bolt.png".to_string(),
                    size: 0.2,
                    opacity: 1.0,
                    rotation: 0.0,
                    tint: [1.0; 3],
                    emissive: 0.0,
                    frame_duration_ms: None,
                },
                trail: None,
                light: None,
                impact_light: None,
            },
        }),
        splash: None,
        credit_source: None,
        viewmodel: None,
        placement: None,
        muzzle_offset: Some([0.1, -0.1, -0.5]),
        resource: None,
        lower_ms: 0,
        raise_ms: 0,
        block_during_reload: None,
        damage_type: None,
    }
}

fn bare(name: &str) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        life_states: None,
        ghost: false,
        faction: None,
        tolerance: None,
        canonical_name: Some(name.to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
    }
}

pub(super) fn weapon_entity(name: &str) -> EntityTypeDescriptor {
    let mut descriptor = bare(name);
    descriptor.weapon = Some(weapon(ResolutionMode::Hitscan));
    descriptor
}

pub(super) fn entry(weapon: &str, attachments: &[(&str, &str)]) -> WeaponMountDescriptor {
    WeaponMountDescriptor {
        weapon: weapon.to_string(),
        attachments: attachments
            .iter()
            .map(|(socket, model)| (socket.to_string(), model.to_string()))
            .collect(),
    }
}

/// A character on `rig` with base `attachments` and an optional table. Its
/// movement block makes it a player class on every path.
pub(super) fn character(
    name: &str,
    rig: &str,
    attachments: &[(&str, &str)],
    table: Option<Vec<WeaponMountDescriptor>>,
) -> EntityTypeDescriptor {
    let mut descriptor = bare(name);
    descriptor.movement = Some(crate::predict_reconcile_harness_test_fixtures::player_descriptor());
    descriptor.mesh = Some(MeshDescriptor {
        model: rig.to_string(),
        shadow_only: false,
        attachments: attachments
            .iter()
            .map(|(socket, model)| (socket.to_string(), model.to_string()))
            .collect(),
        shadow_bias_scale: 1.0,
        animations: HashMap::new(),
        default_state: None,
        locomotion: None,
        weapon_mounts: table,
    });
    descriptor.inventory = Some(InventoryDescriptor {
        loadout: vec!["rifle".to_string(), "pistol".to_string()],
    });
    descriptor
}

/// Two player classes on different rigs, a robot without a table, and the
/// weapons they wield. `player` covers its base `hand_r` and `back` with the
/// rifle's two-socket entry; `carbine` mounts the rifle's hand prop alone.
pub(super) fn descriptors() -> Vec<EntityTypeDescriptor> {
    vec![
        character(
            "player",
            EXO_RIG,
            &[("hand_r", GRIP_TAPE), ("back", BACKPACK)],
            Some(vec![
                entry("rifle", &[("hand_r", RIFLE_EXO), ("back", SLING)]),
                entry("pistol", &[("hand_r", PISTOL_EXO)]),
                entry("carbine", &[("hand_r", RIFLE_EXO)]),
            ]),
        ),
        character(
            "trooper",
            TROOPER_RIG,
            &[],
            Some(vec![
                entry("rifle", &[("hand_r", RIFLE_TROOPER), ("back", SLING)]),
                entry("carbine", &[("hand_r", RIFLE_TROOPER)]),
                // The trooper rig has no `hip` socket.
                entry("pistol", &[("hand_r", PISTOL_TROOPER), ("hip", PISTOL_EXO)]),
            ]),
        ),
        character("robot", EXO_RIG, &[("hand_r", TRIMMER)], None),
        weapon_entity("rifle"),
        weapon_entity("pistol"),
        weapon_entity("carbine"),
        weapon_entity("pickup"),
    ]
}

fn zones(sockets: &[&str]) -> ModelHitZones {
    ModelHitZones::for_test(
        Arc::new(postretro_model::skeleton::Skeleton::default()),
        Arc::new(Vec::new()),
        Vec::new(),
        sockets
            .iter()
            .enumerate()
            .map(|(joint, socket)| (socket.to_string(), SocketBinding::SkinnedJoint(joint)))
            .collect(),
        None,
        Vec::new(),
        Arc::new(postretro_model::pose_modifier::PoseModifierStack::default()),
    )
}

/// The level's loaded model set: both rigs and every prop except `UNLOADED`.
pub(super) fn store() -> HitZoneStore {
    let mut store = HitZoneStore::new();
    store.insert_for_test(
        ModelHandle::from(EXO_RIG),
        zones(&["hand_r", "back", "hand_l"]),
    );
    store.insert_for_test(ModelHandle::from(TROOPER_RIG), zones(&["hand_r", "back"]));
    for prop in [
        GRIP_TAPE,
        BACKPACK,
        TRIMMER,
        RIFLE_EXO,
        RIFLE_TROOPER,
        SLING,
        PISTOL_EXO,
        PISTOL_TROOPER,
    ] {
        store.insert_for_test(ModelHandle::from(prop), zones(&[]));
    }
    store
}

/// The mounted (socket, model) pairs, in the order the mesh holds them.
pub(super) fn mounted(registry: &EntityRegistry, pawn: EntityId) -> Vec<(String, String)> {
    registry
        .get_component::<MeshComponent>(pawn)
        .expect("pawn has a mesh")
        .attachments
        .iter()
        .map(|attachment| (attachment.socket.clone(), attachment.model.clone()))
        .collect()
}

pub(super) fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(socket, model)| (socket.to_string(), model.to_string()))
        .collect()
}

/// An authoritative pawn of `class`, its descriptor mesh materialized with the
/// base attachments, carrying one weapon instance per name with `active`
/// selected. Returns the pawn and its weapon instances.
pub(super) fn spawn_pawn(
    registry: &mut EntityRegistry,
    descriptors: &[EntityTypeDescriptor],
    class: &str,
    spawn_path: DescriptorSpawnPath,
    weapons: &[&str],
    active: Option<usize>,
) -> (EntityId, Vec<EntityId>) {
    let pawn = registry.spawn(Transform::default());
    registry
        .set_component(
            pawn,
            DescriptorProvenance {
                canonical_name: class.to_string(),
                owned_components: [DescriptorComponentKind::Movement].into_iter().collect(),
                map_overrides: Default::default(),
                spawn_path,
            },
        )
        .unwrap();
    registry
        .set_component(
            pawn,
            crate::predict_reconcile_harness_test_fixtures::component(),
        )
        .unwrap();
    assert!(
        crate::scripting::builtins::net_descriptor::materialize_net_mesh_presentation(
            class,
            descriptors,
            registry,
            pawn,
            None,
        )
    );
    let instances: Vec<EntityId> = weapons
        .iter()
        .map(|name| {
            let instance = registry.spawn(Transform::default());
            registry
                .set_component(
                    instance,
                    DescriptorProvenance {
                        canonical_name: name.to_string(),
                        owned_components: [DescriptorComponentKind::Weapon].into_iter().collect(),
                        map_overrides: Default::default(),
                        spawn_path: DescriptorSpawnPath::DefaultWeapon,
                    },
                )
                .unwrap();
            instance
        })
        .collect();
    let mut inventory = Inventory::default();
    for (slot, instance) in instances.iter().enumerate() {
        inventory.wieldables[slot] = Some(*instance);
    }
    inventory.active_slot = active.unwrap_or(WIELDABLE_SLOT_CAPACITY);
    registry.set_component(pawn, inventory).unwrap();
    (pawn, instances)
}

/// Select inventory `slot` (or an empty slot for `None`).
pub(super) fn select(registry: &mut EntityRegistry, pawn: EntityId, slot: Option<usize>) {
    let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
    // Slot capacity is past the last slot: no active weapon.
    inventory.active_slot = slot.unwrap_or(WIELDABLE_SLOT_CAPACITY);
    registry.set_component(pawn, inventory).unwrap();
}

fn write(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    class: &str,
    descriptors: &[EntityTypeDescriptor],
    weapon: Option<&str>,
    store: &HitZoneStore,
) -> bool {
    update_weapon_mount_attachments(registry, pawn, class, descriptors, weapon, store)
}

fn materialized_pawn(
    registry: &mut EntityRegistry,
    descriptors: &[EntityTypeDescriptor],
    class: &str,
) -> EntityId {
    spawn_pawn(
        registry,
        descriptors,
        class,
        DescriptorSpawnPath::PlayerSpawn,
        &[],
        None,
    )
    .0
}

#[test]
fn an_entry_mounts_each_model_at_its_socket_over_the_base_and_binds_through_the_resolver() {
    let descriptors = descriptors();
    let store = store();
    let mut registry = EntityRegistry::new();
    let pawn = materialized_pawn(&mut registry, &descriptors, "player");

    assert!(write(
        &mut registry,
        pawn,
        "player",
        &descriptors,
        Some("rifle"),
        &store
    ));
    crate::sim::resolve_mesh_entity_bindings_for_entities(
        &mut registry,
        &crate::scripting_systems::mesh_anim::MeshClipTables::default(),
        &store,
        [pawn],
    );

    assert_eq!(
        mounted(&registry, pawn),
        pairs(&[("back", SLING), ("hand_r", RIFLE_EXO)]),
        "the rifle's two sockets, one of them not hand_r, replace the base props there"
    );
    let mesh = registry.get_component::<MeshComponent>(pawn).unwrap();
    assert_eq!(mesh.attachments[0].binding, AttachmentBinding::Skinned(1));
    assert_eq!(mesh.attachments[1].binding, AttachmentBinding::Skinned(0));
    assert!(
        !write(
            &mut registry,
            pawn,
            "player",
            &descriptors,
            Some("rifle"),
            &store
        ),
        "an unchanged mount leaves the mesh and its bindings alone"
    );
}

#[test]
fn a_switch_removes_the_old_entry_and_returns_each_covered_base_attachment() {
    let logs = LogCapture::start();
    let descriptors = descriptors();
    let store = store();
    let mut registry = EntityRegistry::new();
    let pawn = materialized_pawn(&mut registry, &descriptors, "player");

    write(
        &mut registry,
        pawn,
        "player",
        &descriptors,
        Some("rifle"),
        &store,
    );
    assert!(write(
        &mut registry,
        pawn,
        "player",
        &descriptors,
        Some("pistol"),
        &store
    ));
    assert_eq!(
        mounted(&registry, pawn),
        pairs(&[("back", BACKPACK), ("hand_r", PISTOL_EXO)]),
        "the sling leaves and the backpack returns; the pistol takes hand_r"
    );

    // A weapon with no entry mounts nothing and leaves the base attachments.
    assert!(write(
        &mut registry,
        pawn,
        "player",
        &descriptors,
        Some("pickup"),
        &store
    ));
    assert_eq!(
        mounted(&registry, pawn),
        pairs(&[("back", BACKPACK), ("hand_r", GRIP_TAPE)])
    );
    logs.assert_logged_once(Level::Warn, PICKUP_MISSING);
}

// Pin P4: the same model on a shared socket, the old entry with an extra socket.
#[test]
fn a_switch_to_an_entry_sharing_a_model_returns_the_extra_socket_to_its_base_or_empties_it() {
    let descriptors = descriptors();
    let store = store();
    let mut registry = EntityRegistry::new();
    for (class, rifle_prop, back_after) in [
        ("player", RIFLE_EXO, Some(BACKPACK)),
        ("trooper", RIFLE_TROOPER, None),
    ] {
        let pawn = materialized_pawn(&mut registry, &descriptors, class);
        write(
            &mut registry,
            pawn,
            class,
            &descriptors,
            Some("rifle"),
            &store,
        );
        assert_eq!(
            mounted(&registry, pawn),
            pairs(&[("back", SLING), ("hand_r", rifle_prop)])
        );

        assert!(write(
            &mut registry,
            pawn,
            class,
            &descriptors,
            Some("carbine"),
            &store
        ));
        let mut expected = vec![("hand_r", rifle_prop)];
        if let Some(base) = back_after {
            expected.insert(0, ("back", base));
        }
        assert_eq!(mounted(&registry, pawn), pairs(&expected), "{class}");
    }
}

// Pin P3: today's writer stripped a base hand_r with no active weapon.
#[test]
fn no_active_weapon_at_install_or_after_a_drop_shows_every_base_attachment() {
    let descriptors = descriptors();
    let store = store();
    let mut registry = EntityRegistry::new();
    let (pawn, _) = spawn_pawn(
        &mut registry,
        &descriptors,
        "player",
        DescriptorSpawnPath::PlayerSpawn,
        &["rifle"],
        None,
    );
    let base = pairs(&[("back", BACKPACK), ("hand_r", GRIP_TAPE)]);

    assert!(!synchronize_weapon_attachment_for_pawn(
        &mut registry,
        pawn,
        &descriptors,
        &store
    ));
    assert_eq!(
        mounted(&registry, pawn),
        base,
        "install with no active weapon"
    );

    select(&mut registry, pawn, Some(0));
    assert!(synchronize_weapon_attachment_for_pawn(
        &mut registry,
        pawn,
        &descriptors,
        &store
    ));
    select(&mut registry, pawn, None);
    assert!(synchronize_weapon_attachment_for_pawn(
        &mut registry,
        pawn,
        &descriptors,
        &store
    ));
    assert_eq!(mounted(&registry, pawn), base, "after dropping the rifle");
}

#[test]
fn a_character_without_a_table_keeps_its_base_hand_prop_through_every_switch_silently() {
    let logs = LogCapture::start();
    let descriptors = descriptors();
    let store = store();
    let mut registry = EntityRegistry::new();
    let (pawn, _) = spawn_pawn(
        &mut registry,
        &descriptors,
        "robot",
        DescriptorSpawnPath::PlayerSpawn,
        &["rifle", "pistol", "pickup"],
        None,
    );

    for slot in [None, Some(0), Some(1), Some(2), Some(0), None] {
        select(&mut registry, pawn, slot);
        assert!(!synchronize_weapon_attachment_for_pawn(
            &mut registry,
            pawn,
            &descriptors,
            &store
        ));
        assert_eq!(mounted(&registry, pawn), pairs(&[("hand_r", TRIMMER)]));
    }
    crate::scripting_systems::weapon_mounts::check_weapon_mount_tables(
        &[descriptors[2].clone(), descriptors[3].clone()],
        &store,
    );
    logs.assert_nothing_above(Level::Info);
}

#[test]
fn two_player_classes_on_different_rigs_each_mount_their_own_bake_for_one_weapon() {
    let descriptors = descriptors();
    let store = store();
    let mut registry = EntityRegistry::new();
    let mut weapon_owners = WeaponOwners::new();
    let (player, _) = spawn_pawn(
        &mut registry,
        &descriptors,
        "player",
        DescriptorSpawnPath::PlayerSpawn,
        &["rifle"],
        Some(0),
    );
    let (trooper, _) = spawn_pawn(
        &mut registry,
        &descriptors,
        "trooper",
        DescriptorSpawnPath::NetworkSlot,
        &["rifle"],
        Some(0),
    );
    weapon_owners.mark_attachment_dirty(player);
    weapon_owners.mark_attachment_dirty(trooper);

    let mut changed = synchronize_weapon_owner_attachments(
        &mut registry,
        &mut weapon_owners,
        &descriptors,
        &store,
    );
    changed.sort();
    let mut expected = vec![player, trooper];
    expected.sort();
    assert_eq!(changed, expected);
    assert_eq!(
        mounted(&registry, player),
        pairs(&[("back", SLING), ("hand_r", RIFLE_EXO)])
    );
    assert_eq!(
        mounted(&registry, trooper),
        pairs(&[("back", SLING), ("hand_r", RIFLE_TROOPER)])
    );
}

#[test]
fn two_switches_in_one_host_tick_leave_only_the_final_entry_mounted() {
    let descriptors = descriptors();
    let store = store();
    let mut registry = EntityRegistry::new();
    let mut weapon_owners = WeaponOwners::new();
    let (pawn, _) = spawn_pawn(
        &mut registry,
        &descriptors,
        "player",
        DescriptorSpawnPath::NetworkSlot,
        &["rifle", "pistol"],
        Some(0),
    );
    weapon_owners.mark_attachment_dirty(pawn);
    synchronize_weapon_owner_attachments(&mut registry, &mut weapon_owners, &descriptors, &store);

    select(&mut registry, pawn, Some(1));
    weapon_owners.mark_attachment_dirty(pawn);
    select(&mut registry, pawn, Some(0));
    weapon_owners.mark_attachment_dirty(pawn);
    select(&mut registry, pawn, Some(1));
    weapon_owners.mark_attachment_dirty(pawn);
    assert_eq!(
        synchronize_weapon_owner_attachments(
            &mut registry,
            &mut weapon_owners,
            &descriptors,
            &store
        ),
        vec![pawn]
    );
    assert_eq!(
        mounted(&registry, pawn),
        pairs(&[("back", BACKPACK), ("hand_r", PISTOL_EXO)])
    );
    assert!(
        synchronize_weapon_owner_attachments(
            &mut registry,
            &mut weapon_owners,
            &descriptors,
            &store
        )
        .is_empty(),
        "a drained queue does no per-frame work"
    );
}

#[test]
fn a_host_pawn_without_provenance_wields_as_the_default_player_class() {
    let descriptors = descriptors();
    let store = store();
    let mut registry = EntityRegistry::new();
    let (pawn, _) = spawn_pawn(
        &mut registry,
        &descriptors,
        "player",
        DescriptorSpawnPath::PlayerSpawn,
        &["rifle"],
        Some(0),
    );
    registry
        .remove_component::<DescriptorProvenance>(pawn)
        .unwrap();

    assert!(synchronize_weapon_attachment_for_pawn(
        &mut registry,
        pawn,
        &descriptors,
        &store
    ));
    assert_eq!(
        mounted(&registry, pawn),
        pairs(&[("back", SLING), ("hand_r", RIFLE_EXO)])
    );
}

// Pin P5 on the host: a level transition respawns the pawn as the new class
// and the install pass synchronizes it with the carried weapon.
#[test]
fn a_class_change_at_a_level_transition_mounts_the_new_class_entry_on_the_host() {
    let logs = LogCapture::start();
    let descriptors = descriptors();
    let store = store();
    let mut registry = EntityRegistry::new();
    let (pawn, _) = spawn_pawn(
        &mut registry,
        &descriptors,
        "player",
        DescriptorSpawnPath::PlayerSpawn,
        &["rifle"],
        Some(0),
    );
    synchronize_weapon_attachment_for_pawn(&mut registry, pawn, &descriptors, &store);
    assert_eq!(
        mounted(&registry, pawn),
        pairs(&[("back", SLING), ("hand_r", RIFLE_EXO)])
    );

    for (class, expected) in [
        (
            "trooper",
            pairs(&[("back", SLING), ("hand_r", RIFLE_TROOPER)]),
        ),
        // A class without a table shows its base attachments, silently.
        ("robot", pairs(&[("hand_r", TRIMMER)])),
    ] {
        // The next level installs a fresh registry and loaded model set.
        let store = self::store();
        let mut next_level = EntityRegistry::new();
        let (pawn, _) = spawn_pawn(
            &mut next_level,
            &descriptors,
            class,
            DescriptorSpawnPath::PlayerSpawn,
            &["rifle"],
            Some(0),
        );
        synchronize_weapon_attachment_for_pawn(&mut next_level, pawn, &descriptors, &store);
        assert_eq!(mounted(&next_level, pawn), expected, "{class}");
    }
    logs.assert_nothing_above(Level::Info);
}

#[test]
fn a_picked_up_weapon_without_an_entry_warns_once_per_character_and_weapon_per_level_load() {
    let logs = LogCapture::start();
    let descriptors = descriptors();
    let mut store = store();
    let mut registry = EntityRegistry::new();
    crate::scripting_systems::weapon_mounts::check_weapon_mount_tables(&descriptors, &store);
    logs.assert_not_logged(Level::Warn, "[Weapon mounts]");

    let (pawn, _) = spawn_pawn(
        &mut registry,
        &descriptors,
        "player",
        DescriptorSpawnPath::PlayerSpawn,
        &["rifle", "pickup"],
        Some(0),
    );
    // First wield, repeat switches, and a respawn of the same class.
    for slot in [1, 0, 1, 0, 1] {
        select(&mut registry, pawn, Some(slot));
        synchronize_weapon_attachment_for_pawn(&mut registry, pawn, &descriptors, &store);
    }
    let (respawned, _) = spawn_pawn(
        &mut registry,
        &descriptors,
        "player",
        DescriptorSpawnPath::PlayerSpawn,
        &["pickup"],
        Some(0),
    );
    synchronize_weapon_attachment_for_pawn(&mut registry, respawned, &descriptors, &store);
    logs.assert_logged_once(Level::Warn, PICKUP_MISSING);
    // Another class's missing entry is its own warning.
    logs.assert_not_logged(Level::Warn, "entity `trooper`");

    // The next level load clears the per-level set and may warn again.
    store.clear();
    synchronize_weapon_attachment_for_pawn(&mut registry, respawned, &descriptors, &store);
    assert_eq!(
        logs.records()
            .iter()
            .filter(|record| record.message.contains(PICKUP_MISSING))
            .count(),
        2
    );
}

#[test]
fn a_loadout_weapon_warned_at_install_does_not_warn_again_at_first_wield() {
    let logs = LogCapture::start();
    let mut descriptors = descriptors();
    // A table that lacks the pistol, which the loadout carries.
    descriptors[0].mesh.as_mut().unwrap().weapon_mounts =
        Some(vec![entry("rifle", &[("hand_r", RIFLE_EXO)])]);
    let store = store();
    let pistol_missing = "[Weapon mounts] entity `player` has no weaponMounts entry for weapon `pistol`; it mounts nothing";

    crate::scripting_systems::weapon_mounts::check_weapon_mount_tables(&descriptors, &store);
    logs.assert_logged_once(Level::Warn, pistol_missing);

    let mut registry = EntityRegistry::new();
    let (pawn, _) = spawn_pawn(
        &mut registry,
        &descriptors,
        "player",
        DescriptorSpawnPath::PlayerSpawn,
        &["rifle", "pistol"],
        Some(1),
    );
    synchronize_weapon_attachment_for_pawn(&mut registry, pawn, &descriptors, &store);
    assert_eq!(
        mounted(&registry, pawn),
        pairs(&[("back", BACKPACK), ("hand_r", GRIP_TAPE)])
    );
    logs.assert_logged_once(Level::Warn, pistol_missing);
}

#[test]
fn an_entry_socket_the_rig_lacks_warns_once_and_the_entry_other_sockets_still_mount() {
    let logs = LogCapture::start();
    let descriptors = descriptors();
    let store = store();
    let mut registry = EntityRegistry::new();
    let pawn = materialized_pawn(&mut registry, &descriptors, "trooper");

    for weapon in ["pistol", "rifle", "pistol", "rifle", "pistol"] {
        write(
            &mut registry,
            pawn,
            "trooper",
            &descriptors,
            Some(weapon),
            &store,
        );
        crate::sim::resolve_mesh_entity_bindings_for_entities(
            &mut registry,
            &crate::scripting_systems::mesh_anim::MeshClipTables::default(),
            &store,
            [pawn],
        );
    }

    assert_eq!(
        mounted(&registry, pawn),
        pairs(&[("hand_r", PISTOL_TROOPER)]),
        "nothing draws on the missing socket; hand_r still mounts"
    );
    logs.assert_logged_once(
        Level::Warn,
        "[Weapon mounts] entity `trooper` mounts weapon `pistol` on socket `hip`, which its model `models/trooper/model.gltf` lacks; nothing mounts there",
    );
    logs.assert_not_logged(Level::Warn, "has no socket");
}

#[test]
fn an_entry_model_missing_from_the_preload_warns_once_and_the_other_sockets_still_mount() {
    let logs = LogCapture::start();
    let mut descriptors = descriptors();
    descriptors[0].mesh.as_mut().unwrap().weapon_mounts = Some(vec![
        entry("rifle", &[("hand_r", UNLOADED), ("back", SLING)]),
        entry("pistol", &[("hand_r", PISTOL_EXO)]),
    ]);
    let store = store();
    let mut registry = EntityRegistry::new();
    let pawn = materialized_pawn(&mut registry, &descriptors, "player");

    for weapon in ["rifle", "pistol", "rifle", "pistol", "rifle"] {
        write(
            &mut registry,
            pawn,
            "player",
            &descriptors,
            Some(weapon),
            &store,
        );
    }

    assert_eq!(
        mounted(&registry, pawn),
        pairs(&[("back", SLING)]),
        "the unloaded hand prop draws nothing, not even the base grip tape"
    );
    logs.assert_logged_once(
        Level::Warn,
        "[Weapon mounts] entity `player` mounts model `models/never_loaded.gltf` on socket `hand_r` for weapon `rifle`, but the model is not loaded; nothing mounts there",
    );
}

#[test]
fn an_entry_naming_no_weapon_never_mounts_and_the_other_entries_still_mount() {
    let logs = LogCapture::start();
    let mut descriptors = descriptors();
    descriptors[0]
        .mesh
        .as_mut()
        .unwrap()
        .weapon_mounts
        .as_mut()
        .unwrap()
        .push(entry("retired_rifle", &[("hand_r", PISTOL_EXO)]));
    let store = store();
    crate::scripting_systems::weapon_mounts::check_weapon_mount_tables(&descriptors, &store);

    let mut registry = EntityRegistry::new();
    let (pawn, _) = spawn_pawn(
        &mut registry,
        &descriptors,
        "player",
        DescriptorSpawnPath::PlayerSpawn,
        &["rifle"],
        Some(0),
    );
    synchronize_weapon_attachment_for_pawn(&mut registry, pawn, &descriptors, &store);

    assert_eq!(
        mounted(&registry, pawn),
        pairs(&[("back", SLING), ("hand_r", RIFLE_EXO)])
    );
    logs.assert_logged_once(
        Level::Warn,
        "[Weapon mounts] entity `player` has a weaponMounts entry for `retired_rifle`, which names no weapon; the entry never mounts",
    );
}

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

// Pin P2: the switch after a deferred table edit mounts the pre-edit entry.
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

/// Fire one shot from `shooter` along -Z at a hitbox target and return the
/// client resolution: its hits, world contacts and projectile launch origin.
fn fire(
    registry: &EntityRegistry,
    shooter: EntityId,
    store: &HitZoneStore,
    resolution: ResolutionMode,
) -> crate::weapon::ClientFireResolution {
    let descriptor = weapon(resolution);
    let placement = postretro_foundation::WeaponPlacementDescriptor {
        offset: postretro_foundation::PlacementOffset {
            right: 0.2,
            up: -0.2,
            forward: 0.4,
        },
        rotation: Default::default(),
    };
    let mut component =
        postretro_entities::components::weapon::WeaponComponent::from_descriptor(&descriptor);
    crate::weapon::resolve_test_client_shot(
        Some(shooter),
        &mut component,
        "rifle",
        0,
        crate::weapon::FireButtonState {
            pressed: true,
            active: true,
        },
        Vec3::new(0.0, 1.5, 0.0),
        Vec3::NEG_Z,
        &placement,
        descriptor.muzzle_offset.map(Vec3::from),
        3,
        &[0.0],
        &[],
        &postretro_physics::collision::moving::CombinedCollisionWorld::static_only(
            &crate::collision::CollisionWorld::new(),
        ),
        registry,
        store,
        0.0,
        0.0,
    )
    .expect("a fresh press fires")
}

#[test]
fn fire_origin_and_hits_are_identical_with_and_without_a_weapon_mount_table() {
    let store = store();
    let mut results = Vec::new();
    for with_table in [true, false] {
        let mut descriptors = descriptors();
        if !with_table {
            for descriptor in &mut descriptors {
                if let Some(mesh) = descriptor.mesh.as_mut() {
                    mesh.weapon_mounts = None;
                }
            }
        }
        let mut registry = EntityRegistry::new();
        let (shooter, _) = spawn_pawn(
            &mut registry,
            &descriptors,
            "player",
            DescriptorSpawnPath::PlayerSpawn,
            &["rifle"],
            Some(0),
        );
        let (target, _) = spawn_pawn(
            &mut registry,
            &descriptors,
            "trooper",
            DescriptorSpawnPath::NetworkSlot,
            &["rifle"],
            Some(0),
        );
        registry
            .set_component(
                target,
                Transform {
                    position: Vec3::new(0.0, 1.0, -6.0),
                    ..Transform::default()
                },
            )
            .unwrap();
        registry
            .set_component(
                target,
                postretro_entities::components::health::HealthComponent::from_descriptor(
                    &postretro_entities::data_descriptors::HealthDescriptor {
                        max: 50.0,
                        hitbox: Some(postretro_entities::data_descriptors::HitboxDescriptor {
                            half_extents: [0.4, 1.0, 0.4],
                            offset: None,
                        }),
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
        for pawn in [shooter, target] {
            synchronize_weapon_attachment_for_pawn(&mut registry, pawn, &descriptors, &store);
        }
        let hitscan = fire(&registry, shooter, &store, ResolutionMode::Hitscan);
        assert_eq!(hitscan.hits.len(), 1, "the hitscan shot hits the target");
        assert_eq!(hitscan.hits[0].target, target);
        let projectile = fire(&registry, shooter, &store, ResolutionMode::Projectile);
        assert!(projectile.projectile_launch.is_some());
        results.push((mounted(&registry, shooter), hitscan, projectile));
    }
    let (with_table, without_table) = (&results[0], &results[1]);
    assert_eq!(
        with_table.0,
        pairs(&[("back", SLING), ("hand_r", RIFLE_EXO)])
    );
    assert_ne!(with_table.0, without_table.0, "fixture: the mounts differ");
    assert_eq!(
        (&with_table.1, &with_table.2),
        (&without_table.1, &without_table.2),
        "a mount is presentation only: fire origin and hits never read it"
    );
}
