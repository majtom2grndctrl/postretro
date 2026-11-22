// Layer resolution admits at most a fixed number of layers at once: never
// more in flight than its cap, the same layers whatever the cap, no stall
// with the governor at one permit, and the layer phase priced over the
// largest layers in flight together, a cache hit's payload included.
// See: context/lib/build_pipeline.md §CLI flags (`--working-set-max-size`)

use std::path::Path;
use std::sync::Arc;

use super::SparseLayer;
use super::layer::{layer_bytes, layer_transient_bytes};
use super::layer_set::{LAYERS_IN_FLIGHT_KIND, LayerCache, LayerPlan, resolve_layers_capped};
use super::patches::PatchSet;
use super::reach::{PatchReachIndex, REACH_INDEX_BYTES_PER_PATCH};
use super::test_scene::{Scene, point};
use super::working_set::in_flight_scratch;
use crate::bake_control::BakeControl;
use crate::cache::StageCache;
use crate::governor::Governor;
use crate::live_bytes::measured;
use crate::map_data::MapLight;
use crate::reporter::StageProgress;
use crate::sh_bake::static_light_refs;
use crate::texel_footprint::NO_EMBEDDED_EMITTERS;

/// Ten distinct lights across the room: more planned layers than workers.
fn ten_lights() -> Vec<MapLight> {
    (0..10)
        .map(|at| {
            point(
                [1.0 + 0.5 * f64::from(at), 2.0, 1.0 + 0.3 * f64::from(at)],
                0.1,
                4.0,
            )
        })
        .collect()
}

/// What one resolution of `lights` over `cache` held: the layers, the most
/// in flight at once, the ledger's peak, and the price of that peak at
/// `cap`: the finished layers, the reach index and the largest `cap` lights'
/// scratch.
struct Resolved {
    layers: Vec<SparseLayer>,
    in_flight: u64,
    /// The largest cache hit's payload held at once: zero when no layer hit.
    hit_payload: u64,
    peak: u64,
    priced: u64,
}

fn resolve(
    scene: &Scene,
    lights: &[MapLight],
    cache: Option<&Path>,
    cap: usize,
    threads: usize,
    control: &BakeControl,
) -> Resolved {
    let patches = PatchSet::build(&scene.geometry);
    scene.with_ctx(lights, |ctx| {
        let refs = static_light_refs(ctx);
        let reach = PatchReachIndex::new(patches.patches()).counts_in_reach(&refs);
        let plan = LayerPlan::new(&refs, &reach, &NO_EMBEDDED_EMITTERS);
        assert!(
            plan.layers.len() > threads,
            "more planned layers than workers"
        );
        let cache = cache.map(|dir| StageCache::new(dir).expect("cache"));
        let layer_cache = cache.as_ref().map(|cache| LayerCache {
            cache,
            keys: scene.keys(),
        });
        let (layers, report) = measured(threads, || {
            resolve_layers_capped(
                &ctx.ray_ctx(),
                &patches,
                &refs,
                &plan,
                layer_cache.as_ref(),
                control,
                cap,
            )
        });
        let priced = reach.iter().map(|&r| layer_bytes(r).unwrap()).sum::<u64>()
            + patches.len() as u64 * REACH_INDEX_BYTES_PER_PATCH
            + in_flight_scratch(
                reach
                    .iter()
                    .map(|&r| layer_transient_bytes(r).unwrap())
                    .collect(),
                cap,
            )
            .unwrap();
        Resolved {
            layers,
            in_flight: report
                .high_water
                .get(LAYERS_IN_FLIGHT_KIND)
                .copied()
                .unwrap_or(0),
            hit_payload: report
                .high_water
                .get("patch layer cache payload")
                .copied()
                .unwrap_or(0),
            peak: report.peak,
            priced,
        }
    })
}

// With the cap below the worker count on more planned layers than workers,
// on one worker and four, cold, warm over an empty cache and warm with some
// layers hit: never more layers in flight than the cap, the same layers as
// one unbounded batch, and the layer phase's price (every finished layer,
// the reach index and the largest cap's scratch, a hit's raw entry among
// them) at or above the ledger's peak (L1).
#[test]
fn layer_resolution_never_exceeds_its_cap() {
    let scene = Scene::new();
    let lights = ten_lights();
    let control = BakeControl::unrestricted();
    let whole = resolve(&scene, &lights, None, lights.len(), 4, &control).layers;
    for threads in [1, 4] {
        for cap in [1, 2, 3] {
            let cold = resolve(&scene, &lights, None, cap, threads, &control);
            let empty = tempfile::tempdir().expect("cache");
            let warm = resolve(&scene, &lights, Some(empty.path()), cap, threads, &control);
            // Half the lights primed: those layers hit, the rest shade.
            let partial = tempfile::tempdir().expect("cache");
            resolve(
                &scene,
                &lights[..5],
                Some(partial.path()),
                cap,
                threads,
                &control,
            );
            let hit = resolve(
                &scene,
                &lights,
                Some(partial.path()),
                cap,
                threads,
                &control,
            );
            assert!(
                hit.hit_payload > 0 && cold.hit_payload == 0,
                "x{threads} cap {cap}: the primed layers hit, the cold ones cannot"
            );
            for (mode, resolved) in [("cold", cold), ("warm empty", warm), ("some hit", hit)] {
                let what = format!("x{threads} cap {cap} {mode}");
                assert!(
                    (1..=cap as u64).contains(&resolved.in_flight),
                    "{what}: {} layers in flight",
                    resolved.in_flight
                );
                assert_eq!(resolved.layers, whole, "{what}: the same layers");
                assert!(
                    resolved.priced >= resolved.peak,
                    "{what}: priced {} B under {} B",
                    resolved.priced,
                    resolved.peak
                );
            }
        }
    }
}

// With the governor at one permit, layer resolution completes at a cap of
// one and at a cap past the worker count: no layer waits on a permit its own
// batch holds (L2).
#[test]
fn layer_resolution_completes_at_one_permit() {
    let scene = Scene::new();
    let lights = ten_lights();
    let governor = Arc::new(Governor::new(1, false));
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::clone(&governor), &progress);
    for cap in [1, 8] {
        let resolved = resolve(&scene, &lights, None, cap, 4, &control);
        assert_eq!(
            resolved.layers.len(),
            lights.len(),
            "cap {cap}: every layer"
        );
    }
    assert_eq!(governor.active(), 0);
}
