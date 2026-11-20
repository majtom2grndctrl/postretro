// Status-engine proofs through the authoritative fixed-tick seam: timing,
// stacking, dedupe, the instance cap, lifecycle clears, the off-tick latch and
// hot reload. Applications here are made through the registry API: in the AI
// stage for an in-tick application, between ticks for an off-tick one.
// See: context/lib/entity_model.md §5 · context/lib/scripting.md §2

use std::sync::Arc;

use glam::Vec3;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
};
use postretro_entities::components::statuses::{StatusApplier, StatusApplyOutcome};
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::DamagePayload;
use postretro_foundation::defense::{DamageTypeDeclaration, DefenseVocabulary, PoolMultiplier};
use postretro_foundation::status::parse_statuses;
use serde_json::{Value, json};

use super::teleport_tick_tests::{Rig, classic};

pub(super) const BURNING: u16 = 0;
pub(super) const WET: u16 = 1;
pub(super) const CORRODING: u16 = 2;

pub(super) fn declarations(burning: Value) -> Vec<postretro_foundation::status::StatusDeclaration> {
    parse_statuses(
        Some(&json!([
            burning,
            { "kind": "status", "name": "wet", "damage": 0, "tickMs": 500, "durationMs": 5000 },
            { "kind": "status", "name": "corroding", "damage": 2, "tickMs": 1000 },
        ])),
        |name| (name == "fire").then_some(0),
    )
    .unwrap()
}

pub(super) fn burning(stacking: &str, tick_ms: f32, duration_ms: Option<f32>) -> Value {
    let mut burning = json!({
        "kind": "status", "name": "burning", "damage": 4, "tickMs": tick_ms,
        "stacking": stacking, "maxStacks": 3,
        "type": { "kind": "damageType", "name": "fire" },
    });
    if let Some(duration) = duration_ms {
        burning["durationMs"] = json!(duration);
    }
    burning
}

pub(super) fn install(registry: &mut EntityRegistry, burning: Value) {
    registry.set_defense_vocabulary(Arc::new(DefenseVocabulary {
        layers: Vec::new(),
        damage_types: vec![DamageTypeDeclaration {
            name: "fire".to_string(),
            matrix: std::collections::BTreeMap::from([(
                "health".to_string(),
                PoolMultiplier::Scale(1.0),
            )]),
        }],
        default_damage_type: None,
        statuses: declarations(burning),
    }));
}

pub(super) struct World {
    pub(super) rig: Rig,
    pub(super) enemy: EntityId,
}

impl World {
    pub(super) fn new(burning: Value) -> Self {
        let mut rig = Rig::new(vec![classic(
            Vec3::new(0.0, -50.0, 0.0),
            1.0,
            Vec3::ZERO,
            false,
        )]);
        rig.bind(Vec::new(), &[], Vec::new());
        install(&mut rig.ctx.registry.borrow_mut(), burning);
        let enemy = spawn(&rig, 1_000_000.0);
        Self { rig, enemy }
    }

    pub(super) fn registry(&self) -> std::cell::RefMut<'_, EntityRegistry> {
        self.rig.ctx.registry.borrow_mut()
    }

    pub(super) fn health(&self, entity: EntityId) -> f32 {
        self.rig.health(entity)
    }

    pub(super) fn tick(&mut self) -> super::TickEvents {
        self.rig.tick()
    }

    /// One tick whose AI stage runs `apply`: an in-tick application.
    pub(super) fn tick_applying(
        &mut self,
        mut apply: impl FnMut(&mut EntityRegistry),
    ) -> super::TickEvents {
        self.rig.tick_with(&[], |registry| apply(registry))
    }

    pub(super) fn host_tick(&self) -> u64 {
        self.rig.ctx.registry.borrow().host_tick()
    }

    pub(super) fn source(&self, id: &str) -> u16 {
        self.registry().intern_status_source(id)
    }
}

