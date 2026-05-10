// Boot splash: CPU-side PNG decode (`load_splash`) and GPU pipeline (`SplashPipeline`).
// `load_splash` is CPU-only so the caller can decode before `Renderer::new` completes.
// See: context/lib/rendering_pipeline.md · context/lib/boot_sequence.md §8

use std::path::PathBuf;

use anyhow::{Context, Result};
use wgpu::util::DeviceExt;

use crate::startup::SplashSource;
use crate::texture::LoadedTexture;

/// Resolve a `SplashSource` to its on-disk path.
fn resolve_path(source: &SplashSource) -> PathBuf {
    match source {
        SplashSource::Base => SplashSource::base_path(),
        SplashSource::Mod(p) => p.clone(),
    }
}

/// Decode the splash PNG referenced by `source` into a CPU-side
/// `LoadedTexture`. Performs no wgpu work — the renderer uploads separately.
///
/// Returns an error on missing file, IO failure, or PNG decode failure. The
/// splash path does not fall back to a checkerboard placeholder: a missing
/// base splash is a packaging bug, and a missing mod-supplied splash is a
/// mod-author bug. The caller decides how to surface the error.
pub(crate) fn load_splash(source: &SplashSource) -> Result<LoadedTexture> {
    let path = resolve_path(source);

    let img = image::open(&path)
        .with_context(|| format!("decoding splash PNG at {}", path.display()))?
        .to_rgba8();
    let (width, height) = img.dimensions();

    Ok(LoadedTexture {
        data: img.into_raw(),
        width,
        height,
        is_placeholder: false,
    })
}

/// Size of `SplashUbo` in bytes — must match the WGSL struct in
/// `splash_vert.wgsl` (`screen_size: vec2<f32>` + `tex_size: vec2<f32>`).
const SPLASH_UBO_SIZE: u64 = 16;
// Two adjacent vec2<f32> pack to 16 bytes with no padding under WGSL layout
// rules. This assert proves byte count only — adding a non-vec2 field requires
// re-checking padding manually.
const _: () = assert!(std::mem::size_of::<[f32; 4]>() == SPLASH_UBO_SIZE as usize);

const SPLASH_VERT_WGSL: &str = include_str!("../shaders/splash_vert.wgsl");
const SPLASH_FRAG_WGSL: &str = include_str!("../shaders/splash_frag.wgsl");

