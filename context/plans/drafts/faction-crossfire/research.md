# Faction Sentiment & Crossfire — research notes

Grounding for `index.md`. Citations verified this session against current source. Line numbers are ephemeral — cite by identifier when they drift.

## Lifecycle: crossfire → ledger → reprioritization

```mermaid
sequenceDiagram
    participant PS as Projectile stage
    participant HC as Health chokepoint<br/>(apply_damage_with_context)
    participant Brain as Victim brain state
    participant Tick as AI tick (compute → targeting)
    participant Sel as select_target fold

    Note over PS: enemy A's shot hits enemy B (bystander)
    PS->>HC: apply_authorized_weapon_impact_damage(attacker=A)
    HC->>Brain: seed time_since_damage_ms=0, damage_bearing, last_known_target_pos
    HC->>Brain: NEW — push ledger entry (A, damage, now)
    Note over Tick: next authoritative AI tick
    Tick->>Sel: target_offers → ranked set: sentiment-hostile candidates<br/>plus over-tolerance ledger attackers (incl. same-faction peers)
    Note over Brain,Sel: ledger + time_since_damage_ms aged in phase so the candidate<br/>recency fact agrees with @brain.timeSinceDamageMs (AC6)
    loop per offered candidate C
        Sel->>Brain: CandidateScope::refresh(C) reads faction, ledger, tolerance
        Note over Sel: facts (for guards): sentiment, damageDealtToMe,<br/>timeSinceDamageFromCandidate, tolerance
        Sel->>Sel: eval candidateFilter (Bool); engine retaliation term<br/>reads C's accrued damage vs tolerance
    end
    Sel->>Tick: rank key = f(distance, retaliation term); hysteresis on effective rank
    Note over Tick: A outranks nearer player when B's<br/>accrued damage from A > B's tolerance toward A
    Tick->>Brain: acquired_target = A (retained; bypasses hostility filter)
```

The `offers.nearest` min is the pure nearest-sentiment-hostile distance, computed inside `target_offers` **before** this fold; it excludes over-tolerance-admitted neutral attackers and is never touched by priority — it prices the think-stride (Invariant: stride price stays pure nearest-hostile distance).

## Retaliation state (per brain, informal)

```mermaid
stateDiagram-v2
    [*] --> Default: no over-tolerance attacker
    Default --> Retaliating: engine retaliation term prefers attacker\n(accrued damage > tolerance)
    Retaliating --> Retaliating: fresh damage keeps ledger hot
    Retaliating --> Default: authored guard stands down\n(@brain.targetDied / @brain.targetVisible)
    Retaliating --> Default: target lost or aggro de-engages\n(mark clears with acquired_target)
    Retaliating --> Retaliating: new over-tolerance challenger\nout-ranks held by > margin (anti-thrash)
    note right of Retaliating
      Ledger decay alone does NOT drop the retained
      attacker, even with a nearer hostile present
      (the retaliation-acquired mark holds it).
      A merely-nearer non-provoking candidate cannot
      unseat it; stand-down is authored, not engine.
    end note
```

## Verified grounding (by area)

### Candidate pool & hostility filter
- `target_offers` walks only `registry.iter_with_kind(ComponentKind::PlayerMovement)`; `target_candidate` hard-requires `PlayerMovementComponent` — `crates/postretro/src/scripting/systems/ai/targeting.rs`. Root cause enemies can't infight.
- Hostility filter: `state.get(FACTION_STATE_FIELD) != enemy_faction`, then `if !hostile { continue; }` — `targeting.rs` (the `!=` comparison). `enemy_faction` from `entity_faction(registry, snap.id)` in `compute.rs`; both `map_or(0.0, …)`.
- `FACTION_STATE_FIELD = "faction"`, `ENEMY_DEFAULT_FACTION = 1.0` (players absent → 0.0) — `crates/postretro/src/scripting/systems/ai/mod.rs`. Seeded unconditionally at `crates/postretro/src/scripting/builtins/data_archetype.rs` (`.set(FACTION_STATE_FIELD, ENEMY_DEFAULT_FACTION)`), gated only on `descriptor.behavior`.
- Durable fact: `BRAIN_TARGET_HOSTILE_INPUT = "@brain.targetHostile"` — `crates/foundation/src/brain.rs`; doc: "durable authored relationship surface. The numeric faction storage is intentionally an interim `@state` implementation detail."
- **Faction is a bare `f32` — no named-faction vocabulary anywhere.** No `defineFaction`, no id/name registration. This spec introduces faction identity.

