// Session membership verbs across both SDK runtimes, through the committed
// frontend fixture that carries the brief's Scripting surface example.
// See: context/lib/scripting.md §10.4 (Session membership)
use super::*;
use postretro_entities::reactions::system_commands::SystemReactionKind;

const TYPESCRIPT_FIXTURE: &str =
    include_str!("../../../../content/dev/scripts/frontend-level-select-fixture.ts");
const LUAU_FIXTURE: &str =
    include_str!("../../../../content/dev/scripts/frontend-level-select-fixture.luau");

/// The engine kind a lowered step names, read from its `primitive`.
fn step_kind(step: &serde_json::Value) -> Option<SystemReactionKind> {
    SystemReactionKind::from_primitive_name(step["primitive"].as_str()?)
}

// `leaveSession()` and `endSession()` lower in TypeScript and Luau to the
// argumentless steps the engine classifies as those kinds, alone and composed
// with `returnToFrontend()` into a run's end.
#[test]
fn session_membership_fixture_lowers_to_the_engine_kinds_in_both_runtimes() {
    // The bundler moves a default export last, where it would become the
    // script's value; bind it instead so the probe stays the last statement.
    let typescript_module = TYPESCRIPT_FIXTURE.replace("export default ", "const fixtureMod = ");
    assert_ne!(
        typescript_module, TYPESCRIPT_FIXTURE,
        "fixture has a default export"
    );
    let typescript = quickjs_fixture_value(&format!(
        "{typescript_module}\nJSON.stringify({{ leave, runEnd, mod: fixtureMod.name }});"
    ));
    let luau = luau_fixture_value(&format!(
        r#"
        local mod = (function()
        {LUAU_FIXTURE}
        end)()
        local function named(name: string)
          for _, reaction in mod.reactions do
            if reaction.name == name then
              return reaction
            end
          end
          error("fixture registers no reaction " .. name)
        end
        return {{ leave = named("pause.leaveSession"), runEnd = named("run.end") }}
        "#
    ));

    for (runtime, value) in [("TypeScript", &typescript), ("Luau", &luau)] {
        let leave = &value["leave"];
        assert_eq!(leave["name"], "pause.leaveSession", "{runtime}");
        assert_eq!(
            step_kind(leave),
            Some(SystemReactionKind::LeaveSession),
            "{runtime}: {leave}"
        );
        let run_end = &value["runEnd"];
        assert_eq!(run_end["name"], "run.end", "{runtime}");
        let steps = run_end["sequence"]
            .as_array()
            .unwrap_or_else(|| panic!("{runtime}: an array body lowers to a sequence: {run_end}"));
        assert_eq!(
            steps.iter().map(step_kind).collect::<Vec<_>>(),
            [
                Some(SystemReactionKind::EndSession),
                Some(SystemReactionKind::ReturnToFrontend),
            ],
            "{runtime}: {run_end}"
        );
        for step in std::iter::once(leave).chain(steps) {
            let args = &step["args"];
            assert!(
                args.as_object().is_some_and(|args| args.is_empty())
                    || args.as_array().is_some_and(|args| args.is_empty()),
                "{runtime}: membership and navigation steps carry no args: {step}"
            );
        }
    }
}
