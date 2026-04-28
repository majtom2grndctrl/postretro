# Scripting Compile Pipeline

## Goal

Three related improvements: unify SDK library imports under the `"postretro"` bare specifier
via an engine-evaluated JS prelude; remove the tsc/npx compile dependency so scripts-build
(SWC) is the sole TS compiler; and move script compilation into the map build step so `prl-build`
compiles scripts when a worldspawn KVP lists them. Together these eliminate the NPM/tsc runtime
dependency and make content packaging coherent — compiling a map also compiles its scripts.

## Scope

### In scope

- **SDK prelude:** bundle `sdk/lib/*.ts` into a committed `sdk/lib/prelude.js`; engine evaluates
  it before user scripts so all SDK lib exports are runtime globals. `import { world } from
  "postretro"` replaces `import { world } from "../../../sdk/lib/world"`.
- **scripts-build only:** remove `tsc`/`npx` fallback detection from the hot-reload watcher;
  scripts-build is the sole TS compiler path.
- **Level compiler script compilation:** worldspawn `scripts_dir` KVP triggers script
  compilation in `prl-build`; compiled `.js` files land beside their `.ts` sources.
- **Hot reload wiring:** connect the watcher to the frame loop and fix the reload path so it
  actually re-evaluates changed scripts in the correct context without duplicating handlers.
- `const enum` usage across file boundaries documented as unsupported and flagged in
  `tsconfig.json` via `isolatedModules: true`.

### Out of scope

- Luau SDK prelude (`sdk/lib/*.luau` equivalents compiled to a prelude). Luau SDK lib stays
  as source files modders import directly for now. This asymmetry is intentional: TS authors use
  `import { world } from "postretro"` (bare specifier, prelude-backed), while Luau authors
  continue to use relative imports from `sdk/lib/` until a Luau prelude is added.
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

### scripts-build only

- [ ] `TsCompilerPath::Tsc` and `TsCompilerPath::Npx` variants removed; detection cascade:
  scripts-build next to engine executable → scripts-build on PATH.
- [ ] Engine logs a clear actionable message when no TS compiler is found (install scripts-build
  or add it to PATH). `.luau` hot reload still works without scripts-build.
- [ ] `tsconfig.json` in `content/tests/scripts/` sets `"isolatedModules": true` (enforces
  the `const enum` ban at type-check time).

### Level compiler script compilation

- [ ] worldspawn `scripts_dir` KVP accepted; value is a path relative to the `.map` file.
- [ ] `prl-build` compiles all `.ts` files in that directory via scripts-build; compiled `.js`
  files land beside their `.ts` sources.
- [ ] `prl-build` exits non-zero with a clear error if `scripts_dir` is set, scripts-build is
  not found, and any `.ts` file lacks a `.js` sibling.
- [ ] `prl-build` succeeds with a warning if scripts-build is not found but all `.ts` files
  already have up-to-date `.js` siblings.
- [ ] FGD `worldspawn` entity updated with `scripts_dir` property.

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

### Task 2: Remove tsc/npx detection

Remove `TsCompilerPath::Tsc` and `TsCompilerPath::Npx` enum variants from `watcher.rs`.
Simplify `detect_with` to two steps: scripts-build next to `current_exe`, then scripts-build on
PATH. Update the missing-compiler log message to be actionable. Update or remove tests that cover
tsc/npx detection paths. Set `"isolatedModules": true` in `content/tests/scripts/tsconfig.json`.

### Task 3: Level compiler script compilation

Parse worldspawn `scripts_dir` property in `parse.rs` using the existing `get_property` pattern.
In `main.rs`, add a script compilation step after map parsing: locate scripts-build using the
same cascade as the engine (next to the compiler binary, then on PATH), enumerate `.ts` files in
the resolved `scripts_dir`, invoke scripts-build per file, and collect errors. Apply the
missing-compiler / stale-js fallback logic from the acceptance criteria. `prl-build` should skip
recompiling a `.ts` file if a `.js` sibling already exists with a newer mtime, so repeated
`prl-build` runs on an unchanged map don't recompile scripts unnecessarily. Update the FGD
`worldspawn` definition to document `scripts_dir`.

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
behavior scripts from disk → log result.

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

The bundler already handles a single entry file with relative imports. A `--prelude` flag
changes the output pass: instead of `StripExternalImports` only removing bare-specifier
declarations, also replace surviving `export` declarations with bare assignments to the global
scope (so `export const world = ...` becomes `world = ...` — the symbol lands as a global in the
QuickJS context rather than as a module export).

### Prelude regeneration in CI

A CI step runs `scripts-build --prelude` and diffs the output against the committed file. Fails
if they diverge. This keeps the committed prelude honest without requiring a custom build.rs.

## Open questions

- **Prelude in pooled contexts:** pooled contexts are recycled per-entity. Evaluating the prelude
  in every warm-up is correct but may be measurable overhead at high entity counts. Profile before
  optimizing; the simple path is evaluate-on-warmup.
- **Luau SDK prelude:** `sdk/lib/*.luau` equivalents don't exist yet. Should this plan stub them
  or defer entirely? The Luau SDK surface is currently weaker than the TS one anyway.
- **scripts-build --prelude export rewriting:** the "replace `export const` with global
  assignment" approach is simple but fragile for complex export patterns (re-exports, default
  exports). An alternative is to wrap the bundled output in a self-invoking function and assign
  each named export to `globalThis`. Either way, the approach should be verified against the full
  sdk/lib surface before landing.
- **Level compiler: scripts_dir or explicit list?** The spec uses a directory (`scripts_dir`)
  because it's simpler and matches the engine's existing "load everything in scripts/" behavior.
  An explicit list (`scripts`) would give authors more control but requires a list parser. If
  the single-directory approach doesn't cover real needs, switch before implementation.
