# Dynamic Mesh Direct Lighting (mesh group 2)

> **Status:** draft.
> **Track:** Lighting / M10 render foundation — roadmap "Dynamic mesh direct lighting."
> **Related:** `context/lib/rendering_pipeline.md` §4 (lighting tiers, anti-double-count), §8 (shader composition), §9 (skinned model pipeline) · `context/plans/in-progress/lighting--entity-direct-sh/` (D10 pins the tier split this spec implements) · sibling spec `M10--dynamic-mesh-shadow-receipt` (consumes this spec's loop).
> **Orchestrator note:** designed to run as phase 1 of a combined run with `M10--dynamic-mesh-shadow-receipt`.

## Goal

Dynamic-tier lights light the world and billboards, and (after M10 shadow casting) receive entity shadows — but they do not light skinned meshes. A mesh standing under a dynamic light reads flat: it is lit only by baked SH (indirect + static-direct atlas). Fill the mesh pipeline's reserved bind group 2 with the forward pass's runtime-light resources and evaluate the same per-fragment dynamic-light loop in the skinned-mesh shader, so dynamic light lands on meshes coherently with the surfaces around them.

## Prerequisites

- `lighting--entity-direct-sh` mesh tasks merged (they are in-tree: `skinned_mesh.wgsl` group-4 superset, `sample_sh_direct`, `DynamicDirectParams` b16). If that plan is still in flight when this runs, coordinate on `skinned_mesh.wgsl` — both edit the fragment lighting composition.

## Scope

### In scope

- **Mesh-pass group 2 allocation.** New bind group layout + bind group in `render/mesh_pass.rs` (today the pipeline layout passes `None` for group 2, explicitly reserved for this task). Binds the SAME per-frame GPU buffers the forward pass binds: dynamic-light records, per-light influence volumes, scripted-animation descriptors + curve samples — plus one small mesh-side uniform carrying light count, time, and debug gating (the mesh camera group carries `view_proj` only; forward reads these from its own `Uniforms`).
- **Per-fragment dynamic-light loop in `skinned_mesh.wgsl`** matching the forward pass's semantics exactly: per-light influence-sphere early-out, point/spot/directional dispatch, all three falloff models, cone attenuation, and scripted per-light animation (brightness/color Catmull-Rom curves, animated spot aim). Lambert diffuse against the mesh's interpolated vertex normal. Sums into the existing lighting composition alongside `sample_sh_indirect` + `sample_sh_direct`.
- **Shared light-evaluation helpers.** Extract the per-light evaluation (falloff, cone attenuation, scripted-descriptor curve evaluation) from `forward.wgsl` into a binding-agnostic shared WGSL snippet per the §8 composition convention, consumed by both shaders. Forward output stays byte-identical.
- **Debug gating.** The new term participates in the lighting-isolation debug modes consistently with the forward pass's `use_dynamic` gating. Pin: the group-2 params uniform carries the SAME `lighting_isolation` value the renderer uploads to forward `Uniforms`, and the mesh derives `use_dynamic` from the same mode set forward does — not a new boolean. The mesh's existing group-4 `DynamicDirectParams.isolation` (baked direct-vs-indirect isolation) is orthogonal and untouched; the two gates multiply (a term renders only when both its gates pass).

### Out of scope (non-goals)

- **Shadow receipt.** The per-light shadow-slot index in the light record is ignored here (per-light visibility = 1.0). The sibling spec `M10--dynamic-mesh-shadow-receipt` adds shadow-map sampling on top of this loop.
- **Static-tier light terms.** `spec_lights` / sdf diffuse and specular stay forward-only. Static direct for movers is owned exclusively by the baked direct SH atlas (`lighting--entity-direct-sh` D10) — this loop evaluates the `is_dynamic`-filtered light set ONLY.
- **Specular / normal-mapped response on meshes.** The mesh path has neither specular maps nor normal maps; diffuse-only response against the vertex normal.
- **Billboards.** `billboard.wgsl` already runs its own dynamic-direct diffuse loop with its own copies of `falloff` / `cone_attenuation`. No billboard changes here; consolidating those copies onto the Task 1 snippet is explicitly out of scope. Constraint: the snippet's helper names must not collide if it is ever appended to the billboard pipeline — note `billboard.wgsl` also defines its own same-named `sample_sh_indirect` / `sample_sh_direct` wrappers, so the collision surface is wider than the light helpers alone.
- **Per-chunk light lists for meshes.** The world's chunk-grid light index is world-geometry-keyed; meshes use the flat loop + influence early-out (same as the world's fallback path). Revisit only against a measured cost at wave scale.

## Acceptance criteria

