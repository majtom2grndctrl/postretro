use serde_json::{Value, json};

use super::*;

fn token(name: &str, budget: Value) -> Value {
    json!({ "kind": "attackToken", "name": name, "budget": budget })
}

fn handle(name: &str) -> Value {
    json!({ "kind": "attackToken", "name": name })
}

fn draws(list: Value) -> Vec<TokenDraw> {
    list.as_array()
        .expect("a list")
        .iter()
        .cloned()
        .map(TokenDraw)
        .collect()
}

fn melee_heavy() -> AttackBudgetVocabulary {
    AttackBudgetVocabulary::from_manifest(
        Some(&json!([
            token("melee", json!({ "limit": 3 })),
            token("heavy", json!({ "limit": 1, "refreshMs": 4000 })),
        ])),
        None,
    )
    .expect("a valid registration")
}

// `defaultToken` is id 0 in every registration and declares the SDK default
// budget; a mod's tokens follow in `attackTokens` order with their declared
// budgets, and an omitted field takes `defineAttackToken`'s default.
#[test]
fn registration_puts_default_token_first_and_mod_tokens_in_list_order() {
    let vocabulary = AttackBudgetVocabulary::from_manifest(
        Some(&json!([
            token("melee", json!({ "limit": 4 })),
            token("heavy", json!({ "refreshMs": 4000 })),
            token("ranged", json!({})),
        ])),
        None,
    )
    .unwrap();
    let names: Vec<&str> = vocabulary
        .tokens()
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(names, ["default", "melee", "heavy", "ranged"]);
    let budgets: Vec<TokenBudget> = vocabulary.declared_budgets().collect();
    assert_eq!(budgets[0], TokenBudget::DEFAULT);
    assert_eq!(
        budgets[1],
        TokenBudget {
            limit: 4,
            refresh_ms: 0.0
        }
    );
    assert_eq!(
        budgets[2],
        TokenBudget {
            limit: 3,
            refresh_ms: 4000.0
        }
    );
    assert_eq!(budgets[3], TokenBudget::DEFAULT);
    assert_eq!(
        AttackBudgetVocabulary::from_manifest(None, None).unwrap(),
        AttackBudgetVocabulary::default(),
        "a mod declaring nothing still has `defaultToken`"
    );
}

// An attack without `draws` draws one `defaultToken`; `draws: []` draws
// nothing; a reference's `cost` defaults to 1.
#[test]
fn draws_resolve_to_token_ids_and_costs() {
    let vocabulary = melee_heavy();
    let mut unresolved = Vec::new();
    assert_eq!(vocabulary.resolve_draws(None, |_| {}), [(0, 1)]);
    assert_eq!(vocabulary.resolve_draws(Some(&[]), |_| {}), []);
    let slam = draws(json!([
        { "token": handle("melee"), "cost": 2 },
        handle("heavy"),
        handle("default"),
    ]));
    assert_eq!(
        vocabulary.resolve_draws(Some(&slam), |name| unresolved.push(name)),
        [(1, 2), (2, 1), (0, 1)]
    );
    assert!(unresolved.is_empty());
    let stale = draws(json!([handle("lobber"), handle("melee")]));
    assert_eq!(
        vocabulary.resolve_draws(Some(&stale), |name| unresolved.push(name)),
        [(1, 1)]
    );
    assert_eq!(unresolved, ["lobber"]);
}

// Luau writes `draws = {}` as an empty table, which crosses as `{}`.
#[test]
fn empty_luau_draws_table_reads_as_an_empty_list() {
    let attack: crate::AttackParams = serde_json::from_value(json!({
        "damage": 1, "maxRange": 2, "cooldownMs": 100, "draws": {}
    }))
    .unwrap();
    assert_eq!(attack.draws, Some(Vec::new()));
    let attack: crate::AttackParams =
        serde_json::from_value(json!({ "damage": 1, "maxRange": 2, "cooldownMs": 100 })).unwrap();
    assert_eq!(attack.draws, None);
}

