// Prepared static offscreen capture: level install, visibility, preloads and frame submission.
// See: context/lib/rendering_pipeline.md §7.8

use anyhow::{Context, Result, anyhow, bail};
use glam::{Mat4, Vec3};
use postretro_level_loader::requested_streaming_mode;

use crate::render::{
    CaptureAdapterIdentity, CaptureGpuTimingState, CaptureGpuTimingWindow, ClearColor,
    LightmapResidencyReport, Renderer, ShResidencyReport, ShSampleRegion,
};
use crate::render_preparation::VisibleRenderPreparation;
use crate::scripting_systems::ghost_render::GhostRenderCollector;
use crate::scripting_systems::mover_ghost_render::MoverGhostRenderCollector;
use crate::session::sh_residency::{ShStreamingSession, require_sync_proof_mode};
use crate::sh_streaming::controller::SyncReadResult;
use crate::startup::session::content_root_from_map;
use crate::startup::worker::derive_prm_root_dev_layout;
use crate::streaming::cell_demand::CellDemand;
use crate::streaming::cluster_hints::decode_level_hints;
use postretro_level_format::cell_residency_set::CellResidencySetSection;

use super::lightmap::{
    CaptureLightmapResidency, preload_capture_lightmap, validate_lightmap_overrides,
};
use super::planar::{CapturePlanar, CapturePlanarReport};
use super::receivers::{
    CaptureCullViews, capture_mesh_models, collect_capture_receiver_draws_with_mirror,
    spawn_capture_receiver_registry,
};
use super::scene::CaptureScene;
use super::setup::{
    apply_scene_reflection, capture_camera, capture_level_geometry,
    capture_static_lights_and_shadow_selection, derive_texture_materials,
    install_capture_animated_promotion_bridge, install_forced_active_animation_descriptors,
    resolve_forced_animated_promotion_rows_from_metadata,
};

/// Portal-walk capture controls diagnostics only; capture has no diagnostic
/// consumer, so avoid allocating a one-frame trace.
const CAPTURE_PORTAL_WALK: bool = false;
/// Measurement advances authored GPU animation without running gameplay or a
/// script VM. Keep this aligned with the engine's default 60 Hz fixed tick so
/// capture samples have an easy-to-reason-about temporal coordinate.
const CAPTURE_MEASUREMENT_ANIMATION_TICKS_PER_SECOND: f64 = 60.0;

#[derive(Debug, Default)]
struct CaptureMeasurementAnimationClock {
    ticks: u64,
}

impl CaptureMeasurementAnimationClock {
    fn advance_frame(&mut self) -> f32 {
        self.ticks = self
            .ticks
            .checked_add(1)
            .expect("capture measurement tick count must fit u64");
        measurement_animation_time_seconds(self.ticks)
    }

    fn time_seconds(&self) -> f32 {
        measurement_animation_time_seconds(self.ticks)
    }
}

pub(super) fn measurement_animation_time_seconds(ticks: u64) -> f32 {
    (ticks as f64 / CAPTURE_MEASUREMENT_ANIMATION_TICKS_PER_SECOND) as f32
}

/// Renderer-owned static scene state prepared once for one or more identical
/// offscreen capture frames. It retains every plain render input whose
/// collection depends on level loading, visibility, or receiver setup.
pub(super) struct PreparedCapture {
    renderer: Renderer,
    sh_streaming: Option<ShStreamingSession>,
    visible_render: VisibleRenderPreparation,
    /// The planar mirror at the scene's tier; inert below very high.
    planar: CapturePlanar,
    view_proj: Mat4,
    eye: Vec3,
    forced_promotion_weights: Vec<(usize, f32)>,
    mover_sample_regions: Vec<ShSampleRegion>,
    measurement_animation: CaptureMeasurementAnimationClock,
    resolution: [u32; 2],
    lightmap_residency: CaptureLightmapResidency,
}

