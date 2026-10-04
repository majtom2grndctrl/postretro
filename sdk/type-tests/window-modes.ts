import type { ComputedRef, Ref } from "postretro";
import { displayModeAction, getGameState, updateState } from "postretro/ui";
const { options, window } = getGameState();
const next: "ui.displayMode.next" = displayModeAction("next");
const apply: "ui.displayMode.apply" = displayModeAction("apply");
const keep: "ui.displayMode.keep" = displayModeAction("keep");
const canApply: ComputedRef<boolean> = window.displayModeCanApply;
updateState(options.windowMode, "exclusive");
// @ts-expect-error Only the three engine modes are supported.
const invalidMode: typeof options.windowMode extends Ref<infer Mode> ? Mode : never = "fullscreen";
// @ts-expect-error The reserved action family is closed.
displayModeAction("toggle");
// @ts-expect-error Picked display fields are readonly.
updateState(window.displayModeWidth, 640);
// @ts-expect-error The countdown is readonly.
updateState(window.displayModeRevertSeconds, 15);
// @ts-expect-error Apply eligibility is a readonly transient value.
updateState(window.displayModeCanApply, true);
void [next, apply, keep, canApply, invalidMode];
