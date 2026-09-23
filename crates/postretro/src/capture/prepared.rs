// Prepared static offscreen capture workload: level install, visibility, and receivers.
// See: context/lib/rendering_pipeline.md §7.8

use std::collections::HashSet;

use anyhow::{Context, Result, anyhow, bail};
use glam::{Mat4, Vec3};
use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry};
use postretro_level_loader::requested_streaming_mode;
use postretro_visibility::VisibleCells;

use crate::render::{
    CaptureAdapterIdentity, CaptureGpuTimingState, CaptureGpuTimingWindow, ClearColor, Renderer,
    ShResidencyReport,
};
use crate::render_preparation::VisibleRenderPreparation;
use crate::runtime_movers::{
    ENGINE_AUTO_CLOSE_MS, KinematicMoverRenderCollector, spawn_loaded_kinematic_movers,
};
use crate::scripting::builtins::{ClassnameDispatch, apply_classname_dispatch, register_builtins};
use crate::scripting::map_entity::MapEntity;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::scripting_systems::mesh_anim::MeshClipTables;
use crate::scripting_systems::mesh_render::MeshRenderCollector;
use crate::session::sh_residency::{ShStreamingSession, require_sync_proof_mode};
use crate::sh_streaming::controller::SyncReadResult;
use crate::startup::session::content_root_from_map;
use crate::startup::worker::derive_prm_root_dev_layout;

use super::scene::CaptureScene;
use super::setup::{
    capture_level_geometry, capture_static_lights_and_shadow_selection, capture_view_projection,
    derive_texture_materials, install_capture_animated_promotion_bridge,
    install_forced_active_animation_descriptors,
    resolve_forced_animated_promotion_rows_from_metadata,
};

/// Portal-walk capture controls diagnostics only; capture has no diagnostic
/// consumer, so avoid allocating a one-frame trace.
const CAPTURE_PORTAL_WALK: bool = false;

/// Renderer-owned static scene state prepared once for one or more identical
/// offscreen capture frames. It retains every plain render input whose
/// collection depends on level loading, visibility, or receiver setup.
pub(super) struct PreparedCapture {
    renderer: Renderer,
    sh_streaming: Option<ShStreamingSession>,
    visible_render: VisibleRenderPreparation,
    view_proj: Mat4,
    eye: Vec3,
    forced_promotion_weights: Vec<(usize, f32)>,
    resolution: [u32; 2],
}

impl PreparedCapture {
    /// Load and install one VM-free capture scene. The resulting state can
    /// render the same authored instant without reloading the PRL, re-uploading
    /// textures, resolving visibility, or recollecting receivers.
    pub(super) fn prepare(scene: &CaptureScene) -> Result<Self> {
        // Load synchronously: capture creates no worker thread or event loop.
        let mut world = postretro_level_loader::load_prl(&scene.map)
            .with_context(|| format!("failed to load `{}`", scene.map))?;
        if world.sh_stream_manifest().is_some() {
            // Validate the PRL first, then enforce Task 10's explicit-mode
            // gate before GPU initialization can mask its named error.
            require_sync_proof_mode(requested_streaming_mode()?)?;
        }

        let [width, height] = scene.resolution;
        let mut renderer = Renderer::new_offscreen(width, height)
            .context("failed to initialize offscreen frame capture renderer")?;

        let texture_materials = derive_texture_materials(&world.texture_names);
        let content_root = content_root_from_map(Some(&scene.map));
        let prm_cache_root = derive_prm_root_dev_layout(&content_root);
        renderer.install_textures(
            &world.texture_names,
            &world.texture_cache_keys,
            &prm_cache_root,
            &texture_materials,
        );
        renderer.normalize_world_uvs(&mut world);
        let (static_lights, static_light_influences, static_entity_shadow_lights) =
            capture_static_lights_and_shadow_selection(
                &world.lights,
                &world.light_influences,
                world.entity_shadow_lights(),
            );
        let geometry = capture_level_geometry(
            &world,
            &texture_materials,
            &static_lights,
            &static_light_influences,
            &static_entity_shadow_lights,
        );
        renderer.install_level_geometry(&geometry);
        let forced_active_writes = install_forced_active_animation_descriptors(
            &mut renderer,
            &world.lights,
            scene.force_active.as_deref(),
        )?;
        let forced_promotion_weights = resolve_forced_animated_promotion_rows_from_metadata(
            &world.lights,
            world.animated_direct_descriptor_indices(),
            world.animated_direct_affinity_lights(),
            scene.force_promotion.as_deref(),
        )?;

        let eye = Vec3::from_array(scene.camera.position);
        let view_proj = capture_view_projection(&scene.camera, width, height);
        let mut scratch = Vec::new();
        let visible_render = VisibleRenderPreparation::for_level(
            &world,
            eye,
            view_proj,
            &[],
            CAPTURE_PORTAL_WALK,
            &mut scratch,
        );
        let sh_streaming = world
            .sh_stream_manifest()
            .cloned()
            .map(|manifest| ShStreamingSession::for_capture(manifest, &renderer))
            .transpose()?;
        let max_preload_frames = world
            .sh_stream_manifest()
            .map(|manifest| manifest.cluster_count() as usize)
            .unwrap_or(0)
            .saturating_mul(4)
            .saturating_add(2);

        // Capture has no script context or levelLoad event. Stand up only the
        // VM-free map-authored receiver state the windowed render frame collects.
        let mut registry = spawn_capture_receiver_registry(&world)?;
        if !forced_promotion_weights.is_empty() {
            install_capture_animated_promotion_bridge(
                &mut renderer,
                &world,
                &static_lights,
                &static_light_influences,
                &mut registry,
                &forced_active_writes,
            )?;
        }
        for model in capture_mesh_models(&registry)? {
            renderer
                .load_skinned_model(&model, &content_root, &prm_cache_root)
                .ok_or_else(|| anyhow!("failed to load capture receiver model `{model}`"))?;
        }
        let mut mover_collector = crate::runtime_movers::KinematicMoverRenderCollector::new();
        let mut mesh_collector = crate::scripting_systems::mesh_render::MeshRenderCollector::new();
        collect_capture_receiver_draws(
            &registry,
            &world,
            &visible_render.visible_cells,
            eye,
            &mut mover_collector,
            &mut mesh_collector,
        );
        renderer.set_kinematic_mover_draws(
            mover_collector.instances(),
            mover_collector.shadow_instances(),
        );
        renderer.set_mover_occluder_aabbs(mover_collector.occluder_aabbs());
        renderer.set_mesh_draws(mesh_collector.instances());

        let mut prepared = Self {
            renderer,
            sh_streaming,
            visible_render,
            view_proj,
            eye,
            forced_promotion_weights,
            resolution: [width, height],
        };
        prepared.preload_visible_sh(max_preload_frames)?;
        Ok(prepared)
    }

