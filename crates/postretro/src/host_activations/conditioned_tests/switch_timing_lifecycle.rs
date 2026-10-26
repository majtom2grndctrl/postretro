// Equip gate retirement and released inventory acceptance across received state.
// See: context/lib/networking.md §Combat authority

use super::*;

fn host_tuning(fixture: &Fixture) -> postretro_combat_model::TuningPayload {
    netcode::tuning_payload_for_pawn(
        &fixture.host.borrow(),
        fixture.host_actors.pawn,
        &fixture.descriptors,
        None,
        &postretro_combat_model::EnvironmentTuning {
            level_gravity: 0.0,
            fluids: Default::default(),
        },
    )
}

fn install_inventory(
    fixture: &Fixture,
    registry: &Rc<RefCell<EntityRegistry>>,
    pawn: EntityId,
    payload: &postretro_combat_model::TuningPayload,
) {
    assert!(crate::scripting::builtins::net_descriptor::materialize_net_local_wieldable_inventory_from_tuning(
        "player",
        &fixture.descriptors,
        &mut registry.borrow_mut(),
        pawn,
        Some(payload),
    ));
}

fn weapon_at(fixture: &Fixture, slot: usize) -> EntityId {
    fixture
        .host
        .borrow()
        .get_component::<Inventory>(fixture.host_actors.pawn)
        .unwrap()
        .wieldables[slot]
        .unwrap()
}

fn prepare_reload(fixture: &Fixture, slot: usize) -> EntityId {
    let target = weapon_at(fixture, slot);
    let mut registry = fixture.host.borrow_mut();
    let mut component = registry
        .get_component::<WeaponComponent>(target)
        .unwrap()
        .clone();
    component.magazine = 20;
    let ammo_type = component.ammo.as_ref().unwrap().ammo_type.clone();
    registry.set_component(target, component).unwrap();
    let mut reserve = registry
        .get_component::<postretro_entities::AmmoReserve>(fixture.host_actors.pawn)
        .unwrap()
        .clone();
    reserve.take(&ammo_type, u32::MAX);
    registry
        .set_component(fixture.host_actors.pawn, reserve)
        .unwrap();
    assert_eq!(
        postretro_entities::components::grant::grant_ammo(
            &mut registry,
            fixture.host_actors.pawn,
            &ammo_type,
            100.0,
        ),
        postretro_entities::components::grant::GrantOutcome::Applied
    );
    target
}

#[test]
fn conditioned_actual_target_release_and_same_tick_refill_refuse_the_old_switch() {
    for refill in [false, true] {
        let mut fixture = Fixture::with_switch_timing(600, 100, mandated_link());
        fixture.step(90, selecting(1));
        fixture.idle(91, 20);
        let old_target = weapon_at(&fixture, 1);
        let original = host_tuning(&fixture);
        let mut emptied = original.clone();
        emptied.wieldables[1] = None;
        let host_tick = fixture.tick;
        install_inventory(&fixture, &fixture.host, fixture.host_actors.pawn, &emptied);
        assert!(
            fixture
                .host
                .borrow()
                .get_component::<Inventory>(fixture.host_actors.pawn)
                .unwrap()
                .wieldables[1]
                .is_none()
        );
        if refill {
            install_inventory(&fixture, &fixture.host, fixture.host_actors.pawn, &original);
            assert_ne!(weapon_at(&fixture, 1), old_target);
        }
        assert_eq!(
            fixture.tick, host_tick,
            "release and refill occur without a host resolution between them"
        );
        fixture.idle(111, 120);
        fixture.assert_settled_selection(0, &["switch-origin"]);
        assert!(matches!(
            fixture
                .switch_receive
                .as_ref()
                .unwrap()
                .resolutions
                .as_slice(),
            [netcode::CurrentSwitchResolution::Refused { .. }]
        ));
        fixture.step(231, held(ActivationLane::Primary, true));
        fixture.idle(232, 100);
        assert!(
            fixture
                .authorized
                .iter()
                .any(|shot| shot.weapon == fixture.host_actors.weapon),
            "origin remains firable after actual release, refill={refill}"
        );
    }
}

