# Keeping your game in its own repository

Your game does not have to live inside a PostRetro installation. Its maps,
textures, models and scripts can sit in a version-controlled repository of your
own, with the engine and the tools installed separately and upgraded
independently. This guide is that workflow end to end.

It assumes you have an SDK bundle unpacked somewhere — that is the installation.
See [docs/modding.md](modding.md) for what a bundle contains.

## Two things, and which one owns what

| | The install (an SDK bundle) | Your project (your repository) |
|---|---|---|
| What it is | Engine binaries, compilers, `postretro-tool`, the SDK, the docs | Your game's content and the marker that names it |
| Who owns it | PostRetro | You |
| In version control | No — it is host-native, and you replace it wholesale to upgrade | Yes |
| How you upgrade it | Unpack a newer bundle | `git pull` |

A **project** is any directory containing a `postretro.toml` marker file. That
file is the whole of the mechanism: it is what tells the tool where your game
starts.

## Two lookups, two anchors

`postretro-tool` answers two questions independently, and understanding the split
is what makes this workflow obvious rather than fiddly:

- **Which project?** It walks up from the directory you ran the command in,
  looking for `postretro.toml`, the way `cargo` finds `Cargo.toml`. The first one
  it finds wins, so nested projects resolve to the innermost.
- **Which binaries?** It looks beside its own executable. An install's
  `bin/postretro-tool` always drives that install's `bin/prl-build` and its
  engine, whatever directory you invoked it from.

So you run the install's tool from inside your repository, and each half anchors
where it should:

```bash
/opt/postretro-sdk/bin/postretro-tool run
```

Put that directory on your `PATH` and the command becomes `postretro-tool run`.
Nothing about the project changes when you move or replace the install.

## Laying out the project

```
my-game/
  postretro.toml        the project marker
  content/base/         your game
    maps/               .map sources and the .prl the compiler writes
    textures/<collection>/
    models/
    scripts/
    start-script.ts
  baked/materials/      generated: .prm material sidecars
  .build-caches/        generated: disposable compiler scratch
  dist/                 generated: payloads and bundles you build
```

The marker:

```toml
[package]
name = "my-game"
mod = "base"
```

**`mod` is a name, not a path.** It names the directory under `content/`, so
`mod = "base"` is `content/base`. `base` is the conventional choice, and the
distribution publishes it at the same `content/base`, so every path stays
identical between development and a shipped payload.

Every mod lives directly under `content/` for a reason. The engine locates your
baked material sidecars by walking up two levels from the mod it mounted, so
`content/base` resolves `baked/materials` at your project root.

Suggested `.gitignore`:

```gitignore
/baked/
/.build-caches/
/dist/
```

`baked/materials/` is runtime-required output, but it is entirely regenerable
from your PNG sources, and its files are content-addressed blobs that churn on
every texture edit. `.build-caches/` is disposable at any time. `dist/` holds
build products.

## Engine assets stay in the install

Your repository holds your game and nothing else. `core/` — the built-in UI
descriptors, the boot splash, the font licences — belongs to the engine, and the
tool takes it from your install every time: `postretro-tool run` passes the
engine `--core-root <install>/core`, and `dist` and `sdk-dist` copy that same
tree into what they build. Nothing is ever read from a `core/` in your project,
so upgrading the install upgrades those assets with no step on your side.

If you launch the engine by hand rather than through the tool, pass
`--core-root` yourself — pointed at your install's `core/`, as an absolute path
or relative to wherever you launch from. Without it the engine still boots, but
the pause menu, the frontend menu, the on-screen keyboard and the boot splash
are simply absent, each announced by a warning in the log and nothing else.

## The authoring loop

From anywhere inside the project:

```bash
postretro-tool run
```

That launches the install's authoring engine with the working directory pinned to
your project root, your content mounted, and the baked materials root already
pointed at your `baked/`. Anything the tool does not recognize forwards straight
to the engine, so `postretro-tool run maps/arena.prl` opens that level directly.
A level path is relative to your mod's folder, not the project root, so there is
no `content/base/` to type. Engine flags you pass yourself win over the tool's
defaults rather than being shadowed.

To recompile one level, call the compiler directly:

```bash
prl-build content/base/maps/arena.map \
  --baked-root baked --cache-dir .build-caches/prl-cache \
  -o content/base/maps/arena.prl
```

This is the one command in the loop where you type those two paths yourself,
because the tool has no single-level build of its own — it compiles levels only
as part of `dist` and `sdk-dist`, where it supplies both flags. Wrap it in a
shell script or a task runner in your repository and you will never type them
twice.

Scripts need no build step during development: the authoring engine compiles
`.ts` at startup and hot-reloads edits while it runs. To compile one by hand:

```bash
scripts-build --in content/base/start-script.ts --out content/base/start-script.js
```

## The one mistake that does not fail

`--baked-root` names **the directory that contains `materials/`** — `baked`, not
`baked/materials`. Both the compiler and the engine take the flag, and both mean
the same thing by it.

If the compiler and the engine end up pointed at different directories, nothing
reports an error. The compiler writes its `.prm` sidecars where it was told; the
engine looks where *it* was told, finds nothing, substitutes a placeholder
texture for every world material, and logs a warning. The game runs. Every
surface is flat grey. It reads as a broken engine, not as a mistyped path, and
that is exactly why it is worth understanding rather than memorising.

This is the whole reason the tooling exists in the shape it does:

- `postretro-tool run` passes the engine its `--baked-root` for you, so the
  engine reads your project's `baked/`.
- `dist` and `sdk-dist` pass the compiler `--baked-root` and `--cache-dir`, then
  copy the finished materials tree into the payload, where the engine's own
  derivation lands on it. They also refuse a `[[recipes]]` entry in
  `postretro.toml` that names either flag — the compiler honours the last
  occurrence, so a recipe naming one would quietly override the tool's and
  reintroduce the same failure.
- The only place you supply them is a direct `prl-build` call, where they must
  match the project's own `baked/` and `.build-caches/prl-cache`.

`--cache-dir` travels with it for a smaller reason with the same shape. Without
it, the compiler's fallback puts its stage cache next to the map it is
compiling — a growing directory of scratch files landing in `content/base/maps/`,
the one directory you are most likely to commit. The cache is disposable and
belongs beside the project, not inside its content.

## Assets and identity

Model textures bake automatically for models placed in a map, and automatically
for every glTF under your content tree during a distribution run. To prepare one
model's sidecars on their own:

```bash
postretro-tool bake-model-textures content/base/models/rifle/scene.gltf
```

It finds your project the same way everything else does and writes into your
project's `baked/materials/`.

Persisted and replicated script state needs a durable identity ledger at
`content/<mod>/identity.json`. After adding a slot:

```bash
postretro-tool mint-identity base
```

Commit the updated file with your content. See `docs/scripting-reference.md`.

## Building distributions

From the project root, exactly as from a bundle:

```bash
postretro-tool dist
```

The payload lands at `dist/<name>/`, carrying your content published at
`content/base` and the install's release engine. `postretro-tool sdk-dist`
produces a content-complete bundle beside it at `dist/<name>-sdk/`.

Both refuse to write outside your project's own `dist/`, and both refuse to
delete a non-empty directory there that no distribution run produced. See
[docs/distribution.md](distribution.md) for the full rules, the level-selection
mechanism, and what to send a recipient.

## Upgrading the install

Unpack the newer bundle and point at it — the engine assets come from there, so
there is nothing to re-copy. Delete `baked/materials/` if the release notes say
the material format changed; the next compile repopulates it. Your repository is otherwise untouched: it holds
content and one small TOML file, and neither has a build machine's paths baked
into it.
