// GPU sweep of the mover SH backface ramp across a mover face: the mover pass's
// lookup setup and `sh_backface_weight`, extracted verbatim from the shaders.
// See: context/lib/rendering_pipeline.md §4 (Mover backface) · context/lib/testing_guide.md §3
//
// The ramp is WGSL, so proving it means running it. A compute pass evaluates
// one fragment per invocation and writes, for each of its eight corners, the
// probe's world position and its backface weight. The CPU then measures every
// probe from the fragment's SURFACE point, the quantity the owner's Ruling A
// (mover-light-flow--mid-travel-poses Amendment 2) defines the weight by.

use postretro_render_cpu::sh_depth_tiles::DepthTileGridWords;
use postretro_render_cpu::sh_volume::{ShGridInfoParams, build_grid_info_bytes};
use wgpu::util::DeviceExt;

use crate::render::gpu_test_harness::{
    GpuCtx, gpu_or_skip, wgsl_const_line, wgsl_function, wgsl_item,
};

const KINEMATIC_WGSL: &str = include_str!("../shaders/kinematic_brush.wgsl");
const SH_SAMPLE_WGSL: &str = include_str!("../shaders/sh_sample.wgsl");

/// The ruled form: floor 0.03, ramp 0.4 cell wide, lookup 0.4 cell off.
const FLOOR: f32 = 0.03;
const RAMP_CELLS: f64 = 0.4;
const CELL: f32 = 2.0;
const GRID_ORIGIN: [f32; 3] = [-3.0, 1.0, 0.5];
const GRID_DIMENSIONS: [u32; 3] = [8, 8, 8];

/// The mover pass's lookup setup: both SH reads lift the lookup, record the
/// lift above the geometric plane and derive the cell from it identically.
fn mover_lookup_setup() -> &'static str {
    let setup = |name: &str| {
        let body = wgsl_function(KINEMATIC_WGSL, name);
        let start = body
            .find("    const SH_NORMAL_OFFSET_M")
            .expect("the mover read lifts its lookup");
        let end_marker = "let gfrac = fract(gf);\n";
        let end = body[start..]
            .find(end_marker)
            .expect("the mover read derives gfrac")
            + start
            + end_marker.len();
        &body[start..end]
    };
    let indirect = setup("sample_sh_indirect");
    assert_eq!(
        indirect,
        setup("sample_sh_direct"),
        "both mover SH reads set up their lookup identically"
    );
    assert!(
        indirect.contains("sh_receiver_lookup_lift = dot(offset_world - world_pos, geo_normal);")
    );
    indirect
}

fn shader_source() -> String {
    let consts = [
        "SH_BACKFACE_NONE",
        "SH_BACKFACE_HARD",
        "SH_BACKFACE_SMOOTH",
        "SH_BACKFACE_SMOOTH_FLOOR",
        "SH_MOVER_LOOKUP_OFFSET_CELLS",
        "SH_MOVER_BACKFACE_RAMP_CELLS",
    ]
    .map(|name| wgsl_const_line(SH_SAMPLE_WGSL, name))
    .join("\n");
    let lift = SH_SAMPLE_WGSL
        .lines()
        .find(|line| line.starts_with("var<private> sh_receiver_lookup_lift:"))
        .expect("the receiver lift is module-scope private state");
    let helpers = ["sh_corner_offset", "sh_backface_weight"]
        .map(|name| wgsl_function(SH_SAMPLE_WGSL, name))
        .join("\n\n");
    let grid_info = wgsl_item(KINEMATIC_WGSL, "struct ShGridInfo {");
    let setup = mover_lookup_setup();
    format!(
        r#"{grid_info}
@group(0) @binding(0) var<uniform> sh_grid: ShGridInfo;
{consts}
{lift}
{helpers}

struct Fragment {{
    world_pos: vec4<f32>,
    offset_normal: vec4<f32>,
    geo_normal: vec4<f32>,
}};
@group(0) @binding(1) var<storage, read> fragments: array<Fragment>;
@group(0) @binding(2) var<storage, read_write> results: array<vec4<f32>>;

fn evaluate(index: u32, world_pos: vec3<f32>, offset_normal: vec3<f32>, geo_normal: vec3<f32>) {{
{setup}
    for (var c: u32 = 0u; c < 8u; c = c + 1u) {{
        let corner_offset = sh_corner_offset(c);
        let weight = sh_backface_weight(corner_offset, gfrac, geo_normal, SH_BACKFACE_SMOOTH);
        let probe = sh_grid.grid_origin + vec3<f32>(gi + corner_offset) * sh_grid.cell_size;
        results[index * 8u + c] = vec4<f32>(probe, weight);
    }}
}}

@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
    let fragment = fragments[id.x];
    evaluate(id.x, fragment.world_pos.xyz, fragment.offset_normal.xyz, fragment.geo_normal.xyz);
}}
"#
    )
}

