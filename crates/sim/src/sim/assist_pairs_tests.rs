//! AC13's pair results through the bound table: clamps, gates, faults and
//! their once-per-(descriptor, state, field) warnings, re-armed by a rebind.
//! See: context/lib/scripting.md §11 (interaction-pair scope) · §12 (Assist)

use std::collections::BTreeMap;

use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{
    EntityTypeDescriptor, LifeStatesDescriptor, ReplicationScope, SlotOwnership, SlotRecord,
    SlotSchema, SlotType, SlotValue, Transform,
};
use postretro_foundation::{Assistable, IrNode, IrValue, LifeStateProfile, Seat};
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::sim::assist::{AssistState, increment};

fn input(name: &str, owner: Option<&str>) -> IrNode {
    IrNode::Input {
        name: name.into(),
        owner: owner.map(str::to_string),
    }
}

fn number(value: f32) -> IrNode {
    IrNode::Const {
        value: IrValue::Number(value),
    }
}

fn record(default: f32, per_owner: bool) -> SlotRecord {
    SlotRecord::new(SlotSchema {
        slot_type: SlotType::Number,
        default: Some(SlotValue::Number(default)),
        range: None,
        persist: false,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network: if per_owner {
            ReplicationScope::OwnerPrivatePlayer
        } else {
            ReplicationScope::None
        },
        per_owner,
        accumulate: None,
    })
}

struct World {
    ctx: ScriptCtx,
    helper: EntityId,
    target: EntityId,
}

impl World {
    /// `radius` and `holdMs` of the `down` state; global `world.radius` /
    /// `world.hold`, per-owner `stats.heal` (the helper holds Seat(0)).
    fn new(radius: NumberOrIr, hold_ms: NumberOrIr) -> Self {
        let ctx = ScriptCtx::new();
        {
            let mut table = ctx.slot_table.borrow_mut();
            table
                .insert_namespace(
                    "world",
                    vec![
                        ("radius".into(), record(2.0, false)),
                        ("hold".into(), record(1000.0, false)),
                    ],
                )
                .unwrap();
            table
                .insert_namespace("stats", vec![("heal".into(), record(0.0, true))])
                .unwrap();
        }
        let descriptor = EntityTypeDescriptor {
            canonical_name: Some("player".into()),
            ghost: false,
            faction: None,
            tolerance: None,
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
            life_states: Some(LifeStatesDescriptor {
                initial: "alive".into(),
                states: BTreeMap::from([
                    ("alive".into(), LifeStateProfile::default()),
                    (
                        "down".into(),
                        LifeStateProfile {
                            assistable: Some(Assistable { radius, hold_ms }),
                            ..Default::default()
                        },
                    ),
                ]),
                rejoin: Default::default(),
                state_ids: Default::default(),
                table_revision: 0,
            }),
        };
        ctx.data_registry
            .borrow_mut()
            .replace_entity_types(vec![descriptor]);
        let (helper, target) = {
            let mut registry = ctx.registry.borrow_mut();
            let mut spawn = |state: u8| {
                let pawn = registry.spawn(Transform::default());
                let mut movement = crate::sim::tests::trigger_movement();
                movement.life_state = state;
                registry.set_component(pawn, movement).unwrap();
                registry
                    .set_component(
                        pawn,
                        DescriptorProvenance {
                            canonical_name: "player".into(),
                            owned_components: Default::default(),
                            map_overrides: Default::default(),
                            spawn_path: DescriptorSpawnPath::PlayerSpawn,
                        },
                    )
                    .unwrap();
                pawn
            };
            let helper = spawn(ALIVE);
            let target = spawn(DOWN);
            registry.bind_pawn_seat(helper, Seat(0));
            (helper, target)
        };
        Self {
            ctx,
            helper,
            target,
        }
    }

    fn set(&self, name: &str, value: f32) {
        self.ctx
            .slot_table
            .borrow_mut()
            .get_mut(name)
            .unwrap()
            .write_value(Some(SlotValue::Number(value)));
    }

    fn pass(&self, table: &AssistPairTable) -> PairValues {
        let registry = self.ctx.registry.borrow();
        table.seed_sides(
            &registry,
            Some(&self.ctx.slot_table.borrow()),
            &[self.helper, self.target],
        );
        let target = AssistTarget::of(&registry, self.target).expect("target is a pawn");
        evaluate(table, self.helper, target)
    }
}

