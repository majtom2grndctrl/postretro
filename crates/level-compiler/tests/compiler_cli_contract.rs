// Compiler subprocess contracts for reporter selection, plain output, and deterministic bakes.
// See: context/lib/build_pipeline.md §Progress reporting, controls, and logging

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::{SectionId, read_container, read_section_data};

const SUMMARY_LABELS: &[&str] = &[
    "Parsing",
    "DataScript",
    "TexValidation",
    "Partitioning",
    "Visibility",
    "Geometry",
    "BVH Build",
    "Cell Visibility",
    "NavMesh",
    "SH Bake",
    "Delta SH Bake",
    "Direct SH Bake",
    "Animated Direct SH Bake",
    "EntityShadowLights",
    "Direct SH Delta Bake",
    "Billboard Direct Scatter Bake",
    "ChunkLightList",
    "Atlas Preparation",
    "Lightmap Bake",
    "ShadowmaskAtlas",
    "AnimLightChunks",
    "AnimWeightMaps",
    "TextureMips",
    "Packing",
    "Total",
];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("level-compiler crate must be two levels below the workspace root")
        .to_path_buf()
}

struct TempBuildDir(PathBuf);

impl TempBuildDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "postretro-level-compiler-cli-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("create isolated compiler output directory");
        Self(path)
    }
}

impl Drop for TempBuildDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn compile_fixture(input: &Path, output: &Path, jobs: usize) -> Output {
    compile_fixture_with_irradiance_format(input, output, jobs, true, None)
}

fn compile_fixture_with_irradiance_format(
    input: &Path,
    output: &Path,
    jobs: usize,
    uncompressed_irradiance: bool,
    forced_scale: Option<u8>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_prl-build"));
    command
        .arg(input)
        .arg("-o")
        .arg(output)
        .arg("--no-cache")
        .arg("--no-tui")
        .arg("-j")
        .arg(jobs.to_string());
    if uncompressed_irradiance {
        command.arg("--uncompressed-irradiance");
    }
    if let Some(scale) = forced_scale {
        command
            .arg("--sh-density-force-scale")
            .arg(scale.to_string());
    }
    command.output().expect("spawn prl-build")
}

fn compile_fixture_for_layer_cache_order(
    input: &Path,
    output: &Path,
    cache_dir: Option<&Path>,
    uncompressed_irradiance: bool,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_prl-build"));
    command
        .env("RUST_LOG", "info")
        .arg(input)
        .arg("-o")
        .arg(output)
        .arg("--no-tui")
        .arg("--verbose")
        .arg("--sh-probe-spacing")
        .arg("4")
        .arg("--lightmap-density")
        .arg("0.25")
        .arg("-j")
        .arg("1");
    if uncompressed_irradiance {
        command.arg("--uncompressed-irradiance");
    }
    if let Some(cache_dir) = cache_dir {
        command.arg("--cache-dir").arg(cache_dir);
    } else {
        command.arg("--no-cache");
    }
    command.output().expect("spawn prl-build")
}

#[test]
fn sh_analysis_is_byte_preserving_for_compiled_prl() {
    let workspace = workspace_root();
    let input = workspace.join("content/dev/maps/specular-shadowmask-capture.map");
    assert!(input.is_file(), "fixture map missing: {}", input.display());

    let temp = TempBuildDir::new();
    let baseline = temp.0.join("baseline.prl");
    let analyzed = temp.0.join("analyzed.prl");
    let analysis_json = temp.0.join("analysis.json");
    let common = |output: &Path| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_prl-build"));
        command
            .arg(&input)
            .arg("-o")
            .arg(output)
            .arg("--no-cache")
            .arg("--no-tui")
            .arg("--uncompressed-irradiance")
            .arg("--sh-probe-spacing")
            .arg("4")
            .arg("--lightmap-density")
            .arg("0.25")
            .arg("-j")
            .arg("1");
        command
    };

    let baseline_build = common(&baseline)
        .output()
        .expect("spawn baseline prl-build");
    assert_success(&baseline_build, 1);
    let analyzed_build = common(&analyzed)
        .arg("--sh-analyze")
        .arg("--sh-analyze-out")
        .arg(&analysis_json)
        .arg("--sh-density-force-scale")
        .arg("3")
        .output()
        .expect("spawn analyzed prl-build");
    assert_success(&analyzed_build, 1);
    assert!(analysis_json.is_file(), "analysis JSON was not written");
    assert_eq!(
        std::fs::read(&baseline).expect("read baseline PRL"),
        std::fs::read(&analyzed).expect("read analyzed PRL"),
        "--sh-analyze and its force-scale measurement must not change emitted bytes",
    );
}

