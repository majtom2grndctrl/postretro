// Typed attack handles through the real SDK in both runtimes: the handle's
// wire shape, the binding-name diagnostic, the pathed rejections through
// `defineEntity` and a bare `defineMod` entity, same-name handles, the turn
// facts' argument check, Luau strict globals, one handle in two archetypes,
// exhaustive-list drops end to end, and lowering running once per load.
// See: context/lib/scripting.md §2 (Attack and stance declarations)

use std::fs;

use log::Level;
use postretro_foundation::{ActionVerb, IrNode, IrValue};
use postretro_test_log_capture::LogCapture;
use serde_json::{Value, json};

use super::tests::temp_mod_root;
use super::*;

fn build(label: &str, files: &[(&str, &str)]) -> StagedManifestBuildResult {
    let dir = temp_mod_root(&format!("attack_handles_{label}"));
    for (file, source) in files {
        fs::write(dir.join(file), source).expect("fixture should be written");
    }
    build_staged_manifest(&dir, 1, &StagedManifestBuildConfig::default())
}

fn built(label: &str, files: &[(&str, &str)]) -> Box<transfer::StagedManifest> {
    let result = build(label, files);
    match result.status {
        StagedManifestBuildStatus::Built(manifest) => manifest,
        other => panic!("{label} must build: {other:?} {:?}", result.diagnostics),
    }
}

fn assert_rejects(label: &str, files: &[(&str, &str)], needles: &[&str]) {
    let result = build(label, files);
    assert_eq!(
        result.status,
        StagedManifestBuildStatus::Failed,
        "{label} must fail load"
    );
    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            needles
                .iter()
                .all(|needle| diagnostic.message.contains(needle))
        }),
        "{label}: no diagnostic mentions all of {needles:?}: {:?}",
        result.diagnostics
    );
}

/// Bundle TypeScript modules through scripts-build (binding-name sugar
/// included) into one `start-script.js`.
fn bundle_typescript(label: &str, modules: &[(&str, &str)]) -> String {
    let dir = temp_mod_root(&format!("attack_handles_ts_{label}"));
    for (file, source) in modules {
        fs::write(dir.join(file), source).expect("TypeScript module writes");
    }
    let entry = fs::canonicalize(dir.join(modules[0].0)).expect("entry exists");
    postretro_script_compiler::bundle_entry(&entry).expect("the module bundles")
}

/// Every number as `f64`: QuickJS hands integers over as integers, Luau as
/// doubles, and the wire shape is what is compared.
fn normalized(value: Value) -> Value {
    match value {
        Value::Number(number) => json!(number.as_f64().expect("finite")),
        Value::Array(items) => Value::Array(items.into_iter().map(normalized).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, normalized(v))).collect())
        }
        other => other,
    }
}

