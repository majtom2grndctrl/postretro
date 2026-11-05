// Defense-layer lifecycle on the host: revive and respawn against the
// last-hit stamp, hot reload by layer name through the real refresh planner,
// health writes that leave layers alone, and the seat-bind, carry and
// join-seed refills.
// See: context/lib/scripting.md §11 (Defense layers) · plans E16 §Pinned orderings

use std::collections::BTreeSet;

use postretro_combat_model::{CarriedState, restore_carried_layers};
use postretro_entities::ScriptCtx;
use postretro_entities::components::defense_layers::{
    DefenseLayersComponent, mark_layers_for_join_seed, refill_layers_on_respawn,
};
use postretro_entities::components::grant::{grant_health, grant_layer};
use postretro_entities::components::health::{HealthComponent, set_health_absolute};
use postretro_entities::data_descriptors::EntityTypeDescriptor;
use postretro_entities::provenance::{
    DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath,
};
use postretro_entities::registry::EntityId;
use postretro_entities::slot_table::SlotValue;
use postretro_foundation::defense::HandleRef;
use postretro_foundation::{
    AuthoredHandle, DefenseLayerEntry, HealthDescriptor, IrNode, NumberOrIr, Seat,
};
use postretro_scripting_core::refresh_plan::{
    apply_descriptor_refresh_plan, plan_descriptor_refresh,
};
use postretro_test_log_capture::LogCapture;

use super::defense_parameter_tests::{
    approx, constant, div, expr, hit, input, lit, mul, session, set_global, spawn, tick,
};
use super::defense_parameters::DefenseParameters;

fn entry(name: &str, capacity: NumberOrIr, rate: f32) -> DefenseLayerEntry {
    DefenseLayerEntry {
        layer: AuthoredHandle::from_handle(&HandleRef::DefenseLayer {
            name: name.to_string(),
        }),
        capacity,
        recharge_delay_ms: Some(lit(3_000.0)),
        recharge_rate: Some(lit(rate)),
        resist: Default::default(),
    }
}

fn health(layers: Vec<DefenseLayerEntry>) -> HealthDescriptor {
    HealthDescriptor {
        max: 100.0,
        layers,
        ..Default::default()
    }
}

fn layers(ctx: &ScriptCtx, id: EntityId) -> DefenseLayersComponent {
    ctx.registry
        .borrow()
        .get_component::<DefenseLayersComponent>(id)
        .unwrap()
        .clone()
}

/// `(value, capacity)` of `id`'s layer of kind `name`.
fn layer(ctx: &ScriptCtx, id: EntityId, name: &str) -> (f32, f32) {
    let component = layers(ctx, id);
    let layer = component.layer(name).unwrap();
    (layer.value, layer.capacity)
}

fn now(ctx: &ScriptCtx) -> u64 {
    ctx.registry.borrow().host_tick()
}

/// Tick until the host tick reads `target`.
fn tick_to(ctx: &ScriptCtx, target: u64) {
    while now(ctx) < target {
        tick(ctx);
    }
}

fn latch(ctx: &ScriptCtx, id: EntityId) {
    let mut registry = ctx.registry.borrow_mut();
    let mut health = registry
        .get_component::<HealthComponent>(id)
        .unwrap()
        .clone();
    health.current = 0.0;
    health.death_handled = true;
    registry.set_component(id, health).unwrap();
}

// R10 / R-P14: latched time counts toward the delay, recharge needs a tick
// that begins above zero health, and a hit on a latched entity restamps.
#[test]
fn revived_pawn_recharges_on_first_live_tick() {
    for second_hit in [false, true] {
        let ctx = session(vec![(
            "pawn",
            false,
            health(vec![entry("shield", lit(100.0), 60.0)]),
        )]);
        let id = spawn(&ctx, "pawn");
        tick(&ctx);
        hit(&ctx, id, 100.0, None);
        let n = now(&ctx);
        tick_to(&ctx, n + 10);
        latch(&ctx, id);
        if second_hit {
            tick_to(&ctx, n + 250);
            hit(&ctx, id, 5.0, None);
        }
        tick_to(&ctx, n + 300);
        // Revived in tick N+300's assist stage, after its tick start.
        set_health_absolute(&mut ctx.registry.borrow_mut(), id, 50.0);
        assert_eq!(layer(&ctx, id, "shield").0, 0.0, "nothing while latched");
        let first = if second_hit { n + 430 } else { n + 301 };
        tick_to(&ctx, first - 1);
        assert_eq!(layer(&ctx, id, "shield").0, 0.0, "second hit {second_hit}");
        tick(&ctx);
        assert!(
            approx(layer(&ctx, id, "shield").0, 1.0),
            "tick {first} adds rate × dt (second hit {second_hit})"
        );
    }
}

