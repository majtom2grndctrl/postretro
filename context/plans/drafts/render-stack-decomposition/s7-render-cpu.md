# s7 — postretro-render-cpu

> Epic: `render-stack-decomposition`. The CPU harvest — the largest direct compile-time win — and the cut that severs the `scripting → render` dependency.

## Goal

Move the CPU islands embedded in GPU files (byte-packing, frame planning, validation) and their no-GPU test suites into a CPU-only crate, so they recompile off the wgpu path and so `scripting/systems/*` depend on shared CPU data instead of the renderer.

## Scope

### In scope
- `postretro-render-cpu`: move the wgpu-free render data/logic —
  - `frame_uniforms.rs` (`FrameUniforms`, `build_uniform_data`, `UNIFORM_SIZE`, `SDF_SHADOW_FLAG_ATLAS_PRESENT`, `LightingIsolation`/`SdfShadowMode`/`DynamicDirectIsolation`),
  - `mesh_instances.rs` (`plan_mesh_frame`, `MeshInstanceInput`, `PlannedInstance`, `ModelDrawGroup`, `MeshFramePlan`, `JointCounts`, `MAX_PALETTE_ENTRIES`, `MAX_INSTANCES`),
  - the mesh-data types `scripting` imports — `mesh_visible` and `ClipMetadata` (carved out of `mesh_pass.rs`),
  - the SH/delta CPU packing types `scripting/light_bridge.rs` imports (carved out of `sh_volume.rs`),
  - `material_plan.rs` CPU half (`plan_submesh_materials`, `build_material_uniform`, `parse_blake3_key`, `resolve_model_open_path_and_handle`),
  - `fog_mask.rs` (`compute_fog_cell_mask`, `sphere_intersects_any_fog_aabb`),
  - `fx::{smoke,fog_volume}` data (`SpriteFrame`, `load_collection_frames`, `SPRITE_INSTANCE_SIZE`, `MAX_SPRITES`, `FogVolume`, `FogSpotLight`, packing constants),
  - the per-function-ruled CPU halves of `loaded_texture` (`.prm` parse/slot-plan/mip math), `sdf_atlas`, `sdf_shadow`, `sh_volume`, `sh_compose`, `animated_lightmap`, `screen_effects::pack_effect_uniform`, `splash::load_splash`,
  - their no-GPU unit tests, including the WGSL byte-layout guards (`group3_shader_bindings`, `uniform_tests`) that pin the packers.
- **WGSL binding-index/stride constants travel with their packers** (no shader-layout drift).
- Re-point `scripting/systems/{mesh_render,mesh_anim,light_bridge,emitter_bridge,particle_render,fog_volume_bridge}.rs` to `postretro-render-cpu`.
- Depend on `postretro-level-loader`, `postretro-geometry`, `postretro-entities`, `postretro-scripting-core`, `postretro-lighting`, `glam`, `bytemuck`.

### Out of scope
- Anything that reads `FullRenderer` fields or owns wgpu resources — stays in `postretro-renderer`.
- The GPU passes themselves (`mesh_pass` GPU half, `sh_volume` GPU compose, `fog_pass`, `smoke` GPU, `sdf_*` passes).

## Acceptance criteria
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; all moved packing/planning/validation tests pass from their relocated home.
- [ ] `cargo tree -p postretro-render-cpu` shows no `wgpu`/`winit`/`glyphon`/`kira`.
- [ ] **No `scripting → render` edge remains:** `rg "use crate::render" crates/postretro/src/scripting` returns nothing (the scripting systems import `postretro-render-cpu` instead); a future `postretro-scripting-core`/scripting layer never depends on `postretro-renderer`.
- [ ] WGSL byte-layout guards (`group3_shader_bindings`, `uniform_tests`, `shader_tests`) pass — moved packers produce identical bytes.
- [ ] Editing `postretro-render-cpu` does not recompile `wgpu`/`naga`.

## Tasks

### Task 1: Per-function membership ruling (split-before-move)
**Open question 5.** Classify each candidate helper: clean leavers (`frame_uniforms`, `mesh_instances`, `fog_mask`, `material_plan` CPU, `fx` data) vs. entangled (`sh_volume`/`sdf_*`/`animated_lightmap` CPU halves that read `FullRenderer` state or share per-frame buffers). Carve `mesh_visible`/`ClipMetadata` out of `mesh_pass.rs` and the SH packing types out of `sh_volume.rs` first (these files are oversized — `mesh_pass.rs` 3529, `sh_volume.rs` 2443 — split along the seam, don't transplant whole).

### Task 2: Extract postretro-render-cpu
Create the crate, move the ruled-in helpers + their constants + tests, re-point scripting systems, wire deps.

## Sequencing
**Phase 1:** Task 1 (ruling + carve), then Task 2 (extract). Needs `s2`, `s3`, `s5`. Epic Phase 4; blocks `s8`.
