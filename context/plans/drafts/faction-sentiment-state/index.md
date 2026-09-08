# Faction Sentiment as Game State

## Goal

Turn faction sentiment from immutable mod-init content into **mutable, decaying, persisted, replicated runtime game state**, and prove it with two consumers: an authored **reaction verb** (`setSentiment`/`adjustSentiment`) that story beats and player choices fire, and an **impact-policy effect** (`adjustSentimentToward`, chained on the damage-impact handles) that makes a backstab between sympathetic factions degrade their relationship — feeding the already-shipped sentiment-driven targeting so the degraded relationship erupts into a brawl. The engine owns the live sentiment table, its decay, its persistence, and its replication; mod authors own the events that move it and the rate it cools.

## Scope

### In scope

- A **live sentiment overlay** — an engine-owned mutable sparse table `(from_idx, to_idx) → current sentiment`, seeded empty (every pair at its authored baseline), holding only pairs currently diverged from baseline. Lives on `ScriptCtx` as its own `Rc<RefCell<_>>`.
- **Switch the AI read path** from the immutable baseline `FactionRegistry` to a live view that resolves overlay-over-baseline. Behavior-preserving while the overlay is empty.
- `setSentiment(from, to, value)` and `adjustSentiment(from, to, delta)` **consequential system-reaction verbs** (name-literal factions), fired from any reaction source — `levelLoad`, crossings, triggers, dialog-button `onPress` (player choice needs no new mechanism).
- An `adjustSentimentToward` **impact-effect method** on the damage-impact handles — `impact.target.adjustSentimentToward(impact.source, delta)` and its source-chained mirror — receiver = "from" faction, argument = "to" faction; the engine resolves each handle's faction index at apply time and moves that directional pair by an author-supplied signed delta.
- **Author-tunable decay** easing each diverged pair back toward its baseline: a global default rate plus an optional per-pair `decay` override, seeded to zero (hold) so unauthored content never decays. A per-tick engine pass; entries reaching baseline are dropped.
- **Campaign persistence** of the diverged overlay, keyed by faction name, surviving level unload and process exit and restored on load.
- **Co-op replication** of the overlay as host-authoritative shared state; clients converge on the host's live values; late joiners receive the current overlay.
- **Reference dev-mod content** exercising all of it: a story/trigger sentiment shift, an emergent backstab→brawl impact policy, a non-zero decay, and a persistence round-trip.

### Out of scope

- **Runtime faction *membership* change** (an entity switching factions mid-session). Unchanged from faction-crossfire's deferral; the per-entity `faction` field stays engine-mutable interim storage with no authored runtime faction-set verb.
- **Mutable tolerance.** Only *sentiment* is runtime-mutable here. The pair/archetype `tolerance` that gates retaliation (faction-crossfire) stays authored content; no `setTolerance` ships.
- **Faction durable identity.** Persistence keys sentiment by faction *name*. A faction renamed between sessions orphans its persisted sentiment, which is dropped with a warning (the ordinary malformed-value degrade path). Rename-stable faction keys are a later addition if a consumer needs them.
- **A new author-readable sentiment fact.** `@candidate.sentiment` already exists (faction-crossfire); once the read path switches it observes the live value with no new fact. No new IR leaf is added.
- **Per-attack aggregate.** The emergent effect fires per *impact*; the `onDamage` per-attack rollup (roadmap Combat Feedback & Economy follow-on) is not required and not built here.
- **A general keyed-state primitive.** The overlay is a bespoke engine table, not a new author-facing keyed-map store type.

## Direction

**Problem.** Faction sentiment is authored once and frozen — `FactionRegistry` is built at mod-init and has no runtime mutator, and the AI reads it from an immutable `Ref<DataRegistry>`. So the relationship layer the game's fiction turns on (betrayals, alliances, grudges) cannot move: a story beat, a player choice, or an in-world betrayal can damage entities but can never change how factions *feel* about each other. The observation that produced this: faction-crossfire made sentiment *drive* targeting, which makes a mutable sentiment immediately legible as emergent behavior — but crossfire deliberately froze sentiment as content, leaving the game-state dimension unbuilt.

