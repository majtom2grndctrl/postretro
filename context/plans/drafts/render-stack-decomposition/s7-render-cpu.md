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
- Depend on `postretro-level-loader`, `postretro-visibility` (for `VisibleCells`, used by `mesh_visible`), `postretro-render-data`, `postretro-entities`, `postretro-scripting-core`, `postretro-lighting`, `glam`, `bytemuck`.

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
Apply the descent rule against current source: a helper leaves if it is wgpu-free **and** does not read `FullRenderer` state; WGSL binding constants travel with their packers. The ruling below was produced by reading current source; carve the two SPLIT files along their seam first (these are oversized — `mesh_pass.rs` 3529, `sh_volume.rs` 2443 — split, don't transplant whole).

**Membership ruling (source-confirmed).** Everything in the candidate surface descends except the GPU-recording halves of two files, which split:

- **DESCEND** (wgpu-free, no `FullRenderer` read): `frame_uniforms.rs` (`FrameUniforms`, `build_uniform_data`, `UNIFORM_SIZE`, `SDF_SHADOW_FLAG_ATLAS_PRESENT`, the three isolation enums); `mesh_instances.rs` whole (`plan_mesh_frame`, `MeshInstanceInput`, `PlannedInstance`, `ModelDrawGroup`, `MeshFramePlan`, `JointCounts`, `instance_casts_into_cone`, `MAX_PALETTE_ENTRIES`, `MAX_INSTANCES`); `material_plan.rs` CPU set (`plan_submesh_materials`, `SubmeshMaterialPlan`, `build_material_uniform`, `parse_blake3_key`, `resolve_model_open_path_and_handle`); `fog_mask.rs` whole; `screen_effects.rs` (`pack_effect_uniform`, `EffectUniform`); `splash.rs` (`load_splash`); the CPU packers in `sh_volume.rs` (`build_grid_info_bytes`, `build_animation_buffers`, the f16 codec, the SH/delta types `light_bridge` imports), `sh_compose.rs` (`build_delta_buffers`, `DeltaComposeBuffers`, `f16_bits_to_f32`), `sdf_atlas.rs` (`build_meta_bytes`, `scatter_bricks_to_atlas`), `sdf_shadow.rs` (`pack_params_bytes`, `SdfShadowTuning`), `animated_lightmap.rs` (`validate_cross_section`, `AnimatedLmDebugConfig`); and `fx::{smoke,fog_volume}` data.
- **SPLIT `mesh_pass.rs`:** carve the pure-CPU `mesh_visible` (`:1918`) + `mesh_visible_in_cell` (`:1932`) — a `LevelWorld`+`VisibleCells` cell-membership predicate, the surface `scripting/systems/mesh_render.rs` imports — and `ClipMetadata` into the CPU crate; leave the GPU draw-recording renderer-side. (`mesh_visible` is pure, not GPU — it carries the `postretro-visibility` dep noted above.)
- **SPLIT `loaded_texture.rs`:** carve the CPU `.prm` parse / mip / slot-plan (`level_byte_size`, `slot_levels`, `header_mip_count`, `texture_slot_plan`); leave `upload_texture_data` (`Device`/`Queue`) renderer-side.
- **Travelling WGSL constants** (move with their packers, never alone): `UNIFORM_SIZE`, `MATERIAL_UNIFORM_SIZE`, `SH_GRID_INFO_SIZE`, `SHADOW_PASS_PARAMS_SIZE`, `SDF_ATLAS_META_SIZE`, `DYNAMIC_DIRECT_PARAMS_SIZE`.

### Task 2: Extract postretro-render-cpu
Create the crate, move the ruled-in helpers + their constants + tests, re-point scripting systems, wire deps.

## Sequencing
**Phase 1:** Task 1 (ruling + carve), then Task 2 (extract). Needs `s2`, `s3`, `s5`. Milestone 2; blocks `s8`.
