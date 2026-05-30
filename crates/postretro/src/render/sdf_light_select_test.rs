// GPU-readback parity test for the shared SDF K-selection helper
// (`sdf_light_select.wgsl`). See:
// context/plans/in-progress/sdf-per-light-shadows/index.md (Rough sketch,
// "Light-selection parity") and architecture.md (the K-selection parity seam).
//
// This is the load-bearing test for the parity seam: the visibility pass and
// the forward shader call the SAME `select_sdf_lights` helper, so verifying the
// helper against a Rust reference comparator pins the order for BOTH consumers
// ("by construction" — the pass↔forward agreement is not separately tested).
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": the
// helper is WGSL, so verifying it requires running it. The harness initializes
// a headless wgpu device and self-skips when no adapter is available (the
// `curve_eval_test.rs` precedent it mirrors).

use wgpu::util::DeviceExt;

/// Per-fragment SDF shadow budget; must match `SDF_SELECT_K` in
/// `sdf_light_select.wgsl`.
const K: usize = 4;
/// Sentinel for an unused slot; must match `SDF_SELECT_NONE` in the helper.
const NONE: u32 = 0xffff_ffff;

// ---------------------------------------------------------------------------
// CPU reference comparator (the pinned total order)
// ---------------------------------------------------------------------------

/// A test light, mirroring the fields the helper reads from `SpecLight`.
#[derive(Clone, Copy)]
struct TestLight {
    position: [f32; 3],
    range: f32,
    /// color × intensity (the helper uses the peak channel as the intensity).
    color: [f32; 3],
    is_sdf: bool,
}

/// Influence metric mirroring `sdf_select_influence` in the helper: the
/// falloff-range attenuation times the light's peak channel; out-of-range or
/// non-positive ⇒ 0 (not selected).
fn influence(light: &TestLight, world: [f32; 3]) -> f32 {
    let dx = light.position[0] - world[0];
    let dy = light.position[1] - world[1];
    let dz = light.position[2] - world[2];
    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
    if light.range > 0.0 && dist > light.range {
        return 0.0;
    }
    let atten = if light.range > 0.0 {
        (1.0 - dist / light.range.max(0.001)).max(0.0)
    } else {
        1.0
    };
    let peak = light.color[0].max(light.color[1]).max(light.color[2]);
    atten * peak
}

/// Reference K-selection: the pinned total order is influence DESCENDING,
/// tie-break light index ASCENDING. Only `sdf`-tagged lights with positive
/// influence are eligible; returns at most K indices into `lights`.
fn reference_select(lights: &[TestLight], world: [f32; 3]) -> Vec<u32> {
    let mut candidates: Vec<(usize, f32)> = lights
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_sdf)
        .map(|(i, l)| (i, influence(l, world)))
        .filter(|(_, inf)| *inf > 0.0)
        .collect();
    // Sort by influence desc, then index asc. The index tie-break makes the
    // order total (no ambiguity for equal influence).
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then_with(|| a.0.cmp(&b.0)));
    candidates
        .into_iter()
        .take(K)
        .map(|(i, _)| i as u32)
        .collect()
}

// ---------------------------------------------------------------------------
// GPU harness
// ---------------------------------------------------------------------------

struct GpuCtx {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn try_init_gpu() -> Option<GpuCtx> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("sdf_light_select_test Device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .ok()?;
    Some(GpuCtx { device, queue })
}

/// Compose the test compute shader: declare the buffers the helper reads (the
/// `has_chunk_grid == 0` path scans the full spec buffer, so no chunk-grid
/// payload is needed) plus an output buffer, then concatenate the SAME shared
/// helper the pass + forward use. One thread per world position writes its K
/// indices + count.
fn shader_source() -> String {
    let prelude = r#"
struct SpecLight {
    position_and_range: vec4<f32>,
    color_and_pad:      vec4<f32>,
};
struct ChunkGridInfo {
    grid_origin: vec3<f32>,
    cell_size: f32,
    dims: vec3<u32>,
    has_chunk_grid: u32,
};
@group(0) @binding(0) var<storage, read> spec_lights: array<SpecLight>;
@group(0) @binding(1) var<uniform> chunk_grid: ChunkGridInfo;
@group(0) @binding(2) var<storage, read> chunk_offsets: array<vec2<u32>>;
@group(0) @binding(3) var<storage, read> chunk_indices: array<u32>;

// Test inputs: one world position per dispatched thread.
@group(0) @binding(4) var<storage, read> test_worlds: array<vec4<f32>>;
// Output: 5 u32 per thread — indices[0..4] then count (K = 4).
@group(0) @binding(5) var<storage, read_write> out_sel: array<u32>;
"#;
    let entry = r#"
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    let world = test_worlds[idx].xyz;
    let sel = select_sdf_lights(world);
    let base = idx * 5u;
    out_sel[base + 0u] = sel.indices[0];
    out_sel[base + 1u] = sel.indices[1];
    out_sel[base + 2u] = sel.indices[2];
    out_sel[base + 3u] = sel.indices[3];
    out_sel[base + 4u] = sel.count;
}
"#;
    format!(
        "{prelude}\n{helper}\n{entry}",
        helper = include_str!("../shaders/sdf_light_select.wgsl"),
    )
}

