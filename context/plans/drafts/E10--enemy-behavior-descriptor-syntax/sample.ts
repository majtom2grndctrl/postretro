// ─────────────────────────────────────────────────────────────────────────────
// SPECULATIVE VISION — not a contract, does not compile against today's SDK.
//
// The reference enemy as a hierarchical FSM (Harel statechart) authored in the
// composition-over-IR style. Two jobs:
//   • inform E10--enemy-behavior-descriptor-syntax (what the near-term flat spec
//     must NOT foreclose, and which slice of the dialect E10 itself ships), and
//   • seed the roadmap specs that build the rest.
//
// Scope tags:
//   [E10-fact]   facts/motion/faction E10 builds now (flat).
//   [E10-syntax] the fluent guard spelling E10 *could* adopt now (owner call).
//   [ATK]        the multi-attack draft (attacks-as-handles, offense selection).
//   [HFSM]       a future spec: recursive composites, layers, scoped transitions.
//   [CMB]        further out: combat stances (telegraphed, attack-gating), planner.
//
// ── The one envelope, recursively ────────────────────────────────────────────
// A brain, a composite activity, and a stateful layer are all the SAME shape:
//     { initial, activities, transitions }
// The brain is the root composite. `transitions` is an adjacency map: keys are
// activity names (or "*"), values are ordered `on(guard, target)` rows, first true
// wins. There is NO `.on()` mutation — the returned maps ARE the graph:
//   • referenced-but-unreturned handle  → pathed LOAD ERROR (never a silent vanish)
//   • returned-but-unreferenced activity → included, + a "unreachable activity" lint
//   • identity is the cross-reference currency; a map key is only this descriptor's
//     local name for a handle (a shared handle may be keyed differently per entity).
// Scoping falls out of nesting: "*" at a level = "while this composite is active"
// (the flat model's `interrupts`, generalized); outer scope beats inner. Because a
// row can only target handles returned AT ITS OWN LEVEL, a parent's source-keyed
// rows reach SIBLINGS (exits) and a nested "*" reaches CHILDREN (internal) — disjoint,
// which also forecloses Harel cross-boundary transitions (a decided [HFSM] cut).
//
// ── Guard hazard, stated precisely ───────────────────────────────────────────
// Guards compile to serializable IR; they never run as live JS. Native `&&`/`||`
// over two node objects does NOT throw and does NOT collapse to a constant — it
// evaluates to the RIGHT-HAND node, silently DROPPING the other conjunct, yielding
// a legal-looking still-dynamic guard that lost a clause. Use `.and()/.or()/.not()`
// (compile to `select` until the and/or/not opcodes land). A `no-native-boolean-ops-
// on-nodes` lint is owed on the [HFSM] task list.
//
// Naming locked: `brain` = the state machine (one per entity) · `behavior` = its
// component slot and emergent whole · `activity` = a node (one of the many behaviors
// the brain composes) · `agent` = entity context · `motions` = motion-handle catalog ·
// `on` = the ordered guarded row (NOT `when`, which keeps its shipped GatedEffect
// meaning) · `layers` = orthogonal regions · `action` = leaf verb · `stance` RESERVED
// for combat · `memory` = interim read-view of E16 per-entity `@state` (see below).
// `animation` never abbreviated.
// ─────────────────────────────────────────────────────────────────────────────

import { defineEntity } from "postretro";
import {
  defineBrain,     // (agent) => the root composite { initial, activities, transitions } — a state machine
  defineActivity,  // an FSM node; a leaf, or composite via `layers`
  defineAttack,    // [ATK] a wieldable/attack handle, referenced by identity
  on,              // on(guard, target): one ordered guarded row (transitions AND layer rows)
  motions,         // motion handles: patrol / chase / hold / moveToAnchor / freeze
} from "postretro/behavior";

// [ATK] Attacks are declared handles; each carries its OWN clip (claw and spit must
//       not animate alike). `weapon:` (enemy-as-wielder) is additive later.
const claw = defineAttack({ damage: 8, range: 2,  cooldownMs: 1200, animation: "claw-swipe" });
const spit = defineAttack({ damage: 4, range: 12, cooldownMs: 800,  animation: "spit"        });

