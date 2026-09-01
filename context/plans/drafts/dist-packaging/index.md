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
- Level-set resolution from the mod itself: scan the emitted mod entry script for `maps/<name>.prl`
  string literals; that set is what ships.
- A committed `dist.toml` manifest holding package name, mod root, and a bake recipe only for the
  outputs whose source or flags the default cannot infer.
- Seven build stages: release binaries, entry-script bundle, level-set resolution, model-texture
  bake, payload assembly, `--release` level bakes, baked-materials copy.
- Payload assembly that excludes source-only and stale-generated files from the working tree.
- A `.dist-incomplete` completion gate that distinguishes a partial payload from a finished one on
  any exit path, including a kill.
- A launcher script that pins the working directory and the mod root.
- Deleting the one `mapCatalog` entry whose level no invocation of the current compiler can build.
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
  resolves a material by `blake3(baseColor PNG bytes)` and opens it by key via
  `cache_filename_for_key` (`crates/renderer/src/render/loaded_texture.rs`), never by enumeration —
  so the cost is payload bytes only. Stage 7 does exclude `atomic_write`'s `*.tmp.*` partials, which
  are not inert-by-key but leftover halves of an interrupted write.
- **A shipped `content/base` game.** `content/base` holds UI descriptors, the splash, and the font
  sources that `crates/ui/src/text.rs` compiles in with `include_bytes!` (shipped for their OFL
  licenses, not read at runtime); the packaged mod is whatever `dist.toml` names.
- **Relativizing `DataScriptSection.source_path`.** Every shipped `.prl` carries the build machine's
  absolute path. Accepted here: an absolute path is functionally harmless, and a payload goes to a
  known recipient. It is not, however, inert — `run_data_script`
  (`crates/scripting-core/src/runtime/data_script.rs`) reads its **extension** to dispatch between
  the Luau and QuickJS paths, so any later relativizing must preserve the extension. That makes the
  deferred fix a value change at the producer with a real correctness constraint, not a one-liner.

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
`campaign-test--0.02-mtex` and three `occlusion-test--*-mtex` variants baked from the same sources at
finer `--lightmap-density`, plus `occlusion-test--shadow-resolution-test`, which no invocation of the
current compiler reproduces at all (Task 1 removes it from the catalog). The directory over-ships and
under-covers at the same time.

*Hold the shipped level set in `dist.toml` as an explicit list, with a scan of the bundle as a
coverage check that fails on mismatch.* Rejected as a duplicate of the mod's own catalog. A scan
trusted enough to fail a build is trusted enough to drive one; inverting it removes the second list
without weakening anything. Measured: a real `scripts-build` bundle of `content/dev/start-script.ts`
contains all 13 catalog paths as string literals, sourced from `defineMapCatalog`'s `path` field
rather than from per-map script imports — so a level with no data script of its own still appears.
`dist.toml` therefore no longer holds the level set. It still keys each `[[recipes]]` entry by
`output` path, so renaming a catalog entry's `path` invalidates its recipe; stage 3 reports that as
an orphan rather than letting it pass silently. That is a narrower coupling to `path` than a second
level list, and it leaves `context/plans/done/mod-map-catalog/index.md`'s commitment intact: `id` is
the identity, `path` is incidental, and nothing in `dist.toml` claims otherwise.

*Evaluate the mod's TypeScript to read `mapCatalog` as a value.* Makes `xtask` a script host — it
would need QuickJS plus the SDK prelude — to reach a structure the compiler flags are not stored in
anyway. The textual scan gets the same set for a regex.

*Let the release engine compile `.ts` at startup.* Reverses the binary-size decision behind the
sidecar split, and would put an `swc` bundler in every shipped copy to do work that is identical on
every launch.

**Placement.** `dist` sits in `xtask`, the crate that already owns multi-stage build orchestration
(`run` builds the `scripts-build` sidecar before launching the engine). It is not engine code: the
engine never assembles a payload. It is not a `prl-build` mode: `prl-build` compiles one map, and
while `compile_worldspawn_data_script` does reach a level's own data script through
`find_scripts_build`, it has no view of the engine binary, the mod's entry script, or the payload.

**Foreclosures.** The level set is only as visible as a textual scan makes it: a catalog path
assembled at runtime (`"maps/" + id + ".prl"`) would not ship, and nothing would report it — the
resolution stage never saw the level, so no check fires and the recipient gets a menu button that
loads nothing. No catalog entry does this today; the exposure is accepted, not mitigated. The
launcher hardcodes a single `--mod`, which forecloses a multi-mod payload; reversible in an
afternoon.

