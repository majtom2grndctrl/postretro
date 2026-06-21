// Renderer unit tests (split from the original `mod tests`).
// See: context/lib/testing_guide.md

use super::super::*;

#[test]
fn uniform_data_has_correct_size() {
    let data = build_uniform_data(&FrameUniforms {
        view_proj: Mat4::IDENTITY,
        camera_position: Vec3::ZERO,
        ambient_floor: 0.05,
        light_count: 0,
        time: 0.0,
        lighting_isolation: LightingIsolation::Normal,
        indirect_scale: 1.0,
        sdf_shadow_flags: 0,
        sdf_shadow_mode: SdfShadowMode::On,
        sdf_force_visibility_one: false,
        dynamic_direct_scale: DEFAULT_DYNAMIC_DIRECT_SCALE,
        dynamic_direct_isolation: DynamicDirectIsolation::Combined,
        has_direct: false,
    });
    assert_eq!(data.len(), UNIFORM_SIZE);
}

/// `sdf_shadow_flags` packs to bytes 96..100 — confirm the bitset round-trips.
#[test]
fn uniform_data_encodes_sdf_shadow_flags_at_correct_offset() {
    let data = build_uniform_data(&FrameUniforms {
        view_proj: Mat4::IDENTITY,
        camera_position: Vec3::ZERO,
        ambient_floor: 0.0,
        light_count: 0,
        time: 0.0,
        lighting_isolation: LightingIsolation::Normal,
        indirect_scale: 1.0,
        sdf_shadow_flags: SDF_SHADOW_FLAG_ATLAS_PRESENT,
        sdf_shadow_mode: SdfShadowMode::On,
        sdf_force_visibility_one: false,
        dynamic_direct_scale: 0.0,
        dynamic_direct_isolation: DynamicDirectIsolation::Combined,
        has_direct: false,
    });
    let flags = u32::from_ne_bytes(data[96..100].try_into().unwrap());
    assert_eq!(flags, SDF_SHADOW_FLAG_ATLAS_PRESENT);
    // `sdf_shadow_mode` at 100..104 — `On` encodes to 0;
    // `sdf_force_visibility_one` at 104..108 (false ⇒ 0). The dynamic-direct
    // tail (108..120) is zero here (scale 0, Combined=0, has_direct=false),
    // and the trailing pad 120..128 stays zero.
    assert_eq!(
        u32::from_ne_bytes(data[100..104].try_into().unwrap()),
        SdfShadowMode::On as u32,
    );
    assert!(data[104..128].iter().all(|&b| b == 0));
}

/// sdf-per-light-shadows Task 3: the dev "force visibility 1.0" toggle
/// packs as a u32 at offset 104..108 (non-zero ⇒ forced) and leaves the
/// trailing pad 108..112 zero. Guards the CPU↔WGSL uniform layout drift
/// for the new field.
#[test]
fn uniform_data_encodes_sdf_force_visibility_one_at_correct_offset() {
    for (force, expected) in [(false, 0u32), (true, 1u32)] {
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
            ambient_floor: 0.0,
            light_count: 0,
            time: 0.0,
            lighting_isolation: LightingIsolation::Normal,
            indirect_scale: 1.0,
            sdf_shadow_flags: 0,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: force,
            dynamic_direct_scale: 0.0,
            dynamic_direct_isolation: DynamicDirectIsolation::Combined,
            has_direct: false,
        });
        assert_eq!(
            u32::from_ne_bytes(data[104..108].try_into().unwrap()),
            expected,
            "sdf_force_visibility_one={force} should encode to {expected} at 104..108",
        );
        assert!(
            data[120..128].iter().all(|&b| b == 0),
            "tail pad 120..128 must stay zero for force={force}",
        );
    }
}

/// Task 6 of `sdf-static-occluder-shadows`: the `SdfShadowMode` selector
/// must round-trip through the `FrameUniforms` byte packer — every
/// variant encodes to its `u32` repr at offset 100..104 with the
/// trailing pad bytes zeroed. Mirrors
/// `uniform_data_encodes_sdf_shadow_flags_at_correct_offset`.
#[test]
fn sdf_shadow_mode_round_trips_through_uniform() {
    for mode in SdfShadowMode::ALL_VARIANTS {
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
            ambient_floor: 0.0,
            light_count: 0,
            time: 0.0,
            lighting_isolation: LightingIsolation::Normal,
            indirect_scale: 1.0,
            sdf_shadow_flags: 0,
            sdf_shadow_mode: mode,
            sdf_force_visibility_one: false,
            dynamic_direct_scale: 0.0,
            dynamic_direct_isolation: DynamicDirectIsolation::Combined,
            has_direct: false,
        });
        let decoded = u32::from_ne_bytes(data[100..104].try_into().unwrap());
        assert_eq!(
            decoded, mode as u32,
            "SdfShadowMode::{:?} should encode to {} at offset 100..104",
            mode, mode as u32,
        );
        // Trailing pad 120..128 stays zero regardless of mode.
        assert!(
            data[120..128].iter().all(|&b| b == 0),
            "trailing pad bytes 120..128 must stay zero for {:?}",
            mode,
        );
    }
}

