// M5's layer stress run: the layer step's CPU time per tick and its heap
// allocations over a synthetic registry, expression parameters at the node
// cap against an all-literal baseline. `#[ignore]`d: run it on purpose with
//   cargo test --release -p postretro-sim --lib -- --ignored --nocapture layer_step_stress
// See: plans E16 §Acceptance (Manual: layer stress run) · research.md §Per-tick cost

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

/// A balanced sum of 128 leaves (255 nodes, depth 8), at the node cap: one
/// `live` read and 127 reads of `tune.zero`, so it evaluates to `live`.
fn capped(live: &str) -> NumberOrIr {
    fn tree(leaves: &mut dyn Iterator<Item = IrNode>, size: usize) -> IrNode {
        if size == 1 {
            return leaves.next().unwrap();
        }
        IrNode::Add {
            a: Box::new(tree(leaves, size / 2)),
            b: Box::new(tree(leaves, size / 2)),
        }
    }
    let mut leaves =
        std::iter::once(input(live)).chain(std::iter::repeat_with(|| input("tune.zero")));
    NumberOrIr::Ir(tree(&mut leaves, 128))
}

fn entry(name: &str, expressions: bool) -> DefenseLayerEntry {
    let (capacity, delay, rate) = if expressions {
        (capped("tune.cap"), capped("tune.zero"), capped("tune.rate"))
    } else {
        (lit(100.0), lit(0.0), lit(60.0))
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

fn run(expressions: bool) -> Run {
    let ctx: ScriptCtx = session(vec![(
        "layered",
        false,
        HealthDescriptor {
            max: 100.0,
            layers: vec![entry("shield", expressions), entry("armor", expressions)],
            ..Default::default()
        },
    )]);
    set_global(&ctx, "tune.cap", 100.0);
    set_global(&ctx, "tune.rate", 60.0);
    let layered: Vec<_> = (0..LAYERED).map(|_| spawn(&ctx, "layered")).collect();
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
    let baseline = run(false);
    let expressions = run(true);
    eprintln!(
        "layer step, {LAYERED} layered (2 layers) + {UNLAYERED} unlayered, {TICKS} ticks, hit every {HIT_EVERY}:\n  \
         literal baseline: {:?}/tick, {} allocations\n  \
         expressions at the node cap: {:?}/tick, {} allocations",
        baseline.per_tick, baseline.allocations, expressions.per_tick, expressions.allocations,
    );
    assert_eq!(
        baseline.allocations, 0,
        "the literal step allocates nothing"
    );
    assert_eq!(
        expressions.allocations, 0,
        "the expression step allocates nothing"
    );
}
