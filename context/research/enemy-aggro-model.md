# Enemy Aggro / Perception Model — Design Intent

> **Status:** design intent / forward-looking — **NOT a ready spec.** Records that enemy
> aggro/perception is meant to grow past today's two scalars into a multifaceted model, and the
> seams already placed for that growth. Built **incrementally, with real consumers** —
> demand-driven, never ahead of need. The candidate dimensions below are an owner-prioritized
> menu, not a plan.

## The current floor

Aggro today is two descriptor scalars on `components.ai` (`AiDescriptor`,
`crates/foundation/src/data_descriptors/types/combat.rs`). Both are **descriptor-owned tuning** —
no FGD/worldspawn surface; maps never override them. Metres.

| Scalar | Role | Default | Semantics (verified in `ai.rs`) |
|---|---|---|---|
| `detectionRange` | **sense radius** | 16 | `idle → alert` when player-to-agent XZ distance ≤ this. Acquisition-gated (think-stride). |
| `leashRange` | **disengage radius** | 50 | `alert → idle` when player-to-agent XZ distance > this. Acquisition-gated. |

The FSM core is `evaluate_transition` in `crates/postretro/src/scripting/systems/ai.rs`; both edges
compare `distance_xz(player_pos, agent_pos)`. Defaults live in
`sdk/behaviors/reference/entities.{ts,luau}`.

These two scalars are the **v0 floor** of aggro, not the ceiling. Detection is a bare radius (no
line-of-sight, no sound); disengage is a single distance with no memory or search.

## Open question: does acquisition range belong to the engine floor or to authored guards?

`context/plans/in-progress/E10--behavior-state-graph/index.md` deliberately deferred this question
to this doc. Three findings from that plan's review panel are concrete evidence for it — recorded
here so the successor spec doesn't re-derive them.

**Finding A — leash is retention-only; `leashRange < detectionRange` oscillates forever.** The leash
is consulted only for an already-retained target (`crates/postretro/src/scripting/systems/ai/mod.rs`
~line 387; `targeting.rs` ~line 42). Fresh acquisition via `select_target` applies no range limit.
Traced with `detectionRange: 18`, `leashRange: 8`, pawn at distance 10: tick N (`idle`, not engaged)
acquires fresh with no leash check, `idle → alert` fires; tick N+1 (`alert`, now engaged) the
retained target at 10 > leash 8, so it's dropped and the state stands down to `idle`; tick N+2 is
identical to tick N. Permanent 2-tick oscillation — `set_destination`/`clear_destination` thrash every
other tick, plus animation flicker once velocity accumulates. `AiDescriptor::validate`
(`crates/foundation/src/data_descriptors/types/combat.rs` ~lines 301–327) checks each range is finite
and positive but imposes no ordering between them, so nothing rejects or warns. Pre-existing: inherited
from the pre-graph FSM, reproduced exactly by the lowered graph. `ai_tests.rs` ~line 746
(`detection_sets_agent_destination_and_leash_clears_it`) uses exactly this tuning and stops one tick
before the oscillation would appear. Net: acquisition has no range gate while retention has one, and
the two are authored as independent numbers with no enforced relationship.

**Finding B — targeting never reads health, so a downed co-op pawn holds an enemy hostage.**
`target_candidate` (`crates/postretro/src/scripting/systems/ai/targeting.rs` ~lines 27–44) admits any
entity with `PlayerMovementComponent` + `Transform` and never reads `HealthComponent`. Aliveness is
checked only by the attack gate (`selected_target_alive`, `mod.rs` ~line 547). In co-op: an enemy
acquires pawn A, A is downed (HP 0, still present — `ai_tests.rs` ~line 1263 documents pawns persist
after death), the enemy stays engaged, so `acquired_target` retention keeps re-resolving A every tick.
It never damages A (gate blocks it) and never re-ranks toward live pawn B standing next to it — the
switch hysteresis (`is_meaningfully_closer`, 1.0 units) makes it worse, since even on an
acquisition-due tick B must be meaningfully closer than a corpse the enemy is standing on. The sim
already has an aliveness notion for this — `alive_players` /
`player_is_present_for_trigger_occupancy` in `crates/postretro/src/sim/mod.rs` ~lines 266–273 — which
AI targeting doesn't share. Pre-existing.

