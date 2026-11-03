// The presentation-overlay authoring surface in both SDK runtimes: the
// `assistableTeammates()` source, template glyphs and the optional anchor socket.
// See: context/lib/ui.md §1 (presentation layer)
use super::*;

/// The marker example lowers byte-identically in TypeScript and Luau: the
/// source is a bare kind tag, a socket-less anchor omits `socket`, and the
/// glyph sits nested inside a stack.
#[test]
fn assistable_teammates_overlay_with_glyph_template_matches_across_runtimes() {
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { Glyph, HStack, Text, VStack, assistableTeammates, defineOverlay,
          definePresentationTemplate, fact } from "postretro/ui";
        const reviveMarker = definePresentationTemplate({
          root: VStack({ gap: 2 }, [
            Text({ content: "DOWNED", fontSize: 14, color: [1, 0.3, 0.3, 1] }),
            HStack({ gap: 4, visibleWhen: fact.bool("inRange") }, [
              Text({ content: "Hold", fontSize: 14, color: [1, 1, 1, 1] }),
              Glyph({ command: "assist" }),
              Text({ content: "to revive", fontSize: 14, color: [1, 1, 1, 1] }),
            ]),
          ]),
          lifetimeMs: 1, motion: { rise: 0, easing: "linear" }, fade: { startMs: 1 },
          spawnScatter: { radius: 0 },
          worldAnchor: { offsetY: 0.6 },
        });
        const glyphRoot = definePresentationTemplate({
          root: Glyph({ command: "assist" }),
          lifetimeMs: 1, motion: { rise: 0, easing: "linear" }, fade: { startMs: 1 },
          spawnScatter: { radius: 0 }, worldAnchor: { offsetY: 0 },
        });
        const overlay = defineOverlay({ over: assistableTeammates(), template: reviveMarker, maxVisible: 8 });
        JSON.stringify({ template: reviveMarker, glyphRoot: glyphRoot.root, overlay });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local UI = require("postretro/ui")
        local reviveMarker = UI.definePresentationTemplate("reviveMarker", {
          root = UI.VStack({ gap = 2 }, {
            UI.Text({ content = "DOWNED", fontSize = 14, color = { 1, 0.3, 0.3, 1 } }),
            UI.HStack({ gap = 4, visibleWhen = UI.fact.bool("inRange") }, {
              UI.Text({ content = "Hold", fontSize = 14, color = { 1, 1, 1, 1 } }),
              UI.Glyph({ command = "assist" }),
              UI.Text({ content = "to revive", fontSize = 14, color = { 1, 1, 1, 1 } }),
            }),
          }),
          lifetimeMs = 1, motion = { rise = 0, easing = "linear" }, fade = { startMs = 1 },
          spawnScatter = { radius = 0 },
          worldAnchor = { offsetY = 0.6 },
        })
        local glyphRoot = UI.definePresentationTemplate("glyphRoot", {
          root = UI.Glyph({ command = "assist" }),
          lifetimeMs = 1, motion = { rise = 0, easing = "linear" }, fade = { startMs = 1 },
          spawnScatter = { radius = 0 }, worldAnchor = { offsetY = 0 },
        })
        local overlay = UI.defineOverlay({ over = UI.assistableTeammates(), template = reviveMarker, maxVisible = 8 })
        return { template = reviveMarker, glyphRoot = glyphRoot.root, overlay = overlay }
    "#;
    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(typescript, luau);
    assert_eq!(
        typescript["overlay"],
        serde_json::json!({
            "over": { "kind": "assistableTeammates" },
            "template": "reviveMarker",
            "maxVisible": 8,
        })
    );
    assert_eq!(
        typescript["template"]["worldAnchor"],
        serde_json::json!({ "offsetY": 0.6 })
    );
    assert_eq!(
        typescript["glyphRoot"],
        serde_json::json!({ "kind": "glyph", "command": "assist" })
    );

    let template: crate::data_descriptors::PresentationTemplate =
        serde_json::from_value(typescript["template"].clone()).unwrap();
    template
        .validate()
        .expect("the Rust template validator admits nested glyphs");
}

