// GPU proof of the floor layer's blur at the hit: a
// one-pixel bright hit in the highlight-free colour, filtered by the
// production chain and read through the production level function and read
// at the hit, spreads to the probe term's half-width for each exponent.
// See: context/lib/testing_guide.md · context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors)

use glam::{UVec2, Vec2};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_MIN_BLUR_MIP, REFLECTION_PROBE_TILE_DIMENSION,
    reflection_probe_full_colour_mip_count, reflection_probe_mip_exponent,
    reflection_probe_mip0_texel_angle, reflection_probe_term_half_width,
};
use postretro_level_format::specular_smoothness::SPECULAR_EXPONENT_MAX;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::glossy_floor_blur::GlossyFloorTermBlur;
use postretro_render_cpu::render_extent::Extent;

use super::GlossyFloorTargets;
use crate::render::Renderer;
use crate::render::glossy_overlay_frame_test_fixtures::renderer;

/// The reference scene: 1024 × 1024 at a 90° vertical field of view, so
/// 512 scene pixels per unit of tangent at depth 1, at High (chain at ½).
const SCENE: u32 = 1024;
const FOCAL: f32 = 512.0;
/// The reference hit: a hit
/// 4 m along the reflected ray, at 8 m of view depth.
const HIT_LENGTH: f32 = 4.0;
const HIT_DEPTH: f32 = 8.0;
/// The bright hit's value, exact in both highlight-free formats.
const BRIGHT: f32 = 1024.0;
/// Profile samples a side of the hit, and their spacing in scene pixels.
const SAMPLES: u32 = 1024;
const STEP: f32 = 0.5;
/// Tolerance on the half-width at half maximum.
const TOLERANCE: f32 = 0.30;
/// The engine section's probe chain: 128² tiles, every mip.
const ANGLE: f32 = reflection_probe_mip0_texel_angle(REFLECTION_PROBE_TILE_DIMENSION);
const MIPS: u32 = 6;

fn engine_term_blur() -> GlossyFloorTermBlur {
    assert_eq!(
        MIPS,
        reflection_probe_full_colour_mip_count(REFLECTION_PROBE_TILE_DIMENSION)
    );
    GlossyFloorTermBlur::of_section(REFLECTION_PROBE_TILE_DIMENSION, MIPS)
}

const PROBE_WGSL: &str = include_str!("../shaders/reflection_probe_blur.wgsl");
const BLUR_WGSL: &str = include_str!("../shaders/glossy_floor_blur.wgsl");

/// The probe's test entry: samples `2 × SAMPLES` points along x and
/// y through the hit, at the level the production function gives
/// the exponent and hit, through the production read.
const PROFILE_WGSL: &str = r"
struct Profile {
    hit: vec2<f32>,
    scene: vec2<f32>,
    exponent: f32,
    hit_length: f32,
    hit_depth: f32,
    focal: f32,
    step: f32,
    samples: f32,
    mip0_texel_angle: f32,
    colour_mip_count: u32,
};
@group(3) @binding(2) var<uniform> profile: Profile;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_profile(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let offset = (floor(at.x) - profile.samples) * profile.step;
    let axis = select(vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), floor(at.y) > 0.5);
    let uv = (profile.hit + axis * offset) / profile.scene;
    let chain_scale = f32(textureDimensions(floor_chain).y) / profile.scene.y;
    let term = GlossyFloorTermBlur(profile.mip0_texel_angle, profile.colour_mip_count);
    let level = glossy_floor_blur_level(profile.exponent, term, profile.hit_length,
        profile.hit_depth, profile.focal, chain_scale, textureNumLevels(floor_chain));
    return vec4<f32>(glossy_floor_blurred_hit(uv, level), level);
}
";

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ProfileUniform {
    hit: [f32; 2],
    scene: [f32; 2],
    exponent: f32,
    hit_length: f32,
    hit_depth: f32,
    focal: f32,
    step: f32,
    samples: f32,
    term: GlossyFloorTermBlur,
}

/// One sampled profile through the hit: x then y, each `2 × SAMPLES`
/// values, and the level read.
struct Spread {
    x: Vec<f32>,
    y: Vec<f32>,
    level: f32,
}

impl Spread {
    /// Half the full width at half maximum along `profile`, scene pixels.
    fn half_width(profile: &[f32]) -> f32 {
        let (peak_at, peak) = profile
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("samples");
        let half = peak * 0.5;
        let crossing = |indices: &mut dyn Iterator<Item = usize>, toward: isize| {
            for index in indices {
                if profile[index] < half {
                    let inner = (index as isize - toward) as usize;
                    let t = (profile[inner] - half) / (profile[inner] - profile[index]);
                    return inner as f32 + t * toward as f32;
                }
            }
            panic!("the profile never falls to half its peak inside the window");
        };
        let right = crossing(&mut (peak_at..profile.len()), 1);
        let left = crossing(&mut (0..=peak_at).rev(), -1);
        (right - left) * 0.5 * STEP
    }

