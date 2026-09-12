# Combat Movement and Tactics — API Design Notes

> **Read this when:** starting the spec for richer enemy movement / combat tactics, or for AI
> co-op companions. Do this **after** the `gameplay-stack-decomposition` epic lands the
> `postretro-ai` crate (its M4) — and after re-grounding this note against the reorganized code.
> **Status:** Pre-spec design exploration. No draft spec exists; none is authored until the
> re-architecting lands and this note is re-verified. Grounded against pre-reorg `main`. The
> re-architecting is behavior-preserving — it moves and renames code without changing function —
> so the *design direction* here survives it, but every identifier named below is orientation only
> and must be re-confirmed before a spec cites it. No file paths are load-bearing.
> **Related:** `context/lib/entity_model.md` §7c (Enemy Brain Component) · `context/lib/scripting.md`
> §11 (Typed Command Buffer), §12 (Reaction Dispatch) · `context/lib/build_pipeline.md`
> §Navigation bake · `context/plans/drafts/gameplay-stack-decomposition/` · roadmap.

---

## Goal

Two horizons, one primitive.

- **Near-term:** make enemy movement more interesting — reactive positioning (strafe/kite, flank,
  cover), coordinated crowds, gap/barrier traversal, and short committed tactics ("fire, strafe 2m,
  fire"; "fire ×3, seek cover") chosen by reading the moment.
- **North star:** reliable AI co-op companions. No companion/partner-AI concept exists today; the
  shipped "co-op" is networked *human* play only.

The design thesis: **both fall out of one generalized motion primitive** — an engaged, reactive
position goal offset from a *dynamic anchor* (a hostile target, a friendly ally, or a world point),
retaining its combat slot, with the offset computed as IR over brain facts. Enemies are the
near-term consumer; companions the north-star consumer of the same API.

## Constraints this design must hold (engine philosophy)

These are floors, not preferences (`scripting.md` §1, §11; `entity_model.md` §7c):

- **Scripts declare; Rust executes.** No author code runs at tick time. Expressiveness comes from a
  richer *vocabulary* and richer *facts*, never from shipped behavior code. New verbs are closed;
  new facts extend an append-only table.
- **Guards evaluate every tick; nothing latches evaluation off.** A commitment window is an authored
  guard over time-in-activity, never an engine mechanism.
- **IR is pure, total, bounded.** No wall-clock, no unseeded RNG, no unbounded loops.
- **Graph evaluation is host-only.** Clients consume replicated state; any randomness is
  host-authoritative and replicated (`co-op-triggers-trap-pools.md`: host-only RNG posture).
- **Fact tables are append-only.** A name's position is its runtime read handle; insert/reorder
  silently re-points every bound program.

## Current surface (pre-reorg grounding — re-verify before drafting)

The enemy brain is an authored hierarchical statechart. A leaf selects a motion verb, an optional
action, and animation; guards are IR over a brain-local fact scope, evaluated outer-to-inner,
wildcard rows first, first true row wins. Identifiers as of pre-reorg `main`:

| Surface | Today |
|---|---|
| **Motion verbs** (`MotionVerb`) | `ChaseTarget`, `MoveToAnchor`, `MoveToLastKnown`, `Patrol`, `Hold`, `Freeze`. Position goals = `MoveToAnchor`/`MoveToLastKnown`/`Patrol`. |
| **Action verbs** (`ActionVerb`) | `Attack(name)` only — names an entry in the graph's `attacks` map. No non-attack action. |
| **Engaged steering** | `ChaseTarget` already steers to the resolved **combat slot** (a standoff-ring position), not straight to the target. Facing decouples from movement (faces target while holding position elsewhere). Standoff radius is a fixed attack param, not IR-computed. No dedicated strafe/circle/orbit verb — lateral offset arises only from which ring slot is assigned. |
| **Combat-slot resolution** | Enemies spread on a ring around the target: 8 directions × 3 radial multipliers at the engagement radius, batch-assigned with min-spacing de-duplication so no two share a slot. |
| **Position-goal anchors** | Fixed world points only. `home_anchor` is the spawn point, fixed for the brain's life; `Patrol` points are anchor-relative; last-known is a snapshot `Vec3`. **The anchor is never a live entity reference.** |
| **Selector layers** | Priority / first-applicable only. No weighting, no stochastic choice. |
| **Committed phases** | Authored as a linear nested graph gated on `timeInActivityMs`. No sequence/phase opcode. |
| **Action cadence** | One shot per commit. No burst/repeat param on the attack verb. Cooldown is an engine timer surfaced as a fact. |
| **Brain facts** (append-only) | `hasTarget`, `targetDistance`, `timeInActivityMs`, `attackCooldownMs`, `acquisitionDue`, `health`, `maxHealth`, `targetHealth`, `targetMaxHealth`, `targetDied`, `distanceFromAnchor`, `targetHostile`, `targetReachable`, `attacksFiredInActivity`, `targetVisible`, `timeSinceDamageMs`, `timeSinceTargetVisible`, `distanceToLastKnown`, `damageBearing`, `damageSourceKnown`. Plus per-entity `@state.*` numeric leaves. |
| **Seeded RNG** | Absent from the brain scope. A seeded engine RNG exists elsewhere (weapon-spread cone sampling, host-only) but is not wired to any brain fact; enemy shots fire straight. |
| **Nav / traversal** | One edge kind (walkable adjacency, bounded by step height). No jump/off-mesh link, no agent jump capability, no airborne locomotion, no cover data. Reachability is adjacency-only pathfinding. Off-mesh links and cover were deliberately scoped out of the nav bake as future additive extensions. |

## The design: three layers

The seeds decompose into three layers that all lower onto the existing statechart IR. Only the
verbs and a handful of facts touch the engine floor; sequences and selection are largely SDK sugar.

**Layer 1 — Verbs.** The closed motion/action vocabulary a leaf selects. This is the one layer with
real engine-floor cost (steering, nav). See the unified primitive below.

**Layer 2 — Sequences.** An ordered, committed micro-program ("fire → strafe 2m → fire"). Authored
via imperative-looking SDK sugar that **lowers to a linear nested graph** with step-completion
guards. No sequence opcode is added to the engine — the sugar emits the same nested-graph IR the
engine already runs, so per-tick evaluation and totality hold by construction. Commitment is the
*absence of inner bail-out rows*; an *outer wildcard* (target died, health critical) still preempts
the combo every tick. The syntax must make **both** ends first-class: the happy-path chain and the
escape hatches.

**Layer 3 — Selection ("read the room").** A declared repertoire of tactics, each with an
applicability guard; prune the inapplicable, then choose. Pruning already exists (selector-layer
guards). "Sometimes X, sometimes Y" is the only new part, and it reduces to **one new seeded-random
brain fact** (see Tension B) read by ordinary priority guards — no new selector policy required.

## The unified motion primitive

Today four verbs bake in their anchor semantics: `ChaseTarget` (anchor = hostile target, engaged,
routes through the combat slot), `MoveToAnchor`/`Patrol`/`MoveToLastKnown` (anchor = fixed point,
non-engaged, cannot act). The engaged position-goal-at-a-slot concept therefore **already exists**
for the hostile-target anchor. The unified primitive generalizes it:

> **Position goal = offset(anchor, IR params), with `engaged` retaining the combat slot and the
> ability to act.** `anchor ∈ { fixed point, hostile target, friendly entity }`. Offset params
> (standoff distance, bearing, lateral/tangential bias) are IR over brain facts, not baked
> constants.

What this buys, and what it costs:

- **Strafe / kite while shooting** — standoff already holds via the combat slot; the missing pieces
  are (a) IR-computed standoff (kite harder when hurt = `standoff = f(health)`) and (b) continuous
  tangential motion (orbit) rather than a static slot. (a) is the new IR-output path; (b) is new
  steering that composes with the existing ring.
- **Flank / bearing approach** — a bearing-offset term on the goal, or a slot-ranking preference for
  the target's exposed side. New (no target-facing fact exists today).
- **Companion follow** — anchor = the player entity, offset behind/beside. Needs the **friendly
  dynamic anchor**, the single most important companion-specific extension.
- **Crowd spacing** — already provided by ring + min-spacing slot assignment. Largely done.

## The two invariant tensions — and their resolutions

**Tension A — an imperative sequence vs. "guards evaluate every tick; nothing latches."**
Resolved by lowering, not by a new mechanism: the sequence is SDK sugar that compiles to a linear
nested graph (Layer 2). The author writes something imperative-looking; the IR is the reactive
statechart, evaluated every tick, total by construction. Commitment = no inner bail rows;
interruptibility = outer wildcard rows. This is the §11 move ("looks like a function, constructs
IR") applied to control flow.

**Tension B — "sometimes" vs. determinism + no unseeded RNG + host-only replication.**
Resolved by exposing **one new brain fact, `@brain.roll`**: a seeded, host-authoritative,
replicated value in `[0,1)`, **re-drawn on activity entry** (stable within an activity, like
`timeInActivityMs` resets on entry). Stochastic choice then becomes an ordinary priority guard
(`roll < 0.3`) — no weighted-selector policy needed. It extends the existing runtime host-only
seeded-RNG posture (weapon spread) into the brain scope. The load-bearing decision is the
**refresh cadence**: per-activity-entry avoids per-tick flicker while giving one fresh decision per
"read the room" moment.

## Sugar vs. net-new — the verdict per behavior

The encouraging result: most seeds are sugar plus append-only facts. Ranked by engine-floor cost.

| Behavior | Verdict | What's new |
|---|---|---|
| Crowd spacing / coordination | **Exists** | Ring + min-spacing slot assignment already spreads engaged enemies. Refinements only. |
| "Fire ×3 then X" | **Sugar** | Guard on the existing `attacksFiredInActivity` fact. |
| Sequences ("fire → strafe → fire") | **Sugar + 1 fact** | SDK sequence syntax lowering to a nested graph; plus a new `displacementInActivity` fact so a step can end after "2 meters" (displacement-since-entry is absent today). |
| "Read the room" pruning | **Exists** | Selector-layer applicability guards. |
| "Sometimes X / sometimes Y" | **1 fact** | The `@brain.roll` seeded fact (Tension B); selection stays priority guards. |
| Strafe / kite while shooting | **Steering + IR output** | IR-computed standoff (needs the brain scope to resolve an *output*, which it does not today) and tangential/orbit steering over the existing slot. |
| Flank / bearing approach | **Steering + fact** | Bearing-offset goal term or slot-ranking preference; likely a target-facing/exposed-side fact. |
| Companion follow-a-player | **Anchor generalization** | Position-goal anchor becomes a live entity reference, offset by IR. |
| Cover-seeking | **New floor (nav)** | Baked cover points (scoped out of the nav bake as a future additive extension) + a cover fact + a cover-move verb. |
| Jump / gap / barrier traversal | **New floor (nav + sim)** | See below — the heaviest item. |
| Companion command channel | **New input source** | See companions below — a non-perception input the brain scope has no equivalent of. |

## Traversal (jump) is a separate axis — new engine floor, not API enrichment

Unlike the planar behaviors, jumping across gaps / over barriers is net-new construction spanning
the navigation bake and agent locomotion — i.e. `postretro-sim`/nav work that the movement API only
*selects into*. Absent today: any off-mesh/jump link kind in the nav graph; any agent jump
capability descriptor (the agent is a single canonical ground-fit capsule; player jump is a
separate, non-AI system); any airborne/ballistic locomotion (steering never authors vertical
motion); and jump-aware reachability. The nav format was explicitly designed to take off-mesh links
*additively*, so pathfinding/funnel/steering should not need a rewrite — but the link kind + bake
producer, a per-archetype jump-capability descriptor, and a leap-trajectory locomotion state are
real new work. **This axis is the strongest argument for the deferral:** it pours new floor into
exactly the sim/nav/ai subsystems the re-architecting is carving. Sequence it after the crates
settle.

## Co-op companion through-line

A companion is, structurally, an enemy brain with allied sentiment (mutable per-pair sentiment
already supports allied; peers are already candidates) plus two net-new capabilities:

1. **Dynamic friendly anchor** — the unified primitive's friendly-entity anchor (follow/cover the
   player, offset by IR). Shared with interesting enemies; not companion-only.
2. **Player→companion command channel** — "hold here," "follow," "attack that." The brain scope's
   facts are all engine-computed perception/state; a player command is an *external input* with no
   equivalent today, and it has a crate-placement question (netcode vs. sim vs. ai) because a remote
   client's command to their companion crosses the wire while graph evaluation stays host-only.

Reliability (the hard part of "reliable partner") is mostly downstream of these plus good facts
(not blocking doorways, reachability, target sharing) — not a new planning system. The brain stays
reactive; the engine floor owns nav/steering quality.

## Design directions decided this session

- One unified motion primitive (position goal = offset(dynamic anchor, IR), engaged-capable);
  enemies near-term, companions north-star, same API.
- Three layers: verbs (engine floor) / sequences (SDK sugar → nested graph) / selection (guards +
  one seeded fact).
- Sequences lower to nested graphs; commitment via outer-wildcard preemption, not a new latch.
- Stochastic choice via a single seeded, replicated, per-activity-entry `@brain.roll` fact + priority
  guards.
- Traversal is a separate, later axis (new nav/sim floor), sequenced after the re-architecting.

## Syntax mockups

Illustrative author-facing spellings live in `combat-movement-examples/` (built on the real
`components.behavior` surface; non-compiling; each file names the fork it probes — see that folder's
README). They exist to pressure-test ergonomics before drafting; they are not content and decide
nothing on their own.

## Open questions — settle at draft time (post-reorg re-grounding)

Each tagged with whether it changes the *shape* of the design or only the *size* of the work.

- **IR output from the brain scope** *(shape)*. IR-computed motion params require the brain scope to
  resolve an *output*; today the brain and candidate scopes are read-only and resolve none. Decide:
  widen the brain scope to a write/resolve path, or add a dedicated motion-parameter scope (cf. the
  movement-local scope in §11). This is the single largest architectural fork.
- **`@brain.roll` refresh cadence** *(shape)*. Per-activity-entry is the working proposal; confirm it
  against replication (host-authoritative, deterministic per entity) and against selectors that want
  a fresh roll without an activity change.
- **Verb collapse vs. verb addition** *(shape)*. Collapse `ChaseTarget`/`MoveToAnchor`/`Patrol`/
  `MoveToLastKnown` into one anchored position-goal verb, or add new engaged verbs (Strafe/Orbit/
  Flank/Cover) alongside them? Collapse is cleaner but a wider change to the closed vocabulary.
- **Command-channel placement** *(shape)*. Which crate owns the player→companion command input, and
  how it reaches host-only graph eval over the wire.
- **Orbit/tangential steering model** *(size)*. Time-varying combat-slot assignment vs. a tangential
  velocity term on the engaged goal.
- **Cover data** *(size)*. Auto-detected at bake vs. mapper-placed; the fact + verb shape follows.
- **Sequence escape-hatch ergonomics** *(size)*. How the sugar surfaces outer-wildcard interrupts so
  authors get commitment without accidentally building an uninterruptible combo.
- **Facts to add** *(size, append-only)*. `displacementInActivity`, `@brain.roll`, a target-facing /
  exposed-side fact for flanking, cover facts. Confirm none already exist post-reorg.

## Non-goals (this design line)

- Drafting a spec now. Deferred until the re-architecting lands and this note is re-grounded.
- Any change to the `gameplay-stack-decomposition` epic. This design consumes the `postretro-ai`
  boundary; it does not reshape it.
- Author-shipped behavior code, a live VM at tick time, or unbounded/while-style IR. The reactive,
  total statechart stays the model.
- Building companions before interesting enemies. Enemies prove the shared primitive first.
