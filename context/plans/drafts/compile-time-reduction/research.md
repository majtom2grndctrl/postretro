# Compile-Time Reduction Research

## Workspace Shape

- Workspace members: `crates/postretro`, `crates/level-format`, `crates/level-compiler`, `crates/script-compiler`, `crates/net`.
- `rust-toolchain.toml` pins `stable`.
- No repo-local `.cargo/config.toml` exists.
- Root dev profile: `opt-level = 1`; dependency dev profile: `opt-level = 2`.
- `postretro` depends directly on `wgpu`, `winit`, `glyphon`, `kira`, `parry3d`, `rquickjs`, `mlua`, `notify`, `gltf`, and `postretro-net`.
- `postretro` has a build dependency on `postretro-script-compiler`.
- `crates/postretro/build.rs` calls `postretro_script_compiler::write_prelude`, so the SWC-based script compiler participates in engine build work.

## Dependency Hotspots To Measure

Check these explicitly in `cargo build --timings`:

- `postretro-script-compiler` and SWC crates.
- `rquickjs-sys`.
- `mlua-sys` and `luau0-src`.
- `wgpu` and `naga`.
- `glyphon` and `cosmic-text`.
- `kira`, `cpal`, and audio decoder dependencies.
- Final engine link time.

Local rough package counts from the research pass: `postretro` normal+build tree is about 460 unique package/version pairs, `postretro-script-compiler` about 180, and `postretro-level-compiler` about 140. Treat these as orientation only; timing data decides priorities.

## Source Seams

`crates/postretro/src/portal_vis.rs` is the cleanest first extraction seam. It depends on `glam::Vec3`, `LevelWorld`, and frustum types from `visibility.rs`. The plan avoids coupling the new crate to full `LevelWorld` by introducing a borrowed portal-world view.

`crates/postretro/src/prl.rs` is feasible but wider. It defines `LevelWorld` and `load_prl`, and reaches into engine-local CPU modules: `geometry`, `material`, and `lighting::influence`. Split internally before moving it to a crate.

Scripting extraction is larger than the initial research summary implied. Core runtime files are separable, but bridge systems reach into render UI types, renderer mesh metadata, PRL types, input modes, visibility, lighting, and FX. Move GPU-free UI descriptor/model data first; leave bridge systems in `postretro` for the first scripting split.

Renderer extraction is intentionally deferred. Root GPU modules outside `render/` would need to move under renderer ownership first, and engine-facing APIs should hide direct `wgpu::SurfaceTexture` exposure.

## Oversized Files

Plan-relevant files already over the split threshold:

- `crates/postretro/src/main.rs` — about 6.7k lines.
- `crates/postretro/src/prl.rs` — about 3.4k lines.
- `crates/postretro/src/render/mesh_pass.rs` — about 3.5k lines.
- `crates/postretro/src/startup/lifecycle.rs` — about 2.4k lines.
- `crates/postretro/src/portal_vis.rs` — about 2.3k lines.
- `crates/postretro/src/render/sh_volume.rs` — about 2.4k lines.
- Several scripting files, including `registry.rs`, `luau.rs`, and `systems/mesh_render.rs`.

Avoid growing these files as part of compile-time work. Split first when a task must touch them substantially.

## Measurement Notes

Use isolated target directories under `/tmp` for repeatability. Do not time ignored level-compiler cold-bake integration tests as routine inputs; the testing guide warns they can take about an hour.

Platform notes:

- macOS: avoid committed repo-wide linker overrides by default.
- Linux: `mold` or `lld` may help link time, but should be opt-in unless installed in the dev/CI image.
- Windows: `rust-lld`/`lld-link` may help, but needs a Windows-host measurement.
- All platforms: measure native-code crates per target triple.
