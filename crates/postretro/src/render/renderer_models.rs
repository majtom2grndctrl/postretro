// Renderer model + mesh-pass resource methods: texture install, skinned model
// loading, mesh draw submission, and UV normalization.
// See: context/lib/resource_management.md

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
                .unwrap_or(crate::material::Material::Default);
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
                crate::material::Material::Default,
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
    /// Called once per distinct `prop_mesh` model by the level-load model sweep
    /// (after classname dispatch); spawning itself happens earlier in
    /// `prop_mesh::handle`. Returns `Some(tags)` on success (the model's glTF
    /// `extras` tags — currently unused by callers, a residual of the old spawn
    /// seam) or `None` on a load error, which also logs a `warn!` naming the path
    /// and leaves the entry uncached (that model renders nothing).
    ///
    /// The renderer owns the GPU upload + the cached skeleton + first clip
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
        let model = match crate::model::gltf_loader::load_model(&model_path) {
            Ok(m) => m,
            Err(err) => {
                log::warn!(
                    "[Model] skinned model load failed for {} : {err} — mesh pass idle",
                    model_path.display(),
                );
                return None;
            }
        };

        let submesh_materials = self.resolve_skinned_model_material(&model, prm_cache_root);

        let crate::model::gltf_loader::LoadedModel {
            mesh,
            skeleton,
            clips,
            tags,
            ..
        } = model;
        let clip_count = clips.len();
        // Name every parsed clip so a multi-clip asset surfaces its full set in
        // the load log (the cache retains them all; the per-frame palette samples
        // the first). Joined as "name (1.23s)" in glTF order.
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
        // FULL clip set is handed to the cache; clip selection is a sibling plan.
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
            skeleton,
            clips,
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
    /// `pub` forwarder over the private `mesh_pass` (same seam as
    /// [`Renderer::skinned_model_clip_by_name`]). Consumed by the level-load model
    /// sweep (`main.rs`) to build the game-side clip tables.
    pub fn skinned_model_clip_metadata(&self, model_handle: &str) -> Vec<mesh_pass::ClipMetadata> {
        self.full()
            .mesh_pass
            .model_clip_metadata(&crate::model::ModelHandle::from(model_handle))
    }

    /// Replace this frame's skinned-mesh instance list with the inputs emitted by
    /// the render-frame mesh collector (already culled, at interpolated
    /// transforms). Called once per frame in the collection sub-stage, before
    /// `render_frame_indirect`. The renderer plans these into per-model draw
    /// groups + palette runs and records the draws; it needs no world reference
    /// because the cull already happened game-side.
    pub fn set_mesh_draws(&mut self, instances: &[mesh_instances::MeshInstanceInput]) {
        self.full_mut().mesh_draws.clear();
        self.full_mut().mesh_draws.extend_from_slice(instances);
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
        model: &crate::model::gltf_loader::LoadedModel,
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
                let tex = load_model_diffuse_texture(
                    device,
                    queue,
                    key_hex,
                    key,
                    prm_cache_root,
                );

                let aniso_sampler = full
                    .mip_count_aniso_samplers
                    .entry(tex.mip_count)
                    .or_insert_with(|| create_mip_aniso_sampler(device, tex.mip_count));
                build_material_bind_group(
                    device,
                    &full.texture_bind_group_layout,
                    &tex,
                    aniso_sampler,
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

    /// Normalize texel-space UVs on every BVH-leaf-bound vertex to `[0,1]`
    /// using the diffuse-texture dimensions just installed by
    /// `install_textures`. Runs on the main thread between `install_textures`
    /// and `install_level_geometry`. Reads `texture.width()`/`height()` off
    /// the wgpu textures owned by `self.loaded_textures` so the dimensions
    /// always match the actual upload.
    pub fn normalize_world_uvs(&self, world: &mut crate::prl::LevelWorld) {
        let mut normalized = vec![false; world.vertices.len()];
        for leaf in &world.bvh.leaves {
            let tex_idx = leaf.material_bucket_id as usize;
            let tex = match self.full().loaded_textures.get(tex_idx) {
                Some(t) => t,
                None => continue,
            };
            let w = tex.diffuse_texture.width();
            let h = tex.diffuse_texture.height();
            if w == 0 || h == 0 {
                continue;
            }
            let start = leaf.index_offset as usize;
            let count = leaf.index_count as usize;
            for i in start..start + count {
                if let Some(&idx) = world.indices.get(i) {
                    let vi = idx as usize;
                    if vi < normalized.len() && !normalized[vi] {
                        if let Some(vert) = world.vertices.get_mut(vi) {
                            vert.base_uv[0] /= w as f32;
                            vert.base_uv[1] /= h as f32;
                            normalized[vi] = true;
                        }
                    }
                }
            }
        }
    }
}