#[derive(Clone, Copy)]
struct Fragment {
    surface: [f32; 3],
    offset_normal: [f32; 3],
    geo_normal: [f32; 3],
}

/// Per fragment, its eight corners' (probe world position, weight).
fn run(ctx: &GpuCtx, fragments: &[Fragment]) -> Vec<[([f32; 3], f32); 8]> {
    let device = &ctx.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("mover backface harness shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source().into()),
    });
    let grid_info = build_grid_info_bytes(ShGridInfoParams {
        grid_origin: GRID_ORIGIN,
        cell_size: [CELL; 3],
        grid_dimensions: GRID_DIMENSIONS,
        atlas_dimensions: [1, 1],
        tile_dimension: 6,
        tile_border: 1,
        atlas_tiles_per_row: 1,
        physical_tile_stride: 6,
        tiles_per_layer: 1,
        atlas_layer_count: 1,
        present: true,
        probe_occlusion_enabled: true,
        depth_tiles: DepthTileGridWords::NONE,
    });
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("mover backface harness sh_grid"),
        contents: &grid_info,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let fragment_bytes: Vec<u8> = fragments
        .iter()
        .flat_map(|f| {
            [f.surface, f.offset_normal, f.geo_normal]
                .into_iter()
                .flat_map(|[x, y, z]| [x, y, z, 0.0])
                .flat_map(f32::to_ne_bytes)
        })
        .collect();
    let fragment_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("mover backface harness fragments"),
        contents: &fragment_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let result_bytes = (fragments.len() * 8 * 16) as u64;
    let results = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mover backface harness results"),
        size: result_bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mover backface harness readback"),
        size: result_bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("mover backface harness pipeline"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mover backface harness bind group"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: fragment_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: results.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(fragments.len() as u32, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&results, 0, &readback, 0, result_bytes);
    ctx.queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| result.unwrap());
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("mover backface harness readback");
    let bytes = slice
        .get_mapped_range()
        .expect("buffer mapped for readback");
    let floats: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_ne_bytes(*chunk))
        .collect();
    drop(bytes);
    floats
        .as_chunks::<32>()
        .0
        .iter()
        .map(|fragment| {
            std::array::from_fn(|c| {
                let [x, y, z, w] = [0, 1, 2, 3].map(|i| fragment[c * 4 + i]);
                ([x, y, z], w)
            })
        })
        .collect()
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = v.iter().map(|c| c * c).sum::<f32>().sqrt();
    v.map(|c| c / length)
}

/// The probe's signed distance from the surface point along the geometric
/// normal, in cells.
fn height_cells(probe: [f32; 3], surface: [f32; 3], normal: [f32; 3]) -> f64 {
    (0..3)
        .map(|i| (f64::from(probe[i]) - f64::from(surface[i])) * f64::from(normal[i]))
        .sum::<f64>()
        / f64::from(CELL)
}

/// The ruled weight: smoothstep over the ramp, plus the floor.
fn ruled_weight(height_cells: f64) -> f64 {
    let r = (height_cells / RAMP_CELLS).clamp(0.0, 1.0);
    r * r * (3.0 - 2.0 * r) + f64::from(FLOOR)
}

/// Fragments on a mover face travelling along its normal past a probe layer:
/// the surface sweeps from 1.5 cells behind `layer` to 1.5 cells past it, so
/// every probe the lookup reaches is seen from both sides of the face.
fn sweep(
    layer: [f32; 3],
    geo_normal: [f32; 3],
    offset_normal: [f32; 3],
    step_cells: f32,
) -> Vec<Fragment> {
    let steps = (3.0 / step_cells).round() as i32;
    (0..=steps)
        .map(|i| {
            let along = (-1.5 + i as f32 * step_cells) * CELL;
            Fragment {
                surface: std::array::from_fn(|axis| layer[axis] + geo_normal[axis] * along),
                offset_normal,
                geo_normal,
            }
        })
        .collect()
}