pub(super) fn spawn(rig: &Rig, max: f32) -> EntityId {
    let mut registry = rig.ctx.registry.borrow_mut();
    let entity = registry.spawn(Transform::default());
    registry
        .set_component(
            entity,
            HealthComponent {
                max,
                current: max,
                hitbox: None,
                death_handled: false,
                pending_kill_credit: None,
                zone_multipliers: Default::default(),
                contributor_ledger: Default::default(),
            },
        )
        .unwrap();
    registry.set_tags(entity, vec!["enemy".into()]).unwrap();
    entity
}

pub(super) fn applier(world: &World, name: &str) -> StatusApplier {
    StatusApplier::Source(world.source(name))
}

pub(super) fn apply(
    registry: &mut EntityRegistry,
    target: EntityId,
    status: u16,
    applier: StatusApplier,
) -> Option<StatusApplyOutcome> {
    let source = registry.intern_status_source("weapon.incendiary");
    registry.apply_status(target, status, applier, source)
}

/// Ticks `n` times, returning the host ticks on which the target lost health.
pub(super) fn damage_ticks(world: &mut World, target: EntityId, n: usize) -> Vec<u64> {
    let mut landed = Vec::new();
    for _ in 0..n {
        let before = world.health(target);
        world.tick();
        if world.health(target) < before {
            landed.push(world.host_tick());
        }
    }
    landed
}

// P5: an unrefreshed `burning` (500 / 3000) lands its first tick `tickMs`
// after the tick it was applied on, then every `tickMs`: six ticks, the last
// on its expiry tick, none after.
#[test]
fn status_timing_lands_six_ticks_on_phase_and_expires() {
    let mut world = World::new(burning("refresh", 500.0, Some(3000.0)));
    let enemy = world.enemy;
    let a = applier(&world, "weapon.incendiary");
    world.tick_applying(|registry| {
        apply(registry, enemy, BURNING, a);
    });
    let applied = world.host_tick();
    let landed = damage_ticks(&mut world, enemy, 400);
    let expected: Vec<u64> = (1..=6).map(|k| applied + 30 * k).collect();
    assert_eq!(landed, expected);
    assert!(!world.registry().has_status(enemy, BURNING));
}

// P5: `tickMs` 1 and 16 tick every fixed tick at 60 Hz; 17 every second.
#[test]
fn status_tick_rounding_matches_fixed_ticks() {
    for (tick_ms, spacing) in [(1.0, 1), (16.0, 1), (17.0, 2)] {
        let mut world = World::new(burning("refresh", tick_ms, None));
        let enemy = world.enemy;
        let a = applier(&world, "weapon.incendiary");
        world.tick_applying(|registry| {
            apply(registry, enemy, BURNING, a);
        });
        let applied = world.host_tick();
        let landed = damage_ticks(&mut world, enemy, 6);
        let expected: Vec<u64> = (1..)
            .map(|k| applied + spacing * k)
            .take_while(|tick| *tick <= applied + 6)
            .collect();
        assert_eq!(landed, expected, "tickMs {tick_ms}");
    }
}

// Eight pellets from one applier on one tick add one stack; two appliers
// create two instances that stack and credit independently.
#[test]
fn status_applications_dedupe_per_applier_per_tick() {
    let mut world = World::new(burning("stack", 500.0, Some(3000.0)));
    let enemy = world.enemy;
    let (a, b) = (applier(&world, "player.a"), applier(&world, "player.b"));
    world.tick_applying(|registry| {
        for _ in 0..8 {
            apply(registry, enemy, BURNING, a);
        }
        apply(registry, enemy, BURNING, b);
    });
    let instances: Vec<_> = world.registry().statuses().sets()[0]
        .instances()
        .copied()
        .collect();
    assert_eq!(instances.len(), 2);
    assert!(instances.iter().all(|instance| instance.stacks == 1));
    world.tick_applying(|registry| {
        apply(registry, enemy, BURNING, a);
    });
    let stacks: Vec<u8> = world.registry().statuses().sets()[0]
        .instances()
        .map(|instance| instance.stacks)
        .collect();
    assert_eq!(stacks, [2, 1], "each applier stacks its own instance");
}

