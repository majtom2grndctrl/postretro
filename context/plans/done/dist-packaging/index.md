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
- A payload-root guard: `--out` may only land the payload inside `dist/`, and a directory no `dist`
  run produced is never deleted. Evaluated at parse time and again at stage 5.
- Payload assembly that excludes source-only and stale-generated files from the working tree.
- A `.dist-incomplete` completion gate whose absence means complete-and-swept, written before any
  other content enters the payload root and removed only after the sweep passes.
- A launcher script that pins the working directory and the mod root.
- Trimming `mapCatalog` in `content/dev/scripts/frontend-menu.ts` to the levels the dev mod ships,
  and dropping the one menu section that trim empties.
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
  opens a material by key — `load_model_diffuse_texture` joins `cache_filename_for_key`'s output
  (defined in `crates/level-format/src/prm.rs`) onto the cache root and never enumerates the
  directory. The key itself comes from `filename_key_for` over whichever source PNG slots are
  present, so extras cost payload bytes only. Stage 7
  still excludes `atomic_write`'s `*.tmp.*` partials: they are equally inert by that argument, but
  AC15 asserts every file under the payload's `baked/materials/` is a `<hex>.prm`, and they are dead
  payload bytes.
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

*Derive the level set from `content/<mod>/maps/*.map`.* Falsified by the content: `content/dev/maps/`
holds 32 `.map` files against a catalog of five levels. Stress fixtures, feature demos and capture
rigs sit in that directory under the same extension, in the same place, as the sources behind the
levels the mod offers, and a listing tells them apart by nothing. Which levels a mod ships is a
catalog decision, and the filesystem holds no record of it.

*Hold the shipped level set in `dist.toml` as an explicit list, with a scan of the bundle as a
coverage check that fails on mismatch.* Rejected as a duplicate of the mod's own catalog. A scan
trusted enough to fail a build is trusted enough to drive one; inverting it removes the second list
at the cost of exactly one case the explicit list would have covered — a catalog path assembled at
runtime, recorded under Foreclosures and accepted. Measured: a real `scripts-build` bundle of `content/dev/start-script.ts`
carries every catalog path as a string literal, sourced from `defineMapCatalog`'s `path` field
rather than from per-map script imports — so a level with no data script of its own still appears.
`dist.toml` therefore holds no level list. It still keys each `[[recipes]]` entry by
`output` path, so renaming a catalog entry's `path` invalidates its recipe; stage 3 reports that as
an orphan rather than letting it pass silently. That is a narrower coupling to `path` than a second
level list, and it leaves `context/plans/done/mod-map-catalog/index.md`'s commitment intact: `id` is
the identity, `path` is incidental, and nothing in `dist.toml` claims otherwise.

*Evaluate the mod's TypeScript to read `mapCatalog` as a value.* Makes `xtask` a script host — it
would need QuickJS plus the SDK prelude — to reach a structure the compiler flags are not stored in
anyway. The textual scan gets the same set from a byte scan of the emitted bundle.

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

`[[recipes]]` ships with no live user: no level in the dev catalog needs one, so Task 5's tests over
synthetic manifests are the whole of its coverage and no run over a committed manifest reaches
resolution's recipe arm, the orphan check, or the density validation. It is kept because a level
whose source or flags cannot be inferred from its output name reaches a payload no other way, and
because a mod that adds one gets a parse-time refusal or an orphan report rather than a silent miss.
That is the price of the mechanism, paid deliberately.

Two concurrent `dist` runs sharing an output root, or sharing a manifest, are unsupported rather than
prevented: nothing locks either resource, and the two shared resources fail differently. Two runs
that resolve the same manifest path share a scratch key, so the second's stage 2 clears the first's
scratch and the first can then install a truncated or foreign entry script that the sweep accepts,
because the sweep compares script names and not bytes. Two runs sharing an output root collide
instead on the `<out>/.<name>.deleting-*` and `<out>/.<name>.marker-*.tmp` namespaces, which are
keyed by `[package] name` and `<out>` and by nothing else: the second run's stage 5 sweep unlinks
the first's in-flight marker temp, and its rename takes the first's payload root out from under it
mid-assembly. Stage 2's manifest-path key removes the first collision for two manifests that happen
to share a `name`; it does not touch the second, which is why the two output roots have to be
disjoint as well. Differing is not disjoint now that AC17 confines every one of them to `dist/`:
`--out dist/postretro-dev` differs from `--out dist` and names the default run's own payload root,
so that run's stage 5 rename and removal take the second run's payload with them. Core
saturation during stage 6 makes deliberate concurrency unattractive, but an interrupted run whose
child survived, or a second mod built from another terminal, reaches the same state. Accepted, not
mitigated; a lockfile on the output root would close it.

**One-way doors.** The command writes to three gitignored trees: `dist/`, `target/dist-work/`, and
`<workspace>/baked/materials/`, which stages 4 and 6 add to. That third one is additive and
content-addressed by `blake3(baseColor PNG bytes)`, so a failed run leaves keyed entries behind and
nothing reverts them; accepted, because a later run reuses them by key.

Everything else this spec adds outside those trees is new committed files — `dist.toml`,
`docs/distribution.md`, and a `README.md` link — reverted by deletion. The one edit to *existing*
committed content is Task 1's trim of the dev mod's `mapCatalog`: the entries it drops and the one
`section(...)` call their tag fed, object literals and a single line, trivially restorable from git,
but it does mean "delete the module and `dist.toml`" is not by itself a full revert.

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
      entry script this run emitted, and the payload's `.prl` set is exactly stage 3's resolved
      outputs. That no payload `.prl` predates this run follows from two mechanisms together, not
      from a timestamp check: stage 5's payload-root delete removes the previous run's bakes, and the
      copy predicate's `.prl` exclusion keeps the working tree's out. `Metadata::created()` is
      unsupported on overlayfs and other common build filesystems.
- [ ] **AC8** `dist` fails before baking, listing every unresolved output, when a scanned level has
      neither a `dist.toml` recipe nor a `.map` at the default location. Adding a catalog level whose
      `.map` shares its stem requires no `dist.toml` edit.
- [ ] **AC9** `dist` fails with a message naming the missing file when a recipe's `.map` source does
      not exist, and fails naming the mod root when that root holds neither `start-script.ts` nor
      `start-script.luau`, or holds both.
- [ ] **AC10** `dist` fails at manifest parse time, naming the offending entry, on a duplicate
      `output`; a `name` that is not a single normal path component; a `mod_root` that is not exactly
      two path components or whose first component is `dist`; and any malformed density in an
      `args` list — the fused `--lightmap-density=<v>` form, `--lightmap-density` as the final
      token, a following token that does not parse as `f32`, or more than one occurrence in one
      list. It fails at resolution,
      naming the entry, on a recipe whose `output` matches no scanned literal, and on an empty
      resolved set.
- [ ] **AC11** A second `dist` run over an existing `dist/<name>/` produces a payload with no file
      left over from the previous run.
- [ ] **AC12** `dist` prints a header per stage naming what it produced; prints the order stage 6
      will bake in — ascending effective lightmap density, ties broken lexicographically by output
      path — before its first bake; and closes with the payload's total size and file count.
- [ ] **AC13** `cargo run -p xtask -- --help` lists `dist` with its flags.
- [ ] **AC14** `docs/distribution.md` takes a reader from a fresh Windows checkout to a shareable
      folder, naming the toolchain prerequisites and the SmartScreen prompt their recipient will see.
- [ ] **AC15** Launching each catalog level renders world surfaces textured and the player viewmodel
      textured, not as placeholders. Every file under `<payload>/baked/materials/` satisfies
      `is_prm_filename`.
- [ ] **AC16** A run that fails before stage 5 deletes the payload root leaves the previous payload
      byte-for-byte untouched. The marker is a one-sided gate: a payload directory holding any
      content and **no** `.dist-incomplete` was produced by a completed, swept run; a directory
      holding one is not known complete. Stage 5 writes the marker before any other content enters
      the root, so the only uncovered window is a kill between creating the root and that write,
      which leaves an empty directory. A payload root restored by stage 5's rename-back after a
      failed removal is partial and carries the marker, so it does not read as complete.