// T1 / T2: `defineStance` and `defineAttack`, named by their bindings in
// TypeScript and explicitly in Luau, cross as `{ kind, name, ...declaration }`
// with the stance nested in the attack, identically in both runtimes.
#[test]
fn attack_and_stance_handles_cross_with_their_declarations_in_both_runtimes() {
    let bundled = bundle_typescript(
        "handle_shape",
        &[(
            "handles.ts",
            r#"
            import { defineAttack, defineStance } from "postretro";
            const overhead = defineStance({ animation: "smash_windup", holdMs: [500, 1200], breakOn: { damage: 25, to: "flinch" } });
            const slam = defineAttack({
              damage: 14, maxRange: 3.5, cooldownMs: 1800,
              beginningStance: overhead,
              release: { animation: "smash_strike", at: "hit" },
            });
            globalThis.__handle = slam;
            "#,
        )],
    );
    let registry = crate::primitives_registry::PrimitiveRegistry::new();
    let subsystem =
        crate::quickjs::QuickJsSubsystem::new(&registry, &crate::quickjs::QuickJsConfig::default())
            .expect("quickjs definition context");
    let ts = subsystem.definition_ctx().with(|ctx| {
        let _: rquickjs::Value = crate::quickjs::run_script(&ctx, &bundled, "handles.js")
            .expect("the handle module evaluates");
        let handle: rquickjs::Value =
            crate::quickjs::run_script(&ctx, "globalThis.__handle", "read")
                .expect("the handle is published");
        crate::conv::js_to_json(&ctx, handle).expect("the handle converts")
    });

    let root = temp_mod_root("attack_handles_luau_shape");
    let lua = crate::luau::build_lua_state(&[], None, Some(&root)).expect("luau state");
    let luau: mlua::Value = lua
        .load(
            r#"
            local overhead = defineStance("overhead", { animation = "smash_windup", holdMs = { 500, 1200 }, breakOn = { damage = 25, to = "flinch" } })
            return defineAttack("slam", {
              damage = 14, maxRange = 3.5, cooldownMs = 1800,
              beginningStance = overhead,
              release = { animation = "smash_strike", at = "hit" },
            })
            "#,
        )
        .eval()
        .expect("the Luau handle evaluates");
    let luau = crate::conv::lua_to_json(luau).expect("the handle converts");

    let expected = json!({
        "kind": "attack", "name": "slam",
        "damage": 14, "maxRange": 3.5, "cooldownMs": 1800,
        "beginningStance": {
            "kind": "stance", "name": "overhead",
            "animation": "smash_windup", "holdMs": [500, 1200],
            "breakOn": { "damage": 25, "to": "flinch" }
        },
        "release": { "animation": "smash_strike", "at": "hit" }
    });
    assert_eq!(normalized(ts), normalized(expected.clone()));
    assert_eq!(normalized(luau), normalized(expected));
}

// T1: a definer called outside a direct top-level binding gets no name from
// scripts-build and throws the binding-name diagnostic; Luau without a name
// raises its own.
#[test]
fn an_unnamed_attack_or_stance_throws_the_binding_name_diagnostic() {
    for (definer, call) in [
        (
            "defineAttack",
            "defineAttack({ damage: 1, maxRange: 1, cooldownMs: 1 })",
        ),
        ("defineStance", "defineStance({ animation: \"idle\" })"),
    ] {
        let source = format!(
            r#"import {{ defineAttack, defineMod, defineStance }} from "postretro";
            const nested = {{ inline: {call} }};
            export default defineMod({{ name: "M", id: "m", version: "1" }});"#
        );
        let bundled = bundle_typescript(definer, &[("start-script.ts", &source)]);
        assert_rejects(
            &format!("{definer}_ts"),
            &[("start-script.js", &bundled)],
            &[&format!(
                "{definer} without an explicit name is binding-name sugar"
            )],
        );
        let luau_call = call.replace(": ", " = ");
        assert_rejects(
            &format!("{definer}_luau"),
            &[(
                "start-script.luau",
                &format!(
                    "local h = {luau_call}\nreturn defineMod({{ name = \"M\", id = \"m\", version = \"1\" }})"
                ),
            )],
            &[&format!("{definer} needs its name")],
        );
    }
}

/// One behavior authoring with each attack slot a placeholder, in both
/// runtimes. `slam` and `jab` are declared before the manifest.
struct Authoring<'a> {
    slam_extra: &'a str,
    attacks: &'a str,
    action: &'a str,
    mark: &'a str,
    row_to: &'a str,
}

const JS_OK: Authoring<'static> = Authoring {
    slam_extra: "",
    attacks: "",
    action: "slam",
    mark: "",
    row_to: "\"engage\"",
};

