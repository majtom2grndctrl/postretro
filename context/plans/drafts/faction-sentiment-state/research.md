# Faction Sentiment as Game State — research notes

Grounding for `index.md`. Citations verified this session against current source. Cite by identifier; line numbers drift.

## What is already built (the foundation this stands on)

Faction-crossfire (#478, merged; plan in `context/plans/in-progress/faction-crossfire/`) shipped the read side:

- **Baseline sentiment matrix.** `FactionRegistry` (`crates/entities/src/data_registry.rs`): `descriptors: Vec<FactionDescriptor>` (name → stable `f32` index; player 0, default-enemy 1, authored from 2 via `index_for_name`) and a sorted sparse `relationship_overrides: Vec<FactionRelationshipOverride>` keyed `(from_idx, to_idx)`. Read methods `sentiment(from,to)`, `tolerance(from,to)`, `relationship(from,to)` with alloc-free `binary_search_by_key`; unlisted pairs fall back to `SAME_FACTION_DEFAULT_SENTIMENT` (0.0) / `CROSS_FACTION_DEFAULT_SENTIMENT` (-1.0). Built once (`from_descriptors(...).with_sentiments(...)`), committed by `DataRegistry::replace_factions` at `session/mod.rs`. **No `set`/`adjust` mutator exists — immutable at runtime.**
- **Per-entity faction.** `EntityStateComponent` field `FACTION_STATE_FIELD = "faction"` (`ai/mod.rs`), read by the AI as `state.get(FACTION_STATE_FIELD)`.
- **AI read path.** `AiTickInputs.factions: &'a FactionRegistry` (`ai/mod.rs`) → `compute::evaluate(..., factions)` → `target_offers` / `select_target_with_attacker_ledger` / `is_hostile(factions, from, to)` (`ai/targeting.rs`) and `@candidate.sentiment` in `ai/candidate_scope.rs`. Sourced from an **immutable `Ref<DataRegistry>`** at the App tick call site (`main.rs`, ~2961/3021) threaded through `sim/mod.rs`. Nothing in the tick mutates it.

The impact-policy substrate (`context/plans/done/E16--impact-policy-substrate/`, plus death + economy extensions) shipped the write mechanism this reuses:

- **Impact dispatch** at the damage chokepoint (`apply_damage_with_context`, `crates/entities/src/components/health.rs`): `ImpactDispatch { amount, health_before, health_after, max_health, target: EntityId, source: Option<EntityId>, producer }`. `IMPACT_TARGET_TOKEN = "@impact.target"`, `IMPACT_SOURCE_TOKEN = "@impact.source"` — command-target tokens, never IR leaves.
- **Closed, extensible effect union** — now 9 wire arms (`Despawn/SetHealth/GrantHealth/GrantAmmo/PlayAnimation/Present/SetOwnerSlot`, Rust `ImpactEffect` in `crates/postretro/src/impact_effects.rs`; `BoundEffect` in `impact_policy.rs`). The death and economy specs already added arms — the "add an arm" path is proven.
- **Source-addressing is live prior art.** `grantHealth`/`grantAmmo`/owner-`slot.set` target `@impact.source`; `CommandRecipient::{Target,Source}` selects the token at `apply_planned`. The E16 substrate's "no v1 effect targets source" is superseded.
- **Faction resolvable at apply time.** `apply_planned` holds `dispatch.source`/`dispatch.target` and `&mut EntityRegistry`; reading a token's faction is `registry.get_component::<EntityStateComponent>(id)?.get("faction")` — the exact call the AI makes. Precedent: `OverlayStateScope::seed`. The `FactionRegistry`/overlay is not on `EntityRegistry`, so the applier must reach it via the `ScriptCtx`/`DataRegistry` handle already on `ImpactPolicyRuntime`.
- **One asymmetry.** Only the *target* is seeded into the per-fire IR read scope; the `SourceHandle` exposes no fact/state accessors (only `grantHealth`/`grantAmmo` today). So `source.faction` is **not** an author IR operand — the effect derives both faction indices engine-side from the two handle tokens.
- **Chained-method authoring idiom (confirmed).** An impact policy is `(impact: Impact) => EffectOrGroup[]`; effects are chained off `impact.target` (`TargetHandle`: `despawn`/`playAnim`/`setHealth`/`state`/`setState`) and `impact.source` (`SourceHandle`: `grantHealth`/`grantAmmo`), each stamping its own recipient token. The `adjustSentimentToward(toward, delta)` method fits this exactly — receiver stamps the "from" token, the `toward` handle argument lowers to the "to" token — so it needs no top-level two-token builder; the builder param is the reusable handle (destructured `{ target, source }`) the author already binds.

## The (a) vs (b) storage decision

Live sentiment must be mutable, decaying, persisted (campaign), and replicated (co-op, host-authoritative). Two shapes were grounded:

- **(a) Durable store slots.** Persistence and replication are a `SlotTable` sweep (`state_persistence.rs`) and a per-declared-slot wire schema (`crates/net/src/state_slots.rs`, `netcode/state_slots.rs`) — both strictly per-static-slot. A faction-pair table maps only by materializing one dotted slot per ordered pair. **Rejected:** (1) the emergent backstab can touch any pair and mid-session slot creation breaks the replication fingerprint, forcing all-N² pairs declared up front; (2) decay is a sparse shrinking set of diverged pairs, which fixed slot declarations do not represent; (3) persistence requires `ownership == Mod` + a durable-identity ledger key, but these slots are engine-synthesized (conceptually engine state) and `mint-identity` cannot see them — a category mismatch.
- **(b) Engine-owned live overlay (chosen).** A sparse `(from,to) → current value` table shaped like `relationship_overrides`, seeded empty (everything at baseline), living on `ScriptCtx` as its own `Rc<RefCell<_>>` sibling to `DataRegistry`/`SlotTable`. AI reads `overlay.get(pair).unwrap_or(baseline.sentiment(pair))`, preserving the alloc-free binary search. Its own `RefCell` keeps writes (frame-end reaction drain, in-tick impact applier, per-tick decay) from overlapping the AI's immutable per-tick read borrow. Cost, taken deliberately: it inherits nothing from the store substrate, so persistence is a new `PersistedState` section and replication is a dedicated host-authoritative sparse sync record. That cost is the honest price of sentiment being engine-owned authoritative game state (like replicated mover phase), not mod-owned store state; the overlay is sparse and small, so both surfaces stay bounded.

## Lifecycle: write → overlay → AI read → persist/replicate

```mermaid
sequenceDiagram
    participant Src as Write source<br/>(story reaction · player choice · impact effect)
    participant OV as Live overlay<br/>(FactionSentimentState, RefCell)
    participant Base as Baseline<br/>(FactionRegistry, immutable)
    participant Tick as AI tick<br/>(compute → targeting)
    participant Net as Host→client sync
    participant Disk as Campaign save

    Note over Src: story/choice → setSentiment/adjustSentiment reaction (frame-end drain)
    Note over Src: backstab → adjustSentiment impact effect (in-tick, projectile stage)
    Src->>OV: set/adjust pair (create entry if absent)
    loop each AI tick
        Tick->>OV: live.sentiment(from,to)
        OV->>Base: fall through when pair not diverged
        Note over Tick: is_hostile / @candidate.sentiment read LIVE
    end
    Note over OV,Base: per-tick decay eases each entry toward baseline;<br/>entry removed at baseline (sparse shrinks)
    OV->>Net: host emits sparse diverged pairs (SharedGlobal); clients converge
    OV->>Disk: diverged pairs persisted by faction name; restore re-resolves names
```

The in-tick impact effect writes during the projectile stage, **before** the same tick's AI read borrow is taken — so a backstab can flip hostility on the same tick. Frame-end reaction writes land for the next tick. Both mutate one table; the RefCell borrows never overlap because the write phases and the AI read phase are disjoint in the frame.

## Peer-compat note

`FactionCompatibilitySnapshot` / `mod_digest::hash_faction_registry` hash the *baseline* registry for content parity. The live overlay is replicated *state*, not content, so it does not enter the content-parity hash — mirroring how replicated mover phase is state, not map content. The snapshot's exhaustive-bind contract is over baseline fields only; the overlay is a separate type and does not touch it.
