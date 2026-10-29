// Timed, interruptible closet reveal with a replicated alarm cue: the SDK
// addressing surface in one set piece — map members, NPC and player groups,
// and a trigger member's per-volume events.
// Luau twin: closet-reveal.luau (byte-identical wire, so every reaction is
// named — derived ids differ per runtime).
// See: context/lib/scripting.md §12

import { defineReaction, getMapEntities, npcs, players, wait } from "postretro";
import type { TriggerEventParams } from "postretro";
import { onStateCrossing, updateState } from "postretro/ui";
import { closetStore } from "./closet-store";

// Stepping off the plate heals whoever stepped off.
const patchUp = defineReaction("closet.patchUp", (on: TriggerEventParams) =>
  on.activators.grantHealth(25),
);

export function setupLevel() {
  const door = getMapEntities("mover", { tag: "closet_door" }); // members, fixed at install
  const closets = getMapEntities("spawner", { tag: "closet_spawner" }); // spawned_tags "closet enemy"
  const plate = getMapEntities("trigger", { tag: "closet_reveal_plate" });
  const alarmLights = getMapEntities("light", { tag: "closet_alarm" }).sort(
    (a, b) => a.position.x - b.position.x, // inspect in JS
  );
  // Group: the placed residents and whatever the spawners release.
  const closet = npcs({ tag: "closet" });

  // Client-local presentation: pulses the alarm lights, staggered west to
  // east, whenever the replicated alarm slot reads nonzero. Every step after
  // the wait below is the host's; this reaction only watches the slot the
  // alarm write settles.
  const alarmLight = defineReaction(
    "closet.alarmLight",
    alarmLights.map((l, i) => l.pulse({ min: 0.3, max: 1.0, periodMs: 400 + i * 50 })),
  );
  // One authored beat: raise the alarm now, hold, then open, release and
  // rouse the closet, and resupply every seat-bound player. Stepping off the
  // plate during the hold cancels the rest.
  const reveal = defineReaction("closet.timedReveal", [
    updateState(closetStore.alarm, 1), // a system step: the replicated alarm write
    wait(800, { interruptible: true }),
    door.map((m) => m.start()),
    closets.map((s) => s.fire()),
    closet.update({ aggro: true }), // whoever exists when this step runs
    closet.damage(5),
    players().grantAmmo("shells.buck", 8), // resolved when this step runs
  ]);

  return {
    reactions: [reveal, alarmLight, patchUp],
    triggerEvents: plate.flatMap((t) => [t.on("enter", [reveal]), t.on("exit", [patchUp])]),
    // Crossing is frame-sampled (O44): nothing writes `alarm` back to 0 in the
    // frame the alarm write lands, so the crossing always observes it.
    crossings: [onStateCrossing(closetStore.alarm, { above: 0 }, [alarmLight])],
  };
}
