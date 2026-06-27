// Renderer-owned boot splash pass: clears the swapchain and draws the decoded
// logo as a single textured quad. Owns its pipeline, bind group layout, sampler,
// uploaded logo texture, and the (pure, GPU-free) sizing math. Deliberately
// independent of the UI pass — no UiPass, UiImageRegistry, UiReadSnapshot,
// glyphon, taffy, or UI JSON. The boot path uses this directly so first pixels
// reach the window before the UI system initializes.
// See: context/lib/boot_sequence.md §1 · context/lib/rendering_pipeline.md §7.8

use bytemuck::{Pod, Zeroable};

use crate::ui_texture::UiTexture;

const SPLASH_WGSL: &str = include_str!("../shaders/splash.wgsl");

// sRGB decode-on-sample pairs with sRGB encode-on-write — no manual gamma in the
// shader, same as the world/UI texture path.
const SPLASH_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Fraction of the smaller window axis the logo's bounding box is allowed to
/// fill, so the logo always sits inside a margin regardless of window size. The
/// logo keeps its source aspect ratio and is centered; whichever axis binds
/// first caps it. A wide banner logo is width-bound on most windows.
const LOGO_MAX_FRACTION: f32 = 0.7;

/// Outcome of a splash present attempt. The boot state machine advances its
/// frame schedule only on `Presented`; a transient surface failure
/// (`Outdated`/`Lost`/timeout) yields `NeedsRedraw` so startup re-requests a
/// redraw WITHOUT recording `first_black_frame` / `first_splash_frame` or moving
/// to the next splash frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentOutcome {
    /// A command buffer was submitted and the surface texture presented.
    Presented,
    /// The surface could not present this attempt (outdated/lost/timeout). The
    /// surface was reconfigured where applicable; the caller should redraw.
    NeedsRedraw,
}

/// Splash uniform: device viewport (vec2 + pad for 16-byte alignment) and the
/// logo's device-pixel rect `[x, y, w, h]`. Mirrors `SplashUniform` in
/// `splash.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct SplashUniform {
    viewport: [f32; 2],
    _pad: [f32; 2],
    rect: [f32; 4],
}

/// The uploaded logo plus its decoded pixel dimensions. Held by the pass between
/// `install_logo` and `clear`; `None` before the logo is installed (frame 0) and
/// after the boot→content handoff, when the pass records only the black clear.
struct InstalledLogo {
    /// Kept alive so the bind group's texture view stays valid for every draw.
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    /// Decoded source pixel dimensions `[width, height]`, the input to the
    /// aspect-preserving sizing math.
    dims: [u32; 2],
}

/// Boot splash pass. One pipeline, one quad. The fullscreen clear is the render
/// pass `LoadOp::Clear`; the logo (when installed) is the single textured quad.
pub(crate) struct BootSplashPass {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
    logo: Option<InstalledLogo>,
}

impl BootSplashPass {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Boot Splash BGL"),
            entries: &[
                // 0: SplashUniform (viewport + logo rect), read in the vertex stage.
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<SplashUniform>() as u64,
                        ),
                    },
                    count: None,
                },
                // 1: logo texture, sampled in the fragment stage.
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // 2: filtering sampler paired with the float-filterable texture.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Boot Splash Shader"),
            source: wgpu::ShaderSource::Wgsl(SPLASH_WGSL.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Boot Splash Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Boot Splash Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                // Geometry is generated from `vertex_index`; no vertex buffer.
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            // No depth target — one color attachment so the pass is trivial.
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    // Alpha blend so transparent regions of the logo PNG let the
                    // black clear show through.
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Boot Splash Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Boot Splash Uniform"),
            size: std::mem::size_of::<SplashUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            uniform_buffer,
            logo: None,
        }
    }

    /// Upload the decoded logo pixels and build its bind group. Replaces any
    /// previously installed logo (idempotent — a re-install on resume just swaps
    /// the texture). After this, splash frames draw the logo over the clear.
    /// Returns the decoded pixel dimensions for boot logging.
    pub fn install_logo(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        loaded: &UiTexture,
    ) -> [u32; 2] {
        let size = wgpu::Extent3d {
            width: loaded.width,
            height: loaded.height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Boot Splash Logo Texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SPLASH_TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &loaded.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * loaded.width),
                rows_per_image: Some(loaded.height),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Boot Splash Logo Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        let dims = [loaded.width, loaded.height];
        self.logo = Some(InstalledLogo {
            _texture: texture,
            bind_group,
            dims,
        });
        dims
    }

    /// Drop the uploaded logo so post-handoff splash frames record only the
    /// black clear. Used on the boot→content transition and on suspend.
    pub fn clear(&mut self) {
        self.logo = None;
    }

    /// Record one splash frame into `view`: clear to black, then draw the logo
    /// quad when one is installed. Pure GPU encode — the surface acquire/present
    /// stays in the renderer so this can be reused on any target if needed.
    pub fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        viewport: [u32; 2],
    ) {
        if let Some(logo) = &self.logo {
            let rect = logo_rect(logo.dims, viewport);
            queue.write_buffer(
                &self.uniform_buffer,
                0,
                bytemuck::bytes_of(&SplashUniform {
                    viewport: [viewport[0] as f32, viewport[1] as f32],
                    _pad: [0.0, 0.0],
                    rect,
                }),
            );
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Boot Splash Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            ..Default::default()
        });

        // Frame 0 (no logo installed) records only the clear, which the boot
        // schedule relies on for the first black frame.
        if let Some(logo) = &self.logo {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &logo.bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}

