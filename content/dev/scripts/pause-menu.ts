import {
  Button,
  CLOSE_DIALOG_ACTION,
  Text,
  Tree,
  VStack,
  defineTheme,
  defineUiTree,
  getDesignTokens,
} from "postretro/ui";
import { askExit, askQuit, openOptions } from "./frontend-menu";

const pauseTheme = defineTheme({
  color: {
    ok: [0.12, 0.72, 0.40, 1.0],
    panel: {
      default: [0.018, 0.026, 0.039, 0.92],
    },
  },
  font: {
    primary: "JetBrains Mono",
    mono: "JetBrains Mono",
  },
  spacing: {
    m: 8,
    l: 16,
  },
});

const { color, font, spacing } = getDesignTokens(pauseTheme);

export const pauseMenu = defineUiTree({
  name: "pauseMenu",
  tree: Tree(
    {
      anchor: "center",
      offset: [0.0, 0.0],
      captureMode: "capture",
      initialFocus: "pauseResume",
      accessibleName: "Pause menu",
      role: "group",
    },
    VStack(
      {
        gap: spacing.m,
        padding: spacing.l,
        align: "stretch",
        focus: { policy: "linear", wrap: true },
        fill: color.panel.default,
      },
      [
        Text({
          content: "PAUSED",
          font: font.mono,
          color: color.ok,
        }),
        Button({
          id: "pauseResume",
          label: "RESUME",
          onPress: CLOSE_DIALOG_ACTION,
        }),
        // The tabbed options screen, accessibility included. It hides this
        // menu while open; its BACK returns here.
        Button({
          id: "pauseOptions",
          label: "OPTIONS",
          onPress: openOptions,
        }),
        Button({
          id: "pauseQuitToMenu",
          label: "QUIT TO MENU",
          onPress: askQuit,
        }),
        Button({
          id: "pauseExitDesktop",
          label: "EXIT TO DESKTOP",
          onPress: askExit,
        }),
      ],
    ),
  ),
});
