# Faction Sentiment & Crossfire Retaliation

## Goal

Give modders an authored **faction** dimension with a directional **sentiment** table and per-pair/per-archetype **tolerance**, and prove it with the first consumer: an enemy damaged past its tolerance **reprioritizes its target onto the attacker** (Doom/Quake infighting), uniformly for any attacker. The engine owns the perception mechanism, the fact vocabulary, and the ranking formula; mod authors own the relationship *content* — faction, sentiment, tolerance, and retaliation tuning — that the engine reads, never needing a rebuild.

## Scope

### In scope

- Widen the AI candidate pool so brain-bearing enemies are selectable targets, not only player pawns — behavior-preserving.
- First-class **faction identity**: named factions declared as manifest content; per-archetype faction assignment (class default, overridable) replacing the hardcoded seed.
- **Directional sentiment table** (`from → to` scalar) authored as manifest content, resolved per candidate in the tick scan; drives the hostility offer filter, replacing the bare `faction != faction` rule. Default sentiment reproduces today's behavior.
- **Tolerance** authored two ways — per faction-pair (a second column on the sentiment table) and per archetype (an override) — resolved per (enemy, candidate).
- **Attacker history**: a bounded per-brain ledger of recent damagers `(attacker, damage, time)`, seeded at the damage chokepoint, aged per tick.
- New engine-computed **candidate-scope facts** for authored guards: `@candidate.sentiment`, `@candidate.damageDealtToMe`, `@candidate.timeSinceDamageFromCandidate`, `@candidate.tolerance`.
- An **engine-owned retaliation ranking term**: when a candidate's recent accumulated damage (from the ledger) exceeds the resolved (enemy, candidate) tolerance, the engine's target-selection rank prefers it, so a damaged enemy turns on its attacker. Shaped by authored scalar tuning (recency window, damage/recency weighting), seeded to defaults. `entity_model.md` §7c's ranking ownership stays engine-side — no authored ranking expression.
- Reference retaliation content in the dev mod proving low-tolerance archetypes reprioritize onto attackers while max-tolerance archetypes hold.

### Out of scope

- Runtime re-authoring of factions/sentiment (they commit at mod-init like other manifest data).
- Faction **membership changes at runtime** (an entity switching factions mid-session). The per-entity faction field remains engine-mutable interim storage; no authored runtime faction-set verb ships here.
- Transient stand-down *mechanism* — "drop the attacker when it dies or leaves sight." This is already expressible as authored wildcard guards over existing target-side facts (`@brain.targetDied`, `@brain.targetVisible`, `@brain.timeSinceTargetVisible`); the reference content uses them, and no new engine stand-down rule is built.
- **Authored ranking expression.** Target ranking stays engine-owned (§7c); this spec adds a retaliation *term* configured by authored data and scalars, not a `candidatePriority` IR expression. A free authored ranking vocabulary is a separate, deliberate future decision (see Alternatives rejected).
- Replication/netcode of the sentiment registry beyond what the existing manifest-drain path already provides. Faction/sentiment are host-authored content; co-op parity rides the existing content-parity gate (`networking.md`), not a new wire section.
- N-ary alliance graph semantics beyond a per-directed-pair scalar (allied = positive sentiment; the pair scalar is the whole model).
- Spatial acceleration of the widened candidate walk. The walk is O(N²) in enemy count — an accepted interim at boomer-shooter counts; spatial partitioning is a deferred perf concern, not built here.

## Direction

**Problem.** Enemies cannot perceive each other as targets — `target_offers` (`targeting.rs`) walks only `ComponentKind::PlayerMovement` holders, so an enemy's shot already lands on a bystanding peer physically (and seeds its damage-perception facts) but the victim can never *select* the attacker. The observation that produced this: crossfire already damages and is investigated (post-combat-perception), yet no infighting emerges, because target *selection* is pool- and faction-gated and neither gate admits a peer.