**Prior commitments.**
- Faction-crossfire (`context/plans/in-progress/faction-crossfire/`) scoped out "Runtime re-authoring of factions/sentiment" and kept sentiment riding the content-parity gate. **This spec is that deferred growth**, and it diverges on parity: once sentiment is mutable it is *state*, not content, so it replicates as host-authoritative state and leaves the content-parity hash (`mod_digest::hash_faction_registry`, over the baseline) untouched — the same split replicated mover phase already makes against static map content.
- `scripting.md` §11 impact-policy substrate: the effect set is "closed but extensible — a later spec that needs a new effect *adds* an arm." The `adjustSentimentToward` effect arm is exactly that addition; source-addressing is already exercised by `grantHealth`/`grantAmmo`.
- `scripting.md` §1: feel lives on a spectrum, engine seeds a default and exposes the axis. Decay rate and the per-pair sentiment deltas are the axes; the seeded defaults (zero decay, unchanged baseline) reproduce today's frozen behavior; a game that wants grudges to cool picks a rate through content.
- `scripting.md` §5/§12: mutable, persisted, replicated, host-authoritative state that authored reactions write. Sentiment matches that contract, but its faction-pair key is content-defined and (for the emergent case) a runtime value, which the static per-slot store cannot address — so it is an engine-owned table rather than store slots (see Alternatives rejected).

**Alternatives rejected.**
- **Live sentiment as durable store slots.** Reuses persistence/replication wholesale. Rejected: the store is strictly per-static-slot, so a faction-pair table forces one dotted slot per ordered pair; the emergent backstab can touch any pair and mid-session slot creation breaks the replication fingerprint, forcing all-N² pairs declared up front; decay is a sparse shrinking set that fixed slots cannot represent; and persistence demands `ownership == Mod` + a mint-identity ledger key that engine-synthesized pair slots cannot obtain. Grounded in `research.md` §"(a) vs (b)".
- **Mutate `FactionRegistry.relationship_overrides` in place.** Simplest write path. Rejected: decay-toward-baseline needs the baseline preserved, and in-place mutation destroys it. The baseline must stay immutable and the live value must be a separate overlay.
- **A bespoke `onFactionHarm` dispatch source for the emergent case.** Rejected by the owner: too specific. The impact-policy substrate already fires at the damage chokepoint with `source`/`target` tokens, so the emergent path is an *effect callable inside an ordinary impact policy* (`defineImpactEvent`), not a new event type.
- **Author-baked decay with a fixed engine rate.** Rejected per §1: it bakes one point on the spectrum. Decay is an authored scalar, seeded to zero.
- **Build the general keyed/dynamic-state substrate now, with sentiment as its first consumer** — a reusable author-facing keyed-map store type that would absorb this table plus any future pair-/key-addressed state. Rejected on proportionality and placement: the overlay is engine-owned authoritative state (like replicated mover phase), not mod-owned store state, so a *store*-shaped general primitive is the wrong home for it; and a general dynamic-state primitive is a large, unshaped design that would balloon this spec's scope for zero additional shipped behavior. The overlay is bounded and sparse (diverged pairs only), so its bespoke persist section and sync record stay small. If several engine-owned keyed-state tables later accrue (store slots, mover phase, this overlay), reconciling them into one substrate is a deliberate future consolidation — not a reason to over-build the first bounded consumer now.

**Placement.** The live overlay, its decay pass, its persistence section, and its sync record are engine-owned authoritative state. The authoring surface — the reaction verbs, the impact-effect method, the decay-rate scalar — is content (no rebuild). The AI's ranking/targeting ownership (§7c) is untouched; this changes only *which* sentiment value the existing filter reads.

## Acceptance criteria

