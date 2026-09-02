# dist-packaging — research notes

Findings that shaped the spec. Not decisions; the spec holds those.

## What a release engine actually reads from disk

Grounded by reading the release-gated paths, not the dev ones.

| Artifact | Path at runtime | Source | Notes |
|---|---|---|---|
| Level | `<map arg>` or catalog `path` joined to content root | `prl-build` | `.prl` is gitignored; zero in the tree |
| Material mips | `<content_root>/../../baked/materials/<hex>.prm` | `prl-build` | `derive_prm_root_dev_layout` (`crates/postretro/src/startup/worker.rs`) |
| Mod entry script | `<content_root>/start-script.js` | `scripts-build` | `.js` gitignored |
| UI descriptors | `content/base/ui/{hud,pauseMenu,frontendMenu,keyboard}.json` | committed | `ui_asset_path` in `crates/ui/src/tree_asset.rs` — cwd-relative, hardcoded `content/base/ui` |
| Splash | `content/base/textures/splash/postretro-ascii-art.png` | committed | `SplashSource::base_path` |
| Models, textures, sounds | `<content_root>/<handle>` | committed | content-root join |
| Fonts | none | `include_bytes!` | `crates/ui/src/text.rs` compiles Inter + JetBrains Mono into the binary |

## Level scripts are embedded, not shipped

`compile_worldspawn_data_script` (`crates/level-compiler/src/main.rs`) reads the worldspawn
`data_script` KVP, shells out to `scripts-build`, and packs the output into the PRL
`DataScriptSection` (`compiled_bytes` + `source_path`). At load, `run_data_script` evaluates
`compiled_bytes` — it never reopens the source file.

Consequence: nothing under `<mod_root>/scripts/` is read by a release engine. `compile_start_script` and the
recursive `.ts` scan (`scan_and_compile_stale_ts`) carry `#[cfg(debug_assertions)]`; `start_watcher`
and `compile_stale_scripts` are cfg-split inside the body and no-op in release
(`crates/scripting-core/src/runtime/core.rs` discards their arguments in the `not(debug_assertions)`
arm).

`DataScriptSection.source_path` is the **absolute** path captured on the build machine, so it leaks
the builder's directory layout into shipped `.prl` files. The path *value* is a diagnostic label (VM
script name, warning text); its *extension* is functional — `run_data_script` computes `is_luau` from
`Path::new(&section.source_path).extension()` to pick the Luau or QuickJS path, the on-disk extension
being the only signal available at runtime.

## Release cannot compile TypeScript

`compile_start_script` and the whole compile scan are `#[cfg(debug_assertions)]`
(`crates/scripting-core/src/runtime/compile.rs`). `mod_init` mirrors the gate: `has_ts_or_js_source`
evaluates `ts_path.is_file()` in debug and literal `false` in release
(`crates/scripting-core/src/runtime/mod_init.rs`).

`run_script_file` accepts a `.ts` extension and feeds the raw bytes to QuickJS, which parses it as
plain JS — so a stray `.ts` in a shipped payload would be a syntax error, not a compile.

The sidecar split is deliberate: "The sidecar exists so the `postretro` *runtime* binary never links
`swc_*` crates (which add meaningful binary size)" — `crates/script-compiler/src/main.rs`.

## The map set cannot be derived from the maps directory

`content/dev/maps/` holds 32 `.map` files; `mapCatalog` (`content/dev/scripts/frontend-menu.ts`)
offers a fraction of them as levels. Stress fixtures, feature demos and capture rigs live in that
directory under the same extension, in the same place, as the sources behind the levels the mod
offers; the listing tells them apart by nothing.

Five catalog entries name a `.prl` with no `.map` of that stem, and Task 1 drops all five:

- `campaign-test--0.02-mtex.prl`
- `occlusion-test--0.01-mtex.prl`
- `occlusion-test--0.015-mtex.prl`
- `occlusion-test--0.02-mtex.prl`
- `occlusion-test--shadow-resolution-test.prl`

The four `-mtex` outputs bake `campaign-test.map` and `occlusion-test.map` at `0.01`-`0.02`
`--lightmap-density` against the `0.04` default (`lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS`), and
both sources ship at that default, so the four demonstrate a compiler knob rather than covering
content nothing else covers. Nothing outside the catalog names them. They are still the shape that
motivates `[[recipes]]`: output name and compiler flags are both build inputs that exist nowhere on
disk.

`occlusion-test--shadow-resolution-test` is different in kind: `prl-build`'s flag surface has no
shadow-resolution option (`--soft-shadow-samples` is penumbra sample count, not resolution), and no
`.map` carries that name, so no invocation of the current compiler produces that artifact. It is a
catalog entry pointing at a level nobody can build. `git log -S` finds no commit recording how it was
made.

