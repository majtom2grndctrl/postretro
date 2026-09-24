# Building a shareable distribution

`postretro-tool dist` makes a self-contained Postretro folder: the release
engine, a launcher, your game's content, baked levels, and baked material
sidecars. You can hand that folder to someone who has no repository, no Rust
toolchain, and no build tools at all.

The tool itself needs none of those things either. It compiles nothing — it runs
the engine and compiler binaries that were built for it, and it finds your
project by looking for a `postretro.toml` marker file, the way `cargo` finds
`Cargo.toml`.

## Where the tool is

In an SDK bundle, the tool is `bin/postretro-tool` (`bin\postretro-tool.exe` on
Windows). Every command below is written from a bundle root, so the paths match
what you actually have.

You can run that same executable from anywhere. It answers two questions
separately, and it is worth knowing which is which:

- **Which project am I building?** Answered by walking up from the directory you
  ran the command in, looking for `postretro.toml`.
- **Where are the engine and the compilers?** Answered by looking beside the
  tool's own executable — so a bundle's `bin/postretro-tool` always finds the
  bundle's own `bin/prl-build` and `bin/postretro-release`, whatever directory
  you ran it from.

That split is what lets one installed bundle build a game whose sources live
somewhere else entirely. See [docs/external-projects.md](external-projects.md).

## Player payload vs. SDK bundle

`dist` (this doc) produces a lean **player payload**: a release engine plus
baked content, meant to be run, not edited. If you instead want to hand someone
the ability to keep editing a mod — TrenchBroom files, script sources, the
level and script compilers, and a debug engine that hot-reloads — use
`sdk-dist`, the **content-complete SDK bundle**. It bakes the same content (so
it is still playable out of the box) and additionally ships the sources and
tools:

```bash
bin/postretro-tool sdk-dist
```

See [docs/modding.md](modding.md) for the SDK bundle's contents and authoring
workflow.

## Build on the operating system you will ship for

Postretro distributions are host-native. The binaries a payload carries are
copies of the ones beside the tool, and those were compiled for one operating
system and link native C/C++ dependencies. Run `dist` on Windows to produce a
Windows payload; run it on Linux to produce a Linux payload. To ship for another
platform, produce the bundle on that platform.

## The project marker

`postretro.toml` marks a directory as a Postretro project. Its presence is what
lets the tool find the tree it is standing in:

```toml
[package]
name = "my-game"
mod = "base"
```

`name` is the payload's folder name. It must be a single path component — no
slashes, no `..`.

`mod` is your mod's name: the directory under the project's `content/` that
holds your authored content, so `mod = "base"` means `content/base`. It is a
name, not a path — no slashes, no `..` — because every mod lives directly under
`content/`. That placement is not a style rule: the engine finds the baked
material sidecars by walking up two levels from the mod it mounted, which lands
at the project root only for `content/<mod>`.

An SDK bundle ships its own `postretro.toml` at the bundle root, already
correct, so the bundle is a project you can build from immediately.

## Make a payload

From the project root — in a bundle, the bundle root:

```bash
bin/postretro-tool dist
```

The payload lands at `dist/<name>/`. With the manifest above, that is
`dist/my-game/`.

Four optional flags:

```bash
bin/postretro-tool dist --project path/to/game
bin/postretro-tool dist --manifest path/to/postretro.toml
bin/postretro-tool dist --out dist/nightly
bin/postretro-tool dist --install-root path/to/engine-install
```

`--project` names the project directory instead of searching for one, and
`--manifest` names its marker file; give one or the other, not both. `--out`
selects the parent directory the payload folder is created in. All three resolve
a relative path from the directory you ran the command in.

`--install-root` names the engine install the payload's `core/` tree is copied
from — the engine's own UI descriptors, boot splash, and font licences. You do
not normally pass it: the tool derives the install from its own location, which
is this bundle's root. Pass it if the tool reports `engine-owned tree `core/` not
found`, which means it is running from somewhere that is not an install. The
install and your project are separate lookups in both directions: your game never
supplies engine assets, and the install never supplies your game.

**The resulting payload root must lie strictly inside the project's own `dist/`
directory.** `--out dist/nightly` is fine; `--out build`, `--out ..`, `--out .`,
and any absolute path outside the project's `dist/` are refused. A payload run
deletes and rewrites that folder, so the rule keeps the blast radius inside one
directory the project owns. On top of containment, the tool refuses to delete a
non-empty folder that holds neither the `.dist-incomplete` marker nor an engine
binary at its top level — so a mistyped path under `dist/` cannot destroy a
directory no distribution run produced.

The run release-bakes every shipped level. A release bake preserves the exact
lighting the shipped game uses and is substantially slower than a development
bake. Expect it to occupy the machine for a while, especially with a large level
set.

## Payload layout

```
dist/<name>/
  postretro[.exe]          the release engine
  <name>.{bat,sh}          launcher: pins the working directory, mounts the game
  core/                    engine-owned assets: UI descriptors, splash, licences
  content/base/            your game's content, with baked levels and entry script
  baked/materials/         .prm material sidecars
