// WGSL Catmull-Rom helper parity test — dispatches a compute pipeline and
// compares against the `splines` crate reference. See: context/plans/in-progress/animated-curve-eval/index.md
//
// This file is an intentional exception to testing_guide.md §3 "No GPU
// context in tests": verifying that `curve_eval.wgsl` matches a reference
// Catmull-Rom evaluator requires running the shader. The test initializes a
// headless wgpu instance/adapter/device and self-skips when no adapter is
// available (typical headless-CI case).

use std::f32::consts::PI;

use splines::{Interpolation, Key, Spline};
use wgpu::util::DeviceExt;

// ---------------------------------------------------------------------------
// Reference evaluators (CPU, via the `splines` crate)
// ---------------------------------------------------------------------------

/// Reference Catmull-Rom evaluator over a uniformly-sampled closed loop.
/// Prepends two samples and appends two so `clamped_sample` at `t ∈ [0, 1)`
/// lands inside a well-defined segment; this matches the WGSL helper's
/// modulo-wrap semantics for `count >= 2`.
fn catmull_rom_reference(samples: &[f32], cycle_t: f32) -> f32 {
    let n = samples.len();
    assert!(n >= 2, "reference only defined for count >= 2");
    let n_i = n as i32;
    let keys: Vec<Key<f32, f32>> = (-2i32..=(n_i + 1))
        .map(|k| {
            Key::new(
                k as f32 / n as f32,
                samples[k.rem_euclid(n_i) as usize],
                Interpolation::CatmullRom,
            )
        })
        .collect();
    Spline::from_vec(keys)
        .clamped_sample(cycle_t)
        .expect("clamped sample should resolve inside the padded key range")
}

/// Per-channel reference for RGB — `splines` does not implement `Interpolate`
/// for `[f32; 3]`, so we run three scalar splines and recombine.
fn catmull_rom_reference_rgb(samples_rgb: &[[f32; 3]], cycle_t: f32) -> [f32; 3] {
    let ch0: Vec<f32> = samples_rgb.iter().map(|c| c[0]).collect();
    let ch1: Vec<f32> = samples_rgb.iter().map(|c| c[1]).collect();
    let ch2: Vec<f32> = samples_rgb.iter().map(|c| c[2]).collect();
    [
        catmull_rom_reference(&ch0, cycle_t),
        catmull_rom_reference(&ch1, cycle_t),
        catmull_rom_reference(&ch2, cycle_t),
    ]
}

// ---------------------------------------------------------------------------
// GPU harness
// ---------------------------------------------------------------------------

const EPSILON: f32 = 1e-4;

/// Authored scalar curve — strictly positive and smooth over one cycle.
fn authored_scalar(count: u32) -> Vec<f32> {
    (0..count)
        .map(|k| (2.0 * PI * k as f32 / count as f32).sin() * 0.5 + 1.0)
        .collect()
}

/// Authored RGB curve — each channel is a phase-shifted sine, bounded > 0.
fn authored_rgb(count: u32) -> Vec<[f32; 3]> {
    (0..count)
        .map(|k| {
            let t = 2.0 * PI * k as f32 / count as f32;
            // arbitrary phase offsets to keep the three channels distinguishable
            [
                t.sin() * 0.4 + 0.6,
                (t + 2.0).sin() * 0.4 + 0.6,
                (t + 4.0).sin() * 0.4 + 0.6,
            ]
        })
        .collect()
}

/// Minimal GPU context used by the parity tests. Returns `None` when the
/// environment has no usable GPU adapter (headless CI without software
/// rasterizer) — callers should treat `None` as "skip the test".
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
        label: Some("curve_eval_test Device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .ok()?;

    Some(GpuCtx { device, queue })
}

/// Build the scalar compute shader — textual concatenation of the binding
/// declarations + the helper + a 1-thread-per-work entry point.
fn scalar_shader_source() -> String {
    let prelude = r#"
@group(0) @binding(0) var<storage, read> anim_samples: array<f32>;
@group(0) @binding(1) var<storage, read_write> out_values: array<f32>;

struct Dispatch {
    count: u32,
    cycle_t: f32,
};
@group(0) @binding(2) var<storage, read> dispatches: array<Dispatch>;
"#;
    let entry = r#"
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    let d = dispatches[idx];
    out_values[idx] = sample_curve_catmull_rom(0u, d.count, d.cycle_t);
}
"#;
    format!(
        "{prelude}\n{helper}\n{entry}",
        helper = include_str!("../shaders/curve_eval.wgsl"),
    )
}