export const sentry = defineEntity({
  canonicalName: "sentry",
  components: {
    health: { max: 70 },
    mesh: { model: "sentry.glb" },

    // `agent` carries this entity's facts and its own reads/actions. `agent.target.*`
    // mirrors `impact.target.*`; every `agent.target.*` ATOM auto-conjoins existence
    // (compiles to `select(agent.target.exists, atom, false)` — per-atom, so an
    // `.or(anchorClause)` is NOT suppressed when untargeted), which retires the old
    // 1e9-sentinel foot-gun. `agent.target.exists` is exempt; `.not()` over a guarded
    // atom reads true untargeted (documented, shown in guard-diagnostics);
    // `agent.target.rawDistance` is the escape hatch.
    behavior: defineBrain((agent) => {   // the `behavior` component is defined by a brain (the machine)
      // ── Breathing room: guards named once, fluent nodes. ──
      const hostileAcquired = agent.target.hostile.and(agent.acquisitionDue);   // [E10-fact] exists auto-conjoined
      const standDown       = agent.target.exists.not().or(agent.target.hostile.not());
      const pastLeash       = agent.distanceFromAnchor.gt(20);                  // [E10-fact] leash is authored
      const atHome          = agent.distanceFromAnchor.le(1);
      const inClaw          = agent.target.distance.le(2);
      const inSpit          = agent.target.distance.between(4, 12).and(agent.target.reachable); // inclusive [E10-fact]

      // ── Leaf activities. `motion:` is sugar for `layers: { move: [motions.x] }`,
      //    so flat is a strict specialization of the layered form, not a 2nd schema.
      //    `speed` is per-activity — the locomotion speed while this activity moves
      //    (patrol strolls, engage sprints, a tanky retreat power-walks); attach it to
      //    a motion handle instead if one activity ever needs several. `animation`
      //    (locomotion clip) stays on leaves until the E21 pose-stack composes
      //    per-layer clips; a composite (engage) therefore carries none. ──
      const patrol = defineActivity({
        animation: "walk",
        motion: motions.patrol,                                       // [E10-fact]
        speed: 3,                                                     // a stroll
        route: { mode: "pingPong", points: [[0, 0], [6, 0], [6, 6]] }, // per-activity, anchor-relative
      });

      const retreat = defineActivity({
        animation: "walk",
        motion: motions.moveToAnchor,                                 // [E10-fact] position-goal motion
        speed: 5,                                                     // hustle home
      });

      // ── [HFSM] engage: a COMPOSITE with two orthogonal layers running at once.
      //    A layer is a selector list here (stateless, per-tick). Motion layers
      //    require a trailing fallback (type-enforced) — locomotion is never
      //    undefined; attack layers may have none (idle = no attack this tick). ──
      const engage = defineActivity({
        layers: {
          move:    [ on(inClaw, motions.hold), motions.chase ],       // fallback: chase
          offense: [ on(inClaw, claw), on(inSpit, spit) ],            // [ATK] no fallback → idle
        },
        speed: 6,                                                     // sprint (governs the move layer's chase)
        onEnter: [ agent.playClip("alert-roar") ],                    // [CMB] telegraph seam (mechanism is [HFSM])
      });

      return {
        initial: patrol,
        activities: { patrol, engage, retreat },   // membership + naming authority
        attacks: { claw, spit },                   // [ATK] authority for attack handles
        candidateFilter: (candidate) => candidate.distance.le(24).and(candidate.alive), // `alive` = sugar for the death latch (not a health test)

        // Adjacency map. Keys typed `Record<keyof activities | "*", Row[]>`, so a
        // typo'd key (`patorl`) fails excess-property checking at author time; targets
        // stay identity-valued. Source-keyed rows target SIBLINGS (exits).
        transitions: {
          patrol:  [ on(hostileAcquired, engage) ],
          retreat: [ on(atHome, patrol), on(hostileAcquired, engage) ],   // re-aggro mid-retreat
          engage:  [ on(pastLeash, retreat), on(standDown, patrol) ],     // leash outranks standDown (declaration order)
          "*":     [ /* root-scoped interrupts — empty: stand-down is correctly scoped
                        to `engage`, so patrol/retreat never see it. This is the F1 fix,
                        structural rather than by convention. */ ],
        },
      };
    }),
  },
});

