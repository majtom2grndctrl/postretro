# dist-packaging — research notes

Findings that shaped the spec. Not decisions; the spec holds those.

## What a release engine actually reads from disk

Grounded by reading the release-gated paths, not the dev ones.

| Artifact | Path at runtime | Source | Notes |
|---|---|---|---|
| Level | `<map arg>` or catalog `path` joined to content root | `prl-build` | `.prl` is gitignored; zero in the tree |
| Material mips | `<content_root>/../../baked/materials/<hex>.prm` | `prl-build` | `derive_prm_root_dev_layout` (`crates/postretro/src/startup/worker.rs`) |
| Mod entry script | `<content_root>/start-script.js` | `scripts-build` | `.js` gitignored |
| UI descriptors | `content/base/ui/{hud,pauseMenu,frontendMenu,keyboard}.json` | committed | `ui_asset_path` in `crates/ui/src/tree_asset.rs` — cwd-relative, hardcoded `content/base/ui` |
| Splash | `content/base/textures/splash/postretro-ascii-art.png` | committed | `SplashSource::base_path` |
| Models, textures, sounds | `<content_root>/<handle>` | committed | content-root join |
| Fonts | none | `include_bytes!` | `crates/ui/src/text.rs` compiles Inter + JetBrains Mono into the binary |

## Level scripts are embedded, not shipped

`compile_worldspawn_data_script` (`crates/level-compiler/src/main.rs`) reads the worldspawn
`data_script` KVP, shells out to `scripts-build`, and packs the output into the PRL
`DataScriptSection` (`compiled_bytes` + `source_path`). At load, `run_data_script` evaluates
`compiled_bytes` — it never reopens the source file.

Consequence: nothing under `<mod_root>/scripts/` is read by a release engine. The recursive
`.ts` scan that suggests otherwise (`scan_and_compile_stale_ts`) is `#[cfg(debug_assertions)]`,
as is `start_watcher`; both no-op in release (`crates/scripting-core/src/runtime/core.rs` has
`let _ = script_root;` in the `not(debug_assertions)` arm).

`DataScriptSection.source_path` is the **absolute** path captured on the build machine. It is a
diagnostic label (VM script name, warning text), so it leaks the builder's directory layout into
shipped `.prl` files. Cosmetic, not functional.

## Release cannot compile TypeScript

`compile_start_script` and the whole compile scan are `#[cfg(debug_assertions)]`
(`crates/scripting-core/src/runtime/compile.rs`). `mod_init` mirrors the gate: `has_ts_or_js_source`
evaluates `ts_path.is_file()` in debug and literal `false` in release
(`crates/scripting-core/src/runtime/mod_init.rs`).

`run_script_file` accepts a `.ts` extension and feeds the raw bytes to QuickJS, which parses it as
plain JS — so a stray `.ts` in a shipped payload would be a syntax error, not a compile.

The sidecar split is deliberate: "The sidecar exists so the `postretro` *runtime* binary never links
`swc_*` crates (which add meaningful binary size)" — `crates/script-compiler/src/main.rs`.

## The map set cannot be derived from the maps directory

`content/dev/maps/` holds 30 `.map` files. `mapCatalog` in `content/dev/scripts/frontend-menu.ts`
lists 13 `.prl` entries, five of which have no matching `.map`:

- `campaign-test--0.02-mtex.prl`
- `occlusion-test--0.01-mtex.prl`
- `occlusion-test--0.015-mtex.prl`
- `occlusion-test--0.02-mtex.prl`
- `occlusion-test--shadow-resolution-test.prl`

They are the same sources compiled at different `--lightmap-density` values. Output name and
compiler flags are therefore both build inputs that exist nowhere on disk.

## prl-build surface used by dist

From `help_text()` in `crates/level-compiler/src/main.rs`:

- `-o <output.prl>` — output path.
- `--release` — "Produce a shippable map: exact lighting, cache bypassed (implies `--no-cache`). The
  interactive default is a fast warm build with approximate indirect lighting; ship only `--release`
  artifacts."