fn ts_source(case: &Authoring, wrapped: bool) -> String {
    let attacks = if case.attacks.is_empty() {
        String::new()
    } else {
        format!("attacks: {},", case.attacks)
    };
    let mark = if case.mark.is_empty() {
        String::new()
    } else {
        format!(", attackTurn: {}", case.mark)
    };
    let entity = format!(
        r#"{{ canonicalName: "brute", components: {{ behavior: {{
             initial: "idle", moveSpeed: 3, {attacks}
             activities: {{
               idle: {{ animation: "idle", motion: "hold" }},
               engage: {{ animation: "walk", motion: "chaseTarget", action: {{ attack: {action} }}{mark} }}
             }},
             transitions: {{ idle: [{{ when: brain.hasTarget, to: {row_to} }}] }}
           }} }} }}"#,
        action = case.action,
        row_to = case.row_to,
    );
    let entity = if wrapped {
        format!("defineEntity({entity})")
    } else {
        entity
    };
    format!(
        r#"const overhead = defineStance("overhead", {{ animation: "smash_windup" }});
           const slam = defineAttack("slam", {{ damage: 8, maxRange: 2, cooldownMs: 1200{slam_extra} }});
           const jab = defineAttack("jab", {{ damage: 4, maxRange: 2, cooldownMs: 500 }});
           globalThis.__postretroModManifest = defineMod({{ name: "M", id: "m", version: "1",
             entities: [{entity}] }});"#,
        slam_extra = case.slam_extra,
    )
}

fn luau_source(case: &Authoring, wrapped: bool) -> String {
    let attacks = if case.attacks.is_empty() {
        String::new()
    } else {
        format!("attacks = {},", case.attacks)
    };
    let mark = if case.mark.is_empty() {
        String::new()
    } else {
        format!(", attackTurn = {}", case.mark)
    };
    let entity = format!(
        r#"{{ canonicalName = "brute", components = {{ behavior = {{
             initial = "idle", moveSpeed = 3, {attacks}
             activities = {{
               idle = {{ animation = "idle", motion = "hold" }},
               engage = {{ animation = "walk", motion = "chaseTarget", action = {{ attack = {action} }}{mark} }}
             }},
             transitions = {{ idle = {{ {{ when = brain.hasTarget, to = {row_to} }} }} }}
           }} }} }}"#,
        action = case.action,
        row_to = case.row_to,
    );
    let entity = if wrapped {
        format!("defineEntity({entity})")
    } else {
        entity
    };
    format!(
        r#"local overhead = defineStance("overhead", {{ animation = "smash_windup" }})
           local slam = defineAttack("slam", {{ damage = 8, maxRange = 2, cooldownMs = 1200{slam_extra} }})
           local jab = defineAttack("jab", {{ damage = 4, maxRange = 2, cooldownMs = 500 }})
           return defineMod({{ name = "M", id = "m", version = "1",
             entities = {{ {entity} }} }})"#,
        slam_extra = case.slam_extra.replace(": ", " = "),
    )
}

/// Run one authoring in both runtimes through both routes, expecting a
/// rejection naming every needle.
fn assert_rejects_everywhere(label: &str, ts: &Authoring, luau: &Authoring, needles: &[&str]) {
    for wrapped in [true, false] {
        let route = if wrapped { "define_entity" } else { "bare" };
        assert_rejects(
            &format!("{label}_{route}_js"),
            &[("start-script.js", &ts_source(ts, wrapped))],
            needles,
        );
        assert_rejects(
            &format!("{label}_{route}_luau"),
            &[("start-script.luau", &luau_source(luau, wrapped))],
            needles,
        );
    }
}

// The unmutated authoring loads in both runtimes and both routes, with the
// attack resolved to its name and the map built from the declaration.
#[test]
fn a_handle_authoring_loads_through_define_entity_and_a_bare_entity() {
    for wrapped in [true, false] {
        let js = built("ok_js", &[("start-script.js", &ts_source(&JS_OK, wrapped))]);
        let luau = built(
            "ok_luau",
            &[("start-script.luau", &luau_source(&JS_OK, wrapped))],
        );
        assert_eq!(js.entities, luau.entities);
        let behavior = js.entities[0].behavior.as_ref().expect("behavior");
        assert_eq!(behavior.attacks.keys().collect::<Vec<_>>(), ["slam"]);
        assert_eq!(behavior.attacks["slam"].damage, Some(8.0));
        assert_eq!(
            behavior.envelope.activities["engage"].action,
            Some(ActionVerb::Attack {
                name: "slam".to_string(),
                at: None
            })
        );
    }
}