- [ ] **AC17** `guard_payload_root` runs before the first build and again before the delete, and
      permits a payload root only when it lies strictly under `<workspace>/dist/`. `--out <dir>`
      resolves against the workspace root, so the default `dist/<package name>` is permitted, as is
      `--out dist/nightly` with `name = "postretro-dev"`; `--out target/ship`, `--out /tmp/ship`,
      `--out content` with `name = "dev"`, `--out context` with `name = "plans"`, and `--out .` with
      `name = "dist"` — the whole `dist/` tree rather than a payload root under it — are all refused.
      `dist/` is gitignored, so no committed input lives there, and stage 1 refuses a `target_dir`
      resolved at or under it, before it builds, that being the one build output that would
      otherwise land there. A directory added to the repository later therefore needs no guard
      edit. Every comparison is component-wise over
      canonicalized paths, never a string prefix, so a sibling named `dist-old` is not read as
      being under `dist`,
      and a path that spells `dist` but resolves elsewhere — `--out dist/link` where `dist/link` is a
      symlink to `content/dev` — is refused. It also refuses a payload root that exists and is not a
      directory in its own right: stage 5's rename-aside, remove, rename-back and
      write-marker-into-it sequence is a directory sequence throughout, so this arm reads the payload
      root's own metadata rather than following it, and a symlink is refused whatever it resolves to.
      Rename moves such a link instead of a tree, and the marker write that follows a failed removal
      lands through it in a directory stage 5 never chose. It further refuses a payload root that already exists, is
      non-empty, and holds at its top level neither `.dist-incomplete` nor the engine binary — the
      engine binary being `postretro`, or `postretro.exe` under `cfg!(windows)`, the name stage 5
      copies it under, probed at the top level only and never recursively — so a mistyped `--out`
      under `dist/` cannot delete a directory no `dist` run produced. That provenance arm permits
      four states: a payload root that does not exist; one that exists and is empty, which is what a
      kill between the root's creation and the marker's write leaves behind; one holding
      `.dist-incomplete`, which is a run stopped mid-payload; and one holding the engine binary,
      which is a completed swept run whose epilogue removed the marker. A refusal names the
      containment rule.
- [ ] **AC18** Every row of **Pinned behaviors** holds. Each row is executed by the task its `Kind`
      column assigns it to.

## Tasks

### Task 1: Manifest schema, dev manifest, and catalog cleanup

Owns `crates/xtask/Cargo.toml`, `crates/xtask/src/dist/manifest.rs`, the committed
workspace-root `dist.toml`, and one deletion in `content/dev/scripts/frontend-menu.ts`. It also
creates `crates/xtask/src/dist/mod.rs` holding nothing but `pub(crate) mod manifest;`, and adds
`mod dist;` to `crates/xtask/src/main.rs` beside the existing `mod crate_graph;`, so the parser
compiles and is testable in the phase that writes it; Task 2 fills `dist/mod.rs` with the stage
driver and adds the dispatch arm. Task 2 consumes the parsed manifest and adds nothing to
`manifest.rs`.

Define and parse `dist.toml`:

```toml
[package]
name = "postretro-dev"
mod_root = "content/dev"

[[recipes]]
output = "maps/<output stem>.prl"
source = "content/<mod>/maps/<source stem>.map"
args = ["--lightmap-density", "0.02"]
```

`name` is the payload folder name and must be a single normal path component — reject separators, `.`
and `..`. `mod_root` is a workspace-relative path that must be exactly two path components; the
runtime's `.prm` root derivation depends on it and any other shape silently degrades every world
texture to a placeholder (see research.md). Its first component may not be `dist`: AC17 confines
every payload root to `<workspace>/dist/`, so a mod root inside that tree would sit where the delete
lands. Each `[[recipes]]` entry is keyed by `output`, a path relative to the mod root carrying its
`maps/` prefix so stage 3 can compare it verbatim against scanned literals; `source`
(workspace-relative `.map`) and `args` are both optional. All manifest
paths use `/` on every host and are compared verbatim. `args` is an array of single tokens, never a
shell string.

Reject at parse time, naming the offending recipe: a duplicate `output`; any `args` containing `-o`,
`--release`, `--tui` or `--no-tui`, which `dist` supplies itself or which would collide with
`--no-tui`; and every malformed density AC10 enumerates. Stage 6 reads the density by finding the
token and taking the next one, so each of those forms would otherwise be silently unread and would
change `bake_order`'s result.

Author the dev mod's `dist.toml` at the workspace root carrying the `[package]` table alone:
`name = "postretro-dev"`, `mod_root = "content/dev"`. Every level the trimmed catalog ships has a
`.map` of its own stem under `content/dev/maps/`, so stage 3's default resolves all of them and the
manifest needs no `[[recipes]]` entry. The block above is the schema, not content to copy in.

Trim `mapCatalog` in `content/dev/scripts/frontend-menu.ts`. Delete the entries
`occlusion-test-shadow-resolution`, whose `occlusion-test--shadow-resolution-test.prl` no invocation
of the current compiler produces (see research.md), and `campaign-test-mtex-002`,
`occlusion-test-mtex-001`, `occlusion-test-mtex-0015` and `occlusion-test-mtex-002`, which bake
`campaign-test.map` and `occlusion-test.map` at a finer `--lightmap-density` than the default while
both sources ship at that default already. The current curated set is `campaign-test`,
`kinematic-platform`, `movement-feel`, `stress-warren-hallway-inspection`, and `combat-demo` — five
levels, every one resolved by stage 3's stem default. The frontend groups the recommended entries
and development-test entries without empty sections.

Add `toml`, `serde` and `blake3` to `crates/xtask/Cargo.toml` as workspace dependencies —
the workspace entry for `serde` already enables `derive`.

### Task 2: `dist` command vertical slice

Owns `crates/xtask/src/dist/mod.rs` (stage driver), `dist/resolve.rs` (scan, recipe resolution, and
the helpers below) and `dist/launcher.rs` (a host-only `emit_launcher` sufficient to verify the
slice; Task 4 completes it). Dispatch from `try_main` as `dist::run(args.collect())` following the
`crate_graph::run` precedent, and add a USAGE line carrying `[--manifest <path>] [--out <dir>]` plus
a COMMANDS entry to `print_help`. `dist` reaches `bake_model_textures_for_gltf`, `workspace_root` and
`run_checked` as private crate-root items from a child module, the way `crate_graph` already reaches
`workspace_root` — no visibility change to `main.rs`.

Factor these into standalone functions in `dist/resolve.rs` so Task 5 can test them without touching
the stage driver. All but `guard_payload_root` are pure; that one canonicalizes and stats, so its
tests build a real tree under `std::env::temp_dir()` keyed by pid and a unique suffix, the pattern
`crates/xtask/src/main.rs`'s existing tests already use — `xtask` declares no dev-dependencies and
Task 5 does not own its `Cargo.toml`:

- `guard_payload_root(payload_root, workspace) -> Result<(), GuardRefusal>` — implements AC17,
  which states the whole permit-and-refuse rule. `GuardRefusal` is an enum beside it in
  `dist/resolve.rs` with one variant per arm — `NotUnderDist`, `NotADirectory`, `NoProvenance` —
  each carrying the offending path, and its `Display` names the containment rule as AC17 requires.
  The arms are evaluated in that order and the first to refuse is the one reported, so a payload
  root that trips more than one has a single defined verdict and the provenance arm never has to
  list the top level of something that is not a directory. The variant is the observable the
  **Pinned behaviors** guard rows assert on. It canonicalizes every path it compares, each by the
  nearest-existing-ancestor rule below, and refuses on `NotUnderDist` rather than permitting when any
  canonicalization fails — containment is what a failed canonicalization leaves unproven, and that
  variant's `Display` already names the containment rule. It takes no output root: that is the
  payload root's parent, which it derives.
- `entry_script_choice(ts_present, luau_present)` — the stage 2 branch.
- `is_prm_filename(name)` — exactly 64 lowercase ASCII hex characters followed by `.prm`, and nothing
  else, matching the `<hex>.prm` names the compiler's texture-mip cache writes, whose stem is
`cache_filename_for_key`'s output and whose extension its callers append. It rejects uppercase hex, a short stem, and
  `atomic_write`'s `<hex>.prm.tmp.<pid>` partials.
- `bake_order(&[Resolved])` — effective `--lightmap-density` ascending, ties broken lexicographically
  by output path.
- `outstanding_outputs(ordered, completed)` — takes `bake_order`'s ordered slice, never stage 3's set,
  so every write of the marker carries one order.

`--manifest <path>` overrides the default `dist.toml` at the workspace root and resolves against the
invoking directory, as CLI paths conventionally do. `--out <dir>` overrides the default `dist/`
output root and resolves against the workspace root, matching `mod_root`, so the command means the
same thing from `crates/xtask/` as from the workspace root. AC17 confines the payload root that
resolution produces to `<workspace>/dist/`, so a legal `--out` names `dist` itself or a directory
under it, and every other value fails the guard before stage 1.

