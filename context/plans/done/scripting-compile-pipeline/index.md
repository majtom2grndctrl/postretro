# Scripting Compile Pipeline

## Goal

Four related improvements: unify SDK library imports under the `"postretro"` bare specifier
for both TypeScript and Luau via engine-evaluated preludes; remove the tsc/npx compile
dependency so scripts-build (SWC) is the sole TS compiler; and move script compilation into
the map build step so `prl-build` compiles scripts when a worldspawn KVP lists them. Together
these eliminate the NPM/tsc runtime dependency, make content packaging coherent, and give Luau
and TypeScript authors identical import ergonomics.

## Scope

### In scope

- **SDK prelude (TypeScript):** bundle `sdk/lib/*.ts` into a committed `sdk/lib/prelude.js`;
  engine evaluates it before user scripts so all SDK lib exports are runtime globals.
  `import { world } from "postretro"` replaces `import { world } from "../../../sdk/lib/world"`.
- **SDK prelude (Luau):** embed `sdk/lib/world.luau` and `sdk/lib/light_animation.luau` via
  `include_str!`; engine evaluates each in every Luau context and sets their return values as
  globals. Luau authors use `world`, `flicker`, etc. directly without any require call.
- **scripts-build only:** remove `tsc`/`npx` fallback detection from the hot-reload watcher;
  scripts-build is the sole TS compiler path.
- **Level compiler script compilation:** worldspawn `script` KVP names a single `.ts` entry
  point; `prl-build` compiles it via scripts-build. Convention: one script per map, shared
  helpers imported from it rather than listed separately.
- **Hot reload wiring:** connect the watcher to the frame loop and fix the reload path so it
  actually re-evaluates changed scripts in the correct context without duplicating handlers.
- `const enum` usage across file boundaries documented as unsupported and flagged in
  `tsconfig.json` via `isolatedModules: true`.

### Out of scope

- Luau hot reload of sdk/lib changes. Prelude is embedded at compile time; engine restart
  required if `world.luau` or `light_animation.luau` change (same constraint as TS prelude).
- Embedding scripts-build into the engine binary. Sidecar model stays.
- QuickJS bytecode compilation (separate optimization concern).
- Type checking in scripts-build (`tsc --noEmit` remains the type-check path for authors).
- Hot reload of sdk/lib changes. Prelude is baked at compile time; engine restart required.
- Changing when `postretro.d.ts` / `postretro.d.luau` are generated (still emitted at debug
  startup; separate concern).

## Acceptance criteria

### SDK prelude

- [ ] `import { world } from "postretro"` resolves at runtime in QuickJS; `world.query(...)`
  returns correct results when called from a behavior script.
- [ ] `import { flicker, pulse, colorShift } from "postretro"` resolves at runtime.
- [ ] `arena-wave.ts` no longer uses a relative path to sdk/lib.
- [ ] `sdk/types/postretro.d.ts` exports `world`, `World`, `LightEntity`, `flicker`, `pulse`,
  `colorShift`, `sweep`, `EasingCurve`, `timeline`, `sequence` (all public sdk/lib symbols) so
  IDE completions work.
- [ ] `sdk/lib/prelude.js` is committed to the repo and included in the engine binary via
  `include_str!`. `cargo build -p postretro` fails with a clear error if the file is absent.
- [ ] `sdk/lib/index.ts` exists as the entry point for prelude generation; re-exports all public
  symbols from `world.ts` and `light_animation.ts`.

### Luau SDK prelude

- [ ] `world`, `flicker`, `pulse`, `colorShift`, `sweep`, `timeline`, `sequence` are available
  as globals in every Luau behavior context without any `require` call.
- [ ] `world:query({ component = "light", tag = "..." })` returns handle objects with
  `:setAnimation`, `:setIntensity`, `:setColor` methods.
- [ ] `sdk/types/postretro.d.luau` exports the same SDK lib symbols as the TS declarations so
  luau-lsp completions work.

### scripts-build only

- [ ] `TsCompilerPath::Tsc` and `TsCompilerPath::Npx` variants removed; detection cascade:
  scripts-build next to engine executable → scripts-build on PATH.
- [ ] Engine logs a clear actionable message when no TS compiler is found (install scripts-build
  or add it to PATH). `.luau` hot reload still works without scripts-build.
