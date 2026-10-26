//! Renderer slider state through the existing serialized uniform uploads.
use super::*;

fn read_receiver_scales(renderer: &Renderer) -> [f32; 3] {
    let full = renderer.full();
    full.mesh_pass.write_light_params(
        &renderer.queue,
        full.total_light_count,
        full.light_count,
        full.light_count + full.animated_baked_light_count as u32,
        full.mesh_dynamic_time,
        full.frame_light_term_mask.bits(),
        full.ambient_floor,
        full.frame_indirect_scale,
    );
    full.kinematic_brush.write_light_params(
        &renderer.queue,
        full.total_light_count,
        full.light_count,
        full.light_count + full.animated_baked_light_count as u32,
        full.mesh_dynamic_time,
        full.frame_light_term_mask.bits(),
        full.ambient_floor,
        full.frame_indirect_scale,
    );
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    let frame = Probe::record(
        renderer,
        &mut encoder,
        &full.uniform_buffer,
        include_str!("../frame_readback.wgsl"),
        128,
    );
    let mover = Probe::record(
        renderer,
        &mut encoder,
        full.kinematic_brush.light_params_buffer_for_test(),
        include_str!("../indirect_scale_readback.wgsl"),
        32,
    );
    let mesh = Probe::record(
        renderer,
        &mut encoder,
        full.mesh_pass.light_params_buffer_for_test(),
        include_str!("../indirect_scale_readback.wgsl"),
        32,
    );
    renderer.queue.submit([encoder.finish()]);
    // These are shader-consumed bytes in the real buffers, not the Rust fields.
    [
        (frame.read(renderer), 92),
        (mover.read(renderer), 24),
        (mesh.read(renderer), 24),
    ]
    .map(|(bytes, offset)| f32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap()))
}

// Regression: movers and meshes read SH at full strength and could not follow
// the renderer slider. Reuse the same initialized passes across frame changes.
#[test]
#[ignore = "requires a GPU adapter; run explicitly for serialized uniform readback"]
fn indirect_scale_runtime_change_reaches_serialized_receiver_uploads_next_frame() {
    let mut renderer = renderer().expect("explicit indirect-scale readback requires a GPU adapter");
    renderer.set_indirect_scale(0.625);
    renderer.update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, 1.0);
    for value in read_receiver_scales(&renderer) {
        assert!((value - 0.625).abs() < f32::EPSILON);
    }

    // Diagnostics run after group 0 is uploaded: all receivers retain that
    // frame's scale until the next uniform update, including repeated uploads.
    renderer.set_indirect_scale(0.125);
    for value in read_receiver_scales(&renderer) {
        assert!((value - 0.625).abs() < f32::EPSILON);
    }
    renderer.update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, 2.0);
    for value in read_receiver_scales(&renderer) {
        assert!((value - 0.125).abs() < f32::EPSILON);
    }
    renderer
        .queue
        .assert_empty("indirect scale runtime proof exit");
    eprintln!("[UploadProof] indirect scale: 3 adapter cases ran");
}
