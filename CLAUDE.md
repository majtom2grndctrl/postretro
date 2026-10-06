# Postretro

Retro-style FPS engine (Doom/Quake boomer shooter, cyberpunk aesthetic, modder-friendly). Rust + wgpu core.

## Start here — required

Read `context/lib/index.md` before doing anything else. It routes to the right context docs for your task. Do not skip this for planning, Q&A, or implementation — the index is short and the routing matters.

## Key constraints

- **No `unsafe` without approval.** See `context/lib/development_guide.md` §3.5.
- **Renderer owns GPU.** All wgpu calls live in the renderer module.
- **Frame order:** Input → Game logic → Audio → Render → Present.

## Content tooling

Authoring runs, asset bakes, the weapon-mount solver, and distribution assembly live in `postretro-tool`, which compiles nothing and ships inside SDK bundles. It finds its project by walking up for `postretro.toml`, and its helper binaries by looking beside its own executable — so build what it needs first. Its engine-owned trees (`core/`, `sdk/`, `docs/`, `tools/`) resolve under the **install root**, which a checkout build in `target/` cannot derive, so pass `--install-root .` from the workspace root for `run`, `dist` and `sdk-dist`.

```bash
cargo run -p postretro-tool -- --help                     # full usage
cargo run -p postretro-tool -- run --install-root . maps/campaign-test.prl
cargo run -p postretro-tool -- bake-model-textures <scene.gltf>
cargo run -p postretro-tool -- solve-weapon-mount <skeleton.gltf> --weapon <weapon.gltf> --check
cargo run -p postretro-tool -- mint-identity dev
```

Human-facing docs live in `docs/` and are copied verbatim into every SDK bundle. Every command there must be runnable by someone holding only a bundle — `cargo run -p xtask -- …` belongs in `context/`, never in `docs/`.
