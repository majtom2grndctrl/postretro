use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_tempdir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "postretro-script-compiler-cli-{label}-{nanos}-{n}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create tempdir");
    dir
}

#[test]
fn dep_json_stdout_is_single_dependency_report() {
    let dir = unique_tempdir("dep-json");
    let entry = dir.join("entry.ts");
    let dep = dir.join("dep.ts");
    let output = dir.join("entry.js");

    fs::write(&dep, "export const value: number = 42;\n").unwrap();
    fs::write(
        &entry,
        r#"
        import { value } from "./dep";
        const doubled = value * 2;
        "#,
    )
    .unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_scripts-build"))
        .arg("--in")
        .arg(&entry)
        .arg("--out")
        .arg(&output)
        .arg("--dep-json")
        .output()
        .expect("run scripts-build");

    assert!(
        result.status.success(),
        "scripts-build failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).trim().is_empty(),
        "success should not emit human diagnostics to stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    let stdout = String::from_utf8(result.stdout).expect("stdout is utf-8");
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should be exactly one JSON object");
    assert!(
        report.is_object(),
        "dep-json stdout must be an object: {stdout}"
    );
    assert_eq!(stdout.trim(), serde_json::to_string(&report).unwrap());

    assert_eq!(
        report["entry"],
        fs::canonicalize(&entry).unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(
        report["output"],
        fs::canonicalize(&output)
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );

    let expected_deps = vec![
        fs::canonicalize(&dep)
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        fs::canonicalize(&entry)
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    ];
    assert_eq!(
        report["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect::<Vec<_>>(),
        expected_deps
    );
    assert!(output.is_file(), "bundled output should still be written");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn dev_start_script_bundles_sdk_authored_hud_without_generated_sibling() {
    let dir = unique_tempdir("dev-hud");
    let output = dir.join("start-script.js");
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let entry = workspace_root.join("content/dev/start-script.ts");
    let generated_sibling = workspace_root.join("content/dev/start-script.js");
    let sibling_before = fs::metadata(&generated_sibling).ok().and_then(|metadata| {
        metadata
            .modified()
            .ok()
            .map(|modified| (metadata.len(), modified))
    });

    let result = Command::new(env!("CARGO_BIN_EXE_scripts-build"))
        .arg("--in")
        .arg(&entry)
        .arg("--out")
        .arg(&output)
        .output()
        .expect("run scripts-build");

    assert!(
        result.status.success(),
        "scripts-build failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );

    let bundled = fs::read_to_string(&output).expect("bundled output exists");
    assert!(
        bundled.contains("getGameState"),
        "HUD builder obtains engine refs through getGameState: {bundled}",
    );
    assert!(
        bundled.contains("hud.reticle"),
        "reticle registration is bundled from hud.ts: {bundled}",
    );
    for removed in ["postretro/game-state", "player.ammo", "intro.flashColor"] {
        assert!(
            !bundled.contains(removed),
            "dev HUD bundle must not reference removed surface {removed:?}: {bundled}",
        );
    }
    let sibling_after = fs::metadata(&generated_sibling).ok().and_then(|metadata| {
        metadata
            .modified()
            .ok()
            .map(|modified| (metadata.len(), modified))
    });
    assert_eq!(
        sibling_after, sibling_before,
        "compiler fixture writes only to the temp output, not content/dev/start-script.js",
    );

    let _ = fs::remove_dir_all(&dir);
}
