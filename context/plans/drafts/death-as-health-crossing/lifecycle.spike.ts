// DESIGN SPIKE — the LIFECYCLE stress test. Type-checks against proposed.d.ts (the WALLs).
//
// The zombie proved "health<=0 is not death." Doom 2016 proves the bigger thing: an entity
// is a STATE MACHINE and impact drives the transitions. The Glory Kill loop is:
//
//     alive ──(health < ~30%)──▶ staggered/falter ──(any hit)──▶ glory-killed (drops health/ammo)
//                                        │
//                                        └──────────(enough overshoot)──▶ normal death
//
// The SAME incoming hit means different things depending on which state the imp is in — so
// impact facts (healthAfter/amount) are not enough. We need PER-ENTITY, modder-owned state
// (`target.state("stagger")`). Impact reads and writes it; the engine has no lifecycle opinion.
//
// This spike exists to answer one question: is the surface expressive enough to build that
// loop? If it type-checks with only the WALL primitives, yes.
//
// "postretro" → SHIPPED.  "postretro/proposed" → a WALL.

import {
  defineImpactEvent,
  type ImpactEvent,
  type LevelManifestWithEvents,
} from "postretro/proposed";

// A per-entity "stagger" state field the modder owns: 0 = alive, 1 = staggered.
const ALIVE = 0;
const STAGGERED = 1;

function impLifecycle(): ImpactEvent {
  return defineImpactEvent({ tag: "imp" }, (impact) => {
    const t = impact.target;
    const stagger = t.state("stagger");                         // per-instance modder state (IR ref)

    // Transitions, authored as mutually-exclusive gated groups. NOTE(spec): this relies on
    // gated groups evaluating INDEPENDENTLY (every group whose `when` holds fires) with the
    // author guaranteeing exclusivity — vs a first-match/cond semantics. That choice is an
    // open question this example surfaces; the conditions below are written to be correct
    // under either reading.
    return [
      // A hit while STAGGERED = a Glory Kill: instant death, drops health + ammo to the source.
      {
        when: stagger.eq(STAGGERED),
        do: [
          t.playAnim("glory_kill"),
          impact.source.grant("health", 25),
          impact.source.grant("ammo", 10),
          t.despawn(),
        ],
      },
      // First drop below 30% max health, while still alive and not yet a kill → FALTER.
      {
        when: t.healthAfter
          .le(t.maxHealth.times(0.3))
          .and(stagger.eq(ALIVE))
          .and(t.healthAfter.gt(0)),
        do: [t.setState("stagger", STAGGERED), t.playAnim("falter")],
      },
      // Normal death path (killed outright, never staggered).
      {
        when: t.healthAfter.le(0).and(stagger.eq(ALIVE)),
        do: [t.playAnim("death"), t.despawn({ afterMs: 1200 })],
      },
    ];
  });
}

export function setupLevel(): LevelManifestWithEvents {
  return {
    reactions: [],
    events: [impLifecycle()],
  };
}

// === GUARDRAILS — these MUST NOT compile. ===================================
defineImpactEvent({ tag: "imp" }, (impact) => {
  const stagger = impact.target.state("stagger");
  // @ts-expect-error per-entity state is an IR ref — no live JS math (use .plus(), not `+`).
  const bad = stagger + 1;
  void bad;
  // @ts-expect-error setState takes a NumberValue, not a string — state fields are numeric.
  impact.target.setState("stagger", "on");
  return [];
});
