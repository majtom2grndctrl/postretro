import { OPEN_CONTROLS_ACTION } from "postretro/ui";

const openControls: "ui.openControls" = OPEN_CONTROLS_ACTION;
// @ts-expect-error The controls action is one exact wire value.
const wrongControls: "ui.openAccessibility" = OPEN_CONTROLS_ACTION;

export { openControls, wrongControls };