### Ranking / retention / stride
- Ranking is pure nearest-XZ-distance, two independent min-reductions: `offers.nearest` in `target_offers` (stride price) and the `nearest_eligible` `fold` in `select_target` (`targeting.rs`), both keyed on `distance.total_cmp`. No scoring abstraction.
- Hysteresis: `TARGET_SWITCH_HYSTERESIS_DISTANCE = 1.0`, `is_meaningfully_closer(candidate, retained)` = `candidate + 1.0 < retained` — `crates/postretro/src/scripting/systems/ai/engine_floor.rs`; applied in `select_target`.
- Retained target bypasses the hostility filter deliberately (`targeting.rs` comment: "never re-gates its target on hostility"); test `retained_target_stays_selected_after_its_faction_turns_friendly`. Retention gated on engagement in `compute.rs` (`engages_active`).
- Priority hook: change the `select_target` fold rank key + `is_meaningfully_closer`; **do not** touch `offers.nearest`. Stride reads `offers.nearest` / retained distance upstream — test `fresh_acquisition_skips_friendlies_so_they_do_not_mask_hostiles` locks the pure-hostile stride.
- Selected target → brain facts hand-off: `BrainFacts { target: Option<(EntityId, f32, Vec3)>, … }` → `BrainScope::refresh` recomputes all target-side facts. Pick a different target and every fact recomputes automatically.
- Aggro gate: `BrainComponent.aggro_armed` (`crates/entities/src/components/brain.rs`) — the only thing suppressing guard evaluation; upstream of all target work.

### Candidate & brain scopes (IR fact tables)
- Candidate scope: `CANDIDATE_INPUTS: [(&str, IrType); 4]` (append-only, "Append, never reorder") — `crates/foundation/src/candidate.rs`: `distance`, `health`, `maxHealth`, `died`. `CandidateValidationScope` + `bind_candidate_filter` (parse-time). Runtime `CandidateScope` + `refresh(registry, candidate, distance)` — `crates/postretro/src/scripting/systems/ai/candidate_scope.rs`; index is the read handle.
- Refresh currently takes only `(registry, candidate, distance)`. Facts needing the evaluating enemy's context (sentiment, ledger, tolerance) require **widening refresh + the single `select_target` call site** — flagged.
- Brain scope: `BRAIN_INPUTS: [(&str, IrType); 20]` (append-only, per-slot pinning tests) — `crates/foundation/src/brain.rs`. `BrainScope::refresh` positional array must match order.
- Adding a fact: const + append to `*_INPUTS` (bump length) + slot-pinning test; append positional value in `refresh`; compute in `compute.rs` (brain) / `refresh` (candidate); SDK leaf (`sdk/lib/brain.ts` + `.luau`); static typedef template block (`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` + `.luau`) then regenerate; drift guard `committed_sdk_types_match_current_registry`.
- `candidateFilter` registered doc: "It can only narrow that offer set; it does not rank candidates or drop a retained target" — `crates/postretro/src/scripting/primitives/mod.rs`. The ranking-side change this spec makes is the engine-owned retaliation term (Task 6), not an authored ranking expression; an authored `candidatePriority` counterpart was considered and rejected (see `index.md` Alternatives rejected).
- Author example today: `candidateFilter: candidate.died.not().and(candidate.distance.le(50))` — `sdk/behaviors/reference/entities.ts` (the pose-fixture enemy). The dev mod composes reference entity modules through `content/dev/start-script.ts` (compiled to `start-script.js` at build).

