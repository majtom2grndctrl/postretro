// Reload, use and drop edges, weapon switches and activation starts reach the host
// once each, in the client's tick order, across a lost datagram and a two-second
// outage in both directions.
// See: context/lib/networking.md §Host input command queue · §Combat authority · context/lib/testing_guide.md

use super::*;

const START: u32 = 1000;

/// An input the client stamps with its tick and the host delivers once, in
/// client-tick order. Within one tick the order is the simulation's: a switch
/// before the presses on its own command, then drop, use, reload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stamped {
    Switch(usize),
    Drop,
    Use,
    Reload,
}

/// One delivery the host made: the host tick, the client tick of the
/// resolution that carried it, and what it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Delivery {
    host_tick: u32,
    resolved_tick: Option<u32>,
    input: Stamped,
}

/// A command's edges in the simulation's stage order. Reload is a level on the
/// wire, so only a rising level is an edge.
fn edges(use_pressed: bool, drop_pressed: bool, reload: bool, reload_was: bool) -> Vec<Stamped> {
    [
        (drop_pressed, Stamped::Drop),
        (use_pressed, Stamped::Use),
        (reload && !reload_was, Stamped::Reload),
    ]
    .into_iter()
    .filter_map(|(edge, input)| edge.then_some(input))
    .collect()
}

impl Fixture {
    /// Every edge and switch the client sent, in client-tick order.
    fn sent_in_tick_order(&self) -> Vec<(u32, Stamped)> {
        let mut commands: Vec<&wire::InputCommand> = self.sent.iter().collect();
        commands.sort_by_key(|command| command.client_tick);
        let mut sent = Vec::new();
        let mut reload_was = false;
        for command in commands {
            let movement = &command.movement;
            for input in edges(
                movement.use_pressed,
                movement.drop_pressed,
                command.reload,
                reload_was,
            ) {
                sent.push((command.client_tick, input));
            }
            reload_was = command.reload;
        }
        sent.extend(self.sent_switches.iter().map(|switch| {
            (
                switch.client_tick,
                Stamped::Switch(usize::from(switch.slot)),
            )
        }));
        sent.sort();
        sent
    }

    /// Every edge the host's resolutions delivered and every switch it applied,
    /// in the order the simulation met them. A resolution releases its switch
    /// before the presses it carries, and the switch applies before the tick's
    /// simulation consumes them, so within one host tick the switch comes first.
    fn delivered_in_order(&self) -> Vec<Delivery> {
        let mut delivered = Vec::new();
        let mut reload_was = false;
        let mut slot = 0;
        for &(host_tick, active) in &self.host_active {
            if active != slot {
                slot = active;
                delivered.push(Delivery {
                    host_tick,
                    resolved_tick: None,
                    input: Stamped::Switch(active),
                });
            }
            for (_, resolved) in self
                .resolved_commands
                .iter()
                .filter(|(tick, _)| *tick == host_tick)
            {
                let command = &resolved.command;
                for input in edges(
                    command.use_pressed,
                    command.drop_pressed,
                    command.reload,
                    reload_was,
                ) {
                    delivered.push(Delivery {
                        host_tick,
                        resolved_tick: Some(resolved.client_tick),
                        input,
                    });
                }
                reload_was = command.reload;
            }
        }
        delivered
    }

    /// Each edge and switch the client sent reached the host once, in client-tick
    /// order, never ahead of its own tick, and the presses of one tick together.
    fn assert_delivered_once_in_tick_order(&self, label: &str) -> Vec<(u32, Delivery)> {
        let sent = self.sent_in_tick_order();
        let delivered = self.delivered_in_order();
        assert_eq!(
            delivered.iter().map(|d| d.input).collect::<Vec<_>>(),
            sent.iter().map(|&(_, input)| input).collect::<Vec<_>>(),
            "{label}: each input once, in client-tick order"
        );
        let paired: Vec<(u32, Delivery)> = sent
            .iter()
            .map(|&(tick, _)| tick)
            .zip(delivered.iter().copied())
            .collect();
        for &(tick, delivery) in &paired {
            if let Some(resolved) = delivery.resolved_tick {
                assert!(
                    resolved >= tick,
                    "{label}: a press stamped {tick} rode the resolution of {resolved}"
                );
            }
        }
        for pair in paired.windows(2) {
            let [(tick, first), (next_tick, second)] = pair else {
                unreachable!("windows of two");
            };
            let both_presses = first.resolved_tick.is_some() && second.resolved_tick.is_some();
            if both_presses && tick == next_tick {
                assert_eq!(
                    first.host_tick, second.host_tick,
                    "{label}: the presses of client tick {tick} share one resolution"
                );
            }
        }
        paired
    }

    fn assert_both_participating(&self, label: &str) {
        assert!(
            self.server.is_participating(CLIENT) && self.client.is_participating(),
            "{label}: nothing disconnects the client"
        );
    }
}