- [ ] **AC1 — behavior-preserving empty overlay.** With no runtime sentiment writes, the full faction-crossfire and AI test suites pass unchanged: the live view resolves every pair to its baseline value, and `is_hostile` / `@candidate.sentiment` / the offer filter behave exactly as against `FactionRegistry` today.
- [ ] **AC2 — reaction verb writes reach the AI.** After a `setSentiment("a","b", v)` reaction fires, the AI's next tick reads `v` for that directional pair (verified by a targeting outcome that flips when `v` crosses zero); `adjustSentiment("a","b", d)` moves the current live value by `d` (relative to the live value, not the baseline). An unlisted pair, written for the first time, reads its written value while its reverse pair still reads baseline (directional).
- [ ] **AC3 — verb is a consequential, host-authoritative reaction.** `setSentiment`/`adjustSentiment` classify as consequential (like `setState`), not presentation: fired from a trigger `on_fire` they take effect in the authoritative tick path, and the write is host-authoritative (a client firing has no local authority — it lands via replication, AC7).
- [ ] **AC4 — emergent impact effect.** An `impact.target.adjustSentimentToward(impact.source, delta)` effect inside a `defineImpactEvent` policy — the receiver handle is the "from" faction, the argument handle the "to" faction — when a damager of faction A hits a victim of faction B, moves the B→A live pair by `delta` (signed: negative degrades, positive bonds); the symmetric `impact.source.adjustSentimentToward(impact.target, delta)` moves A→B. Both faction indices are resolved from the two handle tokens at apply time; a hit whose `source` is absent (contextless damage) or whose source/target lacks a faction is a no-op. Verified for a same-faction hit and a cross-faction hit.
- [ ] **AC5 — backstab→brawl loop.** In the dev mod: two sympathetic-by-baseline factions, an impact policy that degrades A→B (and B→A) on harm, and a tolerance/sentiment configuration such that once the degraded pair crosses hostile, faction-crossfire's offer filter selects across the pair. A scripted A-hits-B backstab drives the pair hostile and A and B begin selecting each other as targets — the emergent brawl — with no engine stand-down rule.
- [ ] **AC6 — decay toward baseline.** With a non-zero decay rate, a diverged pair eases monotonically toward its baseline value over successive ticks and, on reaching baseline (within an epsilon), the overlay entry is removed (the table returns to sparse). Decay never overshoots baseline (a pair below baseline rises to it and stops; a pair above descends to it and stops). With the seeded zero rate a diverged pair holds indefinitely. A pair's decay rate resolves to its per-pair override when authored, else the global default.
- [ ] **AC7 — co-op replication.** A host-side sentiment change (reaction or impact effect) reaches every connected client so the client's live view matches the host's; a client that joins mid-session with a non-empty overlay receives the current diverged pairs. Clients hold no independent authority — the host's overlay is the source of truth, and the AI (host-authoritative) reads the host overlay.
- [ ] **AC8 — campaign persistence.** A diverged overlay saved on clean exit and reloaded restores the same directional pair values, across a level change within the playthrough. A pair that has decayed back to baseline is absent from the save (not persisted at baseline). A persisted pair whose faction name no longer resolves on load is dropped with a warning and does not abort restore.
- [ ] **AC9 — no double-application within a tick.** A pair written by both an in-tick impact effect and a frame-end reaction on the same frame reflects both writes deterministically (impact in-tick before the AI read, reaction at frame-end for the next tick), and two impact effects on the same pair in one tick apply in a defined order — neither is silently dropped.
- [ ] **AC10 — typedef surface.** The reaction verbs appear in the `postretro/ui` module surface and the impact `Effect` arm in the impact-policy surface; the committed typedef drift test and the impact-effect-arm-count / UI-function-inventory surface tests pass with the additions.

## Tasks

### Task 1: Live sentiment overlay + AI read-path switch

