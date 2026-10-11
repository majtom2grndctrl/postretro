// GPU readback of forward.wgsl's owner-based world static specular
// (`world_static_specular`), fed per-face owners through a second vertex
// buffer and a flat varying exactly as the forward pipeline delivers them.
// See: context/lib/rendering_pipeline.md §4 (Static specular ownership)
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": the
// function is WGSL, so verifying it means running it. Each case renders quads
// into a one-row `Rgba32Float` target; a pixel's world position is a fixed
// function of its column. The harness self-skips without an adapter and says
// so; a skipped run proves nothing. Set `POSTRETRO_REQUIRE_GPU` to any
// non-empty value other than `0` to make a missing adapter fail instead.

use super::gpu_test_harness::{GpuCtx, gpu_or_skip, wgsl_const_line, wgsl_function, wgsl_item};

const FORWARD_WGSL: &str = include_str!("../shaders/forward.wgsl");
const MATERIAL_SHADING_WGSL: &str = include_str!("../shaders/material_shading.wgsl");
const LIGHT_FALLOFF_WGSL: &str = include_str!("../shaders/light_falloff.wgsl");
const SDF_LIGHT_SELECT_WGSL: &str = include_str!("../shaders/sdf_light_select.wgsl");

const WIDTH: u32 = 16;
const TEXEL_BYTES: u32 = 16;
const EMPTY: u16 = 0xFFFF;
const NO_OWNERS: [u16; 4] = [EMPTY; 4];

fn shader_source() -> String {
    let prelude = r#"
struct SpecLight {
    position_and_range: vec4<f32>,
    color_and_pad:      vec4<f32>,
    cone_dir_and_type:  vec4<f32>,
    cone_cos:           vec4<f32>,
};
struct Case {
    mask: vec4<f32>,
    sdf_factor: vec4<f32>,
    sdf_indices: vec4<u32>,
    sdf_count: u32,
    owners_readable: u32,
    spec_shadowmask_flags: u32,
    sdf_force_lit: u32,
};
@group(0) @binding(0) var<uniform> uniforms: Case;
@group(0) @binding(1) var<storage, read> spec_lights: array<SpecLight>;
"#;
    let entry = r#"
struct HarnessVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) owners: vec4<u32>,
};

@vertex
fn vs_main(@location(0) ndc: vec2<f32>, @location(1) owners: vec4<u32>) -> HarnessVaryings {
    var out: HarnessVaryings;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.owners = owners;
    return out;
}

@fragment
fn fs_main(in: HarnessVaryings) -> @location(0) vec4<f32> {
    // Pixel column c sits at world x = (c + 0.5 - 8) * 0.25 on a floor facing +Y,
    // viewed from straight above.
    let world = vec3<f32>((in.position.x - 8.0) * 0.25, 0.0, 0.0);
    let up = vec3<f32>(0.0, 1.0, 0.0);
    var sel: SdfLightSelection;
    sel.indices = array<u32, 4>(
        uniforms.sdf_indices.x,
        uniforms.sdf_indices.y,
        uniforms.sdf_indices.z,
        uniforms.sdf_indices.w,
    );
    sel.count = uniforms.sdf_count;
    let specular = world_static_specular(
        world,
        up,
        up,
        8.0,
        1.0,
        in.owners,
        uniforms.mask,
        uniforms.owners_readable != 0u,
        sel,
        uniforms.sdf_factor,
        uniforms.sdf_force_lit != 0u,
    );
    return vec4<f32>(specular, 1.0);
}
"#;
    let consts = [
        "SPEC_OWNER_EMPTY",
        "SPEC_OWNER_CHANNELS",
        "SPEC_LIGHT_TYPE_DIRECTIONAL",
        "SPEC_SHADOWMASK_FLAG_FORCE_ONE",
    ]
    .map(|name| wgsl_const_line(FORWARD_WGSL, name))
    .join("\n");
    let helpers = [
        wgsl_item(SDF_LIGHT_SELECT_WGSL, "struct SdfLightSelection {"),
        wgsl_function(FORWARD_WGSL, "world_static_specular"),
        wgsl_function(FORWARD_WGSL, "world_static_light_specular"),
        wgsl_function(FORWARD_WGSL, "positional_cone_aim"),
        wgsl_function(FORWARD_WGSL, "spec_shadowmask_forced_one"),
        wgsl_function(FORWARD_WGSL, "shadowmask_channel_value"),
        wgsl_function(FORWARD_WGSL, "slice_for_visibility"),
        wgsl_function(FORWARD_WGSL, "cone_attenuation_cos"),
        wgsl_function(MATERIAL_SHADING_WGSL, "blinn_phong"),
        wgsl_function(LIGHT_FALLOFF_WGSL, "light_eval_falloff"),
    ]
    .join("\n\n");
    format!("{prelude}\n{consts}\n{helpers}\n{entry}")
}