**Prior commitments.**
- `entity_model.md` §7c: the engine owns "which entities are offered as candidates at all," ranking, retention, hysteresis; authored candidacy "answers eligibility only, never rank — it produces a boolean." **This spec preserves that ownership.** The engine keeps the rank formula; it gains a retaliation term that reads the engine attacker ledger and the authored tolerance/sentiment data (plus author-tuned scalar knobs). Authored candidacy stays boolean and narrowing-only; no authored ranking expression is introduced. Selection ranking, retention, and hysteresis remain engine-side.
- §7c: "Hostility is mutable per-entity state… the numeric faction leaf beneath it: that storage is interim and migrates under the fact as the relationship model grows." This spec is that growth — it keeps `@brain.targetHostile` as the durable fact and grows the storage from a bare `f32` into a faction identity + sentiment lookup.
- `scripting.md` §1: feel details live on a spectrum; the engine seeds a default and exposes the axis. Sentiment, tolerance, and the retaliation-tuning scalars are the axes; the seeded default reproduces current behavior; a different game picks a different point through content.
- `scripting.md` §11: "any enemy × candidate relation that reduces to a number or a boolean belongs [in the candidate scope] as a fact." Sentiment, per-candidate damage, and tolerance are exactly such relations — exposed as facts for authored guards.
- The think-stride prices off `offers.nearest`, a pure-distance min computed upstream of candidacy/LOS (`targeting.rs`, §7c). The retaliation term must not touch that min — preserved as an invariant.

**Alternatives rejected.**
- **Fully-baked engine reprioritization with no authored data.** A hardcoded infighting rule with fixed thresholds. Rejected: it bakes a single point on the tolerance/aggression spectrum, contradicting §1 — a modder could not make a twitchy faction or a stoic one.
- **Authored `candidatePriority` IR ranking expression** (a Number expression over the candidate scope, symmetric with `candidateFilter`). This was the prior draft. Rejected on reversibility and layer placement: it evolves §7c's engine-owned ranking, and — unlike the engine-internal pieces (ledger, facts, widened walk), which are cheap to reshape pre-release — a shipped ranking-expression vocabulary in the typedef plus dev-mod content is a sticky published contract whose removal is content-breaking with drift-test fallout. Shipping an irreversible-ish contract to win the weaker side of an unsettled ranking-ownership divergence is the wrong trade now. The **chosen** shape — an engine retaliation formula reading authored tolerance/sentiment data plus a few authored scalar knobs — reaches every named behavior (recency/damage tuning, tolerance-gated retaliation, uniform across attackers, per-chapter tuning as content) while keeping §7c intact and staying reversible (scalars, not an expression vocabulary). If free-form authored ranking is later shown necessary, it is added then as its own deliberate decision; the candidate facts already ship, so authored *guards* lose nothing.
- **Faction as a bare `f32` id (no names).** Sentiment could be keyed by the existing numeric faction values. Rejected: authoring `sentiment("cabal","resistance",…)` beats `sentiment(1.0, 2.0,…)`, named factions give validation an anchor (referenced names must resolve), and faction-as-content reads as content. The per-entity storage stays an `f32` index; only the *authoring surface* gains names.
- **Full N×N relationship matrix as engine state.** Considered and narrowed to a per-directed-pair scalar with a default — sparse authored overrides over a behavior-preserving default, not a dense engine matrix.

**Placement.** Faction/sentiment/tolerance definitions and the retaliation-tuning scalars are content (accepting layer, no rebuild). The engine owns the mechanism and vocabulary: the candidate scan, the append-only fact table, the attacker ledger, and the ranking formula — including its new retaliation term. §7c's ranking ownership is preserved. The candidate scan is a per-tick sim-layer cost; the widened walk is an accepted O(N²) interim at boomer-shooter enemy counts, not a spatial broad phase.

## Acceptance criteria

