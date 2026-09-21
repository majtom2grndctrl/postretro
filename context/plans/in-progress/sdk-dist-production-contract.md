# SDK distribution: production-ready contract

> **Status:** in progress. Branch `sdk-dist-production`.
> **Read before any track brief.** Amend here when a track changes a decision.

## Goal

Make a PostRetro SDK bundle a self-sufficient product. Today the bundle ships
`docs/modding.md`, whose final section tells its recipient to run
`cargo run -p xtask -- dist` — a command that needs a repository, a Rust
toolchain, and a crate that is not in the bundle. Three changes close that: the
content tools move out of `xtask` into a shippable `postretro-tool` binary that
discovers its project rather than baking one in at compile time; the `.prm`
materials root gains symmetric overrides so a developer's content can live in
their own repository without every texture silently degrading to a placeholder;
and the engine's own assets move out of `content/base` so a distribution can
publish the developer's game there, which is what the Quake-derived name meant
all along.

## Decisions

Each carries the consequence that makes it load-bearing.

### D1. Engine assets move to `core/` at the tree root

`content/base/ui/*.json` becomes `core/ui/*.json`;
`content/base/textures/splash/*` becomes `core/textures/splash/*`.
`content/base/` is removed from the repository.

*Consequence:* `content/base` becomes free for D6 to publish the developer's
game into. `core/` is a sibling of `content/` and `baked/`, so it reads as
engine-owned to anyone browsing an install, and it is outside the
`<container>/<mod>` two-component shape that `build_pipeline.md`
§Baked texture mips requires of mod roots — engine assets are not a mod.

*Correction this encodes:* the four UI JSON descriptors are **not** dead
fixtures. All four register at boot through `register_tree_from_disk`
(`crates/postretro/src/session/mod.rs:418`). `pauseMenu`, `frontendMenu`, and
`keyboard` are the only implementations of those screens; `hud.json` is a
deliberate fallback that mod content shadows, and
`fallback_hud_descriptor_carries_the_fallback_only_marker` asserts its
`"FALLBACK HUD HP --"` marker stays fallback-only so shadowing tests can prove a
mod replaced it. Deleting any of them removes a working screen.

### D2. Fonts leave content entirely

`content/base/fonts/Inter-Regular.ttf` and
`content/base/fonts/JetBrainsMono-Regular.ttf` move to
`crates/ui/assets/fonts/`. Their OFL licence texts move to `core/licenses/`,
*not* beside the fonts.

*Amended after Track 1.* The licence texts reached every payload only because
`dist` copied `content/base` wholesale. Once the fonts became crate-local build
inputs they reached no payload at all, while the binary still embeds both faces
— and SIL OFL 1.1 requires the licence to accompany the font, embedded or not.
`core/licenses/` is inside the tree D1 already makes `dist` and `sdk-dist` copy,
so the licence ships with the binary that embeds the faces, independent of
whatever fonts the shipped game supplies.

*Consequence:* both faces are `include_bytes!`-ed into the `ui` crate at
`crates/ui/src/text.rs:10` and `:17`. They are a compile-time build input and a
test fixture, never read from disk at runtime. Because `dist` copies
`content/base` wholesale, every payload today ships two font files nothing
opens. Moving them into the crate removes that, and removes the false
affordance that editing them changes anything. `read_font_file` — the genuine
runtime disk path, for mod-supplied fonts — is unaffected.

### D3. Symmetric `--baked-root` on `prl-build` and `postretro`

Both binaries accept `--baked-root <dir>`, naming the directory that *contains*
`materials/`. When absent, both derive exactly as they do today, byte for byte.

*Consequence:* this is the blocker under the external-content convention, not a
documentation gap. `prl-build` walks up to the nearest `Cargo.toml`
(`crates/level-compiler/src/cache.rs:534`, used at
`crates/level-compiler/src/main.rs:130`) and otherwise falls back to
`<map parent>/baked/materials`. The engine derives
`<content_root>/../../baked/materials`
(`crates/postretro/src/startup/worker.rs:108`). A developer's repository has no
`Cargo.toml`, so the compiler writes `<repo>/maps/baked/materials` while the
engine reads `<install>/baked/materials`. Every world material then degrades to
a placeholder with a `warn!` and no failure — the worst available outcome, since
it looks like a broken engine.

