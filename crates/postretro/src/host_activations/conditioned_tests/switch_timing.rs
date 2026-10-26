// Client switch settlement and a second peer's received selection under conditioning.
// See: context/lib/networking.md §Combat authority

use super::*;

#[path = "switch_timing_fixture.rs"]
mod fixture;
pub(super) use fixture::SwitchReceive;

#[test]
fn conditioned_mid_lower_drop_settles_owner_and_observer_on_the_target() {
    let mut fixture = Fixture::with_switch_timing(600, 200, mandated_link());
    let outgoing = fixture.host_actors.weapon;
    let mut switch = neutral();
    switch.select_slot = Some(1);
    fixture.step(90, switch);
    fixture.idle(91, 17);
    {
        let registry = fixture.local.borrow();
        let inventory = registry
            .get_component::<Inventory>(fixture.local_actors.pawn)
            .unwrap();
        assert_eq!(
            inventory.active_slot, 0,
            "owner holds the outgoing weapon during its lower"
        );
        assert_eq!(inventory.switch_target, Some(1));
    }
    let mut drop = neutral();
    drop.movement.drop_pressed = true;
    drop.drop_pressed = true;
    fixture.step(108, drop);
    fixture.idle(109, 150);
    for (registry, pawn) in [
        (&fixture.host, fixture.host_actors.pawn),
        (&fixture.local, fixture.local_actors.pawn),
    ] {
        let registry = registry.borrow();
        let inventory = registry.get_component::<Inventory>(pawn).unwrap();
        assert!(
            inventory.wieldables[0].is_none(),
            "outgoing weapon leaves the inventory: pawn={pawn}, inventory={inventory:?}, drop commands={:?}, host pose={:?}",
            fixture
                .resolved_commands
                .iter()
                .filter(|(_, resolved)| resolved.command.drop_pressed)
                .map(|(_, resolved)| resolved.client_tick)
                .collect::<Vec<_>>(),
            fixture
                .host
                .borrow()
                .get_component::<Transform>(fixture.host_actors.pawn)
        );
        assert_eq!(inventory.active_slot, 1);
        assert_eq!(inventory.switch_target, None);
        assert_eq!(inventory.switch_origin, None);
    }
    assert_eq!(
        fixture
            .host
            .borrow()
            .get_component::<WeaponComponent>(outgoing)
            .unwrap()
            .state,
        postretro_entities::components::wieldable_state::WieldableState::Idle
    );
    let lowering_resolutions: Vec<_> = fixture
        .resolved_commands
        .iter()
        .filter(|(_, command)| (90..108).contains(&command.client_tick))
        .collect();
    assert!(
        !lowering_resolutions.is_empty(),
        "host resolved commands during the lower"
    );
    for (host_tick, _) in lowering_resolutions {
        let slot = fixture
            .host_active
            .iter()
            .find(|(tick, _)| tick == host_tick)
            .unwrap()
            .1;
        assert_eq!(slot, 0, "host holds outgoing slot before the drop");
    }
    let receive = fixture.receive.as_ref().unwrap();
    assert!(!receive.endpoint.has_pending_switch_declarations());
    assert!(
        receive.control_installs >= 2,
        "inventory resend installed after drop"
    );
    let switch = fixture.switch_receive.as_ref().unwrap();
    assert!(
        switch
            .owner_slots
            .iter()
            .any(|(tick, slot)| (90..108).contains(tick) && *slot == Some(0)),
        "received owner-private samples name the outgoing weapon during the lower"
    );
    assert!(
        switch
            .owner_slots
            .iter()
            .filter(|(tick, _)| *tick < 108)
            .all(|(_, slot)| slot.is_none_or(|slot| slot == 0))
    );
    assert_eq!(switch.owner_slots.last().unwrap().1, Some(1));
    assert!(matches!(
        switch.resolutions.as_slice(),
        [netcode::CurrentSwitchResolution::Accepted { .. }]
    ));
    assert_eq!(
        switch
            .observer
            .selections
            .iter()
            .map(|(_, weapon)| weapon.as_str())
            .collect::<Vec<_>>(),
        ["switch-origin", "switch-target"]
    );
}