**One-way doors.** The command reads committed sources and writes to a gitignored `dist/` and a
gitignored `target/dist-work/`. The one edit outside those is Task 1's deletion of the unbuildable
`occlusion-test--shadow-resolution-test` entry from `mapCatalog` — a single catalog entry, a six-line
object literal, trivially restorable from git, but it does mean "delete the module and `dist.toml`"
is not by itself a full revert.

## Acceptance criteria

- [ ] **AC1** `cargo run -p xtask -- dist` on a clean checkout produces `dist/<package name>/`
      containing the engine binary, the launcher, `content/base/`, the mod root tree, and
      `baked/materials/`.
- [ ] **AC2** The produced folder runs on a machine with no repository and no Rust toolchain:
      launching it reaches the frontend menu, and starting each catalog level reaches gameplay.
- [ ] **AC3** The produced folder runs after being moved to an arbitrary directory and after the
      repository it was built from is deleted.
- [ ] **AC4** The payload contains exactly one `.prl` per `maps/<name>.prl` literal in the entry
      script stage 2 emitted, and no others.
- [ ] **AC5** Every `.prl` in the payload is a `--release` bake. `dist` supplies `-o`, `--release`
      and `--no-tui` itself and fails with a message naming the offending recipe when a manifest
      `args` list contains `-o`, `--release`, `--tui` or `--no-tui`.
- [ ] **AC6** No `.map`, `.ts`, `.md`, `.bsp`, `.DS_Store` or `maps/autosave/` file appears anywhere
      in the payload, and the payload's mod root holds exactly one of `start-script.js` or
      `start-script.luau` — never both, never neither.
- [ ] **AC7** The payload's `<mod_root>/start-script.{js,luau}` is byte-identical to the scratch
      entry script this run emitted. The payload's `.prl` set is exactly stage 3's resolved outputs,
      and every one of them was created after this run deleted the payload root — no `.prl` in the
      payload predates stage 5.
- [ ] **AC8** `dist` fails before baking, listing every unresolved output, when a scanned level has
      neither a `dist.toml` recipe nor a `.map` at the default location. Adding a catalog level whose
      `.map` shares its stem requires no `dist.toml` edit.
- [ ] **AC9** `dist` fails with a message naming the missing file when a recipe's `.map` source does
      not exist, and fails naming the mod root when that root holds neither `start-script.ts` nor
      `start-script.luau`, or holds both.
- [ ] **AC10** `dist` fails at manifest parse time, naming the offending entry, on a duplicate
      `output`, a `name` that is not a single normal path component, a `mod_root` that is not exactly
      two path components, or an `args` token of the fused form `--lightmap-density=<v>`. It fails at
      resolution, naming the entry, on a recipe whose `output` matches no scanned literal, and on an
      empty resolved set.
- [ ] **AC11** A second `dist` run over an existing `dist/<name>/` produces a payload with no file
      left over from the previous run.
- [ ] **AC12** `dist` prints, per stage, what it built; prints stage 6's bake order finest lightmap
      density first with ties broken lexicographically by output path; and closes with the payload's
      total size and file count.
- [ ] **AC13** `cargo run -p xtask -- --help` lists `dist` with its flags.
- [ ] **AC14** `docs/distribution.md` takes a reader from a fresh Windows checkout to a shareable
      folder, naming the toolchain prerequisites and the SmartScreen prompt their recipient will see.
- [ ] **AC15** Launching each catalog level renders world surfaces textured and the player viewmodel
      textured, not as placeholders. Every file under `<payload>/baked/materials/` is a
      `<hex>.prm` — no `*.tmp.*` partials.
- [ ] **AC16** A run that fails before stage 5 deletes the payload root leaves the previous payload
      byte-for-byte untouched. Any payload directory containing `.dist-incomplete` was not produced
      by a completed run, and a completed run leaves none.
- [ ] **AC17** `dist` refuses, before deleting anything, when the resolved payload root is, contains,
      or equals the workspace root, the mod root, or `content/`.
- [ ] **AC18** Every row of **Pinned behaviors** holds.

## Tasks

### Task 1: `dist` command vertical slice

Add a `dist` module to `xtask` as a directory module — `crates/xtask/src/dist/mod.rs` (stage driver),
`dist/manifest.rs` (parse), `dist/resolve.rs` (scan and recipe resolution), `dist/launcher.rs`
(`emit_launcher`) — dispatched from `try_main` as `dist::run(args.collect())` following the
`crate_graph::run` precedent, and add a USAGE line carrying `[--manifest <path>] [--out <dir>]` plus
a COMMANDS entry to `print_help`. Splitting the module by file up front is what lets Phase 2 run
concurrently: each later task owns one file.

Define and parse the `dist.toml` manifest, and author the dev mod's instance of it at the workspace
root in this task — the schema and its first real instance are one unit, and stage 3 hard-fails
without it, so the slice cannot be verified otherwise. The shape:

