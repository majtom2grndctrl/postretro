// Expression layer parameters on the host: tick-start evaluation and clamp,
// last-good fault handling, spawn evaluation, and the per-tick cost bounds.
// See: context/lib/scripting.md §11 (Defense layers) · plans E16 §Pinned orderings

use std::sync::Arc;

use postretro_entities::ScriptCtx;
use postretro_entities::components::defense_layers::DefenseLayersComponent;
use postretro_entities::components::entity_state::EntityStateComponent;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
};
use postretro_entities::data_descriptors::EntityTypeDescriptor;
use postretro_entities::registry::{EntityId, Transform};
use postretro_entities::slot_table::{
    NumericRange, ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
};
use postretro_foundation::defense::{
    DamageTypeDeclaration, DefenseVocabulary, HandleRef, PoolMultiplier,
};
use postretro_foundation::{
    AuthoredHandle, DamagePayload, DefenseLayerEntry, HealthDescriptor, IrNode, IrValue,
    NumberOrIr, Seat,
};
use postretro_test_log_capture::LogCapture;

use super::defense_layers::{VISITED, recharge_defense_layers};
use super::defense_parameters::{DefenseParameters, EVALUATIONS};

fn slot(value: f32, per_owner: bool) -> SlotRecord {
    SlotRecord::new(SlotSchema {
        slot_type: SlotType::Number,
        default: Some(SlotValue::Number(value)),
        range: Some(NumericRange {
            min: -1.0e7,
            max: 1.0e7,
        }),
        persist: false,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network: ReplicationScope::None,
        per_owner,
        accumulate: None,
    })
}

fn input(name: &str) -> IrNode {
    IrNode::Input {
        name: name.to_string(),
        owner: None,
    }
}

fn constant(value: f32) -> IrNode {
    IrNode::Const {
        value: IrValue::Number(value),
    }
}

fn div(a: IrNode, b: IrNode) -> IrNode {
    IrNode::Div {
        a: Box::new(a),
        b: Box::new(b),
    }
}

fn mul(a: IrNode, b: IrNode) -> IrNode {
    IrNode::Mul {
        a: Box::new(a),
        b: Box::new(b),
    }
}

fn expr(node: IrNode) -> NumberOrIr {
    NumberOrIr::Ir(node)
}

fn lit(value: f32) -> NumberOrIr {
    NumberOrIr::Literal(value)
}