/// Build the RGB compute shader. `base_color` is passed per-dispatch so we
/// can exercise both the `count >= 2` and degenerate `count == 0` paths.
fn color_shader_source() -> String {
    let prelude = r#"
@group(0) @binding(0) var<storage, read> anim_samples: array<f32>;
@group(0) @binding(1) var<storage, read_write> out_values: array<vec4<f32>>;

struct Dispatch {
    count: u32,
    cycle_t: f32,
    base_r: f32,
    base_g: f32,
    base_b: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};
@group(0) @binding(2) var<storage, read> dispatches: array<Dispatch>;
"#;
    let entry = r#"
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    let d = dispatches[idx];
    let base = vec3<f32>(d.base_r, d.base_g, d.base_b);
    let c = sample_color_catmull_rom(0u, d.count, d.cycle_t, base);
    out_values[idx] = vec4<f32>(c, 0.0);
}
"#;
    format!(
        "{prelude}\n{helper}\n{entry}",
        helper = include_str!("../shaders/curve_eval.wgsl"),
    )
}

/// Shared GPU harness: upload `sample_bytes` + `dispatch_bytes` as storage
/// buffers, dispatch `n` compute threads running `shader_src`, and read back
/// `n * out_elem_bytes` of raw output bytes.
fn run_compute(
    ctx: &GpuCtx,
    shader_src: String,
    sample_bytes: Vec<u8>,
    dispatch_bytes: Vec<u8>,
    n: u32,
    out_elem_bytes: u64,
) -> Vec<u8> {
    let module = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("curve_eval_test shader"),
            source: wgpu::ShaderSource::Wgsl(shader_src.into()),
        });

    // Storage buffers cannot be zero-sized; pad empty sample arrays.
    let sample_bytes: Vec<u8> = if sample_bytes.is_empty() {
        vec![0u8; 4]
    } else {
        sample_bytes
    };

    let samples_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("anim_samples"),
            contents: &sample_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
    let dispatch_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dispatches"),
            contents: &dispatch_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });

    let out_size = (n as u64) * out_elem_bytes;
    let out_size = out_size.max(4); // wgpu rejects zero-sized buffers
    let out_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("out_values"),
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

    let bgl = ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("curve_eval_test bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("curve_eval_test bg"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: samples_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: out_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: dispatch_buf.as_entire_binding(),
            },
        ],
    });
    let pl_layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("curve_eval_test pll"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("curve_eval_test pipeline"),
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
        cpass.dispatch_workgroups(n, 1, 1);
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
    let out = data.to_vec();
    drop(data);
    readback.unmap();
    out
}

/// Upload `samples` + `dispatches` (as POD bytes), dispatch N threads, read
/// back `out_count` f32 results.
fn run_scalar_compute(ctx: &GpuCtx, samples: &[f32], dispatches: &[(u32, f32)]) -> Vec<f32> {
    // Pack dispatches as [u32, f32] pairs — 8 bytes each, no WGSL padding.
    let mut dispatch_bytes: Vec<u8> = Vec::with_capacity(dispatches.len() * 8);
    for (count, cycle_t) in dispatches {
        dispatch_bytes.extend_from_slice(&count.to_ne_bytes());
        dispatch_bytes.extend_from_slice(&cycle_t.to_ne_bytes());
    }

    let mut sample_bytes: Vec<u8> = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        sample_bytes.extend_from_slice(&s.to_ne_bytes());
    }

    let n = dispatches.len() as u32;
    let raw = run_compute(
        ctx,
        scalar_shader_source(),
        sample_bytes,
        dispatch_bytes,
        n,
        4,
    );

    let mut out = Vec::with_capacity(dispatches.len());
    for chunk in raw.chunks_exact(4).take(dispatches.len()) {
        out.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
    }
    out
}