/// GPU-side splash texture format. Matches world textures so wgpu's sRGB
/// decode-on-sample lines up with the sRGB swapchain's encode-on-write —
/// no manual gamma in the fragment shader.
const SPLASH_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Upload a CPU-side `LoadedTexture` into a GPU texture suitable for the
/// splash pass. Returns the texture and its (width, height) so the caller
/// can hand both to `Renderer::install_splash`.
///
/// Free function (not a `SplashPipeline` method) so the caller can decode
/// and upload in the post-black-frame window (Splash frame 0) without the
/// pipeline owning the decode timing. The pipeline already exists when this
/// is called; keeping upload separate is a sequencing choice, not a
/// dependency constraint.
///
/// Lives in the renderer module so all wgpu calls stay inside the renderer
/// boundary (per `context/lib/development_guide.md` §4.1).
pub(crate) fn upload_splash_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    loaded: &LoadedTexture,
) -> (wgpu::Texture, [u32; 2]) {
    let size = wgpu::Extent3d {
        width: loaded.width,
        height: loaded.height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Splash Texture"),
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
    (texture, [loaded.width, loaded.height])
}

/// Pack the splash UBO bytes — `screen_size` then `tex_size` as
/// `vec2<f32>` each. Layout must match the WGSL struct in `splash_vert.wgsl`.
fn pack_splash_ubo(screen: [u32; 2], tex: [u32; 2]) -> [u8; SPLASH_UBO_SIZE as usize] {
    let mut out = [0u8; SPLASH_UBO_SIZE as usize];
    out[0..4].copy_from_slice(&(screen[0] as f32).to_le_bytes());
    out[4..8].copy_from_slice(&(screen[1] as f32).to_le_bytes());
    out[8..12].copy_from_slice(&(tex[0] as f32).to_le_bytes());
    out[12..16].copy_from_slice(&(tex[1] as f32).to_le_bytes());
    out
}

/// Fullscreen splash render pipeline. The pipeline + sampler + UBO are
/// created at renderer init; `bind_group` becomes `Some` when
/// `install_splash` is called and `None` after `clear_splash`.
pub(crate) struct SplashPipeline {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    ubo: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: Option<wgpu::BindGroup>,
    tex_size: Option<[u32; 2]>,
}

impl SplashPipeline {
    pub(crate) fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Splash BGL"),
            entries: &[
                // 0: SplashUbo (screen + texture dimensions)
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(SPLASH_UBO_SIZE),
                    },
                    count: None,
                },
                // 1: splash texture (float-filterable so the same BGL works
                // even though we sample with a non-filtering nearest sampler).
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
                // 2: nearest sampler (declared filtering so the BGL pairs
                // cleanly with a `Float { filterable: true }` texture).
                // A non-filtering sampler satisfies a Filtering binding in
                // wgpu; the reverse is not true — do not change the BGL to
                // NonFiltering.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let vert_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Splash Vertex Shader"),
            source: wgpu::ShaderSource::Wgsl(SPLASH_VERT_WGSL.into()),
        });
        let frag_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Splash Fragment Shader"),
            source: wgpu::ShaderSource::Wgsl(SPLASH_FRAG_WGSL.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Splash Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Splash Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vert_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &frag_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        // Nearest-neighbor, ClampToEdge — letterbox bars sample the splash's
        // solid edge texels.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Splash Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let ubo = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Splash UBO"),
            contents: &[0u8; SPLASH_UBO_SIZE as usize],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        Self {
            pipeline,
            sampler,
            ubo,
            bind_group_layout,
            bind_group: None,
            tex_size: None,
        }
    }

    /// Bind a new splash texture: build a bind group for it and write the
    /// UBO with the texture's dimensions paired with the current screen
    /// dimensions. Replaces any previously-bound splash.
    pub(crate) fn install(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        tex_size: [u32; 2],
        screen_size: [u32; 2],
    ) {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Splash Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.ubo.as_entire_binding(),
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
        self.bind_group = Some(bind_group);
        self.tex_size = Some(tex_size);
        self.write_ubo(queue, screen_size);
    }

    pub(crate) fn clear(&mut self) {
        self.bind_group = None;
        self.tex_size = None;
    }

    pub(crate) fn has_splash(&self) -> bool {
        self.bind_group.is_some()
    }

    /// Update the UBO with a new screen size while keeping the bound
    /// texture's dimensions. No-op if no splash is bound — the caller
    /// (`Renderer::resize`) checks `has_splash()` first, but we double-check
    /// here so this stays safe in isolation.
    pub(crate) fn update_screen_size(&self, queue: &wgpu::Queue, screen_size: [u32; 2]) {
        if self.tex_size.is_some() {
            self.write_ubo(queue, screen_size);
        }
    }

    fn write_ubo(&self, queue: &wgpu::Queue, screen_size: [u32; 2]) {
        let tex = self.tex_size.unwrap_or([1, 1]);
        let bytes = pack_splash_ubo(screen_size, tex);
        queue.write_buffer(&self.ubo, 0, &bytes);
    }

    /// Encode the splash render pass into `encoder`, clearing the swapchain
    /// view to black and drawing the fullscreen triangle if a splash is
    /// bound. With no splash bound, the pass clears to black and draws
    /// nothing — used by the first Splash frame to paint a black screen
    /// before the splash texture has been decoded and uploaded.
    pub(crate) fn encode(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Splash Pass"),
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
        if let Some(bind_group) = self.bind_group.as_ref() {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_splash_base_decodes_committed_png() {
        // Resolve the splash path absolutely from CARGO_MANIFEST_DIR so this
        // test is not racy with other tests that might change the working directory.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let splash_path = std::path::Path::new(manifest_dir)
            .ancestors()
            .nth(2)
            .expect("crates/postretro has a workspace root two levels up")
            .join("content/base/textures/splash/postretro-ascii-art.png");

        let tex = load_splash(&SplashSource::Mod(splash_path)).expect("base splash decodes");
        assert!(tex.width > 0 && tex.height > 0, "non-zero dimensions");
        assert_eq!(
            tex.data.len(),
            (tex.width * tex.height * 4) as usize,
            "RGBA8 byte count matches dimensions",
        );
        assert!(!tex.is_placeholder, "real splash, not a checkerboard");
    }

    #[test]
    fn load_splash_mod_returns_error_for_missing_path() {
        let bogus = PathBuf::from("/nonexistent/path/splash.png");
        let err = load_splash(&SplashSource::Mod(bogus)).expect_err("missing file errors");
        let msg = format!("{err:#}");
        assert!(msg.contains("splash"), "error mentions splash: {msg}");
    }
}
