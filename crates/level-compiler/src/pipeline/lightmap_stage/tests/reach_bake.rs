//! Fused-stage bakes of reach fixtures, and the cull's exactness check: bytes
//! with the reach cull on equal bytes with it off, verify mode re-walking every
//! culled pair.
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use postretro_test_log_capture::CapturedRecord;

use super::*;
use crate::lightmap_bake::Chart;
use crate::lightmap_reach::test_fixtures::ReachFixture;

/// One fused stage run of a reach fixture.
pub(super) struct ReachBake {
    pub(super) lightmap: Vec<u8>,
    pub(super) shadowmask: Option<Vec<u8>>,
    pub(super) counts: ReachCounts,
    /// What the stage logged on the thread it ran on.
    pub(super) records: Vec<CapturedRecord>,
}

/// An edit to the prepared atlas's charts, made before the stage bakes them.
pub(super) type ChartEdit = dyn Fn(&mut [Chart]) + Sync;

/// How a reach fixture bakes.
pub(super) struct ReachRun<'a> {
    pub(super) penumbra_floor: bool,
    pub(super) exact_skips: lightmap_layer::ExactSkips,
    /// Threads in the bake's pool.
    pub(super) workers: usize,
    /// Governor permits.
    pub(super) permits: usize,
    /// The lightmap stage's progress; a fresh one when `None`.
    pub(super) progress: Option<&'a StageProgress>,
    /// Edits the prepared atlas's charts before the stage bakes them.
    pub(super) edit_charts: Option<&'a ChartEdit>,
    /// Runs at every governor admission, on the admitted thread.
    pub(super) enter_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The stage cache; uncached when `None`.
    pub(super) cache: Option<&'a StageCache>,
    /// Reach verify mode, set on the thread the stage runs on; the test
    /// default (on) when `None`.
    pub(super) verify: Option<bool>,
}

impl ReachRun<'_> {
    pub(super) fn new(
        penumbra_floor: bool,
        exact_skips: lightmap_layer::ExactSkips,
        workers: usize,
    ) -> Self {
        Self {
            penumbra_floor,
            exact_skips,
            workers,
            permits: workers,
            progress: None,
            edit_charts: None,
            enter_hook: None,
            cache: None,
            verify: None,
        }
    }
}

/// Bake `lights` over `fixture` with the reach inputs, `exact_skips` and
/// `workers` threads and permits, uncached.
pub(super) fn reach_bake(
    fixture: &ReachFixture,
    lights: &[MapLight],
    penumbra_floor: bool,
    exact_skips: lightmap_layer::ExactSkips,
    workers: usize,
) -> anyhow::Result<ReachBake> {
    reach_bake_with(
        fixture,
        lights,
        &ReachRun::new(penumbra_floor, exact_skips, workers),
    )
}

/// Bake `lights` over `fixture` as `run` says, with the reach inputs.
pub(super) fn reach_bake_with(
    fixture: &ReachFixture,
    lights: &[MapLight],
    run: &ReachRun<'_>,
) -> anyhow::Result<ReachBake> {
    let fresh = StageProgress::indeterminate();
    let progress = run.progress.unwrap_or(&fresh);
    // The stage reads the verify switch and logs on a pool thread, not the
    // test's.
    ThreadPoolBuilder::new()
        .num_threads(run.workers)
        .build()
        .expect("reach fixture pool")
        .install(|| match run.verify {
            Some(on) => with_reach_verify(on, || bake_on_pool(fixture, lights, run, progress)),
            None => bake_on_pool(fixture, lights, run, progress),
        })
}

/// The stage run of [`reach_bake_with`], on its pool.
fn bake_on_pool(
    fixture: &ReachFixture,
    lights: &[MapLight],
    run: &ReachRun<'_>,
    progress: &StageProgress,
) -> anyhow::Result<ReachBake> {
    let capture = LogCapture::start();
    let args = test_args();
    let config = LightmapConfig {
        penumbra_floor: run.penumbra_floor,
        ..config(false)
    };
    let window = PartitionWindow {
        exact_skips: run.exact_skips,
        ..PartitionWindow::default()
    };
    let governor = Arc::new(Governor::new(run.permits, false));
    if let Some(hook) = &run.enter_hook {
        governor.set_enter_hook(Arc::clone(hook));
    }
    let control = BakeControl::new(governor, progress);
    let static_lights = StaticBakedLights::from_lights(lights);
    let alpha_lights = AlphaLightsNs::from_lights(lights);
    let compiled = &fixture.compiled;
    let mut geometry = compiled.geometry.clone();
    let mut prepared =
        lightmap_bake::prepare_atlas(&mut geometry, &static_lights, config.lightmap_density, &[])
            .expect("reach fixture atlas");
    if let Some(edit) = run.edit_charts {
        edit(&mut prepared.charts);
    }
    let output = bake_fused_windowed(
        &args,
        run.cache,
        &control,
        &BakeControl::unrestricted(),
        &mut geometry,
        &static_lights,
        &alpha_lights,
        None,
        &[],
        &crate::texel_footprint::NO_EMBEDDED_EMITTERS,
        &compiled.bvh,
        &compiled.primitives,
        &config,
        prepared,
        Some(ReachInputs {
            tree: fixture.tree(),
            exterior: fixture.exterior(),
            holes: 0,
        }),
        &window,
    )?;
    Ok(ReachBake {
        lightmap: output.lightmap.section.to_bytes(),
        shadowmask: output.shadowmask.map(|section| section.to_bytes()),
        counts: LAST_REACH_COUNTS
            .with(|last| last.get())
            .expect("the stage logs its reach line"),
        records: capture.records(),
    })
}

/// Every exact skip but the reach cull.
pub(super) fn reach_off() -> lightmap_layer::ExactSkips {
    lightmap_layer::ExactSkips {
        reach_cull: false,
        ..lightmap_layer::ExactSkips::ON
    }
}

/// Bake `lights` over `fixture` with every exact skip, then with only the
/// reach cull off, at `penumbra_floor` with four workers, and require equal
/// lightmap and shadowmask bytes, with verify mode on and re-walking every
/// culled pair. The culled bake's counts.
pub(super) fn assert_cull_exact(
    fixture: &ReachFixture,
    lights: &[MapLight],
    penumbra_floor: bool,
) -> ReachCounts {
    let culled = reach_bake(
        fixture,
        lights,
        penumbra_floor,
        lightmap_layer::ExactSkips::ON,
        4,
    )
    .expect("culled bake");
    let counts = culled.counts;
    assert!(counts.verify, "verify mode is on in tests");
    assert!(!counts.switched_off, "floor {penumbra_floor}: {counts:?}");
    assert_eq!(counts.rewalked, counts.culled, "floor {penumbra_floor}");
    let unculled =
        reach_bake(fixture, lights, penumbra_floor, reach_off(), 4).expect("unculled bake");
    assert_eq!(unculled.counts.culled, 0);
    assert_eq!(unculled.counts.walked, counts.walked);
    assert!(
        culled.lightmap == unculled.lightmap,
        "floor {penumbra_floor}: lightmap bytes differ with the reach cull off"
    );
    assert!(
        culled.shadowmask == unculled.shadowmask,
        "floor {penumbra_floor}: shadowmask bytes differ with the reach cull off"
    );
    counts
}
