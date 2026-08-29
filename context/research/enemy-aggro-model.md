# Enemy Aggro / Perception Model — Design Intent

> **Status:** design intent / forward-looking — **NOT a ready spec.** Records that enemy
> aggro/perception is meant to grow past today's floor into a multifaceted model, and the
> seams already placed for that growth. Built **incrementally, with real consumers** —
> demand-driven, never ahead of need. The candidate dimensions below are an owner-prioritized
> menu, not a plan.

## The current floor

The legacy `components.ai` / `AiDescriptor` two-scalar floor this section described (`detectionRange`,
`leashRange`) was retired — `context/plans/done/E10--retire-legacy-ai/`. Today's floor is the
behavior-graph candidacy model: `context/lib/entity_model.md` §7c.

## Open question: does acquisition range belong to the engine floor or to authored guards?

Answered and shipped by `context/plans/done/E10--enemy-aggro-model/` — both, split by tier: the
floor's leash bounds acquisition for legacy `components.ai`, and an authored graph spells its own
radius as a distance clause in its candidate filter. The durable answer is
`context/lib/entity_model.md` §7c ("Candidacy is per-graph eligibility", "the engine holds no
acquisition leash"). The findings that motivated it (leash-oscillation, co-op targeting, and
unguarded `chaseTarget` pursuit) are historical review-panel evidence, superseded by the shipped
model.

## Seams already placed

- **`select_target(registry, from, visible)` chokepoint** — shipped by
  `context/plans/done/E10--enemy-mp-target-selection/`, which lands the targeting-policy plug point,
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

## Enemy-on-enemy damage (friendly fire)

Whether and how much one enemy's attack damages another is a per-game, per-faction-pair policy,
owned by the **Faction & relationship model** (`roadmap.md`, Epic 10), not the engine floor.
Enemy melee applies damage directly to the selected target, so it never strikes a bystander.
Enemy-on-enemy impacts become possible only once enemy **ranged** attacks land: a nearest-of
hitscan ray can put a bystanding enemy in the line of fire. That is the prerequisite tracked in
`context/research/enemy-ranged-attacks.md`. The faction model's per-pair relation is the
declarative surface that would then govern whether such an impact deals damage. Design intent only;
no descriptor surface here.

## Boundary discipline

Richness grows on the **descriptor surface** (taste), staged demand-driven — *"breadth grows with
demand, not ahead of it."* Every exposed knob is an API contract: *"Engine parameters exposed as
scripting primitives carry API contracts"* (`index.md` §2, *Primitive surface is a contract*) —
changing a semantic or range updates SDK types, validators, and defaults in the same pass. Targeting
correctness, determinism, and the `select_target` plumbing stay **engine-owned** — the floor, which
has no spectrum.
