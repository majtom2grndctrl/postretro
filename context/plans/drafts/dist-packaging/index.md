# Dist Packaging

## Goal

`cargo run -p xtask -- dist` produces a self-contained folder that runs on a machine with no
repository, no Rust toolchain, and no build tools. Today every runtime-required artifact except the
committed assets — `.prl` levels, `.prm` material mips, the bundled `start-script.js` — is a
gitignored build product, and a release engine cannot generate any of them. Packaging is the last
thing standing between the engine and a second pair of hands on it.

## Scope

### In scope

- `dist` subcommand in `xtask`, dispatched like `crate-graph` and implemented in its own module.
- Level-set resolution from the mod itself: scan the bundled `start-script.js` for `maps/<name>.prl`
  string literals; that set is what ships.
- A committed `dist.toml` manifest holding package name, mod root, and a bake recipe only for the
  outputs whose source or flags the default cannot infer.
- Five build stages: release engine binary, `start-script` bundle, level-set resolution, payload
  assembly, `--release` level bakes.
- Payload assembly that excludes source-only and stale-generated files from the working tree.
- A launcher script that pins the working directory and the mod root.
- A human-facing `docs/distribution.md` covering the Windows path end to end.

### Out of scope

- **Cross-compilation.** `dist` builds for the host. A Windows payload comes from running `dist` on
  Windows. `rquickjs-sys`, `luau0-src` and `blake3` all compile native C/C++ through `cc`; targeting
  `x86_64-pc-windows-msvc` from Linux requires an MSVC sysroot, and `x86_64-pc-windows-gnu` puts
  Luau and wgpu's DX12 backend on a target this project has never built. Neither belongs inside a
  packaging command.
- **Archive creation.** `dist` emits a folder. Zipping it needs either a new `zip` dependency or a
  shell-out to a tool whose availability differs per platform; Explorer's "Send to → Compressed
  folder" and `actions/upload-artifact` (which zips its input) both already cover it.
- **CI.** No workflow file, no runner configuration. `dist` is the unit CI would call; whether CI
  calls it is a separate decision.
- **Code signing and installers.** An unsigned binary raises Windows SmartScreen. Accepted.
- **Pruning `baked/materials`** to the subset a payload references. Extras are inert — the runtime
  resolves a material by `blake3(baseColor PNG bytes)` and opens it by key, never by enumeration —
  so the cost is payload bytes only.
- **A shipped `content/base` game.** `content/base` holds UI descriptors and the splash today; the
  packaged mod is whatever `dist.toml` names.

## Direction

**Problem.** A release engine cannot build its own content. TypeScript compilation is
`#[cfg(debug_assertions)]` (`compile_start_script`, `scan_and_compile_stale_ts`), levels exist only
as `.map` sources in git, and `.prm` mips are written by `prl-build`. Every path that produces a
runnable tree today runs through a debug build in a checkout. Nothing assembles the artifacts a
stranger's machine needs.

**Prior commitments.**

- `dist/` is already gitignored as "Distribution artifacts", and
  `context/plans/done/repo-layout-base-game/index.md` explicitly deferred `dist/` packaging
  automation as a future step. This spec is that step.
- `prl-build --release` is documented as the only shippable bake: "ship only `--release` artifacts."
  The interactive default is a warm cache build with approximate indirect lighting.
- Launchers own tool building, engines do not: `TsCompilerPath::detect` "is intentionally
  side-effect-free: development launchers own building `scripts-build` before the engine starts."
  `dist` is a launcher in that sense and builds what it needs.
- The runtime binary never links `swc_*`. `dist` moves TypeScript bundling to build time rather than
  reopening that decision.

**Alternatives rejected.**

*Derive the level set from `content/<mod>/maps/*.map`.* Falsified by the content: the directory holds
30 `.map` files, and `mapCatalog` lists 13 levels, five of which have no `.map` of that name —
`campaign-test--0.02-mtex`, and four `occlusion-test--*` variants baked from the same sources at
different `--lightmap-density` values. The directory over-ships and under-covers at the same time.

