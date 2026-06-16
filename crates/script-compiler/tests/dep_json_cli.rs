// CLI integration tests for script dependency reporting and dev HUD bundling.
// See: context/lib/scripting.md

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

fn write_sdk_hud_fixture(dir: &std::path::Path) -> PathBuf {
    let entry = dir.join("start-script.ts");
    fs::write(
        &entry,
        r#"
import {
  Bar,
  Text,
  Tree,
  VStack,
  bindState,
  getGameState,
} from "postretro";

function buildHud() {
  const { player } = getGameState();
  return {
    uiTrees: [
      {
        name: "hud",
        tree: Tree(
          { anchor: "bottomLeft", offset: [24.0, -24.0] },
          VStack(
            { gap: 6.0, padding: 14.0, align: "stretch", fill: "hud.panel" },
            [
              Text({
                content: "HP --",
                color: "hud.text",
                fontSize: 24.0,
                bind: bindState(player.health, { format: "HP {}" }),
              }),
              Bar({
                bind: bindState(player.health, {
                  tween: { durationMs: 180.0, easing: "easeOut" },
                }),
                max: player.maxHealth,
                fill: "ok",
                background: "hud.health.background",
                styleRanges: {
                  max: 1.0,
                  entries: [
                    { upTo: 0.25, color: "critical" },
                    { upTo: 0.5, color: "warning" },
                    { color: "ok" },
                  ],
                },
              }),
            ],
          ),
        ),
        alwaysOn: true,
      },
      {
        name: "hud.reticle",
        tree: Tree(
          { anchor: "center", offset: [0.0, 0.0] },
          Text({ content: "+", font: "mono" }),
        ),
        alwaysOn: true,
      },
    ],
    theme: {
      colors: {
        "hud.panel": [0.018, 0.026, 0.039, 0.82],
        "hud.health.background": [0.035, 0.045, 0.060, 1.0],
        "hud.text": [0.82, 0.95, 0.98, 1.0],
        critical: [0.86, 0.06, 0.12, 1.0],
        warning: [0.95, 0.62, 0.12, 1.0],
        ok: [0.12, 0.72, 0.40, 1.0],
      },
      fonts: { mono: "JetBrains Mono" },
      spacing: {},
    },
  };
}

export function setupMod() {
  const hud = buildHud();
  return {
    name: "hud-fixture",
    uiTrees: hud.uiTrees,
    theme: hud.theme,
    entities: [],
  };
}
"#,
    )
    .expect("write SDK HUD fixture");
    entry
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
fn sdk_hud_fixture_bundles_without_generated_sibling() {
    let dir = unique_tempdir("sdk-hud");
    let output = dir.join("start-script.js");
    let entry = write_sdk_hud_fixture(&dir);
    let generated_sibling = dir.join("start-script.generated.js");

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
        "reticle registration is bundled from the HUD fixture: {bundled}",
    );
    for removed in ["postretro/game-state", "player.ammo", "intro.flashColor"] {
        assert!(
            !bundled.contains(removed),
            "HUD bundle must not reference legacy surface {removed:?}: {bundled}",
        );
    }
    assert!(
        !generated_sibling.exists(),
        "compiler fixture writes only to the requested temp output",
    );

    let _ = fs::remove_dir_all(&dir);
}
