# Postretro

Retro-style FPS engine (Doom/Quake boomer shooter, cyberpunk aesthetic, modder-friendly). Rust + wgpu core.

## Start here — required

Read `context/lib/index.md` before doing anything else. It routes to the right context docs for your task. Do not skip this for planning, Q&A, or implementation — the index is short and the routing matters.

## Key constraints

- **No `unsafe` without approval.** See `context/lib/development_guide.md` §3.5.
- **Renderer owns GPU.** All wgpu calls live in the renderer module.
- **Frame order:** Input → Game logic → Audio → Render → Present.

## Build and run

xtask syntax: `cargo run -p xtask -- run [cargo-run flags...] -- [postretro args...]`.
Simple engine args still work as `cargo run -p xtask -- run [postretro args...]`.

```bash
cargo run -p xtask -- run                     # canonical development engine launch
cargo run -p xtask -- run content/dev/maps/campaign-test.prl  # dev launch with a PRL map
cargo run -p xtask -- run --features dev-tools -- content/dev/maps/campaign-test.prl  # dev-tools launch
cargo run -p xtask -- run --release -- content/dev/maps/campaign-test.prl  # optimized xtask launch
cargo run -p postretro                        # lower-level engine run; assumes scripts-build is already built
cargo run -p postretro-level-compiler -- content/dev/maps/input.map -o content/dev/maps/output.prl  # compile a level (binary: prl-build)
cargo run --release -p postretro              # optimized engine build
RUST_LOG=info cargo run -p xtask -- run       # dev launch with logging
POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- run # log per-pass GPU time (requires TIMESTAMP_QUERY adapter support)
cargo run -p xtask -- dist                    # build release binaries, then assemble a player payload
cargo run -p xtask -- sdk-dist                # same, for the content-complete modder SDK bundle
```

A distribution publishes the developer's mod under the mod root their `postretro.toml` declares — it keeps that name rather than renaming it. `content/base` is the recommended convention for a downstream game; this workspace's own mod root is `content/dev`, so a payload or SDK bundle of it publishes at `content/dev`. Engine-owned assets are `core/`.

## Content tooling

Authoring runs, asset bakes, the weapon-mount solver, and distribution assembly live in `postretro-tool`, which compiles nothing and ships inside SDK bundles. It finds its project by walking up for `postretro.toml`, and its helper binaries by looking beside its own executable — so build what it needs first. Its engine-owned trees (`core/`, `sdk/`, `docs/`, `tools/`) resolve under the **install root**, which a checkout build in `target/` cannot derive, so pass `--install-root .` from the workspace root for `run`, `dist` and `sdk-dist`.

```bash
cargo run -p postretro-tool -- --help                     # full usage
cargo run -p postretro-tool -- run --install-root . maps/campaign-test.prl
cargo run -p postretro-tool -- bake-model-textures <scene.gltf>
cargo run -p postretro-tool -- solve-weapon-mount <skeleton.gltf> --weapon <weapon.gltf> --check
cargo run -p postretro-tool -- mint-identity dev
```

Human-facing docs live in `docs/` and are copied verbatim into every SDK bundle. Every command there must be runnable by someone holding only a bundle — `cargo run -p xtask -- …` belongs here, never in `docs/`.