*Hold the shipped level set in `dist.toml` as an explicit list, with a scan of the bundle as a
coverage check that fails on mismatch.* Rejected as a duplicate of the mod's own catalog. A scan
trusted enough to fail a build is trusted enough to drive one; inverting it removes the second list
without weakening anything. Measured: a real `scripts-build` bundle of `content/dev/start-script.ts`
contains all 13 catalog paths as string literals, sourced from `defineMapCatalog`'s `path` field
rather than from per-map script imports — so a level with no data script of its own still appears.
This also keeps `context/plans/done/mod-map-catalog/index.md`'s commitment that `id` is the identity and `path` is incidental:
`dist.toml` no longer keys anything by output path.

*Evaluate the mod's TypeScript to read `mapCatalog` as a value.* Makes `xtask` a script host — it
would need QuickJS plus the SDK prelude — to reach a structure the compiler flags are not stored in
anyway. The textual scan gets the same set for a regex.

*Let the release engine compile `.ts` at startup.* Reverses the binary-size decision behind the
sidecar split, and would put an `swc` bundler in every shipped copy to do work that is identical on
every launch.

**Placement.** `dist` sits in `xtask`, the crate that already owns multi-stage build orchestration
(`run` builds the `scripts-build` sidecar before launching the engine). It is not engine code: the
engine never assembles a payload. It is not a `prl-build` mode: `prl-build` compiles one map and has
no view of the binary, the scripts, or the payload.

**Foreclosures.** The level set is only as visible as a textual scan makes it: a catalog path
assembled at runtime (`"maps/" + id + ".prl"`) would not ship. No catalog entry does this today, and
the failure is loud — the level is absent from the payload and the resolution stage never saw it.
The launcher hardcodes a single `--mod`, which forecloses a multi-mod payload; reversible in an
afternoon.

**One-way doors.** None material. The command reads committed sources and writes to a gitignored
`dist/`; deleting the module and `dist.toml` restores the current state exactly.

## Acceptance criteria

- [ ] `cargo run -p xtask -- dist` on a clean checkout produces `dist/<package name>/` containing
      the engine binary, the launcher, `content/base/`, the mod root tree, and `baked/materials/`.
- [ ] The produced folder runs on a machine with no repository and no Rust toolchain: launching it
      reaches the frontend menu, and starting each catalog level reaches gameplay.
- [ ] The produced folder runs after being moved to an arbitrary directory and after the repository
      it was built from is deleted.
- [ ] The payload contains exactly one `.prl` per `maps/<name>.prl` literal in the bundled
      `start-script.js`, and no others.
- [ ] Every `.prl` in the payload is a `--release` bake. `dist` supplies `--release`, `--no-tui` and
      `-o` itself and fails with a message naming the offending recipe when a manifest `args` list
      contains any of them.
- [ ] No `.map`, `.ts`, `.md`, or `maps/autosave/` file appears anywhere in the payload.
- [ ] A stale `.prl` or `.js` in the working tree never reaches the payload: after `dist` runs, each
      `.prl` in the payload is byte-identical to the artifact that run baked, and
      `<mod_root>/start-script.js` is byte-identical to the bundle that run emitted.
- [ ] `dist` fails before baking, listing every unresolved output, when a scanned level has neither a
      `dist.toml` recipe nor a `.map` at the default location. Adding a catalog level whose `.map`
      shares its stem requires no `dist.toml` edit.
- [ ] `dist` fails with a message naming the missing file when a recipe's `.map` source does not
      exist, and when the mod root has no `start-script.ts` or `start-script.luau`.
- [ ] A second `dist` run over an existing `dist/<name>/` produces a payload with no file left over
      from the previous run.
- [ ] `dist` prints, per stage, what it built, and closes with the payload's total size and file
      count.
- [ ] `cargo run -p xtask -- --help` lists `dist` with its flags.
- [ ] `docs/distribution.md` takes a reader from a fresh Windows checkout to a shareable folder,
      naming the toolchain prerequisites and the SmartScreen prompt their recipient will see.

## Tasks

### Task 1: `dist` command vertical slice