#[test]
fn forced_hierarchy_output_round_trips_through_production_loader() {
    let workspace = workspace_root();
    let source = workspace.join("content/dev/maps/specular-shadowmask-capture.map");
    assert!(
        source.is_file(),
        "fixture map missing: {}",
        source.display()
    );

    let temp = TempBuildDir::new();
    let input = temp.0.join("forced-hierarchy.map");
    let map = std::fs::read_to_string(&source)
        .expect("read hierarchy source fixture")
        .replacen(
            "\"classname\" \"light\"",
            "\"classname\" \"light_dynamic\"",
            1,
        );
    std::fs::write(&input, map).expect("write hierarchy fixture without static delta lights");
    let output = temp.0.join("forced-hierarchy.prl");
    let build = Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .env("RUST_LOG", "info")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .arg("--no-cache")
        .arg("--no-tui")
        .arg("--verbose")
        .arg("--uncompressed-irradiance")
        .arg("--sh-probe-spacing")
        .arg("1")
        .arg("--lightmap-density")
        .arg("0.25")
        .arg("--sh-density-force-level")
        .arg("1")
        .arg("--sh-density-force-scale")
        .arg("1")
        .arg("-j")
        .arg("1")
        .output()
        .expect("spawn forced hierarchy prl-build");
    assert_success(&build, 1);

    let section = read_sh_volume(&output);
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        section.probes.iter().any(|probe| probe.node_scale == 1),
        "forced hierarchy fixture must emit at least one scale-1 node:\n{stderr}"
    );
    postretro_level_loader::load_prl(output.to_str().expect("UTF-8 fixture path"))
        .expect("production loader must accept forced hierarchy output");
}

fn read_sh_volume(output: &Path) -> OctahedralShVolumeSection {
    let bytes = std::fs::read(output).expect("read compiled PRL");
    let mut cursor = Cursor::new(bytes);
    let metadata = read_container(&mut cursor).expect("read PRL container");
    let section = read_section_data(&mut cursor, &metadata, SectionId::OctahedralShVolume as u32)
        .expect("read OctahedralShVolume section")
        .expect("OctahedralShVolume section must be present");
    OctahedralShVolumeSection::from_bytes(&section).expect("parse v11 OctahedralShVolume")
}

fn run_compiler(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .args(args)
        .output()
        .expect("spawn prl-build")
}

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

