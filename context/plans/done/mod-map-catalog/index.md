# Mod Map Catalog & defineMod

> Prerequisite: `done/runtime-level-lifecycle` (the load/unload mechanism a catalog id resolves into). Prerequisite for `in-progress/reaction-composition` and `ready/mod-frontend-hub`.

## Goal

Introduce `defineMod()` and the mod's **map catalog** — the consolidated, pre-load-discoverable home for per-map metadata (id, path, display name, optional classification tags). The start script exports `defineMod({...})` as its mod manifest, and the engine commits that manifest at mod-init so the catalog is available **before any level loads**. The frontend discovers maps from it; reaction composition reads a level's tags from it when present; level scripts keep only map-specific behavior (`{reactions, crossings}`). This is the manifest spine both the reaction and frontend plans extend.

## Scope

### In scope

- **`defineMod(config: ModManifest): ModManifest`** — a pure typed identity helper (pattern of `defineEntity`) exported as the start script's manifest. Its parameter *is* the generated `ModManifest` type, so it types every field the manifest accepts today (`name`, `entities`, `stores`, `uiTrees`, `theme`, `fonts`, `reactions`, `crossings`) plus the new `maps` catalog, and widens automatically as later plans add fields — no edit to the builder. TypeScript + Luau + generated typedef.
- **Default mod manifest export.** `start-script.ts` uses `export default defineMod({...})`; there is no engine-called `setupMod()` function. `scripts-build` lowers only the entry module's TypeScript default export to `globalThis.__postretroModManifest`, an engine-reserved script-mode slot. Imported modules may have default exports, but they must not overwrite the slot. Mod-init clears that slot before evaluating `start-script.js`, reads it immediately after evaluation, and ignores default-export slots from non-mod-init bundles. Raw generated `start-script.js` is that lowered wire form, not an ES module. `start-script.luau` returns `defineMod({...})` from the chunk as the Luau equivalent.
- **`maps` catalog** on the manifest — a list of entries `{ id, path, name, tags? }`, authored via the `defineMapCatalog([...])` identity helper so the listing can live in its own file (`maps/catalog.ts`) with per-entry type hints, then imported into `defineMod({ maps })`. `id` is the stable logical handle every reference uses (`loadLevel`, `backgroundLevel`, future saves), decoupled from `path` (where the bytes live) so files can move without breaking references — `path`'s incidental filesystem uniqueness is not the identity. `id`, `path`, and `name` are required non-empty strings; missing, non-string, or empty values skip that entry with a warning. Duplicate ids are first-wins and warn. `path` is an authored string relative to the content root only: no absolute paths, Windows prefixes, parent traversal, or normalized path that escapes the content root. Invalid paths skip the entry with a diagnostic. Lean and extensible. Drained at mod-init and held **engine-global**, surviving level unload, mirroring entity types. Missing tags normalize to `[]`.
- **Tags are the classification source when authored.** A catalog entry's optional `tags` are the authoritative classification of that map — the input reaction composition (`reaction-composition`) reads and the frontend filters on. No tags on the `setupLevel` return.
- **Id-centric load via the lifecycle seam.** This plan adds the `LevelSource::Catalog(MapId)` arm to `startup`'s lifecycle request types (the `Path` arm shipped in `runtime-level-lifecycle` is the dev bypass). The redraw-boundary drain resolves a `Catalog(id)` against the engine-global catalog before unloading any current level. A missing id logs a diagnostic, no-ops, preserves Running state, and leaves the installed level untouched. A found entry is copied into in-flight load state, so its `tags` and display metadata are available before the data script runs.
- **Dev raw-path bypass.** The CLI map-path flow (`LevelSource::Path`) loads with no catalog entry, synthesizing default metadata — `tags = []`, `name = file stem` with raw-path fallback, stored raw path string, no catalog id — so dev iteration does not require catalog registration (and such a load never appears in a catalog-driven level-select).
- **Manifest commit atomicity.** Cold and staged mod-init drain, normalize, and validate all manifest lanes (`entities`, `stores`, `maps`, `reactions`, `crossings`, plus UI/theme/font lanes owned by the boot caller) before mutating `DataRegistry` or store schemas. Successful results mutate at one commit boundary. Failed cold or staged manifest validation preserves previously committed entities, stores, maps, global reactions, and global crossings.
- **Staged hot reload.** The catalog replaces atomically at the staged-commit boundary, like entity types and mod-global behavior; failed/stale results preserve the prior catalog. An active level keeps the metadata snapshot installed with it until the next load; staged reload does not re-resolve the active catalog id.

