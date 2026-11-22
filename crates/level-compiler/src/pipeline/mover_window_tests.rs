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
    try_compile_measured(map, out, flags, threads).expect("the map compiles")
}

/// [`compile_measured`], handing back a failed build's error with the
/// projections its gates checked.
#[allow(clippy::result_large_err)] // A test helper: the error carries what the gates checked.
pub(super) fn try_compile_measured(
    map: &Path,
    out: &Path,
    flags: &[&str],
    threads: usize,
) -> Result<
    Measured,
    (
        anyhow::Error,
        Option<(DeltaWorkingSetProjection, ShBaseWorkingSetProjection)>,
    ),
> {
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
        let result = crate::pipeline::run(
            &args,
            cache,
            started,
            reporter,
            Arc::new(crate::governor::Governor::new(threads, false)),
        );
        (result, recorded::take())
    });
    match projections {
        (Ok(()), Some((delta, base))) => Ok(Measured {
            report,
            delta,
            base,
        }),
        (Ok(()), None) => panic!("the gates checked no projection"),
        (Err(error), recorded) => Err((error, recorded)),
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
    assert_sections_priced(what, measured);
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

/// The sum of the named ledger kinds' high-water marks; a kind never held
/// counts zero.
fn kinds(measured: &Measured, names: &[&str]) -> u64 {
    names
        .iter()
        .map(|name| measured.report.high_water.get(name).copied().unwrap_or(0))
        .sum()
}

/// Each section the windows route at or under its own term in the price
/// (D1, D2), what the channel carries at or under the SH-delta gate's
/// carried term (D3), and the finish loop's buffers routed in the mover
/// bake (P1).
pub(super) fn assert_sections_priced(what: &str, measured: &Measured) {
    let base = &measured.base;
    let cost = &base.mover_cost;
    let window = &base.direct_window;
    let checks = [
        (
            "id 34 and its clone",
            kinds(measured, &["id-34 section"]) + kinds(measured, &["id-34 analysis clone"]),
            2 * base.section_bytes,
        ),
        (
            "id 35 and its clone",
            kinds(measured, &["id-35 section", "id-35 analysis clone"]),
            window.direct_volume_bytes,
        ),
        (
            "ids 27, 41 and 45 and their clones",
            kinds(
                measured,
                &[
                    "id-27 section",
                    "id-41 section",
                    "id-45 section",
                    "id-27 analysis clone",
                    "id-41 analysis clone",
                    "id-45 analysis clone",
                ],
            ),
            window.dense_delta_bytes,
        ),
        (
            "entity-shadow selection",
            kinds(measured, &["entity-shadow section"]),
            window.entity_shadow_bytes,
        ),
        (
            "id 57",
            kinds(measured, &["id-57 section"]),
            cost.payload_cap + cost.direct_entry_bytes,
        ),
        (
            "direct entries",
            kinds(measured, &["mover direct entries"]),
            cost.direct_entry_bytes,
        ),
        (
            "posed solids",
            kinds(measured, &["posed solids"]),
            cost.solid_bytes,
        ),
        (
            "bounce and depth blocks",
            kinds(measured, &["mover bounce blocks"]),
            cost.payload_cap,
        ),
        (
            "what the channel carries",
            kinds(
                measured,
                &[
                    "id-34 section",
                    "id-34 analysis clone",
                    "mover bounce blocks",
                    "posed solids",
                ],
            ),
            measured.delta.carried_bytes,
        ),
    ];
    for (name, held, priced) in checks {
        assert!(
            held <= priced,
            "{what}: {name} held {held} B past its term {priced} B"
        );
    }
    assert!(
        measured.delta.carried_bytes > 0,
        "{what}: the SH-delta gate carries the channel"
    );
    for kind in [
        "mover bounce blocks",
        "mover probe cells",
        "mover absent depth",
        "mover finish names",
    ] {
        assert!(
            measured
                .report
                .high_water
                .get(kind)
                .is_some_and(|&bytes| bytes > 0),
            "{what}: the mover bake routes `{kind}`: {:?}",
            measured.report.high_water
        );
    }
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
    let mut cold_projections = Vec::new();
    let cap_flag = |bytes: u64| vec!["--mover-payload-max-size".to_owned(), bytes.to_string()];
    for fidelity in ["low", "high"] {
        let fid = ["--bake-fidelity", fidelity];
        let cold = compile_measured(&map, out.path(), &[&fid[..], &["--no-cache"]].concat(), 4);
        rows.extend(assert_windows_priced(&format!("{fidelity} cold"), &cold));
        cold_projections.push(cold.base.clone());
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
        // The cap at the doors' blocks' bytes, so its term carries no
        // slack: the blocks' kind in the cold build's ledger holds exactly
        // what the cap counts (the payload cap test proves the two agree).
        let blocks = cold.report.high_water["mover bounce blocks"];
        let capped = cap_flag(blocks);
        let capped_cold: Vec<&str> =
            [&fid[..], &["--no-cache"], &[&capped[0], &capped[1]]].concat();
        rows.extend(assert_windows_priced(
            &format!("{fidelity} cold, cap at the blocks"),
            &compile_measured(&map, out.path(), &capped_cold, 4),
        ));
        let capped_cache = tempfile::tempdir().expect("cache");
        let capped_warm: Vec<&str> = [
            &fid[..],
            &["--cache-dir", capped_cache.path().to_str().expect("utf-8")],
            &[&capped[0], &capped[1]],
        ]
        .concat();
        for mode in ["warm empty", "full hit"] {
            rows.extend(assert_windows_priced(
                &format!("{fidelity} {mode}, cap at the blocks"),
                &compile_measured(&map, out.path(), &capped_warm, 4),
            ));
        }
        let capped_primed = tempfile::tempdir().expect("cache");
        let capped_primed_flags: Vec<&str> = [
            &fid[..],
            &["--cache-dir", capped_primed.path().to_str().expect("utf-8")],
            &[&capped[0], &capped[1]],
        ]
        .concat();
        compile_measured(&primer, out.path(), &capped_primed_flags, 4);
        rows.extend(assert_windows_priced(
            &format!("{fidelity} id-34 hit, memos miss, cap at the blocks"),
            &compile_measured(&map, out.path(), &capped_primed_flags, 4),
        ));
        // Every section the closet's doors keep alive is routed: none of the
        // checks above passes on an empty kind. The doors gate no direct
        // light, so their direct bake holds nothing; closet-reveal's does
        // (the on-demand fixture run).
        for kind in [
            "id-34 section",
            "id-35 section",
            "id-57 section",
            "posed solids",
            "mover skip bars",
        ] {
            assert!(
                cold.report
                    .high_water
                    .get(kind)
                    .is_some_and(|&bytes| bytes > 0),
                "{fidelity}: `{kind}` routed: {:?}",
                cold.report.high_water
            );
        }
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
    // Fidelity never enters the price.
    assert_eq!(
        cold_projections[0], cold_projections[1],
        "low and high price alike"
    );
    for row in &rows {
        eprintln!("{row}");
    }
}

/// The two-door closet with its middle light pulsing (`style 5`): ids 27,
/// 41 and 45 bake beside the movers.
fn animated_two_doors(root: &Path) -> PathBuf {
    dev_map_in(root, "mover-closet", "two-doors-animated", |text| {
        let light = "\"origin\" \"320 392 288\"";
        assert!(text.contains(light), "the middle light");
        two_doors(text.replace(light, &format!("{light}\n\"style\" \"5\"")))
    })
}

// With an animated light, the dense deltas and, under `--sh-analyze`, their
// analysis clones and id 35's live through the direct window: every window
// still at or above its ledger, each section at or under its own term (D1,
// D2).
#[test]
fn the_direct_window_covers_every_section_alive() {
    let root = tempfile::tempdir().expect("root");
    let map = animated_two_doors(root.path());
    let out = tempfile::tempdir().expect("out");
    let mut rows = Vec::new();
    for (what, flags) in [
        ("animated, cold", vec!["--no-cache"]),
        ("animated, --sh-analyze", vec!["--no-cache", "--sh-analyze"]),
    ] {
        let measured = compile_measured(&map, out.path(), &flags, 4);
        assert!(
            measured.delta.cumulative_dense_bytes > 0
                && kinds(&measured, &["id-27 section", "id-45 section"]) > 0,
            "{what}: the animated light bakes dense deltas"
        );
        rows.extend(assert_windows_priced(what, &measured));
    }
    for row in &rows {
        eprintln!("{row}");
    }
}

// `--sh-delta-max-size` below ids 27, 41 and 45's dense bytes but at or
// above their post-drop payload admits the build, and the direct window's
// price still covers each delta section as the window holds it, dense (D1).
#[test]
fn a_delta_cap_below_dense_still_prices_the_dense_window() {
    let root = tempfile::tempdir().expect("root");
    let map = animated_two_doors(root.path());
    let out = tempfile::tempdir().expect("out");
    let probe = compile_measured(&map, out.path(), &["--no-cache"], 4);
    let dense = probe.delta.cumulative_dense_bytes;
    // The deltas are held dense past the cap: the window prices them dense.
    let held = kinds(&probe, &["id-27 section", "id-41 section", "id-45 section"]);
    assert!(
        held > dense - 1,
        "the deltas held {held} B, past a cap of {}",
        dense - 1
    );
    let cap = (dense - 1).to_string();
    let measured = compile_measured(
        &map,
        out.path(),
        &["--no-cache", "--sh-delta-max-size", &cap],
        4,
    );
    for row in assert_windows_priced("delta cap one under dense", &measured) {
        eprintln!("{row}");
    }
}

// On the two-door closet the SH-delta gate's projection sits under the SH
// base gate's peak, so the delta gate checks first. One byte under the SH
// base peak refuses by the SH base gate's name, before any SH ray; at the
// peak both SH gates admit, and only the lightmap stage's own gate, which
// reads the same budget after them, may still refuse (G1).
#[test]
fn one_byte_under_the_sh_base_peak_refuses_by_its_name_through_gate_order() {
    let root = tempfile::tempdir().expect("root");
    let map = dev_map_in(root.path(), "mover-closet", "two-doors", two_doors);
    let out = tempfile::tempdir().expect("out");
    let probe = compile_measured(&map, out.path(), &["--no-cache"], 4);
    let (delta, peak) = (
        probe.delta.estimated_peak_bytes,
        probe.base.estimated_peak_bytes,
    );
    assert!(
        delta < peak,
        "the cheaper SH-delta gate checks first: {delta} against {peak}"
    );
    let under = (peak - 1).to_string();
    let (error, recorded) = try_compile_measured(
        &map,
        out.path(),
        &["--no-cache", "--working-set-max-size", &under],
        4,
    )
    .err()
    .expect("one byte under the peak refuses");
    let message = format!("{error:#}");
    assert!(
        message.contains("SH base working-set gate refused before patch shading")
            && message.contains(&format!("estimated peak {peak} bytes")),
        "{message}"
    );
    assert!(recorded.is_some(), "refused at the gates");
    // At the peak both SH gates admit. The lightmap stage's own gate reads
    // the same budget later, with its own estimator, and may refuse there.
    let at = peak.to_string();
    if let Err((error, recorded)) = try_compile_measured(
        &map,
        out.path(),
        &["--no-cache", "--working-set-max-size", &at],
        4,
    ) {
        let message = format!("{error:#}");
        assert!(
            message.contains("Lightmap working-set gate refused") && recorded.is_some(),
            "at the peak, only a later gate may refuse: {message}"
        );
    }
}

// Every mover fixture beyond the closet, at `low` and `high`: cold, warm
// over an empty cache and on a full hit, each priced phase at or above the
// ledger's peak in its window and each section at or under its term (P1,
// P2, D1, D3). `POSTRETRO_MOVER_WINDOW_MAPS` narrows it to a comma-separated
// list of dev map stems.
#[test]
#[ignore = "compiles every mover fixture six times in process (the setpiece alone takes minutes); run on demand"]
fn every_priced_phase_covers_its_window_on_every_mover_fixture() {
    let names: Vec<String> = std::env::var("POSTRETRO_MOVER_WINDOW_MAPS")
        .map(|names| {
            names
                .split(',')
                .map(|name| name.trim().to_owned())
                .collect()
        })
        .unwrap_or_else(|_| {
            [
                "mover-setpiece",
                "mover-pins",
                "closet-reveal",
                "enemy-movers",
            ]
            .map(str::to_owned)
            .to_vec()
        });
    let maps = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev/maps");
    for name in &names {
        // In place: a fixture's data script resolves beside it. Every
        // output goes to the temporary directory.
        let map = maps.join(format!("{name}.map"));
        let out = tempfile::tempdir().expect("out");
        let mut projections = Vec::new();
        for fidelity in ["low", "high"] {
            let fid = ["--bake-fidelity", fidelity];
            let cold = compile_measured(&map, out.path(), &[&fid[..], &["--no-cache"]].concat(), 4);
            for row in assert_windows_priced(&format!("{name} {fidelity} cold"), &cold) {
                eprintln!("{row}");
            }
            projections.push(cold.base);
            let cache = tempfile::tempdir().expect("cache");
            let warm: Vec<&str> = [
                &fid[..],
                &["--cache-dir", cache.path().to_str().expect("utf-8")],
            ]
            .concat();
            for mode in ["warm empty", "full hit"] {
                let measured = compile_measured(&map, out.path(), &warm, 4);
                for row in assert_windows_priced(&format!("{name} {fidelity} {mode}"), &measured) {
                    eprintln!("{row}");
                }
            }
        }
        assert_eq!(
            projections[0], projections[1],
            "{name}: low and high price alike"
        );
    }
}
