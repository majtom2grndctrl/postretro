//! Lightmap partition-window behavior: fold order, throttle and pause, cache I/O
//! outside permits, the resident bound, chart-cull byte identity, and a memo-hit
//! rebuild followed by a one-light edit.
//! Governing context: `context/lib/build_pipeline.md` §Build Cache.

use super::*;
use std::sync::mpsc;
use std::sync::{Condvar, Mutex};
use std::thread;

const TIMEOUT: Duration = Duration::from_secs(10);
const QUIET: Duration = Duration::from_millis(100);

/// Four quads in one bake layer, so each light bakes four governed charts.
const WINDOW_QUADS: usize = 4;

fn window_lights(count: usize) -> Vec<MapLight> {
    (0..count)
        .map(|index| {
            let mut light = point_light(
                DVec3::new(6.0 + 12.0 * index as f64, 5.0 + index as f64, 6.0),
                [1.0, 0.2 + 0.15 * index as f32, 0.4],
            );
            light.falloff_range = 80.0;
            light
        })
        .collect()
}

fn selection() -> EntityShadowLightsSection {
    EntityShadowLightsSection {
        light_indices: vec![0, 1],
    }
}

/// (lightmap bytes, shadowmask bytes) of one fused bake on its own pool.
fn bake_window(
    lights: &[MapLight],
    cache: Option<&StageCache>,
    window: &PartitionWindow,
    governor: Arc<Governor>,
    progress: &StageProgress,
) -> (Vec<u8>, Vec<u8>) {
    let args = test_args();
    let config = config(false);
    let selection = selection();
    let mut geometry = quads_in_one_cell(WINDOW_QUADS);
    let (bvh, primitives, _) = build_bvh(&geometry).expect("window fixture BVH");
    let static_lights = StaticBakedLights::from_lights(lights);
    let alpha_lights = AlphaLightsNs::from_lights(lights);
    let control = BakeControl::new(governor, progress);
    ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .expect("window fixture pool")
        .install(|| {
            let prepared = lightmap_bake::prepare_atlas(
                &mut geometry,
                &static_lights,
                config.lightmap_density,
                &[],
            )
            .expect("window fixture atlas");
            assert_eq!(prepared.layer_count, 1, "window fixture bakes one layer");
            assert_eq!(prepared.placements.len(), WINDOW_QUADS);
            let output = bake_fused_windowed(
                &args,
                cache,
                &control,
                &BakeControl::unrestricted(),
                &mut geometry,
                &static_lights,
                &alpha_lights,
                Some(&selection),
                &bvh,
                &primitives,
                &config,
                prepared,
                window,
            )
            .expect("window fixture bake");
            (
                output.lightmap.section.to_bytes(),
                output
                    .shadowmask
                    .expect("selected lights emit a shadowmask")
                    .to_bytes(),
            )
        })
}

fn sized(size: usize) -> PartitionWindow {
    PartitionWindow {
        size,
        ..PartitionWindow::default()
    }
}

fn unthrottled(lights: &[MapLight]) -> (Vec<u8>, Vec<u8>) {
    bake_window(
        lights,
        None,
        &sized(1),
        Arc::new(Governor::new(4, false)),
        &StageProgress::indeterminate(),
    )
}

/// A one-shot gate a hook thread waits on until the test opens it.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    fn open(&self) {
        *self.open.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.changed.notify_all();
    }

    /// Wait until opened; false on timeout.
    fn wait(&self) -> bool {
        let open = self.open.lock().unwrap_or_else(|p| p.into_inner());
        let (open, _) = self
            .changed
            .wait_timeout_while(open, TIMEOUT, |open| !*open)
            .unwrap_or_else(|p| p.into_inner());
        *open
    }
}