/// Aspect-preserving device-pixel rect `[x, y, w, h]` for the logo, centered in
/// the `viewport` with each axis capped at `LOGO_MAX_FRACTION` of the window so
/// the logo never fills the whole frame or stretches. Pure math — no GPU — so it
/// is unit-tested without a device. A degenerate viewport or source (zero on any
/// axis) yields a zero-size rect, which the draw renders as nothing.
fn logo_rect(src_dims: [u32; 2], viewport: [u32; 2]) -> [f32; 4] {
    let (vw, vh) = (viewport[0] as f32, viewport[1] as f32);
    let (sw, sh) = (src_dims[0] as f32, src_dims[1] as f32);
    if vw <= 0.0 || vh <= 0.0 || sw <= 0.0 || sh <= 0.0 {
        return [0.0, 0.0, 0.0, 0.0];
    }

    // Max box the logo may occupy, then fit the source aspect inside it: scale
    // by whichever axis binds first so neither dimension exceeds the box.
    let max_w = vw * LOGO_MAX_FRACTION;
    let max_h = vh * LOGO_MAX_FRACTION;
    let scale = (max_w / sw).min(max_h / sh);
    let w = sw * scale;
    let h = sh * scale;
    // Center in the viewport.
    let x = (vw - w) * 0.5;
    let y = (vh - h) * 0.5;
    [x, y, w, h]
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-3;

    /// A wide banner logo (the committed asset is ~2028×582, aspect ~3.49) is
    /// width-bound on a 16:9 window: the width cap binds before the height cap,
    /// and the height follows from the source aspect, never stretched.
    #[test]
    fn logo_rect_preserves_aspect_and_is_width_bound_for_wide_banner() {
        let [x, y, w, h] = logo_rect([2028, 582], [1280, 720]);
        // Width capped at the fraction of the window width.
        assert!(
            (w - 1280.0 * LOGO_MAX_FRACTION).abs() < EPS,
            "wide logo is width-bound, got w={w}",
        );
        // Height derives from the source aspect — no stretch.
        let src_aspect = 2028.0 / 582.0;
        assert!(
            (w / h - src_aspect).abs() < EPS,
            "rect preserves source aspect, got {}",
            w / h,
        );
        // Centered: equal margins on each axis.
        assert!(
            (x - (1280.0 - w) * 0.5).abs() < EPS,
            "centered horizontally"
        );
        assert!((y - (720.0 - h) * 0.5).abs() < EPS, "centered vertically");
    }

    /// A tall logo on a wide window is height-bound: the height cap binds first
    /// and the width follows the aspect.
    #[test]
    fn logo_rect_is_height_bound_for_tall_source_on_wide_window() {
        let [_x, _y, w, h] = logo_rect([100, 400], [1600, 600]);
        assert!(
            (h - 600.0 * LOGO_MAX_FRACTION).abs() < EPS,
            "tall logo is height-bound, got h={h}",
        );
        let src_aspect = 100.0 / 400.0;
        assert!((w / h - src_aspect).abs() < EPS, "preserves source aspect");
    }

    /// Both logo axes stay within the margin fraction of the window — the logo
    /// never fills the whole frame.
    #[test]
    fn logo_rect_stays_within_window_margin() {
        let [x, y, w, h] = logo_rect([800, 600], [1024, 768]);
        assert!(w <= 1024.0 * LOGO_MAX_FRACTION + EPS, "width within margin");
        assert!(h <= 768.0 * LOGO_MAX_FRACTION + EPS, "height within margin");
        assert!(x >= 0.0 && y >= 0.0, "rect origin inside the frame");
        assert!(x + w <= 1024.0 + EPS && y + h <= 768.0 + EPS, "rect fits");
    }

    /// A degenerate viewport (zero on an axis, e.g. a minimized window) yields a
    /// zero-size rect rather than a NaN/Inf from the divide — the draw then
    /// renders nothing instead of corrupting the frame.
    #[test]
    fn logo_rect_degenerate_viewport_is_zero() {
        assert_eq!(logo_rect([100, 100], [0, 720]), [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(logo_rect([0, 0], [1280, 720]), [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn splash_uniform_is_32_bytes_no_pad_surprises() {
        // The shader's `SplashUniform` declares vec2 + vec2 pad + vec4 = 32 bytes.
        assert_eq!(std::mem::size_of::<SplashUniform>(), 32);
    }

    #[test]
    fn splash_wgsl_parses_and_validates() {
        let module =
            naga::front::wgsl::parse_str(SPLASH_WGSL).expect("splash.wgsl should parse as WGSL");
        let has_vs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
        let has_fs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "fs_main" && ep.stage == naga::ShaderStage::Fragment);
        assert!(has_vs, "splash.wgsl must export @vertex vs_main");
        assert!(has_fs, "splash.wgsl must export @fragment fs_main");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("splash.wgsl must pass naga validation");
    }
}