/// Pack a SpecLight to its 32-byte WGSL layout (two vec4<f32>). The sdf flag
/// rides `color_and_pad.w` (1.0 ⇒ sdf), mirroring `spec_buffer.rs`.
fn pack_light(l: &TestLight) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[0..4].copy_from_slice(&l.position[0].to_ne_bytes());
    b[4..8].copy_from_slice(&l.position[1].to_ne_bytes());
    b[8..12].copy_from_slice(&l.position[2].to_ne_bytes());
    b[12..16].copy_from_slice(&l.range.to_ne_bytes());
    b[16..20].copy_from_slice(&l.color[0].to_ne_bytes());
    b[20..24].copy_from_slice(&l.color[1].to_ne_bytes());
    b[24..28].copy_from_slice(&l.color[2].to_ne_bytes());
    let flag: f32 = if l.is_sdf { 1.0 } else { 0.0 };
    b[28..32].copy_from_slice(&flag.to_ne_bytes());
    b
}

/// Pack a ChunkGridInfo with `has_chunk_grid == 0` (full-spec-buffer scan).
fn pack_chunk_grid_disabled() -> [u8; 32] {
    // grid_origin(12) + cell_size(4) + dims(12) + has_chunk_grid(4) = 32, all
    // zero ⇒ has_chunk_grid == 0.
    [0u8; 32]
}

/// Run the helper on the GPU for each world position, returning, per position,
/// the K selected indices (with NONE sentinels) and the valid count.
fn run_select(ctx: &GpuCtx, lights: &[TestLight], worlds: &[[f32; 3]]) -> Vec<(Vec<u32>, u32)> {
    let module = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sdf_light_select_test shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source().into()),
        });

    let mut spec_bytes: Vec<u8> = Vec::with_capacity(lights.len().max(1) * 32);
    if lights.is_empty() {
        spec_bytes.extend_from_slice(&[0u8; 32]); // storage buffers can't be empty
    } else {
        for l in lights {
            spec_bytes.extend_from_slice(&pack_light(l));
        }
    }

    let mut world_bytes: Vec<u8> = Vec::with_capacity(worlds.len() * 16);
    for w in worlds {
        world_bytes.extend_from_slice(&w[0].to_ne_bytes());
        world_bytes.extend_from_slice(&w[1].to_ne_bytes());
        world_bytes.extend_from_slice(&w[2].to_ne_bytes());
        world_bytes.extend_from_slice(&0f32.to_ne_bytes());
    }

    let spec_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("spec_lights"),
            contents: &spec_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
    let grid_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chunk_grid"),
            contents: &pack_chunk_grid_disabled(),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    // chunk_offsets / chunk_indices are unused on the disabled path but must
    // still be bound (non-empty) so the bind group is valid.
    let offsets_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chunk_offsets"),
            contents: &[0u8; 8], // one vec2<u32>
            usage: wgpu::BufferUsages::STORAGE,
        });
    let indices_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chunk_indices"),
            contents: &[0u8; 4], // one u32
            usage: wgpu::BufferUsages::STORAGE,
        });
    let worlds_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("test_worlds"),
            contents: &world_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });

    let out_size = (worlds.len() * 5 * 4) as u64; // 5 u32 per thread (K = 4 + count)
    let out_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("out_sel"),
        size: out_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: out_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let storage_ro = wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only: true },
        has_dynamic_offset: false,
        min_binding_size: None,
    };
    let entry = |binding: u32, ty: wgpu::BindingType| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty,
        count: None,
    };
    let bgl = ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sdf_light_select_test bgl"),
            entries: &[
                entry(0, storage_ro),
                entry(
                    1,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(2, storage_ro),
                entry(3, storage_ro),
                entry(4, storage_ro),
                entry(
                    5,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sdf_light_select_test bg"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: spec_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: grid_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: offsets_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: indices_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: worlds_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: out_buf.as_entire_binding(),
            },
        ],
    });
    let pl_layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sdf_light_select_test pll"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("sdf_light_select_test pipeline"),
            layout: Some(&pl_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: None,
        });
        cpass.set_pipeline(&pipeline);
        cpass.set_bind_group(0, &bind_group, &[]);
        cpass.dispatch_workgroups(worlds.len() as u32, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&out_buf, 0, &readback, 0, out_size);
    ctx.queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).ok();
    });
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv().expect("map channel").expect("map ok");
    let data = slice.get_mapped_range();
    let raw: Vec<u32> = data
        .chunks_exact(4)
        .map(|c| u32::from_ne_bytes(c.try_into().unwrap()))
        .collect();
    drop(data);
    readback.unmap();

    raw.chunks_exact(5)
        .map(|c| (vec![c[0], c[1], c[2], c[3]], c[4]))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn sdf(position: [f32; 3], range: f32, peak: f32) -> TestLight {
    TestLight {
        position,
        range,
        color: [peak, peak * 0.5, peak * 0.25],
        is_sdf: true,
    }
}

