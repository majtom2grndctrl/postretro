// Reload press identity across client-tick switch timing and playout trim.
// See: context/lib/networking.md §Combat authority

use super::*;

fn intake_without_resolution(fixture: &mut Fixture) {
    fixture.transport();
    netcode::host_handle_client_messages(
        &mut fixture.server,
        &mut fixture.replication,
        &mut fixture.slots,
        &mut fixture.queues,
        &fixture.owners,
        &mut fixture.pending,
        &mut fixture.open,
        CLIENT,
        fixture.tick,
        u64::from(fixture.tick) * 16_667,
    );
}

// Regression: trim delivered a refused press on A, then its held level started
// a fresh reload on B after the skipped raise despite no release or new press.
#[test]
fn refused_reload_held_through_trim_reloads_incoming_weapon_only_after_release_and_repress() {
    for link in [LinkConfig::perfect(), mandated_link()] {
        let mut fixture = Fixture::with_switch_timing(50, 50, link);
        let target = fixture
            .host
            .borrow()
            .get_component::<Inventory>(fixture.host_actors.pawn)
            .unwrap()
            .wieldables[1]
            .unwrap();
        {
            let mut registry = fixture.host.borrow_mut();
            let Ok(ComponentValue::Weapon(weapon)) =
                registry.get_component_value_mut(target, ComponentKind::Weapon)
            else {
                panic!("incoming slot holds a weapon")
            };
            weapon.magazine -= 1;
        }
        fixture.idle(90, 10);

        // The owner runs the real lower/raise and holds reload from 101. Intake
        // continues, while host command resolution stalls through command 120.
        for tick in 100..=120 {
            let mut command = if tick == 100 { selecting(1) } else { neutral() };
            command.reload = tick >= 101;
            fixture.predict(tick, &mut command);
            fixture.send_input(tick, &command);
            fixture.advance_projectiles();
            intake_without_resolution(&mut fixture);
        }
        for _ in 0..20 {
            intake_without_resolution(&mut fixture);
        }
        let queue = fixture.queues.queue_state_for_test(CLIENT).unwrap();
        assert!(queue.pending_ticks.len() > queue.max_pending_ticks);
        let first = fixture.resolved_commands.len();
        fixture.host_tick();
        fixture.host_tick();
        let recovered = &fixture.resolved_commands[first..];
        assert_eq!(
            recovered
                .iter()
                .map(|(_, command)| command.client_tick)
                .collect::<Vec<_>>(),
            [119, 120],
            "the actual trim skips the repoint at 102 and raise expiry at 105"
        );
        assert_eq!(recovered[0].1.reload_press_tick, Some(101));
        assert_eq!(recovered[0].1.command.firing_slot, 0);
        assert!(recovered[0].1.command.reload);
        assert_eq!(recovered[1].1.reload_press_tick, None);
        assert_eq!(recovered[1].1.command.firing_slot, 1);
        assert!(recovered[1].1.command.reload);
        assert_eq!(fixture.host_selection_changes(), [(120, 1)]);

        for tick in 121..141 {
            let mut command = neutral();
            command.reload = true;
            fixture.step(tick, command);
        }
        assert!(
            fixture
                .reloads
                .iter()
                .all(|(_, _, outcome)| *outcome != sim::ReloadOutcome::Started),
            "the refused hold starts no reload on either weapon, even after incoming raise"
        );

        fixture.step(141, neutral());
        for tick in 142..170 {
            let mut command = neutral();
            command.reload = true;
            fixture.step(tick, command);
        }
        fixture.idle(170, 100);
        let starts: Vec<_> = fixture
            .reloads
            .iter()
            .filter(|(_, _, outcome)| *outcome == sim::ReloadOutcome::Started)
            .collect();
        assert_eq!(starts.len(), 1, "one fresh press starts exactly one reload");
        assert_eq!(starts[0].1, target);
        let started = fixture
            .resolved_commands
            .iter()
            .find(|(host_tick, _)| *host_tick == starts[0].0)
            .unwrap();
        assert_eq!(started.1.reload_press_tick, Some(142));
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
