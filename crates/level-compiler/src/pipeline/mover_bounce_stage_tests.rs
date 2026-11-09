// The SH base gate against the mover bounce half it prices (bake row B14):
// the measured peak of the posed scenes, the bounce bake and the states, cold
// and warm, and the patch cache's single drop after the mover bake.
// See: context/lib/build_pipeline.md §Compiler pipeline (working-set gate)

use std::path::{Path, PathBuf};

use postretro_level_format::probe_depth_tiles::MAX_PROBE_DEPTH_TILES;

use super::{BounceHalfPlan, BouncePlanInputs, plan_bounce_half};
use crate::bake_control::BakeControl;
use crate::cache::StageCache;
use crate::fixture_pipeline::{FixturePipeline, fixture_path, load_map};
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::live_bytes::{Report, measured};
use crate::mover_light_gate::MoverPoses;
use crate::mover_probe_channel::MoverBounceRequest;
use crate::sh_bake::{ShBakeCtx, ShConfig, animation_table_bytes, static_light_refs};
use crate::surface_radiance::{
    DEFAULT_BOUNCE_FILL, MoverBounceOutcome, ShBase, bake_sh_base_with_movers, plan_sh_base,
    stage_events,
};
use crate::texel_footprint::NO_EMBEDDED_EMITTERS;

/// The closet with its door's `light_bounces`, absent or `n`, written under
/// `dir`.
fn closet(dir: &Path, bounces: Option<u32>) -> PathBuf {
    let text = std::fs::read_to_string(fixture_path("mover-closet")).expect("closet map");
    let text = match bounces {
        Some(n) => text.replace(
            "\"name\" \"closet_door\"",
            &format!("\"name\" \"closet_door\"\n\"light_bounces\" \"{n}\""),
        ),
        None => text,
    };
    let path = dir.join(format!("closet-{bounces:?}.map"));
    std::fs::write(&path, text).expect("write closet");
    path
}

