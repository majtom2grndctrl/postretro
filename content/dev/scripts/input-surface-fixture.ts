// Input scripting-surface fixture: a Glyph beside its prompt, a scrolling
// list of level buttons, and a menu entry to the engine controls panel.
//
// A level data script, run in both runtimes by
// `app::input_surface_fixture_tests`. No map references it.
// See: docs/scripting-reference.md (Glyphs, scroll, the controls panel)

import {
  Button,
  Glyph,
  HStack,
  OPEN_CONTROLS_ACTION,
  Text,
  Tree,
  VStack,
  defineUiTree,
  type UiTreeRegistration,
} from "postretro/ui";

const LEVELS = ["e1m1", "e1m2", "e1m3", "e1m4", "e1m5", "e1m6", "e1m7", "e1m8", "e1m9", "e1m10"];

export function setupLevel(_ctx: unknown): { uiTrees: UiTreeRegistration[] } {
  const levelButtons = LEVELS.map((id) =>
    Button({ id: `level_${id}`, label: id.toUpperCase(), onPress: `fixture.start.${id}` }),
  );
  const surface = defineUiTree({
    name: "inputSurfaceFixture",
    tree: Tree(
      {
        anchor: "center",
        offset: [0, 0],
        captureMode: "capture",
        initialFocus: "controls",
        accessibleName: "Input surface fixture",
        role: "group",
      },
      VStack({ gap: 8, padding: 16, align: "stretch", focus: { policy: "linear" } }, [
        HStack({ gap: 8 }, [Glyph({ command: "nav_confirm" }), Text({ content: "SELECT" })]),
        VStack({ scroll: { maxHeight: 320 }, focus: { policy: "linear" } }, levelButtons),
        Button({ id: "controls", label: "CONTROLS", onPress: OPEN_CONTROLS_ACTION }),
      ]),
    ),
  });
  return { uiTrees: [surface] };
}