struct Layer {
    capacity: NumberOrIr,
    delay: Option<NumberOrIr>,
    rate: Option<NumberOrIr>,
    resist: Vec<(&'static str, NumberOrIr)>,
}

fn layer(capacity: NumberOrIr) -> Layer {
    Layer {
        capacity,
        delay: None,
        rate: None,
        resist: Vec::new(),
    }
}

fn health(layers: Vec<Layer>) -> HealthDescriptor {
    HealthDescriptor {
        max: 100.0,
        layers: layers
            .into_iter()
            .map(|layer| DefenseLayerEntry {
                layer: AuthoredHandle::from_handle(&HandleRef::DefenseLayer {
                    name: "shield".to_string(),
                }),
                capacity: layer.capacity,
                recharge_delay_ms: layer.delay,
                recharge_rate: layer.rate,
                resist: layer
                    .resist
                    .into_iter()
                    .map(|(ty, value)| (ty.to_string(), value))
                    .collect(),
            })
            .collect(),
        ..Default::default()
    }
}

/// A host session with the global slots `tune.*`, the per-owner slot
/// `seat.cap`, the shock type and one descriptor per entry of `descriptors`.
fn session(descriptors: Vec<(&str, bool, HealthDescriptor)>) -> ScriptCtx {
    let ctx = ScriptCtx::new();
    {
        let mut table = ctx.slot_table.borrow_mut();
        for (name, value) in [
            ("tune.cap", 100.0),
            ("tune.zero", 0.0),
            ("tune.delay", 3_000.0),
            ("tune.rate", 60.0),
            ("tune.ward", 0.0),
        ] {
            table.insert(name.to_string(), slot(value, false)).unwrap();
        }
        table
            .insert("seat.cap".to_string(), slot(0.0, true))
            .unwrap();
    }
    ctx.registry
        .borrow_mut()
        .set_defense_vocabulary(Arc::new(DefenseVocabulary {
            layers: vec!["shield".to_string()],
            damage_types: vec![DamageTypeDeclaration {
                name: "shock".to_string(),
                matrix: std::collections::BTreeMap::from([(
                    "shield".to_string(),
                    PoolMultiplier::Scale(2.0),
                )]),
            }],
            default_damage_type: None,
        }));
    {
        let mut data = ctx.data_registry.borrow_mut();
        for (name, player, health) in descriptors {
            data.upsert_entity_type(EntityTypeDescriptor {
                life_states: None,
                faction: None,
                ghost: false,
                tolerance: None,
                canonical_name: Some(name.to_string()),
                inventory: None,
                light: None,
                emitter: None,
                movement: player.then(crate::sim::teleport_tick_tests::trigger_movement_descriptor),
                weapon: None,
                touchable: None,
                mesh: None,
                health: Some(health),
                behavior: None,
            });
        }
    }
    DefenseParameters::install(&ctx);
    ctx
}

fn set_global(ctx: &ScriptCtx, name: &str, value: f32) {
    ctx.slot_table
        .borrow_mut()
        .get_mut(name)
        .unwrap()
        .write_value(Some(SlotValue::Number(value)));
}

/// Spawn `descriptor`'s layers the way the data-archetype spawn does:
/// evaluate, then fill.
fn spawn(ctx: &ScriptCtx, descriptor: &str) -> EntityId {
    let health_desc = ctx
        .data_registry
        .borrow()
        .entities
        .iter()
        .find(|d| d.canonical_name.as_deref() == Some(descriptor))
        .and_then(|d| d.health.clone())
        .unwrap();
    let mut registry = ctx.registry.borrow_mut();
    let id = registry.spawn(Transform::default());
    registry
        .set_component(id, HealthComponent::from_descriptor(&health_desc))
        .unwrap();
    registry
        .set_component(id, EntityStateComponent::default())
        .unwrap();
    let mut layers = DefenseLayersComponent::from_descriptor(&health_desc).unwrap();
    layers.descriptor = Some(descriptor.to_string());
    if let Some(source) = registry.defense_parameters().cloned() {
        source.evaluate(&registry, id, &mut layers);
    }
    layers.fill();
    registry.set_component(id, layers).unwrap();
    id
}

fn tick(ctx: &ScriptCtx) {
    let mut registry = ctx.registry.borrow_mut();
    registry.advance_host_tick();
    recharge_defense_layers(&mut registry);
}

fn shield(ctx: &ScriptCtx, id: EntityId) -> (f32, f32) {
    let registry = ctx.registry.borrow();
    let layer = registry
        .get_component::<DefenseLayersComponent>(id)
        .unwrap()
        .layers[0]
        .clone();
    (layer.value, layer.capacity)
}

fn hit(ctx: &ScriptCtx, id: EntityId, amount: f32, ty: Option<u16>) {
    apply_damage_with_context(
        &mut ctx.registry.borrow_mut(),
        id,
        &DamagePayload {
            amount,
            damage_type: ty,
            ..Default::default()
        },
        DamageContext::new("test", DamageProducer::InTick),
    );
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1.0e-3
}

#[test]
fn expression_capacity_tracks_the_store_clamps_and_never_fills_on_raise() {
    let ctx = session(vec![(
        "enemy",
        false,
        health(vec![layer(expr(input("tune.cap")))]),
    )]);
    let id = spawn(&ctx, "enemy");
    assert_eq!(shield(&ctx, id), (100.0, 100.0));
    set_global(&ctx, "tune.cap", 150.0);
    tick(&ctx);
    assert_eq!(
        shield(&ctx, id),
        (100.0, 150.0),
        "raising capacity never fills"
    );
    set_global(&ctx, "tune.cap", 40.0);
    tick(&ctx);
    assert_eq!(shield(&ctx, id), (40.0, 40.0), "lowering capacity clamps");
}

#[test]
fn faulted_layer_parameter_keeps_last_good_and_warns_once_per_field() {
    let capture = LogCapture::start();
    let mut faulty = layer(expr(div(input("tune.cap"), input("tune.zero"))));
    faulty.rate = Some(expr(div(input("tune.rate"), input("tune.zero"))));
    let ctx = session(vec![
        (
            "good-then-bad",
            false,
            health(vec![layer(expr(div(
                input("tune.cap"),
                input("tune.ward"),
            )))]),
        ),
        ("faulty", false, health(vec![faulty])),
    ]);
    // `tune.ward` is 0, so the first descriptor faults until it is set.
    set_global(&ctx, "tune.ward", 1.0);
    let first = spawn(&ctx, "good-then-bad");
    let second = spawn(&ctx, "good-then-bad");
    assert_eq!(shield(&ctx, first), (100.0, 100.0));
    set_global(&ctx, "tune.ward", 0.0);
    tick(&ctx);
    assert_eq!(
        shield(&ctx, first),
        (100.0, 100.0),
        "each entity keeps its last good value"
    );
    assert_eq!(shield(&ctx, second), (100.0, 100.0));
    capture.assert_logged_once(
        log::Level::Warn,
        "descriptor `good-then-bad` `components.health.layers[0].capacity` faulted",
    );
    // Faulting before any good value reads the field's omitted default.
    let faulty_id = spawn(&ctx, "faulty");
    assert_eq!(
        shield(&ctx, faulty_id),
        (0.0, 0.0),
        "capacity falls back to 0"
    );
    assert_eq!(
        ctx.registry
            .borrow()
            .get_component::<DefenseLayersComponent>(faulty_id)
            .unwrap()
            .layers[0]
            .recharge_rate,
        0.0
    );
    capture.assert_logged_once(
        log::Level::Warn,
        "descriptor `faulty` `components.health.layers[0].capacity` faulted",
    );
    capture.assert_logged_once(
        log::Level::Warn,
        "`components.health.layers[0].rechargeRate` faulted",
    );
    // A reinstall (hot reload) re-arms the warning.
    DefenseParameters::install(&ctx);
    tick(&ctx);
    let warnings = capture
        .records()
        .iter()
        .filter(|record| {
            record.message.contains(
                "descriptor `good-then-bad` `components.health.layers[0].capacity` faulted",
            )
        })
        .count();
    assert_eq!(warnings, 2);
}

#[test]
fn capacity_faulting_at_spawn_spawns_empty_and_recharges_unhit() {
    let mut entry = layer(expr(div(input("tune.cap"), input("tune.ward"))));
    entry.rate = Some(lit(60.0));
    let ctx = session(vec![("enemy", false, health(vec![entry]))]);
    let id = spawn(&ctx, "enemy");
    assert_eq!(shield(&ctx, id), (0.0, 0.0));
    set_global(&ctx, "tune.ward", 1.0);
    set_global(&ctx, "tune.cap", 80.0);
    tick(&ctx);
    let (value, capacity) = shield(&ctx, id);
    assert_eq!(capacity, 80.0);
    assert!(
        approx(value, 1.0),
        "never hit: recharges the same tick, {value}"
    );
}

#[test]
fn layer_capacity_tracks_own_state() {
    let ctx = session(vec![(
        "enemy",
        false,
        health(vec![layer(expr(mul(input("@state.tier"), constant(10.0))))]),
    )]);
    let id = spawn(&ctx, "enemy");
    assert_eq!(shield(&ctx, id), (0.0, 0.0));
    ctx.registry
        .borrow_mut()
        .get_component_value_mut(id, postretro_entities::registry::ComponentKind::EntityState)
        .map(|value| {
            if let postretro_entities::registry::ComponentValue::EntityState(state) = value {
                state.set("tier", 3.0);
            }
        })
        .unwrap();
    tick(&ctx);
    assert_eq!(shield(&ctx, id).1, 30.0);
}

#[test]
fn layer_expression_results_clamp_into_literal_ranges() {
    let mut entry = layer(expr(input("tune.cap")));
    entry.delay = Some(expr(input("tune.delay")));
    entry.resist = vec![("shock", expr(input("tune.ward")))];
    let ctx = session(vec![("enemy", false, health(vec![entry]))]);
    let id = spawn(&ctx, "enemy");
    set_global(&ctx, "tune.cap", 2.0e6);
    set_global(&ctx, "tune.delay", 90_000.0);
    set_global(&ctx, "tune.ward", 70.0);
    tick(&ctx);
    let component = ctx
        .registry
        .borrow()
        .get_component::<DefenseLayersComponent>(id)
        .unwrap()
        .clone();
    assert_eq!(component.layers[0].capacity, 1.0e6);
    assert_eq!(component.layers[0].recharge_delay_ms, 60_000.0);
    assert_eq!(component.layers[0].resist[0].value, 64.0);
    set_global(&ctx, "tune.cap", -5.0);
    tick(&ctx);
    assert_eq!(shield(&ctx, id), (0.0, 0.0));
}

#[test]
fn expression_resist_applies_from_next_tick_and_clamps() {
    // Ward 0: resist 1. A write mid-tick leaves that tick's hits alone.
    let mut entry = layer(lit(100.0));
    entry.resist = vec![(
        "shock",
        expr(IrNode::Add {
            a: Box::new(mul(input("tune.ward"), constant(-0.25))),
            b: Box::new(constant(1.0)),
        }),
    )];
    let ctx = session(vec![("player", false, health(vec![entry]))]);
    let id = spawn(&ctx, "player");
    tick(&ctx);
    set_global(&ctx, "tune.ward", 2.0);
    hit(&ctx, id, 10.0, Some(0));
    assert_eq!(
        shield(&ctx, id).0,
        80.0,
        "this tick's hit uses the old resist"
    );
    tick(&ctx);
    hit(&ctx, id, 10.0, Some(0));
    assert_eq!(shield(&ctx, id).0, 70.0, "next tick: resist 0.5");
    set_global(&ctx, "tune.ward", 6.0);
    tick(&ctx);
    hit(&ctx, id, 10.0, Some(0));
    assert_eq!(
        shield(&ctx, id).0,
        70.0,
        "a negative result clamps to 0: immune"
    );
}

#[test]
fn lowered_delay_applies_against_the_original_hit() {
    let mut entry = layer(lit(100.0));
    entry.delay = Some(expr(input("tune.delay")));
    entry.rate = Some(lit(60.0));
    let ctx = session(vec![("enemy", false, health(vec![entry]))]);
    let id = spawn(&ctx, "enemy");
    tick(&ctx);
    let hit_tick = ctx.registry.borrow().host_tick();
    hit(&ctx, id, 100.0, None);
    while ctx.registry.borrow().host_tick() < hit_tick + 60 {
        tick(&ctx);
    }
    assert_eq!(shield(&ctx, id).0, 0.0);
    set_global(&ctx, "tune.delay", 500.0);
    tick(&ctx);
    assert!(
        approx(shield(&ctx, id).0, 1.0),
        "tick N+61 recharges against the hit on N"
    );
}

#[test]
fn trigger_spawned_entity_is_full_before_same_tick_hit() {
    let ctx = session(vec![(
        "enemy",
        false,
        health(vec![layer(expr(input("tune.cap")))]),
    )]);
    tick(&ctx);
    set_global(&ctx, "tune.cap", 50.0);
    let id = spawn(&ctx, "enemy");
    assert_eq!(shield(&ctx, id), (50.0, 50.0));
    hit(&ctx, id, 20.0, None);
    assert_eq!(shield(&ctx, id).0, 30.0);
    assert_eq!(
        ctx.registry
            .borrow()
            .get_component::<HealthComponent>(id)
            .unwrap()
            .current,
        100.0
    );
}

#[test]
fn per_owner_capacity_reads_the_players_seat() {
    let ctx = session(vec![(
        "player",
        true,
        health(vec![layer(expr(input("seat.cap")))]),
    )]);
    ctx.slot_table
        .borrow_mut()
        .get_mut("seat.cap")
        .unwrap()
        .set_per_seat_value(Seat(4), SlotValue::Number(120.0));
    let id = spawn(&ctx, "player");
    assert_eq!(shield(&ctx, id), (0.0, 0.0), "no seat yet: the read faults");
    ctx.registry.borrow_mut().bind_pawn_seat(id, Seat(4));
    tick(&ctx);
    assert_eq!(shield(&ctx, id).1, 120.0);
}

#[test]
fn connected_client_evaluates_no_layer_parameter() {
    let capture = LogCapture::start();
    let ctx = session(vec![(
        "enemy",
        false,
        health(vec![layer(expr(div(
            input("tune.cap"),
            input("tune.zero"),
        )))]),
    )]);
    ctx.owner_slot_writes_enabled.set(false);
    EVALUATIONS.with(|count| count.set(0));
    let id = spawn(&ctx, "enemy");
    for _ in 0..5 {
        tick(&ctx);
    }
    assert_eq!(EVALUATIONS.with(|count| count.get()), 0);
    assert_eq!(shield(&ctx, id), (0.0, 0.0));
    capture.assert_not_logged(log::Level::Warn, "faulted");
}

#[test]
fn literal_layer_parameters_never_evaluate_per_tick() {
    let mut literal = layer(lit(100.0));
    literal.delay = Some(lit(0.0));
    literal.rate = Some(lit(60.0));
    let mut expression = layer(lit(100.0));
    expression.delay = Some(lit(0.0));
    expression.rate = Some(expr(input("tune.rate")));
    let ctx = session(vec![
        ("literal", false, health(vec![literal])),
        ("expression", false, health(vec![expression])),
    ]);
    let literal_ids: Vec<EntityId> = (0..3).map(|_| spawn(&ctx, "literal")).collect();
    for &id in &literal_ids {
        hit(&ctx, id, 100.0, None);
    }
    EVALUATIONS.with(|count| count.set(0));
    for _ in 0..600 {
        tick(&ctx);
    }
    assert_eq!(
        EVALUATIONS.with(|count| count.get()),
        0,
        "literal parameters never evaluate"
    );
    assert_eq!(
        shield(&ctx, literal_ids[0]),
        (100.0, 100.0),
        "recharge still ramps at the literal rate"
    );
    let expression_ids: Vec<EntityId> = (0..2).map(|_| spawn(&ctx, "expression")).collect();
    EVALUATIONS.with(|count| count.set(0));
    tick(&ctx);
    assert_eq!(
        EVALUATIONS.with(|count| count.get()),
        expression_ids.len(),
        "one evaluation per entity of the expression descriptor per tick, none for the rest"
    );
}

#[test]
fn layer_step_visits_only_layered_entities() {
    let ctx = session(vec![("enemy", false, health(vec![layer(lit(10.0))]))]);
    for _ in 0..4 {
        spawn(&ctx, "enemy");
    }
    for _ in 0..6 {
        let mut registry = ctx.registry.borrow_mut();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                HealthComponent::from_descriptor(&HealthDescriptor {
                    max: 10.0,
                    ..Default::default()
                }),
            )
            .unwrap();
    }
    for _ in 0..3 {
        VISITED.with(|count| count.set(0));
        tick(&ctx);
        assert_eq!(VISITED.with(|count| count.get()), 4);
    }
}

