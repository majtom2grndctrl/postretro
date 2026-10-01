// Surface and scene extent derivation: the render-resolution chokepoint.
// See: context/lib/rendering_pipeline.md §7.8

/// A non-zero 2D pixel extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    pub width: u32,
    pub height: u32,
}

impl Extent {
    /// Clamp each axis to at least one pixel.
    pub const fn new(width: u32, height: u32) -> Self {
        Self {
            width: if width == 0 { 1 } else { width },
            height: if height == 0 { 1 } else { height },
        }
    }

    pub fn aspect(self) -> f32 {
        self.width as f32 / self.height as f32
    }
}

/// Renderer vocabulary for the player's render resolution. The player-options
/// policy (which divisor, and Auto's row cap) arrives already translated, so the
/// renderer holds no option names and no cap of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderResolutionPolicy {
    /// Divisor = max(1, floor(scale factor), ceil(surface rows / max_scene_rows)).
    Auto { max_scene_rows: u32 },
    /// A fixed integer divisor. 1 is native.
    Fixed { divisor: u32 },
}

impl Default for RenderResolutionPolicy {
    /// Native until player options arrive. Capture keeps this forever.
    fn default() -> Self {
        Self::Fixed { divisor: 1 }
    }
}

/// The integer divisor a policy selects for a surface at a scale factor.
pub fn resolve_divisor(policy: RenderResolutionPolicy, scale_factor: f64, surface: Extent) -> u32 {
    match policy {
        RenderResolutionPolicy::Fixed { divisor } => divisor.max(1),
        RenderResolutionPolicy::Auto { max_scene_rows } => {
            // A non-finite or sub-1 scale factor floors to at most 0 and falls
            // through to the max(1, …) below.
            let scale_divisor = if scale_factor.is_finite() && scale_factor >= 1.0 {
                scale_factor.floor() as u32
            } else {
                0
            };
            let row_divisor = surface.height.div_ceil(max_scene_rows.max(1));
            scale_divisor.max(row_divisor).max(1)
        }
    }
}

/// ceil(surface / divisor) per axis, never below 1×1. A divisor larger than the
/// surface therefore clamps to a one-pixel axis.
pub fn scene_extent(surface: Extent, divisor: u32) -> Extent {
    let divisor = divisor.max(1);
    Extent::new(
        surface.width.div_ceil(divisor),
        surface.height.div_ceil(divisor),
    )
}

/// Both extents a frame renders at, plus the divisor relating them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderExtents {
    /// Swapchain, resolve output and UI layer.
    pub surface: Extent,
    /// Every scene target, the camera and the viewmodel.
    pub scene: Extent,
    pub divisor: u32,
}

impl RenderExtents {
    pub fn derive(surface: Extent, scale_factor: f64, policy: RenderResolutionPolicy) -> Self {
        let divisor = resolve_divisor(policy, scale_factor, surface);
        Self {
            surface,
            scene: scene_extent(surface, divisor),
            divisor,
        }
    }

    /// `scene * divisor` per axis: the surface-pixel span the upscaled scene
    /// image covers. Never smaller than the surface; the resolve anchors it at
    /// the top-left and crops the right/bottom overshoot. World-anchored UI
    /// must project into this viewport so an anchor lands on the scene pixel it
    /// marks.
    pub fn upscaled_scene(self) -> Extent {
        Extent::new(
            self.scene.width.saturating_mul(self.divisor),
            self.scene.height.saturating_mul(self.divisor),
        )
    }
}

/// What changed between the last committed extents and the new ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtentChange {
    pub extents: RenderExtents,
    pub surface_changed: bool,
    pub scene_changed: bool,
}

/// Window events and option changes only record here; [`ExtentState::commit`]
/// derives once from the final values at the next frame start. Auto keeps no
/// history: every commit derives from the recorded inputs alone.
#[derive(Debug, Clone)]
pub struct ExtentState {
    /// Last non-zero surface. A minimized (0×0) window never reaches it.
    surface: Extent,
    scale_factor: f64,
    policy: RenderResolutionPolicy,
    committed: RenderExtents,
}

impl ExtentState {
    /// Seed from the window as it is at renderer build. macOS sends no
    /// scale-factor event at launch, so the window's current scale must arrive
    /// here rather than only from events.
    pub fn new(surface: Extent, scale_factor: f64, policy: RenderResolutionPolicy) -> Self {
        Self {
            surface,
            scale_factor,
            policy,
            committed: RenderExtents::derive(surface, scale_factor, policy),
        }
    }