Evaluate `guard_payload_root` immediately after parsing the manifest and CLI, before stage 1 — every
input it needs is available that early, and a refusal after a full release build costs the user
minutes for nothing — and evaluate it again at stage 5, because the guard also reads the filesystem
and the tree it compared can have changed under a multi-minute build. Its
verdict covers the payload root's parent as well as the payload root itself — that parent is the
output root, where stage 5 creates and removes the `.<name>.deleting-*` and `.<name>.marker-*.tmp`
siblings, and containment puts it at or under `<workspace>/dist/`.

Then run seven stages in order, printing a header naming what each produced, and failing fast with a
message naming the failing stage and input.

**(1) Build all three binaries at `--release`** so stages 2, 5 and 6 read what this stage wrote:
`postretro` (`--bin postretro` — the crate declares three bins), `prl-build`
(`-p postretro-level-compiler`) and `scripts-build` (`-p postretro-script-compiler`). Do not pass
`--features` — `dev-tools`, `observability` and `capture` are all non-default. Locate each afterwards
by absolute path under `<target-dir>/release/`, never via `PATH`. `target_dir` is the
`target_directory` field of `cargo metadata --format-version 1 --no-deps`, read once by the stage
driver and threaded through every stage that writes or reads that tree. `crate_graph::load_graph`
already shells that command from the workspace root with `CARGO` resolved from the environment,
though it is private to `crate_graph` and its `Graph` keeps only the package list, so the driver
makes its own call rather than reusing it; the field is absolute and honours `CARGO_TARGET_DIR` and a
`build.target-dir` in `.cargo/config.toml` alike, which a read of `CARGO_TARGET_DIR` alone does not.
A `--target-dir` on the outer `cargo run` reaches neither: `cargo metadata` rejects that flag, and
cargo exports no `CARGO_TARGET_DIR` to the process it launches. This stage's `cargo build` inherits
the same environment and is blind to it in the same way, so what this stage writes is what stages 2,
5 and 6 read. Resolve `target_dir` and refuse it before running `cargo build`, not after. Fail this
stage, naming the resolved `target_dir`, when it lies at or under `<workspace>/dist/`: AC17 confines
every payload root to that tree, so a build tree redirected into it can share a path with the
payload root stage 5 deletes. The sharp case is `<target-dir>/release`, whose top level holds the
engine binary, so the provenance arm reads it as a payload root a `dist` run produced and permits
the delete that takes the binaries stages 5 and 6 read. Refusing the whole tree keeps that off the
values `--out` and `name` happen to carry. Equality is refused too, because a `target_dir` of
`<workspace>/dist` puts that same `release/` directory at `dist/release`, which a manifest naming
`release` reaches. Compare the two paths the way stage 5 compares the payload root: canonicalize the
nearest existing ancestor of each, rejoin the remaining components, and compare component-wise. A
`target_dir` refused before the build need not exist yet, so canonicalizing it directly fails on
exactly the configuration this refuses, and `cargo metadata`'s `target_directory` and
`workspace_root`'s `env!`-derived root need not agree on whether either side is symlink-resolved.
Refusing after the build rather than before would leave cargo's output inside
`dist/` — content at a payload root with no marker, which AC16 reads as a completed run and which
the provenance arm, probing the top level only, then refuses on every later
run. Write the lookup here rather than reusing `scripts_build_in_dir` or
`scripts_build_beside`: both are private to their own crates. If `build_scripts_sidecar` is reused it
must be passed `--release`; its `observe` call site passes
`&[]`, which builds debug, and stage 2 would then read a binary this stage did not build. Stage 6
depends on it too, because catalog source maps may carry a worldspawn `data_script` and send
`prl-build` through `find_scripts_build` — though see research.md
for the residual that a release build does not close.

**(2) Emit the mod entry script to scratch** at `<target-dir>/dist-work/<key>/`, where `<key>` is the first 16 lowercase hex characters
of the `blake3` hash of the canonicalized manifest path's bytes, and delete and recreate that directory as this stage's first act so a leftover
bundle from an earlier run of the same manifest is never scanned. Key on the manifest path, not on
`[package] name`: two manifests may carry the same `name`, and stage 2's first act would then destroy
a concurrent run's emitted script. Return the absolute path written; stage 3 reads that value rather
than discovering a file.

Branch on the mod root via `entry_script_choice`: both `start-script.ts` and `start-script.luau`
present -> fail naming the mod root; neither present -> fail naming the mod root; `.ts` only -> run
`scripts-build --in <mod_root>/start-script.ts --out <scratch>/start-script.js`; `.luau` only -> copy
it verbatim to `<scratch>/start-script.luau`. Record which extension was emitted as an `EntryExt` — a
two-variant enum, `Js` and `Luau`, defined in `dist/resolve.rs` beside `entry_script_choice`, whose
return type is `Result<EntryExt, _>`, so both rejections are the function's own; the mod
root the failure names comes from the caller, since the two booleans carry no path — and carry it,
alongside stage 3's resolved output
list, on the run-state struct the stage driver threads from stage 2 through the end of the run.
Task 3's sweep reads that struct; Task 2 builds and populates it but has no consumer for it yet. The both-present failure is `dist`'s own rule, not the runtime's —
`run_mod_init` does not reject that pair in release; see research.md. `mod_init` probes
`start-script.js` then `start-script.luau` and reads no other loose script, so this is the whole
script payload: every per-level script is already embedded in its `.prl`.

**(3) Resolve the level set.** Scan the script stage 2 returned — whichever branch ran; the pattern is
language-agnostic — as text, taking every occurrence of the literal `maps/` followed by a non-empty run of
`[A-Za-z0-9._-]` bytes and keeping the shortest prefix of that run ending in `.prl`, discarding
occurrences with no such prefix; collect into a `BTreeSet<String>`. The scan is hand-rolled over
bytes: `regex` is not a workspace dependency and this pattern does not earn one. The scan is
deliberately textual: the scanner strips nothing, so on the `.luau` branch — a verbatim copy — a
commented-out catalog path is in the set and is indistinguishable from a live one at this layer. On
the `.ts` branch it is not: `scripts-build` emits through an `Emitter` constructed with
`comments: None` (`crates/script-compiler/src/lib.rs`), so the bundle carries no comments and a
commented-out path reaches nothing. For each, resolve a recipe: the matching `[[recipes]]` entry if
present, else
`<mod_root>/maps/<stem>.map` with no extra flags. Fail here, listing every problem at once, when a
scanned level has no recipe and no `.map` at the default location, when a recipe's `source` does not
exist, when a recipe's `output` matches no scanned literal, or when the resolved set is empty.

**(4) Bake model textures.** Walk `<mod_root>/models/` for every `.gltf` and `.glb` and call
`bake_model_textures_for_gltf` against `<workspace>/baked/materials`. `prl-build` bakes *model*
textures only for `prop_mesh` map entities — `bake_model_textures` iterates
`prop_mesh_model_handles`, which skips every other classname — and the dev mod declares its rigs,
viewmodels and enemies in TypeScript
instead, so those models would otherwise reach a payload with no `.prm` and render as placeholders
without failing. A missing or empty `models/` directory is not an error: report zero models and
succeed, creating nothing. Report, rather than fail on, a glTF that yields zero base-color paths:
`resolve_document_base_color_paths` resolves only filesystem-relative URIs, so a `.glb` with
buffer-view images bakes nothing and would otherwise reach the payload as a silent placeholder.

Stages 1 through 4 write nothing into the payload root — that, not "they fail on inputs alone," is
what lets stage 5 delete safely; stage 1 writes `<target-dir>/release/`, stage 2 rewrites scratch, and
stage 4 adds keyed entries to `<workspace>/baked/materials/` that no failure reverts.

**(5) Delete the payload root, then assemble everything except the levels and the materials.** Re-run
`guard_payload_root` first. Canonicalize the nearest **existing** ancestor of each compared path
(`canonicalize` returns `ENOENT` on any path that does not yet exist, which is every first run),
rejoin the remaining components onto it, and compare the rejoined forms — the payload root and
`<workspace>/dist` alike. `workspace_root` derives the root from the compile-time
`CARGO_MANIFEST_DIR` — an `env!`, not a runtime read — so whether that value is symlink-resolved is
not something this spec can rely on, while the payload root is joined onto that root from the CLI or,
for an absolute `--out`, taken as the caller typed it. Comparing a resolved form against an
unresolved one makes the containment test miss in both directions: it reads a payload root reached
through a symlink under `dist/` as still inside `dist/`, and it reads a checkout reached through a
symlink as sitting outside its own `dist/`.

