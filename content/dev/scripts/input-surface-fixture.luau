-- Input scripting-surface fixture, the Luau mirror of
-- `input-surface-fixture.ts`. Both authorings must produce the same
-- descriptors; `app::input_surface_fixture_tests` runs each.
-- See: docs/scripting-reference.md (Glyphs, scroll, the controls panel)

local UI = require("postretro/ui")

local LEVELS = { "e1m1", "e1m2", "e1m3", "e1m4", "e1m5", "e1m6", "e1m7", "e1m8", "e1m9", "e1m10" }

function setupLevel(_ctx: any): LevelManifest
  local levelButtons = {}
  for _, id in LEVELS do
    table.insert(
      levelButtons,
      UI.Button({ id = `level_{id}`, label = string.upper(id), onPress = `fixture.start.{id}` })
    )
  end
  local surface = UI.defineUiTree({
    name = "inputSurfaceFixture",
    tree = UI.Tree({
      anchor = "center",
      offset = { 0, 0 },
      captureMode = "capture",
      initialFocus = "controls",
      accessibleName = "Input surface fixture",
      role = "group",
    }, UI.VStack({ gap = 8, padding = 16, align = "stretch", focus = { policy = "linear" } }, {
      UI.HStack({ gap = 8 }, { UI.Glyph({ command = "nav_confirm" }), UI.Text({ content = "SELECT" }) }),
      UI.VStack({ scroll = { maxHeight = 320 }, focus = { policy = "linear" } }, levelButtons),
      UI.Button({ id = "controls", label = "CONTROLS", onPress = UI.OPEN_CONTROLS_ACTION }),
    })),
  })
  return { uiTrees = { surface } }
end
