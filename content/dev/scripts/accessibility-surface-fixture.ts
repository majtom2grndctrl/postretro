// Accessibility scripting-surface fixture: a mod menu entry to the engine
// panel, panel field actions on mod value buttons (the flash limiter's
// included), a mod slider on the working copy, and content bound to the
// resolved slot and its source slot.
//
// A level data script, run in both runtimes by
// `app::accessibility_surface_fixture_tests`. No map references it.
// See: context/lib/ui.md §4.1

import {
  Button,
  OPEN_ACCESSIBILITY_ACTION,
  Slider,
  Text,
  Tree,
  VStack,
  accessibilityAction,
  defineUiTree,
  getGameState,
  stateEquals,
  type UiTreeRegistration,
} from "postretro/ui";

export function setupLevel(_ctx: unknown): { uiTrees: UiTreeRegistration[] } {
  const { options, accessibility } = getGameState();

  const surface = defineUiTree({
    name: "accessibilitySurfaceFixture",
    tree: Tree(
      {
        anchor: "center",
        offset: [0, 0],
        captureMode: "capture",
        initialFocus: "openA11y",
        accessibleName: "Accessibility surface fixture",
        role: "group",
      },
      VStack({ gap: 8, padding: 16, align: "stretch", focus: { policy: "linear", wrap: true } }, [
        // A mod menu entry to the engine panel.
        Button({ id: "openA11y", label: "ACCESSIBILITY", onPress: OPEN_ACCESSIBILITY_ACTION }),
        // A panel field action on a mod value button: named by its label, its
        // text shows the field's current value.
        Text({ id: "reduceMotionLabel", content: "REDUCE MOTION" }),
        Button({
          id: "reduceMotion",
          labelledBy: "reduceMotionLabel",
          onPress: accessibilityAction("reduceMotion", "cycle"),
          valueText: [
            {
              when: [
                stateEquals(accessibility.reduceMotionFollowsSystem, true),
                stateEquals(accessibility.reduceMotion, true),
              ],
              text: "SYSTEM (ON)",
            },
            {
              when: [stateEquals(accessibility.reduceMotionFollowsSystem, true)],
              text: "SYSTEM (OFF)",
            },
            { when: [stateEquals(accessibility.reduceMotion, true)], text: "ON" },
            { text: "OFF" },
          ],
        }),
        Text({ id: "flashLimiterLabel", content: "FLASH LIMITER" }),
        Button({
          id: "flashLimiter",
          labelledBy: "flashLimiterLabel",
          onPress: accessibilityAction("flashLimiter", "cycle"),
          valueText: [
            { when: [stateEquals(accessibility.flashLimiter, true)], text: "LIMITER ON" },
            { text: "LIMITER OFF" },
          ],
        }),
        Button({
          id: "shakeDown",
          label: "LESS SHAKE",
          onPress: accessibilityAction("screenShakeScale", "decrease"),
        }),
        // A mod options menu edits the working copy, exactly like existing options.
        Text({ id: "shakeLabel", content: "SCREEN SHAKE" }),
        Slider({
          id: "shake",
          labelledBy: "shakeLabel",
          bind: options.screenShakeScale,
          min: 0,
          max: 1,
          step: 0.1,
          capturesNav: ["nav.left", "nav.right"],
        }),
        // Content honoring a preference binds the readonly resolved slot; a
        // menu can show "System" from the source slot.
        Text({
          id: "motionReduced",
          content: "MOTION REDUCED",
          visibleWhen: stateEquals(accessibility.reduceMotion, true),
        }),
        Text({
          id: "followingSystem",
          content: "FOLLOWING SYSTEM",
          visibleWhen: stateEquals(accessibility.reduceMotionFollowsSystem, true),
        }),
      ]),
    ),
  });

  return { uiTrees: [surface] };
}