// A profile override changes only the fields it names; tokens it leaves out
// keep their declared budgets. `default` without a declaration is the
// declared budgets, and an undeclared profile resolves to nothing.
#[test]
fn profile_budgets_override_only_named_fields() {
    let vocabulary = AttackBudgetVocabulary::from_manifest(
        Some(&json!([
            token("melee", json!({ "limit": 3, "refreshMs": 500 })),
            token("heavy", json!({ "limit": 1, "refreshMs": 4000 })),
        ])),
        Some(&json!({
            "duel": [{ "token": handle("melee"), "limit": 1 }],
            "calm": [{ "token": handle("default"), "limit": 1 }, handle("heavy")],
        })),
    )
    .unwrap();
    let duel = vocabulary.profile_budgets("duel").unwrap();
    assert_eq!(
        duel[1],
        TokenBudget {
            limit: 1,
            refresh_ms: 500.0
        }
    );
    assert_eq!(
        duel[2],
        TokenBudget {
            limit: 1,
            refresh_ms: 4000.0
        }
    );
    let calm = vocabulary.profile_budgets("calm").unwrap();
    assert_eq!(
        calm[0],
        TokenBudget {
            limit: 1,
            refresh_ms: 0.0
        },
        "a profile overrides defaultToken"
    );
    assert_eq!(
        vocabulary.profile_budgets("default").unwrap(),
        vocabulary.declared_budgets().collect::<Vec<_>>()
    );
    assert_eq!(vocabulary.profile_budgets("frenzy"), None);
}

// Row A8 (token half) / B19: every registration and reference rule rejects
// with a key path. `(attackTokens, attackBudgets, expected path, expected text)`.
#[test]
fn attack_budget_registration_rejects_with_pathed_errors() {
    let melee = || token("melee", json!({}));
    let cases: Vec<(Value, Value, &str, &str)> = vec![
        (
            json!([token("melee", json!({ "limit": 0 }))]),
            json!(null),
            "attackTokens[0].budget.limit",
            "integer of at least 1",
        ),
        (
            json!([token("melee", json!({ "limit": 1.5 }))]),
            json!(null),
            "attackTokens[0].budget.limit",
            "integer of at least 1",
        ),
        (
            json!([token("melee", json!({ "refreshMs": -1 }))]),
            json!(null),
            "attackTokens[0].budget.refreshMs",
            "finite number of at least 0",
        ),
        (
            json!([token("melee", json!({ "cost": 2 }))]),
            json!(null),
            "attackTokens[0].budget.cost",
            "belongs in an attack's draw",
        ),
        (
            json!([token("melee", json!({ "speed": 2 }))]),
            json!(null),
            "attackTokens[0].budget.speed",
            "takes no field",
        ),
        (
            json!([melee(), melee()]),
            json!(null),
            "attackTokens[1]",
            "registered twice",
        ),
        (
            json!([token("default", json!({}))]),
            json!(null),
            "attackTokens[0]",
            "never lists it",
        ),
        (
            json!([handle("default")]),
            json!(null),
            "attackTokens[0]",
            "never lists it",
        ),
        (
            json!(["melee"]),
            json!(null),
            "attackTokens[0]",
            "bare string",
        ),
        (
            json!([{ "kind": "damageType", "name": "shock" }]),
            json!(null),
            "attackTokens[0]",
            "got a `damageType` handle",
        ),
        (
            json!([melee()]),
            json!({ "duel": [{ "token": handle("melee"), "cost": 2 }] }),
            "attackBudgets.duel[0].cost",
            "belongs in an attack's draw",
        ),
        (
            json!([melee()]),
            json!({ "duel": [{ "token": handle("melee"), "limit": 0 }] }),
            "attackBudgets.duel[0].limit",
            "integer of at least 1",
        ),
        (
            json!([melee()]),
            json!({ "duel": [{ "token": handle("melee"), "refreshMs": -5 }] }),
            "attackBudgets.duel[0].refreshMs",
            "finite number",
        ),
        (
            json!([melee()]),
            json!({ "duel": [handle("melee"), { "token": handle("melee"), "limit": 1 }] }),
            "attackBudgets.duel[1]",
            "overridden twice",
        ),
        (
            json!([melee()]),
            json!({ "duel": ["melee"] }),
            "attackBudgets.duel[0]",
            "bare string",
        ),
        (
            json!([melee()]),
            json!({ "duel": [{ "kind": "damageType", "name": "melee" }] }),
            "attackBudgets.duel[0]",
            "expected an attack token handle, got damage type",
        ),
        (
            json!([melee()]),
            json!({ "duel": [handle("heavy")] }),
            "attackBudgets.duel[0]",
            "not registered",
        ),
        (
            json!([melee()]),
            json!({ "duel": [{ "token": "melee", "limit": 1 }] }),
            "attackBudgets.duel[0].token",
            "bare string",
        ),
    ];
    for (tokens, budgets, path, text) in cases {
        let error = AttackBudgetVocabulary::from_manifest(Some(&tokens), Some(&budgets))
            .expect_err(&format!("{tokens} / {budgets} must reject"));
        assert!(
            error.contains(&format!("`{path}")) && error.contains(text),
            "expected `{path}` … {text}, got: {error}"
        );
    }
}

