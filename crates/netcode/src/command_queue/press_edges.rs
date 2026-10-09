// Reload, use and drop presses retained beside movement playout, and their
// delivery in the client's order.
// See: context/lib/networking.md §Host input command queue
//
// Use and drop arrive as one-tick edges on the wire, so a catch-up trim or
// stale-drop of the carrying command would erase the press. Intake records each
// edge from the reliable-ordered stream first; an advancing resolution delivers
// each one once, in order, after its tick resolves and behind any older retained
// start. Unlike reload these are edges, not levels, so no low tick is needed
// before a recovered press.

use std::collections::VecDeque;

use postretro_net::wire::InputCommand;

use super::ClientCommandState;
use crate::activation_edges::MAX_RETAINED_ACTIVATION_EDGES;
use crate::prediction::client_tick_le;
use crate::sim::SimCommand;
use postretro_foundation::ActivationToken;

/// Per-lane bound, the same as the retained activation start and edge lanes.
const MAX_RETAINED_PRESSES: usize = MAX_RETAINED_ACTIVATION_EDGES;

/// A retained press: its command's client tick and the firing slot that
/// command named, the weapon the client held when it pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Press {
    pub tick: u32,
    pub slot: u8,
}

/// Which due presses an advancing resolution may deliver, by client tick. A
/// press must not reach the weapon ahead of an older start still waiting in its
/// lane, nor ahead of a shot the live execution owes from before it: a reload or
/// drop delivered first would refuse, or fire the wrong weapon for, a shot the
/// client fired before it. On the tick a start is delivered, a later press
/// would also land on that start's weapon. Any input the client stamps with a
/// tick and the host must apply in order can share this gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PressGate {
    /// Nothing older is waiting.
    Open,
    /// Presses stamped at or before this client tick.
    Through(u32),
}

impl PressGate {
    pub(super) fn admits(self, tick: u32) -> bool {
        match self {
            Self::Open => true,
            Self::Through(horizon) => client_tick_le(tick, horizon),
        }
    }

    /// Admits only what both gates admit.
    pub(super) fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Open, gate) | (gate, Self::Open) => gate,
            (Self::Through(a), Self::Through(b)) => {
                Self::Through(serial_oldest([a, b]).unwrap_or(a))
            }
        }
    }

    pub(super) fn through(horizon: Option<u32>) -> Self {
        horizon.map_or(Self::Open, Self::Through)
    }
}

/// Serially oldest of `ticks`, wrap-aware.
fn serial_oldest(ticks: impl IntoIterator<Item = u32>) -> Option<u32> {
    ticks.into_iter().reduce(|oldest, tick| {
        if client_tick_le(tick, oldest) {
            tick
        } else {
            oldest
        }
    })
}

#[derive(Debug, Default)]
pub(super) struct PressEdges {
    use_presses: VecDeque<Press>,
    drop_presses: VecDeque<Press>,
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
        let press = Press {
            tick: command.client_tick,
            slot: command.movement.firing_slot,
        };
        for (pressed, presses) in [
            (command.movement.use_pressed, &mut self.use_presses),
            (command.movement.drop_pressed, &mut self.drop_presses),
        ] {
            if pressed && presses.len() < MAX_RETAINED_PRESSES {
                presses.push_back(press);
            }
        }
    }

    /// Replace the resolved command's use and drop bits with the retained press
    /// at the front of each lane stamped at `tick`. Returns the dropped press's
    /// slot. Only advancing resolutions call this.
    fn deliver(&mut self, tick: Option<u32>, command: &mut SimCommand) -> Option<u8> {
        let use_pressed = take_at(&mut self.use_presses, tick).is_some();
        let dropped = take_at(&mut self.drop_presses, tick);
        command.use_pressed = use_pressed;
        command.movement.use_pressed = use_pressed;
        command.drop_pressed = dropped.is_some();
        command.movement.drop_pressed = dropped.is_some();
        dropped.map(|press| press.slot)
    }

    fn fronts(&self) -> impl Iterator<Item = u32> + '_ {
        [self.use_presses.front(), self.drop_presses.front()]
            .into_iter()
            .flatten()
            .map(|press| press.tick)
    }
}

