// DESIGN SPIKE — the SUBSTRATE STANDS ALONE. Type-checks against the real postretro.d.ts
// plus proposed.d.ts (the WALL).
//
// This file's job: prove the impact-policy machine — per-entity state read/write, a gated
// group, effects, the blessed handle + a cross-scope override — with NO death semantics.
// No `healthAfter.le(0)` policy, no 0-HP inertness, no kill report, no resurrect. A breakable
// crate accrues a per-instance hit count and shatters at a threshold; the entity never
// reaches 0 HP and no engine death machinery is involved. Death is one policy DOMAIN built
// on this substrate (`E16--impact-death-lifecycle`, whose fixtures show the same surface
// driving kill/down/stagger); this fixture is domain-free.
//
// Two substrate rules on display:
//   PRE-EFFECT SNAPSHOT  every group's `when` evaluates against the state as it was BEFORE
//                        any of this impact's effects (bare or gated) applied — evaluate-then-
//                        apply per fire — so the same-impact bare `setState` cannot shift the
//                        threshold gate. The break gate therefore
//                        reads `hits.eq(HITS_TO_BREAK - 1)`: the count BEFORE this hit's
//                        increment. That also makes the gate edge-like — it holds on exactly
//                        one hit per instance.
//   EMERGENT STATE       `state("hits")` is declared nowhere. It springs into existence on
//                        the first `setState`; an unset read is total-zero (`0`). Each
//                        instance accrues independently.
//
// "postretro" → SHIPPED.  "postretro/proposed" → a WALL.

import { defineStore } from "postretro";
import {
  defineImpactEvent,
  slot,
  type EffectOrGroup,
  type Impact,
  type LevelManifest,
  type ModManifest,
} from "postretro/proposed";

const salvage = defineStore("salvage", {
  cratesBroken: { type: "number", default: 0, network: "shared" },
});
const cratesBroken = slot(salvage.state.cratesBroken);

// THE REUSABLE POLICY — an ordinary function, parameterized by threshold. The author decides
// what an impact MEANS for a crate: accrue a dent; shatter at N dents. No health fact is read.
function breakableAfter(hitsToBreak: number) {
  return (impact: Impact): readonly EffectOrGroup[] => {
    const t = impact.target;
    const hits = t.state("hits"); // per-instance modder state (IR ref); unset reads 0
    return [
      // Every impact dents THIS instance: read-modify-write on its own counter.
      t.setState("hits", hits.plus(1)),
      // The shatter threshold. `hits` is the PRE-EFFECT snapshot value, so the bare `setState`
      // above is invisible here (evaluate-then-apply per fire: every `when` and every effect
      // read is served from the pre-fire snapshot, then all effects apply). The gate holds on
      // the hit where the accrued count REACHES the threshold (count was N-1 before this hit) —
      // once per instance, no re-fire. `despawn()` only MARKS; removal runs at end-of-frame, and
      // `playAnim` is an in-tick switch, so it targets a LIVE entity (the switch applies without
      // error). With a bare `despawn()` the crate is reaped this frame before render, so no frame
      // renders the shatter clip — visible playback needs `despawn({ afterMs })`.
      // (Decisions: Despawn ordering.)
      {
        when: hits.eq(hitsToBreak - 1),
        do: [t.playAnim("shatter"), cratesBroken.add(1), t.despawn()],
      },
    ];
  };
}

// THE BLESSED HANDLE — defined once at module scope, pure, threadable across scopes.
const crateImpactEvent = defineImpactEvent("salvage:crate-break", { tag: "crate" }, breakableAfter(3));

// MOD SCOPE — the baseline: any crate shatters after 3 hits.
export function setupMod(): ModManifest {
  return {
    name: "salvage-props",
    stores: [salvage.declaration],
    events: [crateImpactEvent],
  };
}

// MAP SCOPE — a DIFFERENT application scope refines the SAME handle: this map's reinforced
// crates carry both "crate" and "reinforced_crate", so the override narrows by the extra tag
// and wins for that subset (last-registered, load order). Whole-policy replace: a reinforced
// crate runs ONLY the 6-hit variant, never both.
export function setupLevel(): LevelManifest {
  return {
    reactions: [],
    events: [
      crateImpactEvent.override({ tag: "reinforced_crate" }, breakableAfter(6)),
    ],
  };
}

// === GUARDRAILS — these MUST NOT compile. ===================================
defineImpactEvent("salvage:invalid-effect-example", { tag: "crate" }, (impact) => {
  const hits = impact.target.state("hits");
  // @ts-expect-error per-entity state is an IR ref — no live JS math (use .plus(), not `+`).
  const badMath = hits + 1;
  void badMath;
  // @ts-expect-error setState takes a NumberValue, not a string — state fields are numeric.
  impact.target.setState("hits", "dented");
  // @ts-expect-error relational operators don't work on IR refs — use .eq(), not `===`.
  const badCmp = hits === 2;
  void badCmp;
  return [];
});

// @ts-expect-error an override must narrow its base with an additional tag.
crateImpactEvent.override({}, breakableAfter(6));

// @ts-expect-error Effect is constructor-produced; a branded-looking record is not an effect.
const forgedEffect: import("postretro/proposed").Effect = { __impactEffectBrand: true };
void forgedEffect;