// ─────────────────────────────────────────────────────────────────────────────
// [HFSM/CMB] When a layer needs its OWN state (attack windup → commit → recover,
// and the [CMB] `stance` gate: a posture that constrains which attacks offense may
// pick until the creature commits to switching), the layer value stops being a
// selector list and becomes a nested graph — the SAME envelope, recursively:
//
//   offense: {
//     initial: windup,
//     activities: { windup, commit, recover },
//     transitions: {
//       windup:  [ on(agent.timeInActivity.ge(300), commit) ],   // telegraph window
//       commit:  [ on(agent.timeInActivity.ge(120), recover) ],
//       recover: [ on(agent.timeInActivity.ge(500), windup) ],
//       "*":     [ on(agent.memory("staggered").eq(1), recover) ],  // targets a CHILD (internal)
//     },
//   }
//
// The parent `engage` row `on(standDown, patrol)` targets a SIBLING (exit); this
// nested `"*"` targets a CHILD (recover). Disjoint by the membership rule — no
// ambiguity about which fires — and the reason cross-boundary transitions are
// foreclosed.
//
// SCOPE MAP — what each layer owes which spec
//
// [E10-fact]   buildable now, FLAT: distanceFromAnchor / target.reachable /
//   target.hostile facts, moveToAnchor + patrol motion, per-activity `route`, the
//   authored leash, the fresh-scan faction filter behind `agent.target.hostile`.
//   HAND-OFF: the E10 draft currently authors the patrol route as a GRAPH-WIDE block;
//   the endpoint is per-activity (two patrol loops is an obvious ask, and no composite
//   cleanly owns a graph-wide block). Move it onto the patrol activity in E10 now —
//   cheap while there is one patrol activity, authored-content churn if deferred.
//   Same for `moveSpeed`: the draft carries one graph-wide speed; per-activity is the
//   endpoint (patrol strolls, chase sprints), same cheap-now / churn-later trade.
//
// [E10-syntax] the fluent guard spelling (`agent.target.distance.le(2)`) is the cheap
//   near-term slice of "behavior joins the unification": lift the pre-wrapped brain
//   leaves into the shipped NumberRef/BoolRef algebra (the bridge the convergence
//   spec already built for impact facts) — no `defineActivity`, no `on()`, no engine
//   change. OWNER CALL: does E10 ship this now, or author its reference graphs in the
//   old `runtime.le(brain.x, n)` prefix dialect and re-author later? If deferred,
//   every E10 reference graph is written twice.
//
// [ATK] the multi-attack draft: attacks-as-handles, per-attack `animation`, and the
//   `offense` selection layer. Reconcile its `attacks` map onto identity handles.
//
// [HFSM] a NEW roadmap spec — "Hierarchical behavior (statecharts)": the recursive
//   `{ initial, activities, transitions }` envelope; layers (selector | nested graph)
//   with `motion:` sugar and typed motion-fallback; `"*"` scoping with outer-beats-
//   inner priority; the membership authority + unreachable-activity lint; the
//   `no-native-boolean-ops-on-nodes` lint; and two DECIDED CONSTRAINTS to state, not
//   inherit silently: (a) cross-boundary transitions are foreclosed (same-level
//   targets only; a composite's source-keyed exit rows already cover "leave from
//   anywhere inside"); (b) layer history — does re-entering a composite resume its
//   nested graph, or restart at `initial`? — pin it here.
//
// [CMB] further roadmap: combat `stance` (telegraphed posture gating available
//   attacks — the reserved word), onEnter/onExit telegraph clips, and a FEAR-style
//   planner replacing hand-authored `on(...)` order (the `select_transition` seam);
//   GOAP/utility plug in without changing the node model (which is why `goal` is NOT
//   spent on FSM nodes — the planner needs it).
//
// Open name still parked: `memory` is the interim behavior-scope READ-VIEW of the
// same E16 per-entity `@state` fields impact policies and reactions WRITE through
// `setState` — one storage, read-only here by construction (guards never write).
// `agent.memory("faction")` is explicitly interim: when the relation model lands,
// allegiance is engine-modeled (declaration surface, `@candidate.faction`, relation
// facts) and the guard read that matters is already the `agent.target.hostile` fact.
// ─────────────────────────────────────────────────────────────────────────────
