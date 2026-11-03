//! AC1's `assistable` expression rejections in both runtimes, through both
//! commit gates: first load (`run_mod_init`) and reload
//! (`commit_staged_manifest_result`).
//! See: context/lib/scripting.md §11 (interaction-pair scope)

use std::sync::atomic::{AtomicU64, Ordering};

use postretro_entities::ScriptCtx;
use serde_json::{Value, json};

use crate::primitives_registry::PrimitiveRegistry;
use crate::runtime::{ScriptRuntime, ScriptRuntimeConfig, StagedManifestCommitOutcome};
use crate::sequence::SequencedPrimitiveRegistry;
use crate::staged_manifest::{
    StagedManifestBuildConfig, StagedManifestBuildLane, StagedManifestBuildStatus,
    build_staged_manifest,
};

fn input(name: &str, owner: Option<&str>) -> Value {
    match owner {
        Some(owner) => json!({ "op": "input", "name": name, "owner": owner }),
        None => json!({ "op": "input", "name": name }),
    }
}

/// Each refused expression, the field it sits in, and its diagnostic.
fn rejected_rows() -> Vec<(&'static str, Value, &'static str)> {
    let deep = (0..40).fold(
        json!({ "op": "const", "value": 1 }),
        |node, _| json!({ "op": "add", "a": node, "b": { "op": "const", "value": 1 } }),
    );
    vec![
        (
            "radius",
            input("stats.heal", None),
            "byPlayer(assist.helper)",
        ),
        (
            "radius",
            input("player.health", None),
            "byPlayer(assist.target)",
        ),
        (
            "holdMs",
            input("stats.heal", Some("@player")),
            "does not know",
        ),
        ("radius", input("@state.marked", None), "per-entity state"),
        ("holdMs", input("@pair.distance", None), "reserved"),
        (
            "radius",
            input("player.spread", None),
            "each machine publishes",
        ),
        (
            "holdMs",
            input("player.assistProgress", Some("@assist.helper")),
            "depends on",
        ),
        (
            "radius",
            input("nope.value", None),
            "unknown input `nope.value`",
        ),
        (
            "holdMs",
            json!({ "op": "lt", "a": { "op": "const", "value": 1 }, "b": { "op": "const", "value": 2 } }),
            "must produce a number",
        ),
        (
            "radius",
            json!({
                "op": "select",
                "cond": input("player.inLifeState:gone", Some("@assist.target")),
                "a": { "op": "const", "value": 3 },
                "b": { "op": "const", "value": 1.5 },
            }),
            "life state `gone`",
        ),
        ("holdMs", deep, "depth 32"),
    ]
}

fn accepted_expression() -> Value {
    json!({
        "op": "add",
        "a": { "op": "mul", "a": input("stats.heal", Some("@assist.helper")), "b": { "op": "const", "value": -20 } },
        "b": { "op": "const", "value": 3000 },
    })
}

fn manifest(field: &str, expression: Value) -> Value {
    let mut assistable = json!({});
    assistable[field] = expression;
    json!({
        "name": "Assist", "id": "assist-validation", "version": "1",
        "stores": [{
            "namespace": "stats",
            "schema": { "heal": { "type": "number", "default": 0, "perOwner": true, "network": "ownerPrivate" } },
        }],
        "entities": [{
            "canonicalName": "player",
            "components": {
                "movement": {
                    "capsule": { "radius": 0.4, "halfHeight": 0.8, "eyeHeight": 0.5 },
                    "ground": { "speed": { "walk": 7, "run": 11, "crouch": 3 }, "accel": 10, "stepHeight": 0.3, "maxSlope": 45 },
                    "air": { "forwardSteer": 0, "accel": 0.7, "maxControlSpeed": 0.5, "bunnyHop": false, "jumps": 0, "jumpVelocity": 5.5, "jumpCeiling": 0 },
                    "fall": { "terminalVelocity": 40 },
                },
                "lifeStates": {
                    "initial": "alive",
                    "states": { "alive": {}, "down": { "assistable": assistable } },
                },
            },
        }],
    })
}

/// Render JSON as a Luau table constructor.
fn luau(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let fields: Vec<String> = map
                .iter()
                .map(|(key, value)| format!("[{key:?}] = {}", luau(value)))
                .collect();
            format!("{{ {} }}", fields.join(", "))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(luau).collect();
            format!("{{ {} }}", items.join(", "))
        }
        other => other.to_string(),
    }
}

