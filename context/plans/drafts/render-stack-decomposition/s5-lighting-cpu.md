# s5 — postretro-lighting (CPU-math)

> Epic: `render-stack-decomposition`. Splits `crate::lighting` into a CPU-math crate; the GPU pools move into the renderer crate at `s8`.

## Goal

Extract the wgpu-free lighting math (light packing, cone/frustum geometry, specular/influence packing) into a CPU-only crate shared by the renderer and gameplay subsystems, so editing it does not recompile the GPU stack.

## Scope

### In scope
- `postretro-lighting`: move the wgpu-free lighting files — `mod.rs` (`pack_light_with_slot`, `pack_lights`, `pack_lights_with_slots_into`, `patch_shadow_slots`, `patch_cube_slots`, `light_reaches_visible_cell`, `entity_occluder_eligible`, `GPU_LIGHT_SIZE`, `SHADOW_SLOT_BYTE_OFFSET`, `CUBE_SLOT_BYTE_OFFSET`), `influence.rs` (`LightInfluence`, `pack_influence`), `spec_buffer.rs` (`pack_spec_lights`, `SPEC_LIGHT_SIZE`, flag consts), `cone_frustum.rs` (`Aabb`, `cone_frustum_planes`, `aabb_intersects_frustum`, `cone_enclosing_aabb`).
- The packers' byte-layout constants travel with them (shared with WGSL).
- Re-point the cross-subsystem consumers of `cone_frustum`/packers — `weapon/` (hit-zones), `model/mesh`, and the GPU cull/shadow code — to the crate.
- Depend on `glam`, `postretro-geometry` (if `Aabb`/cone math references geometry types — confirm), `serde`.

### Out of scope
- The GPU pool structs `SpotShadowPool`, `CubeShadowPool`, `LightmapResources`, `ChunkGrid` (`spot_shadow.rs`/`cube_shadow.rs`/`lightmap.rs`/`chunk_list.rs`) — they own wgpu and move into `postretro-renderer` at `s8`.
- `script_primitives.rs` placement — see decision below.

## Acceptance criteria
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; light-packing / cone-frustum tests pass from their relocated home.
- [ ] `cargo tree -p postretro-lighting` shows no wgpu/winit/glyphon/kira (and no `mlua`/`rquickjs` unless `script_primitives` lands here behind `script-ffi`).
- [ ] `weapon`, `model`, and the GPU shadow/cull consumers compile against the crate; `GPU_LIGHT_SIZE`/`SPEC_LIGHT_SIZE`/slot byte-offset constants unchanged (no shader-layout drift).
- [ ] The four GPU pool modules are untouched and still compile in their current home (they move at `s8`).

## Tasks

### Task 1: Extract postretro-lighting CPU-math
Create the crate, move the four wgpu-free files + their constants, widen boundary symbols, re-point `weapon`/`model`/cull/shadow consumers.

### Task 2 (decision): script_primitives placement
**Open question 1.** Per `scripting.md §12` handler placement, script-primitive *wiring* co-locates with its subsystem and is invoked from `Session::build` — default: `lighting/script_primitives.rs` stays binary-side, calling into `postretro-lighting`. Alternative: move it into `postretro-lighting` behind a `script-ffi` feature. Pick one; if it stays binary-side, this task is just confirming it compiles against the new crate.

## Sequencing
**Phase 1:** Task 1, then Task 2 (placement confirmation). Largely independent; needs `s2` only if `cone_frustum` references geometry types. Milestone 2.
