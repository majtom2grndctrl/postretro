//! Real campaign manifest/read/drain/scene path for streamed compose writers.
use super::*;

#[test]
#[cfg(feature = "dev-tools")]
#[ignore = "requires compiled campaign-test.prl, streamed SH and all-resident lightmaps"]
fn real_map_streamed_scene_stages_every_indirect_promotion_and_animated_writer() {
    use crate::render::sh_streaming::ShResidencySnapshot;
    use postretro_level_loader::PreparedShCluster;
    use postretro_render_cpu::frame_uniforms::LightTermMask;
    let Some(mut renderer) = renderer() else {
        return;
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    let map = root.join("maps/campaign-test.prl");
    assert!(
        map.is_file(),
        "build campaign-test.prl before running this ignored proof"
    );
    let mut world = postretro_level_loader::load_prl(map.to_str().unwrap()).unwrap();
    let manifest = world
        .sh_stream_manifest()
        .expect("run with POSTRETRO_SH_STREAMING=async or sync-proof")
        .clone();
    assert!(
        world.lightmap_stream_manifest().is_none(),
        "run with POSTRETRO_LIGHTMAP_STREAMING=all-resident"
    );
    assert!(
        manifest.sources().indirect_delta.is_some(),
        "fixture needs indirect animated deltas"
    );
    assert!(
        manifest.sources().direct_delta.is_some(),
        "fixture needs static promotion deltas"
    );
    assert!(
        manifest.sources().animated_direct_delta.is_some(),
        "fixture needs animated direct deltas"
    );
    assert!(
        manifest.base().animation_descriptors.len() > 1,
        "alternate actual animated overrides need two descriptor slots"
    );
    let materials: Vec<_> = world
        .texture_names
        .iter()
        .map(|_| postretro_render_data::material::Material::Default)
        .collect();
    renderer.install_textures(
        &world.texture_names,
        &world.texture_cache_keys,
        &root.join("cache"),
        &materials,
    );
    renderer.normalize_world_uvs(&mut world);
    let lighting = world.take_gpu_lighting_payloads();
    renderer.install_level_geometry(
        &crate::render::level_world_to_geometry(&world, &materials),
        lighting,
    );
    assert!(renderer.sh_residency_snapshot().is_some());

    // The actual loader reads each encoded chunk positionally and validates
    // its complete payload. Deferred owner/retirement admissions are retried
    // through the same public renderer boundary; no synthetic pool is used.
    let cluster_count = manifest.cluster_count();
    assert!(cluster_count > 0);
    let mut targets = vec![u64::MAX; cluster_count.div_ceil(64) as usize];
    if cluster_count % 64 != 0 {
        *targets.last_mut().unwrap() = (1u64 << (cluster_count % 64)) - 1;
    }
    let mut ready: Vec<_> = (0..cluster_count)
        .map(|cluster_id| PreparedShCluster {
            generation: 1,
            content_tag: manifest.content_tag(),
            chunk: manifest
                .read_and_decode_cluster(cluster_id)
                .expect("real positional campaign SH chunk"),
        })
        .collect();
    let mut installed = 0usize;
    let mut reset = Some(targets);
    renderer.set_force_full_resident_sh_compose(true);
    let mut font = postretro_ui::text::build_font_system();
    for round in 0..cluster_count.saturating_mul(4) + 8 {
        let outcome = renderer
            .drain_sh_residency(ShDrainBatch {
                generation: 1,
                content_tag: manifest.content_tag(),
                target_reset: reset.take(),
                ready,
                ..Default::default()
            })
            .expect("real campaign SH drain");
        assert!(
            outcome.dropped.is_empty(),
            "valid targeted campaign chunks must not be dropped"
        );
        installed += outcome.accepted.len();
        ready = outcome.deferred;
        // A dependent requires its canonical owners to be sampleable, not
        // merely installed. Record the real compose work before the next
        // drain promotes those owners and admits their dependent chunks.
        renderer.update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, round as f32);
        renderer.update_viewmodel_view_projection(1.0, Mat4::IDENTITY);
        let encoder = record_window(&mut renderer, &mut font);
        renderer.submit_windowed_frame(encoder);
        renderer
            .queue
            .assert_empty("streamed preload frame complete");
        // Waiting belongs to test preload, outside measured frames. It makes
        // completed growth generations available to the next real drain.
        renderer
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        if ready.is_empty() {
            break;
        }
        assert!(
            round + 1 < cluster_count.saturating_mul(4) + 8,
            "campaign preload stalled: {installed} installed, {} deferred, {} sampleable",
            ready.len(),
            renderer
                .sh_residency_snapshot()
                .unwrap()
                .sampleable_clusters,
        );
    }
    // Publish the last admitted owners after their scene compose completed.
    renderer
        .drain_sh_residency(ShDrainBatch {
            generation: 1,
            content_tag: manifest.content_tag(),
            ..Default::default()
        })
        .expect("publish the final composed campaign clusters");
    assert_eq!(installed, cluster_count as usize);
    assert_eq!(
        renderer.sh_residency_snapshot().unwrap().installed_clusters,
        cluster_count as usize
    );
    assert_eq!(
        renderer
            .sh_residency_snapshot()
            .unwrap()
            .sampleable_clusters,
        cluster_count as usize
    );
    renderer
        .queue
        .assert_empty("streamed real-map preload complete");

    let mut baseline = Vec::new();
    for frame in 0..4 {
        if frame == 2 {
            baseline = renderer
                .queue
                .writer_counts()
                .iter()
                .map(|writer| (writer.site.file(), writer.site.line(), writer.writes))
                .collect();
        }
        let before = renderer.queue.counts();
        let mut mask = LightTermMask::ALL;
        mask.set_enabled(LightTermMask::SPECULAR, frame % 2 != 0);
        renderer.set_light_term_mask(mask);
        // These CPU-only descriptors are consumed by the uniform update.
        for slot in 0..renderer.animated_compose_descriptor_count() as usize {
            renderer.set_animated_light_active(slot, true);
        }
        renderer.update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, frame as f32 + 1.0);
        renderer.set_direct_sh_debug_override(crate::render::DirectShDebugOverride {
            enabled: true,
            selection_index: 0,
            weight: if frame % 2 == 0 { 0.25 } else { 0.75 },
        });
        renderer.set_animated_direct_sh_debug_override(
            crate::render::AnimatedDirectShDebugOverride {
                enabled: true,
                light_index: frame % 2,
            },
        );
        renderer.update_viewmodel_view_projection(1.0, Mat4::IDENTITY);
        let encoder = record_window(&mut renderer, &mut font);
        let ShResidencySnapshot {
            indirect_compose,
            static_direct_compose,
            animated_direct_compose,
            ..
        } = renderer.sh_residency_snapshot().unwrap();
        for (pass, diagnostics) in [
            ("indirect", indirect_compose),
            ("static promotion", static_direct_compose),
            ("animated direct", animated_direct_compose),
        ] {
            assert!(
                diagnostics.rows_composed > 0 && diagnostics.dispatches > 0,
                "real streamed {pass} scene pass must dispatch"
            );
        }
        renderer.submit_windowed_frame(encoder);
        assert_submit(&renderer, before, 1);
    }
    for writer in renderer.queue.writer_counts().iter() {
        let delta = writer.writes
            - baseline
                .iter()
                .find(|(path, line, _)| *path == writer.site.file() && *line == writer.site.line())
                .map_or(0, |(_, _, count)| *count);
        if delta > 0 {
            eprintln!(
                "[StreamedUploadInventory] {}:{} staged={delta}",
                writer.site.file(),
                writer.site.line()
            );
        }
    }
    let promotion = include_str!("../../sh_streaming/direct_compose/passes.rs");
    let expected = [
        (
            "sh_streaming/gpu/indirect/runtime.rs",
            include_str!("../../sh_streaming/gpu/indirect/runtime.rs"),
            "&self.grid_buffer",
        ),
        (
            "sh_streaming/direct_compose/passes.rs",
            promotion,
            "&self.light_term_mask",
        ),
        (
            "sh_streaming/direct_compose/passes.rs",
            promotion,
            "&self.debug_override",
        ),
        (
            "sh_streaming/direct_compose/passes.rs",
            promotion,
            "grid_buffer, 0, &upload.bytes",
        ),
        (
            "sh_streaming/direct_compose/passes/animated_runtime.rs",
            include_str!("../../sh_streaming/direct_compose/passes/animated_runtime.rs"),
            "&self.light_scale",
        ),
    ];
    for (file, source, target) in expected {
        inventory::require_site(&renderer, &baseline, file, source, target, 0);
    }
    let shared_grid_calls = site_writes(
        &renderer,
        "sh_streaming/direct_compose/passes.rs",
        promotion,
        "queue.write_buffer(grid_buffer",
    );
    let shared_grid_line = promotion
        .lines()
        .position(|line| line.contains("queue.write_buffer(grid_buffer"))
        .unwrap() as u32
        + 1;
    let before_grid_calls: u64 = baseline
        .iter()
        .filter(|(file, line, _)| {
            crate::render::uploads::site_file_ends_with(
                file,
                "sh_streaming/direct_compose/passes.rs",
            ) && *line == shared_grid_line
        })
        .map(|(_, _, count)| *count)
        .sum();
    assert_eq!(
        shared_grid_calls - before_grid_calls,
        4,
        "each counted scene dispatches the separate promotion and animated grid uploads"
    );
    eprintln!(
        "[UploadProof] real-map streamed inventory: 2 steady-state adapter cases, all 5 writer sites and both shared grid callers ran"
    );
}