### Out of scope

- The frontend UI that **renders** the catalog (level-select list, filtering) — `mod-frontend-hub`.
- The load/unload **mechanism** — `runtime-level-lifecycle`.
- The **reaction tier** that consumes tags — `reaction-composition`.
- **Rich metadata** — thumbnails, descriptions, campaign-graph / `next`-map ordering. The schema is shaped to grow (additive fields); v1 ships required `id`/`path`/`name` plus optional `tags` only.
- Per-map entity-type registration (entities stay mod-global, not per-map).

## Acceptance criteria

- [ ] `start-script.ts` may use `export default defineMod({ name, entities, stores, uiTrees, theme, fonts, reactions, crossings, maps })`; the engine receives the same manifest wire data as the old hand-built `setupMod()` return plus the new `maps` field; importing the module performs no FFI.
- [ ] `scripts-build` lowers a TypeScript default export in the entry module to `globalThis.__postretroModManifest`. QuickJS mod-init clears that slot before evaluating `start-script.js`, consumes it after evaluation, and does not look up `setupMod`. The output remains script-mode JavaScript with no surviving `import`/`export` declarations. Missing default export, explicit `undefined`/`null`, non-object default export, and thrown export initialization fail mod-init with diagnostics that name the default mod manifest export. A fixture with an imported module default export proves imported defaults do not overwrite the manifest slot.
- [ ] `start-script.luau` returns `defineMod({...})` from the chunk; Luau mod-init captures and consumes that return value directly. Missing/nil return value, non-table return value, and thrown chunk errors fail mod-init with diagnostics that name the mod manifest return.
- [ ] `defineMod()` type-checks all current manifest fields plus `maps` in TypeScript; the Luau form mirrors it; `gen-script-types` emits the `defineMod`/`maps`/map-entry declarations, the regenerated `sdk/types/postretro.d.{ts,luau}` are committed, and the drift test (`committed_sdk_types_match_current_registry`) passes.
- [ ] `defineMod`'s parameter is typed as the generated `ModManifest` (not a sealed copy): a field later added to `ModManifestResult` widens `defineMod`'s accepted config via `gen-script-types`, with no edit to the helper.
- [ ] `defineMapCatalog([...])` returns the array wire-identical (pure identity, no FFI), and the typedef/drift test covers `defineMapCatalog` + `ModMapEntry` with optional `tags` [automated]; that entries type-check at the call site is a TS/Luau compile-time property gated by a committed fixture that must `tsc`-clean locally (the no-tsc-in-CI contract), not a Rust unit test. The `maps` field equally accepts a plain `ModMapEntry[]`.
- [ ] `ModMapEntry.tags` is optional in TypeScript and Luau. Missing, `null`, or `nil` tags normalize to `[]` in Rust before commit. Present tags must be dense arrays/tables of non-empty strings: no holes, nil/undefined elements, non-string elements, or mixed keys. Malformed present tags skip that entry with a warning in both QuickJS and Luau.
- [ ] The map catalog is committed at mod-init and is structurally equal (`assert_eq!` on the `maps` field — `ModMapEntry` derives `PartialEq`+`Clone`) before and after a level unload (engine-global survival) — CPU test mirroring entity types.
- [ ] A load request carrying a catalog `id` resolves to that entry's `path` and loads it via the lifecycle; an `id` absent from the catalog is rejected before unload with a logged diagnostic. If the app is Running, the active level and Running state are preserved.
- [ ] A raw-path dev load (CLI arg, `LevelSource::Path`) loads with no catalog entry and synthesizes default metadata: `tags = []`, stored raw path string, `name = file stem` with raw-string fallback, no catalog id.
- [ ] On a catalog-id load, the resolved entry (with normalized `tags`) is copied from the in-flight load into active level state before the data script runs and before reaction/crossing composition; a CPU test reads it back from that field.
- [ ] Staged reload replaces the catalog atomically; failed and stale staged results preserve the prior catalog. The currently active level keeps its install-time metadata snapshot until a new load.
- [ ] A catalog with a duplicate `id` keeps the first entry, drops the duplicate with a logged warning, and commits the remaining valid entries. Missing, non-string, or empty required `id`/`path`/`name` fields skip that entry with a warning. Invalid `path` strings (absolute, Windows prefix, parent traversal, or normalized escape from the content root) skip with a warning. These skips do not abort mod-init.
- [ ] TypeScript and Luau parity tests cover required field validation, path confinement, tag normalization, malformed present tags, duplicate first-wins behavior, and catalog commit survival.
- [ ] Cold and staged mod-init validate every manifest lane before mutation. Failed validation preserves previously committed entities, stores, maps, global reactions, and global crossings.
- [ ] No new `unsafe`.

