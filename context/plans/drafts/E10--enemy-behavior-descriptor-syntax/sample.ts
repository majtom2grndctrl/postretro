// ─────────────────────────────────────────────────────────────────────────────
// SPECULATIVE EXPLORATION — not a contract, does not compile against today's SDK.
//
// Re-envisions the enemy behavior descriptor in a Vue-Composition-API style,
// carrying the vocabulary from E10--enemy-behavior-descriptor-syntax (position-goal
// motion, distance/reachability/hostility facts, patrol, faction) PLUS the pinned
// multi-attack map. Purpose: see how the whole reference enemy reads under the
// composition idiom before committing an authoring-surface change. Discard after
// the direction is chosen.
//
// Three moves from the shipped composition work (scripting-state-convergence,
// E16--per-player-currency, descriptor-identity-and-naming-sugar) applied here:
//   1. Guards join the fluent node algebra   — brain.targetDistance.le(2)
//                                               (not runtime.le(brain.targetDistance, 2))
//   2. Structure becomes declared handles     — states/attacks are objects, not strings
//   3. A setup() body gives breathing room    — declare intermediates, return the graph
// ─────────────────────────────────────────────────────────────────────────────

import { defineEntity } from "postretro";
import {
  defineBehavior, // the setup() factory: (self) => graph
  defineState,    // FSM node handle; .on(guard, target) wires a transition
  defineAttack,   // wieldable/attack handle (pinned attacks-map, by identity)
  when,           // any-state interrupt: when(cond, targetState) — same verb as the store `when`
  motion,         // motion-verb handles: motion.patrol / .chase / .hold / .moveToAnchor / .freeze
  brain,          // engine-computed fact handles, now fluent NumberRef/BoolRef nodes
  candidate,      // per-candidate fact handles (fresh-acquisition scope)
} from "postretro/behavior";

// A character's attacks are declared wieldables, referenced by identity — the way
// `stores: [store]` passes the handle, not a name. `damage/range/cooldownMs` today;
// `weapon:` (enemy-as-wielder) drops in additively later.
const claw = defineAttack({ damage: 8, range: 2,  cooldownMs: 1200 });
const spit = defineAttack({ damage: 4, range: 12, cooldownMs: 800  });

export const sentry = defineEntity({
  canonicalName: "sentry",
  components: {
    health: { max: 70 },
    mesh: { model: "sentry.glb" /* animation states: idle, walk, throw, attack, death */ },

    // ── The behavior is a setup() body. `self` is the composition context: this
    //    entity's own per-entity reads (self.state("…") → @state.<name>). Everything
    //    declared here is scratch; only what the `return` exposes becomes the graph
    //    (the shipped "unreturned declarations disappear" contract).
    behavior: defineBehavior((self) => {
      // ── Breathing room: name the guards once, reuse them across transitions.
      //    Fluent nodes, the same algebra impact policies use
      //    (impact.target.healthBefore.gt(0).and(…)). `.and()/.not()` compile to
      //    `select` today; they ride the deferred and/or/not opcodes when those land.
      const sighted   = brain.targetDistance.le(16).and(brain.acquisitionDue);
      const inClaw    = brain.targetDistance.le(2);
      const inSpit    = brain.targetDistance.between(4, 12).and(brain.targetReachable);
      const pastLeash = brain.distanceFromAnchor.gt(20); // leash is authored, not an engine field
      const atHome    = brain.distanceFromAnchor.le(1);

      // ── States are handles, not string keys. No name is typed here — the name is
      //    inferred from the binding when the state is exposed in the return
      //    (`states: { patrol, … }`), the naming-sugar rule applied to graph nodes.
      //    `action:` takes the attack handle directly; its type marks it as an attack
      //    reference, so there is no bare `action: "attack"` string. `anim:` stays a
      //    string — it names a mesh animation state the behavior can't type-check.
      const patrol  = defineState({ anim: "walk",   motion: motion.patrol });       // untargeted resting state
      const alert   = defineState({ anim: "walk",   motion: motion.chase  });
      const strike  = defineState({ anim: "attack", motion: motion.chase, action: claw });
      const lob     = defineState({ anim: "throw",  motion: motion.hold,  action: spit });
      const retreat = defineState({ anim: "walk",   motion: motion.moveToAnchor });

      // ── Transitions: fluent guard, handle target. Wired after every state exists,
      //    so the cyclic references (alert⇄strike, …) resolve by identity, no
      //    temporal-dead-zone dance and no typo-able "to" string. `.on()` chains.
      patrol.on(sighted, alert);
      alert.on(inClaw, strike)
           .on(inSpit, lob)
           .on(pastLeash, retreat);
      strike.on(brain.targetDistance.gt(2), alert)
            .on(pastLeash, retreat);
      lob.on(inClaw, strike)
         .on(pastLeash, retreat);
      // Retreat is non-engaged: its only exit is distance-from-home, never a
      // target-distance edge (a dropped target is handled by the interrupts below).
      retreat.on(atHome, patrol);

      return {
        initial: patrol,     // handle — the untargeted resting state, and the stand-down destination
        moveSpeed: 3,
        engagementRadius: 2,

        // Deterministic anchor-relative route; the anchor is the spawn position, so
        // the same descriptor patrols correctly wherever it is placed.
        patrolRoute: { mode: "pingPong", points: [[0, 0], [6, 0], [6, 6]] },

        // Fresh-acquisition eligibility (never re-gates a retained target). Fluent,
        // over the per-candidate scope.
        candidateFilter: candidate.distance.le(24).and(candidate.died.not()),

        // Any-state stand-downs target `patrol` (== initial), so they are skipped
        // in `patrol` itself and never oscillate it. Retention is authored here, not
        // an engine floor rule — the targetHostile fact is the mutable-faction seam.
        interrupts: [
          when(brain.targetHostile.not(), patrol), // target turned friendly → disengage
          when(brain.hasTarget.not(),     patrol), // lost the target        → resume patrol
        ],

        // Exposed by identity, names inferred from the keys (naming sugar).
        states: { patrol, alert, strike, lob, retreat },
        attacks: { claw, spit },
      };
    }),
  },
});

// ─────────────────────────────────────────────────────────────────────────────
// The write side stays where it belongs — guards only READ. Hostility is mutable
// STATE, not a fixed archetype: a reaction or impact policy flips this entity's
// faction through the same fluent store algebra, and the behavior above simply
// observes it via brain.targetHostile. Sketch of a "pacify on scripted event":
//
//   defineReaction("pacify", (self) => set(self.state("faction"), 0));  // now allied with faction 0
//
// and a stagger interrupt reads a state the impact side writes:
//
//   // in the graph:  flinch.on(self.state("staggered").eq(1), …)
//   // in impact:     defineImpactEvent({ tag: "sentry" }, (i) =>
//   //                  [ when(i.amount.gt(THRESHOLD), set(i.target.state("staggered"), 1)) ])
//
// Nouns select state (self.state("…")), verbs describe use (set/update/when, .on),
// and "State" names a stored slot while a fact like targetHostile is computed —
// the same naming discipline the store surface holds to.
// ─────────────────────────────────────────────────────────────────────────────
