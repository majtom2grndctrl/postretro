// Post-UI screen-space effects resolve pass + the renderer-owned `scene_color`
// offscreen target every gameplay scene/UI pass renders into.
// See: context/lib/rendering_pipeline.md §7

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

use crate::scripting::slot_table::SlotValue;

/// Logical-reference resolution the screen-shake amplitude is authored against
/// (`screen.shake` `[dx, dy]` are in logical-reference px). The shader applies a
/// pure UV add, so the px→UV conversion happens HERE in the Rust pack step:
/// `uv_offset = px / reference_dim`. Keeps the WGSL free of any dimension
/// constants — it just adds the offset to the sample UV.
const SHAKE_REFERENCE_WIDTH: f32 = 1280.0;
const SHAKE_REFERENCE_HEIGHT: f32 = 720.0;

/// Per-frame screen-effects uniform consumed by the resolve shader.
///
/// Packed from the frame's `UiReadSnapshot` slot values by [`pack_effect_uniform`]
/// (a pure, GPU-free function — unit-testable per the testing guide's "No GPU
/// context in tests"). Layout mirrors the `EffectUniform` struct in
/// `screen_effects.wgsl`.
///
/// - `flash` — linear RGBA from `screen.flash`; `flash.a` is the over-blend
///   weight. At rest `[0,0,0,0]` so the over-blend is a no-op (`mix(c, _, 0)`).
/// - `vignette` — `xyz` linear tint + `w` strength from `screen.vignette`
///   (`[r, g, b, a]`, `a` = strength). At rest `w == 0` so the edge tint scales
///   to zero (the radial term is multiplied by `w`).
/// - `shake` — the `screen.shake` `[dx, dy]` logical-reference px offset already
///   converted to a UV offset (divided by the reference dims here). At rest
///   `[0,0]` so the sample UV is unchanged.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct EffectUniform {
    pub flash: [f32; 4],
    pub vignette: [f32; 4],
    pub shake: [f32; 2],
    pub _pad: [f32; 2],
}

impl Default for EffectUniform {
    /// The identity/at-rest uniform: transparent flash, zero-strength vignette,
    /// zero shake. Every effect term collapses to a no-op, so the resolve is a
    /// bit-identical identity blit (the parity gate's at-rest case).
    fn default() -> Self {
        Self {
            flash: [0.0, 0.0, 0.0, 0.0],
            vignette: [0.0, 0.0, 0.0, 0.0],
            shake: [0.0, 0.0],
            _pad: [0.0, 0.0],
        }
    }
}

/// Pack the frame's screen-effect slot values into the resolve uniform.
///
/// PURE and GPU-free: reads `screen.flash` / `screen.vignette` / `screen.shake`
/// out of the `UiReadSnapshot` slot map (all `SlotValue::Array`) and lays them
/// into [`EffectUniform`]. Missing / non-array / short slots fall back to the
/// at-rest value for that field, so an unbound frame packs to the identity
/// uniform (matching the parity-gate "unbound" path). The shake px→UV
/// conversion (divide by the 1280×720 reference dims) happens HERE so the shader
/// stays a pure UV add.
pub fn pack_effect_uniform(slot_values: &HashMap<String, SlotValue>) -> EffectUniform {
    let mut uniform = EffectUniform::default();

    if let Some(flash) = read_array(slot_values, "screen.flash") {
        if flash.len() >= 4 {
            uniform.flash = [flash[0], flash[1], flash[2], flash[3]];
        }
    }

    if let Some(vignette) = read_array(slot_values, "screen.vignette") {
        if vignette.len() >= 4 {
            uniform.vignette = [vignette[0], vignette[1], vignette[2], vignette[3]];
        }
    }

    if let Some(shake) = read_array(slot_values, "screen.shake") {
        if shake.len() >= 2 {
            // px → UV: divide each axis by its reference dimension so the shader
            // applies a pure UV add (no dims in WGSL).
            uniform.shake = [
                shake[0] / SHAKE_REFERENCE_WIDTH,
                shake[1] / SHAKE_REFERENCE_HEIGHT,
            ];
        }
    }

    uniform
}

