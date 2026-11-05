// Typed drain through the chokepoint: the mod-wide matrix, pool-local
// resistances, the default type and the impact facts each hit reports.
// See: context/lib/scripting.md §11 (Defense layers)

use std::collections::BTreeMap;
use std::sync::Arc;

use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::defense_layers::DefenseLayersComponent;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, HealthComponent, ImpactDispatch, apply_damage_with_context,
};
use postretro_entities::registry::{EntityId, EntityRegistry, Transform};
use postretro_foundation::defense::{DefenseVocabulary, HandleRef};
use postretro_foundation::{
    AuthoredHandle, BehaviorActivityDescriptor, BehaviorGraphDescriptor, BehaviorGraphEnvelope,
    DamagePayload, DefenseLayerEntry, HealthDescriptor, MotionVerb, NumberOrIr,
};

const TYPES: [&str; 7] = [
    "ballistic",
    "plasma",
    "explosive",
    "shock",
    "corrosive",
    "flechette",
    "solid",
];

/// The brief's dev vocabulary plus `solid` (`{ shield: 1, armor: 0 }`).
fn vocabulary(default: Option<&str>) -> Arc<DefenseVocabulary> {
    let layer = |name: &str| serde_json::json!({ "kind": "defenseLayer", "name": name });
    let ty = |name: &str, matrix: serde_json::Value| serde_json::json!({ "kind": "damageType", "name": name, "matrix": matrix });
    let layers = serde_json::json!([layer("shield"), layer("armor")]);
    let types = serde_json::json!([
        ty("ballistic", serde_json::json!({ "armor": 0.8 })),
        ty("plasma", serde_json::json!({ "shield": 1.5, "armor": 0.5 })),
        ty(
            "explosive",
            serde_json::json!({ "armor": 1.5, "health": 1.25, "shield": 0.75 })
        ),
        ty(
            "shock",
            serde_json::json!({ "shield": 2, "armor": 0.5, "health": 0.75 })
        ),
        ty(
            "corrosive",
            serde_json::json!({ "shield": 0.5, "armor": 2 })
        ),
        ty(
            "flechette",
            serde_json::json!({ "shield": "bypass", "armor": 0 })
        ),
        ty("solid", serde_json::json!({ "shield": 1, "armor": 0 })),
    ]);
    let default = default.map(|name| serde_json::json!({ "kind": "damageType", "name": name }));
    Arc::new(
        DefenseVocabulary::from_manifest(Some(&layers), Some(&types), default.as_ref()).unwrap(),
    )
}

fn type_index(name: &str) -> Option<u16> {
    TYPES
        .iter()
        .position(|ty| *ty == name)
        .map(|index| index as u16)
}

fn world(default: Option<&str>) -> EntityRegistry {
    let mut registry = EntityRegistry::new();
    registry.set_defense_vocabulary(vocabulary(default));
    registry.advance_host_tick();
    registry
}

fn resist(entries: &[(&str, f32)]) -> BTreeMap<String, NumberOrIr> {
    entries
        .iter()
        .map(|(ty, value)| (ty.to_string(), NumberOrIr::Literal(*value)))
        .collect()
}

fn layer(name: &str, capacity: f32, resists: &[(&str, f32)]) -> DefenseLayerEntry {
    DefenseLayerEntry {
        layer: AuthoredHandle::from_handle(&HandleRef::DefenseLayer {
            name: name.to_string(),
        }),
        capacity: NumberOrIr::Literal(capacity),
        recharge_delay_ms: None,
        recharge_rate: None,
        resist: resist(resists),
    }
}

