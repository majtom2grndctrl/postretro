//! A cancel latched on a frame with no fixed tick rides the client's next
//! ticked command and never takes a client tick of its own, so host and
//! client run the same ticks and agree on assist progress across it (O11, I6).
//! See: context/lib/networking.md §Host input command queue · §Combat authority
use super::super::predict_reconcile_harness_test_fixtures::idle_command;
use super::*;
use crate::netcode::client_peek_next_command_tick;
use postretro_entities::EntityRegistry;
use postretro_entities::Transform;
use postretro_foundation::{ActivationLane, ActivationToken};
use postretro_sim::sim::assist::{
    AssistCandidate, AssistHelper, AssistRow, AssistRunner, AssistStepInput, HelperProgress,
    PawnLife, step,
};
use postretro_sim::sim::assist_pairs::PairValues;
use std::collections::BTreeMap;

/// The pair's required count: the hold completes on the 40th advancing tick.
const REQUIRED_TICKS: u32 = 40;
/// Client render polls per second: most polls run no fixed tick.
const RENDER_HZ: u64 = 144;
/// Fixed ticks into the hold of the cancel latched mid-hold, and of the one
/// latched after the hold completed while `assist` stays held.
const MID_HOLD_CANCEL: u32 = 12;
const LATCHED_CANCEL: u32 = 55;

/// One helper's accumulator, stepped the way a runner steps it: a fixed
/// target in range, both pawns alive and unchanged.
struct Accumulator {
    runner: AssistRunner,
    helper: AssistHelper,
    candidate: AssistCandidate,
    lives: [(postretro_entities::EntityId, PawnLife); 2],
    previous: HelperProgress,
}

impl Accumulator {
    fn new(runner: AssistRunner) -> Self {
        let mut registry = EntityRegistry::new();
        let life = PawnLife {
            life_state: 1,
            elapsed: None,
        };
        let helper = AssistHelper {
            id: registry.spawn(Transform::default()),
            position: Vec3::ZERO,
            permits_use: true,
            life,
        };
        let candidate = AssistCandidate {
            id: registry.spawn(Transform::default()),
            position: Vec3::X,
            life,
            assistable: true,
            pair: Some(PairValues {
                radius: 2.0,
                required_ticks: REQUIRED_TICKS,
            }),
        };
        Self {
            runner,
            lives: [(helper.id, life), (candidate.id, life)],
            helper,
            candidate,
            previous: HelperProgress::default(),
        }
    }

    fn step(&mut self, held: bool, real_command: bool) -> AssistRow {
        let outcome = step(&AssistStepInput {
            runner: self.runner,
            helper: self.helper,
            held,
            real_command,
            candidates: std::slice::from_ref(&self.candidate),
            previous: self.previous,
            previous_lives: &self.lives,
        });
        self.previous = HelperProgress {
            progress: outcome.progress,
            target: outcome.target,
            latched: outcome.latched,
        };
        outcome.row
    }
}

fn held_assist(cancel: Option<ActivationToken>) -> SimCommand {
    let mut command = idle_command();
    command.assist_held = true;
    command.activation.cancel = cancel;
    command
}