// Light 1's partition is ready before light 0's; fold and consume still run in
// global light order, and the bytes equal the window-1 bake.
#[test]
fn lightmap_window_folds_in_global_light_order_when_later_light_finishes_first() {
    let lights = window_lights(3);
    let baseline = unthrottled(&lights);

    let light_one_ready = Arc::new(Gate::default());
    let ready_order = Arc::new(Mutex::new(Vec::new()));
    let held = Arc::new(Mutex::new(false));
    let mut window = sized(2);
    let probe_ready = Arc::clone(&light_one_ready);
    let probe_order = Arc::clone(&ready_order);
    window.probe = Some(WindowProbe::with_on_ready(move |item| {
        probe_order.lock().unwrap().push(item);
        if item == 1 {
            probe_ready.open();
        }
    }));
    let hook_ready = Arc::clone(&light_one_ready);
    let hook_held = Arc::clone(&held);
    window.hooks.before_chart = Some(Arc::new(move |light, _layer, _chart| {
        // Hold light 0's first chart, outside any permit.
        let mut held = hook_held.lock().unwrap();
        if light == 0 && !*held {
            *held = true;
            drop(held);
            assert!(hook_ready.wait(), "light 1 never became ready");
        }
    }));
    let probe = window.probe.clone().unwrap();

    let bytes = bake_window(
        &lights,
        None,
        &window,
        Arc::new(Governor::new(4, false)),
        &StageProgress::indeterminate(),
    );

    let ready_order = ready_order.lock().unwrap().clone();
    let position = |item| ready_order.iter().position(|&ready| ready == item).unwrap();
    assert!(
        position(1) < position(0),
        "light 1 must be ready before light 0: {ready_order:?}"
    );
    assert_eq!(probe.consumed(), vec![0, 1, 2], "fold and consume order");
    assert_eq!(bytes, baseline, "window-2 bytes differ from window 1");
}