/// Two edges on one client tick.
type Pair = fn(&mut sim::SimCommand);

const PAIRS: [(&str, Pair); 3] = [
    ("use+reload", |command| {
        command.movement.use_pressed = true;
        command.reload = true;
    }),
    ("drop+use", |command| {
        command.movement.drop_pressed = true;
        command.movement.use_pressed = true;
    }),
    ("drop+reload", |command| {
        command.movement.drop_pressed = true;
        command.reload = true;
    }),
];

// P11: two presses on one tick share a datagram. Losing it once, renet resends
// both on the next flush and dedupes every copy by message id, so each press
// reaches the host once, the two together on their own tick.
#[test]
fn conditioned_two_edges_on_one_tick_deliver_once_across_a_lost_datagram() {
    const PRESS: u32 = 10;
    for (label, pair) in PAIRS {
        let mut fixture = Fixture::new(LinkConfig::perfect(), hitscan_rifle("press", 130.0));
        for offset in 0..40 {
            let mut command = neutral();
            if offset == PRESS {
                pair(&mut command);
            }
            fixture.cut_up = offset == PRESS;
            fixture.step(START + offset, command);
        }
        fixture.cut_up = false;
        assert!(
            fixture.cut_up_packets > 0,
            "{label}: the pair's datagram was lost"
        );
        fixture.idle(START + 40, 60);
        let paired = fixture.assert_delivered_once_in_tick_order(label);
        assert_eq!(paired.len(), 2, "{label}: two edges sent");
        for (_, delivery) in paired {
            assert_eq!(
                delivery.resolved_tick,
                Some(START + PRESS),
                "{label}: the resent pair resolves on its own tick"
            );
        }
    }
}

// P11 under the dev profile's loss and jitter: pairs every 12 ticks for six
// seconds, on several seeds, each press once and each pair together.
#[test]
fn conditioned_two_edges_on_one_tick_deliver_once_under_seeded_loss() {
    const PERIOD: u32 = 12;
    const TICKS: u32 = 360;
    for seed in [MANDATED_SEED, 0x1503, 0x1504] {
        let label = format!("seed {seed:#x}");
        let mut fixture = Fixture::new(
            LinkConfig {
                seed,
                ..mandated_link()
            },
            hitscan_rifle("press", 130.0),
        );
        let lost_before = fixture.up.dropped();
        for offset in 0..TICKS {
            let mut command = neutral();
            if offset % PERIOD == 0 {
                let (_, pair) = PAIRS[(offset / PERIOD) as usize % PAIRS.len()];
                pair(&mut command);
            }
            fixture.step(START + offset, command);
        }
        fixture.idle(START + TICKS, 120);
        assert!(
            fixture.up.dropped() > lost_before,
            "{label}: the seeded link loses uplink datagrams"
        );
        let paired = fixture.assert_delivered_once_in_tick_order(&label);
        assert_eq!(
            paired.len(),
            2 * TICKS.div_ceil(PERIOD) as usize,
            "{label}: every pair delivered"
        );
    }
}

/// The two-second outage, in stream offsets: both directions cut.
const OUTAGE: std::ops::Range<u32> = 60..180;
/// The schedule repeats every `CYCLE` ticks: a start, a use, a drop, a reload,
/// then a switch to the other slot, each on its own tick. A weapon fires every
/// second cycle, well past its 8-tick recovery, and one start per cycle, eight
/// in the outage, keeps the lane far under its 64-start bound.
const CYCLE: u32 = 15;
const RUN_TICKS: u32 = OUTAGE.end + 120;

fn outage_course(offset: u32, slot: usize) -> sim::SimCommand {
    let mut command = match offset % CYCLE {
        0 => stamped_start(START + offset),
        3 => {
            let mut command = neutral();
            command.movement.use_pressed = true;
            command
        }
        6 => {
            let mut command = neutral();
            command.movement.drop_pressed = true;
            command
        }
        8 => {
            let mut command = neutral();
            command.reload = true;
            command
        }
        10 => {
            let mut command = neutral();
            command.select_slot = Some(1 - slot);
            command
        }
        _ => neutral(),
    };
    command.firing_slot = u8::try_from(slot).expect("inventory slots fit a u8");
    command
}

/// A start's terminals: accepted (delivered) and refused, as the client saw them.
fn start_terminals(fixture: &Fixture, start: ActivationToken) -> (usize, usize) {
    let refused = fixture
        .outcomes
        .iter()
        .filter(|outcome| matches!(outcome, wire::ActivationOutcome::InitiationRejected { token, .. } if *token == wire_token(start)))
        .count();
    (fixture.accepted(start), refused)
}

