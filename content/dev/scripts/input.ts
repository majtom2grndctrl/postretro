// The dev mod's input block: controls-panel labels and order, Shift as tap-to-
// dash and hold-to-sprint on one key (left stick press on the pad), and glyph
// art per device family.
// See: docs/scripting-reference.md (Input block)

import type { ModInput } from "postretro";

export const devInput: ModInput = {
  commands: {
    move_forward: { label: "Forward", category: "MOVEMENT", order: 10 },
    move_back: { label: "Back", category: "MOVEMENT", order: 11 },
    move_left: { label: "Strafe left", category: "MOVEMENT", order: 12 },
    move_right: { label: "Strafe right", category: "MOVEMENT", order: 13 },
    look_x: { label: "Look left / right", category: "MOVEMENT", order: 14 },
    look_y: { label: "Look up / down", category: "MOVEMENT", order: 15 },
    jump: { label: "Jump", category: "MOVEMENT", order: 20 },
    crouch: { label: "Crouch", category: "MOVEMENT", order: 21 },
    sprint: {
      label: "Sprint",
      category: "MOVEMENT",
      order: 30,
      keyboardMouse: [{ input: "ShiftLeft", activator: "hold" }],
      gamepad: [{ input: "left_stick_press", activator: "hold" }],
    },
    dash: {
      label: "Dash",
      category: "MOVEMENT",
      order: 31,
      keyboardMouse: [{ input: "ShiftLeft", activator: "tap", threshold: 0.2 }],
      gamepad: [{ input: "left_stick_press", activator: "tap", threshold: 0.2 }],
    },
    shoot: { label: "Fire", category: "COMBAT", order: 40 },
    alt_fire: { label: "Alt fire", category: "COMBAT", order: 41 },
    reload: { label: "Reload", category: "COMBAT", order: 42 },
    use: { label: "Use", category: "COMBAT", order: 43 },
    drop: { label: "Drop weapon", category: "COMBAT", order: 44 },
    cycle_wieldable_next: { label: "Next weapon", category: "WEAPONS", order: 50 },
    cycle_wieldable_previous: { label: "Previous weapon", category: "WEAPONS", order: 51 },
    toggle_last_wieldable: { label: "Last weapon", category: "WEAPONS", order: 52 },
    nav_confirm: { label: "Select", category: "MENUS", order: 60 },
    nav_cancel: { label: "Back", category: "MENUS", order: 61 },
    nav_menu: { label: "Pause", category: "MENUS", order: 62 },
  },
  glyphs: {
    keyboardMouse: "ui/glyphs/kbm",
    xbox: "ui/glyphs/xbox",
    playstation: "ui/glyphs/ps",
    nintendo: "ui/glyphs/nx",
  },
};
