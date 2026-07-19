// DESIGN SPIKE — ERGONOMICS PROBE. Answers "how JS-like can onEvent's callback be
// without lying about the no-live-VM invariant?" Type-checks against the real SDK
// plus postretro/ergo. See proposed-ergo.d.ts.
//
// VERDICT the file demonstrates:
//   RIGHT-ish — the onEvent(event, (event) => …) SHAPE, `event.target`/`.source`/
//     `.overkill` field access, method-style handles, and sequence/parallel
//     composition. It reads like addEventListener.
//   WRONG-ish — an IMPERATIVE statement body. There is no live VM at fire time, so
//     builder calls are pure factories; a statement that discards their return
//     records NOTHING. The bottom guardrail proves the compiler rejects it.
//
// The rule: every line is an expression returning a descriptor; the body COMPOSES
// those returns into a tree. Nothing is a bare side-effect statement.

import { defineReaction, defineStore } from "postretro";
import { playSound } from "postretro/ui";
import { healthCrossing, onEvent, parallel, sequence, slot, type HealthEvent, type Node } from "postretro/ergo";

const econ = defineStore("arena", {
  score:  { type: "number", default: 0, network: "shared" },
  deaths: { type: "number", default: 0, network: "shared" },
});
const score = slot(econ.state.score);
const deaths = slot(econ.state.deaths);

const enemyReward = defineReaction(score.add(100));
const countKill   = defineReaction(deaths.add(1));
const enemySfx    = defineReaction(playSound("enemyDown"));

const enemyDied = healthCrossing({ tag: "enemy" }, { below: 0 });

// === RIGHT-ish. JS-familiar, method-style, sequenced + concurrent — yet every
// line is an expression whose descriptor is COMPOSED into the returned tree.
const enemyDeath = onEvent(enemyDied, (event) =>
  sequence([
    parallel([                                // a concurrent group
      event.target.playDeathAnim(),           // presentation
      event.source.grant("ammo", 5),          // credits the SOURCE
      score.add(event.overkill),              // IR leaf
      enemySfx,
      enemyReward,
      countKill,
    ]),
    event.target.despawn({ after: "anim" }),  // then, after the group
  ]),
);
void enemyDeath;

// === GUARDRAILS — these MUST NOT compile. ===================================

// WRONG-ish: an imperative statement body returns void, not a Node. The discarded
// builder calls would record nothing (no live VM runs them at fire time).
// @ts-expect-error statement body returns void, not a composed Node.
const imperativeBody: (e: HealthEvent) => Node = (e) => { e.target.despawn(); };
void imperativeBody;

// The subject/source distinction is enforced by which methods EXIST:
onEvent(enemyDied, (event) => {
  // @ts-expect-error `source` has no despawn — you despawn the SUBJECT.
  event.source.despawn();
  // @ts-expect-error `target` has no grant — you credit the SOURCE.
  event.target.grant("ammo", 5);
  return event.target.despawn();
});
