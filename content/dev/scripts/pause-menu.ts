import {
  Button,
  CLOSE_DIALOG_ACTION,
  Text,
  Tree,
  VStack,
} from "postretro";

export const PAUSE_MENU_TREE = "pauseMenu";

export function buildPauseMenu() {
  return Tree(
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
      ],
    ),
  );
}
