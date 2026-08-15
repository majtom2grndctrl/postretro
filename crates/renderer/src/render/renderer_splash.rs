// Renderer boot-splash API + UI-state methods. The splash half is the small
// app-facing surface over the renderer-owned `BootSplashPass` (install pixels,
// render a black/logo frame, clear). The UI-state methods (snapshot, theme,
// fonts, focus-rect export) back the gameplay/frontend UI and are unrelated to
// the boot splash.
// See: context/lib/boot_sequence.md §1 · context/lib/ui.md

use super::*;

impl Renderer {
    /// Present an acquired frame handle. Surface ownership stays inside the
    /// renderer; callers only decide whether to present a returned handle.
    pub fn present(&self, handle: PresentHandle) {
        handle.present();
    }

    /// Upload the decoded boot-splash logo into the boot splash pass and build
    /// its bind group. The app decodes the PNG on the boot thread and hands the
    /// pixels here — the renderer owns all GPU work. Idempotent: a re-install
    /// (e.g. on resume) swaps the texture. Returns the decoded pixel dimensions
    /// for boot logging.
    pub fn install_splash_pixels(&mut self, loaded: &postretro_ui::UiTexture) -> [u32; 2] {
        self.boot_splash
            .as_mut()
            .expect("splash pixels require a windowed renderer")
            .install_logo(&self.device, &self.queue, loaded)
    }

    /// Render one boot-splash frame to the swapchain: clear to black, then draw
    /// the logo quad when one is installed. Returns a present handle once a
    /// command buffer is submitted; a transient or recoverable surface failure
    /// returns `Ok(None)` so startup re-requests a redraw WITHOUT advancing its
    /// splash schedule or recording first-frame timings.
    ///
    /// The boot splash writes the swapchain directly — it never touches
    /// `scene_color`, the UI pass, or `UiReadSnapshot` (rendering_pipeline §7.8).
    pub fn render_splash_frame(&mut self) -> Result<Option<PresentHandle>> {
        let Some(handle) = self.acquire_present_handle("splash frame")? else {
            return Ok(None);
        };

        let view = handle.surface_view();

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Splash Frame Encoder"),
            });

        let viewport = [self.surface_config.width, self.surface_config.height];
        self.boot_splash
            .as_ref()
            .expect("splash rendering requires a windowed renderer")
            .encode(&self.queue, &mut encoder, &view, viewport);

        self.queue.submit(std::iter::once(encoder.finish()));
        Ok(Some(handle))
    }

    /// Drop the uploaded boot-splash logo so post-handoff frames record nothing.
    /// Called on the boot→content transition and on suspend.
    pub fn clear_splash(&mut self) {
        if let Some(boot_splash) = self.boot_splash.as_mut() {
            boot_splash.clear();
        }
    }

    /// Store the once-per-frame read snapshot. The App calls this just before each
    /// gameplay/frontend render call; the UI pass reads it when it records. Keeps
    /// the render signature stable. The boot splash does NOT use this.
    pub fn set_ui_snapshot(&mut self, snapshot: postretro_ui::UiReadSnapshot) {
        self.full_mut().ui_snapshot = snapshot;
    }

    /// Store one frame of app-produced passive presentation draw data. It is
    /// folded with retained gameplay UI during the render pass, but never enters
    /// the focus or hit-test export path.
    pub fn set_presentation_draw_data(&mut self, draw: postretro_ui::tree::UiDrawData) {
        self.full_mut().presentation_draw = draw;
    }

    /// Export the flat hit-test / focus rect list for the TOP gameplay-UI stack
    /// layer against the current surface viewport — the reverse twin of the
    /// app→renderer snapshot. The App reads this after a gameplay render (which
    /// laid out the stack) and feeds it to the focus engine the NEXT frame
    /// (N→N+1 in reverse). Empty when no gameplay layer is active. See: ui.md §4.
    pub fn export_ui_focus_rects(&self) -> postretro_ui::tree::FocusRectList {
        let Self {
            surface_config,
            full,
            ..
        } = self;
        let full = full
            .as_ref()
            .expect("renderer full-init must complete before full-ready paths run");
        let viewport = [surface_config.width, surface_config.height];
        // Resolve each focusable button's `selected`/`checked` predicate (M13 G2)
        // against the same frame snapshot the draw build used, so the a11y readback
        // matches the author-wired highlight.
        full.ui.export_top_focus_rects(
            viewport,
            &full.ui_snapshot.slot_values,
            &full.ui_snapshot.cell_values,
        )
    }

    /// Install an override UI theme and bump the theme generation. Engine-side
    /// only (no script bridge): a caller hands a fully-merged `UiTheme` (e.g.
    /// `UiTheme::engine_default().with_override(&doc)`), which every subsequent
    /// descriptor build resolves its tokens against. Bumping the generation
    /// invalidates the retained gameplay tree's baked tokens, so the next gameplay
    /// frame rebuilds the tree with the new values even when its descriptor is
    /// unchanged.
    //
    // The production caller is the G1b mod-init drain (`main.rs`): it merges a
    // mod's `theme` tokens over `engine_default` and installs the result here.
    // `Renderer` needs a GPU device, so this seam is exercised by running the
    // engine, not the CPU test suite; the merge it relies on is covered in
    // `theme.rs`.
    pub fn set_ui_theme(&mut self, theme: postretro_ui::theme::UiTheme) {
        let full = self.full_mut();
        full.ui_theme = theme;
        full.ui_theme_generation = full.ui_theme_generation.wrapping_add(1);
    }

    /// Install a runtime UI font face from owned TTF/OTF bytes into the
    /// session-owned `FontSystem` (the net-new runtime path behind
    /// `UiPass`/glyphon; the engine's primary/mono faces are embedded at compile
    /// time by `postretro_ui::text::build_font_system`). Returns `false` when the
    /// bytes register no face under `family` (a malformed file or a family-name
    /// mismatch), so the caller surfaces a named diagnostic and skips rather than
    /// leaving a `font` token silently resolving to a system fallback.
    pub fn register_ui_font(
        &mut self,
        font_system: &mut postretro_ui::text::FontSystem,
        family: &str,
        ttf_bytes: Vec<u8>,
    ) -> bool {
        self.full_mut()
            .ui
            .register_font(font_system, family, ttf_bytes)
    }
}
