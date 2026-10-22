// A connected client's pawn inventory composes only from the host's tuning
// payload, whichever of baseline and payload lands first.
// See: context/lib/networking.md §What gates, and what replicates instead

use super::*;
use postretro_combat_model::CarriedState;
use postretro_entities::components::inventory::{Inventory, WIELDABLE_SLOT_CAPACITY};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{ComponentKind, Transform};
use postretro_foundation::{ResolutionMode, WeaponDescriptor};
use postretro_test_log_capture::LogCapture;
use std::net::{Ipv4Addr, SocketAddr};

const PAWN: NetworkId = NetworkId(7);
/// The only weapon in the client's own player loadout; the host never names it.
const CLIENT_OWN_WEAPON: &str = "local_pistol";

fn weapon(name: &str) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        ghost: false,
        faction: None,
        tolerance: None,
        canonical_name: Some(name.to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: Some(WeaponDescriptor {
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
            range: 64.0,
            primary: postretro_foundation::WeaponActivationDescriptor::single(
                postretro_foundation::ActivationTrigger::Press,
                120.0,
            ),
            secondary: None,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            splash: None,
            credit_source: None,
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: None,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
        }),
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
    }
}

fn player(loadout: &[&str]) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        ghost: false,
        faction: None,
        tolerance: None,
        canonical_name: Some("player".to_string()),
        inventory: Some(postretro_entities::InventoryDescriptor {
            loadout: loadout.iter().map(|name| (*name).to_string()).collect(),
        }),
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

/// Every weapon either peer names, under the client's own player loadout.
fn client_descriptors() -> Vec<EntityTypeDescriptor> {
    [CLIENT_OWN_WEAPON, "shotgun", "rifle", "plasma"]
        .into_iter()
        .map(weapon)
        .chain([player(&[CLIENT_OWN_WEAPON])])
        .collect()
}

fn environment() -> EnvironmentTuning {
    EnvironmentTuning {
        level_gravity: -9.81,
        fluids: Default::default(),
    }
}

/// The payload the host builds for a pawn it composed from `slots`, active on
/// `active` (a carried selection), with every weapon's range set to `range`.
fn host_payload(slots: &[(usize, &str)], active: usize, range: f32) -> TuningPayload {
    let descriptors: Vec<_> = slots
        .iter()
        .map(|&(_, name)| {
            let mut descriptor = weapon(name);
            descriptor.weapon.as_mut().unwrap().range = range;
            descriptor
        })
        .chain([player(&[])])
        .collect();
    let carried = CarriedState {
        wieldables: std::array::from_fn(|slot| {
            slots
                .iter()
                .find(|&&(at, _)| at == slot)
                .map(|&(_, name)| name.to_string())
        }),
        active_slot: active,
        ..Default::default()
    };
    let mut host = EntityRegistry::new();
    let pawn =
        crate::scripting::builtins::net_descriptor::spawn_net_slot_pawn_with_carried_loadout(
            &crate::scripting::map_entity::MapEntity {
                map_ordinal: None,
                classname: "player_spawn".into(),
                origin: glam::Vec3::ZERO,
                angles: glam::Vec3::ZERO,
                key_values: Default::default(),
                tags: Vec::new(),
            },
            &descriptors,
            &mut host,
            None,
            Some(&carried),
        )
        .expect("the host composes its copy of the pawn");
    tuning_payload_for_pawn(&host, pawn, &descriptors, None, &environment())
}

fn bytes(payload: &TuningPayload) -> Vec<u8> {
    tuning_payload::encode_tuning_payload(payload)
}

fn client_endpoint() -> NetEndpoint {
    NetEndpoint::from_role(
        &NetRole::Connect {
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 1)),
        },
        None,
    )
    .unwrap()
    .unwrap()
}

fn installed(endpoint: &NetEndpoint) -> Option<&TuningPayload> {
    match endpoint {
        NetEndpoint::Client { tuning, .. } => tuning.as_deref(),
        NetEndpoint::Host { .. } => None,
    }
}

/// Arm a fresh local pawn as an applied `local_player` record does: map it,
/// then materialize it against whatever payload the endpoint holds.
fn arm(endpoint: &mut NetEndpoint, registry: &mut EntityRegistry) -> EntityId {
    let pawn = registry.spawn(Transform::default());
    let NetEndpoint::Client { replication, .. } = endpoint else {
        unreachable!();
    };
    replication.arm_local_pawn_for_test(PAWN, pawn);
    rearm(endpoint, registry);
    pawn
}