- [ ] **AC1 — pool widened, behavior preserved.** With no authored factions, the full existing AI test suite passes unchanged; enemies enter the candidate walk but no enemy selects a peer as a target (shared default faction → non-hostile).
- [ ] **AC2 — faction identity, behavior preserved.** An archetype with no authored faction resolves to the default faction; default-faction enemies remain hostile to the player and neutral to peers exactly as before; player pawns remain non-hostile targets.
- [ ] **AC3 — per-archetype faction override.** Two archetypes authored into two different factions with mutually hostile sentiment select and attack each other by default; two archetypes in the same faction do not. One class authored across two factions, and two classes sharing one faction, both behave per their sentiment, not per class.
- [ ] **AC4 — directional sentiment.** Sentiment `A→B` hostile while `B→A` neutral yields A attacking B while B ignores A until provoked. Same-faction and cross-faction defaults reproduce the pre-spec `!=` hostility with zero authoring.
- [ ] **AC5 — sentiment fact.** A behavior guard or candidacy predicate reading `@candidate.sentiment` observes the evaluating enemy's authored sentiment toward each offered candidate; the SDK typedef exposes it and the committed-typedef drift test passes.
- [ ] **AC6 — attacker ledger.** After a peer damages an enemy, the victim's ledger records the attacker with damage and time; the record ages and expires on the same schedule the damage-recency facts use; multiple distinct attackers within the window are all retained up to the ledger bound; the facts `@candidate.damageDealtToMe` and `@candidate.timeSinceDamageFromCandidate` read correctly for each attacker candidate and read zero/sentinel for a non-attacker.
- [ ] **AC7 — tolerance resolution.** `@candidate.tolerance` resolves to the per-archetype override when authored, else the faction-pair tolerance, else the default; verified for a case that takes the override and a case that falls through to the pair value.
- [ ] **AC8 — reprioritization.** A low-tolerance enemy engaged with the player, when damaged past tolerance by a peer that is farther than the player, switches its selected target to that peer; a max-tolerance enemy under identical damage keeps the player. Behavior holds uniformly whether the attacker is same-faction or cross-faction.
- [ ] **AC9 — stride pricing untouched.** `offers.nearest` (the think-stride price) remains the pure nearest-*hostile* distance, unaffected by the retaliation term; the existing stride/friendly-masking tests pass, and a test asserts a far over-tolerance attacker does not alter the stride price.
- [ ] **AC10 — no thrash; transient hold.** A reprioritization switch is stable across consecutive ticks under steady damage (hysteresis on the effective rank); when the attacker's ledger contribution decays, the retained attacker is not dropped mid-engagement by the engine, and an authored guard over `@brain.targetVisible`/`@brain.targetDied` is what stands the enemy down.
- [ ] **AC11 — retaliation inert without authored low tolerance.** With max/default tolerance (nothing authored to lower it), target selection ranks by pure distance identically to pre-spec, verified against a retained-target hysteresis case; the retaliation term prefers a candidate only when its accumulated recent damage exceeds an authored-lower tolerance.

## Tasks

### Task 1: Widen the candidate pool

Generalize target-candidate enumeration so brain-bearing enemies are offered as candidates, not only `PlayerMovement` holders. In `crates/postretro/src/scripting/systems/ai/targeting.rs`, `target_offers` walks `registry.iter_with_kind(ComponentKind::PlayerMovement)` and `target_candidate` hard-requires that component; change the walk to enumerate targetable entities — Health-bearing entities that are damage targets (`entity_model.md` §7: Health-bearing entities are damage targets), which includes both player pawns and brain-bearing enemies — while keeping every other gate (the existing `faction != enemy_faction` hostility filter at the `!=` comparison, LOS, candidacy, the retained-target exclude, and the pure-distance `offers.nearest` min) exactly as-is. The evaluating enemy must never offer itself; keep the existing self/exclude filtering. With every brain enemy still sharing `ENEMY_DEFAULT_FACTION`, the hostility filter evaluates `1.0 != 1.0 == false` for every enemy pair, so no enemy selects a peer and all current behavior is preserved (AC1). The widened walk is O(N²) in enemy count — accepted as an interim at boomer-shooter counts; do not add spatial acceleration here. Do not introduce factions, sentiment, or any registry dependency — this task is the thin enabling slice and must leave the tick's data dependencies unchanged. Update or add targeting tests to cover an enemy present in the candidate walk yet filtered as non-hostile.

