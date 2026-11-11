// Declaring and registering attack tokens and budget profiles through the SDK
// in both runtimes, the wire shapes handles and references cross as, and the
// load failures every token rule raises.
// See: context/lib/scripting.md §2 (Attack token declarations)

use std::fs;

use postretro_foundation::attack_budget::{TokenBudget, TokenDraw};
use serde_json::json;

use super::tests::temp_mod_root;
use super::*;

fn build(label: &str, entry: &str, source: &str) -> StagedManifestBuildResult {
    let dir = temp_mod_root(&format!("attack_budget_{label}"));
    fs::write(dir.join(entry), source).expect("fixture should be written");
    build_staged_manifest(&dir, 1, &StagedManifestBuildConfig::default())
}

fn built(result: StagedManifestBuildResult) -> Box<transfer::StagedManifest> {
    match result.status {
        StagedManifestBuildStatus::Built(manifest) => manifest,
        other => panic!("manifest must build: {other:?} {:?}", result.diagnostics),
    }
}

/// One case in both runtimes: extra declarations, the `defineMod` token
/// fields, and the `slam` attack's `draws` (empty leaves it out).
struct Case<'a> {
    decls: &'a str,
    fields: &'a str,
    draws: &'a str,
}

const TS_TOKENS: &str = "attackTokens: [melee, heavy]";
const LUAU_TOKENS: &str = "attackTokens = { melee, heavy }";

fn ts_mod(case: &Case) -> String {
    let draws = if case.draws.is_empty() {
        String::new()
    } else {
        format!(", draws: {}", case.draws)
    };
    format!(
        r#"const melee = defineAttackToken("melee", {{ limit: 3 }});
           const heavy = defineAttackToken("heavy", {{ limit: 1, refreshMs: 4000 }});
           const shock = defineDamageType("shock", {{}});
           {decls}
           globalThis.__postretroModManifest = defineMod({{ name: "M", id: "m", version: "1",
             damageTypes: [shock], {fields},
             entities: [{{ canonicalName: "brute", components: {{ behavior: {{
               initial: "idle", moveSpeed: 3,
               attacks: {{
                 slam: {{ damage: 14, maxRange: 3, cooldownMs: 1800{draws} }},
                 jab: {{ damage: 8, maxRange: 2, cooldownMs: 1200 }}
               }},
               activities: {{
                 idle: {{ animation: "idle", motion: "hold" }},
                 engage: {{ animation: "walk", motion: "chaseTarget", action: {{ attack: "slam" }} }}
               }},
               transitions: {{}}
             }} }} }}] }});"#,
        decls = case.decls,
        fields = case.fields,
    )
}

fn luau_mod(case: &Case) -> String {
    let draws = if case.draws.is_empty() {
        String::new()
    } else {
        format!(", draws = {}", case.draws)
    };
    format!(
        r#"local melee = defineAttackToken("melee", {{ limit = 3 }})
           local heavy = defineAttackToken("heavy", {{ limit = 1, refreshMs = 4000 }})
           local shock = defineDamageType("shock", {{}})
           {decls}
           return defineMod({{ name = "M", id = "m", version = "1",
             damageTypes = {{ shock }}, {fields},
             entities = {{ {{ canonicalName = "brute", components = {{ behavior = {{
               initial = "idle", moveSpeed = 3,
               attacks = {{
                 slam = {{ damage = 14, maxRange = 3, cooldownMs = 1800{draws} }},
                 jab = {{ damage = 8, maxRange = 2, cooldownMs = 1200 }}
               }},
               activities = {{
                 idle = {{ animation = "idle", motion = "hold" }},
                 engage = {{ animation = "walk", motion = "chaseTarget", action = {{ attack = "slam" }} }}
               }},
               transitions = {{}}
             }} }} }} }} }})"#,
        decls = case.decls,
        fields = case.fields,
    )
}

fn assert_fails(label: &str, entry: &str, source: &str, needle: &str) {
    let result = build(label, entry, source);
    assert_eq!(
        result.status,
        StagedManifestBuildStatus::Failed,
        "{label} ({entry}) must fail load"
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains(needle)),
        "{label} ({entry}): no diagnostic mentions `{needle}`: {:?}",
        result.diagnostics
    );
}

