# Player Movement

> **Read this when:** designing or extending player movement — states, abilities, the author/tuning surface, or anything in the `movement--*` spec series.
> **Key invariant:** movement is custom kinematic, engine-internal, and authored declaratively. The player is a capsule whose velocity the engine sets each tick — never a simulated rigid body, never script-driven per tick.
> **Related:** [Entity Model §5 update order, §7 collision, §7b component](./entity_model.md) · [Scripting](./scripting.md) · [Input](./input.md) · series spec: `plans/done/movement--state-machine/`

---

## 1. Foundation: custom kinematic, not simulated

Movement is hand-authored kinematic motion in the Quake/Doom lineage, not rigid-body dynamics.

- **Collision queries only.** `parry3d` provides shape-casts and intersection tests ("sweep this capsule along this vector — what does it hit?"). That is the whole physics dependency the player uses.
- **No `rapier3d`.** The rigid-body solver (forces, mass, momentum, restitution, constraints) is deliberately absent. Velocity is authored every tick, then collide-and-slid against world geometry. Nothing simulates where the player "ends up."

**Why.** Game-feel for a power-fantasy shooter wants *designed* motion curves, not emergent ones. Custom kinematics is exactly what lets a dash be tuned across a range — from a deterministic, reproducible designed burst (a configured speed for a configured duration, then control handed back; a solver fighting drag could not guarantee it) to a fluid, momentum-composing impulse — all authored, none emergent from a physics solver. Custom kinematics is also trivially deterministic in the fixed game-logic tick (no solver-iteration order to tame), and the genre's signature feels — air-strafing, bunny-hopping, the floaty-but-tight arc — are *artifacts* of a custom model, not of physical realism. Every modern movement-shooter (Doom Eternal, Titanfall/Apex, Ultrakill) uses this same custom-kinematic foundation; the modern part is the state set on top, not the physics.

