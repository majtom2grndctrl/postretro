// Static light buffer: one entry per static light, uploaded once at level load.
// Consumed by the Blinn-Phong specular loop, per-light SDF diffuse loop (both
// in `forward.wgsl`), and the SDF visibility K-selection helper (`sdf_shadow.wgsl`).
// See: context/lib/rendering_pipeline.md

use postretro_level_loader::{LightType, MapLight, ShadowType};

/// Byte size of one `SpecLight` record. WGSL layout is four packed vec4<f32>
/// slots so struct alignment is 16 and array stride is 64.
///
/// Layout (little-endian, matches WGSL storage-buffer ABI on LE hosts):
///   0..12   position           (f32x3)
///   12..16  range              (f32) — falloff_range meters, 0 for directional
///   16..28  color × intensity  (f32x3)
///   28..32  sdf_flag           (f32) — 1.0 if `_shadow_type sdf`, else 0.0
///   32..44  cone_direction     (f32x3) — normalized aim, (0,0,0) for non-spot
///   44..48  light_type         (f32) — SPEC_LIGHT_TYPE_* discriminant
///   48..52  cos(inner_angle)   (f32) — cone full-bright cutoff, 1.0 for non-spot
///   52..56  cos(outer_angle)   (f32) — cone zero cutoff, -1.0 for non-spot
///   56..64  pad                (f32x2)
///
/// Cone direction/angles are packed here (rather than recomputed in-shader) so
/// the static specular loops can apply cone falloff to spot lights — without
/// these fields a spot reads as a point light and over-lights off-axis.
pub const SPEC_LIGHT_SIZE: usize = 64;

/// `cone_dir_and_type.w` discriminant. Point and directional lights are
/// cone-less (full-bright `cos_inner`/`cos_outer` sentinels), so the shader only
/// needs to distinguish "apply cone" (spot) from "skip cone" (everything else).
pub const SPEC_LIGHT_TYPE_POINT: f32 = 0.0;
pub const SPEC_LIGHT_TYPE_SPOT: f32 = 1.0;
pub const SPEC_LIGHT_TYPE_DIRECTIONAL: f32 = 2.0;

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

        // Cone direction + type. Non-spot lights carry a zero direction and the
        // full-bright cone sentinels (cos_inner = 1, cos_outer = -1) so the
        // shader's cone term resolves to 1.0 across the whole sphere.
        let is_spot = l.light_type == LightType::Spot;
        let (dir, light_type, cos_inner, cos_outer) = if is_spot {
            let d = glam::Vec3::from(l.cone_direction).normalize_or_zero();
            (
                [d.x, d.y, d.z],
                SPEC_LIGHT_TYPE_SPOT,
                l.cone_angle_inner.cos(),
                l.cone_angle_outer.cos(),
            )
        } else {
            let light_type = match l.light_type {
                LightType::Directional => SPEC_LIGHT_TYPE_DIRECTIONAL,
                _ => SPEC_LIGHT_TYPE_POINT,
            };
            ([0.0, 0.0, 0.0], light_type, 1.0, -1.0)
        };
        bytes.extend_from_slice(&dir[0].to_le_bytes());
        bytes.extend_from_slice(&dir[1].to_le_bytes());
        bytes.extend_from_slice(&dir[2].to_le_bytes());
        bytes.extend_from_slice(&light_type.to_le_bytes());

        bytes.extend_from_slice(&cos_inner.to_le_bytes());
        bytes.extend_from_slice(&cos_outer.to_le_bytes());
        bytes.extend_from_slice(&0.0f32.to_le_bytes());
        bytes.extend_from_slice(&0.0f32.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_loader::{FalloffModel, LightType};

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
            is_dynamic: false,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
        }
    }

    fn read_f32(bytes: &[u8], off: usize) -> f32 {
        f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap())
    }

    #[test]
    fn spec_light_size_is_64() {
        assert_eq!(SPEC_LIGHT_SIZE, 64);
    }

    #[test]
    fn empty_input_empty_bytes() {
        assert!(pack_spec_lights(&[]).is_empty());
    }

    #[test]
    fn encodes_position_range_and_premultiplied_color() {
        let bytes = pack_spec_lights(&[sample()]);
        assert_eq!(bytes.len(), SPEC_LIGHT_SIZE);
        assert_eq!(read_f32(&bytes, 0), 1.0);
        assert_eq!(read_f32(&bytes, 4), 2.0);
        assert_eq!(read_f32(&bytes, 8), 3.0);
        assert_eq!(read_f32(&bytes, 12), 12.5);
        assert!((read_f32(&bytes, 16) - 0.5).abs() < 1e-6); // 0.25 * 2.0
        assert!((read_f32(&bytes, 20) - 1.0).abs() < 1e-6);
        assert!((read_f32(&bytes, 24) - 2.0).abs() < 1e-6);
        assert_eq!(read_f32(&bytes, 28), 0.0);
    }

    /// A point light packs the non-spot sentinels: zero cone direction, point
    /// type, and a full-bright cone (cos_inner = 1, cos_outer = -1) so the
    /// shader's cone term resolves to 1.0 everywhere. Regression: spot cone data
    /// was dropped from the spec buffer, so spots over-lit off-axis like points.
    #[test]
    fn point_light_packs_full_bright_cone_sentinels() {
        let bytes = pack_spec_lights(&[sample()]); // LightType::Point
        assert_eq!(read_f32(&bytes, 32), 0.0); // cone dir x
        assert_eq!(read_f32(&bytes, 36), 0.0); // cone dir y
        assert_eq!(read_f32(&bytes, 40), 0.0); // cone dir z
        assert_eq!(read_f32(&bytes, 44), SPEC_LIGHT_TYPE_POINT);
        assert_eq!(read_f32(&bytes, 48), 1.0); // cos_inner
        assert_eq!(read_f32(&bytes, 52), -1.0); // cos_outer
    }

    /// A spot light carries its normalized aim, the spot discriminant, and
    /// cos(inner)/cos(outer) so the static specular loop can apply cone falloff.
    #[test]
    fn spot_light_packs_cone_direction_and_angles() {
        let mut spot = sample();
        spot.light_type = LightType::Spot;
        spot.cone_direction = [0.0, -2.0, 0.0]; // non-unit; must normalize
        spot.cone_angle_inner = std::f32::consts::FRAC_PI_6;
        spot.cone_angle_outer = std::f32::consts::FRAC_PI_4;
        let bytes = pack_spec_lights(&[spot]);

        assert!((read_f32(&bytes, 32) - 0.0).abs() < 1e-6);
        assert!((read_f32(&bytes, 36) - (-1.0)).abs() < 1e-6); // normalized
        assert!((read_f32(&bytes, 40) - 0.0).abs() < 1e-6);
        assert_eq!(read_f32(&bytes, 44), SPEC_LIGHT_TYPE_SPOT);
        assert!((read_f32(&bytes, 48) - std::f32::consts::FRAC_PI_6.cos()).abs() < 1e-6);
        assert!((read_f32(&bytes, 52) - std::f32::consts::FRAC_PI_4.cos()).abs() < 1e-6);
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
