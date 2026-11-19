// The loadout wire mirror and the host's join-time gear check.
// See: context/lib/networking.md §Session-state ledger

use std::collections::BTreeMap;

use postretro_combat_model::CarriedState;
use postretro_entities::{EntityTypeDescriptor, LifeStatesDescriptor};
use postretro_foundation::defense::DefenseVocabulary;
use postretro_foundation::{HealthDescriptor, LifeStateProfile, WeaponDescriptor};
use postretro_net::wire::CarriedLoadoutWire;

use super::*;

fn descriptor(name: &str) -> EntityTypeDescriptor {
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

fn weapon(name: &str, resource: serde_json::Value) -> EntityTypeDescriptor {
    let weapon = serde_json::from_value::<WeaponDescriptor>(serde_json::json!({
        "damage": 1.0, "range": 64.0, "resolution": "hitscan",
        "primary": {"trigger": "press", "recoveryMs": 100.0, "steps": [{"kind": "shot"}]},
        "resource": resource,
    }))
    .unwrap()
    .validate()
    .unwrap();
    EntityTypeDescriptor {
        weapon: Some(weapon),
        ..descriptor(name)
    }
}

/// A host mod: a shotgun on shells (magazine 6), a plasma gun on heat, a
/// `scout` pickup with no weapon, a player with 100 health starting `alive`,
/// and one registered layer, `armor`.
fn host_mod() -> (Vec<EntityTypeDescriptor>, DefenseVocabulary) {
    let mut player = descriptor("player");
    player.health = Some(HealthDescriptor {
        max: 100.0,
        ..HealthDescriptor::default()
    });
    player.life_states = Some(LifeStatesDescriptor {
        initial: "alive".to_string(),
        states: BTreeMap::from([
            ("alive".to_string(), LifeStateProfile::default()),
            ("downed".to_string(), LifeStateProfile::default()),
        ]),
        rejoin: BTreeMap::new(),
        state_ids: BTreeMap::from([("alive".to_string(), 0), ("downed".to_string(), 1)]),
        table_revision: 1,
    });
    let descriptors = vec![
        player,
        weapon(
            "shotgun",
            serde_json::json!({"kind": "ammo", "type": "shells", "magazine": 6, "reserve": 12}),
        ),
        weapon(
            "plasma",
            serde_json::json!({"kind": "heat", "heatPerShot": 10.0, "overheatAt": 100.0,
                "coolPerSecond": 5.0, "coolDelayMs": 0.0}),
        ),
        descriptor("scout"),
    ];
    let defense = DefenseVocabulary {
        layers: vec!["armor".to_string()],
        ..DefenseVocabulary::default()
    };
    (descriptors, defense)
}

fn full_carry() -> CarriedState {
    let mut carried = CarriedState {
        health_current: Some(73.25),
        life_state: Some("alive".to_string()),
        active_slot: 2,
        layers: vec![("armor".to_string(), 12.5)],
        ..CarriedState::default()
    };
    carried.wieldables[0] = Some("plasma".to_string());
    carried.wieldables[2] = Some("shotgun".to_string());
    carried.magazines[0] = Some(0);
    carried.magazines[2] = Some(4);
    carried.reserve.set_exact("shells", 19);
    carried
}

#[test]
fn carried_loadout_crosses_the_wire_as_names_and_hundredths() {
    let carried = full_carry();
    let wire = carried_to_wire(Some(&carried)).expect("a carry crosses");
    assert_eq!(wire.health_centi, Some(7_325));
    assert_eq!(wire.layers, vec![("armor".to_string(), 1_250)]);
    assert_eq!(wire.reserve, vec![("shells".to_string(), 19)]);
    assert_eq!(
        wire.wieldables,
        vec![
            Some("plasma".to_string()),
            None,
            Some("shotgun".to_string())
        ],
        "trailing empty slots are omitted"
    );
    let mut warnings = Vec::new();
    assert_eq!(carried_from_wire(&wire, &mut warnings), Some(carried));
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(carried_to_wire(None), None, "no carry crosses as none");

    // Hundredths round: a carried value off the grid lands on its nearest.
    let off_grid = CarriedState {
        health_current: Some(10.004),
        ..CarriedState::default()
    };
    assert_eq!(
        carried_to_wire(Some(&off_grid)).unwrap().health_centi,
        Some(1_000)
    );
}

#[test]
fn join_loadout_this_mod_defines_restores_whole() {
    let (descriptors, defense) = host_mod();
    let gear = HostGear::new(&descriptors, &defense, &[]);
    let carried = full_carry();
    let wire = carried_to_wire(Some(&carried)).unwrap();
    let mut warnings = Vec::new();
    assert_eq!(
        check_join_loadout(&wire, &gear, &mut warnings),
        Some(carried)
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn join_loadout_check_drops_and_clamps_what_the_host_mod_lacks() {
    let (descriptors, defense) = host_mod();
    let gear = HostGear::new(&descriptors, &defense, &[]);
    let wire = CarriedLoadoutWire {
        health_centi: Some(25_000),
        life_state: Some("downed".to_string()),
        reserve: vec![("shells".to_string(), 30), ("rockets".to_string(), 5)],
        wieldables: vec![
            Some("shotgun".to_string()),
            Some("railgun".to_string()),
            Some("scout".to_string()),
            None,
            Some("plasma".to_string()),
        ],
        magazines: vec![Some(40), Some(3), None, Some(2), Some(9)],
        active_slot: 1,
        layers: vec![("armor".to_string(), 5_000), ("shield".to_string(), 2_000)],
    };
    let mut warnings = Vec::new();
    let checked = check_join_loadout(&wire, &gear, &mut warnings).expect("a carry remains");

    let mut expected = CarriedState {
        health_current: Some(100.0),
        life_state: None,
        active_slot: 0,
        layers: vec![("armor".to_string(), 50.0)],
        ..CarriedState::default()
    };
    expected.wieldables[0] = Some("shotgun".to_string());
    expected.wieldables[4] = Some("plasma".to_string());
    expected.magazines[0] = Some(6);
    expected.magazines[4] = Some(0);
    expected.reserve.set_exact("shells", 30);
    assert_eq!(checked, expected);

    let expected_warnings = [
        "dropped ammo `rockets`",
        "dropped weapon `railgun` in slot 1",
        "dropped weapon `scout` in slot 2",
        "clamped weapon `shotgun` magazine in slot 0 from 40 to its magazine size 6",
        "dropped the magazine in slot 3: the slot holds no weapon",
        "clamped weapon `plasma` magazine in slot 4 from 9 to its magazine size 0",
        "clamped health from 250 to the player's maximum 100",
        "life state `downed` arrives as the initial state",
        "dropped defense layer `shield`",
        "active slot 1 holds no kept weapon; selecting slot 0",
    ];
    for expected in expected_warnings {
        assert_eq!(
            warnings
                .iter()
                .filter(|warning| warning.contains(expected))
                .count(),
            1,
            "warns once: {expected}; got {warnings:?}"
        );
    }
    assert_eq!(warnings.len(), expected_warnings.len(), "{warnings:?}");
}

#[test]
fn join_loadout_past_capacity_drops_the_rest_and_selects_slot_zero() {
    let (descriptors, defense) = host_mod();
    let gear = HostGear::new(&descriptors, &defense, &[]);
    let capacity = postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY;
    let mut wieldables = vec![None; capacity + 1];
    wieldables[capacity] = Some("shotgun".to_string());
    let wire = CarriedLoadoutWire {
        wieldables,
        active_slot: u32::try_from(capacity).unwrap() + 3,
        ..CarriedLoadoutWire::default()
    };
    let mut warnings = Vec::new();
    let checked = check_join_loadout(&wire, &gear, &mut warnings).unwrap();
    assert!(checked.wieldables.iter().all(Option::is_none));
    assert_eq!(checked.active_slot, 0);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
}
