//! Adapter-backed gates for the probe colour mips: upload, per-mip reads, and the overlay's selection at every section shape against render-cpu's reference.
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Blur)

use glam::Vec3;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_RGBA16F, f32_to_f16_bits};
use postretro_level_format::octahedral::{
    irradiance_interior_texel_direction, irradiance_tile_source_texel,
};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_TILE_BORDER, REFLECTION_PROBE_TILE_DIMENSION, ReflectionProbeWorldBinding,
    reflection_probe_full_colour_mip_count, reflection_probe_mip_exponent,
    reflection_probe_mip0_texel_angle, reflection_probe_zero_distances,
};
use postretro_render_cpu::reflection_probe_mips::{
    reflection_probe_mip_colour, reflection_probe_mips,
};

use super::Renderer;
use super::fluid_reflection_frame_test_fixtures::FRAME;
use super::fluid_reflection_sums_frame_test_fixtures::{SumsLevel, probe, probe_section};
use super::gpu_test_harness::{offscreen_renderer_or_skip, wgsl_function};

const TILE: u32 = REFLECTION_PROBE_TILE_DIMENSION;
const BORDER: u32 = REFLECTION_PROBE_TILE_BORDER;
const LAYERS: u32 = 2;

fn mips() -> u32 {
    reflection_probe_full_colour_mip_count(TILE)
}

/// A section's colour chain shape, as id 54's header gives it.
#[derive(Clone, Copy, Debug)]
struct Shape {
    tile: u32,
    mips: u32,
}

/// The engine's: 128² tiles, every mip to one block.
fn engine() -> Shape {
    Shape {
        tile: TILE,
        mips: mips(),
    }
}

/// The value naming interior texel `(x, y)` of mip `mip` on `layer`, exact
/// in f16: R the mip and layer, G and B the texel.
fn label(layer: u32, mip: u32, x: u32, y: u32) -> Vec3 {
    Vec3::new((mip + 10 * layer) as f32, x as f32, y as f32)
}

/// Mip `mip` of `layer`'s `tile`-edge tile, border included, bordered by the
/// SH atlas's wrap rule as the compiler writes it.
fn labelled_mip(tile: u32, layer: u32, mip: u32) -> Vec<Vec3> {
    let dimension = tile >> mip;
    (0..dimension)
        .flat_map(|y| (0..dimension).map(move |x| (x, y)))
        .map(|(x, y)| {
            let [ix, iy] = irradiance_tile_source_texel(x, y, dimension, BORDER);
            label(layer, mip, ix, iy)
        })
        .collect()
}

/// A world-only level: two probes of `shape`'s labelled `Rgba16Float` mips
/// each, stored layer-major as id 54 stores them.
fn labelled_level(shape: Shape) -> SumsLevel {
    let mut section = probe_section((0..LAYERS).map(|layer| probe(layer, &[])).collect(), &[]);
    section.tile_dimension = shape.tile;
    section.atlas_format = IRRADIANCE_FORMAT_RGBA16F;
    section.colour_mip_count = shape.mips;
    section.tiles = (0..LAYERS)
        .flat_map(|layer| (0..shape.mips).flat_map(move |mip| labelled_mip(shape.tile, layer, mip)))
        .flat_map(|texel| {
            [texel.x, texel.y, texel.z, 1.0]
                .map(f32_to_f16_bits)
                .into_iter()
                .flat_map(u16::to_le_bytes)
        })
        .collect();
    section.distances = reflection_probe_zero_distances(LAYERS as usize, shape.tile);
    section.world_bindings = vec![
        ReflectionProbeWorldBinding { face: 0, probe: 0 },
        ReflectionProbeWorldBinding { face: 1, probe: 1 },
    ];
    SumsLevel {
        roster: Vec::new(),
        volumes: Vec::new(),
        probes: section,
    }
}

/// One read: toward `direction` on `layer`, either mip `mip` alone or, with
/// `mip` negative, the selection of `exponent`.
#[derive(Clone, Copy, Debug)]
struct Query {
    direction: Vec3,
    exponent: f32,
    layer: u32,
    mip: i32,
}

impl Query {
    fn bytes(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        let words: [u32; 8] = [
            self.direction.x.to_bits(),
            self.direction.y.to_bits(),
            self.direction.z.to_bits(),
            self.exponent.to_bits(),
            self.layer,
            self.mip as u32,
            0,
            0,
        ];
        for (chunk, word) in out.chunks_mut(4).zip(words) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        out
    }
}