fn assert_success(output: &Output, jobs: usize) {
    assert!(
        output.status.success(),
        "prl-build -j {jobs} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn assert_plain_bytes(stream_name: &str, bytes: &[u8]) {
    assert!(
        !bytes.contains(&0x1b),
        "{stream_name} contains an ESC byte: {:?}",
        String::from_utf8_lossy(bytes),
    );
    assert!(
        bytes.iter().enumerate().all(|(index, byte)| {
            !byte.is_ascii_control()
                || matches!(byte, b'\n' | b'\t')
                || (*byte == b'\r' && bytes.get(index + 1) == Some(&b'\n'))
        }),
        "{stream_name} contains a terminal control byte: {:?}",
        String::from_utf8_lossy(bytes),
    );
}

#[test]
fn captured_streams_auto_select_plain_reporter_before_fast_pipeline_failure() {
    let workspace = workspace_root();
    let fixture =
        std::fs::read_to_string(workspace.join("content/dev/maps/wedge-shared-plane.map"))
            .expect("read tiny map fixture");
    let fixture = fixture.replace(
        "\"initialGravity\" \"-9.81\"",
        "\"initialGravity\" \"-9.81\"\n\"data_script\" \"missing.luau\"",
    );
    assert!(
        fixture.contains("\"data_script\" \"missing.luau\""),
        "fast-failure fixture must carry its missing data-script precheck",
    );
    let temp = TempBuildDir::new();
    let input = temp.0.join("missing-data-script.map");
    let output_path = temp.0.join("unused.prl");
    std::fs::write(&input, fixture).expect("write fast-failure map fixture");

    let output = run_compiler(&[
        input.to_str().expect("temporary input path must be UTF-8"),
        "-o",
        output_path
            .to_str()
            .expect("temporary output path must be UTF-8"),
    ]);

    assert!(!output.status.success(), "missing data script must fail");
    assert_plain_bytes("auto stdout", &output.stdout);
    assert_plain_bytes("auto stderr", &output.stderr);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Parsing map...") && stderr.contains("Data script compilation..."),
        "captured streams must reach main's Auto TTY seam and select the line-oriented reporter:\n{stderr}",
    );
    assert!(
        stderr.contains("data_script = missing.luau") && stderr.contains("does not exist"),
        "fixture must fail at the intended cheap post-selection precheck:\n{stderr}",
    );
}

#[test]
fn captured_streams_reject_forced_tui_without_terminal_controls() {
    let output = run_compiler(&["unused.map", "--tui"]);

    assert!(
        !output.status.success(),
        "--tui on captured streams must fail"
    );
    assert_plain_bytes("forced TUI stdout", &output.stdout);
    assert_plain_bytes("forced TUI stderr", &output.stderr);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--tui requires stdin, stdout, and stderr to all be attached to terminals"),
        "forced TUI failure must explain the terminal requirement:\n{stderr}",
    );
}