const DRAWS: &str = "components.behavior.attacks.slam.draws";

// The Scripting surface's token shape loads identically in both runtimes:
// handles cross as `{ kind, name }`, `with` as `{ token, ...fields }`, each
// token carries its declared budget, and a profile its overrides.
#[test]
fn tokens_profiles_and_draws_load_identically_in_both_runtimes() {
    let js = built(build(
        "surface_js",
        "start-script.js",
        &ts_mod(&Case {
            decls: "",
            fields: "attackTokens: [melee, heavy], attackBudgets: { arena: [melee.with({ limit: 1 }), heavy.with({ refreshMs: 2000 })] }",
            draws: "[melee.with({ cost: 2 }), heavy]",
        }),
    ));
    let luau = built(build(
        "surface_luau",
        "start-script.luau",
        &luau_mod(&Case {
            decls: "",
            fields: "attackTokens = { melee, heavy }, attackBudgets = { arena = { melee:with({ limit = 1 }), heavy:with({ refreshMs = 2000 }) } }",
            draws: "{ melee:with({ cost = 2 }), heavy }",
        }),
    ));
    assert_eq!(js.attack_budgets, luau.attack_budgets);
    assert_eq!(
        js.entities, luau.entities,
        "both runtimes produce one descriptor"
    );

    let budgets: Vec<TokenBudget> = js.attack_budgets.declared_budgets().collect();
    assert_eq!(
        budgets,
        [
            TokenBudget::DEFAULT,
            TokenBudget {
                limit: 3,
                refresh_ms: 0.0
            },
            TokenBudget {
                limit: 1,
                refresh_ms: 4000.0
            },
        ]
    );
    let arena = js.attack_budgets.profile_budgets("arena").unwrap();
    assert_eq!(
        arena[1],
        TokenBudget {
            limit: 1,
            refresh_ms: 0.0
        }
    );
    assert_eq!(
        arena[2],
        TokenBudget {
            limit: 1,
            refresh_ms: 2000.0
        }
    );

    let attacks = &js.entities[0].behavior.as_ref().unwrap().attacks;
    assert_eq!(
        attacks["slam"].draws,
        Some(vec![
            TokenDraw(json!({ "token": { "kind": "attackToken", "name": "melee" }, "cost": 2 })),
            TokenDraw(json!({ "kind": "attackToken", "name": "heavy" })),
        ]),
        "a handle crosses as its kind and name; `with` stays off the wire"
    );
    assert_eq!(
        attacks["jab"].draws, None,
        "no `draws` is absent, not empty"
    );
}

// `draws: []` opts out in both runtimes; Luau writes it `{}`.
#[test]
fn empty_draws_opt_out_in_both_runtimes() {
    let js = built(build(
        "empty_js",
        "start-script.js",
        &ts_mod(&Case {
            decls: "",
            fields: TS_TOKENS,
            draws: "[]",
        }),
    ));
    let luau = built(build(
        "empty_luau",
        "start-script.luau",
        &luau_mod(&Case {
            decls: "",
            fields: LUAU_TOKENS,
            draws: "{}",
        }),
    ));
    for manifest in [js, luau] {
        let attacks = &manifest.entities[0].behavior.as_ref().unwrap().attacks;
        assert_eq!(attacks["slam"].draws, Some(Vec::new()));
    }
}

