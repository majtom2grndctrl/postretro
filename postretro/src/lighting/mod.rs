// Direct lighting CPU data: convert MapLight records into the packed
// GpuLight byte layout consumed by the forward pass storage buffer.
//
// See: context/lib/rendering_pipeline.md §4
//      context/plans/in-progress/lighting-foundation/3-direct-lighting.md

pub mod chunk_list;
pub mod influence;
pub mod lightmap;
pub mod spec_buffer;
pub mod spot_shadow;

use crate::prl::{FalloffModel, LightType, MapLight};

/// On-disk size of a single `GpuLight` record in the storage buffer.
///
/// Layout matches the WGSL `GpuLight` struct in `forward.wgsl` — four
/// `vec4<f32>` slots in order:
///   0: position_and_type        (xyz = world position, w = bitcast<f32>(light_type))
///   1: color_and_falloff_model  (xyz = linear RGB × intensity, w = bitcast<f32>(falloff_model))
///   2: direction_and_range      (xyz = aim direction, w = falloff_range meters)
///   3: cone_angles_and_pad      (x = inner angle rad, y = outer angle rad, zw = pad)
///
/// Each vec4<f32> is 16 bytes; the struct has only vec4 members so its
/// alignment is 16 and the array stride is an exact multiple of 16.
pub const GPU_LIGHT_SIZE: usize = 64;

/// Encode the `LightType` discriminant the way the shader expects it:
/// a `u32` bit-cast into the `w` slot of the `position_and_type` vec4.
fn light_type_u32(ty: LightType) -> u32 {
    match ty {
        LightType::Point => 0,
        LightType::Spot => 1,
        LightType::Directional => 2,
    }
}

/// Encode the `FalloffModel` discriminant the same way for the shader.
fn falloff_model_u32(fm: FalloffModel) -> u32 {
    match fm {
        FalloffModel::Linear => 0,
        FalloffModel::InverseDistance => 1,
        FalloffModel::InverseSquared => 2,
    }
}

/// Pack one `MapLight` into the shader-facing `GpuLight` byte layout.
///
/// Pre-multiplies `color × intensity` on the CPU so the shader does one
/// mul per light in the inner loop instead of two. For `Directional` and
/// `Point` lights the unused fields (direction for Point, cone angles for
/// non-Spot, position for Directional in the shader's logic) still need
/// to be zeroed to keep the record deterministic.
///
/// The `slot_index` is written to bytes 56–59 (cone_angles_and_pad z component).
/// Sentinel `0xFFFFFFFF` = no shadow slot allocated.
pub fn pack_light_with_slot(light: &MapLight, slot_index: u32) -> [u8; GPU_LIGHT_SIZE] {
    let mut bytes = [0u8; GPU_LIGHT_SIZE];

    // slot 0: position_and_type — world position (f32x3) + bitcast<f32>(u32 light_type)
    let px = light.origin[0] as f32;
    let py = light.origin[1] as f32;
    let pz = light.origin[2] as f32;
    let type_bits = light_type_u32(light.light_type);
    write_f32(&mut bytes, 0, px);
    write_f32(&mut bytes, 4, py);
    write_f32(&mut bytes, 8, pz);
    write_u32_as_f32(&mut bytes, 12, type_bits);

    // slot 1: color_and_falloff_model — color × intensity + bitcast<f32>(u32 falloff_model)
    let cr = light.color[0] * light.intensity;
    let cg = light.color[1] * light.intensity;
    let cb = light.color[2] * light.intensity;
    let falloff_bits = falloff_model_u32(light.falloff_model);
    write_f32(&mut bytes, 16, cr);
    write_f32(&mut bytes, 20, cg);
    write_f32(&mut bytes, 24, cb);
    write_u32_as_f32(&mut bytes, 28, falloff_bits);

    // slot 2: direction_and_range — cone aim direction + falloff_range
    // cone_direction is already the aim direction (light → target) per MapLight;
    // we store it as-is. Point lights may have [0,0,0] here; the shader
    // branches on light_type so the zero won't be consumed.
    write_f32(&mut bytes, 32, light.cone_direction[0]);
    write_f32(&mut bytes, 36, light.cone_direction[1]);
    write_f32(&mut bytes, 40, light.cone_direction[2]);
    write_f32(&mut bytes, 44, light.falloff_range);

    // slot 3: cone_angles_and_pad — inner + outer cone angles (radians) + shadow slot index
    write_f32(&mut bytes, 48, light.cone_angle_inner);
    write_f32(&mut bytes, 52, light.cone_angle_outer);
    // bytes 56..60 hold the shadow slot index (0..8 or 0xFFFFFFFF for no slot).
    // Shader reads as f32 then bitcasts back to u32; round-trip preserves bit patterns.
    write_u32_as_f32(&mut bytes, 56, slot_index);
    // bytes 60..64 stay zero — reserved pad.

    bytes
}