## Tasks

### Task 1: Default mod manifest export + `defineMod()` / `defineMapCatalog()` helpers + `maps` type

Add `defineMod` to the SDK as an identity helper (pattern of `defineEntity`, sdk/lib/data_script.ts:173): TypeScript in `sdk/lib`, exported from `sdk/lib/index.ts`; Luau equivalent in `sdk/lib/data_script.luau`, added to `DATA_SCRIPT_FIELDS` (luau_prelude.rs:106) and `POSTRETRO_ROOT_MODULE_EXPORTS` (luau_prelude.rs:211). Add a second identity helper `defineMapCatalog(entries: ModMapEntry[]): ModMapEntry[]` the same way (TS + Luau + typedef + root-module export), so the catalog can be authored in its own file with per-entry hints and imported into `maps` — it is pure SDK sugar over the array, no wire/engine change (the `maps` field still accepts a plain `ModMapEntry[]`). Both helpers' function signatures are declared in the hand-written SDK lib blocks `TS_SDK_LIB_BLOCK` (typedef.rs:678) and `LUAU_SDK_LIB_BLOCK` (typedef.rs:1761) — adding a public root export to `sdk/lib/index.ts` requires this (per that file's header note), distinct from the generated `ModManifest`/`ModMapEntry` *types* below. Add the optional `maps` field to the **generated** `ModManifest`, and add a generated `ModMapEntry` type whose `tags` field is optional (`tags?: ReadonlyArray<string>` in TypeScript, `tags: {string}?` in Luau). `ModManifest` is built by the `register_type("ModManifest")` builder (`scripting/primitives/mod.rs:438`), derived from the Rust struct `ModManifestResult` (`scripting/runtime.rs:45`), and locked by the parity test `mod_manifest_registered_type_matches_mod_manifest_result` (`scripting/primitives/mod.rs:527`, `expected_fields` at :551). So: add `maps?` to the `ModManifest` registration, register `ModMapEntry` with `tags?`, add `maps` to `ModManifestResult` (and the parity test's `_shape_anchor` literal at `scripting/primitives/mod.rs:543`), extend `expected_fields` (:551), and regenerate `sdk/types/postretro.d.{ts,luau}` via `gen-script-types`. SDK-parity + typedef-drift coverage.

Change the mod-init entry contract in the same pass. For TypeScript, extend `scripts-build`'s user-entry bundling path (not the SDK prelude path) so the entry module's `export default <expr>` lowers to `globalThis.__postretroModManifest = <expr>` after TypeScript stripping and before module declarations are discarded. Imported modules' default exports must lower locally without assigning the engine-reserved slot. The runtime clears the slot before evaluating `start-script.js`, reads it directly after evaluation, and does not look up or call `setupMod`. Missing, `undefined`, `null`, or non-object exports fail with diagnostics that name the default mod manifest export. If top-level evaluation throws while initializing the default export, mod-init wraps or maps the error so the diagnostic names the default mod manifest export. For Luau, execute `start-script.luau` as a chunk whose return value is the manifest table; switch both cold and staged mod-init paths from fire-and-forget chunk execution to return-value capture, and wrap chunk errors so diagnostics name the returned mod manifest. Missing/nil or non-table returns fail. Update cold mod-init and staged mod-init together so hot reload and startup consume identical manifest shapes. Remove or migrate `setupMod` diagnostics/tests/content; new diagnostics should refer to the default mod manifest export for TypeScript and the returned mod manifest for Luau.

### Task 2: Catalog drain + engine-global structure

Use the `ModManifestResult.maps` field added in Task 1. **Pin the engine-global home:** add `maps: Vec<ModMapEntry>` as a field on `DataRegistry` (scripting/data_registry.rs:18), excluded from `clear()` (:134) exactly as `entities` survives — committed at mod-init alongside `upsert_entity_type` (main.rs:2780). (`DataRegistry` is the engine-global catch-all; it also owns mod-global reactions/crossings.) Drain `maps` from the default/returned manifest in `run_mod_init_quickjs`/`run_mod_init_luau` (runtime.rs:1445/1639) beside `drain_ui_trees_js` etc. (the `drain_*` helpers live in `scripting/data_descriptors.rs`).

**Staged reload (debug-only) is a full lane, not just a method.** `replace_entity_types` (data_registry.rs:126) is the method to mirror, but its *caller* is the real work: add `maps` to `StagedManifest` (staged_manifest.rs:55) and to `build_staged_manifest` (staged_manifest.rs:~364), then add a clone+replace branch in `commit_staged_manifest_result` (runtime.rs:619, `#[cfg(debug_assertions)]`) beside the `replace_entity_types` call — otherwise the replace method ships with nothing calling it. (Note: theme/fonts are NOT precedent — they are not re-committed on reload; only `entities`/`store_declarations` are.) `ModMapEntry` derives `Clone + Debug + PartialEq` (the survival/reload tests assert on the `maps` field).

Cold and staged mod-init must share one pipeline shape: drain all manifest fields, normalize defaults, validate stores/maps/global reactions/global crossings, then mutate `DataRegistry` and store schemas at one commit boundary. A failure before that boundary preserves the previously committed entities, store schemas/values, maps, global reactions, and global crossings.

Validate entries: required `id`, `path`, and `name` fields must be strings and non-empty. `path` must be a relative string with no root/prefix and no parent traversal; reject absolute paths, Windows prefixes, `..`, and any normalized path that escapes the content root. Do this before the single `content_root.join(entry.path)` resolution. On a duplicate `id`, keep the first and warn (first-wins; the duplicate is dropped with a diagnostic so the author can resolve it, never silently missing content). Missing, `null`, or `nil` tags become `[]`. Present `tags` must be dense arrays/tables of non-empty strings with no holes, nil/undefined elements, non-string elements, or mixed keys. Malformed entries and malformed present tags are logged and skipped. Valid entries always commit; degrade, never abort.

### Task 3: Id → path resolution and dev bypass

Add the `LevelSource::Catalog(MapId)` arm to the startup lifecycle's `LevelRequest` (`MapId` is `String` — the catalog entry's `id`, no newtype). The request types live under `crates/postretro/src/startup/`. Add the catalog lookup that resolves a `MapId` to its `{ path, tags, name, id }` entry before any Running unload. `tags` is normalized before it reaches this seam, so consumers always read a concrete `Vec<String>`. The drain forms the absolute map path as `content_root.join(entry.path)` and hands `(map_path, content_root)` to the worker — the same pair shape the dev `Path` arm passes (whose CLI path already arrives resolved), and the worker uses `map_path` as the PRL location directly, so there is no second join. Store the resolved entry on the in-flight load (alongside `map_path` in the `Loading` state), then copy it into active level state before the data script and reaction/crossing composition run. Staged reload keeps that install-time active metadata snapshot; it does not re-resolve the active catalog id. The `Path` dev arm synthesizes default metadata (`tags = []`, `name = file stem` with raw-path fallback, stored raw path string, no id). Missing-id load requests log a diagnostic and no-op without unloading the active level.

