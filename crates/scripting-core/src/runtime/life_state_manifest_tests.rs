//! The 255-name limit must reject both startup and atomic staged manifests.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{run_mod_init_luau, run_mod_init_quickjs};
use crate::primitives_registry::PrimitiveRegistry;
use crate::quickjs::{QuickJsConfig, QuickJsSubsystem};
use crate::staged_manifest::{
    StagedManifestBuildConfig, StagedManifestBuildStatus, build_staged_manifest,
};

const MOVEMENT: &str = r#"{
    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
    ground: { speed: { walk: 7, run: 11, crouch: 3 }, accel: 10, stepHeight: 0.3, maxSlope: 45 },
    air: { forwardSteer: 0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0 },
    fall: { terminalVelocity: 40 }
}"#;

fn source(count: usize, lua: bool) -> String {
    let mut entities = Vec::new();
    for (entity, range) in [("first", 0..128), ("second", 128..count)] {
        let first = range.start;
        let states = range
            .map(|index| format!("state{index}: {{}}"))
            .collect::<Vec<_>>()
            .join(",");
        entities.push(
            [
                "{canonicalName: \"",
                entity,
                "\", components: {movement: ",
                MOVEMENT,
                ", lifeStates: {initial: \"state",
                &first.to_string(),
                "\", states: {",
                &states,
                "}}}}",
            ]
            .concat(),
        );
    }
    let fields = format!(
        r#"name: "LifeStates", id: "life-states", version: "1", entities: [{}]"#,
        entities.join(",")
    );
    if lua {
        // All keys above are identifiers and all values are scalar/object data.
        format!(
            "return {{{}}}",
            fields.replace(':', "=").replace('[', "{").replace(']', "}")
        )
    } else {
        format!("globalThis.__postretroModManifest = {{{fields}}};")
    }
}

#[test]
fn startup_manifest_twins_enforce_the_global_life_state_name_limit() {
    for count in [255, 256] {
        let quickjs =
            QuickJsSubsystem::new(&PrimitiveRegistry::new(), &QuickJsConfig::default()).unwrap();
        let js = run_mod_init_quickjs(&quickjs, &source(count, false), "life-states.js");
        let lua = run_mod_init_luau(
            &[],
            &source(count, true),
            "life-states.luau",
            Path::new("."),
        );
        if count == 255 {
            assert_eq!(js.unwrap().entities, lua.unwrap().entities);
        } else {
            for error in [js.unwrap_err(), lua.unwrap_err()] {
                assert!(
                    error.to_string().contains("255 distinct life-state names"),
                    "{error}"
                );
                assert!(
                    error.to_string().contains("components.lifeStates.states"),
                    "{error}"
                );
            }
        }
    }
}

#[test]
fn staged_manifest_twins_enforce_the_global_life_state_name_limit() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    for count in [255, 256] {
        for lua in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "postretro-life-state-limit-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(
                root.join(if lua {
                    "start-script.luau"
                } else {
                    "start-script.js"
                }),
                source(count, lua),
            )
            .unwrap();
            let result = build_staged_manifest(&root, 1, &StagedManifestBuildConfig::default());
            std::fs::remove_dir_all(root).unwrap();
            if count == 255 {
                assert!(
                    matches!(result.status, StagedManifestBuildStatus::Built(_)),
                    "{:?}",
                    result.diagnostics
                );
            } else {
                assert!(matches!(result.status, StagedManifestBuildStatus::Failed));
                assert!(
                    result.diagnostics.iter().any(|diagnostic| diagnostic
                        .message
                        .contains("255 distinct life-state names")),
                    "{:?}",
                    result.diagnostics
                );
            }
        }
    }
}
