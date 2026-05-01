// Reference behavior: drives entities tagged `rotatorDriver` around the
// world Y axis at a constant rate. Loaded by the engine's behavior-script
// sweep (lexicographic file order) at level load.

import {
  getComponent,
  registerHandler,
  setComponent,
  worldQuery,
} from "postretro";
import type { ScriptCallContext, Transform } from "postretro";

/** Yaw advance rate (degrees per second). */
const ROTATION_RATE_DEG_PER_SEC = 90;

/** Tag every driven entity must carry to be picked up by this script. */
const DRIVER_TAG = "rotatorDriver";

registerHandler("tick", (ctx?: ScriptCallContext) => {
  // `tick` always receives a context, but the SDK type marks it optional
  // (the same handler signature also covers `levelLoad`, which receives
  // none). Bail rather than assert so a future event with no ctx does
  // not crash the surface.
  if (!ctx) return;

  const drivers = worldQuery({ component: "transform", tag: DRIVER_TAG });
  if (drivers.length === 0) return;

  const deltaYawDeg = ROTATION_RATE_DEG_PER_SEC * ctx.delta;

  for (const driver of drivers) {
    const value = getComponent(driver.id, "transform");
    if (value.kind !== "transform") continue;
    // Flat ComponentValue shape: payload fields sit beside `kind` (no `value`
    // wrapper). The cast narrows away the union after the `kind` check.
    const t = value as Transform & { kind: "transform" };
    const updated: Transform = {
      position: t.position,
      rotation: {
        pitch: t.rotation.pitch,
        yaw: t.rotation.yaw + deltaYawDeg,
        roll: t.rotation.roll,
      },
      scale: t.scale,
    };
    setComponent(driver.id, "transform", { kind: "transform", ...updated });
  }
});