- [ ] `tsconfig.json` in `content/tests/scripts/` sets `"isolatedModules": true` (enforces
  the `const enum` ban at type-check time).

### Level compiler script compilation

- [ ] worldspawn `script` KVP accepted; value is a path to a single `.ts` file, relative to
  the `.map` file.
- [ ] `prl-build` compiles that file via scripts-build; compiled `.js` lands beside the `.ts`.
- [ ] `prl-build` exits non-zero with a clear error if `script` is set, scripts-build is not
  found, and no `.js` sibling exists.
- [ ] `prl-build` succeeds with a warning if scripts-build is not found but a `.js` sibling
  already exists.
- [ ] FGD `worldspawn` entry created from scratch (no entry exists today) with `script`,
  `ambient_color`, and `fog_pixel_scale` — all three documented in `build_pipeline.md` but
  missing from the FGD.

### Hot reload wiring

- [ ] `start_watcher` is called from `main.rs` after `ScriptRuntime` is constructed; failure is
  logged as a warning, not fatal.
- [ ] `drain_reload_requests` is called at the top of the frame loop (`RedrawRequested`) before
  game logic fires.
- [ ] On reload, `clear_level_handlers()` is called before re-running scripts; after 3 hot
  reloads, the number of registered handlers matches the count after a single cold load
  (verifiable via `HandlerTable::len()` in tests).
- [ ] Reload re-evaluates behavior scripts in the behavior context (not the definition context).
- [ ] `compiled_output_path` is removed from `ReloadRequest`; `drain_reload_requests` has a
  comment noting that every reload re-runs all behavior scripts.
- [ ] Editing `arena-wave.ts` (or any `.luau` script) while the engine runs causes the new
  handler logic to fire on the next `levelLoad` event without restarting the engine.

## Tasks

### Task 1: SDK prelude

Create `sdk/lib/index.ts` that re-exports all public symbols from `world.ts` and
`light_animation.ts`. Add a `--prelude` mode to scripts-build: `scripts-build --prelude
--sdk-root <dir> --out <file>` bundles the index entry, strips TS syntax, and writes a single
`.js` file with no surviving import/export declarations. Run this once to produce
`sdk/lib/prelude.js` and commit it.

In the `postretro` crate, include the prelude via `include_str!` and evaluate it in every new
QuickJS context before user scripts load (in `ScriptRuntime` init, or wherever behavior and
definition contexts are constructed). The prelude references `world_query`, `set_light_animation`,
etc. — these are already installed as globals when contexts are built, so the prelude's function
bodies resolve correctly.

Update `sdk/types/postretro.d.ts` to export all sdk/lib public symbols alongside the existing
primitive declarations. Update `arena-wave.ts` to `import { world } from "postretro"`.
`content/tests/scripts/arena-wave.js` is a committed compiled artifact; after changing the
import path in `arena-wave.ts`, the existing `.js` is stale and must be deleted and regenerated
by running scripts-build against the updated source as part of completing this task.

Document prelude regeneration: when `sdk/lib/*.ts` changes, run
`cargo run -p postretro-script-compiler -- --prelude --sdk-root sdk/lib --out sdk/lib/prelude.js`
and commit the result.

**Luau prelude:** no bundler step needed. The engine embeds `sdk/lib/world.luau` and
`sdk/lib/light_animation.luau` directly via `include_str!` (same pattern as the JS prelude).
At context construction, evaluate each file in the Luau state and promote its return value to
globals: `world.luau` returns the `world` table → set as global `world`;
`light_animation.luau` returns the `LightAnimationSdk` table → destructure into globals
`flicker`, `pulse`, `colorShift`, `sweep`, `timeline`, `sequence`. No separate
`sdk/lib/prelude.luau` file is needed — the Rust context-init code handles it. When
`sdk/lib/*.luau` changes, `cargo build` picks it up automatically via `include_str!`'s
implicit file dependency.

Update `sdk/types/postretro.d.luau` to export all SDK lib public symbols alongside the
existing primitive declarations, matching the additions made to `postretro.d.ts`.

### Task 2: Remove tsc/npx detection

