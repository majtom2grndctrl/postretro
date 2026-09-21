# Modding with the Postretro SDK

The **SDK bundle** is a self-contained, content-complete modding kit:
everything you need to **play** a Postretro game *and* keep editing it, without
cloning a repository or installing Rust. The maps ship baked, so it runs on
arrival; the sources and compilers ship too, so you can change anything and
rebake.

The bundle is also a Postretro **project** in its own right — it carries a
`postretro.toml` at its root — so the tool it ships can build a player payload
from it with no toolchain of any kind.

## SDK bundle vs. player payload

`dist` and `sdk-dist` both assemble a self-contained folder, but for different
audiences:

| | `dist` (player payload) | `sdk-dist` (SDK bundle) |
|---|---|---|
| Audience | Someone who plays the game | Someone who plays **and** authors it |
| Playable on arrival | Yes | Yes (maps ship baked) |
| Engine build | Release | **Debug**, built with `--features dev-tools` |
| Levels | Baked `.prl` only | Baked `.prl` **and** source `.map` + the compiler |
| Scripts | Baked `.js`/`.luau` only | Baked **and** source `.ts`/`.luau` + the compiler |
| FGD, TS typings, docs, tools, `postretro-tool` | Not included | Included |
| Folder name | `<package name>` | `<package name>-sdk` |
| Size | Lean | Larger (debug engine + sources + tools) |

Both are playable folders. `dist` is the lean build to hand to players. Use
`sdk-dist` when the recipient (including future-you) should also be able to keep
editing — it is a superset, at the cost of size.

## Producing a bundle

From a project root:

```bash
bin/postretro-tool sdk-dist
```

It takes the same flags as `dist`, with the same resolution rules:

```bash
bin/postretro-tool sdk-dist --manifest path/to/postretro.toml
bin/postretro-tool sdk-dist --out dist/nightly
```

`--manifest` names a project marker explicitly instead of searching upward for
one. `--out` selects the parent directory the bundle folder is created in, and
must still land strictly inside the project's own `dist/`. Both resolve a
relative path from the directory you ran the command in. See
[docs/distribution.md](distribution.md) for the containment rule in full.

The bundle is written to `<out>/<package name>-sdk`.

**The bundle is host-native.** Its binaries were compiled for one operating
system — produce the bundle on the OS you want it to run on. See "Build on the
operating system you will ship for" in [docs/distribution.md](distribution.md).

**It bakes the levels *and* ships the sources.** `sdk-dist` runs the same level
and material bakes as `dist`, so the bundle is playable the moment it is
unpacked — but it does not run the player payload's source-exclusion filter over
the content tree, so the `.map`/`.ts` sources ship beside the freshly baked
`.prl`/`.js`, together with the compilers needed to rebake them. If a build stops
partway through, the bundle root carries the same `.dist-incomplete` marker a
player payload uses — see "If a build stops partway through" in
[docs/distribution.md](distribution.md).

## Bundle layout

```
<package name>-sdk/
  postretro[.exe]            authoring engine: debug build, --features dev-tools
  scripts-build[.exe]        beside the engine, where its hot reload looks for it
  postretro.toml             the project marker: this bundle is a project
  <package name>-sdk.{bat,sh}  launcher: pins the working directory, mounts the game
  bin/
    postretro-tool[.exe]     the content tool: run, dist, sdk-dist, asset bakes
    postretro-release[.exe]  optimized engine — what a player payload carries
    prl-build[.exe]          level compiler (release build)
    scripts-build[.exe]      TypeScript -> JS compiler (release build)
    mint-identity[.exe]      durable state-slot identity minting
  sdk/                       TrenchBroom FGD + GameConfig + models, lib/, types/,
                             templates/, behaviors/, type-tests/
  docs/                      this documentation set
  tools/                     Python asset helpers (see tools/README.md)
  core/                      engine-owned assets: UI descriptors, splash, licences
  content/<mod-root>/        the game's tree: SOURCE .map/.ts beside the freshly
                             baked maps/*.prl and the emitted start-script.js
  baked/materials/           .prm material sidecars (so the baked maps render)
  README.md                  generated quickstart for this bundle
```

This bundle publishes its mod under the mod root named in its `postretro.toml`
at the bundle root — the generated `README.md` shows the exact path. Throughout
this document `<mod-root>` stands for that path; substitute it in the commands
below. `content/base` is the recommended convention for a game you start
yourself, but a bundle keeps whatever root its source project declared.

`<mod-root>/` ships both halves: the baked outputs the engine loads to play
(`maps/*.prl`, the emitted `start-script.js`) and the sources you keep editing
(`maps/*.map`, `scripts/`, `start-script.ts` or `start-script.luau`, models,
other assets). Only genuinely regenerable junk is dropped — `.build-caches`,
`maps/autosave/`, `.git*` entries, and any *stale* committed `.prl`/`.js`, since
the run produces fresh ones.

