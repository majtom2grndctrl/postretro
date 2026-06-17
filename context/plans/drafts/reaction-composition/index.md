# Reaction Composition

> Prerequisites: `drafts/mod-map-catalog` (`defineMod` + the catalog whose `tags` classify levels — hard dependency) and `drafts/runtime-level-lifecycle` (its composition step edits the `install_level_payload` code that plan relocates, and recomposition-per-load is meaningful only in the multi-level world — file/sequencing dependency). Prerequisite for `drafts/mod-frontend-hub`'s death / level-flow handling.

## Goal

Let reactions be reused across levels without copy-paste. Add an engine-global **mod-global reaction tier** (declared in `defineMod`, surviving unload like entity types) plus an optional per-reaction **level-tag scope**. The per-level active reaction set is composed at load from `(mod-global matching the level's tags)` ∪ `(level-local)`. A level's classification tags come from its map-catalog entry (`mod-map-catalog`). This completes the tiering that entity types and UI trees already have — reactions are the odd one out today (per-level only, `scripting.md` §2/§10.4).

## Scope

### In scope

- `defineMod` gains `reactions` (and `crossings`) — engine-global *definitions*, committed at mod-init and surviving level unload, mirroring entity types.
- Optional per-reaction `levels` scope (string tags). **Absent = applies to every level** (global is the degenerate case of the selector — one mechanism, not two tiers).
- A level's tags come from its **map-catalog entry** (`mod-map-catalog`), read at install by the loading map's id/path — available before the data script runs. The engine retains the active level's tags for recomposition. The `setupLevel` return stays behavior-only (`{reactions, crossings}`).
- Composition at level install: active reaction set = `(mod-global filtered by level tags)` ∪ `(level-local)`. **Additive union** — matches the existing fire-all-matching dispatch (`fire_named_event`, reaction_dispatch.rs:116). Same treatment for crossings.
- Durable definitions vs per-level activation: mod-global definitions live engine-global (survive unload); the active set clears on unload and recomposes on load — the same split entity *types* (global) and *instances* (per-level) already follow.
- Staged hot reload: mod-global reactions replace atomically (like `replace_entity_types`); the active set recomposes against the current level's retained tags. Failed/stale staged results preserve the prior set.
- SDK sugar: a group helper that stamps `levels` on many reactions at once (the "bundle"), plus generated typedefs for the new `defineMod`/`setupLevel` fields.

### Out of scope

- **Explicit override/suppression** of a lower tier's reaction. Composition is additive; disambiguate via disjoint scopes. The global-plus-scoped-collide case is documented guidance ("global = universal-only"), not an engine feature. Deferred until a concrete need.
- Per-level entity-type registration (entities stay engine-global via `defineMod`, unchanged).
- New reaction primitives or dispatch changes (`loadLevel`/`restartLevel`/`returnToFrontend` belong to `mod-frontend-hub`).
- Tag-match logic beyond set intersection (any-match). No AND/NOT predicates in v1.

## Acceptance criteria

- [ ] A reaction declared in `defineMod` with no `levels` scope fires on every level, with no re-declaration in any level script.
- [ ] A reaction scoped `levels: ["campaign"]` fires on a level whose catalog entry carries the `"campaign"` tag and does not fire on a level lacking that tag.
- [ ] A level composes an active set that is the union of mod-global reactions matching its catalog tags and its own `setupLevel` reactions; all fire on their events.
- [ ] Two levels with disjoint tags each receive only their scoped reactions — a campaign-scoped reaction never fires on a deathmatch-tagged level (no override needed).
- [ ] Mod-global reaction definitions are byte-identical before and after a level unload, while the per-level active set clears and recomposes — CPU test, mirroring entity-type survival.
- [ ] Staged hot reload replaces the mod-global reaction set atomically and recomposes the active set against the current level's tags; failed and stale staged results preserve the prior definitions.
- [ ] Crossings receive the same tiering and scoping as reactions.
- [ ] No new `unsafe`.

## Tasks

### Task 1: Durable mod-global reaction tier

