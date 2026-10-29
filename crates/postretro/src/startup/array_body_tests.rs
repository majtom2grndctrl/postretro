// Array bodies through the real SDK in both runtimes: an array is a
// `{ sequence }`, nested arrays flatten at any depth, an empty one is inert,
// and a body that is not a step raises at author time naming `defineReaction`.
// See: context/lib/scripting.md §12 (Body shapes)

use super::*;

fn ts(source: &str) -> Vec<NamedReaction> {
    sdk_reactions_in_runtime(source, false)
}
fn luau(source: &str) -> Vec<NamedReaction> {
    sdk_reactions_in_runtime(source, true)
}
fn bodies(reactions: &[NamedReaction]) -> Vec<(&str, &ReactionDescriptor)> {
    reactions
        .iter()
        .map(|reaction| (reaction.name.as_str(), &reaction.descriptor))
        .collect()
}

// S21: an array with nested arrays, and a `sequence` list with nested arrays,
// lower to exactly the flat `sequence` they spell, in TS and Luau alike.
#[test]
fn s21_nested_arrays_lower_to_the_flat_sequence_in_both_runtimes() {
    let from_ts = ts(r#"
        import { defineReaction, wait } from "postretro";
        import { playSound } from "postretro/ui";
        export function setupLevel() {
          const a = playSound("a"), b = playSound("b"), c = playSound("c");
          return { reactions: [
            defineReaction("array", [a, [b, wait(5)], c]),
            defineReaction("arrayFlat", { sequence: [a, b, ...wait(5), c] }),
            defineReaction("nested", { sequence: [a, [b, [c]]] }),
            defineReaction("nestedFlat", { sequence: [a, b, c] }),
          ] };
        }
    "#);
    let from_luau = luau(r#"
        local Postretro = require("postretro")
        local UI = require("postretro/ui")
        function setupLevel()
          local a, b, c = UI.playSound("a"), UI.playSound("b"), UI.playSound("c")
          local flatWait = Postretro.wait(5)[1]
          return { reactions = {
            Postretro.defineReaction("array", { a, { b, Postretro.wait(5) }, c }),
            Postretro.defineReaction("arrayFlat", { sequence = { a, b, flatWait, c } }),
            Postretro.defineReaction("nested", { sequence = { a, { b, { c } } } }),
            Postretro.defineReaction("nestedFlat", { sequence = { a, b, c } }),
          } }
        end
    "#);
    assert_eq!(from_ts.len(), 4, "every reaction parses");
    assert_eq!(from_ts, from_luau, "the runtimes lower identically");
    let by_name = bodies(&from_ts);
    assert_eq!(by_name[0].1, by_name[1].1, "array body = its flat sequence");
    assert_eq!(by_name[2].1, by_name[3].1, "nested sequence list = its flat list");
    let ReactionDescriptor::Sequence(steps) = by_name[0].1 else {
        panic!("an array lowers to a sequence");
    };
    assert_eq!(
        steps.iter().map(|step| step.id.clone()).collect::<Vec<_>>(),
        [
            SequenceTarget::System,
            SequenceTarget::System,
            SequenceTarget::Wait,
            SequenceTarget::System,
        ]
    );
}

// S21, the flat half: already-flat content lowers to the object the SDK
// emitted before arrays existed, key for key, so its manifest data and its
// auto id are unchanged.
#[test]
fn already_flat_bodies_emit_the_same_descriptor_in_both_runtimes() {
    let from_ts = ts(r#"
        import { defineReaction, npcs, wait } from "postretro";
        import { playSound } from "postretro/ui";
        export function setupLevel() {
          const bodies = [
            { sequence: [playSound("a"), ...wait(5), npcs({ tag: "m" }).update({ aggro: true })] },
            playSound("b"),
            { progress: { tag: "m", at: 0.5, fire: "half" } },
          ];
          const reactions = bodies.map((body) => {
            const lowered = defineReaction("same", body as any);
            const before = { name: "same", ...body };
            if (JSON.stringify(lowered) !== JSON.stringify(before)) {
              throw new Error(`lowering changed ${JSON.stringify(before)}`);
            }
            const auto = defineReaction(body as any);
            if (auto.name !== defineReaction(JSON.parse(JSON.stringify(body))).name) {
              throw new Error("auto id moved");
            }
            return lowered;
          });
          return { reactions };
        }
    "#);
    assert_eq!(from_ts.len(), 3, "no body changed shape");
    let from_luau = luau(r#"
        local Postretro = require("postretro")
        local UI = require("postretro/ui")
        local function same(a, b)
          for key, value in pairs(a) do
            if b[key] ~= value then return false end
          end
          for key in pairs(b) do
            if a[key] == nil then return false end
          end
          return true
        end
        function setupLevel()
          local sequence = { UI.playSound("a"), Postretro.wait(5)[1] }
          local lowered = Postretro.defineReaction("same", { sequence = sequence })
          if #lowered.sequence ~= 2 or not same(lowered.sequence[1], sequence[1]) then
            error("lowering changed a flat sequence")
          end
          local first = Postretro.defineReaction({ sequence = sequence })
          local second = Postretro.defineReaction({ sequence = { sequence[1], sequence[2] } })
          if first.name ~= second.name then error("auto id moved") end
          return { reactions = { lowered } }
        end
    "#);
    assert_eq!(from_luau.len(), 1);
}

// AC 18: a tracer may return an array or a `{ sequence }`; each array form
// fires as its sequence equivalent. An empty array and an empty Luau table
// are inert reactions that install quietly.
#[test]
fn tracer_arrays_and_empty_bodies_install_as_sequences_in_both_runtimes() {
    let from_ts = ts(r#"
        import { defineReaction, type TriggerEventParams } from "postretro";
        import { playSound } from "postretro/ui";
        export function setupLevel() {
          return { reactions: [
            defineReaction("array", (on: TriggerEventParams) => [on.activators.grantHealth(25), playSound("medkit")]),
            defineReaction("sequence", (on: TriggerEventParams) => ({
              sequence: [on.activators.grantHealth(25), playSound("medkit")],
            })),
            defineReaction("empty", []),
          ] };
        }
    "#);
    let from_luau = luau(r#"
        local Postretro = require("postretro")
        local UI = require("postretro/ui")
        function setupLevel()
          return { reactions = {
            Postretro.defineReaction("array", function(on) return { on.activators:grantHealth(25), UI.playSound("medkit") } end),
            Postretro.defineReaction("sequence", function(on)
              return { sequence = { on.activators:grantHealth(25), UI.playSound("medkit") } }
            end),
            Postretro.defineReaction("empty", {}),
          } }
        end
    "#);
    assert_eq!(from_ts, from_luau);
    let by_name = bodies(&from_ts);
    assert_eq!(by_name[0].1, by_name[1].1, "the array fires as its sequence");
    assert_eq!(by_name[2].1, &ReactionDescriptor::Sequence(Vec::new()));

    // Both install, by name, trigger and after a wait, with nothing above
    // debug for the empty body.
    let capture = LogCapture::start();
    let ctx = ctx_with_reactions(from_ts);
    let table = validated_bindings(&ctx);
    assert!(!is_dropped(&ctx, "array"));
    assert!(capture
        .records()
        .iter()
        .all(|record| record.level > log::Level::Info || !record.message.contains("empty")));
    drop(table);
}

// AC 19: a body that is not a step raises at author time in both runtimes,
// naming `defineReaction`; a reaction handle's error suggests `fire`. A
// map-shaped entry, a command target and a misspelled body key raise the same
// text in both runtimes instead of installing an empty or missing reaction.
#[test]
fn non_step_entries_raise_naming_define_reaction_in_both_runtimes() {
    let cases = [
        (
            "[{ progress: { tag: \"m\", at: 1, fire: \"done\" } }]",
            "{ { progress = { tag = \"m\", at = 1, fire = \"done\" } } }",
            "defineReaction: a `progress` body is not a step",
        ),
        (
            "[{ sequence: [playSound(\"a\")] }]",
            "{ { sequence = { UI.playSound(\"a\") } } }",
            "defineReaction: a nested `{ sequence }` is not a step",
        ),
        (
            "[defineReaction(\"closet\", playSound(\"a\"))]",
            "{ Postretro.defineReaction(\"closet\", UI.playSound(\"a\")) }",
            "defineReaction: reaction `closet` is a reaction handle, not a step; fire it with `fire(r)`",
        ),
        (
            "[{ primitive: \"setState\", args: {}, onComplete: \"next\" }]",
            "{ { primitive = \"setState\", args = {}, onComplete = \"next\" } }",
            "defineReaction: step `setState` carries `onComplete`, which only a body may",
        ),
        (
            "[{ foo: 1 }]",
            "{ { foo = 1 } }",
            "defineReaction: an entry with no `primitive` is not a step",
        ),
        (
            "[playSound(\"a\"), npcs()]",
            "{ UI.playSound(\"a\"), Postretro.npcs() }",
            "defineReaction: an entry with no `primitive` is not a step",
        ),
        (
            "npcs()",
            "Postretro.npcs()",
            "defineReaction: a body must be `progress`, `primitive`, `sequence` or a list of steps",
        ),
        (
            "{ sequnce: [playSound(\"a\")] }",
            "{ sequnce = { UI.playSound(\"a\") } }",
            "defineReaction: a body must be `progress`, `primitive`, `sequence` or a list of steps",
        ),
        (
            "{ sequence: { foo: 1 } }",
            "{ sequence = { foo = 1 } }",
            "defineReaction: `sequence` must be a list of steps",
        ),
        (
            "Object.assign([playSound(\"a\")], { primitive: \"setState\" })",
            "{ UI.playSound(\"a\"), primitive = \"setState\" }",
            "defineReaction: a list of steps cannot also carry `primitive`",
        ),
    ];
    for (ts_body, luau_body, expected) in cases {
        let capture = LogCapture::start();
        let reactions = ts(&format!(
            r#"
            import {{ defineReaction, npcs }} from "postretro";
            import {{ playSound }} from "postretro/ui";
            export function setupLevel() {{
              return {{ reactions: [defineReaction("bad", ({ts_body}) as any)] }};
            }}
            "#
        ));
        assert!(reactions.is_empty(), "QuickJS: {expected}");
        assert!(
            capture.records().iter().any(|record| record.message.contains(expected)),
            "QuickJS raised `{expected}`: {:?}",
            capture.records()
        );
        drop(capture);

        let capture = LogCapture::start();
        let reactions = luau(&format!(
            r#"
            local Postretro = require("postretro")
            local UI = require("postretro/ui")
            function setupLevel()
              return {{ reactions = {{ Postretro.defineReaction("bad", {luau_body}) }} }}
            end
            "#
        ));
        assert!(reactions.is_empty(), "Luau: {expected}");
        assert!(
            capture.records().iter().any(|record| record.message.contains(expected)),
            "Luau raised `{expected}`: {:?}",
            capture.records()
        );
    }
}
