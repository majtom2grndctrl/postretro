# Research — Mesh Render Pass + `MeshComponent`

Code-grounding for the spec. All citations verified against source at draft time. Ephemeral — superseded by code once shipped.

## Locked contracts (the slice + glTF loading, already shipped)

- **Skinned vertex layout** — `model/mesh.rs:21-35` (`SkinnedVertex`), GPU layout derived in `render/mesh_pass.rs:171-213`. Stride 32: position `Float32x3`@0, base_uv `Unorm16x2`@12, normal_oct `Uint16x2`@16, tangent_packed `Uint16x2`@20, joints `Uint8x4`@24, weights `Unorm8x4`@28. `MAX_JOINTS = 256` (`model/mesh.rs`). Rigid = degenerate single-bone (joint 0 @ weight 255).
- **Bone palette + per-instance** — `BonePaletteEntry { matrix: [[f32;4];4] }` 64 B (`model/mod.rs:43-45`). Shared storage buffer, per-instance base index, `base + joint` addressing (`skinned_mesh.wgsl:114-120`). Per-instance uniform 80 B = `model: mat4` + `base_and_pad: vec4<u32>` (base at byte 64); test-pinned `mesh_pass.rs:499-535`.
- **Mesh-pass shape** — `mesh_pass.rs`: own pipeline layout, groups `[Some(camera), Some(material), None, Some(instance)]` (`mesh_pass.rs:142-151`); `front_face: Ccw` + `cull_mode: Back`; glTF→engine basis identity (`mesh_pass.rs:19-30`); depth `Less` + `depth_write_enabled: true` (`mesh_pass.rs:227-233`), recorded in a dedicated pass loading the depth attachment writably, after opaque forward / before billboards (`render/mod.rs` ~4133-4205). Slice draws one direct `draw_indexed` (`mesh_pass.rs:340-397`).
- **Shader** — `skinned_mesh.wgsl`: group 0 camera (b0), group 1 material (b0 base + b5 aniso sampler), group 2 unallocated, group 3 instance (b0 palette storage, b1 instance uniform). Fragment is flat-lit `FLAT_AMBIENT = 1.0` (`skinned_mesh.wgsl:160`). Vertex already computes `out.world_normal` (`skinned_mesh.wgsl:148`) but **not** world position. Normal transform upper-3×3 is rotation/uniform-scale-only (`skinned_mesh.wgsl:144-147`).
- **Contracts doc** — `rendering_pipeline.md` §9 (committed vs. provisional: vertex set + palette scheme committed; lighting/instancing/depth-variant held open). §10 group budget + target hardware.

## Render integration (current, single-model)

- `set_mesh_draws(&mut self, draws: &[Mat4])` → field `mesh_draws: Vec<Mat4>` (`render/mod.rs:2750`, field `:911`).
- Assert `mesh_draws.len() <= 1` at `render/mod.rs:4140`.
- Camera bind group set at slot 0: `mesh_enc.set_bind_group(0, &self.uniform_bind_group, &[])` (`render/mod.rs:4197`).
- Palette/anim per frame: `sample_clip(&anim.clip, &anim.skeleton, now_seconds as f32, &mut self.bone_palette_scratch)` then `update_palette(&queue, 0, &scratch)` (`render/mod.rs:4164, 4173`). **Time source = render wall-clock** `now_seconds`, flagged TODO for tick time (`render/mod.rs:4156`).
- `load_skinned_model(&mut self, model_path: &Path, prm_cache_root: &Path) -> Option<Vec<String>>` (tags) (`render/mod.rs:2691`); uploads via `MeshPass::set_model(&device, &SkinnedMesh, Vec<(BindGroup, Range<u32>)>)` (`mesh_pass.rs:272`). Renderer holds at-most-one `UploadedModel` (`mesh_pass.rs:67-91`).
- Loader: `model::gltf_loader::load_model(path) -> Result<LoadedModel, ModelLoadError>` (`gltf_loader.rs:229`). `LoadedModel { mesh: SkinnedMesh, skeleton: Skeleton, clips: Vec<AnimationClip>, submeshes: Vec<Submesh>, tags: Vec<String> }` (`gltf_loader.rs:43-63`). `Submesh { material_key: String /*blake3 hex*/, indices: Range<u32> }` (`gltf_loader.rs:30-37`).
- Material: `build_material_bind_group(device, bgl, &LoadedTexture, aniso_sampler, Material, label) -> BindGroup` (`render/mod.rs:434`); keys resolved by blake3 content-hash of base-color PNG via `parse_blake3_key` → `load_textures` → `.prm` cache (`render/mod.rs:2780-2811`). `plan_submesh_materials` dedups distinct keys (`render/mod.rs:526-546`).
- `sample_clip(clip, skeleton, time, out: &mut Vec<BonePaletteEntry>)` loops via `time.rem_euclid(clip.duration)` (`model/anim.rs:41-82`).
- **Cache pattern to mirror:** `loaded_textures: Vec<LoadedTexture>`, `mip_count_aniso_samplers: HashMap<u32, Sampler>` (`render/mod.rs:731,737`); `SmokePass::sheets: HashMap<String, SpriteSheet>` (`render/smoke.rs`). New: handle→`UploadedModel` map + per-model skeleton/clip.
- Collector drops the handle: `let ComponentValue::Mesh(_mesh) = value else { continue }` (`mesh_render.rs:58`); reads `Transform` directly, packs `Mat4` (`mesh_render.rs:53-72`).