// "refresh" resets time left without moving the phase; "stack" caps at
// `maxStacks`, multiplies damage, and at the cap still refreshes.
#[test]
fn status_stacking_caps_multiplies_and_keeps_phase() {
    let mut world = World::new(burning("stack", 500.0, Some(3000.0)));
    let enemy = world.enemy;
    let a = applier(&world, "weapon.incendiary");
    world.tick_applying(|registry| {
        apply(registry, enemy, BURNING, a);
    });
    let applied = world.host_tick();
    for _ in 0..4 {
        world.tick_applying(|registry| {
            apply(registry, enemy, BURNING, a);
        });
    }
    let instance = *world.registry().statuses().sets()[0]
        .instances()
        .next()
        .unwrap();
    assert_eq!(instance.stacks, 3, "capped at maxStacks");
    assert_eq!(
        instance.next_due,
        applied + 30,
        "stacking never moves the phase"
    );
    assert_eq!(
        instance.expires,
        Some(world.host_tick() + 180),
        "a capped stack still refreshes"
    );
    let before = world.health(enemy);
    while world.host_tick() < applied + 30 {
        world.tick();
    }
    assert_eq!(before - world.health(enemy), 12.0, "damage × stacks");
}

// P2: a hit that refreshes on the expiry tick keeps the instance, lands that
// tick's damage on its old phase, and ticks on; until-cleared never ends.
#[test]
fn status_refresh_on_expiry_tick_keeps_instance_and_phase() {
    let mut world = World::new(burning("refresh", 500.0, Some(1000.0)));
    let enemy = world.enemy;
    let a = applier(&world, "weapon.incendiary");
    world.tick_applying(|registry| {
        apply(registry, enemy, BURNING, a);
    });
    let applied = world.host_tick();
    while world.host_tick() + 1 < applied + 60 {
        world.tick();
    }
    let before = world.health(enemy);
    world.tick_applying(|registry| {
        apply(registry, enemy, BURNING, a);
    });
    assert_eq!(world.host_tick(), applied + 60, "the expiry tick");
    assert_eq!(before - world.health(enemy), 4.0, "the due tick landed");
    assert!(
        world.registry().has_status(enemy, BURNING),
        "refreshed, so it survives"
    );
    let landed = damage_ticks(&mut world, enemy, 61);
    assert_eq!(landed, vec![applied + 90, applied + 120]);

    let mut world = World::new(burning("refresh", 500.0, None));
    let enemy = world.enemy;
    world.tick_applying(|registry| {
        apply(registry, enemy, BURNING, a);
    });
    assert_eq!(
        damage_ticks(&mut world, enemy, 3000).len(),
        100,
        "until cleared"
    );
}

// P18: a 17th instance replaces the least time left; among equals the
// oldest; until-cleared ranks most; the new instance always enters.
#[test]
fn status_cap_replaces_least_time_left() {
    let mut world = World::new(burning("refresh", 500.0, Some(3000.0)));
    let enemy = world.enemy;
    let appliers: Vec<_> = (0..17).map(|n| applier(&world, &format!("a{n}"))).collect();
    let until_cleared = applier(&world, "forever");
    world.tick_applying(|registry| {
        apply(registry, enemy, CORRODING, until_cleared);
        for &a in &appliers[..15] {
            apply(registry, enemy, BURNING, a);
        }
    });
    world.tick_applying(|registry| {
        apply(registry, enemy, BURNING, appliers[15]);
    });
    world.tick_applying(|registry| {
        apply(registry, enemy, BURNING, appliers[16]);
    });
    let instances: Vec<_> = world.registry().statuses().sets()[0]
        .instances()
        .copied()
        .collect();
    assert_eq!(instances.len(), 16);
    assert!(
        instances.iter().any(|i| i.applier == until_cleared),
        "until-cleared ranks most"
    );
    assert!(
        instances.iter().any(|i| i.applier == appliers[16]),
        "the new instance entered"
    );
    assert!(
        !instances.iter().any(|i| i.applier == appliers[0]),
        "the oldest of the equal least-time-left instances went"
    );
}

