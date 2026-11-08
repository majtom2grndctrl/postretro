// GPU readback harness for the SH depth-visibility decode: runs
// `sh_corner_depth_visibility` and its tile lookup, extracted verbatim from
// sh_sample.wgsl, beside today's pre-tile body kept here as a reference string.
// See: context/lib/rendering_pipeline.md §4 · context/lib/testing_guide.md §3 (GPU tests)
//
// Intentional exception to testing_guide.md §5 "Testing the GPU layer": the
// decode is WGSL, so verifying it means running it. A compute pass evaluates
// one query per invocation (`textureSampleLevel` needs no fragment stage) and
// writes raw f32 bits, so bit-for-bit claims compare bits, not floats.

use postretro_level_format::probe_depth_tiles::ProbeDepthTilesSection;
use postretro_render_cpu::sh_compose::u16_slice_to_bytes;
use postretro_render_cpu::sh_depth_tiles::{DepthTileGridWords, depth_tile_texture_plan};
use postretro_render_cpu::sh_volume::{ShGridInfoParams, build_grid_info_bytes};
use wgpu::util::DeviceExt;

use crate::render::gpu_test_harness::{GpuCtx, wgsl_const_line, wgsl_function, wgsl_item};

const SH_SAMPLE_WGSL: &str = include_str!("../shaders/sh_sample.wgsl");
const FORWARD_WGSL: &str = include_str!("../shaders/forward.wgsl");

/// `sh_corner_depth_visibility` as it stood before id 59 (read at 2968de0ad):
/// the bit-for-bit reference an untiled probe must still match.
const TODAY_CORNER_DEPTH_VISIBILITY: &str = r#"
fn today_corner_depth_visibility(idx: vec3<i32>, sample_world: vec3<f32>, is_valid: bool) -> f32 {
    if (!is_valid) {
        return 0.0;
    }

    let moments = sh_depth_moment_pair(textureLoad(sh_depth_moments, idx, 0));
    let mean = moments.x;
    let mean2 = moments.y;
    let variance = max(mean2 - mean * mean, SH_DEPTH_MIN_VARIANCE_M2);
    let probe_world = sh_grid.grid_origin + vec3<f32>(idx) * sh_grid.cell_size;
    let distance = length(sample_world - probe_world);
    let delta = max(distance - mean - sh_probe_depth_bias(), 0.0);
    let visibility = select(1.0, variance / (variance + delta * delta), delta > 0.0);
    return clamp(visibility, SH_DEPTH_MIN_VISIBILITY, 1.0);
}
"#;

fn shader_source() -> String {
    let consts = [
        "SH_DEPTH_MIN_VARIANCE_M2",
        "SH_DEPTH_BIAS_CELL_FRACTION",
        "SH_DEPTH_MIN_VISIBILITY",
        "SH_DEPTH_TILE_REFERENCE_FLAG",
        "SH_DEPTH_TILE_INDEX_MASK",
        "SH_DEPTH_TILE_EDGE",
        "SH_DEPTH_TILE_BORDER",
        "SH_DEPTH_TILE_BORDERED_EDGE",
    ]
    .map(|name| wgsl_const_line(SH_SAMPLE_WGSL, name))
    .join("\n");
    let helpers = [
        "sh_probe_depth_bias",
        "sh_depth_moment_pair",
        "sh_chebyshev_visibility",
        "oct_encode_l1",
        "sh_depth_tile_moments",
        "sh_corner_depth_visibility",
    ]
    .map(|name| wgsl_function(SH_SAMPLE_WGSL, name))
    .join("\n\n");
    let grid_info = wgsl_item(FORWARD_WGSL, "struct ShGridInfo {");
    let entry = r#"
struct Query {
    idx: vec4<i32>,
    sample_world: vec4<f32>,
};
@group(0) @binding(0) var<uniform> sh_grid: ShGridInfo;
@group(0) @binding(1) var sh_depth_moments: texture_3d<u32>;
@group(0) @binding(2) var sh_depth_tiles: texture_2d<f32>;
@group(0) @binding(3) var sh_atlas_sampler: sampler;
@group(0) @binding(4) var<storage, read> queries: array<Query>;
@group(0) @binding(5) var<storage, read_write> results: array<vec4<u32>>;

@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let query = queries[id.x];
    let idx = query.idx.xyz;
    let is_valid = query.idx.w != 0;
    let visibility = sh_corner_depth_visibility(idx, query.sample_world.xyz, is_valid);
    let today = today_corner_depth_visibility(idx, query.sample_world.xyz, is_valid);
    // The moments the tile path read, when this probe is tiled.
    let packed = textureLoad(sh_depth_moments, idx, 0);
    var tile_moments = vec2<f32>(-1.0);
    if ((packed.g & SH_DEPTH_TILE_REFERENCE_FLAG) != 0u) {
        let probe_world = sh_grid.grid_origin + vec3<f32>(idx) * sh_grid.cell_size;
        tile_moments = sh_depth_tile_moments(
            packed.g & SH_DEPTH_TILE_INDEX_MASK,
            query.sample_world.xyz - probe_world,
        );
    }
    results[id.x] = vec4<u32>(
        bitcast<u32>(visibility),
        bitcast<u32>(today),
        bitcast<u32>(tile_moments.x),
        bitcast<u32>(tile_moments.y),
    );
}
"#;
    format!("{grid_info}\n{consts}\n{helpers}\n{TODAY_CORNER_DEPTH_VISIBILITY}\n{entry}")
}