### Task 2: Faction identity vocabulary

Introduce named factions as manifest content and per-archetype faction assignment, replacing the hardcoded faction seed — behavior-preserving. Today `crates/postretro/src/scripting/builtins/data_archetype.rs` unconditionally writes `set(FACTION_STATE_FIELD, ENEMY_DEFAULT_FACTION)` (bare `f32 1.0`) for every brain-bearing entity, and faction has no name anywhere. Add a `factions` authored collection to the manifest following the `stores` template: an SDK builder and `ModManifestInput` field (`sdk/lib/data_script.ts`), a parsed field on `ModManifestResult` (`crates/scripting-core/src/runtime/types.rs`), parse-and-validate in `manifest_from_js_value` (`crates/scripting-core/src/staged_manifest.rs`), and a drain into a new `DataRegistry` faction registry (`crates/entities/src/data_registry.rs`, held in `ScriptCtx`, surviving level unload like `entities`). Each faction declares a stable string name; the engine assigns each a stable numeric index at commit and the per-entity `FACTION_STATE_FIELD` stores that index (the `f32` slot is retained as interim index storage per §7c). Add an optional `faction` key to `EntityTypeComponents` (`sdk/types/postretro.d.ts` and the Rust `EntityTypeDescriptor`, `crates/entities/src/data_descriptors/types/entity.rs`); at the archetype seed site, resolve the authored faction name to its index and seed that, falling back to a built-in default enemy faction when unauthored so player-vs-enemy hostility is unchanged (AC2). Player pawns continue to leave the field absent (index resolving to the player faction). Validation rejects an archetype naming an undeclared faction. The offer filter and `entity_faction` continue to compare faction indices with the existing `!=` rule in this task — sentiment replaces the rule in Task 3 — so behavior is preserved. Cover: unauthored archetype resolves to default; two archetypes authored into two declared factions store distinct indices; an undeclared faction name is a commit error.

### Task 3: Directional sentiment table and sentiment-driven offer filter

Replace the symmetric `faction != faction` hostility rule with a directional sentiment lookup, and expose sentiment as a candidate fact. Add sentiment authoring to the `factions`/manifest surface: directional entries keyed `(from_faction, to_faction)` carrying a `sentiment` scalar and a `tolerance` scalar (both authored here as one coherent table; `tolerance` is consumed in Phase 4 — pre-emptive wiring for the planned Task 5/6 consumer, not dead data). Drain the resolved sentiment/tolerance matrix into the faction registry from Task 2. Thread a read handle for that registry into the per-tick targeting scan — the scan currently takes only `&EntityRegistry` and does not read `DataRegistry`, so this is new plumbing; mirror how `SlotTable` reaches `state_crossings::detect` (pass the session/`ScriptCtx`-owned registry alongside the entity registry into `target_offers`/`select_target`). Change the offer-filter hostility test from index-inequality to "sentiment(evaluating→candidate) is at or below the hostile threshold," seeding the default sentiment so an unauthored different-faction pair is hostile and an unauthored same-faction pair is neutral — reproducing the pre-spec `!=` exactly (AC4) with zero authoring. Add the `@candidate.sentiment` Number fact: append `CANDIDATE_SENTIMENT_INPUT` to `CANDIDATE_INPUTS` (`crates/foundation/src/candidate.rs`, append-only with a slot-pinning test), widen `CandidateScope::refresh` (`crates/postretro/src/scripting/systems/ai/candidate_scope.rs`) to receive the evaluating enemy's faction index and the registry handle so it can resolve sentiment per candidate, thread the widened arguments through the single `select_target` call site, add the SDK leaf on `candidate` (`sdk/lib/brain.ts` + `sdk/lib/brain.luau`), and the static typedef template block (`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` + `.luau`) then regenerate so the committed-typedef drift test passes (AC5). The `offers.nearest` pure-distance min stays computed before this filter and unchanged (AC9). Integration test: two authored factions with hostile sentiment fight; same faction does not; default content unchanged. This is the thin vertical slice through the authoring→drain→tick→behavior seam — land and exercise it end-to-end before Phase 4 fans out.