    /// Capture is a fixed authored instant, so make its visible SH closure
    /// sampleable before either PNG publication or timed measurement begins.
    fn preload_visible_sh(&mut self, max_frames: usize) -> Result<()> {
        let Some(streaming) = self.sh_streaming.as_mut() else {
            return Ok(());
        };
        streaming.update_targets(&self.visible_render.visible_cells, 0.0)?;
        for _ in 0..max_frames {
            if self
                .sh_streaming
                .as_ref()
                .is_some_and(ShStreamingSession::all_targets_sampleable)
            {
                self.renderer.reset_capture_measurement_timing();
                return Ok(());
            }
            loop {
                match self
                    .sh_streaming
                    .as_mut()
                    .expect("streaming capture was initialized")
                    .read_one_sync()?
                {
                    SyncReadResult::Prepared(_) => {}
                    SyncReadResult::NoTargetReady => break,
                }
            }
            let _ = self.capture_measurement_frame()?;
        }
        bail!("SH capture preload did not make its visible cluster closure sampleable")
    }

    fn take_sh_drain_batch(&mut self) -> Result<postretro_level_loader::ShDrainBatch> {
        self.sh_streaming
            .as_mut()
            .map(ShStreamingSession::prepare_batch)
            .transpose()
            .map(|batch| batch.unwrap_or_default())
    }

    /// Render the prepared static workload through the unchanged PNG/readback
    /// capture path.
    pub(super) fn capture_frame(&mut self) -> Result<Vec<u8>> {
        let sh_drain_batch = self.take_sh_drain_batch()?;
        let result = self.renderer.capture_frame_indirect(
            self.visible_render.camera_cull(),
            &self.visible_render.light_reachable_cell_mask,
            &self.visible_render.reachable_cell_aabbs,
            &self.visible_render.fog_reachable,
            Some(self.visible_render.stats.camera_cell),
            self.view_proj,
            self.eye,
            &[],
            &self.forced_promotion_weights,
            ClearColor {
                r: 0.05,
                g: 0.05,
                b: 0.08,
                a: 1.0,
            },
            true,
            sh_drain_batch,
        )?;
        if let Some(streaming) = self.sh_streaming.as_mut() {
            streaming.apply_outcome(result.outcome, &self.renderer)?;
            if result.compose_submitted {
                streaming.mark_compose_submitted();
            }
        }
        result.frame
    }