fn take_at(presses: &mut VecDeque<Press>, tick: Option<u32>) -> Option<Press> {
    if presses.front().map(|press| press.tick) == tick && tick.is_some() {
        presses.pop_front()
    } else {
        None
    }
}

impl ClientCommandState {
    /// Oldest reload, use, or drop press whose tick has resolved.
    pub(super) fn oldest_due_press(&self, resolved_tick: u32) -> Option<u32> {
        serial_oldest(
            self.pending_reload_presses
                .front()
                .map(|press| press.tick)
                .into_iter()
                .chain(self.press_edges.fronts())
                .filter(|tick| client_tick_le(*tick, resolved_tick)),
        )
    }

    /// Deliver the presses of one client tick: the oldest due press tick, once
    /// `gate` admits it. Presses of different client ticks never share a
    /// resolution, so the sim's fixed stage order (drop before use, reload
    /// after) cannot reorder them. A delivered reload or drop names the slot it
    /// was pressed in, and this resolution fires from that slot so the press
    /// reaches the weapon the client held.
    ///
    /// Bound: each advancing resolution delivers every lane's press at the
    /// oldest admitted tick (a reload may first need one low tick), so a press
    /// waits at most one resolution per older press tick, plus whatever holds
    /// `gate`.
    pub(super) fn deliver_presses(
        &mut self,
        resolved_tick: u32,
        gate: PressGate,
        command: &mut SimCommand,
    ) {
        let tick = self
            .oldest_due_press(resolved_tick)
            .filter(|tick| gate.admits(*tick));
        let reloaded = self.deliver_reload_press(resolved_tick, tick, command);
        let dropped = self.press_edges.deliver(tick, command);
        if let Some(slot) = reloaded.or(dropped) {
            command.firing_slot = slot;
        }
    }

    /// Reload is a level on the wire. A due press that is not this tick's, or
    /// that follows a high level the weapon already saw, emits a low tick and
    /// stays queued: the low tick clears `WeaponComponent::reload_press_consumed`,
    /// and a press held back must not be stood in for by the raw level.
    fn deliver_reload_press(
        &mut self,
        resolved_tick: u32,
        tick: Option<u32>,
        command: &mut SimCommand,
    ) -> Option<u8> {
        let mut delivered = None;
        if let Some(press) = self
            .pending_reload_presses
            .front()
            .copied()
            .filter(|press| client_tick_le(press.tick, resolved_tick))
        {
            if self.last_emitted_reload || tick != Some(press.tick) {
                command.reload = false;
            } else {
                command.reload = true;
                self.pending_reload_presses.pop_front();
                delivered = Some(press.slot);
            }
        }
        self.last_emitted_reload = command.reload;
        delivered
    }

    /// A non-advancing hold replays its held command's reload level. While the
    /// oldest due reload press is held back, by an older press, an older start,
    /// or the live execution, the replayed level must not deliver it early.
    pub(super) fn hold_gated_reload(
        &self,
        live: Option<ActivationToken>,
        command: &mut SimCommand,
    ) {
        let Some(cursor) = self.resolved_cursor else {
            return;
        };
        let held_back = self
            .pending_reload_presses
            .front()
            .filter(|press| client_tick_le(press.tick, cursor))
            .is_some_and(|press| {
                self.oldest_due_press(cursor) != Some(press.tick)
                    || !self.order_gate(live, None).admits(press.tick)
            });
        if held_back {
            command.reload = false;
        }
    }
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

    fn start(command: &mut InputCommand) {
        command.activation.initiation = Some(postretro_net::wire::WireActivationToken {
            start_tick: command.client_tick,
            lane: 0,
        });
    }

