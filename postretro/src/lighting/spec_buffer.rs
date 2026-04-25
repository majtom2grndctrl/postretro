// Spec-only light buffer: one entry per static light, uploaded once at
// level load and consumed by the Blinn-Phong path in `forward.wgsl`.
// See: lighting-chunk-lists/ Task B step 1

use crate::prl::MapLight;

/// Byte size of one `SpecLight` record. WGSL layout is two packed vec4<f32>
/// slots so struct alignment is 16 and array stride is 32.
///
/// Layout (little-endian, matches WGSL storage-buffer ABI on LE hosts):
///   0..12   position           (f32x3)
///   12..16  range              (f32) — falloff_range meters, 0 for directional
///   16..28  color × intensity  (f32x3)
///   28..32  pad                (0)
pub const SPEC_LIGHT_SIZE: usize = 32;

/// Pack the static subset of `lights` into the shader-facing `SpecLight`
/// byte layout. Lights with `is_dynamic == true` are skipped — they are
/// already driven by the dynamic `GpuLight` loop in `forward.wgsl` and
/// must not appear in the static spec buffer (double-count risk).
pub fn pack_spec_lights(lights: &[MapLight]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(lights.len() * SPEC_LIGHT_SIZE);
    for l in lights.iter().filter(|l| !l.is_dynamic) {
        let px = l.origin[0] as f32;
        let py = l.origin[1] as f32;
        let pz = l.origin[2] as f32;
        bytes.extend_from_slice(&px.to_le_bytes());
        bytes.extend_from_slice(&py.to_le_bytes());
        bytes.extend_from_slice(&pz.to_le_bytes());
        bytes.extend_from_slice(&l.falloff_range.to_le_bytes());

        let cr = l.color[0] * l.intensity;
        let cg = l.color[1] * l.intensity;
        let cb = l.color[2] * l.intensity;
        bytes.extend_from_slice(&cr.to_le_bytes());
        bytes.extend_from_slice(&cg.to_le_bytes());
        bytes.extend_from_slice(&cb.to_le_bytes());
        bytes.extend_from_slice(&0f32.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prl::{FalloffModel, LightType};

    fn sample() -> MapLight {
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
            tag: None,
        }
    }

    #[test]
    fn spec_light_size_is_32() {
        assert_eq!(SPEC_LIGHT_SIZE, 32);
    }

    #[test]
    fn empty_input_empty_bytes() {
        assert!(pack_spec_lights(&[]).is_empty());
    }

    #[test]
    fn encodes_position_range_and_premultiplied_color() {
        let bytes = pack_spec_lights(&[sample()]);
        assert_eq!(bytes.len(), SPEC_LIGHT_SIZE);
        let read_f32 = |off: usize| f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        assert_eq!(read_f32(0), 1.0);
        assert_eq!(read_f32(4), 2.0);
        assert_eq!(read_f32(8), 3.0);
        assert_eq!(read_f32(12), 12.5);
        assert!((read_f32(16) - 0.5).abs() < 1e-6); // 0.25 * 2.0
        assert!((read_f32(20) - 1.0).abs() < 1e-6);
        assert!((read_f32(24) - 2.0).abs() < 1e-6);
        assert_eq!(read_f32(28), 0.0);
    }

    #[test]
    fn packs_multiple_records_contiguously() {
        let bytes = pack_spec_lights(&[sample(), sample()]);
        assert_eq!(bytes.len(), 2 * SPEC_LIGHT_SIZE);
    }

    #[test]
    fn skips_dynamic_lights() {
        let mut dyn_light = sample();
        dyn_light.is_dynamic = true;
        let bytes = pack_spec_lights(&[sample(), dyn_light, sample()]);
        assert_eq!(bytes.len(), 2 * SPEC_LIGHT_SIZE);
    }
}
