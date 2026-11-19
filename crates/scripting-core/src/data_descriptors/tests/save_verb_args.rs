// Save verbs and `checkpoint`: reaction parsing refuses a bad row or an empty
// name in both runtimes, as a body and as a sequence step.
// See: context/lib/scripting.md §10.4

use super::super::*;

/// Parse one `defineReaction("r", body)` authored through the TypeScript SDK.
fn parse_typescript(body: &str) -> Result<NamedReaction, DescriptorError> {
    let runtime = rquickjs::Runtime::new().unwrap();
    let context = rquickjs::Context::full(&runtime).unwrap();
    context.with(|ctx| {
        crate::quickjs::evaluate_prelude(&ctx).unwrap();
        let value: JsValue = ctx.eval(format!("defineReaction('r', {body})")).unwrap();
        named_reaction_from_js(&ctx, value)
    })
}

/// Parse one `Postretro.defineReaction("r", body)` authored through the Luau
/// SDK, with `UI` bound to `require("postretro/ui")`.
fn parse_luau(body: &str) -> Result<NamedReaction, DescriptorError> {
    let lua = crate::luau::build_lua_state(
        &[],
        None,
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))),
    )
    .unwrap();
    let value: LuaValue = lua
        .load(format!(
            "local Postretro = require('postretro')\nlocal UI = require('postretro/ui')\nreturn Postretro.defineReaction('r', {body})"
        ))
        .eval()
        .unwrap();
    named_reaction_from_lua(value)
}

// Each case: the TypeScript call, the Luau call naming the same row as Luau
// counts it, and the diagnostic each runtime gives, naming verb and value.
const REFUSED: &[(&str, &str, &str, &str)] = &[
    (
        "continueRun(16)",
        "UI.continueRun(17)",
        "`continueRun` row index must be a whole number from 0 to 15, got 16",
        "`continueRun` row index must be a whole number from 1 to 16 (Luau rows count from 1), got 17",
    ),
    (
        "deleteRun(-1)",
        "UI.deleteRun(0)",
        "`deleteRun` row index must be a whole number from 0 to 15, got -1",
        "`deleteRun` row index must be a whole number from 1 to 16 (Luau rows count from 1), got 0",
    ),
    (
        "selectCharacter(1.5)",
        "UI.selectCharacter(1.5)",
        "`selectCharacter` row index must be a whole number from 0 to 15, got 1.5",
        "`selectCharacter` row index must be a whole number from 1 to 16 (Luau rows count from 1), got 1.5",
    ),
    (
        "createCharacter('2')",
        "UI.createCharacter('2')",
        "`createCharacter` row index must be a whole number from 0 to 15, got \"2\"",
        "`createCharacter` row index must be a whole number from 1 to 16 (Luau rows count from 1), got \"2\"",
    ),
    (
        "deleteCharacter(100)",
        "UI.deleteCharacter(101)",
        "`deleteCharacter` row index must be a whole number from 0 to 15, got 100",
        "`deleteCharacter` row index must be a whole number from 1 to 16 (Luau rows count from 1), got 101",
    ),
    (
        "newRun(16, 'e1m1')",
        "UI.newRun(17, 'e1m1')",
        "`newRun` row index must be a whole number from 0 to 15, got 16",
        "`newRun` row index must be a whole number from 1 to 16 (Luau rows count from 1), got 17",
    ),
    (
        "newRun(0, '')",
        "UI.newRun(1, '')",
        "`newRun` level (a catalog map id) must be a non-empty string, got \"\"",
        "`newRun` level (a catalog map id) must be a non-empty string, got \"\"",
    ),
    (
        "checkpoint('')",
        "UI.checkpoint('')",
        "`checkpoint` name must be a non-empty string, got \"\"",
        "`checkpoint` name must be a non-empty string, got \"\"",
    ),
];

#[test]
fn save_verb_parse_refuses_bad_rows_and_empty_names_in_both_runtimes_naming_verb_and_value() {
    for &(typescript, luau, typescript_detail, luau_detail) in REFUSED {
        for (runtime, body, site, detail) in [
            (
                "TypeScript",
                typescript.to_string(),
                "primitive",
                typescript_detail,
            ),
            (
                "TypeScript",
                format!("[{typescript}]"),
                "sequence step 0",
                typescript_detail,
            ),
            ("Luau", luau.to_string(), "primitive", luau_detail),
            (
                "Luau",
                format!("{{ {luau} }}"),
                "sequence step 0",
                luau_detail,
            ),
        ] {
            let parsed = if runtime == "TypeScript" {
                parse_typescript(&body)
            } else {
                parse_luau(&body)
            };
            let error = parsed
                .map(|reaction| format!("parsed: {reaction:?}"))
                .expect_err(&format!("{runtime} `{body}` is refused"))
                .to_string();
            let expected = format!("reaction `r` {site}: {detail}");
            assert!(
                error.contains(&expected),
                "{runtime} `{body}`: expected `{expected}` in `{error}`"
            );
        }
    }
}

// The last valid row in each runtime parses, and crosses 0-based.
#[test]
fn save_verb_parse_accepts_the_last_row_in_both_runtimes() {
    for (runtime, parsed) in [
        (
            "TypeScript",
            parse_typescript("[newRun(15, 'e1m1'), checkpoint('bridge')]"),
        ),
        (
            "Luau",
            parse_luau("{ UI.newRun(16, 'e1m1'), UI.checkpoint('bridge') }"),
        ),
    ] {
        let reaction = parsed.unwrap_or_else(|error| panic!("{runtime}: {error}"));
        let ReactionDescriptor::Sequence(steps) = reaction.descriptor else {
            panic!("{runtime}: an array body lowers to a sequence");
        };
        assert_eq!(steps[0].primitive, "newRun", "{runtime}");
        assert_eq!(steps[0].args["index"].as_f64(), Some(15.0), "{runtime}");
        assert_eq!(steps[0].args["map"], "e1m1", "{runtime}");
        assert_eq!(
            steps[1].args,
            serde_json::json!({ "name": "bridge" }),
            "{runtime}"
        );
    }
}
