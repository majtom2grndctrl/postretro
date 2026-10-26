// Synchronous equip callbacks are ordered before the command lanes drain.
use super::*;
use crate::host_switch_timing::{HostSwitchTiming, PendingLower, SwitchAction, SwitchFrontier};
use postretro_net::wire::{ClientSwitchDeclaration, SwitchDeclarationKind};

fn declaration(id: u32, tick: u32) -> ClientSwitchDeclaration {
    ClientSwitchDeclaration {
        declaration_id: id,
        slot: id as u8,
        client_tick: tick,
        kind: SwitchDeclarationKind::Switch,
    }
}

fn command(tick: u32) -> InputCommand {
    crate::netcode::wire_convert::sim_command_to_input(&neutral_sim_command(0.0), tick)
}

fn apply<'a>(
    events: &'a mut Vec<(u32, bool)>,
    lower_ticks: u32,
) -> impl FnMut(&mut HostSwitchTiming, SwitchAction, SwitchFrontier) + 'a {
    move |timing, action, _| match action {
        SwitchAction::Repoint => {
            let lower = timing.lower.take().expect("a repoint has a lower");
            events.push((lower.declaration.declaration_id, true));
        }
        SwitchAction::Declaration(delivery) => {
            let (declaration, refused) = match delivery {
                SwitchDelivery::Apply(declaration) => (declaration, false),
                SwitchDelivery::Refuse(declaration) => (declaration, true),
            };
            let previous = timing.lower.take();
            let retarget = previous
                .filter(|lower| client_tick_le(declaration.client_tick, lower.repoint_tick));
            if let Some(previous) = previous {
                events.push((previous.declaration.declaration_id, retarget.is_none()));
            }
            if refused {
                events.push((declaration.declaration_id, false));
            } else {
                timing.lower = Some(PendingLower {
                    declaration,
                    origin_slot: 0,
                    outgoing: EntityId::from_raw(1),
                    target: EntityId::from_raw(2),
                    repoint_tick: retarget
                        .map_or(declaration.client_tick.wrapping_add(lower_ticks), |lower| {
                            lower.repoint_tick
                        }),
                });
            }
        }
    }
}

#[test]
fn callback_zero_lower_repoints_in_its_begin_resolution_without_a_stream() {
    let mut queues = HostCommandQueues::new();
    queues.retain_switch(7, declaration(1, 50));
    let mut events = Vec::new();
    assert!(
        queues
            .resolve_tick_with_switches(7, Some(&mut apply(&mut events, 0)))
            .is_none()
    );
    assert_eq!(events, [(1, true)]);
    assert!(queues.switch_timing(7).unwrap().lower.is_none());
}

#[test]
fn same_repoint_tick_retarget_supersedes_before_one_repoint() {
    let mut state = ClientCommandState {
        resolved_cursor: Some(10),
        latest_observed_reload: Some((10, false)),
        ..Default::default()
    };
    state.retain_switch(declaration(1, 6));
    state.retain_switch(declaration(2, 10));
    let mut events = Vec::new();
    state.deliver_switches(None, Some(&mut apply(&mut events, 4)));
    assert_eq!(events, [(1, false), (2, true)]);
    assert!(state.switches.timing.lower.is_none());
}

#[test]
fn trimmed_commands_keep_later_switch_behind_the_earlier_repoint() {
    let mut queues = HostCommandQueues::new();
    queues.retain_switch(7, declaration(1, 2));
    queues.retain_switch(7, declaration(2, 8));
    for tick in 0..20 {
        assert!(queues.ingest(7, &command(tick)));
    }
    let mut events = Vec::new();
    queues.resolve_tick_with_switches(7, Some(&mut apply(&mut events, 4)));
    assert_eq!(events, [(1, true)]);
    queues.resolve_tick_with_switches(7, Some(&mut apply(&mut events, 4)));
    assert_eq!(events, [(1, true), (2, true)]);
}

#[test]
fn silent_lower_survives_host_wait_bound_and_gates_its_repoint_tick() {
    let mut state = ClientCommandState {
        resolved_cursor: Some(6),
        latest_observed_reload: Some((6, false)),
        ..Default::default()
    };
    state.retain_switch(declaration(1, 6));
    let mut events = Vec::new();
    state.deliver_switches(None, Some(&mut apply(&mut events, 4)));
    for _ in 0..=switch_lane::SWITCH_WAIT_TICKS {
        state.host_tick += 1;
        state.deliver_switches(None, Some(&mut apply(&mut events, 4)));
    }
    assert!(events.is_empty());
    assert!(state.switch_gate().admits(9));
    assert!(!state.switch_gate().admits(10));
    assert!(state.switches.timing.lower.is_some());
}

#[test]
fn forced_later_declaration_settles_parked_predecessor_first() {
    let mut state = ClientCommandState {
        resolved_cursor: Some(6),
        latest_observed_reload: Some((6, false)),
        ..Default::default()
    };
    state.retain_switch(declaration(1, 6));
    let mut events = Vec::new();
    state.deliver_switches(None, Some(&mut apply(&mut events, 4)));
    state.retain_switch(declaration(2, 12));
    state.host_tick += switch_lane::SWITCH_WAIT_TICKS;
    state.deliver_switches(None, Some(&mut apply(&mut events, 4)));
    assert_eq!(events, [(1, true)]);
    assert_eq!(
        state
            .switches
            .timing
            .lower
            .unwrap()
            .declaration
            .declaration_id,
        2
    );
}

#[test]
fn reload_delivery_retains_the_press_tick_after_a_trim_and_clears_it_next_resolution() {
    let mut queues = HostCommandQueues::new();
    for tick in 0..20 {
        let mut input = command(tick);
        input.reload = tick == 3;
        assert!(queues.ingest(7, &input));
    }
    let resolved = queues.resolve_tick(7).unwrap();
    assert!(resolved.client_tick > 3);
    assert_eq!(resolved.reload_press_tick, Some(3));
    assert!(resolved.command.reload);
    assert_eq!(queues.resolve_tick(7).unwrap().reload_press_tick, None);
}

#[test]
fn malformed_later_declaration_cannot_overtake_a_parked_reply() {
    let mut state = ClientCommandState {
        resolved_cursor: Some(6),
        latest_observed_reload: Some((6, false)),
        ..Default::default()
    };
    state.retain_switch(declaration(1, 6));
    let mut events = Vec::new();
    state.deliver_switches(None, Some(&mut apply(&mut events, 4)));
    state.retain_switch(declaration(2, 6 + switch_lane::SWITCH_LEAD_TICKS + 1));
    state.deliver_switches(None, Some(&mut apply(&mut events, 4)));
    assert_eq!(events, [(1, true), (2, false)]);
    assert!(state.switches.timing.lower.is_none());
}

#[test]
fn overflow_later_declaration_cannot_overtake_a_parked_reply() {
    let mut state = ClientCommandState {
        resolved_cursor: Some(6),
        latest_observed_reload: Some((6, false)),
        ..Default::default()
    };
    state.retain_switch(declaration(1, 6));
    let mut events = Vec::new();
    state.deliver_switches(None, Some(&mut apply(&mut events, 4)));
    for id in 2..=switch_lane::MAX_RETAINED_SWITCHES as u32 + 2 {
        state.retain_switch(declaration(id, 12 + id));
    }
    state.deliver_switches(None, Some(&mut apply(&mut events, 4)));
    assert_eq!(events, [(1, true)]);
    assert_eq!(
        state
            .switches
            .timing
            .lower
            .unwrap()
            .declaration
            .declaration_id,
        2
    );
}