/// A spec light as `pack_spec_lights` lays it out.
#[derive(Clone, Copy)]
struct Light {
    position: [f32; 3],
    range: f32,
    color: [f32; 3],
    sdf: bool,
    aim: [f32; 3],
    kind: f32,
}

impl Light {
    /// A linear-falloff point light 2 m above the edge between the two faces.
    fn point(color: [f32; 3]) -> Self {
        Self {
            position: [0.0, 2.0, 0.0],
            range: 10.0,
            color,
            sdf: false,
            aim: [0.0; 3],
            kind: 0.0,
        }
    }

    fn directional(origin: [f32; 3], aim: [f32; 3]) -> Self {
        Self {
            position: origin,
            range: 0.0,
            color: [1.0, 1.0, 1.0],
            sdf: false,
            aim,
            kind: 2.0,
        }
    }

    fn bytes(self) -> Vec<u8> {
        let floats = [
            self.position[0],
            self.position[1],
            self.position[2],
            self.range,
            self.color[0],
            self.color[1],
            self.color[2],
            if self.sdf { 1.0 } else { 0.0 },
            self.aim[0],
            self.aim[1],
            self.aim[2],
            self.kind,
            1.0,
            -1.0,
            0.0,
            0.0,
        ];
        floats.iter().flat_map(|f| f.to_ne_bytes()).collect()
    }
}

/// Columns `[x0, x1)` of the one-row target, all of one face's owners.
struct Face {
    columns: (u32, u32),
    owners: [u16; 4],
}

#[derive(Clone, Copy)]
struct Case {
    mask: [f32; 4],
    sdf_factor: [f32; 4],
    sdf_indices: [u32; 4],
    sdf_count: u32,
    owners_readable: bool,
    force_one: bool,
    sdf_force_lit: bool,
}

impl Default for Case {
    fn default() -> Self {
        Self {
            mask: [1.0; 4],
            sdf_factor: [1.0; 4],
            sdf_indices: [u32::MAX; 4],
            sdf_count: 0,
            owners_readable: true,
            force_one: false,
            sdf_force_lit: false,
        }
    }
}

impl Case {
    fn bytes(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        for f in self.mask.iter().chain(&self.sdf_factor) {
            out.extend_from_slice(&f.to_ne_bytes());
        }
        let words = [
            self.sdf_indices[0],
            self.sdf_indices[1],
            self.sdf_indices[2],
            self.sdf_indices[3],
            self.sdf_count,
            u32::from(self.owners_readable),
            // `SPEC_SHADOWMASK_FLAG_FORCE_ONE`, bit 0.
            u32::from(self.force_one),
            u32::from(self.sdf_force_lit),
        ];
        for word in words {
            out.extend_from_slice(&word.to_ne_bytes());
        }
        out
    }
}

