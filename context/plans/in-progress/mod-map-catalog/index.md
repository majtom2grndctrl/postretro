# Mod Map Catalog & defineMod

> Prerequisite: `drafts/runtime-level-lifecycle` (the load/unload mechanism a catalog id resolves into). Prerequisite for `drafts/reaction-composition` and `drafts/mod-frontend-hub`.

## Goal

Introduce `defineMod()` and the mod's **map catalog** — the consolidated, pre-load-discoverable home for per-map metadata (id, path, display name, optional classification tags). The catalog is declared in `defineMod` and committed at mod-init, so it is available **before any level loads**. The frontend discovers maps from it; reaction composition reads a level's tags from it when present; level scripts keep only map-specific behavior (`{reactions, crossings}`). This is the manifest spine both the reaction and frontend plans extend.

## Scope

### In scope

- **`defineMod(config: ModManifest): ModManifest`** — a pure typed identity helper (pattern of `defineEntity`) returned from `setupMod()`. Its parameter *is* the generated `ModManifest` type, so it types every field the manifest accepts today (`name`, `entities`, `uiTrees`, `theme`, `fonts`) plus the new `maps` catalog, and widens automatically as later plans add fields (`reactions`, `frontend`) — no edit to the builder. TypeScript + Luau + generated typedef. `setupMod()` stays the engine-called entry point — `defineMod()` is what it returns, preserving the return-based FFI crossing (`scripting.md` §12).
- **`maps` catalog** on the manifest — a list of entries `{ id, path, name, tags? }`, authored via the `defineMapCatalog([...])` identity helper so the listing can live in its own file (`maps/catalog.ts`) with per-entry type hints, then imported into `defineMod({ maps })`. `id` is the stable logical handle every reference uses (`loadLevel`, `backgroundLevel`, future saves), decoupled from `path` (where the bytes live) so files can move without breaking references — `path`'s incidental filesystem uniqueness is not the identity. Lean and extensible. Drained at mod-init and held **engine-global**, surviving level unload, mirroring entity types. Missing tags normalize to `[]`.
- **Tags are the classification source when authored.** A catalog entry's optional `tags` are the authoritative classification of that map — the input reaction composition (`reaction-composition`) reads and the frontend filters on. No tags on the `setupLevel` return.
- **Id-centric load via the lifecycle seam.** This plan adds the `LevelSource::Catalog(MapId)` arm to the lifecycle's `LevelRequest` (the `Path` arm shipped in `runtime-level-lifecycle` is the dev bypass). The state-machine drain resolves a `Catalog(id)` against the engine-global catalog — exactly like the install-time entity-registry lookup — and the resolved entry becomes install state for that load, so its `tags` are available at install with no extra plumbing. An id absent from the catalog is rejected with a logged diagnostic — no load.
- **Dev raw-path bypass.** The CLI map-path flow (`runtime-level-lifecycle`'s `LevelSource::Path`) loads with no catalog entry, synthesizing a default entry — `tags = []`, `name = file stem`, no catalog id — so dev iteration does not require catalog registration (and such a load never appears in a catalog-driven level-select).
- **Staged hot reload.** The catalog replaces atomically at the staged-commit boundary, like entity types; failed/stale results preserve the prior catalog.

### Out of scope

- The frontend UI that **renders** the catalog (level-select list, filtering) — `mod-frontend-hub`.
- The load/unload **mechanism** — `runtime-level-lifecycle`.
- The **reaction tier** that consumes tags — `reaction-composition`.
- **Rich metadata** — thumbnails, descriptions, campaign-graph / `next`-map ordering. The schema is shaped to grow (additive fields); v1 ships `id`/`path`/`name`/`tags` only.
- Per-map entity-type registration (entities stay mod-global, not per-map).

## Acceptance criteria

- [ ] `defineMod({ name, entities, uiTrees, theme, fonts, maps })` returned from `setupMod()` is wire-identical to the hand-built manifest (`maps` is *added* to the existing fields, which include `fonts`); importing the module performs no FFI.
- [ ] `defineMod()` type-checks all current manifest fields plus `maps` in TypeScript; the Luau form mirrors it; `gen-script-types` emits the `defineMod`/`maps`/map-entry declarations, the regenerated `sdk/types/postretro.d.{ts,luau}` are committed, and the drift test (`committed_sdk_types_match_current_registry`) passes.
- [ ] `defineMod`'s parameter is typed as the generated `ModManifest` (not a sealed copy): a field later added to `ModManifestResult` widens `defineMod`'s accepted config via `gen-script-types`, with no edit to the helper.
- [ ] `defineMapCatalog([...])` returns the array wire-identical (pure identity, no FFI), and the typedef/drift test covers `defineMapCatalog` + `ModMapEntry` [automated]; that entries type-check at the call site is a TS/Luau compile-time property gated by a committed fixture that must `tsc`-clean locally (the no-tsc-in-CI contract), not a Rust unit test. The `maps` field equally accepts a plain `ModMapEntry[]`.
- [ ] The map catalog is committed at mod-init and is structurally equal (`assert_eq!` on the `maps` field — `ModMapEntry` derives `PartialEq`+`Clone`) before and after a level unload (engine-global survival) — CPU test mirroring entity types.
- [ ] A load request carrying a catalog `id` resolves to that entry's `path` and loads it via the lifecycle; an `id` absent from the catalog is rejected with a logged diagnostic and no load occurs.
- [ ] A raw-path dev load (CLI arg, `LevelSource::Path`) loads with no catalog entry and synthesizes default metadata: `tags = []`, `name = file stem`, no catalog id.
- [ ] On a catalog-id load, the resolved entry (with its `tags`) is stored on the in-flight load; a CPU test reads its `tags` back from that field, confirming they are available before the data script runs.
- [ ] Staged reload replaces the catalog atomically; failed and stale staged results preserve the prior catalog.
- [ ] A catalog with a duplicate `id` keeps the first entry, drops the duplicate with a logged warning, and commits the remaining valid entries; an entry with an empty `path` is skipped and logged — neither aborts mod-init.
- [ ] No new `unsafe`.

## Tasks

### Task 1: `defineMod()` / `defineMapCatalog()` helpers + `maps` type

Add `defineMod` to the SDK as an identity helper (pattern of `defineEntity`, sdk/lib/data_script.ts:173): TypeScript in `sdk/lib`, exported from `sdk/lib/index.ts`; Luau equivalent in `sdk/lib/data_script.luau`, added to `DATA_SCRIPT_FIELDS` (luau_prelude.rs:106) and `POSTRETRO_ROOT_MODULE_EXPORTS` (luau_prelude.rs:211). Add a second identity helper `defineMapCatalog(entries: ModMapEntry[]): ModMapEntry[]` the same way (TS + Luau + typedef + root-module export), so the catalog can be authored in its own file with per-entry hints and imported into `maps` — it is pure SDK sugar over the array, no wire/engine change (the `maps` field still accepts a plain `ModMapEntry[]`). Both helpers' function signatures are declared in the hand-written SDK lib blocks `TS_SDK_LIB_BLOCK` (typedef.rs:678) and `LUAU_SDK_LIB_BLOCK` (typedef.rs:1761) — adding a public root export to `sdk/lib/index.ts` requires this (per that file's header note), distinct from the generated `ModManifest`/`ModMapEntry` *types* below. Add the optional `maps` field (and a new `ModMapEntry` type) to the **generated** `ModManifest`: it is not hand-written — it is built by the `register_type("ModManifest")` builder (`scripting/primitives/mod.rs:438`), derived from the Rust struct `ModManifestResult` (`scripting/runtime.rs:45`), and locked by the parity test `mod_manifest_registered_type_matches_mod_manifest_result` (`scripting/primitives/mod.rs:527`, `expected_fields` at :551). So: add `maps?` to the `ModManifest` registration, register `ModMapEntry` the same way, add `maps` to `ModManifestResult` (and the parity test's `_shape_anchor` literal at `scripting/primitives/mod.rs:543`), extend `expected_fields` (:551), and regenerate `sdk/types/postretro.d.{ts,luau}` via `gen-script-types`. SDK-parity + typedef-drift coverage.

### Task 2: Catalog drain + engine-global structure

Add `maps` to `ModManifestResult` (runtime.rs:45). **Pin the engine-global home:** add `maps: Vec<ModMapEntry>` as a field on `DataRegistry` (scripting/data_registry.rs:18), excluded from `clear()` (:134) exactly as `entities` survives — committed at mod-init alongside `upsert_entity_type` (main.rs:2780). (`DataRegistry` is the engine-global catch-all; `reaction-composition` later adds `global_reactions` there too.) Drain `maps` from the manifest in `run_mod_init_quickjs`/`run_mod_init_luau` (runtime.rs:1445/1639) beside `drain_ui_trees_js` etc. (the `drain_*` helpers live in `scripting/data_descriptors.rs`).

**Staged reload (debug-only) is a full lane, not just a method.** `replace_entity_types` (data_registry.rs:126) is the method to mirror, but its *caller* is the real work: add `maps` to `StagedManifest` (staged_manifest.rs:55) and to `build_staged_manifest` (staged_manifest.rs:~364), then add a clone+replace branch in `commit_staged_manifest_result` (runtime.rs:619, `#[cfg(debug_assertions)]`) beside the `replace_entity_types` call — otherwise the replace method ships with nothing calling it. (Note: theme/fonts are NOT precedent — they are not re-committed on reload; only `entities`/`store_declarations` are.) `ModMapEntry` derives `Clone + Debug + PartialEq` (the survival/reload tests assert on the `maps` field).

Validate entries: non-empty `path`, and unique `id` — on a duplicate `id`, keep the first and warn (first-wins; the duplicate is dropped with a diagnostic so the author can resolve it, never silently missing content); entries with an empty `path` are logged and skipped. Valid entries always commit; degrade, never abort.

### Task 3: Id → path resolution and dev bypass

Add the `LevelSource::Catalog(MapId)` arm to the lifecycle's `LevelRequest` (`MapId` is `String` — the catalog entry's `id`, no newtype). The enum lives in `startup/lifecycle.rs`, created by the prerequisite — `grep "enum LevelSource"` to locate it. Add the catalog lookup that resolves a `MapId` to its `{ path, tags, name, id }` entry. The drain forms the absolute map path as `content_root.join(entry.path)` and hands `(map_path, content_root)` to the worker — the same pair shape the dev `Path` arm passes (whose CLI path already arrives resolved), and the worker uses `map_path` as the PRL location directly, so there is no second join. It stores the resolved entry on the in-flight load (alongside `map_path` in the `Loading` state) so the install path can read its `tags` before the data script runs — a CPU test reads them back from that field; `reaction-composition` is the eventual consumer (`loadLevel` / the frontend level-select read through this seam). The `Path` dev arm synthesizes a default entry (`tags = []`, `name = file stem`, no id). Missing-id load requests log a diagnostic and no-op.

