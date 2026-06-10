# Dynamic Mesh Shadow Receipt

> **Status:** draft.
> **Track:** Lighting / M10 render foundation — roadmap "Dynamic mesh shadow receipt" (bullet added on the M10 shadows branch).
> **Related:** `context/lib/rendering_pipeline.md` §4, §8, §9 · sibling spec `M10--dynamic-mesh-direct-lighting` (hard dependency — supplies the per-light term this spec attenuates) · `context/plans/ready/M10--dynamic-mesh-shadows/` (supplies the pools).
> **Orchestrator note:** phase 2 of a combined run with `M10--dynamic-mesh-direct-lighting`.

## Goal

M10 shadow casting made entities throw shadows; the direct-lighting sibling makes dynamic lights illuminate entities. This spec closes the triangle: entities *receive* crisp runtime shadows. Attenuate each dynamic light's per-light term in the skinned-mesh shader by sampling that light's existing shadow map — the spot 2D-array and point cube-array pools — consumed from the entity shader side, not only the world shader. Gives crisp world→entity (geometry shadowing an enemy under a spot) and entity→entity (enemies shadowing each other) shadows, replacing the soft probe-coarse SH approximation as the only world→entity signal.

## Prerequisites (hard)

- **`M10--dynamic-mesh-shadows` merged to main.** SATISFIED (2026-06, PR #114): the 96-slot spot 2D-array, 6-slot point cube-array, and the dual slot indices in the light record are in-tree. Note its Task 7 (static-depth caching) was cut at landing.
- **`M10--dynamic-mesh-direct-lighting` complete.** There is no per-light term on the mesh to attenuate until group 2 exists; that spec hardwires per-light visibility to 1.0 as the seam this spec fills.

## Scope

### In scope

- **Shared shadow-sampling helpers.** Factor the forward pass's `sample_spot_shadow` and `sample_point_shadow` (3×3 PCF; UV-space taps for spot, orthonormal-basis angular taps for cube) into a binding-agnostic shared WGSL snippet per the §8 composition convention, consumed by both `forward.wgsl` and `skinned_mesh.wgsl`. Forward output stays byte-identical.
- **Mesh-side shadow bindings.** Extend the mesh pass's group 2 (allocated by the sibling spec) with the shadow resources the forward pass binds in its group 5: spot depth 2D-array + comparison sampler + the 96-entry light-space matrices buffer, and the cube-array depth texture conditional on adapter cube-array support (mirroring forward's conditional binding and shader-variant gating). Same underlying GPU resources, mesh-specific layout.
- **Per-light attenuation in the mesh loop.** Replace the sibling spec's hardwired visibility with the forward pass's exact slot logic: spot slot from the light record's spot-slot field, cube slot from the cube-slot field, `0xFFFFFFFF` sentinel ⇒ unshadowed, shadow factor multiplies the per-light attenuation.
- **Self-shadow bias validation.** A receiving entity also renders into the same shadow maps as an occluder. Validate the existing bias scheme (point: world-space depth bias; spot: depth compare) against skinned curved surfaces; tune bias (and/or add a normal-offset at the sample site) until self-shadow acne is not visible at gameplay distance on the dev models.
- **Budget + layout tests.** Extend the mesh pipeline's bind-group-layout assertions, keep the matrices-array-length regression test (`light_space_matrices_array_len_matches_pool` precedent) covering the mesh shader's declaration, and record the mesh pipeline's sampled-texture count.

### Out of scope (non-goals)

- **World→entity shadows from point lights.** The cube pool is entity-only in v1 (no world geometry in cube faces), so point-light receipt yields entity→entity only. Worlded cube faces are a future pool change, not this spec.
- **Any pool or casting-side change.** Slot ranking, pool capacities, the skinned-depth pass, and `entity_occluder_eligible` gating are untouched; this spec is a pure consumer.
- **Shadow receipt for billboards** and for the baked/static light tiers (static direct on movers stays the soft SH-direct term by design).
- **Static-depth caching (M10 Task 7)** — cut at M10 landing (per-frame world-depth re-render is the baseline); orthogonal to receipt either way.

## Acceptance criteria

