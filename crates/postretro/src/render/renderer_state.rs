// Renderer state accessors and toggles: lighting/SDF isolation modes, freeze,
// vsync, occluder counters, and the diagnostics slider getters/setters.
// See: context/lib/rendering_pipeline.md

use super::*;

impl Renderer {
    /// Direct setter used by the debug-panel dropdown. Logs only on actual
    /// transition so spam-clicks on the current mode stay quiet.
    #[cfg(feature = "dev-tools")]
    pub fn set_lighting_isolation(&mut self, mode: LightingIsolation) {
        if self.full().lighting_isolation != mode {
            self.full_mut().lighting_isolation = mode;
            log::info!("[Renderer] Lighting isolation: {}", mode.label());
        }
    }

    #[cfg(feature = "dev-tools")]
    pub fn lighting_isolation(&self) -> LightingIsolation {
        self.full().lighting_isolation
    }

    /// Direct setter for the `SdfShadowMode`; used by the debug-panel dropdown.
    /// Logs only on transition so spam clicks on the current mode stay quiet.
    #[cfg(feature = "dev-tools")]
    pub fn set_sdf_shadow_mode(&mut self, mode: SdfShadowMode) {
        if self.full().sdf_shadow_mode != mode {
            self.full_mut().sdf_shadow_mode = mode;
            log::info!("[Renderer] SDF shadow mode: {}", mode.label());
        }
    }

    #[cfg(feature = "dev-tools")]
    pub fn sdf_shadow_mode(&self) -> SdfShadowMode {
        self.full().sdf_shadow_mode
    }

    /// Dev toggle (panel checkbox): force per-light SDF visibility to 1.0 so
    /// the forward sdf-tag diffuse term lands unshadowed. The no-double-count
    /// A/B: forced-1.0 must reproduce the pre-change render.
    #[cfg(feature = "dev-tools")]
    pub fn set_sdf_force_visibility_one(&mut self, force: bool) {
        if self.full().sdf_force_visibility_one != force {
            self.full_mut().sdf_force_visibility_one = force;
            log::info!("[Renderer] SDF force visibility 1.0: {force}");
        }
    }

    #[cfg(feature = "dev-tools")]
    pub fn sdf_force_visibility_one(&self) -> bool {
        self.full().sdf_force_visibility_one
    }

    #[cfg(feature = "dev-tools")]
    pub fn freeze_time(&self) -> bool {
        self.full().freeze_time
    }

    /// Pin/unpin `uniforms.time`. Used by the debug panel to freeze all
    /// curve-driven animation while diagnosing time-dependent artifacts.
    #[cfg(feature = "dev-tools")]
    pub fn set_freeze_time(&mut self, freeze: bool) {
        self.full_mut().freeze_time = freeze;
    }

    /// Most recent averaged GPU-timing window, or `None` when GPU timing is
    /// disabled / no window has elapsed yet. The debug panel reads this each
    /// frame; the underlying snapshot is overwritten every
    /// `AVG_WINDOW_FRAMES` frames.
    #[cfg(feature = "dev-tools")]
    pub fn frame_timing_snapshot(&self) -> Option<&frame_timing::FrameTimingSnapshot> {
        self.full()
            .frame_timing
            .as_ref()
            .and_then(|t| t.last_window())
    }

