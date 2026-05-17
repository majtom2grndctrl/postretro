# Start-script import freshness

## Goal

`start-script.js` must be rebuilt when any `.ts` file it transitively imports
is edited — not only when `start-script.ts` itself changes. Today the mod-init
freshness check only compares `start-script.ts` vs `start-script.js` mtimes,
so editing a bundled import (e.g. `content/dev/scripts/player.ts`) leaves the
stale bundle running and changes appear lost.

## Scope

### In scope

- Expand the mod-init staleness check so a `start-script.js` that is older
  than any of its transitive `.ts`/`.tsx`/`.js`/`.mjs` imports counts as stale
  and is rebuilt before `setupMod` runs.
- Cover both the cold-path entry from `run_mod_init` and the recursive
  startup TS scan (`compile_stale_scripts`) so the two paths produce
  consistent results.
- Apply to debug builds only — the freshness check is already gated on
  `cfg(debug_assertions)`; release builds ship a pre-built `start-script.js`.
- Treat unresolved relative imports (file moved/deleted) as a force-rebuild
  signal so a broken bundle surfaces as a compile error rather than a silent
  stale run.
- Bare specifiers (e.g. `"postretro"`) are external by definition and
  contribute nothing to the freshness comparison.

### Out of scope

- Watching transitive imports at runtime via the FS watcher (the watcher
  already classifies any mod-root `.ts` edit as `ModInit`; nested edits under
  `scripts/` already trigger `Scripts`, and the mod-init re-run path runs the
  same freshness check this plan fixes).
- Persisting an import graph across runs (the graph is rebuilt each scan).
- Hot-reload of definition scripts inside a running VM (not implemented;
  unrelated).
- Adding a `--list-files` mode to `scripts-build` (see *Rough sketch* for why
  parsing in-engine is preferred for this iteration).
- Bundler-level incremental builds (`swc_bundler` re-bundles the whole entry
  every time; this plan only changes when we *decide* to rebuild).
- Extending the freshness model to `.luau` `require` graphs.

## Acceptance criteria

- [ ] Editing a `.ts` file that `start-script.ts` imports transitively
      (direct import, or import-of-import) causes the next `run_mod_init`
      call in a debug build to recompile `start-script.js` before evaluating
      it. Observable: the new bundle's exported behavior takes effect on the
      next mod-init without the user touching `start-script.ts`.
- [ ] When no `.ts` in the transitive closure has changed since
      `start-script.js` was last written, `run_mod_init` does not invoke
      `scripts-build`. Observable via absence of the `[Scripting]` compile
      log line and unchanged `start-script.js` mtime.
- [ ] When a relative import named in any reachable `.ts` cannot be resolved
      from disk (file deleted or renamed), the freshness check treats the
      bundle as stale and rebuilds, surfacing the bundler's error through
      the existing `ScriptError::InvalidArgument` path returned from
      `run_mod_init`.
- [ ] The startup recursive scan (currently `scan_and_compile_stale_ts`
      under `mod_root` shallow + `script_root` recursive) accounts for
      transitive imports when deciding whether `start-script.js` is stale,
      so a single `compile_stale_scripts` pass leaves no stale mod-root
      bundle behind.
- [ ] Bare-specifier imports (e.g. `import { defineEntity } from "postretro"`)
      are ignored by the freshness check — their presence neither triggers
      a rebuild nor causes a resolution error.
- [ ] Release builds (`cfg(not(debug_assertions))`) are unchanged: no scan,
      no bundler invocation, no new dependencies on the script-compiler
      crate in the release engine binary.
- [ ] A cycle in the import graph (`a.ts` imports `b.ts` imports `a.ts`)
      does not hang or stack-overflow the freshness check; each file is
      visited at most once per scan.

## Tasks

### Task 1: Transitive import walker

Build a debug-only helper that, given an entry `.ts` path, returns the set
of on-disk files reachable through relative imports. The walker parses each
file's `import`/`export` declarations with a lightweight scan (regex or a
hand-rolled tokenizer over the import-statement prefix is sufficient — full
parsing is unnecessary), resolves each relative specifier the same way the
bundler does (`./foo` → tries `.ts`, `.tsx`, `.js`, `.mjs`, then `<dir>/index.<ext>`),
canonicalizes resolved paths to deduplicate, and skips bare specifiers.
Unresolvable relative specifiers are reported in the returned value so the
caller can decide to force-rebuild. The walker lives next to the existing
freshness helpers in `crates/postretro/src/scripting/runtime.rs` (or a new
sibling module if `runtime.rs` would cross the file-size yellow flag — see
`context/lib/development_guide.md` §2.1).

