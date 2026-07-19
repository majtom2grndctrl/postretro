// DESIGN SPIKE — ERGONOMICS PROBE. Answers "how JS-like can onEvent's callback be
// without lying about the no-live-VM invariant?" and, after grounding, "does a
// sequence/parallel effect tree survive the two-arm split?"
//
// VERDICTS the file demonstrates:
//   RIGHT-ish — the onEvent(event, (event) => …) SHAPE, `event.target`/`.source`/
//     `.overkill` access, and method-style handles. It reads like addEventListener.
//   WRONG-ish (1) — an IMPERATIVE statement body. No live VM at fire time, so
//     builder calls are pure factories; a statement that discards their return
//     records NOTHING. The builder's return type makes that a compile error.
//   WRONG-ish (2) — a rich temporal sequence/parallel TREE. Grounding showed
//     shipped sequencing is instant (no wait), cross-arm order is fixed
//     (consequential before presentation — backwards vs "anim then despawn"), and
//     despawn timing is already an engine PROPERTY. So a handler is a flat effect
//     SET; timing lives on properties (despawn afterMs); a real timed pause is a
//     SEPARATE WALL (a duration primitive), not `sequence`.

import { defineReaction, defineStore } from "postretro";
import { playSound } from "postretro/ui";
import { healthCrossing, onEvent, slot, type HealthEvent, type Effect } from "postretro/ergo";

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

// === RIGHT-ish. JS-familiar, method-style — and a FLAT SET (auto-partitioned by
// arm). No sequence/parallel: without a duration primitive they'd be identical,
// and cross-arm "then" is architecturally fixed. `despawn({ afterMs })` carries the
// only real timing, as a property reusing the shipped deferred-despawn seam.
const enemyDeath = onEvent(enemyDied, (event) => [
  event.target.playDeathAnim(),          // presentation
  event.source.grant("ammo", 5),         // consequential — credits the SOURCE
  score.add(event.overkill),             // consequential — IR leaf
  enemySfx, enemyReward, countKill,
  event.target.despawn({ afterMs: 1500 }), // consequential — timer property
]);
void enemyDeath;

// === GUARDRAILS — these MUST NOT compile. ===================================

// WRONG-ish (1): a statement body returns void, not an Effect. The discarded
// builder calls would record nothing (no live VM runs them at fire time).
// @ts-expect-error statement body returns void, not an Effect/Effect set.
const imperativeBody: (e: HealthEvent) => Effect | readonly Effect[] = (e) => { e.target.despawn(); };
void imperativeBody;

// Subject/source distinction, enforced by which methods EXIST:
onEvent(enemyDied, (event) => [
  // @ts-expect-error `source` has no despawn — you despawn the SUBJECT.
  event.source.despawn(),
  // @ts-expect-error `target` has no grant — you credit the SOURCE.
  event.target.grant("ammo", 5),
]);