// Declaration hold is separate from a packet outage: commands keep resolving
// while the declaration waits, then the normal pending chain settles it.
#[test]
fn delayed_switch_declaration_keeps_input_flowing_and_settles_once() {
    let mut fixture = Fixture::with_switch_timing(600, 200, LinkConfig::perfect());
    fixture.switch_receive.as_mut().unwrap().hold_declarations = true;
    let mut command = neutral();
    command.select_slot = Some(1);
    fixture.step(90, command);
    fixture.idle(91, 5);
    assert!(fixture.sent_switches.is_empty());
    assert!(
        fixture
            .resolved_commands
            .iter()
            .any(|(_, resolved)| resolved.client_tick > 90)
    );
    assert!(
        fixture
            .receive
            .as_ref()
            .unwrap()
            .endpoint
            .has_pending_switch_declarations()
    );
    fixture.release_switch_declarations();
    fixture.idle(96, 150);
    assert_eq!(fixture.sent_switches.len(), 1);
    assert!(
        !fixture
            .receive
            .as_ref()
            .unwrap()
            .endpoint
            .has_pending_switch_declarations()
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
    for (registry, pawn) in [
        (&fixture.host, fixture.host_actors.pawn),
        (&fixture.local, fixture.local_actors.pawn),
    ] {
        assert_eq!(
            registry
                .borrow()
                .get_component::<Inventory>(pawn)
                .unwrap()
                .active_slot,
            1
        );
    }
}

fn selecting(slot: usize) -> sim::SimCommand {
    let mut command = neutral();
    command.select_slot = Some(slot);
    command
}

#[test]
fn conditioned_lower_boundaries_match_the_owners_actual_repoint_tick() {
    for lower_ms in [0, 1, 17, 50, 51] {
        let mut fixture = Fixture::with_switch_timing(lower_ms, 100, mandated_link());
        let mut owner_repoint = None;
        for tick in 90..150 {
            fixture.step(tick, if tick == 90 { selecting(1) } else { neutral() });
            if owner_repoint.is_none()
                && fixture
                    .local
                    .borrow()
                    .get_component::<Inventory>(fixture.local_actors.pawn)
                    .unwrap()
                    .active_slot
                    == 1
            {
                owner_repoint = Some(tick);
            }
        }
        fixture.idle(150, 100);
        assert_eq!(
            fixture.host_selection_changes(),
            [(owner_repoint.unwrap(), 1)],
            "lowerMs={lower_ms}"
        );
        fixture.assert_settled_selection(1, &["switch-origin", "switch-target"]);
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
}

#[test]
fn conditioned_retarget_chains_repoint_once_and_settle_every_declaration() {
    // The 200ms lower reaches its twelfth client machine step on tick 101.
    // Include a declaration on that step, before the owner's machine repoints.
    for (lower_ms, retargets, expected_tick, final_slot, observed) in [
        (600, vec![(95, 2)], 125, 2, "switch-third"),
        (200, vec![(101, 2)], 101, 2, "switch-third"),
        (600, vec![(95, 2), (99, 3)], 125, 3, "switch-fourth"),
    ] {
        let mut fixture = Fixture::with_switch_timing(lower_ms, 100, mandated_link());
        for tick in 90..160 {
            let command = if tick == 90 {
                selecting(1)
            } else if let Some((_, slot)) = retargets.iter().find(|(at, _)| *at == tick) {
                selecting(*slot)
            } else {
                neutral()
            };
            fixture.step(tick, command);
        }
        fixture.idle(160, 100);
        assert_eq!(
            fixture.host_selection_changes(),
            [(expected_tick, final_slot)]
        );
        fixture.assert_settled_selection(final_slot, &["switch-origin", observed]);
        let resolutions = &fixture.switch_receive.as_ref().unwrap().resolutions;
        assert_eq!(
            resolutions.len(),
            retargets.len() + 1,
            "one outcome per declaration"
        );
        assert!(
            resolutions[..retargets.len()]
                .iter()
                .all(|resolution| *resolution == netcode::CurrentSwitchResolution::None),
            "superseded outcomes settle the real front chain without changing the current switch"
        );
        assert!(matches!(
            resolutions.last(),
            Some(netcode::CurrentSwitchResolution::Accepted { .. })
        ));
    }
}

#[test]
fn conditioned_refused_retarget_rolls_the_whole_pending_chain_back_to_origin() {
    let mut fixture = Fixture::with_switch_timing(600, 100, mandated_link());
    fixture.step(90, selecting(1));
    fixture.idle(91, 4);
    // The owner still holds this slot in its independently received inventory.
    let target = fixture
        .host
        .borrow()
        .get_component::<Inventory>(fixture.host_actors.pawn)
        .unwrap()
        .wieldables[2]
        .unwrap();
    fixture.host.borrow_mut().despawn(target).unwrap();
    fixture.step(95, selecting(2));
    fixture.idle(96, 160);
    assert_eq!(fixture.sent_switches.len(), 2);
    fixture.assert_settled_selection(0, &["switch-origin"]);
    assert!(fixture.host_selection_changes().is_empty());
    assert!(matches!(
        fixture
            .switch_receive
            .as_ref()
            .unwrap()
            .resolutions
            .as_slice(),
        [
            netcode::CurrentSwitchResolution::None,
            netcode::CurrentSwitchResolution::Refused {
                rollback_slot: 0,
                ..
            }
        ]
    ));
    for (registry, pawn) in [
        (&fixture.host, fixture.host_actors.pawn),
        (&fixture.local, fixture.local_actors.pawn),
    ] {
        let registry = registry.borrow();
        let active = registry
            .get_component::<Inventory>(pawn)
            .unwrap()
            .active_wieldable()
            .unwrap();
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(active)
                .unwrap()
                .state,
            postretro_entities::components::wieldable_state::WieldableState::Idle
        );
    }
}

#[test]
fn conditioned_same_tick_retarget_and_drop_land_only_the_latest_target() {
    let mut fixture = Fixture::with_switch_timing(600, 200, mandated_link());
    fixture.step(90, selecting(1));
    fixture.idle(91, 17);
    let mut command = selecting(2);
    command.movement.drop_pressed = true;
    command.drop_pressed = true;
    fixture.step(108, command);
    fixture.idle(109, 160);
    fixture.assert_settled_selection(2, &["switch-origin", "switch-third"]);
    assert!(matches!(
        fixture
            .switch_receive
            .as_ref()
            .unwrap()
            .resolutions
            .as_slice(),
        [
            netcode::CurrentSwitchResolution::None,
            netcode::CurrentSwitchResolution::Accepted { .. }
        ]
    ));
    for (registry, pawn) in [
        (&fixture.host, fixture.host_actors.pawn),
        (&fixture.local, fixture.local_actors.pawn),
    ] {
        assert!(
            registry
                .borrow()
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0]
                .is_none()
        );
    }
    assert_eq!(fixture.host_selection_changes().len(), 1);
}