    /// The reflected-angle half-width this spread reads at the reference
    /// hit, the larger of its two axes.
    fn angle(&self, hit_length: f32) -> f32 {
        let width = Self::half_width(&self.x).max(Self::half_width(&self.y));
        (width * HIT_DEPTH / (FOCAL * hit_length)).atan()
    }
}

/// Floor targets at the reference extent, the highlight-free colour black
/// but for one `BRIGHT` texel at `hit`, the chain recorded by production.
fn chain_over_one_bright_texel(renderer: &Renderer, hit: UVec2) -> GlossyFloorTargets {
    let floor = renderer.full().glossy_overlay.floor();
    let mut targets = None;
    assert!(floor.layout.sync(
        &renderer.device,
        &mut targets,
        Some(engine_term_blur()),
        FluidReflectionQuality::High,
        Extent {
            width: SCENE,
            height: SCENE,
        },
    ));
    let targets = targets.expect("High runs the floor layer");
    let texture = targets.highlight_free_texture();
    let (texel_bytes, bright) = match texture.format() {
        // 2^10: exponent 25 (bias 15), mantissa 0, in each 11/11/10 field.
        wgpu::TextureFormat::Rg11b10Ufloat => {
            let bits: u32 = (25 << 6) | ((25 << 6) << 11) | ((25 << 5) << 22);
            (4, bits.to_le_bytes().to_vec())
        }
        wgpu::TextureFormat::Rgba16Float => {
            let one = half_bits(BRIGHT);
            (
                8,
                [one, one, one, half_bits(1.0)]
                    .map(u16::to_le_bytes)
                    .concat(),
            )
        }
        format => panic!("unexpected highlight-free format {format:?}"),
    };
    let mut texels = vec![0_u8; (SCENE * SCENE * texel_bytes) as usize];
    let at = ((hit.y * SCENE + hit.x) * texel_bytes) as usize;
    texels[at..at + texel_bytes as usize].copy_from_slice(&bright);
    let queue = renderer.queue.raw();
    queue.write_texture(
        texture.as_image_copy(),
        &texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(SCENE * texel_bytes),
            rows_per_image: Some(SCENE),
        },
        texture.size(),
    );
    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Glossy floor blur proof chain"),
        });
    floor.record_chain(&mut encoder, &targets);
    queue.submit([encoder.finish()]);
    targets
}

fn half_bits(value: f32) -> u16 {
    assert_eq!(
        value,
        1024.0_f32.min(value).max(1.0),
        "an exact power of two"
    );
    // 2^e: exponent e + 15, mantissa 0.
    ((value.log2() as u16) + 15) << 10
}

/// The test pipeline's group 3: the chain, its production sampler and the
/// profile uniform.
struct ProfileProbe {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    target: wgpu::Texture,
}

impl ProfileProbe {
    fn new(device: &wgpu::Device) -> Self {
        let mut entries = super::glossy_floor_level_layout_entries().to_vec();
        entries[2].ty = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Glossy floor blur proof BGL"),
            entries: &entries,
        });
        let empty = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Glossy floor blur proof empty BGL"),
            entries: &[],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Glossy floor blur proof layout"),
            bind_group_layouts: &[Some(&empty), Some(&empty), Some(&empty), Some(&layout)],
            immediate_size: 0,
        });
        let source = format!("{BLUR_WGSL}\n{PROBE_WGSL}\n{PROFILE_WGSL}");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Glossy floor blur proof shader"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Glossy floor blur proof pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_profile"),
                targets: &[Some(wgpu::TextureFormat::Rgba32Float.into())],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Glossy floor blur proof profile"),
            size: wgpu::Extent3d {
                width: 2 * SAMPLES,
                height: 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self {
            pipeline,
            layout,
            target,
        }
    }

    /// The profile through `hit` of `targets`' chain at `exponent` and
    /// `hit_length`.
    fn spread(
        &self,
        renderer: &Renderer,
        targets: &GlossyFloorTargets,
        hit: UVec2,
        exponent: f32,
        hit_length: f32,
    ) -> Spread {
        use wgpu::util::DeviceExt;
        let device = &renderer.device;
        let uniform = ProfileUniform {
            hit: (hit.as_vec2() + Vec2::splat(0.5)).to_array(),
            scene: [SCENE as f32; 2],
            exponent,
            hit_length,
            hit_depth: HIT_DEPTH,
            focal: FOCAL,
            step: STEP,
            samples: SAMPLES as f32,
            term: engine_term_blur(),
        };
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Glossy floor blur proof uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let floor = renderer.full().glossy_overlay.floor();
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Glossy floor blur proof group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(targets.chain_view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(floor.layout.march_sampler()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buffer.as_entire_binding(),
                },
            ],
        });
        let empty = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Glossy floor blur proof empty group"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[],
        });
        let view = self.target.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Glossy floor blur proof pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            for slot in 0..3 {
                pass.set_bind_group(slot, &empty, &[]);
            }
            pass.set_bind_group(3, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        renderer.queue.raw().submit([encoder.finish()]);
        let bytes = crate::render::fluid_reflection_frame_test_fixtures::read_back(
            renderer,
            &self.target,
            16,
            wgpu::TextureAspect::All,
        );
        let texels: Vec<[f32; 4]> = bytemuck::pod_collect_to_vec(&bytes);
        let row = (2 * SAMPLES) as usize;
        Spread {
            x: texels[..row].iter().map(|texel| texel[0]).collect(),
            y: texels[row..].iter().map(|texel| texel[0]).collect(),
            level: texels[0][3],
        }
    }
}

