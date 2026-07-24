// Post-UI screen-space effects resolve pass + the renderer-owned `scene_color`
// offscreen target every gameplay scene/UI pass renders into.
// See: context/lib/rendering_pipeline.md §7.8

use std::collections::HashMap;

use super::SCENE_COLOR_FORMAT;
use postretro_entities::SlotValue;
use postretro_render_cpu::screen_effects::{EffectUniform, pack_effect_uniform};

/// The offscreen color target every gameplay scene + UI pass renders into, plus
/// the fullscreen-triangle resolve pass that samples it into the swapchain.
///
/// Modeled on `FogPass::composite_pipeline` / `fog_composite.wgsl`: a
/// no-vertex-buffer fullscreen triangle (`draw(0..3, 0..1)`). The resolve runs
/// EVERY frame as the sole swapchain writer for the gameplay path — never
/// skipped at rest.
///
/// `scene_color` is a linear [`SCENE_COLOR_FORMAT`] target. The resolve samples
/// it without sRGB decoding, tonemaps to display range, then writes the sRGB
/// swapchain target so hardware performs the sole store conversion.
///
/// **Effect seam.** The resolve composes flash/vignette/shake on top of the
/// tonemapped scene. [`pack_effect_uniform`] packs the frame's `screen.*` slot
/// values into [`EffectUniform`] (binding 2 of group 0); the shader applies the
/// math in `screen_effects.wgsl`. At-rest slot values pack to the identity
/// uniform and every effect term ALU-collapses to a no-op after tonemapping.
pub struct ScreenEffectsPass {
    /// Offscreen color target. The scene/UI passes render here; the resolve
    /// samples it. Recreated on resize at the surface size.
    color_texture: wgpu::Texture,
    /// View into `color_texture` used both as the scene/UI passes' color
    /// attachment and as the resolve's sampled source.
    color_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
    /// References `color_view`; rebuilt on resize.
    bind_group: wgpu::BindGroup,
    resolve_pipeline: wgpu::RenderPipeline,
    /// Same shader/operator as the windowed resolve, but targeting deterministic
    /// RGBA8 sRGB capture bytes with transient effects held at rest.
    capture_pipeline: wgpu::RenderPipeline,
    /// Per-frame effect uniform (flash/vignette/shake). Written every frame from
    /// the packed snapshot values; persists across resize (recreating the texture
    /// rebuilds the bind group, which re-references this buffer).
    effect_buffer: wgpu::Buffer,
}

impl ScreenEffectsPass {
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let (color_texture, color_view) = create_scene_color(device, width, height);

        // NEAREST / pixel-aligned sampling preserves the scene's texel grid.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Screen Effects Resolve Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Screen Effects BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // Per-frame effect uniform, initialized at rest for a neutral resolve.
        let effect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Screen Effects Uniform Buffer"),
            size: std::mem::size_of::<EffectUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = create_bind_group(
            device,
            &bind_group_layout,
            &color_view,
            &sampler,
            &effect_buffer,
        );

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Screen Effects Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/screen_effects.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Screen Effects Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let resolve_pipeline = create_resolve_pipeline(
            device,
            &layout,
            &shader,
            surface_format,
            "Screen Effects Resolve Pipeline",
        );
        let capture_pipeline = create_resolve_pipeline(
            device,
            &layout,
            &shader,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            "Screen Effects Capture Tonemap Pipeline",
        );