/// The overlay's own read functions, verbatim, over test bindings.
fn shader() -> String {
    let overlay = include_str!("../shaders/glossy_overlay.wgsl");
    let fluid = include_str!("../shaders/fluid_reflection.wgsl");
    format!(
        "@group(0) @binding(0) var fluid_probe_array: texture_2d_array<f32>;\n\
         @group(0) @binding(1) var sh_atlas_sampler: sampler;\n\
         struct FluidProbeTable {{ tile: vec4<u32>, }};\n\
         @group(0) @binding(2) var<uniform> fluid_probes: FluidProbeTable;\n\
         struct Query {{ direction: vec3<f32>, exponent: f32, layer: u32, mip: i32, \
         pad0: u32, pad1: u32, }};\n\
         @group(0) @binding(3) var<storage, read> queries: array<Query>;\n\
         @group(0) @binding(4) var<storage, read_write> results: array<vec4<f32>>;\n\
         {blur}\n{oct}\n{tile_uv}\n{mip}\n{colour}\n\
         @compute @workgroup_size(1)\n\
         fn main(@builtin(global_invocation_id) id: vec3<u32>) {{\n\
             let q = queries[id.x];\n\
             var colour: vec3<f32>;\n\
             if q.mip >= 0 {{\n\
                 colour = glossy_overlay_probe_mip(q.direction, q.layer, u32(q.mip));\n\
             }} else {{\n\
                 colour = glossy_overlay_probe_colour(q.direction, q.layer, q.exponent);\n\
             }}\n\
             results[id.x] = vec4<f32>(colour, 0.0);\n\
         }}\n",
        blur = include_str!("../shaders/reflection_probe_blur.wgsl"),
        oct = wgsl_function(overlay, "oct_encode_unquantized"),
        tile_uv = wgsl_function(fluid, "fluid_reflection_tile_uv"),
        mip = wgsl_function(overlay, "glossy_overlay_probe_mip"),
        colour = wgsl_function(overlay, "glossy_overlay_probe_colour"),
    )
}

