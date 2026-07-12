# E17-B research notes

Source grounding behind the spec. File:line references are current at draft time and will drift — they are a starting map for the implementer, not durable contract. Verify before editing.

## Roadmap framing

- Roadmap item: `context/plans/roadmap.md` Epic 17, item **B - Kinematic visual parity** (item A shipped; item C — trigger/command surface — also shipped, so B is renderer-only and independent of the scripting seam).
- Naming drift: the shipped E17-A spec (`context/plans/done/E17--kinematic-platform-foundation/index.md`) forward-references "E17-B" for the `world.query`/command work. On the current roadmap that is **C** (shipped), not this spec. B is purely visual.

## Current mover render path (E17-A, shipped)

- Module: `crates/renderer/src/render/kinematic_brush.rs` (948 lines; ~275 are tests). Pass struct `KinematicBrushPass`.
- Shader: `crates/renderer/src/shaders/kinematic_brush.wgsl` (~289 lines). Concatenated with `sh_sample.wgsl`, `curve_eval.wgsl`, `light_eval.wgsl`, `shadow_sample.wgsl`.
- Recorded in its own "Kinematic Brush Pass" in `renderer_render_frame.rs` (~374–401), after the opaque world pass, before skinned meshes. Loads scene color + depth, writes depth (`depth_write_enabled: true`, `depth_compare: Less`).
- Bind groups (5 of 8): 0 camera (shared `uniform_bind_group`), 1 **shared material bind group** (albedo+specular+shininess+normal+sampler — but shader reads only b0 albedo + b5 sampler), 2 mover-owned light group (runtime dynamic lights + spot/cube shadow-pool resources), 3 mover instance transforms, 4 mesh SH superset (`sh_volume_resources.mesh_bind_group`). Test constant `KINEMATIC_BIND_GROUP_COUNT = 5`.
- Vertex format: world `WorldVertex` (stride 36). Pipeline declares only 4 attributes: position, base_uv, normal_oct, **tangent (location 3 — present but unused by the shader)**. Lightmap UV/layer present-but-zeroed, not declared as attributes; PRL validation rejects movers carrying lightmap data (`crates/level-format/src/kinematic_geometry.rs`).
- Lighting today: mesh-style — SH indirect (`sample_sh_indirect`) + baked static-direct SH (`sample_sh_direct`, gated by `dynamic_direct.has_direct`) + runtime dynamic loop (`accumulate_dynamic_direct`, diffuse only). Output `base_color.rgb * (ambient_floor + indirect + direct + dynamic)`.
- **Receiver already wired:** `accumulate_dynamic_direct` calls `sample_point_shadow` (kinematic_brush.wgsl ~219) and `sample_spot_shadow` (~231). Movers receive spot + cube shadows today.
- CPU path: `Renderer::set_kinematic_mover_draws(&[KinematicMoverInstance{mover_id, transform}])` (`renderer_models.rs`) → `upload_instances` (64-byte mat4 per instance). Game side: `KinematicMoverRenderCollector::collect` (`crates/postretro/src/runtime_movers.rs`) — iterates `ComponentKind::KinematicMover`, culls by mover AABB vs visible-cell bounds, uses the interpolated transform. Geometry install: `KinematicBrushPass::install_geometry` — concatenates all movers into one vertex/index buffer, per-*material* index ranges via `derive_material_ranges`.
- **Casting absent:** movers are not in the depth pre-pass and are drawn only in the beauty pass. `renderer_shadow_passes.rs` has zero `kinematic`/`mover` references. No mover depth-only pipeline exists.

## World material contract (parity target)

- Forward shader: `crates/renderer/src/shaders/forward.wgsl` (1273 lines), `fs_main`.
  - Albedo: `sample_color` → `sample_post_retro` (texel-grid reconstruction + `textureSampleGrad` through `aniso_sampler`, group 1 b5). Mover already matches this — pinned by `kinematic_brush_shader_matches_forward_post_retro_sampling`.
  - Normal: `sample_normal(t_normal)` (BC5 RG decode, z reconstruct); TBN from `world_tangent` + `bitangent_sign` (decoded from packed tangent in VS).
  - Specular: `sample_color(spec_texture).r` (R channel of `_s.png`, `R8Unorm`) × `blinn_phong(L, V, N_bump, light_color, spec_exp, spec_int)`; `spec_exp = max(material.shininess, 1.0)`. World runs this over its **static per-chunk spec-light list** with SDF half-res shadow visibility.
  - `blinn_phong` (forward.wgsl ~366): `H=normalize(L+V); color * pow(max(dot(N,H),0),exp) * intensity`. No Fresnel (retro).