fn baked(position: [f32; 3], range: f32, peak: f32) -> TestLight {
    TestLight {
        is_sdf: false,
        ..sdf(position, range, peak)
    }
}

/// The GPU helper's selection matches the Rust reference comparator's pinned
/// total order (influence desc, index asc) across a set of fixed light layouts,
/// and never returns more than K indices.
#[test]
fn k_selection_matches_reference_order_and_is_bounded() {
    let Some(ctx) = try_init_gpu() else {
        eprintln!("[sdf_light_select_test] skipping: no GPU adapter available");
        return;
    };

    // A mix of sdf + baked lights at varied distances and intensities, with a
    // deliberate exact tie (two equal-influence sdf lights) to exercise the
    // index tie-break, and >K sdf lights to exercise the drop.
    let lights = vec![
        sdf([10.0, 0.0, 0.0], 100.0, 1.0),  // 0: far, dim-ish
        baked([0.5, 0.0, 0.0], 100.0, 5.0), // 1: NOT sdf — must be ignored
        sdf([1.0, 0.0, 0.0], 100.0, 2.0),   // 2: near, bright
        sdf([2.0, 0.0, 0.0], 100.0, 2.0),   // 3: tie candidate
        sdf([2.0, 0.0, 0.0], 100.0, 2.0),   // 4: exact tie with 3 (same pos/peak)
        sdf([3.0, 0.0, 0.0], 100.0, 1.5),   // 5
        sdf([50.0, 0.0, 0.0], 4.0, 9.0),    // 6: out of range from origin → dropped
    ];
    let worlds = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [5.0, 0.0, 0.0],
        [-20.0, 0.0, 0.0],
    ];

    let gpu = run_select(&ctx, &lights, &worlds);
    assert_eq!(gpu.len(), worlds.len());

    for (w, (indices, count)) in worlds.iter().zip(gpu.iter()) {
        // Bounded: count <= K, and exactly `count` leading non-sentinel slots.
        assert!(*count as usize <= K, "count {count} exceeds K={K} at {w:?}");
        let selected: Vec<u32> = indices.iter().copied().take(*count as usize).collect();
        for slot in indices.iter().skip(*count as usize) {
            assert_eq!(*slot, NONE, "slot beyond count must be the NONE sentinel");
        }

        let expected = reference_select(&lights, *w);
        assert_eq!(*count as usize, expected.len(), "count mismatch at {w:?}");
        assert_eq!(selected, expected, "selection order mismatch at {w:?}");
    }
}

/// No sdf lights in range ⇒ empty selection (count 0, all sentinels). Guards
/// the "treated lit" default the visibility pass relies on.
#[test]
fn k_selection_empty_when_no_sdf_in_range() {
    let Some(ctx) = try_init_gpu() else {
        eprintln!("[sdf_light_select_test] skipping: no GPU adapter available");
        return;
    };

    let lights = vec![
        baked([0.0, 0.0, 0.0], 100.0, 5.0), // not sdf
        sdf([100.0, 0.0, 0.0], 2.0, 9.0),   // sdf but far out of range
    ];
    let worlds = [[0.0, 0.0, 0.0]];
    let gpu = run_select(&ctx, &lights, &worlds);
    let (indices, count) = &gpu[0];
    assert_eq!(*count, 0, "expected no selected lights");
    assert_eq!(indices, &[NONE, NONE, NONE, NONE]);
}