// Row A8 (token half): a draw's own rules, registry-free (both runtimes run
// them in descriptor validation), and registration's.
#[test]
fn attack_draws_reject_with_pathed_errors() {
    let path = "components.behavior.attacks.slam.draws";
    let shape_cases: Vec<(Value, &str, &str)> = vec![
        (
            json!([{ "token": handle("melee"), "cost": 0 }]),
            "[0].cost",
            "integer of at least 1",
        ),
        (
            json!([{ "token": handle("melee"), "cost": 1.5 }]),
            "[0].cost",
            "integer of at least 1",
        ),
        (
            json!([{ "token": handle("melee"), "limit": 1 }]),
            "[0].limit",
            "belongs in a profile override",
        ),
        (
            json!([{ "token": handle("melee"), "refreshMs": 1 }]),
            "[0].refreshMs",
            "belongs in a profile override",
        ),
        (
            json!([handle("melee"), { "token": handle("melee"), "cost": 2 }]),
            "[1]",
            "drawn twice",
        ),
        (json!(["melee"]), "[0]", "bare string"),
        (
            json!([{ "kind": "defenseLayer", "name": "shield" }]),
            "[0]",
            "got defense layer",
        ),
        (
            json!([{ "token": handle("melee"), "weight": 1 }]),
            "[0].weight",
            "no field",
        ),
        (
            json!(
                (0..=MAX_ATTACK_DRAWS)
                    .map(|i| handle(&format!("t{i}")))
                    .collect::<Vec<_>>()
            ),
            "",
            "at most 8",
        ),
    ];
    for (list, suffix, text) in shape_cases {
        let error = check_draw_shapes(path, &draws(list.clone()))
            .expect_err(&format!("{list} must reject"));
        assert!(
            error.contains(&format!("`{path}{suffix}`")) && error.contains(text),
            "expected `{path}{suffix}` … {text}, got: {error}"
        );
    }
    let vocabulary = melee_heavy();
    let error = vocabulary
        .check_draws(
            path,
            Some(&draws(json!([handle("melee"), handle("lobber")]))),
        )
        .unwrap_err();
    assert!(
        error.contains(&format!("`{path}[1]`")) && error.contains("not registered"),
        "{error}"
    );
    vocabulary
        .check_draws(path, Some(&draws(json!([{ "token": handle("melee"), "cost": 2 }, handle("heavy"), handle("default")]))))
        .expect("registered tokens and `defaultToken` resolve");
    vocabulary
        .check_draws(path, None)
        .expect("no draws draws `defaultToken`");
}