// Regression: successful plain builds finalized their warning tally before
// reporting an over-budget cache live set.
#[test]
fn successful_plain_cache_budget_warning_precedes_exact_final_tally() {
    let workspace = workspace_root();
    let input = workspace.join("content/dev/maps/wedge-shared-plane.map");
    let temp = TempBuildDir::new();
    let cache_dir = temp.0.join("cache");

    let run = |name: &str, cache_budget: &str, build_mode: Option<&str>| {
        let output_path = temp.0.join(format!("{name}.prl"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_prl-build"));
        command
            // Isolate the cache-reporting contract from fixture diagnostics
            // such as the warm-SH approximation and missing-light warnings.
            .env("RUST_LOG", "off,prl_build::cache=warn")
            .arg(&input)
            .arg("-o")
            .arg(&output_path)
            .arg("--no-tui")
            .arg("--cache-dir")
            .arg(&cache_dir)
            .arg("--cache-max-size")
            .arg(cache_budget)
            .arg("-j")
            .arg("1");
        if let Some(build_mode) = build_mode {
            command.arg(build_mode);
        }
        command.output().expect("spawn cache-reporting prl-build")
    };

    let over_budget = run("over-budget", "1", None);
    assert_success(&over_budget, 1);
    let over_stdout = String::from_utf8_lossy(&over_budget.stdout);
    let over_stderr = String::from_utf8_lossy(&over_budget.stderr);
    assert_eq!(warning_count(&over_stdout), 1);
    assert_eq!(
        count_occurrences(&over_stdout, "[cache] build read/wrote"),
        1,
        "the final warning history must contain one cache-budget record:\n{over_stdout}"
    );
    assert_eq!(
        count_occurrences(&over_stderr, "[cache] build read/wrote"),
        1,
        "the live plain stream must emit the cache-budget record once:\n{over_stderr}"
    );

    for (name, budget, mode) in [
        ("under-budget", "2GiB", None),
        ("no-cache", "1", Some("--no-cache")),
        ("release", "1", Some("--release")),
    ] {
        let output = run(name, budget, mode);
        assert_success(&output, 1);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            warning_count(&stdout),
            0,
            "{name} must finish with a silent warning tally:\n{stdout}"
        );
        assert_eq!(
            count_occurrences(&stdout, "[cache] build read/wrote"),
            0,
            "{name} final warning history must omit the cache-budget warning:\n{stdout}"
        );
        assert_eq!(
            count_occurrences(&stderr, "[cache] build read/wrote"),
            0,
            "{name} live stream must omit the cache-budget warning:\n{stderr}"
        );
    }
}

fn summary_labels(stdout: &str) -> Vec<&str> {
    let mut lines = stdout
        .lines()
        .skip_while(|line| *line != "Build Summary:")
        .skip(1);
    let mut labels = Vec::new();

    for line in &mut lines {
        if !line.starts_with("  ") {
            break;
        }
        let duration = line
            .split_ascii_whitespace()
            .next_back()
            .expect("summary row must contain a duration");
        let duration_start = line
            .rfind(duration)
            .expect("duration token must occur in its summary row");
        let label = line[2..duration_start].trim_end();

        let seconds = duration
            .strip_suffix('s')
            .expect("summary duration must end in 's'");
        let (_, fractional) = seconds
            .split_once('.')
            .expect("summary duration must contain a decimal point");
        assert_eq!(
            fractional.len(),
            2,
            "summary duration must retain two decimal places: {line:?}",
        );
        let seconds: f32 = seconds.parse().expect("summary duration must be numeric");
        assert_eq!(
            line,
            format!("  {label:<15} {seconds:>6.2}s"),
            "Build Summary row formatting drifted",
        );
        labels.push(label);
    }

    labels
}

fn warning_count(stdout: &str) -> usize {
    let summary_offset = stdout
        .find("Build Summary:")
        .expect("successful compiler output must contain a Build Summary");
    let warning_offset = stdout
        .rfind("Warnings: ")
        .expect("plain compiler output must end with a warning tally section");
    assert!(
        warning_offset > summary_offset,
        "warning tally must follow the successful Build Summary",
    );
    stdout[warning_offset..]
        .lines()
        .find_map(|line| line.strip_prefix("Warnings: "))
        .expect("warning tally section must begin with its count")
        .parse()
        .expect("warning tally must be an integer")
}

/// Real-map projection gate for the default-spacing warren. A zero budget
/// intentionally refuses after the production CSR plan but before base SH,
/// making this practical to run whenever animated reach policy changes.
#[test]
#[ignore = "real warren CSR projection; run on demand with -- --ignored"]
fn warren_zero_budget_projects_current_membership_before_base_sh_bake() {
    let workspace = workspace_root();
    let input = workspace.join("content/dev/maps/stress-warren-hallway-inspection.map");
    assert!(input.is_file(), "fixture map missing: {}", input.display());

    let temp = TempBuildDir::new();
    let output_path = temp.0.join("must-not-be-written.prl");
    let output = Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .env("RUST_LOG", "info")
        .arg(&input)
        .arg("-o")
        .arg(&output_path)
        .arg("--no-cache")
        .arg("--no-tui")
        .arg("--verbose")
        .arg("--sh-probe-spacing")
        .arg("1.0")
        .arg("--lightmap-density")
        .arg("0.25")
        .arg("--sh-delta-working-set-max-size")
        .arg("0")
        .output()
        .expect("spawn current-policy warren projection");
    assert!(
        !output.status.success(),
        "zero-budget projection must refuse before baking"
    );

    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    eprintln!("{diagnostic}");
    assert_eq!(
        count_occurrences(&diagnostic, "derived animated-bake reservation"),
        3,
        "the generated script targets exactly three of the six animated warren lights:\n{diagnostic}",
    );
    let dense_bytes = |section: &str| {
        let marker = format!("{section} dense ");
        assert_eq!(
            count_occurrences(&diagnostic, &marker),
            1,
            "projection must report {section} exactly once:\n{diagnostic}",
        );
        diagnostic
            .split_once(&marker)
            .and_then(|(_, tail)| tail.split_once(" bytes"))
            .and_then(|(bytes, _)| bytes.parse::<u64>().ok())
            .unwrap_or_else(|| {
                panic!("projection must report dense bytes after `{marker}`:\n{diagnostic}")
            })
    };
    assert_eq!(dense_bytes("DeltaShVolumes (id 27)"), 363_184_128);
    assert_eq!(dense_bytes("DirectShDeltaVolumes (id 41)"), 1_616_615_424);
    assert_eq!(
        dense_bytes("AnimatedDirectShDeltaVolumes (id 45)"),
        202_033_152
    );

    let refusal_marker = "SH delta working-set gate refused before dense baking: estimated peak ";
    assert_eq!(
        count_occurrences(&diagnostic, refusal_marker),
        1,
        "projection must emit one pre-base-SH refusal diagnostic:\n{diagnostic}",
    );
    let refusal = diagnostic
        .split_once(refusal_marker)
        .map(|(_, refusal)| refusal)
        .expect("projection diagnostic must expose its refusal details");
    let (peak, refusal) = refusal
        .split_once(" bytes exceeds budget 0 bytes (")
        .and_then(|(peak, tail)| peak.parse::<u64>().ok().map(|peak| (peak, tail)))
        .expect("projection refusal must expose the zero-budget estimated peak");
    assert_eq!(peak, 6_545_498_112);
    let (cumulative, copy_chain_factor) = refusal
        .split_once(" cumulative dense bytes × copy-chain factor ")
        .and_then(|(cumulative, factor)| {
            cumulative.parse::<u64>().ok().zip(
                factor
                    .split_once(')')
                    .and_then(|(factor, _)| factor.parse::<u64>().ok()),
            )
        })
        .expect("projection refusal must expose cumulative dense bytes and copy-chain factor");
    assert_eq!(cumulative, 2_181_832_704);
    assert_eq!(copy_chain_factor, 3);
    assert!(
        !diagnostic.contains("SH volume bake..."),
        "zero-budget projection must refuse before the base-SH ray stage:\n{diagnostic}",
    );
    assert!(
        !output_path.exists(),
        "a refused plan-only projection must not emit a PRL",
    );
}

// Regression: throttling and non-TTY reporting were previously verified only
// by manual runs, leaving output determinism and the CLI text contract exposed.
#[test]
#[ignore = "two cold prl-build bakes; run on demand with -- --ignored"]
fn plain_cli_is_deterministic_and_preserves_progress_summary_contracts() {
    let workspace = workspace_root();
    let input = workspace.join("content/dev/maps/specular-shadowmask-capture.map");
    assert!(input.is_file(), "fixture map missing: {}", input.display());

    let temp = TempBuildDir::new();
    let serial_prl = temp.0.join("serial.prl");
    let parallel_prl = temp.0.join("parallel.prl");
    let parallel_jobs = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);

    let serial = compile_fixture(&input, &serial_prl, 1);
    assert_success(&serial, 1);
    let parallel = compile_fixture(&input, &parallel_prl, parallel_jobs);
    assert_success(&parallel, parallel_jobs);

    let serial_bytes = std::fs::read(&serial_prl).expect("read serial PRL");
    let parallel_bytes = std::fs::read(&parallel_prl).expect("read parallel PRL");
    assert_eq!(
        serial_bytes, parallel_bytes,
        "-j 1 and -j {parallel_jobs} must produce byte-identical PRLs with all other flags fixed",
    );

    let mut warning_counts = Vec::new();
    for (name, output) in [("serial", &serial), ("parallel", &parallel)] {
        assert_plain_bytes(&format!("{name} stdout"), &output.stdout);
        assert_plain_bytes(&format!("{name} stderr"), &output.stderr);

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            summary_labels(&stdout),
            SUMMARY_LABELS,
            "Build Summary row labels or order drifted for the {name} build",
        );
        warning_counts.push(warning_count(&stdout));
        assert!(
            stderr.lines().any(|line| {
                line.contains("Lightmap Bake:") && line.contains('%') && line.contains("ETA")
            }),
            "{name} non-TTY stderr must contain a discrete lightmap percent/ETA progress line:\n{stderr}",
        );
        assert!(
            stderr.lines().any(|line| {
                line.contains("ShadowmaskAtlas:") && line.contains('%') && line.contains("ETA")
            }),
            "{name} non-TTY stderr must contain live shadowmask percent/ETA progress:\n{stderr}",
        );
    }
    assert_eq!(
        warning_counts[0], warning_counts[1],
        "throttling must not change the warning tally",
    );
}