### Task 4: Attacker ledger and per-candidate damage facts

Retain recent attackers on the brain and expose them as candidate facts. Combat-perception seeds only a single damage bearing/position; add a bounded ledger of recent damagers `(attacker EntityId, accumulated damage, time)` to the brain component (`crates/entities/src/components/brain.rs`), written at the same Health chokepoint that seeds `time_since_damage_ms`/`damage_bearing` (`apply_damage_with_context`, `crates/entities/src/components/health.rs`) from `DamageContext.attacker`, and aged/expired each AI tick on the same recency schedule the damage facts use (`compute.rs` ages `time_since_damage_ms` toward its sentinel). The ledger is bounded (fixed small capacity of distinct recent attackers; on overflow evict the least-recent) — state the capacity as a constant, not a layout. Add two candidate facts, `@candidate.damageDealtToMe` (Number: accumulated recent damage this candidate dealt the evaluating enemy, 0 if absent from the ledger) and `@candidate.timeSinceDamageFromCandidate` (Number: ms since this candidate last damaged the evaluating enemy, a large sentinel if never), appended to `CANDIDATE_INPUTS` (`crates/foundation/src/candidate.rs`, append-only, slot-pinning tests). Widen `CandidateScope::refresh` to receive the evaluating enemy's ledger (extending the Task 3 widening; the refresh already gains the enemy's context there) and resolve both facts per candidate by matching the candidate's `EntityId` against ledger entries. Add SDK leaves + typedef template entries + regenerate (drift test). Facts read zero/sentinel for a non-attacker candidate and correctly per attacker for multiple concurrent attackers (AC6). These per-candidate values are also what the Task 6 retaliation term reads — compute them once in `refresh`.

### Task 5: Tolerance resolution and fact

Resolve per-(enemy, candidate) tolerance from the two authored sources and expose it as a candidate fact. Tolerance is authored per faction-pair (the `tolerance` column on the sentiment entries from Task 3) and per archetype (a new optional `tolerance` key on `EntityTypeComponents`, seeded onto the evaluating entity's state like faction in Task 2). Resolution precedence, per owner decision: the per-archetype override wins when authored; otherwise the faction-pair tolerance; otherwise a default that makes crossfire-provoked reprioritization inert unless authored (a high/max default, so unauthored content never spontaneously infights). Add the `@candidate.tolerance` Number fact (the evaluating enemy's resolved tolerance toward this candidate), appended to `CANDIDATE_INPUTS` (append-only, slot-pinning test), resolved in the widened `CandidateScope::refresh` using the evaluating enemy's archetype/faction and the candidate's faction. Add SDK leaf + typedef template + regenerate (drift test). Verify the fact takes the archetype override in one case and falls through to the pair value in another (AC7). This task shares only the `CANDIDATE_INPUTS` table and `CandidateScope::refresh` with Task 4; append at distinct tail slots and populate distinct positional entries to stay non-conflicting. The resolved per-candidate tolerance is also the threshold the Task 6 retaliation term compares against.

### Task 6: Engine retaliation ranking term

