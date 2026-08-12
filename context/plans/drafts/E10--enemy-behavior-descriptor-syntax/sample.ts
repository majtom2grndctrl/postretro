// ─────────────────────────────────────────────────────────────────────────────
// SPECULATIVE VISION — not a contract, does not compile against today's SDK.
//
// The reference enemy as a HIERARCHICAL FSM (Harel statechart), in the
// Vue-Composition authoring style. Purpose is twofold:
//   • inform E10--enemy-behavior-descriptor-syntax (what the near-term vocabulary
//     should NOT preclude), and
//   • seed roadmap entries for the larger behavior-architecture specs that follow.
// Scope tags mark which layer each construct belongs to:
//   [E10]   this spec (position-goal motion, distance/reachability/hostility facts,
//           patrol, faction) — buildable now, flat.
//   [ATK]   the multi-attack draft (attacks-as-handles, offense selection).
//   [HFSM]  a future architecture spec: composite activities, orthogonal layers,
//           scoped transitions, entry/exit actions.
//   [CMB]   further out: combat stances (telegraphed, attack-gating) and a
//           FEAR-style planner over the same graph.
//
// Statechart concepts used, and their PostRetro spelling:
//   superstate / composite state   → a composite `activity` (engage)
//   orthogonal regions (concurrent)→ `layers` (the spec already names this the
//                                     "parallel layers" future extension; a flat
//                                     graph is "one layer of a future set")
//   substate                       → a leaf `activity` (patrol, retreat)
//   superstate transition          → a transition declared ON the composite; it
//                                     is scoped to it, so it can't fire elsewhere
//   entry / exit action            → onEnter / onExit (the telegraph seam)
//
// Naming locked this session: `agent` is the entity context; `stance` is RESERVED
// for combat (telegraphed posture that gates available attacks); `action` stays the
// leaf verb; `motion` stays locomotion. `activity` is the level-1 word (swap with
// `routine`/`goal` freely — it is the one still-open name).
// ─────────────────────────────────────────────────────────────────────────────

import { defineEntity } from "postretro";
import {
  defineBehavior,  // the setup() factory: (agent) => the whole brain
  defineActivity,  // an FSM node; leaf or composite (composite carries `layers`)
  defineAttack,    // [ATK] a wieldable/attack handle, referenced by identity
  when,            // guarded choice: when(guard, value) — layer entry or transition
  motion,          // motion-verb handles: patrol / chase / hold / moveToAnchor / freeze
} from "postretro/behavior";

// [ATK] Attacks are declared handles. `damage/range/cooldownMs` today; `weapon:`
//       (enemy-as-wielder) is additive later.
const claw = defineAttack({ damage: 8, range: 2,  cooldownMs: 1200 });
const spit = defineAttack({ damage: 4, range: 12, cooldownMs: 800  });