/// Rejections both runtimes share: options passed to `assistableTeammates`,
/// the overlay checks that already guarded `damagedEnemies`, an empty glyph
/// command, an empty socket, and every kind templates still refuse.
#[test]
fn overlay_and_template_rejections_match_across_runtimes() {
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { Glyph, Spacer, VStack, assistableTeammates, defineOverlay,
          definePresentationTemplate } from "postretro/ui";
        const message = (build: () => unknown): string => {
          try { build(); return "accepted"; } catch (error) { return String((error as Error).message); }
        };
        const props = (root: unknown, worldAnchor?: unknown) => ({
          root, lifetimeMs: 1, motion: { rise: 0, easing: "linear" }, fade: { startMs: 1 },
          spawnScatter: { radius: 0 }, ...(worldAnchor === undefined ? {} : { worldAnchor }),
        });
        const text = { kind: "text", content: "x", fontSize: 12, color: [1, 1, 1, 1] };
        const anchored = (definePresentationTemplate as any)("anchored", props(text, { offsetY: 0 }));
        const unanchored = (definePresentationTemplate as any)("unanchored", props(text));
        JSON.stringify({
          sourceKey: message(() => (assistableTeammates as any)({ radius: 2 })),
          sourceNonObject: message(() => (assistableTeammates as any)(3)),
          sourceEmpty: message(() => (assistableTeammates as any)({})),
          capLow: message(() => defineOverlay({ over: assistableTeammates(), template: anchored, maxVisible: 0 })),
          capHigh: message(() => defineOverlay({ over: assistableTeammates(), template: anchored, maxVisible: 65 })),
          noAnchor: message(() => defineOverlay({ over: assistableTeammates(), template: unanchored, maxVisible: 1 })),
          badSource: message(() => defineOverlay({ over: { kind: "players" } as any, template: anchored, maxVisible: 1 })),
          emptyCommand: message(() => (definePresentationTemplate as any)("glyph", props({ kind: "glyph", command: "" }))),
          nestedEmptyCommand: message(() => (definePresentationTemplate as any)("glyph", props(VStack({}, [{ kind: "glyph", command: "" } as any])))),
          emptySocket: message(() => (definePresentationTemplate as any)("socket", props(text, { socket: "", offsetY: 0 }))),
          spacer: message(() => (definePresentationTemplate as any)("spacer", props(Spacer({ flexGrow: 1 })))),
          button: message(() => (definePresentationTemplate as any)("button", props({ kind: "button", label: "x" }))),
          glyph: message(() => (definePresentationTemplate as any)("glyph", props(Glyph({ command: "assist" })))),
        });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local UI = require("postretro/ui")
        local function message(build)
          local ok, err = pcall(build)
          if ok then return "accepted" end
          return (string.gsub(tostring(err), "^[^:]+:%d+: ", ""))
        end
        local function props(root, worldAnchor)
          return { root = root, lifetimeMs = 1, motion = { rise = 0, easing = "linear" },
            fade = { startMs = 1 }, spawnScatter = { radius = 0 }, worldAnchor = worldAnchor }
        end
        local text = { kind = "text", content = "x", fontSize = 12, color = { 1, 1, 1, 1 } }
        local anchored = UI.definePresentationTemplate("anchored", props(text, { offsetY = 0 }))
        local unanchored = UI.definePresentationTemplate("unanchored", props(text))
        return {
          sourceKey = message(function() return UI.assistableTeammates({ radius = 2 }) end),
          sourceNonObject = message(function() return UI.assistableTeammates(3) end),
          sourceEmpty = message(function() return UI.assistableTeammates({}) end),
          capLow = message(function() return UI.defineOverlay({ over = UI.assistableTeammates(), template = anchored, maxVisible = 0 }) end),
          capHigh = message(function() return UI.defineOverlay({ over = UI.assistableTeammates(), template = anchored, maxVisible = 65 }) end),
          noAnchor = message(function() return UI.defineOverlay({ over = UI.assistableTeammates(), template = unanchored, maxVisible = 1 }) end),
          badSource = message(function() return UI.defineOverlay({ over = { kind = "players" }, template = anchored, maxVisible = 1 }) end),
          emptyCommand = message(function() return UI.definePresentationTemplate("glyph", props({ kind = "glyph", command = "" })) end),
          nestedEmptyCommand = message(function() return UI.definePresentationTemplate("glyph", props(UI.VStack({}, { { kind = "glyph", command = "" } }))) end),
          emptySocket = message(function() return UI.definePresentationTemplate("socket", props(text, { socket = "", offsetY = 0 })) end),
          spacer = message(function() return UI.definePresentationTemplate("spacer", props(UI.Spacer({ flexGrow = 1 }))) end),
          button = message(function() return UI.definePresentationTemplate("button", props({ kind = "button", label = "x" })) end),
          glyph = message(function() return UI.definePresentationTemplate("glyph", props(UI.Glyph({ command = "assist" }))) end),
        }
    "#;
    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    let expected = serde_json::json!({
        "sourceKey": "assistableTeammates: unknown key `radius`",
        "sourceNonObject": "assistableTeammates: takes no options",
        "sourceEmpty": "accepted",
        "capLow": "defineOverlay: `maxVisible` must be an integer between 1 and 64",
        "capHigh": "defineOverlay: `maxVisible` must be an integer between 1 and 64",
        "noAnchor": "defineOverlay: `template.worldAnchor` is required for an overlay",
        "badSource": "defineOverlay: `over` must come from damagedEnemies or assistableTeammates",
        "emptyCommand": "definePresentationTemplate: `root.command` must be nonempty",
        "nestedEmptyCommand": "definePresentationTemplate: `root.children[0].command` must be nonempty",
        "emptySocket": "definePresentationTemplate: `worldAnchor.socket` must be nonempty",
        "spacer": "definePresentationTemplate: `root.kind` \"spacer\" is not supported; passive templates allow text, bar, image, glyph, vstack, and hstack",
        "button": "definePresentationTemplate: `root.kind` \"button\" is not supported; passive templates allow text, bar, image, glyph, vstack, and hstack",
        "glyph": "accepted",
    });
    assert_eq!(typescript, expected);
    // Luau paths are 1-based; every other message is identical.
    let mut luau_expected = expected;
    luau_expected["nestedEmptyCommand"] =
        "definePresentationTemplate: `root.children[1].command` must be nonempty".into();
    assert_eq!(luau, luau_expected);
}