- [ ] A skinned mesh standing behind static geometry relative to a dynamic spot light reads shadowed in that light's term (world→entity), with the shadow edge consistent with the world-surface shadow from the same light.
- [ ] With two entities under a dynamic spot or point light with `casts_entity_shadows` enabled, one entity's shadow falls visibly across the other (entity→entity).
- [ ] A dynamic light without a shadow slot (sentinel) lights meshes unshadowed — identical to the sibling spec's output.
- [ ] On adapters without cube-array support, point lights light meshes unshadowed and the mesh pipeline builds without error (mirrors forward's fallback).
- [ ] No visible self-shadow acne on the dev skinned models at gameplay distance under both spot and point shadowed lights.
- [ ] Forward-pass world rendering byte-identical after the helper extraction.
- [ ] Mesh pipeline bind-group assertions extended; matrices-array-length test covers the mesh shader; sampled-texture counts recorded for the mesh pipeline (both cube-support variants).

## Tasks

### Task 1: Shared shadow-sampling snippet
Extract `sample_spot_shadow`, `sample_point_shadow`, and their helpers (PCF kernels, `cube_face_ndc_depth`, bias constants) from `forward.wgsl` into a shared snippet appended at pipeline creation; consumers declare the depth textures / sampler / matrices buffer at their own (group, binding) before the snippet (the `sh_sample.wgsl` precedent). Forward consumes it; output byte-identical.

### Task 2: Mesh shadow bindings
Extend the mesh group-2 BGL + bind group with spot depth array, comparison sampler, light-space matrices (uniform, not storage — see sketch), and the conditional cube-array entry at the binding slots the sibling spec reserves (b5–b8); plumb adapter capability through mesh pipeline creation via the same `strip_point_shadow_cube` marker mechanism forward uses. Renderer rebinds when pool textures are recreated.

### Task 3: Attenuate the mesh loop
In `skinned_mesh.wgsl`'s dynamic-light loop, read both slot fields from the light record, sample via the Task 1 helpers, multiply into per-light attenuation — replacing the sibling spec's hardwired 1.0. Sentinel and bounds behavior identical to forward.

### Task 4: Bias validation + tests
Visual pass for self-shadow acne on skinned models (tune bias / normal-offset as needed, keeping world-surface shadows unchanged); extend layout/budget/regression tests per AC.

## Sequencing

**Phase 1 (sequential):** Task 1 — forward refactor blocks shader work.
**Phase 2 (sequential):** Task 2 — bindings block the loop edit.
**Phase 3 (sequential):** Task 3 — consumes Task 1 helpers + Task 2 bindings.
**Phase 4 (sequential):** Task 4 — verifies the assembled feature.

## Rough sketch

- Light record slots: `GpuLight.cone_angles_and_pad.z` = spot slot, `.w` = cube slot; sentinel `NO_SHADOW_SLOT = 0xFFFFFFFF` (`lighting/spot_shadow.rs`).
- Pools: spot `SHADOW_POOL_SIZE = 96` slots, 1024² `Depth32Float` 2D-array; cube `CUBE_COUNT = 6` slots × `CUBE_FACES = 6` faces at `CUBE_FACE_RESOLUTION = 512`, sampled as `texture_depth_cube_array`; shared `compare_sampler`. Cube projection constants the mesh sampler must reproduce: `CUBE_NEAR_CLIP = 0.1`, far = `falloff_range.max(0.5)`.
- Forward sampling today: group 5 — b0 spot depth array, b1 comparison sampler, b2 `light_space_matrices` as `var<uniform> LightSpaceMatrices { m: array<mat4x4<f32>, 96> }` (a UNIFORM, deliberately not a storage buffer — stays under `max_storage_buffers_per_shader_stage` (default 8); the mesh declaration must do the same), b5 cube array (conditional). PCF: `SPOT_SHADOW_PCF_RADIUS = 1.0`, 9 taps both paths; point bias `POINT_SHADOW_DEPTH_BIAS = 0.08`.
- No-cube gating mechanism (mirror it exactly): `render::strip_point_shadow_cube` text-strips the `// CUBE_SHADOW_BINDING`-tagged declaration and replaces the `sample_point_shadow` body between the `CUBE_SHADOW_BODY_BEGIN`/`CUBE_SHADOW_BODY_END` markers with `return 1.0;`. The Task 1 shared snippet must preserve these markers, and the mesh pipeline applies the same strip on no-cube adapters.
- Mesh bindings ride group 2 at the slots the sibling spec reserves (b5 spot depth array, b6 comparison sampler, b7 light-space matrices, b8 conditional cube array — a mesh-specific layout over the same resources) rather than importing forward's group-5 BGL — that layout carries SDF factor + scene depth entries the mesh shader must not sample.
- Self-shadow note: the receiving fragment's own depth is in the map (entity occluders render via `record_skinned_depth`). Spot path has no slope-scale bias today; if acne appears on curved skinned surfaces, prefer a sample-site normal-offset (world-space, normal-scaled) over raising the global bias, to avoid peter-panning the world's shadows.

## Open questions

- Exact bias/normal-offset values for skinned receivers — settle by visual check in Task 4; constraint: world-surface shadow appearance unchanged.
- Whether the mesh pipeline's total sampled-texture count needs trimming on the no-cube-support path — record first, act only if a limit is approached.
