// Status stage bounds: 1024 entities carrying four instances each, with
// expiry and reapply churn, a 17th application on full entities and four
// pawns in a status fluid. With markers (`damage: 0`) the stage and its
// applications allocate nothing once warm; the damaging run's per-tick time
// and allocations are measured, with the chokepoint and policy cost counted
// apart from the stage's own bookkeeping. An entity with no instance costs
// nothing. The measurement is `#[ignore]`d: run it on purpose with
//   cargo test --release -p postretro-sim --lib -- --ignored --nocapture status_stage_damaging_stress
// See: context/lib/testing_guide.md §Resource bounds · context/lib/entity_model.md §5

use std::sync::Arc;
use std::time::{Duration, Instant};

use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::statuses::StatusApplier;
use postretro_entities::{EntityId, EntityRegistry, ScriptCtx, Transform};
use postretro_foundation::defense::DefenseVocabulary;
use postretro_foundation::status::parse_statuses;
use postretro_foundation::{ImmersionComponent, ImpactEventDescriptor};
use serde_json::{Value, json};

use super::status_stage::run_status_stage;
use crate::impact_policy::ImpactPolicyRuntime;

const AFFLICTED: usize = 1024;
/// Entities filled to the 16-instance cap that take a 17th every tick.
const FULL: usize = 16;
const PAWNS: usize = 4;
const WARM_TICKS: u64 = 120;
const TICKS: u64 = 600;
/// Every this many ticks the short statuses are reapplied to all.
const REAPPLY_EVERY: u64 = 20;

/// Four statuses: a short one that expires and is reapplied, a medium one, an
/// until-cleared one, and one at an odd period; `damage` 0 makes all markers.
fn vocabulary(damage: f32) -> Arc<DefenseVocabulary> {
    let status = |name: &str, tick_ms: f32, duration_ms: Option<f32>| {
        let mut status =
            json!({ "kind": "status", "name": name, "damage": damage, "tickMs": tick_ms });
        if let Some(duration) = duration_ms {
            status["durationMs"] = json!(duration);
        }
        status
    };
    Arc::new(DefenseVocabulary {
        statuses: parse_statuses(
            Some(&json!([
                status("flash", 16.0, Some(50.0)),
                status("smoulder", 100.0, Some(500.0)),
                status("rot", 500.0, None),
                status("hum", 17.0, Some(1000.0)),
            ])),
            |_| None,
        )
        .unwrap(),
        ..Default::default()
    })
}