- Shared material BGL (group 1): builder `material_bind_group_layout_entries()` (`crates/renderer/src/render/pipeline_layout.rs`), BGL created as `texture_bind_group_layout` (`renderer_init_resources.rs`). Bindings: 0 albedo (`Rgba8UnormSrgb`), 2 specular (`R8Unorm`), 3 `MaterialUniform` (shininess, 32 B), 4 normal (`Bc5RgUnorm`), 5 `aniso_sampler`.
- Shininess is a per-material **uniform** constant, not a texture: `Material::shininess()` (`crates/render-data/src/material.rs`) — Metal 64, Glass 96, Neon 32, Wood 16, Concrete 4, Grate 8, Default 32.
- Sidecar resolution: `.prm` bundle (blake3 cache key) carries slot 0 diffuse, slot 1 specular (`_s`), slot 2 normal (`_n`); `TextureSlotPolicy::WorldBundle` uploads all three (`loaded_texture.rs`), missing → neutral placeholder. Per-material bind group built by `build_material_bind_group` (`material_plan.rs`). Movers reuse world materials (`FaceMeta.texture_index` → same `gpu_textures` slot), so the full bundle already resolves.
- **Conclusion:** material parity is shader-side only — declare group-1 b2/b3/b4, forward the tangent, TBN + Blinn-Phong. No BGL, upload, or vertex-format change. Camera position for `V` is already in the shared group-0 buffer (forward reads `uniforms.camera_position`); the mover shader just declares the field.

### The specular-light nuance (why static-light specular is deferred)

The mover's group 2 is the **runtime dynamic-light** path, not the static `spec_lights` chunk buffer. World specular is keyed to baked BVH-chunk light lists a moving object can't index. Specular under dynamic lights is a drop-in in `accumulate_dynamic_direct` (light vector + view vector both available, shadow term already sampled). Specular under baked static lights would need the billboard precedent (per-object cell→chunk→spec-light resolution) — new bindings + per-mover chunk resolution, materially larger. Hence the spec scopes specular to dynamic lights and defers static-light specular.

## Shadow-caster mechanism (template to mirror)

- Depth-only skinned pipeline: `skinned_depth.wgsl` (~88 lines). Group 0 = `LightSpaceUniforms{light_proj}` (per-render, dynamic offset); group 3 = shared bone palette + instance SSBO (`@builtin(instance_index)`). Vertex-only, `fragment: None`. Projection `light_proj × (model × (skin × pos))`. Owner `MeshPass::depth_pipeline` (`mesh_pass.rs`) — `Depth32Float`, `depth_compare = Less`, bias `constant: 2, slope_scale: 1.5`. One pipeline serves spot slots and cube faces (matrix + target view supplied per render pass).
- Recorder: `MeshPass::record_skinned_depth(pass, plan, filter, light_space_bind_group, dynamic_offset, cone_planes)` (`mesh_pass.rs`). Per-instance CPU cone cull via `instance_casts_into_cone` → `aabb_intersects_frustum(bounds.transformed(...), cone_planes)` (`crates/render-cpu/src/mesh_instances.rs`, math in `crates/render-data/src/cone_frustum.rs`). Entities are not in the world BVH; this is the per-slot cull.
- Recording sites: `renderer_shadow_passes.rs` (1141 lines) — `record_spot_shadow_depth` (dynamic slot: `Clear` world draw + entity occluders same pass; promoted slot: copy cached depth → dedicated `LoadOp::Load` entity pass) and `record_cube_shadow_depth` (per face, same shape). Entity occluders gated on `slot_entity_eligible[slot]`.

## Shadow pools + receiver sites

