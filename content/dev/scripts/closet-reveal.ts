// Timed, interruptible closet reveal with a replicated alarm cue.
// See: context/lib/scripting.md §12

import { defineReaction, npcs, getMapEntities, fire, wait } from "postretro";
import type { NamedReactionDescriptor } from "postretro";
import { onStateCrossing, updateState } from "postretro/ui";
import { closetStore } from "./closet-store";

// System-targeted state write: a Primitive reaction, dispatched via `fire()`
// — never a sequence step.
const raiseAlarm = defineReaction("closet.raiseAlarm", updateState(closetStore.alarm, 1));

// An NPC group command, resolved when the reaction fires.
const releaseCloset = defineReaction(
  "closet.releaseCloset",
  npcs({ tag: "closet_enemies" }).update({ aggro: true }),
);

export function setupLevel() {
  const door = getMapEntities("mover", { tag: "closet_door" });
  const alarmLights = getMapEntities("light", { tag: "closet_alarm" });

  // Client-local presentation: pulses the closet_alarm light whenever the
  // replicated alarm slot reads nonzero. Runs on every client independently
  // of the host scheduler — the wait above is host-only and never reaches
  // here; this reaction only watches the slot the alarm write settles.
  const alarmLight = defineReaction("closet.alarmLight", {
    sequence: alarmLights.flatMap((l) => l.pulse({ min: 0.3, max: 1.0, periodMs: 400 })),
  });

  // One authored beat: raise the alarm now, hold, then slam and release
  // together. Stepping off the plate during the hold cancels both.
  const reveal = defineReaction("closet.timedReveal", {
    sequence: [
      ...fire(raiseAlarm), // dispatched; replicates, clients light it locally
      ...wait(800, { interruptible: true }), // enrolls the remainder, stops here
      ...door.flatMap((m) => m.start()), // resumes ~48 ticks later
      ...fire(releaseCloset), // dispatches the NPC-group release
    ],
  });

  const reactions: NamedReactionDescriptor[] = [reveal, raiseAlarm, releaseCloset, alarmLight];

  return {
    reactions,
    // No "exit" registration needed — V5 derives the Exit edge from the
    // interruptible wait.
    triggerEvents: getMapEntities("trigger", { tag: "closet_reveal_plate" }).map((plate) =>
      plate.on("enter", [reveal]),
    ),
    // Crossing is frame-sampled (O44): this fixture never re-writes `alarm`
    // back to 0 in the same frame the alarm-raising landing runs, so no
    // opposing landing ever shares a frame with the write and the crossing
    // always observes it.
    crossings: [onStateCrossing(closetStore.alarm, { above: 0 }, [alarmLight])],
  };
}