Remove `TsCompilerPath::Tsc` and `TsCompilerPath::Npx` enum variants from `watcher.rs`.
Simplify `detect_with` to two steps: scripts-build next to `current_exe`, then scripts-build on
PATH. Update the missing-compiler log message to be actionable. Update or remove tests that cover
tsc/npx detection paths. Set `"isolatedModules": true` in `content/tests/scripts/tsconfig.json`.

### Task 3: Level compiler script compilation

Parse worldspawn `script` property in `parse.rs` using the existing `get_property` pattern.
In `main.rs`, add a script compilation step after map parsing: locate scripts-build using the
same two-step cascade as the engine (next to the compiler binary, then on PATH), resolve the
`script` path relative to the `.map` file, and invoke scripts-build on that single file. Apply
the missing-compiler / stale-js fallback logic from the acceptance criteria. Skip recompiling if
a `.js` sibling already exists with a newer mtime.

**Convention note for the FGD:** `script` names one entry-point file. Shared helpers are
imported from it. Maps do not list multiple scripts; if behavior is complex, it is organized into
modules that a single entry point imports. Document this convention in the FGD property
description and in `context/lib/scripting.md §9` (Compilation Tooling).

**Detection cascade — duplication, not sharing.** `watcher.rs` is gated on
`#[cfg(debug_assertions)]`, so the level-compiler crate cannot import `TsCompilerPath` from it.
Add a small private `fn find_scripts_build() -> Option<PathBuf>` in
`crates/level-compiler/src/main.rs` that re-implements the two-step cascade — the same ~20 lines
as `detect_with` in `watcher.rs`. Add a comment in both locations noting the duplication and
pointing to the other file.

Create the FGD `worldspawn` entry from scratch (no `@SolidClass worldspawn` exists in
`sdk/TrenchBroom/postretro.fgd` today). Include `script`, `ambient_color`, and
`fog_pixel_scale` — all three documented in `build_pipeline.md` but missing from the FGD.

### Task 4: Fix hot reload wiring

Four problems found by audit — all in the engine crate, no compiler changes needed:

1. **Not wired:** `start_watcher` and `drain_reload_requests` are never called from `main.rs`.
   Call `start_watcher` after `ScriptRuntime` construction; call `drain_reload_requests` at the
   top of the `RedrawRequested` handler.

2. **Wrong context:** `reload_definition_context` rebuilds the definition context only. Behavior
   scripts (`registerHandler`) live in the behavior context. The reload path must rebuild the
   behavior context and re-run all behavior script files.

3. **Handler duplication:** `HandlerTable` appends on every `registerHandler` call with no
   deduplication. Reload must call `clear_level_handlers()` before re-running scripts. Note:
   `clear_level_handlers()`'s existing doc comment says "called on level unload"; `scripting.md
   §8` will need updating in the same change to reflect that it is also called during hot reload.
   That doc update is in scope for this task.

4. **Dead field:** `compiled_output_path` on `ReloadRequest` is `#[allow(dead_code)]` and
   discarded in `drain_reload_requests`. Remove `compiled_output_path` from `ReloadRequest`
   entirely. Add a comment in `drain_reload_requests` noting that every reload re-runs all
   behavior scripts (full rebuild; targeted single-file reload is not implemented).

The correct reload sequence: receive `ReloadRequest` → clear level handlers → re-run all
behavior scripts from disk → if a level is currently loaded (i.e. `level_load_fired` is `true`),
immediately re-fire `levelLoad` so authors see results without restarting → log result.

`level_load_fired` is a one-shot flag set after the first fire in `main.rs`'s `RedrawRequested`
handler; it must **not** be reset by hot reload. The re-fire on reload is a dev-iteration
convenience (so newly registered handlers see a `levelLoad` event), not a level reload — leaving
the flag set preserves the "fire once per level lifetime" contract for the cold-load path.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the SDK import convention; `arena-wave.ts`
change depends on it.

**Phase 2 (concurrent):** Tasks 2, 3, and 4 — all independent of each other; all can start once
Task 1 ships.

## Rough sketch

### Prelude evaluation timing

The prelude evaluates once per context construction. Definition and behavior contexts are
long-lived (level lifetime) — the cost is negligible. Pooled contexts are pre-warmed; evaluate
the prelude during warm-up, not on each recycle. QuickJS allows freezing the global object after
pre-warm so pooled contexts don't accumulate mutations from script runs.