- [ ] *(Human visual check)* A dynamic point or spot light visibly brightens a skinned mesh; brightness and falloff read consistent with adjacent world surfaces lit by the same light, on a dev map.
- [ ] *(Human visual check)* A scripted animated light (brightness or color curves) modulates the mesh in phase and in hue with the world surfaces it lights — same frame, same curve values.
- [ ] *(Human visual check)* An animated spot with aim curves sweeps across a mesh coherently with its world cone.
- [ ] Static (`static_light_map` / sdf) lights produce zero change in mesh lighting — verified structurally: the buffer bound to the mesh loop is the `filter_dynamic_lights` output, so static lights are excluded by construction (the baked direct atlas remains the sole static-direct source on movers; no double-count).
- [ ] With zero dynamic lights, the mesh loop contributes nothing — output reduces to the existing `indirect + direct` composition (structural/CPU assertion).
- [ ] Helper extraction does not change forward behavior: extracted helper bodies are textually unchanged, `forward.wgsl` still passes naga validation, and no behavioral edit lands inside a moved helper.
- [ ] Lighting-isolation debug modes gate the mesh dynamic term exactly as they gate the world dynamic term.
- [ ] Existing render tests pass; a mesh group-2 bind-group-layout assertion is added (none exists today — model it on the billboard pipeline's storage-count guard).

## Tasks

### Task 1: Shared light-evaluation WGSL snippet
Extract the per-light evaluation helpers from `forward.wgsl` — `falloff`, `cone_attenuation` (the angle form the dynamic loop calls; `cone_attenuation_cos` is the static-path variant and stays in forward unless extraction is free), the scripted-descriptor evaluation (Catmull-Rom brightness/color/aim sampling, `scripted_light_intensity_scalar`) — into a binding-agnostic shared snippet appended at pipeline creation (the `sh_sample.wgsl` precedent). Helpers take buffer values as parameters or reference consumer-declared names; declare no bindings themselves. Forward.wgsl consumes the snippet; output byte-identical.

### Task 2: Mesh group 2 bind group
Define the group-2 BGL + bind group in `render/mesh_pass.rs` and thread it through pipeline layout and per-frame binding. Entries: the renderer's existing dynamic-light storage buffer (the `is_dynamic`-filtered set), the influence-volume buffer, the scripted-descriptor + animation-sample buffers (these are forward GROUP-3 resources — b13 descriptors, b12 `anim_samples` — re-bound at new mesh-side (group, binding) slots over the same underlying GPU buffers), and a new small uniform (light count, time, dynamic-direct debug gate). Pin: the uniform's `time` is written from the same render-clock value the renderer uploads to forward `Uniforms.time` that frame — phase coherence of animated curves depends on it. The renderer owns the buffers already — this task adds only the mesh-side layout, bind group creation, and per-frame rebind on buffer reallocation. Also append `curve_eval.wgsl` to the skinned-mesh shader source (`SKINNED_MESH_SHADER_SOURCE` currently excludes it with a "mesh never evaluates animated layers" comment — that comment becomes false here; update it): the Catmull-Rom samplers the scripted evaluation calls live there, not in `forward.wgsl`.

### Task 3: Mesh shader loop
Add the dynamic-light loop to `skinned_mesh.wgsl`'s fragment stage using the Task 1 helpers and Task 2 bindings: influence early-out, type dispatch, scripted animation, Lambert against the interpolated normal, per-light visibility hardwired 1.0 (the receipt spec replaces this). Sum into the existing `indirect + direct` composition before the albedo multiply.

### Task 4: Debug gating + tests
Wire the isolation gating through the group-2 uniform; add the mesh group-2 BGL assertion test. The no-change checks are STRUCTURAL/CPU tests, not pixel tests (no headless render-compare harness exists): assert the buffer bound at group-2 b0 is fed by `filter_dynamic_lights` output (static lights excluded by construction), and assert the loop contributes nothing when the uniform's light count is zero. The visual pass on a dev map with animated lights is a HUMAN checkpoint, not agent-verifiable.

## Sequencing

**Phase 1 (sequential):** Task 1 — forward refactor blocks the shader work.
**Phase 2 (sequential):** Task 2 — bindings block the loop.
**Phase 3 (sequential):** Task 3 — consumes Task 1 helpers + Task 2 bindings.
**Phase 4 (sequential):** Task 4 — verifies the assembled feature.

## Rough sketch

- Light record: `GpuLight` (4×vec4; type in `position_and_type.w`, falloff model in `color_and_falloff_model.w`, spot shadow slot in `cone_angles_and_pad.z`, cube shadow slot in `.w` — forward's loop reads both today; this spec ignores both, the receipt sibling consumes them). Rust packing: `lighting/mod.rs` `pack_light_with_slot`.
- The forward set is built by `filter_dynamic_lights` (`render/mod.rs` ~5426, `.filter(|(_, l)| l.is_dynamic)`) — binding the same buffer is what makes the D10 tier split hold by construction. Do NOT bind the shadow-candidate set (`is_dynamic || casts_entity_shadows`-filtered) — that set exists for pool assignment, not for lighting.
- Forward loop being mirrored: `forward.wgsl` ~1054–1180 (the `for (var i ... < light_count)` dynamic loop). Scripted descriptors: `scripted_light_descriptors` (forward group 3 b13) + Catmull-Rom sample buffer; evaluation ~1074–1121.
- Mesh group-2 binding map (PINNED across both specs — the receipt spec extends this layout, not the params uniform): this spec owns b0 lights, b1 influence, b2 descriptors, b3 anim samples, b4 params uniform; b5–b8 are reserved for the receipt spec's shadow entries (spot depth array, comparison sampler, light-space matrices, conditional cube array). Group indices 0/1/3/4 are pinned by `rendering_pipeline.md` §9 — only group 2 is free.
- Mesh fragment normal: interpolated skinned normal (`skinned_mesh.wgsl` vertex output); no `N_bump` on this path.

## Open questions

- None blocking. Helper-extraction granularity (one snippet vs. two) and exact binding numbers settle at implementation.
