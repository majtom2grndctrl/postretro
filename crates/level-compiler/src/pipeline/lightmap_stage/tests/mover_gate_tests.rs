//! The mover light gate inside the fused walk: light-window independence, its texel memo, the
//! post-hit pass that re-reads only the partitions a memo miss needs, and its share of the
//! predicted peak.
use super::*;
use crate::fixture_pipeline::{FixturePipeline, load_fixture};
use crate::light_namespaces::AnimatedBakedLights;
use crate::mover_light_gate::{GateLights, log_rows};

const FIXTURE: &str = "test_mover_light_gate";

struct GateBake {
    output: FusedLightingOutput,
    lightmap_total: Option<usize>,
    lightmap_completed: usize,
}

/// Bake the fixture's lights with `poses` under a `window`-light window,
/// four permits wide.
fn bake(
    fx: &FixturePipeline,
    lights: &[MapLight],
    poses: &MoverPoses,
    window: usize,
    cache: Option<&StageCache>,
) -> GateBake {
    let mut geometry = fx.geometry.clone();
    let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH must build");
    let static_lights = StaticBakedLights::from_lights(lights);
    let alpha_lights = AlphaLightsNs::from_lights(lights);
    let config = config(false);
    let prepared =
        lightmap_bake::prepare_atlas(&mut geometry, &static_lights, config.lightmap_density, &[])
            .expect("fixture atlas must prepare");
    let progress = StageProgress::indeterminate();
    let governor = Arc::new(Governor::new(4, false));
    // On this thread, so its caller-thread lines reach a capture; the
    // governor bounds its workers to four.
    let output = bake_fused_with_movers(
        &test_args(),
        cache,
        &BakeControl::new(governor, &progress),
        &BakeControl::unrestricted(),
        &mut geometry,
        &static_lights,
        &alpha_lights,
        None,
        &[],
        &crate::texel_footprint::NO_EMBEDDED_EMITTERS,
        &bvh,
        &primitives,
        &config,
        prepared,
        None,
        Some(poses),
        &PartitionWindow {
            size: window,
            ..PartitionWindow::default()
        },
    )
    .expect("gate fixture bake must succeed");
    GateBake {
        output,
        lightmap_total: progress.total(),
        lightmap_completed: progress.completed(),
    }
}

fn poses_of(fx: &FixturePipeline) -> MoverPoses {
    MoverPoses::resolve(&fx.kinematic_movers, &fx.kinematic_waypoints)
}

/// Every static row, as text.
fn rows_text(output: &FusedLightingOutput) -> String {
    let tally = output.mover_light.as_ref().expect("the gate ran");
    format!("{:?}", tally.rows().collect::<Vec<_>>())
}

/// The gate's `[MoverLight]` report over `output`'s static rows.
fn report(fx: &FixturePipeline, poses: &MoverPoses, output: &FusedLightingOutput) -> Vec<String> {
    let static_lights = StaticBakedLights::from_lights(&fx.lights);
    let animated_lights = AnimatedBakedLights::from_lights(&fx.lights);
    let lights = GateLights::resolve(&static_lights, &animated_lights);
    let capture = LogCapture::start();
    log_rows(
        poses,
        &lights,
        output.mover_light.as_ref().expect("the gate ran"),
        |source| format!("light {source}"),
    );
    capture
        .records()
        .into_iter()
        .filter(|record| record.message.contains("[MoverLight]"))
        .map(|record| format!("{} {}", record.level, record.message))
        .collect()
}

/// Move the last waypoint named `waypoint` by `offset`, engine metres.
fn move_last_waypoint(fx: &mut FixturePipeline, waypoint: &str, offset: DVec3) {
    let at = fx
        .kinematic_waypoints
        .iter()
        .position(|w| w.name == waypoint)
        .expect("the fixture names the waypoint");
    fx.kinematic_waypoints[at].origin += offset;
}

