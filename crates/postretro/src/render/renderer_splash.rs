// Renderer boot-splash API + UI-state methods. The splash half is the small
// app-facing surface over the renderer-owned `BootSplashPass` (install pixels,
// render a black/logo frame, clear). The UI-state methods (snapshot, theme,
// fonts, focus-rect export) back the gameplay/frontend UI and are unrelated to
// the boot splash.
// See: context/lib/boot_sequence.md §1 · context/lib/ui.md

use super::*;

use crate::render::splash_pass::PresentOutcome;

impl Renderer {
    /// Upload the decoded boot-splash logo into the boot splash pass and build
    /// its bind group. The app decodes the PNG on the boot thread and hands the
    /// pixels here — the renderer owns all GPU work. Idempotent: a re-install
    /// (e.g. on resume) swaps the texture. Returns the decoded pixel dimensions
    /// for boot logging.
    pub fn install_splash_pixels(&mut self, loaded: &crate::ui_texture::UiTexture) -> [u32; 2] {
        self.boot_splash
            .install_logo(&self.device, &self.queue, loaded)
    }

    /// Render one boot-splash frame to the swapchain: clear to black, then draw
    /// the logo quad when one is installed. Returns `Presented` once a command
    /// buffer is submitted and the surface texture presents; a transient surface
    /// failure returns `NeedsRedraw` so startup re-requests a redraw WITHOUT
    /// advancing its splash schedule or recording first-frame timings.
    ///
    /// The boot splash writes the swapchain directly — it never touches
    /// `scene_color`, the UI pass, or `UiReadSnapshot` (rendering_pipeline §7.8).
    pub fn render_splash_frame(&mut self) -> PresentOutcome {
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex) => tex,
            wgpu::CurrentSurfaceTexture::Suboptimal(tex) => {
                self.surface.configure(&self.device, &self.surface_config);
                tex
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return PresentOutcome::NeedsRedraw;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                // Reconfigure and ask for another redraw without advancing the
                // splash state — the frame never presented.
                self.surface.configure(&self.device, &self.surface_config);
                return PresentOutcome::NeedsRedraw;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                log::warn!("[Renderer] surface validation error during splash; requesting redraw");
                return PresentOutcome::NeedsRedraw;
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Splash Frame Encoder"),
            });

        let viewport = [self.surface_config.width, self.surface_config.height];
        self.boot_splash
            .encode(&self.queue, &mut encoder, &view, viewport);

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        PresentOutcome::Presented
    }

    /// Drop the uploaded boot-splash logo so post-handoff frames record nothing.
    /// Called on the boot→content transition and on suspend.
    pub fn clear_splash(&mut self) {
        self.boot_splash.clear();
    }

    /// Store the once-per-frame read snapshot. The App calls this just before each
    /// gameplay/frontend render call; the UI pass reads it when it records. Keeps
    /// the render signature stable. The boot splash does NOT use this.
    pub fn set_ui_snapshot(&mut self, snapshot: ui::UiReadSnapshot) {
        self.full_mut().ui_snapshot = snapshot;
    }

    /// Export the flat hit-test / focus rect list for the TOP gameplay-UI stack
    /// layer against the current surface viewport — the reverse twin of the
    /// app→renderer snapshot. The App reads this after a gameplay render (which
    /// laid out the stack) and feeds it to the focus engine the NEXT frame
    /// (N→N+1 in reverse). Empty when no gameplay layer is active. See: ui.md §4.
    pub fn export_ui_focus_rects(&self) -> ui::tree::FocusRectList {
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
    pub fn set_ui_theme(&mut self, theme: ui::theme::UiTheme) {
        let full = self.full_mut();
        full.ui_theme = theme;
        full.ui_theme_generation = full.ui_theme_generation.wrapping_add(1);
    }

    /// Install a runtime UI font face from owned TTF/OTF bytes (the net-new
    /// runtime path behind `UiPass`/glyphon's `FontSystem`; the engine's primary/mono
    /// faces are embedded at compile time). Renderer-owns-GPU: the glyphon
    /// `FontSystem` lives in the renderer, so the mod-init drain in `main.rs` reads
    /// the TTF bytes itself and hands them here. Returns `false` when the bytes
    /// register no face under `family` (a malformed file or a family-name
    /// mismatch), so the caller surfaces a named diagnostic and skips rather than
    /// leaving a `font` token silently resolving to a system fallback.
    pub fn register_ui_font(&mut self, family: &str, ttf_bytes: Vec<u8>) -> bool {
        self.full_mut().ui.register_font(family, ttf_bytes)
    }
}
