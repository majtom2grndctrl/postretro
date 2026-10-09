// Use and drop rising edges retained beside movement playout.
// See: context/lib/networking.md §Host input command queue
//
// Both arrive as one-tick edges on the wire, so a catch-up trim or stale-drop of
// the carrying command would erase the press. Intake records each edge from the
// reliable-ordered stream first; an advancing resolution delivers each one once,
// in order, after its tick resolves. Unlike reload these are edges, not levels,
// so no low tick is needed before a recovered press.

use std::collections::VecDeque;

use postretro_net::wire::InputCommand;

use crate::netcode::prediction::client_tick_le;
use crate::sim::SimCommand;

/// Per-lane bound, matching the other per-client retained-edge bounds.
const MAX_RETAINED_PRESSES: usize = 64;

#[derive(Debug, Default)]
pub(super) struct PressEdges {
    use_presses: VecDeque<u32>,
    drop_presses: VecDeque<u32>,
    /// Newest command tick observed. Only strictly newer commands contribute an
    /// edge, so duplicate or stale retransmits cannot add a press.
    latest_observed: Option<u32>,
}

impl PressEdges {
    pub(super) fn observe(&mut self, command: &InputCommand) {
        if self
            .latest_observed
            .is_some_and(|tick| client_tick_le(command.client_tick, tick))
        {
            return;
        }
        self.latest_observed = Some(command.client_tick);
        for (pressed, presses) in [
            (command.movement.use_pressed, &mut self.use_presses),
            (command.movement.drop_pressed, &mut self.drop_presses),
        ] {
            if pressed && presses.len() < MAX_RETAINED_PRESSES {
                presses.push_back(command.client_tick);
            }
        }
    }

    /// Replace the resolved command's use and drop bits with at most one due
    /// retained press each. Only advancing resolutions call this.
    pub(super) fn deliver(&mut self, resolved_tick: u32, command: &mut SimCommand) {
        let use_pressed = take_due(&mut self.use_presses, resolved_tick);
        let drop_pressed = take_due(&mut self.drop_presses, resolved_tick);
        command.use_pressed = use_pressed;
        command.movement.use_pressed = use_pressed;
        command.drop_pressed = drop_pressed;
        command.movement.drop_pressed = drop_pressed;
    }
}

fn take_due(presses: &mut VecDeque<u32>, resolved_tick: u32) -> bool {
    let due = presses
        .front()
        .is_some_and(|tick| client_tick_le(*tick, resolved_tick));
    if due {
        presses.pop_front();
    }
    due
}

#[cfg(test)]
mod tests {
    use super::super::*;

    const CLIENT: u64 = 7;

    fn command(client_tick: u32) -> InputCommand {
        let mut command = crate::netcode::wire_convert::sim_command_to_input(
            &neutral_sim_command(0.0),
            client_tick,
            0.0,
        );
        command.movement.wish_dir = [1.0, 0.0];
        command
    }

    // Regression: a use or drop press inside a catch-up trimmed range vanished.
    #[test]
    fn use_and_drop_presses_inside_a_trimmed_range_each_deliver_exactly_once() {
        let mut queues = HostCommandQueues::new();
        // Disarm the buildup latch with an ordinary stream, then let a host stall
        // pile up a backlog deep enough to trim.
        for tick in 0..2 {
            assert!(queues.ingest(CLIENT, &command(tick)));
        }
        let mut resolved = vec![queues.resolve_tick(CLIENT).unwrap()];
        for tick in 2..14 {
            let mut command = command(tick);
            command.movement.use_pressed = tick == 3 || tick == 5;
            command.movement.drop_pressed = tick == 4;
            assert!(queues.ingest(CLIENT, &command));
        }
        for _ in 0..30 {
            resolved.push(queues.resolve_tick(CLIENT).unwrap());
        }
        assert!(
            resolved
                .windows(2)
                .any(|pair| pair[1].client_tick.wrapping_sub(pair[0].client_tick) > 1),
            "the backlog must take the catch-up trim"
        );
        let uses: Vec<u32> = resolved
            .iter()
            .filter(|r| r.command.use_pressed)
            .map(|r| r.client_tick)
            .collect();
        let drops: Vec<u32> = resolved
            .iter()
            .filter(|r| r.command.drop_pressed)
            .map(|r| r.client_tick)
            .collect();
        assert_eq!(uses.len(), 2, "both use presses survive the trim: {uses:?}");
        assert_eq!(
            drops.len(),
            1,
            "the drop press survives the trim: {drops:?}"
        );
        assert!(
            resolved
                .iter()
                .all(|r| r.command.use_pressed == r.command.movement.use_pressed
                    && r.command.drop_pressed == r.command.movement.drop_pressed),
            "full command and movement mirror carry the same edge"
        );
    }

    #[test]
    fn duplicate_and_stale_press_retransmits_add_no_edge() {
        let mut queues = HostCommandQueues::new();
        for tick in 0..3 {
            let mut command = command(tick);
            command.movement.use_pressed = tick == 1;
            queues.ingest(CLIENT, &command);
        }
        let mut stale = command(1);
        stale.movement.use_pressed = true;
        stale.movement.drop_pressed = true;
        assert!(!queues.ingest(CLIENT, &stale));
        let mut presses = (0, 0);
        for _ in 0..12 {
            let resolved = queues.resolve_tick(CLIENT).unwrap();
            presses.0 += usize::from(resolved.command.use_pressed);
            presses.1 += usize::from(resolved.command.drop_pressed);
        }
        assert_eq!(presses, (1, 0));
    }

    #[test]
    fn press_waits_for_its_own_tick_to_resolve() {
        let mut queues = HostCommandQueues::new();
        for tick in 0..4 {
            let mut command = command(tick);
            command.movement.drop_pressed = tick == 3;
            queues.ingest(CLIENT, &command);
        }
        let ticks: Vec<(u32, bool)> = (0..6)
            .map(|_| {
                let resolved = queues.resolve_tick(CLIENT).unwrap();
                (resolved.client_tick, resolved.command.drop_pressed)
            })
            .collect();
        assert!(
            ticks.iter().all(|(tick, dropped)| *dropped == (*tick == 3)),
            "{ticks:?}"
        );
    }
}