// R12 / R-P27: respawn refills at the capacity it evaluates and neither sets
// nor clears the stamp.
#[test]
fn respawn_neither_sets_nor_clears_the_last_hit_stamp() {
    let ctx = session(vec![(
        "pawn",
        false,
        health(vec![entry("shield", expr(input("tune.cap")), 60.0)]),
    )]);
    let id = spawn(&ctx, "pawn");
    tick(&ctx);
    hit(&ctx, id, 200.0, None);
    let n = now(&ctx);
    let stamp = layers(&ctx, id).last_hit_tick;
    assert_eq!(stamp, Some(n));
    latch(&ctx, id);
    tick_to(&ctx, n + 60);
    {
        let mut registry = ctx.registry.borrow_mut();
        set_health_absolute(&mut registry, id, 100.0);
        refill_layers_on_respawn(&mut registry, id);
    }
    assert_eq!(layer(&ctx, id, "shield"), (100.0, 100.0));
    assert_eq!(
        layers(&ctx, id).last_hit_tick,
        stamp,
        "respawn keeps the stamp"
    );
    set_global(&ctx, "tune.cap", 150.0);
    tick(&ctx);
    assert_eq!(layer(&ctx, id, "shield"), (100.0, 150.0));
    tick_to(&ctx, n + 179);
    assert_eq!(
        layer(&ctx, id, "shield").0,
        100.0,
        "the delay counts from N"
    );
    tick(&ctx);
    assert!(approx(layer(&ctx, id, "shield").0, 101.0));
}

// R14 / R-P4: a respawn evaluates capacity itself and refills to it; a hit
// later in the tick a store write lowers capacity uses the capacity in force.
#[test]
fn respawn_fills_to_capacity_in_force() {
    let ctx = session(vec![
        (
            "pawn",
            false,
            health(vec![entry("shield", expr(input("tune.cap")), 0.0)]),
        ),
        (
            "enemy",
            false,
            health(vec![entry("shield", expr(input("tune.ward")), 0.0)]),
        ),
    ]);
    set_global(&ctx, "tune.ward", 100.0);
    let pawn = spawn(&ctx, "pawn");
    let enemy = spawn(&ctx, "enemy");
    tick(&ctx);
    hit(&ctx, pawn, 70.0, None);
    hit(&ctx, enemy, 20.0, None);
    // Tick N: the store writes, then a 30-point hit on the enemy.
    set_global(&ctx, "tune.cap", 150.0);
    set_global(&ctx, "tune.ward", 40.0);
    hit(&ctx, enemy, 30.0, None);
    assert_eq!(layer(&ctx, enemy, "shield"), (50.0, 100.0));
    // Tick N+1 starts with the pawn's respawn.
    refill_layers_on_respawn(&mut ctx.registry.borrow_mut(), pawn);
    tick(&ctx);
    assert_eq!(layer(&ctx, pawn, "shield"), (150.0, 150.0));
    assert_eq!(layer(&ctx, enemy, "shield"), (40.0, 40.0));
}

