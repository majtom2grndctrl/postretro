# postretro-lighting (CPU-math)

> Epic: `E19--render-stack-decomposition`. Splits `crate::lighting` into a CPU-math crate; the GPU pools move into the renderer crate at `E19--renderer-gpu`.

## Goal

Extract the wgpu-free lighting math (light packing, specular/influence packing) into a CPU-only crate, so editing the lighting math does not recompile the GPU stack, and so the script-light primitive wiring co-locates with it (Task 2). The packers themselves have only renderer consumers; the crate's payoff is the warm-edit firewall, not cross-subsystem sharing.

## Scope

### In scope
- `postretro-lighting`: move the wgpu-free lighting files — `mod.rs` (`pack_light_with_slot`, `pack_lights`, `pack_lights_with_slots_into`, `patch_shadow_slots`, `patch_cube_slots`, `light_reaches_visible_cell`, `entity_occluder_eligible`, `GPU_LIGHT_SIZE`, `SHADOW_SLOT_BYTE_OFFSET`, `CUBE_SLOT_BYTE_OFFSET`), `influence.rs` (`pack_influence` **only** — the bare `LightInfluence` already sank to `postretro-render-data` per hub Decision 10; `pack_influence` imports it back), `spec_buffer.rs` (`pack_spec_lights`, `SPEC_LIGHT_SIZE`, flag consts `SPEC_LIGHT_TYPE_{POINT,SPOT,DIRECTIONAL}` + `SPEC_LIGHT_SDF_FLAG`). Also carve `NO_SHADOW_SLOT` (`pub const … u32 = 0xFFFF_FFFF`) out of `spot_shadow.rs` — the moved packers reference it and `spot_shadow.rs` (staying until `E19--renderer-gpu`) re-imports it from here.
- `cone_frustum.rs` does **not** live here — it moves to `postretro-render-data` (`E19--render-data`). It is geometry/AABB math with the widest fan-out (model/weapon/cull/renderer), and it delegated to `compute_cull` (a GPU module bound for `postretro-renderer`), so homing it in lighting would have created a `lighting → renderer` dependency cycle via `compute_cull`. See `E19--render-data` and the hub Decision.
- The packers' byte-layout constants travel with them (shared with WGSL).
- Re-point the packer consumers — the renderer resource-init/slot modules (`render/mod.rs`, `render/renderer_init_resources.rs`, `render/renderer_light_slots.rs`, `render/renderer_resources.rs`) — to the crate. These are resource/slot code, not literally "cull"; there are no non-GPU packer consumers. Doc-comment-only mentions in `render/renderer_types.rs`/`render/renderer_lighting.rs` need no import change. The `cone_frustum`/`Aabb` consumers (weapon hit-zones, model, cull/shadow) now depend on `postretro-render-data`, not `postretro-lighting`.
- Depend on `glam`, `postretro-level-loader` (the packers use `LightType`/`MapLight`/`ShadowType`/`FalloffModel`), and `postretro-render-data` (settled — `pack_influence` takes `&[postretro_render_data::influence::LightInfluence]`). **Not** `serde` — none of the three moved files use it; `script_primitives.rs` uses `serde_json`, so it travels behind the `script-ffi` gate. The `render-data` dep is driven by `influence.rs`, not cone math (cone math has moved out).
- Add an optional `script-ffi` feature gating the script-primitive wiring (Task 2), off by default, per the `scripting.md §12` Cargo pattern (foundation/entities precedent: `script-ffi = ["dep:rquickjs", "dep:mlua", ...]`). When enabled it pulls the VM crates the wiring needs; default builds stay wgpu/VM-free.

### Out of scope
- The GPU pool structs `SpotShadowPool`, `CubeShadowPool`, `LightmapResources`, `ChunkGrid` (`spot_shadow.rs`/`cube_shadow.rs`/`lightmap.rs`/`chunk_list.rs`) — they own wgpu and move into `postretro-renderer` at `E19--renderer-gpu`.
- The marshalling substrate `script_primitives.rs` calls — stays in `scripting-core` (the VM-agnostic typedef/marshalling floor). Only the lighting *wiring* descends here (Task 2).