/// Re-apply the armed pawn's record, as every later `local_player` snapshot
/// and a full baseline refresh do.
fn rearm(endpoint: &NetEndpoint, registry: &mut EntityRegistry) {
    let NetEndpoint::Client { replication, .. } = endpoint else {
        unreachable!();
    };
    let armed = replication.armed_local_pawn().unwrap();
    let tuning = installed(endpoint);
    remote_materialize::materialize_armed_local_pawn(
        &armed,
        &client_descriptors(),
        registry,
        tuning.and_then(|payload| payload.movement.as_ref()),
        tuning,
        false,
    );
}

fn install(endpoint: &mut NetEndpoint, registry: &mut EntityRegistry, payload: &[u8]) {
    endpoint.install_tuning_payload(payload, registry, &client_descriptors());
}

fn inventory(registry: &EntityRegistry, pawn: EntityId) -> Option<Inventory> {
    registry.get_component::<Inventory>(pawn).ok().cloned()
}

/// The canonical name in each slot of `pawn`'s inventory.
fn layout(registry: &EntityRegistry, pawn: EntityId) -> Vec<Option<String>> {
    inventory(registry, pawn)
        .expect("the pawn holds an inventory")
        .wieldables
        .iter()
        .map(|weapon| {
            weapon.map(|weapon| {
                registry
                    .get_component::<DescriptorProvenance>(weapon)
                    .unwrap()
                    .canonical_name
                    .clone()
            })
        })
        .collect()
}

fn expected_layout(slots: &[(usize, &str)]) -> Vec<Option<String>> {
    (0..WIELDABLE_SLOT_CAPACITY)
        .map(|slot| {
            slots
                .iter()
                .find(|&&(at, _)| at == slot)
                .map(|&(_, name)| name.to_string())
        })
        .collect()
}

/// No weapon instance anywhere in the client registry, so nothing came from
/// its own loadout either.
fn assert_no_weapon(registry: &EntityRegistry, when: &str) {
    assert_eq!(
        registry.iter_with_kind(ComponentKind::Weapon).count(),
        0,
        "{when}: a weapon instance exists"
    );
}

const HOST_SLOTS: [(usize, &str); 2] = [(0, "shotgun"), (2, "plasma")];

// Regression: a pawn baseline that armed before the host's payload composed
// from the client's own descriptor, and the late payload then adopted its
// spawn slot. The armed pawn now holds nothing until the payload installs,
// which composes the host's layout once, on its spawn slot.
#[test]
fn late_tuning_install_composes_from_the_payload_alone() {
    let mut registry = EntityRegistry::new();
    let mut endpoint = client_endpoint();
    let pawn = arm(&mut endpoint, &mut registry);
    assert!(
        inventory(&registry, pawn).is_none(),
        "armed, payload in flight"
    );
    assert_no_weapon(&registry, "before the payload");
    rearm(&endpoint, &mut registry);
    assert!(
        inventory(&registry, pawn).is_none(),
        "a re-arm composes nothing"
    );

    install(
        &mut endpoint,
        &mut registry,
        &bytes(&host_payload(&HOST_SLOTS, 2, 96.0)),
    );

    assert_eq!(layout(&registry, pawn), expected_layout(&HOST_SLOTS));
    let composed = inventory(&registry, pawn).unwrap();
    assert_eq!((composed.active_slot, composed.spawn_active_slot), (2, 2));
    assert_eq!(registry.iter_with_kind(ComponentKind::Weapon).count(), 2);
}

#[test]
fn late_tuning_install_with_an_empty_spawn_slot_composes_on_the_first_populated_slot() {
    let mut registry = EntityRegistry::new();
    let mut endpoint = client_endpoint();
    let pawn = arm(&mut endpoint, &mut registry);
    let payload = host_payload(&HOST_SLOTS, 2, 96.0).with_spawn_active_slot(1);

    install(&mut endpoint, &mut registry, &bytes(&payload));

    let composed = inventory(&registry, pawn).unwrap();
    assert_eq!((composed.active_slot, composed.spawn_active_slot), (0, 0));
}