        Self {
            color_texture,
            color_view,
            sampler,
            bind_group_layout,
            bind_group,
            resolve_pipeline,
            capture_pipeline,
            effect_buffer,
        }
    }

    /// The offscreen color target view the gameplay scene + UI passes render
    /// into (their color attachment, replacing the swapchain `view`).
    pub fn scene_color_view(&self) -> &wgpu::TextureView {
        &self.color_view
    }

    /// The renderer-owned raw HDR scene target. Post-scene passes sample this
    /// before the display/capture resolves.
    pub(super) fn scene_color_texture(&self) -> &wgpu::Texture {
        &self.color_texture
    }

    /// Recreate `scene_color` at the new surface size and rebuild the resolve
    /// bind group. Called alongside the depth-target recreation on resize.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let (color_texture, color_view) = create_scene_color(device, width, height);
        self.color_texture = color_texture;
        self.color_view = color_view;
        self.bind_group = create_bind_group(
            device,
            &self.bind_group_layout,
            &self.color_view,
            &self.sampler,
            &self.effect_buffer,
        );
    }

    /// Record the resolve pass: a fullscreen-triangle blit from `scene_color`
    /// into the swapchain `view`, composing the frame's screen effects
    /// (flash/vignette/shake) after its soft-knee tonemap. The sole swapchain
    /// writer for the gameplay path — encoded every frame, never gated.
    ///
    /// Writes the per-frame effect uniform from the packed `slot_values` first.
    /// At rest all three effect slots collapse to no-ops (see [`pack_effect_uniform`]
    /// and the WGSL), so only the tonemap changes an at-rest scene.
    pub fn encode_resolve(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        swapchain_view: &wgpu::TextureView,
        slot_values: &HashMap<String, SlotValue>,
    ) {
        let uniform = pack_effect_uniform(slot_values);
        queue.write_buffer(&self.effect_buffer, 0, bytemuck::bytes_of(&uniform));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Screen Effects Resolve Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: swapchain_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Resolve covers the full swapchain (fullscreen triangle), so
                    // the prior swapchain contents are fully overwritten. Clear
                    // keeps the load deterministic without an extra dependency.
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            ..Default::default()
        });
        pass.set_pipeline(&self.resolve_pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1); // fullscreen triangle from vertex_index — no vertex buffer
    }

    /// Tonemap the raw HDR scene into the fixed RGBA8 sRGB capture format.
    /// Capture intentionally omits transient screen effects, so it writes an
    /// at-rest effect uniform while reusing the resolve shader and its tonemap.
    pub(super) fn encode_capture_tonemap(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        width: u32,
        height: u32,
    ) -> wgpu::Texture {
        queue.write_buffer(
            &self.effect_buffer,
            0,
            bytemuck::bytes_of(&EffectUniform::default()),
        );
        let capture_scene_view = self
            .scene_color_texture()
            .create_view(&wgpu::TextureViewDescriptor::default());
        let capture_bind_group = create_bind_group(
            device,
            &self.bind_group_layout,
            &capture_scene_view,
            &self.sampler,
            &self.effect_buffer,
        );
        let capture_texture = create_capture_color(device, width, height);
        let capture_view = capture_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Screen Effects Capture Tonemap Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &capture_view,
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
        pass.set_pipeline(&self.capture_pipeline);
        pass.set_bind_group(0, &capture_bind_group, &[]);
        pass.draw(0..3, 0..1);
        capture_texture
    }
}

/// Allocate the linear HDR `scene_color` target at the surface size,
/// single-sample. `RENDER_ATTACHMENT` (scene/UI passes draw into it) +
/// `TEXTURE_BINDING` (display and capture resolves sample it). `0` dims clamp to
/// `1` to keep
/// texture creation valid during transient zero-size resize events (mirrors the
/// depth target's `prepass_attachment_extent`).
fn create_scene_color(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Scene Color Texture"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SCENE_COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_capture_color(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Scene Capture Tonemap Texture"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn create_resolve_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    target_format: wgpu::TextureFormat,
    label: &'static str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
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
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    color_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    effect_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Screen Effects Bind Group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(color_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: effect_buffer.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    /// The resolve shader must parse and declare the fullscreen vertex +
    /// fragment entry points (mirrors `fog_composite_wgsl_parses`).
    #[test]
    fn screen_effects_wgsl_parses() {
        let src = include_str!("../shaders/screen_effects.wgsl");
        let module =
            naga::front::wgsl::parse_str(src).expect("screen_effects.wgsl should parse as WGSL");
        let has_vs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
        let has_fs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "fs_main" && ep.stage == naga::ShaderStage::Fragment);
        assert!(has_vs, "screen_effects.wgsl must export @vertex vs_main");
        assert!(has_fs, "screen_effects.wgsl must export @fragment fs_main");
    }
}
