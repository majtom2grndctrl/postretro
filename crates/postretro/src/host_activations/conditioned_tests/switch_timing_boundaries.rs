// Reload and received-selection boundaries across the conditioned production lanes.
// See: context/lib/networking.md §Combat authority

use super::*;

fn seed_reload_immediately_before_begin(fixture: &mut Fixture) {
    if fixture.queues.resolved_cursor_for_test(CLIENT) == Some(99)
        && fixture.queues.has_pending_tick_for_test(CLIENT, 100)
    {
        fixture.seed_origin_reload(16, false);
        fixture.before_switch_resolution = None;
    }
}

#[test]
fn conditioned_atomic_reload_due_on_begin_completes_only_for_multitick_lower() {
    for lower_ms in [0, 1, 600] {
        let mut fixture = Fixture::with_switch_timing(lower_ms, 100, mandated_link());
        fixture.before_switch_resolution = Some(seed_reload_immediately_before_begin);
        let mut owner_repoint = None;
        for tick in 90..250 {
            let mut command = if tick == 100 { selecting(1) } else { neutral() };
            command.reload = tick == 100;
            fixture.step(tick, command);
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
        assert!(
            fixture.before_switch_resolution.is_none(),
            "the real queue reached the exact begin boundary"
        );
        let completed = lower_ms == 600;
        let completions: Vec<_> = fixture
            .reloads
            .iter()
            .filter(|(_, weapon, outcome)| {
                *weapon == fixture.host_actors.weapon
                    && matches!(outcome, sim::ReloadOutcome::Completed { .. })
            })
            .collect();
        assert_eq!(
            completions.len(),
            usize::from(completed),
            "lowerMs={lower_ms}"
        );
        if completed {
            let (host_tick, _, outcome) = completions[0];
            assert_eq!(
                fixture
                    .resolved_commands
                    .iter()
                    .find(|(tick, _)| tick == host_tick)
                    .unwrap()
                    .1
                    .client_tick,
                100
            );
            assert_eq!(*outcome, sim::ReloadOutcome::Completed { transferred: 10 });
        }
        assert_eq!(
            fixture.host_selection_changes(),
            [(owner_repoint.unwrap(), 1)]
        );
        assert!(
            !fixture.reloads.iter().any(|(_, weapon, outcome)| {
                *weapon == fixture.host_actors.weapon && *outcome == sim::ReloadOutcome::Started
            }),
            "a press on begin starts no new reload"
        );
        let expected = if completed { (30, 90) } else { (20, 100) };
        assert_eq!(fixture.origin_reload_balance(), expected);
        fixture.step(250, selecting(0));
        fixture.idle(251, 150);
        fixture.assert_settled_selection(0, &["switch-origin", "switch-target", "switch-origin"]);
        assert_eq!(fixture.origin_reload_balance(), expected);
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
            fixture.switch_receive.as_ref().unwrap().resolutions.len(),
            2
        );
    }
}

#[test]
fn conditioned_received_owner_and_observer_selections_follow_the_repoint_snapshot() {
    const BEGIN: u32 = 100;
    const REPOINT: u32 = 135;
    let mut fixture = Fixture::with_switch_timing(600, 100, mandated_link());
    let mut owner_lower_samples = 0;
    let mut observer_lower_samples = 0;
    let mut owner_after_samples = 0;
    let mut observer_after_samples = 0;
    let mut last_owner_source = None;
    let mut last_observer_source = None;
    for tick in 90..300 {
        fixture.step(
            tick,
            if tick == BEGIN {
                selecting(1)
            } else {
                neutral()
            },
        );
        let netcode::NetEndpoint::Client { replication, .. } =
            &fixture.receive.as_ref().unwrap().endpoint
        else {
            unreachable!()
        };
        let switch = fixture.switch_receive.as_ref().unwrap();
        for (source_tick, last_source, owner) in [
            (
                replication.latest_server_tick(),
                &mut last_owner_source,
                true,
            ),
            (
                switch.observer.replication.latest_server_tick(),
                &mut last_observer_source,
                false,
            ),
        ] {
            let Some(source_tick) = source_tick else {
                continue;
            };
            if *last_source == Some(source_tick) {
                continue;
            }
            *last_source = Some(source_tick);
            let Some((_, resolved)) = fixture
                .resolved_commands
                .iter()
                .find(|(host_tick, _)| *host_tick == source_tick)
            else {
                continue;
            };
            let expected_slot = usize::from(resolved.client_tick >= REPOINT);
            if owner {
                assert_eq!(
                    fixture.projection.agreed_slot(),
                    Some(expected_slot),
                    "owner-private source client tick {}",
                    resolved.client_tick
                );
            } else {
                assert_eq!(
                    switch
                        .observer
                        .replication
                        .observed_active_weapon_archetype(wire::NetworkId(fixture.pawn_network())),
                    Some(if expected_slot == 0 {
                        "switch-origin"
                    } else {
                        "switch-target"
                    }),
                    "observer source client tick {}",
                    resolved.client_tick
                );
            }
            if (BEGIN..REPOINT).contains(&resolved.client_tick) {
                if owner {
                    owner_lower_samples += 1
                } else {
                    observer_lower_samples += 1
                }
            } else if resolved.client_tick >= REPOINT {
                if owner {
                    owner_after_samples += 1
                } else {
                    observer_after_samples += 1
                }
            }
        }
    }
    assert!(owner_lower_samples > 1 && observer_lower_samples > 1);
    assert!(owner_after_samples > 1 && observer_after_samples > 1);
    assert_eq!(fixture.host_selection_changes(), [(REPOINT, 1)]);
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

#[test]
fn client_resync_detector_sends_nothing_for_real_pending_lowering_or_raising_state() {
    use crate::client_weapon::{ActiveSlotResync, ActiveSlotResyncObservation};
    use postretro_entities::components::wieldable_state::WieldableState;
    let fixture = Fixture::with_switch_timing(600, 100, mandated_link());
    for (pending, state) in [
        (true, WieldableState::Idle),
        (false, WieldableState::Lowering),
        (false, WieldableState::Raising),
    ] {
        let mut registry = fixture.local.borrow_mut();
        let weapon = registry
            .get_component::<Inventory>(fixture.local_actors.pawn)
            .unwrap()
            .active_wieldable()
            .unwrap();
        let Ok(ComponentValue::Weapon(component)) =
            registry.get_component_value_mut(weapon, ComponentKind::Weapon)
        else {
            panic!("active wieldable is a weapon")
        };
        component.state = state;
        let observation = ActiveSlotResyncObservation::read(
            &registry,
            fixture.local_actors.pawn,
            pending,
            Some(2),
        )
        .unwrap();
        assert_eq!(observation.declarations_pending, pending);
        assert_eq!(observation.local_switching, !pending);
        let mut detector = ActiveSlotResync::default();
        for _ in 0..300 {
            assert_eq!(
                detector.observe(observation, DT),
                None,
                "pending={pending}, state={state:?}"
            );
        }
    }
}