// The static gate's rows and report never depend on the light window.
#[test]
fn mover_gate_rows_and_report_match_across_light_window_sizes() {
    let fx = load_fixture(FIXTURE);
    let poses = poses_of(&fx);
    assert_eq!(poses.analysed.len(), 2, "both plates are analysed");
    let one = bake(&fx, &fx.lights, &poses, 1, None);
    let rows = rows_text(&one.output);
    let lines = report(&fx, &poses, &one.output);
    assert!(
        lines.iter().any(|line| line.starts_with("INFO"))
            && one.output.mover_light.as_ref().unwrap().rows().count() > 2,
        "the fixture gates rows at info\n{}",
        lines.join("\n")
    );
    for line in lines.iter().filter(|line| line.starts_with("INFO")) {
        assert!(
            !line.contains("texel(s)") && !line.contains("probe(s)"),
            "per-row counts never appear at info: {line}"
        );
    }
    for window in [3, 8] {
        let other = bake(&fx, &fx.lights, &poses, window, None);
        assert_eq!(rows_text(&other.output), rows, "window {window} rows");
        assert_eq!(
            report(&fx, &poses, &other.output),
            lines,
            "window {window} report"
        );
    }
}

// A warm bake serves every light from the memo; a moved mover re-gates only
// its lights, from cached partitions, with no visibility trace, and its rows
// and bytes equal a cold bake's.
#[test]
fn mover_gate_memo_serves_unchanged_lights_and_a_moved_mover_regates_from_cached_partitions() {
    let mut fx = load_fixture(FIXTURE);
    let poses = poses_of(&fx);
    let dir = fresh_cache_dir("mover_gate_memo");
    let cache = StageCache::new(&dir).expect("create gate memo cache");
    let cold = bake(&fx, &fx.lights, &poses, 3, Some(&cache));
    let gated_lights = cache.test_access("mover_light_texels").writes;
    assert!(gated_lights > 0, "the cold bake stores each gated light");
    cache.clear_test_accesses();

    let logs = LogCapture::start();
    let warm = bake(&fx, &fx.lights, &poses, 3, Some(&cache));
    logs.assert_logged_once(Level::Info, "[cache] lightmap_section hit");
    logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas hit");
    logs.assert_logged_once(Level::Info, "[cache] mover_light_texels hit");
    logs.assert_not_logged(Level::Warn, "[cache] corrupt");
    let memo = cache.test_access("mover_light_texels");
    assert_eq!((memo.read_hits, memo.writes), (gated_lights, 0));
    assert_eq!(
        cache.test_access("lightmap_layer").read_attempts,
        0,
        "nothing re-reads"
    );
    assert_eq!(rows_text(&warm.output), rows_text(&cold.output));
    drop(logs);
    cache.clear_test_accesses();

    // Slide the east plate's far end 2 m further; the west plate stays.
    move_last_waypoint(&mut fx, "east_b", DVec3::new(2.0, 0.0, 0.0));
    let moved = poses_of(&fx);
    let reference = bake(&fx, &fx.lights, &moved, 3, None);
    let logs = LogCapture::start();
    let edited = bake(&fx, &fx.lights, &moved, 3, Some(&cache));
    logs.assert_logged_once(Level::Info, "[cache] lightmap_section hit");
    logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas hit");
    logs.assert_logged(Level::Info, "[cache] mover_light_texels miss");
    let memo = cache.test_access("mover_light_texels");
    let regated = memo.writes;
    assert!(regated > 0 && memo.read_hits + regated == memo.read_attempts);
    let layers = cache.test_access("lightmap_layer");
    assert_eq!(
        layers.read_attempts, layers.read_hits,
        "no partition re-bakes"
    );
    assert_eq!(
        layers.read_hits,
        regated * edited.output.lightmap.layer_count as usize,
        "each re-gated light reads each of its partitions once"
    );
    assert_eq!(rows_text(&edited.output), rows_text(&reference.output));
    assert_eq!(
        edited.output.lightmap.section.to_bytes(),
        reference.output.lightmap.section.to_bytes()
    );
    assert_eq!(
        edited.output.shadowmask.as_ref().map(|s| s.to_bytes()),
        reference.output.shadowmask.as_ref().map(|s| s.to_bytes()),
        "a gate-only re-read never moves a shadowmask byte"
    );
    assert_eq!(
        edited.lightmap_total,
        Some(edited.lightmap_completed),
        "progress ends at 100%"
    );
    drop(logs);
    drop(cache);
    let _ = std::fs::remove_dir_all(&dir);
}