### D4. `postretro-tool`: one shippable multicall binary

New workspace member `crates/tool`, package `postretro-tool`, binary
`postretro-tool`. Subcommands: `dist`, `sdk-dist`, `run`, `bake-model-textures`,
`solve-weapon-mount`, `mint-identity`.

*Consequence:* `xtask` cannot be shipped. `workspace_root()`
(`crates/xtask/src/main.rs:1331`) is `env!("CARGO_MANIFEST_DIR")`, resolved at
compile time, and every command routes through it — a shipped `xtask` would
carry the build machine's absolute path. `postretro-tool` replaces that with
runtime discovery (D5).

One multicall binary rather than several, because the subcommands share the
manifest parser, the output-root containment guard, and the completion-gate
machinery. Splitting them would duplicate all three.

### D5. `postretro.toml` is the project marker, found by walking up

`postretro-tool` locates the project root by walking parents from the working
directory for the first `postretro.toml`, the way cargo finds `Cargo.toml`. It
supersedes `dist.toml`; the repository's own `dist.toml` is renamed and
extended. No compatibility shim is kept — this project is pre-stable
(`context/lib/index.md`).

*Consequence:* the marker is what lets a content repository be a project in its
own right, and it re-anchors the output-root containment guard. That guard's
safety argument today is "`<workspace>/dist/` is gitignored and holds no
committed input" (`build_pipeline.md` §Output-root containment); in an arbitrary
repository that argument does not hold on its own, so containment anchors to the
marker's directory and the **provenance** check (a completion marker or an
engine binary at the root's top level) carries the real weight.

### D6. A distribution publishes the developer's mod into `content/base`

`dist` and `sdk-dist` write the mod tree to `<payload>/content/base/`,
regardless of the project's own mod-root name, and the launcher passes
`--mod content/base`.

*Consequence:* `content/base` is two components, so the runtime grandparent
derivation still resolves `<payload>/baked/materials` and
`build_pipeline.md` §Baked texture mips holds unchanged. The shipped-level-set
scan reads mod-root-relative `maps/<name>.prl` literals, so the rename does not
disturb it. This is a destination-path parameter change at payload assembly, not
a redesign of the stages.

### D7. The tool never compiles Rust and never links the script VM

`postretro-tool` locates helper binaries by explicit flag
(`--engine`, `--prl-build`, `--scripts-build`, `--mint-identity`), defaulting to
conventional paths beside itself. `xtask dist` becomes: cargo-build the
binaries, then invoke `postretro-tool dist` with their paths.

*Consequence:* this is the one real seam in the existing `dist`. Stage 1 builds
release binaries and needs cargo; stages 2 through 7 are pure content work and
do not. Putting the seam anywhere else means either shipping cargo or splitting
a stage. It also keeps `mint-identity` out of the tool's link graph:
`crates/sim/src/bin/mint_identity.rs` pulls in `postretro-scripting-core`'s
runtime, so linking it would drag rquickjs and mlua into a tool that otherwise
needs neither.

### D8. `sdk-dist` ships a release engine and the tool

Beyond today's debug `--features dev-tools` engine, the bundle carries
`bin/postretro-release`, `bin/postretro-tool`, and `bin/mint-identity`.

*Consequence:* without a release engine the bundle's recipient cannot produce
the player payload `docs/modding.md` promises, and the alternatives are refusing
outright or shipping a debug build under the name "player payload". Costs one
additional release engine build per `sdk-dist` run.

### D9. `postretro-tool run` drives the external authoring loop

It discovers `postretro.toml`, then launches the engine with `--mod` and
`--baked-root` already correct.

*Consequence:* the engine learns nothing about `postretro.toml` — it keeps plain
flags, and changing the manifest schema stays a tool change. Without this the
modder types two absolute paths whose failure mode is the silent placeholder
degradation D3 exists to prevent.

### D11. The tool places the stage cache; it never lands in `maps/`