/// Exponents from the widest blur the reference window holds to the cap.
const EXPONENTS: [f32; 6] = [12.0, 16.0, 32.0, 64.0, 128.0, 256.0];

// GPU: a one-pixel bright hit, filtered by the
// production scene-colour chain and read at the production level, spreads
// at the reference hit (4 m along the ray, 8 m deep, 512 px per unit of
// tangent, High's ½ chain) to a reflected-angle half-width at half maximum
// within ±30% of the probe term's at the same exponent; the spread grows as
// the exponent falls; at one exponent it grows with hit length as the probe
// term's footprint does (the same angle at a quarter, half and twice the
// reference length); and the exponent cap and the minimum-blur exponent
// read the same spread, bit for bit. Two hits, one on every level's texel
// corner and one off every level's grid, bound the trilinear read's
// dependence on where a hit lands.
#[test]
fn a_one_pixel_hit_spreads_to_the_probe_terms_blur_at_the_reference_hit() {
    let Some(renderer) = renderer() else {
        return;
    };
    let probe = ProfileProbe::new(&renderer.device);
    for hit in [
        UVec2::splat(SCENE / 2),
        UVec2::new(SCENE / 2 + 37, SCENE / 2 - 21),
    ] {
        let targets = chain_over_one_bright_texel(&renderer, hit);
        let mut previous = f32::INFINITY;
        for exponent in EXPONENTS {
            let spread = probe.spread(&renderer, &targets, hit, exponent, HIT_LENGTH);
            let measured = spread.angle(HIT_LENGTH);
            let probe_term = reflection_probe_term_half_width(exponent, ANGLE, MIPS);
            let ratio = measured / probe_term;
            eprintln!(
                "hit {hit}, n = {exponent}: level {:.2}, half-width {measured:.4} rad \
                 against the probe term's {probe_term:.4} ({ratio:.3})",
                spread.level
            );
            assert!(
                (ratio - 1.0).abs() <= TOLERANCE,
                "hit {hit}, n = {exponent}: {measured} rad against {probe_term} ({ratio})"
            );
            assert!(
                measured <= previous,
                "hit {hit}: n = {exponent} spreads wider than the exponent below it"
            );
            previous = measured;
        }
        // One exponent at four hit lengths: the footprint follows the
        // probe term's cone, the same angle at each.
        let probe_term = reflection_probe_term_half_width(64.0, ANGLE, MIPS);
        let mut previous = 0.0;
        for hit_length in [
            HIT_LENGTH / 4.0,
            HIT_LENGTH / 2.0,
            HIT_LENGTH,
            HIT_LENGTH * 2.0,
        ] {
            let spread = probe.spread(&renderer, &targets, hit, 64.0, hit_length);
            let width = Spread::half_width(&spread.x).max(Spread::half_width(&spread.y));
            let ratio = spread.angle(hit_length) / probe_term;
            eprintln!(
                "hit {hit}, n = 64, length {hit_length}: level {:.2}, \
                 {width:.1} px ({ratio:.3})",
                spread.level
            );
            assert!(
                (ratio - 1.0).abs() <= TOLERANCE,
                "hit {hit}, length {hit_length}: {ratio}"
            );
            assert!(width > previous, "the spread grows with hit length");
            previous = width;
        }
        // The cap and the minimum-blur exponent read one spread.
        let min_blur =
            reflection_probe_mip_exponent(REFLECTION_PROBE_MIN_BLUR_MIP as u32, ANGLE) * 1.0001;
        for hit_length in [HIT_LENGTH / 2.0, HIT_LENGTH, HIT_LENGTH * 2.0] {
            let cap = probe.spread(&renderer, &targets, hit, SPECULAR_EXPONENT_MAX, hit_length);
            let at_min = probe.spread(&renderer, &targets, hit, min_blur, hit_length);
            assert_eq!(cap.level, at_min.level, "length {hit_length}");
            assert!(
                cap.x == at_min.x && cap.y == at_min.y,
                "length {hit_length}: the cap reads the minimum blur"
            );
        }
    }
}