#[test]
fn remote_reload_raise_boundary_uses_the_press_tick_even_after_catchup_trim() {
    for (raise_ms, expiry) in [(0, 91), (1, 91), (100, 96)] {
        for trimmed in [false, true] {
            for next_tick in [false, true] {
                let mut fixture = Fixture::with_switch_timing(
                    0,
                    raise_ms,
                    if trimmed {
                        mandated_link()
                    } else {
                        LinkConfig::perfect()
                    },
                );
                let target = fixture
                    .host
                    .borrow()
                    .get_component::<Inventory>(fixture.host_actors.pawn)
                    .unwrap()
                    .wieldables[1]
                    .unwrap();
                if let Ok(ComponentValue::Weapon(weapon)) = fixture
                    .host
                    .borrow_mut()
                    .get_component_value_mut(target, ComponentKind::Weapon)
                {
                    weapon.magazine -= 1;
                }
                let press = expiry + u32::from(next_tick);
                for tick in 90..160 {
                    fixture.cut_up = trimmed && tick < 115;
                    let mut command = if tick == 90 { selecting(1) } else { neutral() };
                    command.reload = tick == press;
                    fixture.step(tick, command);
                }
                fixture.idle(160, 100);
                let started = fixture
                    .reloads
                    .iter()
                    .filter(|(_, weapon, outcome)| {
                        *weapon == target && *outcome == sim::ReloadOutcome::Started
                    })
                    .count();
                assert_eq!(
                    started,
                    usize::from(next_tick),
                    "raiseMs={raise_ms}, trimmed={trimmed}, press={press}"
                );
                if trimmed {
                    assert!(
                        fixture
                            .playout
                            .windows(2)
                            .any(|pair| pair[1].1.wrapping_sub(pair[0].1) > 1),
                        "the outage must actually trim the command cursor"
                    );
                }
                fixture.assert_settled_selection(1, &["switch-origin", "switch-target"]);
            }
        }
    }
}

