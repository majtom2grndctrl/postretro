# s8 — postretro-renderer (GPU)

> Epic: `render-stack-decomposition`. The terminal cut: the single GPU crate that restores "Renderer owns GPU" within one boundary and hides `wgpu::SurfaceTexture`.

## Goal

Extract all wgpu-touching render code into `postretro-renderer`, **absorbing** the GPU modules that currently live outside `render/`, and expose an engine-facing API that names no wgpu types — so the binary and every other crate are wgpu-free and the invariant holds by construction.

## Scope

### In scope
- Move all of `render/` that touches wgpu: `Renderer`/`FullRenderer` (`renderer_types.rs`), every `renderer_*.rs` impl file, every pass (forward/depth/wireframe/shadow/`mesh_pass` GPU/`smoke`/`fog_pass`/`screen_effects`/`splash_pass`/`sh_volume` compose/`sh_compose`/`animated_lightmap`/`sdf_atlas`/`sdf_shadow`/`frame_timing`/`debug_lines`/`debug_ui` GPU), `pipeline_layout` BGL objects, `loaded_texture` GPU upload, and the GPU UI pass (`ui/mod.rs` `UiPass` + `ui/text.rs` `UiTextRenderer`).
- **Absorb the stray GPU modules** so no wgpu lives outside this crate: `compute_cull`, `candidate_cull`, `shadow_cull`, and `lighting::{spot_shadow,cube_shadow,lightmap,chunk_list}`. (`candidate_cull_mirror`/`candidate_cull_probes` are CPU test oracles — keep test-side or in `render-cpu`/dev.)
- **Opaque present handle:** replace `render_frame_indirect`'s `Result<Option<wgpu::SurfaceTexture>>` return with an opaque renderer-owned handle; the binary calls `renderer.present(handle)`. The handle encapsulates surface acquire (Success/Suboptimal/Outdated/Lost/Timeout/Validation), surface `TextureView` creation, encoder completion, and `present()`. Unify the gameplay and splash present paths behind it.
- Depend on `postretro-ui`, `postretro-render-cpu`, `postretro-visibility`, `postretro-level-loader`, `postretro-geometry`, `postretro-material`, `postretro-lighting`, `model` (CPU loader — crate or binary module per open question 3), `postretro-entities`/`scripting-core` (snapshot values), `wgpu`, `winit`, `glyphon`, `glam`, `bytemuck`.
- Update binary consumers (`main.rs`, `startup/lifecycle.rs`, `startup/splash_lifecycle.rs`, `session/mod.rs`) to import the renderer crate and drive the present loop via the handle.

### Out of scope
- The `FullRenderer` encapsulation refactor (converting `pub(super)` reach-in to owned-handle constructors). `pub(super)` stays **within** the crate; `FullRenderer` remains private. (Deferred `s9`.)
- Pass-level GPU sub-crates.
- `render-diagnostics` extraction (deferred `s10`).

## Acceptance criteria
- [ ] `postretro-renderer` is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; renderer/GPU and shader-parity tests pass from their relocated home.
- [ ] **Invariant restored:** `rg wgpu` over every crate except `postretro-renderer` (and the binary's thin present driver) is empty — the cull and lighting-GPU modules now live inside the renderer crate.
- [ ] **No wgpu in the public API:** no consumer of `postretro-renderer` imports `wgpu`; `wgpu::SurfaceTexture`/`TextureView` do not appear in any engine-facing signature. The present handle is opaque.
- [ ] The dependency graph is acyclic and one-way (proven by `cargo build --workspace`); no lower crate depends on `postretro-renderer`.
- [ ] Behavior-preserving: a fixture map renders identically (no visible-output regression in existing render tests); the typedef drift test is byte-identical; WGSL byte-layout guards pass.
- [ ] Editing the binary's non-render code does not recompile `postretro-renderer`.

## Tasks

### Task 1: Absorb stray GPU modules
Move `compute_cull`/`candidate_cull`/`shadow_cull` + the four lighting GPU pools into the renderer crate (open question 7: stage into `render/` first, or move directly at cut time).

### Task 2: Extract postretro-renderer
Create the crate; move all wgpu render code + the GPU UI pass; wire deps on the lower crates; widen boundary symbols. `mesh_pass.rs`/`sh_volume.rs` are oversized but this is largely a move — split only if substantial edits are needed (split-before-extend).

### Task 3: Opaque present handle
Introduce the handle type; rework `render_frame_indirect` and the splash path to return/consume it; move surface acquire/error handling behind it; update the binary present loop to `renderer.present(handle)`.

## Sequencing
**Phase 1:** Task 1, then Task 2, then Task 3. Needs `s2`–`s7` (+ `model`). Milestone 3 (terminal). The full verification gate (invariant `rg`, `cargo tree` isolation across all crates, acyclicity, typedef drift, WGSL) runs here.
