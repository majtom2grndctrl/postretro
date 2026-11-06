// Drain-before-health and tick-start recharge for defense layers, driven the
// way the fixed tick drives them: advance the host tick, recharge, then hit.
// See: context/lib/scripting.md §11 (Defense layers)

use postretro_entities::components::defense_layers::DefenseLayersComponent;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
};
use postretro_entities::registry::{EntityId, EntityRegistry, Transform};
use postretro_foundation::defense::HandleRef;
use postretro_foundation::{
    AuthoredHandle, DamagePayload, DefenseLayerEntry, HealthDescriptor, NumberOrIr,
};

use super::defense_layers::recharge_defense_layers;

fn entry(name: &str, capacity: f32, delay_ms: Option<f32>, rate: Option<f32>) -> DefenseLayerEntry {
    DefenseLayerEntry {
        layer: AuthoredHandle::from_handle(&HandleRef::DefenseLayer {
            name: name.to_string(),
        }),
        capacity: NumberOrIr::Literal(capacity),
        recharge_delay_ms: delay_ms.map(NumberOrIr::Literal),
        recharge_rate: rate.map(NumberOrIr::Literal),
        resist: Default::default(),
    }
}

fn spawn(registry: &mut EntityRegistry, health: f32, layers: Vec<DefenseLayerEntry>) -> EntityId {
    let descriptor = HealthDescriptor {
        max: health,
        layers,
        ..Default::default()
    };
    let id = registry.spawn(Transform::default());
    registry
        .set_component(id, HealthComponent::from_descriptor(&descriptor))
        .unwrap();
    if let Some(layers) = DefenseLayersComponent::from_descriptor(&descriptor) {
        registry.set_component(id, layers).unwrap();
    }
    id
}

fn hit(registry: &mut EntityRegistry, id: EntityId, amount: f32) {
    apply_damage_with_context(
        registry,
        id,
        &DamagePayload {
            amount,
            ..Default::default()
        },
        DamageContext::new("test.layers", DamageProducer::InTick),
    );
}

/// Start the next fixed tick the way `begin_tick` and the frame loop do.
fn start_tick(registry: &mut EntityRegistry) {
    registry.advance_host_tick();
    recharge_defense_layers(registry);
}

fn values(registry: &EntityRegistry, id: EntityId) -> Vec<f32> {
    registry
        .get_component::<DefenseLayersComponent>(id)
        .unwrap()
        .layers
        .iter()
        .map(|layer| layer.value)
        .collect()
}

fn health(registry: &EntityRegistry, id: EntityId) -> f32 {
    registry
        .get_component::<HealthComponent>(id)
        .unwrap()
        .current
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1.0e-3
}

#[test]
fn untyped_hit_spills_in_layer_order() {
    let mut registry = EntityRegistry::new();
    let id = spawn(
        &mut registry,
        100.0,
        vec![
            entry("shield", 30.0, None, None),
            entry("armor", 50.0, None, None),
        ],
    );
    start_tick(&mut registry);
    hit(&mut registry, id, 100.0);
    assert_eq!(values(&registry, id), [0.0, 0.0]);
    assert_eq!(health(&registry, id), 80.0);
    let dispatch = registry.take_impact_dispatches().pop().unwrap();
    assert_eq!(
        (dispatch.raw, dispatch.amount, dispatch.absorbed),
        (100.0, 100.0, 80.0)
    );
    assert!(dispatch.broke);
}

#[test]
fn fully_absorbed_hit_dispatches_and_never_latches() {
    let mut registry = EntityRegistry::new();
    let id = spawn(
        &mut registry,
        100.0,
        vec![entry("shield", 30.0, None, None)],
    );
    start_tick(&mut registry);
    hit(&mut registry, id, 20.0);
    assert_eq!(values(&registry, id), [10.0]);
    assert_eq!(health(&registry, id), 100.0);
    let dispatch = registry.take_impact_dispatches().pop().unwrap();
    assert_eq!((dispatch.amount, dispatch.absorbed), (20.0, 20.0));
    assert!(!dispatch.broke);
    assert_eq!(dispatch.health_before, dispatch.health_after);
}

#[test]
fn empty_layer_is_skipped_and_never_breaks() {
    let mut registry = EntityRegistry::new();
    let id = spawn(
        &mut registry,
        100.0,
        vec![entry("shield", 30.0, None, None)],
    );
    start_tick(&mut registry);
    hit(&mut registry, id, 30.0);
    assert!(registry.take_impact_dispatches().pop().unwrap().broke);
    hit(&mut registry, id, 20.0);
    let dispatch = registry.take_impact_dispatches().pop().unwrap();
    assert!(!dispatch.broke);
    assert_eq!(dispatch.absorbed, 0.0);
    assert_eq!(health(&registry, id), 80.0);
}

#[test]
fn two_hits_on_one_tick_drain_in_order_and_only_the_emptying_hit_breaks() {
    let mut registry = EntityRegistry::new();
    let id = spawn(
        &mut registry,
        100.0,
        vec![entry("shield", 30.0, None, None)],
    );
    start_tick(&mut registry);
    hit(&mut registry, id, 20.0);
    hit(&mut registry, id, 20.0);
    let dispatches = registry.take_impact_dispatches();
    assert_eq!(dispatches.len(), 2);
    assert!(!dispatches[0].broke);
    assert!(dispatches[1].broke);
    assert_eq!(values(&registry, id), [0.0]);
    assert_eq!(health(&registry, id), 90.0);
}