/// Render `faces` with `lights` under `case`; one RGB specular sum per column.
fn render(ctx: &GpuCtx, lights: &[Light], faces: &[Face], case: Case) -> Vec<[f32; 3]> {
    use wgpu::util::DeviceExt;

    let device = &ctx.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("world_specular_test shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source().into()),
    });

    let mut positions: Vec<u8> = Vec::new();
    let mut owners: Vec<u8> = Vec::new();
    for face in faces {
        let x0 = face.columns.0 as f32 / WIDTH as f32 * 2.0 - 1.0;
        let x1 = face.columns.1 as f32 / WIDTH as f32 * 2.0 - 1.0;
        for [x, y] in [
            [x0, -1.0],
            [x1, -1.0],
            [x1, 1.0],
            [x0, -1.0],
            [x1, 1.0],
            [x0, 1.0],
        ] {
            positions.extend_from_slice(&f32::to_ne_bytes(x));
            positions.extend_from_slice(&f32::to_ne_bytes(y));
            for owner in face.owners {
                owners.extend_from_slice(&owner.to_ne_bytes());
            }
        }
    }
    let vertex_count = faces.len() as u32 * 6;
    let position_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("world_specular_test positions"),
        contents: &positions,
        usage: wgpu::BufferUsages::VERTEX,
    });
    let owner_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("world_specular_test owners"),
        contents: &owners,
        usage: wgpu::BufferUsages::VERTEX,
    });
    let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("world_specular_test case"),
        contents: &case.bytes(),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let light_bytes: Vec<u8> = lights.iter().flat_map(|light| light.bytes()).collect();
    let light_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("world_specular_test spec lights"),
        contents: &light_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });

    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("world_specular_test layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("world_specular_test bind group"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: light_buffer.as_entire_binding(),
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("world_specular_test pipeline layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let target_format = wgpu::TextureFormat::Rgba32Float;
    let position_attributes = [wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 0,
        shader_location: 0,
    }];
    // The owner stream's layout: one Uint16x4 per vertex in its own buffer.
    let owner_attributes = [wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Uint16x4,
        offset: 0,
        shader_location: 1,
    }];
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("world_specular_test pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[
                Some(wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &position_attributes,
                }),
                Some(wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &owner_attributes,
                }),
            ],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("world_specular_test target"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: target_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let row_bytes = (WIDTH * TEXEL_BYTES).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("world_specular_test readback"),
        size: u64::from(row_bytes),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("world_specular_test pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: -1.0,
                        g: -1.0,
                        b: -1.0,
                        a: 0.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_vertex_buffer(0, position_buffer.slice(..));
        pass.set_vertex_buffer(1, owner_buffer.slice(..));
        pass.draw(0..vertex_count, 0..1);
    }
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(1),
            },
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("map world_specular_test readback");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll world_specular_test device");
    let bytes = slice
        .get_mapped_range()
        .expect("buffer mapped for readback");
    let texels = bytes[..(WIDTH * TEXEL_BYTES) as usize]
        .as_chunks::<{ TEXEL_BYTES as usize }>()
        .0
        .iter()
        .map(|texel| {
            let alpha = f32::from_ne_bytes(texel[12..16].try_into().unwrap());
            assert_eq!(alpha, 1.0, "every column must be covered by a face");
            std::array::from_fn(|c| f32::from_ne_bytes(texel[c * 4..c * 4 + 4].try_into().unwrap()))
        })
        .collect();
    drop(bytes);
    readback.unmap();
    texels
}

fn one_face(owners: [u16; 4]) -> [Face; 1] {
    [Face {
        columns: (0, WIDTH),
        owners,
    }]
}

fn assert_close(actual: [f32; 3], expected: [f32; 3], what: &str) {
    for c in 0..3 {
        assert!(
            (actual[c] - expected[c]).abs() <= 1.0e-5 + expected[c].abs() * 1.0e-4,
            "{what}: channel {c} is {actual:?}, expected {expected:?}"
        );
    }
}

fn scaled(value: [f32; 3], scale: f32) -> [f32; 3] {
    value.map(|c| c * scale)
}

const RED: [f32; 3] = [1.0, 0.0, 0.0];
const GREEN: [f32; 3] = [0.0, 1.0, 0.0];
const BLUE: [f32; 3] = [0.0, 0.0, 1.0];
const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
/// Columns either side of the edge shared by the two faces in `two_faces`.
const LEFT_OF_EDGE: usize = 7;
const RIGHT_OF_EDGE: usize = 8;

fn two_faces(left: [u16; 4], right: [u16; 4]) -> [Face; 2] {
    [
        Face {
            columns: (0, 8),
            owners: left,
        },
        Face {
            columns: (8, WIDTH),
            owners: right,
        },
    ]
}

