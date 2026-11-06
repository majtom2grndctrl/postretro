// Property test of the compaction fit claim, over arrivals and departures on near-full pools.
//
// Blocks are cut guillotine-style from the pool's own layers, so the whole set
// nearly always packs and a history can drive the pool to within a few texels
// of full. The cuts make every shape at once: full-width rows, full-height
// columns, blocks over half a layer and slivers. A layer may cut all one way,
// which sets all-tall blocks against all-wide ones. A band block may sit in
// any gap, which fragments the pool further.

use std::cell::Cell;
use std::cmp::Reverse;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

use super::gpu_mirror::GpuMirror;
use super::pool_test_fixtures::*;
use super::*;

const MAX_LAYERS: u32 = 5;

#[derive(Debug, Clone)]
struct Cut {
    /// Index into the pieces so far, wrapped.
    piece: u32,
    across: bool,
    /// Eighths of the piece's side.
    at: u32,
}

#[derive(Debug, Clone)]
struct Step {
    /// Bit `b` asks for block `b` as mandatory.
    arrive: u64,
    /// Bit `b` drops block `b` from every target.
    leave: u64,
    /// Bit `b` asks for block `b` as band.
    band: u64,
}

#[derive(Debug, Clone)]
struct History {
    layers: u32,
    /// Per layer: its cuts, and the one way they all run, if any.
    cuts: Vec<(Vec<Cut>, Option<bool>)>,
    steps: Vec<Step>,
}

fn cut() -> impl Strategy<Value = Cut> {
    (any::<u32>(), any::<bool>(), 1u32..8).prop_map(|(piece, across, at)| Cut { piece, across, at })
}

fn step() -> impl Strategy<Value = Step> {
    (any::<[u64; 2]>(), any::<[u64; 3]>(), any::<[u64; 2]>()).prop_map(|(arrive, leave, band)| {
        Step {
            // Dense arrivals and sparse departures keep the pool near full.
            arrive: arrive[0] | arrive[1],
            leave: leave[0] & leave[1] & leave[2],
            band: band[0] & band[1],
        }
    })
}

fn history() -> impl Strategy<Value = History> {
    (2..=MAX_LAYERS).prop_flat_map(|layers| {
        (
            Just(layers),
            prop::collection::vec(
                (
                    prop::collection::vec(cut(), 1..6),
                    prop::option::weighted(0.5, any::<bool>()),
                ),
                layers as usize,
            ),
            prop::collection::vec(step(), 2..12),
        )
            .prop_map(|(layers, cuts, steps)| History {
                layers,
                cuts,
                steps,
            })
    })
}

fn extents_of(history: &History) -> Vec<(u32, u32)> {
    let mut blocks = Vec::new();
    for (cuts, one_way) in &history.cuts {
        let mut pieces = vec![(EDGE, EDGE)];
        for cut in cuts {
            let at = cut.piece as usize % pieces.len();
            let (w, h) = pieces[at];
            let across = one_way.unwrap_or(cut.across);
            let side = if across { w } else { h };
            let part = (side * cut.at / 8).next_multiple_of(ALIGN);
            if part == 0 || part >= side {
                continue;
            }
            if across {
                pieces[at] = (part, h);
                pieces.push((w - part, h));
            } else {
                pieces[at] = (w, part);
                pieces.push((w, h - part));
            }
        }
        blocks.extend(pieces);
    }
    blocks
}

/// Layers a from-scratch pack of `set` fills, tallest first, if it fits.
fn scratch_layers(extents: &[(u32, u32)], set: u64) -> Option<u32> {
    let mut demand: Vec<(u32, u32)> = (0..extents.len())
        .filter(|&b| set >> b & 1 != 0)
        .map(|b| extents[b])
        .collect();
    demand.sort_unstable_by_key(|&(w, h)| (Reverse(h), Reverse(w)));
    crate::lightmap_pool::place_all_resident(&demand, ALIGN, EDGE).map(|pool| pool.layer_count)
}

/// The claim's premise: `set` packs from scratch, tallest first, into `layers`.
fn packs(extents: &[(u32, u32)], set: u64, layers: u32) -> bool {
    scratch_layers(extents, set).is_some_and(|used| used <= layers)
}

#[derive(Debug, Default)]
struct Tally {
    histories: Cell<u64>,
    drains: Cell<u64>,
    compactions: Cell<u64>,
    histories_compacted: Cell<u64>,
    histories_parked: Cell<u64>,
    parked: Cell<u64>,
    built_over: Cell<u64>,
    /// Compactions that found no order with no layer spare, each ending its
    /// history. The planner may miss there and take logged growth.
    missed_no_spare: Cell<u64>,
    /// Compactions with a layer spare, and those that found no order: the
    /// claim says there are none.
    with_spare: Cell<u64>,
    missed_with_spare: Cell<u64>,
}

fn add(cell: &Cell<u64>, by: u64) {
    cell.set(cell.get() + by);
}