Then delete. Create `<out>` with `create_dir_all` first, so the sweep that follows has a directory to enumerate on a first-ever run. Remove every `<out>/.<name>.deleting-*` and every `<out>/.<name>.marker-*.tmp` sibling unconditionally. If the payload root does not exist, there is nothing to delete — skip the rename and the removal and go straight to creating it, which is the first-ever run and the run following a kill that left the root already renamed aside. Otherwise rename the payload root to `<out>/.<name>.deleting-<pid>`, and remove the renamed tree. The rename
makes the disappearance of `dist/<name>/` atomic; it does not make the removal atomic. A removal
that fails — a running copy holding `postretro.exe` open on Windows is the common case — has
already unlinked whatever it walked before the failing entry, so the tree is partial. Write
`.dist-incomplete` into the aside tree first — status line `stage 5`, every resolved output as an
outstanding line, temp-then-rename like every other marker write — and only then rename the tree
back, so no interruption can leave a populated payload root without a marker: a kill before the
rename-back leaves an aside tree and no payload root, which is the state the next run's sweep
collects, and a kill after it leaves a root that already carries the marker. Then fail the stage
naming both paths. If the marker write or the rename-back itself fails, fail the stage naming the
aside path, which then holds the only copy of the partial tree. The restored tree carries the marker
because it is a partial payload, not a complete one. A removal interrupted by a kill leaves an aside
tree the next run's unconditional sweep of that namespace collects. Two `dist` runs sharing an output
root are not supported, and that is what makes the unconditional sweep safe: any aside found is by
definition abandoned. The pid in the name is a diagnostic label, not a liveness test — nothing in the
workspace can test whether a pid is live, and adding that capability buys nothing the no-concurrency
rule does not already give.

Immediately after creating the payload root, and before any other content enters it, write
`<payload>/.dist-incomplete`. Every write of the marker — this first one and each rewrite — goes to
a temp file at `<out>/.<name>.marker-<pid>.tmp` and is renamed into place, so nothing but the marker
itself ever enters the payload root ahead of it. Stage 5's opening sweep covers the
`<out>/.<name>.marker-*.tmp` namespace as well as `<out>/.<name>.deleting-*`, removing every sibling
in both unconditionally, so a temp file left by a killed marker write is collected on the next run
rather than accumulating one per kill. Its first line names the stage the run is attempting — `stage 5` at
this point, rewritten to `stage 6` immediately before the first bake — and each following line is an outstanding `output` key exactly as stage 3 resolved it:
mod-root-relative, `maps/` prefix retained, `/` separators, one per line, in `bake_order` order, with
a trailing newline. The file therefore always holds at least its status line and is never empty, so
truncation is always detectable. Write it, and every rewrite, as temp-then-rename. The marker is a
completion gate: stage 6 rewrites it once after each successful bake with `outstanding_outputs` over
`bake_order`'s ordering. The rewrite that follows the final bake is the one that writes `stage 7` and
no output lines; stage 6 never writes a `stage 6` status line with an empty output list, so the
marker distinguishes a run stopped inside stage 6 from one stopped at or after stage 7. The run
epilogue removes it. Task 2 has no epilogue and no sweep — it leaves the marker in place
unconditionally, and every payload it produces carries one by design; Task 3 adds the sweep and the
sweep-gated removal.

Then copy the release `postretro` binary to the payload root; emit the launcher via
`launcher::emit_launcher(payload_root, package_name, mod_root)` — Task 4 owns that file thereafter,
and the minimum this task needs is a script beside the binary that pins its own directory as the
working directory, runs the engine with `--mod <mod_root>` and no map argument, and carries the
executable bit on a POSIX host; copy `content/base/` and the mod root tree preserving their
workspace-relative paths; and install the scratch entry script last at
`<payload>/<mod_root>/start-script.js` or `.luau` matching the extension stage 2 emitted. Delete both
`start-script.js` and `start-script.luau` from the payload's mod root immediately before installing:
on the `.ts` branch install-last alone would suffice, but on the `.luau` branch the stale artifact
and the installed one do not share a path, and the payload would hold exactly the pair `run_mod_init`
rejects. Keep the tree copy deliberately simple — Task 3 owns the exclusion rules.

**(6) Bake the levels, one at a time**, with the subprocess's working directory set to the workspace
root the way `run_postretro` already does — `find_scripts_build`'s stale-sidecar rebuild shells
`cargo build` with no `current_dir` of its own and would otherwise inherit whatever cwd `dist` was
launched from. Create each `-o` parent directory first: `precheck_output_dir` otherwise prompts on
stdin and is not gated on `--no-tui` (see research.md). Task 3's filter excludes every file type
`content/dev/maps/` contains, so that directory does not otherwise exist in the payload. Run
`prl-build` with the level's source, `--release`, `--no-tui`, its `args`, and
`-o <payload>/<mod_root>/<output>`, and rewrite `.dist-incomplete` after each success. Bakes run
strictly sequentially — `prl-build` already parallelizes internally, so a second concurrent bake
oversubscribes the machine and multiplies shadowmask atlas peak memory. Order by `bake_order`, and print
that order under the stage header before the first bake, as AC12 requires; ascending density
approximates descending peak memory, and research.md records why that is a
fail-early heuristic and not a derivation. Every level the dev catalog ships bakes at the default
density, so the order printed there is the lexicographic one; the density term is what keeps the
order right for a mod whose manifest sets a density of its own. Read the density from the recipe's
`args`, defaulting to
`lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS` (0.04); a worldspawn `_lightmap_density` KVP can also
set it and `dist` does not read one, but no catalog map authors one today.

**(7) Copy `<workspace>/baked/materials/` to `<payload>/baked/materials/`**, after the last bake,
copying only files satisfying `is_prm_filename`. Create the payload directory unconditionally and
treat an absent source as zero files: `baked/` is gitignored in full and stages 4 and 6 are its only
writers, so nothing creates it on a clean checkout with no models and no textured surfaces. Copying
during assembly instead ships a payload with no materials, or with model textures and no world
textures, which the engine degrades to placeholders without failing.

On any failure at any stage, exit non-zero and leave `.dist-incomplete` in place. Close by printing
the payload's file count and total size. Task 3's epilogue runs before this summary, so the count
describes the payload as it ships rather than including a marker that is about to be removed. Verify the slice by launching the produced payload from its
own directory and reaching gameplay from the frontend menu, and by confirming the payload's entry
script is byte-identical to stage 2's output. That launch is an eyes-on check on a host with a
display: stage 1 passes no `--features`, so the payload binary carries neither `observability` nor
`capture` and nothing scripts it. An agent that cannot run it finishes the rest of the task and
reports the launch as owed rather than as passed.

### Task 3: Payload exclusion rules and the completion sweep

Owns `crates/xtask/src/dist/payload.rs` and every call site in `dist/mod.rs` that invokes it: the
stage-5 tree copy, stage 7's materials copy, the post-run sweep, and the run epilogue that removes
`.dist-incomplete` after a passing sweep. Task 2 leaves the marker in place unconditionally; this
task introduces the epilogue. Nothing else: `dist/resolve.rs` is not this task's, so
`is_prm_filename` and `EntryExt` are called from here and never moved here — another task is adding
test modules to that file in the same phase.

Replace stage 5's tree copy with a filtered copy. Exclude, by extension, `.map`, `.ts`, `.md`, `.prl`,
`.js` and `.bsp`; exclude any path with a `maps/autosave/` component, and any `.build-caches`,
`.gitignore`, `.gitkeep` or `.DS_Store` entry. Grade them, because they are not equally load-bearing:

- `.prl` is the load-bearing one. A developer's working tree accumulates warm-cache `.prl` bakes under
  the mod root, and while stage 6 wins at the paths it owns, a stale `.prl` for a level *not* in the
  resolved set has no later writer and would ship as an extra level, breaking AC4.
- `.map`, `.ts`, `.md` and `.bsp` are source-only inputs a runtime never reads.
- `.js` is redundancy plus byte cost. `mod_init` reads no loose script but the entry script, so a
  stale non-entry `.js` is inert; excluding it makes the bundle stage the only writer of that
  extension.
- `.build-caches` and `.DS_Store` are defensive only. `prl-build`'s stage cache resolves through
  `cache::find_workspace_root` to `<workspace>/.build-caches`, outside both copied trees.

