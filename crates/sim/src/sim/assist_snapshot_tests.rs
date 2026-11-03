//! The snapshot's assist facts: each helper's pair table from the registry
//! state as it stands, and the received count from the last pass.
//! See: context/lib/networking.md §Version gates

use super::*;
use crate::sim::assist_snapshot::PawnAssistFacts;

impl World {
    fn facts(&self, helpers: &[usize]) -> Vec<PawnAssistFacts> {
        self.facts_or_none(helpers)
            .into_iter()
            .map(|facts| facts.expect("a seated helper has facts"))
            .collect()
    }

    fn facts_or_none(&self, helpers: &[usize]) -> Vec<Option<PawnAssistFacts>> {
        let helpers: Vec<EntityId> = helpers.iter().map(|index| self.pawns[*index]).collect();
        self.state.snapshot_facts(
            &self.ctx.registry.borrow(),
            &self.ctx.slot_table.borrow(),
            &helpers,
        )
    }

    fn pairs(&self, helper: usize) -> Vec<(EntityId, PairValues)> {
        self.facts(&[helper]).remove(0).pairs
    }

    fn received(&self, target: usize) -> u32 {
        self.facts(&[target]).remove(0).received_required_ticks
    }
}

const LITERAL: PairValues = PairValues {
    radius: 1.5,
    required_ticks: SIX,
};

/// Every other pawn whose `assistable` bool is set has an entry, in
/// `players()` order, whatever the helper's own state; a pawn never lists
/// itself.
#[test]
fn assist_snapshot_lists_every_other_assistable_pawn_whatever_the_helpers_state() {
    let world = World::new(Some(literal(1.5, 100.0)), 4);
    world.enter(1, "down");
    world.enter(3, "wounded");
    let (one, three) = (world.pawns[1], world.pawns[3]);
    assert_eq!(world.pairs(0), vec![(one, LITERAL), (three, LITERAL)]);
    // A downed helper, which may not `use`, still gets its table.
    assert_eq!(world.pairs(1), vec![(three, LITERAL)]);
    assert_eq!(world.pairs(3), vec![(one, LITERAL)]);
    // Facts come back in the helpers' order.
    let facts = world.facts(&[3, 0]);
    assert_eq!(facts[0].pairs, vec![(one, LITERAL)]);
    assert_eq!(facts[1].pairs.len(), 2);
}

/// O59: the last candidate leaving its assistable state empties the table.
#[test]
fn assist_snapshot_o59_last_candidate_leaving_empties_the_table() {
    let world = World::new(Some(literal(1.5, 100.0)), 2);
    world.enter(1, "down");
    assert_eq!(world.pairs(0).len(), 1);
    world.enter(1, "alive");
    assert!(world.pairs(0).is_empty());
}

/// A pair whose radius is at or below 0 carries radius 0 with its required
/// ticks; a radius fault carries radius 0; a `holdMs` fault carries
/// `required_ticks` 0.
#[test]
fn assist_snapshot_unassistable_and_faulted_pairs() {
    // radius = 2 × the helper's `stats.heal`.
    let world = World::new(
        Some(Assistable {
            radius: NumberOrIr::Ir(IrNode::Mul {
                a: Box::new(input("stats.heal", Some("@assist.helper"))),
                b: Box::new(IrNode::Const {
                    value: IrValue::Number(2.0),
                }),
            }),
            hold_ms: NumberOrIr::Literal(100.0),
        }),
        3,
    );
    world.enter(2, "down");
    world.set_owned("stats.heal", 0, 0.0);
    world.set_owned("stats.heal", 1, 1.0);
    let target = world.pawns[2];
    let facts = world.facts(&[0, 1]);
    assert_eq!(
        facts[0].pairs,
        vec![(
            target,
            PairValues {
                radius: 0.0,
                required_ticks: SIX
            }
        )]
    );
    assert_eq!(
        facts[1].pairs,
        vec![(
            target,
            PairValues {
                radius: 2.0,
                required_ticks: SIX
            }
        )]
    );

    // radius = world.radius / world.hold faults at hold 0.
    let world = World::new(
        Some(Assistable {
            radius: NumberOrIr::Ir(IrNode::Div {
                a: Box::new(input("world.radius", None)),
                b: Box::new(input("world.hold", None)),
            }),
            hold_ms: NumberOrIr::Literal(100.0),
        }),
        2,
    );
    world.enter(1, "down");
    world.set("world.hold", 0.0);
    assert_eq!(
        world.pairs(0),
        vec![(
            world.pawns[1],
            PairValues {
                radius: 0.0,
                required_ticks: SIX
            }
        )]
    );

    // holdMs = world.hold / world.radius faults at radius 0.
    let world = World::new(
        Some(Assistable {
            radius: NumberOrIr::Literal(1.5),
            hold_ms: NumberOrIr::Ir(IrNode::Div {
                a: Box::new(input("world.hold", None)),
                b: Box::new(input("world.radius", None)),
            }),
        }),
        2,
    );
    world.enter(1, "down");
    world.set("world.radius", 0.0);
    assert_eq!(
        world.pairs(0),
        vec![(
            world.pawns[1],
            PairValues {
                radius: 1.5,
                required_ticks: 0
            }
        )]
    );
}

