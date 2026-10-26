// Client tick ordering across host drain stalls and delayed declarations.
// See: context/lib/networking.md §Host input command queue · §Combat authority
use super::*;

fn intake_without_resolution(f: &mut Fixture) {
    f.transport();
    netcode::host_handle_client_messages(
        &mut f.server,
        &mut f.replication,
        &mut f.slots,
        &mut f.queues,
        &f.owners,
        &mut f.pending,
        &mut f.open,
        CLIENT,
        f.tick,
        u64::from(f.tick) * 16_667,
    );
}

// The owner, wire codec, conditioner and intake run; only host drain stalls.
fn stalled_owner_step(f: &mut Fixture, tick: u32, mut command: sim::SimCommand) {
    f.predict(tick, &mut command);
    f.send_input(tick, &command);
    f.advance_projectiles();
    intake_without_resolution(f);
}

fn flush_stalled_intake(f: &mut Fixture) {
    for _ in 0..20 {
        intake_without_resolution(f);
    }
}

fn depth(f: &Fixture) -> usize {
    f.queues
        .queue_state_for_test(CLIENT)
        .unwrap()
        .pending_ticks
        .len()
}

fn assert_accepted_chain(f: &Fixture, count: usize) {
    let receive = f.switch_receive.as_ref().unwrap();
    assert_eq!(receive.resolutions.len(), count);
    assert!(receive.resolutions.iter().all(|r| matches!(
        r,
        netcode::CurrentSwitchResolution::Accepted { .. } | netcode::CurrentSwitchResolution::None
    )));
    assert!(matches!(
        receive.resolutions.last(),
        Some(netcode::CurrentSwitchResolution::Accepted { .. })
    ));
    assert_eq!(f.sent_switches.len(), count);
    assert_eq!(
        receive
            .raw_outcomes
            .iter()
            .map(|(_, id, yes)| (*id, *yes))
            .collect::<Vec<_>>(),
        (0..count as u32).map(|id| (id, true)).collect::<Vec<_>>()
    );
}

#[test]
fn conditioned_reselects_send_nothing_and_three_retargets_survive_actual_trim() {
    let mut f = Fixture::with_switch_timing(600, 100, mandated_link());
    for tick in 90..145 {
        let command = match tick {
            90 | 92 => selecting(1),
            91 | 94 => selecting(0),
            95 | 96 | 103 => selecting(2),
            99 | 100 => selecting(3),
            _ => neutral(),
        };
        stalled_owner_step(&mut f, tick, command);
    }
    flush_stalled_intake(&mut f);
    assert_eq!(
        f.sent_switches
            .iter()
            .map(|d| d.client_tick)
            .collect::<Vec<_>>(),
        [90, 95, 99, 103],
        "reselects create no declaration"
    );
    assert!(
        depth(&f)
            > f.queues
                .queue_state_for_test(CLIENT)
                .unwrap()
                .max_pending_ticks
    );
    let previous = f.playout.last().unwrap().1;
    let first = f.playout.len();
    f.idle(145, 180);
    assert!(
        f.playout[first].1 > previous + 1,
        "the deep backlog actually trims"
    );
    assert_eq!(f.host_selection_changes().len(), 1);
    f.assert_settled_selection(2, &["switch-origin", "switch-third"]);
    let receive = f.switch_receive.as_ref().unwrap();
    assert_eq!(receive.resolutions.len(), 4);
    assert!(
        receive.resolutions[..3]
            .iter()
            .all(|r| *r == netcode::CurrentSwitchResolution::None)
    );
    assert!(matches!(
        receive.resolutions[3],
        netcode::CurrentSwitchResolution::Accepted { .. }
    ));
    assert_eq!(
        receive
            .raw_outcomes
            .iter()
            .map(|(_, id, yes)| (*id, *yes))
            .collect::<Vec<_>>(),
        [(0, false), (1, false), (2, false), (3, true)]
    );
}