Also exclude `.luau` under the mod root's `scripts/` — per-level Luau data scripts are source-only in
exactly the way `.ts` is, since `prl-build` embeds their compiled bytes in the `.prl` — and `.luau` at
the mod root itself when stage 2 took the `.ts` branch. The mod-root rule changes no payload outcome
on its own: stage 5 already deletes both entry-script names before installing. Keep it so
`should_exclude` is correct without depending on a stage-5 cleanup step, not because it guards a live
failure. Factor the decision into
`should_exclude(path: &Path, mod_root: &Path, emitted: EntryExt) -> bool` — "at the mod root" is not
decidable from a workspace-relative path alone, and the copy runs over two trees — and unit-test it:
each excluded extension, a nested `maps/autosave/` path, both mod-root `.luau` cases, a `scripts/*.luau` path, a
`.gitkeep`, and a `.png`, `.gltf`, `.glb`, `.bin`, `.wav`, `.json`, `.jpg`, `.txt` and `.ttf` path
that must survive.

Also factor stage 7's copy into `copy_prm_tree(src, dst)` here, and unit-test it against an absent
source directory (P12): it creates the destination, copies zero files, and succeeds.

Then add a sweep over the finished payload, run once after stage 7 and only on a successful run.
The run-state struct the driver threads from stage 2 carries the emitted extension and stage 3's
resolved output list; the sweep reads it. Its
forbidden set is NOT the copy predicate's: the payload legitimately contains the stage-2 entry script
and the stage-6 `.prl` files, so a sweep reusing the copy list could never pass. The sweep forbids
`.map`, `.ts`, `.md`, `.bsp`, `maps/autosave/` and `.DS_Store`, and asserts three positives — the
payload's mod-root script set is exactly one of `{start-script.js}` or `{start-script.luau}`, matching
the branch stage 2 took; its `.prl` set is exactly the resolved outputs; and every file under
`baked/materials/` satisfies `is_prm_filename`. A passing sweep is what authorizes the epilogue to
remove `.dist-incomplete`; a failing sweep exits non-zero with the marker still in place. The
epilogue goes between the sweep and the stage driver's closing summary, not after it, so the count
AC12 requires describes the payload as it ships.

Exercise the sweep with a full `dist` run passing this task's own `--out` and its own `--manifest`.
The `--out` is a sibling directory under `dist/`, never one at or under another run's payload root.
The `--manifest` is a copy of the workspace `dist.toml` at a different path. Task 4 runs `dist`
concurrently against the default manifest and output root: the `--out` keeps the payload roots and
the `.<name>.deleting-*` and `.<name>.marker-*.tmp` namespaces apart, and the `--manifest` keeps
stage 2's scratch key apart, since that key hashes the manifest path and stage 2's first act deletes
the directory it names.

### Task 4: Launcher and launch contract

Owns `crates/xtask/src/dist/launcher.rs` and nothing else — Task 2 already calls
`emit_launcher(payload_root, package_name, mod_root)` from stage 5 with a host-only implementation
sufficient to verify the slice. This task completes that file; it does not touch the stage driver, so
it runs concurrently with Tasks 3 and 5.

Emit `<package name>.bat` on a Windows host, `<package name>.sh` (executable) elsewhere. The batch
file sets its own directory as the working directory (`cd /d "%~dp0"`) and runs
`postretro.exe --mod <mod_root>` with the mod root taken verbatim from the manifest; the shell script
does the equivalent with `cd "$(dirname "$0")"` and runs `./postretro` — `.` is not on `PATH`. Both
matter because every content path the engine resolves is joined against the process working directory:
`ui_asset_path` hardcodes `content/base/ui`, `SplashSource::base_path` hardcodes the splash PNG, and
the content root defaults to the grandparent of `content/dev/maps/campaign-test.prl` when no argument
is given. Passing `--mod` explicitly keeps a payload whose mod root is not `content/dev` working, and
pinning the working directory keeps it working when launched from a shortcut or another directory. Do not pass a map argument: with none, the mod's `frontend.menuTree` drives the first
screen, which is what a recipient should get. Verify the emitted launcher by producing a payload
with `cargo run -p xtask -- dist`, invoking the launcher from a different working directory than the
payload root, and confirming the frontend menu appears. That last step is eyes-on: the payload binary
carries no feature that scripts an observation, so an agent without a display reports it as owed
rather than as passed.

### Task 5: Resolution, manifest and helper tests

Owns the `#[cfg(test)]` modules in `crates/xtask/src/dist/resolve.rs` and `dist/manifest.rs`. Runs
concurrently with Tasks 3 and 4; it adds test modules to files Tasks 1 and 2 wrote and neither of
them edits.

The scanner: a bundle fixture containing two `maps/*.prl` literals, one inside a line comment, a
`maps/` string that is not a `.prl`, and the same literal twice. The fixture is hand-authored, not
produced by `scripts-build`: it stands for the `.luau` branch's verbatim copy, since the `.ts`
branch's bundle carries no comments. The expected set contains **both** real literals including the
commented-out one — the scan is textual and language-agnostic by design — and not the non-`.prl`
string, deduplicated. The resolver: an output with a matching recipe takes the
recipe's source and args; an output with none takes the stem default; an output with neither a recipe
nor a `.map` at the default location is an error, and several such outputs report together.

Unit-test the manifest parser against every AC10 parse-time rejection, including AC5's forbidden-flag
list (`-o`, `--release`, `--tui`, `--no-tui`), all four malformed-density forms, and a `mod_root`
whose first component is `dist` — that covers P21 and P43. Unit-test the resolver against AC10's
orphan-recipe case, AC10's empty-set case (P10), and AC9's missing-`source` case.

Then cover every **Pinned behaviors** row tagged `unit` by testing the helper it names:
`guard_payload_root` for P8, P9, P31, P36, P40, P41 and P42 — it takes the workspace root as an
argument, so those rows run against a workspace synthesized under `std::env::temp_dir()` rather than
against the repository's own `dist/`, each tree keyed by pid and a unique suffix and removed
afterwards, the pattern `crates/xtask/src/main.rs`'s existing tests already use. `xtask` declares no
dev-dependencies, and this task owns neither `crates/xtask/Cargo.toml` nor the workspace manifest, so
these fixtures use `std` alone. Then test `entry_script_choice` for P19 and P19b; `is_prm_filename` for
P17 and P28; `bake_order` for P20; `outstanding_outputs` for P23 and P32. The consumer of these
failures is the person running `dist`, who gets every unresolved output named at once rather than a
bake that dies on the fourth level after twenty minutes.

### Task 6: Payload verification

Produces a real payload with `cargo run -p xtask -- dist` and executes every **Pinned behaviors** row
tagged `manual`. It authors throwaway fixtures and mutates the working tree to create preconditions,
reverting each afterwards; it commits no source change. Runs after Phase 3, because the rows exercise
the filtered copy, the launcher and the sweep together.

This task exists because AC2, AC3, AC15 and AC16's kill clause are otherwise delivered by nothing:
Task 2 verifies one level launched in place with the repository present, and Task 4 verifies a foreign
working directory. AC3's repository-deletion clause is the one that would falsify the absolute
`DataScriptSection.source_path` the Out of scope section accepts.

Group the rows by what each needs:

- **Plain run** (P5, P6, P11, P13, P14, P15, P16, P18, P29, P30, P33, P35, P39): a
  normal `dist` invocation,
  some preceded by planting an artifact (a warm-cache `.prl`, a stale `start-script.js`, a
  `debug/scripts-build` with no release peer, a stray `.<name>.marker-*.tmp`) or by editing
  `mapCatalog`. P11 needs a synthetic mod root covering both of its arms — `models/` removed
  entirely, and `models/` present holding zero glTF — because `content/dev/models/` holds ten glTF.
  P35 needs `dist/` removed entirely first: Tasks 2 and 4 each leave a payload at `dist/<name>/`,
  and with one present stage 5 takes the rename branch and P35's outcome cannot be observed.
  P29 needs a `.ts` under `sdk/lib` touched after
  the last `scripts-build` relink, and a `dist` launched from outside the workspace —
  `cargo run --manifest-path <workspace>/Cargo.toml -p xtask -- dist` — so the inherited cwd and the
  pinned one are different directories and the row can fail. Run it under `RUST_LOG=info`:
  `find_scripts_build` logs the rebuild it shells, and that `cargo build` carries no working
  directory of its own, so it fails outside a workspace and succeeds under the pinned one.
