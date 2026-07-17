// Renderer boot-phase construction and GPU init: instance/adapter/device setup,
// surface configuration, and the boot splash. Full-phase build: `renderer_full_init.rs`.
// See: context/lib/rendering_pipeline.md

use super::*;

const MAX_OFFSCREEN_CAPTURE_DIMENSION: u32 = 8192;

impl Renderer {
    /// Boot phase: build only the minimal GPU state needed to present the boot
    /// splash — instance, surface, adapter, device, queue, surface configuration,
    /// and the direct `BootSplashPass`. The full renderer (pipelines, lighting,
    /// shadow pools, screen effects, mesh/UI/fog passes, debug lines) is built
    /// later by `finish_full_init`, so first pixels reach the window before that
    /// heavier setup runs. See: context/lib/boot_sequence.md §1.
    ///
    /// Device creation STILL requests the full feature/limit set that eventual
    /// full init needs (`request_renderer_device`) — wgpu features can't be added
    /// after the device exists, so the request happens once, here, up front. The
    /// adapter fail-fast checks that protect hard renderer requirements (and the
    /// ones the boot splash itself relies on, e.g. an srgb-capable surface format)
    /// run here too, before the first splash draw.
    ///
    /// Geometry and textures install later via `install_level_geometry` /
    /// `install_textures`.
    pub fn new(window: &Arc<Window>) -> Result<Self> {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let surface = instance
            .create_surface(window.clone())
            .context("failed to create wgpu surface")?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .context("no suitable GPU adapter found")?;

        let (device, queue, has_multi_draw_indirect, cube_array_supported) =
            request_renderer_device_with_capabilities(&adapter)?;
        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: surface_caps.alpha_modes[0],
            desired_maximum_frame_latency: 2,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);
        log::info!("[Renderer] vsync on");

        // Renderer-owned boot splash pass (clear + logo quad). Built here so the
        // splash path can present before the full renderer is exercised.
        let boot_splash = splash_pass::BootSplashPass::new(&device, surface_format);

        Ok(Self {
            device,
            queue,
            surface: Some(surface),
            surface_config,
            is_surface_configured: true,
            surface_reconfigure_pending: false,
            has_multi_draw_indirect,
            cube_array_supported,
            boot_splash: Some(boot_splash),
            // Full renderer is built on the first `finish_full_init` /
            // `ensure_full_ready`, after the boot splash has presented.
            full: None,
        })
    }

    /// Build a full-ready renderer for deterministic offscreen capture. This
    /// path deliberately creates neither a window nor a `wgpu::Surface`; the
    /// scene target is the capture output and no present path is available.
    pub fn new_offscreen(capture_width: u32, capture_height: u32) -> Result<Self> {
        validate_offscreen_capture_dimensions(capture_width, capture_height)?;

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .context("frame capture requires a GPU adapter")?;
        let (device, queue, has_multi_draw_indirect, cube_array_supported) =
            request_renderer_device_with_capabilities(&adapter)?;

        // This fixed target format makes capture bytes independent of whichever
        // swapchain formats a windowed surface happens to advertise.
        let capture_format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let full = build_full_renderer(
            &device,
            &queue,
            capture_format,
            capture_width,
            capture_height,
            has_multi_draw_indirect,
            cube_array_supported,
        )?;
        Ok(Self {
            device,
            queue,
            surface: None,
            // Retained as the renderer's common target dimensions/format store.
            // Offscreen construction never configures or accesses a surface.
            surface_config: wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: capture_format,
                width: capture_width,
                height: capture_height,
                present_mode: wgpu::PresentMode::AutoVsync,
                alpha_mode: wgpu::CompositeAlphaMode::Opaque,
                desired_maximum_frame_latency: 2,
                view_formats: vec![],
            },
            is_surface_configured: false,
            surface_reconfigure_pending: false,
            has_multi_draw_indirect,
            cube_array_supported,
            boot_splash: None,
            full: Some(Box::new(full)),
        })
    }

    /// Build (or rebuild) the full renderer from current boot state. Idempotent
    /// across surface recreation: any existing `FullRenderer` is dropped (its GPU
    /// resources released) and a fresh one built from the live `surface_config`,
    /// so a suspend→resume that recreates the surface can re-run completion
    /// without re-running app-side deferred session init. Builds with no level
    /// loaded; level data installs later via `install_level_geometry`.
    ///
    /// No raw wgpu handles cross the app boundary — the app calls this; the
    /// renderer stays the sole GPU owner.
    pub fn finish_full_init(&mut self) -> Result<()> {
        let full = build_full_renderer(
            &self.device,
            &self.queue,
            self.surface_config.format,
            self.surface_config.width,
            self.surface_config.height,
            self.has_multi_draw_indirect,
            self.cube_array_supported,
        )?;
        self.full = Some(Box::new(full));
        log::info!("[Renderer] Full renderer initialization complete");
        Ok(())
    }

    /// Ensure the full renderer exists. No-op when already full-ready, so callers
    /// on full-ready-gated paths can call it unconditionally. The boot→content
    /// handoff calls this before clearing the splash and before any Frontend /
    /// Loading-completion / Running / UI / scene path runs.
    pub fn ensure_full_ready(&mut self) -> Result<()> {
        if self.full.is_none() {
            self.finish_full_init()?;
        }
        Ok(())
    }
}