A real constraint solver may eventually earn a *scoped, opt-in* place (e.g. a deferred grapple's swinging rope), never as the movement foundation.

**View feel is render-side, not tick-side.** First-person view feel — head bob, strafe tilt, ambient sway — reads the followed pawn's velocity and grounded flag, writes nothing back. It runs at render rate. Its integrator state lives engine-side, never on the movement component or the per-tick interpolation state — tick state stays position-only. Entry: `view_feel.rs`.

## 2. Author surface: declarative

Authors tune and compose movement through descriptor data — never per-tick imperative script.

States live natively in Rust. Authors control **which states exist, their tuning, and their transition triggers** as descriptor fields and (eventually) declarative transition rules. They do **not** write per-tick movement callbacks.

**Why.** Movement runs every tick in the fixed game-logic step (`entity_model.md` §5, update order 1, before camera follow). Driving state logic through QuickJS/Luau per tick would add FFI cost and determinism risk on the hottest path. It also holds the standing invariant that movement is engine-internal — scripts cannot read or write the movement component through `worldQuery` (`entity_model.md` §7b). The seam is shaped so a future script-driven path could resolve behind it without reshaping callers, but that path is not built.

### The shape of the surface

In its full form the declarative surface is a **transition graph** assembled from three engine-owned, *closed* vocabularies. Closed does not mean small — a closed vocabulary can be arbitrarily expressive (`scripting.md` §11). What it forbids is author-shipped runtime code: the engine owns the evaluator, authors describe behavior as data. Tuning ships first; author-defined transitions follow across the series.

| Vocabulary | Author picks | Engine owns |
|---|---|---|
| **States** | which native states exist + their tuning | each state's per-tick velocity intent (Rust) |
| **Triggers** | a boolean combination (`all`/`any`) over a closed predicate set — `grounded`, `airborne`, `touchingWall`, `<input>Edge`, `speedAbove/Below`, `cooldownReady`, `chargesRemaining`, `elapsedMs` | predicate evaluation |
| **Carry-rules** | the velocity transform applied at a transition edge — `keepHorizontal`, `keepBoost`, `zero`, `projectOntoWallPlane`, `reflect`, `scale` | the transforms + momentum-conservation consistency (§6) |

A transition is one data row — `{ from, to, when, carry }`. The author wires native states into chains the engine never pre-built (dash → wall-run → wall-launch); the velocity math under each state and each carry-rule stays native.

**The line — one test:** a declarative element is anything the engine can evaluate each tick *from data alone*, with no author-shipped runtime code, no per-tick movement callback, and no reads outside the movement component. What is forbidden is a retained author function the engine calls each tick — that reintroduces the FFI/determinism cost on the hottest path and breaks the fixed-tick determinism and momentum-carry invariant. The constraint is on *who owns the evaluator*, not on how expressive the vocabulary is.

A richer authored surface — response curves, a velocity expression like `boost = f(speed, charges, grounded)` — is reachable *without* an author function: the **Typed Command Buffer** (`scripting.md` §11) lets the author describe such behavior as a typed, engine-evaluated IR that crosses the FFI as data. Movement may adopt it later (a transition graph, response curves) and it is the canonical mechanism if so. Nothing here commits to it now. New predicates and carry-rules are still added deliberately, vetted against the flexibility band (§3) — never via a general escape hatch.

## 3. The flexibility band

The author surface targets a *band* of expressiveness, not a point.

We don't couple what we build to a single use case. We distill movement mechanics to first principles and expose them through an approachable, composable API. A dash cooldown and a dash with charges each suit different games — the surface should be expressive enough that either is implementable.

| Edge | Rule |
|---|---|
| **Floor (min flexibility)** | A modder can compose the native states, declarative transitions, and tuning into movement *unlike* the specific game being built — without the engine having pre-built that exact mechanic. |
| **Ceiling (max generality)** | The vocabulary stays FPS-shaped. Never a content-agnostic movement/physics framework that caters to nothing. |

**Stress-test references.** Ultrakill and Neon White (slide-cancel chaining, dash routing, "movement *is* the game") are the **ceiling references**. The yardstick: can the declarative surface compose this? If the surface can reach that ceiling through composition, it is flexible enough.

**Guardrail.** Primitives stay in FPS vocabulary — states (crouch, slide, wall-run, vault), velocity intent, capsule, ground/air params, ability budgets, transitions. Never generic `apply_force(body)` or `register_update_callback`. Use the band to judge a proposed descriptor field: *FPS-flexible* (expands what authors compose in FPS terms) vs *generic framework creep* (a sandbox primitive that makes the common case — a crouch, a dash — as costly as the exotic one).

## 4. State-machine seam

The movement tick splits into two halves with a clean seam between them.

- **Shared physics substrate.** Collide-and-slide: sweep-and-slide, step-up, floor-push, ground-stick, contact/landing resolution. Runs regardless of state. Takes a desired velocity, returns the resolved position plus contact results. Behavior is fixed — states change *intent*, not collision.
- **Per-state velocity intent.** Each state authors the desired velocity for the tick (gravity, acceleration, friction, bursts). The current state is an explicit value on the movement component. A dispatch point runs the active state's intent, calls the substrate, then applies any returned transition.

**Contact flows forward, not sideways.** The substrate is the sole collision-query path. A state intent that needs surface contact (wall normal, floor normal) reads it from the substrate's contact result carried forward from the prior tick — never by querying collision itself. This holds the split: intents author velocity, the substrate owns collision.

**Per-state data ownership.** Each state's live data (timers, boosts) is owned through one uniform convention; the dispatch resolves the component-vs-active-state borrow once. A new state adds its data without widening the dispatch.

Today's walk/run/jump/air-control is the baseline `Normal` state; later states (crouch, slide, wall-run, vault) plug in behind the same seam. Ability budgets (air-jump, air-dash) refresh through one landing-refresh point so they reset uniformly.

## 5. Design north-stars

| Reference | Borrow |
|---|---|
| **Doom Eternal** | Primary aesthetic match. High base speed, no ADS slowdown, dash-with-charges, double-jump, auto-mantle. Movement is a combat tool. |
| **Titanfall 2 / Apex** | Speed-preserving slide (converts sprint/downhill speed into a decaying boost rather than capping), timed wall-run, auto-mantle/vault. |
| **Ultrakill / Neon White** | Ceiling reference — can the surface compose it? (see §3) |

## 6. Cross-cutting policies to decide early

Two policies cut across every state and define the modern feel. Both are foundations, not per-state details — settle each before the specs that need it, not by emerging from one state and refactoring the rest. Settle the policy and seam, not the full breadth; breadth grows with the states.

- **Momentum conservation.** The biggest modern-feel differentiator — slide→jump keeps slide speed, wall-run→jump launches off the wall vector — and the transition seam's spine; four later states depend on it. Set the velocity-carry policy at the transition layer before `movement--slide`. Deciding it inside slide bakes in slide-shaped logic that wall-run and vault then refactor. **Decided** (`movement--cross-cutting-policies`): velocity carry is owned by the dispatch point that applies the transition, never inside a state intent. A transition's `carry` composes a horizontal-rule and a boost-rule (the base+boost velocity model) over §2's closed vocabulary; wall-relative rules (`projectOntoWallPlane`, `reflect`) land with `movement--wall-run`.
- **Input forgiveness.** Coyote time (jump grace after leaving a ledge), jump buffering (jump pressed just before landing fires on contact). Foundation-level — shapes edge-input derivation, which every state reads. Settle the edge-input model up front, not after five states consume those edges. **Decided** (`movement--cross-cutting-policies`): coyote and jump-buffer windows are descriptor-tuned, derived once per tick into edges that intents consume in place of raw button bits.

## 7. Non-goals

- Rigid-body dynamics for the player (forces, mass, restitution, constraint solving).
- Per-tick script-authored movement (imperative callbacks). The author surface is declarative.
- Map-overridable movement tuning — movement physics is descriptor-owned, never FGD KVPs (`entity_model.md` §4).
- Networked movement (prediction, rollback).
