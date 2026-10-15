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
            corners: 1,
            dependencies: 1,
            incarnation: 1,
            valid: true,
        },
        rgb: [1.0, 2.0, 3.0],
    });
    readback
        .slot_incarnations
        .insert(7, SlotIncarnation::new(1));
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
fn streamed_reduction_preserves_unaffected_colors_and_rejects_reused_slots() {
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
    assert!(
        state.probe_readback.identities.is_empty(),
        "completed maps retire diagnostic identity work"
    );
    let rgb = state
        .probe_irradiance(0)
        .expect("completed live irradiance");
    for (actual, expected) in rgb.into_iter().zip([10.5, 2.5, 0.0]) {
        assert!(
            (actual - expected).abs() < 1.0e-6,
            "physical 8x8 origin, logical 6x6 interior: {rgb:?}"
        );
    }
    // Regression: even a pending promotion that cannot yet complete used to
    // clear every marker on every stationary-camera drain.
    assert!(!fixture.state.pending_promotion.is_empty());
    for _ in 0..3 {
        fixture.drain_many(&ctx.device, &queue, &[]).unwrap();
        let actual = fixture.state.probe_irradiance(0).unwrap();
        assert!(
            actual
                .into_iter()
                .zip(rgb)
                .all(|(a, b)| (a - b).abs() < 1.0e-6)
        );
    }
    // A redundant ready completion is dropped without affecting the sample.
    assert_eq!(
        fixture
            .drain_many(&ctx.device, &queue, &[0])
            .unwrap()
            .dropped,
        [0]
    );
    assert!(fixture.state.probe_irradiance(0).is_some());
    let state = &mut fixture.state;
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
        state.probe_irradiance(0).is_some(),
        "unrelated install and append-only pool growth must preserve an unaffected probe color"
    );
    for (actual, expected) in state.probe_irradiance(0).unwrap().into_iter().zip(rgb) {
        assert!(
            (actual - expected).abs() < 1.0e-6,
            "unchanged irradiance survives growth"
        );
    }
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
    // Retire the real owning cluster and reinstall it in the same drain.
    // The first-fit allocator reuses the same word, but the pending map must
    // not publish the previous occupant's color after that ABA transition.
    let retained_word = state.sampled_words[0];
    fixture.evict_and_reinstall(&ctx.device, &queue, 1);
    let state = &mut fixture.state;
    state.sampled_words[0] = retained_word;
    state.probe_readback.sample_words_changed();
    state
        .probe_readback
        .reconcile(state.grid_dimensions, &state.sampled_words);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    state.post_submit_probe_readback(&ctx.device);
    assert!(
        state.probe_irradiance(0).is_none(),
        "reused slot rejects pending and cached foreign color"
    );
    // Teardown may still dispatch a callback; it retains no renderer handles.
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
        "[ProbeReadbackProof] storage→sampled reduction, no-op/deferred drains, pool growth, ABA rejection and pending-map teardown ran"
    );
}

#[test]
fn adaptive_corner_changes_invalidate_only_dependent_cached_and_pending_colors() {
    // Regression: a target word can remain identical while a sampled L1 corner
    // disappears, changing the normalized reconstruction of that marker.
    let dims = [8, 8, 8];
    let mut words = vec![word(1, 1, 9); 512];
    words[510] = word(0, 0, 30);
    let dense = 1 + 8 + 64;
    let dependent = Identity {
        dense,
        word: words[dense],
        corners: corner_mask(&probe_request(dense, dims, &words).unwrap()),
        dependencies: probe_request_with_dependencies(dense, dims, &words)
            .unwrap()
            .1,
        incarnation: 1,
        valid: true,
    };
    let unaffected = Identity {
        dense: 510,
        word: words[510],
        corners: 1,
        dependencies: 1,
        incarnation: 2,
        valid: true,
    };
    let mut readback = StreamedProbeReadback {
        wanted: true,
        mapping: true,
        ..Default::default()
    };
    readback
        .slot_incarnations
        .extend([(9, SlotIncarnation::new(1)), (30, SlotIncarnation::new(2))]);
    readback.identities = vec![dependent, unaffected];
    readback.colors = vec![
        ProbeColor {
            identity: dependent,
            rgb: [1.0; 3],
        },
        ProbeColor {
            identity: unaffected,
            rgb: [2.0; 3],
        },
    ];
    let corner = node_corner_coord([0, 0, 0], 1, 7).unwrap();
    let index = (corner[0] + corner[1] * 8 + corner[2] * 64) as usize;
    words[index] = 0;
    readback.sample_words_changed();
    readback.reconcile(dims, &words);
    assert!(readback.color(dense, words[dense]).is_none());
    assert!(readback.color(510, words[510]).is_some());
    assert!(!readback.identities[0].valid);
    assert!(readback.identities[1].valid);
    words[index] = word(1, 1, 9);
    readback.sample_words_changed();
    readback.reconcile(dims, &words);
    assert!(
        !readback.identities[0].valid,
        "a rejected pending reconstruction cannot revive"
    );
}

