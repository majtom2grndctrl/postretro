# Building a shareable distribution

`dist` makes a self-contained Postretro folder: it includes the release engine,
launcher, content, baked levels, and baked material sidecars. You can give that
folder to someone who does not have this repository, Rust, or any build tools.

## Build on the operating system you will ship for

Postretro distributions are host-native. Run `dist` on Windows to produce a
Windows payload, and run it on Linux to produce a Linux payload. Cross
compilation is deliberately not supported by this command: `rquickjs-sys`,
`luau0-src`, and `blake3` compile native C/C++ through `cc`, so a different
target needs a matching toolchain and sysroot that this packaging workflow does
not configure or validate.

For a Windows build machine, install:

- [Rust](https://www.rust-lang.org/tools/install) with the MSVC toolchain
  (`x86_64-pc-windows-msvc` for a normal 64-bit build).
- Visual Studio Build Tools with the **Desktop development with C++** workload
  (the MSVC C++ build tools it provides are required for native dependencies).

## Make a payload

From the workspace root, run:

```powershell
cargo run -p xtask -- dist
```

The default manifest is `dist.toml`; its `[package] name` determines the folder
name. With the current manifest, the result is `dist/postretro-dev/`.

Two optional flags are available:

```powershell
cargo run -p xtask -- dist --manifest path\to\dist.toml
cargo run -p xtask -- dist --out dist\nightly
```

`--manifest` selects a different manifest. A relative manifest path is resolved
from the directory where you invoked the command. `--out` selects the parent
directory for the payload; a relative value is resolved from the workspace root.
The resulting payload root must be strictly inside this checkout's `dist/`
directory. For example, `--out dist/nightly` is valid, while `--out target/ship`,
`--out ..`, an absolute path outside the checkout's `dist/`, and `--out .` are
refused. This containment rule protects source and build directories from the
payload replacement step.

The command builds release binaries and release-bakes every selected level. A
release bake preserves the exact lighting used by the shipped game, but is
substantially slower than a development bake. Plan for it to use the machine for
some time, especially when the level set is large.

## Choose the levels and bake recipes

The shipped level set is the `maps/<name>.prl` paths written literally in the
selected mod's emitted entry-script catalog. To add or drop a shipped level,
add or remove that catalog entry, then make sure its source map normally exists
at `<mod_root>/maps/<name>.map`.

`dist.toml` identifies the package and mod root:

```toml
[package]
name = "postretro-dev"
mod_root = "content/dev"
```

Add a `[[recipes]]` entry when a selected level needs a non-default map source
or bake arguments. The `output` must exactly match the catalog literal. For
example:

```toml
[[recipes]]
output = "maps/custom.prl"
source = "content/dev/maps/custom-source.map"
args = ["--lightmap-density", "0.02"]
```

Remove the corresponding catalog entry to stop shipping a level. Remove its
recipe as well if it is no longer referenced: an orphaned recipe is an error.
For an ordinary level at the default source path, no recipe is needed. Do not
put `-o`, `--release`, `--tui`, or `--no-tui` in `args`; `dist` owns those
options so every shipped `.prl` is written to the payload as a release bake.

## If a build stops partway through

While `dist` is assembling a payload, its root contains `.dist-incomplete`.
Its presence means that folder is **not known complete**. Conversely, a payload
folder holding content and no `.dist-incomplete` was completed and swept by
`dist`.

The marker's first line is a status line: `stage 5`, `stage 6`, or `stage 7`,
identifying payload assembly, level baking, or material copying. Each following
line is a level still outstanding, written as a mod-root-relative
`maps/<name>.prl` path with `/` separators. If the marker contains only its
status line, all level bakes completed; the run stopped in the named later work
or after the final bake.

A failed replacement may also leave a sibling directory named like
`.postretro-dev.deleting-<unique suffix>`. It is the previous payload that was
renamed aside before deletion, so an error message names both that directory and
the payload root to make recovery visible. Do not send that aside directory.
Fix the reported problem and run `dist` again; the next run collects it before
assembling the replacement payload.

## Zip and send it

After a successful build, archive the payload folder itself, preserving its
top-level directory. On Windows, for the default package:

```powershell
Compress-Archive -Path dist\postretro-dev -DestinationPath postretro-dev.zip
```

Send the resulting ZIP. The recipient should extract it completely, keep the
folder layout intact, and start the launcher inside the extracted folder (for
the default package, `postretro-dev.bat`). The launcher sets the working
directory correctly, so it can be started from a shortcut or by double-clicking.

## What recipients should expect

The Windows engine binary is unsigned. Windows may show **"Windows protected
your PC"** on first launch. To proceed, choose **More info**, then **Run
anyway**. This is expected for this build; only run a payload received from a
source you trust.

The game requires a graphics adapter that supports DirectX 12 or Vulkan. On its
first run it writes editable player settings under
`%APPDATA%\postretro\config\settings.toml`.

On Windows there can be a brief white flash when the window is created, before
the game splash is first presented. This is a documented cosmetic startup
artifact; the window intentionally remains visible because hiding it before the
first frame can prevent Windows from delivering the redraw that starts boot.