// Regression: a second layer-cache traversal could hide after the fused stage
// returned because later-stage reporting did not identify that exact boundary.
#[test]
#[ignore = "one cold and one warm full-pipeline prl-build bake; run on demand with -- --ignored"]
fn full_pipeline_closes_layer_cache_reads_at_the_fused_return_boundary() {
    let workspace = workspace_root();
    let fixture = workspace.join("content/dev/maps/specular-shadowmask-capture.map");
    assert!(
        fixture.is_file(),
        "fixture map missing: {}",
        fixture.display()
    );

    let temp = TempBuildDir::new();
    let cold =
        compile_fixture_for_layer_cache_order(&fixture, &temp.0.join("cold.prl"), None, false);
    assert_success(&cold, 1);
    let cold_stderr = String::from_utf8_lossy(&cold.stderr);
    let cold_shadow_stage = cold_stderr
        .find("Shadowmask atlas bake...")
        .expect("cold build must publish live shadowmask progress");
    let cold_fused_return = cold_stderr
        .find("[Compiler] fused lightmap/shadowmask stage returned")
        .expect("cold build must publish the exact fused-return sentinel");
    let cold_packing_stage = cold_stderr
        .find("Packing and writing...")
        .expect("cold build must reach the later packing stage");
    assert!(
        !cold_stderr.contains("[cache] lightmap_layer "),
        "the cache-disabled cold path must never attempt a layer-cache read:\n{cold_stderr}"
    );
    assert!(
        cold_shadow_stage < cold_fused_return && cold_fused_return < cold_packing_stage,
        "the cold fused-return sentinel must follow live shadowmask work and precede packing"
    );

    let cache_dir = temp.0.join("cache");
    let seeded = compile_fixture_for_layer_cache_order(
        &fixture,
        &temp.0.join("seeded.prl"),
        Some(&cache_dir),
        false,
    );
    assert_success(&seeded, 1);

    let warm = compile_fixture_for_layer_cache_order(
        &fixture,
        &temp.0.join("warm.prl"),
        Some(&cache_dir),
        true,
    );
    assert_success(&warm, 1);
    let warm_stderr = String::from_utf8_lossy(&warm.stderr);
    let shadow_stage = warm_stderr
        .find("Shadowmask atlas bake...")
        .expect("warm build must publish live shadowmask progress");
    let fused_return = warm_stderr
        .find("[Compiler] fused lightmap/shadowmask stage returned")
        .expect("warm build must publish the exact fused-return sentinel");
    let packing_stage = warm_stderr
        .find("Packing and writing...")
        .expect("warm build must reach the later packing stage");
    assert!(
        shadow_stage < fused_return && fused_return < packing_stage,
        "the warm fused-return sentinel must follow live shadowmask work and precede packing"
    );

    let layer_accesses: Vec<_> = warm_stderr
        .match_indices("[cache] lightmap_layer ")
        .map(|(offset, _)| offset)
        .collect();
    assert!(
        !layer_accesses.is_empty(),
        "the re-keyed warm section must exercise layer-cache probes:\n{warm_stderr}"
    );
    assert!(
        warm_stderr.contains("[cache] lightmap_section miss"),
        "the irradiance-format change must miss the whole lightmap memo:\n{warm_stderr}"
    );
    assert!(
        warm_stderr.contains("[cache] shadowmask_atlas hit"),
        "the irradiance-format change must leave the shadowmask memo keyed identically:\n{warm_stderr}"
    );
    assert!(
        warm_stderr.contains("[cache] lightmap_layer hit"),
        "the irradiance-format change must retain and read an unchanged layer cache entry:\n{warm_stderr}"
    );
    assert!(
        !warm_stderr.contains("[cache] lightmap_layer miss"),
        "the section-only re-key must not invalidate any layer partition:\n{warm_stderr}"
    );
    assert!(
        layer_accesses
            .iter()
            .all(|&offset| shadow_stage < offset && offset < fused_return),
        "every warm layer-cache probe must occur inside the fused stage, before its exact return sentinel:\n{warm_stderr}"
    );

    let warm_bytes = std::fs::read(temp.0.join("warm.prl")).expect("read warm pipeline output");
    let mut warm_cursor = Cursor::new(warm_bytes);
    let warm_meta = read_container(&mut warm_cursor).expect("decode warm pipeline output");
    assert!(
        warm_meta
            .find_section(SectionId::ShadowmaskAtlas as u32)
            .is_some(),
        "the warm proof fixture must exercise and pack ShadowmaskAtlas"
    );
}