#[test]
fn owner_specular_lights_only_the_face_that_owns_the_light() {
    let Some(ctx) = gpu_or_skip("owner_specular_lights_only_the_face_that_owns_the_light") else {
        return;
    };
    // Two point lights at the same spot over the shared edge, both in range of
    // every column. Light 0 owns the left face, light 1 the right face.
    let lights = [Light::point(RED), Light::point(GREEN)];
    let out = render(
        &ctx,
        &lights,
        &two_faces([0, EMPTY, EMPTY, EMPTY], [1, EMPTY, EMPTY, EMPTY]),
        Case::default(),
    );
    let (left, right) = (out[LEFT_OF_EDGE], out[RIGHT_OF_EDGE]);
    assert!(
        left[0] > 0.0 && left[1] == 0.0,
        "left of the edge: {left:?}"
    );
    assert!(
        right[1] > 0.0 && right[0] == 0.0,
        "right of the edge: {right:?}"
    );
    // The flat varying carries each face's owners whole: the two edge columns
    // mirror each other, so neither face blends in the other's light.
    assert_close([right[1], 0.0, 0.0], [left[0], 0.0, 0.0], "edge symmetry");

    // A face with no owners adds no static specular, whatever reaches it.
    let none = render(&ctx, &lights, &one_face(NO_OWNERS), Case::default());
    assert!(none.iter().all(|px| *px == [0.0; 3]), "{none:?}");
}

#[test]
fn owner_specular_follows_the_baked_mask_of_its_channel() {
    let Some(ctx) = gpu_or_skip("owner_specular_follows_the_baked_mask_of_its_channel") else {
        return;
    };
    let lights = [Light::point(WHITE)];
    let face = one_face([0, EMPTY, EMPTY, EMPTY]);
    let lit = render(&ctx, &lights, &face, Case::default());
    let occluded = render(
        &ctx,
        &lights,
        &face,
        Case {
            mask: [0.0, 1.0, 1.0, 1.0],
            ..Case::default()
        },
    );
    let half = render(
        &ctx,
        &lights,
        &face,
        Case {
            mask: [0.5, 1.0, 1.0, 1.0],
            ..Case::default()
        },
    );
    for column in 0..WIDTH as usize {
        assert!(
            lit[column][0] > 0.0,
            "column {column} lit: {:?}",
            lit[column]
        );
        assert_eq!(occluded[column], [0.0; 3], "column {column} occluded");
        assert_close(half[column], scaled(lit[column], 0.5), "half-lit channel");
    }

    // A block miss, or no usable mask, drops every owner.
    let unreadable = render(
        &ctx,
        &lights,
        &face,
        Case {
            owners_readable: false,
            ..Case::default()
        },
    );
    assert!(
        unreadable.iter().all(|px| *px == [0.0; 3]),
        "{unreadable:?}"
    );
    // The debug force-one toggle lights owners only, never non-owners.
    let forced = render(
        &ctx,
        &lights,
        &face,
        Case {
            mask: [0.0; 4],
            force_one: true,
            ..Case::default()
        },
    );
    assert_eq!(forced, lit, "force-one reads every owner channel as lit");
}

#[test]
fn owner_position_selects_its_own_channel_in_either_mask_group() {
    let Some(ctx) = gpu_or_skip("owner_position_selects_its_own_channel_in_either_mask_group")
    else {
        return;
    };
    let lights = [
        Light::point(RED),
        Light::point(GREEN),
        Light::point(BLUE),
        Light::point(WHITE),
    ];
    let all = one_face([0, 1, 2, 3]);
    for position in 0..4usize {
        let mut mask = [0.0; 4];
        mask[position] = 1.0;
        let through_position = render(
            &ctx,
            &lights,
            &all,
            Case {
                mask,
                ..Case::default()
            },
        );
        let alone = render(
            &ctx,
            &lights,
            &one_face([position as u16, EMPTY, EMPTY, EMPTY]),
            Case::default(),
        );
        assert_eq!(
            through_position,
            alone,
            "owner position {position} must read channel {position} (group {}, channel {})",
            position / 2,
            position % 2
        );
    }

    // Swapping two owners' positions together with their mask planes leaves
    // the image unchanged, within and across groups.
    let mask = [0.9, 0.6, 0.3, 0.15];
    let base = render(
        &ctx,
        &lights,
        &all,
        Case {
            mask,
            ..Case::default()
        },
    );
    for (a, b) in [(0usize, 1usize), (1, 2), (0, 3)] {
        let mut owners = [0u16, 1, 2, 3];
        owners.swap(a, b);
        let mut swapped_mask = mask;
        swapped_mask.swap(a, b);
        let swapped = render(
            &ctx,
            &lights,
            &one_face(owners),
            Case {
                mask: swapped_mask,
                ..Case::default()
            },
        );
        for column in 0..WIDTH as usize {
            assert_close(swapped[column], base[column], "owner swap");
        }
    }
}