## SH lighting (settled — Milestone 9 shipped)

- Helper `shaders/sh_sample.wgsl`: `sample_sh_indirect_corners_depth_aware(gi, gfrac, sample_world, shading_normal, geo_normal, reject_backface, probe_occlusion_enabled) -> vec3<f32>` (`sh_sample.wgsl:186-207`). Binding-agnostic — consumer declares `sh_total_atlas`(b1), `sh_atlas_sampler`(b2), `sh_grid: ShGridInfo`(b10), `sh_depth_moments: texture_3d`(b14) before append.
- World path: `forward.wgsl` declares those at **group 3** b1/b2/b10/b14 (`forward.wgsl:170-184`); `sample_sh_indirect(world_pos, shading_normal, geo_normal)` applies a `0.1×cell_size` normal offset + grid-coord clamp, calls the depth-aware sampler with `reject_backface = true`, `probe_occlusion = sh_grid.probe_occlusion != 0` (`forward.wgsl:430-457`).
- **Dynamic-entity precedent (billboard):** per-fragment, `reject_backface = false`, Chebyshev **on** (`billboard.wgsl:264-285`). Fog: per-march-step, both off (`fog_volume.wgsl:281-305`). → meshes follow the billboard variant: per-fragment, backface off, Chebyshev on.
- Renderer bind group: `ShVolumeResources { pub bind_group, pub bind_group_layout }` (`render/sh_volume.rs:89-91`); BGL entries visibility `FRAGMENT | COMPUTE` (`sh_volume.rs:584`). Bound at group 3 in forward/billboard/fog (`render/mod.rs:4097,4232,4277`). **Reusable at a different slot:** wgpu bind groups are group-index-agnostic at `set_bind_group` time; put `bind_group_layout` at the new slot in the mesh pipeline layout and bind `bind_group` there. The mesh fragment is a FRAGMENT stage already covered by the BGL visibility; declaring only b1/b2/b10/b14 in the shader is fine (BGL may carry extra entries the shader ignores).

## Classname + entity wiring

- `ClassnameHandler = fn(&MapEntity, &mut EntityRegistry) -> Option<EntityId>` (`builtins/mod.rs:31-34`). `ClassnameDispatch { handlers: HashMap<&'static str, ClassnameHandler> }` (`:39-72`). `register_builtins` registers `billboard_emitter::handle` (`:77-82`). `apply_classname_dispatch(entities, dispatch, registry) -> HashSet<String>` — claims classname before invoking, writes `set_map_kvps` after a successful spawn, debug-skips unregistered (`:101-137`). Built-ins win the two-sweep (`build_pipeline.md` §Built-in Classname Routing).
- Template `builtins/billboard_emitter.rs`: `CLASSNAME` const (`:20`), `default_component()` (`:25-42`), `kvp_f32`/`kvp_string` log-and-fallback (`:48-82`), `handle()` builds `Transform { position: origin, rotation: rotation_quat(), scale: ONE }`, `try_spawn(transform, &tags)`, `set_component` (`:134-158`). Handler does **not** set tags (try_spawn does) or KVPs (dispatch does).
- `MapEntity { classname: String, origin: Vec3, angles: Vec3, key_values: HashMap, tags: Vec<String> }` (`map_entity.rs:16-25`); `rotation_quat()` = `Quat::from_euler(YXZ, angles.y, angles.x, angles.z)`, radians (`:43-51`).
- Registry: `ComponentKind::Mesh = 9` (in VARIANTS) (`registry.rs:99,108-119`); `ComponentValue::Mesh(MeshComponent)` serde tag `"mesh"` (`:163`, `components/mesh.rs:31-38`); `Transform { position, rotation, scale }` (`registry.rs:130-134`); `try_spawn`, `set_component`, `iter_with_kind` (`:564,666,537`). `MeshComponent { model: String }` (`components/mesh.rs:11-13`).
- **Interpolation gap:** `frame_timing.rs` `InterpolableState { position }` lerps position only, used for the **player camera** (`main.rs:1021-1029`). No per-entity prev/current transform storage (grep of `scripting/` finds none). `entity_model.md:94` states renderer interpolates between tick states — true only for the camera today. Hence Task A.

## External / best practice

- Per-instance data via storage buffer indexed by `@builtin(instance_index)` is the current wgpu idiom (Learn Wgpu — Instancing; WebGPU Fundamentals — Storage Buffers).
- `first_instance`/`base_instance` is unreliable: requires `INDIRECT_FIRST_INSTANCE`, and reads as 0 in shaders on DX12 even when enabled (gfx-rs/wgpu#2471). → palette base lives in the per-instance SSBO entry, addressed by `instance_index`.
- Instanced palette skinning: per-instance contiguous palette run, `base + joint` lookup (NVIDIA GPU Gems 3 ch.2). Matches the slice's scheme.

Sources: https://sotrh.github.io/learn-wgpu/beginner/tutorial7-instancing/ · https://webgpufundamentals.org/webgpu/lessons/webgpu-storage-buffers.html · https://docs.rs/wgpu/latest/wgpu/util/struct.DrawIndexedIndirectArgs.html · https://github.com/gfx-rs/wgpu/issues/2471 · https://developer.nvidia.com/gpugems/gpugems3/part-i-geometry/chapter-2-animated-crowd-rendering