// P9: an all-empty payload composes an empty inventory; a later payload that
// adds a weapon fills that slot through the merge and makes it active, while
// `spawn_active_slot` keeps its composed value.
#[test]
fn late_empty_payload_composes_every_slot_empty_and_a_later_fill_activates_it() {
    let mut registry = EntityRegistry::new();
    let mut endpoint = client_endpoint();
    let pawn = arm(&mut endpoint, &mut registry);

    install(
        &mut endpoint,
        &mut registry,
        &bytes(&host_payload(&[], 0, 96.0)),
    );
    let empty = inventory(&registry, pawn).expect("an empty payload still composes");
    assert!(empty.wieldables.iter().all(Option::is_none));
    assert_eq!(empty.active_slot, 0);
    assert_no_weapon(&registry, "after an empty payload");

    install(
        &mut endpoint,
        &mut registry,
        &bytes(&host_payload(&[(2, "plasma")], 2, 96.0)),
    );
    let filled = inventory(&registry, pawn).unwrap();
    assert_eq!(layout(&registry, pawn), expected_layout(&[(2, "plasma")]));
    assert_eq!(filled.active_slot, 2, "the first weapon becomes active");
    assert_eq!(
        filled.spawn_active_slot, 0,
        "filled by the merge, not recomposed"
    );
}

// P1: Control drains before Snapshot, so a payload in the same frame as the
// baseline installs with no armed pawn and the arm composes once from it.
#[test]
fn payload_before_the_arm_composes_once_on_the_spawn_slot() {
    let mut registry = EntityRegistry::new();
    let mut endpoint = client_endpoint();
    install(
        &mut endpoint,
        &mut registry,
        &bytes(&host_payload(&HOST_SLOTS, 2, 96.0)),
    );
    assert_no_weapon(&registry, "no pawn to compose for");

    let pawn = arm(&mut endpoint, &mut registry);

    assert_eq!(layout(&registry, pawn), expected_layout(&HOST_SLOTS));
    let composed = inventory(&registry, pawn).unwrap();
    assert_eq!((composed.active_slot, composed.spawn_active_slot), (2, 2));
    rearm(&endpoint, &mut registry);
    assert_eq!(
        inventory(&registry, pawn).unwrap().wieldables,
        composed.wieldables
    );
}

// P4: a second payload retunes each same-canonical instance in place, keeping
// its magazine and live timers, and never re-applies its spawn slot to the
// selection the player left untouched.
#[test]
fn second_payload_retunes_in_place_and_leaves_the_active_slot() {
    let mut registry = EntityRegistry::new();
    let mut endpoint = client_endpoint();
    let pawn = arm(&mut endpoint, &mut registry);
    install(
        &mut endpoint,
        &mut registry,
        &bytes(&host_payload(&HOST_SLOTS, 2, 96.0)),
    );
    let composed = inventory(&registry, pawn).unwrap();
    let plasma = composed.wieldables[2].unwrap();
    let mut live = registry
        .get_component::<WeaponComponent>(plasma)
        .unwrap()
        .clone();
    live.magazine = 3;
    live.cooldown_remaining_ms = 47.0;
    registry.set_component(plasma, live).unwrap();

    install(
        &mut endpoint,
        &mut registry,
        &bytes(&host_payload(&HOST_SLOTS, 0, 140.0)),
    );

    let after = inventory(&registry, pawn).unwrap();
    assert_eq!(after.wieldables, composed.wieldables, "same instances");
    assert_eq!(
        (after.active_slot, after.spawn_active_slot),
        (2, 2),
        "a later payload never moves the selection"
    );
    let retuned = registry.get_component::<WeaponComponent>(plasma).unwrap();
    assert_eq!(retuned.range, 140.0, "the new tuning applied");
    assert_eq!(retuned.magazine, 3);
    assert_eq!(retuned.cooldown_remaining_ms, 47.0);
}

// P3: every applied local record re-arms. After the player switched off the
// composed slot, a re-arm merges only: no recompose, no move.
#[test]
fn rearm_after_a_switch_neither_recomposes_nor_moves_the_active_slot() {
    let mut registry = EntityRegistry::new();
    let mut endpoint = client_endpoint();
    let pawn = arm(&mut endpoint, &mut registry);
    install(
        &mut endpoint,
        &mut registry,
        &bytes(&host_payload(&HOST_SLOTS, 2, 96.0)),
    );
    let mut switched = inventory(&registry, pawn).unwrap();
    switched.active_slot = 0;
    switched.switch_origin = Some(2);
    registry.set_component(pawn, switched.clone()).unwrap();

    for _ in 0..3 {
        rearm(&endpoint, &mut registry);
    }
    // The same payload resent: an unchanged pawn tuning installs nothing.
    install(
        &mut endpoint,
        &mut registry,
        &bytes(&host_payload(&HOST_SLOTS, 2, 96.0)),
    );

    let after = inventory(&registry, pawn).unwrap();
    assert_eq!(after.wieldables, switched.wieldables);
    assert_eq!(after.active_slot, 0);
    assert_eq!(registry.iter_with_kind(ComponentKind::Weapon).count(), 2);
}