    /// Resolve after a stall: ingest `backlog` at once, then one new command
    /// per resolution.
    fn resolve_stall(
        queues: &mut HostCommandQueues,
        backlog: impl IntoIterator<Item = InputCommand>,
        resolutions: u32,
    ) -> Vec<ResolvedCommand> {
        for tick in 0..2 {
            assert!(queues.ingest(CLIENT, &command(tick)));
        }
        queues.resolve_tick(CLIENT).unwrap();
        let mut next = 2;
        for command in backlog {
            next = command.client_tick + 1;
            assert!(queues.ingest(CLIENT, &command));
        }
        (0..resolutions)
            .map(|offset| {
                queues.ingest(CLIENT, &command(next + offset));
                queues.resolve_tick(CLIENT).unwrap()
            })
            .collect()
    }

    fn only_once(resolved: &[ResolvedCommand], found: impl Fn(&ResolvedCommand) -> bool) -> usize {
        let indices: Vec<usize> = resolved
            .iter()
            .enumerate()
            .filter(|&(_, resolved)| found(resolved))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(indices.len(), 1, "delivered exactly once: {indices:?}");
        indices[0]
    }

    // Regression: a stall ending in a reload delivered the reload by command tick
    // while the earlier starts waited in their lane, so the host refused them.
    #[test]
    fn reload_and_drop_stamped_after_retained_starts_wait_until_those_starts_are_delivered() {
        let mut queues = HostCommandQueues::new();
        let backlog = (2..20).map(|tick| {
            let mut command = command(tick);
            if tick == 3 || tick == 11 {
                start(&mut command);
            }
            command.reload = (15..17).contains(&tick);
            command.movement.drop_pressed = tick == 16;
            command
        });
        let resolved = resolve_stall(&mut queues, backlog, 6);
        assert!(
            resolved[0].client_tick > 2,
            "the backlog must take the catch-up trim"
        );
        let starts = [3, 11].map(|tick| {
            only_once(&resolved, |resolved| {
                resolved
                    .command
                    .activation
                    .initiation
                    .is_some_and(|token| token.start_tick == tick)
            })
        });
        let reload = only_once(&resolved, |resolved| resolved.command.reload);
        let dropped = only_once(&resolved, |resolved| resolved.command.drop_pressed);
        assert!(starts[0] < starts[1], "starts leave in order");
        assert!(
            starts[1] < reload && starts[1] < dropped,
            "starts {starts:?}, reload {reload}, drop {dropped}"
        );
    }

    // Regression: a late start for slot 0, delivered by a command sent after the
    // client switched to slot 1, set that whole tick's firing slot, so slot 1's
    // reload press on the same command reloaded slot 0's weapon instead.
    #[test]
    fn a_late_start_for_another_slot_never_carries_a_later_reload_press() {
        let mut queues = HostCommandQueues::new();
        for tick in 0..2 {
            assert!(queues.ingest(CLIENT, &command(tick)));
        }
        queues.resolve_tick(CLIENT).unwrap();
        let switched = |tick: u32| {
            let mut command = command(tick);
            command.movement.firing_slot = u8::from(tick > 3);
            if tick == 3 {
                start(&mut command);
            }
            command.reload = tick == 19;
            command
        };
        for tick in 2..21 {
            assert!(queues.ingest(CLIENT, &switched(tick)));
        }
        let resolved: Vec<ResolvedCommand> = (21..24)
            .map(|tick| {
                queues.ingest(CLIENT, &switched(tick));
                queues.resolve_tick(CLIENT).unwrap()
            })
            .collect();
        let fired = &resolved[0];
        assert_eq!(
            fired
                .command
                .activation
                .initiation
                .map(|token| token.start_tick),
            Some(3)
        );
        assert_eq!(fired.command.firing_slot, 0, "the start fires its own slot");
        assert!(!fired.command.reload, "the later press waits");
        let reload = only_once(&resolved, |resolved| resolved.command.reload);
        assert_eq!(resolved[reload].command.firing_slot, 1);
        assert!(resolved[reload].command.activation.initiation.is_none());
    }