## Acceptance criteria
Inherits the epic global acceptance criteria — see `E19--render-stack-decomposition/index.md`. Durable decisions are captured into `context/lib/` per spec as each spec is approved — not in one batch at first promotion. **On approval, `scripting.md §12` must be updated**: record that once a subsystem is its own crate, its script-primitive wiring co-locates there behind `script-ffi` (the crate-descent this spec establishes) — otherwise §12's "handlers … not new crates" text contradicts the shipped tree.
- [ ] Crate is a workspace member; `cargo build --workspace` + `cargo test --workspace` pass; light-packing tests pass from their relocated home.
- [ ] `cargo tree -p postretro-lighting` (default features) shows no wgpu/winit/glyphon/kira and no `mlua`/`rquickjs` — the `script_primitives` wiring is gated behind the off-by-default `script-ffi` feature, so it pulls the VM crates only when that feature is enabled.
- [ ] The renderer resource-init/slot modules (the packer consumers) compile against the crate; `GPU_LIGHT_SIZE`/`SPEC_LIGHT_SIZE`/slot byte-offset constants unchanged (no shader-layout drift). (`weapon`/`model`/`cone_frustum` consumers depend on `postretro-render-data` now, not this crate.)
- [ ] The four GPU pool modules (`spot_shadow.rs`, `cube_shadow.rs`, `lightmap.rs`, `chunk_list.rs`) are untouched and still compile in their current home (they move at `E19--renderer-gpu`) — except `spot_shadow.rs` loses only the `NO_SHADOW_SLOT` const (carved into `postretro-lighting`; `spot_shadow.rs` re-imports it from there).

## Tasks

### Task 1: Extract postretro-lighting CPU-math
Create the crate, move the three wgpu-free files (`mod.rs`, `influence.rs`, `spec_buffer.rs`) + their constants (including the `NO_SHADOW_SLOT` sentinel carved from `spot_shadow.rs`) + their inline `#[cfg(test)]` light-packing tests, widen boundary symbols, re-point the packer consumers (the four `render/` modules listed in Scope). `cone_frustum.rs` is **not** part of this move — it lands in `postretro-render-data` (`E19--render-data`), and its consumers re-point there.

### Task 2: Descend script_primitives wiring behind `script-ffi`
**Decision (was open question 1): the scripting wiring descends into `postretro-lighting` behind the optional `script-ffi` feature.** Move `lighting/script_primitives.rs` into the crate under that feature gate, and re-point the three sites that reach it by the old path: `scripting/primitives/light.rs` (`pub(crate) use crate::lighting::script_primitives::*;`), `scripting/entity_world_primitives.rs` (`use crate::lighting::script_primitives as light;`), and `bin/gen_script_types.rs` (`#[path = "../lighting/script_primitives.rs"]`). The marshalling substrate it calls stays in `postretro-scripting-core` (`primitives_registry::{ContextScope, PrimitiveRegistry}`, `sequence::{SequenceError, SequencedPrimitiveRegistry}`).

Registrars stay invoked as today, just re-homed: `register_sequenced_light_primitives` from `Session::build` directly, and `register_light_entity_primitives` from `register_all` (which `Session::build` calls). Plumbing: export both registrars as `pub` items gated on `script-ffi`; the binary enables `postretro-lighting/script-ffi` (forwarded from the runtime's upper-floor feature per §12) so `Session::build` can reach them. `script-ffi` mirrors the entities precedent — beyond `dep:rquickjs`/`dep:mlua` it forwards `postretro-entities/script-ffi` and `postretro-foundation/script-ffi` (the file imports `postretro_entities` types and `postretro_foundation::Vec3Lit`) and pulls `dep:serde_json`; enumerate the exact set from the file's `use` graph at implementation time.

Principle: this **extends** `scripting.md §12` — §12 places handler *wiring* in the binary ("not new crates"), and co-locating that wiring with its subsystem once the subsystem is itself a crate is the natural generalization. §12 directly governs the two halves this keeps: the marshalling substrate stays in `scripting-core`, and `script-ffi` is off by default per the §12 Cargo pattern. With the feature off the crate stays VM-free; with it on it pulls `rquickjs`/`mlua`. The hub names `postretro-lighting` the first instance and the precedent the Epic 16 combat crate mirrors.

## Sequencing
**Phase 1:** Task 1, then Task 2 (placement confirmation). Hard prerequisites (both already in `done/`): `postretro-render-data` (`E19--render-data`, for `LightInfluence`) and `postretro-level-loader` (`E19--level-loader`, for the light typedefs the packers consume). Milestone 2.