*Added after Track 2.* `postretro-tool run` and the tool's bake path pass
`--cache-dir <project>/.build-caches` alongside `--baked-root`.

*Consequence:* `prl-build`'s stage-cache fallback is its own `Cargo.toml` walk,
and in an external content repository it resolves to the map's own directory —
Track 2 observed `.build-caches/` appearing inside `<repo>/levels/maps/`. Every
modder would find a growing cache directory sitting among their `.map` sources,
in the one directory they are most likely to commit. The cache is disposable
(§Build Cache) and belongs beside the project, not inside its content. This is
the same class of defect as D3, differing only in that it is visible rather than
silent, so it is fixed the same way: the tool that owns the manifest passes the
path, and nobody types it.

### D12. `docs/` is modder-facing only; engine-developer commands live elsewhere

*Added before Track 4.* Every command in `docs/` must be runnable by someone
holding only an SDK bundle. The engine-developer workflow — `cargo run -p xtask
-- …` — lives in `CLAUDE.md`, `AGENTS.md`, and `context/lib/`, never in `docs/`.

*Consequence:* `sdk-dist` copies `docs/` verbatim into every bundle, so a
workspace-only instruction there is unrunnable by definition for the tree's
actual reader — which is how `docs/modding.md` came to end with
`cargo run -p xtask -- dist`. `index.md` already routes `docs/` as "game / mod
author docs", so this makes an existing split enforceable rather than inventing
one, and it is what lets Track 4's grep gate be exact instead of approximate.

### D13. No `.prl.pack.lock` reaches any distribution

*Added at owner request, after Track 3.* Neither a player payload nor an SDK
bundle carries `.<name>.prl.pack.lock`. Both outputs remove the lock siblings
their level bakes create, and both sweeps refuse the pattern.

*Consequence, and where the fix must not go:* the lock's persistence is
deliberate. `crates/level-compiler/src/pack.rs` keeps the lock pathname between
runs on purpose — its comment explains that removing it would let a waiter hold
the old inode while a new compiler locks a freshly created one. So `prl-build`
must keep writing and keeping it; a "fix" that deletes the lock at the end of a
bake reintroduces a real publication race for every concurrent compile in a
workspace.

The packaging stage is the right place because the bake writes straight into the
payload, *after* the assembly copy filter has already run — which is exactly why
the filter never sees these files. Removing them at packaging time is safe in a
way it is not in a workspace: nothing recompiles a player payload's `.prl` in
place, and a modder who rebakes inside an SDK bundle simply recreates the lock
on demand. The sweep refusal is what keeps a later change from quietly
reintroducing them.

### D14. Engine-owned trees come from the install root, always

*Added after Track 4; corrected by the owner before Track 3 implemented it.*
`core/`, and for `sdk-dist` also `sdk/`, `docs/`, and `tools/`, resolve under the
**install root only**. The project root is never consulted for them. The install
root comes from `--install-root <dir>`, defaulting to a directory derived from
the tool's own executable location.

The project root is a separate, independent lookup: `--project <dir>`, or
`--manifest <file>` to name a marker directly, or a walk up from the working
directory as a convenience. Neither lookup falls back to the other.

*First draft was wrong.* It resolved the project root first and fell back to the
install. That quietly reintroduces what D1 removed — a game tree able to shadow
engine assets, decided by whichever directory happens to exist — and it makes the
dev checkout the primary case with the shipped layout as its fallback, which is
backwards. `core/` is a single path component precisely so `--mod` cannot
redirect it; resolution order must not hand back the same power.

*The dev checkout is the case that tempts the fallback, and it has a clean
answer.* In a workspace the tool sits at `target/debug/postretro-tool` with no
`core/` beside it — but the workspace **is** the install, and `xtask` already
knows the workspace root, so `xtask dist` and `xtask sdk-dist` pass
`--install-root <workspace>` explicitly. The tool's rule stays single and
unconditional, and the one piece of checkout-specific knowledge lives in the one
crate entitled to it. A "walk up from `target/debug` looking for a marker"
heuristic is the same trial-and-error resolution in a different coat, and is
excluded.