// P24: a marker ticks without entering the chokepoint: no impact, no
// contributor; `hasStatus` sees it.
#[test]
fn marker_status_never_enters_chokepoint() {
    let mut world = World::new(burning("refresh", 500.0, None));
    let enemy = world.enemy;
    let a = applier(&world, "fluid.water");
    world.tick_applying(|registry| {
        apply(registry, enemy, WET, a);
    });
    world.registry().take_impact_dispatches();
    for _ in 0..120 {
        world.tick();
    }
    assert!(world.registry().take_impact_dispatches().is_empty());
    assert!(world.registry().has_status(enemy, WET));
    let registry = world.registry();
    let health = registry.get_component::<HealthComponent>(enemy).unwrap();
    assert!(health.contributor_ledger.entries().is_empty());
}

// P6 and the lethal tick: a due tick on a target already at zero deals
// nothing; a lethal tick latches on its own tick, credits the source id, and
// the latch clears the target's statuses. A health ≤ 0 target refuses a new
// instance.
#[test]
fn status_lethal_tick_latches_credits_and_clears() {
    let mut world = World::new(burning("refresh", 500.0, None));
    let doomed = spawn(&world.rig, 4.0);
    let a = applier(&world, "weapon.incendiary");
    world.tick_applying(|registry| {
        apply(registry, doomed, BURNING, a);
    });
    let applied = world.host_tick();
    let mut death_tick = None;
    for _ in 0..40 {
        world.tick();
        let latched = world
            .registry()
            .get_component::<HealthComponent>(doomed)
            .unwrap()
            .death_handled;
        if latched && death_tick.is_none() {
            death_tick = Some(world.host_tick());
        }
    }
    assert_eq!(death_tick, Some(applied + 30), "latched on the lethal tick");
    let registry = world.registry();
    let health = registry.get_component::<HealthComponent>(doomed).unwrap();
    assert!(health.death_handled);
    let credit = health
        .pending_kill_credit
        .as_ref()
        .expect("credit frozen at the latch");
    assert_eq!(
        credit.contributor_ledger.entries()[0].source_id,
        "weapon.incendiary"
    );
    assert!(
        !registry.has_status(doomed, BURNING),
        "the latch clears statuses"
    );
    drop(registry);
    assert!(apply(&mut world.registry(), doomed, BURNING, a).is_none());

    // A rocket kill and a due tick on one tick: the tick deals nothing.
    let mut world = World::new(burning("refresh", 500.0, None));
    let target = spawn(&world.rig, 50.0);
    world.tick_applying(|registry| {
        apply(registry, target, BURNING, a);
    });
    let applied = world.host_tick();
    while world.host_tick() + 1 < applied + 30 {
        world.tick();
    }
    world.registry().take_impact_dispatches();
    world.tick_applying(|registry| {
        let mut context = DamageContext::new("rocket", DamageProducer::InTick);
        context.attacker = None;
        apply_damage_with_context(
            registry,
            target,
            &DamagePayload {
                amount: 500.0,
                impulse: Vec3::ZERO,
                damage_type: None,
            },
            context,
        );
    });
    let dispatches = world.registry().take_impact_dispatches();
    assert_eq!(dispatches.len(), 1, "only the rocket's impact");
    assert!(dispatches[0].status.is_none());
}