#[test]
fn slot_incarnation_tracking_stays_bounded_across_overlay_traversal() {
    // Regression: safety bookkeeping must track only cached/in-flight slots,
    // not accumulate one entry for every slot encountered over the level.
    let count = 1024;
    let mut readback = StreamedProbeReadback::default();
    for step in 0..100 {
        readback.identities = (0..count)
            .map(|dense| Identity {
                dense,
                word: word(0, 0, (step * count + dense) as u32),
                corners: 1,
                dependencies: 1,
                incarnation: 0,
                valid: true,
            })
            .collect();
        readback.capture_incarnations();
        assert!(readback.slot_incarnations.len() <= 2 * count);
        readback.colors = readback
            .identities
            .iter()
            .copied()
            .map(|identity| ProbeColor {
                identity,
                rgb: [1.0; 3],
            })
            .collect();
    }
}

#[test]
#[ignore = "manual marker-cache CPU measurement at representative and maximum diagnostic budgets"]
fn measure_marker_cache_capture_and_reconciliation_cost() {
    for (level, count) in [(0, 4096), (0, MAX_PROBES), (1, 4096), (1, MAX_PROBES)] {
        let dims = [48, 48, 48];
        let words: Vec<_> = (0..48 * 48 * 48)
            .map(|dense| {
                let x = dense % 48;
                let y = dense / 48 % 48;
                let z = dense / (48 * 48);
                if level == 0 {
                    word(0, 0, dense as u32)
                } else {
                    word(1, 0, ((x / 4 + (y / 4) * 12 + (z / 4) * 144) * 8) as u32)
                }
            })
            .collect();
        let identities: Vec<_> = (0..count)
            .map(|dense| Identity {
                dense,
                word: words[dense],
                corners: corner_mask(&probe_request(dense, dims, &words).unwrap()),
                dependencies: probe_request_with_dependencies(dense, dims, &words)
                    .unwrap()
                    .1,
                incarnation: 0,
                valid: true,
            })
            .collect();
        let mut readback = StreamedProbeReadback {
            identities,
            wanted: true,
            mapping: true,
            ..Default::default()
        };
        readback.capture_incarnations();
        readback.colors = readback
            .identities
            .iter()
            .copied()
            .map(|identity| ProbeColor {
                identity,
                rgb: [1.0; 3],
            })
            .collect();
        let repeats = 20;
        let start = Instant::now();
        for _ in 0..repeats {
            readback.capture_incarnations();
        }
        let capture = start.elapsed().as_secs_f64() * 1000.0 / f64::from(repeats);
        let start = Instant::now();
        for _ in 0..repeats {
            readback.sample_words_changed();
            readback.reconcile(dims, &words);
        }
        let reconcile = start.elapsed().as_secs_f64() * 1000.0 / f64::from(repeats);
        assert_eq!(readback.colors.len(), count);
        eprintln!(
            "[ProbeCacheCpuProof] level={level} markers={count} capture_incarnations_ms={capture:.3} reconcile_cached_and_pending_ms={reconcile:.3}"
        );
    }
}