### scripts-build --prelude mode

The bundler already handles a single entry file with relative imports. A `--prelude` flag adds a
second AST visitor pass before the existing one.

**Two-visitor approach:**

1. **`ExportToGlobal` (new, runs first):** rewrites `export const x = expr` (and other named
   `ExportDecl` forms) into a bare `ExprStmt` containing `globalThis.x = expr`. For declarations
   that get inlined from relative imports and surface as `export` keywords on `var`/`let`/`const`
   /`function`/`class`, the visitor drops the `export` keyword and emits a trailing
   `globalThis.x = x` assignment so the binding lands as a global. Re-exports
   (`export { x } from "./y"`) and default exports (`export default ...`) need explicit handling
   too — the visitor enumerates the named symbols expected from `sdk/lib/index.ts` and asserts
   on unsupported forms rather than silently dropping them.

2. **`StripExternalImports` (existing, runs second):** continues to do its job —
   `items.retain(|item| !matches!(item, ModuleItem::ModuleDecl(_)))` cleans up any remaining
   `ModuleDecl` nodes (bare-specifier imports, leftover re-exports `ExportToGlobal` chose not to
   handle inline). The current sketch was wrong about this visitor preserving exports: it
   removes ALL `ModuleDecl` nodes, both imports and exports. By the time the prelude output is
   emitted, exports would already be gone — hence the need for `ExportToGlobal` to run first.

`globalThis` is available in QuickJS and is the right target for making symbols available
across script evaluations sharing the same context. The prelude is evaluated once per context
construction; user scripts loaded afterward see `world`, `flicker`, etc. on `globalThis`.

### Detection cascade — engine + level compiler

Both the engine watcher (`crates/postretro/src/scripting/watcher.rs`) and the level compiler
(`crates/level-compiler/src/main.rs`) need to locate `scripts-build` using the same two-step
cascade: next to the current binary, then on `PATH`. The watcher's `TsCompilerPath::detect_with`
can't be reused because `watcher.rs` is gated on `#[cfg(debug_assertions)]` and lives in a
different crate. Duplicate the ~20 lines as a private `find_scripts_build()` in level-compiler
and leave a comment in both files pointing at the other. If the cascade ever grows (e.g.
add a `POSTRETRO_SCRIPTS_BUILD` env var override), promote it to a shared crate then.

### Hot reload: levelLoad re-fire

After clearing handlers and re-running behavior scripts, check `level_load_fired` (the one-shot
flag in `main.rs`'s `RedrawRequested` handler). If it's `true`, immediately re-fire `levelLoad`
so newly registered handlers run without restarting the engine. Don't reset the flag — the
re-fire is a dev-iteration convenience, not a level reload, and resetting would break the
"fire once per level lifetime" contract for the cold-load path.

### Prelude regeneration in CI

A CI step runs `scripts-build --prelude` and diffs the output against the committed file. Fails
if they diverge. This keeps the committed prelude honest without requiring a custom build.rs.

## Open questions

- **Prelude in pooled contexts:** pooled contexts are recycled per-entity. Evaluating the prelude
  in every warm-up is correct but may be measurable overhead at high entity counts. Profile before
  optimizing; the simple path is evaluate-on-warmup.
- **Luau prelude globals:** `light_animation.luau` returns a table (`LightAnimationSdk`) whose
  fields are promoted to individual globals. Verify mlua's `eval::<Table>()` returns the table
  cleanly and that iterating its fields for `globals.set()` covers all six helpers without
  hardcoding names. If mlua requires a different eval path, adjust accordingly.
- **scripts-build --prelude export coverage:** `ExportToGlobal` handles named `ExportDecl` and
  inlined `export` keywords cleanly. Re-exports (`export { x } from "./y"`) and default exports
  are out of scope for the initial sdk/lib surface (`world.ts` and `light_animation.ts` use only
  named `export const`/`export function`). Verify against the full sdk/lib surface before
  landing — if a re-export form is needed, extend the visitor; don't fall back to the
  self-invoking-function alternative without justification.
- **Level compiler: single entry point** — resolved. `script` names one `.ts` file. Shared
  helpers are imported from it; the map does not list multiple scripts.