```

Two of those names are load-bearing.

`core/` is what the engine owns and a game never replaces — the built-in UI
descriptors, the boot splash, the font licences. It sits outside `content/`
precisely so that mounting a game never redirects it.

`content/base` is your game's own tree — the mod this example's
`postretro.toml` declares. A payload publishes `content/<mod>` under whatever
mod your project names, keeping that name rather than renaming it, and `base` is
the recommended convention for a game. The launcher mounts your declared mod for
you.

A payload is correct only as a whole tree with the working directory pinned to
its root. The launcher does that pinning itself, so it works from a shortcut or
a double-click. Do not flatten the folder, and do not move `core/`.

## Choose the levels and bake recipes

The shipped level set is the `maps/<name>.prl` paths written **literally** in
your mod's emitted entry-script catalog. To add or drop a shipped level, add or
remove that catalog entry, then make sure its source map exists at
`content/<mod>/maps/<name>.map`.

A path the script assembles at runtime rather than writing as a literal ships
nothing and reports nothing — no stage ever saw the level, so the recipient gets
a menu entry that loads nothing. Write catalog paths as plain string literals.

Add a `[[recipes]]` entry to `postretro.toml` when a level needs a non-default
map source or extra bake arguments. The `output` must match the catalog literal
exactly and must start with `maps/`:

```toml
[[recipes]]
output = "maps/custom.prl"
source = "content/base/maps/custom-source.map"
args = ["--lightmap-density", "0.02"]
```

`source` is project-relative. A recipe that matches no catalog literal is
reported as an orphan rather than passing silently, so remove a recipe when you
stop shipping its level.

Some arguments are refused in `args` because the tool supplies them itself:

| Refused in `args` | Why |
|---|---|
| `-o`, `--release`, `--tui`, `--no-tui` | The tool decides what kind of bake a distribution runs. `--release` is the only shippable one. |
| `--baked-root`, `--cache-dir` | These two must stay consistent between the compiler and the engine. The compiler reads the last occurrence of each, so a recipe naming one would override the tool's and silently send the sidecars somewhere the engine does not look. |

`--lightmap-density` must be written as two tokens (`"--lightmap-density",
"0.02"`, never `"--lightmap-density=0.02"`), and its value must be a finite
number greater than zero. The tool bakes levels one at a time, ordered by
ascending lightmap density, so an over-large bake fails early.

## If a build stops partway through

While a run is working, the payload root contains `.dist-incomplete`. Its
presence means that folder is **not known complete**. A payload folder holding
content and no `.dist-incomplete` was finished and swept.

The marker's first line is a status line — `stage 5`, `stage 6`, or `stage 7`,
meaning payload assembly, level baking, or material copying. Each following line
is a level still outstanding, written as a mod-root-relative `maps/<name>.prl`
path with `/` separators. A marker holding only its status line means every level
baked and the run stopped in the named later work.

A failed replacement may also leave a sibling directory named like
`.my-game.deleting-<number>`. That is the previous payload, renamed aside before
deletion. Do not send it. Fix the reported problem and run `dist` again; the next
run collects it before assembling the replacement.

## Zip and send it

Archive the payload folder itself, keeping its top-level directory. On Windows,
for the manifest above:

```powershell
Compress-Archive -Path dist\my-game -DestinationPath my-game.zip
```

Send the ZIP. The recipient extracts it completely, keeps the folder layout
intact, and starts the launcher inside the extracted folder (`my-game.bat` on
Windows, `./my-game.sh` elsewhere). The launcher sets the working directory
correctly, so it can be started from a shortcut or by double-clicking.

## What recipients should expect

The Windows engine binary is unsigned. Windows may show **"Windows protected
your PC"** on first launch. To proceed, choose **More info**, then **Run
anyway**. This is expected for this build; only run a payload from a source you
trust.

The game requires a graphics adapter that supports DirectX 12 or Vulkan. On its
first run it writes editable player settings under
`%APPDATA%\postretro\config\settings.toml`.

On Windows there can be a brief white flash when the window is created, before
the splash is first presented. This is a cosmetic startup artifact; the window
intentionally stays visible, because hiding it before the first frame can stop
Windows delivering the redraw that starts boot.