### Task 2: Wire the walker into `compile_start_script_if_stale`

Change the bundle-is-stale decision so the comparison is `js_mtime <= max(ts_mtime
for ts in transitive_closure(start_script_ts))` instead of `js_mtime <= ts_mtime`
for the entry alone. On any unresolved relative specifier in the closure,
treat as stale. The single existing `<=` mtime comparison (which handles
same-second saves) is preserved per file.

### Task 3: Apply the same check in the startup scan

`scan_and_compile_stale_ts` calls `compile_one_if_stale` for each `.ts` it
finds. For the specific case of the mod-root entry (`start-script.ts`), it
must use the transitive-aware decision from Task 2 rather than the per-file
sibling comparison. Nested `.ts` files under `script_root` keep their
per-file sibling check — those are leaf compilation targets in the existing
contract and their bundling (if any) is the next mod-init's concern.

### Task 4: Tests

- Unit test: walker returns the entry plus every reachable relative file;
  bare specifiers excluded; cycles terminate.
- Unit test: walker reports unresolved relative specifiers.
- Integration test (debug-only, mirrors existing
  `compile_stale_scripts_recompiles_ts_with_stale_js_sibling` style): write
  `start-script.ts` importing `./helper.ts`, build the `.js`, backdate
  `start-script.js` so it is older than `helper.ts`, run the freshness
  check, assert `scripts-build` ran and `start-script.js` was rewritten.
- Integration test: edit only the deeper grandchild (`a.ts` imports `b.ts`
  imports `c.ts`; touch `c.ts`); assert rebuild fires.
- Integration test: no edits → no rebuild.
- Integration test: missing relative import forces a rebuild attempt; the
  resulting bundler error propagates through `run_mod_init` as
  `ScriptError::InvalidArgument`.

## Sequencing

Task 1 → Task 2 and Task 3 in parallel → Task 4 (some unit tests can land
with Task 1).

## Rough sketch

**Why parse in-engine instead of adding a `--list-files` mode to
`scripts-build`:** the sidecar already does the full bundle pipeline
(parse + resolve + strip + emit) in one shot, and a second sidecar
invocation per mod-init just to learn the import graph doubles the cost of
the steady-state "nothing changed" path. An in-engine scan with a small
regex over `^\s*(import|export)\b.*from\s*["']([^"']+)["']` and
`^\s*import\s*["']([^"']+)["']` is enough: TS-specific syntax does not
affect import-statement shape, and false positives (e.g. an import string
inside a comment) only cost extra mtime stats, never correctness.

**Resolver parity:** mirror `resolve_with_extensions` from
`crates/script-compiler/src/lib.rs` (extension order: `ts`, `tsx`, `js`,
`mjs`; directory → `index.<ext>`). Diverging here would mean the freshness
check and the bundler disagree about which files belong to the bundle.

**Canonicalization:** canonicalize each resolved path before insertion into
the visited set so symlinks and `./a/../b`-style specifiers deduplicate
correctly. Matches the bundler's `std::fs::canonicalize` step.

**Failure modes:**
- Read error on a file in the closure → log a warning, treat as stale
  (rebuild surfaces the underlying error consistently through the bundler).
- Walker can't read `start-script.ts` itself → fall through to the existing
  error path in `compile_start_script_if_stale` (stat failure on the entry).

## Open questions

- Should the walker also follow `.js` and `.mjs` imports (the bundler does)?
  Recommendation: yes — the resolver already lists those extensions, so a
  hand-written `start-script.ts → ./vendor.js` import would otherwise miss
  updates. Confirm with project owner before implementation if there is a
  reason to restrict to `.ts` only.
- Is there value in caching the resolved import graph between calls within
  a single process lifetime? The frame-loop re-runs `run_mod_init` on
  every `ReloadKind::ModInit` event. A per-process cache keyed on entry
  mtime would skip re-parsing when nothing changed, but the simpler
  always-rescan path is fine for now — file counts are small (single
  digits in the dev mod). Defer until profiling shows it matters.