#[test]
fn disabling_markers_retires_idle_identities_and_skips_superseded_pending_work() {
    let identity = Identity {
        dense: 0,
        word: word(0, 0, 9),
        corners: 1,
        dependencies: 1,
        incarnation: 1,
        valid: true,
    };
    let mut readback = StreamedProbeReadback {
        wanted: true,
        identities: vec![identity],
        ..Default::default()
    };
    readback.set_overlay(false, Vec3::ZERO, 30.0);
    assert!(readback.identities.is_empty());
    readback.set_overlay(true, Vec3::ZERO, 30.0);
    readback.identities.push(identity);
    readback.mapping = true;
    readback.copied_epoch = readback.epoch;
    readback.set_overlay(false, Vec3::ZERO, 30.0);
    readback.sample_words_changed();
    readback.reconcile([1, 1, 1], &[0]);
    assert_eq!(
        readback.identities.len(),
        1,
        "pending decode retains only its range length"
    );
    assert!(
        readback.identities[0].valid,
        "superseded pending identity is never visited for reconciliation"
    );
}

#[test]
fn adaptive_corner_arrival_preserves_zero_weight_boundaries_and_node_retirement_rejects_all() {
    let dims = [8, 8, 8];
    let mut words = vec![word(1, 1, 9); 512];
    words[511] = 0;
    let identities: Vec<_> = [0, 1 + 8 + 64]
        .into_iter()
        .map(|dense| {
            let (request, dependencies) =
                probe_request_with_dependencies(dense, dims, &words).unwrap();
            Identity {
                dense,
                word: words[dense],
                corners: corner_mask(&request),
                dependencies,
                incarnation: 1,
                valid: true,
            }
        })
        .collect();
    let mut readback = StreamedProbeReadback {
        wanted: true,
        mapping: true,
        identities,
        ..Default::default()
    };
    readback
        .slot_incarnations
        .insert(9, SlotIncarnation::new(1));
    readback.colors = readback
        .identities
        .iter()
        .copied()
        .map(|identity| ProbeColor {
            identity,
            rgb: [1.0; 3],
        })
        .collect();
    words[511] = word(1, 1, 9);
    readback.sample_words_changed();
    readback.reconcile(dims, &words);
    assert!(
        readback.color(0, words[0]).is_some(),
        "a zero-weight arriving corner cannot change this marker"
    );
    assert!(readback.identities[0].valid);
    assert!(
        readback.color(73, words[73]).is_none(),
        "a contributing arriving corner changes the normalized request"
    );
    assert!(!readback.identities[1].valid);
    // The allocator reserves and releases the entire eight-slot L1 node.
    // Every reconstruction carries the base slot, including this boundary one.
    readback.retire_slots(9..17);
    readback.reconcile(dims, &words);
    assert!(readback.color(0, words[0]).is_none());
    assert!(!readback.identities[0].valid);
}

#[test]
fn probe_overlay_composes_its_sampling_region_without_forcing_unrelated_resident_rows() {
    use crate::render::ShSampleRegionSets;
    use postretro_render_cpu::frame_uniforms::LightTermMask;
    let map = super::super::install_tests::SyntheticMap::new([128, 8, 8], [1, 1, 1]);
    let mut state = map.state();
    state.indirect_resident_rows = (0..128).collect();
    let visible = postretro_visibility::VisibleCells::Culled(Default::default());
    let inputs = ShSampleRegionSets {
        visible_cells: &visible,
        fog_cells: &[],
        movers: &[],
    };
    let camera = Vec3::new(40.0, 3.0, 3.0);
    let prepare = |state: &mut ShResidencyState| {
        state
            .prepare_compose_frame(
                inputs,
                None,
                false,
                false,
                true,
                false,
                true,
                false,
                LightTermMask::ALL,
                crate::render::direct_sh_compose::DirectShDebugOverride::default(),
                crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride::default(),
                &[],
                &[],
            )
            .unwrap();
    };
    prepare(&mut state);
    assert!(state.compose_region_rows.is_empty());
    state.set_probe_overlay(true, camera, 1.0);
    prepare(&mut state);
    assert!(!state.compose_region_rows.is_empty());
    assert!(state.compose_region_rows.len() < 128);
    assert!(!state.compose_region_rows.contains(&0));
    for coord in crate::render::sh_diagnostics_grid::GridBounds::around_sphere(
        [128, 8, 8],
        Vec3::ZERO,
        Vec3::ONE,
        camera,
        1.0,
    )
    .coordinates()
    {
        if (Vec3::from(coord.map(|n| n as f32)) - camera).length_squared() > 1.0 {
            continue;
        }
        let row = coord[0] / 4 + (coord[1] / 4) * 32 + (coord[2] / 4) * 64;
        assert!(
            state.compose_region_rows.contains(&row),
            "marker row {row} must compose"
        );
    }
    state.set_probe_overlay(false, camera, 1.0);
    prepare(&mut state);
    assert!(state.compose_region_rows.is_empty());
}