A bundle acting as its own project has both lookups land on one directory. That
is a coincidence of layout, not a rule in the code.

*Consequence:* Track 4 found that both outputs copy these trees from the project
root alone, so an external content repository — the layout this whole session
exists to enable — cannot build a distribution without a recipient first copying
engine-owned directories into their game repo. Worse, `postretro-tool run` pins
the working directory to the project root and the engine resolves `core/…`
cwd-relative, so an external project silently loses the pause menu, frontend
menu, and on-screen keyboard with only warnings.

The two-step order is what makes all three layouts work with one rule: a
workspace has `core/` at the project root and nothing beside
`target/debug/postretro-tool`; an external project has it only in the install
beside the tool; a bundle acting as its own project has both, pointing at the
same tree. The tool already resolves helper binaries by searching beside its own
executable, so the install root is a path it can derive.

### D10. `content/dev` stays in the repository

The engine's own test content — fixtures, stress maps, capture rigs — is
referenced by workspace-relative path throughout the test suite and tooling. It
keeps working through the in-repo `Cargo.toml` derivation and does not migrate
to the external convention.

*Consequence:* the external convention is proven by documentation and by the
tool's own tests, not by moving content that CI depends on.

## Invariants

No track may break these.

1. **No upward crate edges.** `layering_invariants_hold`
   (`crates/xtask/src/crate_graph.rs:496`) enforces that nothing depends on the
   `postretro` binary, `foundation` is a leaf, `entities` depends only on
   `foundation`, `postretro-net` has no internal dependencies, and only
   `postretro` depends on `postretro-ai`. `postretro-tool` depends downward only
   — `postretro-level-compiler`, `postretro-level-format`, `postretro-model` —
   exactly as `xtask` does today, and never on `postretro` or `postretro-sim`.
2. **Adding a workspace member invalidates the crate-graph snapshot.**
   `cargo run -p xtask -- crate-graph --check` is a preflight gate; regenerate
   `context/lib/crate-graph.md` with `--write` in the same change.
3. **Default behaviour is byte-identical.** With no `--baked-root` and no
   `postretro.toml` beyond the repository's own, every existing path — dev run,
   `cargo test`, `prl-build` from the workspace — resolves the same directories
   it resolves on `main`.
4. **`--baked-root` names the parent of `materials/`, not `materials/` itself.**
   `--baked-root /p/baked` reads and writes `/p/baked/materials/<hex>.prm`. Both
   binaries agree on this; the opposite reading is the silent-placeholder bug.
5. **A mod root is exactly two `/`-separated components.** Unchanged from
   `build_pipeline.md` §Baked texture mips. `content/base` satisfies it; `core/`
   is deliberately outside it and is not a mod root.
6. **Payload stages 1 through 4 write nothing into the payload root.** That is
   what makes stage 5's delete safe (`build_pipeline.md` §Distribution
   packaging). Preserved through the move to `postretro-tool`.
7. **`--release` is the only shippable bake.** The tool supplies it; a manifest
   recipe may not. Bakes run one at a time, ordered by ascending effective
   lightmap density, ties broken lexicographically by output path.
8. **The completion marker's format is unchanged.** First line names the stage;
   every following line is one outstanding level as a mod-root-relative
   `maps/<name>.prl` with `/` separators.
9. **Engine assets are not mod content.** Nothing under `core/` is resolved
   through the mod content root, and `--mod` never redirects it.
10. **Prose follows `context/lib/context_style_guide.md`; files follow
    `development_guide.md` §2** (~400–500 lines yellow, ~600+ split first; tests
    exempt).
11. **An engine flag that takes a value must join `resolve_map_path`'s skip
    list** (`crates/postretro/src/startup/session.rs`). That scan treats the
    first non-flag argument as the map path, so a flag it does not recognise
    leaves its *value* exposed and the engine loads it as the level. Track 2
    hit this with `--baked-root` and pinned it with
    `prm_root_flag_value_is_not_mistaken_for_the_map_path`.
12. **`--baked-root` and `--cache-dir` are passed together or not at all.**
    Passing the baked root to only one binary, or passing it without the cache
    dir, each reproduces a defect the other closes (D3, D11). The tool owns both.

