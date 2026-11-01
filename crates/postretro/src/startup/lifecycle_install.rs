//! Level install stages the commit shares with the main-thread oracle
//! (identity and parity, segment A, world stages, light bridge), plus the
//! oracle's own sequencer and renderer stage, compiled for tests only.
//! See: context/lib/boot_sequence.md §3

use super::lifecycle_install_presentation::{LevelFogMasks, LevelSounds};
use super::lifecycle_model_sweep::LevelModels;
use super::*;

/// Why a delivered level payload was refused before any install mutation.
/// Routed through `finish_level_failure` like any other failed level load.
/// The main-thread oracle's, so test-only; production loads fail through
/// their stages.
#[cfg(test)]
#[derive(Debug, thiserror::Error)]
pub(crate) enum LevelInstallRejection {
    #[error(transparent)]
    GeometryRange(#[from] render::LevelGeometryRangeError),
    #[error("static collision: {0}")]
    StaticCollision(#[from] crate::collision::CollisionMeshError),
    /// The renderer refused the level's resources: only a streamed-SH
    /// residency plan or first pool generation can. It arrives after identity
    /// and segment A ran, but ends the load through the same route.
    #[error(transparent)]
    RendererInstall(#[from] render::LevelInstallError),
}

/// Where the stages after the renderer stage take the level's models, fog
/// masks and sounds from: computed on this thread (the main-thread install),
/// or a level load's bundle products (the commit).
pub(super) struct LevelPresentation<'a> {
    pub(super) models: LevelModels<'a>,
    pub(super) fog_masks: LevelFogMasks,
    pub(super) sounds: LevelSounds,
}

/// What the renderer stage hands the light bridge: map lights, their
/// influences, the baked animation descriptors, the animated baked roster, and
/// the scripted-sample float count.
pub(super) type LevelLightSeed = (
    Vec<postretro_level_loader::MapLight>,
    Vec<postretro_render_data::influence::LightInfluence>,
    Vec<postretro_level_format::sh_volume::AnimationDescriptor>,
    Vec<u32>,
    u32,
);

impl App {
    pub(super) fn retain_active_level_tags_for_install(&mut self) {
        if let Some(load) = self.level_load.as_ref() {
            self.active_level_tags = load.entry.tags.clone();
            self.active_level_source = Some(level_source_for_load_entry(&load.entry));
        } else {
            self.active_level_tags.clear();
            self.active_level_source = None;
        }
    }

    /// Install a delivered level payload on the main thread: GPU texture upload
    /// (from baked `.prm` mip sidecars), UV normalization, GPU geometry upload,
    /// light bridge, collision commit, fog/trigger populate, classname dispatch, data script,
    /// archetype sweep, and `levelLoad` fire. Each stage is recorded into
    /// `self.level_timings` for log line C.
    ///
    /// Texture upload runs before geometry upload: `.prm` slot dimensions
    /// drive UV normalization, so the renderer must have produced
    /// `LoadedTexture`s before the per-leaf texel-space UVs can be converted to
    /// `[0,1]`.
    ///
    /// The main-thread test oracle; production never calls it. Assumes
    /// `self.renderer` is `Some` and `world` is populated. Rejects invalid
    /// indirect index ranges and a static collision mesh parry refuses before
    /// any install mutation.
    // The main-thread oracle, compiled for tests only: production loads
    // commit a bundle instead; the staged-load tests compare the commit
    // against this install.
    #[cfg(test)]
    pub(super) fn install_level_payload(
        &mut self,
        mut world: postretro_level_loader::LevelWorld,
        prm_cache_root: PathBuf,
    ) -> Result<(), LevelInstallRejection> {
        // Before parity, navigation, textures, or any other install mutation:
        // release indirect draws rely on this unchanged BVH/index mapping.
        render::validate_level_geometry_ranges(&world.bvh.leaves, world.indices.len())?;
        // The static collision trimesh is built here, also before any mutation,
        // so a mesh parry rejects fails the load through the same route. Install
        // always follows unload, so what stays untouched is the empty world.
        // Committed just before segment B.
        let static_collision = crate::collision::CollisionWorld::from_level(&world)?;
        self.level_timings.record("static_collision");
        self.install_level_identity(&world);
        let script_ctx = self.install_level_segment_a(&world);
        let Some(light_seed) = self.install_level_renderer_stage(&mut world, &prm_cache_root)?
        else {
            self.install_level_without_renderer(world, static_collision);
            return Ok(());
        };
        self.install_level_world_stages(
            world,
            &script_ctx,
            light_seed,
            static_collision,
            &prm_cache_root,
            LevelPresentation {
                models: LevelModels::Parse,
                fog_masks: LevelFogMasks::Upload,
                sounds: LevelSounds::Load,
            },
        );
        Ok(())
    }

    /// The renderer-less route: no renderer, so no renderer stage and none
    /// of the stages after it. The level stays installed, so its collision
    /// does too.
    pub(super) fn install_level_without_renderer(
        &mut self,
        world: postretro_level_loader::LevelWorld,
        static_collision: crate::collision::CollisionWorld,
    ) {
        self.collision_world = static_collision;
        self.level = Some(world);
        // Keeps the App's drift current for tests and later
        // refreshes; there is no renderer to hand it to.
        self.resolve_fluid_surface_drift();
    }

    /// Every stage after the renderer stage, shared by the main-thread
    /// install and a level load's commit: the level becomes `App::level`,
    /// then the light bridge, segment B with the level's models, the
    /// post-load passes, the camera pose, fog masks, sprite collections,
    /// sounds, and the level clocks. `presentation` says where the models,
    /// fog masks and sounds come from. Returns what sprite registration
    /// created, a counted exception at commit.
    pub(super) fn install_level_world_stages(
        &mut self,
        world: postretro_level_loader::LevelWorld,
        script_ctx: &postretro_entities::ScriptCtx,
        light_seed: LevelLightSeed,
        static_collision: crate::collision::CollisionWorld,
        prm_cache_root: &Path,
        presentation: LevelPresentation<'_>,
    ) -> postretro_renderer::GpuCreationCounts {
        let LevelPresentation {
            models,
            fog_masks,
            sounds,
        } = presentation;
        // Stash the world after the renderer mutations so downstream reads of
        // `self.level` (and segment B) see the normalized vertices.
        self.level = Some(world);
        self.install_level_light_bridge(script_ctx, light_seed);

        // `suppress` gates connected-client suppression of host-replicated world
        // placements and the boot pawn (`false` for single-player, listen host,
        // and headless installs).
        let suppress = self.is_connected_client();
        let products = self.install_level_segment_b(
            script_ctx,
            prm_cache_root,
            static_collision,
            suppress,
            models,
        );
        let first_spawn = self.install_level_post_load_passes(script_ctx, products);
        self.install_level_camera_pose(first_spawn);
        self.install_level_fog_masks(fog_masks);
        let sprite_created =
            self.install_level_sprite_collections(script_ctx, prm_cache_root, suppress);
        self.install_level_sounds(sounds);

        self.script_time = 0.0;
        // Animation clock is level-relative like `script_time`. The scale field
        // is engine config, not level state, so it is not reset here.
        self.anim_time = 0.0;
        sprite_created
    }

    /// Environment volumes, per-level view-feel resets and the retained level
    /// tags, then the join seed and level parity a peer is admitted on.
    pub(super) fn install_level_identity(&mut self, world: &postretro_level_loader::LevelWorld) {
        // Before the parity digest is published and before any tick: a peer
        // admitted on this level's digest, and the first tick, resolve against
        // this level's volumes, never an empty or stale set. Assigned whole, so
        // no earlier level's volume survives; the renderer-less return in
        // `install_level_payload` keeps it installed.
        self.install_environment_volumes(world);
        self.view_feel_state = crate::view_feel::ViewFeelState::default();
        self.view_feel_followed_pawn = None;
        self.view_feel_descriptor = None;
        self.retain_active_level_tags_for_install();
        let join_seed = {
            let session = self
                .session
                .as_ref()
                .expect("session installed before level install");
            crate::scripting::state_persistence::join_seed_from_persisted_state(
                session.persisted_state.as_ref(),
                session.player_options.player_id,
            )
        };
        if let Some(endpoint) = self
            .session
            .as_mut()
            .expect("session installed before level install")
            .net_endpoint
            .as_mut()
        {
            let level_content_digest =
                crate::runtime_movers::level_content_digest(&world.kinematic_geometry, world);
            let source = self
                .active_level_source
                .as_ref()
                .expect("active level source retained before parity installation");
            endpoint.set_join_seed(join_seed);
            let identity = level_identity(source, &self.content_root);
            self.published_level_identity = Some(identity.clone());
            endpoint.set_level_parity(Some((identity, level_content_digest)));
            endpoint.set_relevel_catalog_id(match source {
                LevelSource::Catalog(id) => Some(id.clone()),
                LevelSource::Path(_) => None,
            });
        }
    }

    /// The script handle the later stages share, deferred presentation
    /// descriptors, segment A (gravity and navigation), and the per-level resets
    /// of session presentation state.
    pub(super) fn install_level_segment_a(
        &mut self,
        world: &postretro_level_loader::LevelWorld,
    ) -> postretro_entities::ScriptCtx {
        // The whole script tranche lives on `Session` (built post-first-pixel).
        // Level install only runs in Loading/Running, where the session is
        // installed. Clone the `ScriptCtx` handle (cheap `Rc` bump) so the many
        // `script_ctx.*` reads below borrow nothing of `self` — the non-`Clone`
        // session subsystems (bridges, collectors, registries) are reached
        // through disjoint `self.session.as_mut()` borrows at each site, kept
        // disjoint from the long-lived `renderer` borrow.
        let script_ctx = self
            .session
            .as_ref()
            .expect("session installed before level install")
            .scripting
            .script_ctx
            .clone();
        if self
            .session
            .as_mut()
            .expect("session installed before level install")
            .scripting
            .script_runtime
            .install_deferred_mesh_descriptors(&script_ctx)
        {
            log::info!(
                "[Scripting] installed deferred presentation descriptors for the next level model sweep"
            );
        }
        // Segment A of the CPU world install: seed gravity from the level's
        // authored value (before the data script runs, so a `getGravity()`
        // in `setupLevel` / `levelLoad` reactions sees it) and build the runtime
        // navigation graph. Renderer-free; the nav build reads the un-normalized
        // navmesh section, which the renderer UV pass below does not touch. The
        // windowed path runs its renderer upload and — critically — the
        // light-bridge populate BETWEEN segments A and B, so light entity ids
        // precede the fog entity ids segment B creates (both bridges key dirty
        // tracking on `EntityId`).
        self.nav_graph = install_world_gravity_and_nav(world, &script_ctx);
        let session = self
            .session
            .as_mut()
            .expect("session installed before level install");
        // Clear any in-flight `screen.flash` decay so a flash never bleeds
        // across a level load.
        session.scripting.flash_decay.reset();
        // Clear any in-flight vignette/shake (SE) so neither bleeds across a
        // level load — the slots reset to their identity rest values.
        session.scripting.vignette_decay.reset();
        session.scripting.shake_decay.reset();
        // Reset the input-mode tracker so a mid-transition mode never bleeds
        // across levels.
        session.scripting.input_mode_tracker.reset();
        script_ctx
    }

    /// Player render tiers, then textures, UV normalize, geometry and fluid
    /// surfaces. Returns what the light bridge needs, or `None` when no
    /// renderer exists and the caller takes the renderer-less return. A
    /// renderer refusal of the level's resources is a rejection.
    // The main-thread oracle's, compiled for tests only: a load's set
    // already holds this work when its bundle commits.
    #[cfg(test)]
    fn install_level_renderer_stage(
        &mut self,
        world: &mut postretro_level_loader::LevelWorld,
        prm_cache_root: &Path,
    ) -> Result<Option<LevelLightSeed>, LevelInstallRejection> {
        // Derive material properties from texture names so the renderer can
        // populate per-material uniforms (shininess) without re-parsing. Load
        // prep builds the same table with the same function.
        let texture_materials = crate::level_load::level_texture_materials(&world.texture_names);

        let (shadow_quality, surface_depth_quality, reflection_quality) = {
            let options = &self
                .session
                .as_ref()
                .expect("session installed before level install")
                .player_options;
            (
                options.shadow_quality,
                options.surface_depth_quality,
                options.reflection_quality,
            )
        };
        self.configure_player_shadow_quality(shadow_quality);
        // Before `install_textures`, so every material this level builds is
        // packed at the player's tier rather than built at the default and
        // rewritten.
        self.apply_player_surface_depth_quality(surface_depth_quality);
        // Before the fluid install, which sizes the reflection target from
        // the kept tier. The tier the renderer already holds is a no-op.
        self.apply_player_reflection_quality(reflection_quality);
        let renderer = match self.renderer.as_mut() {
            Some(r) => r,
            None => {
                log::error!("[Engine] install_level_payload called with no renderer");
                return Ok(None);
            }
        };

        // 1. Textures first — uploaded from the .prm sidecars; their slot
        //    dimensions feed the UV normalize pass.
        renderer.install_textures(
            &world.texture_names,
            &world.texture_cache_keys,
            prm_cache_root,
            &texture_materials,
        );
        self.level_timings.record("texture_upload");

        // 2. UV normalize using freshly-uploaded diffuse-texture dimensions.
        //    The parsed level carries texel-space UVs; converted to `[0,1]`
        //    here so install_level_geometry uploads the final values. Staged
        //    loads normalize in prep on the load pool instead.
        renderer.normalize_world_uvs(world);
        self.level_timings.record("uv_normalize");

        // 3. Now geometry: vertex_buffer + index_buffer upload to GPU. The
        //    lightmap and shadowmask payloads move into the upload, which
        //    drops them; the stashed world keeps only their headers.
        let gpu_lighting_payloads = world.take_gpu_lighting_payloads();
        let geometry = render::level_world_to_geometry(world, &texture_materials);
        // Test seam: a refusal no fixture's real data provokes.
        if let Some(error) = super::lifecycle_test_fixtures::take_injected_geometry_install_error()
        {
            return Err(error.into());
        }
        renderer.install_level_geometry(&geometry, gpu_lighting_payloads)?;
        self.level_timings.record("geometry_upload");
        // Fluid surfaces after textures (their UVs normalize by the
        // installed dimensions) and geometry (which drops the last level's).
        renderer.install_fluid_surfaces(
            &world.environment_volumes.volumes,
            world.reflection_probes.as_ref(),
        );

        Ok(Some(self.install_level_renderer_tail(world)))
    }

    /// The renderer stage's tail on both paths, once the level's set is the
    /// active one: reseed the SH diagnostics and hand the light bridge its
    /// seed, whose scripted-sample count the active set decides.
    pub(super) fn install_level_renderer_tail(
        &mut self,
        world: &postretro_level_loader::LevelWorld,
    ) -> LevelLightSeed {
        let renderer = self
            .renderer
            .as_ref()
            .expect("the renderer stage runs only with a renderer");
        let fgd_sample_float_count = (renderer.scripted_sample_byte_offset() / 4) as u32;
        // Reseed the SH diagnostic per-light visibility bitmap to match the
        // freshly-installed level's animated-light count. Reset `seeded` so the
        // panel re-pulls defaults on the next open. `debug_ui` is session-owned;
        // read the renderer's delta count (disjoint `self.renderer` borrow) first.
        #[cfg(feature = "dev-tools")]
        {
            let delta_count = renderer.sh_delta_volumes().len();
            if let Some(debug_ui) = self
                .session
                .as_mut()
                .and_then(|session| session.debug_ui.as_mut())
            {
                debug_ui.sh_diagnostics_state.per_light_visible.clear();
                debug_ui
                    .sh_diagnostics_state
                    .per_light_visible
                    .resize(delta_count, false);
                debug_ui.sh_diagnostics_state.seeded = false;
            }
        }

        (
            world.lights.clone(),
            world.light_influences.clone(),
            world.animation_descriptors().to_vec(),
            world.animated_direct_descriptor_indices().to_vec(),
            fgd_sample_float_count,
        )
    }

    /// One light entity per map-authored light, before segment B creates the
    /// fog entities.
    fn install_level_light_bridge(
        &mut self,
        script_ctx: &postretro_entities::ScriptCtx,
        light_seed: LevelLightSeed,
    ) {
        let (
            map_lights,
            map_light_influences,
            baked_light_descriptors,
            animated_baked_descriptor_indices,
            fgd_sample_float_count,
        ) = light_seed;
        // One `LightComponent` entity per map-authored light; stable
        // `EntityId`s the bridge's dirty tracker keys off for the level's
        // lifetime.
        {
            let mut registry = script_ctx.registry.borrow_mut();
            self.session
                .as_mut()
                .expect("session installed before level install")
                .light_bridge
                .populate_from_level_with_influences(
                    &map_lights,
                    &map_light_influences,
                    &baked_light_descriptors,
                    &mut registry,
                    fgd_sample_float_count,
                );
            self.session
                .as_mut()
                .expect("session installed before level install")
                .light_bridge
                .set_animated_baked_promotion_roster(&animated_baked_descriptor_indices);
        }
    }
}
