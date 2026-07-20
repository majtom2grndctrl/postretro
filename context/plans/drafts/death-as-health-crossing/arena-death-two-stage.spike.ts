// DESIGN SPIKE — TWO-STAGE variant, for A/B against arena-death.spike.ts.
//
// Same grounded model (engine owns IMPACT; death is DERIVED). The difference is
// SHAPE: here impact is ENRICHED into a NAMED DERIVED EVENT — you DEFINE `killed`
// with an edge + a payload of IR facts — and a SEPARATE consumer does the effects.
// This is the literal reading of "we aren't adding a previously defined listener,
// we're defining a new event." It costs an extra type (the derived event, its
// payload) but pays off the moment a SECOND behavior wants to consume "kill"
// (achievements, spawner budget, announcer) without re-deriving the edge.
//
// The two invariants still hold and are still compiler-enforced at the bottom:
//   1. Payload facts are IR refs — `200 * kill.xp` fails.
//   2. Subject (damaged) vs source (damager) stay distinct through the payload.

import { defineStore } from "postretro";
import { entities, slot, type LevelManifestWithEvents } from "postretro/proposed";

const econ = defineStore("arena", {
  deaths: { type: "number", default: 0, network: "shared" },
});
const deaths = slot(econ.state.deaths);

const grunts = entities.query({ tag: "grunt" });

// === STAGE 1 — DEFINE the derived event. Enrich impact into `killed`: it fires on
// the kill edge and carries named IR facts + the two entity tokens. No effects here.
const killed = grunts.defineEvent((impact) => ({
  when: impact.target.crossedBelow(0),                        // the firing edge
  props: {
    subject: impact.target,                                   // pass-through token
    source: impact.source,                                    // pass-through token
    xp: impact.target.level.times(1.25).times(200),           // IR fact
    overkill: impact.target.healthAfter.lt(-10),              // IR fact (BoolRef)
  },
}));

// === STAGE 2 — CONSUME it. `kill` is the payload, typed exactly from stage 1.
// Any number of consumers can bind to `killed`; this one does the arena behavior.
export function setupLevel(): LevelManifestWithEvents {
  return {
    reactions: [],
    events: [
      killed.on((kill) => [
        kill.subject.playDeathAnim(),                          // presentation
        kill.source.grant("xp", kill.xp),                      // consequential — credits SOURCE
        deaths.add(1),                                         // consequential
        kill.subject.despawn({ afterMs: 1500 }),               // consequential — timer property
        { when: kill.overkill, do: [ kill.source.grant("style", 1) ] }, // inline gate
      ]),
    ],
  };
}

// === GUARDRAILS — these MUST NOT compile. ===================================
const killedGuard = grunts.defineEvent((impact) => ({
  when: impact.target.crossedBelow(0),
  props: {
    subject: impact.target,
    source: impact.source,
    xp: impact.target.level.times(1.25).times(200),
  },
}));
killedGuard.on((kill) => {
  // @ts-expect-error live JS math on an IR payload fact — use .times(), not `*`.
  const badXp = 200 * kill.xp;
  void badXp;
  // @ts-expect-error `source` has no despawn — you despawn the SUBJECT.
  kill.source.despawn();
  // @ts-expect-error `subject` has no grant — you credit the SOURCE.
  kill.subject.grant("xp", 5);
  // @ts-expect-error `overkill` was never enriched onto this event — not in payload.
  void kill.overkill;
  return [];
});