impl PreparedCapture {
    /// Load and install one VM-free capture scene. The resulting state can
    /// render the same authored instant without reloading the PRL, re-uploading
    /// textures, resolving visibility, or recollecting receivers.
    pub(super) fn prepare(scene: &CaptureScene) -> Result<Self> {
        // Load synchronously: capture creates no worker thread or event loop.
        let mut world = postretro_level_loader::load_prl(&scene.map)
            .with_context(|| format!("failed to load `{}`", scene.map))?;
        // Check before renderer construction/setters or the first texture upload.
        // Capture's geometry adapter and UV normalization preserve these slices.
        crate::render::validate_level_geometry_ranges(&world.bvh.leaves, world.indices.len())
            .context("capture level failed the indirect index-range check")?;
        if world.sh_stream_manifest().is_some() {
            // Validate the PRL first, then enforce the sync-proof mode gate
            // (`require_sync_proof_mode`) before GPU initialization can mask
            // its named error.
            require_sync_proof_mode(requested_streaming_mode()?)?;
        }
        validate_lightmap_overrides(scene, &world)?;

        let [width, height] = scene.resolution;
        let mut renderer = Renderer::new_offscreen(width, height)
            .context("failed to initialize offscreen frame capture renderer")?;
        renderer.set_force_full_resident_sh_compose(scene.force_full_resident_sh_compose);
        if let Some(mask) = scene.light_term_mask() {
            renderer.set_light_term_mask(mask);
        }
        // Before the fluid install sizes a reflection target from the tier.
        apply_scene_reflection(&mut renderer, scene);
        if let Some(mode) = scene.ghost_transparency {
            renderer.set_ghost_transparency(mode.renderer());
        }
        renderer.set_cpu_timing(crate::cpu_timing::gate_from_env());

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
        // Capture installs the way the game does: the payloads move into the
        // upload and the world keeps headers only.
        let gpu_lighting_payloads = world.take_gpu_lighting_payloads();
        let mut geometry = capture_level_geometry(
            &world,
            &texture_materials,
            &static_lights,
            &static_light_influences,
            &static_entity_shadow_lights,
        );
        if let (Some(streaming), Some(cap)) = (
            geometry.lightmap_streaming.as_mut(),
            scene.lightmap_pool_cap_layers,
        ) {
            streaming.pool_cap_layers = cap;
        }
        renderer.install_level_geometry(&geometry, gpu_lighting_payloads);
        renderer.install_fluid_surfaces(
            &world.environment_volumes.volumes,
            world.reflection_probes.as_ref(),
        );
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

        let camera = capture_camera(&scene.camera, width, height);
        let eye = camera.eye_position;
        let view_proj = camera.view_projection;
        let mut scratch = postretro_visibility::PortalWalkScratch::new();
        let visible_render = VisibleRenderPreparation::for_level(
            &world,
            eye,
            view_proj,
            &[],
            CAPTURE_PORTAL_WALK,
            &mut scratch,
            postretro_visibility::TimingGate::OFF,
        );
        // The mirror step runs after the camera walk, before any consumer
        // reads visibility, and before the first frame (the preloads below).
        let planar = CapturePlanar::prepare(&world, &mut renderer, scene, &camera, &visible_render);
        // The level's id 49, decoded once for SH and lightmaps.
        let hints = decode_level_hints(world.cluster_directory())
            .context("[Capture] id-49 cluster hints")?;
        let lightmap_residency = preload_capture_lightmap(
            &world,
            &mut renderer,
            &visible_render,
            planar.drawn_cells(&visible_render.visible_cells),
            scene,
            hints.as_deref(),
        )?;
        let sh_streaming = world
            .sh_stream_manifest()
            .cloned()
            .map(|manifest| {
                let hints = hints
                    .clone()
                    .context("[Capture] a streamed SH level carries no id 49")?;
                ShStreamingSession::for_capture(manifest, &renderer, hints)
            })
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
        let mut ghost_links = crate::ghost_links::GhostLinks::install(&world, &registry);
        // Capture loads no manifest: a scene's own style table stands in for
        // `ghostStyles`. Without one, every link keeps the floor style.
        if let Some(styles) = scene.ghost_styles.as_deref() {
            ghost_links.bind_styles(&world, &super::ghosts::ghost_style_registry(styles));
        }
        let mut ghost_collector = GhostRenderCollector::new();
        let mut mover_ghost_collector = MoverGhostRenderCollector::new();
        collect_capture_receiver_draws_with_mirror(
            &registry,
            &world,
            CaptureCullViews {
                main: &visible_render.visible_cells,
                drawn: planar.drawn_cells(&visible_render.visible_cells),
                mirror: planar.mirror_cells(),
            },
            eye,
            &mut mover_collector,
            &mut mesh_collector,
            &mut ghost_links,
            &mut ghost_collector,
            &mut mover_ghost_collector,
        );
        renderer.set_kinematic_mover_draws(
            mover_collector.instances(),
            mover_collector.shadow_instances(),
        );
        renderer.set_kinematic_ghost_draws(mover_ghost_collector.instances());
        renderer.set_mover_occluder_aabbs(mover_collector.occluder_aabbs());
        // Capture loads no manifest, so every fluid volume draws at the fixed
        // capture opacity, with no fog or wave, reflecting at reflectivity 1
        // wherever a face binds a probe.
        let mut fluid_collector =
            postretro_render_cpu::fluid_surface::FluidSurfaceRenderCollector::new();
        renderer.set_fluid_surface_draws(fluid_collector.collect(
            &world.environment_volumes.volumes,
            &visible_render.visible_cells,
            |cell| world.cell_bounds(cell as usize),
            eye,
            |volume| {
                (!world.environment_volumes.volumes[volume].fluid.is_empty()).then(|| {
                    crate::fluid_presentation::capture_surface_look(
                        postretro_render_cpu::fluid_surface::reflection::volume_has_bound_face(
                            world.reflection_probes.as_ref(),
                            volume,
                        ),
                    )
                })
            },
        ));
        renderer.set_mesh_draws(mesh_collector.instances());
        renderer.set_ghost_mesh_draws(ghost_collector.instances());
        let mover_sample_regions = mover_collector.sh_sample_regions().to_vec();

        let mut prepared = Self {
            renderer,
            sh_streaming,
            visible_render,
            planar,
            view_proj,
            eye,
            forced_promotion_weights,
            mover_sample_regions,
            measurement_animation: CaptureMeasurementAnimationClock::default(),
            resolution: [width, height],
            lightmap_residency,
        };
        prepared.preload_visible_sh(max_preload_frames, world.cell_residency_set.as_ref())?;
        Ok(prepared)
    }