#[test]
fn held_trigger_first_predicted_shot_after_raise_is_admitted_on_clean_and_trimmed_links() {
    for raise_ms in [0, 1, 100] {
        for trimmed in [false, true] {
            let mut fixture = Fixture::with_switch_timing(
                0,
                raise_ms,
                if trimmed {
                    mandated_link()
                } else {
                    LinkConfig::perfect()
                },
            );
            for tick in 90..145 {
                fixture.cut_up = trimmed && tick < 115;
                let mut command = held(ActivationLane::Primary, tick == 90);
                if tick == 90 {
                    command.select_slot = Some(1);
                }
                fixture.step(tick, command);
            }
            fixture.idle(145, 160);
            let first = fixture
                .snapshots
                .first()
                .expect("held trigger fires after the raise")
                .activation
                .shot_id;
            assert_eq!(first.start_tick, if raise_ms == 100 { 96 } else { 91 });
            assert!(
                fixture.authorized.iter().any(|shot| shot.shot_id == first),
                "host admits the first predicted start after the raise: raiseMs={raise_ms}, trimmed={trimmed}"
            );
            fixture.assert_settled_selection(1, &["switch-origin", "switch-target"]);
        }
    }
}

#[test]
fn conditioned_switch_abandons_nonblocking_reload_without_later_transfer() {
    for lower_ms in [0, 600] {
        let mut fixture = Fixture::with_switch_timing(lower_ms, 100, mandated_link());
        fixture.seed_origin_reload(10_000, false);
        fixture.idle(90, 10);
        fixture.step(100, selecting(1));
        fixture.idle(101, 109);
        assert_eq!(fixture.origin_reload_balance(), (20, 100));
        fixture.step(210, selecting(0));
        fixture.idle(211, 150);
        fixture.assert_settled_selection(0, &["switch-origin", "switch-target", "switch-origin"]);
        assert_eq!(fixture.origin_reload_balance(), (20, 100));
        assert_eq!(
            fixture
                .host
                .borrow()
                .get_component::<WeaponComponent>(fixture.host_actors.weapon)
                .unwrap()
                .state,
            postretro_entities::components::wieldable_state::WieldableState::Idle
        );
        assert!(
            !fixture
                .reloads
                .iter()
                .any(|(_, weapon, outcome)| *weapon == fixture.host_actors.weapon
                    && matches!(outcome, sim::ReloadOutcome::Completed { .. }))
        );
        assert_eq!(
            fixture.switch_receive.as_ref().unwrap().resolutions.len(),
            2
        );
        assert!(
            fixture
                .switch_receive
                .as_ref()
                .unwrap()
                .resolutions
                .iter()
                .all(|resolution| matches!(
                    resolution,
                    netcode::CurrentSwitchResolution::Accepted { .. }
                ))
        );
    }
}