A distribution publishes a game under its declared mod root, keeping that name,
and `core/` is what the engine owns and a game never replaces. Neither is
arbitrary; see "Payload layout" in [docs/distribution.md](distribution.md).

## Play it first

The maps are already baked, so before editing anything you can just run it. From
the bundle root:

```bash
bin/postretro-tool run
```

Or start the launcher (`<package name>-sdk.bat` on Windows). Either way the
working directory ends up pinned to the bundle root, which is what makes the
content paths resolve.

## The authoring loop

1. **Author a level in TrenchBroom.** Load `sdk/TrenchBroom/postretro.fgd` as
   the game definition and point the texture path at the `textures/` directory
   inside `<mod-root>/`.
   See `docs/level_design.md` for entity and lighting reference.

2. **Recompile the level:**

   ```bash
   bin/prl-build <mod-root>/maps/<name>.map \
     --baked-root baked --cache-dir .build-caches/prl-cache \
     -o <mod-root>/maps/<name>.prl
   ```

   `--baked-root` names the directory that *contains* `materials/` — not
   `materials/` itself. Get it wrong and nothing fails: the compiler writes its
   `.prm` sidecars somewhere the engine does not read, and every world texture
   renders as a placeholder with a warning in the log. It looks like a broken
   engine rather than a mistyped path. `--cache-dir` keeps the disposable stage
   cache out of your `maps/` directory.

   You should not have to type either one anywhere else. `bin/postretro-tool run`
   gives the engine its matching `--baked-root`, and a `dist` or `sdk-dist` run
   supplies both to the compiler itself.

3. **Author scripts in TypeScript** against `sdk/types/` and `sdk/lib/`.

4. **Run it:**

   ```bash
   bin/postretro-tool run
   ```

   The debug engine auto-compiles any `.ts` with a same-stem `.js` sibling at
   startup and hot-reloads script edits while it runs — just save the file. To
   compile a script by hand instead:

   ```bash
   bin/scripts-build --in <mod-root>/start-script.ts --out <mod-root>/start-script.js
   ```

   Because it is a `--features dev-tools` build, the egui inspector and debug UI
   are available; see `docs/diagnostics.md` for the diagnostic keyboard chords.

`bin/postretro-tool run` forwards anything it does not recognize straight to the
engine, so `bin/postretro-tool run <mod-root>/maps/<name>.prl` loads that level
directly instead of starting at the frontend.

## Keeping your game in its own repository

You do not have to author inside the bundle. Your content can live in a
version-controlled repository of your own, with the bundle installed separately
as the engine you build it with. That is the recommended shape once a game grows
past experimenting, and it is what `postretro.toml` exists for. See
[docs/external-projects.md](external-projects.md).

## Why the SDK ships a debug engine

This is the one place the SDK engine intentionally differs from what players
run, and it is worth understanding why.

TypeScript startup auto-compile and TS hot reload are gated on debug builds, not
on the `dev-tools` feature. `dev-tools` only adds the egui inspector UI on top.
A release engine holds no TypeScript compiler at all: every script it runs must
already be compiled ahead of time, which is what a `dist` run does when it
packages a player payload. A modder needs both halves — debug for the
edit-and-see loop, `dev-tools` for the inspector — so the bundle's root engine is
a debug build with that feature enabled.

The bundle also carries `bin/postretro-release`, an optimized engine with default
features. That is the engine a player payload gets, and the one `dist` copies.

If you want to type-check your scripts, run `tsc --noEmit` yourself;
`scripts-build` does not type-check.

## Docs and tools in the bundle

- `docs/level_design.md` — TrenchBroom entities, lights, fog volumes, textures
  (including specular and Surface Depth height maps), map sealing.
- `docs/scripting-reference.md` — the scripting API surface.
- `docs/weapon-mounts.md` — rigid weapon mount authoring workflow.
- `docs/diagnostics.md` — runtime diagnostic keyboard chords.
- `docs/distribution.md` — producing a lean, shippable player build.
- `docs/external-projects.md` — keeping your game in its own repository.
- `tools/` — asset helpers. The Python ones (specular, normal and emissive map
  generation, model rebaking) need Python plus a small virtual environment; see
  `tools/README.md` before running them. `tools/texture-tool` is a Rust program
  that writes a whole diffuse/specular/normal/height bundle in one run — it is
  the one thing in this bundle that needs a Rust toolchain you install yourself;
  see `tools/texture-tool/README.md`.

## Shipping a finished mod

The SDK bundle is playable, but it is heavy — a debug engine, sources, and
developer tooling. Once your game is ready to hand to players, build the lean
player payload instead:

```bash
bin/postretro-tool dist
```

That is the whole command. No repository, no toolchain — the tool runs the
binaries already sitting in `bin/`. See [docs/distribution.md](distribution.md)
for choosing levels, zipping and sending the payload, and what recipients should
expect.