Add an engine-owned retaliation term to target selection so a damaged enemy reprioritizes onto its attacker, keeping §7c's ranking ownership. In the `select_target` fold (`crates/postretro/src/scripting/systems/ai/targeting.rs`), after `CandidateScope::refresh` has computed each candidate's accumulated recent damage (Task 4) and resolved tolerance (Task 5), compute an engine retaliation preference: a candidate whose accumulated recent damage exceeds its resolved tolerance is preferred over the nearest-hostile default, with a deterministic tiebreak among several such attackers (higher accumulated damage, then nearer). Fold this into the selection rank key (currently pure `distance.total_cmp`) and the retention/hysteresis decision (`is_meaningfully_closer`, `engine_floor.rs`, currently distance-only) so a sufficiently provoking attacker can unseat a retained target. State as constraints, leaving the exact rank-key combination (subtractive offset vs. tiered key) to implementation: the retaliation term is inert when tolerance is never exceeded, so max-default-tolerance content ranks by pure distance identical to pre-spec (AC11); more provocation ⇒ more preferred; hysteresis still resists per-tick churn on the effective rank (AC10). The accumulation is shaped by authored scalar tuning — a recency window and a damage/recency weighting — authored on the behavior graph (or archetype) and seeded to defaults so unauthored content needs none; expose these as **scalars, not an IR expression**. The `offers.nearest` stride-price min must remain pure distance, computed upstream, untouched by the retaliation term (AC9). Do **not** add an authored `candidatePriority` IR expression or any authored ranking descriptor: ranking stays engine-owned. The candidate facts from Tasks 3–5 remain for authored guards; the retaliation term reads the same per-candidate ledger/tolerance values `refresh` already computes, not the IR facts.

### Task 7: Reference retaliation content and integration proof

Author faction/tolerance content in the dev mod that exercises the engine retaliation term, and lock the behavior with an integration test. Using the vocabulary from Tasks 2–6, add reference content (`content/dev/start-script.js` and the mirrored Luau if applicable): a low-tolerance archetype (a per-archetype `tolerance` override, or a low faction-pair tolerance) that reprioritizes onto an over-tolerance attacker, and a max-tolerance archetype that holds its target under identical damage; set any retaliation-tuning scalars where non-default behavior is wanted. Include the transient stand-down as an authored guard over `@brain.targetVisible`/`@brain.targetDied` (no new engine rule; guards may read the `@candidate.*`/`@brain.*` facts). Add an integration test asserting AC8 and AC10: a low-tolerance enemy engaged with the player and shot by a farther peer switches to the peer and holds the switch across ticks; a max-tolerance enemy under the same stimulus keeps the player; the switch fires uniformly for a same-faction and a cross-faction attacker. This is the consumer that proves the foundation — if authored faction/tolerance data plus the engine term cannot produce the behavior, the foundation is wrong and earlier tasks must change.

## Sequencing

