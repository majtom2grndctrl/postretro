# E17-B — Kinematic Visual Parity

> **Epic:** 17 — Kinematic Geometry and Moving Platforms.
> **Depends on:** E17-A (kinematic platform foundation) ✓. Independent of E17-C (trigger/command surface) ✓.
> **Scope note (assumed, pending confirmation):** specular responds to dynamic lights only; static-light specular deferred (see Open questions). One spec, one wave.

## Goal

Bring kinematic brush movers to the static world's visual contract. Movers already draw SH/dynamic-lit and already receive dynamic shadows; two gaps remain. Close them: the mover shader samples the same material slots static geometry does — normal maps, specular, and material shininess — and active movers cast into the runtime shadow pools, so a moving platform reads like the static geometry around it.

## Scope

### In scope

- **Material parity (shader).** The mover fragment shader samples the normal map, specular map, and per-material shininess from the shared material bind group it already binds, and forwards the tangent it already carries. Normal-map perturbation applies to the diffuse response under all of the mover's light sources (SH indirect, baked static-direct SH, runtime dynamic). Blinn-Phong specular is computed under the mover's **runtime dynamic lights**, attenuated by the same per-light shadow term the diffuse loop already samples.
- Diffuse Post-Retro filtering stays byte-identical to today (existing parity test unchanged).
- **Shadow casting.** A depth-only kinematic occluder pipeline — position-only, per-instance mover transform, projected by the per-render light-space matrix — mirrors the skinned depth pipeline without skinning.
- Mover occluders record into every occupied slot the entity occluders already serve: dynamic spot slots, dynamic point cube faces, promoted-spot slots, and promoted-cube faces, with `LoadOp::Load` after the world/cached depth.
- Per-slot/face CPU cone cull of movers, using the conservative world-space mover AABB the render collector already computes (movers are not in the world BVH).
- Mover→world, mover→mesh, and mover→mover shadows (mover-side shadow *receipt* already exists).

### Out of scope

- Specular under **baked static lights** (the per-chunk static spec-light list). Deferred — see Open questions.
- Baked lightmap / SDF occlusion cast by movers. Moving objects never enter static bakes (roadmap invariant).
- Static lightmap sampling or lightmap-UV vertex attributes on movers; those bytes stay zeroed.
- Any change to mover motion, collision, carry, or networking (E17-A owns those).
- Scripting / `world.query` / mover command surface (shipped as E17-C).
- Rotation and angular carry (E17-D), doors/blocking (E17-E), visibility-bearing movers and moving shadow-casting lights (E17-F).
- Any new bind group, new required adapter feature, or raising `max_bind_groups` above 8.

## Acceptance criteria

- [ ] A mover face whose material has an `_n` normal-map sibling shows bump shading response; a mover and a static wall sharing the same material and lit by the same lights shade consistently at matching orientation.
- [ ] A mover face with an `_s` specular sibling shows a Blinn-Phong highlight under a dynamic spot or point light — tighter with a shinier material (Metal), broader with a dull one (Concrete) — and the highlight is suppressed where that light's shadow term occludes the fragment.
- [ ] The existing mover diffuse Post-Retro filtering parity test still passes (no diffuse regression).
- [ ] An active mover casts a shadow onto world geometry under a dynamic spot light and under a dynamic point light.
- [ ] An active mover casts a shadow onto a skinned mesh standing beside it.
- [ ] With two movers, one casts a shadow onto the other.
- [ ] A mover casts a shadow onto world/mesh receivers under a promoted static light.
- [ ] The cast shadow tracks the mover each frame while it moves and is stable when the mover is docked/idle.
- [ ] A mover outside a light's shadow cone/frustum is not drawn into that slot (per-slot cull — assertable via the occluder submission count, mirroring the entity-occluder counters).
- [ ] Maps with no movers, and static maps generally, render unchanged.
- [ ] No new `unsafe`; no non-renderer module imports `wgpu` or creates GPU resources (grep/review gate).
- [ ] The mover material and occluder paths add no bind group (`max_bind_groups` stays 8) and require no new adapter feature.