/// Run `queries` through the overlay's read functions over the installed
/// level's own colour array, its table naming `tile`.
fn read(renderer: &Renderer, tile: u32, queries: &[Query]) -> Vec<Vec3> {
    let device = &renderer.device;
    let probes = renderer
        .reflection_probes()
        .expect("the level's probes install");
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("probe mip read test"),
        source: wgpu::ShaderSource::Wgsl(shader().into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("probe mip read test"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    // The overlay's probe sampler, as `glossy_overlay.rs` builds it.
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("probe mip read test sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let init = |label: &str, contents: &[u8], usage: wgpu::BufferUsages| {
        use wgpu::util::DeviceExt;
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage,
        })
    };
    let table: Vec<u8> = [tile, BORDER, LAYERS, 0]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let table = init("tile", &table, wgpu::BufferUsages::UNIFORM);
    let query_bytes: Vec<u8> = queries.iter().flat_map(Query::bytes).collect();
    let query_buffer = init("queries", &query_bytes, wgpu::BufferUsages::STORAGE);
    let size = (queries.len() * 16) as u64;
    let results = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("results"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("results staging"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(probes.colour_view()),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: table.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: query_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: results.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(queries.len() as u32, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&results, 0, &staging, 0, size);
    renderer
        .queue
        .submit_unbatched([encoder.finish()], "probe mip read test");
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).ok();
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll");
    rx.recv().expect("map callback").expect("map");
    let bytes = slice.get_mapped_range().expect("mapped range").to_vec();
    bytes
        .chunks(16)
        .map(|chunk| {
            let f = |i: usize| f32::from_le_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
            Vec3::new(f(0), f(1), f(2))
        })
        .collect()
}

/// Interior texels of an `interior`-texel edge near the octahedral folds
/// and corners, and a few inside.
fn fold_texels(interior: u32) -> Vec<(u32, u32)> {
    let last = interior - 1;
    let mut texels = vec![
        (0, 0),
        (last, 0),
        (0, last),
        (last, last),
        (interior / 2, 0),
        (0, interior / 2),
        (last, interior / 2 - 1),
        (interior / 2 - 1, last),
        (interior / 2, interior / 2),
    ];
    texels.sort_unstable();
    texels.dedup();
    texels
}

fn installed(shape: Shape) -> Option<Renderer> {
    let mut renderer = offscreen_renderer_or_skip("glossy_probe_mips_gpu_test", FRAME, FRAME)?;
    labelled_level(shape).install(&mut renderer);
    Some(renderer)
}

// GPU half: the installed colour array holds every mip of id 54's
// chain on every layer, sampled only; its distance array one level.
#[test]
fn every_colour_mip_uploads_into_a_sampled_only_array() {
    let Some(renderer) = installed(engine()) else {
        return;
    };
    let [colour, distance] = renderer
        .reflection_probes()
        .expect("the level's probes install")
        .arrays();
    assert_eq!(colour.mip_level_count(), mips());
    assert_eq!(colour.depth_or_array_layers(), LAYERS);
    assert_eq!(distance.mip_level_count(), 1);
    for array in [colour, distance] {
        assert_eq!(
            array.usage(),
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            "no render-attachment or storage usage"
        );
    }
    // Each mip of each layer holds its own tile: a texel-centre read at mip
    // m on layer l returns that mip's label.
    let queries: Vec<Query> = (0..LAYERS)
        .flat_map(|layer| {
            (0..mips()).map(move |mip| Query {
                direction: Vec3::from(irradiance_interior_texel_direction(
                    0,
                    0,
                    TILE >> mip,
                    BORDER,
                )),
                exponent: 0.0,
                layer,
                mip: mip as i32,
            })
        })
        .collect();
    for (query, read) in queries.iter().zip(read(&renderer, TILE, &queries)) {
        assert_eq!(
            read,
            label(query.layer, query.mip as u32, 0, 0),
            "layer {} mip {}",
            query.layer,
            query.mip
        );
    }
}

// GPU half: at every colour mip down to 4², the direction
// the compiler wrote an interior texel for reads that texel, near the folds
// included, on either layer; and a selection between two mips reads each at
// its own UV, matching render-cpu `reflection_probe_mip_colour`.
#[test]
fn a_direction_reads_its_own_texel_at_every_mip_and_across_a_selection() {
    let Some(renderer) = installed(engine()) else {
        return;
    };
    let mut queries = Vec::new();
    for layer in 0..LAYERS {
        for mip in 0..mips() {
            let dimension = TILE >> mip;
            for (x, y) in fold_texels(dimension - 2 * BORDER) {
                queries.push((
                    Query {
                        direction: Vec3::from(irradiance_interior_texel_direction(
                            x, y, dimension, BORDER,
                        )),
                        exponent: 0.0,
                        layer,
                        mip: mip as i32,
                    },
                    Some((x, y)),
                ));
            }
        }
    }
    // Selections: exponents at and between each mip's lobe, the cap and the
    // minimum, toward mip-2 texel centres near the folds.
    let angle = reflection_probe_mip0_texel_angle(TILE);
    let mut exponents = vec![1.0, 8.0, 12.0, 16.0, 64.0, 8192.0];
    for mip in 2..mips() {
        exponents.push(reflection_probe_mip_exponent(mip, angle));
        if mip + 1 < mips() {
            exponents.push(
                (reflection_probe_mip_exponent(mip, angle)
                    * reflection_probe_mip_exponent(mip + 1, angle))
                .sqrt(),
            );
        }
    }
    for layer in 0..LAYERS {
        for &exponent in &exponents {
            for (x, y) in fold_texels(TILE / 4 - 2 * BORDER) {
                queries.push((
                    Query {
                        direction: Vec3::from(irradiance_interior_texel_direction(
                            x,
                            y,
                            TILE >> 2,
                            BORDER,
                        )),
                        exponent,
                        layer,
                        mip: -1,
                    },
                    None,
                ));
            }
        }
    }
    let plain: Vec<Query> = queries.iter().map(|(query, _)| *query).collect();
    let reads = read(&renderer, TILE, &plain);
    let chains: Vec<Vec<Vec<Vec3>>> = (0..LAYERS)
        .map(|layer| {
            (0..mips())
                .map(|mip| labelled_mip(TILE, layer, mip))
                .collect()
        })
        .collect();
    let mut selections = 0;
    for ((query, texel), read) in queries.iter().zip(reads) {
        match texel {
            Some((x, y)) => {
                let expected = label(query.layer, query.mip as u32, *x, *y);
                assert!(
                    (read - expected).abs().max_element() < 0.05,
                    "layer {} mip {} ({x}, {y}): read {read}, wrote {expected}",
                    query.layer,
                    query.mip
                );
            }
            None => {
                let chain: Vec<&[Vec3]> = chains[query.layer as usize]
                    .iter()
                    .map(Vec::as_slice)
                    .collect();
                let expected = reflection_probe_mip_colour(
                    &chain,
                    TILE,
                    BORDER,
                    query.direction,
                    query.exponent,
                );
                let selection = reflection_probe_mips(query.exponent, TILE, mips());
                // R names the mips: it pins the selection and blend exactly.
                assert!(
                    (read.x - expected.x).abs() < 2.0e-3,
                    "n {} layer {}: mips {read} vs {expected} ({selection:?})",
                    query.exponent,
                    query.layer
                );
                // G and B name texels: a read one texel off misses by ≥ 1.
                assert!(
                    (read - expected).abs().max_element() < 0.3,
                    "n {} layer {} toward {}: {read} vs {expected} ({selection:?})",
                    query.exponent,
                    query.layer,
                    query.direction
                );
                selections += 1;
            }
        }
    }
    assert!(selections > 0);
    eprintln!(
        "[mip selection] {} queries, {selections} selections",
        queries.len()
    );
}

// GPU half of the header-derived selection: a 64² and a 256² section, each
// with its full chain, select the mips render-cpu's reference gives at that
// tile's α₀ (one index lower and higher than at 128²), and a one-mip
// section, at 128² and 256², never reads past mip 0. The overlay reads α₀
// from the probe table's tile dimension and the chain's length from the
// installed array, so no engine tile size or mip count is baked in.
#[test]
fn the_overlay_selects_the_sections_own_mips_at_every_tile_size() {
    let exponents = [
        1.0f32, 8.0, 12.0, 16.0, 24.0, 40.0, 64.0, 100.0, 160.0, 256.0, 576.0, 2048.0, 8192.0,
    ];
    let shapes = [
        Shape {
            tile: 64,
            mips: reflection_probe_full_colour_mip_count(64),
        },
        Shape {
            tile: 256,
            mips: reflection_probe_full_colour_mip_count(256),
        },
        Shape {
            tile: TILE,
            mips: 1,
        },
        Shape { tile: 256, mips: 1 },
    ];
    for shape in shapes {
        let Some(renderer) = installed(shape) else {
            return;
        };
        // Toward texel centres of the sharpest mip a selection reads, near
        // the folds: mip 1 of a full chain, mip 0 of a one-mip one.
        let probe_mip = 1.min(shape.mips - 1);
        let dimension = shape.tile >> probe_mip;
        let mut queries = Vec::new();
        for layer in 0..LAYERS {
            for &exponent in &exponents {
                for (x, y) in fold_texels(dimension - 2 * BORDER) {
                    queries.push(Query {
                        direction: Vec3::from(irradiance_interior_texel_direction(
                            x, y, dimension, BORDER,
                        )),
                        exponent,
                        layer,
                        mip: -1,
                    });
                }
            }
        }
        let reads = read(&renderer, shape.tile, &queries);
        let chains: Vec<Vec<Vec<Vec3>>> = (0..LAYERS)
            .map(|layer| {
                (0..shape.mips)
                    .map(|mip| labelled_mip(shape.tile, layer, mip))
                    .collect()
            })
            .collect();
        let mut deepest = 0.0f32;
        for (query, read) in queries.iter().zip(reads) {
            let selection = reflection_probe_mips(query.exponent, shape.tile, shape.mips);
            let chain: Vec<&[Vec3]> = chains[query.layer as usize]
                .iter()
                .map(Vec::as_slice)
                .collect();
            let expected = reflection_probe_mip_colour(
                &chain,
                shape.tile,
                BORDER,
                query.direction,
                query.exponent,
            );
            // R names the mips (mip + 10 × layer): it pins the selection
            // and blend.
            let read_mip = read.x - (10 * query.layer) as f32;
            assert!(
                (read.x - expected.x).abs() < 2.0e-3,
                "{shape:?} n {} layer {}: mips {read} vs {expected} ({selection:?})",
                query.exponent,
                query.layer
            );
            assert!(
                (read - expected).abs().max_element() < 0.3,
                "{shape:?} n {} layer {} toward {}: {read} vs {expected}",
                query.exponent,
                query.layer,
                query.direction
            );
            assert!(
                read_mip <= (shape.mips - 1) as f32 + 1.0e-3,
                "{shape:?}: n {} reads mip {read_mip}, past the chain",
                query.exponent
            );
            if shape.mips == 1 {
                assert!(
                    read_mip.abs() < 1.0e-3,
                    "{shape:?}: n {} reads mip {read_mip}",
                    query.exponent
                );
            }
            deepest = deepest.max(read_mip);
        }
        eprintln!(
            "[mip selection] {shape:?}: {} selections, deepest mip read {deepest:.3}",
            queries.len()
        );
        if shape.mips > 1 {
            // The roughest exponents reach the section's own last mip.
            assert!(
                (deepest - (shape.mips - 1) as f32).abs() < 1.0e-3,
                "{shape:?}: deepest {deepest}"
            );
        }
    }
}