    /// Submit and complete one prepared static sample without PNG readback.
    pub(super) fn capture_measurement_frame(&mut self) -> Result<Option<CaptureGpuTimingWindow>> {
        let sh_drain_batch = self.take_sh_drain_batch()?;
        let result = self.renderer.capture_measurement_frame_indirect(
            self.visible_render.camera_cull(),
            &self.visible_render.light_reachable_cell_mask,
            &self.visible_render.reachable_cell_aabbs,
            &self.visible_render.fog_reachable,
            Some(self.visible_render.stats.camera_cell),
            self.view_proj,
            self.eye,
            &[],
            &self.forced_promotion_weights,
            ClearColor {
                r: 0.05,
                g: 0.05,
                b: 0.08,
                a: 1.0,
            },
            true,
            sh_drain_batch,
        )?;
        if let Some(streaming) = self.sh_streaming.as_mut() {
            streaming.apply_outcome(result.outcome, &self.renderer)?;
            if result.compose_submitted {
                streaming.mark_compose_submitted();
            }
        }
        result.frame
    }

    /// Start a fresh GPU timing window after warmup submissions complete.
    pub(super) fn reset_measurement_timing(&mut self) {
        self.renderer.reset_capture_measurement_timing();
    }

    pub(super) fn measurement_adapter_identity(&self) -> CaptureAdapterIdentity {
        self.renderer.capture_measurement_adapter_identity().clone()
    }

    pub(super) fn measurement_timing_state(&self) -> CaptureGpuTimingState {
        self.renderer.capture_measurement_timing_state()
    }

    pub(super) fn sh_residency_report(&self) -> Result<Option<ShResidencyReport>> {
        let report = self.renderer.sh_residency_report();
        let Some(streaming) = self.sh_streaming.as_ref() else {
            return Ok(report);
        };
        let lifecycle = streaming.residency_lifecycle_summary()?;
        Ok(report.map(|report| report.with_streaming_lifecycle_summary(lifecycle)))
    }

    pub(super) const fn resolution(&self) -> [u32; 2] {
        self.resolution
    }
}

/// Materialize just the VM-free receivers capture can render at a loaded
/// instant. This deliberately does not call `install_world_cpu`: that path
/// also runs data scripts and fires `levelLoad`.
pub(super) fn spawn_capture_receiver_registry(
    world: &postretro_level_loader::LevelWorld,
) -> Result<EntityRegistry> {
    let mut registry = EntityRegistry::new();
    spawn_loaded_kinematic_movers(&mut registry, world, ENGINE_AUTO_CLOSE_MS)
        .context("failed to spawn capture kinematic movers")?;

    // `MapEntity` is the scripting-facing adapter over PRL records. Built-in
    // dispatch is VM-free, so this gives capture the exact map-authored
    // `prop_mesh` spawn contract without admitting the data-script sweep.
    let map_entities: Vec<MapEntity> = world.map_entities.iter().cloned().map(Into::into).collect();
    let mut dispatch = ClassnameDispatch::new();
    register_builtins(&mut dispatch);
    apply_classname_dispatch(&map_entities, &dispatch, &mut registry);

    Ok(registry)
}

/// Validate model handles placed by capture's built-in map dispatch. An empty
/// prop handle must fail capture instead of silently removing its draw.
/// The renderer cache key is this verbatim string, so load it before the mesh
/// collector submits an instance using the same handle.
pub(super) fn capture_mesh_models(registry: &EntityRegistry) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for (id, value) in registry.iter_with_kind(ComponentKind::Mesh) {
        let ComponentValue::Mesh(mesh) = value else {
            continue;
        };
        if mesh.model.is_empty() {
            bail!("capture prop_mesh receiver {id:?} has an absent or empty `model` key");
        }
        if seen.insert(mesh.model.clone()) {
            models.push(mesh.model.clone());
        }
    }
    Ok(models)
}

/// Mirror the windowed render-frame collector calls for capture's spawned
/// receivers. A single-instant capture has no tick history or animation clock:
/// alpha is therefore 1.0 and animation time is 0.0.
pub(super) fn collect_capture_receiver_draws(
    registry: &EntityRegistry,
    world: &postretro_level_loader::LevelWorld,
    visible_cells: &VisibleCells,
    eye: Vec3,
    mover_collector: &mut KinematicMoverRenderCollector,
    mesh_collector: &mut MeshRenderCollector,
) {
    mover_collector.collect(registry, world, visible_cells, 1.0);

    let clip_tables = MeshClipTables::new();
    let hit_zones = HitZoneStore::new();
    mesh_collector.collect_with_hit_zones(
        registry,
        world,
        visible_cells,
        1.0,
        0.0,
        &clip_tables,
        eye,
        &hit_zones,
    );
}