/// One evaluation: dense lattice probe `probe` (x, y, z) weighing a point at
/// `sample_world`.
#[derive(Clone, Copy)]
pub(super) struct Query {
    pub(super) probe: [i32; 3],
    pub(super) valid: bool,
    pub(super) sample_world: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
pub(super) struct QueryResult {
    /// `sh_corner_depth_visibility` today, as bits.
    pub(super) visibility_bits: u32,
    /// The pre-tile reference body, as bits.
    pub(super) today_bits: u32,
    /// What the tile lookup read for a tiled probe; `[-1, -1]` otherwise.
    pub(super) tile_moments: [f32; 2],
}

impl QueryResult {
    pub(super) fn visibility(&self) -> f32 {
        f32::from_bits(self.visibility_bits)
    }
}

/// A lattice of `grid` probes with origin 0 and unit cells, moment texels
/// `moments` (`[R, G]` per probe, B/A zero), and `tiles` bound as the tile
/// texture with their grid words.
pub(super) struct Lattice<'a> {
    pub(super) grid: [u32; 3],
    pub(super) moments: &'a [[u16; 2]],
    pub(super) tiles: Option<&'a ProbeDepthTilesSection>,
}

pub(super) fn run(ctx: &GpuCtx, lattice: &Lattice<'_>, queries: &[Query]) -> Vec<QueryResult> {
    let device = &ctx.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sh_depth_tiles harness shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source().into()),
    });
    let words = DepthTileGridWords::for_layout(lattice.tiles.and_then(|t| t.texture_layout()));
    let grid_info = build_grid_info_bytes(ShGridInfoParams {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: lattice.grid,
        atlas_dimensions: [1, 1],
        tile_dimension: 6,
        tile_border: 1,
        atlas_tiles_per_row: 1,
        physical_tile_stride: 6,
        tiles_per_layer: 1,
        atlas_layer_count: 1,
        present: true,
        probe_occlusion_enabled: true,
        depth_tiles: words,
    });
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("harness sh_grid"),
        contents: &grid_info,
        usage: wgpu::BufferUsages::UNIFORM,
    });

    let [gx, gy, gz] = lattice.grid;
    assert_eq!(lattice.moments.len(), (gx * gy * gz) as usize);
    let moment_texels: Vec<u16> = lattice
        .moments
        .iter()
        .flat_map(|&[r, g]| [r, g, 0, 0])
        .collect();
    let moments = device.create_texture_with_data(
        &ctx.queue,
        &wgpu::TextureDescriptor {
            label: Some("harness sh_depth_moments"),
            size: wgpu::Extent3d {
                width: gx,
                height: gy,
                depth_or_array_layers: gz,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba16Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &u16_slice_to_bytes(&moment_texels),
    );
    let plan = depth_tile_texture_plan(lattice.tiles);
    let flat: Vec<u16> = plan.texels.iter().flatten().copied().collect();
    let tiles = device.create_texture_with_data(
        &ctx.queue,
        &wgpu::TextureDescriptor {
            label: Some("harness sh_depth_tiles"),
            size: wgpu::Extent3d {
                width: plan.extent[0],
                height: plan.extent[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &u16_slice_to_bytes(&flat),
    );
    // The SH atlas sampler's descriptor (`ShVolumeResources::new`).
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("harness sh_atlas_sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let query_bytes: Vec<u8> = queries
        .iter()
        .flat_map(|query| {
            let [x, y, z] = query.probe;
            let [sx, sy, sz] = query.sample_world;
            [x, y, z, i32::from(query.valid)]
                .map(i32::to_ne_bytes)
                .into_iter()
                .chain([sx, sy, sz, 0.0].map(f32::to_ne_bytes))
                .flatten()
                .collect::<Vec<u8>>()
        })
        .collect();
    let query_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("harness queries"),
        contents: &query_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let result_bytes = (queries.len() * 16) as u64;
    let results = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("harness results"),
        size: result_bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("harness readback"),
        size: result_bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("sh_depth_tiles harness pipeline"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let moments_view = moments.create_view(&Default::default());
    let tiles_view = tiles.create_view(&Default::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sh_depth_tiles harness bind group"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&moments_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&tiles_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: query_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: results.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(queries.len() as u32, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&results, 0, &readback, 0, result_bytes);
    ctx.queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| result.unwrap());
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("harness readback");
    let bytes = slice
        .get_mapped_range()
        .expect("buffer mapped for readback");
    let words: Vec<u32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| u32::from_ne_bytes(*chunk))
        .collect();
    drop(bytes);
    words
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[visibility_bits, today_bits, x, y]| QueryResult {
            visibility_bits,
            today_bits,
            tile_moments: [f32::from_bits(x), f32::from_bits(y)],
        })
        .collect()
}
