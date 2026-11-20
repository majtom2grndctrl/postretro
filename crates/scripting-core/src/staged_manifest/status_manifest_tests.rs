// Declaring and registering status kinds through the SDK in both runtimes, and
// the load failures a bad status declaration raises.
// See: context/lib/scripting.md §2 (Status declarations)

use std::fs;

use postretro_foundation::status::StatusStacking;

use super::tests::temp_mod_root;
use super::*;

const TS_STATUS_MOD: &str = r#"
    const heat = defineDamageType("fire", { health: 1.25 });
    const burning = defineStatus("burning", {
        damage: 4, type: heat, tickMs: 500, durationMs: 3000, stacking: "stack", maxStacks: 3,
        presentation: { emitter: emitter({ sprite: "ember", rate: 24, lifetime: 0.6, velocity: [0, 1.5, 0] }), tickSound: "burn_tick" },
    });
    const corroding = defineStatus("corroding", { damage: 2, tickMs: 1000 });
    const wet = defineStatus("wet", { damage: 0, tickMs: 500, durationMs: 5000 });
    globalThis.__postretroModManifest = defineMod({
        name: "Statuses", id: "statuses", version: "1",
        damageTypes: [heat],
        statuses: [burning, corroding, wet],
    });
"#;

const LUAU_STATUS_MOD: &str = r#"
    local heat = defineDamageType("fire", { health = 1.25 })
    local burning = defineStatus("burning", {
        damage = 4, type = heat, tickMs = 500, durationMs = 3000, stacking = "stack", maxStacks = 3,
        presentation = { emitter = emitter({ sprite = "ember", rate = 24, lifetime = 0.6, velocity = { 0, 1.5, 0 } }), tickSound = "burn_tick" },
    })
    local corroding = defineStatus("corroding", { damage = 2, tickMs = 1000 })
    local wet = defineStatus("wet", { damage = 0, tickMs = 500, durationMs = 5000 })
    return defineMod({
        name = "Statuses", id = "statuses", version = "1",
        damageTypes = { heat },
        statuses = { burning, corroding, wet },
    })
"#;

fn build(runtime: &str, entry: &str, source: &str) -> StagedManifestBuildResult {
    let dir = temp_mod_root(&format!("status_{runtime}"));
    fs::write(dir.join(entry), source).expect("fixture should be written");
    build_staged_manifest(&dir, 1, &StagedManifestBuildConfig::default())
}

fn built(result: StagedManifestBuildResult) -> Box<transfer::StagedManifest> {
    match result.status {
        StagedManifestBuildStatus::Built(manifest) => manifest,
        other => panic!("manifest must build: {other:?} {:?}", result.diagnostics),
    }
}

#[test]
fn ts_and_luau_register_statuses_and_produce_identical_declarations() {
    let js = built(build("js", "start-script.js", TS_STATUS_MOD));
    let luau = built(build("luau", "start-script.luau", LUAU_STATUS_MOD));
    let names: Vec<&str> = js
        .defense
        .statuses
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["burning", "corroding", "wet"]);
    assert_eq!(js.defense.statuses, luau.defense.statuses);
    let burning = &js.defense.statuses[0];
    assert_eq!(burning.damage_type, Some(0));
    assert_eq!(
        (burning.tick_ticks, burning.duration_ticks),
        (30, Some(180))
    );
    assert_eq!(
        (burning.stacking, burning.max_stacks),
        (StatusStacking::Stack, 3)
    );
    assert!(burning.presentation.emitter.is_some());
    assert_eq!(
        burning.presentation.tick_sound.as_deref(),
        Some("burn_tick")
    );
    assert_eq!(js.defense.statuses[1].duration_ticks, None);
    assert!(js.defense.statuses[2].is_marker());
}

#[test]
fn bad_status_declarations_fail_load_naming_the_key_path() {
    let cases = [
        (
            "range",
            r#"defineStatus("s", { damage: 1, tickMs: 500, durationMs: 100 })"#,
            "statuses[0].durationMs",
        ),
        (
            "key",
            r#"defineStatus("s", { damage: 1, tickMs: 500, chance: 0.5 })"#,
            "statuses[0].chance",
        ),
        (
            "type",
            r#"defineStatus("s", { damage: 1, tickMs: 500, type: defineDamageType("ice", {}) })"#,
            "statuses[0].type",
        ),
        ("bare", r#""s""#, "statuses[0]"),
    ];
    for (label, status, needle) in cases {
        let source = format!(
            "globalThis.__postretroModManifest = defineMod({{ name: \"M\", id: \"m\", version: \"1\", statuses: [{status}] }});"
        );
        let result = build(&format!("bad_{label}"), "start-script.js", &source);
        assert_eq!(result.status, StagedManifestBuildStatus::Failed, "{label}");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(needle)),
            "{label}: no diagnostic mentions `{needle}`: {:?}",
            result.diagnostics
        );
    }
    let luau = r#"return defineMod({ name = "M", id = "m", version = "1",
        statuses = { defineStatus("s", { damage = 1, tickMs = 0 }) } })"#;
    let result = build("bad_luau", "start-script.luau", luau);
    assert_eq!(result.status, StagedManifestBuildStatus::Failed);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("statuses[0].tickMs")),
        "{:?}",
        result.diagnostics
    );
}
