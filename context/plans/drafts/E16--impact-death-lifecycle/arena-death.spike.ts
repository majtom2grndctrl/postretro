// DESIGN SPIKE — the CANONICAL pre-spec artifact (v4). Type-checks against the real
// postretro.d.ts + postretro/ui plus proposed.d.ts (the WALLs).
//
// THE MODEL (grounded): the engine owns IMPACT; DEATH is a modder POLICY over impact facts.
// `defineImpactEvent(...)` returns a BLESSED HANDLE (like defineStore's handle — pure data,
// registered only by returning it through a manifest's `events`). The handle is the
// cross-scope reference currency: a mod defines the baseline, a MAP refines it in a DIFFERENT
// application scope via `handle.override(...)`, which returns a linked override to return from
// the map manifest. Nothing mutates; the engine merges base + override by identity at load.
//
// The reusable POLICY is a plain function of impact (ordinary JS composition). The blessed
// HANDLE is what makes that policy a named engine event you can thread and override.
//
// "postretro" / "postretro/ui" → SHIPPED.  "postretro/proposed" → a WALL.

import { defineStore } from "postretro";
import {
  defineImpactEvent,
  slot,
  type EffectOrGroup,
  type GatedEffect,
  type Impact,
  type LevelManifestWithEvents,
  type ModManifestWithEvents,
} from "postretro/proposed";

const econ = defineStore("arena", {
  deaths: { type: "number", default: 0, network: "shared" },
});
const deaths = slot(econ.state.deaths);

// THE BASELINE POLICY — an ordinary function of impact, not a hoisted event. The author
// decides health-depleted means dead, and the author removes the entity (the engine does not
// auto-despawn at 0 HP). Reuse it anywhere by CALLING it.
function baseGruntDeath(impact: Impact): readonly EffectOrGroup[] {
  return [
    {
      when: impact.target.healthAfter.le(0), // author's death policy, expressed in IR
      do: [
        impact.target.playAnim("death"),
        impact.source.grant("xp", impact.target.level.times(1.25).times(200)),
        deaths.add(1),
        impact.target.despawn({ afterMs: 1500 }),
      ],
    },
  ];
}

// Quake zombie: health<=0 is NOT death. Only a gib-level overshoot kills; otherwise the
// zombie DOWNS and stands back up.
function zombiePolicy(impact: Impact): readonly EffectOrGroup[] {
  const lethal = impact.target.healthAfter.le(-40);
  const downed = impact.target.healthAfter.le(0).and(lethal.not());
  return [
    { when: lethal, do: [impact.target.playAnim("gib"), impact.source.grant("xp", 100), impact.target.despawn()] },
    { when: downed, do: [impact.target.playAnim("down"), impact.target.setHealth(impact.target.level.times(20), { afterMs: 3000 })] },
  ];
}

// BLESSED HANDLES — defined ONCE at module scope, pure, threadable across application scopes.
const gruntImpactEvent = defineImpactEvent({ tag: "grunt" }, baseGruntDeath);
const zombieImpactEvent = defineImpactEvent({ tag: "zombie" }, zombiePolicy);

// MOD SCOPE — register the baseline behaviors mod-wide by returning the handles.
export function setupMod(): ModManifestWithEvents {
  return {
    name: "arena-combat",
    events: [gruntImpactEvent, zombieImpactEvent],
  };
}

// MAP SCOPE — a DIFFERENT application scope. Refine the SAME blessed handle: for arena_1
// grunts, REUSE the baseline and add a style payout. `override(...)` returns a linked override
// to return from THIS manifest — no mutation, no re-declaration of the base.
export function setupLevel(): LevelManifestWithEvents {
  return {
    reactions: [],
    events: [
      gruntImpactEvent.override({ zone: "arena_1" }, (impact) => [
        ...baseGruntDeath(impact),
        { when: impact.target.healthAfter.le(0), do: [impact.source.grant("style", 5)] },
      ]),
    ],
  };
}

// === GUARDRAILS — these MUST NOT compile. ===================================
defineImpactEvent({ tag: "grunt" }, (impact) => {
  // @ts-expect-error live JS math on an IR ref — use .times(), not `*`.
  const badXp = 200 * impact.target.level;
  void badXp;
  // @ts-expect-error relational operators don't work on IR refs — use .le(), not `<=`.
  const badCmp = impact.target.healthAfter <= 0;
  void badCmp;
  // @ts-expect-error `source` has no despawn — you despawn the TARGET.
  impact.source.despawn();
  // @ts-expect-error `target` has no grant — you credit the SOURCE.
  impact.target.grant("xp", 5);
  // @ts-expect-error a NumberRef is not a BoolRef gate.
  const badGate: GatedEffect = { when: impact.target.healthAfter, do: [] };
  void badGate;
  return [];
});

// @ts-expect-error a bare {kind:"impact"} is NOT an ImpactEvent handle — the brand can't be
// forged, so authored effects/IR must come from defineImpactEvent and survive the seam.
const badHandle: import("postretro/proposed").ImpactEvent = { kind: "impact" };
void badHandle;
