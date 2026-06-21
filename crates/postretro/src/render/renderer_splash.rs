// Renderer splash + UI-state methods: splash install/render, UI snapshot,
// theme, fonts, and focus-rect export.
// See: context/lib/ui.md

use super::*;

impl Renderer {
    /// Install the active splash: upload the logo (reusing the splash texture
    /// upload), build its UI bind group, and install the logo so the JSON-loaded
    /// splash descriptor records through the UI pass in `render_splash_frame`.
    /// May be called more than once (mod-override swap in splash frame 1).
    pub fn install_splash_from_loaded(
        &mut self,
        loaded: &crate::ui_texture::UiTexture,
    ) -> [u32; 2] {
        // Force the splash tree's one-time JSON load + parse now, at install
        // (early in boot), rather than lazily on the first splash frame's render.
        ui::splash::force_splash_tree_init();
        let (texture, dims) = splash::upload_splash_texture(&self.device, &self.queue, loaded);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.ui.make_texture_bind_group(&self.device, &view);
        // Register the logo under the splash's known asset key, so the splash
        // descriptor's `image` node resolves to this bind group through the
        // registry (only known keys are pre-registered).
        self.ui_images
            .register(ui::splash::SPLASH_LOGO_ASSET, texture, bind_group);
        // Shape the logo to the decoded image so it never stretches: its natural
        // reference size flows from the real pixel dims (content-driven via the
        // measure seam), not a hardcoded constant.
        self.splash_logo_size = Some(ui::splash::splash_logo_reference_size(dims));
        dims
    }

    /// The active splash's capture/passthrough mode, for the App to drive the
    /// input-dispatch seam (`UiDispatch::set_mode`). `None` when no splash is
    /// installed. The splash is non-interactive, so this reports `Passthrough`.
    pub fn splash_capture_mode(&self) -> Option<crate::input::UiCaptureMode> {
        self.splash_logo_size
            .map(|_| ui::splash::splash_capture_mode())
    }

    /// Store the once-per-frame read snapshot. The App calls this just before each
    /// render call (splash phase and gameplay path); the UI pass reads it when it
    /// records. Keeps both render signatures stable.
    pub fn set_ui_snapshot(&mut self, snapshot: ui::UiReadSnapshot) {
        self.ui_snapshot = snapshot;
    }

    /// Export the flat hit-test / focus rect list for the TOP gameplay-UI stack
    /// layer against the current surface viewport — the reverse twin of the
    /// app→renderer snapshot. The App reads this after a gameplay render (which
    /// laid out the stack) and feeds it to the focus engine the NEXT frame
    /// (N→N+1 in reverse). Empty when no gameplay layer is active. See: ui.md §4.
    pub fn export_ui_focus_rects(&self) -> ui::tree::FocusRectList {
        let viewport = [self.surface_config.width, self.surface_config.height];
        // Resolve each focusable button's `selected`/`checked` predicate (M13 G2)
        // against the same frame snapshot the draw build used, so the a11y readback
        // matches the author-wired highlight.
        self.ui.export_top_focus_rects(
            viewport,
            &self.ui_snapshot.slot_values,
            &self.ui_snapshot.cell_values,
        )
    }

    /// Install an override UI theme and bump the theme generation. Engine-side
    /// only (no script bridge): a caller hands a fully-merged `UiTheme` (e.g.
    /// `UiTheme::engine_default().with_override(&doc)`), which every subsequent
    /// descriptor build resolves its tokens against. Bumping the generation
    /// invalidates the retained gameplay tree's baked tokens, so the next gameplay
    /// frame rebuilds the tree with the new values even when its descriptor is
    /// unchanged. The splash re-derives its tree each frame, so it picks up the
    /// new theme on its next frame with no extra bookkeeping.
    //
    // The production caller is the G1b mod-init drain (`main.rs`): it merges a
    // mod's `theme` tokens over `engine_default` and installs the result here.
    // `Renderer` needs a GPU device, so this seam is exercised by running the
    // engine, not the CPU test suite; the merge it relies on is covered in
    // `theme.rs`.
    pub fn set_ui_theme(&mut self, theme: ui::theme::UiTheme) {
        self.ui_theme = theme;
        self.ui_theme_generation = self.ui_theme_generation.wrapping_add(1);
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
        self.ui.register_font(family, ttf_bytes)
    }