fn validate_offscreen_capture_dimensions(capture_width: u32, capture_height: u32) -> Result<()> {
    if capture_width == 0 || capture_height == 0 {
        anyhow::bail!("offscreen capture dimensions must be non-zero");
    }
    if capture_width > MAX_OFFSCREEN_CAPTURE_DIMENSION
        || capture_height > MAX_OFFSCREEN_CAPTURE_DIMENSION
    {
        anyhow::bail!(
            "offscreen capture dimensions must not exceed {MAX_OFFSCREEN_CAPTURE_DIMENSION}"
        );
    }
    Ok(())
}

/// Request the renderer's complete device feature/limit set and derive the
/// adapter capabilities that full initialization needs. Both windowed and
/// surfaceless construction use this path so capture cannot accidentally get a
/// reduced device contract.
fn request_renderer_device_with_capabilities(
    adapter: &wgpu::Adapter,
) -> Result<(wgpu::Device, wgpu::Queue, bool, bool)> {
    log::info!("[Renderer] GPU adapter: {}", adapter.get_info().name);

    let downlevel = adapter.get_downlevel_capabilities();
    let has_multi_draw_indirect = downlevel
        .flags
        .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION);
    if has_multi_draw_indirect {
        log::info!("[Renderer] Indirect execution supported (multi_draw_indexed_indirect)");
    } else {
        log::info!(
            "[Renderer] Indirect execution not supported — using singular draw_indexed_indirect fallback"
        );
    }

    // Cube-array support gates the dynamic point-light shadow pool. Absent →
    // the cube pool is disabled (None) and point shadows are cleanly off; the
    // spot path is entirely unaffected (no panic, no validation error).
    let cube_array_supported = downlevel
        .flags
        .contains(wgpu::DownlevelFlags::CUBE_ARRAY_TEXTURES);
    if cube_array_supported {
        log::info!("[Renderer] Cube-array textures supported (dynamic point shadows enabled)");
    } else {
        log::info!(
            "[Renderer] Cube-array textures unsupported — dynamic point-light shadows disabled"
        );
    }

    // FrameTiming=None → zero runtime cost when timing isn't requested or supported.
    let adapter_features = adapter.features();
    let gpu_timing_requested = std::env::var("POSTRETRO_GPU_TIMING").ok().as_deref() == Some("1");
    let gpu_timing_supported = adapter_features.contains(wgpu::Features::TIMESTAMP_QUERY);
    let enable_gpu_timing = gpu_timing_requested && gpu_timing_supported;
    // BC5-compressed normal maps are a hard requirement (not optional like
    // GPU timing): the .prm baker emits BC5 normal slots unconditionally.
    let (device, queue) = request_renderer_device(
        adapter,
        cube_array_supported,
        enable_gpu_timing,
        gpu_timing_requested,
        gpu_timing_supported,
    )?;

    Ok((device, queue, has_multi_draw_indirect, cube_array_supported))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offscreen_capture_dimensions_require_non_zero_sizes_within_the_texture_limit() {
        assert!(validate_offscreen_capture_dimensions(1, 1).is_ok());
        assert!(
            validate_offscreen_capture_dimensions(
                MAX_OFFSCREEN_CAPTURE_DIMENSION,
                MAX_OFFSCREEN_CAPTURE_DIMENSION,
            )
            .is_ok()
        );

        assert_eq!(
            validate_offscreen_capture_dimensions(0, 1)
                .unwrap_err()
                .to_string(),
            "offscreen capture dimensions must be non-zero"
        );
        assert_eq!(
            validate_offscreen_capture_dimensions(MAX_OFFSCREEN_CAPTURE_DIMENSION + 1, 1)
                .unwrap_err()
                .to_string(),
            "offscreen capture dimensions must not exceed 8192"
        );
        assert_eq!(
            validate_offscreen_capture_dimensions(1, MAX_OFFSCREEN_CAPTURE_DIMENSION + 1)
                .unwrap_err()
                .to_string(),
            "offscreen capture dimensions must not exceed 8192"
        );
    }
}