// A tick with the applier despawned still lands and credits the source id.
#[test]
fn status_tick_after_applier_despawn_credits_source_id() {
    let mut world = World::new(burning("refresh", 500.0, None));
    let enemy = world.enemy;
    let shooter = spawn(&world.rig, 100.0);
    world.tick_applying(|registry| {
        let source = registry.intern_status_source("weapon.incendiary");
        registry.apply_status(enemy, BURNING, StatusApplier::Entity(shooter), source);
    });
    world.registry().despawn(shooter).unwrap();
    world.registry().take_impact_dispatches();
    for _ in 0..30 {
        world.tick();
    }
    let dispatches = world.registry().take_impact_dispatches();
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].source, None);
    assert_eq!(dispatches[0].source_id, "weapon.incendiary");
    assert_eq!(dispatches[0].status, Some(BURNING));
}

// P10: two "stack" applications in two tickless frames add one stack,
// counted on the next tick; the first tick lands `tickMs` after it.
#[test]
fn off_tick_applications_latch_to_next_tick_and_dedupe() {
    let mut world = World::new(burning("stack", 500.0, Some(3000.0)));
    let enemy = world.enemy;
    let a = applier(&world, "script.applyStatus");
    world.tick();
    apply(&mut world.registry(), enemy, BURNING, a);
    apply(&mut world.registry(), enemy, BURNING, a);
    assert!(
        !world.registry().has_status(enemy, BURNING),
        "latched, not live"
    );
    world.tick();
    let counted = world.host_tick();
    let instance = *world.registry().statuses().sets()[0]
        .instances()
        .next()
        .unwrap();
    assert_eq!(instance.stacks, 1);
    assert_eq!(instance.next_due, counted + 30);
}

// P25, P26: an off-tick clear discards a latched application of that kind;
// a latched application to a target that despawned or died is dropped; a
// level unload discards every latched application.
#[test]
fn latched_status_application_dropped_at_boundaries() {
    let mut world = World::new(burning("refresh", 500.0, None));
    let enemy = world.enemy;
    let a = applier(&world, "script.applyStatus");
    world.tick();
    apply(&mut world.registry(), enemy, BURNING, a);
    world.registry().clear_status(enemy, BURNING);
    world.tick();
    assert!(
        !world.registry().has_status(enemy, BURNING),
        "apply then clear: none"
    );
    world.registry().clear_status(enemy, BURNING);
    apply(&mut world.registry(), enemy, BURNING, a);
    world.tick();
    assert!(
        world.registry().has_status(enemy, BURNING),
        "clear then apply: one"
    );

    let gone = spawn(&world.rig, 10.0);
    let dead = spawn(&world.rig, 10.0);
    apply(&mut world.registry(), gone, BURNING, a);
    apply(&mut world.registry(), dead, BURNING, a);
    world.registry().despawn(gone).unwrap();
    postretro_entities::components::health::set_health_absolute(&mut world.registry(), dead, 0.0);
    world.tick();
    assert!(!world.registry().has_status(dead, BURNING));

    apply(&mut world.registry(), enemy, CORRODING, a);
    world.registry().clear_for_level_unload();
    assert!(world.registry().statuses().pending().is_empty());
    assert!(world.registry().statuses().sets().is_empty());
}