Every catalog level Task 1 leaves in place has a `.map` of its own stem: `campaign-test`,
`combat-demo`, `trap-pools`, `occlusion-test` and the four `test_animated_weight_maps_*` maps.

Bake memory. `drafts/lighting-scale--cold-bake-reaching-light-spike/out-of-scope-findings.md` records
a confirmed SIGKILL at 16 GB in the shadowmask atlas stage, on a **157-light** map at
`--lightmap-density 0.25`; `1.0` completes, so peak rises as texel size falls. Catalog maps carry far
fewer lights (`campaign-test` 31, `occlusion-test` 9, `combat-demo` 8, `trap-pools` 0), and every one
of them bakes at the `0.04` default.

(The two figures for that fixture measure different things: 16 GB is the observed ceiling at which
the process was SIGKILLed; ~14 GB is the shadowmask plan's own before-figure for the stage's resident
set.)

`context/plans/done/shadowmask-bake-scaling/index.md` has since bounded the lights term — ~14 GB to
order 1 GB on the same fixture — so the recorded SIGKILL is not reproducible as recorded. The fix
reaches the cold path `--release` takes: `bake_shadowmask_atlas_cached_with_window` delegates to
`bake_shadowmask_atlas_with_window` with the same resident-layer window when the cache is `None`.

It leaves the density term untouched by its own non-goals: the atlas-size output buffer is "a function
of lightmap density", owned by the draft `lighting-scale--lightmap-bake-scaling`, and the change is
"a large constant-factor reduction on the dominant term, not an asymptotic bound" where "finer density
or many more lights re-consume the headroom".

The `pipeline.rs` lightmap composite that materializes all N `LightmapLayer`s does **not** apply to
`dist`. `stage_cache_is_enabled` is `!args.release && !args.no_cache`, so `--release` takes the cold
arm — annotated "No layer reads/writes" — which calls `bake_lightmap_controlled` and allocates one
`CompositedAtlas::zeroed(atlas_w, atlas_h, layer_count)`. That term is density-driven and
light-count-independent. The deferral to `lighting-scale--lightmap-bake-incremental-flush` Task 2
covers the warm/cached path, which `dist` never takes.

Every level `dist` bakes runs at the `0.04` default, so nothing in the shipped set loads the density
axis.

## prl-build surface used by dist

From `help_text()` in `crates/level-compiler/src/main.rs`:

- `-o <output.prl>` — output path.
- `--release` — "Produce a shippable map: exact lighting, cache bypassed (implies `--no-cache`). The
  interactive default is a fast warm build with approximate indirect lighting; ship only `--release`
  artifacts."
- `--no-tui` — forces `ReporterMode::Plain` via `select_reporter_mode`'s `TuiPreference::Disable`
  arm. A run with any non-tty stream already selects `Plain` under the `Auto` default, so this matters
  only when `dist` inherits a terminal.
- `--lightmap-density <METERS>`, `--sh-probe-spacing <METERS>`, etc. — the per-recipe knobs.

`.prm` output root is **not** a flag. `resolve_prm_root_via_cargo` walks for a `Cargo.toml` ancestor
of the map source and lands on `<workspace>/baked/materials`. Dist bakes from workspace sources, so
`.prm` files land there and must be copied into the payload afterward.

Unreferenced `.prm` files in that tree are inert: the runtime resolves a material to
`blake3(baseColor PNG bytes)` and looks the file up by key (`cache_filename_for_key`), so extras
cost payload size only.

## scripts-build surface

`scripts-build --in <INPUT.ts> --out <OUTPUT.js>` (`crates/script-compiler/src/main.rs`). Bundles
relative imports, strips TS syntax, drops bare specifiers. `content/dev/start-script.ts` imports
`../../sdk/behaviors/reference/entities`, so the bundle reaches outside the content root, and the
emitted `.js` is self-contained. Process cwd does not constrain this: `RelativeOnlyResolver::resolve`
joins each specifier against the importing module's own directory, and the entry path is canonicalized
before bundling.

## Launch-path resolution

- `resolve_content_root` (`crates/postretro/src/startup/session.rs`): `--mod <dir>`, else
  `--content-root <dir>`, else the map path's grandparent.
- `content_root_from_map(None)` falls back to `DEFAULT_MAP_PATH` = `content/dev/maps/campaign-test.prl`,
  so a no-argument launch resolves the content root to `content/dev`.
- No map argument means no boot map; the mod's `frontend.menuTree` drives the first screen.
- Everything is joined against the process cwd. Explorer sets cwd to the containing directory for
  both a double-clicked `.exe` and a double-clicked `.bat`.

## Windows

Already exercised: `context/lib/boot_sequence.md` documents the Win32 white-flash and the confirmed
hidden-window boot hang. No `cfg(unix)` outside tests; `scripts_build_in_dir` already probes
`scripts-build.exe`.