    /// Record a surface resize. A zero dimension (minimize) is ignored, so the
    /// extents stay at the last non-zero surface.
    pub fn record_surface(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface = Extent { width, height };
    }

    pub fn record_scale_factor(&mut self, scale_factor: f64) {
        self.scale_factor = scale_factor;
    }

    pub fn record_policy(&mut self, policy: RenderResolutionPolicy) {
        self.policy = policy;
    }

    /// Derive from the recorded values. `Some` only when either extent differs
    /// from the last commit — the caller rebuilds exactly then.
    pub fn commit(&mut self) -> Option<ExtentChange> {
        let next = RenderExtents::derive(self.surface, self.scale_factor, self.policy);
        let surface_changed = next.surface != self.committed.surface;
        let scene_changed = next.scene != self.committed.scene;
        self.committed = next;
        (surface_changed || scene_changed).then_some(ExtentChange {
            extents: next,
            surface_changed,
            scene_changed,
        })
    }

    /// The extents targets are currently built at.
    pub fn committed(&self) -> RenderExtents {
        self.committed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAP: RenderResolutionPolicy = RenderResolutionPolicy::Auto {
        max_scene_rows: 1440,
    };

    fn ext(width: u32, height: u32) -> Extent {
        Extent { width, height }
    }

    #[test]
    fn scene_extent_rounds_up_per_axis_for_odd_and_even_surfaces() {
        assert_eq!(scene_extent(ext(2560, 1440), 2), ext(1280, 720));
        assert_eq!(scene_extent(ext(2561, 1441), 2), ext(1281, 721));
        assert_eq!(scene_extent(ext(1001, 999), 3), ext(334, 333));
        assert_eq!(scene_extent(ext(1920, 1080), 1), ext(1920, 1080));
    }

    #[test]
    fn upscaled_scene_spans_at_least_the_surface() {
        let odd = RenderExtents::derive(
            ext(1001, 563),
            1.0,
            RenderResolutionPolicy::Fixed { divisor: 4 },
        );
        assert_eq!(odd.scene, ext(251, 141));
        assert_eq!(odd.upscaled_scene(), ext(1004, 564));

        let native = RenderExtents::derive(ext(1001, 563), 1.0, RenderResolutionPolicy::default());
        assert_eq!(native.upscaled_scene(), native.surface);
    }

    #[test]
    fn scene_extent_clamps_an_oversize_divisor_to_one_pixel() {
        assert_eq!(scene_extent(ext(3, 2), 4), ext(1, 1));
        assert_eq!(scene_extent(ext(1, 1), 4), ext(1, 1));
        assert_eq!(scene_extent(ext(640, 2), 3), ext(214, 1));
    }

    #[test]
    fn a_zero_divisor_is_treated_as_native() {
        assert_eq!(scene_extent(ext(640, 480), 0), ext(640, 480));
        assert_eq!(
            resolve_divisor(
                RenderResolutionPolicy::Fixed { divisor: 0 },
                1.0,
                ext(640, 480)
            ),
            1
        );
    }

    #[test]
    fn auto_ignores_a_degenerate_scale_factor_and_cap() {
        assert_eq!(resolve_divisor(CAP, f64::NAN, ext(1920, 1080)), 1);
        assert_eq!(resolve_divisor(CAP, -2.0, ext(1920, 1080)), 1);
        assert_eq!(
            resolve_divisor(
                RenderResolutionPolicy::Auto { max_scene_rows: 0 },
                1.0,
                ext(4, 4)
            ),
            4
        );
    }

    // P1: a 2× window with no scale-factor event after it opens.
    #[test]
    fn a_state_seeded_from_a_2x_window_starts_at_half_the_surface() {
        let state = ExtentState::new(ext(2560, 1440), 2.0, CAP);
        assert_eq!(state.committed().scene, ext(1280, 720));
        assert_eq!(state.committed().surface, ext(2560, 1440));
    }

    // P2: a saved non-Auto option recorded before full init builds there, and
    // full init's own commit then finds nothing to rebuild.
    #[test]
    fn a_policy_recorded_before_the_first_commit_needs_no_later_rebuild() {
        let mut state = ExtentState::new(ext(2560, 1440), 2.0, RenderResolutionPolicy::default());
        state.record_policy(RenderResolutionPolicy::Fixed { divisor: 3 });
        let change = state.commit().expect("policy changed the scene extent");
        assert_eq!(change.extents.scene, ext(854, 480));
        assert!(!change.surface_changed);
        assert_eq!(state.commit(), None);
    }

    // P3, P4: scale and resize in either order commit once from final values.
    #[test]
    fn scale_then_resize_and_resize_then_scale_commit_once_from_final_values() {
        for scale_first in [true, false] {
            let mut state = ExtentState::new(ext(1280, 720), 1.0, CAP);
            if scale_first {
                state.record_scale_factor(2.0);
                state.record_surface(2560, 1440);
            } else {
                state.record_surface(2560, 1440);
                state.record_scale_factor(2.0);
            }
            let change = state.commit().expect("both extents changed");
            assert_eq!(change.extents.surface, ext(2560, 1440));
            assert_eq!(change.extents.scene, ext(1280, 720));
            assert!(change.surface_changed);
            assert!(
                !change.scene_changed,
                "1280x720 at 1x and 2x is the same scene"
            );
            assert_eq!(state.commit(), None, "exactly one rebuild");
        }
    }

    // P5: a scale-factor change with no resize.
    #[test]
    fn a_scale_change_without_resize_rebuilds_only_when_the_divisor_changes() {
        let mut state = ExtentState::new(ext(2560, 1440), 1.0, CAP);
        state.record_scale_factor(2.0);
        let change = state.commit().expect("divisor 1 -> 2");
        assert!(change.scene_changed && !change.surface_changed);
        assert_eq!(change.extents.scene, ext(1280, 720));

        state.record_scale_factor(2.5);
        assert_eq!(state.commit(), None, "floor(2.5) keeps divisor 2");
    }

    // P6: resize then an option change in the same frame.
    #[test]
    fn a_resize_and_an_option_change_in_one_frame_commit_once() {
        let mut state = ExtentState::new(ext(1280, 720), 1.0, CAP);
        state.record_surface(1920, 1080);
        state.record_policy(RenderResolutionPolicy::Fixed { divisor: 2 });
        let change = state.commit().expect("both changed");
        assert_eq!(change.extents.surface, ext(1920, 1080));
        assert_eq!(change.extents.scene, ext(960, 540));
        assert_eq!(state.commit(), None);
    }

    // P7: minimize and restore.
    #[test]
    fn a_zero_resize_keeps_the_last_non_zero_surface_and_builds_nothing() {
        let mut state = ExtentState::new(ext(1920, 1080), 1.0, CAP);
        state.record_surface(0, 0);
        assert_eq!(state.commit(), None);
        state.record_surface(0, 1080);
        assert_eq!(state.commit(), None);
        assert_eq!(state.committed().surface, ext(1920, 1080));
        state.record_surface(1920, 1080);
        assert_eq!(state.commit(), None, "restored to the same size");
    }

    // P8: an option change while minimized.
    #[test]
    fn an_option_change_while_minimized_derives_from_the_last_non_zero_surface() {
        let mut state = ExtentState::new(ext(1920, 1080), 1.0, CAP);
        state.record_surface(0, 0);
        state.record_policy(RenderResolutionPolicy::Fixed { divisor: 4 });
        let change = state.commit().expect("scene changed");
        assert_eq!(change.extents.scene, ext(480, 270), "never 1x1");
        state.record_surface(1920, 1080);
        assert_eq!(state.commit(), None);
    }

    // P9: Auto at the 1440-row boundary keeps no history.
    #[test]
    fn auto_crossing_the_row_cap_and_back_rebuilds_once_per_step() {
        let mut state = ExtentState::new(ext(2560, 1440), 1.0, CAP);
        assert_eq!(state.committed().divisor, 1);
        state.record_surface(2560, 1441);
        let up = state.commit().expect("divisor 2");
        assert_eq!((up.extents.divisor, up.extents.scene.height), (2, 721));
        state.record_surface(2560, 1440);
        let down = state.commit().expect("divisor 1");
        assert_eq!((down.extents.divisor, down.extents.scene.height), (1, 1440));
        assert_eq!(state.commit(), None);
    }

    // P16: a rebuilt renderer re-reads the scale and re-applies the option.
    #[test]
    fn a_fresh_state_from_reread_inputs_restores_the_pre_suspend_extents() {
        let mut before = ExtentState::new(ext(2560, 1600), 2.0, RenderResolutionPolicy::default());
        before.record_policy(CAP);
        before.commit();
        let mut after = ExtentState::new(ext(2560, 1600), 2.0, RenderResolutionPolicy::default());
        after.record_policy(CAP);
        after.commit();
        assert_eq!(after.committed(), before.committed());
    }
}
