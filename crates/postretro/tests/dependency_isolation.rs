// Dependency-tree guards for CPU stage timing: the shared timing crate stays a
// leaf, and Tracy never reaches a shipped or dependency-free diagnostic build.
// See: context/lib/rendering_pipeline.md §12 · context/lib/development_guide.md §6.4

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("postretro crate must be two levels below the workspace root")
        .to_path_buf()
}

/// Package names in `package`'s normal-edge dependency tree for `features`.
fn normal_dependency_names(package: &str, features: &[&str]) -> Vec<String> {
    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    command
        .args(["tree", "-p", package, "-e", "normal", "--prefix", "none"])
        .args(["--format", "{p}"])
        .current_dir(workspace_root());
    if !features.is_empty() {
        command.arg("--features").arg(features.join(","));
    }
    let output = command.output().expect("run cargo tree");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

#[test]
fn tracy_is_absent_from_shipped_and_dependency_free_feature_sets() {
    // Default = the release payload; dev-tools = the SDK's authoring engine;
    // observability + observe-live + capture = the dependency-free diagnostics.
    for features in [
        &[][..],
        &["dev-tools"][..],
        &["observability", "observe-live", "capture"][..],
    ] {
        let names = normal_dependency_names("postretro", features);
        assert!(names.iter().any(|name| name == "postretro-stage-timing"));
        let tracy: Vec<_> = names
            .iter()
            .filter(|name| name.starts_with("tracy"))
            .collect();
        assert!(tracy.is_empty(), "{features:?} pulls in {tracy:?}");
    }
}

#[test]
fn tracy_feature_is_the_only_way_tracy_enters() {
    let names = normal_dependency_names("postretro", &["tracy"]);
    assert!(names.iter().any(|name| name == "tracy-client"));
}

#[test]
fn shared_timing_crate_depends_on_no_engine_crate() {
    for features in [&[][..], &["tracy"][..]] {
        let names = normal_dependency_names("postretro-stage-timing", features);
        let engine: Vec<_> = names
            .iter()
            .filter(|name| name.starts_with("postretro") && *name != "postretro-stage-timing")
            .collect();
        assert!(engine.is_empty(), "{features:?}: {engine:?}");
    }
}
