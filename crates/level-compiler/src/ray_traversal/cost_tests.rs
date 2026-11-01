// Tracer cost counters on a real map: box tests, leaf visits and triangle tests per ray.
// See: context/lib/build_pipeline.md §Ray traversal

use std::sync::Arc;
use std::time::Instant;

use crate::bake_counters::{TraceCounts, counted};

/// Map compiled by [`trace_counts_on_a_map`] unless
/// `POSTRETRO_TRACE_COUNT_MAP` names another under `content/dev/maps/`.
const DEFAULT_MAP: &str = "stress-warren-hallway-inspection-mini";

/// One cold in-process compile of a map, counted on a fresh pool. Run with
/// `--release` and `--nocapture`; the per-ray table is the before/after
/// record of a tracer change. Counts are integer sums over workers, so a
/// second run prints the same table.
#[test]
#[ignore = "compiles a stress map cold; run on demand with --release --nocapture"]
fn trace_counts_on_a_map() {
    let map = std::env::var("POSTRETRO_TRACE_COUNT_MAP").unwrap_or_else(|_| DEFAULT_MAP.into());
    let out = tempfile::tempdir().expect("output dir");
    let prl = out.path().join(format!("{map}.prl"));
    let baked = out.path().join("baked");
    std::fs::create_dir_all(baked.join("materials")).expect("baked root");
    let argv = [
        crate::fixture_pipeline::fixture_path(&map)
            .to_str()
            .expect("UTF-8 path")
            .to_owned(),
        "--release".to_owned(),
        "--no-tui".to_owned(),
        "-o".to_owned(),
        prl.to_str().expect("UTF-8 path").to_owned(),
        "--baked-root".to_owned(),
        baked.to_str().expect("UTF-8 path").to_owned(),
    ];
    let args = crate::parse_args_from(argv.into_iter()).expect("arguments parse");
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let ((), counts) = counted(workers, || {
        let started = Instant::now();
        let reporter: Arc<dyn crate::reporter::Reporter> = Arc::new(
            crate::reporter::PlainReporter::new(started, crate::logger::LogSink::default()),
        );
        crate::pipeline::run(
            &args,
            None,
            started,
            reporter,
            Arc::new(crate::governor::Governor::new(workers, false)),
        )
        .expect("map compiles");
    });
    println!("trace counts on {map}:");
    for (kind, trace) in [
        ("shadow", counts.shadow_trace),
        ("gather", counts.gather_trace),
    ] {
        println!("{kind}: {}", row(&trace));
    }
}

fn row(trace: &TraceCounts) -> String {
    let per_ray = |n: u64| n as f64 / trace.rays.max(1) as f64;
    format!(
        "rays {} | box tests {} ({:.3}/ray) | leaf visits {} ({:.3}/ray) | triangle tests {} ({:.3}/ray) | det rejections {} ({:.6}/ray)",
        trace.rays,
        trace.box_tests,
        per_ray(trace.box_tests),
        trace.leaf_visits,
        per_ray(trace.leaf_visits),
        trace.triangle_tests,
        per_ray(trace.triangle_tests),
        trace.det_rejections,
        per_ray(trace.det_rejections),
    )
}