fn spawn(
    registry: &mut EntityRegistry,
    health: f32,
    layers: Vec<DefenseLayerEntry>,
    health_resist: &[(&str, f32)],
) -> EntityId {
    let descriptor = HealthDescriptor {
        max: health,
        layers,
        resist: resist(health_resist),
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

/// Set a layer's current value, as an earlier fight would have left it.
fn set_layer(registry: &mut EntityRegistry, id: EntityId, index: usize, value: f32) {
    let mut layers = registry
        .get_component::<DefenseLayersComponent>(id)
        .unwrap()
        .clone();
    layers.layers[index].value = value;
    registry.set_component(id, layers).unwrap();
}

fn hit(
    registry: &mut EntityRegistry,
    id: EntityId,
    amount: f32,
    ty: Option<&str>,
) -> ImpactDispatch {
    let mut context = DamageContext::new("test.attacker", DamageProducer::InTick);
    context.attacker = Some(EntityId::from_raw(99));
    apply_damage_with_context(
        registry,
        id,
        &DamagePayload {
            amount,
            damage_type: ty.and_then(type_index),
            ..Default::default()
        },
        context,
    );
    registry
        .take_impact_dispatches()
        .pop()
        .expect("the hit dispatches")
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

fn health(registry: &EntityRegistry, id: EntityId) -> &HealthComponent {
    registry.get_component::<HealthComponent>(id).unwrap()
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1.0e-3
}

#[test]
fn default_damage_type_types_every_untyped_producer() {
    // With `defaultDamageType: ballistic`, a hit naming no type drains armor
    // at ×0.8 and reports ballistic.
    let mut typed = world(Some("ballistic"));
    let id = spawn(&mut typed, 100.0, vec![layer("armor", 100.0, &[])], &[]);
    let dispatch = hit(&mut typed, id, 20.0, None);
    assert_eq!(values(&typed, id), [84.0]);
    assert_eq!(dispatch.damage_type, type_index("ballistic"));
    assert!(approx(dispatch.amount, 16.0));
    // Without a default it stays untyped: ×1 everywhere, no type true.
    let mut untyped = world(None);
    let id = spawn(
        &mut untyped,
        100.0,
        vec![layer("armor", 100.0, &[("shock", 0.0)])],
        &[],
    );
    let dispatch = hit(&mut untyped, id, 20.0, None);
    assert_eq!(values(&untyped, id), [80.0]);
    assert_eq!(dispatch.damage_type, None);
    // Untyped damage ignores every resist.
    let id = spawn(&mut untyped, 100.0, Vec::new(), &[("shock", 0.0)]);
    hit(&mut untyped, id, 20.0, None);
    assert_eq!(health(&untyped, id).current, 80.0);
}

#[test]
fn plasma_bolt_strips_shield_and_spills_to_health() {
    let mut registry = world(Some("ballistic"));
    let id = spawn(&mut registry, 100.0, vec![layer("shield", 30.0, &[])], &[]);
    let dispatch = hit(&mut registry, id, 30.0, Some("plasma"));
    assert_eq!(values(&registry, id), [0.0]);
    assert!(approx(health(&registry, id).current, 90.0));
    assert!(approx(dispatch.amount, 40.0));
    assert!(approx(dispatch.absorbed, 30.0));
    assert_eq!(dispatch.damage_type, type_index("plasma"));
    let id = spawn(&mut registry, 100.0, vec![layer("armor", 50.0, &[])], &[]);
    hit(&mut registry, id, 20.0, Some("plasma"));
    assert_eq!(values(&registry, id), [40.0]);
}

#[test]
fn explosive_splash_cracks_armor_and_flesh() {
    let mut registry = world(Some("ballistic"));
    let shielded = spawn(&mut registry, 100.0, vec![layer("shield", 30.0, &[])], &[]);
    hit(&mut registry, shielded, 20.0, Some("explosive"));
    assert_eq!(values(&registry, shielded), [15.0]);
    let armored = spawn(&mut registry, 100.0, vec![layer("armor", 30.0, &[])], &[]);
    let dispatch = hit(&mut registry, armored, 30.0, Some("explosive"));
    assert_eq!(values(&registry, armored), [0.0]);
    assert!(approx(health(&registry, armored).current, 87.5));
    assert!(approx(dispatch.amount, 42.5));
    assert!(approx(dispatch.absorbed, 30.0));
    assert_eq!(dispatch.damage_type, type_index("explosive"));
}

#[test]
fn ballistic_default_spills_raw_remainder_through_layers() {
    let mut registry = world(Some("ballistic"));
    let id = spawn(
        &mut registry,
        100.0,
        vec![layer("shield", 30.0, &[]), layer("armor", 50.0, &[])],
        &[],
    );
    let dispatch = hit(&mut registry, id, 100.0, None);
    assert_eq!(values(&registry, id), [0.0, 0.0]);
    assert!(approx(health(&registry, id).current, 92.5));
    assert!(approx(dispatch.amount, 87.5));
    assert!(approx(dispatch.absorbed, 80.0));
}

#[test]
fn spillover_carries_raw_not_scaled_remainder() {
    let mut registry = world(None);
    let id = spawn(&mut registry, 100.0, vec![layer("shield", 30.0, &[])], &[]);
    let dispatch = hit(&mut registry, id, 20.0, Some("shock"));
    assert!(approx(health(&registry, id).current, 96.25));
    assert!(approx(dispatch.raw, 20.0));
    assert!(approx(dispatch.amount, 33.75));
    assert!(approx(dispatch.absorbed, 30.0));
    assert!(approx(dispatch.health_before - dispatch.health_after, 3.75));
    // A hit that bypasses every layer absorbs nothing.
    let id = spawn(&mut registry, 100.0, vec![layer("shield", 30.0, &[])], &[]);
    let dispatch = hit(&mut registry, id, 20.0, Some("flechette"));
    assert_eq!(dispatch.absorbed, 0.0);
    assert!(approx(dispatch.amount, 20.0));
}

#[test]
fn bypass_and_zero_multipliers_on_each_pool() {
    let mut registry = world(None);
    // `"bypass"` passes the shield even with a resist of 0 for the type.
    let id = spawn(
        &mut registry,
        100.0,
        vec![layer("shield", 30.0, &[("flechette", 0.0)])],
        &[],
    );
    hit(&mut registry, id, 20.0, Some("flechette"));
    assert_eq!(values(&registry, id), [30.0]);
    assert_eq!(health(&registry, id).current, 80.0);
    // `health.resist` 0 deals no health damage.
    let id = spawn(&mut registry, 100.0, Vec::new(), &[("shock", 0.0)]);
    let dispatch = hit(&mut registry, id, 20.0, Some("shock"));
    assert_eq!(health(&registry, id).current, 100.0);
    assert_eq!(dispatch.amount, 0.0);
}

#[test]
fn resist_zero_everywhere_is_immune_but_restamps_and_credits_nothing() {
    let mut registry = world(None);
    let id = spawn(
        &mut registry,
        100.0,
        vec![layer("shield", 30.0, &[("shock", 0.0)])],
        &[("shock", 0.0)],
    );
    let dispatch = hit(&mut registry, id, 20.0, Some("shock"));
    assert_eq!(values(&registry, id), [30.0]);
    assert_eq!(health(&registry, id).current, 100.0);
    assert_eq!(
        (dispatch.amount, dispatch.absorbed, dispatch.broke),
        (0.0, 0.0, false)
    );
    assert_eq!(
        registry
            .get_component::<DefenseLayersComponent>(id)
            .unwrap()
            .last_hit_tick,
        Some(registry.host_tick()),
        "the blocked hit still restarts the delay"
    );
    assert_eq!(
        health(&registry, id)
            .contributor_ledger
            .total_recorded_hits(),
        0,
        "a hit dealing no points credits no contributor"
    );
    // At zero the shield is skipped, and health's own ×0 still holds.
    set_layer(&mut registry, id, 0, 0.0);
    hit(&mut registry, id, 20.0, Some("shock"));
    assert_eq!(health(&registry, id).current, 100.0);
}

#[test]
fn layer_resist_scales_that_layer_only() {
    let mut registry = world(None);
    let id = spawn(
        &mut registry,
        100.0,
        vec![layer("shield", 30.0, &[("shock", 0.5)])],
        &[],
    );
    let dispatch = hit(&mut registry, id, 40.0, Some("shock"));
    assert_eq!(values(&registry, id), [0.0]);
    assert!(approx(health(&registry, id).current, 92.5));
    assert!(approx(dispatch.amount, 37.5));
    assert!(dispatch.broke);
}

#[test]
fn health_resist_scales_health_only() {
    let mut registry = world(None);
    let enemy = spawn(
        &mut registry,
        70.0,
        vec![layer("armor", 120.0, &[])],
        &[("shock", 0.5)],
    );
    let dispatch = hit(&mut registry, enemy, 300.0, Some("shock"));
    assert_eq!(values(&registry, enemy), [0.0]);
    assert!(approx(health(&registry, enemy).current, 47.5));
    assert!(approx(dispatch.amount, 142.5));
    assert!(approx(dispatch.absorbed, 120.0));
    let unlayered = spawn(&mut registry, 100.0, Vec::new(), &[("shock", 0.5)]);
    let dispatch = hit(&mut registry, unlayered, 20.0, Some("shock"));
    assert!(approx(health(&registry, unlayered).current, 92.5));
    assert!(approx(dispatch.amount, 7.5));
}

#[test]
fn zero_resist_layer_absorbs_while_above_zero() {
    let mut registry = world(None);
    let id = spawn(
        &mut registry,
        100.0,
        vec![layer("shield", 30.0, &[("shock", 0.0)])],
        &[],
    );
    let dispatch = hit(&mut registry, id, 50.0, Some("shock"));
    assert_eq!(values(&registry, id), [30.0]);
    assert_eq!(health(&registry, id).current, 100.0);
    assert_eq!((dispatch.amount, dispatch.broke), (0.0, false));
    set_layer(&mut registry, id, 0, 0.0);
    let dispatch = hit(&mut registry, id, 50.0, Some("shock"));
    assert!(approx(health(&registry, id).current, 62.5));
    assert!(approx(dispatch.amount, 37.5));
    assert_eq!(dispatch.absorbed, 0.0);
}

#[test]
fn zero_multiplier_layer_stops_the_rest_of_the_hit() {
    let mut registry = world(None);
    let id = spawn(
        &mut registry,
        100.0,
        vec![layer("shield", 30.0, &[]), layer("armor", 50.0, &[])],
        &[],
    );
    let dispatch = hit(&mut registry, id, 20.0, Some("flechette"));
    assert_eq!(values(&registry, id), [30.0, 50.0]);
    assert_eq!(health(&registry, id).current, 100.0);
    assert_eq!(
        (dispatch.raw, dispatch.amount, dispatch.absorbed),
        (20.0, 0.0, 0.0)
    );
    assert!(!dispatch.broke);
    assert_eq!(
        health(&registry, id)
            .contributor_ledger
            .total_recorded_hits(),
        0
    );
    // With the armor already empty it is skipped, and health takes the hit.
    set_layer(&mut registry, id, 1, 0.0);
    let dispatch = hit(&mut registry, id, 20.0, Some("flechette"));
    assert_eq!(values(&registry, id), [30.0, 0.0]);
    assert_eq!(health(&registry, id).current, 80.0);
    assert!(approx(dispatch.amount, 20.0));
    // An earlier layer's drain stands when a ×0 layer stops the rest.
    let id = spawn(
        &mut registry,
        100.0,
        vec![layer("shield", 10.0, &[]), layer("armor", 50.0, &[])],
        &[],
    );
    let dispatch = hit(&mut registry, id, 30.0, Some("solid"));
    assert_eq!(values(&registry, id), [0.0, 50.0]);
    assert_eq!(health(&registry, id).current, 100.0);
    assert!(approx(dispatch.amount, 10.0));
    assert!(approx(dispatch.absorbed, 10.0));
    assert!(dispatch.broke);
    assert!(
        health(&registry, id)
            .contributor_ledger
            .total_recorded_hits()
            > 0
    );
}

#[test]
fn capacity_zero_layer_is_present_and_skipped() {
    let mut registry = world(None);
    let id = spawn(&mut registry, 100.0, vec![layer("shield", 0.0, &[])], &[]);
    let dispatch = hit(&mut registry, id, 20.0, None);
    assert_eq!(values(&registry, id), [0.0]);
    assert!(!dispatch.broke);
    assert_eq!(health(&registry, id).current, 80.0);
}

#[test]
fn lethal_hit_counts_overkill_into_points_dealt() {
    let mut registry = world(None);
    let id = spawn(&mut registry, 100.0, vec![layer("shield", 10.0, &[])], &[]);
    let mut component = health(&registry, id).clone();
    component.current = 5.0;
    registry.set_component(id, component).unwrap();
    let dispatch = hit(&mut registry, id, 30.0, None);
    assert_eq!(
        (dispatch.amount, dispatch.absorbed, dispatch.raw),
        (30.0, 10.0, 30.0)
    );
    assert!(approx(dispatch.health_after, -15.0));
    assert!(approx(
        health(&registry, id)
            .contributor_ledger
            .total_recorded_damage(),
        30.0
    ));
}

#[test]
fn hit_on_latched_target_drains_and_reports_but_credits_nothing() {
    let mut registry = world(None);
    let id = spawn(&mut registry, 100.0, vec![layer("armor", 50.0, &[])], &[]);
    let mut component = health(&registry, id).clone();
    component.current = 0.0;
    component.death_handled = true;
    registry.set_component(id, component).unwrap();
    let dispatch = hit(&mut registry, id, 20.0, None);
    assert_eq!(values(&registry, id), [30.0]);
    assert_eq!((dispatch.amount, dispatch.absorbed), (20.0, 20.0));
    assert_eq!(
        health(&registry, id)
            .contributor_ledger
            .total_recorded_hits(),
        0
    );
    // It emptying a layer still reports broke.
    let dispatch = hit(&mut registry, id, 30.0, None);
    assert!(dispatch.broke);
    // Under the ballistic default the same hit drains 16 from armor.
    let mut typed = world(Some("ballistic"));
    let id = spawn(&mut typed, 100.0, vec![layer("armor", 50.0, &[])], &[]);
    let mut component = health(&typed, id).clone();
    component.current = 0.0;
    component.death_handled = true;
    typed.set_component(id, component).unwrap();
    let dispatch = hit(&mut typed, id, 20.0, None);
    assert_eq!(values(&typed, id), [34.0]);
    assert!(approx(dispatch.amount, 16.0));
    assert!(approx(dispatch.absorbed, 16.0));
}

#[test]
fn shield_only_damage_credits_the_attacker_as_a_contributor() {
    let mut registry = world(None);
    let id = spawn(&mut registry, 100.0, vec![layer("shield", 50.0, &[])], &[]);
    hit(&mut registry, id, 20.0, None);
    assert_eq!(health(&registry, id).current, 100.0);
    assert!(approx(
        health(&registry, id)
            .contributor_ledger
            .recorded_damage_by_source("test.attacker")
            .unwrap(),
        20.0
    ));
}

/// A brain that holds still, enough for the chokepoint to record perception.
fn idle_brain() -> BrainComponent {
    BrainComponent::from_graph(&BehaviorGraphDescriptor {
        knockback: Default::default(),
        envelope: BehaviorGraphEnvelope {
            initial: "idle".to_string(),
            activities: BTreeMap::from([(
                "idle".to_string(),
                BehaviorActivityDescriptor {
                    sound: None,
                    animation: None,
                    motion: Some(MotionVerb::Hold),
                    action: None,
                    on_enter: None,
                    layers: Default::default(),
                },
            )]),
            transitions: Default::default(),
        },
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: Default::default(),
        engagement_radius: None,
        move_speed: 1.0,
    })
}

fn brain(registry: &EntityRegistry, id: EntityId) -> &BrainComponent {
    registry.get_component::<BrainComponent>(id).unwrap()
}

/// Age the brain's damage memory to `ms` since the last hit, as quiet ticks
/// would.
fn age_brain(registry: &mut EntityRegistry, id: EntityId, ms: f32) {
    let mut aged = brain(registry, id).clone();
    aged.time_since_damage_ms = ms;
    aged.age_recent_attackers(ms);
    registry.set_component(id, aged).unwrap();
}

// Brain threat counts points dealt, so a hit the shield fully absorbs still
// raises the attacker's threat and resets damage recency. A hit a ×0 layer
// blocks deals 0 points: it adds no threat and leaves the attacker's own
// recency alone, while the brain's damage recency still resets on its
// positive raw amount.
#[test]
fn shield_absorbed_hit_raises_threat_by_points_dealt_and_resets_recency() {
    let attacker = EntityId::from_raw(99);
    let mut registry = world(None);
    let id = spawn(
        &mut registry,
        100.0,
        vec![layer("shield", 50.0, &[]), layer("armor", 50.0, &[])],
        &[],
    );
    registry.set_component(id, idle_brain()).unwrap();
    age_brain(&mut registry, id, 500.0);

    // Plasma is ×1.5 on the shield: 20 raw drains 30, all of it absorbed.
    let dispatch = hit(&mut registry, id, 20.0, Some("plasma"));
    assert_eq!(health(&registry, id).current, 100.0);
    assert!(approx(dispatch.amount, 30.0));
    assert!(approx(dispatch.absorbed, 30.0));
    assert_eq!(brain(&registry, id).time_since_damage_ms, 0.0);
    let threat = brain(&registry, id)
        .recent_attacker(attacker)
        .expect("a fully absorbed hit records its attacker");
    assert!(
        approx(threat.accumulated_damage, 30.0),
        "threat is the points dealt, not the raw amount"
    );
    assert_eq!(threat.time_since_damage_ms, 0.0);

    // Flechette bypasses the shield and meets armor at ×0: 0 points dealt.
    age_brain(&mut registry, id, 500.0);
    let dispatch = hit(&mut registry, id, 20.0, Some("flechette"));
    assert_eq!(values(&registry, id), [20.0, 50.0]);
    assert_eq!(dispatch.amount, 0.0);
    let threat = brain(&registry, id).recent_attacker(attacker).unwrap();
    assert!(
        approx(threat.accumulated_damage, 30.0),
        "a 0-point hit adds no threat"
    );
    assert_eq!(
        threat.time_since_damage_ms, 500.0,
        "a 0-point hit leaves the attacker's recency alone"
    );
    assert_eq!(
        brain(&registry, id).time_since_damage_ms,
        0.0,
        "a blocked hit with a positive raw amount still resets damage recency"
    );
}
