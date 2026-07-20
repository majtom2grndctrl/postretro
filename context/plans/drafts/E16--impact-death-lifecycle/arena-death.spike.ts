// DESIGN SPIKE — the HANDLE MODEL. Type-checks against the real postretro.d.ts + postretro/ui
// plus proposed.d.ts (the WALLs).
//
// This file's job: the blessed handle, cross-scope OVERRIDE, and policy reuse — how a mod
// defines a baseline death behavior and a map refines it. (Per-entity state and the lifecycles
// it unlocks — zombie, Doom stagger — live in lifecycle.spike.ts.)
//
// THE MODEL (grounded): the engine owns IMPACT; DEATH is a modder POLICY over impact facts.
// `defineImpactEvent(...)` returns a BLESSED HANDLE (like defineStore's handle — pure data,
// registered only by returning it through a manifest's `events`). The handle is the
// cross-scope reference currency: a mod defines the baseline, a MAP refines it in a DIFFERENT
// application scope via `handle.override(...)`, which returns a linked override to return from
// the map manifest. Nothing mutates; the engine merges base + override, most-recently-executed
// winning for a matched entity.
//
// The reusable POLICY is a plain function of impact (ordinary JS composition). The blessed
// HANDLE is what makes that policy a named engine event you can thread and override.
//
// NOTE: crediting the SOURCE (grant xp/style/ammo) is deferred — see the spec's Out-of-scope /
// roadmap. Rewards here are modelled as mod-store writes (slot.add), which ship in v1.
//
// "postretro" / "postretro/ui" → SHIPPED.  "postretro/proposed" → a WALL.

import { defineStore } from "postretro";
import {
  defineImpactEvent,
  slot,
  type EffectOrGroup,
  type GatedEffect,
  type Impact,
  type LevelManifest,
  type ModManifest,
} from "postretro/proposed";

const econ = defineStore("arena", {
  deaths: { type: "number", default: 0, network: "shared" },
  arenaKills: { type: "number", default: 0, network: "shared" },
});
const deaths = slot(econ.state.deaths);
const arenaKills = slot(econ.state.arenaKills);

// THE BASELINE POLICY — an ordinary function of impact, not a hoisted event. The author decides
// health-depleted means dead, and the author removes the entity (the engine does not auto-despawn
// at 0 HP). Reuse it anywhere by CALLING it.
function baseGruntDeath(impact: Impact): readonly EffectOrGroup[] {
  return [
    {
      when: impact.target.healthAfter.le(0), // author's death policy, expressed in IR
      do: [
        impact.target.playAnim("death"),
        deaths.add(1),
        impact.target.despawn({ afterMs: 1500 }),
      ],
    },
  ];
}

// THE BLESSED HANDLE — defined ONCE at module scope, pure, threadable across application scopes.
const gruntImpactEvent = defineImpactEvent({ tag: "grunt" }, baseGruntDeath);

// MOD SCOPE — register the baseline behavior mod-wide by returning the handle.
export function setupMod(): ModManifest {
  return {
    name: "arena-combat",
    events: [gruntImpactEvent],
  };
}

// MAP SCOPE — a DIFFERENT application scope. Refine the SAME blessed handle: arena grunts carry
// both "grunt" and "arena_grunt", so the override narrows the base set by the extra tag and wins
// for that subset. REUSE the baseline and add an arena-kills tally. `override(...)` returns a
// linked override to return from THIS manifest — no mutation, no re-declaration of the base.
export function setupLevel(): LevelManifest {
  return {
    reactions: [],
    events: [
      gruntImpactEvent.override({ tag: "arena_grunt" }, (impact) => [
        ...baseGruntDeath(impact),
        { when: impact.target.healthAfter.le(0), do: [arenaKills.add(1)] },
      ]),
    ],
  };
}

// === GUARDRAILS — these MUST NOT compile. ===================================
defineImpactEvent({ tag: "grunt" }, (impact) => {
  // @ts-expect-error live JS math on an IR ref — use .times(), not `*`.
  const badMath = 200 * impact.target.healthAfter;
  void badMath;
  // @ts-expect-error relational operators don't work on IR refs — use .le(), not `<=`.
  const badCmp = impact.target.healthAfter <= 0;
  void badCmp;
  // @ts-expect-error `source` is a published token with no v1 effect methods (grant deferred).
  impact.source.despawn();
  // @ts-expect-error a NumberRef is not a BoolRef gate.
  const badGate: GatedEffect = { when: impact.target.healthAfter, do: [] };
  void badGate;
  return [];
});

// @ts-expect-error a bare {kind:"impact"} is NOT an ImpactEvent handle — the brand can't be
// forged, so authored effects/IR must come from defineImpactEvent and survive the seam.
const badHandle: import("postretro/proposed").ImpactEvent = { kind: "impact" };
void badHandle;
