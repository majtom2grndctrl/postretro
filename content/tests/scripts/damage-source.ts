// Reference behavior: emits a synthetic `damage` event every
// DAMAGE_INTERVAL_SEC seconds and surfaces a one-shot diagnostic on
// level load. Loaded by the engine's behavior-script sweep at level load.

import { emitEvent, registerHandler, worldQuery } from "postretro";
import type { ScriptCallContext } from "postretro";

const DAMAGE_INTERVAL_SEC = 3.0;
const DAMAGE_AMOUNT = 10;
const TARGET_TAG = "damageTarget";

let elapsedSinceLastEmit = 0;

registerHandler("levelLoad", () => {
  // QuickJS behavior context exposes no `console.log`; the only side
  // channel scripts have is `emitEvent`. Emit unconditionally so the
  // levelLoad handler is observable in the `game_events` log stream
  // regardless of whether the map carries matching entities.
  const targets = worldQuery({
    component: "transform",
    tag: TARGET_TAG,
  });
  emitEvent({
    kind: "damageSource:levelLoad",
    payload: { targets: targets.length, tag: TARGET_TAG },
  });
  elapsedSinceLastEmit = 0;
});

registerHandler("tick", (ctx?: ScriptCallContext) => {
  if (!ctx) return;
  elapsedSinceLastEmit += ctx.delta;
  if (elapsedSinceLastEmit < DAMAGE_INTERVAL_SEC) return;
  elapsedSinceLastEmit -= DAMAGE_INTERVAL_SEC;

  emitEvent({
    kind: "damage",
    payload: { source: null, amount: DAMAGE_AMOUNT },
  });
});