// `alive` < `down` in the mod-wide sorted name table.
const ALIVE: u8 = 0;
const DOWN: u8 = 1;

fn expression_world() -> World {
    World::new(
        NumberOrIr::Ir(input("world.radius", None)),
        NumberOrIr::Ir(input("world.hold", None)),
    )
}

#[test]
fn assist_pairs_radius_rows_clamp_gate_and_pass_through() {
    let world = expression_world();
    let table = AssistPairTable::bind(&world.ctx);
    for (radius, expected) in [
        (20.0, 16.0),
        (16.0, 16.0),
        (2.5, 2.5),
        (0.0, 0.0),
        (-1.0, 0.0),
    ] {
        world.set("world.radius", radius);
        let values = world.pass(&table);
        assert_eq!(values.radius, expected, "radius {radius}");
        assert_eq!(values.required_ticks, required_count(1000.0));
    }
    // An unassistable pair (radius ≤ 0) is never in range, even at distance 0.
    assert!(
        !PairValues {
            radius: 0.0,
            required_ticks: 1
        }
        .in_range(0.0)
    );
    assert!(
        PairValues {
            radius: 2.5,
            required_ticks: 1
        }
        .in_range(2.5)
    );
}

#[test]
fn assist_pairs_hold_rows_clamp_convert_and_complete_on_first_tick_at_zero() {
    let world = expression_world();
    let table = AssistPairTable::bind(&world.ctx);
    for (hold_ms, expected) in [
        (90000.0, required_count(60000.0)),
        (60000.0, 3600),
        (3000.0, 180),
        (100.0, 6),
        (0.0, 1),
    ] {
        world.set("world.hold", hold_ms);
        assert_eq!(
            world.pass(&table).required_ticks,
            expected,
            "holdMs {hold_ms}"
        );
    }
}

/// O60: a `holdMs` of −0.0 counts as 0 — required count one, completing on
/// the first held tick, with no fault warning.
#[test]
fn assist_pairs_o60_negative_zero_hold_completes_on_first_tick_without_warning() {
    let world = World::new(
        NumberOrIr::Literal(1.5),
        NumberOrIr::Ir(IrNode::Mul {
            a: Box::new(input("world.hold", None)),
            b: Box::new(number(-1.0)),
        }),
    );
    world.set("world.hold", 0.0);
    let table = AssistPairTable::bind(&world.ctx);
    let capture = LogCapture::start();
    let values = world.pass(&table);
    assert_eq!(values.required_ticks, 1);
    assert_eq!(
        increment(values.required_ticks),
        crate::sim::assist::PROGRESS_UNIT
    );
    capture.assert_not_logged(log::Level::Warn, "assistable.holdMs");
}

fn fault_warnings(capture: &LogCapture, field: &str) -> usize {
    capture
        .records()
        .iter()
        .filter(|record| {
            record.level == log::Level::Warn
                && record
                    .message
                    .contains(&format!("states.down.assistable.{field}` faulted"))
        })
        .count()
}

/// Every fault row: the pair takes its sentinel, and each (descriptor, state,
/// field) warns once per bind; a rebind re-arms it.
#[test]
fn assist_pairs_fault_rows_take_their_sentinel_and_warn_once_per_bind() {
    // Non-finite: division by zero; negative: a negative global.
    let world = World::new(
        NumberOrIr::Ir(IrNode::Div {
            a: Box::new(number(1.0)),
            b: Box::new(input("world.radius", None)),
        }),
        NumberOrIr::Ir(input("world.hold", None)),
    );
    world.set("world.radius", 0.0);
    world.set("world.hold", -5.0);
    let mut state = AssistState::default();
    state.rebind(&world.ctx);
    let capture = LogCapture::start();
    for _ in 0..3 {
        assert_eq!(world.pass(state.pair_table()), PairValues::UNASSISTABLE);
    }
    assert_eq!(fault_warnings(&capture, "radius"), 1);
    assert_eq!(fault_warnings(&capture, "holdMs"), 1);
    // A non-finite holdMs is the same fault.
    world.set("world.hold", f32::INFINITY);
    assert_eq!(world.pass(state.pair_table()).required_ticks, 0);
    assert_eq!(fault_warnings(&capture, "holdMs"), 1);

    capture.clear();
    state.rebind(&world.ctx);
    world.pass(state.pair_table());
    world.pass(state.pair_table());
    assert_eq!(
        fault_warnings(&capture, "radius"),
        1,
        "a rebind re-arms the warning"
    );
    assert_eq!(
        fault_warnings(&capture, "holdMs"),
        1,
        "a rebind re-arms the warning"
    );

    // Recovered values evaluate normally under the same bind.
    world.set("world.radius", 0.5);
    world.set("world.hold", 0.0);
    assert_eq!(
        world.pass(state.pair_table()),
        PairValues {
            radius: 2.0,
            required_ticks: 1
        }
    );
}

