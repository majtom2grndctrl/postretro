import { OPEN_ACCESSIBILITY_ACTION, accessibilityAction } from "postretro/ui";

const open: "ui.openAccessibility" = OPEN_ACCESSIBILITY_ACTION;
const cycle: "ui.accessibility.cycle.reduceMotion" = accessibilityAction("reduceMotion", "cycle");
const step: "ui.accessibility.increase.sfxVolume" = accessibilityAction("sfxVolume", "increase");
const limiter: "ui.accessibility.cycle.flashLimiter" = accessibilityAction("flashLimiter", "cycle");
// @ts-expect-error The flash limiter is a toggle; it does not step.
const steppedLimiter = accessibilityAction("flashLimiter", "decrease");
// @ts-expect-error Toggles cycle; they do not step.
const steppedToggle = accessibilityAction("monoAudio", "increase");
// @ts-expect-error Numeric fields step; they do not cycle.
const cycledNumber = accessibilityAction("masterVolume", "cycle");
void [open, cycle, step, limiter, steppedLimiter, steppedToggle, cycledNumber];