Introduce the engine-owned mutable overlay and route the AI read through it, behavior-preserving. Add a `FactionSentimentState` type (`crates/entities/src/data_registry.rs`, beside `FactionRegistry`) holding a sorted sparse `Vec` of `(from_idx, to_idx, current_value)` diverged pairs, mirroring `relationship_overrides`' representation and `binary_search_by_key` lookup. Give it: `get(from,to) -> Option<f32>`; a `set(from,to,value)` and `adjust(from,to,delta)` mutator (create/update/remove-if-at-baseline, where "baseline" is supplied by the caller so the overlay itself needs no baseline handle); a `decay_step(rate_for)` taking a per-pair rate resolver and a baseline resolver (implemented empty-of-policy here — Task 4 owns the per-tick invocation and rate authoring); and an iterator over diverged pairs for Tasks 5/6. Seed it empty. Hold it on `ScriptCtx` as `faction_sentiment: Rc<RefCell<FactionSentimentState>>`, initialized at the same commit site as `replace_factions` (`session/mod.rs`) and surviving level unload like `factions`. Add a read view `LiveFactionSentiment<'a> { baseline: &'a FactionRegistry, overlay: &'a FactionSentimentState }` exposing `sentiment(from,to)`, `tolerance(from,to)`, `relationship(from,to)` that resolve overlay-over-baseline (overlay only overrides `sentiment`; `tolerance` falls through to baseline, since tolerance is not mutable — Scope). Change `AiTickInputs.factions` from `&FactionRegistry` to this view, and update the read sites (`compute::evaluate`, `target_offers`, `select_target_with_attacker_ledger`, `is_hostile`, `candidate_scope`) to call the view — a mechanical rename since the view exposes the same method set. At the App tick call site (`main.rs` ~2961/3021 → `sim/mod.rs`), borrow `script_ctx.faction_sentiment` immutably alongside the existing `data_registry` borrow and build the view; the two borrows are read-only and non-overlapping with any write phase. Do not add any writer here beyond a test-only seam. With an empty overlay the view returns baseline for every pair, so all current behavior holds (AC1). Cover: view returns baseline for an unmodified pair; view returns the overlay value for a seeded pair while the reverse pair returns baseline.

### Task 2: `setSentiment` / `adjustSentiment` reaction verbs

Add the name-literal write verbs as consequential system reactions — the first real writer, completing the thin slice. Follow the `setState` dispatch class (consequential, host-authoritative, in-tick when trigger-fired), not the `flashScreen` presentation class: add `SystemReactionCommand::SetSentiment { from: String, to: String, value: f32 }` and `AdjustSentiment { from, to, delta: f32 }` (`crates/entities/src/reactions/system_commands.rs`); register both primitives in `register_system_reaction_primitives` with camelCase-deserialized args structs (`crates/postretro/src/scripting/systems/system_reactions.rs`); and add the apply arms in the system-reaction drain that resolve `from`/`to` to indices via `FactionRegistry::index_for_name` against the baseline and call the overlay `set`/`adjust` (passing the baseline value so `adjust` can drop an at-baseline entry) under a `RefMut` on `faction_sentiment` (the drain runs at frame-end / in-tick consequential dispatch, disjoint from the AI read borrow). An unresolved faction name warns and no-ops (the primitive's `ScriptError` path). Add SDK builders returning `PrimitiveReactionDescriptor`s in `sdk/lib/ui/reactions.ts`/`.luau`, export from `sdk/lib/prelude.ts`, declare their signatures in the typedef templates (`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` or the UI-module block + `.luau`), regenerate `sdk/types/postretro.{d.ts,d.luau}`, and add both names to the `UI_FUNCTIONS` list in `crates/postretro/src/scripting/typedef/tests/surface.rs` so `root_type_outputs_do_not_expose_ui_authoring_helpers` and the drift test pass (AC10). Integration test: a `levelLoad` `setSentiment` write flips a two-faction targeting outcome; `adjustSentiment` moves relative to the live value; directionality holds (AC2, AC3).

### Task 3: `adjustSentimentToward` impact-effect method

Add the impact effect for the emergent path, authored as a chained handle method. Add an `AdjustSentiment` arm to the closed effect union (`ImpactEffect` in `crates/postretro/src/impact_effects.rs`; `BoundEffect::AdjustSentiment { from_recipient, to_recipient, delta }` in `impact_policy.rs`). Unlike every shipped effect — each of which addresses exactly one recipient token — this arm carries *two* tokens (the receiver, its "from" faction, and the argument handle, its "to" faction), so it is a substrate-shape extension of `CommandRecipient`, not a copy of a single-recipient arm like `grantHealth`; `/review-implementability` should scrutinize the two-recipient widening specifically. Extend the single-recipient `CommandRecipient` handling so an effect can carry a second recipient token: `bind_effect` reads the receiver token (`@impact.target` or `@impact.source`, stamped by the handle as existing methods do) as `from_recipient` and lowers the argument handle to `to_recipient`, and `plan_effect`/`apply_planned` carry both. The `delta` is a bound `NumberRef` operand evaluated against the frozen scope, mirroring `grantHealth`'s `amount`. At apply time, resolve each recipient token → `EntityId` → faction index via `registry.get_component::<EntityStateComponent>(id)?.get(FACTION_STATE_FIELD)` (the AI's own accessor), then move the (from,to) overlay pair by `delta` under a `RefMut` on `faction_sentiment`, reached through the `ScriptCtx`/`DataRegistry` handle on `ImpactPolicyRuntime` (the applier needs the baseline value too, for the at-baseline drop). A missing `source` (`dispatch.source == None`), a despawned recipient, or a recipient with no faction field is a no-op (AC4). Author it as a method on **both** `TargetHandle` and `SourceHandle` (`sdk/lib/data_script.ts`/`.luau`) — `adjustSentimentToward(toward, delta): Effect`, receiver = "from" faction, `toward` = the other handle = "to" faction — matching the existing chained-effect idiom (`target.despawn()`, `source.grantHealth(…)`) rather than a top-level function; the receiver handle stamps its token as the other methods do and the `toward` handle lowers to the second token. Add the arm to the `ImpactEffectWire` union, declare the method on both handle interfaces in the impact-policy typedef template block, regenerate, and bump the per-handle method (`): Effect;`) counts and the effect-builder table in `impact_policy_surface_uses_author_ids_and_closed_effect_union` (`surface.rs`) (AC10). Test: a projectile from an A-faction shooter hitting a B-faction victim, with `impact.target.adjustSentimentToward(impact.source, delta)`, moves the B→A pair by `delta`, and the source-chained form moves A→B; a contextless hit no-ops; same-faction and cross-faction both resolve (AC4).

### Task 4: Author-tunable decay toward baseline

Add the per-tick decay pass and its authoring surface. Resolve a decay rate per pair: a global default authored on the manifest faction surface plus an optional per-pair `decay` on the authored sentiment entries (`FactionSentimentDescriptor` gains a `decay` field; `data_registry.rs` baseline resolves it like `tolerance`, and the manifest/SDK/typedef surface carries it — `sdk/lib/data_script.ts`/`.luau`, `staged_manifest.rs`, the typedef template + regenerate). Seed the global default and every unauthored pair to **zero** (hold), so unauthored content never decays (AC6, and the behavior-preserving invariant). Invoke `FactionSentimentState::decay_step` once per authoritative tick from the fixed-tick loop (`sim/mod.rs`), upstream of the AI read, under a `RefMut` that is released before the AI's immutable view borrow is taken — the decay write and the AI read are disjoint phases within the tick. Each diverged pair eases toward its baseline by its resolved rate × `dt`, clamped so it never crosses baseline, and is removed from the overlay when within an epsilon of baseline. A zero or sub-tick effective rate short-circuits (no accrual, entry unchanged) so the pass is provably inert for held content. Test: a seeded diverged pair with a non-zero rate converges monotonically and is dropped at baseline; a zero-rate pair holds; per-pair override beats the global default (AC6).

### Task 5: Campaign persistence of the overlay

Persist and restore the diverged overlay across levels and process exit. Add a `faction_sentiment` section to `PersistedState` (`crates/postretro/src/scripting/state_persistence.rs`) as a map keyed by the directional faction *name* pair (resolved from `FactionRegistry::descriptors` at save time), value the current sentiment. Add collect (on the same clean-exit/periodic save path the slot sweep uses, gated by the existing `StateStoreLifecycle`) and overlay-restore functions: collect iterates the overlay's diverged pairs, maps each `(from_idx,to_idx)` back to names, and writes the value; a pair at baseline is absent from the overlay so it is naturally absent from the save (AC8). Restore, run after mod-init commit (so the baseline registry exists to resolve names), re-resolves each saved name pair to current indices via `index_for_name` and `set`s the overlay; a name that no longer resolves is dropped with a warning and does not abort the restore, matching the substrate's malformed-value degrade rule. Bump `CURRENT_STATE_VERSION` and handle the older-version file (no faction section → empty overlay). Test: save with two diverged pairs, restore, assert both round-trip; a decayed-to-baseline pair is absent; an unresolvable saved name is dropped without aborting.

### Task 6: Co-op replication of the overlay

Replicate the overlay host→client as host-authoritative shared state. Add a dedicated sparse sync record carrying diverged pairs as `(from_idx: u16, to_idx: u16, value: f32)` (a new record type/section in the netcode state path — `crates/net/src/state_slots.rs` for the wire type and `crates/postretro/src/netcode/state_slots.rs` for host produce / client apply — kept separate from the per-slot `StateSlotDescriptor` schema, which does not fit a dynamic sparse set). Host: after the tick, emit the current diverged set (full-set baseline for a joining client; delta for steady state, reusing the existing baseline/delta/ack cadence). Client: apply the received set into its `faction_sentiment` overlay (all-or-nothing, like `apply_store_slot_batch`), so the client's `LiveFactionSentiment` view matches the host; a pair absent from a host delta because it decayed to baseline is removed on the client. Clients never write the overlay from local reactions/effects — those are host-authoritative (AC3, AC7); a client-fired verb reaches the host through the existing command path and lands via this replication. The overlay is state, not content, so it does not enter `mod_digest::hash_faction_registry` (parity is over the baseline only). Test (host + client harness): a host write appears on the client; a mid-session join receives the current diverged set; a decayed pair clears on the client (AC7).

### Task 7: Reference content and integration proof

Author dev-mod content exercising every surface and lock it with an integration test. In `content/dev/scripts/` (composed through `content/dev/start-script.ts`, mirrored in Luau where applicable): declare two factions that are sympathetic (neutral-or-allied) by baseline but whose tolerance/sentiment configuration means a degraded pair becomes a selectable crossfire target (building on faction-crossfire's offer filter); an impact policy (`defineImpactEvent`) whose harm effect chains `impact.target.adjustSentimentToward(impact.source, <negative>)` and the source-chained reverse to degrade the pair both ways; a non-zero `decay` so the grudge cools; and a story trigger firing `setSentiment`/`adjustSentiment` for the scripted-beat and player-choice cases. Integration test asserting AC5 and the round-trip: a scripted A-hits-B backstab through the full fixed-tick projectile path drives the A↔B pair hostile on the impact tick, A and B then select each other (the brawl), decay eases the pair back toward baseline over subsequent ticks with no further harm, a `setSentiment` beat overrides it, and a save/restore across a level change preserves a diverged pair. This is the consumer that proves the foundation: if the authored verbs plus the impact effect cannot produce the brawl and survive a round-trip, an earlier task is wrong.

## Sequencing

**Phase 1 (sequential):** Task 1 (overlay + read-path switch, behavior-preserving) → Task 2 (reaction verb, first writer) — the thin vertical slice through authoring → write → overlay → AI read; Task 2's integration test falsifies the boundary. Task 2 consumes Task 1's overlay and view.
**Phase 2 (concurrent):** Task 5 (persistence) and Task 6 (replication) — pure-Rust engine consumers of Task 1's overlay, file-disjoint from each other and from the SDK/typedef surface; both testable against Task 2's writer.
**Phase 3 (sequential):** Task 3 (impact-effect method) → Task 4 (decay + manifest field) — both edit the shared SDK/typedef surface (`sdk/lib/data_script.*`, the typedef templates, `surface.rs`), so they run in order rather than concurrently, as faction-crossfire's Phase 4 did for `CANDIDATE_INPUTS`.
**Phase 4 (sequential):** Task 7 (reference content + integration proof) — consumes Tasks 2–6.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Live overlay | `FactionSentimentState` (on `ScriptCtx`) | n/a (engine state) | n/a | n/a | n/a |
| Read view | `LiveFactionSentiment<'a>` (in `AiTickInputs`) | n/a | n/a | n/a | n/a |
| Set verb | `SystemReactionCommand::SetSentiment` | `"setSentiment"` | `setSentiment` | `setSentiment` | via `on_fire` reaction address |
| Adjust verb | `SystemReactionCommand::AdjustSentiment` | `"adjustSentiment"` | `adjustSentiment` | `adjustSentiment` | via `on_fire` reaction address |
| Impact effect method | `ImpactEffect::AdjustSentiment` / `BoundEffect::AdjustSentiment` | `"adjustSentiment"` (impact effect wire) | `adjustSentimentToward` (`TargetHandle`/`SourceHandle` method) | `adjustSentimentToward` | n/a |
| Per-pair decay | `FactionSentimentDescriptor.decay` | `"decay"` | `decay` | `decay` | n/a |
| Global decay default | manifest faction-surface field | `"factionSentimentDecay"` | `factionSentimentDecay` | `factionSentimentDecay` | n/a |
| Persist section | `PersistedState.faction_sentiment` | name-pair keyed map | n/a | n/a | n/a |
| Sync record | faction-sentiment sync record | sparse `(u16,u16,f32)` list, SharedGlobal | n/a | n/a | n/a |