/// baked-static-direct-sh Task 6: the dynamic-direct tail of the shared
/// group-0 `Uniforms` must round-trip through the byte packer. `direct_scale`
/// repurposes the former `_sdf_pad1` slot (108..112); isolation + has_direct
/// land in the fresh 16-byte row (112..120), with 120..128 padding.
#[test]
fn uniform_data_encodes_dynamic_direct_tail_at_correct_offsets() {
    let data = build_uniform_data(&FrameUniforms {
        view_proj: Mat4::IDENTITY,
        camera_position: Vec3::ZERO,
        ambient_floor: 0.0,
        light_count: 0,
        time: 0.0,
        lighting_isolation: LightingIsolation::Normal,
        indirect_scale: 1.0,
        sdf_shadow_flags: 0,
        sdf_shadow_mode: SdfShadowMode::On,
        sdf_force_visibility_one: false,
        dynamic_direct_scale: 0.25,
        dynamic_direct_isolation: DynamicDirectIsolation::IndirectOnly,
        has_direct: true,
    });
    let scale = f32::from_ne_bytes(data[108..112].try_into().unwrap());
    assert!((scale - 0.25).abs() < 1e-6, "direct_scale at 108..112");
    assert_eq!(
        u32::from_ne_bytes(data[112..116].try_into().unwrap()),
        DynamicDirectIsolation::IndirectOnly as u32,
        "dynamic_direct_isolation at 112..116",
    );
    assert_eq!(
        u32::from_ne_bytes(data[116..120].try_into().unwrap()),
        1,
        "has_direct at 116..120",
    );
    assert!(
        data[120..128].iter().all(|&b| b == 0),
        "trailing pad 120..128 must stay zero",
    );
}

#[test]
fn uniform_data_encodes_view_proj_camera_and_lighting_fields() {
    let camera = Vec3::new(10.0, 20.0, 30.0);
    let ambient_floor = 0.125_f32;
    let light_count = 7_u32;
    let indirect_scale = 0.5_f32;
    let data = build_uniform_data(&FrameUniforms {
        view_proj: Mat4::IDENTITY,
        camera_position: camera,
        ambient_floor,
        light_count,
        time: 0.0,
        lighting_isolation: LightingIsolation::Normal,
        indirect_scale,
        sdf_shadow_flags: 0,
        sdf_shadow_mode: SdfShadowMode::On,
        sdf_force_visibility_one: false,
        dynamic_direct_scale: DEFAULT_DYNAMIC_DIRECT_SCALE,
        dynamic_direct_isolation: DynamicDirectIsolation::Combined,
        has_direct: false,
    });

    // view_proj: first 64 bytes = 16 f32 identity columns.
    let mut floats = Vec::new();
    for chunk in data.chunks_exact(4).take(16) {
        floats.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
    }
    let identity = Mat4::IDENTITY.to_cols_array();
    for i in 0..16 {
        let epsilon = 1e-6;
        assert!(
            (floats[i] - identity[i]).abs() < epsilon,
            "view_proj[{i}] mismatch: expected {}, got {}",
            identity[i],
            floats[i],
        );
    }

    // camera_position at bytes 64..76.
    let cx = f32::from_ne_bytes(data[64..68].try_into().unwrap());
    let cy = f32::from_ne_bytes(data[68..72].try_into().unwrap());
    let cz = f32::from_ne_bytes(data[72..76].try_into().unwrap());
    assert_eq!(cx, 10.0);
    assert_eq!(cy, 20.0);
    assert_eq!(cz, 30.0);

    // ambient_floor at bytes 76..80.
    let af = f32::from_ne_bytes(data[76..80].try_into().unwrap());
    assert!((af - ambient_floor).abs() < 1e-6);

    // light_count at bytes 80..84.
    let lc = u32::from_ne_bytes(data[80..84].try_into().unwrap());
    assert_eq!(lc, light_count);

    // time at bytes 84..88 (passed 0.0 in this test).
    let t = f32::from_ne_bytes(data[84..88].try_into().unwrap());
    assert_eq!(t, 0.0);

    // lighting_isolation at bytes 88..92 (passed Normal = 0).
    let iso = u32::from_ne_bytes(data[88..92].try_into().unwrap());
    assert_eq!(iso, 0);

    // indirect_scale at bytes 92..96.
    let scale = f32::from_ne_bytes(data[92..96].try_into().unwrap());
    assert!((scale - indirect_scale).abs() < 1e-6);
}

// Regression: spot-shadow clock skew — GPU `time` uniform must equal
// `script_time` so shadow-pool eligibility (CPU) and GPU animation phase
// stay in sync. Using wall-clock here instead would desync them.
#[test]
fn uniform_data_encodes_script_time_as_gpu_time_field() {
    let script_time = 3.75_f32;
    let data = build_uniform_data(&FrameUniforms {
        view_proj: Mat4::IDENTITY,
        camera_position: Vec3::ZERO,
        ambient_floor: 0.0,
        light_count: 0,
        time: script_time,
        lighting_isolation: LightingIsolation::Normal,
        indirect_scale: 1.0,
        sdf_shadow_flags: 0,
        sdf_shadow_mode: SdfShadowMode::On,
        sdf_force_visibility_one: false,
        dynamic_direct_scale: DEFAULT_DYNAMIC_DIRECT_SCALE,
        dynamic_direct_isolation: DynamicDirectIsolation::Combined,
        has_direct: false,
    });
    // time at bytes 84..88.
    let t = f32::from_ne_bytes(data[84..88].try_into().unwrap());
    assert!(
        (t - script_time).abs() < 1e-6,
        "GPU time ({t}) must equal script_time ({script_time})",
    );
}