#[test]
fn conditioned_two_switches_inside_a_drain_stall_keep_order_with_and_without_trim() {
    for trimmed in [false, true] {
        let mut f = Fixture::with_switch_timing(1, 0, mandated_link());
        if !trimmed {
            // Empty the pre-existing network pipeline before this bounded stall.
            // Every pending command still passes the production resolver.
            flush_stalled_intake(&mut f);
            for _ in 0..30 {
                if depth(&f) == 0 {
                    break;
                }
                f.host_tick();
            }
            assert_eq!(depth(&f), 0);
            assert_eq!(
                f.queues
                    .queue_state_for_test(CLIENT)
                    .unwrap()
                    .resolved_cursor,
                Some(89)
            );
        }
        let resume = if trimmed { 125 } else { 97 };
        for tick in 90..resume {
            stalled_owner_step(
                &mut f,
                tick,
                match tick {
                    90 => selecting(1),
                    95 => selecting(2),
                    _ => neutral(),
                },
            );
        }
        flush_stalled_intake(&mut f);
        assert_eq!(
            depth(&f)
                > f.queues
                    .queue_state_for_test(CLIENT)
                    .unwrap()
                    .max_pending_ticks,
            trimmed
        );
        let first = f.playout.len();
        let previous = f.playout.last().unwrap().1;
        f.idle(resume, 180);
        assert_eq!(f.playout[first].1 > previous + 1, trimmed);
        assert_accepted_chain(&f, 2);
        if trimmed {
            assert_eq!(f.host_selection_changes().last().unwrap().1, 2);
            let observed: Vec<_> = f
                .switch_receive
                .as_ref()
                .unwrap()
                .observer
                .selections
                .iter()
                .map(|(_, name)| name.as_str())
                .collect();
            assert!(
                observed == ["switch-origin", "switch-third"]
                    || observed == ["switch-origin", "switch-target", "switch-third"]
            );
            f.assert_settled_selection(2, &observed);
        } else {
            assert_eq!(f.host_selection_changes(), [(90, 1), (95, 2)]);
            f.assert_settled_selection(2, &["switch-origin", "switch-target", "switch-third"]);
        }
    }
}

#[test]
fn conditioned_late_retarget_begins_a_fresh_lower_after_the_first_repoint() {
    for extra_delay in [0, 20] {
        let mut f = Fixture::with_switch_timing(600, 100, mandated_link());
        f.step(90, selecting(1));
        f.idle(91, 4);
        f.switch_receive.as_mut().unwrap().hold_declarations = true;
        f.step(95, selecting(2));
        let mut next = 96;
        while f.host_selection_changes().is_empty() && next < 200 {
            f.step(next, neutral());
            next += 1;
        }
        assert_eq!(f.host_selection_changes(), [(125, 1)]);
        f.idle(next, extra_delay);
        next += extra_delay;
        f.release_switch_declarations();
        f.idle(next, 160);
        assert_accepted_chain(&f, 2);
        let changes = f.host_selection_changes();
        assert_eq!(changes[0], (125, 1));
        assert_eq!(changes.len(), 2);
        assert!(changes[1].0 >= 130, "fresh lower is stamped 95 + 36 - 1");
        f.assert_settled_selection(2, &["switch-origin", "switch-target", "switch-third"]);
    }
}

#[test]
fn conditioned_long_lower_trim_repoints_before_the_first_post_raise_reload() {
    let mut f = Fixture::with_switch_timing(600, 100, mandated_link());
    let target = f
        .host
        .borrow()
        .get_component::<Inventory>(f.host_actors.pawn)
        .unwrap()
        .wieldables[1]
        .unwrap();
    if let Ok(ComponentValue::Weapon(w)) = f
        .host
        .borrow_mut()
        .get_component_value_mut(target, ComponentKind::Weapon)
    {
        w.magazine -= 1;
    }
    for tick in 90..150 {
        let mut command = if tick == 90 { selecting(1) } else { neutral() };
        command.reload = tick == 132;
        stalled_owner_step(&mut f, tick, command);
    }
    flush_stalled_intake(&mut f);
    assert!(
        depth(&f)
            > f.queues
                .queue_state_for_test(CLIENT)
                .unwrap()
                .max_pending_ticks
    );
    let first = f.playout.len();
    let previous = f.playout.last().unwrap().1;
    f.idle(150, 160);
    assert!(f.playout[first].1 > previous + 1);
    let reloads: Vec<_> = f
        .reloads
        .iter()
        .filter(|(_, weapon, result)| *weapon == target && *result == sim::ReloadOutcome::Started)
        .collect();
    assert_eq!(reloads.len(), 1);
    let resolved = f
        .resolved_commands
        .iter()
        .find(|(tick, _)| *tick == reloads[0].0)
        .unwrap();
    assert_eq!(resolved.1.reload_press_tick, Some(132));
    let repoint_host_tick = f
        .host_active
        .windows(2)
        .find(|p| p[0].1 == 0 && p[1].1 == 1)
        .unwrap()[1]
        .0;
    assert!(repoint_host_tick <= reloads[0].0);
    assert_accepted_chain(&f, 1);
    f.assert_settled_selection(1, &["switch-origin", "switch-target"]);
}

