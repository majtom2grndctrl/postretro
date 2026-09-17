# Modding with the Postretro SDK

The **SDK bundle** is a self-contained, content-complete modding kit:
everything you need to **play** a Postretro mod *and* keep editing it, without
cloning this repository or installing Rust. The maps ship baked, so it runs on
arrival; the sources and compilers ship too, so you can change anything and
rebake. It's produced by `sdk-dist`, a sibling of the `dist` command described
in [docs/distribution.md](distribution.md).

## SDK bundle vs. player payload

`dist` and `sdk-dist` both assemble a self-contained folder, but for
different audiences:

| | `dist` (player payload) | `sdk-dist` (SDK bundle) |
|---|---|---|
| Audience | Someone who plays the game | Someone who plays **and** authors it |
| Playable on arrival | Yes | Yes (maps ship baked) |
| Engine build | Release | **Debug**, built with `--features dev-tools` |
| Levels | Baked `.prl` only | Baked `.prl` **and** source `.map` + the compiler |
| Scripts | Baked `.js`/`.luau` only | Baked **and** source `.ts`/`.luau` + the compiler |
| TrenchBroom FGD, TS typings, docs, tools | Not included | Included |
| Folder name | `<package name>` | `<package name>-sdk` |
| Size | Lean | Larger (debug engine + sources + tools) |

Both are playable folders. `dist` is the lean build to hand to players. Use
`sdk-dist` when the recipient (including future-you) should also be able to
keep editing the mod — it's a superset, at the cost of size.

## Producing a bundle

From the workspace root:

```bash
cargo run -p xtask -- sdk-dist
```

This uses the same flags as `dist`, with the same resolution rules:

```bash
cargo run -p xtask -- sdk-dist --manifest path/to/dist.toml
cargo run -p xtask -- sdk-dist --out dist/nightly
```

`--manifest` defaults to `dist.toml` and selects the package name and mod
root; a relative path resolves from the directory you invoked the command
in. `--out` defaults to `<workspace>/dist` and selects the parent directory
for the bundle; a relative value resolves from the workspace root, and the
result must land strictly inside this checkout's `dist/` directory (the same
containment rule `dist` enforces).

The bundle is written to `<out>/<package name>-sdk`. With the default
manifest, that's `dist/postretro-dev-sdk/`.

**The bundle is host-native.** `sdk-dist` builds the engine and tool binaries
on the machine you run it on, the same way `dist` does — run it on the OS you
want the bundle to run on. See "Build on the operating system you will ship
for" in [docs/distribution.md](distribution.md) for why cross-compilation
isn't supported.

**It bakes the levels *and* ships the sources.** `sdk-dist` runs the same
level and material bakes as `dist`, so the bundle is playable the moment it's
unpacked — but it does *not* run the player payload's source-exclusion filter
over the mod tree, so the `.map`/`.ts` sources ship beside the freshly baked
`.prl`/`.js`, together with the compilers needed to rebake them. That's the
content-complete part: play now, or edit and rebake. If a build stops partway
through, the bundle root carries the same `.dist-incomplete` marker `dist`
uses — see "If a build stops partway through" in
[docs/distribution.md](distribution.md) for how to read and recover from it.

## Bundle layout

```
<package name>-sdk/
  postretro[.exe]            authoring engine: debug build, --features dev-tools
  bin/
    prl-build[.exe]          level compiler (release build)
    scripts-build[.exe]      TypeScript -> JS compiler (release build)
  sdk/                       TrenchBroom FGD + GameConfig + models, lib/, types/,
                             templates/, behaviors/, type-tests/
  docs/                      level_design.md, scripting-reference.md,
                             weapon-mounts.md, diagnostics.md, distribution.md
  tools/                     Python asset helpers (see tools/README.md)
  content/base/              UI descriptors + splash, copied verbatim
  content/<mod>/             your mod's tree: SOURCE .map/.ts beside the freshly
                             baked maps/*.prl and the emitted start-script.js
  baked/materials/           .prm material sidecars (so the baked maps render)
  README.md                  generated quickstart for this bundle
```