## Tasks

### Task 1: Mover shader material parity

Extend the mover fragment shader to reach the world material contract using resources the mover pipeline already binds (the shared material bind group, group 1) and vertex data it already carries (the packed tangent at location 3). Declare the group-1 bindings the shader omits today — the specular texture, the material uniform (shininess), and the tangent-space normal map — matching the world forward shader's binding indices. Forward a world-space tangent and bitangent sign through the vertex shader, reconstruct the TBN basis, and perturb the shading normal from the normal-map sample before both the indirect and direct lighting terms. Add a Blinn-Phong specular term inside the existing dynamic-direct light loop: per dynamic light, a Blinn-Phong contribution scaled by that light's existing attenuation, cone, and shadow-visibility factors, where specular intensity is the specular map's red channel, the exponent is `max(material.shininess, 1.0)`, and the view vector uses the camera world position. The camera position is already present in the shared camera uniform buffer the mover binds at group 0 — declare that field in the mover's camera struct (a WGSL struct-declaration change; no Rust or bind-group change). Keep the diffuse Post-Retro sampling byte-identical (its parity test must still pass). To keep the mover's normal/specular math lock-step with the world path, either extract the shared normal-decode and Blinn-Phong WGSL into a concatenated helper both shaders append, or duplicate them into the mover shader with a byte-identity parity test, following the `skin_matrix` precedent. This is shader-side plus the minimal Rust to declare the added bindings in the mover pipeline's material group usage; the material bind-group layout and per-material bind groups already carry all slots, so no upload-path or BGL change is needed.

### Task 2: Depth-only kinematic occluder pipeline