export const sentry = defineEntity({
  canonicalName: "sentry",
  components: {
    health: { max: 70 },
    mesh: { model: "sentry.glb" },

    // `agent` is the composition context: it carries this entity's engine-computed
    // facts (agent.targetDistance, …) AND its own mutable per-entity reads
    // (agent.flag("faction")) and self-actions (agent.play(…)). One namespace for
    // "me"; `candidate` below is the only other entity a guard ever sees.
    behavior: defineBehavior((agent) => {
      // ── Breathing room: name guards once, fluent nodes. `.and/.or/.not` compile
      //    to `select` today (or ride the deferred and/or/not opcodes). ──
      const hostileInSight = agent.hasTarget.and(agent.targetHostile).and(agent.acquisitionDue);
      const standDown      = agent.hasTarget.not().or(agent.targetHostile.not()); // [E10] faction seam
      const pastLeash      = agent.distanceFromAnchor.gt(20);                      // [E10] leash is authored
      const atHome         = agent.distanceFromAnchor.le(1);
      const inClaw         = agent.targetDistance.le(2);
      const inSpit         = agent.targetDistance.between(4, 12).and(agent.targetReachable); // [E10]

      // ── Leaf activities: no target stake, a single motion. ──
      const patrol = defineActivity({
        anim: "walk",
        motion: motion.patrol,                                   // [E10]
        route: { mode: "pingPong", points: [[0, 0], [6, 0], [6, 6]] }, // anchor-relative [E10]
      });

      const retreat = defineActivity({
        anim: "walk",
        motion: motion.moveToAnchor,                             // [E10] position-goal motion
      });

      // ── [HFSM] engage: a COMPOSITE activity. Two orthogonal `layers` run
      //    concurrently while it is active — the creature closes distance AND
      //    picks an attack in the same tick. This is the level the flat model
      //    flattened: today `alert` (chase) and `strike`/`lob` (attack) are peer
      //    states; here chase is the `move` layer and attacking is the `offense`
      //    layer of ONE activity.
      const engage = defineActivity({
        layers: {
          // locomotion region: hold at melee, else close the gap. First match wins;
          // the bare trailing value is the fallback.
          move: [ when(inClaw, motion.hold), motion.chase ],
          // [ATK] offense region: fire the first attack whose range matches. No
          // fallback → the layer is idle (still chasing via `move`) when out of range.
          offense: [ when(inClaw, claw), when(inSpit, spit) ],
        },
        anim: "attack",                    // anim derives from the active offense; see history note
        onEnter: agent.play("alert-roar"), // [CMB] entry action — the telegraph seam
      });

      // ── Transitions. Declared where they belong in the hierarchy. ──
      patrol.on(hostileInSight, engage);
      retreat.on(atHome, patrol)
             .on(hostileInSight, engage);       // re-aggro mid-retreat

      // [HFSM] scoped superstate transitions: these live ON engage, so they fire
      // ONLY while engaged. `standDown → patrol` is the old any-state interrupt,
      // now correctly scoped — it can't touch patrol, so patrol never oscillates
      // (this is the clean form of the F1 fix the flat spec had to convention around).
      engage.on(pastLeash, retreat)            // leash pulls out of combat
            .on(standDown,  patrol);           // target gone or turned friendly

      return {
        initial: patrol,
        moveSpeed: 3,
        activities: { patrol, engage, retreat }, // names inferred from keys (naming sugar)
        attacks: { claw, spit },                 // [ATK]

        // [E10] fresh-acquisition eligibility; a callback over the per-candidate
        // scope, never re-gating a retained target.
        candidateFilter: (candidate) => candidate.distance.le(24).and(candidate.alive),
      };
    }),
  },
});

// ─────────────────────────────────────────────────────────────────────────────
// SCOPE MAP — what each layer tells which spec
//
// [E10] buildable now, FLAT (no composite/layers): the facts (distanceFromAnchor,
//   targetReachable, targetHostile), moveToAnchor/patrol motion + route, the
//   authored leash, the fresh-scan faction filter behind agent.targetHostile.
//   The E10 spec should keep the descriptor envelope from precluding the [HFSM]
//   shape — i.e. don't bake "a state is a leaf with one motion+action" as a hard
//   contract; leave room for a node to become composite.
//
// [ATK] the multi-attack draft: attacks-as-handles and the `offense` selection.
//   Reconcile its `attacks` map onto handles referenced by identity.
//
// [HFSM] a NEW roadmap spec — "Hierarchical behavior (statecharts)": composite
//   activities, orthogonal `layers`, transitions scoped to a composite, entry/exit
//   actions, and history (does re-entering `engage` resume the prior layer choice?
//   — pin it there, out of scope here). This is the spec that dissolves the flat
//   any-state-interrupt scoping hack and sets up the planner seam.
//
// [CMB] further roadmap: combat `stance` (a telegraphed posture that CONSTRAINS
//   which attacks the offense layer may pick until the creature commits to switching
//   — the reserved word), onEnter/onExit telegraph animations, and a FEAR-style
//   planner replacing hand-authored transition order (`select_transition` is the
//   pinned seam). GOAP/utility scoring plug in here without changing the node model.
//
// Naming decisions carried forward: agent (context) · activity (level-1 node, swap
// with routine/goal) · motion (locomotion) · action (leaf verb) · layers (orthogonal
// regions) · stance (RESERVED, combat) · flag/… (the not-"state" word for per-entity
// mutable reads — still open).
// ─────────────────────────────────────────────────────────────────────────────