// T4: a string, or anything but an attack handle, where one is expected
// rejects load with a pathed error in both runtimes, through `defineEntity`
// and a bare `defineMod` entity, by the one lowering.
#[test]
fn a_string_where_an_attack_handle_is_expected_rejects_load_everywhere() {
    // QuickJS names entries from 0, Luau from 1.
    const BEHAVIOR: &str = "entities[";
    let cases: [(&str, Authoring, Authoring, &str, &str); 7] = [
        (
            "list_string",
            Authoring {
                attacks: "[\"slam\"]",
                ..JS_OK
            },
            Authoring {
                attacks: "{ \"slam\" }",
                ..JS_OK
            },
            "components.behavior.attacks[0]",
            "bare string",
        ),
        (
            "name_keyed_map",
            Authoring {
                attacks: "{ slam: { damage: 8, maxRange: 2, cooldownMs: 1200 } }",
                ..JS_OK
            },
            Authoring {
                attacks: "{ slam = { damage = 8, maxRange = 2, cooldownMs = 1200 } }",
                ..JS_OK
            },
            "`components.behavior.attacks` is a name-keyed map",
            "defineAttack",
        ),
        (
            "action_string",
            Authoring {
                action: "\"slam\"",
                ..JS_OK
            },
            Authoring {
                action: "\"slam\"",
                ..JS_OK
            },
            "components.behavior.activities.engage.action.attack",
            "bare string",
        ),
        (
            "mark_string",
            Authoring {
                mark: "\"slam\"",
                ..JS_OK
            },
            Authoring {
                mark: "\"slam\"",
                ..JS_OK
            },
            "components.behavior.activities.engage.attackTurn",
            "bare string",
        ),
        (
            "row_to_string",
            Authoring {
                row_to: "\"slam\"",
                ..JS_OK
            },
            Authoring {
                row_to: "\"slam\"",
                ..JS_OK
            },
            "components.behavior.transitions.idle[0]",
            "slam",
        ),
        (
            "beginning_stance_string",
            Authoring {
                slam_extra: ", beginningStance: \"overhead\"",
                ..JS_OK
            },
            Authoring {
                slam_extra: ", beginningStance: \"overhead\"",
                ..JS_OK
            },
            "components.behavior.activities.engage.action.attack.beginningStance",
            "pass the stance handle `defineStance` returned",
        ),
        (
            "ending_stance_string",
            Authoring {
                slam_extra: ", endingStance: \"overhead\"",
                ..JS_OK
            },
            Authoring {
                slam_extra: ", endingStance: \"overhead\"",
                ..JS_OK
            },
            "components.behavior.activities.engage.action.attack.endingStance",
            "bare string",
        ),
    ];
    for (label, ts, luau, path, detail) in &cases {
        assert_rejects_everywhere(label, ts, luau, &[BEHAVIOR, path, detail]);
    }
}