Cross-compiling from Linux is the obstacle, not the code. `rquickjs-sys` (C), `luau0-src` 0.18 (C++,
vendored via `mlua`'s `luau` feature) and `blake3` all build native sources through `cc`. Targeting
`x86_64-pc-windows-msvc` from Linux needs an MSVC sysroot; vendored C++ through that path is where
such setups usually fail. `x86_64-pc-windows-gnu` avoids MSVC but puts Luau and wgpu's DX12 backend
on an untested target.

No TLS stack in the tree — `renet`/`renet_netcode` over plain UDP, no `rustls`/`ring`.

## xtask conventions

`try_main` dispatches on the first argument; `crate_graph::run(args)` is the precedent for a command
whose body lives in its own module. Shared helpers: `workspace_root()`, `run_checked(&mut Command,
label)`, `status_code(...)`, `build_scripts_sidecar(...)`.

`crates/xtask/src/main.rs` is 2190 lines. The dist command follows the `crate_graph` precedent, so
main.rs gains a dispatch arm and help lines only — no split task is owed by the split-before-extend
rule. The file's size is a standing concern for whoever next adds a command body inline.

## The bundle carries every catalog path (measured)

Ran the real sidecar, not a reasoned argument:

```
cargo build -p postretro-script-compiler --bin scripts-build
./target/debug/scripts-build --in content/dev/start-script.ts --out /tmp/start-script.js
grep -o 'maps/[A-Za-z0-9._-]*\.prl' /tmp/start-script.js | sort -u
```

48 KB bundle, every `mapCatalog` path present as a string literal.

The literals originate in the catalog's own `path` field (`defineMapCatalog` in
`content/dev/scripts/frontend-menu.ts`), reachable from `start-script.ts` via
`import { mapCatalog }` and `maps: mapCatalog` in `defineMod`. They do **not** come from per-map
script imports, so a level with no data script of its own still appears.

Residual weakness, unchanged by which side drives: a path assembled at runtime
(`"maps/" + id + ".prl"`) is invisible to a textual scan. No catalog entry does this today — every
path is a literal.

## Derivations behind Task 2's stage rules

Moved out of the task paragraph, which the executor reads; the decisions stay there, the reasoning
lives here.

**`precheck_output_dir` and the `-o` parent.** It writes an interactive `[Y/n]` prompt to stderr and
waits on stdin when the output parent is missing, and `main` calls it unconditionally after
`select_reporter_mode` — it is not gated on `--no-tui`. On EOF, `parse_dir_answer(None)` yields
`DirAnswer::Abort` and it bails; its doc comment says as much ("EOF on stdin ... is treated as 'no
answer' -> abort, so this never hangs"). So the behavior splits by context: a blocking prompt under an
inherited terminal, an immediate abort under closed or redirected stdin. Creating the parent first is
the simplest way to make the stage behave identically in both; redirecting `prl-build`'s stdin would
also work.

**Sequential bakes.** `prl-build` parallelizes internally at `default_jobs_for(logical_cores)`:
`cores - 2` above 8, `cores - 1` for 2 through 8, `1` below. A second concurrent bake therefore
oversubscribes the machine and multiplies shadowmask atlas peak memory.

**Bake ordering is a heuristic.** Atlas area scales with the inverse square of texel size, but peak
also scales with light count, so the two pull in opposite directions. Ascending density is a cheap
proxy for descending peak memory — good enough to fail early, not a derivation.

**Stage 2's both-present rule is `dist`'s, not the runtime's.** `run_mod_init` gates its both-present
rejection on `has_ts_or_js_source`, whose `.ts` probe is `#[cfg(debug_assertions)]` and literally
`false` in release. A release engine therefore sees `.ts` + `.luau` as no conflict and silently takes
the Luau path. The pair `run_mod_init` genuinely rejects is `.js` + `.luau`. `dist` fails on
`.ts` + `.luau` because it has no rule for picking an entry language and must not invent one.

**A release sidecar does not fully foreclose a mid-bake `cargo build`.** `find_scripts_build` probes beside `current_exe`, falls back to scanning `PATH`, then calls
`is_compiler_stale` on whichever it found. After stage 1 the beside-probe always resolves —
`prl-build` runs from `<target-dir>/release/`, where stage 1 just wrote `scripts-build` — so the
`PATH` fallback never fires here and the mtime compared is the binary stage 1 built. The staleness
check is still the residual. It compares that mtime against the newest under
`compiler_freshness_roots()` — `crates/script-compiler/src` **and `sdk/lib`**.
`sdk/lib` holds `.ts` files that are not `include_str!`'d and so are not cargo-tracked. Edit one:
stage 1's `cargo build` is a no-op that does not relink, the binary stays older than the edited file,
and every data-script-carrying bake re-shells `cargo build`. That rebuild is itself a no-op, so the
cost is seconds per level rather than incorrectness — but stage 1 does not prevent it, and P29 pins
that the behavior is documented rather than silent.