- Spot: `crates/renderer/src/lighting/spot_shadow.rs` — `SpotShadowPool`, `SHADOW_POOL_SIZE = 96`, `Depth32Float`, 1024². Per-slot light-space matrix in `matrices_buffer` (group-5 b2, fragment sampling) + `shadow_vs_uniform_buffer` (depth-pass VS, dynamic offset `slot*stride`).
- Cube: `crates/renderer/src/lighting/cube_shadow.rs` — `CubeShadowPool`, `CUBE_COUNT = 6` × 6 faces, 512². Per-face 90° matrices from `cube_face_matrices`. Gated by `CUBE_ARRAY_TEXTURES` (absent → pool `None`, point shadows cleanly off via `strip_point_shadow_cube`, spot unaffected).
- Promoted static-light cache: `promoted_depth_cache.rs` — `MAX_PROMOTED_SPOT = 8` (1024² layers), `MAX_PROMOTED_CUBE = 2` (×6 = 12 faces, 512²). Lifecycle: cold frame renders world depth into the cache (`needs_world_render = !warm`); every frame copies cache → live pool; warm frames skip world render + cone-cull; entity occluders draw into the live pool with `LoadOp::Load` after the copy. **Movers must cast into the live pool only, never the cache** (mirror entity occluders).
- Pool structs own GPU resources + occupancy only; render-pass recording lives in `renderer_shadow_passes.rs`. A mover-caster spec touches the recording file, not the pool structs.
- Receiver sites (all already sample the pools — no receiver change needed): world `forward.wgsl` (~1181 point, ~1206 spot), mesh `skinned_mesh.wgsl` (~529 point, ~558 spot), **mover `kinematic_brush.wgsl` (~219 point, ~231 spot)**. Shared PCF helpers in `shadow_sample.wgsl`.

## Budget / limits

- `max_bind_groups = 8` (`renderer_init_resources.rs`); groups 0–6 allocated, one free. The mover depth-only draw binds only group 0 (light-space uniform) + a mover-instance group → no ninth group.
- Required feature `TEXTURE_COMPRESSION_BC` (material textures, not shadow-specific). `TIMESTAMP_QUERY` optional (timing). `CUBE_ARRAY_TEXTURES` gates the cube pool (graceful). A mover-occluder path adds no new required feature.

## Static-light promotion for movers (specular + casting under static lights)

The decision to give movers specular under **promoted static lights** (not the full static-chunk list) rests on these facts.

- **Promotion is skinned-mesh-driven only today.** The gate `selected_static_light_has_shadow_entity` (`crates/renderer/src/render/renderer_light_slots.rs` ~1079–1105) iterates only the skinned `MeshFramePlan` instances; `update_dynamic_light_slots` (~96–108) `continue`s past a selected static light with no intersecting skinned mesh. The plan is built skinned-only at `renderer_render_frame.rs` (~71–82, `promotion_mesh_frame_plan` from `mesh_draws`). Movers (`KinematicMoverInstance`) never enter `mesh_draws`/`MeshFramePlan`. **Consequence:** a static light near a mover-only scene is not promoted. The spec adds mover world-bounds to this gate (Task 3).
- **The mover loop already iterates promoted records.** `accumulate_dynamic_direct` (`kinematic_brush.wgsl` ~170) loops `kinematic_light_params.light_count`, written as `full.total_light_count` (`renderer_render_frame.rs` ~366) = `full.light_count + full.promoted_static_records.len()` (`renderer_light_slots.rs` ~851). The mover's group-2 `lights`/`light_influence` bind the **same** `full.lights_buffer`/`full.influence_buffer` the mesh pass binds (`renderer_full_init.rs` ~425–448; `renderer_resources.rs` ~344–358), with promoted records appended at the tail (`build_count_split_light_upload` ~1134–1149). So a specular term in that loop covers dynamic + promoted static lights with no new bindings.
- **Crossfade weight `w` is pre-folded into the promoted record's color** on the CPU (`renderer_light_slots.rs` ~1134–1137: `weighted.intensity *= record.weight`). Both mesh and mover diffuse terms use `effective_color = light.color_and_falloff_model.xyz` (already carrying `w`), so a specular term scaled by `effective_color` inherits `w` for free. The `(1−w)` baked-direct half lives in the composed direct SH atlas (`base − Σ wᵢ·deltaᵢ`), which the mover already samples via the shared group-4 `mesh_bind_group` (`renderer_render_frame.rs` ~397). The baked direct atlas is diffuse L2-SH irradiance only (no view-dependent lobe), so runtime specular does not double-count.
- **Movers cast no shadows today.** The dynamic and promoted spot/cube entity-occluder passes (`renderer_shadow_passes.rs` ~312–336, ~549–572, ~394–411, ~626–641) draw only `mesh_pass.record_skinned_depth(...)`. No kinematic depth pipeline exists. Casting under a promoted static light needs both (i) the light promoted (Task 3) and (ii) a mover occluder drawn into the promoted `LoadOp::Load` pass (Tasks 2/4).