#[test]
fn probe_overlay_composes_rounded_probe_positions_when_world_bounds_collapse() {
    use crate::render::ShSampleRegionSets;
    use postretro_render_cpu::frame_uniforms::LightTermMask;
    let map =
        super::super::install_tests::SyntheticMap::new([16, 4, 4], [1, 1, 1]).direct_base_only();
    let mut state = map.state();
    state.base_metadata.grid_origin = [1_000_000.0, 0.0, 0.0];
    state.base_metadata.cell_size = [0.01, 1.0, 1.0];
    state.sample_region_index = super::super::sample_region_index::SampleRegionIndex::build(
        &super::super::sample_regions::SampleRegionGrid {
            origin: state.base_metadata.grid_origin,
            cell_size: state.base_metadata.cell_size,
            dimensions: state.grid_dimensions,
            dense_nodes: &state.dense_node,
        },
        std::iter::empty(),
    )
    .expect("precision fixture has a valid L0 layout");
    for cluster in [1, 2] {
        state
            .install(None, &map.prepared(&state, cluster))
            .expect("install animated rows");
    }
    let camera = Vec3::new(1_000_000.0 + 0.0625, 1.0, 1.0);
    state.set_probe_overlay(true, camera, 0.01);
    let prepare =
        |state: &mut ShResidencyState, indirect_active| {
            let visible = postretro_visibility::VisibleCells::Culled(Default::default());
            state.prepare_compose_frame(
            ShSampleRegionSets { visible_cells: &visible, fog_cells: &[], movers: &[] },
            None, false, false, true, false, indirect_active, false,
            LightTermMask::ALL,
            crate::render::direct_sh_compose::DirectShDebugOverride::default(),
            crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride::default(),
            &[], &[],
        ).expect("plan precision fixture");
        };
    // Finish the initial residency repair, then inspect a steady frame whose
    // only sampling consumer is the overlay. No dirty-row bypass can hide a gap.
    prepare(&mut state, false);
    let plan = state
        .compose_frame_plan
        .take()
        .expect("initial compose plan");
    state
        .commit_indirect_compose(&plan.indirect, 1)
        .expect("commit initial repair");
    state
        .promote_completed(&mut super::super::StagedUploads::default())
        .expect("promote probes");
    prepare(&mut state, false);
    assert!(
        state
            .compose_frame_plan
            .as_ref()
            .expect("steady plan")
            .indirect
            .rows()
            .is_empty()
    );

    let mut requests = Vec::new();
    let mut identities = Vec::new();
    collect_requests(
        &state.base_metadata,
        &state.sampled_words,
        camera,
        0.01,
        &mut requests,
        &mut identities,
    );
    let selected_rows: std::collections::BTreeSet<_> = identities
        .iter()
        .map(|identity| {
            state
                .affinity_row_for_dense(identity.dense as u32)
                .expect("selected probe row")
        })
        .collect();
    assert_eq!(selected_rows, std::collections::BTreeSet::from([1, 2]));
    for row in selected_rows {
        assert!(
            state.compose_region_rows.contains(&row),
            "selected marker row {row} must be gated"
        );
    }
    // A later animation tick must repair row 2 as well as row 1 before a fresh
    // readback. Slot identities and residency remain unchanged between ticks.
    prepare(&mut state, true);
    assert_eq!(
        state
            .compose_frame_plan
            .as_ref()
            .expect("animation plan")
            .indirect
            .rows(),
        &[1, 2]
    );
}