## File ownership per track

A track edits only what is listed for it. Compile-forced spillover outside the
list is allowed and must be reported.

### Track 1 — `core/` engine asset root (D1, D2)

- `content/base/**`, moving to `core/**` and `crates/ui/assets/fonts/**`
- `crates/ui/src/text.rs`, `tree_asset.rs`, `keyboard_asset.rs`, `demo.rs`
- `crates/postretro/src/startup/mod.rs`, `render/splash.rs`, `main.rs`,
  `render/ui_lifecycle_render_test.rs`, `session/mod.rs`
- `crates/renderer/src/render/splash_pass.rs` (doc comment only)
- `crates/xtask/src/dist/mod.rs`, `crates/xtask/src/sdk_dist/mod.rs` — the
  `content/base` copy pair only (`dist/mod.rs:416`, `sdk_dist/mod.rs:348`)
- `sdk/lib/ui/reactions.ts` (doc comment)

### Track 2 — symmetric `--baked-root` (D3)

- `crates/level-compiler/src/main.rs` (arg parsing, `resolve_prm_root_via_cargo`)
- `crates/postretro/src/startup/session.rs`, `startup/worker.rs`,
  `startup/mod.rs` (threading only)

### Track 3 — `postretro-tool` (D4 through D9)

- `crates/tool/**` (new)
- `crates/xtask/src/main.rs`, `crates/xtask/src/dist/**`,
  `crates/xtask/src/sdk_dist/**`
- `Cargo.toml` (workspace members, dependencies)
- `dist.toml`, becoming `postretro.toml`
- `.gitignore` if required

### Track 4 — documentation (all decisions)

- `docs/distribution.md`, `docs/modding.md`, new external-project guide
- `docs/scripting-reference.md`, `docs/level_design.md` — both carry
  `content/base/ui` and `content/base/textures` paths that D1 invalidates
- `context/lib/build_pipeline.md`, `context/lib/ui.md`,
  `context/lib/boot_sequence.md`, `context/lib/index.md`,
  `context/lib/resource_management.md`
- `context/lib/crate-graph.md` (regenerated, not hand-edited)
- `CLAUDE.md` and `AGENTS.md` — the `prl-build` example compiles to
  `content/base/maps/output.prl`, which D6 makes a distribution-only path

## Acceptance per track

### Track 1

```bash
grep -rn "content/base" --include=*.rs --include=*.ts crates/ sdk/
test ! -e content/base
cargo test -p postretro-ui --lib
cargo test -p postretro --bin postretro startup
cargo run -p xtask -- run content/dev/maps/campaign-test.prl
```

The grep returns no output; `test` exits 0; both test commands report `ok` with
0 failed and a non-zero count.

The engine run is the artifact, not a proxy for it: `load_named_tree` degrades a
missing descriptor to a `warn!` and keeps booting, so a broken path passes every
compile and test gate above and shows up only as a missing screen. Read the log
for `[UI] tree asset` and look at the window.

### Track 2

```bash
cargo test -p postretro-level-compiler --bin prl-build baked_root
cargo test -p postretro --bin postretro prm_root
```

Expect `ok` with at least 3 and at least 2 tests passed respectively.

Required assertions: with the flag absent, `prl-build`'s resolved root equals
the `Cargo.toml`-walk result and the engine's equals the grandparent walk, both
unchanged; with the flag present on both sides pointing at one directory, the
two resolved `materials/` paths are equal. That last equality is the check that
would have caught the defect D3 describes.

### Track 3

```bash
cargo run -p xtask -- crate-graph --check
cargo test -p postretro-tool
cargo run -p xtask -- dist
test -d dist/postretro-dev/content/base
test ! -e dist/postretro-dev/.dist-incomplete
cargo run -p xtask -- sdk-dist
ls dist/postretro-dev-sdk/bin/
```

`crate-graph --check` and both `test` commands exit 0; `cargo test` reports `ok`
with 0 failed; `dist` prints `Distribution complete`; `sdk-dist` prints
`SDK distribution complete`; the `bin/` listing holds `postretro-tool`,
`prl-build`, `scripts-build`, `mint-identity`, and `postretro-release`.

