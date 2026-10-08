// Timed, interruptible closet reveal with a replicated alarm cue: the SDK
// addressing surface in one set piece — map members, NPC and player groups,
// and a trigger member's per-volume events.
// Luau twin: closet-reveal.luau (byte-identical wire, so every reaction is
// named — derived ids differ per runtime).
// See: context/lib/scripting.md §12

import { defineReaction, getMapEntities, npcs, players, fire, wait } from "postretro";
import type { TriggerEventParams } from "postretro";
import { onStateCrossing, updateState } from "postretro/ui";
import { closetStore } from "./closet-store";

// System-targeted state write: a primitive reaction, dispatched via `fire()`.
const raiseAlarm = defineReaction("closet.raiseAlarm", updateState(closetStore.alarm, 1));
// Stepping off the plate heals whoever stepped off.
const patchUp = defineReaction("closet.patchUp", (on: TriggerEventParams) =>
  on.activators.grantHealth(25),
);
// Every seat-bound player, resolved when the reaction fires.
const resupply = defineReaction("closet.resupply", players().grantAmmo("shells.buck", 8));

export function setupLevel() {
  const door = getMapEntities("mover", { tag: "closet_door" }); // members, fixed at install
  const closets = getMapEntities("spawner", { tag: "closet_spawner" }); // spawned_tags "closet"
  const plate = getMapEntities("trigger", { tag: "closet_reveal_plate" });
  const alarmLights = getMapEntities("light", { tag: "closet_alarm" }).sort(
    (a, b) => a.position.x - b.position.x, // inspect in JS
  );
  // Group: the placed residents and whatever the spawners release.
  const closet = npcs({ tag: "closet" });

  // Client-local presentation: pulses the alarm lights, staggered west to
  // east, whenever the replicated alarm slot reads nonzero. The wait below is
  // host-only; this reaction only watches the slot the alarm write settles.
  const alarmLight = defineReaction("closet.alarmLight", {
    sequence: alarmLights.flatMap((l, i) =>
      l.pulse({ min: 0.3, max: 1.0, periodMs: 400 + i * 50 }),
    ),
  });
  // One authored beat: raise the alarm now, hold, then open, release and
  // rouse the closet. Stepping off the plate during the hold cancels the rest.
  const reveal = defineReaction("closet.timedReveal", {
    sequence: [
      ...fire(raiseAlarm),
      ...wait(800, { interruptible: true }),
      ...door.flatMap((m) => m.start()),
      ...closets.flatMap((s) => s.fire()),
      closet.update({ aggro: true }), // whoever exists when this step runs
      closet.damage(5),
      ...fire(resupply),
    ],
  });

  return {
    reactions: [reveal, raiseAlarm, alarmLight, patchUp, resupply],
    triggerEvents: plate.flatMap((t) => [t.on("enter", [reveal]), t.on("exit", [patchUp])]),
    // Crossing is frame-sampled (O44): nothing writes `alarm` back to 0 in the
    // frame the alarm write lands, so the crossing always observes it.
    crossings: [onStateCrossing(closetStore.alarm, { above: 0 }, [alarmLight])],
  };
}