#[test]
fn layer_recharges_after_delay_at_rate_and_stops_at_capacity() {
    let mut registry = EntityRegistry::new();
    let id = spawn(
        &mut registry,
        100.0,
        vec![entry("shield", 30.0, Some(3_000.0), Some(60.0))],
    );
    start_tick(&mut registry);
    let hit_tick = registry.host_tick();
    hit(&mut registry, id, 30.0);
    // Nothing through tick N+179; tick N+180 adds rate × dt before any hit.
    while registry.host_tick() < hit_tick + 179 {
        start_tick(&mut registry);
    }
    assert_eq!(values(&registry, id), [0.0]);
    start_tick(&mut registry);
    assert!(
        approx(values(&registry, id)[0], 1.0),
        "{:?}",
        values(&registry, id)
    );
    for _ in 0..200 {
        start_tick(&mut registry);
    }
    assert_eq!(values(&registry, id), [30.0]);
}

#[test]
fn delay_expiry_counts_whole_ticks_after_long_uptime() {
    let mut registry = EntityRegistry::new();
    let id = spawn(
        &mut registry,
        100.0,
        vec![entry("shield", 30.0, Some(3_000.0), Some(60.0))],
    );
    for _ in 0..100_000 {
        registry.advance_host_tick();
    }
    start_tick(&mut registry);
    let hit_tick = registry.host_tick();
    hit(&mut registry, id, 30.0);
    while registry.host_tick() < hit_tick + 179 {
        start_tick(&mut registry);
    }
    assert_eq!(values(&registry, id), [0.0]);
    start_tick(&mut registry);
    assert!(approx(values(&registry, id)[0], 1.0));
}

#[test]
fn any_positive_raw_hit_restarts_every_layer_delay() {
    let mut registry = EntityRegistry::new();
    let id = spawn(
        &mut registry,
        100.0,
        vec![
            entry("shield", 30.0, Some(500.0), Some(60.0)),
            entry("armor", 30.0, Some(500.0), Some(60.0)),
        ],
    );
    start_tick(&mut registry);
    hit(&mut registry, id, 40.0);
    for _ in 0..40 {
        start_tick(&mut registry);
    }
    let ramping = values(&registry, id);
    assert!(ramping[0] > 0.0 && ramping[0] < 30.0, "{ramping:?}");
    // A hit fully absorbed by the shield restarts the armor's delay too.
    hit(&mut registry, id, 1.0);
    let held = values(&registry, id);
    for _ in 0..29 {
        start_tick(&mut registry);
    }
    assert_eq!(
        values(&registry, id),
        held,
        "nothing recharges inside the restarted delay"
    );
    start_tick(&mut registry);
    assert!(values(&registry, id)[0] > held[0]);
}

#[test]
fn rate_zero_never_recharges_and_delay_zero_starts_next_tick() {
    let mut registry = EntityRegistry::new();
    let armor = spawn(
        &mut registry,
        100.0,
        vec![entry("armor", 30.0, Some(0.0), None)],
    );
    let shield = spawn(
        &mut registry,
        100.0,
        vec![entry("shield", 30.0, Some(0.0), Some(60.0))],
    );
    start_tick(&mut registry);
    hit(&mut registry, armor, 10.0);
    hit(&mut registry, shield, 10.0);
    start_tick(&mut registry);
    assert_eq!(values(&registry, armor), [20.0]);
    assert!(approx(values(&registry, shield)[0], 21.0));
}

#[test]
fn omitted_recharge_fields_default_to_3000ms_and_never() {
    let layers = DefenseLayersComponent::from_descriptor(&HealthDescriptor {
        max: 100.0,
        layers: vec![entry("shield", 30.0, None, None)],
        ..Default::default()
    })
    .unwrap();
    assert_eq!(layers.layers[0].recharge_delay_ms, 3_000.0);
    assert_eq!(layers.layers[0].recharge_rate, 0.0);
}

#[test]
fn expiry_tick_recharges_before_that_ticks_hit() {
    let mut registry = EntityRegistry::new();
    let id = spawn(
        &mut registry,
        100.0,
        vec![entry("shield", 30.0, Some(500.0), Some(60.0))],
    );
    start_tick(&mut registry);
    let hit_tick = registry.host_tick();
    hit(&mut registry, id, 30.0);
    while registry.host_tick() < hit_tick + 30 {
        start_tick(&mut registry);
    }
    // Tick N+30 recharged 1 point first; its hit then drains it and restamps.
    assert!(approx(values(&registry, id)[0], 1.0));
    hit(&mut registry, id, 1.0);
    assert!(approx(values(&registry, id)[0], 0.0));
    for _ in 0..29 {
        start_tick(&mut registry);
    }
    assert!(approx(values(&registry, id)[0], 0.0));
    start_tick(&mut registry);
    assert!(approx(values(&registry, id)[0], 1.0));
}

#[test]
fn zero_health_entities_clamp_but_never_recharge() {
    let mut registry = EntityRegistry::new();
    let id = spawn(
        &mut registry,
        10.0,
        vec![entry("shield", 30.0, Some(0.0), Some(60.0))],
    );
    start_tick(&mut registry);
    hit(&mut registry, id, 40.0);
    assert_eq!(health(&registry, id), 0.0);
    for _ in 0..10 {
        start_tick(&mut registry);
    }
    assert_eq!(values(&registry, id), [0.0]);
}

#[test]
fn layer_step_visits_only_layered_entities() {
    let mut registry = EntityRegistry::new();
    let layered = spawn(
        &mut registry,
        100.0,
        vec![entry("shield", 30.0, None, Some(60.0))],
    );
    let plain = spawn(&mut registry, 100.0, Vec::new());
    assert!(
        registry
            .get_component::<DefenseLayersComponent>(layered)
            .is_ok()
    );
    assert!(
        registry
            .get_component::<DefenseLayersComponent>(plain)
            .is_err()
    );
    start_tick(&mut registry);
    hit(&mut registry, plain, 10.0);
    assert_eq!(health(&registry, plain), 90.0);
}
