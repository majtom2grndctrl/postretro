// The SH working-set gates against what the build holds: each priced phase
// at or above the live-bytes ledger's peak inside that phase's window, the
// mover bake and the direct half among them, through the whole pipeline.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Memory)

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use super::delta_working_set::DeltaWorkingSetProjection;
use super::sh_working_set_gates::recorded;
use crate::live_bytes::{Report, measured};
use crate::reporter::{PlainReporter, Reporter};
use crate::surface_radiance::ShBaseWorkingSetProjection;

/// Dev map `name`'s text, `edit`ed, written as `root/maps/<stem>.map` beside
/// a link to the dev textures.
pub(super) fn dev_map_in(
    root: &Path,
    name: &str,
    stem: &str,
    edit: impl FnOnce(String) -> String,
) -> PathBuf {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let text = std::fs::read_to_string(
        workspace
            .join("content/dev/maps")
            .join(format!("{name}.map")),
    )
    .expect("read dev map");
    std::fs::create_dir_all(root.join("maps")).expect("maps dir");
    let textures = root.join("textures");
    if !textures.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(workspace.join("content/dev/textures"), &textures)
            .expect("link the dev textures");
    }
    let path = root.join("maps").join(format!("{stem}.map"));
    std::fs::write(&path, edit(text)).expect("write map");
    path
}

/// One measured build of `map` with `flags`: the ledger and the projections
/// its gates checked.
pub(super) struct Measured {
    pub(super) report: Report,
    pub(super) delta: DeltaWorkingSetProjection,
    pub(super) base: ShBaseWorkingSetProjection,
}

/// Compile `map` in process on `threads` workers under one ledger.
pub(super) fn compile_measured(map: &Path, out: &Path, flags: &[&str], threads: usize) -> Measured {
    let path = |path: &Path| path.to_string_lossy().into_owned();
    let args = crate::parse_args_from(
        [
            path(map),
            "-o".to_owned(),
            path(&out.join("out.prl")),
            "--baked-root".to_owned(),
            path(&out.join("baked")),
        ]
        .into_iter()
        .chain(flags.iter().map(|flag| (*flag).to_owned())),
    )
    .expect("arguments");
    let cache = (!args.no_cache && !args.release)
        .then(|| crate::construct_stage_cache(&args).expect("a warm build"));
    let (projections, report) = measured(threads, || {
        let started = Instant::now();
        let reporter: Arc<dyn Reporter> = Arc::new(PlainReporter::new(
            started,
            crate::logger::LogSink::default(),
        ));
        crate::pipeline::run(
            &args,
            cache,
            started,
            reporter,
            Arc::new(crate::governor::Governor::new(threads, false)),
        )
        .expect("the map compiles");
        recorded::take().expect("the gates checked")
    });
    let (delta, base) = projections;
    Measured {
        report,
        delta,
        base,
    }
}

/// The priced bytes for the ledger window ending at `label`.
fn price_of(label: &str, measured: &Measured) -> u64 {
    let base = &measured.base;
    match label {
        "gate" => base.gate_phase_bytes,
        "plan and probe layout" | "layers" => base.layer_phase_bytes,
        "sum" => base.sum_phase_bytes,
        "later bounces" => base.bounce_phase_bytes,
        "probe gather" => base.gather_phase_bytes,
        "mover bake" => base.mover_bounce_phase_bytes,
        "volume store" | "volume load" => base.volume_entry_phase_bytes,
        "sh delta stages" => measured.delta.estimated_peak_bytes,
        "mover direct" => base.mover_direct_phase_bytes,
        other => panic!("no priced phase for the ledger window `{other}`"),
    }
}

/// Assert every window `measured` marked sits at or under its price, and
/// return one row per window.
pub(super) fn assert_windows_priced(what: &str, measured: &Measured) -> Vec<String> {
    let labels: Vec<&str> = measured
        .report
        .phases
        .iter()
        .map(|phase| phase.label)
        .collect();
    for needed in ["mover bake", "sh delta stages", "mover direct"] {
        assert!(
            labels.contains(&needed),
            "{what}: no `{needed}` window in {labels:?}"
        );
    }
    measured
        .report
        .phases
        .iter()
        .map(|phase| {
            let price = price_of(phase.label, measured);
            let row = format!(
                "{what}: {:<22} priced {:>12} measured {:>12} ({:.2}x)",
                phase.label,
                price,
                phase.peak,
                price as f64 / phase.peak.max(1) as f64
            );
            assert!(price >= phase.peak, "priced under the ledger: {row}");
            row
        })
        .collect()
}