```toml
[package]
name = "postretro-dev"
mod_root = "content/dev"

[[recipes]]
output = "maps/campaign-test--0.02-mtex.prl"
source = "content/dev/maps/campaign-test.map"
args = ["--lightmap-density", "0.02"]
```

`name` is the payload folder name and must be a single normal path component — reject separators,
`.` and `..`. `mod_root` is a workspace-relative path that must be exactly two path components.
Each `[[recipes]]` entry is keyed by `output`, a path relative to the mod root carrying its `maps/`
prefix so stage 3 can compare it verbatim against the scanned literals; `source` (workspace-relative
`.map`) and `args` (extra `prl-build` flags) are both optional. All manifest paths use `/` on every
host and are compared verbatim — a manifest authored with `\` would never match a scanned literal.
`args` is an array of single tokens, never a shell string: stage 6 reads the lightmap density by
finding the token `--lightmap-density` and taking the next one, so the fused `--lightmap-density=0.02`
form is a parse error rather than a silently unread value. Duplicate `output` keys are a parse error.

The `mod_root` shape is load-bearing, not stylistic: the runtime's `derive_prm_root_dev_layout`
(`crates/postretro/src/startup/worker.rs`) walks two parents from the content root to find
`baked/materials`, so a one- or three-segment mod root resolves somewhere the payload has no
materials and every world texture silently degrades to a placeholder. Recipes are overrides, not the
level set — that comes from stage 3. Support `--manifest <path>` to override the default `dist.toml`
at the workspace root, and `--out <dir>` to override the default `dist/` output root; both `--out`
and the default resolve against the workspace root, matching `mod_root`, so the command means the
same thing invoked from `crates/xtask/` as from the workspace root. Reject at parse time, naming the
offending recipe, any `args` list containing `-o`, `--release`, `--tui` or `--no-tui`: `dist` supplies
`-o`, `--release` and `--no-tui` itself, `--tui` would collide with the `--no-tui` that `prl-build`'s
`parse_args_from` treats as mutually exclusive, and a recipe re-supplying `-o` silently redirects
output away from the payload.

Also delete the `occlusion-test--shadow-resolution-test` entry from `mapCatalog` in
`content/dev/scripts/frontend-menu.ts` as part of this task. `prl-build` exposes no shadow-resolution
flag and no `.map` carries that name, so no invocation of the current compiler reproduces that
artifact; with the level set derived from the catalog, a menu entry naming an unbuildable level would
fail stage 3 on every run. It sits in the tag-`variant` "Bake Variants" section, which drops from 5
entries to 4 and is not emptied. That leaves a 12-entry catalog: 8 resolved by the stem default, 4 by
recipe.

Then run seven stages in order, printing a header and what it produced per stage, and failing fast
with a message naming the failing stage and input.

**(1) Build all three binaries at `--release`** so stages 2, 5 and 6 read what this stage wrote:
`postretro` (`--bin postretro` — the crate declares three bins), `prl-build`
(`-p postretro-level-compiler`) and `scripts-build` (`-p postretro-script-compiler`). Do not pass
`--features` — `dev-tools`, `observability` and `capture` are all non-default. Locate each afterwards
by absolute path under `<target-dir>/release/`, where `<target-dir>` is `CARGO_TARGET_DIR` when set
and `<workspace>/target` otherwise, never via `PATH`. If `build_scripts_sidecar` is reused it must be
passed `--release`; called with `&[]` as the `observe` path does, it emits a debug `scripts-build`.
That matters twice over: stage 2 would read a binary this stage did not build, and stage 6 would too
— three of the eight distinct `.map` sources behind the twelve catalog entries (`campaign-test`,
`combat-demo`, `trap-pools`) carry a worldspawn `data_script` KVP, so `prl-build` calls
`find_scripts_build`, which probes beside `current_exe` first, and a debug-only sidecar sends it back
into `cargo build` mid-bake.

**(2) Emit the mod entry script to scratch** at `<target-dir>/dist-work/`, not into the payload —
stage 3 must read it, and stages 3 and 4 must be allowed to fail, before stage 5 deletes anything.
Delete and recreate the scratch directory as this stage's first act: the path is fixed rather than
keyed by package name, so without a clear, a run under a different `--manifest`, or a run following
one whose `scripts-build` was killed mid-write, can leave a bundle from another mod or a truncated
one in place. Return the absolute path written, and have stage 3 read that value rather than
discovering a file in the directory — a stage 3 that globs scratch can scan a previous run's output
and resolve a level set for the wrong mod, or a subset of the right one, with no check firing.

Branch on the mod root: `.ts` present -> run
`scripts-build --in <mod_root>/start-script.ts --out <scratch>/start-script.js`; else `.luau` present
-> copy it verbatim to `<scratch>/start-script.luau`; both present -> fail naming the mod root;
neither -> fail naming the mod root. Record which extension was emitted — stage 5 installs under that
name, and Task 2's exclusion and sweep both branch on it. The both-present failure is `dist`'s own
rule, not the runtime's: `run_mod_init` gates its both-present rejection on `has_ts_or_js_source`,
whose `.ts` probe is `#[cfg(debug_assertions)]` and literally `false` in release, so a release engine
sees `.ts` + `.luau` as no conflict and silently takes the Luau path. `dist` has no rule for picking
an entry language and must not invent one. `mod_init` probes `start-script.js` then
`start-script.luau` and reads no other loose script, so this is the whole script payload — every
per-level script is already embedded in its `.prl` by `prl-build`.