Add a `dist` module to `xtask` (`crates/xtask/src/dist.rs`), dispatched from `try_main` as
`dist::run(args.collect())` following the `crate_graph::run` precedent, and add its usage line to
`print_help`. Define and parse the `dist.toml` manifest: a `[package]` table with `name` (payload
folder name) and `mod_root` (workspace-relative path to the mod, e.g. `content/dev`), plus an array
of `[[recipes]]` tables, each keyed by `output` (a path relative to the mod root, e.g.
`maps/campaign-test--0.02-mtex.prl`) and carrying optional `source` (workspace-relative `.map` path)
and optional `args` (extra `prl-build` flags). Recipes are overrides, not the level set — that comes
from stage 3. Support `--manifest <path>` to override the default `dist.toml` at the workspace root,
and `--out <dir>` to override the default `dist/` output root. Reject at parse time, naming the
offending recipe, any `args` list containing `-o`, `--release`, `--tui` or `--no-tui`: `dist`
supplies all four itself, and a recipe that re-supplies them either duplicates a flag or silently
redirects the output away from the payload. Then run five stages in order, printing a header per
stage and failing fast with a message naming the failing stage and input. (1) Build the
`scripts-build` sidecar and `cargo build --release -p postretro --bin postretro` — do not pass
`--features`, since `dev-tools`, `observability` and `capture` are all non-default. (2) Run
`scripts-build --in <mod_root>/start-script.ts --out <payload>/<mod_root>/start-script.js`, or copy
`<mod_root>/start-script.luau` verbatim when the mod is Luau-authored; fail naming the mod root when
it holds neither. `mod_init` probes `start-script.js` then `start-script.luau` and reads no other
loose script, so this is the whole script payload — every per-level script is already embedded in its
`.prl` by `prl-build`. (3) Resolve the level set: scan the emitted `start-script.js` as text for
`maps/<name>.prl` string literals, and for each one resolve a bake recipe — the matching `[[recipes]]`
entry if present, else the default `<mod_root>/maps/<stem>.map` with no extra flags. Fail here,
listing every unresolved output at once, when a scanned level has no recipe and no `.map` at the
default location, or when a recipe's `source` does not exist; a `--release` bake run is minutes per
level and must not begin against inputs already known to be absent. (4) Copy the release binary to
the payload root, copy `content/base/` and the mod root tree into the payload preserving their
workspace-relative paths, and copy `<workspace>/baked/materials/` to `<payload>/baked/materials/`.
(5) For each resolved level, run the `prl-build` binary with its source, `--release`, `--no-tui`, its
`args`, and `-o <payload>/<mod_root>/<output>`. Bakes run last on purpose: they dominate wall-clock,
and a failure at level 12 should not discard the cheap stages. Delete an existing `dist/<name>/`
before stage 2 so no file survives a previous run. Close by printing the payload's file count and
total size. Keep the stage-4 copy deliberately simple — Task 2 owns the exclusion rules — but note
that stage 4 must not overwrite the stage-2 bundle. Verify the slice by launching the produced
payload from its own directory and reaching gameplay from the frontend menu.

### Task 2: Payload exclusion rules and stale-artifact guards

Replace the tree copy in the `xtask` `dist` module's assembly stage with a filtered copy. Exclude,
by extension, `.map`, `.ts`, `.md`, `.prl`, `.js` and `.bsp`; exclude any path with a `maps/autosave/`
component, and any `.build-caches` or `.DS_Store` entry. The first three are source-only inputs a
runtime never reads. The `.prl` and `.js` exclusions are the load-bearing ones and are not
housekeeping: a developer's working tree accumulates warm-cache `.prl` bakes and debug-scan `.js`
bundles at exactly the paths the bake and bundle stages write to, so an unfiltered copy can overwrite
an exact-lighting level with an approximate one, or a fresh bundle with a stale one, depending on
copy order. Excluding both from the tree copy makes the bake and bundle stages the only writers of
those extensions, which is the guarantee the Invariants table calls "no working-tree build product
reaches the payload". Factor the exclusion decision into a pure predicate over a workspace-relative
path and unit-test it: each excluded extension, a nested `maps/autosave/` path, and a `.png`,
`.gltf`, `.bin`, `.ogg`, `.wav`, `.json` and `.ttf` path that must survive. Then add a post-assembly
sweep that walks the finished payload and fails, listing offenders, if any excluded extension is
present — the check that survives a future edit to the copy routine.

