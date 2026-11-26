# Modding with the Postretro SDK

The **SDK bundle** is the Postretro engine and everything you need to build a
game with it, without cloning a repository or installing Rust: the level and
script compilers, the content tool, the SDK definitions, these docs, and a
debug engine that hot-reloads your scripts.

**It ships no game content.** You bring your own maps, textures, models and
scripts. The bundle carries one empty starter mod so the tools have something
to run against on arrival.

## SDK bundle vs. player payload

| | SDK bundle | `dist` (player payload) |
|---|---|---|
| Audience | You, building a game | Someone who plays your game |
| Game content | None — an empty starter mod | Your game, baked |
| Engine build | **Debug**, built with `--features dev-tools`, plus an optimized `bin/postretro-release` | Release |
| Compilers, FGD, TS typings, docs, tools, `postretro-tool` | Included | Not included |
| Folder name | `postretro-sdk` | your package name |

You build player payloads with the bundle's tool; see
[docs/distribution.md](distribution.md).

**The bundle is host-native.** Its binaries were compiled for one operating
system — use a bundle built for the OS you are on. See "Build on the operating
system you will ship for" in [docs/distribution.md](distribution.md).

## Bundle layout

```
postretro-sdk/
  postretro[.exe]            authoring engine: debug build, --features dev-tools
  scripts-build[.exe]        beside the engine, where its hot reload looks for it
  postretro.toml             the starter project's marker
  bin/
    postretro-tool[.exe]     the content tool: run, dist, asset bakes
    postretro-release[.exe]  optimized engine — what a player payload carries
    prl-build[.exe]          level compiler (release build)
    scripts-build[.exe]      TypeScript -> JS compiler (release build)
    mint-identity[.exe]      durable state-slot identity minting
  sdk/                       TrenchBroom FGD + GameConfig + models, lib/, types/,
                             templates/ (the starter mod and tsconfig.json),
                             behaviors/, type-tests/
  docs/                      this documentation set
  tools/                     Python asset helpers (see tools/README.md)
  core/                      engine-owned assets: UI descriptors, splash, licences
  content/base/              the empty starter mod: start-script.ts and its
                             compiled start-script.js, tsconfig.json, maps/
  README.md                  generated quickstart for this bundle
```

`core/` is what the engine owns and a game never replaces. See "Payload layout"
in [docs/distribution.md](distribution.md).

## Install a downloaded bundle

Release bundles are named `postretro-sdk-<version>-<platform>`: `macos-arm64`
(Apple Silicon), `macos-x64` (Intel Mac) or `windows-x64`. Unpack it anywhere;
the archive holds one `postretro-sdk/` folder.

These builds are unsigned. On macOS, Gatekeeper blocks binaries from a
downloaded archive until you clear its quarantine flag once, from the folder
you unpacked into:

```bash
xattr -dr com.apple.quarantine postretro-sdk
```

On Windows, SmartScreen may show **"Windows protected your PC"** the first time
an executable runs. Choose **More info**, then **Run anyway**. Only do either
for a bundle from a source you trust.

## Check the install

From the bundle root:

```bash
bin/postretro-tool run
```

The engine boots the starter mod to its fallback menu. There are no levels yet;
that is expected.

## Where your game lives

Keep your game in a version-controlled repository of your own, with the bundle
installed beside it as the engine you build with. That is the recommended shape,
and it is what `postretro.toml` exists for. Copy `sdk/templates/starter/` to
start a new mod. See [docs/external-projects.md](external-projects.md).

You can also experiment inside the bundle: the bundle root is a project whose
mod is `content/base`. Throughout this document `<mod-root>` stands for your
mod's folder — `content/base` in either case if you follow the convention.

The starter project's package is `postretro-sdk`, so runs from it keep their
settings and saved state under that name — see "Where player data lives" in
[docs/distribution.md](distribution.md) for the directory on each platform.

## The authoring loop

1. **Author a level in TrenchBroom.** Load `sdk/TrenchBroom/postretro.fgd` as
   the game definition and point the texture path at the `textures/` directory
   inside `<mod-root>/`. Save maps under `<mod-root>/maps/`.
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
   gives the engine its matching `--baked-root`, and a `dist` run supplies both to
   the compiler itself.

3. **Author scripts in TypeScript** against `sdk/types/` and `sdk/lib/`.

   Your editor gets its completions, hover docs and type errors from the
   `tsconfig.json` at `<mod-root>/`. It covers `start-script.ts` and every
   script below it, so typing inside `defineMod({ ... })` — the `render` block,
   say — offers each field and flags a value the engine does not accept. If your
   mod root has no `tsconfig.json`, copy `sdk/templates/tsconfig.json` there.
   If the editor still reports `Cannot find module 'postretro'` after you add
   one, restart its TypeScript server.

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
engine, so `bin/postretro-tool run maps/<name>.prl` loads that level directly
instead of starting at the frontend. The level path is relative to your mod's
folder, so you leave off the `<mod-root>/` prefix.

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
- `docs/teleporters.md` — classic and seamless teleporters: map keys, room
  copies, and turning them on and off from scripts.
- `docs/level-seams.md` — level seams: joining two maps so players walk from
  one into the other where they left off, seam checkpoints and scripting.
- `docs/ghost-links.md` — ghost links: seeing one room copy from another,
  ghost styles, ghost audio and scripting them.
- `docs/weapon-mounts.md` — rigid weapon mount authoring workflow.
- `docs/character-animation.md` — shared clip libraries: characters that ship
  as mesh and skeleton and take their clips from one file.
- `docs/diagnostics.md` — runtime diagnostic keyboard chords and simulated
  network lag for co-op testing.
- `docs/camera-routes.md` — generating, recording and playing camera routes
  to measure how a level streams.
- `docs/distribution.md` — producing a lean, shippable player build.
- `docs/external-projects.md` — keeping your game in its own repository.
- `tools/` — asset helpers. The Python ones (specular, normal and emissive map
  generation, model rebaking) need Python plus a small virtual environment; see
  `tools/README.md` before running them. `tools/texture-tool` is a Rust program
  that writes a whole diffuse/specular/normal/height bundle in one run — it is
  the one exception in this bundle, shipping as source rather than a prebuilt
  binary, so it needs the Rust toolchain (rustup/cargo) you install yourself,
  and without one you cannot build it; see `tools/texture-tool/README.md` and
  the toolchain note in `docs/level_design.md`.

## Shipping a finished game

Once your game is ready to hand to players, build the lean player payload from
your project root:

```bash
bin/postretro-tool dist
```

That is the whole command. No repository, no toolchain — the tool runs the
binaries already sitting in the bundle's `bin/`.

It ships the levels your mod's `maps` catalog names, so a mod with none — the
starter as it arrives — is refused.

**Name the package first.** A project's `[package].name` is where its players'
settings and saves live. If you built from the starter, change `postretro-sdk` in
`postretro.toml` to your game's own name before your first release: renaming it
afterwards strands every player's settings, `player_id` and saves under the old
name. See [docs/distribution.md](distribution.md)
for choosing levels, zipping and sending the payload, and what recipients should
expect.