Add a renderer-owned depth-only pipeline and recorder for mover occluders in a **new module**, not by extending the 948-line mover pass file. Mirror the skinned depth pipeline (`skinned_depth.wgsl` / `MeshPass::depth_pipeline`): vertex-only, no fragment stage, `Depth32Float`, `depth_compare = Less`, the same depth bias the skinned occluders use, projecting `light_space_proj × (model × position)`. The vertex layout reads only position from the shared mover vertex buffer; group 0 is the per-render light-space-matrix uniform (bind the renderer's existing spot `shadow_vs_bind_group` or cube `cube_shadow_vs_bind_group` with the per-slot/per-face dynamic offset, exactly as the skinned occluders do); a second group binds the mover per-instance model-transform buffer the beauty pass already uploads. Expose a recorder that iterates active movers, cone-culls each by its conservative world-space AABB against the supplied frustum planes (reusing the shared cone-frustum math), issues one depth draw per surviving mover over that mover's index range, and returns the submitted count. This requires a **per-mover index-range table recorded at geometry install** (the concatenation already knows mover boundaries; the beauty pass records per-*material* ranges — add per-*mover* ranges beside them). The recorder assumes the caller has already opened the render pass and laid down the world/cached depth; it never clears and never touches the promoted-depth cache — movers are dynamic occluders drawn into the live pool each active frame, exactly like entity occluders. No new bind group beyond the two it binds; no new adapter feature.

### Task 3: Wire mover occluders into the shadow record loop

Call the Task 2 recorder from the shadow depth passes so movers cast into every slot the entity occluders already serve. Thread the per-frame mover occluder set — per-mover model transform, world-space AABB, and index range — from the game side into the shadow-pass recording path, alongside the mesh frame plan; call-site wiring in `main.rs` only. In `renderer_shadow_passes.rs`, after the existing entity-occluder draw in each of the four occluder contexts — the dynamic spot pass, each dynamic cube face pass, the promoted-spot `LoadOp::Load` entity pass, and the promoted-cube `LoadOp::Load` entity pass — invoke the recorder with that slot/face's light-space bind group, dynamic offset, and cone planes (the same cone matrix the entity occluders cull against). Gate mover occluders on the same per-slot eligibility the entity occluders use, so a slot that renders entity shadows also renders mover shadows. Keep these additions to call-site wiring plus a small shared cull invocation; if the recording functions cannot absorb the call sites without new logic, split the file along the spot/cube seam first (behavior-preserving), then add the wiring. Movers already sample the spot and cube pools as receivers, so no receiver-side change is needed — mover→mover shadows fall out of this task.

### Task 4: Demo map, verification, and docs

Extend a dev map (or the E17 platform dev map) with a moving platform authored against a material that has `_n` and `_s` siblings and a shiny material class, positioned under a dynamic spot light and a dynamic point light, beside a static wall of the same material, near a skinned mesh, and near a second mover — so every acceptance criterion is directly observable. Verify normal/specular parity against the static wall and the moving shadow onto world, mesh, and the other mover. Update the durable contracts that changed: `context/lib/rendering_pipeline.md` §3 / §7.3 for the mover material path (now full world-material parity, not albedo-only) and a new mover depth-occluder note in §7.1 (movers cast into dynamic and promoted spot/cube slots via a depth-only pipeline, cone-culled per slot, never into the static-depth cache).

## Sequencing

**Phase 1 (concurrent):** Task 1 (mover material shader), Task 2 (depth-only occluder pipeline + recorder) — disjoint: a color-shader edit vs. a new depth pipeline module.
**Phase 2 (sequential):** Task 3 — wires the Task 2 recorder into the shadow passes; consumes its recorder and the per-mover index ranges.
**Phase 3 (sequential):** Task 4 — demo, verification, docs; consumes Task 1's shading and Task 3's casting.

## Rough sketch

- Mover shader: `crates/renderer/src/shaders/kinematic_brush.wgsl` — declare group-1 bindings 2 (specular), 3 (`MaterialUniform`), 4 (`t_normal`); add world tangent + bitangent sign to `VertexOutput`; reconstruct TBN and perturb the normal (mirror `forward.wgsl`'s `sample_normal` + TBN block); add Blinn-Phong in `accumulate_dynamic_direct`; declare `camera_position` in the mover `CameraUniforms` struct (already in the bound buffer).
- Mover pass Rust: `crates/renderer/src/render/kinematic_brush.rs` — the material BGL and per-material bind groups already include all slots; only the pipeline's shader-source/material-group usage changes. Record per-mover index ranges at `install_geometry` beside the per-material ranges.
- New caster module: e.g. `crates/renderer/src/render/kinematic_depth.rs` + `crates/renderer/src/shaders/kinematic_depth.wgsl`, mirroring `skinned_depth.wgsl` and `MeshPass::depth_pipeline` (`mesh_pass.rs`). Reuse the beauty pass's per-mover instance transform buffer and shared geometry buffers.
- Cone-cull math: `crates/render-data/src/cone_frustum.rs` (`cone_frustum_planes`, `aabb_intersects_frustum`); per-mover world AABBs from `KinematicMoverRenderCollector` (`crates/postretro/src/runtime_movers.rs`).
- Wiring: `crates/renderer/src/render/renderer_shadow_passes.rs` (`record_spot_shadow_depth`, `record_cube_shadow_depth`, and the two promoted `LoadOp::Load` passes).

**Oversized-file watch:** `renderer_shadow_passes.rs` (1141 lines) and `kinematic_brush.rs` (948, ~30% tests). Task 2 avoids growing the mover pass file by adding a new module; Task 3 keeps `renderer_shadow_passes.rs` additions to call-site wiring, with a conditional behavior-preserving split along the spot/cube seam only if the call sites cannot land cleanly.

## Open questions

- **Specular under baked static lights (deferred decision).** Static world surfaces get specular from a baked per-chunk spec-light list keyed to BVH leaves; a moving object can't index it directly. Reaching that parity would follow the billboard precedent (resolve the mover's cell → chunk → static spec-light list and run the world spec loop), adding static-light bindings to the mover pass and a per-mover chunk resolution — a materially larger change that would likely split this into two waves. Deferred; revisit if statically-lit rooms show a visible specular gap between a platform and its static neighbors. Recorded as a decision, not an omission.
- **`renderer_shadow_passes.rs` split.** Whether Task 3's four call sites justify a behavior-preserving split of the 1141-line file is left to the implementer; the task instructs wiring-only additions first.
