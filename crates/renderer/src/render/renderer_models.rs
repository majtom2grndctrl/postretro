// Renderer model + mesh-pass resource methods: skinned model loading, mesh
// draw submission, and UV normalization.
// See: context/lib/resource_management.md

use super::mesh_pass::{MeshPass, ModelAnimationData};
use super::*;
use postretro_render_cpu::texture_prep::{PreparedModelMaterials, prepare_model_materials};

impl Renderer {
    /// Load one skinned model into the active level set's models (or, with
    /// none, stage it for the next build): parse the glTF,
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
        let (model_path, _) = resolve_model_open_path_and_handle(model_rel, content_root);
        let parsed = postretro_model::gltf_loader::load_model(&model_path);
        self.upload_parsed_skinned_model(model_rel, &model_path, &parsed, prm_cache_root)
    }

    /// [`load_skinned_model`](Self::load_skinned_model) for a glTF the caller
    /// already parsed. The level-load model sweep parses each file once and
    /// hands the same result to this upload and to the game-side hit-zone store,
    /// so neither re-reads the file nor re-hashes its source PNGs. `parsed` is
    /// borrowed: the sweep's other consumer still needs it, so the skeleton,
    /// clips and pose stack the mesh pass keeps are cloned here.
    ///
    /// `model_path` is the path the caller opened (`content_root.join(model_rel)`);
    /// it names the file in the failure warning. A parse error warns here, and
    /// the model stays uncached, exactly as when this method parsed it itself.
    pub fn upload_parsed_skinned_model(
        &mut self,
        model_rel: &str,
        model_path: &Path,
        parsed: &Result<
            postretro_model::gltf_loader::LoadedModel,
            postretro_model::gltf_loader::ModelLoadError,
        >,
        prm_cache_root: &Path,
    ) -> Option<Vec<String>> {
        // The verbatim cache key, the same value `resolve_model_open_path_and_handle`
        // pairs with the open path (see `load_skinned_model`'s doc).
        let handle = postretro_model::ModelHandle::from(model_rel.to_string());
        let model = match parsed {
            Ok(m) => m,
            Err(err) => {
                log::warn!(
                    "[Model] model load failed for {} : {err} — mesh pass idle",
                    model_path.display(),
                );
                return None;
            }
        };

        let (submesh_materials, samplers) =
            self.resolve_skinned_model_material(model, prm_cache_root);

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

        // `handle` is the verbatim cache key — see `load_skinned_model`'s doc. The
        // full clip set is handed to the entry; per-instance sample parameters
        // select clips during palette sampling.
        // `resolve_skinned_model_material` (a `&mut self` helper) already ran
        // above into `submesh_materials`, so destructuring `self` here is safe.
        let Self { device, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let entry = MeshPass::upload_model(
            device,
            mesh,
            submesh_materials,
            ModelAnimationData {
                skeleton: skeleton.clone(),
                clips: clips.clone(),
                pose_stack: pose_stack.clone(),
            },
        );
        match full.level.as_mut() {
            // The active set is already published: its new samplers join the
            // renderer's maps now, as its publish would have merged them.
            Some(level) => {
                level.models.insert(handle, std::sync::Arc::new(entry));
                full.samplers.merge(samplers);
            }
            None => {
                full.pending_models
                    .insert(handle, std::sync::Arc::new(entry));
                full.pending_samplers.merge(samplers);
            }
        }

        log::info!(
            "[Model] skinned model uploaded: {} clip(s) parsed, {} tag(s)",
            clip_count,
            tags.len(),
        );
        Some(tags.clone())
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
        self.active_models()
            .clip_metadata(&postretro_model::ModelHandle::from(model_handle))
    }

    /// The local-space bound for a cached skinned model, keyed by the same
    /// `model_handle` string as [`Self::skinned_model_clip_metadata`].
    pub fn skinned_model_local_bounds(
        &self,
        model_handle: &str,
    ) -> postretro_render_data::cone_frustum::Aabb {
        self.active_models()
            .local_bounds(&postretro_model::ModelHandle::from(model_handle))
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

    /// Replace this frame's ghost instances (`rendering_pipeline.md` §7.9):
    /// each a [`MeshPresentation::Ghost`] input at its mapped transform, culled
    /// at its mapped position and carrying its presented look. They share the
    /// skinned palette/instance budget, plan after every real drawable mesh,
    /// and draw unlit through the mod's `render.ghosts` mode, never into
    /// shadow depth.
    ///
    /// [`MeshPresentation::Ghost`]: postretro_render_cpu::mesh_instances::MeshPresentation::Ghost
    pub fn set_ghost_mesh_draws(
        &mut self,
        instances: &[postretro_render_cpu::mesh_instances::MeshInstanceInput],
    ) {
        self.full_mut().ghost_mesh_draws.clear();
        self.full_mut()
            .ghost_mesh_draws
            .extend_from_slice(instances);
    }

    /// Replace this frame's mover ghosts (`rendering_pipeline.md` §7.9): each a
    /// mover's geometry at a link's mapped model with its presented look,
    /// already culled at its mapped bounds. A mover in two links arrives twice
    /// and draws twice. Drawn unlit through the mod's `render.ghosts` mode, after
    /// the skinned ghosts; never into shadow depth or occluder bounds.
    pub fn set_kinematic_ghost_draws(
        &mut self,
        instances: &[postretro_render_cpu::ghost_instance::KinematicGhostInstance],
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
        // Ghost bounds resolve through the active level's mover geometry.
        let Some(level) = full.level.as_ref() else {
            full.kinematic_ghost.clear();
            return;
        };
        full.kinematic_ghost
            .upload(device, queue, instances, &level.kinematic);
    }

    /// Replace this frame's projectile billboard ghosts, as `(collection,
    /// packed ghost sprites)` per sprite collection
    /// (`ghost_instance::pack_ghost_sprite_instance`), already culled at their
    /// mapped positions. Drawn unlit and additively after the billboards, in
    /// both `render.ghosts` modes.
    pub fn set_billboard_ghost_draws<'a>(
        &mut self,
        collections: impl IntoIterator<Item = (&'a str, &'a [u8])>,
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
        full.billboard_ghost.upload(device, queue, collections);
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
        // Mover ids resolve through the active level's geometry; with none
        // installed no mover draws.
        let Some(level) = full.level.as_ref() else {
            full.kinematic_brush.clear_frame_draws();
            return;
        };
        full.kinematic_brush.upload_instances(
            device,
            queue,
            &level.kinematic,
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
        self.queue.assert_empty("level mesh install");
        self.full_mut().mesh_pass.clear_for_level_load();
    }

    /// The active set's models, else those staged for the next build.
    fn active_models(&self) -> &super::mesh_pass::LevelModels {
        let full = self.full();
        full.level
            .as_ref()
            .map_or(&full.pending_models, |level| &level.models)
    }

    /// Resolve each submesh's material key (content-hash hex → `.prm`) to a
    /// material bind group, returning one `(bind group, index range)` per
    /// submesh in submesh order for the mesh pass to draw, and the
    /// character-model samplers it created that the renderer's add-only map
    /// lacked (never written into it here).
    ///
    /// Dedup: one GPU material bind group is built per *distinct* key — a model
    /// reusing a material across primitives builds it once and shares it. Each
    /// submesh range is then paired with its (possibly shared) bind group. The
    /// dedup + range bookkeeping and each key's sidecar read, parse and plan
    /// are the GPU-free [`prepare_model_materials`], which a level load also
    /// runs on its pool; this method is the thin GPU layer that uploads the
    /// textures and builds the bind groups.
    ///
    /// Degrades to a placeholder per distinct key when its key is absent/garbled
    /// or its `.prm` is missing. Model materials consume only diffuse; specular
    /// and normal always use neutral placeholders in this slice.
    fn resolve_skinned_model_material(
        &mut self,
        model: &postretro_model::gltf_loader::LoadedModel,
        prm_cache_root: &Path,
    ) -> (Vec<(wgpu::BindGroup, std::ops::Range<u32>)>, SamplerMaps) {
        let PreparedModelMaterials { plan, diffuse } =
            prepare_model_materials(&model.submeshes, prm_cache_root);
        let mut samplers = SamplerMaps::default();

        let surface_depth_quality = self.surface_depth_quality;
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
            .zip(&diffuse)
            .map(|(key_hex, prepared)| {
                let tex = upload_prepared_texture(device, queue.raw(), prepared);

                let character_model_sampler =
                    samplers.character(&full.samplers, device, tex.mip_count);
                // Skinned models are outside Surface Depth's scope: they never
                // bind the world material bundle's surface map —
                // the model diffuse plan binds the neutral single-channel
                // specular placeholder — so their uniform is flat whatever the
                // switch says. The uniform buffer handle is therefore
                // deliberately not retained; the bind group keeps the buffer
                // alive and nothing will ever rewrite it.
                build_material_bind_group(
                    device,
                    &full.texture_bind_group_layout,
                    &tex,
                    &character_model_sampler,
                    Material::Default,
                    surface_depth_quality,
                    &format!("Skinned Model Material {key_hex}"),
                )
                .bind_group
            })
            .collect();

        // The resulting Vec is moved into the mesh pass (ownership transfer), so
        // each slot must hold its own handle. Clone the shared handle (cheap Arc
        // clone inside wgpu) for submeshes that reuse a distinct material.
        let submeshes = plan
            .draws
            .into_iter()
            .map(|draw| (distinct_bind_groups[draw.distinct].clone(), draw.indices))
            .collect();
        (submeshes, samplers)
    }

    /// Normalize texel-space UVs on static world and kinematic mover vertices
    /// to `[0,1]` using the diffuse-texture dimensions just installed by
    /// `install_textures`. Runs on the main thread between `install_textures`
    /// and `install_level_geometry`. Reads `texture.width()`/`height()` off
    /// the wgpu textures `install_textures` staged (else the active level's)
    /// so the dimensions always match the actual upload.
    pub fn normalize_world_uvs(&self, world: &mut postretro_level_loader::LevelWorld) {
        let full = self.full();
        let loaded: &[LoadedTexture] = match (&full.pending_textures, &full.level) {
            (Some(pending), _) => &pending.loaded_textures,
            (None, Some(level)) => &level.loaded_textures,
            (None, None) => &[],
        };
        let texture_dimensions: Vec<(u32, u32)> = loaded_texture_dimensions(loaded)
            .into_iter()
            .map(|[width, height]| (width, height))
            .collect();
        postretro_render_cpu::uv_normalize::normalize_world_uvs_with_dims(
            world,
            &texture_dimensions,
        );
    }
}

pub(super) fn loaded_texture_dimensions(textures: &[LoadedTexture]) -> Vec<[u32; 2]> {
    textures
        .iter()
        .map(|tex| [tex.diffuse_texture.width(), tex.diffuse_texture.height()])
        .collect()
}