**(3) Resolve the level set.** Scan the script stage 2 returned — whichever branch ran; the
`maps/<name>.prl` literal pattern is language-agnostic — as text, and dedupe the literals. For each,
resolve a bake recipe: the matching `[[recipes]]` entry if present, else the default
`<mod_root>/maps/<stem>.map` with no extra flags. Fail here, listing every problem at once, when a
scanned level has no recipe and no `.map` at the default location, when a recipe's `source` does not
exist, when a recipe's `output` matches no scanned literal (an orphan is always a rename or deletion
the manifest missed), or when the resolved set is empty. A `--release` bake is minutes per level and
must not begin against inputs already known to be absent.

**(4) Bake model textures.** Walk `<mod_root>/models/` for every `.gltf` and `.glb` and call
`bake_model_textures_for_gltf` against `<workspace>/baked/materials`. `prl-build` bakes textures only
for `prop_mesh` map entities (`prop_mesh_model_handles`), and the dev mod declares the player rig,
the viewmodels and the enemies in TypeScript instead — only `decraniated_low_poly_retro_pixel` is a
`prop_mesh` model — so those models would otherwise reach a payload with no `.prm` and the engine
would render them as placeholders without failing. A missing or empty `models/` directory is not an
error: the stage reports zero models and succeeds.

This stage runs before the delete, not after it, because it fails on inputs alone — a truncated or
malformed glTF fails in seconds. Every stage that can fail on inputs alone is therefore on the
pre-delete side of the sequence, which is what makes stage 5's rationale below literally true rather
than approximately true.

**(5) Delete the payload root, then assemble everything except the levels and the materials.**
Delete here and not earlier: stages 1 through 4 are the ones that fail on inputs alone, so a run that
cannot succeed leaves the previous payload intact and shippable. Before removing anything,
canonicalize the nearest **existing** ancestor of the resolved payload root — `std::fs::canonicalize`
returns `ENOENT` on a path that does not exist, which is every first run and every typo'd `--out` —
and refuse by path-prefix comparison when the resolved root is, contains, or equals the canonicalized
workspace root, mod root, or `content/`: `--out content` with `name = "dev"` otherwise deletes the
committed mod. Make the delete all-or-nothing (rename the root aside, then remove the renamed tree),
so a payload whose binary is held open by a running copy on Windows fails the stage without leaving a
tree that is neither the old payload nor the new one.

Immediately after creating the payload root, write `<payload>/.dist-incomplete` listing every output
stage 3 resolved. It is a completion gate, not a failure marker: stage 6 rewrites it after each
successful bake with the outputs still outstanding, and stage 7's last act deletes it. Any exit path
other than a completed stage 7 — a failed stage, a Ctrl-C, an OOM kill — therefore leaves the marker
behind, which a marker written only on a handled failure cannot do. Without it a partial payload
launches, offers a full menu, and is indistinguishable from a complete one on disk.

Then copy the release `postretro` binary to the payload root, emit the launcher via
`launcher::emit_launcher(payload_root, package_name, mod_root)` (Task 3 owns that file thereafter),
copy `content/base/` and the mod root tree preserving their workspace-relative paths, and install the
scratch entry script last at `<payload>/<mod_root>/start-script.js` or `.luau` matching the extension
stage 2 emitted. Installing an entry script named `.js` when stage 2 emitted Luau hands Luau source
to QuickJS: `run_mod_init` dispatches strictly by filename, reading `start-script.js` through
`run_mod_init_quickjs` whenever it exists. Installing last is what keeps a developer tree's stale
gitignored `start-script.js` from overwriting the fresh one in this task standing alone; Task 2's
`.js` exclusion then makes the bundle stage the only writer of that path, and the ordering becomes
belt-and-braces. Keep the tree copy deliberately simple — Task 2 owns the exclusion rules.

