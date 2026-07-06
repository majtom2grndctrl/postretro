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
