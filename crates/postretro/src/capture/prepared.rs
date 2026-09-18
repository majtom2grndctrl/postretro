// Prepared static offscreen capture workload: level install, visibility, and receivers.
// See: context/lib/rendering_pipeline.md §7.8

use std::collections::HashSet;

use anyhow::{Context, Result, anyhow, bail};
use glam::{Mat4, Vec3};
use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry};
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

use crate::render::{
    CaptureAdapterIdentity, CaptureGpuTimingState, CaptureGpuTimingWindow, ClearColor,
    LevelGeometry, Renderer, ShResidencyReport, level_world_to_geometry,
};
use crate::runtime_movers::{
    ENGINE_AUTO_CLOSE_MS, KinematicMoverRenderCollector, spawn_loaded_kinematic_movers,
};
use crate::scripting::builtins::{ClassnameDispatch, apply_classname_dispatch, register_builtins};
use crate::scripting::map_entity::MapEntity;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::scripting_systems::mesh_anim::MeshClipTables;
use crate::scripting_systems::mesh_render::MeshRenderCollector;
use crate::startup::session::content_root_from_map;
use crate::startup::worker::derive_prm_root_dev_layout;

use super::driver::{
    capture_static_lights_and_shadow_selection, capture_view_projection, derive_texture_materials,
    install_capture_animated_promotion_bridge, install_forced_active_animation_descriptors,
    light_reachable_cell_mask, reachable_cell_aabbs, resolve_forced_animated_promotion_rows,
};
use super::scene::CaptureScene;

/// Portal-walk capture controls diagnostics only; capture has no diagnostic
/// consumer, so avoid allocating a one-frame trace.
const CAPTURE_PORTAL_WALK: bool = false;

/// Renderer-owned static scene state prepared once for one or more identical
/// offscreen capture frames. It retains every plain render input whose
/// collection depends on level loading, visibility, or receiver setup.
pub(super) struct PreparedCapture {
    renderer: Renderer,
    visible_cells: VisibleCells,
    visibility_path: VisibilityPath,
    light_reachable_cell_mask: Vec<bool>,
    reachable_cell_aabbs: Vec<(Vec3, Vec3)>,
    fog_reachable: Vec<u32>,
    camera_cell: u32,
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
                &world.entity_shadow_lights,
            );
        let geometry = LevelGeometry {
            lights: &static_lights,
            light_influences: &static_light_influences,
            entity_shadow_lights: &static_entity_shadow_lights,
            ..level_world_to_geometry(&world, &texture_materials)
        };
        renderer.install_level_geometry(&geometry);
        let forced_active_writes = install_forced_active_animation_descriptors(
            &mut renderer,
            &world.lights,
            scene.force_active.as_deref(),
        )?;
        let forced_promotion_weights = resolve_forced_animated_promotion_rows(
            &world.lights,
            world.animated_direct_sh_delta_volumes.as_ref(),
            scene.force_promotion.as_deref(),
        )?;

        let eye = Vec3::from_array(scene.camera.position);
        let view_proj = capture_view_projection(&scene.camera, width, height);
        let mut scratch = Vec::new();
        let (visibility, _frustum) = postretro_visibility::determine_visible_cells(
            eye,
            view_proj,
            &world,
            &[],
            CAPTURE_PORTAL_WALK,
            &mut scratch,
        );
        let visible_cells = visibility.visible_cells;
        let fog_reachable = visibility.fog_reachable;
        let stats = visibility.stats;
        let light_reachable_cell_mask = light_reachable_cell_mask(&world, &fog_reachable);
        let reachable_cell_aabbs = reachable_cell_aabbs(&world, &fog_reachable);

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
            &visible_cells,
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

        Ok(Self {
            renderer,
            visible_cells,
            visibility_path: stats.path,
            light_reachable_cell_mask,
            reachable_cell_aabbs,
            fog_reachable,
            camera_cell: stats.camera_cell,
            view_proj,
            eye,
            forced_promotion_weights,
            resolution: [width, height],
        })
    }

    /// Render the prepared static workload through the unchanged PNG/readback
    /// capture path.
    pub(super) fn capture_frame(&mut self) -> Result<Vec<u8>> {
        self.renderer.capture_frame_indirect(
            CameraCullVisibility {
                cells: &self.visible_cells,
                path: self.visibility_path,
            },
            &self.light_reachable_cell_mask,
            &self.reachable_cell_aabbs,
            &self.fog_reachable,
            Some(self.camera_cell),
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
        )
    }

    /// Submit and complete one prepared static sample without PNG readback.
    pub(super) fn capture_measurement_frame(&mut self) -> Result<Option<CaptureGpuTimingWindow>> {
        self.renderer.capture_measurement_frame_indirect(
            CameraCullVisibility {
                cells: &self.visible_cells,
                path: self.visibility_path,
            },
            &self.light_reachable_cell_mask,
            &self.reachable_cell_aabbs,
            &self.fog_reachable,
            Some(self.camera_cell),
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
        )
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

    pub(super) fn sh_residency_report(&self) -> Option<ShResidencyReport> {
        self.renderer.sh_residency_report().cloned()
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