**(6) Bake the levels, one at a time.** For each resolved level create the `-o` parent directory
first — `precheck_output_dir` in `crates/level-compiler/src/main.rs` writes an interactive `[Y/n]`
prompt to stderr and waits on stdin when the parent is missing, and it is not gated on `--no-tui`, so
a missing directory blocks under an inherited terminal and aborts the bake on EOF under a closed or
redirected stdin. Creating the parent first is the only way to make the stage behave the same in both
contexts. Task 2's filter excludes every file type `content/dev/maps/` contains (30 `.map`, 6 `.md`),
so that directory does not otherwise exist in the payload. Then run `prl-build` with the level's
source, `--release`, `--no-tui`, its `args`, and `-o <payload>/<mod_root>/<output>`, and rewrite
`.dist-incomplete` after each success.

Bakes run strictly sequentially: `prl-build` already parallelizes internally at a job count
`default_jobs_for` bands by core count (`cores - 2` above 8 logical cores, `cores - 1` for 2 to 8,
`1` below), so a second concurrent bake oversubscribes the machine and multiplies shadowmask atlas
peak memory. Order them by effective `--lightmap-density` ascending, finest first, breaking ties
lexicographically by output path — eight of the twelve levels sit at the default, and without a
tiebreak the printed order and `.dist-incomplete`'s contents vary run to run against identical
inputs. Ascending density is a cheap approximation of descending peak memory: atlas area scales with
the inverse square of texel size, though peak also scales with light count, so the ordering is a
heuristic for failing early, not a derivation. Read the density from the recipe's `args`, defaulting
to `lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS` (0.04). A worldspawn `_lightmap_density` KVP can
also set it and `dist` does not read one; no catalog map authors one today — the only occurrence in
`content/dev/maps/` is inside a comment in the non-catalog `switch-demo.map` — so the assumption is
stated rather than checked, and a map that authored one would bake correctly but sort wrong.

**(7) Copy `<workspace>/baked/materials/` to `<payload>/baked/materials/`**, after the last bake,
copying only `<hex>.prm` files and skipping `atomic_write`'s `*.tmp.*` partials, which an interrupted
earlier bake can leave behind. This stage exists separately because `.prm` mips are a side effect of
baking, not a pre-existing input: `baked/` is gitignored in full, so on the clean checkout the
acceptance criteria name, that directory does not exist until stage 4 creates it —
`bake_diffuse_texture` reaches `atomic_write`, which calls `create_dir_all` on the parent — and is
not complete until stage 6 has added the world materials. Copying it during assembly ships a payload
with no materials at all, or with model textures and no world textures, and the engine degrades every
missing texture to a placeholder without failing. Delete `<payload>/.dist-incomplete` as the last act
of this stage.

On any failure, exit non-zero and leave `.dist-incomplete` in place. Do not run Task 2's sweep on a
failed run: its assertions are written against a finished payload and would fail on the missing
`.prl`, masking the real error. The next run deletes the whole payload regardless.

Close by printing the payload's file count and total size. Verify the slice by launching the produced
payload from its own directory and reaching gameplay from the frontend menu, and by confirming the
payload's entry script is byte-identical to stage 2's output.

### Task 2: Payload exclusion rules and stale-artifact guards

Owns `crates/xtask/src/dist/payload.rs` and the two call sites in `dist/mod.rs` that invoke it.

Replace the tree copy in stage 5 with a filtered copy. Exclude, by extension, `.map`, `.ts`, `.md`,
`.prl`, `.js` and `.bsp`; exclude any path with a `maps/autosave/` component, and any
`.build-caches` or `.DS_Store` entry. The first three are source-only inputs a runtime never reads.
The `.prl` and `.js` exclusions are the load-bearing ones and are not housekeeping: a developer's
working tree accumulates warm-cache `.prl` bakes and debug-scan `.js` bundles under the mod root.
Stage 6 and stage 5's entry-script install both write after the tree copy, so they win at the paths
they own — but a stale `.prl` for a level that is *not* in the resolved set has no later writer, and
would ship as an extra level, breaking AC4. Excluding both from the tree copy makes the bake and
bundle stages the only writers of those extensions, which is the guarantee the Invariants table calls
"no working-tree source or level artifact reaches the payload".

Also exclude `.luau` at the mod root, but only when stage 2 took the `.ts` branch: there the emitted
`start-script.js` plus a copied `start-script.luau` form exactly the pair `run_mod_init` rejects. When
stage 2 took the `.luau` branch, that file is the payload's entry script and stage 5 installs it, so
the exclusion must not fire. Factor the exclusion decision into a pure predicate over a
workspace-relative path plus the emitted-extension flag, and unit-test it: each excluded extension, a
nested `maps/autosave/` path, both mod-root `.luau` cases, and a `.png`, `.gltf`, `.glb`, `.bin`,
`.wav`, `.json`, `.jpg`, `.txt` and `.ttf` path that must survive.