The reaction-verb wire spellings mirror the existing camelCase system reactions (`flashScreen`, `setState`); the impact-effect wire primitive string mirrors the existing effect arms (`grantHealth`, `slot.set`). The two authoring surfaces are deliberately distinct in name and shape: the reaction is a top-level, name-literal verb (`adjustSentiment("cabal","resistance", d)`, `postretro/ui`), while the impact effect is a chained handle method (`impact.target.adjustSentimentToward(impact.source, d)`, `postretro` impact effects) whose receiver is the "from" faction and argument the "to" faction — so the direction reads from the call site rather than from token argument order.

## Orderings

The overlay is mutable state written by two event kinds (reaction, impact) and a timer (decay), and read by the AI. Scenarios the implementation must resolve, cited by the Task 4/7 tests:

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Impact write vs AI read, same tick | Impact effect writes in the projectile stage, before the same tick's AI read borrow | The AI reads the post-write value on that tick — a backstab can flip hostility same-tick (AC4, AC5) |
| Reaction write vs AI read | `setSentiment` drains at frame-end (consequential), after the tick | Takes effect on the next tick, not the current one (AC2, AC3) |
| Decay vs write, same tick | Decay runs once per tick upstream of the AI read; a write may also land that frame | Decay eases the pre-write value; a same-frame impact write then moves from the decayed value (deterministic: decay first, in-tick impact next, frame-end reaction last) (AC6, AC9) |
| Two impact writes to one pair, one tick | Two policies (or two hits) both `adjustSentimentToward` the same pair | Both deltas apply, in impact-dispatch order (per-fire, like the substrate's per-fire effect application); neither dropped (AC9) |
| Decay reaches baseline | A diverged pair eases to within epsilon of baseline | The overlay entry is removed; the pair reverts to baseline resolution and to absence from persistence/replication (AC6, AC8) |
| Adjust overshoots baseline via decay | Decay would step past baseline in one tick | Clamp to baseline exactly, then remove — never overshoot to the far side (AC6) |
| Write involving the player faction | `source` or `target` resolves to `PLAYER_FACTION_INDEX` (0), or a verb names no faction that maps to the player | The pair is written like any other index pair; the AI already treats player-vs-enemy via sentiment, so no special case — but the player pawn carries no authored faction name, so a name-literal verb cannot target it (only the impact effect, via the token, can involve index 0) |
| Contextless / faction-less impact | `dispatch.source == None`, or a recipient lacks the `faction` field | The `adjustSentimentToward` effect no-ops (no pair to move), matching the ledger's contextless-hit rule (AC4) |
| Save while a pair is decaying | Clean exit mid-decay | The current (mid-decay) value persists; restore resumes decay from it. A pair already at baseline is absent from the save (AC8) |
| Restore with a renamed faction | A persisted name pair no longer resolves against the loaded baseline | Dropped with a warning; restore continues for resolvable pairs (AC8) |
| Co-op join with non-empty overlay | A client joins after host-side divergence | The client receives the current diverged set as a baseline sync and its view matches the host (AC7) |
| Client-fired verb | A connected client fires `setSentiment` | No local authority; the command reaches the host, the host writes its overlay, replication returns the value to the client (AC3, AC7) |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Baseline stays immutable; live = overlay-over-baseline | Task 1 (separate overlay type; view resolves overlay-then-baseline) | Any write path mutating `FactionRegistry.relationship_overrides` instead of the overlay would destroy the decay target | AC1, AC6 |
| Empty overlay is behavior-preserving | Task 1 (empty seed), Task 4 (zero default rate) | A non-zero default rate or a seed that diverges a pair without authoring | AC1, AC6 |
| The AI is the only in-tick reader; writes never overlap its read borrow | Task 1 (own `RefCell`), Task 4 (decay released before the view borrow) | A write taken while the AI's immutable view borrow is held (RefCell panic) | AC1, AC9 |
| Sentiment is state, not content; replicates, does not gate parity | Task 6 (host-authoritative sync; overlay excluded from the compat hash) | Adding the overlay to `hash_faction_registry`, or a client writing its own overlay | AC7 |
| Diverged set is sparse; at-baseline pairs are absent everywhere | Task 1 (remove-at-baseline mutator), Task 4 (decay removal) | A pair left in the overlay at its baseline value would persist and replicate needlessly and never decay-remove | AC6, AC8 |
| Decay eases toward baseline and never overshoots | Task 4 (clamp-to-baseline step + epsilon removal) | A step that crosses baseline, or a rate applied without `dt` scaling | AC6 |

## Script syntax examples

```ts
defineMod({
  factions: [defineFaction("cabal"), defineFaction("resistance")],
  sentiment: [
    // baseline: sympathetic (neutral), a slow grudge-cooling decay.
    sentiment("cabal", "resistance", { sentiment: 0, tolerance: 5.0, decay: 0.1 }),
    sentiment("resistance", "cabal", { sentiment: 0, tolerance: 5.0, decay: 0.1 }),
  ],
  factionSentimentDecay: 0,          // global default: hold (unauthored pairs never decay)

  // Emergent: a backstab sours the victim's faction on the attacker's; crossfire targeting does the rest.
  // The builder param IS the reusable handle — destructure it and chain effects off each party.
  events: [
    defineImpactEvent("factions:crossfire", { /* filter */ }, ({ target, source, amount }) => [
      { when: amount.gt(0), do: [
          target.adjustSentimentToward(source, read(amount).times(-0.01)),   // victim's faction → attacker's
          source.adjustSentimentToward(target, read(amount).times(-0.01)),   // and reciprocate, for a two-sided brawl
      ]},
    ]),
  ],
});

// Story beat / player choice — factions are author-time names.
onStateCrossing(getGameState().quest.betrayedResistance, { above: 0.5 }, [
  setSentiment("cabal", "resistance", -1),   // absolute flip to hostile
]);
// a dialog button firing adjustSentiment("resistance", "cabal", -0.3) is the same path.
```

## Open questions

- **Sentiment value range.** Baseline sentiment is arbitrary finite `f32` today (only `<0`/`0`/`>0` are semantically load-bearing). This spec adds no clamp — `adjustSentiment` overshoot just yields a more-negative/more-positive value, still correctly hostile/allied. Whether to expose an authored `[min,max]` range (bounding decay and writes) is deferred; no consumer needs it now (`scripting.md` §1's "expose the axis" argues for it later).
- **Decay curve.** Task 4 uses linear ease toward baseline (rate × dt). Whether a modder wants non-linear cool-off (exponential) is deferred to an authored curve if a case demands it.
- **Faction durable identity.** Persistence keys by name, so a faction rename orphans persisted sentiment (dropped, not migrated). If a shipping campaign needs rename-stable faction keys, that is a follow-on mirroring the store's `identity.json` mechanism — out of scope here per the owner's v1 framing.
- **Reverse-pair convenience.** Story writes are directional; a `setSentimentSymmetric(a, b, v)` sugar that writes both directions is a possible SDK convenience, left out of v1 (authors write both entries, as the example shows).