Then, from `dist/postretro-dev-sdk/` with no cargo on `PATH`, the bundle's own
tool must produce a payload:

```bash
cd dist/postretro-dev-sdk && ./bin/postretro-tool dist
```

That run is the acceptance. It is the exact thing the bundle's documentation
promises today and cannot deliver, and no workspace-side test substitutes for
it — every workspace test has a `Cargo.toml` ancestor and a cargo binary, which
are the two things the recipient does not have.

### Track 4

```bash
grep -rn "cargo run -p xtask" docs/
grep -rn "content/base/ui\|content/base/fonts\|content/base/textures" docs/ context/lib/
```

Both greps return no output.

`docs/` is the tree `sdk-dist` copies into the bundle. A `cargo run -p xtask`
instruction there is, by construction, an instruction its reader cannot follow.

## Known-red on this machine, before any track ran

Track 1 confirmed each of these against unmodified `HEAD`. They are not
regressions, and no track should spend time re-deriving them. The owner asked
for the three test and format failures to be **fixed** rather than documented,
so Track 2's scope was extended to cover the two test fixtures and its file
ownership extended to `crates/postretro/src/startup/lifecycle.rs`.

- **`cargo fmt --all -- --check`** is red on `main` in four files no track
  edits: `crates/level-compiler/src/texture_mips.rs`,
  `crates/render-cpu/src/surface_depth.rs`,
  `crates/render-data/src/material.rs`, and
  `crates/renderer/src/render/tests/surface_depth_tests.rs`. Running `rustfmt`
  on `level-compiler`'s crate root follows its `mod` tree, so `texture_mips.rs`
  is already fixed on this branch as a side effect. The remaining three stay
  untouched — unrelated churn does not belong in this diff — so preflight's
  format check will report them at landing, and that report is expected.
- **`startup::lifecycle::tests::level_identity_keeps_outside_content_root_absolute`**
  fails on Windows. `/tmp/test.prl` is rooted but not absolute there, so
  `level_identity` takes its `cwd.join` branch and yields `C:/tmp/test.prl`.
  A test-fixture assumption, not a product bug — the fixture is fixed to build
  a genuinely absolute path on both platforms, and the production function is
  left alone. The fix must keep the test exercising the outside-the-content-root
  branch its name claims; one that passes by no longer reaching that branch is
  worse than the failure.
- **`model_byte_report_names_textures_relative_to_the_content_root`** in
  `level-compiler` fails on backslash-versus-slash separators. Fixed by
  normalizing the separator where the report name is built, matching what
  `level_identity` already does, rather than by loosening the assertion: the
  byte-accounting report name is a key meant to be comparable across machines,
  and one that differs by platform defeats that.
- **`cargo test -p xtask`** fails
  `prop_writer_output_loads_and_checks_without_cli_axes_or_euler` because Python
  is not installed here. The test shells the real Python prop writer on purpose
  — it exists to cover the writer → loader → declared-check seam without a
  second hand-authored JSON shape. Owner's call: install Python, leave the test
  alone. A bare interpreter suffices despite the script's `bpy` and `mathutils`
  imports, because the test injects empty stub modules for both before loading
  it, so nothing in the Blender API or `tools/requirements.txt` is reached.
  Track 3 owns `crates/xtask/src/main.rs` and should make the failure name the
  missing interpreter instead of surfacing the Store stub's message raw.

  *Resolved.* Python 3.13.14 installed through `uv python install --default`,
  which lands `python3` in `…/scoop/persist/uv/python/shims`. That directory
  already precedes `AppData/Local/Microsoft/WindowsApps` on PATH, so the Store
  alias stubs — the source of the "Python was not found" message — are shadowed
  without touching App Execution Aliases. `uv`'s `--default` flag is marked
  experimental, so if a bare `python3` ever disappears from PATH again, that is
  the first thing to check.

## Track 1 spillover, already applied