#[test]
fn empty_owner_ends_the_entry_and_out_of_range_owner_adds_nothing() {
    let Some(ctx) = gpu_or_skip("empty_owner_ends_the_entry_and_out_of_range_owner_adds_nothing")
    else {
        return;
    };
    let lights = [Light::point(RED), Light::point(GREEN)];
    let terminated = render(
        &ctx,
        &lights,
        &one_face([0, EMPTY, 1, EMPTY]),
        Case::default(),
    );
    assert!(
        terminated.iter().all(|px| px[0] > 0.0 && px[1] == 0.0),
        "an owner after an empty channel is never read: {terminated:?}"
    );
    let out_of_range = render(
        &ctx,
        &lights,
        &one_face([99, EMPTY, EMPTY, EMPTY]),
        Case::default(),
    );
    assert!(
        out_of_range.iter().all(|px| *px == [0.0; 3]),
        "{out_of_range:?}"
    );
}

#[test]
fn sdf_specular_comes_only_from_the_fragment_selection() {
    let Some(ctx) = gpu_or_skip("sdf_specular_comes_only_from_the_fragment_selection") else {
        return;
    };
    let mut sdf_light = Light::point(BLUE);
    sdf_light.sdf = true;
    let lights = [Light::point(RED), sdf_light];
    let face = one_face(NO_OWNERS);

    // An SDF light the fragment did not select adds nothing, even in range.
    let unselected = render(&ctx, &lights, &face, Case::default());
    assert!(
        unselected.iter().all(|px| *px == [0.0; 3]),
        "{unselected:?}"
    );

    let selected = |factor: f32, force_lit: bool| {
        render(
            &ctx,
            &lights,
            &face,
            Case {
                sdf_indices: [1, u32::MAX, u32::MAX, u32::MAX],
                sdf_count: 1,
                sdf_factor: [factor, 1.0, 1.0, 1.0],
                sdf_force_lit: force_lit,
                ..Case::default()
            },
        )
    };
    let full = selected(1.0, false);
    let half = selected(0.5, false);
    let forced = selected(0.0, true);
    for column in 0..WIDTH as usize {
        assert!(
            full[column][2] > 0.0 && full[column][0] == 0.0,
            "{:?}",
            full[column]
        );
        assert_close(
            half[column],
            scaled(full[column], 0.5),
            "SDF visibility slice",
        );
        assert_eq!(forced[column], full[column], "SDF force-lit");
    }
}

// Missing data — a block miss, a missing or refused mask, placeholder mode —
// leaves owners unreadable (fs_main's `static_spec_owners_readable`). That
// drops every owner's specular and leaves the SDF selection's exactly as it
// was.
#[test]
fn unreadable_owners_drop_static_specular_and_keep_sdf_specular_unchanged() {
    let Some(ctx) =
        gpu_or_skip("unreadable_owners_drop_static_specular_and_keep_sdf_specular_unchanged")
    else {
        return;
    };
    let mut sdf_light = Light::point(BLUE);
    sdf_light.sdf = true;
    let lights = [Light::point(RED), sdf_light];
    let face = one_face([0, EMPTY, EMPTY, EMPTY]);
    let with_sdf = |owners_readable: bool| Case {
        sdf_indices: [1, u32::MAX, u32::MAX, u32::MAX],
        sdf_count: 1,
        sdf_factor: [0.5, 1.0, 1.0, 1.0],
        owners_readable,
        ..Case::default()
    };
    let readable = render(&ctx, &lights, &face, with_sdf(true));
    let unreadable = render(&ctx, &lights, &face, with_sdf(false));
    let sdf_only = render(&ctx, &lights, &one_face(NO_OWNERS), with_sdf(true));
    for column in 0..WIDTH as usize {
        assert!(readable[column][0] > 0.0, "{:?}", readable[column]);
        assert_eq!(unreadable[column][0], 0.0, "no static non-SDF specular");
        assert_eq!(
            unreadable[column], sdf_only[column],
            "SDF specular is unchanged"
        );
        assert_eq!(unreadable[column][2], readable[column][2]);
    }
}