#[test]
fn zero_tick_cancel_rides_a_predicted_tick_and_keeps_host_assist_in_step() {
    let mut h = RelayHarness::new(LinkConfig::perfect(), LinkConfig::perfect());
    h.join();
    h.log_datagrams = true;
    let first_tick = client_peek_next_command_tick(Some(&h.endpoint)).expect("a client endpoint");
    let token = ActivationToken {
        start_tick: first_tick,
        lane: ActivationLane::Primary,
    };

    // A client rendering at 144 Hz: on each poll that runs no fixed tick the
    // frame loop sends nothing, and the cancel latched there rides the next
    // fixed tick's command.
    let mut client_ticks = 0u32;
    let mut cancel_ticks = Vec::new();
    h.run_with_client_poll_rate(90, RENDER_HZ, |_| {
        let fixed = client_ticks;
        client_ticks += 1;
        let cancel = [MID_HOLD_CANCEL, LATCHED_CANCEL].contains(&fixed);
        if cancel {
            cancel_ticks.push(first_tick.wrapping_add(fixed));
        }
        held_assist(cancel.then_some(token))
    });
    // Let the host drain every predicted tick.
    for _ in 0..8 {
        h.frame(&held_assist(None));
        client_ticks += 1;
    }
    let next_tick = client_peek_next_command_tick(Some(&h.endpoint)).expect("a client endpoint");
    assert_eq!(
        next_tick.wrapping_sub(first_tick),
        client_ticks,
        "a client tick is allocated only by a predicted fixed tick"
    );
    assert_eq!(cancel_ticks.len(), 2);

    // On the wire: every Input the client sent names a predicted tick, and the
    // cancel rides exactly the predicted commands it was latched for.
    let sent: BTreeMap<u32, bool> = h
        .up_log
        .iter()
        .filter(|datagram| datagram.channel == Some(Channel::Input))
        .filter_map(|datagram| datagram.index)
        .flat_map(|index| h.uplink_input(index))
        .filter_map(|message| match message {
            ClientMessage::Input(input) => {
                Some((input.client_tick, input.activation.cancel.is_some()))
            }
            _ => None,
        })
        .collect();
    assert!(
        sent.keys().all(|tick| *tick < next_tick),
        "no Input names a tick the client did not predict"
    );
    // The last predicted command leaves at the client's next poll.
    assert_eq!(
        sent.range(first_tick..).count() as u32,
        client_ticks - 1,
        "one Input per predicted tick, and none besides"
    );
    let cancelled: Vec<u32> = sent
        .iter()
        .filter_map(|(tick, cancel)| cancel.then_some(*tick))
        .collect();
    assert_eq!(cancelled, cancel_ticks);

    // The client's helper bar: one advancing step per predicted tick.
    let mut client = Accumulator::new(AssistRunner::Client);
    let predicted: BTreeMap<u32, (HelperProgress, AssistRow)> = (first_tick..next_tick)
        .map(|tick| {
            let row = client.step(true, true);
            (tick, (client.previous, row))
        })
        .collect();

    // The host's accumulator over what it actually resolved for the pawn.
    let mut host = Accumulator::new(AssistRunner::Host);
    let mut resolved_real = BTreeMap::new();
    let window = h.host_trace.iter().skip_while(|trace| {
        trace
            .resolved
            .as_ref()
            .is_none_or(|resolved| resolved.client_tick != first_tick)
    });
    for trace in window {
        let resolved = trace
            .resolved
            .as_ref()
            .expect("every host tick of a live stream resolves a command");
        let real = resolved.source == ResolutionSource::Real;
        let row = host.step(resolved.command.assist_held, real);
        assert_ne!(
            row,
            AssistRow::Release,
            "nothing in the stream reads as a release"
        );
        if real {
            assert!(
                (first_tick..next_tick).contains(&resolved.client_tick),
                "the host resolves only ticks the client predicted: {}",
                resolved.client_tick
            );
            assert!(
                resolved.command.assist_held,
                "every command carries the held level"
            );
            let previous = resolved_real.insert(resolved.client_tick, (host.previous, row));
            assert!(previous.is_none(), "a client tick advances the host once");
        }
    }
    // The host trails by its playout buffer; what it resolved is every
    // predicted tick up to its cursor, each once, and nothing else.
    let resolved_through = *resolved_real
        .keys()
        .last()
        .expect("the host resolved ticks");
    assert!(resolved_through > cancel_ticks[1] + 4);
    assert_eq!(
        resolved_real.keys().copied().collect::<Vec<_>>(),
        predicted
            .range(..=resolved_through)
            .map(|(tick, _)| *tick)
            .collect::<Vec<_>>(),
        "host and client run the same client ticks"
    );

    let completion = first_tick + REQUIRED_TICKS - 1;
    for (tick, (client, client_row)) in predicted.range(..=resolved_through) {
        let (host, host_row) = resolved_real[tick];
        assert_eq!(host.latched, client.latched, "latch at tick {tick}");
        assert_eq!(host_row, *client_row, "row at tick {tick}");
        if !client.latched {
            assert_eq!(host.progress, client.progress, "progress at tick {tick}");
        }
        assert_eq!(
            client.latched,
            *tick >= completion,
            "completion at tick {tick}"
        );
    }

    // The mid-hold cancel advanced the hold one increment on both sides; the
    // cancel after completion left the latch set.
    let [mid, latched] = [cancel_ticks[0], cancel_ticks[1]];
    assert_eq!(resolved_real[&mid].1, AssistRow::Advance);
    assert_eq!(
        resolved_real[&mid].0.progress - resolved_real[&(mid - 1)].0.progress,
        postretro_sim::sim::assist::increment(REQUIRED_TICKS)
    );
    assert_eq!(resolved_real[&latched].1, AssistRow::Latched);
    assert!(resolved_real[&latched].0.latched);
}
