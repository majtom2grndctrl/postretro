# Impact-Derived Death Lifecycle (E16)

## Goal

Death is the first policy domain on the impact-policy substrate (`E16--impact-policy-substrate`). The engine holds NO death concept keyed to HP: reaching 0 HP is inert (no auto-removal, no forced state), removal is the modder's `despawn`, and the engine's kill report fires off that despawn — not off 0 HP. A mod authors what a hit *means* — kill, overkill, stagger, down-and-resurrect — as data over impact facts; this spec makes the engine's remaining death machinery (the death sweep, the `death_handled` latch, kill credit, `playerDied`, `alive_players`) consistent with that thesis, and ships the reference death policies. It adds **no** new authoring/SDK surface — engine semantics plus reference policies over the substrate's surface.

## Prerequisites

- **`E16--impact-policy-substrate`** — the impact dispatch source, the per-entity-state keystone + composite binding, the effects (including the deferred-effect component's ordered queue / inert flag and the end-of-frame removal pass this spec's kill report hooks), the `defineImpactEvent` authoring surface, and the evaluator. The fixtures here import that folder's `proposed.d.ts` via `postretro/proposed` (this folder's `tsconfig.json` maps it; single source of truth for the WALL).
- Shipped: Epic 10 (weapon + health/damage foundation, death sweep) · Epic 14 (behavior-IR substrate) · Epic 15 through Phase 3.5 (state replication).

## Scope

