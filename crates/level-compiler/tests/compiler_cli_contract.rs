// Compiler subprocess contracts for reporter selection, plain output, and deterministic bakes.
// See: context/plans/in-progress/level-compiler-tui/index.md

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
    "NavMesh",
    "Lightmap Bake",
    "SH Bake",
    "Delta SH Bake",
    "Direct SH Bake",
    "EntityShadowLights",
    "Direct SH Delta Bake",
    "ShadowmaskAtlas",
    "ChunkLightList",
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
    compile_fixture_with_irradiance_format(input, output, jobs, true)
}

fn compile_fixture_with_irradiance_format(
    input: &Path,
    output: &Path,
    jobs: usize,
    uncompressed_irradiance: bool,
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
    command.output().expect("spawn prl-build")
}

fn read_sh_volume(output: &Path) -> OctahedralShVolumeSection {
    let bytes = std::fs::read(output).expect("read compiled PRL");
    let mut cursor = Cursor::new(bytes);
    let metadata = read_container(&mut cursor).expect("read PRL container");
    let section = read_section_data(&mut cursor, &metadata, SectionId::OctahedralShVolume as u32)
        .expect("read OctahedralShVolume section")
        .expect("OctahedralShVolume section must be present");
    OctahedralShVolumeSection::from_bytes(&section).expect("parse v10 OctahedralShVolume")
}

fn run_compiler(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .args(args)
        .output()
        .expect("spawn prl-build")
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

// Regression: throttling and non-TTY reporting were previously verified only
// by manual runs, leaving output determinism and the CLI text contract exposed.
#[test]
#[ignore = "two cold prl-build bakes; run on demand with -- --ignored"]
fn plain_cli_is_deterministic_and_preserves_progress_summary_contracts() {
    let workspace = workspace_root();
    let input = workspace.join("content/dev/maps/test_animated_weight_maps_single.map");
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
    }
    assert_eq!(
        warning_counts[0], warning_counts[1],
        "throttling must not change the warning tally",
    );
}

/// The v10 brick-major stored base atlas must be deterministic at the compiler seam:
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
        let build = compile_fixture_with_irradiance_format(&input, output, 1, true);
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
        let build = compile_fixture_with_irradiance_format(&input, output, 1, false);
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