/// Swap `name`'s descriptor for `health` the way a hot reload does: plan
/// against the live registry, apply, commit, then rebind the parameters.
fn hot_reload(ctx: &ScriptCtx, name: &str, health: HealthDescriptor) {
    let old = ctx.data_registry.borrow().entities.clone();
    let mut new = old.clone();
    let edited: &mut EntityTypeDescriptor = new
        .iter_mut()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some(name))
        .unwrap();
    edited.health = Some(health);
    let plan = plan_descriptor_refresh(&old, &new, &ctx.registry.borrow());
    apply_descriptor_refresh_plan(&plan, &mut ctx.registry.borrow_mut()).unwrap();
    let mut data = ctx.data_registry.borrow_mut();
    for descriptor in new {
        data.upsert_entity_type(descriptor);
    }
    drop(data);
    DefenseParameters::install(ctx);
}

fn spawn_owned(ctx: &ScriptCtx, name: &str) -> EntityId {
    let id = spawn(ctx, name);
    ctx.registry
        .borrow_mut()
        .set_component(
            id,
            DescriptorProvenance {
                canonical_name: name.to_string(),
                owned_components: BTreeSet::from([DescriptorComponentKind::Health]),
                map_overrides: Default::default(),
                spawn_path: DescriptorSpawnPath::MapPlacement,
            },
        )
        .unwrap();
    id
}

// L3 / R-P8: hot reload keeps values, last good parameters and the stamp by
// layer name, in the edit's order.
#[test]
fn hot_reload_keeps_layer_values_and_stamp_by_name() {
    let start = || {
        health(vec![
            entry("armor", lit(50.0), 60.0),
            entry("shield", lit(100.0), 60.0),
        ])
    };
    let edits = [
        // The edit reorders and lowers shield capacity to 60.
        health(vec![
            entry("shield", lit(60.0), 60.0),
            entry("armor", lit(50.0), 60.0),
        ]),
        // An expression that evaluates to 60, then faults.
        health(vec![
            entry(
                "shield",
                expr(div(input("tune.cap"), input("tune.ward"))),
                60.0,
            ),
            entry("armor", lit(50.0), 60.0),
        ]),
        // An expression that faults on its first evaluation.
        health(vec![
            entry(
                "shield",
                expr(div(input("tune.cap"), input("tune.zero"))),
                60.0,
            ),
            entry("armor", lit(50.0), 60.0),
        ]),
    ];
    for (variant, edit) in edits.into_iter().enumerate() {
        let capture = LogCapture::start();
        let ctx = session(vec![("crate", false, start())]);
        set_global(&ctx, "tune.cap", 60.0);
        set_global(&ctx, "tune.ward", 1.0);
        let id = spawn_owned(&ctx, "crate");
        tick(&ctx);
        hit(&ctx, id, 130.0, None);
        let n = now(&ctx);
        grant_layer(&mut ctx.registry.borrow_mut(), id, "armor", Some(15.0));
        assert_eq!(layer(&ctx, id, "shield"), (20.0, 100.0));
        tick_to(&ctx, n + 60);
        hot_reload(&ctx, "crate", edit);
        tick(&ctx);
        let names: Vec<_> = layers(&ctx, id)
            .layers
            .iter()
            .map(|layer| layer.name.clone())
            .collect();
        assert_eq!(names, ["shield", "armor"], "the edit's order");
        assert_eq!(layers(&ctx, id).last_hit_tick, Some(n), "the stamp stays");
        assert_eq!(
            layer(&ctx, id, "armor"),
            (15.0, 50.0),
            "armor keeps its value"
        );
        match variant {
            0 => assert_eq!(layer(&ctx, id, "shield"), (20.0, 60.0)),
            1 => {
                assert_eq!(layer(&ctx, id, "shield"), (20.0, 60.0));
                set_global(&ctx, "tune.ward", 0.0);
                tick(&ctx);
                assert_eq!(layer(&ctx, id, "shield"), (20.0, 60.0), "last good 60");
            }
            _ => {
                assert_eq!(layer(&ctx, id, "shield"), (20.0, 100.0), "pre-reload 100");
                capture.assert_logged_once(
                    log::Level::Warn,
                    "descriptor `crate` `components.health.layers[0].capacity` faulted",
                );
            }
        }
        // The first recharge is at N+180, counted from the kept stamp.
        tick_to(&ctx, n + 179);
        assert_eq!(layer(&ctx, id, "shield").0, 20.0);
        tick(&ctx);
        assert!(approx(layer(&ctx, id, "shield").0, 21.0));
        let armor = layer(&ctx, id, "armor").0;
        hit(&ctx, id, 10.0, None);
        assert!(
            approx(layer(&ctx, id, "shield").0, 11.0),
            "shield drains first"
        );
        assert_eq!(layer(&ctx, id, "armor").0, armor);
    }
}