#[test]
fn conditioned_zero_lower_repoints_once_in_the_trimmed_declaration_resolution() {
    let mut f = Fixture::with_switch_timing(0, 100, mandated_link());
    for tick in 90..125 {
        stalled_owner_step(
            &mut f,
            tick,
            if tick == 90 { selecting(1) } else { neutral() },
        );
    }
    flush_stalled_intake(&mut f);
    assert!(
        depth(&f)
            > f.queues
                .queue_state_for_test(CLIENT)
                .unwrap()
                .max_pending_ticks
    );
    let first = f.playout.len();
    let previous = f.playout.last().unwrap().1;
    f.step(125, neutral());
    let resolutions = f.resolved_commands.len();
    for _ in 0..30 {
        f.transport();
    }
    assert_eq!(f.resolved_commands.len(), resolutions);
    assert_eq!(
        f.switch_receive
            .as_ref()
            .unwrap()
            .raw_outcomes
            .iter()
            .map(|(_, id, accepted)| (*id, *accepted))
            .collect::<Vec<_>>(),
        vec![(0, true)],
        "the first resolution queues its reply without another resolution"
    );
    f.idle(126, 159);
    assert!(f.playout[first].1 > previous + 1);
    assert_eq!(f.host_selection_changes().len(), 1);
    assert_accepted_chain(&f, 1);
    f.assert_settled_selection(1, &["switch-origin", "switch-target"]);
}

fn parked_silent_owner() -> Fixture {
    let mut f = Fixture::with_switch_timing(600, 100, mandated_link());
    f.step(90, selecting(1));
    f.idle(91, 6);
    // Drain only what already left the owner. No new Input is produced.
    for _ in 0..25 {
        f.host_tick();
    }
    let queue = f.queues.queue_state_for_test(CLIENT).unwrap();
    assert_eq!(queue.lower_repoint_tick, Some(125));
    assert!(queue.newest_tick.unwrap() < 125);
    assert!(queue.pending_ticks.is_empty());
    f
}

#[test]
fn conditioned_silent_owner_keeps_expired_host_lower_until_input_resumes() {
    let mut f = parked_silent_owner();
    let frozen_newest = f.queues.queue_state_for_test(CLIENT).unwrap().newest_tick;
    for _ in 0..120 {
        f.host_tick();
        let registry = f.host.borrow();
        let inventory = registry
            .get_component::<Inventory>(f.host_actors.pawn)
            .unwrap();
        assert_eq!(
            (inventory.active_slot, inventory.switch_target),
            (0, Some(1))
        );
        let weapon = registry
            .get_component::<WeaponComponent>(f.host_actors.weapon)
            .unwrap();
        assert_eq!(
            weapon.state,
            postretro_entities::components::wieldable_state::WieldableState::Lowering
        );
        assert!(!weapon.state.allows_fire());
        assert!(!weapon.state.allows_reload());
    }
    assert_eq!(
        f.host
            .borrow()
            .get_component::<WeaponComponent>(f.host_actors.weapon)
            .unwrap()
            .state_remaining_ms,
        0
    );
    assert_eq!(
        f.queues.queue_state_for_test(CLIENT).unwrap().newest_tick,
        frozen_newest
    );
    assert!(f.switch_receive.as_ref().unwrap().raw_outcomes.is_empty());
    assert!(f.host_selection_changes().is_empty());
    f.step(97, stamped_start(97));
    let mut reload = neutral();
    reload.reload = true;
    f.step(98, reload);
    for _ in 0..30 {
        f.host_tick();
    }
    assert!(
        f.queues
            .queue_state_for_test(CLIENT)
            .unwrap()
            .newest_tick
            .unwrap()
            < 125
    );
    assert!(f.reloads.is_empty());
    assert!(f.switch_receive.as_ref().unwrap().raw_outcomes.is_empty());
    assert!(f.host_selection_changes().is_empty());
    f.idle(99, 240);
    assert_accepted_chain(&f, 1);
    f.assert_settled_selection(1, &["switch-origin", "switch-target"]);
}