**Finding C — a `chaseTarget` state with no exit guard is a level-wide pursuer, with no diagnostic.**
Authored graphs carry `leash_range: None` by design (the plan's deliberate v1 decision), and
`select_target` has no range limit, so a state authored with `motion: "chaseTarget"` and no
disengagement guard validates cleanly and yields an enemy that pursues from anywhere on the level. The
plan's stated v1 answer is that an authored graph owns both engagement and disengagement through its
own guards. Compounding it: `@brain.targetDistance` uses a `1.0e9` no-target sentinel, so `gt`/`ge`
distance guards read true when there is no target while `le`/`lt` read false — a disengagement guard
written only as `gt(targetDistance, N)` behaves differently from one gated on `@brain.hasTarget`. This
is the plan's explicitly deferred case, not pre-existing.

Together: acquisition currently has no range gate anywhere in the engine floor, while retention has
one that only fires after engagement — leaving the open question of whether a floor-level acquisition
range belongs on `AiDescriptor` (the plan rejected `leashRange` reuse for this as "a second spelling
of disengagement that silently outranks the guards") or whether acquisition/disengagement should stay
entirely in authored guards, with better validation and diagnostics for cases like Finding C.

## Seams already placed

- **`select_target(registry, from, visible)` chokepoint** — the ready plan
  `context/plans/ready/E10--enemy-mp-target-selection/` lands the targeting-policy plug point,
  replacing the single-pawn `player_position` (`ai.rs:282`). Its injectable
  `visible: Option<impl Fn(EntityId) -> bool>` predicate is **where perception slots in**; the plan
  anticipates it widening `bool → weight` for graded ranking (bias by perceptual proximity), a
  widening of the same chokepoint, not a relocation.
- **The `visible` predicate resolves progressively.** Exact eye-to-target LOS via BVH ray queries is
  the roadmap's *"Enemy line-of-sight + cover"* bullet; a view-independent Cell→Cell broad-phase is
  the substrate in `context/research/cell-visibility-substrate.md` (a cheap gate before the exact
  raycast). Same seam, resolvers added demand-driven.
- **The taste axis is already named.** `context/lib/scripting.md` §1: *"Every feel detail — movement
  accel, view sway, **enemy aggression**, difficulty pacing — lives on a spectrum; the engine bakes
  in no point on it."* Aggression is explicitly a descriptor-exposed spectrum, staged
  demand-driven — *"breadth grows with demand, not ahead of it."*

## Candidate dimensions — illustrative, NOT committed

An owner-prioritized menu of where richness could land, not a roadmap. Each waits for its own real
consumer.

| Dimension | Sketch | v0 today |
|---|---|---|
| Stimulus-based detection | sight (LOS cone) + sound (gunfire/footstep noise events) | pure sense radius |
| Threat / target prioritization | most-recently-damaged, lowest-HP, highest-threat policies over `select_target` | nearest (v1) |
| Alert propagation / pack aggro | one enemy aggroing wakes nearby allies | none (per-enemy) |
| Per-archetype aggression profiles | cautious / aggressive / skittish temperament as a descriptor axis | one profile |
| Memory & persistence | search-last-known-position, forget timer, re-aggro on re-sight | leash (crude disengage) |

## Boundary discipline

Richness grows on the **descriptor surface** (taste), staged demand-driven — *"breadth grows with
demand, not ahead of it."* Every exposed knob is an API contract: *"Engine parameters exposed as
scripting primitives carry API contracts"* (`index.md` §2, *Primitive surface is a contract*) —
changing a semantic or range updates SDK types, validators, and defaults in the same pass. Targeting
correctness, determinism, and the `select_target` plumbing stay **engine-owned** — the floor, which
has no spectrum.