// P19: hot reload keeps live instances and their next due tick, drops a
// removed kind, applies a changed damage from the next tick and a changed
// `tickMs` after it, and clamps a lowered `maxStacks` on the next tick.
#[test]
fn status_hot_reload_preserves_phase_and_applies_changes() {
    let mut world = World::new(burning("stack", 500.0, None));
    let enemy = world.enemy;
    let a = applier(&world, "weapon.incendiary");
    for _ in 0..3 {
        world.tick_applying(|registry| {
            apply(registry, enemy, BURNING, a);
        });
    }
    world.tick_applying(|registry| {
        apply(registry, enemy, CORRODING, a);
    });
    let due = world.registry().statuses().sets()[0]
        .instances()
        .next()
        .unwrap()
        .next_due;
    // Reorder, drop `corroding`, halve tickMs, raise damage, cap stacks at 1.
    let reloaded = parse_statuses(
        Some(&json!([
            { "kind": "status", "name": "wet", "damage": 0, "tickMs": 500 },
            { "kind": "status", "name": "burning", "damage": 10, "tickMs": 250,
              "stacking": "stack", "maxStacks": 1 },
        ])),
        |_| None,
    )
    .unwrap();
    {
        let mut registry = world.registry();
        let mut vocabulary = (**registry.defense_vocabulary()).clone();
        vocabulary.statuses = reloaded;
        registry.set_defense_vocabulary(Arc::new(vocabulary));
    }
    let instances: Vec<_> = world.registry().statuses().sets()[0]
        .instances()
        .copied()
        .collect();
    assert_eq!(instances.len(), 1, "the removed kind's instance went");
    assert_eq!(instances[0].status, 1, "kept by name across the reorder");
    assert_eq!(instances[0].next_due, due, "the next due tick is kept");
    let landed_before = world.health(enemy);
    let ticks_to_second = (due + 15 - world.host_tick()) as usize;
    let landed = damage_ticks(&mut world, enemy, ticks_to_second);
    assert_eq!(
        landed,
        vec![due, due + 15],
        "the new tickMs spaces later ticks"
    );
    assert_eq!(
        landed_before - world.health(enemy),
        20.0,
        "stacks clamp to 1 at 10 each"
    );
}

// A status with `type` omitted ticks as the mod's `defaultDamageType`; a
// typed one drains layers before health by its matrix and restamps the
// layers' recharge delay.
#[test]
fn status_ticks_are_typed_hits_through_layers() {
    use postretro_entities::components::defense_layers::{DefenseLayer, DefenseLayersComponent};
    let mut world = World::new(burning("refresh", 500.0, None));
    let enemy = world.enemy;
    {
        let mut registry = world.registry();
        let mut vocabulary = (**registry.defense_vocabulary()).clone();
        vocabulary.layers = vec!["shield".to_string()];
        vocabulary.damage_types[0].matrix.insert(
            "shield".to_string(),
            postretro_foundation::defense::PoolMultiplier::Scale(2.0),
        );
        vocabulary.default_damage_type = Some(0);
        registry.set_defense_vocabulary(Arc::new(vocabulary));
        registry
            .set_component(
                enemy,
                DefenseLayersComponent {
                    descriptor: None,
                    layers: vec![DefenseLayer {
                        name: "shield".to_string(),
                        value: 100.0,
                        capacity: 100.0,
                        recharge_delay_ms: 3000.0,
                        recharge_rate: 0.0,
                        resist: Vec::new(),
                        refill: Default::default(),
                    }],
                    health_resist: Vec::new(),
                    last_hit_tick: None,
                },
            )
            .unwrap();
    }
    let a = applier(&world, "weapon.incendiary");
    world.tick_applying(|registry| {
        apply(registry, enemy, BURNING, a);
        apply(registry, enemy, CORRODING, a);
    });
    let health_before = world.health(enemy);
    world.registry().take_impact_dispatches();
    for _ in 0..60 {
        world.tick();
    }
    let dispatches = world.registry().take_impact_dispatches();
    let burn = dispatches
        .iter()
        .find(|d| d.status == Some(BURNING))
        .unwrap();
    let corrode = dispatches
        .iter()
        .find(|d| d.status == Some(CORRODING))
        .unwrap();
    assert_eq!(burn.damage_type, Some(0));
    assert_eq!(
        corrode.damage_type,
        Some(0),
        "untyped takes the default type"
    );
    assert!(burn.absorbed > 0.0, "the shield absorbed the burn");
    assert_eq!(
        world.health(enemy),
        health_before,
        "layers drained before health"
    );
    let registry = world.registry();
    let layers = registry
        .get_component::<DefenseLayersComponent>(enemy)
        .unwrap();
    assert_eq!(layers.layers[0].value, 100.0 - 2.0 * 4.0 * 2.0 - 2.0 * 2.0);
    assert!(
        layers.last_hit_tick.is_some(),
        "a tick restamps the recharge delay"
    );
}
