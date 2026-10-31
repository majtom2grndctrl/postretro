//! Shared CLI working-set budget, from projection through warm memo admission.
//! See: context/plans/done/lighting-scale--lightmap-layer-budget-gate.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::{SectionId, read_container, read_section_data};

const SH_ADMITTED: &str = "SH delta working-set gate: estimated peak ";
const SH_REFUSED: &str = "SH delta working-set gate refused before dense baking: estimated peak ";
const SH_BASE_ADMITTED: &str = "SH base working-set gate: estimated peak ";
const SH_BASE_REFUSED: &str =
    "SH base working-set gate refused before patch shading: estimated peak ";
const LIGHTMAP_ADMITTED: &str = "Lightmap working-set gate: predicted ";
const LIGHTMAP_REFUSED: &str = "Lightmap working-set gate refused before baking: predicted ";

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}

fn fixture() -> PathBuf {
    workspace().join("content/dev/maps/test_animated_weight_maps_mixed.map")
}

fn diagnostic(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn command(input: &Path, output: &Path, scratch: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_prl-build"));
    command
        .current_dir(workspace())
        .env("RUST_LOG", "info")
        .env("NO_COLOR", "1")
        .arg(input)
        .arg("-o")
        .arg(output)
        .args([
            "--no-tui",
            "--verbose",
            "--sh-probe-spacing",
            "4",
            "--lightmap-density",
            "0.25",
            "--soft-shadow-samples",
            "4",
            "--uncompressed-irradiance",
            "-j",
            "1",
        ])
        .arg("--baked-root")
        .arg(scratch.join("baked"));
    command
}

fn build(
    input: &Path,
    scratch: &Path,
    label: &str,
    cache: Option<&Path>,
    budget: Option<u64>,
) -> (Output, PathBuf) {
    let output_path = scratch.join(format!("{label}.prl"));
    let mut command = command(input, &output_path, scratch);
    if let Some(cache) = cache {
        command.arg("--cache-dir").arg(cache);
    } else {
        command.arg("--no-cache");
    }
    if let Some(budget) = budget {
        command
            .arg("--working-set-max-size")
            .arg(budget.to_string());
    }
    (
        command
            .output()
            .expect("run tiny compiler contract fixture"),
        output_path,
    )
}

fn admitted_bytes(log: &str, marker: &str) -> u64 {
    assert_eq!(
        log.matches(marker).count(),
        1,
        "expected one admission line: {log}"
    );
    let tail = log.split_once(marker).unwrap().1;
    let line = tail.lines().next().unwrap();
    assert!(line.contains(", admitted"), "gate was not admitted: {line}");
    tail.split_once(" bytes")
        .unwrap()
        .0
        .parse()
        .expect("gate reports exact bytes")
}

fn assert_success(output: &Output) -> String {
    let log = diagnostic(output);
    assert!(output.status.success(), "tiny compile failed: {log}");
    log
}

fn assert_before_lightmap_work(log: &str) {
    for marker in [
        "[cache] lightmap_section",
        "[cache] shadowmask_atlas",
        "[cache] lightmap_layer",
        "[ShadowmaskAtlas] cut-face estimate:",
    ] {
        assert!(
            !log.contains(marker),
            "refusal ran lightmap work `{marker}`: {log}"
        );
    }
}

/// Markers of the work an SH gate refusal must precede: texture reads (the
/// colour-space validation and the material stage), the static emitter
/// tests, and the SH base bake's patch shading and rays.
fn assert_before_sh_work(log: &str) {
    for marker in [
        "Texture color-space validation...",
        "World material bake...",
        "Embedded-emitter classification:",
        "SH volume bake...",
    ] {
        assert!(
            !log.contains(marker),
            "refusal ran SH work `{marker}`: {log}"
        );
    }
    assert_before_lightmap_work(log);
}

/// The one line `marker` opens, which must say the gate refused.
fn refused_bytes(log: &str, marker: &str) -> u64 {
    assert_eq!(
        log.matches(marker).count(),
        1,
        "expected one refusal line: {log}"
    );
    let tail = log.split_once(marker).unwrap().1;
    let line = tail.lines().next().unwrap();
    assert!(line.contains(", refused"), "gate was not refused: {line}");
    tail.split_once(" bytes")
        .unwrap()
        .0
        .parse()
        .expect("gate reports exact bytes")
}

/// One SH gate as its log lines name it, and its seed build's prediction.
#[derive(Clone, Copy, Debug)]
struct ShGate {
    /// Opens the gate's verdict line, admitted or refused.
    verdict: &'static str,
    /// Opens the gate's named refusal error.
    refused: &'static str,
    bytes: u64,
}

/// Both SH gates' seed predictions, cheaper first: the order the pipeline
/// checks them in, whichever gate the fixture prices lower.
fn sh_gates_cheaper_first(log: &str) -> (ShGate, ShGate) {
    let base = ShGate {
        verdict: SH_BASE_ADMITTED,
        refused: SH_BASE_REFUSED,
        bytes: admitted_bytes(log, SH_BASE_ADMITTED),
    };
    let delta = ShGate {
        verdict: SH_ADMITTED,
        refused: SH_REFUSED,
        bytes: admitted_bytes(log, SH_ADMITTED),
    };
    assert!(
        base.bytes > 0 && delta.bytes > 0 && base.bytes != delta.bytes,
        "the fixture must tell the two SH gates apart: {log}"
    );
    if base.bytes < delta.bytes {
        (base, delta)
    } else {
        (delta, base)
    }
}

// AC 30, research P7 and P24: the SH base gate admits at its own logged
// prediction and refuses one byte under it by name, before any texture
// read, emitter test, patch shading or SH ray, cold and warm. Both SH gates
// predict first and check cheapest first, whichever that is: a budget
// between them refuses by the one it fails, and a budget under both by the
// cheaper one.
#[test]
fn sh_base_working_set_gate_refuses_by_name_at_exact_boundary_cold_and_warm() {
    let scratch = tempfile::tempdir().unwrap();
    let cache = scratch.path().join("cache");
    let input = fixture();
    let (seed, _) = build(&input, scratch.path(), "seed", Some(&cache), None);
    let log = assert_success(&seed);
    let (cheaper, dearer) = sh_gates_cheaper_first(&log);
    assert!(
        log.find(cheaper.verdict).unwrap() < log.find(dearer.verdict).unwrap(),
        "the cheaper gate is checked first: {log}"
    );

    for (label, stage_cache) in [("cold", None), ("warm", Some(cache.as_path()))] {
        // Between the predictions: the cheaper gate admits, the dearer
        // refuses by its own name.
        for budget in [cheaper.bytes, dearer.bytes - 1] {
            let (between, path) = build(
                &input,
                scratch.path(),
                &format!("{label}-between-{budget}"),
                stage_cache,
                Some(budget),
            );
            let log = diagnostic(&between);
            assert!(!between.status.success() && !path.exists(), "{log}");
            assert_eq!(admitted_bytes(&log, cheaper.verdict), cheaper.bytes);
            assert_eq!(refused_bytes(&log, dearer.verdict), dearer.bytes);
            assert!(
                log.contains(&format!(
                    "{}{} bytes exceeds budget {budget} bytes",
                    dearer.refused, dearer.bytes
                )),
                "{label}: {log}"
            );
            assert!(!log.contains(cheaper.refused), "{label}: {log}");
            assert_before_sh_work(&log);
        }

        // Under both: the cheaper gate refuses by its own name, and the
        // dearer one is never checked.
        let budget = cheaper.bytes - 1;
        let (below, path) = build(
            &input,
            scratch.path(),
            &format!("{label}-below-both"),
            stage_cache,
            Some(budget),
        );
        let log = diagnostic(&below);
        assert!(!below.status.success() && !path.exists(), "{log}");
        assert_eq!(refused_bytes(&log, cheaper.verdict), cheaper.bytes);
        assert!(
            log.contains(&format!(
                "{}{} bytes exceeds budget {budget} bytes",
                cheaper.refused, cheaper.bytes
            )),
            "{label}: {log}"
        );
        assert!(
            !log.contains(dearer.verdict) && !log.contains(dearer.refused),
            "{label}: the dearer gate is checked after the cheaper one: {log}"
        );
        assert_before_sh_work(&log);
    }
}

#[test]
fn shared_working_set_cli_reaches_lightmap_gate_cold_and_warm_at_exact_boundary() {
    let scratch = tempfile::tempdir().unwrap();
    let cache = scratch.path().join("cache");
    let input = fixture();
    let (seed, _) = build(&input, scratch.path(), "seed", Some(&cache), None);
    let log = assert_success(&seed);
    let delta = admitted_bytes(&log, SH_ADMITTED);
    let base = admitted_bytes(&log, SH_BASE_ADMITTED);
    // The budget both SH gates admit at: the dearer one's prediction.
    let (_, dearer_sh) = sh_gates_cheaper_first(&log);
    let sh = dearer_sh.bytes;
    let lightmap = admitted_bytes(&log, LIGHTMAP_ADMITTED);
    assert!(
        sh < lightmap,
        "fixture must distinguish the SH gates from the lightmap gate: {log}"
    );
    assert!(log.contains("[cache] lightmap_section miss"), "{log}");
    assert!(log.contains("[cache] shadowmask_atlas miss"), "{log}");
    let admission = log
        .split_once(LIGHTMAP_ADMITTED)
        .unwrap()
        .1
        .lines()
        .next()
        .unwrap();
    let layers: u32 = admission
        .split_once(" bytes, ")
        .unwrap()
        .1
        .split_once(" bytes, ")
        .unwrap()
        .1
        .split_once(" packed bake layers")
        .unwrap()
        .0
        .parse()
        .unwrap();
    assert!(layers > 0);

    for (label, stage_cache) in [("cold", None), ("warm", Some(cache.as_path()))] {
        let (at_budget, path) = build(
            &input,
            scratch.path(),
            &format!("{label}-at"),
            stage_cache,
            Some(lightmap),
        );
        let at_log = assert_success(&at_budget);
        assert!(path.is_file());
        assert_eq!(admitted_bytes(&at_log, LIGHTMAP_ADMITTED), lightmap);
        assert_eq!(admitted_bytes(&at_log, SH_ADMITTED), delta);
        assert_eq!(admitted_bytes(&at_log, SH_BASE_ADMITTED), base);
        assert!(
            at_log.contains(&format!(
                "budget {lightmap} bytes, {layers} packed bake layers, admitted"
            )),
            "{at_log}"
        );
        if stage_cache.is_some() {
            assert!(at_log.contains("[cache] lightmap_section hit"), "{at_log}");
            assert!(at_log.contains("[cache] shadowmask_atlas hit"), "{at_log}");
        }

        let (below, path) = build(
            &input,
            scratch.path(),
            &format!("{label}-below"),
            stage_cache,
            Some(lightmap - 1),
        );
        let refused = diagnostic(&below);
        assert!(
            !below.status.success(),
            "one byte below must refuse: {refused}"
        );
        assert!(!path.exists());
        assert_eq!(admitted_bytes(&refused, SH_ADMITTED), delta);
        assert_eq!(admitted_bytes(&refused, SH_BASE_ADMITTED), base);
        assert!(refused.contains(&format!("{LIGHTMAP_REFUSED}{lightmap} bytes exceeds budget {} bytes on {layers} packed bake layers (--working-set-max-size)", lightmap - 1)), "{refused}");
        assert_before_lightmap_work(&refused);
    }

    let (at_sh, path) = build(&input, scratch.path(), "at-sh", None, Some(sh));
    let log = diagnostic(&at_sh);
    assert_eq!(admitted_bytes(&log, SH_ADMITTED), delta);
    assert_eq!(admitted_bytes(&log, SH_BASE_ADMITTED), base);
    assert!(
        !at_sh.status.success() && !path.exists(),
        "later lightmap refusal expected: {log}"
    );
    assert!(
        log.to_lowercase().contains("lightmap") && log.contains("refused"),
        "{log}"
    );
    assert_before_lightmap_work(&log);

    let (below_sh, path) = build(&input, scratch.path(), "below-sh", None, Some(sh - 1));
    let log = diagnostic(&below_sh);
    assert!(!below_sh.status.success() && !path.exists(), "{log}");
    assert!(
        log.contains(&format!(
            "{}{sh} bytes exceeds budget {} bytes",
            dearer_sh.refused,
            sh - 1
        )),
        "{log}"
    );
    assert!(
        !log.contains(LIGHTMAP_ADMITTED) && !log.contains(LIGHTMAP_REFUSED),
        "{log}"
    );
    assert_before_sh_work(&log);
}

#[test]
fn retired_working_set_flag_is_rejected() {
    let scratch = tempfile::tempdir().unwrap();
    let retired = ["--sh", "delta", "working", "set", "max", "size"].join("-");
    // Regression: option-first and option-only invocations treated it as input.
    for args in [
        vec![retired.as_str(), "1GiB", "input.map"],
        vec!["input.map", retired.as_str(), "1GiB"],
        vec![retired.as_str()],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_prl-build"))
            .current_dir(scratch.path())
            .env("NO_COLOR", "1")
            .args(args)
            .output()
            .unwrap();
        let log = diagnostic(&output);
        assert!(!output.status.success());
        assert!(
            log.contains(&format!("unexpected argument: {retired}")),
            "{log}"
        );
        assert!(!scratch.path().join("input.prl").exists());
    }
}

#[test]
fn animated_only_map_compiles_at_sh_budget_with_empty_static_lightmap() {
    let scratch = tempfile::tempdir().unwrap();
    let map = std::fs::read_to_string(fixture()).unwrap();
    let static_origin = "\"origin\" \"48 32 100\"";
    assert_eq!(map.matches(static_origin).count(), 1);
    let map = map.replacen(
        static_origin,
        &format!("{static_origin}\n\"style\" \"2\""),
        1,
    );
    let input = scratch.path().join("animated-only.map");
    std::fs::write(&input, map).unwrap();
    let (seed, _) = build(&input, scratch.path(), "animated-seed", None, None);
    let log = assert_success(&seed);
    let sh = admitted_bytes(&log, SH_ADMITTED);
    assert_eq!(admitted_bytes(&log, LIGHTMAP_ADMITTED), 0);
    let (limited, path) = build(&input, scratch.path(), "animated-limited", None, Some(sh));
    let log = assert_success(&limited);
    assert_eq!(admitted_bytes(&log, LIGHTMAP_ADMITTED), 0);
    assert!(
        log.contains("no static lightmap placements"),
        "fixture must exercise the animated branch: {log}"
    );
    for line in log.lines().filter(|line| line.contains("WARN")) {
        assert!(
            !line.contains("cell blocks")
                && !line.contains("runtime")
                && !line.contains("all-resident"),
            "misleading animated refusal warning: {line}"
        );
    }
    let mut file = Cursor::new(std::fs::read(path).unwrap());
    let meta = read_container(&mut file).unwrap();
    let lightmap = read_section_data(&mut file, &meta, SectionId::Lightmap as u32)
        .unwrap()
        .unwrap();
    assert!(
        LightmapSection::from_bytes(&lightmap)
            .unwrap()
            .blocks
            .is_empty()
    );
    assert!(
        meta.find_section(SectionId::AnimatedLightChunks as u32)
            .is_none()
    );
    assert!(
        meta.find_section(SectionId::AnimatedLightWeightMaps as u32)
            .is_none()
    );
}