    /// Capture is a fixed authored instant, so make its settle set sampleable
    /// before either PNG publication or timed measurement begins: the same SH
    /// answer the settle chokepoint asks a level entry. Optional targets
    /// (seam-warm, band) do not hold it.
    fn preload_visible_sh(
        &mut self,
        max_frames: usize,
        residency_set: Option<&CellResidencySetSection>,
    ) -> Result<()> {
        let Some(streaming) = self.sh_streaming.as_mut() else {
            return Ok(());
        };
        // Capture never sets L: its reach is a fresh stage's default.
        let reach = residency_set.map(|set| {
            CellDemand::new(set.max_lead).frame(
                set,
                self.visible_render.stats.camera_cell,
                self.visible_render.stats.path,
                self.planar.drawn_cells(&self.visible_render.visible_cells),
            )
        });
        // Like Settling, the preload presents nothing: it counts no miss.
        streaming.suspend_visible_misses(true);
        streaming.update_targets(
            self.planar
                .sh_visible_cells(&self.visible_render.visible_cells),
            reach,
            0.0,
        )?;
        for _ in 0..max_frames {
            if self
                .sh_streaming
                .as_ref()
                .is_some_and(|streaming| streaming.unsettled_targets() == Some(0))
            {
                self.renderer.reset_capture_measurement_timing();
                if let Some(streaming) = self.sh_streaming.as_mut() {
                    streaming.suspend_visible_misses(false);
                }
                return Ok(());
            }
            while let SyncReadResult::Prepared(_) = self
                .sh_streaming
                .as_mut()
                .expect("streaming capture was initialized")
                .read_one_sync()?
            {}
            // Preload only warms SH residency. Pinning here would feed each
            // forced `w` back into the promotion ramp, which cannot advance at
            // capture's frozen instant, so a pinned zero would drop the row's
            // record before the captured frame could pin it.
            let _ = self.submit_frame_without_readback(false, 0.0)?;
        }
        bail!("SH capture preload did not make its settle set sampleable")
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
        self.capture_frame_at(0.0)
    }

