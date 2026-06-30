# s5 — postretro-lighting (CPU-math)

> Epic: `render-stack-decomposition`. Splits `crate::lighting` into a CPU-math crate; the GPU pools move into the renderer crate at `s8`.

## Goal

Extract the wgpu-free lighting math (light packing, cone/frustum geometry, specular/influence packing) into a CPU-only crate shared by the renderer and gameplay subsystems, so editing it does not recompile the GPU stack.

## Scope

### In scope
- `postretro-lighting`: move the wgpu-free lighting files — `mod.rs` (`pack_light_with_slot`, `pack_lights`, `pack_lights_with_slots_into`, `patch_shadow_slots`, `patch_cube_slots`, `light_reaches_visible_cell`, `entity_occluder_eligible`, `GPU_LIGHT_SIZE`, `SHADOW_SLOT_BYTE_OFFSET`, `CUBE_SLOT_BYTE_OFFSET`), `influence.rs` (`LightInfluence`, `pack_influence`), `spec_buffer.rs` (`pack_spec_lights`, `SPEC_LIGHT_SIZE`, flag consts), `cone_frustum.rs` (`Aabb`, `cone_frustum_planes`, `aabb_intersects_frustum`, `cone_enclosing_aabb`).
- The packers' byte-layout constants travel with them (shared with WGSL).
- Re-point the cross-subsystem consumers of `cone_frustum`/packers — `weapon/` (hit-zones), `model/mesh`, and the GPU cull/shadow code — to the crate.
- Depend on `glam`, `postretro-render-data` (if `Aabb`/cone math references geometry types — confirm), `serde`.
- Add an optional `script-ffi` feature gating the script-primitive wiring (Task 2), off by default, per the `scripting.md §12` Cargo pattern (foundation/entities precedent: `script-ffi = ["dep:rquickjs", "dep:mlua", ...]`). When enabled it pulls the VM crates the wiring needs; default builds stay wgpu/VM-free.

### Out of scope
- The GPU pool structs `SpotShadowPool`, `CubeShadowPool`, `LightmapResources`, `ChunkGrid` (`spot_shadow.rs`/`cube_shadow.rs`/`lightmap.rs`/`chunk_list.rs`) — they own wgpu and move into `postretro-renderer` at `s8`.
- The marshalling substrate `script_primitives.rs` calls — stays in `scripting-core` (the VM-agnostic typedef/marshalling floor). Only the lighting *wiring* descends here (Task 2).

## Acceptance criteria
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; light-packing / cone-frustum tests pass from their relocated home.
- [ ] `cargo tree -p postretro-lighting` (default features) shows no wgpu/winit/glyphon/kira and no `mlua`/`rquickjs` — the `script_primitives` wiring is gated behind the off-by-default `script-ffi` feature, so it pulls the VM crates only when that feature is enabled.
- [ ] `weapon`, `model`, and the GPU shadow/cull consumers compile against the crate; `GPU_LIGHT_SIZE`/`SPEC_LIGHT_SIZE`/slot byte-offset constants unchanged (no shader-layout drift).
- [ ] The four GPU pool modules are untouched and still compile in their current home (they move at `s8`).

## Tasks

### Task 1: Extract postretro-lighting CPU-math
Create the crate, move the four wgpu-free files + their constants, widen boundary symbols, re-point `weapon`/`model`/cull/shadow consumers.

### Task 2: Descend script_primitives wiring behind `script-ffi`
**Decision (was open question 1): the scripting wiring descends into `postretro-lighting` behind the optional `script-ffi` feature.** Move `lighting/script_primitives.rs` into the crate under that feature gate; the marshalling substrate it calls stays in `scripting-core`; the registrar is still invoked from `Session::build`. Principle: `scripting.md §12` handler-placement spirit — wiring co-locates with its subsystem. This is the precedent the Epic 16 combat crate will mirror. With `script-ffi` off, the crate stays VM-free; with it on, it pulls `rquickjs`/`mlua` per the §12 Cargo pattern.

## Sequencing
**Phase 1:** Task 1, then Task 2 (placement confirmation). Largely independent; needs `s2` only if `cone_frustum` references geometry types. Milestone 2.
