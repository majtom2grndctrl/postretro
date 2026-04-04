# PostRetro

Retro-style FPS engine (Doom/Quake boomer shooter, cyberpunk aesthetic). Rust + OpenGL 3.3 core.

## Start here

Load `context/lib/index.md` — it routes to the right docs for your task.

## Key constraints

- **No `unsafe` without approval.** See `context/lib/development_guide.md` §3.5.
- **Renderer owns GPU.** All wgpu calls live in the renderer module.
- **Frame order:** Input → Game logic → Audio → Render → Present.
- **glam pinned to 0.30** for type compatibility with qbsp. Do not bump without checking qbsp's dependency.

## Build and run

```bash
cargo run              # debug build
cargo run --release    # optimized build
RUST_LOG=info cargo run  # with logging
```