/// Ruling A's Floor row (Amendment 2): across a sweep of surface distances on
/// both sides of a mover face, the backface ramp is continuous and every valid
/// probe's weight stays at least 0.03. Nothing zeroes or excludes a probe:
/// every corner of every fragment keeps a weight. The weight is measured from
/// the surface point, not the lookup point, with the lookup lifted 0.4 cell,
/// so it holds full weight where the lookup crosses a probe layer.
#[test]
fn mover_backface_ramp_is_continuous_across_the_face_and_never_below_the_floor() {
    let test = "mover_backface_ramp_is_continuous_across_the_face_and_never_below_the_floor";
    let Some(ctx) = gpu_or_skip(test) else {
        return;
    };
    const STEP_CELLS: f32 = 0.005;
    // A point in the grid's interior, off every lattice plane but the swept one.
    let interior = |axis_on_layer: usize| -> [f32; 3] {
        std::array::from_fn(|axis| {
            let cells = if axis == axis_on_layer {
                4.0
            } else {
                3.3 + axis as f32 * 0.6
            };
            GRID_ORIGIN[axis] + cells * CELL
        })
    };
    let x = [1.0, 0.0, 0.0];
    let cases = [
        ("+x face", interior(0), x, x),
        ("-z face", interior(2), [0.0, 0.0, -1.0], [0.0, 0.0, -1.0]),
        // A normal-mapped face lifts its lookup along the bumped normal; the
        // weight still measures from the surface along the geometric normal.
        (
            "+y face, bumped lift",
            interior(1),
            [0.0, 1.0, 0.0],
            normalize([0.3, 1.0, -0.2]),
        ),
        // A rotated mover's face.
        (
            "diagonal face",
            interior(0),
            normalize([1.0, 1.0, 0.0]),
            normalize([1.0, 1.0, 0.0]),
        ),
    ];
    // smoothstep's steepest slope is 1.5 / ramp per cell of height.
    let max_jump = 1.5 * f64::from(STEP_CELLS) / RAMP_CELLS + 1e-4;
    for (label, layer, geo_normal, offset_normal) in cases {
        let fragments = sweep(layer, geo_normal, offset_normal, STEP_CELLS);
        let results = run(&ctx, &fragments);
        // Each probe's (height, weight) in sweep order.
        let mut by_probe: std::collections::BTreeMap<[i64; 3], Vec<(f64, f32)>> =
            Default::default();
        for (fragment, corners) in fragments.iter().zip(&results) {
            for &(probe, weight) in corners {
                let h = height_cells(probe, fragment.surface, geo_normal);
                assert!(
                    weight >= FLOOR,
                    "{label}: probe {probe:?} at {h:.4} cells from the surface weighs {weight}, below the floor"
                );
                let ruled = ruled_weight(h);
                assert!(
                    (f64::from(weight) - ruled).abs() <= 1e-4,
                    "{label}: probe {probe:?} at {h:.4} cells weighs {weight}, the ruled ramp gives {ruled}"
                );
                let key = probe.map(|c| (c / CELL).round() as i64);
                by_probe.entry(key).or_default().push((h, weight));
            }
        }
        let mut crossed_both_ends = 0;
        for (probe, samples) in &by_probe {
            for pair in samples.windows(2) {
                let [(h0, w0), (h1, w1)] = [pair[0], pair[1]];
                assert!(
                    f64::from((w1 - w0).abs()) <= max_jump,
                    "{label}: probe {probe:?} jumps {w0} -> {w1} between {h0:.4} and {h1:.4} cells"
                );
            }
            let behind = samples.iter().any(|&(h, w)| h < -0.1 && w == FLOOR);
            let full = samples.iter().any(|&(h, _)| h > RAMP_CELLS + 0.1);
            if behind && full {
                crossed_both_ends += 1;
            }
        }
        assert!(
            crossed_both_ends > 0,
            "{label}: some probe must be swept from behind the face, through the face plane and \
             the ramp's far end, while staying a corner"
        );
    }
}