#[test]
fn conditioned_late_outgoing_drop_resend_preserves_an_already_repointed_owner() {
    let mut fixture = Fixture::with_switch_timing(600, 200, mandated_link());
    fixture.hold_tuning_updates();
    let installs = fixture.receive.as_ref().unwrap().control_installs;
    fixture.step(90, selecting(1));
    fixture.idle(91, 17);
    let mut drop = neutral();
    drop.drop_pressed = true;
    drop.movement.drop_pressed = true;
    fixture.step(108, drop);
    fixture.idle(109, 100);
    {
        let registry = fixture.local.borrow();
        let inventory = registry
            .get_component::<Inventory>(fixture.local_actors.pawn)
            .unwrap();
        assert_eq!(
            inventory.active_slot, 1,
            "owner reached its own lower end before the resend"
        );
        assert!(
            inventory.wieldables[0].is_some(),
            "outgoing drop has not installed yet"
        );
    }
    assert_eq!(fixture.receive.as_ref().unwrap().control_installs, installs);
    assert!(
        fixture
            .host
            .borrow()
            .get_component::<Inventory>(fixture.host_actors.pawn)
            .unwrap()
            .wieldables[0]
            .is_none()
    );
    fixture.release_tuning();
    fixture.idle(209, 100);
    fixture.assert_settled_selection(1, &["switch-origin", "switch-target"]);
    assert!(fixture.receive.as_ref().unwrap().control_installs > installs);
    assert!(
        fixture
            .local
            .borrow()
            .get_component::<Inventory>(fixture.local_actors.pawn)
            .unwrap()
            .wieldables[0]
            .is_none()
    );
    assert!(matches!(
        fixture
            .switch_receive
            .as_ref()
            .unwrap()
            .resolutions
            .as_slice(),
        [netcode::CurrentSwitchResolution::Accepted { .. }]
    ));
}

#[test]
fn conditioned_switch_during_raise_starts_a_fresh_lower_and_replaces_the_raise_gate() {
    let mut fixture = Fixture::with_switch_timing(200, 1200, mandated_link());
    let third = prepare_reload(&fixture, 2);
    // Host tuning owns the shorter incoming raise; install it before either
    // peer begins, then both equip paths consume precisely the same value.
    let mut payload = host_tuning(&fixture);
    payload.wieldables[2].as_mut().unwrap().raise_ms = 1;
    install_inventory(&fixture, &fixture.host, fixture.host_actors.pawn, &payload);
    install_inventory(
        &fixture,
        &fixture.local,
        fixture.local_actors.pawn,
        &payload,
    );
    for tick in 90..150 {
        let mut command = match tick {
            90 => selecting(1),
            110 => selecting(2),
            _ => neutral(),
        };
        command.reload = tick == 123;
        fixture.step(tick, command);
    }
    fixture.idle(150, 130);
    fixture.assert_settled_selection(2, &["switch-origin", "switch-target", "switch-third"]);
    assert_eq!(fixture.host_selection_changes(), [(101, 1), (121, 2)]);
    assert_eq!(
        fixture
            .reloads
            .iter()
            .filter(
                |(_, weapon, outcome)| *weapon == third && *outcome == sim::ReloadOutcome::Started
            )
            .count(),
        1
    );
    let (_, command) = fixture
        .resolved_commands
        .iter()
        .find(|(_, command)| command.reload_press_tick == Some(123))
        .unwrap();
    assert_eq!(command.client_tick, 123);
    assert!(
        command.reload_press_tick.unwrap() < fixture.host_selection_changes()[0].0 + 72,
        "new reload precedes the abandoned long raise boundary"
    );
    assert_eq!(
        fixture.switch_receive.as_ref().unwrap().resolutions.len(),
        2
    );
    let receive = fixture.switch_receive.as_ref().unwrap();
    assert_eq!(
        receive
            .raw_outcomes
            .iter()
            .map(|(_, id, accepted)| (*id, *accepted))
            .collect::<Vec<_>>(),
        [(0, true), (1, true)]
    );
    // Settling an accepted older declaration leaves the newer pending choice
    // alone, so its current-choice resolution may deliberately be None.
    assert!(receive.resolutions.iter().all(|outcome| matches!(
        outcome,
        netcode::CurrentSwitchResolution::Accepted { .. } | netcode::CurrentSwitchResolution::None
    )));
    assert!(matches!(
        receive.resolutions.last(),
        Some(netcode::CurrentSwitchResolution::Accepted { .. })
    ));
}

#[test]
fn conditioned_removing_the_raising_weapon_allows_the_next_ticks_reload() {
    let mut fixture = Fixture::with_switch_timing(0, 1200, mandated_link());
    let origin = prepare_reload(&fixture, 0);
    fixture.step(90, selecting(1));
    fixture.idle(91, 29);
    let mut payload = host_tuning(&fixture);
    payload.wieldables[1] = None;
    // A removal delivered on both peers between fixed commands exercises the
    // production release rule before the very next stamped reload command.
    install_inventory(&fixture, &fixture.host, fixture.host_actors.pawn, &payload);
    install_inventory(
        &fixture,
        &fixture.local,
        fixture.local_actors.pawn,
        &payload,
    );
    let mut reload = neutral();
    reload.reload = true;
    fixture.step(120, reload);
    fixture.idle(121, 120);
    fixture.assert_settled_selection(0, &["switch-origin", "switch-target", "switch-origin"]);
    assert_eq!(
        fixture
            .reloads
            .iter()
            .filter(
                |(_, weapon, outcome)| *weapon == origin && *outcome == sim::ReloadOutcome::Started
            )
            .count(),
        1
    );
    assert!(
        fixture
            .resolved_commands
            .iter()
            .any(|(_, command)| command.reload_press_tick == Some(120))
    );
}

