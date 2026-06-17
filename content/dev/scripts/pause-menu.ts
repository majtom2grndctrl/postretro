import {
  Button,
  CLOSE_DIALOG_ACTION,
  EXIT_TO_DESKTOP_ACTION,
  Text,
  Tree,
  VStack,
  defineUiTree,
} from "postretro";

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
        gap: "m",
        padding: "l",
        align: "stretch",
        focus: { policy: "linear", wrap: true },
        fill: "panel.default",
      },
      [
        Text({
          content: "PAUSED",
          font: "mono",
          color: "ok",
        }),
        Button({
          id: "pauseResume",
          label: "RESUME",
          onPress: CLOSE_DIALOG_ACTION,
        }),
        Button({
          id: "pauseExitDesktop",
          label: "EXIT TO DESKTOP",
          onPress: EXIT_TO_DESKTOP_ACTION,
        }),
      ],
    ),
  ),
});