**Phase 1 (sequential):** Task 1 — widen the pool; thin enabling slice, behavior-preserving, no new data dependencies.
**Phase 2 (sequential):** Task 2 — faction identity; behavior-preserving; establishes the faction registry and per-archetype assignment Task 3 consumes.
**Phase 3 (sequential):** Task 3 — sentiment table + offer filter + `@candidate.sentiment`; the thin vertical slice that falsifies the authoring→drain→tick boundary. Consumes Task 2's registry; widens `CandidateScope::refresh`, which Tasks 4–5 extend.
**Phase 4 (concurrent):** Task 4 (attacker ledger + damage facts), Task 5 (tolerance resolution + fact) — independent; both append distinct tail slots to `CANDIDATE_INPUTS` and populate distinct entries in the already-widened refresh.
**Phase 5 (sequential):** Task 6 — engine retaliation ranking term; consumes the ledger/tolerance values from Tasks 4–5 and folds a retaliation preference into the rank key/hysteresis.
**Phase 6 (sequential):** Task 7 — reference content + integration proof; consumes Task 6.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Factions collection | `ModManifestResult.factions` | `"factions"` | `factions` | `factions` | n/a |
| Faction declaration | `FactionDescriptor` (name → index at commit) | `"faction"` entry | `defineFaction`/inline | inline | n/a |
| Sentiment entry | faction-registry matrix | `"sentiment"` | `sentiment` | `sentiment` | n/a |
| Per-archetype faction | `EntityTypeComponents` faction key | `"faction"` | `faction` | `faction` | n/a |
| Per-archetype tolerance | `EntityTypeComponents` tolerance key | `"tolerance"` | `tolerance` | `tolerance` | n/a |
| Retaliation tuning | behavior-graph/archetype retaliation scalars | `"retaliation"` | `retaliation` | `retaliation` | n/a |
| Sentiment fact | `CANDIDATE_SENTIMENT_INPUT` | n/a (IR input name) | `candidate.sentiment` | `candidate.sentiment` | n/a |
| Damage-dealt fact | `CANDIDATE_DAMAGE_DEALT_TO_ME_INPUT` | n/a | `candidate.damageDealtToMe` | `candidate.damageDealtToMe` | n/a |
| Damage-recency fact | `CANDIDATE_TIME_SINCE_DAMAGE_FROM_CANDIDATE_INPUT` | n/a | `candidate.timeSinceDamageFromCandidate` | `candidate.timeSinceDamageFromCandidate` | n/a |
| Tolerance fact | `CANDIDATE_TOLERANCE_INPUT` | n/a | `candidate.tolerance` | `candidate.tolerance` | n/a |

Exact IR input string names (the `@candidate.*` spellings) follow the existing `CANDIDATE_INPUT_PREFIX` convention in `crates/foundation/src/candidate.rs`; casing for the camelCase leaves mirrors the existing `candidate.distance`/`candidate.died` leaves.

## Orderings

The attacker ledger is mutable state written by an event (damage) and aged by a timer; the retaliation term reads it. Scenarios the implementation must resolve, cited by the Task 7 integration test:

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Two peers damage one enemy on one tick | Both `apply_damage_with_context` calls land before the next AI tick reads the ledger | Both attackers recorded; the retaliation tiebreak (higher accumulated damage, then nearer) decides which is reprioritized onto — not last-writer-wins |
| Projectile impact vs. AI tick | Projectile damage resolves at the projectile stage, before the AI apply pass reads facts (per grounding; the AI apply pass re-reads and does not clobber the ledger) | Ledger and damage-recency facts seeded by the impact survive into the same/next tick's selection |
| Ledger entry decays below tolerance mid-engagement | Aging crosses the tolerance threshold while the attacker is the retained target | Engine does not drop the retained attacker on decay alone (retention holds); stand-down is the authored guard's job (AC10) |
| Attacker dies while retained | `@brain.targetDied` latches while the dead attacker is retained | Authored wildcard guard stands the enemy down; no engine special-case |
| Attacker despawns while in ledger | A ledger `EntityId` no longer resolves in the registry when `CandidateScope::refresh` runs | The candidate walk never offers a despawned entity, so a stale ledger id contributes to no candidate's facts; ledger entry expires by aging, not by liveness polling |
| Mutual crossfire | Enemy A and enemy B damage each other within the window | Each independently reprioritizes onto the other per its own tolerance; no shared/global state couples the two decisions |
| Zero-damage / contextless hit | A hit with `DamageContext.attacker` absent | No ledger entry added (no attacker identity); damage-recency facts follow their existing contextless-hit rule |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Stride price stays pure nearest-hostile distance | Task 1 (keeps `offers.nearest`), Task 3 (filter change upstream of nearest) | Threatened by Task 6 if the retaliation term touches `offers.nearest` or derives the stride from the reprioritized target | AC9 |
| Unauthored content behaves exactly as pre-spec | Task 1 (shared default faction), Task 2 (default faction seed), Task 3 (default sentiment reproduces `!=`), Task 6 (retaliation inert at default tolerance) | Any default that shifts hostility or ranking without authoring | AC1, AC2, AC4, AC11 |
| Ranking ownership stays engine-side (§7c) | Task 6 (engine formula; no authored ranking descriptor) | An authored ranking expression re-introduced under any name | AC8, AC11 |
| `CANDIDATE_INPUTS` is append-only; index is the runtime read handle | Tasks 3, 4, 5 append at the tail | An insert/reorder silently re-points every bound program | AC5, AC6, AC7 (slot-pinning tests) |
| `@brain.targetHostile` remains the durable relationship fact; faction storage stays interim | Task 2 (index storage under the fact), Task 3 (sentiment computes hostility) | A guard binding directly to the numeric faction leaf instead of the fact | AC4, AC5 |
| Attacker ledger recency matches the damage-facts schedule | Task 4 (seed at chokepoint, age per tick) | A separate/again-clobbered aging path desynchronizing ledger vs `timeSinceDamageMs` | AC6 |
| Retaliation switch is hysteresis-stable | Task 6 (effective-rank hysteresis) | Retaliation-term flicker thrashing the target frame-to-frame | AC10 |

