# Postretro

Retro-style FPS engine (Doom/Quake boomer shooter, cyberpunk aesthetic, modder-friendly). Rust + wgpu core.

## Start here — required

Read `context/lib/index.md` before doing anything else. It routes to the right context docs for your task. Do not skip this for planning, Q&A, or implementation — the index is short and the routing matters.

## Key constraints

- **No `unsafe` without approval.** See `context/lib/development_guide.md` §3.5.
- **Renderer owns GPU.** All wgpu calls live in the renderer module.
- **Frame order:** Input → Game logic → Audio → Render → Present.

## Build and run

```bash
cargo run -p postretro                        # engine (debug)
cargo run -p postretro -- assets/maps/test.prl  # engine with a PRL map
cargo run -p postretro-level-compiler -- input.map -o output.prl  # compile a level (binary: prl-build)
cargo run --release -p postretro              # optimized engine build
RUST_LOG=info cargo run -p postretro          # with logging
POSTRETRO_GPU_TIMING=1 cargo run -p postretro # log per-pass GPU time (requires TIMESTAMP_QUERY adapter support)
```
