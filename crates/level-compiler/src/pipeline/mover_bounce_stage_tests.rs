// The SH base gate against the mover bounce half it prices:
// the measured peak of the posed scenes, the bounce bake and the states, cold
// and warm, and the patch cache's single drop after the mover bake.
// See: context/lib/build_pipeline.md §Compiler pipeline (working-set gate)

use std::path::{Path, PathBuf};
use std::sync::Arc;

use postretro_level_format::probe_depth_tiles::MAX_PROBE_DEPTH_TILES;

use super::{BounceHalfPlan, BouncePlanInputs, plan_bounce_half};
use crate::bake_control::BakeControl;
use crate::bake_fidelity::{BakeFidelity, BakeMode};
use crate::cache::StageCache;
use crate::fixture_pipeline::{FixturePipeline, fixture_path, load_map};
use crate::governor::Governor;
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::live_bytes::{Report, measured};
use crate::mover_light_gate::MoverPoses;
use crate::mover_probe_channel::{MoverBounceRequest, PosedMovers};
use crate::reporter::StageProgress;
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

/// Run `f` over the fixture's SH bake context and mover poses, placed along
/// each path as the pipeline places them.
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
    let poses = fixture.placed_mover_poses(CONFIG.probe_spacing);
    f(&ctx, &poses)
}

const CONFIG: ShConfig = ShConfig { probe_spacing: 1.0 };

/// The bounce half's plan as the pipeline makes it, at high fidelity: each
/// mover's own `light_bounces`.
fn plan_half(
    fixture: &FixturePipeline,
    ctx: &ShBakeCtx<'_>,
    poses: &MoverPoses,
    fill: f32,
) -> BounceHalfPlan {
    plan_half_at(
        fixture,
        ctx,
        poses,
        fill,
        BakeMode::Interactive(BakeFidelity::High),
    )
}