### Task 4: Tests and docs

CPU coverage: catalog survival across unload, id→path resolution, missing-id rejection, dev raw-path bypass, staged replace/preserve. At promotion, document the catalog as the per-map metadata home and the level-script-as-behavior-only split in `scripting.md` (§2 manifest, the catalog) and `boot_sequence.md` (mod-init commit, engine-global lifetime).

## Sequencing

**Phase 1 (sequential):** Task 1 — builder + types pin the wire shape.
**Phase 2 (sequential):** Task 2 — drain and engine-global storage.
**Phase 3 (sequential):** Task 3 — resolution + dev bypass, consuming Task 2's structure.
**Phase 4 (sequential):** Task 4 — tests and docs.

## Boundary inventory

| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| `defineMod` | n/a (SDK only) | n/a | `defineMod()` | `defineMod()` |
| `defineMapCatalog` | n/a (SDK only) | n/a | `defineMapCatalog()` | `defineMapCatalog()` |
| map catalog | engine-global catalog struct | `"maps"` | `maps` | `maps` |
| map id | `id: String` | `"id"` | `id` | `id` |
| map path | `path: PathBuf` | `"path"` | `path` | `path` |
| display name | `name: String` | `"name"` | `name` | `name` |
| classification tags | `tags: Vec<String>` | `"tags"` optional, defaults to `[]` | `tags?` | `tags?` |