**Net:** specular-under-promoted-static and promoted-slot casting share one enabler — movers in the promotion-relevance set — so both land in one wave. The full per-chunk static-light specular (billboard precedent) stays the deferred escalation.

## Design-review outcomes (round-2 consensus)

- **Specular gated to the promoted-static tail.** The mover light loop packs dynamic-tier `level_lights` first, promoted records appended (`renderer_light_slots.rs` ~1114–1149); `total_light_count = light_count + promoted_static_records.len()` (~851). So `i >= dynamic_light_count` selects exactly the promoted-static tail, and `level_lights` is dynamic-tier only (static baked lights live in the lightmap / SH / `spec_lights`, never this buffer). A `dynamic_light_count` field in the mover params uniform gates specular to promoted records with no new binding.
- **No dynamic-light specular anywhere today — incomplete, not intentional (owner).** Forward's static specular uses `spec_lights` only (`forward.wgsl` ~1021–1091); its dynamic loop is diffuse-only (~1093–1219); meshes too (§9). Mover specular matches: static-only this wave. Engine-wide dynamic specular is a future perf-gated spec across all surface types; when it lands, movers get it nearly for free (they already loop the dynamic records — only the gate opens).
- **SH-dominant escalation dependency (grounded).** The baked static-direct atlas stores octahedral irradiance per direction, not raw SH coefficients: `sample_probe_atlas_tex` does `oct_encode_unquantized(dir)` → `textureSampleLevel` (`sh_sample.wgsl` ~98–119); "the two atlases differ only in the radiance they store, not in probe layout" (~221). No L1 coefficient band to read — dominant-direction extraction needs a multi-tap reconstruction (weighted centroid/argmax), not a free read. That is why SH-dominant is the deferred escalation, not the primary path.
- **Promotion contention.** Movers share the fixed 8-spot / 2-cube promoted budget with meshes via the existing ranker (`assign_shadow_pool_slots_with_promoted_static` ~879). A closer mover-light out-ranking a farther mesh-light is correct behavior, not a bug; verify existing entity-shadow tests hold in a mixed mover+mesh scene.
- **Generic rigid occluder.** The new depth path is shaped as a generic rigid-instance occluder (position-only VS; recorder over buffers + transforms + ranges + AABBs), movers the sole caller this wave. Position-only rigid is simpler *and* more general than faking a degenerate bone on `skinned_depth.wgsl`; future rigid casters (E17-F, debris, chunk clusters, props/viewmodels) join as callers without a fork.
- **Shared material-shading WGSL.** The snippet is `blinn_phong` / `sample_normal` / TBN only — consumed by **forward + mover**, not the skinned-mesh shader. Correction after anchor review: `skinned_mesh.wgsl` is diffuse-only and defines none of these (comment at `skinned_mesh.wgsl:404`: "no specular and no normal-map perturbation"); it uses plain `textureSample`, not `sample_post_retro`. Appending the material snippet there would leave `sample_normal`→`sample_post_retro` unresolved (naga rejects even unreached fns) and collide on `oct_decode`. `oct_decode` is therefore **not** in the snippet — each shader keeps its own. Sharing is by concatenation (§8): `blinn_phong` and TBN are arg-only, but `sample_normal` reads the module-scope `aniso_sampler` (group 1 b5) both consumers declare — so it shares by binding-name resolution like `sh_sample.wgsl`, not because it is binding-free (the earlier "skin_matrix precedent does not apply" framing was inaccurate; `sample_normal` is a module-scope-binding reader too, just one all consumers satisfy identically).

## Oversized files a B implementation touches

- `crates/renderer/src/render/renderer_shadow_passes.rs` — 1141 (Task 4 wiring; conditional split).
- `crates/renderer/src/render/renderer_light_slots.rs` — ~1281 (Task 3 promotion-gate change; localized).
- `crates/renderer/src/render/kinematic_brush.rs` — 948 (Task 1 shader-source wiring + per-mover ranges; Task 2 avoids growing it via a new module).
- `crates/renderer/src/render/mesh_pass.rs` — 3362 (reference template only; not extended).
- `crates/renderer/src/shaders/forward.wgsl` — 1273 (reference for normal/specular WGSL; source of a possible shared helper).
