# scripting-tools dedup

> **Status:** draft
> **Related:** `crates/postretro/src/scripting/watcher.rs` · `crates/level-compiler/src/main.rs`

---

## Goal

Consolidate duplicated `scripts-build` detection and `.ts`/`.js` mtime
freshness logic into a shared crate so the engine and the level-compiler
share one canonical implementation.

---

## Background

Two crates today carry near-identical helpers:

| Concern | Engine (canonical) | Level-compiler (duplicate) |
|---------|--------------------|----------------------------|
| `scripts-build` discovery cascade (next-to-exe → PATH) | `crates/postretro/src/scripting/watcher.rs` `TsCompilerPath::detect` / `detect_with` | `crates/level-compiler/src/main.rs` `find_scripts_build` |
| `.ts` → `.js` mtime freshness | `compile_start_script_if_stale` (uses watcher helpers) in `crates/postretro/src/scripting/runtime.rs` | `js_is_fresh` in `crates/level-compiler/src/main.rs` |
| `scripts-build` subprocess invoke | `run_ts_compiler` in `crates/postretro/src/scripting/watcher.rs` | inline `Command::new` block in `compile_worldspawn_data_script` |

The engine copy is `#[cfg(debug_assertions)]`-gated (release builds don't
hot-reload), so level-compiler cannot import it directly. The level-compiler
runs offline and must work in release builds, so it carries its own copy.

The Mod Script Layer plan explicitly called this out:

> `scripts-build` detection and the mtime freshness check are currently
> duplicated between the file watcher and the level-compiler startup path;
> both must share a single debug-only implementation.

That requirement was satisfied inside the engine (watcher and runtime now
share `TsCompilerPath::detect` / `run_ts_compiler`), but the cross-crate
duplication with level-compiler remains.

---

## Approach

Promote the shared helpers into a small library crate so both binaries
depend on it. Name candidate: `postretro-scripts-tools`.

**Surface (initial):**

- `TsCompilerPath` enum + `detect()` / `detect_with(...)`
- `js_is_fresh(ts_path, js_path) -> Option<bool>`
- `run_ts_compiler(compiler, input, output) -> Result<(), String>`

The engine's `cfg(debug_assertions)` gate moves off the helpers and onto
the watcher module that consumes them. The level-compiler drops its
private copies.

---

## Out of scope

- Caching/memoization of compiler discovery
- Adding a `POSTRETRO_SCRIPTS_BUILD` env var override
- Moving any unrelated scripting code into the new crate

---

## Acceptance criteria

- One implementation of the discovery cascade in the workspace.
- One implementation of the mtime freshness check in the workspace.
- One implementation of the `scripts-build` subprocess invocation.
- Engine and level-compiler both depend on the shared crate.
- All existing tests still pass; the cascade tests in `watcher.rs` move
  to the new crate.
