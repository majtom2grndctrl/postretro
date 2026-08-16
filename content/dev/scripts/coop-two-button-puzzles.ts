// DESIGN SPIKE — not a shipping content script.
//
// Goal: a map with two puzzles (North, South). Each puzzle has TWO buttons that
// must be held simultaneously; completing a puzzle runs a light animation once
// and opens a door. This file is written against ONLY the shapes that exist in
// the SDK today (post E18 trigger-event-fanout). Every place the shipped
// vocabulary cannot express the intent is marked `WALL:` — those walls are the
// design work that remains, and each names the not-yet-shipped surface it maps to.
//
// Substrate that IS shipped and load-bearing here:
//   - trigger_volume contact via `on_fire` / `on_exit` KVPs (authored on the brush)
//   - reactions fired by name; sequence bodies splicing light + mover steps
//   - `setState` (updateState) writing a store slot in-tick
//   - `onStateCrossing` watching ONE number slot for an above/below edge

import { world, defineReaction, defineStore } from "postretro";
import type { NamedReactionDescriptor } from "postretro";
import { onStateCrossing, updateState } from "postretro/ui";

// --- Mission-scoped store: one number per puzzle. ---------------------------
// Intent: `northHeld` counts how many of the two North buttons are currently
// held (0, 1, or 2). The crossing below wants to fire when it reaches 2.
const puzzles = defineStore("coopPuzzles", {
  northHeld: { type: "number", default: 0, range: [0, 2] },
  southHeld: { type: "number", default: 0, range: [0, 2] },
});

export function setupLevel() {
  const reactions: NamedReactionDescriptor[] = [];

  // --- Query the payoff entities per puzzle. --------------------------------
  // These return typed handles: movers expose start()/stop()/reverse(); lights
  // expose fade()/pulse()/… — each emitting a SequenceStep[].
  const northDoor = world.query({ component: "kinematic_mover", tag: "north-door" });
  const northLight = world.query({ component: "light", tag: "north-beacon" });
  const southDoor = world.query({ component: "kinematic_mover", tag: "south-door" });
  const southLight = world.query({ component: "light", tag: "south-beacon" });

  // --- Payoff reaction: light-once THEN door-open. --------------------------
  // A single `sequence` body can splice a light step and a mover step because
  // both are per-entity SequenceSteps. `fade` carries playCount:1 → runs once.
  //
  // WALL #1 (no timed wait): sequence steps fire in array order AT DISPATCH,
  // with no delay between them. The door opens the same instant the light
  // starts, not after the 800ms flare finishes. There is no `wait(ms)` step in
  // the SequenceStep union, and `onComplete` chains on a later dispatch hop, not
  // a wall-clock delay. The "theatrical pause" the goal asks for is unshippable
  // today. → needs a sequencing/delay primitive.
  reactions.push(
    defineReaction("solveNorth", {
      sequence: [
        ...northLight.flatMap((l) => l.fade({ from: 0, to: 1, periodMs: 800 })),
        ...northDoor.flatMap((d) => d.start()),
      ],
    }),
    defineReaction("solveSouth", {
      sequence: [
        ...southLight.flatMap((l) => l.fade({ from: 0, to: 1, periodMs: 800 })),
        ...southDoor.flatMap((d) => d.start()),
      ],
    }),
  );

  // --- Buttons write occupancy into the store. ------------------------------
  // Each button brush is authored with on_fire = "<name>Down", on_exit =
  // "<name>Up". The reactions push the button's contribution into the store.
  //
  // WALL #2 (no increment): `updateState` writes an ABSOLUTE value. Two buttons
  // that both write `northHeld = 1` produce an OR ("at least one held"), not a
  // COUNT — a second press cannot advance 1→2, and one release clobbers the
  // other's contribution back to 0. To make `northHeld` a real count of held
  // buttons we need `northHeld += 1` / `-= 1` (the state-delta-reactions draft),
  // OR per-button boolean slots plus a way to sum them (see WALL #3).
  reactions.push(
    defineReaction("northAdown", updateState(puzzles.northHeld, 1)),
    defineReaction("northAup", updateState(puzzles.northHeld, 0)),
    defineReaction("northBdown", updateState(puzzles.northHeld, 1)),
    defineReaction("northBup", updateState(puzzles.northHeld, 0)),
  );

  // --- The join: "both held → fire the payoff". -----------------------------
  // Intent: when `northHeld` reaches 2, fire `solveNorth`.
  //
  // WALL #3 (single-slot, threshold-only observer): `onStateCrossing` watches
  // ONE Number slot for an above/below edge. It cannot express `northA && northB`
  // across two boolean slots, and `above: 1` only reads as "reached 2" if some
  // mechanism actually counts to 2 — which WALL #2 says nothing does. The clean
  // predicate ("both plates occupied") wants an IR boolean over N slots, i.e. the
  // crossings-with-an-IR-predicate generalization we sketched. Written here in
  // its shipped-but-underpowered form, riding on the (broken) counter above:
  const crossings = [
    onStateCrossing(puzzles.northHeld, { above: 1, max: 2 }, ["solveNorth"]),
    onStateCrossing(puzzles.southHeld, { above: 1, max: 2 }, ["solveSouth"]),
  ];

  // NOTE: simultaneity across two buttons is exactly E18-B (coop-activation-
  // policy), which is NOT shipped. The trigger system already tracks a
  // per-trigger occupancy COUNT internally, but it is engine-owned and not
  // exposed to script — so script cannot even read "2 occupants" to gate on it.

  return { reactions, crossings };
}