#[test]
fn atomic_reload_due_on_switch_begin_completes_only_before_a_multitick_lower() {
    // A clean stream makes the seeded machine's next pass unambiguous. Seed
    // from the actual resolved client cursor, never an assumed host delay.
    for lower_ms in [0, 1, 600] {
        let mut fixture = Fixture::with_switch_timing(lower_ms, 100, LinkConfig::perfect());
        let mut seeded = false;
        for tick in 90..250 {
            let mut command = if tick == 100 { selecting(1) } else { neutral() };
            command.reload = tick == 100;
            fixture.step(tick, command);
            if !seeded
                && fixture
                    .resolved_commands
                    .last()
                    .is_some_and(|(_, command)| command.client_tick == 99)
            {
                fixture.seed_origin_reload(16, false);
                seeded = true;
            }
        }
        assert!(seeded);
        let completions: Vec<_> = fixture
            .reloads
            .iter()
            .filter(|(_, weapon, outcome)| {
                *weapon == fixture.host_actors.weapon
                    && matches!(outcome, sim::ReloadOutcome::Completed { .. })
            })
            .collect();
        let completed = lower_ms == 600;
        assert_eq!(
            completions.len(),
            usize::from(completed),
            "lowerMs={lower_ms}"
        );
        if completed {
            let completion_tick = completions[0].0;
            let resolved = fixture
                .resolved_commands
                .iter()
                .find(|(host_tick, _)| *host_tick == completion_tick)
                .unwrap();
            assert_eq!(
                resolved.1.client_tick, 100,
                "completion belongs to the declaration's begin resolution"
            );
            assert!(matches!(
                completions[0].2,
                sim::ReloadOutcome::Completed { transferred: 10 }
            ));
        }
        assert_eq!(
            fixture.origin_reload_balance(),
            if completed { (30, 90) } else { (20, 100) }
        );
        assert!(
            !fixture
                .reloads
                .iter()
                .any(|(_, weapon, outcome)| *weapon == fixture.host_actors.weapon
                    && *outcome == sim::ReloadOutcome::Started),
            "the reload level on begin starts no new reload"
        );
        fixture.step(250, selecting(0));
        fixture.idle(251, 150);
        fixture.assert_settled_selection(0, &["switch-origin", "switch-target", "switch-origin"]);
        assert_eq!(
            fixture
                .host
                .borrow()
                .get_component::<WeaponComponent>(fixture.host_actors.weapon)
                .unwrap()
                .state,
            postretro_entities::components::wieldable_state::WieldableState::Idle
        );
        assert_eq!(
            fixture.origin_reload_balance(),
            if completed { (30, 90) } else { (20, 100) }
        );
    }
}

#[test]
fn conditioned_blocking_reload_refuses_before_the_owner_can_repoint() {
    let mut fixture = Fixture::with_switch_timing(1200, 100, mandated_link());
    fixture.seed_origin_reload(10_000, true);
    fixture.idle(90, 10);
    fixture.step(100, selecting(1));
    fixture.idle(101, 39);
    // The owner's authored lower would not repoint until tick171. Its actual
    // refusal has already traversed both conditioned directions by tick140.
    fixture.assert_settled_selection(0, &["switch-origin"]);
    assert!(fixture.host_selection_changes().is_empty());
    assert!(matches!(
        fixture
            .switch_receive
            .as_ref()
            .unwrap()
            .resolutions
            .as_slice(),
        [netcode::CurrentSwitchResolution::Refused {
            rollback_slot: 0,
            ..
        }]
    ));
    assert_eq!(fixture.origin_reload_balance(), (20, 100));
    let registry = fixture.local.borrow();
    let inventory = registry
        .get_component::<Inventory>(fixture.local_actors.pawn)
        .unwrap();
    assert_eq!(
        registry
            .get_component::<WeaponComponent>(inventory.active_wieldable().unwrap())
            .unwrap()
            .state,
        postretro_entities::components::wieldable_state::WieldableState::Idle
    );
}