    /// Rebuilds the swapchain via surface.configure (Alt+Shift+V diagnostic chord).
    /// `vsync_enabled` is full-phase state; the surface reconfigure is boot-phase
    /// — destructure so the boot fields and the full flag borrow disjointly.
    pub fn toggle_vsync(&mut self) -> bool {
        let Self {
            device,
            surface,
            surface_config,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("toggle_vsync is a full-ready diagnostic path");
        full.vsync_enabled = !full.vsync_enabled;
        surface_config.present_mode = if full.vsync_enabled {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        surface.configure(device, surface_config);
        full.vsync_enabled
    }

    pub fn vsync_enabled(&self) -> bool {
        self.full().vsync_enabled
    }
}

impl Renderer {
    /// Count of skinned entity occluder instances submitted into spot shadow
    /// slots last frame (summed across slots). The CPU-side verification for the
    /// out-of-cone acceptance criterion — an instance the per-light cone cull
    /// rejects is never tallied here. No GPU readback.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn spot_entity_occluders_submitted(&self) -> u32 {
        self.full().spot_entity_occluders_submitted
    }

    /// Count of skinned entity occluder instances submitted into CUBE point-light
    /// shadow faces last frame (summed across occupied slots × 6 faces). The
    /// CPU-side verification that entity occluders render only for eligible point
    /// lights and only inside a face frustum. No GPU readback.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn cube_entity_occluders_submitted(&self) -> u32 {
        self.full().cube_entity_occluders_submitted
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn ambient_floor(&self) -> f32 {
        self.full().ambient_floor
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_ambient_floor(&mut self, value: f32) {
        self.full_mut().ambient_floor = value.clamp(0.0, 1.0);
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn indirect_scale(&self) -> f32 {
        self.full().indirect_scale
    }

    /// Takes effect on the next `update_per_frame_uniforms` upload.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_indirect_scale(&mut self, value: f32) {
        self.full_mut().indirect_scale = value.clamp(0.0, 1.0);
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn dynamic_direct_scale(&self) -> f32 {
        self.full().dynamic_direct_scale
    }

    /// Takes effect on the next `update_per_frame_uniforms` upload.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_dynamic_direct_scale(&mut self, value: f32) {
        self.full_mut().dynamic_direct_scale = value.clamp(0.0, 1.0);
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn dynamic_direct_isolation(&self) -> DynamicDirectIsolation {
        self.full().dynamic_direct_isolation
    }

    /// Takes effect on the next `update_per_frame_uniforms` upload.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_dynamic_direct_isolation(&mut self, mode: DynamicDirectIsolation) {
        self.full_mut().dynamic_direct_isolation = mode;
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn probe_occlusion_enabled(&self) -> bool {
        self.full().probe_occlusion_enabled
    }

    /// Takes effect immediately for the SH grid uniform and persists across
    /// level reloads because `install_level_geometry` seeds rebuilt resources
    /// from this renderer state.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_probe_occlusion_enabled(&mut self, enabled: bool) {
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("set_probe_occlusion_enabled is a full-ready path");
        if full.probe_occlusion_enabled != enabled {
            full.probe_occlusion_enabled = enabled;
            full.sh_volume_resources
                .set_probe_occlusion_enabled(queue, enabled);
            log::info!("[Renderer] Probe Occlusion: {enabled}");
        }
    }

    // --- Task 7: SDF / Fog quality-slider seams ---
    //
    // The SDF knobs live on `SdfShadowPass.tuning` — pure uniform scalars
    // packed each frame in `pack_params_bytes` (no resource rebuild). The fog
    // knobs split: `step_size` is a per-frame uniform repacked in
    // `upload_params`; `fog_pixel_scale` is a resource-rebuild knob already
    // owned by `set_fog_pixel_scale` above.

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn sdf_max_march_steps(&self) -> u32 {
        self.full().sdf_shadow_pass.tuning().max_march_steps
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_sdf_max_march_steps(&mut self, steps: u32) {
        self.full_mut().sdf_shadow_pass.set_max_march_steps(steps);
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn sdf_open_space_skip_threshold(&self) -> f32 {
        self.full()
            .sdf_shadow_pass
            .tuning()
            .open_space_skip_threshold
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_sdf_open_space_skip_threshold(&mut self, threshold: f32) {
        self.full_mut()
            .sdf_shadow_pass
            .set_open_space_skip_threshold(threshold);
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn sdf_penumbra_k(&self) -> f32 {
        self.full().sdf_shadow_pass.tuning().penumbra_k
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_sdf_penumbra_k(&mut self, k: f32) {
        self.full_mut().sdf_shadow_pass.set_penumbra_k(k);
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn sdf_surface_bias(&self) -> f32 {
        self.full().sdf_shadow_pass.tuning().surface_bias
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_sdf_surface_bias(&mut self, bias: f32) {
        self.full_mut().sdf_shadow_pass.set_surface_bias(bias);
    }

    /// Current per-frame fog raymarch step size (world units). Read by the
    /// debug-UI slider on first draw so it shows the live value rather than
    /// the construction default.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn fog_step_size(&self) -> f32 {
        self.full().fog.step_size
    }

    /// Update the fog raymarch step size in place. `FogPass.step_size` is
    /// read by `upload_params` on the next frame, so this is a pure uniform
    /// write — no resource rebuild. Clamped to a positive minimum to guard
    /// against a runaway slider stalling the raymarch.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_fog_step_size(&mut self, step_size: f32) {
        self.full_mut().fog.step_size = step_size.max(0.01);
    }

    /// Current `fog_pixel_scale` — read by the debug-UI slider on first draw.
    /// The setter (`set_fog_pixel_scale` above) drives a scatter-target
    /// rebuild rather than a per-frame uniform write.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn fog_pixel_scale(&self) -> u32 {
        self.full().fog.pixel_scale
    }

    /// Boot-ready: the surface/device/queue/boot-splash exist and the surface is
    /// configured. The boot splash path requires only this. True immediately
    /// after `Renderer::new`, and re-true after a resize reconfigures the surface.
    pub fn is_boot_ready(&self) -> bool {
        self.is_surface_configured
    }

    /// Full-ready: the full renderer (pipelines, passes, resources) is built.
    /// Required by Frontend, Loading completion, Running, the UI pass, and the
    /// scene render path. Becomes true after `finish_full_init` / `ensure_full_ready`.
    pub fn is_full_ready(&self) -> bool {
        self.full.is_some()
    }

    #[allow(dead_code)]
    pub fn has_compute_cull(&self) -> bool {
        self.full().compute_cull.is_some()
    }
}