// B19: `defaultToken` needs no entry in `attackTokens`. A mod registering only
// `melee` loads an attack without `draws` and one drawing `defaultToken`, and
// a profile may override it; listing it in `attackTokens` rejects.
#[test]
fn default_token_needs_no_registration_and_rejects_when_listed() {
    let js = built(build(
        "default_js",
        "start-script.js",
        &ts_mod(&Case {
            decls: "",
            fields: "attackTokens: [melee], attackBudgets: { calm: [defaultToken.with({ limit: 1 })] }",
            draws: "[defaultToken, melee]",
        }),
    ));
    let luau = built(build(
        "default_luau",
        "start-script.luau",
        &luau_mod(&Case {
            decls: "",
            fields: "attackTokens = { melee }, attackBudgets = { calm = { defaultToken:with({ limit = 1 }) } }",
            draws: "{ defaultToken, melee }",
        }),
    ));
    assert_eq!(js.attack_budgets, luau.attack_budgets);
    assert_eq!(js.attack_budgets.token_index("default"), Some(0));
    assert_eq!(
        js.attack_budgets.profile_budgets("calm").unwrap()[0],
        TokenBudget {
            limit: 1,
            refresh_ms: 0.0
        },
        "a profile overrides `defaultToken`"
    );

    assert_fails(
        "default_listed_js",
        "start-script.js",
        &ts_mod(&Case {
            decls: "",
            fields: "attackTokens: [melee, defaultToken]",
            draws: "",
        }),
        "attackTokens[1]",
    );
    assert_fails(
        "default_listed_luau",
        "start-script.luau",
        &luau_mod(&Case {
            decls: "",
            fields: "attackTokens = { melee, defaultToken }",
            draws: "",
        }),
        "attackTokens[1]",
    );
}