// P12: a total outage longer than the redundancy. renet keeps every unacked
// message and resends it when the link returns, so the burst brings every edge,
// switch and start sent during it; the lanes deliver them once each in client-
// tick order. A start reaches one terminal: delivered, or refused for lagging
// past the 500 ms allowance. No switch is refused as malformed, and nothing
// disconnects.
#[test]
fn conditioned_two_second_outage_delivers_every_lane_once_in_client_tick_order() {
    for (label, link) in [
        ("clean", LinkConfig::perfect()),
        ("mandated", mandated_link()),
    ] {
        let mut fixture = Fixture::new(link, hitscan_rifle("press", 130.0));
        fixture.mirror_rejection_despawn = true;
        fixture.equip_second(&hitscan_rifle("press", 130.0));
        for offset in 0..RUN_TICKS {
            let cut = OUTAGE.contains(&offset);
            fixture.cut_up = cut;
            fixture.cut_down = cut;
            let slot = fixture
                .local
                .borrow()
                .get_component::<Inventory>(fixture.local_actors.pawn)
                .expect("the local pawn holds an inventory")
                .active_slot;
            fixture.step(START + offset, outage_course(offset, slot));
        }
        fixture.cut_up = false;
        fixture.cut_down = false;
        fixture.idle(START + RUN_TICKS, 240);
        assert!(
            fixture.cut_up_packets > 0 && fixture.cut_down_packets > 0,
            "{label}: the outage cut both directions"
        );
        fixture.assert_both_participating(label);

        // Edges and switches.
        let in_outage = |tick: u32| OUTAGE.contains(&(tick - START));
        let sent = fixture.sent_in_tick_order();
        let sent_in_outage = sent.iter().filter(|(tick, _)| in_outage(*tick)).count();
        let switches_in_outage = fixture
            .sent_switches
            .iter()
            .filter(|switch| in_outage(switch.client_tick))
            .count();
        assert!(
            sent_in_outage >= 24 && switches_in_outage >= 6,
            "{label}: the outage holds presses ({sent_in_outage}) and switches ({switches_in_outage})"
        );
        fixture.assert_delivered_once_in_tick_order(label);
        let declared: Vec<(u32, bool)> = fixture
            .sent_switches
            .iter()
            .map(|switch| (switch.declaration_id, true))
            .collect();
        let answered: Vec<(u32, bool)> = fixture
            .switch_outcomes
            .iter()
            .map(|&(_, id, accepted)| (id, accepted))
            .collect();
        assert_eq!(
            answered, declared,
            "{label}: every switch accepted once, in declaration order; none refused as malformed"
        );

        // Starts.
        let mut starts: Vec<ActivationToken> = fixture
            .sent
            .iter()
            .filter_map(|command| command.activation.initiation)
            .map(|wire| token(wire.start_tick, ActivationLane::Primary))
            .collect();
        starts.sort_by_key(|start| start.start_tick);
        starts.dedup();
        let mut delivered = Vec::new();
        let mut lagged = Vec::new();
        for &start in &starts {
            let (accepted, refused) = start_terminals(&fixture, start);
            assert_eq!(
                accepted + refused,
                1,
                "{label}: start {} reaches exactly one terminal (accepted {accepted}, refused {refused})",
                start.start_tick
            );
            let minted = fixture
                .authorized
                .iter()
                .filter(|shot| shot.shot_id.start_tick == start.start_tick)
                .count();
            assert_eq!(
                minted, accepted,
                "{label}: start {} minted {minted} shots",
                start.start_tick
            );
            if refused == 1 {
                let why: Vec<netcode::LaneRefusal> = fixture
                    .lane_refusals
                    .iter()
                    .filter(|(_, token, _)| *token == start)
                    .map(|&(_, _, why)| why)
                    .collect();
                assert_eq!(
                    why,
                    [netcode::LaneRefusal::Lagged],
                    "{label}: start {} is refused only for lagging past the allowance",
                    start.start_tick
                );
                let refused_at = fixture
                    .lane_refusals
                    .iter()
                    .find(|(_, token, _)| *token == start)
                    .map(|&(at, _, _)| at)
                    .expect("the lane refused it");
                lagged.push((start.start_tick - START, refused_at));
            } else {
                delivered.push(start.start_tick - START);
            }
        }
        let outage_starts = starts
            .iter()
            .filter(|start| in_outage(start.start_tick))
            .count();
        eprintln!(
            "[outage lanes] {label}: {} edges and switches ({sent_in_outage} in the outage), \
             {} switches; {} starts ({outage_starts} in the outage): delivered at offsets \
             {delivered:?}, refused for lag (offset, host tick) {lagged:?}; cut {} up / {} down datagrams",
            sent.len(),
            fixture.sent_switches.len(),
            starts.len(),
            fixture.cut_up_packets,
            fixture.cut_down_packets,
        );
        assert!(
            outage_starts >= 6,
            "{label}: the outage holds starts ({outage_starts})"
        );
    }
}