### Task 4: Tests and docs

CPU coverage: TypeScript entry-module default manifest export, imported-module default export isolation, Luau returned manifest, missing/undefined/null/non-object manifest diagnostics, nil/non-table Luau return diagnostics, cold/staged parity, manifest commit atomicity, catalog validation parity, catalog survival across unload, id→path resolution, missing-id no-op from Running, active metadata snapshot, dev raw-path bypass, staged replace/preserve. Update `content/dev/start-script.ts` to the default-export form. Verify and adjust durable docs in `scripting.md` (§2 manifest, the catalog) and `boot_sequence.md` (mod-init commit, engine-global lifetime, runtime load semantics); promotion should not leave future-tense docs work behind.

## Sequencing

**Phase 1 (sequential):** Task 1 — builder + types pin the wire shape.
**Phase 2 (sequential):** Task 2 — drain and engine-global storage.
**Phase 3 (sequential):** Task 3 — resolution + dev bypass, consuming Task 2's structure.
**Phase 4 (sequential):** Task 4 — tests and docs.

## Boundary inventory

| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| default mod manifest export | `ModManifestResult` | `globalThis.__postretroModManifest` / chunk return | `export default defineMod(...)` | `return defineMod(...)` |
| `defineMod` | n/a (SDK only) | n/a | `defineMod()` | `defineMod()` |
| `defineMapCatalog` | n/a (SDK only) | n/a | `defineMapCatalog()` | `defineMapCatalog()` |
| map catalog | engine-global catalog struct | `"maps"` | `maps` | `maps` |
| map id | `id: String` | `"id"` | `id` | `id` |
| authored map path | `path: String` | `"path"` | `path` | `path` |
| resolved map path | `PathBuf` on in-flight load | n/a | n/a | n/a |
| display name | `name: String` | `"name"` | `name` | `name` |
| classification tags | `tags: Vec<String>` | `"tags"` optional, defaults to `[]` | `tags?` | `tags?` |