/// Read a slot's `Array` payload, or `None` when the slot is absent or not an
/// array (the at-rest/unbound fallback path).
fn read_array<'a>(slot_values: &'a HashMap<String, SlotValue>, name: &str) -> Option<&'a [f32]> {
    match slot_values.get(name) {
        Some(SlotValue::Array(values)) => Some(values.as_slice()),
        _ => None,
    }
}

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
/// **Effect seam.** The resolve composes flash/vignette/shake on top of the
/// identity blit. [`pack_effect_uniform`] packs the frame's `screen.*` slot
/// values into [`EffectUniform`] (binding 2 of group 0); the shader applies the
/// math in `screen_effects.wgsl`. At-rest slot values pack to the identity
/// uniform and every effect term ALU-collapses to a no-op, so the output stays
/// byte-identical to the pre-SE blit (parity gate holds for unbound + at-rest).
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

        // Per-frame effect uniform, initialized at-rest (identity) so the very
        // first frame before any write is still a byte-identical blit.
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
            effect_buffer,
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
            &self.effect_buffer,
        );
    }

    /// Record the resolve pass: a fullscreen-triangle blit from `scene_color`
    /// into the swapchain `view`, composing the frame's screen effects
    /// (flash/vignette/shake) on top of the identity blit. The sole swapchain
    /// writer for the gameplay path — encoded every frame, never gated.
    ///
    /// Writes the per-frame effect uniform from the packed `slot_values` first.
    /// At rest all three effect slots collapse to no-ops (see [`pack_effect_uniform`]
    /// and the WGSL), so the output stays byte-identical to the pre-SE blit.
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
    use super::*;
    use crate::scripting::slot_table::SlotValue;
    use std::collections::HashMap;

    fn slots(pairs: &[(&str, SlotValue)]) -> HashMap<String, SlotValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    /// An empty/unbound snapshot packs to the identity uniform — every effect
    /// term is a no-op, so the resolve is a byte-identical blit (parity gate's
    /// unbound path).
    #[test]
    fn pack_unbound_snapshot_yields_identity_uniform() {
        let uniform = pack_effect_uniform(&HashMap::new());
        assert_eq!(uniform, EffectUniform::default());
    }

    /// At-rest slot values (transparent flash, zero vignette strength, zero
    /// shake) pack to the identity/zero uniform (parity gate's at-rest path).
    #[test]
    fn pack_at_rest_slots_yield_identity_uniform() {
        let snapshot = slots(&[
            ("screen.flash", SlotValue::Array(vec![0.0, 0.0, 0.0, 0.0])),
            (
                "screen.vignette",
                SlotValue::Array(vec![0.0, 0.0, 0.0, 0.0]),
            ),
            ("screen.shake", SlotValue::Array(vec![0.0, 0.0])),
        ]);
        let uniform = pack_effect_uniform(&snapshot);
        assert_eq!(uniform, EffectUniform::default());
    }

    /// The snapshot→uniform mapping: each `screen.*` slot lands in its uniform
    /// field, and the shake `[dx, dy]` logical-reference px are converted to a
    /// UV offset by dividing by the 1280×720 reference dims HERE in the pack
    /// step (the shader applies a pure UV add).
    #[test]
    fn pack_maps_slots_and_converts_shake_px_to_uv() {
        let snapshot = slots(&[
            ("screen.flash", SlotValue::Array(vec![1.0, 0.2, 0.3, 0.5])),
            (
                "screen.vignette",
                SlotValue::Array(vec![0.1, 0.0, 0.4, 0.8]),
            ),
            // 128 px horizontal, 72 px vertical against the 1280×720 reference.
            ("screen.shake", SlotValue::Array(vec![128.0, 72.0])),
        ]);
        let uniform = pack_effect_uniform(&snapshot);

        assert_eq!(uniform.flash, [1.0, 0.2, 0.3, 0.5]);
        assert_eq!(uniform.vignette, [0.1, 0.0, 0.4, 0.8]);
        // px → UV: 128 / 1280 = 0.1, 72 / 720 = 0.1.
        assert!((uniform.shake[0] - 0.1).abs() < 1e-6);
        assert!((uniform.shake[1] - 0.1).abs() < 1e-6);
    }

    /// Non-array / short / wrong-type slots fall back to the at-rest value for
    /// that field, so a partial snapshot still packs the present fields and
    /// leaves the rest identity (the unbound fallback path).
    #[test]
    fn pack_falls_back_to_identity_for_missing_or_malformed_slots() {
        let snapshot = slots(&[
            // Present + valid flash.
            ("screen.flash", SlotValue::Array(vec![0.5, 0.5, 0.5, 0.25])),
            // Wrong type — ignored, vignette stays identity.
            ("screen.vignette", SlotValue::Number(1.0)),
            // Too short — ignored, shake stays identity.
            ("screen.shake", SlotValue::Array(vec![10.0])),
        ]);
        let uniform = pack_effect_uniform(&snapshot);

        assert_eq!(uniform.flash, [0.5, 0.5, 0.5, 0.25]);
        assert_eq!(uniform.vignette, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(uniform.shake, [0.0, 0.0]);
    }

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