## Script syntax examples

```ts
// Proposed design — content-authored, no engine rebuild.
defineMod({
  factions: [
    defineFaction("cabal"),
    defineFaction("resistance"),
  ],
  sentiment: [
    // directional; unlisted pairs default (different → hostile, same → neutral).
    // sentiment drives default hostility; tolerance gates crossfire retaliation.
    sentiment("cabal", "resistance", { sentiment: -1, tolerance: 0.2 }),
    sentiment("resistance", "cabal", { sentiment: -1, tolerance: 0.2 }),
    // allies: positive sentiment suppresses default cross-faction hostility
    sentiment("cabal", "cabal", { sentiment: 0, tolerance: 5.0 }),
  ],
  entities: [
    defineEntity({
      canonicalName: "cabal_grunt",
      components: {
        faction: "cabal",
        tolerance: 0.1,           // twitchy: turns on whoever scratches it
        behavior: gruntGraph,
        health: { max: 50 },
      },
    }),
  ],
});

// Retaliation ranking is engine-owned, gated by the authored tolerance above.
// Authors tune it with scalars, and read the facts in guards — no ranking expression.
const gruntGraph = defineBehavior({
  candidateFilter: candidate.died.not(),   // eligibility only (narrowing)
  retaliation: { windowMs: 1500 },         // recency window for accrued damage (seeded default otherwise)
  // guards may read the facts, e.g. flee a candidate that has hit me hard:
  //   when: candidate.damageDealtToMe.gt(40)  ->  "flee"
  // ... activities / guards, incl. authored stand-down over @brain.targetVisible
});
```

## Open questions

- **Retaliation-tuning surface (Task 6).** The exact scalar knobs (recency window, damage/recency weighting) and their authoring home (behavior graph vs archetype) are a review/implementation detail; defaults must reproduce pre-spec behavior. Included because the owner explicitly wants recency/damage tuning; they are scalars, not an expression vocabulary. If even these should be deferred to a fixed seeded formula, that is a one-line narrowing.
- **Rank-key combination (Task 6).** Subtractive offset vs. a tiered `(retaliating, distance)` key is left to implementation under the stated constraints (retaliation preferred; inert at default tolerance; hysteresis-stable).
- **Sentiment hostile-threshold exposure.** The offer filter treats sentiment ≤ threshold as hostile; the threshold is seeded (default 0) and not authored in this spec. Whether to expose it as a tunable is deferred — `scripting.md` §1's "expose the axis" argues for it later, but no consumer needs it now.
- **Resolved:** the §7c ranking-ownership divergence. Per the `/validate-plan` reshape, ranking stays engine-owned (a retaliation term reading authored data + scalars); no authored `candidatePriority` expression ships. A free authored ranking vocabulary is a separate future decision, not this spec.