// The resident partition count reaches the window and never exceeds it, cold
// and warm all-miss, at more permits than the window.
#[test]
fn lightmap_window_resident_partitions_reach_and_never_exceed_window() {
    let lights = window_lights(5);
    let baseline = unthrottled(&lights);
    for warm in [false, true] {
        let dir = fresh_cache_dir("window_bound");
        let cache = warm.then(|| StageCache::new(&dir).expect("window bound cache"));
        let window_size = 2;
        let mut window = sized(window_size);
        let light_one_ready = Arc::new(Gate::default());
        let probe_ready = Arc::clone(&light_one_ready);
        window.probe = Some(WindowProbe::with_on_ready(move |item| {
            if item == 1 {
                probe_ready.open();
            }
        }));
        // Holding light 0 until light 1 is resident makes both resident at once.
        let held = Arc::new(Mutex::new(false));
        let hook_ready = Arc::clone(&light_one_ready);
        window.hooks.before_chart = Some(Arc::new(move |light, _layer, _chart| {
            let mut held = held.lock().unwrap();
            if light == 0 && !*held {
                *held = true;
                drop(held);
                assert!(hook_ready.wait(), "light 1 never became ready");
            }
        }));
        let probe = window.probe.clone().unwrap();

        let bytes = bake_window(
            &lights,
            cache.as_ref(),
            &window,
            Arc::new(Governor::new(4, false)),
            &StageProgress::indeterminate(),
        );

        assert_eq!(probe.max_resident(), window_size, "warm: {warm}");
        assert_eq!(probe.consumed(), (0..lights.len()).collect::<Vec<_>>());
        assert_eq!(bytes, baseline, "warm: {warm}");
        if let Some(cache) = cache {
            let layers = cache.test_access("lightmap_layer");
            assert_eq!(layers.read_hits, 0);
            assert_eq!(layers.writes, lights.len());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// Permits drop to one while the window is partly admitted, then pause holds
// admission; a bake started at one permit matches too.
#[test]
fn lightmap_window_throttle_and_pause_admit_one_item_and_keep_bytes() {
    let lights = window_lights(5);
    let baseline = unthrottled(&lights);

    let at_one = bake_window(
        &lights,
        None,
        &sized(3),
        Arc::new(Governor::new(1, false)),
        &StageProgress::indeterminate(),
    );
    assert_eq!(at_one, baseline, "a bake started at one permit");

    let governor = Arc::new(Governor::new(4, false));
    let progress = StageProgress::indeterminate();
    let release = Arc::new(Gate::default());
    let throttled = Arc::new(Mutex::new(false));
    let max_active_after = Arc::new(AtomicU64::new(0));
    let (admitted_tx, admitted_rx) = mpsc::channel();
    let admitted_tx = Mutex::new(admitted_tx);
    let hook_governor = Arc::downgrade(&governor);
    let hook_release = Arc::clone(&release);
    let hook_throttled = Arc::clone(&throttled);
    let hook_max = Arc::clone(&max_active_after);
    governor.set_enter_hook(Arc::new(move || {
        let governor = hook_governor.upgrade().expect("governor outlives the bake");
        if *hook_throttled.lock().unwrap() {
            hook_max.fetch_max(governor.active() as u64, Ordering::SeqCst);
            admitted_tx.lock().unwrap().send(()).unwrap();
            return;
        }
        admitted_tx.lock().unwrap().send(()).unwrap();
        assert!(hook_release.wait(), "in-flight items were never released");
    }));

    let (done_tx, done_rx) = mpsc::channel();
    thread::scope(|scope| {
        let bake_governor = Arc::clone(&governor);
        let bake_progress = progress.clone();
        let lights = &lights;
        scope.spawn(move || {
            done_tx
                .send(bake_window(
                    lights,
                    None,
                    &sized(3),
                    bake_governor,
                    &bake_progress,
                ))
                .unwrap();
        });

        // Four items hold permits inside the window; the rest wait.
        for _ in 0..4 {
            admitted_rx
                .recv_timeout(TIMEOUT)
                .expect("initial admission");
        }
        *throttled.lock().unwrap() = true;
        governor.set_paused(true);
        governor.set_permits(1);
        release.open();

        // Admitted items drain, and pause admits nothing new.
        let deadline = std::time::Instant::now() + TIMEOUT;
        while progress.completed() < 4 && std::time::Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!(progress.completed(), 4, "in-flight items must finish");
        assert!(
            matches!(
                admitted_rx.recv_timeout(QUIET),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "a paused window admitted a new item"
        );

        governor.set_paused(false);
        let bytes = done_rx
            .recv_timeout(TIMEOUT)
            .expect("throttled bake stalled");
        assert_eq!(bytes, baseline, "throttled bytes");
    });
    assert_eq!(
        max_active_after.load(Ordering::SeqCst),
        1,
        "at most one lightmap ray item runs after permits drop to one"
    );
}

/// Chart-cull fixture: the window quads, all on one bake layer.
fn cull_quads() -> GeometryResult {
    quads_in_one_cell(WINDOW_QUADS)
}

/// One cached, culled bake of a fixture, and what unculled bakes of the same
/// lights produce.
struct CullRun {
    /// `(light, layer, chart)` for every chart the cull let through, sorted.
    baked: Vec<(usize, u32, usize)>,
    /// Bake layers the fixture atlas holds.
    layer_count: u32,
    /// Texels each light's unculled partitions light, summed over layers.
    lit_texels: Vec<usize>,
}

/// The fixture's charts in face order. Chart planning never reads the lights,
/// so a placeholder light stands in for the real set.
fn fixture_charts(build: fn() -> GeometryResult) -> Vec<lightmap_bake::Chart> {
    let mut geometry = build();
    let placeholder = vec![point_light(DVec3::ZERO, [1.0, 1.0, 1.0])];
    let static_lights = StaticBakedLights::from_lights(&placeholder);
    lightmap_bake::prepare_atlas(
        &mut geometry,
        &static_lights,
        config(false).lightmap_density,
        &[],
    )
    .expect("cull fixture atlas")
    .charts
}

/// Bake `lights` through the window with the chart cull active and a cache,
/// then assert progress completes at its published total and every cached
/// partition equals the unculled per-light, per-layer bake.
fn cull_bake_matches_unculled(
    build: fn() -> GeometryResult,
    lights: &[MapLight],
    label: &str,
) -> CullRun {
    let args = test_args();
    let config = config(false);
    let dir = fresh_cache_dir(label);
    let cache = StageCache::new(&dir).expect("cull cache");
    let baked = Arc::new(Mutex::new(Vec::new()));
    let mut window = sized(2);
    let record = Arc::clone(&baked);
    window.hooks.before_chart = Some(Arc::new(move |light, layer, chart| {
        record.lock().unwrap().push((light, layer, chart));
    }));
    let progress = StageProgress::indeterminate();

    let mut geometry = build();
    let (bvh, primitives, _) = build_bvh(&geometry).expect("cull fixture BVH");
    let static_lights = StaticBakedLights::from_lights(lights);
    let alpha_lights = AlphaLightsNs::from_lights(lights);
    let control = BakeControl::new(Arc::new(Governor::new(4, false)), &progress);
    let placements = ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .expect("cull fixture pool")
        .install(|| {
            let prepared = lightmap_bake::prepare_atlas(
                &mut geometry,
                &static_lights,
                config.lightmap_density,
                &[],
            )
            .expect("cull fixture atlas");
            let placements = prepared.placements.len();
            bake_fused_windowed(
                &args,
                Some(&cache),
                &control,
                &BakeControl::unrestricted(),
                &mut geometry,
                &static_lights,
                &alpha_lights,
                None,
                &bvh,
                &primitives,
                &config,
                prepared,
                &window,
            )
            .expect("cull fixture bake");
            placements
        });
    let published = progress.total().expect("lightmap total is published");
    assert_eq!(published, placements * lights.len());
    assert_eq!(progress.completed(), published);
    let mut baked = baked.lock().unwrap().clone();
    baked.sort_unstable();

    // Each cached partition equals an unculled bake of that light and layer.
    let mut geometry = build();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let static_lights = StaticBakedLights::from_lights(lights);
    let prepared =
        lightmap_bake::prepare_atlas(&mut geometry, &static_lights, config.lightmap_density, &[])
            .unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let mut lit_texels = vec![0; lights.len()];
    for (index, light) in lights.iter().enumerate() {
        for layer in 0..prepared.layer_count {
            let key = CacheKey::new(
                "lightmap_layer",
                lightmap_layer::LAYER_FORMAT_VERSION,
                &lightmap_layer::layer_input_hash(
                    light,
                    &shared,
                    &primitives,
                    &geometry,
                    config.lightmap_density,
                    args.soft_shadow_samples,
                    layer,
                ),
            );
            let unculled = lightmap_layer::bake_light_layer_controlled(
                light,
                &shared,
                &bvh,
                &primitives,
                &geometry,
                layer,
                args.soft_shadow_samples,
                &BakeControl::unrestricted(),
            );
            lit_texels[index] += unculled.texels.len();
            assert_eq!(
                cache.get(&key),
                Some(unculled.to_bytes()),
                "light {index} layer {layer}: culled partition differs from the unculled bake"
            );
        }
    }
    let layer_count = prepared.layer_count;
    drop(cache);
    let _ = std::fs::remove_dir_all(&dir);
    CullRun {
        baked,
        layer_count,
        lit_texels,
    }
}

// A light that reaches some charts plus a directional light that reaches all.
// The cull skips charts out of the point light's range, yet every cached
// partition equals the unculled per-light bake, and progress completes at the
// published total.
#[test]
fn chart_cull_is_byte_identical_and_progress_completes() {
    let mut near = point_light(DVec3::new(6.0, 3.0, 6.0), [1.0, 0.6, 0.3]);
    near.falloff_range = 6.0;
    let mut sun = point_light(DVec3::ZERO, [0.3, 0.4, 1.0]);
    sun.light_type = LightType::Directional;
    sun.cone_direction = Some([0.2, -1.0, 0.1]);
    let lights = vec![near, sun];

    let run = cull_bake_matches_unculled(cull_quads, &lights, "chart_cull");

    assert_eq!(
        run.baked.iter().filter(|(light, _, _)| *light == 0).count(),
        1,
        "the near light reaches one quad: {:?}",
        run.baked
    );
    assert_eq!(
        run.baked.iter().filter(|(light, _, _)| *light == 1).count(),
        WINDOW_QUADS,
        "the directional light reaches every quad"
    );
    assert!(
        run.lit_texels.iter().all(|&lit| lit > 0),
        "each light lights something"
    );
}

// The cull keeps a chart at distance == falloff_range, so a texel exactly
// there still lights under the inverse models and is zero under Linear.
#[test]
fn chart_cull_keeps_chart_whose_nearest_texel_is_at_falloff_range() {
    let charts = fixture_charts(cull_quads);
    let texel = crate::chart_raster::chart_texel_world_position(&charts[0], 0, 0);
    let models = [
        ("linear", FalloffModel::Linear, false),
        ("inverse_distance", FalloffModel::InverseDistance, true),
        ("inverse_squared", FalloffModel::InverseSquared, true),
    ];
    for (label, model, lights_at_range) in models {
        // Straight above the texel, so no other texel is nearer.
        let origin = DVec3::new(
            f64::from(texel.x),
            f64::from(texel.y + 4.0),
            f64::from(texel.z),
        );
        let mut light = point_light(origin, [1.0, 0.6, 0.3]);
        light.falloff_model = model;
        // The bake measures from the f32 origin, so use that exact distance.
        light.falloff_range = origin.as_vec3().distance(texel);
        let at_range = lightmap_bake::falloff(&light, light.falloff_range);

        let run = cull_bake_matches_unculled(
            cull_quads,
            std::slice::from_ref(&light),
            &format!("chart_cull_texel_range_{label}"),
        );

        assert_eq!(
            run.baked,
            vec![(0, 0, 0)],
            "{label}: only the chart at range"
        );
        if lights_at_range {
            assert!(at_range > 0.0, "{label}: falloff at range");
            assert!(run.lit_texels[0] > 0, "{label}: texel at range lights");
        } else {
            assert!(at_range < 1.0e-6, "{label}: falloff at range");
            assert_eq!(run.lit_texels[0], 0, "{label}: nothing lights");
        }
    }
}

// The cull compares the padded texel bounds with `<=`: a chart whose padded
// bounds are exactly falloff_range away is baked, one ulp short it is not.
// Neither lights a texel, since the padding puts every texel beyond the range.
#[test]
fn chart_cull_keeps_chart_at_padded_bounds_range_and_drops_it_one_ulp_beyond() {
    let charts = fixture_charts(cull_quads);
    let (min, max) = chart_texel_bounds(&charts[0]).expect("fixture chart has texels");
    let origin = Vec3::new((min.x + max.x) * 0.5, max.y + 3.0, (min.z + max.z) * 0.5);
    let at_bounds = origin.distance(Vec3::new(origin.x, max.y, origin.z));
    let one_ulp_short = f32::from_bits(at_bounds.to_bits() - 1);
    for (label, range, kept) in [("at", at_bounds, true), ("beyond", one_ulp_short, false)] {
        let mut light = point_light(
            DVec3::new(
                f64::from(origin.x),
                f64::from(origin.y),
                f64::from(origin.z),
            ),
            [1.0, 0.6, 0.3],
        );
        light.falloff_model = FalloffModel::InverseSquared;
        light.falloff_range = range;

        let run = cull_bake_matches_unculled(
            cull_quads,
            std::slice::from_ref(&light),
            &format!("chart_cull_bounds_range_{label}"),
        );

        let expected = if kept { vec![(0, 0, 0)] } else { Vec::new() };
        assert_eq!(run.baked, expected, "{label}: baked charts");
        assert_eq!(
            run.lit_texels[0], 0,
            "{label}: padded texels are out of range"
        );
    }
}

// The cull reads only distance, never a spot's cone: charts the cone points
// away from are still baked, and they bake to nothing, matching the unculled
// bake.
#[test]
fn chart_cull_bakes_charts_a_spot_cone_points_away_from_to_unculled_bytes() {
    let spot = |direction: [f32; 3]| {
        let mut light = point_light(DVec3::new(20.0, 6.0, 6.0), [1.0, 0.8, 0.5]);
        light.light_type = LightType::Spot;
        light.falloff_range = 40.0;
        light.cone_direction = Some(direction);
        light.cone_angle_inner = Some(0.1);
        light.cone_angle_outer = Some(0.3);
        light
    };
    // Down lights part of the quad below the spot; up lights nothing.
    let lights = vec![spot([0.0, -1.0, 0.0]), spot([0.0, 1.0, 0.0])];

    let run = cull_bake_matches_unculled(cull_quads, &lights, "chart_cull_spot");

    let every_quad_per_light: Vec<_> = (0..lights.len())
        .flat_map(|light| (0..WINDOW_QUADS).map(move |chart| (light, 0, chart)))
        .collect();
    assert_eq!(run.baked, every_quad_per_light, "every quad is in range");
    assert!(run.lit_texels[0] > 0, "the downward cone lights a quad");
    assert_eq!(run.lit_texels[1], 0, "the upward cone lights nothing");
}

// A zero or negative falloff_range clamps to a tiny positive range: a light
// away from the surface reaches no chart, and one inside a chart's bounds
// reaches only that chart. Neither lights a texel.
#[test]
fn chart_cull_reaches_only_enclosing_chart_when_falloff_range_is_not_positive() {
    let light_at = |origin: DVec3, range: f32| {
        let mut light = point_light(origin, [1.0, 0.6, 0.3]);
        light.falloff_range = range;
        light
    };
    let above = DVec3::new(6.0, 3.0, 6.0);
    let in_chart_zero = DVec3::new(6.0, 0.0, 6.0);
    let lights = vec![
        light_at(above, 0.0),
        light_at(above, -5.0),
        light_at(in_chart_zero, 0.0),
        light_at(in_chart_zero, -5.0),
    ];

    let run = cull_bake_matches_unculled(cull_quads, &lights, "chart_cull_range_not_positive");

    assert_eq!(
        run.baked,
        vec![(2, 0, 0), (3, 0, 0)],
        "only the lights inside chart 0's bounds bake it"
    );
    assert!(run.lit_texels.iter().all(|&lit| lit == 0), "nothing lights");
}

// Charts on two bake layers: each (layer, light) item culls only its own
// layer's charts, and every cached partition still equals the unculled bake.
#[test]
fn chart_cull_is_byte_identical_when_charts_span_two_bake_layers() {
    let mut first = point_light(DVec3::new(6.0, 3.0, 6.0), [1.0, 0.6, 0.3]);
    first.falloff_range = 6.0;
    let mut second = point_light(DVec3::new(22.0, 3.0, 6.0), [0.3, 0.6, 1.0]);
    second.falloff_range = 6.0;
    let lights = vec![first, second];

    let run = cull_bake_matches_unculled(two_layer_geometry, &lights, "chart_cull_two_layers");

    assert_eq!(run.layer_count, 2, "fixture must exercise two bake layers");
    let reached: Vec<_> = run
        .baked
        .iter()
        .map(|&(light, _, chart)| (light, chart))
        .collect();
    assert_eq!(reached, vec![(0, 0), (1, 1)], "each light reaches its quad");
    assert_ne!(
        run.baked[0].1, run.baked[1].1,
        "the reached charts sit on different bake layers"
    );
    assert!(
        run.lit_texels.iter().all(|&lit| lit > 0),
        "each light lights"
    );
}

// At one permit, a held partition put (or get) holds no permit: another
// light's chart work admits and runs before the I/O is released.
#[test]
fn lightmap_window_cache_io_holds_no_permit() {
    let lights = window_lights(3);
    let baseline = unthrottled(&lights);
    let mut edited = lights.clone();
    edited[1].intensity = 0.5;
    let edited_baseline = unthrottled(&edited);

    for held_get in [false, true] {
        let dir = fresh_cache_dir("window_io");
        let cache = StageCache::new(&dir).expect("window io cache");
        if held_get {
            // Seed every partition, then edit light 1 so only it misses.
            bake_window(
                &lights,
                Some(&cache),
                &sized(1),
                Arc::new(Governor::new(4, false)),
                &StageProgress::indeterminate(),
            );
        }
        let bake_lights = if held_get { &edited } else { &lights };
        let expected = if held_get {
            &edited_baseline
        } else {
            &baseline
        };

        let governor = Arc::new(Governor::new(1, false));
        let holding = Arc::new(Gate::default());
        let release = Arc::new(Gate::default());
        let max_active = Arc::new(AtomicU64::new(0));
        let admitted_while_held = Arc::new(AtomicU64::new(0));
        let hook_governor = Arc::downgrade(&governor);
        let hook_max = Arc::clone(&max_active);
        let hook_holding = Arc::clone(&holding);
        let hook_admitted = Arc::clone(&admitted_while_held);
        let hook_release = Arc::clone(&release);
        governor.set_enter_hook(Arc::new(move || {
            let governor = hook_governor.upgrade().expect("governor outlives the bake");
            hook_max.fetch_max(governor.active() as u64, Ordering::SeqCst);
            if *hook_holding.open.lock().unwrap() && !*hook_release.open.lock().unwrap() {
                hook_admitted.fetch_add(1, Ordering::SeqCst);
            }
        }));

        let mut window = sized(2);
        let io_holding = Arc::clone(&holding);
        let io_release = Arc::clone(&release);
        let hold = Arc::new(move |light: usize, _layer: u32| {
            if light == 0 {
                io_holding.open();
                assert!(io_release.wait(), "held cache I/O was never released");
            }
        });
        if held_get {
            window.hooks.before_get = Some(hold);
        } else {
            window.hooks.before_put = Some(hold);
        }
        // Light 1's chart work waits, outside any permit, until light 0's
        // I/O is held, so it is necessarily ready during the hold.
        let charts_wait = Arc::clone(&holding);
        window.hooks.before_chart = Some(Arc::new(move |light, _layer, _chart| {
            if light != 0 {
                assert!(charts_wait.wait(), "light 0's cache I/O never started");
            }
        }));

        let (done_tx, done_rx) = mpsc::channel();
        thread::scope(|scope| {
            let bake_governor = Arc::clone(&governor);
            let window = &window;
            let cache = &cache;
            scope.spawn(move || {
                done_tx
                    .send(bake_window(
                        bake_lights,
                        Some(cache),
                        window,
                        bake_governor,
                        &StageProgress::indeterminate(),
                    ))
                    .unwrap();
            });

            assert!(holding.wait(), "light 0's cache I/O never started");
            let deadline = std::time::Instant::now() + TIMEOUT;
            while admitted_while_held.load(Ordering::SeqCst) == 0
                && std::time::Instant::now() < deadline
            {
                thread::yield_now();
            }
            let admitted = admitted_while_held.load(Ordering::SeqCst);
            release.open();
            assert!(
                admitted > 0,
                "held_get {held_get}: no chart ray work admitted while light 0's I/O was held"
            );
            let bytes = done_rx.recv_timeout(TIMEOUT).expect("io-held bake stalled");
            assert_eq!(&bytes, expected, "held_get {held_get}");
        });
        assert_eq!(
            max_active.load(Ordering::SeqCst),
            1,
            "one chart item at a time"
        );
        drop(cache);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// A no-edit rebuild hits both section memos and reads no partition, yet its
// use record keeps every partition the memos summarize. The next build's
// prune, under a budget smaller than the set, spares them, so a one-light edit
// re-bakes only that light.
#[test]
fn memo_hit_rebuild_then_light_edit_rebakes_one_light() {
    // Lights 0 and 1 are shadow-selected; light 2 is protected only by the
    // lightmap memo's mark, and light 3 is the one edited.
    let lights = window_lights(4);
    let root = fresh_cache_dir("memo_record");
    let input = root.join("fixture.map");
    let cache_dir = root.join("cache");
    let args = crate::parse_args_from(
        [
            input.to_string_lossy().into_owned(),
            "--verbose".to_owned(),
            "--soft-shadow-samples".to_owned(),
            "4".to_owned(),
            "--cache-dir".to_owned(),
            cache_dir.to_string_lossy().into_owned(),
            "--cache-max-size".to_owned(),
            "1".to_owned(),
        ]
        .into_iter(),
    )
    .expect("memo record arguments");
    let build = |lights: &[MapLight]| {
        let cache = crate::construct_stage_cache(&args).expect("cache enabled");
        bake_window(
            lights,
            Some(&cache),
            &sized(2),
            Arc::new(Governor::new(4, false)),
            &StageProgress::indeterminate(),
        );
        let section_hit = cache.test_access("lightmap_section").read_hits == 1;
        let shadowmask_hit = cache.test_access("shadowmask_atlas").read_hits == 1;
        let layers = cache.test_access("lightmap_layer");
        cache.finish_successful_build(1);
        (section_hit, shadowmask_hit, layers)
    };

    let (_, _, first) = build(&lights);
    assert_eq!(first.writes, lights.len());

    let (section_hit, shadowmask_hit, rebuild) = build(&lights);
    assert!(section_hit && shadowmask_hit, "both memos must hit");
    assert_eq!(rebuild.read_attempts, 0, "memo hits read no partition");

    let mut edited = lights.clone();
    edited[3].intensity = 0.5;
    let (section_hit, _, after_edit) = build(&edited);
    assert!(!section_hit, "the edit must miss the lightmap memo");
    assert_eq!(
        after_edit.read_hits,
        lights.len() - 1,
        "unedited lights hit"
    );
    assert_eq!(
        after_edit.read_attempts - after_edit.read_hits,
        1,
        "only the edited light re-bakes"
    );
    let _ = std::fs::remove_dir_all(&root);
}