- `--no-tui` — line-oriented progress; required for non-interactive runs.
- `--lightmap-density <METERS>`, `--sh-probe-spacing <METERS>`, etc. — the per-variant knobs.

`.prm` output root is **not** a flag. `resolve_prm_root_via_cargo` walks for a `Cargo.toml` ancestor
of the map source and lands on `<workspace>/baked/materials`. Dist bakes from workspace sources, so
`.prm` files land there and must be copied into the payload afterward.

Unreferenced `.prm` files in that tree are inert: the runtime resolves a material to
`blake3(baseColor PNG bytes)` and looks the file up by key (`cache_filename_for_key`), so extras
cost payload size only.

## scripts-build surface

`scripts-build --in <INPUT.ts> --out <OUTPUT.js>` (`crates/script-compiler/src/main.rs`). Bundles
relative imports, strips TS syntax, drops bare specifiers. `content/dev/start-script.ts` imports
`../../sdk/behaviors/reference/entities` — outside the content root — so bundling must run from the
workspace, and the emitted `.js` is self-contained.

## Launch-path resolution

- `resolve_content_root` (`crates/postretro/src/startup/session.rs`): `--mod <dir>`, else
  `--content-root <dir>`, else the map path's grandparent.
- `content_root_from_map(None)` falls back to `DEFAULT_MAP_PATH` = `content/dev/maps/campaign-test.prl`,
  so a no-argument launch resolves the content root to `content/dev`.
- No map argument means no boot map; the mod's `frontend.menuTree` drives the first screen.
- Everything is joined against the process cwd. Explorer sets cwd to the containing directory for
  both a double-clicked `.exe` and a double-clicked `.bat`.

## Windows

Already exercised: `context/lib/boot_sequence.md` documents the Win32 white-flash and the confirmed
hidden-window boot hang. No `cfg(unix)` outside tests; `scripts_build_in_dir` already probes
`scripts-build.exe`.

Cross-compiling from Linux is the obstacle, not the code. `rquickjs-sys` (C), `luau0-src` 0.18 (C++,
vendored via `mlua`'s `luau` feature) and `blake3` all build native sources through `cc`. Targeting
`x86_64-pc-windows-msvc` from Linux needs an MSVC sysroot; vendored C++ through that path is where
such setups usually fail. `x86_64-pc-windows-gnu` avoids MSVC but puts Luau and wgpu's DX12 backend
on an untested target.

No TLS stack in the tree — `renet`/`renet_netcode` over plain UDP, no `rustls`/`ring`.

## xtask conventions

`try_main` dispatches on the first argument; `crate_graph::run(args)` is the precedent for a command
whose body lives in its own module. Shared helpers: `workspace_root()`, `run_checked(&mut Command,
label)`, `status_code(...)`, `build_scripts_sidecar(...)`.

`crates/xtask/src/main.rs` is 2190 lines. The dist command follows the `crate_graph` precedent, so
main.rs gains a dispatch arm and help lines only — no split task is owed by the split-before-extend
rule. The file's size is a standing concern for whoever next adds a command body inline.

## The bundle carries every catalog path (measured)

Ran the real sidecar, not a reasoned argument:

```
cargo build -p postretro-script-compiler --bin scripts-build
./target/debug/scripts-build --in content/dev/start-script.ts --out /tmp/start-script.js
grep -o 'maps/[A-Za-z0-9._-]*\.prl' /tmp/start-script.js | sort -u
```

48 KB bundle, all 13 `mapCatalog` paths present as string literals.

The literals originate in the catalog's own `path` field (`defineMapCatalog` in
`content/dev/scripts/frontend-menu.ts`), reachable from `start-script.ts` via
`import { mapCatalog }` and `maps: mapCatalog` in `defineMod`. They do **not** come from per-map
script imports, so a level with no data script of its own still appears.

Residual weakness, unchanged by which side drives: a path assembled at runtime
(`"maps/" + id + ".prl"`) is invisible to a textual scan. No catalog entry does this today — all 13
are literals.
