//! Actual renderer patch and bridge-transaction GPU proofs.
use super::*;

#[test]
fn actual_light_count_patch_survives_the_preceding_full_uniform_write() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let before = renderer.queue.counts();
    // The full upload contains a distinguishable stale count. The slot updater
    // recomputes the actual zero-light scene and patches its count afterward.
    renderer.full_mut().total_light_count = 17;
    renderer.update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, 1.0);
    renderer
        .update_dynamic_light_slots_with_capture_overrides(
            Vec3::ZERO,
            0.1,
            &[],
            &[],
            &[],
            1.0,
            None,
            &[],
        )
        .unwrap();
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    let probe = Probe::record(
        &renderer,
        &mut encoder,
        &renderer.full().uniform_buffer,
        include_str!("frame_readback.wgsl"),
        128,
    );
    renderer.queue.submit([encoder.finish()]);
    assert_submit(&renderer, before, 1);
    assert_eq!(&probe.read(&renderer)[120..124], &0u32.to_ne_bytes());
    assert!(
        site_writes(
            &renderer,
            "renderer_light_slots.rs",
            include_str!("../renderer_light_slots.rs"),
            "queue.write_buffer("
        ) > 0
    );
    eprintln!("[UploadProof] actual light-count patch: 1 adapter case ran");
}

fn install_spot_state(renderer: &mut Renderer) {
    let light = postretro_level_loader::MapLight {
        origin: [0.0, 0.0, 0.0],
        light_type: postretro_level_loader::LightType::Spot,
        intensity: 1.0,
        color: [0.25, 0.5, 0.75],
        falloff_model: postretro_level_loader::FalloffModel::Linear,
        falloff_range: 20.0,
        cone_angle_inner: 0.3,
        cone_angle_outer: 0.4,
        cone_direction: [0.0, 0.0, -1.0],
        is_dynamic: true,
        casts_entity_shadows: true,
        animated_slot: None,
        tags: vec![],
        cell_index: 0,
        shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
    };
    let influence = postretro_render_data::influence::LightInfluence {
        center: Vec3::ZERO,
        radius: 20.0,
    };
    let full = renderer.full_mut();
    full.level_lights = vec![light.clone()];
    full.level_light_source_indices = vec![0];
    full.level_light_influences = vec![influence.clone()];
    full.shadow_candidate_lights = vec![light];
    full.shadow_candidate_source_indices = vec![0];
    full.shadow_candidate_influences = vec![influence];
    full.shadow_candidate_selection_indices = vec![None];
    full.shadow_candidate_animated_baked_indices = vec![None];
}
fn commit_spot_bridge(renderer: &mut Renderer) -> Vec<u8> {
    let lights = postretro_lighting::pack_lights(&renderer.full().level_lights);
    let influence =
        postretro_lighting::influence::pack_influence(&renderer.full().level_light_influences);
    assert!(renderer.upload_light_bridge_snapshot(
        &lights,
        &influence,
        &[0; 48],
        &[],
        &[1.0],
        &[],
        &[]
    ));
    lights
}

#[test]
fn actual_shadow_slot_patch_preserves_the_bridges_base_light_bytes() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_spot_state(&mut renderer);
    let before = renderer.queue.counts();
    let bridge = commit_spot_bridge(&mut renderer);
    assert_eq!(
        &bridge[56..60],
        &postretro_lighting::NO_SHADOW_SLOT.to_ne_bytes()
    );
    renderer.update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, 1.0);
    renderer
        .update_dynamic_light_slots_with_capture_overrides(
            Vec3::new(0.0, 0.0, 2.0),
            0.1,
            &[1.0],
            &[],
            &[],
            1.0,
            None,
            &[],
        )
        .unwrap();
    let slot = renderer.full().spot_shadow_pool.slot_assignment[0];
    assert_ne!(
        slot,
        postretro_lighting::NO_SHADOW_SLOT,
        "real spot ranker must assign a shadow slot"
    );
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    let probe = Probe::record(
        &renderer,
        &mut encoder,
        &renderer.full().lights_buffer,
        include_str!("light_readback.wgsl"),
        64,
    );
    renderer.queue.submit([encoder.finish()]);
    assert_submit(&renderer, before, 1);
    let actual = probe.read(&renderer);
    assert_eq!(
        &actual[..56],
        &bridge[..56],
        "the slot patch retains animated/base light bytes"
    );
    assert_eq!(&actual[56..60], &slot.to_ne_bytes());
    assert!(
        renderer.queue.counts().writes - before.writes >= 5,
        "bridge, uniforms and real slot writers staged"
    );
    eprintln!("[UploadProof] actual bridge/slot patch: 1 adapter case ran");
}

#[test]
fn rejected_bridge_snapshot_reports_no_commit_and_retains_the_gpu_snapshot() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_spot_state(&mut renderer);
    let before = renderer.queue.counts();
    let committed = commit_spot_bridge(&mut renderer);
    let before_rejection = renderer.queue.counts();
    let lights_mirror = renderer.full().last_lights_upload.clone();
    let influences_mirror = renderer.full().last_influence_upload.clone();
    assert!(!renderer.upload_light_bridge_snapshot(
        &[0; 1],
        &[0; 16],
        &[0; 48],
        &[],
        &[1.0],
        &[],
        &[]
    ));
    assert_eq!(renderer.queue.counts().writes, before_rejection.writes);
    assert_eq!(
        renderer.queue.counts().direct_writes,
        before_rejection.direct_writes
    );
    assert_eq!(renderer.full().last_lights_upload, lights_mirror);
    assert_eq!(renderer.full().last_influence_upload, influences_mirror);
    assert_eq!(renderer.full().light_count, 1);
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    let probe = Probe::record(
        &renderer,
        &mut encoder,
        &renderer.full().lights_buffer,
        include_str!("light_readback.wgsl"),
        64,
    );
    renderer.queue.submit([encoder.finish()]);
    assert_submit(&renderer, before, 1);
    assert_eq!(probe.read(&renderer), committed);
    eprintln!("[UploadProof] rejected bridge retains committed GPU snapshot: 1 adapter case ran");
}