/// Like `run_scalar_compute` but reads back vec4<f32> per dispatch (RGB + pad).
fn run_color_compute(
    ctx: &GpuCtx,
    samples: &[f32],
    dispatches: &[(u32, f32, [f32; 3])],
) -> Vec<[f32; 3]> {
    // Dispatch struct in WGSL is 32 bytes (8 × f32/u32 with padding for
    // vec3 alignment). We mirror it here: count, cycle_t, base_r, base_g,
    // base_b, then three f32 pad slots.
    let mut dispatch_bytes: Vec<u8> = Vec::with_capacity(dispatches.len() * 32);
    for (count, cycle_t, base) in dispatches {
        dispatch_bytes.extend_from_slice(&count.to_ne_bytes());
        dispatch_bytes.extend_from_slice(&cycle_t.to_ne_bytes());
        dispatch_bytes.extend_from_slice(&base[0].to_ne_bytes());
        dispatch_bytes.extend_from_slice(&base[1].to_ne_bytes());
        dispatch_bytes.extend_from_slice(&base[2].to_ne_bytes());
        dispatch_bytes.extend_from_slice(&0f32.to_ne_bytes());
        dispatch_bytes.extend_from_slice(&0f32.to_ne_bytes());
        dispatch_bytes.extend_from_slice(&0f32.to_ne_bytes());
    }

    let mut sample_bytes: Vec<u8> = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        sample_bytes.extend_from_slice(&s.to_ne_bytes());
    }

    let n = dispatches.len() as u32;
    let raw = run_compute(
        ctx,
        color_shader_source(),
        sample_bytes,
        dispatch_bytes,
        n,
        16,
    );

    let mut out = Vec::with_capacity(dispatches.len());
    for chunk in raw.chunks_exact(16).take(dispatches.len()) {
        let r = f32::from_ne_bytes(chunk[0..4].try_into().unwrap());
        let g = f32::from_ne_bytes(chunk[4..8].try_into().unwrap());
        let b = f32::from_ne_bytes(chunk[8..12].try_into().unwrap());
        out.push([r, g, b]);
    }
    out
}

fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
    (a - b).abs() <= eps
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// GPU output matches the `splines` reference across a broad (count, cycle_t)
/// grid. Also covers knot-exact parity: at `t_k = k / count` the evaluator
/// reproduces the stored sample within 1e-4.
#[test]
fn curve_eval_scalar_matches_splines_reference() {
    let Some(ctx) = try_init_gpu() else {
        eprintln!("[curve_eval_test] skipping: no GPU adapter available");
        return;
    };

    let counts = [2u32, 3, 4, 8];
    let grid_ts = [0.0f32, 0.17, 0.5, 0.83, 0.99];

    for &count in &counts {
        let samples = authored_scalar(count);

        // Interior grid + knot points (t_k = k/count).
        let mut ts: Vec<f32> = grid_ts.to_vec();
        for k in 0..count {
            ts.push(k as f32 / count as f32);
        }

        let dispatches: Vec<(u32, f32)> = ts.iter().map(|&t| (count, t)).collect();
        let gpu = run_scalar_compute(&ctx, &samples, &dispatches);

        assert_eq!(gpu.len(), dispatches.len());

        for (i, &(_, t)) in dispatches.iter().enumerate() {
            let got = gpu[i];
            assert!(
                got.is_finite(),
                "non-finite output at count={count} t={t}: {got}"
            );
            let expected = catmull_rom_reference(&samples, t);
            assert!(
                approx_eq(got, expected, EPSILON),
                "count={count} t={t}: gpu={got} reference={expected} (delta {})",
                (got - expected).abs()
            );
        }

        // Knot-exact: t_k = k/count reproduces samples[k] within epsilon.
        for k in 0..count {
            let t = k as f32 / count as f32;
            let idx = dispatches
                .iter()
                .position(|&(c, td)| c == count && td == t)
                .expect("knot t must be present in dispatch grid");
            let got = gpu[idx];
            let expected = samples[k as usize];
            assert!(
                approx_eq(got, expected, EPSILON),
                "knot-exact failure count={count} k={k}: gpu={got} sample={expected}"
            );
        }
    }
}

/// At the cycle boundary the curve wraps continuously: `sample(1 - eps)` and
/// `sample(0)` differ by O(eps) on a smooth authored curve. Guards against a
/// regression where the wrap picked up the wrong neighbor set.
#[test]
fn curve_eval_wraps_continuously_at_cycle_boundary() {
    let Some(ctx) = try_init_gpu() else {
        eprintln!("[curve_eval_test] skipping: no GPU adapter available");
        return;
    };

    let count = 8u32;
    let samples = authored_scalar(count);
    let dispatches = [(count, 0.0f32), (count, 1.0 - 1e-3)];
    let gpu = run_scalar_compute(&ctx, &samples, &dispatches);

    let near_one = gpu[1];
    let zero = gpu[0];
    let delta = (near_one - zero).abs();
    // Smooth curve, eps = 1e-3 → delta should be a small multiple of eps.
    // Catmull-Rom derivative on the authored sine is bounded by ~2π, so
    // 1e-2 is generous and catches any discontinuity.
    assert!(
        delta < 1e-2,
        "wrap discontinuity: |sample(1-1e-3) - sample(0)| = {delta} (zero={zero} near_one={near_one})"
    );
    assert!(zero.is_finite() && near_one.is_finite());
}