// T4: two handles of one name load when their declarations are equal (a Luau
// re-`require` makes copies) and reject, naming both places, when unequal.
#[test]
fn same_name_handles_load_when_equal_and_reject_when_unequal() {
    const ATTACKS_LUAU: &str = r#"return { slam = defineAttack("slam", { damage = 8, maxRange = 2, cooldownMs = 1200 }) }"#;
    let copies = built(
        "equal_copies_luau",
        &[
            ("attacks.luau", ATTACKS_LUAU),
            (
                "start-script.luau",
                &luau_source(
                    &Authoring {
                        attacks: "{ require(\"./attacks.luau\").slam }",
                        action: "require(\"./attacks.luau\").slam",
                        ..JS_OK
                    },
                    true,
                ),
            ),
        ],
    );
    assert_eq!(
        copies.entities[0].behavior.as_ref().unwrap().attacks.len(),
        1
    );
    built(
        "equal_copies_js",
        &[(
            "start-script.js",
            &ts_source(
                &Authoring {
                    attacks: "[defineAttack(\"slam\", { damage: 8, maxRange: 2, cooldownMs: 1200 })]",
                    ..JS_OK
                },
                true,
            ),
        )],
    );

    let needles = [
        "`components.behavior.activities.engage.action.attack` declares attack \"slam\" differently from `components.behavior.attacks[0]`",
    ];
    assert_rejects(
        "unequal_js",
        &[(
            "start-script.js",
            &ts_source(
                &Authoring {
                    attacks: "[defineAttack(\"slam\", { damage: 9, maxRange: 2, cooldownMs: 1200 })]",
                    ..JS_OK
                },
                true,
            ),
        )],
        &needles,
    );
    assert_rejects(
        "unequal_luau",
        &[(
            "start-script.luau",
            &luau_source(
                &Authoring {
                    attacks: "{ defineAttack(\"slam\", { damage = 9, maxRange = 2, cooldownMs = 1200 }) }",
                    ..JS_OK
                },
                false,
            ),
        )],
        &needles,
    );
}

// T5: `attackTurnFree` and `attackTurnHeld` given a string or nil throw at the
// call, naming the fact, in both runtimes.
#[test]
fn turn_facts_given_a_string_or_nil_throw_at_the_call() {
    for (index, (fact, js_arg, luau_arg, found)) in [
        (
            "attackTurnHeld",
            "\"slam\"",
            "\"slam\"",
            "the bare string \"slam\"",
        ),
        (
            "attackTurnFree",
            "\"slam\"",
            "\"slam\"",
            "the bare string \"slam\"",
        ),
        ("attackTurnHeld", "undefined", "nil", ""),
        ("attackTurnFree", "undefined", "nil", ""),
    ]
    .into_iter()
    .enumerate()
    {
        let message = format!("{fact}(attack): expected an attack handle from defineAttack, got");
        let js_found = if found.is_empty() { "undefined" } else { found };
        let luau_found = if found.is_empty() { "nil" } else { found };
        assert_rejects(
            &format!("turn_fact_{index}_js"),
            &[(
                "start-script.js",
                &format!(
                    "const guard = {fact}({js_arg});\nglobalThis.__postretroModManifest = defineMod({{ name: \"M\", id: \"m\", version: \"1\" }});"
                ),
            )],
            &[&message, js_found],
        );
        assert_rejects(
            &format!("turn_fact_{index}_luau"),
            &[(
                "start-script.luau",
                &format!(
                    "local guard = {fact}({luau_arg})\nreturn defineMod({{ name = \"M\", id = \"m\", version = \"1\" }})"
                ),
            )],
            &[&message, luau_found, "start-script.luau\"]:1:"],
        );
    }
}

// T5: Luau data scripts run under strict globals, so a misspelled handle in a
// table field and a misspelled `when` guard are reported at load, naming the
// global and its line, rather than reading nil as an absent field.
#[test]
fn a_misspelled_luau_global_in_a_table_field_is_reported_at_load() {
    let misspelled_handle = luau_source(
        &Authoring {
            action: "slamm",
            ..JS_OK
        },
        true,
    );
    let line = misspelled_handle
        .lines()
        .position(|line| line.contains("slamm"))
        .expect("the misspelling is in the source")
        + 1;
    assert_rejects(
        "misspelled_handle",
        &[("start-script.luau", &misspelled_handle)],
        &[&format!(
            "start-script.luau\"]:{line}: undeclared global `slamm`"
        )],
    );

    let misspelled_guard =
        luau_source(&JS_OK, true).replace("when = brain.hasTarget", "when = hasTargt");
    let line = misspelled_guard
        .lines()
        .position(|line| line.contains("hasTargt"))
        .expect("the misspelling is in the source")
        + 1;
    assert_rejects(
        "misspelled_guard",
        &[("start-script.luau", &misspelled_guard)],
        &[&format!(
            "start-script.luau\"]:{line}: undeclared global `hasTargt`"
        )],
    );

    // A required module is held to the same rule.
    assert_rejects(
        "misspelled_in_module",
        &[
            (
                "attacks.luau",
                "return { slam = defineAttak(\"slam\", {}) }",
            ),
            (
                "start-script.luau",
                "local A = require(\"./attacks.luau\")\nreturn defineMod({ name = \"M\", id = \"m\", version = \"1\" })",
            ),
        ],
        &["attacks.luau\"]:1: undeclared global `defineAttak`"],
    );
}