// Regression: the host's active slot must follow the lower on every press tick,
// including its distinct before-touch repoint boundary.
#[test]
fn conditioned_drop_and_reload_presses_follow_every_tick_of_the_lower() {
    const BEGIN: u32 = 100;
    const REPOINT: u32 = 105; // Six 60 Hz machine passes for 100 ms.
    for drop_press in [false, true] {
        for pressed_at in BEGIN..=REPOINT {
            let mut fixture = Fixture::with_switch_timing(100, 0, mandated_link());
            fixture.seed_origin_reload(5000, false);
            {
                let mut registry = fixture.host.borrow_mut();
                let Ok(ComponentValue::Weapon(component)) = registry
                    .get_component_value_mut(fixture.host_actors.weapon, ComponentKind::Weapon)
                else {
                    panic!("origin is a weapon");
                };
                component.state =
                    postretro_entities::components::wieldable_state::WieldableState::Idle;
                component.state_remaining_ms = 0;
                component.state_total_ms = 0;
            }
            fixture.idle(90, BEGIN - 90);
            for tick in BEGIN..=REPOINT + 1 {
                let mut command = neutral();
                command.select_slot = (tick == BEGIN).then_some(1);
                if tick == pressed_at {
                    command.drop_pressed = drop_press;
                    command.movement.drop_pressed = drop_press;
                    command.reload = !drop_press;
                }
                fixture.step(tick, command);
            }
            fixture.idle(REPOINT + 2, 100);
            if drop_press {
                let removed = usize::from(pressed_at == REPOINT);
                for (registry, pawn) in [
                    (&fixture.host, fixture.host_actors.pawn),
                    (&fixture.local, fixture.local_actors.pawn),
                ] {
                    let registry = registry.borrow();
                    let inventory = registry.get_component::<Inventory>(pawn).unwrap();
                    assert!(
                        inventory.wieldables[removed].is_none(),
                        "press at {pressed_at}: slot {removed} is released"
                    );
                    assert_eq!(inventory.active_slot, 1 - removed);
                    assert!(inventory.switch_target.is_none());
                }
                assert!(
                    fixture
                        .resolved_commands
                        .iter()
                        .any(|(_, resolved)| resolved.command.drop_pressed
                            && resolved.client_tick == pressed_at)
                );
            } else {
                assert!(
                    !fixture
                        .reloads
                        .iter()
                        .any(|(_, _, outcome)| *outcome == sim::ReloadOutcome::Started),
                    "reload at {pressed_at} starts nothing during lower or at repoint"
                );
                assert!(
                    fixture
                        .resolved_commands
                        .iter()
                        .any(|(_, resolved)| resolved.command.reload
                            && resolved.reload_press_tick == Some(pressed_at))
                );
            }
            assert!(
                !fixture
                    .receive
                    .as_ref()
                    .unwrap()
                    .endpoint
                    .has_pending_switch_declarations()
            );
        }
    }
}

#[test]
fn conditioned_resync_repoints_immediately_without_lower_or_raise() {
    let mut fixture = Fixture::with_switch_timing(600, 600, mandated_link());
    {
        let mut registry = fixture.local.borrow_mut();
        let mut inventory = registry
            .get_component::<Inventory>(fixture.local_actors.pawn)
            .unwrap()
            .clone();
        inventory.active_slot = 1;
        registry
            .set_component(fixture.local_actors.pawn, inventory)
            .unwrap();
    }
    fixture.idle(90, 10);
    fixture.declare_switch(1, 100);
    fixture.correct_switch_origin(1);
    fixture.unsent_switches.get_mut(&100).unwrap().kind = wire::SwitchDeclarationKind::Resync;
    fixture.step(100, neutral());
    fixture.idle(101, 100);
    fixture.assert_settled_selection(1, &["switch-origin", "switch-target"]);
    let (host_tick, _) = fixture
        .resolved_commands
        .iter()
        .find(|(_, resolved)| resolved.client_tick == 100)
        .unwrap();
    assert_eq!(
        fixture
            .host_active
            .iter()
            .find(|(tick, _)| tick == host_tick)
            .unwrap()
            .1,
        1
    );
    let registry = fixture.host.borrow();
    let target = registry
        .get_component::<Inventory>(fixture.host_actors.pawn)
        .unwrap()
        .active_wieldable()
        .unwrap();
    assert_eq!(
        registry
            .get_component::<WeaponComponent>(target)
            .unwrap()
            .state,
        postretro_entities::components::wieldable_state::WieldableState::Idle
    );
    assert!(
        !fixture
            .queues
            .remote_equip_gate(CLIENT, target, 100)
            .reload_refused
    );
}