/// O55: a frame-end write after the frame's last pass is read by the
/// snapshot's table and by the next tick's pass alike.
#[test]
fn assist_snapshot_o55_frame_end_write_is_read_by_the_table_and_the_next_pass() {
    let mut world = healer_world();
    world.enter(1, "down");
    world.set("world.hold", 1000.0);
    world.tick(&[HOLD, IDLE]);
    let before = world.pairs(0)[0].1.required_ticks;
    assert_eq!(before, required_count(1000.0));
    // The frame-end reaction write, then serialization.
    world.set_owned("stats.heal", 0, 50.0);
    let written = required_count(500.0);
    assert_eq!(world.pairs(0)[0].1.required_ticks, written);
    world.tick(&[HOLD, IDLE]);
    assert_eq!(
        world
            .state
            .pair_values(world.pawns[0], world.pawns[1])
            .unwrap()
            .required_ticks,
        written
    );
}

/// O42 (host half): a rebind mid-hold keeps progress, and the next table
/// carries the new `required_ticks`.
#[test]
fn assist_snapshot_o42_rebind_mid_hold_keeps_progress_and_the_table_carries_the_new_count() {
    let mut world = helper_and_downed_target();
    for _ in 0..3 {
        world.tick(&[HOLD, IDLE]);
    }
    let kept = world.progress(0);
    assert_eq!(kept, 3 * increment(SIX));
    assert_eq!(world.pairs(0)[0].1.required_ticks, SIX);

    world
        .ctx
        .data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor(Some(literal(1.5, 1000.0)))]);
    world.state.rebind(&world.ctx);
    let slower = required_count(1000.0);
    assert_eq!(world.progress(0), kept);
    assert_eq!(world.pairs(0)[0].1.required_ticks, slower);
    world.tick(&[HOLD, IDLE]);
    assert_eq!(world.progress(0), kept + increment(slower));
    assert_eq!(world.received(1), slower);
}

/// `received_required_ticks` names the highest-progress helper on the pawn,
/// ties by `players()` order, and is 0 with none or while the pawn is not
/// assistable. O40 (host half): slow B, then fast A overtakes; after A's
/// completion revives T it falls to 0.
#[test]
fn assist_snapshot_o40_received_ticks_follow_the_highest_progress_helper() {
    // B = pawn 0 at heal 0 (1060 ms), A = pawn 1 at heal 53 (530 ms), T = 2.
    let mut world = World::new(
        Some(Assistable {
            radius: NumberOrIr::Literal(1.5),
            hold_ms: NumberOrIr::Ir(IrNode::Add {
                a: Box::new(input("world.hold", None)),
                b: Box::new(IrNode::Mul {
                    a: Box::new(input("stats.heal", Some("@assist.helper"))),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(-10.0),
                    }),
                }),
            }),
        }),
        3,
    );
    world.set("world.hold", 1060.0);
    world.set_owned("stats.heal", 1, 53.0);
    world.place(0, 2.5);
    world.place(1, 1.0);
    world.enter(2, "down");
    let (slow, fast) = (required_count(1060.0), required_count(530.0));
    assert_eq!((slow, fast), (64, 32));

    assert_eq!(world.received(2), 0, "no helper yet");
    // B alone for four ticks.
    for _ in 0..4 {
        world.tick(&[HOLD, IDLE, IDLE]);
        assert_eq!(world.received(2), slow);
    }
    // Both hold; A's increment is twice B's, so they tie after four more
    // ticks, and the tie goes to B, earlier in `players()` order.
    for _ in 0..4 {
        world.tick(&[HOLD, HOLD, IDLE]);
        assert_eq!(world.received(2), slow);
    }
    assert_eq!(world.progress(0), world.progress(1));
    world.tick(&[HOLD, HOLD, IDLE]);
    assert_eq!(world.received(2), fast, "A passed B");
    // A completes on its own 32nd advancing tick.
    let landed = world.ticks_to_complete(&[HOLD, HOLD, IDLE], 40);
    assert_eq!(landed, Some(fast - 5));
    // The completion's fire list revives T in the pass: 0 at that snapshot.
    world.enter(2, "alive");
    assert_eq!(world.received(2), 0);
}

/// A helper whose seat is in a disconnect hold is outside the `players()`
/// group: it gets no facts and evaluates nothing, so its seatless per-owner
/// read spends no fault warning, and a later genuine fault on a seated helper
/// still warns.
#[test]
fn assist_snapshot_held_seat_helper_evaluates_nothing_and_spends_no_warning() {
    // radius = world.radius / the helper's `stats.heal`.
    let world = World::new(
        Some(Assistable {
            radius: NumberOrIr::Ir(IrNode::Div {
                a: Box::new(input("world.radius", None)),
                b: Box::new(input("stats.heal", Some("@assist.helper"))),
            }),
            hold_ms: NumberOrIr::Literal(100.0),
        }),
        3,
    );
    world.enter(2, "down");
    world.set_owned("stats.heal", 0, 2.0);
    world
        .ctx
        .registry
        .borrow_mut()
        .clear_pawn_seat(world.pawns[1]);
    let faults = |capture: &LogCapture| {
        capture
            .records()
            .iter()
            .filter(|record| record.message.contains("assistable.radius` faulted"))
            .count()
    };

    let capture = LogCapture::start();
    let facts = world.facts_or_none(&[1, 0]);
    assert_eq!(facts[0], None, "the held seat's helper gets no facts");
    assert_eq!(
        facts[1].as_ref().map(|facts| facts.pairs.clone()),
        Some(vec![(
            world.pawns[2],
            PairValues {
                radius: 1.0,
                required_ticks: SIX
            }
        )])
    );
    assert_eq!(faults(&capture), 0, "the held helper spent no warning");

    // A genuine fault: the seated helper's divisor goes to 0.
    world.set_owned("stats.heal", 0, 0.0);
    assert_eq!(world.pairs(0)[0].1.radius, 0.0);
    assert_eq!(faults(&capture), 1);
}
