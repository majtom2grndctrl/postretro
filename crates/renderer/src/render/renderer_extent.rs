// Surface and scene extents: record-only window/option setters, the once-per-frame
// commit, and the scene-target rebuild it drives.
// See: context/lib/rendering_pipeline.md §7.8

use super::*;
use postretro_render_cpu::render_extent::{
    Extent, ExtentChange, RenderExtents, RenderResolutionPolicy,
};

impl Renderer {
    /// Record a window resize. Nothing rebuilds until [`Renderer::commit_extents`];
    /// a zero dimension (minimize) is ignored.
    pub fn record_surface_size(&mut self, width: u32, height: u32) {
        self.extent_state.record_surface(width, height);
    }

    /// Record the window's scale factor (a `ScaleFactorChanged` event).
    pub fn record_scale_factor(&mut self, scale_factor: f64) {
        self.extent_state.record_scale_factor(scale_factor);
    }

    /// Record the player's render resolution, already translated at the app's
    /// render-profile chokepoint. Safe in both phases: full init builds at the
    /// recorded value, so a saved option needs no rebuild after it.
    pub fn set_render_resolution(&mut self, policy: RenderResolutionPolicy) {
        self.extent_state.record_policy(policy);
    }

    /// The extents every target is currently built at.
    pub fn render_extents(&self) -> RenderExtents {
        self.extent_state.committed()
    }

    /// The scene extent: camera and viewmodel aspect derive from it.
    pub fn scene_extent(&self) -> Extent {
        self.extent_state.committed().scene
    }

    /// Derive both extents from the recorded values and rebuild once if either
    /// changed. The binary calls this after the frame's option writes and before
    /// it builds the camera; render entries call it again, which is a no-op
    /// unless something was recorded since.
    pub fn commit_extents(&mut self) -> Option<ExtentChange> {
        let change = self.extent_state.commit()?;
        self.apply_extent_change(change);
        Some(change)
    }

    /// Capture renders at the offscreen renderer's surface extent (whatever it
    /// was built or resized to) at divisor 1, whatever render resolution was
    /// recorded, so captures stay comparable across machines. It pins the
    /// policy, so it is offscreen-only: on a windowed renderer it would replace
    /// the player's recorded render resolution.
    pub(super) fn commit_native_capture_extents(&mut self) {
        debug_assert!(
            self.surface.is_none(),
            "capture pins native extents only on an offscreen renderer"
        );
        self.extent_state
            .record_policy(RenderResolutionPolicy::default());
        self.commit_extents();
    }

    fn apply_extent_change(&mut self, change: ExtentChange) {
        let RenderExtents { surface, scene, .. } = change.extents;
        if change.surface_changed {
            self.surface_config.width = surface.width;
            self.surface_config.height = surface.height;
            if let Some(surface) = self.surface.as_ref() {
                surface.configure(&self.device, &self.surface_config);
                self.is_surface_configured = true;
                self.surface_reconfigure_pending = false;
            }
        }
        // Boot phase: the splash re-projects against the new backbuffer, and
        // full init builds every target at the committed extents.
        if self.full.is_none() {
            return;
        }
        if change.surface_changed {
            let Self { device, full, .. } = self;
            let full = full.as_mut().expect("checked above");
            full.screen_effects.resize_ui_layer(device, surface);
        }
        if change.scene_changed {
            self.rebuild_scene_targets(scene);
        }
    }

    /// Recreate every scene-sized target at `scene`. No scene target reads the
    /// surface size: this is their only resize path.
    fn rebuild_scene_targets(&mut self, scene: Extent) {
        let Self { device, full, .. } = self;
        let full = full
            .as_mut()
            .expect("scene targets rebuild only on a full renderer");
        let Extent { width, height } = scene;
        let (_depth_texture, depth_view) = create_depth_texture(device, width, height);
        full.depth_view = depth_view;
        // Recreate the HDR scene target before bloom so bloom can rebuild its
        // source view and resolution-dependent parameter table.
        full.screen_effects.resize(device, scene);
        full.bloom.resize(
            device,
            width,
            height,
            full.screen_effects.scene_color_texture(),
        );
        full.fog.resize(device, width, height, &full.depth_view);
        // The SDF shadow target is half the scene; the depth view also changed,
        // so its pass bind group is rebuilt.
        full.sdf_shadow_pass
            .resize(device, &full.depth_view, width, height);
        // Group 5 references the SDF factor and scene depth, both just
        // recreated. The cube binding's presence is fixed for the renderer's
        // lifetime (`Some` pool iff CUBE_ARRAY_TEXTURES), so the BGL is rebuilt
        // with the same flag.
        let cube_array_supported = full.cube_shadow_pool.is_some();
        let spot_shadow_bgl = SpotShadowPool::bind_group_layout(device, cube_array_supported);
        let cube_sampling_view = full.cube_shadow_pool.as_ref().map(|p| &p.sampling_view);
        full.spot_shadow_pool.rebuild_bind_group(
            device,
            &spot_shadow_bgl,
            &full.sdf_shadow_pass.shadow_view,
            &full.depth_view,
            cube_sampling_view,
        );
    }
}
