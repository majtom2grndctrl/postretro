// DESIGN SPIKE — the KEYSTONE. Type-checks against proposed.d.ts (the WALLs).
//
// This file's job: per-entity modder state and the lifecycles it unlocks — the two things that
// make DEATH a modder policy rather than an engine event. (The blessed handle + cross-scope
// override live in arena-death.spike.ts.) Two cases:
//
//   ZOMBIE (Quake)   health<=0 is NOT death. A gib-level overshoot kills; otherwise the zombie
//                    DOWNS and stands back up (setHealth after a delay). Same crossing, opposite
//                    meaning — no per-entity state needed, just the unfloored healthAfter.
//   IMP (Doom 2016)  an entity is a STATE MACHINE and impact drives transitions:
//                    alive ─(health<30%)→ staggered ─(any hit)→ glory-kill ; else → death.
//                    The SAME hit means different things per state, so we need PER-ENTITY,
//                    modder-owned state (`target.state("stagger")`). This is the keystone.
//
// Death is authored in IR; the engine has no lifecycle opinion. Per the spec, gated groups
// evaluate INDEPENDENTLY (every group whose `when` holds fires), so the author writes
// mutually-exclusive `when`s for a state machine.
//
// "postretro" → SHIPPED.  "postretro/proposed" → a WALL.

import {
  defineImpactEvent,
  type LevelManifestWithEvents,
} from "postretro/proposed";

// A per-entity "stagger" state field the modder owns: 0 = alive, 1 = staggered.
const ALIVE = 0;
const STAGGERED = 1;

// IMP — the Doom-2016 Glory Kill loop, a per-entity state machine driven by impact. A blessed
// handle at module scope (pure data), NOT a function that returns one.
const impLifecycle = defineImpactEvent({ tag: "imp" }, (impact) => {
  const t = impact.target;
  const stagger = t.state("stagger"); // per-instance modder state (IR ref)

  // Mutually-exclusive gated groups (independent evaluation; the author guarantees exclusivity).
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

// ZOMBIE — health<=0 is NOT death. Only a gib-level overshoot kills; otherwise the zombie DOWNS
// and resurrects. No per-entity state — the unfloored healthAfter carries enough (a large
// negative = a big overshoot = a gib).
const zombieLifecycle = defineImpactEvent({ tag: "zombie" }, (impact) => {
  const t = impact.target;
  const lethal = t.healthAfter.le(-40); // big overshoot → truly dead (gib)
  const downed = t.healthAfter.le(0).and(lethal.not()); // depleted, not gibbed → flop
  return [
    { when: lethal, do: [t.playAnim("gib"), impact.source.grant("xp", 100), t.despawn()] },
    { when: downed, do: [t.playAnim("down"), t.setHealth(t.maxHealth, { afterMs: 3000 })] }, // resurrect
  ];
});

export function setupLevel(): LevelManifestWithEvents {
  return {
    reactions: [],
    events: [impLifecycle, zombieLifecycle],
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
