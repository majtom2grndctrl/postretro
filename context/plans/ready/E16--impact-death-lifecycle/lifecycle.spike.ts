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
  type LevelManifest,
} from "postretro/proposed";

// A per-entity "stagger" state field the modder owns: 0 = alive, 1 = staggered.
const ALIVE = 0;
const STAGGERED = 1;

// IMP — the Doom-2016 Glory Kill loop, a per-entity state machine driven by impact. A blessed
// handle at module scope (pure data), NOT a function that returns one.
const impLifecycle = defineImpactEvent("salvage:imp-lifecycle", { tag: "imp" }, (impact) => {
  const t = impact.target;
  const stagger = t.state("stagger"); // per-instance modder state (IR ref)

  // Mutually-exclusive gated groups (independent evaluation; the author guarantees exclusivity).
  return [
    // A hit while STAGGERED = a Glory Kill: instant death. (Health/ammo drops to the SOURCE are
    // deferred — grant is out of v1 scope; see roadmap.)
    {
      when: stagger.eq(STAGGERED),
      do: [t.playAnim("glory_kill"), t.despawn()],
    },
    // First drop below 30% max health, while still alive and not yet a kill → FALTER.
    {
      when: t.healthAfter
        .le(t.maxHealth.times(0.3))
        .and(stagger.eq(ALIVE))
        .and(t.healthAfter.gt(0)),
      do: [t.setState("stagger", STAGGERED), t.playAnim("falter")],
    },
    // Normal death path (killed outright, never staggered). The `healthBefore.gt(0)` makes this
    // the kill EDGE — it fires only on the crossing hit, not on every subsequent hit while the imp
    // persists through its 1200 ms despawn window (a bare `healthAfter.le(0)` would re-fire).
    {
      when: t.healthBefore.gt(0).and(t.healthAfter.le(0)).and(stagger.eq(ALIVE)),
      do: [t.playAnim("death"), t.despawn({ afterMs: 1200 })],
    },
  ];
});

// ZOMBIE — health<=0 is NOT death. Only a gib-level overshoot kills; otherwise the zombie DOWNS
// and resurrects. No per-entity state — the unfloored healthAfter carries enough (a large
// negative = a big overshoot = a gib).
const zombieLifecycle = defineImpactEvent("salvage:zombie-lifecycle", { tag: "zombie" }, (impact) => {
  const t = impact.target;
  // GIB is a LEVEL, DOWN is an EDGE — the asymmetry is deliberate.
  //   lethal: fire whenever the overshoot is this deep, from ANY state — you can gib an
  //           already-downed (0-HP) zombie, so this must NOT be edge-guarded.
  //   downed: fire only on the hit that CROSSES 0 from alive (`healthBefore.gt(0)`). A downed
  //           zombie persists at 0 HP through its 3000 ms resurrect window; a bare level gate
  //           would re-flop (re-enqueue `setHealth`, restart "down") on every hit in that window.
  const lethal = t.healthAfter.le(-40); // big overshoot → truly dead (gib), from any state
  const downed = t.healthBefore.gt(0).and(t.healthAfter.le(0)).and(lethal.not()); // the down edge
  return [
    { when: lethal, do: [t.playAnim("gib"), t.despawn()] },
    { when: downed, do: [t.playAnim("down"), t.setHealth(t.maxHealth, { afterMs: 3000 })] }, // resurrect
  ];
});

export function setupLevel(): LevelManifest {
  return {
    reactions: [],
    events: [impLifecycle, zombieLifecycle],
  };
}

// === GUARDRAILS — these MUST NOT compile. ===================================
defineImpactEvent("salvage:invalid-imp-effect", { tag: "imp" }, (impact) => {
  const stagger = impact.target.state("stagger");
  // @ts-expect-error per-entity state is an IR ref — no live JS math (use .plus(), not `+`).
  const bad = stagger + 1;
  void bad;
  // @ts-expect-error setState takes a NumberValue, not a string — state fields are numeric.
  impact.target.setState("stagger", "on");
  return [];
});
