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
- `const enum` usage across file boundaries documented as unsupported and flagged in
  `tsconfig.json` via `isolatedModules: true`.

### Out of scope

- Luau SDK prelude (`sdk/lib/*.luau` equivalents compiled to a prelude). Luau SDK lib stays
  as source files modders import directly for now.
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
missing-compiler / stale-js fallback logic from the acceptance criteria. Update the FGD
`worldspawn` definition to document `scripts_dir`.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the SDK import convention; `arena-wave.ts`
change depends on it.

**Phase 2 (concurrent):** Task 2 and Task 3 — independent of each other; both can start once
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

A CI step (or a `cargo xtask` alias) runs `scripts-build --prelude` and diffs the output against
the committed file. Fails if they diverge. This keeps the committed prelude honest without
requiring a custom build.rs.

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