// R-P15: a reload that adds a layer evaluates its parameters as spawn does,
// then fills it.
#[test]
fn hot_reload_evaluates_and_fills_an_added_layer() {
    let ctx = session(vec![(
        "crate",
        false,
        health(vec![entry("shield", lit(100.0), 0.0)]),
    )]);
    set_global(&ctx, "tune.ward", 5.0);
    let id = spawn_owned(&ctx, "crate");
    tick(&ctx);
    hit(&ctx, id, 80.0, None);
    hot_reload(
        &ctx,
        "crate",
        health(vec![
            entry("shield", lit(100.0), 0.0),
            entry("armor", expr(mul(input("tune.ward"), constant(10.0))), 0.0),
        ]),
    );
    tick(&ctx);
    assert_eq!(layer(&ctx, id, "armor"), (50.0, 50.0));
    assert_eq!(layer(&ctx, id, "shield"), (20.0, 100.0));
}

// L4: health writes leave layers alone; respawn fills them; a carry restores
// them only with carried health.
#[test]
fn health_writes_leave_layers_alone() {
    let ctx = session(vec![(
        "pawn",
        false,
        health(vec![entry("shield", lit(100.0), 0.0)]),
    )]);
    let id = spawn(&ctx, "pawn");
    hit(&ctx, id, 60.0, None);
    {
        let mut registry = ctx.registry.borrow_mut();
        set_health_absolute(&mut registry, id, 10.0);
        grant_health(&mut registry, id, 20.0);
    }
    assert_eq!(layer(&ctx, id, "shield"), (40.0, 100.0));
    refill_layers_on_respawn(&mut ctx.registry.borrow_mut(), id);
    assert_eq!(layer(&ctx, id, "shield"), (100.0, 100.0));

    for (health_current, expected) in [(Some(80.0), 35.0), (Some(0.0), 100.0), (None, 100.0)] {
        let carried = CarriedState {
            health_current,
            layers: vec![("shield".to_string(), 35.0)],
            ..Default::default()
        };
        let fresh = spawn(&ctx, "pawn");
        restore_carried_layers(Some(&carried), &mut ctx.registry.borrow_mut(), fresh);
        tick(&ctx);
        assert_eq!(
            layer(&ctx, fresh, "shield").0,
            expected,
            "{health_current:?}"
        );
    }
}

/// A player descriptor whose shield capacity reads `seat.cap` (+ `plus`).
fn per_owner_pawn(plus: f32, rate: f32) -> ScriptCtx {
    let capacity = if plus == 0.0 {
        input("seat.cap")
    } else {
        IrNode::Add {
            a: Box::new(input("seat.cap")),
            b: Box::new(constant(plus)),
        }
    };
    session(vec![(
        "pawn",
        true,
        health(vec![entry("shield", expr(capacity), rate)]),
    )])
}

fn set_seat_cap(ctx: &ScriptCtx, seat: Seat, value: f32) {
    ctx.slot_table
        .borrow_mut()
        .get_mut("seat.cap")
        .unwrap()
        .set_per_seat_value(seat, SlotValue::Number(value));
}

fn spawn_seated(ctx: &ScriptCtx, seat: Seat) -> EntityId {
    let id = spawn(ctx, "pawn");
    ctx.registry.borrow_mut().bind_pawn_seat(id, seat);
    id
}