/// The closet with a second door on the first's path.
fn two_doors(text: String) -> String {
    super::mover_bounce_stage::tests::two_doors(text)
}

// The closet's two-door edit, at `low` and `high`: cold, warm over an empty
// cache, on a full hit, on an id-34 hit whose doors miss their memos (a
// `light_bounces` edit after priming), and as a `low` build over a
// `high`-primed cache; and each with `--mover-payload-max-size` set to the
// doors' blocks' bytes, so the cap's term carries no slack (P1, P2, W1, D1,
// D3).
#[test]
fn every_priced_phase_covers_its_window_on_the_closet() {
    let root = tempfile::tempdir().expect("root");
    let doors = |bounces: u32| {
        move |text: String| {
            two_doors(text.replace(
                "\"name\" \"closet_door\"",
                &format!("\"name\" \"closet_door\"\n\"light_bounces\" \"{bounces}\""),
            ))
        }
    };
    let map = dev_map_in(root.path(), "mover-closet", "two-doors", doors(2));
    let primer = dev_map_in(root.path(), "mover-closet", "two-doors-primer", doors(1));
    let mut rows = Vec::new();
    let out = tempfile::tempdir().expect("out");
    let cap_flag = |bytes: u64| vec!["--mover-payload-max-size".to_owned(), bytes.to_string()];
    for fidelity in ["low", "high"] {
        let fid = ["--bake-fidelity", fidelity];
        let cold = compile_measured(&map, out.path(), &[&fid[..], &["--no-cache"]].concat(), 4);
        rows.extend(assert_windows_priced(&format!("{fidelity} cold"), &cold));
        let cache = tempfile::tempdir().expect("cache");
        let warm: Vec<&str> = [
            &fid[..],
            &["--cache-dir", cache.path().to_str().expect("utf-8")],
        ]
        .concat();
        rows.extend(assert_windows_priced(
            &format!("{fidelity} warm empty"),
            &compile_measured(&map, out.path(), &warm, 4),
        ));
        rows.extend(assert_windows_priced(
            &format!("{fidelity} full hit"),
            &compile_measured(&map, out.path(), &warm, 4),
        ));
        let primed = tempfile::tempdir().expect("cache");
        let primed_flags: Vec<&str> = [
            &fid[..],
            &["--cache-dir", primed.path().to_str().expect("utf-8")],
        ]
        .concat();
        compile_measured(&primer, out.path(), &primed_flags, 4);
        rows.extend(assert_windows_priced(
            &format!("{fidelity} id-34 hit, memos miss"),
            &compile_measured(&map, out.path(), &primed_flags, 4),
        ));
        // The cap at the doors' blocks' bytes: read from the cold build's
        // log-free ledger, the blocks' own kind at its high-water.
        let blocks = cold.report.high_water["mover bounce blocks"];
        let capped = cap_flag(blocks);
        let capped: Vec<&str> = [&fid[..], &["--no-cache"], &[&capped[0], &capped[1]]].concat();
        rows.extend(assert_windows_priced(
            &format!("{fidelity} cold, cap at the blocks"),
            &compile_measured(&map, out.path(), &capped, 4),
        ));
    }
    // A `low` build over a `high`-primed cache.
    let high = tempfile::tempdir().expect("cache");
    let high_flags = [
        "--bake-fidelity",
        "high",
        "--cache-dir",
        high.path().to_str().expect("utf-8"),
    ];
    compile_measured(&map, out.path(), &high_flags, 4);
    let low_over_high = [
        "--bake-fidelity",
        "low",
        "--cache-dir",
        high.path().to_str().expect("utf-8"),
    ];
    rows.extend(assert_windows_priced(
        "low over high",
        &compile_measured(&map, out.path(), &low_over_high, 4),
    ));
    for row in &rows {
        eprintln!("{row}");
    }
}