// A mover edit plus a spec-index shift misses both the shadowmask and the
// gate memo over a reused lightmap; one read per partition feeds both.
#[test]
fn a_shadowmask_and_gate_miss_over_a_reused_lightmap_read_each_partition_once() {
    let mut fx = load_fixture(FIXTURE);
    let poses = poses_of(&fx);
    let dir = fresh_cache_dir("mover_gate_union");
    let cache = StageCache::new(&dir).expect("create gate union cache");
    bake(&fx, &fx.lights, &poses, 3, Some(&cache));
    cache.clear_test_accesses();

    move_last_waypoint(&mut fx, "west_b", DVec3::new(0.0, 0.0, -1.0));
    let moved = poses_of(&fx);
    // An SDF-shadow light ahead of the rest shifts every spec index and no
    // layer key.
    let mut sdf = fx.lights[0].clone();
    sdf.shadow_type = ShadowType::Sdf;
    let mut shifted = fx.lights.clone();
    shifted.insert(0, sdf);
    let reference = bake(&fx, &shifted, &moved, 3, None);
    let logs = LogCapture::start();
    let edited = bake(&fx, &shifted, &moved, 3, Some(&cache));
    logs.assert_logged_once(Level::Info, "[cache] lightmap_section hit");
    logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas miss");
    logs.assert_logged(Level::Info, "[cache] mover_light_texels miss");
    let layers = cache.test_access("lightmap_layer");
    let layer_lights = StaticBakedLights::from_lights(&shifted)
        .entries()
        .iter()
        .filter(|entry| entry.light.shadow_type != ShadowType::Sdf)
        .count();
    assert_eq!(
        layers.read_attempts, layers.read_hits,
        "no partition re-bakes"
    );
    assert_eq!(
        layers.read_hits,
        layer_lights * edited.output.lightmap.layer_count as usize,
        "every light owns, so the union is every partition, each read once"
    );
    assert_eq!(rows_text(&edited.output), rows_text(&reference.output));
    assert_eq!(
        edited.output.shadowmask.as_ref().map(|s| s.to_bytes()),
        reference.output.shadowmask.as_ref().map(|s| s.to_bytes()),
    );
    assert_eq!(edited.lightmap_total, Some(edited.lightmap_completed));
    drop(logs);
    drop(cache);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Delete every static light's layer entries from `dir`, as an eviction
/// would; the section and shadowmask memos stay.
fn evict_layers(fx: &FixturePipeline, dir: &std::path::Path) -> usize {
    let mut geometry = fx.geometry.clone();
    let (_, primitives, _) = build_bvh(&geometry).expect("fixture BVH must build");
    let static_lights = StaticBakedLights::from_lights(&fx.lights);
    let config = config(false);
    let prepared =
        lightmap_bake::prepare_atlas(&mut geometry, &static_lights, config.lightmap_density, &[])
            .expect("fixture atlas must prepare");
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let mut evicted = 0;
    for entry in static_lights.entries() {
        for layer in 0..prepared.layer_count {
            let hash = lightmap_layer::layer_input_hash(
                entry.light,
                &shared,
                &primitives,
                &geometry,
                config.lightmap_density,
                test_args().soft_shadow_samples,
                config.penumbra_floor,
                crate::ray_traversal::TRACER_EPOCH,
                layer,
            );
            let key = CacheKey::new(
                "lightmap_layer",
                lightmap_layer::LAYER_FORMAT_VERSION,
                &hash,
            );
            if std::fs::remove_file(dir.join(key.as_filename())).is_ok() {
                evicted += 1;
            }
        }
    }
    evicted
}

// A gate miss whose layer entries are gone re-bakes them, as the
// ShadowmaskAtlas re-read does, and still reports what a cold bake does.
#[test]
fn a_gate_miss_rebakes_a_partition_whose_layer_entry_is_gone() {
    let mut fx = load_fixture(FIXTURE);
    let poses = poses_of(&fx);
    let dir = fresh_cache_dir("mover_gate_evicted");
    let cache = StageCache::new(&dir).expect("create gate eviction cache");
    bake(&fx, &fx.lights, &poses, 3, Some(&cache));
    assert!(evict_layers(&fx, &dir) > 0, "the cold bake stored layers");
    cache.clear_test_accesses();

    move_last_waypoint(&mut fx, "east_b", DVec3::new(2.0, 0.0, 0.0));
    let moved = poses_of(&fx);
    let reference = bake(&fx, &fx.lights, &moved, 3, None);
    let logs = LogCapture::start();
    let edited = bake(&fx, &fx.lights, &moved, 3, Some(&cache));
    logs.assert_logged_once(Level::Info, "[cache] lightmap_section hit");
    logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas hit");
    let regated = cache.test_access("mover_light_texels").writes;
    let layers = cache.test_access("lightmap_layer");
    assert_eq!(layers.read_hits, 0, "every layer entry is gone");
    assert_eq!(
        layers.writes,
        regated * edited.output.lightmap.layer_count as usize,
        "each re-gated light re-bakes and stores each of its partitions"
    );
    assert_eq!(rows_text(&edited.output), rows_text(&reference.output));
    assert_eq!(
        edited.output.shadowmask.as_ref().map(|s| s.to_bytes()),
        reference.output.shadowmask.as_ref().map(|s| s.to_bytes()),
    );
    assert_eq!(edited.lightmap_total, Some(edited.lightmap_completed));
    drop(logs);
    drop(cache);
    let _ = std::fs::remove_dir_all(&dir);
}

// The gate's bytes enter the peak only with an analysed mover, and its
// per-partition lists are charged over the most crowded layer's charts, since
// a partition walks one layer.
#[test]
fn predicted_peak_charges_the_gate_only_when_a_mover_is_analysed() {
    let fx = load_fixture(FIXTURE);
    let mut geometry = fx.geometry.clone();
    let static_lights = StaticBakedLights::from_lights(&fx.lights);
    let prepared = lightmap_bake::prepare_atlas(&mut geometry, &static_lights, 0.25, &[])
        .expect("fixture atlas must prepare");
    let counts = metadata_counts(&prepared, &fx.lights);
    let without = predicted_peak(&prepared, counts, false, LIGHTMAP_PARTITION_WINDOW);
    let with = predicted_peak(
        &prepared,
        counts.with_movers(2),
        false,
        LIGHTMAP_PARTITION_WINDOW,
    );
    let mut layer_charts = vec![0u64; prepared.layer_count as usize];
    for placement in &prepared.placements {
        layer_charts[placement.layer as usize] += 1;
    }
    let largest_layer = layer_charts.into_iter().max().unwrap_or(0);
    let lights = counts.layer_lights as u64;
    let workers = (LIGHTMAP_PARTITION_WINDOW as u64).min(lights);
    let gate = super::super::mover_gate::predicted_gate_bytes(lights, 2, workers, largest_layer);
    assert!(gate > 0, "the gate's tallies and lists");
    assert_eq!(
        with.metadata - without.metadata,
        gate,
        "the gate's bound, over the largest layer's charts"
    );
    assert!(
        with.cache_metadata > without.cache_metadata,
        "one memo entry per layer light"
    );
    assert_eq!(
        with.total() - without.total(),
        (with.metadata - without.metadata) + (with.cache_metadata - without.cache_metadata),
        "nothing else moves"
    );
}