Then add a sweep over the finished payload, run once after stage 7 and only on a successful run. Its
forbidden set is NOT the copy predicate's: the payload legitimately contains the stage-2 entry script
and the stage-6 `.prl` files, so a sweep reusing the copy list could never pass. The sweep forbids
`.map`, `.ts`, `.md`, `.bsp`, `maps/autosave/` and `.DS_Store`, and additionally asserts three
positives — the payload's mod-root script set is exactly one of `{start-script.js}` or
`{start-script.luau}`, matching the branch stage 2 took; its `.prl` set is exactly the stage-3
resolved outputs, each with a creation time after stage 5's deletion; and every file under
`baked/materials/` matches `<hex>.prm`. Those assertions are what make AC7 and AC15 checkable.

### Task 3: Launcher and launch contract

Owns `crates/xtask/src/dist/launcher.rs` and nothing else — Task 1 already calls
`emit_launcher(payload_root, package_name, mod_root)` from stage 5 with a host-only implementation
sufficient to verify the slice. This task completes that file; it does not touch the stage driver, so
it runs concurrently with Task 2.

Emit a launcher beside the engine binary in the payload: `<package name>.bat` on a Windows host,
`<package name>.sh` (executable) elsewhere. The batch file sets its own directory as the working
directory (`cd /d "%~dp0"`) and runs `postretro.exe --mod <mod_root>` with the mod root taken
verbatim from the manifest; the shell script does the equivalent with `cd "$(dirname "$0")"` and runs
`./postretro` — `.` is not on `PATH`. Both matter because every content path the engine resolves is
joined against the process working directory — `ui_asset_path` hardcodes `content/base/ui`,
`SplashSource::base_path` hardcodes the splash PNG path, and the content root defaults to the
grandparent of `content/dev/maps/campaign-test.prl` when no argument is given. Passing `--mod`
explicitly keeps a payload whose mod root is not `content/dev` working, and pinning the working
directory keeps the payload working when launched from a shortcut or another directory. Do not make
the launcher pass a map argument: with no map the mod's `frontend.menuTree` drives the first screen,
which is the behavior a recipient should get. Verify by running the payload from a different working
directory via the launcher and reaching the frontend menu.

### Task 4: Resolution tests and pinned behaviors

Owns the `#[cfg(test)]` modules in `crates/xtask/src/dist/resolve.rs` and `dist/manifest.rs`. Runs
concurrently with Tasks 2 and 3; it adds test modules to files Task 1 wrote and neither of them edits.

Unit-test the two pure pieces of the resolution stage. The scanner: a bundle fixture containing two
`maps/*.prl` literals, one inside a line comment, a `maps/` string that is not a `.prl`, and the same
literal twice — the result is the deduplicated set. The resolver: an output with a matching recipe
takes the recipe's source and args; an output with none takes the stem default; an output with
neither a recipe nor a `.map` at the default location is an error, and several such outputs report
together rather than one at a time. Unit-test the manifest parser against each AC10 rejection.

Then cover every row of **Pinned behaviors** marked `unit` as a test in these modules, and record the
rows marked `manual` as a checklist in the task's completion notes — they need a real payload and a
real launch. The consumer of a resolution failure is the payload's recipient, who otherwise gets a
menu button that loads nothing and no way to tell whether the level is missing or broken.

### Task 5: Distribution guide

Write `docs/distribution.md` — human-facing, so `context_style_guide.md` does not govern it — taking a
reader from a checkout to a folder they can hand to someone. Cover: the host-builds-for-host rule and
why cross-compiling is not offered (`rquickjs-sys`, `luau0-src` and `blake3` compile native C/C++);
Windows prerequisites (Visual Studio Build Tools with the C++ workload, the MSVC Rust toolchain); the
`dist` invocation and its flags; how to edit `dist.toml` to add or drop a level; that `--release`
bakes are exact-lighting and take substantially longer than a dev bake; what `.dist-incomplete` means
if they find one; how to zip and send the folder; and what the recipient sees — the SmartScreen
"Windows protected your PC" prompt on an unsigned binary and the "More info → Run anyway" path
through it, a DX12-or-Vulkan GPU requirement, settings written to
`%APPDATA%\postretro\config\settings.toml` (`ProjectDirs::config_dir` under `directories` 6), and the
documented white flash at window creation (`context/lib/boot_sequence.md`). Link it from `README.md`.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice, falsifies the payload-layout and launch-path
assumptions before any hardening is written against them. It carries the manifest and the catalog
deletion because stage 3 hard-fails without them, so no later task can unblock its own verification.

**Phase 2 (concurrent):** Task 2, Task 3, Task 4 — split by file, not by claim. Task 2 owns
`dist/payload.rs` plus its two call sites in `dist/mod.rs`; Task 3 owns `dist/launcher.rs`; Task 4
owns the test modules in `dist/resolve.rs` and `dist/manifest.rs`. Only Task 2 edits the stage driver.

