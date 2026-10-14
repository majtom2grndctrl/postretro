use super::*;
use crate::render::gpu_test_harness::bc_gpu_or_skip;
use crate::render::sh_streaming::gpu::UploadOrderSh;

fn word(level: u32, scale: u32, slot: u32) -> u32 {
    (slot << 5) | (scale << 3) | 4 | level
}

#[test]
fn adaptive_requests_follow_scaled_node_corners_and_renormalize_missing_corners() {
    // Regression: light-value markers must reconstruct the entire adaptive
    // node, including corners beyond the current four-probe affinity brick.
    let dims = [8, 8, 8];
    let mut words = vec![word(1, 1, 9); 512];
    let dense = 1 + 8 + 64;
    let request = probe_request(dense, dims, &words).unwrap();
    let expected = (0..8u8)
        .map(|corner| node_l1_corner_weight([1, 1, 1], corner, 1).unwrap() * f32::from(corner))
        .sum::<f32>();
    let value: f32 = request
        .weights
        .iter()
        .enumerate()
        .map(|(corner, weight)| weight * corner as f32)
        .sum();
    assert!((value - expected).abs() < 1.0e-6);
    assert_eq!(request.slots, [9, 10, 11, 12, 13, 14, 15, 16]);
    let last = node_corner_coord([0, 0, 0], 1, 7).unwrap();
    let last_dense = (last[0] + last[1] * 8 + last[2] * 64) as usize;
    words[last_dense] = 0;
    let request = probe_request(dense, dims, &words).unwrap();
    assert!(request.weights[7].abs() < 1.0e-6);
    assert!((request.weights.iter().sum::<f32>() - 1.0).abs() < 1.0e-6);
    for level in [0, 2] {
        let request = probe_request(0, [1, 1, 1], &[word(level, 0, 23)]).unwrap();
        assert_eq!(request.slots[0], 23);
        assert!((request.weights[0] - 1.0).abs() < 1.0e-6);
    }
    assert!(probe_request(0, [1, 1, 1], &[0]).is_none());
}

#[test]
fn readback_cadence_pending_work_and_reused_slots_never_publish_foreign_colors() {
    let start = Instant::now();
    let mut readback = StreamedProbeReadback::default();
    readback.set_overlay(true, Vec3::ZERO, 30.0);
    assert!(readback.wants_copy(start));
    readback.last_copy = Some(start);
    assert!(!readback.wants_copy(start + Duration::from_millis(99)));
    assert!(readback.wants_copy(start + READBACK_INTERVAL));
    readback.mapping = true;
    assert!(!readback.wants_copy(start + Duration::from_secs(1)));
    readback.mapping = false;
    readback.copied = true;
    assert!(!readback.wants_copy(start + Duration::from_secs(1)));
    readback.colors.push(ProbeColor {
        identity: Identity {
            dense: 1,
            word: word(0, 0, 7),
        },
        rgb: [1.0, 2.0, 3.0],
    });
    assert!(readback.color(1, word(0, 0, 7)).is_some());
    assert!(readback.color(1, 0).is_none());
    assert!(readback.color(1, word(0, 0, 8)).is_none());
    readback.copied_epoch = readback.epoch;
    readback.invalidate();
    assert_ne!(readback.copied_epoch, readback.epoch);
    assert!(
        readback.color(1, word(0, 0, 7)).is_none(),
        "even an ABA slot reuse must invalidate its old color"
    );
    readback.set_overlay(false, Vec3::ZERO, 30.0);
    assert!(!readback.wanted());
}

#[test]
fn request_collection_obeys_the_marker_radius_and_segment_budget() {
    let base = super::super::install_tests::SyntheticMap::new([4, 4, 4], [1, 1, 1]).base();
    let words = vec![word(0, 0, 1); 64];
    let mut requests = Vec::new();
    let mut identities = Vec::new();
    collect_requests(
        &base,
        &words,
        Vec3::ZERO,
        0.5,
        &mut requests,
        &mut identities,
    );
    assert_eq!(identities.len(), 1);
    assert_eq!(identities[0].dense, 0);
    assert_eq!(requests.len(), 1);
    let mut large = base;
    large.grid_dimensions = [(MAX_PROBES + 100) as u32, 1, 1];
    let words = vec![word(0, 0, 1); MAX_PROBES + 100];
    collect_requests(
        &large,
        &words,
        Vec3::ZERO,
        f32::MAX,
        &mut requests,
        &mut identities,
    );
    assert_eq!(requests.len(), MAX_PROBES);
    assert_eq!(identities.len(), MAX_PROBES);
}