// T7: one handle serves two archetypes, one through `defineEntity` and one
// bare; each resolves it by name.
#[test]
fn one_handle_serves_two_archetypes_by_name() {
    let ts = built(
        "two_archetypes_js",
        &[(
            "start-script.js",
            r#"const slam = defineAttack("slam", { damage: 8, maxRange: 2, cooldownMs: 1200 });
               const graph = (activity) => ({ initial: activity, moveSpeed: 3,
                 activities: { [activity]: { animation: "walk", motion: "chaseTarget", action: { attack: slam } } },
                 transitions: {} });
               globalThis.__postretroModManifest = defineMod({ name: "M", id: "m", version: "1",
                 entities: [
                   defineEntity({ canonicalName: "brute", components: { behavior: graph("pound") } }),
                   { canonicalName: "ogre", components: { behavior: graph("crush") } },
                 ] });"#,
        )],
    );
    let luau = built(
        "two_archetypes_luau",
        &[(
            "start-script.luau",
            r#"local slam = defineAttack("slam", { damage = 8, maxRange = 2, cooldownMs = 1200 })
               local function graph(activity)
                 return { initial = activity, moveSpeed = 3,
                   activities = { [activity] = { animation = "walk", motion = "chaseTarget", action = { attack = slam } } },
                   transitions = {} }
               end
               return defineMod({ name = "M", id = "m", version = "1",
                 entities = {
                   defineEntity({ canonicalName = "brute", components = { behavior = graph("pound") } }),
                   { canonicalName = "ogre", components = { behavior = graph("crush") } },
                 } })"#,
        )],
    );
    assert_eq!(ts.entities, luau.entities);
    for (entity, activity) in ts.entities.iter().zip(["pound", "crush"]) {
        let behavior = entity.behavior.as_ref().expect("behavior");
        assert_eq!(behavior.attacks["slam"].damage, Some(8.0));
        assert_eq!(
            behavior.envelope.activities[activity].action,
            Some(ActionVerb::Attack {
                name: "slam".to_string(),
                at: None
            })
        );
    }
}

// T3 / P41 end to end: under `attacks: [slam]`, a row to `jab` whose guard
// reads `jab`'s turn drops with a log and loads (a); a kept row whose guard
// reads `jab`'s turn rejects load naming the guard (b).
#[test]
fn an_exhaustive_list_drops_rows_to_unlisted_attacks_and_checks_only_kept_guards() {
    let capture = LogCapture::start();
    let dropped_js = ts_source(&Authoring { attacks: "[slam]", ..JS_OK }, true).replace(
        "transitions: { idle: [{ when: brain.hasTarget, to: \"engage\" }] }",
        "transitions: { idle: [{ when: attackTurnHeld(jab), to: jab }, { when: brain.hasTarget, to: \"engage\" }] }",
    );
    let dropped_luau = luau_source(&Authoring { attacks: "{ slam }", ..JS_OK }, true).replace(
        "transitions = { idle = { { when = brain.hasTarget, to = \"engage\" } } }",
        "transitions = { idle = { { when = attackTurnHeld(jab), to = jab }, { when = brain.hasTarget, to = \"engage\" } } }",
    );
    let js = built("p41a_js", &[("start-script.js", &dropped_js)]);
    let luau = built("p41a_luau", &[("start-script.luau", &dropped_luau)]);
    assert_eq!(js.entities, luau.entities);
    let rows = &js.entities[0]
        .behavior
        .as_ref()
        .unwrap()
        .envelope
        .transitions["idle"];
    assert_eq!(rows.len(), 1, "only the row to `jab` drops");
    assert_eq!(rows[0].to, "engage");
    let drops = capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == Level::Info
                && record.message.contains("brute: `components.behavior.transitions.idle[0]` drops its row to attack \"jab\"")
        })
        .count();
    assert_eq!(drops, 2, "one log per load, in each runtime");

    let kept_js = ts_source(
        &Authoring {
            attacks: "[slam]",
            ..JS_OK
        },
        true,
    )
    .replace("when: brain.hasTarget", "when: attackTurnFree(jab)");
    let kept_luau = luau_source(
        &Authoring {
            attacks: "{ slam }",
            ..JS_OK
        },
        false,
    )
    .replace("when = brain.hasTarget", "when = attackTurnFree(jab)");
    let needles = [
        "entities[0].components.behavior.transitions.idle[0].when",
        "@attackTurnFree.jab",
    ];
    assert_rejects("p41b_js", &[("start-script.js", &kept_js)], &needles);
    assert_rejects("p41b_luau", &[("start-script.luau", &kept_luau)], &needles);
}

