//! End-to-end regression coverage for the compiler's plain CLI contract.
//!
//! This suite shells out to the already-built `prl-build` test binary. The
//! cold bake is intentionally ignored because it exercises the complete
//! lightmap and SH pipelines twice; run it on demand with `-- --ignored`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

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
    Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .arg(input)
        .arg("-o")
        .arg(output)
        .arg("--no-cache")
        .arg("--no-tui")
        .arg("--uncompressed-irradiance")
        .arg("-j")
        .arg(jobs.to_string())
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
        bytes
            .iter()
            .all(|byte| !byte.is_ascii_control() || matches!(byte, b'\n' | b'\r' | b'\t')),
        "{stream_name} contains a terminal control byte: {:?}",
        String::from_utf8_lossy(bytes),
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
