// Renderer model + mesh-pass resource methods: texture install, skinned model
// loading, mesh draw submission, and UV normalization.
// See: context/lib/resource_management.md

use super::mesh_pass::ModelAnimationData;
use super::*;

impl Renderer {
    /// Rebuilds all material bind groups from baked `.prm` mip sidecars.
    /// `texture_materials` must be parallel to `texture_names`; entries beyond
    /// its length fall back to `Material::Default`. Caller drives the order:
    /// `install_textures` runs before `install_level_geometry` because the
    /// uploaded diffuse dimensions feed `normalize_world_uvs`.
    /// See: context/lib/boot_sequence.md §3 (Level Install Order) · context/lib/build_pipeline.md
    pub fn install_textures(
        &mut self,
        texture_names: &[String],
        texture_cache_keys: &TextureCacheKeysSection,
        prm_cache_root: &Path,
        texture_materials: &[Material],
    ) {
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");

        // Cache materials so `install_level_geometry` can also recompute the
        // per-leaf material lookup without re-deriving them. (Mirrors the
        // pre-refactor flow where geometry install populated this field.)
        full.stored_texture_materials = texture_materials.to_vec();

        let loaded = load_textures(
            device,
            queue,
            texture_names,
            texture_cache_keys,
            prm_cache_root,
        );

        // Sampler pool grows monotonically: every distinct `mip_count` seen in
        // this batch needs a sampler with matching `lod_max_clamp`. The `1`
        // entry seeded in `Renderer::new` covers placeholders; new mip counts
        // beyond `1` arrive here when real textures load.
        for tex in &loaded {
            full.mip_count_aniso_samplers
                .entry(tex.mip_count)
                .or_insert_with(|| create_mip_aniso_sampler(device, tex.mip_count));
        }

        let mut gpu_textures: Vec<GpuTexture> = Vec::with_capacity(loaded.len());
        for (idx, tex) in loaded.iter().enumerate() {
            let aniso_sampler = full
                .mip_count_aniso_samplers
                .get(&tex.mip_count)
                .expect("aniso mip sampler must have been eagerly populated");
            let material = texture_materials
                .get(idx)
                .copied()
                .unwrap_or(postretro_render_data::material::Material::Default);
            let bind_group = build_material_bind_group(
                device,
                &full.texture_bind_group_layout,
                tex,
                aniso_sampler,
                material,
                &format!("Material {idx}"),
            );
            gpu_textures.push(GpuTexture { bind_group });
        }

        if gpu_textures.is_empty() {
            // No textures referenced by the level — keep the placeholder slot
            // so the world pipeline still has a bind group bound.
            let placeholder = placeholder_loaded_texture(device, queue);
            let aniso_sampler = full
                .mip_count_aniso_samplers
                .get(&1)
                .expect("mip_count 1 aniso sampler is seeded at Renderer::new");
            let bind_group = build_material_bind_group(
                device,
                &full.texture_bind_group_layout,
                &placeholder,
                aniso_sampler,
                postretro_render_data::material::Material::Default,
                "Placeholder Material",
            );
            full.loaded_textures = vec![placeholder];
            full.gpu_textures = vec![GpuTexture { bind_group }];
            log::info!("[Renderer] Textures installed: 1 (placeholder fallback)");
            return;
        }

        full.loaded_textures = loaded;
        full.gpu_textures = gpu_textures;
        log::info!("[Renderer] Textures installed: {}", full.gpu_textures.len());
    }