/// Run one history. Every drain whose mandatory set packs from scratch must
/// place with no growth, unless its compaction found no order, which ends the
/// history; and its copies must keep every texel.
fn run(history: &History, tally: &Tally) {
    let extents = extents_of(history);
    let count = extents.len();
    assert!(count <= 64);
    let Some(mut model) = LightmapPoolModel::new(extents.clone(), ALIGN, EDGE, history.layers)
    else {
        return;
    };
    let layers = model.layers();
    let mut gpu = GpuMirror::new(&model);
    let all = if count == 64 {
        u64::MAX
    } else {
        (1 << count) - 1
    };
    let mut mandatory_set = 0u64;
    let mut band_set = 0u64;
    let (mut compacted, mut parked) = (false, false);

    for step in &history.steps {
        let leaving = step.leave & all & (mandatory_set | band_set);
        let mut arriving = step.arrive & all & !leaving & !mandatory_set;
        // Shed arrivals until the premise holds: the claim is about sets
        // that pack, and shedding the last keeps the pool as full as it can.
        while arriving != 0 && !packs(&extents, (mandatory_set & !leaving) | arriving, layers) {
            arriving &= !(1 << (63 - arriving.leading_zeros()));
        }
        let next = (mandatory_set & !leaving) | arriving;
        if !packs(&extents, next, layers) {
            continue;
        }
        let banded = step.band & all & !next & !band_set & !leaving;
        let set: Vec<_> = (0..count as u32)
            .filter_map(|b| {
                if arriving >> b & 1 != 0 {
                    Some(mandatory(b))
                } else if banded >> b & 1 != 0 {
                    Some(band(b, b % 7))
                } else {
                    None
                }
            })
            .collect();
        let remove: Vec<u32> = (0..count as u32)
            .filter(|&b| leaving >> b & 1 != 0)
            .collect();
        let ready: Vec<u32> = (0..count as u32)
            .filter(|&b| (arriving | banded) >> b & 1 != 0 && !model.is_resident(b))
            .collect();

        model.plan_drain(DrainRequest {
            pool_cap_layers: layers,
            target_reset: None,
            target_set: &set,
            target_remove: &remove,
            ready: &ready,
        });
        let plan = model.plan();
        add(&tally.drains, 1);
        let spare = scratch_layers(&extents, next).map_or(0, |used| layers - used);
        if plan.compaction_rolled_back || matches!(plan.repack, Some(RepackKind::Compaction(_))) {
            add(&tally.with_spare, u64::from(spare >= 1));
        }
        if plan.compaction_rolled_back {
            if spare >= 1 {
                add(&tally.missed_with_spare, 1);
            } else {
                add(&tally.missed_no_spare, 1);
            }
            return;
        }
        assert_eq!(plan.growth, None, "{next:b} packs, so it needs no growth");
        assert!(plan.deferred.is_empty(), "nothing waits on a retirement");
        for b in 0..count as u32 {
            if next >> b & 1 != 0 {
                assert!(
                    model.is_resident(b),
                    "mandatory {b} not placed, set {next:b}"
                );
            }
        }
        if let Some(RepackKind::Compaction(stats)) = plan.repack {
            add(&tally.compactions, 1);
            add(&tally.parked, u64::from(stats.parked));
            add(&tally.built_over, u64::from(stats.built_over));
            compacted = true;
            parked |= stats.parked > 0;
        }
        gpu.execute(model.plan());
        gpu.assert_matches(&model);
        assert_eq!(model.texture_allocations(), 1, "no second pool");
        assert_disjoint_within_layers(&model);

        mandatory_set = next;
        let resident =
            (0..count as u32).fold(0, |bits, b| bits | u64::from(model.is_resident(b)) << b);
        band_set = (band_set | banded) & !leaving & !arriving & resident;
    }
    add(&tally.histories, 1);
    add(&tally.histories_compacted, u64::from(compacted));
    add(&tally.histories_parked, u64::from(parked));
}

fn run_cases(cases: u32) -> Tally {
    let config = Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    };
    let rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);
    let tally = Tally::default();
    TestRunner::new_with_rng(config, rng)
        .run(&history(), |history| {
            run(&history, &tally);
            Ok(())
        })
        .unwrap();
    println!(
        "compaction: {} histories, {} drains, {} compactions in {} histories, parking in {}, {} \
         parks, {} target layers built over a layer, {} compactions with a layer spare, {} missed \
         with one, {} missed with none",
        tally.histories.get(),
        tally.drains.get(),
        tally.compactions.get(),
        tally.histories_compacted.get(),
        tally.histories_parked.get(),
        tally.parked.get(),
        tally.built_over.get(),
        tally.with_spare.get(),
        tally.missed_with_spare.get(),
        tally.missed_no_spare.get(),
    );
    tally
}

#[test]
fn compaction_places_any_set_that_packs_from_scratch() {
    let tally = run_cases(2048);
    // A set with a layer spare must place. One with none may miss and take
    // logged growth, so those misses are reported above, not asserted.
    assert_eq!(tally.missed_with_spare.get(), 0);
    // A generator that stops reaching compaction proves nothing.
    assert!(
        tally.histories_compacted.get() >= 256,
        "only {} histories compacted",
        tally.histories_compacted.get()
    );
    assert!(
        tally.histories_parked.get() >= 64,
        "only {} histories parked a block",
        tally.histories_parked.get()
    );
}

/// The long hunt for layouts the planner cannot order. On demand: it counts
/// them as evidence for the fit claim, and a routine gate could not afford it.
#[test]
#[ignore = "stuck-layout hunt; set POSTRETRO_COMPACTION_HUNT_CASES"]
fn compaction_hunt_counts_layouts_with_no_order_found() {
    let cases = std::env::var("POSTRETRO_COMPACTION_HUNT_CASES")
        .ok()
        .and_then(|cases| cases.parse().ok())
        .unwrap_or(1_000_000);
    run_cases(cases);
}