Add engine-global definition storage to `DataRegistry` (data_registry.rs:18) — `global_reactions` (and `global_crossings`) carrying a `NamedReaction`/`CrossingDescriptor` plus its `levels` scope. Survive `clear()` (data_registry.rs:134, which clears only the per-level active sets). Add `reactions`/`crossings` to `ModManifestResult` (runtime.rs:45) and `StagedManifest` (staged_manifest.rs:55); drain them in `run_mod_init_quickjs`/`run_mod_init_luau` (runtime.rs:1445/1639). Commit at mod-init mirroring `upsert_entity_type` (main.rs:2778) and provide atomic replace mirroring `replace_entity_types` (data_registry.rs:126) for staged reload.

### Task 2: Read level tags from the catalog

A loading map's tags come from its `mod-map-catalog` entry, resolved by id/path at install via the catalog lookup that plan exposes — no `LevelManifest`/`setupLevel` change (the return stays `{reactions, crossings}`). Retain the active level's tags on the app (`active_level_tags`) so a staged reload can recompose. A map with no catalog entry (dev raw-path load) has empty tags and matches only unscoped global reactions.

### Task 3: Composition step

Turn `populate_from_manifest` (data_registry.rs:43) into the composition entry: active `reactions` = `(global_reactions whose levels intersect the level tags, or whose levels is empty)` ∪ `level-local reactions`; same for crossings. It runs at install between the data-script drain (main.rs:3661) and `crossing_detector.initialize` (main.rs:3674), so the detector sees composed crossings. Add a recompose path callable on staged reload using `active_level_tags`.

### Task 4: SDK surface

Add `reactions`/`crossings` to the `defineMod` config and the per-reaction `levels` selector; add a group helper that stamps `levels` on many reactions. Add `tags` to the `setupLevel` return type. Extend the hand-written `ModManifest` and level-manifest types in both typedef blocks (typedef.rs:678/1761) and wire Luau exports (luau_prelude.rs). Pin the wire casing per the boundary inventory.

### Task 5: Tests and docs

CPU coverage: global-applies-everywhere, scoped-matches-only, disjoint-modes isolation, definition survival across unload, staged replace + recompose. Update `scripting.md` §2/§10 (reaction tiering, scope selector, composition) and `boot_sequence.md` §3/§5 (composition stage, durable-vs-active lifetimes).

## Sequencing

**Phase 1 (sequential):** Task 1, then Task 2 — both edit `runtime.rs` drains; sequence to avoid contention. Task 1 establishes the durable tier; Task 2 the tag input.
**Phase 2 (sequential):** Task 3 — composition consumes both the durable tier and the level tags.
**Phase 3 (sequential):** Task 4 — SDK matches the settled Rust/wire shapes.
**Phase 4 (sequential):** Task 5 — tests and docs.

## Boundary inventory

| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| mod-global reactions | `DataRegistry.global_reactions` | `"reactions"` (on manifest) | `reactions` | `reactions` |
| reaction scope | `levels: Vec<String>` (empty = all) | `"levels"` | `levels` | `levels` |
| level tags (read) | catalog entry `tags` (from `mod-map-catalog`) | — | — | — |
| crossings tier | `DataRegistry.global_crossings` | `"crossings"` | `crossings` | `crossings` |

## Script syntax examples

```ts
// Proposed design
import { defineMod, defineReaction, scopeReactions, playSound } from "postretro";

export function setupMod() {
  return defineMod({
    name: "My Campaign",
    reactions: [
      defineReaction({ name: "levelLoad", steps: [playSound("ambient/hum")] }), // every level
      ...scopeReactions(["campaign"], [campaignDeath, objectiveTracking]),       // campaign levels only
      ...scopeReactions(["deathmatch"], [dmRespawn, scoreLimit]),                // deathmatch levels only
    ],
  });
}

// levels/e1m1.ts — behavior only; the map's tags live in the catalog
// (mod-map-catalog), so the campaign-scoped reactions compose in by tag.
export function setupLevel(ctx) {
  return { reactions: [thisMapsSetpiece], crossings: [] }; // map-unique behavior
}
```

## Open questions

- Whether the `levels` selector belongs on the reaction entry (per-reaction field) or only via the `scopeReactions` group helper. Leaning: per-reaction field is the engine model; the helper is sugar over it. Confirm at promotion.