    /// Load one skinned model into the renderer's model cache: parse the glTF,
    /// resolve each submesh's material key (blake3 content-hash of the base-color
    /// PNG, the same recipe the level compiler uses to name `.prm` sidecars) to a
    /// `LoadedTexture`, build one bind group per distinct key, and upload to the
    /// mesh pass.
    ///
    /// Called once per distinct mesh model by the level-load model sweep (after
    /// classname dispatch). Returns `Some(tags)` on success (the model's glTF
    /// `extras` tags — currently unused by callers, a residual of the old spawn
    /// seam) or `None` on a load error, which logs a `warn!` naming the path and
    /// leaves the entry uncached (that model renders nothing).
    ///
    /// The renderer owns the GPU upload, cached skeleton, and all animation clips
    /// (inside the mesh pass's model cache); the per-frame draw list
    /// (`mesh_draws`) is supplied each frame by the render-frame mesh collector
    /// via [`set_mesh_draws`], not seeded here.
    ///
    /// Open path vs. cache key are deliberately decoupled. The glTF file is
    /// opened from `content_root.join(model_rel)` (every other asset joins the
    /// content root), but the model is cached under the VERBATIM `model_rel`
    /// string — that is the `MeshComponent.model` handle the spawn attaches and
    /// the per-frame planner groups by, so the key must match it exactly (a
    /// joined key would miss the planner's `models.get(&group.model)` lookup and
    /// silently drop every draw). Re-loading the same handle replaces the cache
    /// entry (idempotent upload).
    ///
    /// [`set_mesh_draws`]: Self::set_mesh_draws
    pub fn load_skinned_model(
        &mut self,
        model_rel: &str,
        content_root: &Path,
        prm_cache_root: &Path,
    ) -> Option<Vec<String>> {
        let (model_path, handle) = resolve_model_open_path_and_handle(model_rel, content_root);
        let model = match postretro_model::gltf_loader::load_model(&model_path) {
            Ok(m) => m,
            Err(err) => {
                log::warn!(
                    "[Model] model load failed for {} : {err} — mesh pass idle",
                    model_path.display(),
                );
                return None;
            }
        };

        let submesh_materials = self.resolve_skinned_model_material(&model, prm_cache_root);

        let postretro_model::gltf_loader::LoadedModel {
            mesh,
            skeleton,
            clips,
            tags,
            pose_stack,
            ..
        } = model;
        let clip_count = clips.len();
        // Name every parsed clip so a multi-clip asset surfaces its full set in
        // the load log. Per-instance sample parameters select cached clips.
        // Joined as "name (1.23s)" in glTF order.
        if !clips.is_empty() {
            let clip_summary = clips
                .iter()
                .map(|clip| format!("'{}' ({:.2}s)", clip.name, clip.duration))
                .collect::<Vec<_>>()
                .join(", ");
            log::info!(
                "[Model] skinned model animation: {} clip(s) [{}], {} joints",
                clip_count,
                clip_summary,
                skeleton.joints.len(),
            );
        }

        // `handle` (the verbatim cache key) was derived alongside the open path
        // by `resolve_model_open_path_and_handle` — see this method's doc. The
        // Full clip set is handed to the cache; per-instance sample parameters
        // select clips during palette sampling.
        // `resolve_skinned_model_material` (a `&mut self` helper) already ran
        // above into `submesh_materials`, so destructuring `self` here is safe.
        let Self { device, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        full.mesh_pass.insert_model(
            device,
            handle,
            &mesh,
            submesh_materials,
            ModelAnimationData {
                skeleton,
                clips,
                pose_stack,
            },
        );

        log::info!(
            "[Model] skinned model uploaded: {} clip(s) parsed, {} tag(s)",
            clip_count,
            tags.len(),
        );
        Some(tags)
    }

    /// The clip metadata (name + duration) for a cached skinned model, in glTF
    /// (authored) index order, keyed by the same `model_handle` string
    /// `load_skinned_model` cached it under. Returns an empty `Vec` when the model
    /// is not cached or has no animation — no error, no panic.
    ///
    /// `pub` forwarder over the private `mesh_pass` clip-metadata seam. Consumed
    /// by the level-load model sweep (`main.rs`) to build the game-side clip
    /// tables.
    pub fn skinned_model_clip_metadata(
        &self,
        model_handle: &str,
    ) -> Vec<postretro_render_cpu::mesh_pass::ClipMetadata> {
        self.full()
            .mesh_pass
            .model_clip_metadata(&postretro_model::ModelHandle::from(model_handle))
    }

    /// The local-space bound for a cached skinned model, keyed by the same
    /// `model_handle` string as [`Self::skinned_model_clip_metadata`].
    pub fn skinned_model_local_bounds(
        &self,
        model_handle: &str,
    ) -> postretro_render_data::cone_frustum::Aabb {
        self.full()
            .mesh_pass
            .model_local_bounds(&postretro_model::ModelHandle::from(model_handle))
    }

    /// Replace this frame's skinned-mesh instance list with the inputs emitted by
    /// the render-frame mesh collector (forward visibility and selected-static
    /// shadow relevance already classified, at interpolated transforms). Called
    /// once per frame in the collection sub-stage, before `render_frame_indirect`.
    /// The renderer plans these into per-model draw groups + palette runs and
    /// records the draws; it needs no world reference because classification
    /// already happened game-side.
    pub fn set_mesh_draws(
        &mut self,
        instances: &[postretro_render_cpu::mesh_instances::MeshInstanceInput],
    ) {
        self.full_mut().mesh_draws.clear();
        self.full_mut().mesh_draws.extend_from_slice(instances);
    }

    /// Upload this frame's cull-split kinematic mover instances. The visible
    /// subset feeds beauty draws; every present mover stays in the transform
    /// buffer so shadow depth can cone-cull independently of camera PVS.
    pub fn set_kinematic_mover_draws(
        &mut self,
        visible_instances: &[kinematic_brush::KinematicMoverInstance],
        shadow_instances: &[kinematic_brush::KinematicMoverInstance],
    ) {
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        full.kinematic_mover_draws.clear();
        full.kinematic_mover_draws
            .extend_from_slice(visible_instances);
        full.kinematic_mover_shadow_draws.clear();
        full.kinematic_mover_shadow_draws
            .extend_from_slice(shadow_instances);
        full.kinematic_brush.upload_instances(
            device,
            queue,
            &full.kinematic_mover_draws,
            &full.kinematic_mover_shadow_draws,
        );
    }

    /// Replace this frame's all-present kinematic mover bounds. This
    /// renderer-owned CPU state gates static-light promotion and feeds the
    /// rigid occluder recorder directly in the shadow pass.
    ///
    /// The render collector must call this before `render_frame_indirect`; an
    /// empty slice clears stale movers so static-only maps retain their existing
    /// promotion behavior.
    pub fn set_mover_occluder_aabbs(&mut self, aabbs: &[rigid_occluder_depth::MoverOccluderAabb]) {
        let full = self.full_mut();
        full.mover_occluder_aabbs.clear();
        full.mover_occluder_aabbs.extend_from_slice(aabbs);
    }

    /// Reset per-level transient mesh-pass state at level load. `pub` forwarder
    /// over the private `mesh_pass`; called from the level-load model sweep at the
    /// model-cache install site (where each distinct model uploads). Empties the
    /// `"smooth"`-interrupt snapshot store and the per-entity palette cache —
    /// entity seeds are not stable across levels, so stale state must not survive.
    pub fn clear_mesh_pass_for_level_load(&mut self) {
        self.full_mut().mesh_pass.clear_for_level_load();
    }

    /// Resolve each submesh's material key (content-hash hex → `.prm`) to a
    /// material bind group, returning one `(bind group, index range)` per
    /// submesh in submesh order for the mesh pass to draw.
    ///
    /// Dedup: one GPU material bind group is built per *distinct* key — a model
    /// reusing a material across primitives builds it once and shares it. Each
    /// submesh range is then paired with its (possibly shared) bind group. The
    /// dedup + range bookkeeping is the GPU-free [`plan_submesh_materials`];
    /// this method is the thin GPU layer that builds the bind groups.
    ///
    /// Degrades to a placeholder per distinct key when its key is absent/garbled
    /// or its `.prm` is missing. Model materials consume only diffuse; specular
    /// and normal always use neutral placeholders in this slice.
    fn resolve_skinned_model_material(
        &mut self,
        model: &postretro_model::gltf_loader::LoadedModel,
        prm_cache_root: &Path,
    ) -> Vec<(wgpu::BindGroup, std::ops::Range<u32>)> {
        let plan = plan_submesh_materials(&model.submeshes);

        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");

        // Build one material bind group per distinct key (deduped). Indexed
        // parallel to `plan.distinct_keys` so each submesh draw indexes into it.
        let distinct_bind_groups: Vec<wgpu::BindGroup> = plan
            .distinct_keys
            .iter()
            .map(|key_hex| {
                let key = parse_blake3_key(key_hex);
                let tex = load_model_diffuse_texture(device, queue, key_hex, key, prm_cache_root);

                let character_model_sampler = full
                    .mip_count_character_model_samplers
                    .entry(tex.mip_count)
                    .or_insert_with(|| create_mip_character_model_sampler(device, tex.mip_count));
                build_material_bind_group(
                    device,
                    &full.texture_bind_group_layout,
                    &tex,
                    character_model_sampler,
                    Material::Default,
                    &format!("Skinned Model Material {key_hex}"),
                )
            })
            .collect();

        // The resulting Vec is moved into the mesh pass (ownership transfer), so
        // each slot must hold its own handle. Clone the shared handle (cheap Arc
        // clone inside wgpu) for submeshes that reuse a distinct material.
        plan.draws
            .into_iter()
            .map(|draw| (distinct_bind_groups[draw.distinct].clone(), draw.indices))
            .collect()
    }

    /// Normalize texel-space UVs on static world and kinematic mover vertices
    /// to `[0,1]` using the diffuse-texture dimensions just installed by
    /// `install_textures`. Runs on the main thread between `install_textures`
    /// and `install_level_geometry`. Reads `texture.width()`/`height()` off
    /// the wgpu textures owned by `self.loaded_textures` so the dimensions
    /// always match the actual upload.
    pub fn normalize_world_uvs(&self, world: &mut postretro_level_loader::LevelWorld) {
        let texture_dimensions = loaded_texture_dimensions(&self.full().loaded_textures);
        normalize_static_world_uvs(
            &mut world.vertices,
            &world.indices,
            &world.bvh.leaves,
            &texture_dimensions,
        );
        normalize_kinematic_mover_uvs(&mut world.kinematic_geometry.movers, &texture_dimensions);
    }
}

fn loaded_texture_dimensions(textures: &[LoadedTexture]) -> Vec<[u32; 2]> {
    textures
        .iter()
        .map(|tex| [tex.diffuse_texture.width(), tex.diffuse_texture.height()])
        .collect()
}

fn texture_dimensions_for_index(
    texture_dimensions: &[[u32; 2]],
    texture_index: u32,
) -> Option<(f32, f32)> {
    let dimensions = texture_dimensions.get(texture_index as usize)?;
    let [width, height] = *dimensions;
    if width == 0 || height == 0 {
        return None;
    }
    Some((width as f32, height as f32))
}

fn normalize_static_world_uvs(
    vertices: &mut [postretro_render_data::geometry::WorldVertex],
    indices: &[u32],
    leaves: &[postretro_render_data::geometry::BvhLeaf],
    texture_dimensions: &[[u32; 2]],
) {
    let mut normalized = vec![false; vertices.len()];
    for leaf in leaves {
        let Some((width, height)) =
            texture_dimensions_for_index(texture_dimensions, leaf.material_bucket_id)
        else {
            continue;
        };
        let start = leaf.index_offset as usize;
        let end = start.saturating_add(leaf.index_count as usize);
        for i in start..end {
            if let Some(&idx) = indices.get(i) {
                let vertex_index = idx as usize;
                if vertex_index < normalized.len() && !normalized[vertex_index] {
                    if let Some(vertex) = vertices.get_mut(vertex_index) {
                        vertex.base_uv[0] /= width;
                        vertex.base_uv[1] /= height;
                        normalized[vertex_index] = true;
                    }
                }
            }
        }
    }
}

fn normalize_kinematic_mover_uvs(
    movers: &mut [postretro_level_loader::LoadedKinematicMover],
    texture_dimensions: &[[u32; 2]],
) {
    for mover in movers {
        let mut normalized = vec![false; mover.vertices.len()];
        let mut offset = 0usize;
        for face in &mover.face_meta {
            if offset + 2 >= mover.indices.len() {
                break;
            }
            let face_base = mover.indices[offset];
            let start = offset;
            while offset + 2 < mover.indices.len() && mover.indices[offset] == face_base {
                offset += 3;
            }
            normalize_kinematic_face_uvs(
                &mut mover.vertices,
                &mover.indices[start..offset],
                face.texture_index,
                texture_dimensions,
                &mut normalized,
            );
        }
    }
}

fn normalize_kinematic_face_uvs(
    vertices: &mut [postretro_level_format::geometry::Vertex],
    indices: &[u32],
    texture_index: u32,
    texture_dimensions: &[[u32; 2]],
    normalized: &mut [bool],
) {
    let Some((width, height)) = texture_dimensions_for_index(texture_dimensions, texture_index)
    else {
        return;
    };
    for &index in indices {
        let vertex_index = index as usize;
        if vertex_index < normalized.len() && !normalized[vertex_index] {
            if let Some(vertex) = vertices.get_mut(vertex_index) {
                vertex.uv[0] /= width;
                vertex.uv[1] /= height;
                normalized[vertex_index] = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_level_format::geometry::{FaceMeta, Vertex};
    use postretro_render_data::geometry::{BvhLeaf, WorldVertex};

    fn world_vertex(base_uv: [f32; 2]) -> WorldVertex {
        WorldVertex {
            position: [0.0, 0.0, 0.0],
            base_uv,
            normal_oct: [0, 0],
            tangent_packed: [0, 0],
            lightmap_uv: [0, 0],
            lightmap_layer: 0,
        }
    }

    fn kinematic_vertex(uv: [f32; 2]) -> Vertex {
        Vertex::new(
            [0.0, 0.0, 0.0],
            uv,
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        )
    }

    fn loaded_mover(
        vertices: Vec<Vertex>,
        indices: Vec<u32>,
        face_meta: Vec<FaceMeta>,
    ) -> postretro_level_loader::LoadedKinematicMover {
        postretro_level_loader::LoadedKinematicMover {
            mover_id: 7,
            name: "lift".to_string(),
            tags: Vec::new(),
            origin: Vec3::ZERO,
            path: "a".to_string(),
            speed_mps: 1.0,
            wait_ms: 0.0,
            move_mode: 0,
            start_on_spawn: true,
            vertices,
            indices,
            face_meta,
            spin_axis: Vec3::ZERO,
            spin_speed_deg_s: 0.0,
            spin_accel_deg_s2: 0.0,
            carry_yaw: false,
            block_policy: "displace".to_string(),
            crush_damage: 0.0,
            crush_interval_ms: 0.0,
            auto_close_ms: None,
            open_event: None,
            close_event: None,
            blocked_event: None,
            crush_event: None,
            sealed_portal_ids: Vec::new(),
            carried_lights: Vec::new(),
        }
    }

    fn assert_uv_approx(actual: [f32; 2], expected: [f32; 2]) {
        const EPSILON: f32 = 0.000_001;
        assert!(
            (actual[0] - expected[0]).abs() <= EPSILON
                && (actual[1] - expected[1]).abs() <= EPSILON,
            "actual={actual:?} expected={expected:?}",
        );
    }

    #[test]
    fn static_world_uvs_normalize_once_per_bvh_vertex() {
        let mut vertices = vec![
            world_vertex([64.0, 16.0]),
            world_vertex([32.0, 32.0]),
            world_vertex([16.0, 8.0]),
        ];
        let indices = vec![0, 1, 2, 0, 2, 1];
        let leaves = vec![BvhLeaf {
            aabb_min: [0.0, 0.0, 0.0],
            material_bucket_id: 0,
            aabb_max: [1.0, 1.0, 1.0],
            index_offset: 0,
            index_count: 6,
            cell_id: 0,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }];

        normalize_static_world_uvs(&mut vertices, &indices, &leaves, &[[64, 32]]);

        assert_uv_approx(vertices[0].base_uv, [1.0, 0.5]);
        assert_uv_approx(vertices[1].base_uv, [0.5, 1.0]);
        assert_uv_approx(vertices[2].base_uv, [0.25, 0.25]);
    }

    #[test]
    fn kinematic_mover_uvs_normalize_by_face_texture_dimensions() {
        let mut movers = vec![loaded_mover(
            vec![
                kinematic_vertex([64.0, 32.0]),
                kinematic_vertex([32.0, 16.0]),
                kinematic_vertex([128.0, 64.0]),
                kinematic_vertex([16.0, 8.0]),
                kinematic_vertex([128.0, 64.0]),
                kinematic_vertex([64.0, 32.0]),
                kinematic_vertex([32.0, 16.0]),
            ],
            vec![0, 1, 2, 0, 2, 3, 4, 5, 6],
            vec![
                FaceMeta {
                    leaf_index: 0,
                    texture_index: 1,
                },
                FaceMeta {
                    leaf_index: 0,
                    texture_index: 2,
                },
            ],
        )];

        normalize_kinematic_mover_uvs(&mut movers, &[[32, 16], [128, 64], [256, 128]]);

        let vertices = &movers[0].vertices;
        assert_uv_approx(vertices[0].uv, [0.5, 0.5]);
        assert_uv_approx(vertices[1].uv, [0.25, 0.25]);
        assert_uv_approx(vertices[2].uv, [1.0, 1.0]);
        assert_uv_approx(vertices[3].uv, [0.125, 0.125]);
        assert_uv_approx(vertices[4].uv, [0.5, 0.5]);
        assert_uv_approx(vertices[5].uv, [0.25, 0.25]);
        assert_uv_approx(vertices[6].uv, [0.125, 0.125]);
    }
}