// P6–P8: a payload failing its epoch check before composition composes
// nothing and logs; the next good one composes from itself; a failure after
// composition keeps the inventory (E16 O42).
#[test]
fn tuning_failure_before_composition_composes_nothing_and_after_keeps_the_inventory() {
    let capture = LogCapture::start();
    let mut registry = EntityRegistry::new();
    let mut endpoint = client_endpoint();
    let pawn = arm(&mut endpoint, &mut registry);
    let mut stale: serde_json::Value =
        serde_json::from_slice(&bytes(&host_payload(&HOST_SLOTS, 2, 96.0))).unwrap();
    stale["epoch"] = serde_json::json!(postretro_combat_model::TUNING_PAYLOAD_EPOCH - 1);
    let stale = serde_json::to_vec(&stale).unwrap();

    install(&mut endpoint, &mut registry, &stale);
    capture.assert_logged(
        log::Level::Error,
        "[Net] tuning payload epoch/decode failure",
    );
    assert!(installed(&endpoint).is_none());
    assert!(inventory(&registry, pawn).is_none());
    assert_no_weapon(&registry, "after a failed payload");

    install(
        &mut endpoint,
        &mut registry,
        &bytes(&host_payload(&HOST_SLOTS, 2, 96.0)),
    );
    assert_eq!(layout(&registry, pawn), expected_layout(&HOST_SLOTS));
    let composed = inventory(&registry, pawn).unwrap();
    assert_eq!(composed.active_slot, 2);

    install(&mut endpoint, &mut registry, &stale);
    assert!(installed(&endpoint).is_none());
    assert_eq!(
        inventory(&registry, pawn).unwrap(),
        composed,
        "kept as it was"
    );
}

// P12, P13: a demotion or level unload drops the installed payload. A pawn
// armed after either holds nothing until the next payload installs; it never
// composes from the previous payload or the client's own descriptor.
#[test]
fn client_demotion_or_unload_before_the_payload_composes_nothing_until_the_next() {
    for demote in [true, false] {
        let mut registry = EntityRegistry::new();
        let mut endpoint = client_endpoint();
        let old = arm(&mut endpoint, &mut registry);
        install(
            &mut endpoint,
            &mut registry,
            &bytes(&host_payload(&[(0, "rifle")], 0, 96.0)),
        );
        assert_eq!(layout(&registry, old), expected_layout(&[(0, "rifle")]));

        if demote {
            endpoint.demote_client_state(&mut registry);
            assert!(!registry.exists(old), "demotion despawns the mapped pawn");
        } else {
            endpoint.reset_level_scoped_client_state();
            // Level unload clears the registry.
            registry = EntityRegistry::new();
        }
        assert!(installed(&endpoint).is_none(), "the payload is dropped");

        let pawn = arm(&mut endpoint, &mut registry);
        assert!(inventory(&registry, pawn).is_none(), "demote {demote}");
        assert_no_weapon(&registry, "re-armed before the next payload");

        install(
            &mut endpoint,
            &mut registry,
            &bytes(&host_payload(&HOST_SLOTS, 2, 96.0)),
        );
        assert_eq!(layout(&registry, pawn), expected_layout(&HOST_SLOTS));
        assert_eq!(inventory(&registry, pawn).unwrap().active_slot, 2);
    }
}

// With adoption gone, a host that drops its active weapon and the client agree
// only because the merge's release repoints to the first populated slot, the
// same rule the host's drop applies.
#[test]
fn later_payload_dropping_the_active_weapon_lands_where_the_host_does() {
    let three = [(0, "shotgun"), (1, "rifle"), (2, "plasma")];
    let mut registry = EntityRegistry::new();
    let mut endpoint = client_endpoint();
    let pawn = arm(&mut endpoint, &mut registry);
    install(
        &mut endpoint,
        &mut registry,
        &bytes(&host_payload(&three, 2, 96.0)),
    );
    let plasma = inventory(&registry, pawn).unwrap().wieldables[2].unwrap();

    // The host dropped the plasma: its slot empties and the host repoints to
    // the first populated slot, 0.
    let mut dropped = host_payload(&three, 2, 96.0);
    dropped.wieldables[2] = None;
    install(&mut endpoint, &mut registry, &bytes(&dropped));

    assert_eq!(layout(&registry, pawn), expected_layout(&three[..2]));
    assert_eq!(inventory(&registry, pawn).unwrap().active_slot, 0);
    assert!(
        !registry.exists(plasma),
        "the released instance is despawned"
    );
}