    // Regression: a drop stamped before a start, both inside a stall, reached the
    // host on the start's tick, so the dropped weapon fired.
    #[test]
    fn drop_stamped_before_a_retained_start_reaches_the_host_on_an_earlier_tick() {
        let mut queues = HostCommandQueues::new();
        let backlog = (2..20).map(|tick| {
            let mut command = command(tick);
            command.movement.drop_pressed = tick == 5;
            if tick == 7 {
                start(&mut command);
            }
            command
        });
        let resolved = resolve_stall(&mut queues, backlog, 4);
        let dropped = resolved
            .iter()
            .position(|resolved| resolved.command.drop_pressed)
            .expect("the drop survives the stall");
        let fired = resolved
            .iter()
            .position(|resolved| resolved.command.activation.initiation.is_some())
            .expect("the start survives the stall");
        assert!(dropped < fired, "drop on {dropped}, start on {fired}");
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

    // Regression: presses from different client ticks delivered on one
    // resolution applied in the sim's fixed stage order, so a drop pressed after
    // a use reached the host first.
    #[test]
    fn presses_of_different_client_ticks_never_share_a_resolution() {
        let mut queues = HostCommandQueues::new();
        let backlog = (2..20).map(|tick| {
            let mut command = command(tick);
            command.movement.use_pressed = tick == 3;
            command.movement.drop_pressed = tick == 4;
            command.reload = tick == 4;
            command
        });
        let resolved = resolve_stall(&mut queues, backlog, 6);
        assert!(
            resolved[0].client_tick > 4,
            "the backlog must take the catch-up trim"
        );
        let used = only_once(&resolved, |resolved| resolved.command.use_pressed);
        let dropped = only_once(&resolved, |resolved| resolved.command.drop_pressed);
        let reloaded = only_once(&resolved, |resolved| resolved.command.reload);
        assert!(used < dropped, "use on {used}, drop on {dropped}");
        assert_eq!(
            dropped, reloaded,
            "presses of one client tick ride together"
        );
    }

    // Regression: a retained reload or drop press reached whatever weapon the
    // delivering command named, so a reload pressed on slot 1 before a switch to
    // slot 0 reloaded slot 0's weapon.
    #[test]
    fn reload_and_drop_reach_the_slot_they_were_pressed_in_after_a_switch() {
        let mut queues = HostCommandQueues::new();
        let backlog = (2..20).map(|tick| {
            let mut command = command(tick);
            command.movement.firing_slot = u8::from(tick <= 4);
            command.reload = tick == 3;
            command.movement.drop_pressed = tick == 4;
            if tick == 7 {
                start(&mut command);
            }
            command
        });
        let resolved = resolve_stall(&mut queues, backlog, 6);
        assert!(
            resolved[0].client_tick > 7,
            "the backlog must take the catch-up trim"
        );
        let reloaded = only_once(&resolved, |resolved| resolved.command.reload);
        let dropped = only_once(&resolved, |resolved| resolved.command.drop_pressed);
        let fired = only_once(&resolved, |resolved| {
            resolved.command.activation.initiation.is_some()
        });
        assert_eq!(resolved[reloaded].command.firing_slot, 1);
        assert_eq!(resolved[dropped].command.firing_slot, 1);
        assert_eq!(resolved[fired].command.firing_slot, 0, "slot 0 fires");
        assert!(reloaded < dropped && dropped < fired);
    }

    // Regression: a frontier freeze replayed the held command's high reload
    // level while its press waited behind an older retained start, so the
    // weapon saw a rising edge and reloaded ahead of that start.
    #[test]
    fn frontier_freeze_never_replays_a_reload_press_held_behind_an_older_start() {
        let mut queues = HostCommandQueues::new();
        let mut reload = command(10);
        reload.reload = true;
        let mut state = ClientCommandState {
            resolved_cursor: Some(10),
            last_resolved: Some(reload),
            last_emitted_reload: false,
            pending_reload_presses: VecDeque::from([Press { tick: 6, slot: 0 }]),
            latest_observed_reload: Some((10, true)),
            ..Default::default()
        };
        state.activation_edges.observe_start(
            postretro_foundation::ActivationToken {
                start_tick: 5,
                lane: postretro_foundation::ActivationLane::Primary,
            },
            0,
            crate::sim::RemoteStartAim {
                pitch: 0.0,
                yaw: 0.0,
            },
        );
        queues.clients.insert(CLIENT, state);
        let frozen = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(frozen.source, ResolutionSource::Held);
        assert_eq!(frozen.client_tick, 11, "a non-advancing freeze");
        assert!(
            !frozen.command.reload,
            "the held-back press is not replayed"
        );
    }

    fn burst() -> postretro_foundation::ActivationProgram {
        use postretro_foundation::ActivationStep::{Shot, Wait};
        postretro_foundation::ActivationProgram::new(
            vec![Shot, Wait { ticks: 2 }, Shot, Wait { ticks: 2 }, Shot],
            None,
            8,
        )
        .unwrap()
    }

    /// A start at tick 4 stalled into the catch-up trim, then a press stamped
    /// `press_after` ticks later. Admits the start as the host machine would
    /// (`charges` for a charged action) and resolves `resolutions` more ticks,
    /// ending the execution after `live_for` of them. The first resolution
    /// delivers the start.
    fn late_start_then_reload(
        press_after: u32,
        charges: bool,
        live_for: usize,
        resolutions: u32,
    ) -> Vec<ResolvedCommand> {
        let mut queues = HostCommandQueues::new();
        let pressed = 4 + press_after;
        let backlog = (2..pressed + 12).map(|tick| {
            let mut command = command(tick);
            if tick == 4 {
                start(&mut command);
            }
            command.reload = tick == pressed;
            command
        });
        let mut resolved = resolve_stall(&mut queues, backlog, 1);
        let start = resolved[0]
            .command
            .activation
            .initiation
            .expect("the late start is delivered first");
        assert!(resolved[0].client_tick > pressed, "the press is due");
        let id = postretro_foundation::ActivationId {
            pawn: 1,
            token: start,
        };
        let weapon = postretro_entities::EntityId::from_raw(9);
        assert!(queues.activations.accept(CLIENT, id, weapon, &burst(), 0));
        queues.activation_admitted(CLIENT, start, charges);
        let next = pressed + 12;
        for offset in 0..resolutions {
            if offset as usize == live_for {
                queues.activations.terminal(CLIENT, id, 0);
            }
            queues.ingest(CLIENT, &command(next + offset));
            resolved.push(queues.resolve_tick(CLIENT).unwrap());
        }
        resolved
    }

    // Regression: after a late start's delivery, a reload the client pressed
    // mid-burst reached the host on the next tick and cut short the burst the
    // client had fired up to that press.
    #[test]
    fn press_stamped_mid_burst_waits_until_the_late_bursts_clock_reaches_it() {
        let resolved = late_start_then_reload(3, false, usize::MAX, 8);
        let reload = only_once(&resolved, |resolved| resolved.command.reload);
        assert_eq!(
            reload, 3,
            "delivered when the burst's clock reaches the press"
        );
    }

    /// The wait on a live execution is bounded: it ends when the execution ends,
    /// however far its clock trails the press.
    #[test]
    fn press_waiting_on_a_live_execution_goes_once_it_ends() {
        let resolved = late_start_then_reload(40, false, 3, 8);
        let reload = only_once(&resolved, |resolved| resolved.command.reload);
        assert_eq!(reload, 4, "held while live, delivered on the next tick");
    }

    /// A charge's shots begin at its release, so a press during the charge
    /// waits on nothing.
    #[test]
    fn press_during_an_unreleased_charge_is_not_held() {
        let resolved = late_start_then_reload(3, true, usize::MAX, 8);
        let reload = only_once(&resolved, |resolved| resolved.command.reload);
        assert_eq!(reload, 1);
    }
}