/// Run `f` over the fixture's SH bake context and mover poses.
fn with_ctx<R>(fixture: &FixturePipeline, f: impl FnOnce(&ShBakeCtx<'_>, &MoverPoses) -> R) -> R {
    let static_lights = StaticBakedLights::from_lights(&fixture.lights);
    let animated_lights = AnimatedBakedLights::from_lights(&fixture.lights);
    let ctx = ShBakeCtx {
        bounce_materials: None,
        bvh: &fixture.bvh,
        primitives: &fixture.primitives,
        geometry: &fixture.geometry,
        tree: &fixture.tree,
        exterior_leaves: &fixture.exterior_leaves,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: fixture.lights.len(),
    };
    let poses = MoverPoses::resolve(&fixture.kinematic_movers, &fixture.kinematic_waypoints);
    f(&ctx, &poses)
}

const CONFIG: ShConfig = ShConfig { probe_spacing: 1.0 };

/// The bounce half's plan as the pipeline makes it.
fn plan_half(
    fixture: &FixturePipeline,
    ctx: &ShBakeCtx<'_>,
    poses: &MoverPoses,
    fill: f32,
) -> BounceHalfPlan {
    let direct_lights: Vec<_> = ctx
        .static_lights
        .entries()
        .iter()
        .map(|entry| entry.light)
        .chain(
            ctx.animated_lights
                .entries()
                .iter()
                .map(|entry| entry.light),
        )
        .collect();
    plan_bounce_half(BouncePlanInputs {
        poses,
        movers: &fixture.kinematic_movers,
        texture_names: &fixture.geometry.texture_names,
        geometry: &fixture.geometry,
        tree: &fixture.tree,
        exterior_leaves: &fixture.exterior_leaves,
        probe_spacing: CONFIG.probe_spacing,
        bounce_fill: fill,
        direct_lights: &direct_lights,
        uncompressed: false,
        tile_cap: MAX_PROBE_DEPTH_TILES,
    })
    .expect("the bounce half plans")
}

/// The SH base gate, priced with `plan`, and the SH base bake with its
/// request.
fn bake_base(
    ctx: &ShBakeCtx<'_>,
    plan: &BounceHalfPlan,
    request: &MoverBounceRequest,
    fill: f32,
    cache: Option<&StageCache>,
) -> (u64, ShBase, MoverBounceOutcome) {
    let (handoff, mut projection) = plan_sh_base(
        ctx.geometry,
        &static_light_refs(ctx),
        animation_table_bytes(ctx.animated_lights, ctx.total_light_count),
        CONFIG.probe_spacing,
        fill,
        u64::MAX,
    )
    .expect("priced");
    projection.price_movers(&plan.cost);
    let (base, outcome) = bake_sh_base_with_movers(
        ctx,
        &CONFIG,
        handoff,
        &NO_EMBEDDED_EMITTERS,
        cache,
        &BakeControl::unrestricted(),
        MAX_PROBE_DEPTH_TILES,
        request,
    );
    (projection.estimated_peak_bytes, base, outcome)
}

/// What one measured bake of the bounce half saw.
struct Measured {
    predicted: u64,
    report: Report,
    events: Vec<String>,
    logs: Vec<String>,
}

/// Plan the bounce half and the SH base gate as the pipeline does, then
/// bake the SH base with the movers, all on one measured pool of `threads`.
fn bake(
    fixture: &FixturePipeline,
    fill: f32,
    cache: Option<&StageCache>,
    threads: usize,
) -> Measured {
    with_ctx(fixture, |ctx, poses| {
        let ((predicted, events, logs), report) = measured(threads, || {
            // The measured pool's thread runs the stage: capture its lines.
            let logs = postretro_test_log_capture::LogCapture::start();
            stage_events::take();
            let plan = plan_half(fixture, ctx, poses, fill);
            assert_eq!(plan.request.jobs.len(), 1, "the closet's door is analysed");
            let (predicted, base, outcome) = bake_base(ctx, &plan, &plan.request, fill, cache);
            drop((base, outcome, plan));
            let logs = logs.records().into_iter().map(|r| r.message).collect();
            (predicted, stage_events::take(), logs)
        });
        Measured {
            predicted,
            report,
            events,
            logs,
        }
    })
}

fn open(dir: &Path) -> StageCache {
    StageCache::new(dir).expect("cache")
}

// Bake row B14: on the closet, with the door re-baking every bounce, none
// (depth only) or two, at fill 1 and 0, on one worker and four, the SH base
// gate's prediction with the mover channel priced covers the measured peak
// cold, warm over an empty cache, warm on an id-34 hit whose door misses its
// memo (another `light_bounces` primed the cache), and warm on a full hit.
// Cold, the mover bake finishes before the patch cache drops, which it does
// once.
#[test]
fn the_sh_base_gate_covers_the_mover_bounce_peak_cold_and_warm() {
    let maps = tempfile::tempdir().expect("maps dir");
    let mut rows = Vec::new();
    for (fill, bounces, other) in [
        (DEFAULT_BOUNCE_FILL, None, Some(2)),
        (DEFAULT_BOUNCE_FILL, Some(0), None),
        (DEFAULT_BOUNCE_FILL, Some(2), Some(0)),
        (0.0, None, Some(0)),
    ] {
        let fixture = load_map(&closet(maps.path(), bounces));
        let primer = load_map(&closet(maps.path(), other));
        for threads in [1, 4] {
            let cold = bake(&fixture, fill, None, threads);
            let order: Vec<&str> = cold
                .events
                .iter()
                .map(String::as_str)
                .filter(|event| {
                    [
                        stage_events::MOVER_BOUNCE_DONE,
                        stage_events::PATCH_CACHE_DROPPED,
                    ]
                    .contains(event)
                })
                .collect();
            assert_eq!(
                order,
                [
                    stage_events::MOVER_BOUNCE_DONE,
                    stage_events::PATCH_CACHE_DROPPED
                ],
                "cold: the mover bake runs before the patch cache's one drop"
            );
            let empty = tempfile::tempdir().expect("cache");
            let warm_empty = bake(&fixture, fill, Some(&open(empty.path())), threads);
            let full_hit = bake(&fixture, fill, Some(&open(empty.path())), threads);
            assert!(
                full_hit
                    .logs
                    .iter()
                    .any(|line| line.contains("mover_probe_bounce hit (1 mover(s))")),
                "{:?}",
                full_hit.logs
            );
            let primed = tempfile::tempdir().expect("cache");
            bake(&primer, fill, Some(&open(primed.path())), threads);
            let memo_miss = bake(&fixture, fill, Some(&open(primed.path())), threads);
            assert!(
                memo_miss
                    .logs
                    .iter()
                    .any(|line| line.contains("sh_base_volume hit"))
                    && memo_miss
                        .logs
                        .iter()
                        .any(|line| line.contains("mover_probe_bounce miss (1 of 1")),
                "an id-34 hit whose door re-bakes: {:?}",
                memo_miss.logs
            );
            for (mode, measured) in [
                ("cold", cold),
                ("warm empty", warm_empty),
                ("id-34 hit, memo miss", memo_miss),
                ("full hit", full_hit),
            ] {
                rows.push(format!(
                    "fill {fill} bounces {bounces:?} x{threads} {mode:<22} predicted {:>10} \
                     measured {:>10} ({:.2}x) at peak {:?}",
                    measured.predicted,
                    measured.report.peak,
                    measured.predicted as f64 / measured.report.peak.max(1) as f64,
                    measured.report.at_peak,
                ));
                assert!(
                    measured.predicted >= measured.report.peak,
                    "the gate predicts less than the bake holds: {}",
                    rows.last().expect("row")
                );
            }
        }
    }
    for row in &rows {
        eprintln!("{row}");
    }
}

// Bake row B14: over budget, the SH base gate refuses the mover-priced
// projection by name, where the same map priced without its mover is
// admitted.
#[test]
fn a_budget_the_mover_channel_exceeds_refuses_with_the_sh_base_gates_error() {
    let maps = tempfile::tempdir().expect("maps dir");
    let fixture = load_map(&closet(maps.path(), None));
    with_ctx(&fixture, |ctx, poses| {
        let plan = plan_half(&fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
        let gate = |budget: u64| {
            plan_sh_base(
                &fixture.geometry,
                &static_light_refs(ctx),
                animation_table_bytes(ctx.animated_lights, ctx.total_light_count),
                1.0,
                DEFAULT_BOUNCE_FILL,
                budget,
            )
            .expect("priced")
            .1
        };
        let mut priced = gate(u64::MAX);
        let static_peak = priced.estimated_peak_bytes;
        priced.price_movers(&plan.cost);
        let mover_peak = priced.estimated_peak_bytes;
        assert!(mover_peak > static_peak, "{static_peak} → {mover_peak}");

        let budget = mover_peak - 1;
        let mut projection = gate(budget);
        projection
            .admit()
            .expect("without the mover, the budget admits");
        projection.price_movers(&plan.cost);
        let error = projection
            .admit()
            .expect_err("the mover channel exceeds the budget");
        let message = error.to_string();
        assert!(
            message.contains("SH base working-set gate refused before patch shading")
                && message.contains(&format!("estimated peak {mover_peak} bytes"))
                && message.contains("mover bounce"),
            "{message}"
        );
    });
}

/// The closet baked with its door's `light_bounces` at `bounces`, cold.
fn closet_states(
    maps: &Path,
    bounces: Option<u32>,
) -> (u32, Vec<crate::mover_probe_channel::MoverBounceStates>) {
    let fixture = load_map(&closet(maps, bounces));
    with_ctx(&fixture, |ctx, poses| {
        let plan = plan_half(&fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
        let job_bounces = plan.request.jobs[0].bounces;
        let (_, _, outcome) = bake_base(ctx, &plan, &plan.request, DEFAULT_BOUNCE_FILL, None);
        (job_bounces, outcome.movers)
    })
}

// The `light_bounces` key: absent re-bakes id 34's every bounce (five at
// fill 1) and a count past it bakes the same; two re-bakes two and keeps
// the same closed-pose depth; zero stores no bounce entry, yet its
// depth-only trace bins the same closed-pose depth as the full gather.
#[test]
fn light_bounces_zero_bakes_depth_alone_and_a_count_past_id_34s_bakes_id_34s() {
    let maps = tempfile::tempdir().expect("maps dir");
    let (every_n, every) = closet_states(maps.path(), None);
    let (past_n, past) = closet_states(maps.path(), Some(99));
    let (two_n, two) = closet_states(maps.path(), Some(2));
    let (zero_n, zero) = closet_states(maps.path(), Some(0));
    assert_eq!((every_n, past_n, two_n, zero_n), (5, 5, 2, 0));
    assert_eq!(past, every, "a count past id 34's bakes id 34's");
    assert!(!every[0].bounce.is_empty() && !every[0].depth.is_empty());
    assert!(!two[0].bounce.is_empty());
    assert_eq!(two[0].depth, every[0].depth, "depth does not depend on N");
    assert!(zero[0].bounce.is_empty(), "zero stores no bounce entry");
    assert_eq!(
        zero[0].depth, every[0].depth,
        "the depth-only trace bins the full gather's closed-pose depth"
    );
}

// Bake row B8: a test-only re-bake of every valid probe at each terminus,
// on the closet and the setpiece, differs from id 34 beyond the
// affected-probe rule (each probe's own BC6H round-trip bar and the
// perceptual floor), recomputed here from the kept tiles, exactly at the
// probes each mover's bounce states store. Task 11c: each stored probe
// stores a terminus's state exactly where that terminus's re-bake leaves the
// round-trip bar alone. Heavy: whole posed re-bakes.
#[test]
#[ignore = "re-bakes every valid probe at both termini of every mover; run on demand"]
fn a_full_re_bake_differs_from_id_34_beyond_the_bar_only_where_the_domain_admits() {
    use std::collections::{BTreeMap, BTreeSet};

    use crate::mover_probe_channel::{
        BOUNCE_FLOOR_LEVELS, compact_slots, compact_tile, display_levels, max_abs_difference,
        round_trip_error,
    };

    for name in ["mover-closet", "mover-setpiece"] {
        let fixture = crate::fixture_pipeline::load_fixture(name);
        with_ctx(&fixture, |ctx, poses| {
            let mut plan = plan_half(&fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
            let mut request = std::mem::take(&mut plan.request);
            assert!(!request.jobs.is_empty(), "{name} poses a mover");
            // Production's states, then the same poses with every tile kept.
            let (_, _, stored) = bake_base(ctx, &plan, &request, DEFAULT_BOUNCE_FILL, None);
            request.keep_every_tile = true;
            request.keep_poses = true;
            let (_, base, outcome) = bake_base(ctx, &plan, &request, DEFAULT_BOUNCE_FILL, None);
            assert_eq!(outcome.poses.len(), 2 * request.jobs.len());
            let layout = crate::sh_bake::probe_grid_layout_of(
                ctx.geometry,
                ctx.tree,
                ctx.exterior_leaves,
                CONFIG.probe_spacing,
            );
            let valid = layout.validity.iter().filter(|&&v| v != 0).count();
            let slots = compact_slots(&layout);
            let dim = base.section.tile_dimension as usize;
            let mut beyond: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
            let mut past_bar: BTreeMap<(u32, usize), BTreeSet<u32>> = BTreeMap::new();
            for pose in &outcome.poses {
                assert_eq!(
                    pose.probes.len(),
                    valid,
                    "{name}: every valid probe re-baked"
                );
                let set = beyond.entry(pose.mover_id).or_default();
                let bar_set = past_bar
                    .entry((pose.mover_id, pose.terminus.index()))
                    .or_default();
                for probe in &pose.probes {
                    let tile = probe.tile.as_ref().expect("every tile kept");
                    let id34 = compact_tile(&base.section, slots[probe.probe as usize] as usize);
                    let difference = max_abs_difference(tile, &id34);
                    if difference > round_trip_error(&id34, dim, false) {
                        bar_set.insert(probe.probe);
                        if display_levels(tile, &id34) > BOUNCE_FLOOR_LEVELS {
                            set.insert(probe.probe);
                        }
                    }
                }
            }
            assert_eq!(stored.movers.len(), request.jobs.len());
            for states in &stored.movers {
                let stored: BTreeSet<u32> = states.bounce.iter().map(|probe| probe.probe).collect();
                let expected = beyond.remove(&states.mover_id).unwrap_or_default();
                eprintln!(
                    "{name} mover #{}: {} probe(s) past the rule, {} stored, of {valid} valid",
                    states.mover_id,
                    expected.len(),
                    stored.len()
                );
                assert_eq!(
                    stored, expected,
                    "{name} mover #{}: stored probes are the full re-bake's beyond the rule",
                    states.mover_id
                );
                let mut one_sided = 0;
                for probe in &states.bounce {
                    for terminus in 0..2 {
                        let leaves_bar = past_bar
                            .get(&(states.mover_id, terminus))
                            .is_some_and(|set| set.contains(&probe.probe));
                        assert_eq!(
                            probe.stored[terminus], leaves_bar,
                            "{name} mover #{} probe {} terminus {terminus}: a state is stored \
                             exactly where it leaves the round-trip bar",
                            states.mover_id, probe.probe
                        );
                    }
                    one_sided += usize::from(probe.stored != [true, true]);
                }
                eprintln!(
                    "{name} mover #{}: {one_sided} of {} stored probe(s) store one terminus",
                    states.mover_id,
                    states.bounce.len()
                );
            }
            assert!(
                beyond.values().all(BTreeSet::is_empty),
                "{name}: {beyond:?}"
            );
        });
    }
}