#[test]
fn directional_owner_highlight_follows_its_aim_not_its_origin() {
    let Some(ctx) = gpu_or_skip("directional_owner_highlight_follows_its_aim_not_its_origin")
    else {
        return;
    };
    let face = one_face([0, EMPTY, EMPTY, EMPTY]);
    let tilted = [0.6, -0.8, 0.0];
    let here = render(
        &ctx,
        &[Light::directional([0.0, 5.0, 0.0], tilted)],
        &face,
        Case::default(),
    );
    let elsewhere = render(
        &ctx,
        &[Light::directional([-40.0, -3.0, 17.0], tilted)],
        &face,
        Case::default(),
    );
    assert_eq!(
        here, elsewhere,
        "moving the origin must not move the highlight"
    );
    assert!(
        here[0][0] > 0.0,
        "a sun above the floor lights it: {:?}",
        here[0]
    );

    // Every column sees the same sun, so a directional highlight is uniform
    // across the floor, and it changes with the aim.
    assert!(here.iter().all(|px| *px == here[0]), "{here:?}");
    let overhead = render(
        &ctx,
        &[Light::directional([0.0, 5.0, 0.0], [0.0, -1.0, 0.0])],
        &face,
        Case::default(),
    );
    assert!(
        overhead[0][0] > here[0][0],
        "straight down beats tilted under a top-down view"
    );
    // A sun aimed up from below the floor lights nothing.
    let below = render(
        &ctx,
        &[Light::directional([0.0, 5.0, 0.0], [0.0, 1.0, 0.0])],
        &face,
        Case::default(),
    );
    assert!(below.iter().all(|px| *px == [0.0; 3]), "{below:?}");
    // A zero aim falls back to straight down for its cone as well as its
    // light direction, so it reads exactly as an authored straight-down sun.
    let unaimed = render(
        &ctx,
        &[Light::directional([0.0, 5.0, 0.0], [0.0; 3])],
        &face,
        Case::default(),
    );
    assert_eq!(unaimed, overhead, "a zero aim is the straight-down sun");
}

// A directional light tagged `_shadow_type sdf` has no baked direct to
// follow: its SDF visibility slice is traced toward its origin and its SDF
// diffuse shines from there. Its specular stays the positional highlight it
// always was — keyed on the origin, with the zero-aim cone a point light at
// that origin gets — while an owned directional follows its aim.
#[test]
fn sdf_directional_specular_stays_keyed_on_its_origin() {
    let Some(ctx) = gpu_or_skip("sdf_directional_specular_stays_keyed_on_its_origin") else {
        return;
    };
    let tilted = [0.6, -0.8, 0.0];
    let sdf_sun = |origin: [f32; 3]| Light {
        sdf: true,
        ..Light::directional(origin, tilted)
    };
    let selected = Case {
        sdf_indices: [0, u32::MAX, u32::MAX, u32::MAX],
        sdf_count: 1,
        ..Case::default()
    };
    let face = one_face(NO_OWNERS);
    let here = render(&ctx, &[sdf_sun([0.0, 2.0, 0.0])], &face, selected);
    let moved = render(&ctx, &[sdf_sun([1.5, 2.0, 0.0])], &face, selected);
    assert!(here[LEFT_OF_EDGE][0] > 0.0, "{here:?}");
    assert_ne!(here, moved, "moving the origin moves the highlight");
    assert!(
        here.iter().any(|px| *px != here[0]),
        "a positional highlight varies across the floor: {here:?}"
    );

    // The same light as a cone-less point at that origin, through the owner
    // half: the expression SDF specular evaluated before owners existed.
    let point = Light {
        position: [0.0, 2.0, 0.0],
        range: 0.0,
        ..Light::point(WHITE)
    };
    let as_point = render(
        &ctx,
        &[point],
        &one_face([0, EMPTY, EMPTY, EMPTY]),
        Case::default(),
    );
    for column in 0..WIDTH as usize {
        assert_close(
            here[column],
            as_point[column],
            "SDF sun as its origin point",
        );
    }

    // An owned directional with the same origin and aim follows the aim.
    let owned = render(
        &ctx,
        &[Light::directional([0.0, 2.0, 0.0], tilted)],
        &one_face([0, EMPTY, EMPTY, EMPTY]),
        Case::default(),
    );
    assert!(owned.iter().all(|px| *px == owned[0]), "{owned:?}");
    assert_ne!(owned, here);
}