### In scope
- **0 HP made inert.** Reaching 0 HP no longer auto-removes an entity or forces a death state. Removal happens only via an explicit modder `despawn` effect.
- **Kill report decoupled from HP.** The sweep still latches `death_handled` and captures kill *credit* at first-0-HP, but the kill *report* (`on_entity_killed` / scoring) fires only when a modder `despawn` actually removes the entity. No despawn → no engine kill. `playerDied` stays keyed to 0 HP (players respawn, they don't despawn).
- **Resurrect re-arm.** `setHealth` clears the `death_handled` latch AND the pending kill credit, so a downed-then-resurrected entity re-arms kill detection and is never reported for the down it recovered from.
- **Reference death policies.** The two fixtures — the grunt handle-model baseline + override (`arena-death.spike.ts`) and the zombie/imp lifecycles (`lifecycle.spike.ts`), the imp's per-entity stagger machine showcasing the keystone in a death context.

### Out of scope
- **Fine-grained downed-enemy AI.** Coarse engagement disable reuses the existing aggro toggle, which is **tag-wide** (`updateEnemyState` resolves tags, not instances) — so v1 can disengage a downed *tag* but not a single downed instance. Per-instance downed disable and nuanced downed behavior are deferred.
- **DoT / environmental death.** App-drain-sourced impacts run no policy in v1 (the substrate's producer-contexts decision), so authored death does not yet fire for DoT/environmental damage — the roadmap follow-on un-stubs that producer.
- **Crediting the source (rewards).** `grant(...)` defers to the `resource-grant chokepoint + dev-mod reference` roadmap item; the substrate publishes `source` as a command-target token, no v1 effect targets it.
- **Anything the substrate already scopes out** — per-entity state replication, opening the FSM state set, a general deferred-effect scheduler, spatial `zone` filtering, healing via the damage path.

## Decisions

Pinned semantics (no TBD). The general substrate decisions — tokens-not-leaves, independent gated groups, override precedence, derived identity, producer contexts, effect arms, despawn ordering, unfloored `healthAfter`, level-vs-edge gating, per-entity-state schema, manifest composition — live in `E16--impact-policy-substrate` and are not restated here.

- **FSM coexistence (v1).** (a) The FSM's 0-HP→Death transition and auto-despawn are removed; 0 HP is inert. (b) The FSM retains idle/alert/attack/death, but its `death` state is **no longer HP-reachable**. A modder `despawn` effect quiesces a brain entity by setting the per-entity **inert flag** the AI tick early-outs on (the substrate's effects task builds both) — steering hold, no attack, no animation re-request, so the modder's `playAnim` owns the death presentation — and enqueues removal on the deferred-effect component. It does NOT enter `LogicalState::Death` or use `BrainComponent.death_despawn_remaining_ms`; the FSM `death` state stays defined for future non-HP use. (c) Modder overlay states (stagger/downed) live in per-entity state, invisible to the FSM. (d) Coarse engagement disable for a downed/dead enemy reuses the existing aggro toggle, which resolves by **tag, not instance** (`updateEnemyState`) — v1 can disengage a downed *tag* but not a single downed instance; per-instance downed AI is deferred (Out of scope).
- **Resurrect & kill re-arm.** The engine's resurrect recovery keys off `brain.state == Death`, not the `death_handled` latch; `death_handled` is set only by the death sweep and is never cleared today. Once 0-HP→Death is removed, the `brain.state` recovery no longer fires from HP restoration — so the load-bearing re-arm is `setHealth` resetting `death_handled`, which re-enables kill detection for a resurrected entity. "Preserve resurrect recovery" means preserve the `death_handled` re-arm, not the now-vestigial FSM-state recovery. Under the kill-report decoupling below, `setHealth` also discards the pending kill credit — a downed entity that stands back up was never killed, so nothing of the down survives to be reported.
- **The kill report fires off despawn, not off 0 HP.** The thesis says death is the modder's despawn — so the engine may not report a kill at 0 HP. Split latch from report: at first-0-HP the sweep still latches `death_handled` and **captures the kill credit** (contributor-ledger snapshot + tags) into a pending structure latched beside it, but emits nothing. Latching credit at first-0-HP is safe from corpse-hit theft: `apply_damage_with_context` records contributors only while `!death_handled`, so the ledger is frozen from the latch on. The **report** (`on_entity_killed` → progress/scoring) fires when a modder `despawn` effect actually removes the entity, in the substrate's end-of-frame removal pass, using the latched credit. Consequences, all deliberate: a policy that leaves an entity at 0 HP produces **no engine kill** (the modder counts it via a store write if wanted); a `despawn({ afterMs: N })` kill scores at removal, ~N ms after the kill edge; a despawn of a never-latched entity (removed above 0 HP — e.g. the substrate's breakable crate) reports nothing; and only the removal pass emits — direct `registry.despawn` callers (level teardown) never report kills. Together with the re-arm above this kills resurrect-inflation: a zombie no longer counts as a kill per down-cycle; only a down that actually ends in removal reports, exactly once. `playerDied` is exempt — it stays keyed to 0 HP in the sweep's player branch, unchanged.

## Acceptance criteria

- [ ] An entity reduced to 0 HP by a policy that omits `despawn` **remains present** (not auto-removed); a later frame can still observe and act on it.
- [ ] A `despawn()` removes the entity at end-of-frame — a same-group `playAnim` still plays; `despawn({ afterMs: N })` removes it ~N ms later.
- [ ] A brain entity with a pending `despawn` goes **inert** — it stops steering and attacking for its death-anim window, and its modder `playAnim` is not overwritten by an FSM animation request (no `LogicalState::Death` entry).
- [ ] Zombie fixture: an entity at `healthAfter.le(0)` above the gib threshold plays a "down" clip, is not removed, and regains health after the delay via `setHealth(x, { afterMs })`; below the gib threshold it despawns.
- [ ] Kill edge: a policy whose death gate is `healthBefore.gt(0).and(healthAfter.le(0))` counts `deaths.add(1)` and plays its death anim **exactly once** even when the entity is hit again while it persists through its `despawn` window; a bare `healthAfter.le(0)` level gate would re-fire (the fixtures use the edge).
- [ ] A resurrected entity (via `setHealth`) can be killed again — its `death_handled` latch is cleared, so kill detection re-arms.
- [ ] Kill report off despawn: for a normal kill+despawn policy, the engine kill report (`on_entity_killed` with the credit latched at first-0-HP) fires **exactly once, at the removal** — for `despawn({ afterMs: N })`, ~N ms after the kill edge. An entity left at 0 HP by a policy that omits `despawn` produces **no** engine kill however many sweeps it persists (replaces the former plain-entity kill-counted-once semantics; the plain branch still latches `death_handled` so credit is captured once and corpse hits record nothing).
- [ ] Resurrect never inflates kills: a downed-then-resurrected entity is not reported for that down (`setHealth` discards the pending credit); a later genuine re-kill re-latches, and its eventual despawn reports exactly once.
- [ ] A despawn of a never-latched entity (removed while above 0 HP) emits no kill report; direct `registry.despawn` callers outside the removal pass never emit one.
- [ ] `playerDied` fires exactly once at the player's first 0-HP sweep, unchanged — the player branch stays HP-keyed (players respawn, they don't despawn).
- [ ] `alive_players` treats a latched-but-undespawned 0-HP player as present (presence follows despawn, not HP). (`alive_players` is an internal set — assert the predicate directly or via trigger occupancy, not through pure gameplay.)
- [ ] HUD health and replication behave unchanged for a normal kill+despawn; scoring counts the same kills, now timed to removal. (Kill-counted-once and HUD-reads-0 are runnable; "replication unchanged / remote pruned on despawn" is a review/regression gate — a no-regression claim.)
- [ ] The two spike files (`arena-death.spike.ts`, `lifecycle.spike.ts`) type-check with `postretro/proposed` resolved to the substrate folder's `proposed.d.ts`; once the substrate's authoring surface ships they type-check against the *shipped* SDK unchanged.

## Tasks

### Task 1: 0 HP inert + resurrect re-arm
Remove auto-removal at 0 HP. In `sweep_deaths`, keep the plain-non-player branch's tag/ledger capture but delete its immediate `registry.despawn`; because the entity now persists at 0 HP, **add the `death_handled` latch to the plain branch exactly as the brain branch does** (read the latch, skip if set, else set it before capturing tags/ledger) — otherwise pass 1 re-collects the undespawned entity every tick and re-counts its kill. Gate the AI-tick auto-despawn: the FSM no longer transitions to `Death` from 0 HP; with nothing HP-driven entering `Death`, nothing seeds `death_despawn_remaining_ms`, so the AI-tick despawn countdown goes vestigial — a modder despawn no longer routes through it (removal and the inert-quiesce are owned by the substrate's deferred-effect component and inert early-out, not this brain field). Correct the resurrect model: the existing `brain.state == Death` recovery becomes vestigial once the 0-HP→Death transition is gone (nothing puts a brain into `Death` from HP); the load-bearing re-arm for kill *detection* is resetting the `death_handled` latch (set only by the sweep, never cleared today) — extend the substrate's `setHealth` chokepoint to clear it (Task 2 extends the same write to discard the pending kill credit). Reconcile the single `alive_players` occupancy predicate: a latched-but-undespawned 0-HP player counts as present (presence follows despawn, not HP). Verify HUD and replication are untouched (they key off the health component's floored value and the despawn event, not the latch). `ai.rs` and `health.rs` are large — make surgical edits along the existing seams; do not restructure.

