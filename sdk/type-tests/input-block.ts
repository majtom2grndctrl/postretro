import type { CommandId, ModInput, ModManifest } from "postretro";
import { defineMod } from "postretro";

// The brief's Scripting-surface example (input block only).
const manifest: ModManifest = defineMod({
  name: "Neon",
  id: "acme.neon",
  version: "1",
  input: {
    commands: {
      dash: {
        label: "Dash", category: "Movement", order: 30,
        keyboardMouse: [{ input: "ShiftLeft", activator: "tap", threshold: 0.2 }],
        gamepad: [{ input: "left_stick_press" }],
      },
      sprint: { keyboardMouse: [{ input: "ShiftLeft", activator: "hold" }], gamepad: [] },
      alt_fire: { show: false },
    },
    glyphs: { keyboardMouse: "ui/glyphs/kbm", xbox: "ui/glyphs/xbox",
              playstation: "ui/glyphs/ps", nintendo: "ui/glyphs/nx" },
  },
});

const uiCommand: CommandId = "nav_confirm";
// @ts-expect-error Mod-defined commands are reserved; the command set is engine-closed.
const modCommand: CommandId = "postretro.dev.dash";

const unknownCommand: ModInput = {
  commands: {
    // @ts-expect-error An unknown command ID is not a key of the commands map.
    dahs: { label: "Dash" },
  },
};

const unknownActivator: ModInput = {
  commands: {
    // @ts-expect-error The activator set is closed: press, release, tap, hold.
    dash: { keyboardMouse: [{ input: "ShiftLeft", activator: "doubleTap" }] },
  },
};

void [manifest, uiCommand, modCommand, unknownCommand, unknownActivator];