// Row A8 (token half) and B19: every token rule rejects load with a key path,
// in both runtimes. `(label, ts case, luau case, needle)`.
#[test]
fn attack_budget_declarations_reject_with_pathed_errors() {
    let nine_ts = (0..9)
        .map(|i| format!("const t{i} = defineAttackToken(\"t{i}\", {{}});"))
        .collect::<String>();
    let nine_luau = (0..9)
        .map(|i| format!("local t{i} = defineAttackToken(\"t{i}\", {{}})\n"))
        .collect::<String>();
    let nine_list_ts = (0..9)
        .map(|i| format!("t{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let nine_tokens_ts = format!("attackTokens: [{nine_list_ts}]");
    let nine_draws_ts = format!("[{nine_list_ts}]");
    let nine_tokens_luau = format!("attackTokens = {{ {nine_list_ts} }}");
    let nine_draws_luau = format!("{{ {nine_list_ts} }}");
    let draw = |suffix: &str| format!("{DRAWS}{suffix}");

    let cases: Vec<(&str, Case, Case, String)> = vec![
        (
            "limit-zero",
            Case {
                decls: r#"const bad = defineAttackToken("bad", { limit: 0 });"#,
                fields: "attackTokens: [melee, heavy, bad]",
                draws: "",
            },
            Case {
                decls: r#"local bad = defineAttackToken("bad", { limit = 0 })"#,
                fields: "attackTokens = { melee, heavy, bad }",
                draws: "",
            },
            "attackTokens[2].budget.limit".to_string(),
        ),
        (
            "limit-fraction",
            Case {
                decls: r#"const bad = defineAttackToken("bad", { limit: 1.5 });"#,
                fields: "attackTokens: [melee, heavy, bad]",
                draws: "",
            },
            Case {
                decls: r#"local bad = defineAttackToken("bad", { limit = 1.5 })"#,
                fields: "attackTokens = { melee, heavy, bad }",
                draws: "",
            },
            "attackTokens[2].budget.limit".to_string(),
        ),
        (
            "refresh-negative",
            Case {
                decls: r#"const bad = defineAttackToken("bad", { refreshMs: -1 });"#,
                fields: "attackTokens: [melee, heavy, bad]",
                draws: "",
            },
            Case {
                decls: r#"local bad = defineAttackToken("bad", { refreshMs = -1 })"#,
                fields: "attackTokens = { melee, heavy, bad }",
                draws: "",
            },
            "attackTokens[2].budget.refreshMs".to_string(),
        ),
        (
            "refresh-non-finite",
            Case {
                decls: r#"const bad = defineAttackToken("bad", { refreshMs: Infinity });"#,
                fields: "attackTokens: [melee, heavy, bad]",
                draws: "",
            },
            Case {
                decls: r#"local bad = defineAttackToken("bad", { refreshMs = math.huge })"#,
                fields: "attackTokens = { melee, heavy, bad }",
                draws: "",
            },
            "refreshMs".to_string(),
        ),
        (
            "cost-in-profile",
            Case {
                decls: "",
                fields: "attackTokens: [melee, heavy], attackBudgets: { duel: [melee.with({ cost: 2 })] }",
                draws: "",
            },
            Case {
                decls: "",
                fields: "attackTokens = { melee, heavy }, attackBudgets = { duel = { melee:with({ cost = 2 }) } }",
                draws: "",
            },
            "attackBudgets.duel[0].cost".to_string(),
        ),
        (
            "limit-in-draw",
            Case {
                decls: "",
                fields: TS_TOKENS,
                draws: "[melee.with({ limit: 1 })]",
            },
            Case {
                decls: "",
                fields: LUAU_TOKENS,
                draws: "{ melee:with({ limit = 1 }) }",
            },
            "attacks.slam.draws[0].limit".to_string(),
        ),
        (
            "refresh-in-draw",
            Case {
                decls: "",
                fields: TS_TOKENS,
                draws: "[melee.with({ refreshMs: 10 })]",
            },
            Case {
                decls: "",
                fields: LUAU_TOKENS,
                draws: "{ melee:with({ refreshMs = 10 }) }",
            },
            "attacks.slam.draws[0].refreshMs".to_string(),
        ),
        (
            "cost-zero",
            Case {
                decls: "",
                fields: TS_TOKENS,
                draws: "[melee.with({ cost: 0 })]",
            },
            Case {
                decls: "",
                fields: LUAU_TOKENS,
                draws: "{ melee:with({ cost = 0 }) }",
            },
            "attacks.slam.draws[0].cost".to_string(),
        ),
        (
            "cost-fraction",
            Case {
                decls: "",
                fields: TS_TOKENS,
                draws: "[melee.with({ cost: 1.5 })]",
            },
            Case {
                decls: "",
                fields: LUAU_TOKENS,
                draws: "{ melee:with({ cost = 1.5 }) }",
            },
            "attacks.slam.draws[0].cost".to_string(),
        ),
        (
            "drawn-twice",
            Case {
                decls: "",
                fields: TS_TOKENS,
                draws: "[melee, melee.with({ cost: 2 })]",
            },
            Case {
                decls: "",
                fields: LUAU_TOKENS,
                draws: "{ melee, melee:with({ cost = 2 }) }",
            },
            "drawn twice".to_string(),
        ),
        (
            "overridden-twice",
            Case {
                decls: "",
                fields: "attackTokens: [melee, heavy], attackBudgets: { duel: [melee.with({ limit: 1 }), melee.with({ refreshMs: 0 })] }",
                draws: "",
            },
            Case {
                decls: "",
                fields: "attackTokens = { melee, heavy }, attackBudgets = { duel = { melee:with({ limit = 1 }), melee:with({ refreshMs = 0 }) } }",
                draws: "",
            },
            "attackBudgets.duel[1]".to_string(),
        ),
        (
            "bare-string-draw",
            Case {
                decls: "",
                fields: TS_TOKENS,
                draws: r#"["melee"]"#,
            },
            Case {
                decls: "",
                fields: LUAU_TOKENS,
                draws: r#"{ "melee" }"#,
            },
            draw("[0]"),
        ),
        (
            "other-kind-draw",
            Case {
                decls: "",
                fields: TS_TOKENS,
                draws: "[shock]",
            },
            Case {
                decls: "",
                fields: LUAU_TOKENS,
                draws: "{ shock }",
            },
            "expected an attack token handle, got damage type `shock`".to_string(),
        ),
        (
            "unregistered-draw",
            Case {
                decls: r#"const lobber = defineAttackToken("lobber", {});"#,
                fields: TS_TOKENS,
                draws: "[lobber]",
            },
            Case {
                decls: r#"local lobber = defineAttackToken("lobber", {})"#,
                fields: LUAU_TOKENS,
                draws: "{ lobber }",
            },
            draw("[0]"),
        ),
        (
            "too-many-draws",
            Case {
                decls: &nine_ts,
                fields: &nine_tokens_ts,
                draws: &nine_draws_ts,
            },
            Case {
                decls: &nine_luau,
                fields: &nine_tokens_luau,
                draws: &nine_draws_luau,
            },
            "an attack draws at most 8".to_string(),
        ),
        (
            "bare-string-token",
            Case {
                decls: "",
                fields: r#"attackTokens: ["melee"]"#,
                draws: "",
            },
            Case {
                decls: "",
                fields: r#"attackTokens = { "melee" }"#,
                draws: "",
            },
            "attackTokens[0]".to_string(),
        ),
        (
            "other-kind-token",
            Case {
                decls: "",
                fields: "attackTokens: [melee, shock]",
                draws: "",
            },
            Case {
                decls: "",
                fields: "attackTokens = { melee, shock }",
                draws: "",
            },
            "got a `damageType` handle".to_string(),
        ),
        (
            "bare-string-profile",
            Case {
                decls: "",
                fields: r#"attackTokens: [melee], attackBudgets: { duel: ["melee"] }"#,
                draws: "",
            },
            Case {
                decls: "",
                fields: r#"attackTokens = { melee }, attackBudgets = { duel = { "melee" } }"#,
                draws: "",
            },
            "attackBudgets.duel[0]".to_string(),
        ),
        (
            "unregistered-profile",
            Case {
                decls: "",
                fields: "attackTokens: [melee], attackBudgets: { duel: [heavy.with({ limit: 2 })] }",
                draws: "",
            },
            Case {
                decls: "",
                fields: "attackTokens = { melee }, attackBudgets = { duel = { heavy:with({ limit = 2 }) } }",
                draws: "",
            },
            "attackBudgets.duel[0]".to_string(),
        ),
        (
            "duplicate-name",
            Case {
                decls: r#"const again = defineAttackToken("melee", {});"#,
                fields: "attackTokens: [melee, again]",
                draws: "",
            },
            Case {
                decls: r#"local again = defineAttackToken("melee", {})"#,
                fields: "attackTokens = { melee, again }",
                draws: "",
            },
            "registered twice".to_string(),
        ),
        (
            "mod-token-named-default",
            Case {
                decls: r#"const mine = defineAttackToken("default", {});"#,
                fields: "attackTokens: [mine]",
                draws: "",
            },
            Case {
                decls: r#"local mine = defineAttackToken("default", {})"#,
                fields: "attackTokens = { mine }",
                draws: "",
            },
            "attackTokens[0]".to_string(),
        ),
        (
            "unknown-budget-field",
            Case {
                decls: r#"const bad = defineAttackToken("bad", { limt: 2 });"#,
                fields: "attackTokens: [bad]",
                draws: "",
            },
            Case {
                decls: r#"local bad = defineAttackToken("bad", { limt = 2 })"#,
                fields: "attackTokens = { bad }",
                draws: "",
            },
            "attackTokens[0].budget.limt".to_string(),
        ),
    ];
    for (label, ts, luau, needle) in &cases {
        assert_fails(
            &format!("{label}_js"),
            "start-script.js",
            &ts_mod(ts),
            needle,
        );
        assert_fails(
            &format!("{label}_luau"),
            "start-script.luau",
            &luau_mod(luau),
            needle,
        );
    }
}

// An unnamed `defineAttackToken` outside a compiled binding declaration
// throws in TypeScript; Luau always names it.
#[test]
fn unnamed_token_declarations_fail_in_both_runtimes() {
    assert_fails(
        "unnamed_js",
        "start-script.js",
        &ts_mod(&Case {
            decls: "const bad = defineAttackToken({ limit: 2 });",
            fields: TS_TOKENS,
            draws: "",
        }),
        "binding-name sugar",
    );
    assert_fails(
        "unnamed_luau",
        "start-script.luau",
        &luau_mod(&Case {
            decls: "local bad = defineAttackToken({ limit = 2 })",
            fields: LUAU_TOKENS,
            draws: "",
        }),
        "defineAttackToken needs its name",
    );
}