- **Failure injection** (P1, P2, P3, P4, P22, P23b, P24, P34, P38, P44, P45): a truncated `.gltf`; an
  unreadable `.prm` in the workspace materials tree; an unreadable mod-root subdirectory; a
  `prl-build` forced
  non-zero on the fourth level; and SIGKILL, for P1 mid-bake, for P23b inside the marker write, for
  P24 mid-remove, and for P34 between the payload root's creation and the marker's rename. P44 needs
  no window either: export `CARGO_TARGET_DIR` under `<workspace>/dist/` before the run and stage 1
  refuses. P45's window is stage 1's release build, minutes wide — run with `--out dist/x` against an
  absent payload root, plant that root and one stray file while stage 1 builds, and confirm stage 5
  refuses instead of deleting it. P3 and P4 need no window: the truncated `.gltf` and the forced-non-zero `prl-build` are
  planted before the run. P2 and P38 need none either, once the failure is a permissions change made
  before the run on something the stages ahead of the failing one never open: for P38, make
  `<mod_root>/textures/` unreadable, so stages 1-4 pass — stage 4 reaches base-color images through
  each glTF's own relative URIs and every one of them resolves under `<mod_root>/models/` — and
  stage 5 fails partway through the mod-root tree copy with the marker already written; for P2,
  plant a `<64 lowercase hex>.prm` with its read bit cleared in `<workspace>/baked/materials/`,
  which stages 4 and 6 write past by key and never read and which stage 7 selects with
  `is_prm_filename` and then cannot open. Do not instead make the payload root unwritable: every
  marker rewrite renames into that directory, so stage 6 fails at its next bake and the marker never
  reaches the `stage 7` status line P2 requires. P1's and
  P24's windows are wide enough to hit by hand. P22, P23b and P34 are not: for each, insert a
  temporary sleep at the named point — between stage 7 and the sweep, inside the marker write, and
  between the payload root's creation and the marker's rename — take the action during the pause,
  and revert the sleep afterwards. P22's forbidden file is plantable only during such a pause:
  nothing reachable from the working tree survives the copy predicate and then fails the sweep,
  because the sweep's forbidden set is a subset of the copy's exclusions.
- **Second manifest** (P26): a second `dist.toml` at a different path carrying the *same*
  `[package] name` as the first — that sameness is the whole of what P26 pins, and the differing
  manifest paths are what keep the scratch keys apart — and an `--out` that is a sibling directory
  under `dist/`, because the `.<name>.deleting-*` and `.<name>.marker-*.tmp` namespaces are keyed by
  `name` and `<out>` and by nothing else. A merely different `--out` is not enough now that AC17
  confines every output root to one tree: `--out dist/postretro-dev` differs from the default
  `--out dist` and names run A's own payload root, so run A's stage 5 renames and removes run B's
  payload out from under it and neither outcome column can be observed. Run A's window between stage 2 and stage 5 is too narrow
  to hit by hand, so hold it there with the same temporary-sleep technique, start run B during the
  pause, and revert the sleep afterwards.
- **Windows gate** (P7, P25): both rows turn on a running copy holding `postretro.exe` open, and
  `dist` builds for the host, so both are executed on a Windows machine against a payload built
  there. Push the implementation branch to the remote as it stands, adding no commit. On a Windows
  host that can build this workspace — AC14 has `docs/distribution.md` name the toolchain that host
  needs — clone or fetch that branch, check it out, and run `cargo run -p xtask -- dist`. Everything
  the two rows read is built on that host; nothing is carried across from the POSIX run. Then launch
  the payload's `<package name>.bat`, leave it sitting at the frontend menu, and run
  `cargo run -p xtask -- dist` again from the same checkout. That second run drives both rows: P7
  reads the state it leaves at `dist/<name>/` and the stage's exit, P25 reads the order of writes
  that produced it, and the `.<name>.deleting-<pid>` tree the failure message names is where the
  marker landed first. Done is both outcome columns observed on that host and reported with its
  Windows version; neither row is passed from a POSIX run, and the task is unfinished while either
  is unobserved.

Concretely for AC2, AC3 and AC15: launch every resolved level to gameplay; move the payload folder to
an unrelated directory and relaunch; rename the checkout aside and relaunch; and confirm world
surfaces and the player viewmodel render textured rather than as placeholders.

### Task 7: Distribution guide

Write `docs/distribution.md` — human-facing, so `context_style_guide.md` does not govern it — taking a
reader from a checkout to a folder they can hand to someone. Cover: the host-builds-for-host rule and
why cross-compiling is not offered (`rquickjs-sys`, `luau0-src` and `blake3` compile native C/C++);
Windows prerequisites (Visual Studio Build Tools with the C++ workload, the MSVC Rust toolchain); the
`dist` invocation and its flags, including the containment of `--out` AC17 states; how to edit
`dist.toml` to add or drop a level; that `--release` bakes are exact-lighting and take substantially
longer than a dev bake; what `.dist-incomplete` means if they find one — its presence means the
folder is not known complete and its absence over a folder holding any content means a completed,
swept run; its first line is a status line naming the
stage the run was attempting, one of `stage 5`, `stage 6` or `stage 7`; and each line after it is one
level that had not been baked yet, written as the mod-root-relative `maps/<name>.prl` path with `/`
separators, so a marker holding only its status line means every level baked and the run stopped
after that; what a `.<name>.deleting-*` directory beside the payload is, why a failed run's message names one,
and that the next run collects it; how to zip and send the folder; and what the recipient sees — the
SmartScreen "Windows protected your PC" prompt on an unsigned binary and the "More info -> Run anyway"
path through it, a DX12-or-Vulkan GPU requirement, settings written to
`%APPDATA%\postretro\config\settings.toml` (`ProjectDirs::config_dir` under `directories` 6), and the
documented white flash at window creation (`context/lib/boot_sequence.md`). Link it from `README.md`.

## Sequencing

**Phase 1 (sequential):** Task 1 — the manifest schema, the dev `dist.toml`, and the catalog trim.
Stage 3 hard-fails without all three, so nothing downstream can verify itself until they exist.

**Phase 2 (sequential):** Task 2 — the vertical slice, which falsifies the payload-layout and
launch-path assumptions before any hardening is written against them.

**Phase 3 (concurrent):** Task 3, Task 4, Task 5 — split by file, not by claim. Task 3 owns
`dist/payload.rs` plus its call sites in `dist/mod.rs`; Task 4 owns `dist/launcher.rs`; Task 5
owns the test modules in `dist/resolve.rs` and `dist/manifest.rs`. Only Task 3 edits the stage driver.
The three also split by filesystem, because two concurrent `dist` runs over one output root are
unsupported: Task 5's `guard_payload_root` rows describe path shapes and are executed against a
synthesized workspace, never against the workspace's own `dist/`, and Task 4's launcher
verification is the only end-to-end `dist` invocation this phase schedules against the default
manifest and the default output root. A run Task 3 makes to exercise the sweep passes both its own
`--out` and its own `--manifest`. The `--out` is a sibling directory under `dist/`, never one at or
under another run's payload root, which keeps the payload roots and the `.<name>.deleting-*` and
`.<name>.marker-*.tmp` namespaces apart. The `--manifest` is a copy at a different path, which keeps
stage 2's scratch key apart — that key hashes the manifest path, `--out` does not reach it, and two
runs resolving one manifest path share the directory stage 2 deletes as its first act. Both of those
runs reach stage 6, so the phase costs two full `--release` bakes of the catalog on top of two cold
worktree builds, and the two bakes contend for cores and for shadowmask atlas memory — the same
saturation that keeps bakes sequential within a run. Budget the phase for that; a machine that
cannot hold two concurrent `--release` bakes runs Task 4 after Task 3 returns, Task 5 alongside
either.