> `path` is authored as a relative string under the mod/content root and resolved to an absolute `PathBuf` as `content_root.join(entry.path)` at load (Task 3) — one resolution, no double-join. The raw-path bypass stores the raw path string in synthesized metadata and uses the file stem as the display name, falling back to that raw string when the stem is empty. The id→load crossing is the engine-internal `LevelSource::Catalog(MapId)` arm in startup's `LevelRequest` (`MapId` = `String`) — not a script-facing wire field.

## Script syntax examples

```ts
// maps/catalog.ts — the catalog is its own concern, with per-entry type hints
import { defineMapCatalog } from "postretro";

export const mapCatalog = defineMapCatalog([
  { id: "e1m1", path: "maps/e1m1.prl", name: "Entryway",   tags: ["campaign"] },
  { id: "e1m2", path: "maps/e1m2.prl", name: "Underhalls" },
  { id: "dm1",  path: "maps/dm1.prl",  name: "Arena",      tags: ["deathmatch"] },
]);

// start-script.ts — assemble the manifest from imported concerns
import { defineMod } from "postretro";
import { mapCatalog } from "./maps/catalog";

export default defineMod({
  name: "My Campaign",
  entities,
  theme,
  maps: mapCatalog,
});

// levels/e1m1.ts — behavior only; classification lives in the catalog
export function setupLevel(ctx) {
  return { reactions: [thisMapsSetpiece], crossings: [] };
}
```

## Open questions

- Resolved: load is id-centric. This plan ships the `LevelSource::Catalog(MapId)` resolution seam; the dev raw path is the bypass. `loadLevel`'s *authoring signature* (how a mod names the id) is owned by `mod-frontend-hub`, but it resolves through this seam.
