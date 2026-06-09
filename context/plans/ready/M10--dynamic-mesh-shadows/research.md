# Research — Dynamic Mesh Shadow Casting

Code anchors and external findings behind the spec. Line numbers drift; treat as starting points.

## Code anchors (confirmed this session)

**Shadow pool** — `crates/postretro/src/lighting/spot_shadow.rs`
- `SHADOW_POOL_SIZE = 64` (:52). Docs/old plans saying 12 are stale; `forward.wgsl:412` "(0..7)" comment is stale.
- **Latent bug (this session):** the `LightSpaceMatrices.m` uniform is hardcoded `array<mat4x4<f32>, 12>` in both `forward.wgsl:241` and `fog_volume.wgsl:74`, while the Rust pool is 64 and `rank_lights` assigns up to 64 slots. Any slot ≥ 12 indexes that array out of bounds → wrong/clamped projection matrix. The 12→64 bump never updated the WGSL. Spec Task 6 raises the pool to 96 and brings both shader arrays to 96 in lockstep, with a const/size pin so they can't drift again.
- `SpotShadowPool` (:76); `slot_cone_matrices: [Option<Mat4>; SHADOW_POOL_SIZE]` (:103) — already in tree; the per-slot light-space matrix the skinned-depth pass needs.
- `light_space_matrix(light)` (:19): `perspective_rh(fov_y = 2·cone_outer clamped, aspect 1, SHADOW_NEAR_CLIP, far = falloff_range)`. `SHADOW_NEAR_CLIP = 0.1` (:12).
- `rank_lights` (:331); eligibility `is_dynamic || casts_entity_shadows` then `LightType::Spot` (:355). Array texture (layers), `Depth32Float`, comparison sampler, group 5.
- Pool depth pass renders **world geometry only** today; entity occluders are the gap.

**Mesh pass** — `crates/postretro/src/render/mesh_pass.rs`, `mesh_instances.rs`, `shaders/skinned_mesh.wgsl`
- `skin_matrix(joints, weights, base)` (skinned_mesh.wgsl:197) — reusable skinning kernel. `BonePaletteEntry` (skinned_mesh.wgsl:71).
- `palette_buffer` / `instance_buffer` written via `queue.write_buffer` inside `render_frame` (:534, :557) — **after** the shadow passes in frame order.
- `MAX_PALETTE_ENTRIES = 4096`, `MAX_INSTANCES = MAX_PALETTE_ENTRIES` (mesh_instances.rs:24,37).
- Pipeline layout: group 0 camera, group 1 material, **group 2 = None (unallocated, reserved for future dynamic-direct light loop)**, group 3 instance SSBO, group 4 SH superset. No group 5 (shadow pool) bound.
- CPU leaf-cull (`mesh_visible*` :618,632), instanced `draw_indexed` per submesh — not in the BVH/indirect path.
- **No per-model/instance bound exists.** `SkinnedMesh` (mesh.rs:65) and `UploadedModel` (mesh_pass.rs:99) carry no AABB/radius; `PlannedInstance` (mesh_instances.rs:58) has only `transform`. The per-light entity cone cull needs one → Task 1 adds a load-time local bound (glTF accessor min/max or vertex scan) on the uploaded model, transformed per instance.

**Forward** — `crates/postretro/src/shaders/forward.wgsl`
- Per-light loop (:913+), influence-sphere early-out, `sample_spot_shadow(slot, world_pos, light_proj)` (:413) multiplies attenuation; slot from `bitcast<u32>(light.cone_angles_and_pad.z)` (:1004); `light_space_matrices` group 5 binding 2 (:243).
- Point lights cast no shadow today. Sampled-texture budget at 13/16 (Metal hard cap 16).

**Lighting state**
- Entities lit by baked SH-indirect + baked SH-direct (skinned_mesh.wgsl `sample_sh_direct`). No runtime per-light loop on meshes. The contrast "shaded side" is the baked SH-direct term (`lighting--entity-direct-sh`, in-progress).
- Baked SH-direct **is occlusion-tested** through the world BVH (`soft_visibility`, defined in `lightmap_bake.rs:1227`, called from `sh_bake.rs:892`; `segment_clear` `sh_bake.rs:517`; test `direct_sh_shadowed_probe_is_dimmer_than_lit_probe`, `direct_sh_bake.rs`). Soft (32-sample area), coarse (probe spacing). Only `static_light_map` lights contribute; `sdf` and `is_dynamic` excluded. → baked world→entity already handled.

**Authoring flags** — `sdk/TrenchBroom/postretro.fgd`, `crates/level-compiler/src/format/quake_map.rs`, `map_data.rs`, `crates/postretro/src/prl.rs`
- `cast_shadows`: hardcoded `true` (`quake_map.rs:525`), no KVP, never read at runtime (only an unreachable bake branch, `lightmap_bake.rs:1235`). **Dead — but wide.** It is on the PRL wire (`level-format/alpha_lights.rs`, `pack.rs:122`, `prl.rs:179/398`), in the bake (`lightmap_bake.rs`, `sh_bake.rs`), runtime light packing (`lighting/mod.rs`), and the **scripting surface contract**: `sdk/types/postretro.d.ts:281` `LightComponent.castShadows` (+ `.d.luau`), `scripting/components/light.rs`, `primitives/light.rs`, `systems/light_bridge.rs`, `refresh_plan.rs`, `builtins/data_archetype.rs`. Retiring it = ~12 files across 3 crates + SDK types + a breaking PRL-record change. Decision (owner): keep it in this plan, fully scoped (Task 3b).
- `casts_entity_shadows`: from `_cast_entity_shadows` (0/1); currently gates the whole pool (world geometry), misnamed vs behavior.
- `is_dynamic`: classname-derived (`light_dynamic`/`light_dynamic_spot`), not a KVP.
- `shadow_type` (`StaticLightMap`/`Sdf`): from `_shadow_type`; disjoint direct-shadow technique for static lights (enforced no-double-count).