**Phase 4 (concurrent):** Task 6, Task 7 — verification against a real payload, and the guide. They
share no file and neither consumes the other's output. Task 6's Windows gate runs on a second
machine against the branch as pushed, so it contends with nothing in this phase.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Every payload `.prl` is a `--release` bake | Task 2 (stage 6 supplies `--release`), Task 3 (`.prl` exclusion) | A manifest `args` list passing `-o`, `--release`, `--tui` or `--no-tui`; a working-tree `.prl` at a path no bake overwrites | AC5, AC7 |
| No working-tree source file or level artifact reaches the payload | Task 3 (exclusion predicate + post-run sweep) | Any future copy path added to stage 5; stage 7, which copies a gitignored tree filtered only by `is_prm_filename` | AC6, AC7, AC15 |
| Payload paths resolve cwd-relative from the payload root | Task 2 (stage 5 tree copy preserves workspace-relative paths), Task 4 (launcher pins cwd) | Flattening the tree, or relocating `content/base` — `ui_asset_path` and `SplashSource::base_path` hardcode it | AC2, AC3 |
| The payload's mod root holds exactly one entry script, under the extension stage 2 emitted | Task 2 (stage 2 branch, stage 5 pre-install delete and install), Task 3 (branch-scoped mod-root `.luau` exclusion) | Installing under a fixed `.js` name, which hands Luau source to QuickJS; a stale mod-root `start-script.js` surviving the copy on the `.luau` branch | AC6, AC7 |
| Payload materials cover payload content | Task 2 (stage 4 bakes every glTF under the mod's `models/`; stage 7 copies after every writer) | Copying `baked/materials` before stage 4 or 6, which ships an absent or half-written directory | AC15 |
| Payload level set equals the `maps/*.prl` literals in the emitted entry script | Task 2 (stage 3 resolves the set from that script) | A level with no recipe and no stem-matching `.map`; an orphan recipe after a `path` rename | AC4, AC8, AC10 |
| A delete can never reach content no `dist` run produced | Task 1 (`mod_root` may not be `dist`-rooted, so no manifest puts a mod inside the tree the delete works in), Task 2 (`guard_payload_root`, evaluated after parsing and again at stage 5, its arms tried in the order Task 2 fixes: containment, then the directory check, then provenance) | Committed content added under `dist/`, or a stage taught to read an input from there; comparing paths in mixed canonicalized and un-canonicalized forms; a payload root whose provenance is read from the marker alone, which a completed run has removed | AC10, AC17 |
| A payload with content and no marker is complete and swept | Task 2 (stage 5 writes the marker before any other content, writes it into the aside tree before the rename-back restores that tree, and stage 6 rewrites it per bake), Task 3 (sweep gates the epilogue's removal) | Any exit path added after stage 5 that removes the marker without a passing sweep; any path that leaves a partial tree at the payload root without writing one | AC16 |

The level set is *not* invariant against a catalog path assembled at runtime rather than written as a
literal. That case ships nothing and reports nothing; it is accepted, unchecked exposure, recorded
under Foreclosures.

## Pinned behaviors

Rows the implementation must satisfy. `unit` rows name the helper Task 5 tests, except the
`payload.rs` helpers Task 3 tests; `manual` rows are
Task 6's, against a real payload, and the two tagged `manual (Windows)` are its Windows gate. `N` is
the resolved level count, 5 for the dev mod.

This table travels with the briefing for Tasks 3, 5 and 6, alongside the Goal, the acceptance
criteria and the Invariants. Each of those tasks is defined by the row ids it must execute, and a
briefing that names row ids without the table names work its agent cannot read.

| # | Scenario | Ordering | Expected outcome | Kind |
|---|---|---|---|---|
| P1 | Interrupted level bake | Payload assembled; stage 6 completes half the set; SIGKILL mid-bake | The marker is present, its first line is `stage 6`, and its output lines are the outstanding levels in `bake_order` order | manual |
| P2 | Failure after the last bake | Stages 1-6 succeed; stage 7's copy fails (an unreadable `.prm` in the workspace materials tree) | Non-zero exit; the marker is present with first line `stage 7` and no output lines | manual |
| P3 | Malformed glTF | A `.gltf` under `<mod_root>/models/` is truncated; stages 1-3 pass | Stage 4 fails; the previous payload is byte-identical to what it was before the run. `<workspace>/baked/materials/` may have gained keyed entries | manual |
| P4 | Bake fails mid-set | `prl-build` exits non-zero on level 4 of N | Run stops at level 4 and does not attempt the rest; the marker's first line is `stage 6` and its output lines are the failed level plus every level never attempted | manual |
| P5 | Second run, level removed | Run 1 ships N levels; a level is removed from `mapCatalog`; run 2 succeeds | The payload holds exactly N-1 `.prl`; the removed path does not exist | manual |
| P6 | Second run over an incomplete payload | Run 1 fails mid-bake leaving the marker; inputs fixed; run 2 succeeds | N `.prl` and no `.dist-incomplete` | manual |
| P7 | Re-run while the payload is running | Run 1's payload holds the binary open; run 2 reaches stage 5's delete | `dist/<name>/` afterwards holds what the partial removal left, carries `.dist-incomplete` with status line `stage 5`, and the stage exits non-zero naming both it and the aside path | manual (Windows) |
| P8 | First-ever run, default output | No `dist/`; payload root is `<workspace>/dist/<package name>`. Then the same inputs with the payload root present, non-empty and holding the engine binary, as a completed run leaves it | `guard_payload_root` rejoins the non-existent tail onto the nearest existing ancestor and permits; it permits the second state too, on the provenance arm | unit |
| P9 | Guard trip, payload root outside `dist/` | `--out context` with `name = "plans"`; `--out target/ship` with `name = "postretro-dev"`; `--out /tmp/ship` with `name = "postretro-dev"`; `--out dist-old` with `name = "ship"`, a sibling whose name shares `dist`'s leading characters; `--out baked` with `name = "materials"`, which names the tree stage 4 writes keyed entries into; and `--out .` with `name = "dist"`, which names `dist/` itself rather than a root strictly under it | `guard_payload_root` refuses each on the `NotUnderDist` arm — the absolute path, the prefix-sharing sibling AC17's component-wise comparison exists to keep out, and the `dist/` tree itself included | unit |
| P10 | Empty scan | The emitted entry script contains zero `maps/*.prl` literals | The resolver returns the empty-set error | unit |
| P11 | Empty models directory | `<mod_root>/models/` absent, or present with zero glTF | Stage 4 reports 0 models, succeeds, and creates no directory | manual |
| P12 | Absent workspace materials tree | `copy_prm_tree` is called with a source directory that does not exist | It creates the destination directory, copies zero files, and returns success | unit |
| P13 | Stale working-tree `.prl` | `content/dev/maps/campaign-test.prl` exists as a warm-cache dev bake | It is excluded from the copy, and the payload's `.prl` set is exactly stage 3's resolved outputs | manual |
| P14 | Stale working-tree `start-script.js` | `content/dev/start-script.js` exists from an earlier debug launch, differing from the fresh bundle | The payload's entry script is byte-identical to this run's scratch output | manual |
| P15 | Stale release binary | `<target-dir>/release/postretro` predates this run and cargo does not relink it | The payload's binary is byte-identical to `<target-dir>/release/postretro` as it stands after stage 1 completes | manual |
| P16 | Debug sidecar reachable | `<target-dir>/debug/scripts-build` exists, release does not | Stage 1 builds the release sidecar; stage 2 invokes the release path | manual |
| P17 | `.prm` filename discrimination | Names `<64 lowercase hex>.prm`, the same with uppercase hex, a short stem, and `<hex>.prm.tmp.48291` | `is_prm_filename` accepts only the first | unit |
| P18 | Luau-only mod | Mod root holds `start-script.luau`, no `.ts`, no `.js` | The payload holds `<mod_root>/start-script.luau` and no `start-script.js`, and reaches the frontend menu | manual |
| P19 | `.ts` and `.luau` both present | Both at the mod root | `entry_script_choice` returns an error | unit |
| P19b | Neither entry script present | Mod root holds no `start-script.ts` and no `start-script.luau` | `entry_script_choice` returns an error | unit |
| P20 | Deterministic bake order | Levels sharing the default density; `bake_order` called twice on the same input | Identical order both times, ties broken lexicographically | unit |
| P21 | Malformed density in `args` | Each of: `--lightmap-density=0.02`; `--lightmap-density` as the final token; a following token that is not an `f32`; two occurrences in one list | Manifest parse fails naming the recipe, in all four cases | unit |
| P22 | Sweep fails on a finished payload | Stages 1-7 succeed; the sweep finds a forbidden file or a `.prl`-set mismatch | Non-zero exit **and** the marker still present — the epilogue removes it only after a passing sweep | manual |
| P23 | Outstanding list contents and order | `outstanding_outputs` over `bake_order`'s ordering with a completed prefix | Returns exactly the uncompleted entries, in `bake_order` order | unit |
| P23b | Kill during the marker rewrite | Stage 6 completes a bake; SIGKILL lands inside the `.dist-incomplete` write | The marker is present and parses — a status line plus zero or more output lines, never truncated | manual |
| P24 | Kill during stage 5's delete | Run 2 renames the payload root aside, then is killed mid-remove | `dist/<name>/` does not exist; run 3 removes the orphaned aside tree, then finds no payload root to rename, skips the rename and removal, and creates the root directly | manual |
| P25 | Delete fails after a successful rename | Run 1's payload holds `postretro.exe` open; `remove_dir_all` unlinks `content/base/` before reaching the open binary | Stage 5 writes `.dist-incomplete` into the aside tree, renames the half-tree back, and exits non-zero naming both paths; no directory holding content is left without a marker | manual (Windows) |
| P26 | Two manifests, same package name | Run A between stage 2 and stage 5; run B with a different `--manifest`, the same `[package] name`, and an `--out` under `dist/` that is neither run A's payload root nor a directory inside it | The scratch paths differ (they are keyed by manifest path), so run A installs its own entry script | manual |
| P28 | Non-`.prm` neighbours in the materials listing | `is_prm_filename` over a listing holding a `<64 hex>.prm`, a `<hex>.prm.tmp.48291` partial, a `.gitignore`, and a subdirectory name | Exactly the `<64 hex>.prm` is accepted; stage 7 copies that one file and nothing else | unit |
| P29 | Stale SDK `.ts` edit | A `.ts` under `sdk/lib` is edited since `scripts-build` was last relinked; stage 1 runs and cargo does not relink | Stage 6 completes; `prl-build`'s own sidecar rebuild runs with the workspace root as its cwd rather than an inherited one | manual |
| P30 | `.luau` branch with a stale mod-root `.js` | Mod root holds `start-script.luau` and a gitignored `start-script.js` | The payload's mod root holds only `start-script.luau`; launching reaches the frontend menu rather than `run_mod_init`'s both-present rejection | manual |
| P31 | Symlinked path under `dist/` | `dist/link` is a symlink to `content/dev`, with `--out dist/link` and `name = "maps"`; then the default output on a checkout reached through a symlink | `guard_payload_root` refuses the first on the `NotUnderDist` arm — canonicalizing the workspace side alone would have read the payload root as under `dist/` and permitted the delete — and permits the second, where canonicalizing both sides is what keeps a symlinked checkout's own `dist/` usable | unit |
| P32 | Outstanding list after the final bake | `outstanding_outputs` with every output completed | Returns empty, which the marker renders as a `stage 7` status line and no output lines | unit |
| P33 | Stage output surface | A successful run's stdout | A header per stage naming what it produced, stage 6's bake order printed before the first bake, and a closing file count and total size | manual |
| P34 | Kill between root creation and the marker write | Stage 5 removes the aside tree, creates `<out>/<name>`, SIGKILL before the marker's rename | The one uncovered window: `dist/<name>/` exists, is empty, and carries no marker; the next run's stage 5 deletes it | manual |
| P35 | First-ever run, nothing at the payload root | `dist/` absent entirely; a full `dist` run to completion | Stage 5 creates `<out>` and the payload root without attempting a rename or a removal, writes the marker, and the run completes with no `.dist-incomplete` after the sweep | manual |
| P36 | Guard permit, a non-default output root | `--out dist/nightly` with `name = "postretro-dev"`, payload root absent | `guard_payload_root` permits: a payload root need not be `dist/`'s immediate child, only strictly under it | unit |
| P38 | Failure during payload assembly | Stages 1-4 succeed; the marker is written; stage 5's tree copy then fails | Non-zero exit; the marker is present with first line `stage 5` and every resolved output as an outstanding line | manual |
| P39 | Stray marker temp collected | `<out>/.<name>.marker-99999.tmp` is planted; a full `dist` run to completion | `<out>` holds no `.<name>.marker-*.tmp` afterwards, and the planted file never enters the payload root | manual |
| P40 | Guard trip, a payload root nobody built | `--out dist` with `name = "notes"`, where `dist/notes/` already exists holding one stray file and, at its top level, neither `.dist-incomplete` nor the engine binary; then the same directory emptied. The payload root is under `dist/` and is a directory, so the provenance arm is the only arm that can refuse either half | `guard_payload_root` refuses the first on the provenance arm and permits the second, so a mistyped `--out` cannot delete a tree no `dist` run produced while the kill window P34 leaves behind stays collectable | unit |
| P41 | Guard permit, a payload root a run did build | Three existing non-empty payload roots under `dist/`: one holding the engine binary at its top level and no marker, as a completed swept run leaves it; one holding `.dist-incomplete` and no binary; one holding both. On a Windows host the binary is named `postretro.exe` | `guard_payload_root` permits all three on the provenance arm, so the guard that protects AC11's second run and P6's resumed run does not refuse them | unit |
| P42 | Guard trip, payload root exists and is not a directory | `--out dist` with `name = "notes.txt"`, where `dist/notes.txt` exists as a regular file; then `--out dist` with `name = "link"`, where `dist/link` is a symlink to the directory `dist/real`: containment permits both paths, so `NotADirectory` is the only arm that can refuse either | `guard_payload_root` refuses both on the `NotADirectory` arm, ahead of a provenance arm that would otherwise have to read the top level of a regular file; the symlink is refused although it resolves under `dist/`, because the arm reads the payload root's own metadata rather than following it | unit |
| P43 | `mod_root` under `dist/` | A manifest carrying `mod_root = "dist/packaged"` | Manifest parse fails naming the entry, keeping the mod root out of the tree stage 5's delete works in | unit |
| P44 | Target directory inside `dist/` | `CARGO_TARGET_DIR` set to a path under `<workspace>/dist/` that does not yet exist; a full `dist` run | Stage 1 fails naming the resolved `target_dir`, that path still does not exist afterwards so no build ran and nothing cargo wrote landed under `dist/`, and no later stage runs, so stage 5's delete never reaches the tree holding the binaries stages 5 and 6 read | manual |
| P45 | Guard re-evaluated at stage 5 | `--out dist/x` with the payload root absent, so the parse-time evaluation permits; while stage 1 builds, create that payload root holding one stray file and neither `.dist-incomplete` nor the engine binary | Stage 5 refuses on the provenance arm and exits non-zero; the planted directory is still there afterwards, so the second evaluation is what refused | manual |

## Open questions

- **Resolved — the four `-mtex` catalog variants are dropped from `mapCatalog`.** Nothing outside the
  catalog and this spec references `campaign-test-mtex-002`, `occlusion-test-mtex-001`,
  `occlusion-test-mtex-0015` or `occlusion-test-mtex-002`; none carries a `.map` of its own, and all
  four demonstrate only a finer `--lightmap-density` on `campaign-test.map` and `occlusion-test.map`,
  both of which still ship at the default density. That settles the question this section used to ask
  — whether the four survive a `--release` bake on the density axis, the term the merged shadowmask
  work fixed for light count but explicitly left untouched for density — because they were the only
  catalog levels loading that axis at all. Provenance was not establishable: the checkout is a shallow
  clone whose graft boundary predates the four entries, so the case for dropping them rests on nothing
  referencing them, not on the task that created them being closed.
- **Resolved — P7 and P25 run as a manual gate on a Windows host, not a deferral or a follow-up.**
  `dist` builds for the host (Out of scope: Cross-compilation), so only a Windows host produces a
  running copy holding `postretro.exe` open, the precondition both rows turn on. Task 6's Windows gate
  pushes the implementation branch as it stands and executes both rows there against a payload built on
  that host. AC18 stands unchanged: both rows become executable rather than excluded from it.

## Rough sketch

New directory module `crates/xtask/src/dist/`, entered from `try_main` as `dist::run(args.collect())`
alongside the `crate_graph` arm; `main.rs` gains a dispatch arm and help lines only.

Reuse the existing xtask helpers: `workspace_root()` and `run_checked(&mut Command, label)` for every
subprocess, and set the working directory on the `prl-build` invocation the way `run_postretro`
already does. Resolve `cargo` the same way the `run` path does — `std::env::var_os("CARGO")` with a
`"cargo"` fallback. `build_scripts_sidecar` is reusable only if passed `--release`; its `observe`
call site passes `&[]`, which builds debug. All of these are private crate-root items, reachable from
`dist` as a child module without any visibility change.

Locate the built binaries under `<target-dir>/release/` by name, appending `.exe` under
`cfg!(windows)` — the same shape as `scripts_build_in_dir` in `crates/scripting-core/src/watcher.rs`,
whose `.exe` probe is worth copying. The lookup itself must be written here: both in-repo analogues
are private to their own crates, and `scripts_build_beside` additionally encodes a Cargo `deps/`
test-layout preference.

`.prm` output is not addressable by flag: `resolve_prm_root_via_cargo` in
`crates/level-compiler/src/main.rs` walks for a `Cargo.toml` ancestor of the map source and lands on
`<workspace>/baked/materials`. Since `dist` bakes from workspace sources the mips land there, which is
why the copy is stage 7 — the directory is gitignored, and stages 4 and 6 are its only writers. The
runtime side of that path is `derive_prm_root_dev_layout` in
`crates/postretro/src/startup/worker.rs`, which resolves the content root's grandparent plus
`baked/materials` — so the payload's `<root>/baked/materials` is what a payload rooted at
`<root>/content/<mod>` reads.

Model-texture baking is in-process: `bake_model_textures_for_gltf(gltf_path, prm_root)` already lives
in `crates/xtask/src/main.rs` behind the `bake-model-textures` command. `dist` calls it directly
rather than shelling out.

`toml` and `serde` are workspace dependencies; `xtask` has neither in its own manifest today and needs
both. The workspace `serde` entry already enables `derive`.