Track 1's grep gate is literal, so five files holding `content/base` as an
unrelated *example* path were renamed to `content/example`:
`crates/level-compiler/src/main.rs`,
`crates/postretro/src/startup/{session,worker}.rs`, and
`crates/xtask/src/dist/payload.rs` — files owned by Tracks 2 and 3. All are
test-only string literals. `crates/level-format/src/gltf_resolve.rs` held a
false positive (a file literally named `base.png`) now written `/assets/base.png`.

## Left open by Track 2, for Track 3 or the review to weigh

Track 2 verified the external-content path end to end against a real repository
with no `Cargo.toml` ancestry, including a negative control that reproduced the
D3 defect. These are the edges it could not reach.

- **`--capture` ignores `--baked-root`.** `crates/postretro/src/capture/driver.rs`
  calls the runtime derivation directly, outside the `App` and worker path the
  flag threads through. A capture run against external content will silently
  placeholder. The capture rig is workspace-only by construction today, so this
  is dormant rather than broken — but it is the first place to look if the rig
  ever targets a project.
- **The headless and observability level-load path** derives its content root
  separately in `crates/postretro/src/session/mod.rs`, and no test above
  exercises whether it reaches the threaded worker or bypasses it.
- **Multi-level loads within one session.** The override is read once at boot
  and cloned per load, so a level change keeps it. Asserted structurally, not by
  a running-session test.

## Open questions

None blocking. D1 through D11 were settled with the owner before their
respective dispatches.

**The 11 pre-existing `level-compiler` failures, now diagnosed.** They are two
unrelated causes, and only one of them is confined to test code.

**`cache.rs` (2 failures) — a production defect on Windows, not a test
artifact.** `StageCache::get` opens the entry with `fs::File::open` — a
read-only handle — and then calls `set_modified` to bump the entry's mtime for
LRU. Windows requires `FILE_WRITE_ATTRIBUTES` to set a file time, which a
read-only handle does not carry, so the call fails with `Access is denied`
(os error 5). It is wrapped in `let _ =` as best-effort, so **on Windows the
LRU touch has never worked and nothing reports it**. `prune_to_budget`
therefore evicts by write time rather than use time — it degrades from LRU to
FIFO — and the case the code comment says the touch exists to protect (a
long-stable entry hit every build but never rewritten) is exactly the one
wrongly evicted. The two failing tests share the cause through their own
read-only `set_mtime` helper, which is why the production bug stayed masked:
the test that would catch it fails first, for its own reason.

Verified directly: a standalone program shows `set_modified` returning
`Err(PermissionDenied, os error 5)` on a handle from `File::open` and `Ok(())`
on one from `OpenOptions::new().write(true)`.

The careful fix is **not** to open the entry for writing in `get`. That would
turn a read-only cache directory from "reads fine, no LRU touch" into a total
cache miss, since the open itself would fail. Keep the read handle read-only
and perform the touch through a separate short-lived write handle, still
best-effort — the read path then behaves exactly as it does today and only the
touch improves.

**`pack.rs` (9 failures) — test-only.** Temp filenames interpolate
`std::thread::current().name()`, which under `cargo test` is the test path
(`pack::tests::…`). `::` is illegal in a Windows filename, so the writes fail
with os error 123. All ten occurrences sit below `mod tests`; no production
path builds a filename this way. The fix is one shared helper rather than ten
sanitized copies, sanitizing any character illegal in a path rather than `::`
specifically, so a test added later cannot reintroduce it. Keep the thread name
in the result after sanitizing: these land in the shared temp directory, and
the name is what identifies which test left one behind. `parse.rs` already uses
`thread::current().id()` for the same purpose and is a usable precedent.

Both land as their own commits on this branch, separate from the distribution
work, so preflight can go green here without burying them in the diff.

## Sequencing

Sequential on the branch, not concurrent in worktrees. Track 1 and Track 2 both
edit `crates/postretro`, so isolated worktrees would each pay the engine's cold
build and then conflict on merge; Track 3 consumes both, and its brief sharpens
once the `core/` destination and the `--baked-root` flag names are real. Order:
1, 2, 3, 4, then one review track, then `/preflight`.