> `path` is authored relative to the mod/content root and resolved to an absolute `PathBuf` as `content_root.join(entry.path)` at load (Task 3) — one resolution, no double-join. The id→load crossing is the engine-internal `LevelSource::Catalog(MapId)` arm added to `runtime-level-lifecycle`'s `LevelRequest` (`MapId` = `String`) — not a script-facing wire field.

## Script syntax examples

```ts
// maps/catalog.ts — the catalog is its own concern, with per-entry type hints
import { defineMapCatalog } from "postretro";

export const mapCatalog = defineMapCatalog([
  { id: "e1m1", path: "maps/e1m1.prl", name: "Entryway",   tags: ["campaign"] },
  { id: "e1m2", path: "maps/e1m2.prl", name: "Underhalls", tags: ["campaign"] },
  { id: "dm1",  path: "maps/dm1.prl",  name: "Arena",      tags: ["deathmatch"] },
]);

// start-script.ts — assemble the manifest from imported concerns
import { defineMod } from "postretro";
import { mapCatalog } from "./maps/catalog";

export function setupMod() {
  return defineMod({
    name: "My Campaign",
    entities,
    theme,
    maps: mapCatalog,
  });
}

// levels/e1m1.ts — behavior only; classification lives in the catalog
export function setupLevel(ctx) {
  return { reactions: [thisMapsSetpiece], crossings: [] };
}
```

## Open questions

- Resolved: load is id-centric. This plan ships the `LevelSource::Catalog(MapId)` resolution seam; the dev raw path is the bypass. `loadLevel`'s *authoring signature* (how a mod names the id) is owned by `mod-frontend-hub`, but it resolves through this seam.
