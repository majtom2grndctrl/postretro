# Mod Script Layer

> **Status:** ready
> **Prerequisite for:** Player Spawn (M7)
> **Related:** `context/lib/scripting.md`

---

## Goal

Establish a mod-level script execution layer that runs before any level loads. Cross-level concerns — player entity type registration, game-wide setup — live here rather than in per-map scripts.

---

## Motivation

The existing scripting lifecycle is entirely level-scoped: data scripts run at level load, behavior scripts run during play, everything clears on level unload. Entity types registered via `registerEntity` do survive level unload (they go into the engine-global type registry), but there is no authoring surface for a mod to declare things at engine init time, before any map is loaded.

M7 needs: player entity classname registered before any `info_player_start` spawn logic runs.

---

## Scope

### In scope

Throughout this spec, "engine errors at init" means: log a clear diagnostic at error level and exit with a non-zero status — no panics.

- **Mod entry point** — authored as `start-script.ts` (TypeScript) or `start-script.luau` (Luau); in release, the engine loads `start-script.js` (TypeScript) or `start-script.lua` (Luau) before any level; in debug, see the Debug compile path bullet. The mod root is the content root directory as resolved by `content_root_from_map` (already handles the no-map case via the `DEFAULT_MAP_PATH` fallback in `resolve_map_path`; e.g. `content/tests/` for a map at `content/tests/maps/test-3.prl`; with no map argument, the engine currently defaults the map to `content/tests/maps/test-3.prl` and so resolves the mod root to `content/tests/`). `start-script` must export a function `setupMod()` that the engine calls at init; `setupMod()` returns a mod manifest object (at minimum `{ name: string }` in TypeScript, equivalent table in Luau). The engine stores the manifest. If both `start-script.js` and `start-script.lua` exist in the same mod root, the engine errors at init. The manifest is the foundation for mod identity; future engine features (mod inheritance, selective base-game component registration) will read it. Domain scripts (`actors/`, `weapons/`, etc.) are pulled in via `import`/`require` from start-script; no auto-scanning of domain folders. The mod root is fixed at engine init; switching mods requires a full engine restart.
- **Debug compile path** — in debug builds, if `start-script.js` is missing or older than `start-script.ts`, the engine compiles `start-script.ts` → `start-script.js` via `scripts-build`. For Luau, debug loads `start-script.luau` directly (mlua handles both `.luau` and `.lua` natively; no separate compile step in debug). Release builds expect pre-compiled `start-script.{js,lua}` to already exist. The debug hot-reload watcher gains a recursive watch on the content root so edits to `start-script.ts` or any file it imports (e.g. `actors/player.ts`) trigger recompilation without a restart. `scripts-build` detection and the mtime freshness check are currently duplicated between the file watcher and the level-compiler startup path; both must share a single debug-only implementation.
- **Import resolution** — all relative imports in `start-script.{ts,luau}` resolve from the mod root. For TypeScript, `scripts-build` bundles starting from the mod-root entry file (e.g. `./actors/player.ts` → `<mod-root>/actors/player.ts`). For Luau, `require` is currently in the deny-list in `luau.rs` (nil'd out before sandbox freeze) — there is no custom resolver wired into any VM today. Mod-init must wire a `require` resolver in the Luau VM that roots relative paths at the mod root; the same resolver wiring must be added to the per-level data context VM — level scripts will increasingly depend on mod-provided modules as the engine matures. Implement and document in `luau.rs`.
- **Mod-init context lifecycle** — fresh short-lived data-context VM at engine init, identical in primitive surface to the per-level data context (the one `registerEntity` runs in). The VM is dropped after init; registrations persist in the engine-global type registry. The engine reads the start-script's exports (TypeScript: ESM named exports; Luau: globals assigned at the top level of the script — `function setupMod() ... end` or `setupMod = function() ... end`, not keys on a returned table). It calls `setupMod`. If `registerLevelManifest` is also exported, the engine logs a warning and ignores it. All other exports are silently ignored. `setupMod` takes no arguments. If `setupMod` is not exported from the start-script, the engine errors at init (this is distinct from an absent start-script, where neither source nor compiled file exists). If `setupMod` throws or returns a non-object, or if the returned object is missing a `name` field, the engine errors at init.
- **Absent start-script** — if a mod has no `start-script` in debug builds, no mod-init context is created and the engine boots with the type registry empty of mod-declared types. In release builds, a missing `start-script.{js,lua}` is an error at init — modders must ship a compiled start-script that includes at minimum a mod manifest with a `name` field.
- **Boot sequence** — mod-init runs after CLI parsing (which resolves the mod root from the map argument, falling back to the `DEFAULT_MAP_PATH` constant when no map is given) and after primitive registration (which installs Rust-side primitives before any VM context is constructed, so the mod-init VM sees the full primitive surface). Mod-init completes before any level loads.
- **SDK and scripting reference** — document the mod entry point lifecycle and the mod metadata object shape. Add the `ModManifest` type (the `setupMod()` return shape) to `gen-script-types` output so `sdk/types/postretro.d.{ts,luau}` carry it; the drift-detection test in `cargo test` should catch stale type definitions. Player classname is registered via `registerEntity` side effects, not via a manifest field; `ModManifest` carries metadata only.

### Out of scope

- Campaign progression state
- Cross-map persistent entity state
- Networked session setup

---

## Acceptance criteria

- When a `start-script` is present, the mod entry point runs at engine init before any level loads, regardless of runtime (TypeScript or Luau) or build mode (debug or release).
- Entity types registered in start-script are present in the engine-global type registry immediately after engine init, before any level loads.
- An entity type registered in a domain script imported by start-script (not directly in start-script itself) is also present in the engine-global type registry after engine init.
- A relative import in `start-script` (e.g. `./actors/player`) resolves against the mod root and loads the target file.
- A relative import inside a file imported by `start-script` (e.g. `actors/player.ts` importing `actors/util.ts`) also resolves against the mod root.
- Mod entry point lifecycle documented in `docs/scripting-reference.md`.
- `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` include a `ModManifest` type and the `cargo test` drift-detection test passes.
- In debug builds, a mod with no `start-script` boots successfully with an empty type registry. In release builds, a missing `start-script.{js,lua}` causes an error at init.
- If both `start-script.js` and `start-script.lua` exist in the same mod root, the engine errors at init.
- The mod-init context runs exactly once per engine init; it does not re-run on subsequent level loads.
- A `require('./actors/player')` in `start-script.luau` resolves against the mod root and loads the target file.
- If `setupMod` is not exported from the start-script, the engine exits with a non-zero status and a diagnostic error message.
- If `setupMod` throws or returns a non-object, or if the returned object is missing a `name` field, the engine exits with a non-zero status and a diagnostic error message.
- `context/lib/scripting.md` updated to describe the mod-init lifecycle stage and the Luau `require` resolver contract.