### Combat-perception facts (already merged; ledger extends them)
- Brain fields: `time_since_damage_ms`, `damage_bearing`, `last_known_target_pos`, `damage_source_known` — `crates/entities/src/components/brain.rs`. Seeded in `apply_damage_with_context` (`crates/entities/src/components/health.rs`) from `DamageContext.attacker: Option<EntityId>`; aged toward sentinel each AI tick in `compute.rs`.
- `DamageContext.attacker` populated for contact attacks (`apply.rs`) and projectiles (`projectile_stage.rs` via `apply_authorized_weapon_impact_damage`). The id is reduced to bearing/position — **not retained**; the ledger adds that retention.
- Crossfire already lands physically: `projectile_collision_excludes` (`projectile_stage.rs`) excludes only shooter + projectiles; a bystanding enemy is a valid target.
- Clobber moot: projectile damage resolves at the projectile stage; the AI apply pass re-reads the brain and writes only the locomotion latch (`apply.rs`), so ledger/damage facts survive.

### Manifest authored-data pattern
- `ModManifestResult` (`crates/scripting-core/src/runtime/types.rs`) carries `entities`, `ui_trees`, `maps`, `store_declarations`, etc., each "Drained into `DataRegistry` by the boot caller after `run_mod_init`."
- Path for `entities`: `defineMod` (`sdk/lib/data_script.ts`) → `manifest_from_js_value` (`crates/scripting-core/src/staged_manifest.rs`) → `data_registry.replace_entity_types` (`crates/scripting-core/src/runtime/core.rs`) → `DataRegistry` (`crates/entities/src/data_registry.rs`).
- `DataRegistry` held in `ScriptCtx`, engine-global, survives level unload. Store `SlotTable` precedent for a keyed table threaded into a subsystem: `state_crossings::detect(&mut self, slot_table: &SlotTable)`.
- **The AI tick does not read `DataRegistry` today** — `target_offers` takes only `&EntityRegistry`. Threading the faction registry into the tick is new plumbing (Task 3).
- `EntityTypeDescriptor` (`crates/entities/src/data_descriptors/types/entity.rs`) / `EntityTypeComponents` (`sdk/types/postretro.d.ts`) presets: `light, emitter, movement, inventory, weapon, touchable, mesh, health, behavior`. No faction/tolerance/state-seed field — new optional keys attach here.
- Validation: SDK builders throw synchronously; engine-side `manifest_from_js_value` returns `ScriptError::InvalidArgument`; `store_identity::validate_attempt` is the precedent for cross-referential validation (referenced names resolve).

## Direction-question notes (reshaped after `/validate-plan`)

- **Q2 placement.** Definitions (factions, sentiment, tolerance) and the retaliation-tuning scalars are content (no rebuild). Engine owns: the candidate scan (a per-tick sim-layer cost; O(N²) widened-walk interim, not a spatial broad phase), the append-only fact vocabulary, the attacker ledger, and the ranking formula — including its retaliation term. §7c ranking ownership is preserved.
- **Q3/Q5 foreclosure / one-way door.** Baking faction == class would be the one-way door; avoided (faction is an authored, overridable dimension). Faction identity as named content is reversible (per-entity storage stays an `f32` index). The engine-internal pieces (ledger, facts, widened walk, sentiment table) are cheap to reshape pre-release. An authored `candidatePriority` IR ranking expression would have been the sticky part (shipped typedef + content, content-breaking to remove) — so it was rejected in favor of an engine retaliation term reading authored data + scalars, which keeps §7c intact and stays reversible.
- **Q6 alternative.** Two rivals recorded in `index.md` Direction: a fully-baked no-data engine rule (too rigid, violates §1) and a full authored `candidatePriority` IR expression (too sticky/irreversible for the weak side of a ranking-ownership divergence). The chosen middle — engine formula + authored data + scalar knobs — came from the `/validate-plan` reviewer and reaches every named behavior with §7c ownership intact.
- **Reshape outcome.** `/validate-plan` returned *Reshape* scoped to Task 6; owner accepted, conditioned on faction sentiment remaining central (it does — sentiment drives hostility, resolves tolerance by alignment, and is a readable guard fact). Task 6 rewritten from an authored ranking expression to an engine retaliation term; AC11 recast; Alternatives corrected to rebut the middle option; Task 1 framed as an accepted O(N²) interim.
