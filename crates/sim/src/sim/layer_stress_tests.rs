// Layer stress run: the layer step's CPU time per tick and its heap
// allocations over a synthetic registry, expression parameters at the node
// cap against an all-literal baseline. `#[ignore]`d: run it on purpose with
//   cargo test --release -p postretro-sim --lib -- --ignored --nocapture layer_step_stress
// See: context/lib/scripting.md §11 (Defense layers)

use std::time::{Duration, Instant};

use postretro_entities::ScriptCtx;
use postretro_entities::registry::Transform;
use postretro_foundation::defense::HandleRef;
use postretro_foundation::{
    AuthoredHandle, DefenseLayerEntry, HealthDescriptor, IrNode, NumberOrIr,
};

use super::defense_layers::recharge_defense_layers;
use super::defense_parameter_tests::{hit, input, lit, session, set_global, spawn};

const LAYERED: usize = 1_000;
const UNLAYERED: usize = 1_000;
const TICKS: u64 = 600;
const HIT_EVERY: u64 = 30;

/// Which parameters the run binds.
#[derive(Clone, Copy, PartialEq)]
enum Parameters {
    Literal,
    /// At the node cap, reading global slots: shared once per descriptor.
    Global,
    /// At the node cap, reading the entity's own `@state.*`: per entity.
    PerEntity,
}

/// A balanced sum of 128 leaves (255 nodes, depth 8), at the node cap: one
/// `live` read and 127 reads of `zero`, so it evaluates to `live`.
fn capped(live: &str, zero: &str) -> NumberOrIr {
    fn tree(leaves: &mut dyn Iterator<Item = IrNode>, size: usize) -> IrNode {
        if size == 1 {
            return leaves.next().unwrap();
        }
        IrNode::Add {
            a: Box::new(tree(leaves, size / 2)),
            b: Box::new(tree(leaves, size / 2)),
        }
    }
    let mut leaves = std::iter::once(input(live)).chain(std::iter::repeat_with(|| input(zero)));
    NumberOrIr::Ir(tree(&mut leaves, 128))
}

fn entry(name: &str, parameters: Parameters) -> DefenseLayerEntry {
    let (capacity, delay, rate) = match parameters {
        Parameters::Literal => (lit(100.0), lit(0.0), lit(60.0)),
        Parameters::Global => (
            capped("tune.cap", "tune.zero"),
            capped("tune.zero", "tune.zero"),
            capped("tune.rate", "tune.zero"),
        ),
        Parameters::PerEntity => (
            capped("@state.cap", "@state.zero"),
            capped("@state.zero", "@state.zero"),
            capped("@state.rate", "@state.zero"),
        ),
    };
    DefenseLayerEntry {
        layer: AuthoredHandle::from_handle(&HandleRef::DefenseLayer {
            name: name.to_string(),
        }),
        capacity,
        recharge_delay_ms: Some(delay),
        recharge_rate: Some(rate),
        resist: Default::default(),
    }
}

struct Run {
    per_tick: Duration,
    allocations: usize,
}

fn run(parameters: Parameters) -> Run {
    let ctx: ScriptCtx = session(vec![(
        "layered",
        false,
        HealthDescriptor {
            max: 100.0,
            layers: vec![entry("shield", parameters), entry("armor", parameters)],
            ..Default::default()
        },
    )]);
    set_global(&ctx, "tune.cap", 100.0);
    set_global(&ctx, "tune.rate", 60.0);
    let layered: Vec<_> = (0..LAYERED)
        .map(|_| {
            let id = spawn(&ctx, "layered");
            // The per-entity run seeds each entity's state to the global
            // run's values, then refills at the capacity it now reads.
            if parameters == Parameters::PerEntity {
                let mut registry = ctx.registry.borrow_mut();
                let state = registry.entity_state_mut(id).unwrap();
                state.set("cap", 100.0);
                state.set("rate", 60.0);
                postretro_entities::components::defense_layers::refill_layers_on_respawn(
                    &mut registry,
                    id,
                );
            }
            id
        })
        .collect();
    for _ in 0..UNLAYERED {
        ctx.registry.borrow_mut().spawn(Transform::default());
    }
    let mut stepped = Duration::ZERO;
    let mut allocations = 0;
    for tick in 0..TICKS {
        if tick % HIT_EVERY == 0 {
            for &id in &layered {
                hit(&ctx, id, 30.0, None);
            }
        }
        let mut registry = ctx.registry.borrow_mut();
        registry.advance_host_tick();
        let probe = crate::alloc_probe::AllocSnapshot::arm();
        let start = Instant::now();
        recharge_defense_layers(&mut registry);
        stepped += start.elapsed();
        allocations += probe.allocs_since();
    }
    Run {
        per_tick: stepped / TICKS as u32,
        allocations,
    }
}

#[test]
#[ignore = "stress measurement: run with --release --ignored --nocapture"]
fn layer_step_stress_1000_layered_entities() {
    let baseline = run(Parameters::Literal);
    let global = run(Parameters::Global);
    let per_entity = run(Parameters::PerEntity);
    eprintln!(
        "layer step, {LAYERED} layered (2 layers) + {UNLAYERED} unlayered, {TICKS} ticks, hit every {HIT_EVERY}:\n  \
         literal baseline: {:?}/tick, {} allocations\n  \
         global reads at the node cap (shared): {:?}/tick, {} allocations\n  \
         own-state reads at the node cap (per entity): {:?}/tick, {} allocations",
        baseline.per_tick,
        baseline.allocations,
        global.per_tick,
        global.allocations,
        per_entity.per_tick,
        per_entity.allocations,
    );
    for (label, run) in [
        ("literal", &baseline),
        ("global", &global),
        ("per-entity", &per_entity),
    ] {
        assert_eq!(run.allocations, 0, "the {label} step allocates nothing");
    }
}