#[test]
fn conditioned_target_loss_refuses_the_switch_and_leaves_the_origin_firable() {
    for retenant in [false, true] {
        let mut fixture = Fixture::with_switch_timing(600, 100, mandated_link());
        fixture.step(90, selecting(1));
        fixture.idle(91, 20);
        {
            let mut registry = fixture.host.borrow_mut();
            let mut inventory = registry
                .get_component::<Inventory>(fixture.host_actors.pawn)
                .unwrap()
                .clone();
            let old_target = inventory.wieldables[1].unwrap();
            if retenant {
                let replacement = registry.spawn(Transform::default());
                let weapon = registry
                    .get_component::<WeaponComponent>(old_target)
                    .unwrap()
                    .clone();
                registry.set_component(replacement, weapon).unwrap();
                let provenance = registry
                    .get_component::<DescriptorProvenance>(old_target)
                    .unwrap()
                    .clone();
                registry.set_component(replacement, provenance).unwrap();
                inventory.wieldables[1] = Some(replacement);
                registry
                    .set_component(fixture.host_actors.pawn, inventory)
                    .unwrap();
            } else {
                registry.despawn(old_target).unwrap();
            }
        }
        fixture.idle(111, 100);
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
        let mut fire = held(ActivationLane::Primary, true);
        fixture.predict(211, &mut fire);
        fixture.send_input(211, &fire);
        fixture.host_tick();
        fixture.idle(212, 80);
        assert!(
            fixture
                .authorized
                .iter()
                .any(|shot| shot.weapon == fixture.host_actors.weapon),
            "the origin fires after the refused target loss, retenant={retenant}"
        );
    }
}

#[test]
fn conditioned_outgoing_start_during_lower_is_refused_and_death_keeps_repoint() {
    for dies in [false, true] {
        let mut fixture = Fixture::with_switch_timing(600, 100, mandated_link());
        fixture.step(90, selecting(1));
        fixture.idle(91, 19);
        if dies {
            for (registry, pawn) in [
                (&fixture.host, fixture.host_actors.pawn),
                (&fixture.local, fixture.local_actors.pawn),
            ] {
                let mut registry = registry.borrow_mut();
                let mut health = registry
                    .get_component::<HealthComponent>(pawn)
                    .cloned()
                    .unwrap_or_else(|_| health(None));
                health.current = 0.0;
                registry.set_component(pawn, health).unwrap();
            }
            fixture.step(110, neutral());
        } else {
            fixture.step(110, stamped_start(110));
        }
        fixture.idle(111, 120);
        fixture.assert_settled_selection(1, &["switch-origin", "switch-target"]);
        assert!(fixture.authorized.is_empty());
        if !dies {
            assert!(rejected(&fixture, token(110, ActivationLane::Primary)));
        }
        let (host_tick, _) = fixture
            .resolved_commands
            .iter()
            .find(|(_, resolved)| resolved.client_tick == 125)
            .unwrap();
        assert_eq!(
            fixture
                .host_active
                .iter()
                .find(|(tick, _)| tick == host_tick)
                .unwrap()
                .1,
            1,
            "the repoint remains at the owner's tick whether or not it dies"
        );
    }
}