/// The v11 node-aware stored base atlas must be deterministic at the compiler seam:
/// `--no-cache` selects the monolithic bake and the pipeline then chooses the
/// uncompressed debug payload or default BC6H payload. `gate-heavily-lit` keeps
/// the four cold bakes representative without making the regular test target
/// expensive.
#[test]
#[ignore = "four cold prl-build bakes on gate-heavily-lit; run on demand with -- --ignored"]
fn gate_heavily_lit_cold_compact_sh_output_is_deterministic() {
    let workspace = workspace_root();
    let input = workspace.join("content/dev/maps/gate-heavily-lit.map");
    assert!(input.is_file(), "fixture map missing: {}", input.display());

    let temp = TempBuildDir::new();
    let uncompressed_a = temp.0.join("uncompressed-a.prl");
    let uncompressed_b = temp.0.join("uncompressed-b.prl");
    let bc6h_a = temp.0.join("bc6h-a.prl");
    let bc6h_b = temp.0.join("bc6h-b.prl");

    for output in [&uncompressed_a, &uncompressed_b] {
        let build = compile_fixture_with_irradiance_format(&input, output, 1, true, Some(1));
        assert_success(&build, 1);
    }
    assert_eq!(
        std::fs::read(&uncompressed_a).expect("read first uncompressed PRL"),
        std::fs::read(&uncompressed_b).expect("read second uncompressed PRL"),
        "two uncompressed --no-cache bakes must be byte-identical",
    );
    let uncompressed_section = read_sh_volume(&uncompressed_a);
    assert_eq!(
        uncompressed_section.irradiance_format, IRRADIANCE_FORMAT_RGBA16F,
        "--uncompressed-irradiance must preserve the compact RGBA16F payload",
    );

    for output in [&bc6h_a, &bc6h_b] {
        let build = compile_fixture_with_irradiance_format(&input, output, 1, false, Some(1));
        assert_success(&build, 1);
    }
    let first_bc6h = read_sh_volume(&bc6h_a);
    let second_bc6h = read_sh_volume(&bc6h_b);
    assert_eq!(first_bc6h.irradiance_format, IRRADIANCE_FORMAT_BC6H);
    assert_eq!(second_bc6h.irradiance_format, IRRADIANCE_FORMAT_BC6H);
    assert_eq!(
        first_bc6h.compact_atlas.len(),
        second_bc6h.compact_atlas.len(),
        "lossy BC6H output is gated on stable compact-section length, not byte identity",
    );
}
