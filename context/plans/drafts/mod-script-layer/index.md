# Mod Script Layer

> **Status:** draft
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

- **Mod entry point** — `start-script.{ts,luau}` at the mod root. The mod root is the content root directory as resolved by `content_root_from_map` (e.g. `content/tests/` for a map at `content/tests/maps/test-3.prl`). Engine loads it at init, before any level. The script returns a mod metadata object (at minimum: `name`). Domain scripts (`actors/`, `weapons/`, etc.) are pulled in via `import`/`require` from start-script; no auto-scanning of domain folders. If both `start-script.ts` and `start-script.luau` exist in the same mod, the engine errors at init.
- **TypeScript compile path** — at engine startup, `start-script.ts` is compiled to a sibling `start-script.js` via `scripts-build` if the `.js` is missing or older than the `.ts` (same freshness check used by `prl-build`). This path runs in both debug and release. The debug hot-reload watcher gains a non-recursive watch on the content root so edits to `start-script.ts` trigger recompilation without a restart. `scripts-build` detection logic is promoted from its current two duplicated copies (`watcher.rs::TsCompilerPath::detect` and `level-compiler/src/main.rs::find_scripts_build`) to a shared helper.
- **Mod-init context lifecycle** — single short-lived mod-init VM context at engine init. Same `registerEntity` API surface as the existing level-load data context. `registerLevelManifest` is not invoked from this path. Registrations land in the engine-global type registry and persist for the engine's lifetime.
- **Absent start-script** — if a mod has no `start-script`, no mod-init context is created; the engine boots with the type registry empty of mod-declared types. No error.
- **SDK and scripting reference** — document the mod entry point lifecycle and the mod metadata object shape.

### Out of scope

- Campaign progression state
- Cross-map persistent entity state
- Networked session setup

---

## Acceptance criteria

- `start-script.{ts,luau}` runs at engine init before any level loads.
- Entity types registered in start-script (and domain scripts it imports) are available when the first level's `info_player_start` spawn logic fires.
- An entity type registered in a domain script imported by start-script (not directly in start-script itself) is available at first-level entity spawn.
- Mod entry point lifecycle documented in `docs/scripting-reference.md`.
- A mod with no `start-script.{ts,luau}` boots successfully with no error.
- If both `start-script.ts` and `start-script.luau` exist in the same mod, the engine errors at init.
- The mod-init context runs exactly once per engine init; it does not re-run on subsequent level loads.