#[test]
fn streamed_reduction_reads_storage_written_tiles_with_physical_stride_and_discards_stale_maps() {
    let Some(ctx) = bc_gpu_or_skip("streamed probe irradiance") else {
        return;
    };
    let queue = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let mut fixture = UploadOrderSh::new(&ctx.device, &ctx.queue, 64);
    fixture.drain(&ctx.device, &queue, 0);
    let state = &mut fixture.state;
    state.base_metadata.tile_dimension = 6;
    state.base_metadata.tile_border = 1;
    state.sampled_words[0] = word(0, 0, 1);
    state.set_probe_overlay(true, Vec3::ZERO, 100.0);
    let pool = state.gpu.as_ref().unwrap();
    let module = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Diagnostic storage gradient"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../shaders/sh_probe_readback_test.wgsl").into(),
            ),
        });
    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Diagnostic storage gradient"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    let bindings = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Diagnostic storage gradient"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&pool.total_storage_view),
        }],
    });
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bindings, &[]);
        let extent = pool.shape.extent();
        pass.dispatch_workgroups(
            extent.width.div_ceil(8),
            extent.height.div_ceil(8),
            extent.depth_or_array_layers,
        );
    }
    state.encode_probe_readback(&ctx.device, &queue, &mut encoder);
    let gpu = state.probe_readback.gpu.as_ref().unwrap();
    assert!(
        gpu.requests.size() + gpu.colors.size() + gpu.params.size() + gpu.readback.size()
            <= 8 * 1024 * 1024,
        "diagnostic GPU buffers stay within 8 MiB regardless of physical pool size"
    );
    queue.submit([encoder.finish()]);
    state.post_submit_probe_readback(&ctx.device);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    state.post_submit_probe_readback(&ctx.device);
    let rgb = state
        .probe_irradiance(0)
        .expect("completed live irradiance");
    for (actual, expected) in rgb.into_iter().zip([10.5, 2.5, 0.0]) {
        assert!(
            (actual - expected).abs() < 1.0e-6,
            "physical 8x8 origin, logical 6x6 interior: {rgb:?}"
        );
    }
    state.probe_readback.last_copy = None;
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    state.encode_probe_readback(&ctx.device, &queue, &mut encoder);
    queue.submit([encoder.finish()]);
    state.post_submit_probe_readback(&ctx.device);
    let old_layers = state.gpu.as_ref().unwrap().shape.layers;
    // A real install grows the physical pool while the old map is in flight.
    fixture.drain(&ctx.device, &queue, 1);
    let state = &mut fixture.state;
    assert!(state.gpu.as_ref().unwrap().shape.layers > old_layers);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    state.post_submit_probe_readback(&ctx.device);
    assert!(
        state.probe_irradiance(0).is_none(),
        "an in-flight result from the previous residency cannot land"
    );
    let pool = state.gpu.as_ref().unwrap();
    state.sampled_words[0] = word(0, 0, pool.shape.tiles_per_layer);
    let bindings = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Diagnostic storage gradient after pool growth"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&pool.total_storage_view),
        }],
    });
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bindings, &[]);
        let extent = pool.shape.extent();
        pass.dispatch_workgroups(
            extent.width.div_ceil(8),
            extent.height.div_ceil(8),
            extent.depth_or_array_layers,
        );
    }
    state.probe_readback.last_copy = None;
    state.encode_probe_readback(&ctx.device, &queue, &mut encoder);
    queue.submit([encoder.finish()]);
    state.post_submit_probe_readback(&ctx.device);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    state.post_submit_probe_readback(&ctx.device);
    let rgb = state.probe_irradiance(0).unwrap();
    for (actual, expected) in rgb.into_iter().zip([2.5, 2.5, 1.0]) {
        assert!(
            (actual - expected).abs() < 1.0e-6,
            "new array layer after pool growth: {rgb:?}"
        );
    }
    state.probe_readback.last_copy = None;
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    state.encode_probe_readback(&ctx.device, &queue, &mut encoder);
    queue.submit([encoder.finish()]);
    state.post_submit_probe_readback(&ctx.device);
    drop(fixture);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    eprintln!(
        "[ProbeReadbackProof] storage→sampled reduction, pool growth, stale-map rejection and pending-map teardown ran"
    );
}