#[test]
fn conditioned_wait_and_malformed_release_parked_reply_before_later_declaration() {
    for malformed in [false, true] {
        let mut f = parked_silent_owner();
        f.weapon_only_input = true;
        let initial = f.queues.queue_state_for_test(CLIENT).unwrap();
        let stamp = if malformed {
            initial.newest_tick.unwrap() + initial.lead_ticks + 1
        } else {
            130
        };
        f.switch_receive.as_mut().unwrap().hold_declarations = true;
        for tick in 97..=stamp {
            let mut command = if tick == stamp {
                selecting(2)
            } else {
                neutral()
            };
            f.predict(tick, &mut command);
        }
        assert_eq!(f.unsent_switches.len(), 1);
        // Deliberately adversarial independent declaration latency; Input stays
        // absent while all transport/control and production intake keep running.
        f.release_switch_declarations();
        let mut intake_tick = None;
        let mut forced_tick = None;
        for _ in 0..initial.wait_ticks + 60 {
            f.host_tick();
            let queue = f.queues.queue_state_for_test(CLIENT).unwrap();
            assert_eq!(queue.newest_tick, initial.newest_tick);
            assert!(queue.pending_ticks.is_empty());
            if intake_tick.is_none() && queue.retained_switches == 1 {
                intake_tick = Some(queue.host_tick);
            }
            if f.host
                .borrow()
                .get_component::<Inventory>(f.host_actors.pawn)
                .unwrap()
                .active_slot
                == 1
            {
                forced_tick.get_or_insert(queue.host_tick);
            } else {
                assert!(forced_tick.is_none(), "no second repoint without its Input");
            }
            if forced_tick.is_some()
                && f.switch_receive.as_ref().unwrap().raw_outcomes.len()
                    >= if malformed { 2 } else { 1 }
            {
                break;
            }
        }
        assert!(forced_tick.is_some());
        if !malformed {
            // retained_at precedes the first observable retained sample by one
            // resolution; this pins the actual production wait bound.
            assert_eq!(
                forced_tick.unwrap() - intake_tick.unwrap(),
                initial.wait_ticks - 1
            );
        }
        assert_eq!(
            f.switch_receive
                .as_ref()
                .unwrap()
                .raw_outcomes
                .first()
                .map(|(_, id, accepted)| (*id, *accepted)),
            Some((0, true))
        );
        // Give the observer enough conditioned snapshots to see the forced first
        // target before resuming Input, even when the second declaration is refused.
        for _ in 0..30 {
            f.host_tick();
        }
        let observed: Vec<_> = f
            .switch_receive
            .as_ref()
            .unwrap()
            .observer
            .selections
            .iter()
            .map(|(_, name)| name.as_str())
            .collect();
        assert_eq!(observed, ["switch-origin", "switch-target"]);
        f.idle(stamp + 1, 320);
        let receive = f.switch_receive.as_ref().unwrap();
        assert_eq!(
            receive
                .raw_outcomes
                .iter()
                .map(|(_, id, accepted)| (*id, *accepted))
                .collect::<Vec<_>>(),
            [(0, true), (1, !malformed)]
        );
        assert_eq!(receive.resolutions.len(), 2);
        if malformed {
            f.assert_settled_selection(1, &["switch-origin", "switch-target"]);
        } else {
            f.assert_settled_selection(2, &["switch-origin", "switch-target", "switch-third"]);
        }
    }
}