fn spawn(registry: &mut EntityRegistry) -> EntityId {
    let entity = registry.spawn(Transform::default());
    registry
        .set_component(
            entity,
            HealthComponent {
                max: 1.0e9,
                current: 1.0e9,
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

struct World {
    ctx: ScriptCtx,
    afflicted: Vec<EntityId>,
    full: Vec<EntityId>,
    appliers: [StatusApplier; 5],
    source: u16,
}

impl World {
    fn new(damage: f32, empty: usize) -> Self {
        let ctx = ScriptCtx::new();
        let (afflicted, full, appliers, source) = {
            let mut registry = ctx.registry.borrow_mut();
            registry.set_defense_vocabulary(vocabulary(damage));
            registry.set_status_applications_immediate(true);
            let afflicted: Vec<EntityId> = (0..AFFLICTED).map(|_| spawn(&mut registry)).collect();
            let full: Vec<EntityId> = (0..FULL).map(|_| spawn(&mut registry)).collect();
            for _ in 0..PAWNS {
                let pawn = spawn(&mut registry);
                registry
                    .set_component(
                        pawn,
                        ImmersionComponent {
                            immersion: 0.2,
                            fluid: Some(0),
                        },
                    )
                    .unwrap();
            }
            for _ in 0..empty {
                registry.spawn(Transform::default());
            }
            registry.set_fluid_statuses([("lava", Some("smoulder"))]);
            let source = registry.intern_status_source("weapon.stress");
            let appliers = [0, 1, 2, 3, 4].map(|index| {
                StatusApplier::Source(registry.intern_status_source(&format!("stress.{index}")))
            });
            (afflicted, full, appliers, source)
        };
        Self {
            ctx,
            afflicted,
            full,
            appliers,
            source,
        }
    }

    /// One tick's applications: the short statuses on every afflicted
    /// entity every `REAPPLY_EVERY` ticks (all four on the first tick), and a
    /// 17th instance on every full entity, each tick from a rotating applier.
    fn apply(&self, registry: &mut EntityRegistry, tick: u64) {
        let applier = self.appliers[0];
        if tick == 0 {
            for &entity in &self.afflicted {
                for status in 0..4 {
                    registry.apply_status(entity, status, applier, self.source);
                }
            }
            for &entity in &self.full {
                for status in 0..4 {
                    for &applier in &self.appliers[..4] {
                        registry.apply_status(entity, status, applier, self.source);
                    }
                }
            }
        } else if tick.is_multiple_of(REAPPLY_EVERY) {
            for &entity in &self.afflicted {
                registry.apply_status(entity, 0, applier, self.source);
                registry.apply_status(entity, 1, applier, self.source);
            }
        }
        let rotating = self.appliers[(tick % 5) as usize];
        for &entity in &self.full {
            registry.apply_status(entity, (tick % 4) as u16, rotating, self.source);
        }
    }
}

struct Run {
    stage_per_tick: Duration,
    allocations: usize,
    live_instances: usize,
}

/// `WARM_TICKS` unmeasured, then `TICKS` measured: each tick's applications
/// and the stage, under the probe. `on_impact` runs after each damaging tick.
fn run(world: &World, mut on_impact: impl FnMut(&mut EntityRegistry)) -> Run {
    let mut stage = Duration::ZERO;
    let mut allocations = 0;
    for tick in 0..WARM_TICKS + TICKS {
        let mut registry = world.ctx.registry.borrow_mut();
        registry.advance_host_tick();
        let measured = tick >= WARM_TICKS;
        let probe = crate::alloc_probe::AllocSnapshot::arm();
        world.apply(&mut registry, tick);
        let start = Instant::now();
        run_status_stage(&mut registry, &mut on_impact);
        if measured {
            stage += start.elapsed();
            allocations += probe.allocs_since();
        }
    }
    let live_instances = world
        .ctx
        .registry
        .borrow()
        .statuses()
        .sets()
        .iter()
        .map(|set| set.instances().count())
        .sum();
    Run {
        stage_per_tick: stage / TICKS as u32,
        allocations,
        live_instances,
    }
}

// Markers: after warm-up, every tick's applications, fluid applications,
// due ticks, expiry and 17th-instance replacement allocate nothing. Only
// afflicted entities have a set; the 10 000 empty ones are never visited.
#[test]
fn status_stage_marker_stress_allocates_nothing() {
    let world = World::new(0.0, 10_000);
    let result = run(&world, |_| {});
    assert_eq!(result.allocations, 0, "the marker run allocates nothing");
    let registry = world.ctx.registry.borrow();
    assert_eq!(
        registry.statuses().sets().len(),
        AFFLICTED + FULL + PAWNS,
        "only afflicted entities carry a set"
    );
    for &entity in &world.full {
        let count = registry
            .statuses()
            .sets()
            .iter()
            .find(|set| set.entity == entity)
            .map(|set| set.instances().count());
        assert!(
            count.is_some_and(|count| count <= 16),
            "a 17th replaces, never grows: {count:?}"
        );
    }
    assert!(result.live_instances > 0);
}

// An entity with no instance costs nothing: with none anywhere the stage
// returns before visiting any entity, however many exist, and allocates
// nothing.
#[test]
fn status_stage_skips_entities_without_instances() {
    let ctx = ScriptCtx::new();
    let mut registry = ctx.registry.borrow_mut();
    registry.set_defense_vocabulary(vocabulary(4.0));
    for _ in 0..10_000 {
        spawn(&mut registry);
    }
    registry.advance_host_tick();
    let probe = crate::alloc_probe::AllocSnapshot::arm();
    run_status_stage(&mut registry, &mut |_| panic!("no instance, no tick"));
    assert_eq!(probe.allocs_since(), 0);
    assert!(registry.statuses().sets().is_empty());
}

fn policy() -> ImpactEventDescriptor {
    let effect: Value = json!({
        "when": { "op": "gt", "a": { "op": "input", "name": "@impact.amount" }, "b": { "op": "const", "value": 0.0 } },
        "do": [{ "primitive": "setState", "target": "@impact.target",
                 "args": { "name": "burnt", "value": { "op": "const", "value": 1.0 } } }],
    });
    ImpactEventDescriptor {
        id: "stress".into(),
        is_override: false,
        levels: Vec::new(),
        filter_tag: Some("enemy".into()),
        policy: vec![effect],
    }
}

// The damaging run's cost, reported for the plan: the stage's bookkeeping
// alone (markers), plus the chokepoint (damaging ticks, no policy), plus one
// impact policy evaluated per tick.
#[test]
#[ignore = "stress measurement: run with --release --ignored --nocapture"]
fn status_stage_damaging_stress_measure() {
    let markers = run(&World::new(0.0, 0), |_| {});
    let chokepoint_world = World::new(4.0, 0);
    let chokepoint = run(&chokepoint_world, |registry| {
        registry.take_impact_dispatches();
    });
    let policy_world = World::new(4.0, 0);
    let runtime = std::cell::RefCell::new(ImpactPolicyRuntime::new(policy_world.ctx.clone()));
    runtime.borrow_mut().replace_global_events(vec![policy()]);
    let policies = run(&policy_world, |registry| {
        runtime.borrow_mut().evaluate_pending_in_registry(registry);
    });
    eprintln!(
        "status stage, {AFFLICTED} entities x 4 instances + {FULL} full (17th each tick) + {PAWNS} pawns in a status fluid, {TICKS} ticks:\n  \
         markers (bookkeeping only): {:?}/tick, {} allocations, {} live\n  \
         damaging, no policy (+ chokepoint): {:?}/tick, {} allocations ({:.1}/tick)\n  \
         damaging, one policy (+ policy): {:?}/tick, {} allocations ({:.1}/tick)",
        markers.stage_per_tick,
        markers.allocations,
        markers.live_instances,
        chokepoint.stage_per_tick,
        chokepoint.allocations,
        chokepoint.allocations as f64 / TICKS as f64,
        policies.stage_per_tick,
        policies.allocations,
        policies.allocations as f64 / TICKS as f64,
    );
    assert_eq!(markers.allocations, 0);
}