#[test]
fn conditioned_dropped_lowering_instance_is_idle_and_fires_for_either_next_holder() {
    for remote_holder in [false, true] {
        let mut fixture = Fixture::with_switch_timing(600, 100, mandated_link());
        let outgoing = fixture.host_actors.weapon;
        fixture.step(90, selecting(1));
        fixture.idle(91, 17);
        let mut drop = neutral();
        drop.drop_pressed = true;
        drop.movement.drop_pressed = true;
        fixture.step(108, drop);
        fixture.idle(109, 130);
        fixture.assert_settled_selection(1, &["switch-origin", "switch-target"]);
        assert_eq!(
            fixture
                .host
                .borrow()
                .get_component::<WeaponComponent>(outgoing)
                .unwrap()
                .state,
            postretro_entities::components::wieldable_state::WieldableState::Idle
        );
        assert!(
            fixture
                .host
                .borrow()
                .get_component::<postretro_entities::components::touchable::TouchableComponent>(
                    outgoing
                )
                .is_ok(),
            "actual touch-stage drop left a world pickup"
        );
        if remote_holder {
            assert!(sim::acquire_wieldable_at_for_test(
                &mut fixture.host.borrow_mut(),
                fixture.host_actors.pawn,
                0,
                outgoing
            ));
            fixture.idle(239, 100);
            assert!(
                fixture
                    .local
                    .borrow()
                    .get_component::<Inventory>(fixture.local_actors.pawn)
                    .unwrap()
                    .wieldables[0]
                    .is_some(),
                "reacquisition arrives in production tuning"
            );
            fixture.step(339, selecting(0));
            fixture.idle(340, 100);
            fixture.step(440, held(ActivationLane::Primary, true));
            fixture.idle(441, 100);
            fixture
                .assert_settled_selection(0, &["switch-origin", "switch-target", "switch-origin"]);
            assert!(
                fixture
                    .authorized
                    .iter()
                    .any(|shot| shot.weapon == outgoing),
                "the host admits the new holder's start on the same dropped instance"
            );
        } else {
            let host_pawn = fixture.host.borrow().local_player_pawn().unwrap();
            fixture
                .host
                .borrow_mut()
                .set_component(host_pawn, Inventory::default())
                .unwrap();
            assert!(sim::acquire_wieldable_at_for_test(
                &mut fixture.host.borrow_mut(),
                host_pawn,
                0,
                outgoing
            ));
            let magazine = fixture
                .host
                .borrow()
                .get_component::<WeaponComponent>(outgoing)
                .unwrap()
                .magazine;
            sim::run_local_weapon_fire_for_test(
                &fixture.host,
                host_pawn,
                &weapon::WeaponFireCommand {
                    button: weapon::FireButtonState {
                        pressed: true,
                        active: true,
                    },
                    aim_origin: Vec3::new(20.0, 0.5, 0.0),
                    aim_direction: Vec3::NEG_Z,
                    can_fire: true,
                },
                &fixture.world,
                &fixture.zones,
                DT,
            );
            assert!(
                fixture
                    .host
                    .borrow()
                    .get_component::<WeaponComponent>(outgoing)
                    .unwrap()
                    .magazine
                    < magazine,
                "host-local weapon stage admits and debits the acquired world instance"
            );
        }
    }
}

#[test]
fn conditioned_drop_during_a_long_raise_retires_the_gate_before_the_old_expiry() {
    let mut fixture = Fixture::with_switch_timing(0, 5000, mandated_link());
    let origin = prepare_reload(&fixture, 0);
    fixture.step(90, selecting(1));
    fixture.idle(91, 29);
    let mut drop = neutral();
    drop.drop_pressed = true;
    drop.movement.drop_pressed = true;
    fixture.step(120, drop);
    // Let the authoritative drop and inventory resend arrive without producing
    // another owner command. Its very next stamped tick is still 121.
    for _ in 0..40 {
        fixture.host_tick();
    }
    assert_eq!(
        fixture
            .local
            .borrow()
            .get_component::<Inventory>(fixture.local_actors.pawn)
            .unwrap()
            .active_slot,
        0
    );
    let mut reload = neutral();
    reload.reload = true;
    fixture.step(121, reload);
    fixture.idle(122, 200);
    fixture.assert_settled_selection(0, &["switch-origin", "switch-target", "switch-origin"]);
    assert_eq!(
        fixture
            .reloads
            .iter()
            .filter(
                |(_, weapon, outcome)| *weapon == origin && *outcome == sim::ReloadOutcome::Started
            )
            .count(),
        1
    );
    let (started_host_tick, _, _) = fixture
        .reloads
        .iter()
        .find(|(_, weapon, outcome)| *weapon == origin && *outcome == sim::ReloadOutcome::Started)
        .unwrap();
    let delivered_tick = fixture
        .resolved_commands
        .iter()
        .find(|(host_tick, _)| host_tick == started_host_tick)
        .unwrap()
        .1
        .reload_press_tick
        .unwrap();
    assert_eq!(delivered_tick, 121);
    assert!(
        delivered_tick < fixture.host_selection_changes()[0].0 + 300,
        "reload precedes the dropped weapon's old raise expiry"
    );
}