**Shipped dependency** — `shadow-cone-cull` (code merged; plan folder still in `in-progress/` pending "land the plane")
- `lighting/cone_frustum.rs`: `cone_frustum_planes(&Mat4) -> [Vec4;6]`, `aabb_intersects_frustum(&Aabb, &[Vec4;6]) -> bool`, `cone_enclosing_aabb(&Mat4) -> Aabb` (all `pub(crate)`). CPU mirror of `bvh_cull.wgsl`'s `is_aabb_outside_frustum`. Reuse for per-light entity culling.
- `shadow_cull::ShadowCullPipeline` (`crates/postretro/src/shadow_cull.rs`): `dispatch_occupied_slots(...)` (GPU per-slot cone cull of world BVH) + `draw_slot_indirect(pass, slot, None)`. Owned by `Renderer` as `shadow_cull: Option<ShadowCullPipeline>`.
- Spot depth pass in `render/mod.rs`: `for slot in used_slots` → new "Spot Shadow Depth Pass" render pass per slot → `shadow_cull.draw_slot_indirect`. Entity occluder draws append here. `slot_cone_matrices` populated in `update_dynamic_light_slots`.
- World occluders are GPU-BVH-culled; entities aren't in the BVH → entity culling is CPU per-instance. No conflict.

## External findings

**Filtering for hard edges.** Classic depth-compare + slope-scaled bias; hardware 2×2 PCF (or a small fixed-radius kernel) for anti-aliased-but-crisp edges. Variance family (VSM/ESM/EVSM/MSM) is for *soft* prefilterable penumbrae — off-aesthetic and extra storage; keep the engine's Chebyshev moments for probe visibility only.
- LearnOpenGL Shadow Mapping; MS "Common Techniques to Improve Shadow Depth Maps"; MJP "A Sampling of Shadow Techniques".

**Point lights = cube, not dual-paraboloid.** DPSM warps on low-poly (per-vertex paraboloid projection) — worst option here. 6-face cube is the pragmatic standard; no robust single-pass omni path in core WebGPU.
- NVIDIA GPU Gems Ch.12; LearnOpenGL Point Shadows; DPSM critiques (Diary of a Graphics Programmer; gamedev.net).

**Cube-array feasibility (wgpu).** `texture_depth_cube_array` + `textureSampleCompareLevel` works on Metal/DX12/Vulkan; gated by `DownlevelFlags::CUBE_ARRAY_TEXTURES` (present on native, absent on GLES/WebGL only). No `unsafe`. Bevy ships exactly this (`shadow_sampling.wgsl`): direction-vector sampling, PCF in an orthonormal basis around the light-local vector, per-face largest-axis depth + normal-offset bias. ~24 MB per 1024² Depth32Float cube. +1 sampled-texture binding (→14/16 Metal).
- Bevy `crates/bevy_pbr/src/render/shadow_sampling.wgsl`; wgpu `DownlevelFlags`/`TextureViewDimension` docs; gfx-rs/wgpu #1746; WGSL spec depth texture types.

**Atlas/array & VRAM.** Spot pool is a fixed init-time allocation = `SHADOW_POOL_SIZE × 1024² × 4 B`: 256 MB at 64, **384 MB at 96** (the spec's capacity bump). Measured real on a 4 GB Radeon Pro 5500M at 64. Spot and point cannot share a texture (2D-array vs cube-array sampler dimension), so the cube pool is necessarily separate; the constraint is the sampled-texture binding budget (13/16, Metal hard 16 — cube → 14/16), which caps how many shadow classes can coexist. Size the cube pool to realistic concurrent demand (PVS-culled + ranked), not worst case.

**Static/dynamic split (DOOM model).** Cached-static + recompute-dynamic depth is the canonical many-light technique — but motivated by *moving* lights. Lights are fixed here, so it's an optimization, deferred.

## Decisions taken (and why)
- **Cast only from `is_dynamic` lights.** Only lights with a separable runtime forward term can be per-light shadowed without double-counting a baked lightmap.
- **Entities receive no shadows in v1.** No runtime light loop on the mesh shader (group 2 unallocated); deferred to *Dynamic mesh direct lighting*. Baked world→entity already covered by SH-direct.
- **One combined plan, spot-first.** Spot and point share the skinned-depth spine and a pool reshape; building the cube path validates the cube-ready seam. Spot front-loaded so point is a cuttable tail.
- **Cube-array (Option A) over packed faces.** Seamless hardware filtering + trivial sampling beat saving one binding.