// L5 / R-P6 (sim half): an unseated spawn waits silently for its seat; the
// bind evaluates per-owner capacity, fills, and applies a carried value.
#[test]
fn seat_bind_evaluates_per_owner_capacity_and_restores_carry() {
    let capture = LogCapture::start();
    let ctx = per_owner_pawn(50.0, 60.0);
    set_seat_cap(&ctx, Seat(1), 100.0);
    set_seat_cap(&ctx, Seat(2), 100.0);
    set_seat_cap(&ctx, Seat(3), 100.0);
    let fresh = spawn(&ctx, "pawn");
    assert_eq!(
        layer(&ctx, fresh, "shield"),
        (0.0, 0.0),
        "waits for its seat"
    );
    ctx.registry.borrow_mut().bind_pawn_seat(fresh, Seat(1));
    assert_eq!(layer(&ctx, fresh, "shield"), (150.0, 150.0));

    for (seat, health_current, expected) in
        [(Seat(2), Some(80.0), 120.0), (Seat(3), Some(0.0), 150.0)]
    {
        let carried = CarriedState {
            health_current,
            layers: vec![("shield".to_string(), 120.0)],
            ..Default::default()
        };
        let pawn = spawn(&ctx, "pawn");
        restore_carried_layers(Some(&carried), &mut ctx.registry.borrow_mut(), pawn);
        ctx.registry.borrow_mut().bind_pawn_seat(pawn, seat);
        assert_eq!(layer(&ctx, pawn, "shield"), (expected, 150.0), "{seat:?}");
    }
    capture.assert_not_logged(log::Level::Warn, "faulted");
}

// L7 sim half / R-P16, R-P24, R-P25, R-P28: a landed seed redoes the spawn
// fill at the next tick start, before its hits, only for a layer full at
// its pre-seed capacity, however it got full.
#[test]
fn join_seed_refills_only_layers_full_at_pre_seed_capacity() {
    let seed_to = |ctx: &ScriptCtx, pawn: EntityId, value: f32| {
        set_seat_cap(ctx, Seat(1), value);
        mark_layers_for_join_seed(&mut ctx.registry.borrow_mut(), pawn);
    };
    let seed = |ctx: &ScriptCtx, pawn: EntityId| seed_to(ctx, pawn, 100.0);
    // R-P16: full 50 / 50 refills to 150; 30 / 50 keeps 30.
    for (pre_hit, expected) in [(None, 150.0), (Some(20.0), 30.0)] {
        let ctx = per_owner_pawn(50.0, 0.0);
        let pawn = spawn_seated(&ctx, Seat(1));
        assert_eq!(layer(&ctx, pawn, "shield"), (50.0, 50.0));
        if let Some(amount) = pre_hit {
            hit(&ctx, pawn, amount, None);
        }
        tick(&ctx);
        seed(&ctx, pawn);
        tick(&ctx);
        assert_eq!(
            layer(&ctx, pawn, "shield"),
            (expected, 150.0),
            "{pre_hit:?}"
        );
    }
    // R-P24: the refill lands before that tick's first hit.
    let ctx = per_owner_pawn(50.0, 0.0);
    let pawn = spawn_seated(&ctx, Seat(1));
    seed(&ctx, pawn);
    tick(&ctx);
    hit(&ctx, pawn, 20.0, None);
    assert_eq!(layer(&ctx, pawn, "shield"), (130.0, 150.0));
    // R-P25: hit, then a grant refill, before the seed.
    let ctx = per_owner_pawn(50.0, 0.0);
    let pawn = spawn_seated(&ctx, Seat(1));
    hit(&ctx, pawn, 20.0, None);
    grant_layer(&mut ctx.registry.borrow_mut(), pawn, "shield", None);
    seed(&ctx, pawn);
    tick(&ctx);
    assert_eq!(layer(&ctx, pawn, "shield"), (150.0, 150.0));
    // R-P28: 0 / 0 is full; the refill beats the recharge.
    let ctx = per_owner_pawn(0.0, 60.0);
    let pawn = spawn_seated(&ctx, Seat(1));
    assert_eq!(layer(&ctx, pawn, "shield"), (0.0, 0.0));
    seed_to(&ctx, pawn, 150.0);
    tick(&ctx);
    assert_eq!(layer(&ctx, pawn, "shield"), (150.0, 150.0), "not 1 / 150");
}