### Task 3: Launcher and launch contract

Have the `xtask` `dist` module emit a launcher beside the engine binary in the payload: `<package
name>.bat` on a Windows host, `<package name>.sh` (executable) elsewhere. The batch file sets its own
directory as the working directory (`cd /d "%~dp0"`) and runs `postretro.exe --mod <mod_root>` with
the mod root taken verbatim from the manifest; the shell script does the equivalent with `cd "$(dirname
"$0")"`. Both matter because every content path the engine resolves is joined against the process
working directory — `ui_asset_path` hardcodes `content/base/ui`, `SplashSource::base_path` hardcodes
the splash PNG path, and the content root defaults to the grandparent of
`content/dev/maps/campaign-test.prl` when no argument is given. Passing `--mod` explicitly keeps a
payload whose mod root is not `content/dev` working, and pinning the working directory keeps the
payload working when launched from a shortcut or another directory. Do not make the launcher pass a
map argument: with no map the mod's `frontend.menuTree` drives the first screen, which is the
behavior a recipient should get. Verify by running the payload from a different working directory
via the launcher and reaching the frontend menu.

### Task 4: Dev-mod bake recipes and resolution tests

Author the workspace-root `dist.toml` for the dev mod — `name`, `mod_root = "content/dev"` — and a
`[[recipes]]` entry only for the outputs the default cannot infer. The default is
`<mod_root>/maps/<stem>.map` with no extra flags, which resolves 8 of the dev catalog's 13 levels
(`campaign-test`, `combat-demo`, `trap-pools`, `occlusion-test`, and the four
`test_animated_weight_maps_*`); do not write recipes for those. The remaining five have no `.map` of
their own name and differ from their source only by compiler flags: `campaign-test--0.02-mtex` from
`campaign-test.map`, and `occlusion-test--0.01-mtex`, `occlusion-test--0.015-mtex`,
`occlusion-test--0.02-mtex` and `occlusion-test--shadow-resolution-test` from `occlusion-test.map`.
The `--*-mtex` four encode a `--lightmap-density` value in their names; note beside each recipe where
its flags came from. The `--shadow-resolution-test` variant's flags are not recoverable from the
catalog entry or its tags — raise it rather than guessing, and leave the entry out until the owner
supplies them. Then unit-test the two pure pieces of the resolution stage in the `xtask` `dist`
module. The scanner: a bundle fixture containing two `maps/*.prl` literals, one inside a line
comment, a `maps/` string that is not a `.prl`, and the same literal twice — the result is the
deduplicated set. The resolver: an output with a matching recipe takes the recipe's source and args;
an output with none takes the stem default; an output with neither a recipe nor a `.map` at the
default location is an error, and several such outputs report together rather than one at a time.
Both consumers are the recipient of the payload, who otherwise gets a menu button that loads nothing
and no way to tell whether the level is missing or broken.

### Task 5: Distribution guide

