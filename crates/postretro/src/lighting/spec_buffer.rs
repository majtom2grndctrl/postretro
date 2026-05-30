// Static light buffer: one entry per static light, uploaded once at level load.
// Consumed by the Blinn-Phong specular loop, per-light SDF diffuse loop (both
// in `forward.wgsl`), and the SDF visibility K-selection helper (`sdf_shadow.wgsl`).
// See: context/lib/rendering_pipeline.md

use crate::prl::{MapLight, ShadowType};

/// Byte size of one `SpecLight` record. WGSL layout is two packed vec4<f32>
/// slots so struct alignment is 16 and array stride is 32.
///
/// Layout (little-endian, matches WGSL storage-buffer ABI on LE hosts):
///   0..12   position           (f32x3)
///   12..16  range              (f32) — falloff_range meters, 0 for directional
///   16..28  color × intensity  (f32x3)
///   28..32  sdf_flag           (f32) — 1.0 if `_shadow_type sdf`, else 0.0
pub const SPEC_LIGHT_SIZE: usize = 32;

/// `color_and_pad.w` value flagging an SDF-typed light so the forward loop
/// routes it onto the runtime diffuse + SDF-visibility path. Decoded with
/// `w > 0.5` (see `forward.wgsl`). `static_light_map` lights and dynamic-tier
/// lights carry 0.0 — they need no `spec_lights` flag (`static_light_map` →
/// `lm_irr`; dynamic → shadow-map path; dynamic is skipped from this buffer
/// entirely via `is_dynamic`).
pub const SPEC_LIGHT_SDF_FLAG: f32 = 1.0;

/// Pack the static subset of `lights` into the shader-facing `SpecLight`
/// byte layout. Lights with `is_dynamic == true` are skipped — they are
/// already driven by the dynamic `GpuLight` loop in `forward.wgsl` and
/// must not appear in the static spec buffer (double-count risk).
///
/// `sdf`-tagged lights set the `color_and_pad.w` flag so the forward loop
/// knows which static lights get the runtime per-light diffuse + SDF
/// visibility path (Tasks 2–3); all others carry 0.0.
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
        let sdf_flag = if l.shadow_type == ShadowType::Sdf {
            SPEC_LIGHT_SDF_FLAG
        } else {
            0.0
        };
        bytes.extend_from_slice(&sdf_flag.to_le_bytes());
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
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            leaf_index: 0,
            shadow_type: crate::prl::ShadowType::StaticLightMap,
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

    /// `sdf`-typed lights set the `color_and_pad.w` flag (decoded `w > 0.5`);
    /// `static_light_map` lights carry 0.0. This is the seam the forward loop
    /// reads to route the runtime per-light SDF path.
    #[test]
    fn sdf_tag_sets_color_and_pad_w_flag() {
        let read_flag = |bytes: &[u8]| f32::from_le_bytes(bytes[28..32].try_into().unwrap());

        let mut sdf = sample();
        sdf.shadow_type = ShadowType::Sdf;
        assert!(read_flag(&pack_spec_lights(&[sdf])) > 0.5);

        let baked = sample(); // ShadowType::StaticLightMap
        assert_eq!(read_flag(&pack_spec_lights(&[baked])), 0.0);

        // Dynamic lights are skipped entirely (is_dynamic), so no record is
        // emitted — verified separately by `skips_dynamic_lights`.
    }
}