/// Legacy wrapper for backward compatibility. Calls `pack_light_with_slot` with `NO_SHADOW_SLOT`.
pub fn pack_light(light: &MapLight) -> [u8; GPU_LIGHT_SIZE] {
    pack_light_with_slot(light, crate::lighting::spot_shadow::NO_SHADOW_SLOT)
}

/// Pack the full light list into a contiguous byte buffer suitable for
/// `queue.write_buffer` into the shader's `array<GpuLight>` binding.
///
/// Each light's shadow slot index is set to `NO_SHADOW_SLOT` (no shadow).
/// For runtime slot allocation, use `pack_lights_with_slots` instead.
pub fn pack_lights(lights: &[MapLight]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(lights.len() * GPU_LIGHT_SIZE);
    for light in lights {
        bytes.extend_from_slice(&pack_light(light));
    }
    bytes
}

/// Pack the full light list with per-light shadow slot assignments.
///
/// `slot_indices` must have the same length as `lights`.
/// Each entry is a slot index (0..8) or `NO_SHADOW_SLOT` for unshadowed.
#[allow(dead_code)]
pub fn pack_lights_with_slots(lights: &[MapLight], slot_indices: &[u32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(lights.len() * GPU_LIGHT_SIZE);
    for (light, &slot) in lights.iter().zip(slot_indices.iter()) {
        bytes.extend_from_slice(&pack_light_with_slot(light, slot));
    }
    bytes
}

#[inline]
fn write_f32(dst: &mut [u8], offset: usize, value: f32) {
    dst[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

/// Write a `u32` into the slot, reusing the same 4 bytes a `f32` would
/// occupy. The shader reconstructs the integer via `bitcast<u32>(...)` on
/// the vec4's `w` component. Using `to_ne_bytes` matches `write_f32` and
/// survives the cross-field copy because both use native byte order and
/// wgpu rejects mismatched layouts at pipeline creation time if anything
/// gets skewed.
#[inline]
fn write_u32_as_f32(dst: &mut [u8], offset: usize, value: u32) {
    dst[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_point() -> MapLight {
        MapLight {
            origin: [1.0, 2.0, 3.0],
            light_type: LightType::Point,
            intensity: 2.0,
            color: [0.25, 0.5, 1.0],
            falloff_model: FalloffModel::InverseSquared,
            falloff_range: 12.5,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, 0.0],
            cast_shadows: false,
            is_dynamic: false,
        }
    }

    fn sample_spot() -> MapLight {
        MapLight {
            origin: [-4.0, 1.0, 0.0],
            light_type: LightType::Spot,
            intensity: 1.5,
            color: [1.0, 0.8, 0.6],
            falloff_model: FalloffModel::Linear,
            falloff_range: 20.0,
            cone_angle_inner: 0.5,
            cone_angle_outer: 0.8,
            cone_direction: [0.0, -1.0, 0.0],
            cast_shadows: true,
            is_dynamic: false,
        }
    }

    fn sample_directional() -> MapLight {
        MapLight {
            origin: [0.0, 100.0, 0.0],
            light_type: LightType::Directional,
            intensity: 0.9,
            color: [0.8, 0.9, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 0.0,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, -1.0, 0.0],
            cast_shadows: false,
            is_dynamic: false,
        }
    }

    fn read_f32(src: &[u8], offset: usize) -> f32 {
        f32::from_ne_bytes(src[offset..offset + 4].try_into().unwrap())
    }

    fn read_u32(src: &[u8], offset: usize) -> u32 {
        u32::from_ne_bytes(src[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn gpu_light_size_matches_expected_stride() {
        // 4 vec4<f32> slots × 16 bytes each.
        assert_eq!(GPU_LIGHT_SIZE, 64);
    }

    #[test]
    fn pack_point_light_encodes_position_color_type() {
        let light = sample_point();
        let bytes = pack_light(&light);

        assert_eq!(read_f32(&bytes, 0), 1.0);
        assert_eq!(read_f32(&bytes, 4), 2.0);
        assert_eq!(read_f32(&bytes, 8), 3.0);
        assert_eq!(read_u32(&bytes, 12), 0); // Point

        // color × intensity is pre-multiplied on the CPU.
        assert!((read_f32(&bytes, 16) - 0.5).abs() < 1e-6);
        assert!((read_f32(&bytes, 20) - 1.0).abs() < 1e-6);
        assert!((read_f32(&bytes, 24) - 2.0).abs() < 1e-6);
        assert_eq!(read_u32(&bytes, 28), 2); // InverseSquared

        // range lives in slot 2 w.
        assert_eq!(read_f32(&bytes, 44), 12.5);

        // slot index should be NO_SHADOW_SLOT.
        assert_eq!(
            read_u32(&bytes, 56),
            crate::lighting::spot_shadow::NO_SHADOW_SLOT
        );
    }

    #[test]
    fn pack_spot_light_encodes_direction_and_cone_angles() {
        let light = sample_spot();
        let bytes = pack_light(&light);

        assert_eq!(read_u32(&bytes, 12), 1); // Spot
        assert_eq!(read_u32(&bytes, 28), 0); // Linear

        // direction (slot 2 xyz)
        assert_eq!(read_f32(&bytes, 32), 0.0);
        assert_eq!(read_f32(&bytes, 36), -1.0);
        assert_eq!(read_f32(&bytes, 40), 0.0);
        assert_eq!(read_f32(&bytes, 44), 20.0);

        // cone angles (slot 3 x/y)
        assert_eq!(read_f32(&bytes, 48), 0.5);
        assert_eq!(read_f32(&bytes, 52), 0.8);
    }

    #[test]
    fn pack_directional_light_has_zero_range_and_type_2() {
        let light = sample_directional();
        let bytes = pack_light(&light);

        assert_eq!(read_u32(&bytes, 12), 2); // Directional
        assert_eq!(read_f32(&bytes, 44), 0.0); // no range for directional
        // direction is still stored so the shader can compute -L.
        assert_eq!(read_f32(&bytes, 36), -1.0);
    }

    #[test]
    fn pack_lights_empty_list_is_empty_buffer() {
        let bytes = pack_lights(&[]);
        assert!(bytes.is_empty());
    }

    #[test]
    fn pack_lights_concatenates_records() {
        let lights = vec![sample_point(), sample_spot(), sample_directional()];
        let bytes = pack_lights(&lights);
        assert_eq!(bytes.len(), 3 * GPU_LIGHT_SIZE);

        // First record's type should still be Point.
        assert_eq!(read_u32(&bytes, 12), 0);
        // Second record's type should be Spot.
        assert_eq!(read_u32(&bytes, GPU_LIGHT_SIZE + 12), 1);
        // Third record's type should be Directional.
        assert_eq!(read_u32(&bytes, 2 * GPU_LIGHT_SIZE + 12), 2);
    }

    #[test]
    fn cone_pad_is_zeroed_for_point_light() {
        let bytes = pack_light(&sample_point());
        // cone_angles_and_pad slot, z = shadow slot index, w = pad = bytes 60..64
        assert_eq!(
            read_u32(&bytes, 56),
            crate::lighting::spot_shadow::NO_SHADOW_SLOT
        );
        for &b in &bytes[60..64] {
            assert_eq!(b, 0);
        }
    }

    #[test]
    fn pack_lights_with_slots_encodes_slot_indices() {
        let lights = vec![sample_point(), sample_spot()];
        let slots = vec![0u32, 1u32];
        let bytes = pack_lights_with_slots(&lights, &slots);

        // First light at slot 0.
        assert_eq!(read_u32(&bytes, 56), 0);
        // Second light at slot 1.
        assert_eq!(read_u32(&bytes, GPU_LIGHT_SIZE + 56), 1);
    }

    #[test]
    fn pack_lights_with_slots_no_slot_sentinel() {
        let lights = vec![sample_point(), sample_spot()];
        let slots = vec![crate::lighting::spot_shadow::NO_SHADOW_SLOT, 2u32];
        let bytes = pack_lights_with_slots(&lights, &slots);

        // First light unshadowed.
        assert_eq!(
            read_u32(&bytes, 56),
            crate::lighting::spot_shadow::NO_SHADOW_SLOT
        );
        // Second light at slot 2.
        assert_eq!(read_u32(&bytes, GPU_LIGHT_SIZE + 56), 2);
    }
}
