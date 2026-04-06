# Postretro

Retro-style FPS engine (Doom/Quake boomer shooter, cyberpunk aesthetic, modder-friendly). Rust + wgpu core.

## Start here

Load `context/lib/index.md` — it routes to the right docs for your task.

## Key constraints

- **No `unsafe` without approval.** See `context/lib/development_guide.md` §3.5.
- **Renderer owns GPU.** All wgpu calls live in the renderer module.
- **Frame order:** Input → Game logic → Audio → Render → Present.
- **glam pinned to 0.30** for type compatibility with qbsp. Do not bump without checking qbsp's dependency.

## Build and run

```bash
cargo run -p postretro                        # engine (debug)
cargo run -p postretro -- assets/maps/test.bsp  # engine with a BSP map
cargo run -p postretro -- assets/maps/test.prl  # engine with a PRL map
cargo run -p postretro-level-compiler -- input.map -o output.prl  # compile a level (binary: prl-build)
cargo run --release -p postretro              # optimized engine build
RUST_LOG=info cargo run -p postretro          # with logging
```