`content/<mod>/` ships both halves: the baked outputs the engine loads to play
(`maps/*.prl`, the emitted `start-script.js`) and the sources you keep editing
(`maps/*.map`, `scripts/`, `start-script.ts`/`.luau`, models, other assets).
Only genuinely regenerable junk is dropped — `.build-caches`, `maps/autosave/`,
and any *stale* committed `.prl`/`.js` (the bake produces fresh ones).

## Play it first

The maps are already baked, so before editing anything you can just run it.
From the bundle root (cwd must be the bundle root so content paths resolve):

```bash
./postretro
```

## The authoring loop

1. **Author a level in TrenchBroom.** Load `sdk/TrenchBroom/postretro.fgd` as
   the game definition and point the texture path at
   `content/<mod>/textures/`. See `docs/level_design.md` in the bundle for
   entity and lighting reference.
2. **Compile it:**
   ```bash
   bin/prl-build maps/<name>.map -o content/<mod>/maps/<name>.prl
   ```
3. **Author scripts in TypeScript** against `sdk/types/` and `sdk/lib/`.
4. **Run the engine from the bundle root:**
   ```bash
   ./postretro
   ```
   (cwd must be the bundle root so content paths resolve). The debug engine
   auto-compiles any `.ts` with a same-stem `.js` sibling at startup, and
   hot-reloads script edits while it runs — just save the file. If you'd
   rather compile a script by hand instead of relying on auto-compile:
   ```bash
   bin/scripts-build --in content/<mod>/start-script.ts --out content/<mod>/start-script.js
   ```
   Because it's a `--features dev-tools` build, the egui inspector/debug UI
   is also available; see `docs/diagnostics.md` in the bundle for the
   diagnostic keyboard chords.

## Why the SDK ships a debug engine

This is the one place the SDK engine intentionally differs from what
players run, and it's worth understanding why.

TypeScript startup auto-compile and TS hot reload are gated on
`#[cfg(debug_assertions)]` — a **debug** build — not on the `dev-tools`
feature. `dev-tools` only adds the egui inspector UI on top. A release
engine holds no TypeScript compiler at all: every script it runs must
already be compiled ahead of time (that's what `dist` does when it packages
a player payload). A modder needs both halves — debug for the edit-and-see
iteration loop, `dev-tools` for the inspector — so the SDK bundle's engine is
built as `cargo build -p postretro --bin postretro --features dev-tools`,
without `--release`. See `context/lib/scripting.md` §8 for the underlying
compilation-tooling contract.

If you want to type-check your scripts, run `tsc --noEmit` yourself;
`scripts-build` does not type-check.

## Docs and tools in the bundle

- `docs/level_design.md` — TrenchBroom entities, lights, fog volumes,
  textures, map sealing.
- `docs/scripting-reference.md` — the scripting API surface.
- `docs/weapon-mounts.md` — rigid weapon mount authoring workflow.
- `docs/diagnostics.md` — runtime diagnostic keyboard chords.
- `docs/distribution.md` — this same guide, for when you're ready to ship a
  player build.
- `tools/` — Python asset helpers (specular/normal/emissive map generation,
  model rebaking, etc.). These need Python plus a small virtual environment;
  see `tools/README.md`'s "Python tool setup" section in the bundle before
  running them.

## Shipping a finished mod

The SDK bundle is playable, but it's heavy — a debug engine, sources, and
developer tooling. Once your mod is ready to hand to players, build the lean
player payload instead (release engine, baked content only, no sources or
tools):

```bash
cargo run -p xtask -- dist
```

See [docs/distribution.md](distribution.md) for the full player-payload
workflow (choosing levels, zipping and sending the payload, what recipients
should expect).