    /// Measurement's inspectable PNG uses the same stepped animation instant
    /// as its completed warmup/sample sequence. Ordinary capture remains at 0.
    pub(super) fn capture_measurement_output_frame(&mut self) -> Result<Vec<u8>> {
        self.capture_frame_at(self.measurement_animation.time_seconds())
    }

    /// Hand the renderer the planar mirror's light reach for the next world
    /// frame. Each frame's light-slot update consumes it, so every frame
    /// capture renders (SH preload, warmup, samples, the PNG) sets it again,
    /// as play does; set once, every frame after the first would rank no
    /// mirror-only light.
    fn set_mirror_light_reach(&mut self) {
        self.renderer
            .set_planar_mirror_light_reach(self.planar.mirror_light_cell_aabbs());
    }

    fn capture_frame_at(&mut self, animation_time_seconds: f32) -> Result<Vec<u8>> {
        let sh_drain_batch = self.take_sh_drain_batch()?;
        self.set_mirror_light_reach();
        let result = self.renderer.capture_frame_indirect(
            self.visible_render.camera_cull(),
            &self.visible_render.light_reachable_cell_mask,
            &self.visible_render.reachable_cell_aabbs,
            &self.visible_render.fog_reachable,
            postretro_renderer::ShSampleRegionSets {
                visible_cells: self.planar.drawn_cells(&self.visible_render.visible_cells),
                fog_cells: self
                    .planar
                    .sample_fog_cells(&self.visible_render.fog_reachable),
                movers: &self.mover_sample_regions,
            },
            Some(self.visible_render.stats.camera_cell),
            self.view_proj,
            self.eye,
            &[],
            &self.forced_promotion_weights,
            animation_time_seconds,
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

    /// Renderer CPU stages for the frame just captured. Its gate is the one
    /// `prepare` read from the environment.
    pub(super) fn cpu_stages(
        &self,
    ) -> &postretro_stage_timing::StageFrame<postretro_renderer::cpu_stages::RenderStage> {
        self.renderer.cpu_stages()
    }

    /// Submit and complete one prepared static sample without PNG readback.
    pub(super) fn capture_measurement_frame(&mut self) -> Result<Option<CaptureGpuTimingWindow>> {
        let animation_time_seconds = self.measurement_animation.advance_frame();
        self.submit_frame_without_readback(true, animation_time_seconds)
    }

    fn submit_frame_without_readback(
        &mut self,
        pin_promotion: bool,
        animation_time_seconds: f32,
    ) -> Result<Option<CaptureGpuTimingWindow>> {
        let sh_drain_batch = self.take_sh_drain_batch()?;
        self.set_mirror_light_reach();
        let forced_promotion_weights: &[(usize, f32)] = if pin_promotion {
            &self.forced_promotion_weights
        } else {
            &[]
        };
        let result = self.renderer.capture_measurement_frame_indirect(
            self.visible_render.camera_cull(),
            &self.visible_render.light_reachable_cell_mask,
            &self.visible_render.reachable_cell_aabbs,
            &self.visible_render.fog_reachable,
            postretro_renderer::ShSampleRegionSets {
                visible_cells: self.planar.drawn_cells(&self.visible_render.visible_cells),
                fog_cells: self
                    .planar
                    .sample_fog_cells(&self.visible_render.fog_reachable),
                movers: &self.mover_sample_regions,
            },
            Some(self.visible_render.stats.camera_cell),
            self.view_proj,
            self.eye,
            &[],
            forced_promotion_weights,
            animation_time_seconds,
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

    pub(super) fn measurement_partial_timing_frames(&self) -> u32 {
        self.renderer.capture_measurement_partial_timing_frames()
    }

    pub(super) fn sh_residency_report(&self) -> Result<Option<ShResidencyReport>> {
        let report = self.renderer.sh_residency_report();
        let Some(streaming) = self.sh_streaming.as_ref() else {
            return Ok(report);
        };
        let lifecycle = streaming.residency_lifecycle_summary()?;
        Ok(report.map(|report| report.with_streaming_lifecycle_summary(lifecycle)))
    }

    pub(super) fn lightmap_residency_report(&self) -> Option<LightmapResidencyReport> {
        self.renderer.lightmap_residency_report().cloned()
    }

    pub(super) const fn lightmap_streaming(&self) -> CaptureLightmapResidency {
        self.lightmap_residency
    }

    pub(super) const fn resolution(&self) -> [u32; 2] {
        self.resolution
    }

    /// The report's planar block; `None` below very high.
    pub(super) fn planar_report(&self) -> Option<CapturePlanarReport> {
        self.planar.report(&self.visible_render.visible_cells)
    }
}

#[cfg(test)]
mod measurement_animation_tests {
    use super::*;
    use crate::capture::scene::{MAX_MEASUREMENT_SAMPLE_FRAMES, MAX_MEASUREMENT_WARMUP_FRAMES};

    #[test]
    fn measurement_frames_advance_animation_without_vm() {
        let mut clock = CaptureMeasurementAnimationClock::default();
        assert_eq!(clock.time_seconds(), 0.0);

        let first = clock.advance_frame();
        let second = clock.advance_frame();

        assert_eq!(first, measurement_animation_time_seconds(1));
        assert_eq!(second, measurement_animation_time_seconds(2));
        assert!(second > first);
    }

    // Regression: repeated f32 additions drifted at the accepted measurement maximum.
    #[test]
    fn maximum_measurement_tick_count_derives_one_stable_time_coordinate() {
        let total =
            u64::from(MAX_MEASUREMENT_WARMUP_FRAMES) + u64::from(MAX_MEASUREMENT_SAMPLE_FRAMES);
        let mut clock = CaptureMeasurementAnimationClock { ticks: total - 1 };

        let actual = clock.advance_frame();
        let expected = (total as f64 / 60.0) as f32;

        assert_eq!(clock.ticks, total);
        assert_eq!(actual, expected);
        assert_eq!(clock.time_seconds(), expected);
    }
}

#[cfg(test)]
mod mirror_light_reach_tests {
    // Regression: capture set the planar mirror's light reach once, and the
    // first world frame's light-slot update consumed it, so every later
    // frame (SH preload, warmup, samples, the PNG) ranked no mirror-only
    // light. Every world-frame entry sets it again before rendering.
    #[test]
    fn capture_world_frames_each_set_the_mirror_light_reach_before_rendering() {
        let source = include_str!("prepared.rs")
            .split("#[cfg(test)]\nmod ")
            .next()
            .expect("production code precedes the test modules");
        for (entry, render) in [
            (
                "fn capture_frame_at(",
                "self.renderer.capture_frame_indirect(",
            ),
            (
                "fn submit_frame_without_readback(",
                "self.renderer.capture_measurement_frame_indirect(",
            ),
        ] {
            assert_eq!(source.matches(render).count(), 1, "{render}");
            let body = source
                .split(entry)
                .nth(1)
                .and_then(|rest| rest.split("\n    }\n").next())
                .unwrap_or_else(|| panic!("{entry} exists"));
            let set = body
                .find("self.set_mirror_light_reach();")
                .unwrap_or_else(|| panic!("{entry} sets the mirror light reach"));
            let rendered = body
                .find(render)
                .unwrap_or_else(|| panic!("{entry} renders through {render}"));
            assert!(set < rendered, "{entry} sets the reach before it renders");
        }
    }
}