#[test]
fn layer_step_performs_zero_heap_allocations() {
    let mut entry = layer(expr(input("tune.cap")));
    entry.delay = Some(expr(input("tune.delay")));
    entry.rate = Some(expr(input("tune.rate")));
    entry.resist = vec![(
        "shock",
        expr(IrNode::Add {
            a: Box::new(input("@state.tier")),
            b: Box::new(input("tune.ward")),
        }),
    )];
    let ctx = session(vec![("enemy", false, health(vec![entry]))]);
    set_global(&ctx, "tune.delay", 0.0);
    set_global(&ctx, "tune.ward", 1.0);
    let ids: Vec<EntityId> = (0..8).map(|_| spawn(&ctx, "enemy")).collect();
    for &id in &ids {
        hit(&ctx, id, 50.0, None);
    }
    // Warm-up tick, then the measured window.
    tick(&ctx);
    // Positive control: the probe sees an allocation inside a window.
    let control = crate::alloc_probe::AllocSnapshot::arm();
    let boxed = std::hint::black_box(Box::new(7_u64));
    assert!(
        control.allocs_since() >= 1,
        "the counting allocator is installed"
    );
    drop(boxed);
    let window = crate::alloc_probe::AllocSnapshot::arm();
    tick(&ctx);
    let allocs = window.allocs_since();
    assert_eq!(allocs, 0, "evaluation, clamp and recharge allocate nothing");
    assert!(
        shield(&ctx, ids[0]).0 > 50.0,
        "the measured tick did real work"
    );
}