/// A side value an expression reads that the side lacks (a per-owner value
/// for a seatless pawn) faults before evaluation: radius out of range,
/// holdMs kept.
#[test]
fn assist_pairs_missing_side_value_faults_before_evaluation() {
    let world = World::new(
        NumberOrIr::Ir(input("stats.heal", Some("@assist.target"))),
        NumberOrIr::Ir(input("stats.heal", Some("@assist.target"))),
    );
    let table = AssistPairTable::bind(&world.ctx);
    let capture = LogCapture::start();
    assert_eq!(world.pass(&table), PairValues::UNASSISTABLE);
    assert_eq!(fault_warnings(&capture, "radius"), 1);
    assert_eq!(fault_warnings(&capture, "holdMs"), 1);
    // A pawn the pass did not seed reads as missing too.
    let registry = world.ctx.registry.borrow();
    table.seed_sides(
        &registry,
        Some(&world.ctx.slot_table.borrow()),
        &[world.helper],
    );
    let target = AssistTarget::of(&registry, world.target).unwrap();
    assert_eq!(
        evaluate(&table, world.helper, target),
        PairValues::UNASSISTABLE
    );
}

/// The healer recipe: a single-player helper reads its per-owner value at its
/// seat, and the target's life state picks the radius.
#[test]
fn assist_pairs_single_player_helper_reads_its_per_owner_value_at_its_seat() {
    let world = World::new(
        NumberOrIr::Ir(IrNode::Select {
            cond: Box::new(input("player.inLifeState:down", Some("@assist.target"))),
            a: Box::new(number(3.0)),
            b: Box::new(number(1.5)),
        }),
        NumberOrIr::Ir(IrNode::Add {
            a: Box::new(IrNode::Mul {
                a: Box::new(input("stats.heal", Some("@assist.helper"))),
                b: Box::new(number(-20.0)),
            }),
            b: Box::new(number(3000.0)),
        }),
    );
    world
        .ctx
        .slot_table
        .borrow_mut()
        .get_mut("stats.heal")
        .unwrap()
        .set_per_seat_value(Seat(0), SlotValue::Number(100.0));
    let table = AssistPairTable::bind(&world.ctx);
    assert_eq!(
        world.pass(&table),
        PairValues {
            radius: 3.0,
            required_ticks: required_count(1000.0)
        }
    );
}

#[test]
fn assist_pairs_literal_profile_skips_evaluation_and_unbound_states_are_unassistable() {
    let world = World::new(NumberOrIr::Literal(2.0), NumberOrIr::Literal(5000.0));
    let table = AssistPairTable::bind(&world.ctx);
    let registry = world.ctx.registry.borrow();
    // No seeding: a literal-only profile never reads a side.
    let target = AssistTarget::of(&registry, world.target).unwrap();
    assert_eq!(
        evaluate(&table, world.helper, target),
        PairValues {
            radius: 2.0,
            required_ticks: required_count(5000.0)
        }
    );
    assert!(table.declares("player", DOWN));
    assert!(!table.declares("player", ALIVE));
    let alive = AssistTarget {
        life_state: ALIVE,
        ..target
    };
    assert_eq!(
        evaluate(&table, world.helper, alive),
        PairValues::UNASSISTABLE
    );
    drop(registry);

    // A connected client binds nothing.
    world.ctx.owner_slot_writes_enabled.set(false);
    let client = AssistPairTable::bind(&world.ctx);
    assert!(!client.declares("player", DOWN));
}