**Phase 3 (sequential):** Task 5 — documents the finished command surface.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Every payload `.prl` is a `--release` bake | Task 1 (stage 6 supplies `--release`) | A manifest `args` list passing `-o`, `--release`, `--tui` or `--no-tui`; a working-tree `.prl` at a path no bake overwrites | AC5, AC7 |
| No working-tree source file or level artifact reaches the payload | Task 2 (exclusion predicate + post-payload sweep) | Any future copy path added to stage 5; stage 7, which copies a gitignored tree wholesale and is filtered only to `<hex>.prm` | AC6, AC7, AC15 |
| Payload paths resolve cwd-relative from the payload root | Task 1 (stage 5 tree copy preserves workspace-relative paths), Task 3 (launcher pins cwd) | Flattening the tree, or relocating `content/base` — `ui_asset_path` and `SplashSource::base_path` hardcode it | AC2, AC3 |
| The payload's mod root holds exactly one entry script, under the extension stage 2 emitted | Task 1 (stage 2 branch + stage 5 install), Task 2 (branch-scoped mod-root `.luau` exclusion) | Installing under a fixed `.js` name, which hands Luau source to QuickJS; shipping a `.luau` beside an emitted `.js`, the pair `run_mod_init` rejects | AC6, AC7 |
| Payload materials cover payload content | Task 1 (stage 4 bakes every glTF under the mod's `models/`; stage 7 copies after every writer) | Copying `baked/materials` before stage 4 or 6, which ships an absent or half-written directory | AC15 |
| Payload level set equals the `maps/*.prl` literals in the emitted entry script | Task 1 (stage 3 resolves the set from that script) | A level with no recipe and no stem-matching `.map`; an orphan recipe after a `path` rename | AC4, AC8, AC10 |
| A payload on disk is either complete or marked | Task 1 (stage 5 writes `.dist-incomplete`, stage 6 rewrites it, stage 7 deletes it) | Any exit path added between stages 5 and 7 that removes the marker without completing | AC16 |

The level set is *not* invariant against a catalog path assembled at runtime rather than written as a
literal. That case ships nothing and reports nothing; it is accepted, unchecked exposure, recorded
under Foreclosures.

## Pinned behaviors

Rows the implementation must satisfy, each concrete enough to test from. `unit` rows are Task 4's;
`manual` rows need a real payload.

| # | Scenario | Ordering | Expected outcome | Kind |
|---|---|---|---|---|
| P1 | Interrupted level bake | Payload assembled (stage 5); stage 6 completes 6 of 12; process receives SIGINT or SIGKILL mid-bake-7 | `.dist-incomplete` is present and names outputs 7-12 | manual |
| P2 | Failure after the last bake | Stages 1-6 succeed; stage 7's copy fails (target read-only) | Non-zero exit and `.dist-incomplete` present | manual |
| P3 | Malformed glTF | A `.gltf` under `<mod_root>/models/` is truncated; stages 1-3 pass | Stage 4 fails; the previous payload is byte-identical to what it was before the run | manual |
| P4 | Bake fails mid-set | 12 resolved levels; `prl-build` exits non-zero on level 4 | Run stops at level 4 and does not attempt 5-12; `.dist-incomplete` lists 9 outputs — the failed one plus the 8 never attempted | manual |
| P5 | Second run, level removed | Run 1 ships 12 levels; a level is removed from `mapCatalog`; run 2 succeeds | The payload holds exactly 11 `.prl`; the removed path does not exist | manual |
| P6 | Second run over an incomplete payload | Run 1 fails at bake 4 leaving the marker; inputs fixed; run 2 succeeds | 12 `.prl` and no `.dist-incomplete` | manual |
| P7 | Re-run while the payload is running | Run 1's payload is launched and holds the binary open; run 2 reaches stage 5's delete | Stage 5 fails without leaving a partially deleted tree | manual |
| P8 | First-ever run, no `dist/` | `dist/` does not exist; stage 5 canonicalizes | The guard evaluates against the nearest existing ancestor, permits the run, and creates `dist/<name>/` | unit |
| P9 | Guard trip | `--out content` with `mod_root = "content/dev"` | Non-zero exit before any filesystem mutation; `content/` is byte-identical afterwards | unit |
| P10 | Empty scan | The emitted entry script contains zero `maps/*.prl` literals | Stage 3 fails naming the scanned script; stage 5 has not run, so a previous payload is untouched | unit |
| P11 | Empty models directory | `<mod_root>/models/` absent, or present with zero glTF | Stage 4 reports 0 models and succeeds; it is not skipped in a way that also skips its fail-fast role | unit |
| P12 | Nothing to bake into materials | Stages 4 and 6 write no `.prm` | Stage 7 creates an empty `<payload>/baked/materials/` and succeeds | unit |
| P13 | Stale working-tree `.prl` | `content/dev/maps/campaign-test.prl` exists as a warm-cache dev bake predating the run | The payload's copy was created after stage 5's deletion and differs from the working-tree bytes | manual |
| P14 | Stale working-tree `start-script.js` | `content/dev/start-script.js` exists from an earlier debug launch, differing from the fresh bundle | The payload's entry script is byte-identical to this run's scratch output | manual |
| P15 | Stale release binary | `<target-dir>/release/postretro` exists from an older build | The payload's binary has an mtime at or after stage 1's completion | manual |
| P16 | Debug sidecar reachable | `<target-dir>/debug/scripts-build` exists, release does not | Stage 1 builds the release sidecar; stage 2 invokes the release path | manual |
| P17 | Cross-run `.prm` leftovers | `<workspace>/baked/materials/` holds `<hex>.prm.tmp.48291` from an OOM-killed earlier bake | No `*.tmp.*` file reaches the payload; `<hex>.prm` extras do, and are accepted | unit |
| P18 | Luau-only mod | Mod root holds `start-script.luau`, no `.ts`, no `.js` | The payload holds `<mod_root>/start-script.luau` and no `start-script.js`, and reaches the frontend menu | manual |
| P19 | `.ts` and `.luau` both present | Both at the mod root | Stage 2 fails naming the mod root, before stage 5 deletes | unit |
| P20 | Deterministic bake order | 8 levels at the default density; run twice | Stage 6's printed order is identical across runs, and a forced failure on the same level produces an identical `.dist-incomplete` | unit |
| P21 | Fused density flag | A recipe carries `args = ["--lightmap-density=0.02"]` | Manifest parse fails naming the recipe | unit |

## Open questions

- **`--release` peak memory on the four `-mtex` variants.** Nobody has baked
  `campaign-test--0.02-mtex` or the three `occlusion-test--*-mtex` levels at `--release`.
  `drafts/lighting-scale--cold-bake-reaching-light-spike/out-of-scope-findings.md` records a
  confirmed SIGKILL at 16 GB in the shadowmask atlas stage on a 157-light map at
  `--lightmap-density 0.25`, with `1.0` completing. These variants bake at `0.01`-`0.02` — finer than
  both the `0.04` default and the config that died — on maps carrying far fewer lights
  (`campaign-test` 31, `occlusion-test` 9). The two factors pull in opposite directions and the
  outcome is unmeasured, so AC2 may be unachievable on a given machine. Stage 6's bake ordering is a
  fail-early heuristic, not a mitigation. If one OOMs: drop the variant from `mapCatalog`, or coarsen
  its density in `dist.toml`. **Owner: project owner**, before Task 1 is accepted.

## Rough sketch

New directory module `crates/xtask/src/dist/`, entered from `try_main` as `dist::run(args.collect())`
alongside the `crate_graph` arm; `main.rs` gains a dispatch arm and help lines only.

Reuse the existing xtask helpers: `workspace_root()` and `run_checked(&mut Command, label)` for every
subprocess. Resolve `cargo` the same way the `run` path does — `std::env::var_os("CARGO")` with a
`"cargo"` fallback. `build_scripts_sidecar` is reusable only if passed `--release`; its `observe`
call site passes `&[]`, which builds debug.

Locate the built binaries under `<target-dir>/release/` by name, appending `.exe` under
`cfg!(windows)` — the same shape as `scripts_build_in_dir` in `crates/scripting-core/src/watcher.rs`.
`<target-dir>` honors `CARGO_TARGET_DIR`; both in-repo analogues (`scripts_build_in_dir`,
`scripts_build_beside`) resolve relative to `current_exe` instead, so neither is a drop-in.

`.prm` output is not addressable by flag: `resolve_prm_root_via_cargo` in
`crates/level-compiler/src/main.rs` walks for a `Cargo.toml` ancestor of the map source and lands on
`<workspace>/baked/materials`. Since `dist` bakes from workspace sources the mips land there, which
is why the copy is stage 7 — the directory is gitignored, stage 4 creates it and stage 6 finishes it.
The runtime side of that path is `derive_prm_root_dev_layout` in
`crates/postretro/src/startup/worker.rs`, which resolves the content root's grandparent plus
`baked/materials` — so the payload's `<root>/baked/materials` is what a payload rooted at
`<root>/content/<mod>` reads.

Model-texture baking is in-process: `bake_model_textures_for_gltf(gltf_path, prm_root)` already
lives in `crates/xtask/src/main.rs` behind the `bake-model-textures` command. `dist` calls it
directly rather than shelling out.

`toml` and `serde` are workspace dependencies; `xtask` has neither in its own manifest today and
needs both, `serde` with the derive feature.
