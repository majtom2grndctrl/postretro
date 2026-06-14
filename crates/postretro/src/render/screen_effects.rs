// Post-UI screen-space effects resolve pass + the renderer-owned `scene_color`
// offscreen target every gameplay scene/UI pass renders into.
// See: context/lib/rendering_pipeline.md §7

/// The offscreen color target every gameplay scene + UI pass renders into, plus
/// the fullscreen-triangle resolve pass that samples it into the swapchain.
///
/// Modeled on `FogPass::composite_pipeline` / `fog_composite.wgsl`: a
/// no-vertex-buffer fullscreen triangle (`draw(0..3, 0..1)`). The resolve runs
/// EVERY frame as the sole swapchain writer for the gameplay path — never
/// skipped at rest.
///
/// **Byte-identity contract.** `scene_color` is allocated at the sRGB surface
/// format, single-sample, and the resolve sampler is NEAREST / pixel-aligned, so
/// each `scene_color` texel maps 1:1 to its swapchain texel with no resample.
/// The per-pass sRGB-encode + 8-bit quantize that landed in `scene_color`
/// round-trips losslessly to the swapchain (and fog_composite's dither lands on
/// the same 8-bit grid). This makes the foundation-task identity blit output
/// byte-identical to the pre-SE direct-to-swapchain pipeline.
///
/// **Effect seam.** This task's resolve is an IDENTITY BLIT — no effect uniform
/// is bound, so the shader copies `scene_color` through unchanged. A later SE
/// task packs flash/vignette/shake into an effect uniform and applies the math
/// in `screen_effects.wgsl`; pre-effects the seam is left clean (group 0 carries
/// only the sampled texture + sampler).
pub struct ScreenEffectsPass {
    /// Offscreen color target. The scene/UI passes render here; the resolve
    /// samples it. Recreated on resize at the surface size.
    color_texture: wgpu::Texture,
    /// View into `color_texture` used both as the scene/UI passes' color
    /// attachment and as the resolve's sampled source.
    color_view: wgpu::TextureView,
    /// Surface format (sRGB) the target is allocated at — load-bearing for
    /// byte-identity. Cached so resize recreates the texture at the same format.
    format: wgpu::TextureFormat,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
    /// References `color_view`; rebuilt on resize.
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
}

impl ScreenEffectsPass {
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let (color_texture, color_view) = create_scene_color(device, width, height, surface_format);

        // NEAREST / pixel-aligned: 1:1 texel passthrough so the identity blit is
        // byte-identical to the pre-SE direct-to-swapchain output.
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
            ],
        });

        let bind_group = create_bind_group(device, &bind_group_layout, &color_view, &sampler);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Screen Effects Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/screen_effects.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Screen Effects Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Screen Effects Pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
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
                module: &shader,
                entry_point: Some("fs_main"),
                // Sole swapchain writer for the gameplay path — opaque overwrite,
                // no blend (the source already carries the fully composited frame).
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

        Self {
            color_texture,
            color_view,
            format: surface_format,
            sampler,
            bind_group_layout,
            bind_group,
            pipeline,
        }
    }

    /// The offscreen color target view the gameplay scene + UI passes render
    /// into (their color attachment, replacing the swapchain `view`).
    pub fn scene_color_view(&self) -> &wgpu::TextureView {
        &self.color_view
    }

    /// Recreate `scene_color` at the new surface size and rebuild the resolve
    /// bind group. Called alongside the depth-target recreation on resize.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let (color_texture, color_view) = create_scene_color(device, width, height, self.format);
        self.color_texture = color_texture;
        self.color_view = color_view;
        self.bind_group = create_bind_group(
            device,
            &self.bind_group_layout,
            &self.color_view,
            &self.sampler,
        );
    }

    /// Record the resolve pass: a fullscreen-triangle identity blit from
    /// `scene_color` into the swapchain `view`. The sole swapchain writer for the
    /// gameplay path — encoded every frame, never gated.
    pub fn encode_resolve(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        swapchain_view: &wgpu::TextureView,
    ) {
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1); // fullscreen triangle from vertex_index — no vertex buffer
    }
}

/// Allocate the `scene_color` target: surface format (sRGB), surface size,
/// single-sample. `RENDER_ATTACHMENT` (scene/UI passes draw into it) +
/// `TEXTURE_BINDING` (the resolve samples it). `0` dims clamp to `1` to keep
/// texture creation valid during transient zero-size resize events (mirrors the
/// depth target's `prepass_attachment_extent`).
fn create_scene_color(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
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
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    color_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
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