### Task 2: Kill report off despawn
Move kill-report emission from the 0-HP sweep to the despawn that actually removes the entity (Decisions: kill report fires off despawn). Today `sweep_deaths` (`crates/postretro/src/scripting/systems/health.rs:89-185`) captures tags + a `ContributorLedgerSnapshot` at the latch and pushes them into `DeathReport.killed_tags`/`killed_contributor_ledgers`, which `run_death_sweep` (`crates/postretro/src/sim/mod.rs:816-833`, called at `:311`) feeds through `ProgressTracker::on_entity_killed` (`crates/scripting-core/src/reaction_dispatch.rs:59`) into the death-event drain. Rework in three moves:
- **Latch credit, emit nothing.** At first-0-HP, both non-player branches (brain, and the plain branch Task 1 latches) still set `death_handled` and capture the credit — but into a **pending-kill-credit slot latched beside `death_handled`** (tags + ledger snapshot), not into the `DeathReport`. The sweep's non-player output shrinks to the latch + pending credit; `player_died` and the player ledger stay as they are. Corpse-hit theft is already impossible: `apply_damage_with_context` records contributors only while `!death_handled` (`crates/entities/src/components/health.rs:330`), so the credit frozen at the latch is the credit.
- **Report at removal.** In the substrate's end-of-frame removal pass — the single sink both immediate and elapsed-countdown despawns feed — removing a marked entity takes its pending credit, if any, and emits the kill report there: tags → `ProgressTracker::on_entity_killed`, ledger snapshot alongside, resulting events fired through the same death-event drain `run_death_sweep` uses today. No pending credit (the entity never latched — despawned above 0 HP) → remove silently. Only this pass emits; direct `registry.despawn` callers do not.
- **Resurrect clears the pending credit.** Extend the `setHealth` chokepoint (beyond Task 1's `death_handled` clear) to discard the pending credit — a downed-then-resurrected entity is never reported; a later genuine re-kill re-latches fresh credit at its new first-0-HP, and the eventual despawn reports it once. This is what kills resurrect-inflation (a zombie counted per down-cycle).
`playerDied` is untouched: the sweep's player branch stays keyed to 0 HP. Scoring/progress consumers are unchanged in shape — same `on_entity_killed` feed, same event drain — but the timing follows removal (a `despawn({ afterMs: 1500 })` grunt scores ~1500 ms after the kill edge). Where the pending credit lives (a field beside `death_handled` on `HealthComponent`, or a sim-side map keyed by entity) is the implementer's call; it must be cleared by `setHealth` and consumed exactly once by the removal pass.

## Sequencing

**After the substrate** (`E16--impact-policy-substrate`, all five tasks — this spec's Task 1 needs its effects/inert early-out; Task 2 hooks its removal pass).
**Phase 1:** Task 1 (0 HP inert + resurrect re-arm) — the sweep unfusing and latch semantics Task 2 restructures further.
**Phase 2:** Task 2 (kill report off despawn) — consumes Task 1's everywhere-latched sweep and the substrate's removal pass.

## Boundary inventory

None — this spec adds no new names on any boundary. The substrate spec's Boundary inventory is the complete authoring/wire surface; this spec is engine semantics (sweep/latch/report/`setHealth` internals) plus fixtures written against that surface.

## Script syntax examples

See the fixtures, split by job: `arena-death.spike.ts` (the handle model — grunt baseline + tag-narrowed override + policy reuse), `lifecycle.spike.ts` (the keystone in a death context — per-entity state, zombie resurrect, the Doom-2016 stagger/glory-kill machine). Both import the authoring surface from the substrate folder's `proposed.d.ts` (`postretro/proposed`). Canonical shape:

```ts
// Proposed design — the death policy is data over impact facts.
const gruntImpactEvent = defineImpactEvent({ tag: "grunt" }, (impact) => [
  // The KILL EDGE, not the level `healthAfter.le(0)` — else this group re-fires on every hit
  // while the corpse persists through its despawn window, double-counting `deaths` (see the
  // substrate's level-vs-edge decision).
  { when: impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0)), do: [
      impact.target.playAnim("death"),                     // presentation
      deaths.add(1),                                        // consequential store write
      impact.target.despawn({ afterMs: 1500 }),            // engine does not auto-remove at 0 HP
  ]},
]);
// A map refines the same handle in a different scope; merged by derived id, last-registered wins.
gruntImpactEvent.override({ tag: "arena_grunt" }, (impact) => [ /* reuse base + extra store write */ ]);
```