/// Degenerate counts: 0 → 1.0 exactly, 1 → the single sample exactly.
#[test]
fn curve_eval_handles_degenerate_counts() {
    let Some(ctx) = try_init_gpu() else {
        eprintln!("[curve_eval_test] skipping: no GPU adapter available");
        return;
    };

    // count == 0: empty curve returns 1.0 regardless of t.
    let dispatches0 = [(0u32, 0.0f32), (0u32, 0.5f32), (0u32, 0.99f32)];
    let gpu0 = run_scalar_compute(&ctx, &[], &dispatches0);
    for (i, v) in gpu0.iter().enumerate() {
        assert_eq!(*v, 1.0, "count=0 dispatch {i}: expected 1.0, got {v}");
        assert!(v.is_finite());
    }

    // count == 1: returns the stored sample for any t.
    let single = [0.7331f32];
    let dispatches1 = [(1u32, 0.0f32), (1u32, 0.3f32), (1u32, 0.999f32)];
    let gpu1 = run_scalar_compute(&ctx, &single, &dispatches1);
    for (i, v) in gpu1.iter().enumerate() {
        assert!(
            approx_eq(*v, single[0], EPSILON),
            "count=1 dispatch {i}: expected {}, got {v}",
            single[0]
        );
        assert!(v.is_finite());
    }
}

/// RGB variant: three-channel GPU output matches the per-channel
/// `splines` reference. Smaller grid — the scalar test already proves the
/// polynomial; this confirms the RGB wrapper indexes channels correctly.
#[test]
fn curve_eval_rgb_matches_splines_reference() {
    let Some(ctx) = try_init_gpu() else {
        eprintln!("[curve_eval_test] skipping: no GPU adapter available");
        return;
    };

    let counts = [2u32, 3, 4, 8];
    let grid_ts = [0.0f32, 0.17, 0.5, 0.83, 0.99];
    let base = [0.1f32, 0.2, 0.3];

    for &count in &counts {
        let samples_rgb = authored_rgb(count);
        // Flatten to a contiguous f32 stream matching the WGSL stride-3 layout.
        let flat: Vec<f32> = samples_rgb.iter().flat_map(|c| c.iter().copied()).collect();

        let dispatches: Vec<(u32, f32, [f32; 3])> =
            grid_ts.iter().map(|&t| (count, t, base)).collect();
        let gpu = run_color_compute(&ctx, &flat, &dispatches);

        for (i, &(_, t, _)) in dispatches.iter().enumerate() {
            let got = gpu[i];
            for (c, gv) in got.iter().enumerate() {
                assert!(
                    gv.is_finite(),
                    "non-finite rgb output count={count} t={t} channel={c}: {gv}"
                );
            }
            let expected = catmull_rom_reference_rgb(&samples_rgb, t);
            for c in 0..3 {
                assert!(
                    approx_eq(got[c], expected[c], EPSILON),
                    "count={count} t={t} channel={c}: gpu={} reference={} (delta {})",
                    got[c],
                    expected[c],
                    (got[c] - expected[c]).abs()
                );
            }
        }
    }

    // count == 0: empty curve returns `base_color` exactly (within EPSILON),
    // regardless of t. `catmull_rom_reference` is undefined for n < 2 — assert
    // against the fallback directly.
    let base0 = [0.42f32, 0.37, 0.91];
    let dispatches0: Vec<(u32, f32, [f32; 3])> = [0.0f32, 0.5, 0.99]
        .iter()
        .map(|&t| (0u32, t, base0))
        .collect();
    let gpu0 = run_color_compute(&ctx, &[], &dispatches0);
    for (i, got) in gpu0.iter().enumerate() {
        for c in 0..3 {
            assert!(
                approx_eq(got[c], base0[c], EPSILON),
                "count=0 dispatch {i} channel={c}: gpu={} expected base={}",
                got[c],
                base0[c]
            );
            assert!(got[c].is_finite());
        }
    }

    // count == 1: single-sample curve returns the stored RGB triplet (within
    // EPSILON) for any t, ignoring `base_color`.
    let single = [0.13f32, 0.57, 0.89];
    let flat1: Vec<f32> = single.to_vec();
    let dispatches1: Vec<(u32, f32, [f32; 3])> = [0.0f32, 0.3, 0.999]
        .iter()
        .map(|&t| (1u32, t, base0))
        .collect();
    let gpu1 = run_color_compute(&ctx, &flat1, &dispatches1);
    for (i, got) in gpu1.iter().enumerate() {
        for c in 0..3 {
            assert!(
                approx_eq(got[c], single[c], EPSILON),
                "count=1 dispatch {i} channel={c}: gpu={} expected sample={}",
                got[c],
                single[c]
            );
            assert!(got[c].is_finite());
        }
    }
}