    /// Returns `Err` on swapchain failure; caller exits the event loop on error.
    pub fn render_splash_frame(&mut self) -> Result<()> {
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex) => tex,
            wgpu::CurrentSurfaceTexture::Suboptimal(tex) => {
                self.surface.configure(&self.device, &self.surface_config);
                tex
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                anyhow::bail!("surface lost during splash");
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("surface validation error during splash");
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
        self.record_splash_ui(&mut encoder, &view, viewport);

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        Ok(())
    }

    /// Record the splash through the UI pass into `view`, clearing to black first.
    /// Calls `build_splash_descriptor` (clones the once-loaded `splash.json` tree,
    /// substitutes the version line) and lays the tree out via `UiPass::layout_tree`.
    /// The background fill is
    /// drawn as a separate first quad outside the tree. `encode` is called
    /// unconditionally with `LoadOp::Clear(BLACK)` — on frame 0 the draw lists are
    /// empty and the pass only applies the clear.
    fn record_splash_ui(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        viewport: [u32; 2],
    ) {
        // Background fill quad (drawn first, behind the panel) stays outside the
        // tree — it is a plain oversized letterbox fill, not part of the panel
        // composition. Projected through the `layout` path like before.
        let bg = ui::splash::SplashDescriptor::background_element(splash::splash_bg_rgba());
        let mut panel_list = ui::layout::project(&[bg], viewport);

        // Lay the splash descriptor tree out (panel/fill quads + logo image batch
        // + version text), rebuilt each frame from the stored logo size and the
        // snapshot's version line. Empty when no splash is installed (frame 0).
        let mut draw = ui::tree::UiDrawData::default();
        if let Some(logo_size) = self.splash_logo_size {
            let desc = ui::splash::build_splash_descriptor(&self.ui_snapshot.version_line);
            // The logo `image` node sizes from the asset's natural reference size
            // via the measure seam — thread it in keyed by the splash logo asset.
            let mut image_sizes = ui::tree::ImageSizes::new();
            image_sizes.insert(ui::splash::SPLASH_LOGO_ASSET.to_string(), logo_size);
            // The splash tree carries no state bindings, so it resolves against
            // an empty slot map — behavior unchanged from before binding landed.
            let empty_slots = std::collections::HashMap::new();
            draw = self.ui.layout_tree(
                desc.tree(),
                viewport,
                &image_sizes,
                &empty_slots,
                &self.ui_theme,
            );
        }

        // The tree's panel quads (border + fill) draw behind the logo/text, in
        // the white-texel batch with the background fill — panels + bg share the
        // 1×1 white texel, so they concatenate into one batch.
        panel_list
            .instances
            .extend_from_slice(&draw.quads.instances);

        let white_bg = self.ui.white_bind_group().clone();
        let mut batches: Vec<ui::UiBatch> = Vec::new();
        if !panel_list.is_empty() {
            batches.push(ui::UiBatch {
                list: &panel_list,
                bind_group: &white_bg,
            });
        }
        // Each image batch (the logo) binds the texture its asset key resolves to
        // through the registry. An unknown key degrades by skipping just that
        // batch. Logged at debug, not warn: this runs every frame with no dedup,
        // so a persistently-missing key would spam the log at warn level (§6.1).
        for (asset, list) in &draw.images {
            if list.is_empty() {
                continue;
            }
            match self.ui_images.resolve(asset) {
                Some(bind_group) => batches.push(ui::UiBatch { list, bind_group }),
                None => log::debug!(
                    "[Renderer] UI image asset key '{asset}' is not registered — skipping its draw"
                ),
            }
        }

        // Wrap the splash's assembled batches + text in a single-layer
        // composition — the same encode unit the gameplay modal stack funnels
        // through, so the splash also satisfies the once-per-composition prepare
        // guard. The splash builds its quads from a standalone `panel_list` plus
        // the tree's panel/logo/text draw data (not a `UiDrawData` stack), so it
        // borrows the assembled batches/text directly via `from_batches`.
        let composition = ui::UiComposition::from_batches(batches, draw.texts.clone());

        // The splash path ALWAYS opens the pass with the black clear, even when
        // the composition is empty (frame 0 before install) — the boot "frame-0
        // black" step depends on this. The gameplay-path empty-tree early-out is
        // separate (see `render_frame_indirect`).
        self.ui.encode(
            &self.device,
            &self.queue,
            encoder,
            view,
            viewport,
            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            &composition,
        );
    }

    /// Clear the active splash + its logo registration so post-transition frames
    /// record no splash. The UI pass itself survives.
    pub fn clear_splash(&mut self) {
        self.splash_logo_size = None;
        self.ui_images.clear();
    }
}