// S5: a row without `when` loads in both runtimes and is always enabled.
#[test]
fn a_row_without_when_loads_as_always_enabled_in_both_runtimes() {
    let js = built(
        "no_when_js",
        &[(
            "start-script.js",
            &ts_source(&JS_OK, true).replace("when: brain.hasTarget, ", ""),
        )],
    );
    let luau = built(
        "no_when_luau",
        &[(
            "start-script.luau",
            &luau_source(&JS_OK, true).replace("when = brain.hasTarget, ", ""),
        )],
    );
    assert_eq!(js.entities, luau.entities);
    let row = &js.entities[0]
        .behavior
        .as_ref()
        .unwrap()
        .envelope
        .transitions["idle"][0];
    assert_eq!(row.to, "engage");
    assert_eq!(
        row.when,
        IrNode::Const {
            value: IrValue::Bool(true)
        }
    );
}

// T2: lowering runs once per entity read at mod load and again at a staged
// reload, and never for a level's data script.
#[test]
fn lowering_runs_once_per_mod_load_and_staged_reload_never_per_level() {
    use crate::data_descriptors::lowering_runs;

    let root = temp_mod_root("attack_handles_runs");
    let start_script = luau_source(&JS_OK, true).replace(
        "entities = { defineEntity(",
        "entities = { { canonicalName = \"lamp\", components = {} }, defineEntity(",
    );
    fs::write(root.join("start-script.luau"), &start_script).expect("start script writes");

    let ctx = postretro_entities::ScriptCtx::new();
    let primitives = crate::primitives_registry::PrimitiveRegistry::new();
    let mut runtime = crate::runtime::ScriptRuntime::new(
        &primitives,
        &crate::runtime::ScriptRuntimeConfig::default(),
        &ctx,
    )
    .expect("runtime");

    let before = lowering_runs();
    runtime.run_mod_init(&root).expect("mod init");
    assert_eq!(
        lowering_runs() - before,
        1,
        "one behavior lowers once at mod load"
    );

    let level = postretro_level_format::data_script::DataScriptSection {
        compiled_bytes: b"function setupLevel(ctx) return {} end".to_vec(),
        source_path: "level.luau".to_string(),
    };
    let before = lowering_runs();
    let _ = runtime.run_data_script(&level, &root);
    let _ = runtime.run_data_script(&level, &root);
    assert_eq!(lowering_runs(), before, "a level load lowers nothing");

    let before = lowering_runs();
    let result = build_staged_manifest(&root, 2, &StagedManifestBuildConfig::default());
    assert!(
        matches!(result.status, StagedManifestBuildStatus::Built(_)),
        "{:?}",
        result.diagnostics
    );
    assert_eq!(
        lowering_runs() - before,
        1,
        "a staged reload lowers each behavior once"
    );
}