Write `docs/distribution.md` — human-facing, so `context_style_guide.md` does not govern it — taking a
reader from a checkout to a folder they can hand to someone. Cover: the host-builds-for-host rule and
why cross-compiling is not offered (`rquickjs-sys`, `luau0-src` and `blake3` compile native C/C++);
Windows prerequisites (Visual Studio Build Tools with the C++ workload, the MSVC Rust toolchain); the
`dist` invocation and its flags; how to edit `dist.toml` to add or drop a level; that `--release`
bakes are exact-lighting and take substantially longer than a dev bake; how to zip and send the
folder; and what the recipient sees — the SmartScreen "Windows protected your PC" prompt on an
unsigned binary and the "More info → Run anyway" path through it, a DX12-or-Vulkan GPU requirement,
settings written to `%APPDATA%\postretro\settings.toml`, and the documented white flash at window
creation (`context/lib/boot_sequence.md`). Link it from `README.md`.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice, falsifies the payload-layout and launch-path
assumptions before any hardening is written against them.
**Phase 2 (concurrent):** Task 2, Task 3 — independent; Task 2 edits the assembly stage, Task 3 adds
launcher emission.
**Phase 3 (sequential):** Task 4 — recipes and resolution tests land against the assembled payload
from Tasks 2 and 3.
**Phase 4 (sequential):** Task 5 — documents the finished command surface.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Every payload `.prl` is a `--release` bake | Task 1 (bake stage supplies `--release`) | A manifest `args` list passing `--release`, `--tui` or `-o`; a working-tree `.prl` copied over a baked one | AC 5, AC 7 |
| No working-tree build product reaches the payload | Task 2 (exclusion predicate + post-assembly sweep) | Any future copy path added to the assembly stage | AC 6, AC 7 |
| Payload paths resolve cwd-relative from the payload root | Task 1 (tree copy preserves workspace-relative paths), Task 3 (launcher pins cwd) | Flattening the tree, or relocating `content/base` — `ui_asset_path` and `SplashSource::base_path` hardcode it | AC 2, AC 3 |
| The only loose script a release engine reads is `<mod_root>/start-script.{js,luau}` | Task 1 (bundle stage) | Shipping loose `.ts`, which QuickJS parses as JS and rejects; every other script is embedded in its `.prl` | AC 2, AC 6 |
| Payload level set equals the mod's catalog | Task 1 (stage 3 resolves the set from the bundle) | A catalog path built at runtime rather than written as a literal; a level with no recipe and no stem-matching `.map` | AC 4, AC 8 |

## Rough sketch

New module `crates/xtask/src/dist.rs`, entered from `try_main` as `dist::run(args.collect())`
alongside the `crate_graph` arm; `main.rs` gains a dispatch arm and help lines only.

Reuse the existing xtask helpers: `workspace_root()`, `run_checked(&mut Command, label)` for every
subprocess, and `build_scripts_sidecar(&cargo, &workspace_root, &[])` for stage 1's sidecar half.
Resolve `cargo` the same way the `run` path does — `std::env::var_os("CARGO")` with a `"cargo"`
fallback.

Locate the built binaries under `<workspace>/target/release/` by name, appending `.exe` under
`cfg!(windows)` — the same shape as `scripts_build_in_dir` in `crates/scripting-core/src/watcher.rs`.

`.prm` output is not addressable by flag: `resolve_prm_root_via_cargo` in
`crates/level-compiler/src/main.rs` walks for a `Cargo.toml` ancestor of the map source and lands on
`<workspace>/baked/materials`. Since `dist` bakes from workspace sources, the mips land there and
stage 4 copies the tree. The runtime side of that path is `derive_prm_root_dev_layout` in
`crates/postretro/src/startup/worker.rs`, which resolves the content root's grandparent plus
`baked/materials` — so the payload's `<root>/baked/materials` is what a payload rooted at
`<root>/content/<mod>` reads.

`toml` is already a workspace dependency; `xtask` needs it added to its own manifest along with
`serde` for the derive.

## Open questions

- Four of the five catalog variants encode their `--lightmap-density` in their names. The
  `occlusion-test--shadow-resolution-test` variant's flags are not recoverable from the catalog.
  Task 4 raises it rather than guessing; until the owner supplies the flags, `dist` fails on that
  level as unresolved — which is the correct behavior, but blocks a full payload.
- A cold `--release` bake bypasses the cache entirely, and
  `drafts/lighting-scale--cold-bake-reaching-light-spike/out-of-scope-findings.md` records two
  ship-config cold bakes SIGKILL-ed at 16 GB in the shadowmask stage. Thirteen sequential bakes in
  one command inherit that risk. Stage ordering keeps a crash from discarding the cheap stages, but
  a crash at level 12 still discards eleven bakes. If it bites in practice, a per-level
  skip-if-already-baked-this-run is the smallest fix; splitting bake from assemble is not, because
  verifying `--release` provenance across a process boundary needs a marker PRL does not carry.
- `DataScriptSection.source_path` embeds the absolute path of the build machine into every shipped
  `.prl` as a diagnostic label. Cosmetic, and it leaks the builder's directory layout. Left as-is;
  worth a follow-up if payloads are ever published broadly.