/// [`plan_half`] for a build baking `mode`.
fn plan_half_at(
    fixture: &FixturePipeline,
    ctx: &ShBakeCtx<'_>,
    poses: &MoverPoses,
    fill: f32,
    mode: BakeMode,
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
    let posed = PosedMovers::resolve(
        poses,
        &fixture.kinematic_movers,
        &fixture.geometry.texture_names,
    )
    .expect("the fixture's movers pose");
    plan_bounce_half(BouncePlanInputs {
        poses,
        posed: &posed,
        movers: &fixture.kinematic_movers,
        layout: &fixture.probe_layout(CONFIG.probe_spacing),
        probe_spacing: CONFIG.probe_spacing,
        bounce_fill: fill,
        direct_lights: &direct_lights,
        uncompressed: false,
        tile_cap: MAX_PROBE_DEPTH_TILES,
        mode,
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
/// bake the SH base with the movers, all on one measured pool of `threads`,
/// at high fidelity on the closet, whose door alone is analysed.
fn bake(
    fixture: &FixturePipeline,
    fill: f32,
    cache: Option<&StageCache>,
    threads: usize,
) -> Measured {
    bake_at(
        fixture,
        fill,
        cache,
        threads,
        BakeMode::Interactive(BakeFidelity::High),
        1,
    )
}

/// [`bake`] for a build baking `mode`, with `movers` movers analysed.
fn bake_at(
    fixture: &FixturePipeline,
    fill: f32,
    cache: Option<&StageCache>,
    threads: usize,
    mode: BakeMode,
    movers: usize,
) -> Measured {
    with_ctx(fixture, |ctx, poses| {
        let ((predicted, events, logs), report) = measured(threads, || {
            // The measured pool's thread runs the stage: capture its lines.
            let logs = postretro_test_log_capture::LogCapture::start();
            stage_events::take();
            let plan = plan_half_at(fixture, ctx, poses, fill, mode);
            assert_eq!(plan.request.jobs.len(), movers, "every mover is analysed");
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

// On the closet, with the door re-baking every bounce, none
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
                    .any(|line| line.contains("mover_probe_bounce hit (")
                        && line.ends_with("of 1 mover(s))")),
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
                        .any(|line| line.contains("mover_probe_bounce miss (")
                            && line.ends_with("over 1 of 1 mover(s))")),
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

// In process, on the setpiece, whose three movers each bake mid poses (the
// big room's thick slabs among them), at `low` and at `high`, on four
// workers, the SH base gate's
// prediction with the mover channel priced covers the measured peak cold,
// warm over an empty cache, and on a full memo hit. Every pose is priced:
// the posed scenes, one pose's posed bounce and later-bounce buffers, a mid
// pose's near-field states and gather, the mover-absent later bounces, and
// the raw states one mover's poses hold before it reduces. A `low` build
// bakes fewer posed bounces than it is priced for, so its bound is looser.
#[test]
fn the_sh_base_gate_covers_every_poses_bake_on_the_setpiece_at_either_fidelity() {
    let fixture = load_map(&fixture_path("mover-setpiece"));
    let mut rows = Vec::new();
    // The closet's test covers one worker against four; a single-worker
    // setpiece bake costs minutes.
    let threads = 4;
    for mode in [BakeMode::Interactive(BakeFidelity::Low), BakeMode::Release] {
        let cold = bake_at(&fixture, DEFAULT_BOUNCE_FILL, None, threads, mode, 3);
        let empty = tempfile::tempdir().expect("cache");
        let warm = |cache: &StageCache| {
            bake_at(&fixture, DEFAULT_BOUNCE_FILL, Some(cache), threads, mode, 3)
        };
        let warm_empty = warm(&open(empty.path()));
        let full_hit = warm(&open(empty.path()));
        assert!(
            full_hit
                .logs
                .iter()
                .any(|line| line.contains("mover_probe_bounce hit (")
                    && line.ends_with("of 3 mover(s))")),
            "{:?}",
            full_hit.logs
        );
        for (cache, measured) in [
            ("cold", cold),
            ("warm empty", warm_empty),
            ("full hit", full_hit),
        ] {
            rows.push(format!(
                "{mode:?} x{threads} {cache:<10} predicted {:>10} measured {:>10} ({:.2}x) \
                 at peak {:?}",
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
    for row in &rows {
        eprintln!("{row}");
    }
}

// Over budget, the SH base gate refuses the mover-priced
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

// No geometry knot is dropped to meet a count: each one is a pose the
// bounce half plans a scene for, and the SH base gate prices it, refusing
// by name a budget the geometry knots alone push the mover channel past.
#[test]
fn the_sh_base_gate_prices_every_geometry_knot_and_refuses_by_name_below_it() {
    let fixture = load_map(&fixture_path("enemy-movers"));
    with_ctx(&fixture, |ctx, placed| {
        let geometry: u32 = placed
            .analysed
            .iter()
            .map(|mover| mover.knot_counts.geometry)
            .sum();
        assert!(
            geometry > 0,
            "the sweeper and crushers place geometry knots"
        );
        // The same placement with no probes to enclose: even poses alone.
        let mut even_only =
            MoverPoses::resolve(&fixture.kinematic_movers, &fixture.kinematic_waypoints);
        even_only
            .place_poses(
                &fixture.kinematic_movers,
                CONFIG.probe_spacing,
                &crate::sh_bake::ProbeGridLayout::no_probes(),
            )
            .expect("even poses place");
        let with_knots = plan_half(&fixture, ctx, placed, DEFAULT_BOUNCE_FILL);
        let without = plan_half(&fixture, ctx, &even_only, DEFAULT_BOUNCE_FILL);
        for (job, mover) in with_knots.request.jobs.iter().zip(&placed.analysed) {
            assert_eq!(
                job.scenes.len(),
                mover.poses.len(),
                "{}: a scene for every pose, geometry knots included",
                mover.label
            );
            assert_eq!(
                mover.poses.len() as u32,
                mover.knot_counts.even + mover.knot_counts.geometry
            );
        }
        let gate = |plan: &BounceHalfPlan, budget: u64| {
            let mut projection = plan_sh_base(
                &fixture.geometry,
                &static_light_refs(ctx),
                animation_table_bytes(ctx.animated_lights, ctx.total_light_count),
                1.0,
                DEFAULT_BOUNCE_FILL,
                budget,
            )
            .expect("priced")
            .1;
            projection.price_movers(&plan.cost);
            projection
        };
        let knotted_peak = gate(&with_knots, u64::MAX).estimated_peak_bytes;
        let even_peak = gate(&without, u64::MAX).estimated_peak_bytes;
        assert!(
            knotted_peak > even_peak,
            "geometry knots are priced: {even_peak} → {knotted_peak}"
        );
        let budget = knotted_peak - 1;
        gate(&without, budget)
            .admit()
            .expect("the even poses alone fit the budget");
        let message = gate(&with_knots, budget)
            .admit()
            .expect_err("the geometry knots pass the budget")
            .to_string();
        assert!(
            message.contains("SH base working-set gate refused before patch shading")
                && message.contains(&format!("estimated peak {knotted_peak} bytes"))
                && message.contains("mover bounce"),
            "{message}"
        );
    });
}

/// The SH base bake with the fixture's movers, on a control that reports
/// progress: what it completed, and the total it published.
fn progress_of(fixture: &FixturePipeline, cache: Option<&StageCache>) -> (usize, Option<usize>) {
    progress_at(fixture, cache, DEFAULT_BOUNCE_FILL, None)
}

/// [`progress_of`] at bounce fill `fill`, with mover depth's tile range
/// lowered to `depth_cap` where given.
fn progress_at(
    fixture: &FixturePipeline,
    cache: Option<&StageCache>,
    fill: f32,
    depth_cap: Option<u32>,
) -> (usize, Option<usize>) {
    with_ctx(fixture, |ctx, poses| {
        let mut plan = plan_half(fixture, ctx, poses, fill);
        if let Some(cap) = depth_cap {
            plan.request.depth_cap = cap;
        }
        let (handoff, _) = plan_sh_base(
            ctx.geometry,
            &static_light_refs(ctx),
            animation_table_bytes(ctx.animated_lights, ctx.total_light_count),
            CONFIG.probe_spacing,
            fill,
            u64::MAX,
        )
        .expect("priced");
        let progress = StageProgress::indeterminate();
        let control = BakeControl::new(Arc::new(Governor::new(usize::MAX, false)), &progress);
        bake_sh_base_with_movers(
            ctx,
            &CONFIG,
            handoff,
            &NO_EMBEDDED_EMITTERS,
            cache,
            &control,
            MAX_PROBE_DEPTH_TILES,
            &plan.request,
        );
        (progress.completed(), progress.total())
    })
}

// The SH base's progress ends exactly at the total it published, with the
// door re-baking every bounce, none (depth only) or two: cold, warm over an
// empty cache, on a full hit, and on an id-34 hit whose door misses its
// memo. The mover bake counts in the total, a volume hit advances only what
// the movers do not re-run, and a memo hit advances its job at once.
#[test]
fn the_sh_base_progress_ends_at_its_total_with_movers_cold_and_warm() {
    let maps = tempfile::tempdir().expect("maps dir");
    for (bounces, other) in [(None, Some(2)), (Some(0), None), (Some(2), Some(0))] {
        let fixture = load_map(&closet(maps.path(), bounces));
        let primer = load_map(&closet(maps.path(), other));
        let check = |mode: &str, (completed, total): (usize, Option<usize>)| {
            assert_eq!(
                Some(completed),
                total,
                "bounces {bounces:?} {mode}: progress ends at its total"
            );
        };
        check("cold", progress_of(&fixture, None));
        let cache = tempfile::tempdir().expect("cache");
        check(
            "warm empty",
            progress_of(&fixture, Some(&open(cache.path()))),
        );
        check("full hit", progress_of(&fixture, Some(&open(cache.path()))));
        let primed = tempfile::tempdir().expect("cache");
        progress_of(&primer, Some(&open(primed.path())));
        check(
            "id-34 hit, memo miss",
            progress_of(&fixture, Some(&open(primed.path()))),
        );
    }
}

/// The closet as [`closet`] writes it, then `edit`ed, under `dir`.
fn closet_edited(
    dir: &Path,
    bounces: Option<u32>,
    name: &str,
    edit: impl FnOnce(String) -> String,
) -> PathBuf {
    let text = std::fs::read_to_string(closet(dir, bounces)).expect("closet map");
    let edited = edit(text.clone());
    assert_ne!(edited, text, "{name}: the edit applies");
    let path = dir.join(format!("closet-{name}-{bounces:?}.map"));
    std::fs::write(&path, edited).expect("write closet");
    path
}

/// Every light black: bounce one is zero, so id 34's and every pose's
/// later bounces stop at a zero source.
fn dark(text: String) -> String {
    text.replace("\"_color\" \"255 244 230\"", "\"_color\" \"0 0 0\"")
}

/// A second door, `closet_door_b`, the first's copy on its path: two
/// mover jobs, so a tile range the first fills stops the bake before the
/// second.
fn two_doors(text: String) -> String {
    let start = text
        .find("{\n\"classname\" \"kinematic_mover\"")
        .expect("the door");
    let end = start
        + text[start..]
            .find("\n{\n\"classname\" \"kinematic_waypoint\"")
            .expect("its waypoints")
        + 1;
    let door = text[start..end].replace("\"name\" \"closet_door\"", "\"name\" \"closet_door_b\"");
    format!("{}{door}{}", &text[..end], &text[end..])
}

// The SH base's progress ends exactly at its total on each early break:
// fill 0, where neither id 34 nor a pose gathers a later bounce; a dark
// closet, whose zero bounce one stops id 34's and the pose's later bounces
// at a zero source; and two doors under a tile range of zero, where the
// first door's depth stops the bake before the second door's job runs.
// Cold, warm over an empty cache, and on a full hit.
#[test]
fn the_sh_base_progress_ends_at_its_total_on_every_early_break() {
    let maps = tempfile::tempdir().expect("maps dir");
    let cases = [
        ("fill 0", load_map(&closet(maps.path(), None)), 0.0, None),
        (
            "dark, posed zero source",
            load_map(&closet_edited(maps.path(), Some(3), "dark", dark)),
            DEFAULT_BOUNCE_FILL,
            None,
        ),
        (
            "depth cap, depth only",
            load_map(&closet_edited(maps.path(), Some(0), "two-doors", two_doors)),
            DEFAULT_BOUNCE_FILL,
            Some(0),
        ),
        (
            "depth cap, posed bounces",
            load_map(&closet_edited(maps.path(), Some(2), "two-doors", two_doors)),
            DEFAULT_BOUNCE_FILL,
            Some(0),
        ),
    ];
    for (case, fixture, fill, depth_cap) in &cases {
        if depth_cap.is_some() {
            assert_eq!(fixture.kinematic_movers.len(), 2, "{case}: two mover jobs");
        }
        let check = |mode: &str, (completed, total): (usize, Option<usize>)| {
            assert_eq!(
                Some(completed),
                total,
                "{case} {mode}: progress ends at its total"
            );
        };
        check("cold", progress_at(fixture, None, *fill, *depth_cap));
        let cache = tempfile::tempdir().expect("cache");
        check(
            "warm empty",
            progress_at(fixture, Some(&open(cache.path())), *fill, *depth_cap),
        );
        check(
            "full hit",
            progress_at(fixture, Some(&open(cache.path())), *fill, *depth_cap),
        );
    }
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

/// The closet door made 2.8 m deep and lifted only 24 units at its open
/// waypoint, baking four poses (`light_poses`): the slab encloses valid
/// probes in the doorway at every pose, and others at some.
fn fat_door(text: String) -> String {
    let start = text.find("// brush door_slab").expect("the door slab");
    let end = start + text[start..].find("\n}\n").expect("its end");
    let slab = text[start..end]
        .replace(" 504 ", " 552 ")
        .replace(" 488 ", " 440 ");
    format!("{}{slab}{}", &text[..start], &text[end..])
        .replace("\"origin\" \"320 496 168\"", "\"origin\" \"320 496 80\"")
        .replace(
            "\"name\" \"closet_door\"",
            "\"name\" \"closet_door\"\n\"light_poses\" \"4\"",
        )
}

// The enclosing rule per pose kind: a pose whose
// posed brushes enclose a probe's position, inside one brush by at least
// the gather's lift, gathers zero bounce there (every lifted ray starts in
// solid). A terminus stores no bounce state for it: the probe is unmarked
// and reads id 34, so rest is unchanged. A mid pose stores the state its own
// posed bake gives, zero radiance, wherever that leaves the probe's bars
// against id 34, and reads the base where id 34 is already within them; it
// never leaves the probe unmarked, so nothing blends across an enclosed mid
// pose (and epoch 7 writes no across marks). Inside a run of enclosing
// poses that is; the run's first and last mid poses compose reads (after the
// ramp) store what compose reads at the adjoining clear pose instead (edge
// copy), and the fixture copies some. One enclosed at both termini
// stores, if anything, only mid poses' states. Enclosure is recomputed here
// from the solids' own half-spaces, never read from the bake. Every lifted
// ray of an enclosed probe starts inside the pose, so its depth is the dark
// tile (every ray at distance 0) at that pose, stored at either pose kind,
// and Chebyshev still rejects it. A probe on a face, inside within the
// lift, keeps the per-ray rule. The bounce masks stay tight: each entry's
// stored masks are exactly its probes' stored poses, non-empty, within the
// valid mask, and its base marks lie inside them.
#[test]
fn an_enclosing_terminus_reads_the_base_and_an_enclosing_mid_pose_stores_its_dark_state() {
    use std::collections::BTreeMap;

    use postretro_level_format::mover_probe_channel::{PoseMark, effective_knots};
    use postretro_level_format::probe_depth_tiles::bordered_tile;

    use crate::mover_probe_channel::{
        BounceBlocks, MoverDirectBake, ProbeCells, enclosure_inset, value_sources,
    };
    use crate::ray_traversal::RAY_EPSILON;
    use crate::sh_bake::{TileRayTable, probe_gather_directions, vec3_from};

    let maps = tempfile::tempdir().expect("maps dir");
    let fixture = load_map(&closet_edited(maps.path(), None, "fat-door", fat_door));
    with_ctx(&fixture, |ctx, poses| {
        let plan = plan_half(&fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
        let (_, _, outcome) = bake_base(ctx, &plan, &plan.request, DEFAULT_BOUNCE_FILL, None);
        let [states] = outcome.movers.as_slice() else {
            panic!("one door");
        };
        let pose_count = states.pose_count;
        assert_eq!(pose_count, 4, "the door bakes its authored four poses");
        let last = pose_count - 1;
        let posed = PosedMovers::resolve(
            poses,
            &fixture.kinematic_movers,
            &fixture.geometry.texture_names,
        )
        .expect("the door poses");
        let solids = &posed.movers()[0].solids;
        let layout = crate::sh_bake::probe_grid_layout_of(
            ctx.geometry,
            ctx.tree,
            ctx.exterior_leaves,
            CONFIG.probe_spacing,
        );
        let directions = probe_gather_directions();
        let dark = bordered_tile(&TileRayTable::new(&directions).interior(&[0.0; 256]));
        let bounce: BTreeMap<u32, _> = states.bounce.iter().map(|p| (p.probe, p)).collect();
        let depth: BTreeMap<u32, _> = states.depth.iter().map(|p| (p.probe, p)).collect();
        let (mut both, mut one, mut other_stored, mut deep) = (0, 0, 0, 0);
        let (mut mid_enclosed, mut mid_dark, mut mid_base, mut mid_copied) = (0, 0, 0, 0);
        // Runs and their sources are found over the knots compose reads.
        let door = fixture
            .kinematic_movers
            .iter()
            .find(|mover| mover.mover_id == states.mover_id)
            .expect("the door is a map mover");
        let read = effective_knots(
            &poses
                .mover(states.mover_id)
                .expect("the door is analysed")
                .knots,
            door.light_ramp_start,
            door.light_ramp_end,
        );
        let (mut band, mut band_dark) = (0, 0);
        for (probe, position) in layout.probe_positions.iter().enumerate() {
            if layout.validity[probe] == 0 {
                continue;
            }
            let probe = probe as u32;
            let position = vec3_from(*position);
            // Enclosure from the solids' own half-spaces, inset by the lift.
            let inset = enclosure_inset(position);
            let enclosed: Vec<bool> = solids
                .iter()
                .map(|solid| solid.contains_by(position, inset))
                .collect();
            let lifted: Vec<bool> = solids
                .iter()
                .map(|solid| {
                    directions
                        .iter()
                        .all(|&dir| solid.contains(position + dir * RAY_EPSILON))
                })
                .collect();
            for k in 0..pose_count {
                assert!(
                    !enclosed[k] || lifted[k],
                    "probe {probe} pose {k}: enclosed, yet a lifted ray escapes"
                );
                if solids[k].contains(position) && !enclosed[k] {
                    band += 1;
                    band_dark += usize::from(lifted[k]);
                }
            }
            if !enclosed.contains(&true) {
                continue;
            }
            if let Some(entry) = bounce.get(&probe) {
                use postretro_level_format::mover_probe_channel::MOVER_IRRADIANCE_TILE_F16_COUNT;
                let mut stored = entry
                    .tiles
                    .as_chunks::<MOVER_IRRADIANCE_TILE_F16_COUNT>()
                    .0
                    .iter();
                let tiles: Vec<_> = entry
                    .marks
                    .iter()
                    .map(|&mark| (mark == PoseMark::Stored).then(|| stored.next().expect("a tile")))
                    .collect();
                let sources = value_sources(&enclosed, &read);
                for (k, (&enclosed, &mark)) in enclosed.iter().zip(&entry.marks).enumerate() {
                    let tile = tiles[k];
                    if !enclosed {
                        continue;
                    }
                    if k == 0 || k == last {
                        assert_eq!(
                            mark,
                            PoseMark::Unmarked,
                            "probe {probe}: enclosing terminus {k} reads the base"
                        );
                        continue;
                    }
                    let from = sources[k];
                    if from != k {
                        // A run edge: the adjoining clear pose's value, what
                        // compose reads there.
                        let expected = match entry.marks[from] {
                            PoseMark::Unmarked if from == 0 || from == last => PoseMark::Base,
                            other => other,
                        };
                        assert_eq!(mark, expected, "probe {probe} pose {k}: copies pose {from}");
                        assert_eq!(tile, tiles[from], "probe {probe} pose {k}: its tile");
                        mid_copied += 1;
                        continue;
                    }
                    match mark {
                        PoseMark::Stored => {
                            assert!(
                                tile.expect("stored").iter().all(|&half| half == 0),
                                "probe {probe} pose {k}: the stored state is zero bounce"
                            );
                            mid_dark += 1;
                        }
                        PoseMark::Base => mid_base += 1,
                        PoseMark::Unmarked => {
                            panic!("probe {probe} pose {k}: an enclosed mid pose blends across")
                        }
                    }
                }
                other_stored += 1;
            }
            if enclosed[0] && enclosed[last] {
                both += 1;
            } else if enclosed[0] || enclosed[last] {
                one += 1;
            }
            mid_enclosed += enclosed[1..last].iter().filter(|&&e| e).count();
            let record = depth
                .get(&probe)
                .unwrap_or_else(|| panic!("probe {probe}: an enclosing pose keeps its depth"));
            let mut tiles = record.tiles.chunks_exact(dark.len());
            for (k, (&mark, &enclosed)) in record.marks.iter().zip(&enclosed).enumerate() {
                let tile = (mark == PoseMark::Stored).then(|| tiles.next().expect("a stored tile"));
                if !enclosed {
                    continue;
                }
                let tile = tile.unwrap_or_else(|| {
                    panic!("probe {probe}: the enclosing pose {k} stores depth")
                });
                deep += 1;
                assert_eq!(tile, dark.as_slice(), "probe {probe} pose {k}: dark");
            }
        }
        eprintln!(
            "fat door: {both} probe(s) enclosed at both termini, {one} at one, \
             {other_stored} of them in a bounce entry, {deep} dark tile(s), \
             {mid_enclosed} enclosed (probe, mid pose) pair(s), in an entry {mid_dark} \
             storing zero bounce, {mid_base} reading the base and {mid_copied} copied at a \
             run edge; {band} (probe, pose) \
             pair(s) inside within the lift, {band_dark} with every lifted ray inside"
        );
        assert!(
            both > 0 && one > 0 && deep > 0 && mid_enclosed > 0 && mid_dark > 0 && mid_copied > 0,
            "the fixture encloses probes, at mid poses too, some storing their dark state and              some copied at a run edge"
        );

        let cells = ProbeCells::of_layout(&layout);
        let blocks = BounceBlocks::new(std::slice::from_ref(states), &cells);
        assert!(!blocks.bounce.is_empty(), "the door still changes bounce");
        for part in &blocks.bounce {
            let affected = part.entry.affected_probe_mask();
            let valid = cells.valid_masks[part.cell as usize];
            assert!(
                affected != 0 && affected & !valid == 0,
                "cell {}",
                part.cell
            );
            for masks in &part.entry.poses {
                assert_eq!(masks.base & !affected, 0, "cell {}: base inside", part.cell);
                assert_eq!(masks.base & masks.stored, 0, "cell {}", part.cell);
            }
        }
        let section = crate::mover_probe_channel::channel_section(
            &MoverDirectBake::default(),
            &blocks,
            &cells,
            &crate::mover_probe_channel::MoverRecords::new(&fixture.kinematic_movers, poses),
            &crate::mover_probe_channel::MoverPayloadBars::default(),
            |_, _, _| false,
            |_| true,
        )
        .expect("assembles")
        .expect("a section");
        assert_eq!(
            postretro_level_format::mover_probe_channel::MoverProbeChannelSection::from_bytes(
                &section.to_bytes()
            )
            .expect("decodes"),
            section
        );
    });
}

// A tile range the closet door's depth alone passes, well past it: the
// bake keeps exactly one record past it, the build fails by name before any
// state skips, with the door's whole count, exact, and the cut states are
// never memoized, so a full-range build over that cache bakes the door's
// whole depth. A hit stored under the full range is cut the same way.
#[test]
fn a_mover_past_the_tile_range_keeps_one_record_past_it_and_fails_by_name() {
    let maps = tempfile::tempdir().expect("maps dir");
    let fixture = load_map(&closet(maps.path(), Some(0)));
    with_ctx(&fixture, |ctx, poses| {
        let full_plan = plan_half(&fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
        let full_cache = tempfile::tempdir().expect("cache");
        let (_, _, full) = bake_base(
            ctx,
            &full_plan,
            &full_plan.request,
            DEFAULT_BOUNCE_FILL,
            Some(&open(full_cache.path())),
        );
        let carried = full.movers[0].depth.len();
        assert!(carried >= 4, "the door carries depth: {carried}");
        let cap = (carried / 2) as u32;
        let mut narrow = plan_half(&fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
        narrow.request.depth_cap = cap;

        let check = |mode: &str, cache: Option<&StageCache>| {
            let (_, base, mut outcome) =
                bake_base(ctx, &narrow, &narrow.request, DEFAULT_BOUNCE_FILL, cache);
            assert_eq!(
                outcome.movers[0].depth[..],
                full.movers[0].depth[..cap as usize + 1],
                "{mode}: one record past the range, the full bake's first"
            );
            assert_eq!(outcome.movers[0].depth_carried, carried, "{mode}");
            let error = super::skip_mid_poses(
                &mut outcome,
                &fixture.kinematic_movers,
                poses,
                &base.section,
                &base.depth_tiles,
                false,
                cap,
            )
            .expect_err("the door alone passes the range")
            .to_string();
            assert!(
                error.contains(&format!(
                    "MoverProbeChannel (id 57): {carried} probes carry closed-pose depth"
                )),
                "{mode}: the door's whole count, exact: {error}"
            );
        };
        let narrow_cache = tempfile::tempdir().expect("cache");
        check("cold", None);
        check("warm empty", Some(&open(narrow_cache.path())));
        check("memo hit past the range", Some(&open(full_cache.path())));

        let (_, _, rebuilt) = bake_base(
            ctx,
            &full_plan,
            &full_plan.request,
            DEFAULT_BOUNCE_FILL,
            Some(&open(narrow_cache.path())),
        );
        assert_eq!(
            rebuilt.movers, full.movers,
            "the cut states were never memoized"
        );
    });
}

// Two doors on one path under a tile range the first alone passes: cold
// stops before the second door and fails with a lower bound, since the
// second never ran. Warm over an empty cache, and over a cache holding both
// doors' full-range states, stop at the same door, hold no states for the
// second, and fail with the same text.
#[test]
fn warm_and_cold_stop_at_the_same_mover_past_the_tile_range() {
    let maps = tempfile::tempdir().expect("maps dir");
    let fixture = load_map(&closet_edited(maps.path(), Some(0), "two-doors", two_doors));
    with_ctx(&fixture, |ctx, poses| {
        let full_plan = plan_half(&fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
        assert_eq!(full_plan.request.jobs.len(), 2, "two mover jobs");
        let primed = tempfile::tempdir().expect("cache");
        let (_, _, full) = bake_base(
            ctx,
            &full_plan,
            &full_plan.request,
            DEFAULT_BOUNCE_FILL,
            Some(&open(primed.path())),
        );
        assert!(!full.stopped && full.movers.len() == 2);
        let carried = full.movers[0].depth_carried;
        assert!(carried >= 4, "the door carries depth: {carried}");
        let cap = (carried / 2) as u32;
        let mut narrow = plan_half(&fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
        narrow.request.depth_cap = cap;

        let run = |cache: Option<&StageCache>| {
            let (_, base, mut outcome) =
                bake_base(ctx, &narrow, &narrow.request, DEFAULT_BOUNCE_FILL, cache);
            let error = super::skip_mid_poses(
                &mut outcome,
                &fixture.kinematic_movers,
                poses,
                &base.section,
                &base.depth_tiles,
                false,
                cap,
            )
            .expect_err("the first door alone passes the range")
            .to_string();
            (outcome.movers, outcome.stopped, error)
        };
        let (cold_movers, cold_stopped, cold_error) = run(None);
        assert!(cold_stopped && cold_movers.len() == 1, "cold stops");
        assert!(
            cold_error.contains(&format!(
                "at least {carried} probes carry closed-pose depth"
            )),
            "a lower bound: {cold_error}"
        );
        let empty = tempfile::tempdir().expect("cache");
        for (mode, cache) in [
            ("warm empty", empty.path()),
            ("warm over full-range hits", primed.path()),
        ] {
            let (movers, stopped, error) = run(Some(&open(cache)));
            assert_eq!(movers, cold_movers, "{mode}: the same states");
            assert_eq!(
                (stopped, &error),
                (cold_stopped, &cold_error),
                "{mode}: the same stop and error"
            );
        }
    });
}

// Regression: a warm build whose id 34 and every pose hit read no
// mover-absent later bounces, so their memo left its use set and a prune
// could evict it, re-tracing id 34's later bounces at the next mover edit.
// The closet door bakes mid poses, so it reads them: a volume hit marks their
// memo used without reading it, and every entry the cold-cache build wrote
// stays in the warm build's use set.
#[test]
fn a_volume_hit_that_reads_no_absent_bounces_still_marks_their_memo_used() {
    let maps = tempfile::tempdir().expect("maps dir");
    let fixture = load_map(&closet(maps.path(), None));
    let cache = tempfile::tempdir().expect("cache");
    with_ctx(&fixture, |ctx, poses| {
        let plan = plan_half(&fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
        assert!(
            plan.request.jobs[0].scenes.len() > 2,
            "the door bakes mid poses"
        );
        let first = open(cache.path());
        bake_base(ctx, &plan, &plan.request, DEFAULT_BOUNCE_FILL, Some(&first));
        let wrote_absent = first
            .test_accesses_by_stage()
            .get("mover_absent_bounces")
            .is_some_and(|access| access.writes + access.streamed_writes > 0);
        assert!(
            wrote_absent,
            "the cold-cache build memoizes the later bounces"
        );
        let mut written: Vec<String> = std::fs::read_dir(cache.path())
            .expect("list the cache")
            .flatten()
            .filter(|entry| entry.path().is_file())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| !name.ends_with(".tmp"))
            .collect();
        written.sort();

        let warm = open(cache.path());
        bake_base(ctx, &plan, &plan.request, DEFAULT_BOUNCE_FILL, Some(&warm));
        let accesses = warm.test_accesses_by_stage();
        assert!(
            accesses
                .get("mover_absent_bounces")
                .is_none_or(|access| access.read_attempts == 0),
            "every pose hits, so nothing reads the later bounces"
        );
        let used = warm.test_live_entry_names();
        let unused: Vec<&String> = written
            .iter()
            .filter(|name| !used.contains(*name))
            .collect();
        assert!(
            unused.is_empty(),
            "{} of {} entries left the warm build's use set: {unused:?}",
            unused.len(),
            written.len()
        );
    });
}

/// The closet door baking four poses (`light_poses`) under a ramp that
/// squeezes both mid poses onto t′ 0, so compose never reads them.
fn squeezed_door(text: String) -> String {
    text.replace(
        "\"name\" \"closet_door\"",
        "\"name\" \"closet_door\"\n\"light_poses\" \"4\"\n\"light_ramp_start\" \"0.7\"\n\
         \"light_ramp_end\" \"0.75\"",
    )
}

// Regression: a bake the tile range stopped or cut, whose skips then dropped
// mid-pose-only depth records, came back within the range and built, the
// movers past the stop and the cut records silently losing their states.
// Every depth record here stores at the door's mid poses alone, which its
// ramp squeezes out: skipping drops them all. Past a range of 3, a bake that
// stopped before a later mover fails by name with a lower bound; one whose
// range cut the door's records, and the last job keeping exactly one record
// past the range, fail with the door's whole count, exact; all before any
// record skips.
#[test]
fn a_bake_cut_short_past_the_tile_range_fails_by_name_before_its_skips() {
    use postretro_level_format::mover_probe_channel::PoseMark;
    use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;

    use crate::mover_probe_channel::{DepthProbe, MoverBounceStates};

    let maps = tempfile::tempdir().expect("maps dir");
    let fixture = load_map(&closet_edited(
        maps.path(),
        Some(0),
        "squeezed",
        squeezed_door,
    ));
    with_ctx(&fixture, |ctx, poses| {
        let plan = plan_half(&fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
        let [job] = plan.request.jobs.as_slice() else {
            panic!("one door");
        };
        assert_eq!(
            job.scenes.len(),
            4,
            "the door bakes its authored four poses"
        );
        let (_, base, _) = bake_base(
            ctx,
            &plan,
            &MoverBounceRequest::default(),
            DEFAULT_BOUNCE_FILL,
            None,
        );
        let cap = 3u32;
        let probes: Vec<u32> = (0..base.section.probes.len() as u32)
            .filter(|&probe| base.section.probes[probe as usize].validity != 0)
            .take(cap as usize + 1)
            .collect();
        assert_eq!(probes.len(), cap as usize + 1, "the closet's valid probes");
        // One record past the range, as the bake keeps it, each stored at
        // both mid poses and neither terminus.
        let outcome = |stopped: bool, carried: usize| {
            let depth: Vec<DepthProbe> = probes
                .iter()
                .map(|&probe| DepthProbe {
                    probe,
                    marks: vec![
                        PoseMark::Unmarked,
                        PoseMark::Stored,
                        PoseMark::Stored,
                        PoseMark::Unmarked,
                    ],
                    tiles: vec![[0u16; 2]; 2 * PROBE_DEPTH_TILE_TEXELS],
                })
                .collect();
            let mut states = MoverBounceStates::new(job.mover_id, 4, Vec::new(), depth);
            states.depth_carried = carried;
            MoverBounceOutcome {
                movers: vec![states],
                stopped,
                ..MoverBounceOutcome::default()
            }
        };
        let skip = |bake: &mut MoverBounceOutcome, tile_cap: u32| {
            super::skip_mid_poses(
                bake,
                &fixture.kinematic_movers,
                poses,
                &base.section,
                &base.depth_tiles,
                false,
                tile_cap,
            )
        };

        let mut whole = outcome(false, cap as usize + 1);
        skip(&mut whole, MAX_PROBE_DEPTH_TILES).expect("within the full range");
        assert!(
            whole.movers[0].depth.is_empty() && whole.movers[0].depth_carried == 0,
            "skipping alone drops every record the squeezed poses store"
        );

        // Exactly one record past the range, the last job: neither stopped
        // nor cut, and its skips would bring it under, yet it fails with its
        // whole count, exact, as any earlier job would have.
        let mut one_past = outcome(false, cap as usize + 1);
        let error = skip(&mut one_past, cap)
            .expect_err("one record past the range fails")
            .to_string();
        assert!(
            error.contains(&format!(
                "MoverProbeChannel (id 57): {} probes carry closed-pose depth",
                cap + 1
            )),
            "the door's whole count, exact: {error}"
        );
        assert_eq!(
            one_past.movers[0].depth.len(),
            cap as usize + 1,
            "no record skips before the refusal"
        );

        let mut stopped = outcome(true, cap as usize + 1);
        let error = skip(&mut stopped, cap)
            .expect_err("a bake that stopped fails")
            .to_string();
        assert!(
            error.contains(&format!(
                "MoverProbeChannel (id 57): at least {} probes carry closed-pose depth",
                cap + 1
            )),
            "a lower bound: {error}"
        );
        assert_eq!(
            stopped.movers[0].depth.len(),
            cap as usize + 1,
            "no record skips before the refusal"
        );

        let mut cut = outcome(false, cap as usize + 5);
        let error = skip(&mut cut, cap)
            .expect_err("a cut mover fails")
            .to_string();
        assert!(
            error.contains(&format!(
                "MoverProbeChannel (id 57): {} probes carry closed-pose depth",
                cap + 5
            )),
            "the door's whole count, exact: {error}"
        );
        assert_eq!(
            cut.movers[0].depth.len(),
            cap as usize + 1,
            "no record skips before the refusal"
        );
    });
}

// A test-only re-bake of every valid probe at each terminus,
// on the closet and the setpiece, differs from id 34 beyond the
// affected-probe rule (each probe's own BC6H round-trip bar and the
// perceptual floor), recomputed here from the kept tiles, exactly at the
// probes each mover's bounce states store. A terminus that encloses the
// probe reads id 34 by rule, so its re-bake joins neither set. Each stored probe
// stores a terminus's state exactly where that terminus's re-bake leaves the
// round-trip bar alone. Heavy: whole posed re-bakes.
#[test]
#[ignore = "re-bakes every valid probe at both termini of every mover; run on demand"]
fn a_full_re_bake_differs_from_id_34_beyond_the_bar_only_where_the_domain_admits() {
    use std::collections::{BTreeMap, BTreeSet};

    use postretro_level_format::mover_probe_channel::PoseMark;

    use crate::mover_probe_channel::{
        BOUNCE_FLOOR_LEVELS, compact_slots, compact_tile, display_levels, enclosure_inset,
        max_abs_difference, round_trip_error,
    };
    use crate::ray_traversal::RAY_EPSILON;
    use crate::sh_bake::{probe_gather_directions, vec3_from};

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
            let pose_counts: BTreeMap<u32, usize> = request
                .jobs
                .iter()
                .map(|job| (job.mover_id, job.scenes.len()))
                .collect();
            assert_eq!(
                outcome.poses.len(),
                pose_counts.values().sum::<usize>(),
                "every pose handed back"
            );
            let layout = crate::sh_bake::probe_grid_layout_of(
                ctx.geometry,
                ctx.tree,
                ctx.exterior_leaves,
                CONFIG.probe_spacing,
            );
            let valid = layout.validity.iter().filter(|&&v| v != 0).count();
            let slots = compact_slots(&layout);
            // Enclosure from the posed solids' own half-spaces at the probe,
            // inset by the gather's lift, never the bake's own flag.
            let posed = PosedMovers::resolve(
                poses,
                &fixture.kinematic_movers,
                &fixture.geometry.texture_names,
            )
            .expect("the poses resolve");
            let directions = probe_gather_directions();
            let mut enclosed_pairs = 0;
            let dim = base.section.tile_dimension as usize;
            let mut beyond: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
            let mut past_bar: BTreeMap<(u32, usize), BTreeSet<u32>> = BTreeMap::new();
            for pose in &outcome.poses {
                let last = pose_counts[&pose.mover_id] - 1;
                if pose.pose != 0 && pose.pose != last {
                    // A mid pose hands back its near-field bounce one alone.
                    assert!(pose.probes.is_empty() && !pose.domain.is_empty());
                    continue;
                }
                let terminus = usize::from(pose.pose == last);
                assert_eq!(
                    pose.probes.len(),
                    valid,
                    "{name}: every valid probe re-baked"
                );
                let set = beyond.entry(pose.mover_id).or_default();
                let bar_set = past_bar.entry((pose.mover_id, terminus)).or_default();
                let solid = &posed
                    .movers()
                    .iter()
                    .find(|mover| mover.mover_id == pose.mover_id)
                    .expect("the pose's mover")
                    .solids[pose.pose];
                for probe in &pose.probes {
                    let position = vec3_from(layout.probe_positions[probe.probe as usize]);
                    if solid.contains_by(position, enclosure_inset(position)) {
                        // An enclosing terminus reads id 34: its dark
                        // re-bake is never a state.
                        assert!(
                            directions
                                .iter()
                                .all(|&dir| solid.contains(position + dir * RAY_EPSILON)),
                            "{name} probe {}: enclosed, yet a lifted ray escapes",
                            probe.probe
                        );
                        enclosed_pairs += 1;
                        continue;
                    }
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
            eprintln!("{name}: {enclosed_pairs} enclosed (probe, terminus) pair(s) skipped");
            assert_eq!(stored.movers.len(), request.jobs.len());
            let at_terminus = |probe: &crate::mover_probe_channel::BounceProbe| {
                let last = probe.marks.len() - 1;
                [0, last].map(|pose| probe.marks[pose] == PoseMark::Stored)
            };
            for states in &stored.movers {
                // Probes a terminus stores: the termini decide them alone;
                // a probe only a mid pose stores is the near-field bake's.
                let stored: BTreeSet<u32> = states
                    .bounce
                    .iter()
                    .filter(|probe| at_terminus(probe).contains(&true))
                    .map(|probe| probe.probe)
                    .collect();
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
                    let termini = at_terminus(probe);
                    if !termini.contains(&true) {
                        continue;
                    }
                    for (terminus, &stored) in termini.iter().enumerate() {
                        let leaves_bar = past_bar
                            .get(&(states.mover_id, terminus))
                            .is_some_and(|set| set.contains(&probe.probe));
                        assert_eq!(
                            stored, leaves_bar,
                            "{name} mover #{} probe {} terminus {terminus}: a state is stored \
                             exactly where it leaves the round-trip bar",
                            states.mover_id, probe.probe
                        );
                    }
                    one_sided += usize::from(termini != [true, true]);
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

#[path = "mover_mid_pose_tests.rs"]
mod mid_pose;

#[path = "mover_mid_pose_compose_tests.rs"]
mod mid_pose_compose;
