-- Accessibility scripting-surface fixture, the Luau mirror of
-- `accessibility-surface-fixture.ts`. Both authorings must produce the same
-- descriptors; `app::accessibility_surface_fixture_tests` runs each.
-- See: context/lib/ui.md §4.1

local UI = require("postretro/ui")

function setupLevel(_ctx: any): LevelManifest
  local state = UI.getGameState()
  local options = state.options
  local accessibility = state.accessibility

  local surface = UI.defineUiTree({
    name = "accessibilitySurfaceFixture",
    tree = UI.Tree({
      anchor = "center",
      offset = { 0, 0 },
      captureMode = "capture",
      initialFocus = "openA11y",
      accessibleName = "Accessibility surface fixture",
      role = "group",
    }, UI.VStack({
      gap = 8,
      padding = 16,
      align = "stretch",
      focus = { policy = "linear", wrap = true },
    }, {
      -- A mod menu entry to the engine panel.
      UI.Button({ id = "openA11y", label = "ACCESSIBILITY", onPress = UI.OPEN_ACCESSIBILITY_ACTION }),
      -- A panel field action on a mod value button: named by its label, its
      -- text shows the field's current value.
      UI.Text({ id = "reduceMotionLabel", content = "REDUCE MOTION" }),
      UI.Button({
        id = "reduceMotion",
        labelledBy = "reduceMotionLabel",
        onPress = UI.accessibilityAction("reduceMotion", "cycle"),
        valueText = {
          {
            when = {
              UI.stateEquals(accessibility.reduceMotionFollowsSystem, true),
              UI.stateEquals(accessibility.reduceMotion, true),
            },
            text = "SYSTEM (ON)",
          },
          {
            when = { UI.stateEquals(accessibility.reduceMotionFollowsSystem, true) },
            text = "SYSTEM (OFF)",
          },
          { when = { UI.stateEquals(accessibility.reduceMotion, true) }, text = "ON" },
          { text = "OFF" },
        },
      }),
      UI.Text({ id = "flashLimiterLabel", content = "FLASH LIMITER" }),
      UI.Button({
        id = "flashLimiter",
        labelledBy = "flashLimiterLabel",
        onPress = UI.accessibilityAction("flashLimiter", "cycle"),
        valueText = {
          { when = { UI.stateEquals(accessibility.flashLimiter, true) }, text = "LIMITER ON" },
          { text = "LIMITER OFF" },
        },
      }),
      UI.Button({
        id = "shakeDown",
        label = "LESS SHAKE",
        onPress = UI.accessibilityAction("screenShakeScale", "decrease"),
      }),
      -- A mod options menu edits the working copy, exactly like existing options.
      UI.Text({ id = "shakeLabel", content = "SCREEN SHAKE" }),
      UI.Slider({
        id = "shake",
        labelledBy = "shakeLabel",
        bind = options.screenShakeScale,
        min = 0,
        max = 1,
        step = 0.1,
        capturesNav = { "nav.left", "nav.right" },
      }),
      -- Content honoring a preference binds the readonly resolved slot; a
      -- menu can show "System" from the source slot.
      UI.Text({
        id = "motionReduced",
        content = "MOTION REDUCED",
        visibleWhen = UI.stateEquals(accessibility.reduceMotion, true),
      }),
      UI.Text({
        id = "followingSystem",
        content = "FOLLOWING SYSTEM",
        visibleWhen = UI.stateEquals(accessibility.reduceMotionFollowsSystem, true),
      }),
    })),
  })

  return { uiTrees = { surface } }
end