#[test]
fn conditioned_retained_overflow_repoints_parked_lower_before_the_oldest_release() {
    let mut f = parked_silent_owner();
    f.weapon_only_input = true;
    let initial = f.queues.queue_state_for_test(CLIENT).unwrap();
    let bound = initial.max_retained_switches;
    assert_eq!(
        bound, 64,
        "the authored source chain below spans two real lowers"
    );
    f.switch_receive.as_mut().unwrap().hold_declarations = true;
    // Both lower clocks are the owner's actual weapon machine. Its first lower
    // ends at125; the next begins130, re-targets through165, then begins166.
    for tick in 97..=194 {
        let mut command = if tick >= 130 {
            selecting(if tick % 2 == 0 {
                2
            } else if tick <= 165 {
                3
            } else {
                0
            })
        } else {
            neutral()
        };
        f.predict(tick, &mut command);
    }
    assert_eq!(f.unsent_switches.len(), bound + 1);
    let overflow = f.unsent_switches.remove(&194).unwrap();
    f.release_switch_declarations();
    flush_stalled_intake(&mut f);
    let full = f.queues.queue_state_for_test(CLIENT).unwrap();
    assert_eq!(full.retained_switches, bound);
    assert_eq!(full.lower_repoint_tick, Some(125));
    assert_eq!(full.newest_tick, initial.newest_tick);
    assert!(full.pending_ticks.is_empty());
    assert_eq!(
        f.host
            .borrow()
            .get_component::<Inventory>(f.host_actors.pawn)
            .unwrap()
            .active_slot,
        0
    );
    assert!(f.switch_receive.as_ref().unwrap().raw_outcomes.is_empty());
    f.send_switch_declaration(overflow);
    flush_stalled_intake(&mut f);
    let overflowed = f.queues.queue_state_for_test(CLIENT).unwrap();
    assert_eq!(
        overflowed.retained_switches, bound,
        "65th declaration releases the oldest at the retained bound"
    );
    assert_eq!(overflowed.lower_repoint_tick, Some(125));
    assert_eq!(overflowed.newest_tick, initial.newest_tick);
    f.host_tick();
    let forced = f.queues.queue_state_for_test(CLIENT).unwrap();
    assert_eq!(
        forced.lower_repoint_tick,
        Some(165),
        "oldest forced declaration begins a fresh lower after parked repoint"
    );
    assert_eq!(
        f.host
            .borrow()
            .get_component::<Inventory>(f.host_actors.pawn)
            .unwrap()
            .active_slot,
        1
    );
    assert!(
        forced.host_tick - initial.host_tick < initial.wait_ticks,
        "overflow, not wait, forced the repoint"
    );
    for _ in 0..30 {
        f.host_tick();
    }
    let receive = f.switch_receive.as_ref().unwrap();
    assert_eq!(
        receive
            .raw_outcomes
            .iter()
            .map(|(_, id, yes)| (*id, *yes))
            .collect::<Vec<_>>(),
        [(0, true)]
    );
    assert_eq!(
        receive
            .observer
            .selections
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>(),
        ["switch-origin", "switch-target"]
    );
    f.idle(195, 350);
    let receive = f.switch_receive.as_ref().unwrap();
    let expected: Vec<_> = (0..=65).map(|id| (id, matches!(id, 0 | 36 | 65))).collect();
    assert_eq!(
        receive
            .raw_outcomes
            .iter()
            .map(|(_, id, yes)| (*id, *yes))
            .collect::<Vec<_>>(),
        expected,
        "every superseded lower and each repoint settles front-first exactly once"
    );
    assert_eq!(receive.resolutions.len(), 66);
    let observed: Vec<_> = receive
        .observer
        .selections
        .iter()
        .map(|(_, name)| name.as_str())
        .collect();
    assert!(
        observed == ["switch-origin", "switch-target", "switch-third"]
            || observed
                == [
                    "switch-origin",
                    "switch-target",
                    "switch-fourth",
                    "switch-third"
                ]
    );
    f.assert_settled_selection(2, &observed);
    let final_queue = f.queues.queue_state_for_test(CLIENT).unwrap();
    assert_eq!(final_queue.retained_switches, 0);
    assert_eq!(final_queue.lower_repoint_tick, None);
}