fn start_script(manifest: &Value, lua: bool) -> (&'static str, String) {
    if lua {
        ("start-script.luau", format!("return {}", luau(manifest)))
    } else {
        (
            "start-script.js",
            format!("globalThis.__postretroModManifest = {manifest};"),
        )
    }
}

struct ModRoot(std::path::PathBuf);

impl ModRoot {
    fn new(manifest: &Value, lua: bool) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "postretro-assist-validation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (file, source) = start_script(manifest, lua);
        std::fs::write(root.join(file), source).unwrap();
        Self(root)
    }
}

impl Drop for ModRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn runtime(ctx: &ScriptCtx) -> ScriptRuntime {
    ScriptRuntime::new(
        &PrimitiveRegistry::new(),
        &ScriptRuntimeConfig {
            skip_identity_enforcement: true,
            ..Default::default()
        },
        ctx,
    )
    .unwrap()
}

fn assert_rejection(context: &str, error: &str, field: &str, reason: &str) {
    assert!(
        error.contains(&format!(
            "components.lifeStates.states.down.assistable.{field}"
        )),
        "{context}: {error}"
    );
    assert!(error.contains(reason), "{context}: {error}");
}

#[test]
fn assistable_expression_rows_reject_first_load_through_run_mod_init_in_both_runtimes() {
    for lua in [false, true] {
        let ctx = ScriptCtx::new();
        let mut runtime = runtime(&ctx);
        for (field, expression, reason) in rejected_rows() {
            let root = ModRoot::new(&manifest(field, expression), lua);
            let error = runtime
                .run_mod_init(&root.0)
                .expect_err("a refused pair read fails mod-init")
                .to_string();
            assert_rejection(&format!("lua={lua}"), &error, field, reason);
            assert!(
                ctx.slot_table.borrow().get("stats.heal").is_none(),
                "a refused load leaves the live store unchanged"
            );
        }
        let root = ModRoot::new(&manifest("holdMs", accepted_expression()), lua);
        runtime
            .run_mod_init(&root.0)
            .unwrap_or_else(|error| panic!("lua={lua}: {error}"));
    }
}

#[test]
fn assistable_expression_rows_reject_reload_through_commit_staged_manifest_result_in_both_runtimes()
{
    for lua in [false, true] {
        let ctx = ScriptCtx::new();
        let mut runtime = runtime(&ctx);
        let mut generation = 0;
        let mut commit = |runtime: &mut ScriptRuntime, manifest: &Value| {
            generation += 1;
            let root = ModRoot::new(manifest, lua);
            let result =
                build_staged_manifest(&root.0, generation, &StagedManifestBuildConfig::default());
            runtime.staged_manifest_lane =
                Some(StagedManifestBuildLane::new_for_test_latest(generation));
            (
                matches!(result.status, StagedManifestBuildStatus::Built(_)),
                result.diagnostics.clone(),
                runtime.commit_staged_manifest_result(
                    &result,
                    &ctx,
                    &SequencedPrimitiveRegistry::new(),
                ),
            )
        };
        for (field, expression, reason) in rejected_rows() {
            let (built, diagnostics, outcome) = commit(&mut runtime, &manifest(field, expression));
            assert_eq!(
                built,
                reason != "depth 32",
                "lua={lua} `{reason}`: only the over-deep row stops at parse: {diagnostics:?}"
            );
            if built {
                let StagedManifestCommitOutcome::Rejected { reason: error, .. } = outcome else {
                    panic!("lua={lua} `{field}`: a refused pair read must reject: {outcome:?}");
                };
                assert_rejection(&format!("lua={lua}"), &error, field, reason);
            } else {
                // The raw adapters already refuse an over-deep expression at
                // parse, at the same key path.
                let error = diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                assert_rejection(&format!("lua={lua} (parse)"), &error, field, reason);
            }
            assert!(ctx.slot_table.borrow().get("stats.heal").is_none());
        }
        let (_, diagnostics, outcome) =
            commit(&mut runtime, &manifest("holdMs", accepted_expression()));
        assert!(
            matches!(outcome, StagedManifestCommitOutcome::Committed { .. }),
            "lua={lua}: {outcome:?} {diagnostics:?}"
        );
    }
}
