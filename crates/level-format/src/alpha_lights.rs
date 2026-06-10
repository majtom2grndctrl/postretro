// AlphaLights PRL section (ID 18): flat per-light record array for the
// direct-lighting path in sub-plan 3 of the Lighting Foundation plan.
//
// **INTERIM FORMAT.** This section exists to unblock direct lighting before
// the entity system lands. Do not build stable consumers against this layout
// — it will be replaced by proper entity serialization in Milestone 6+.
// See: context/plans/done/lighting-foundation/1-fgd-canonical.md
//      §AlphaLights PRL section

use crate::FormatError;

/// Light shape discriminant. Wire-level `u8`; matches the semantic enum in
/// `postretro-level-compiler::map_data::LightType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AlphaLightType {
    Point = 0,
    Spot = 1,
    Directional = 2,
}

impl AlphaLightType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Point),
            1 => Some(Self::Spot),
            2 => Some(Self::Directional),
            _ => None,
        }
    }
}

/// Falloff model discriminant. Wire-level `u8`; matches the semantic enum in
/// `postretro-level-compiler::map_data::FalloffModel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AlphaFalloffModel {
    Linear = 0,
    InverseDistance = 1,
    InverseSquared = 2,
}

impl AlphaFalloffModel {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Linear),
            1 => Some(Self::InverseDistance),
            2 => Some(Self::InverseSquared),
            _ => None,
        }
    }
}

/// How a baked-tier light's **direct** shadow resolves. Wire-level `u8`;
/// matches the semantic `ShadowType` enum in
/// `postretro-level-compiler::map_data`. Two values only — the dynamic tier is
/// NOT a shadow-type value; it reaches the runtime via the separate
/// `is_dynamic` field (set by classname). The direct techniques are disjoint —
/// a light's direct shadow comes from exactly one — so no contribution is
/// double-counted. See `context/plans/in-progress/sdf-per-light-shadows/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum AlphaShadowType {
    /// Direct shadow baked into the lightmap (free, fixed). The default.
    #[default]
    StaticLightMap = 0,
    /// Runtime SDF-traced per-light direct shadow (sparse, tweakable, no re-bake).
    Sdf = 1,
}

impl AlphaShadowType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::StaticLightMap),
            1 => Some(Self::Sdf),
            _ => None,
        }
    }
}

/// One serialised light record. Fixed-size on disk:
/// `ALPHA_LIGHT_RECORD_SIZE` (74) bytes per record.
#[derive(Debug, Clone, PartialEq)]
pub struct AlphaLightRecord {
    /// World position, engine meters (Y-up).
    pub origin: [f64; 3],
    pub light_type: AlphaLightType,
    /// Linear brightness multiplier applied to `color`. Format-normalized
    /// at the translator boundary — runtime consumers treat this as a
    /// straight linear scalar with no further scaling.
    pub intensity: f32,
    /// Linear RGB, 0-1.
    pub color: [f32; 3],
    pub falloff_model: AlphaFalloffModel,
    /// Meters; meaningful for Point and Spot only.
    pub falloff_range: f32,
    /// Radians; 0.0 if not Spot.
    pub cone_angle_inner: f32,
    /// Radians; 0.0 if not Spot.
    pub cone_angle_outer: f32,
    /// Normalized aim vector; `[0,0,0]` if Point.
    pub cone_direction: [f32; 3],
    /// Routes this light onto the dynamic (shadow-map) tier. Set by the
    /// dynamic-tier CLASSNAME (`light_dynamic` / `light_dynamic_spot`), NOT by a
    /// shadow-type value. `false` for the baked tier (`static_light_map` / `sdf`).
    /// Intensity-only animation does **not** set this flag — that stays on the
    /// animated-baked path and needs no per-frame shadow re-render.
    pub is_dynamic: bool,
    /// Per-light opt-in for casting shadows from dynamic ENTITIES (enemies /
    /// moving meshes). FGD `_cast_entity_shadows`; valid only on dynamic-tier
    /// lights (`is_dynamic`), where it defaults `true`. The compiler warn-clears
    /// it on any non-dynamic (baked-tier) light, so a `true` here always implies
    /// `is_dynamic`.
    pub casts_entity_shadows: bool,
    /// BSP leaf index containing the light origin, baked at compile time for
    /// the runtime PVS cull. `u32::MAX` is the reserved sentinel for
    /// "unassigned / cannot determine leaf" (e.g. the light origin landed in
    /// a solid leaf — a map-authoring error). Runtime culls these and warns.
    pub leaf_index: u32,
    /// How this baked-tier light's direct shadow resolves (FGD `_shadow_type`,
    /// 2 values). The direct techniques are disjoint, so no light's
    /// contribution is double-counted. Records from a `.prl` predating the
    /// shadow-type field decode `StaticLightMap`. The dynamic tier rides
    /// `is_dynamic`, not this field.
    pub shadow_type: AlphaShadowType,
}

/// Sentinel `leaf_index` for lights whose origin could not be assigned to a
/// non-solid leaf at compile time. Runtime consumers cull these and emit a
/// warning at load.
pub const ALPHA_LIGHT_LEAF_UNASSIGNED: u32 = u32::MAX;

/// Byte size of a single serialised `AlphaLightRecord` in the current layout.
/// 24 (origin) + 1 (type) + 4 (intensity) + 12 (color) + 1 (falloff model)
/// + 4 (range) + 4 + 4 (cone angles) + 12 (cone dir) + 1 (is_dynamic)
/// + 1 (casts_entity_shadows) + 4 (leaf_index) + 1 (shadow_type) = 73.
pub const ALPHA_LIGHT_RECORD_SIZE: usize = 73;

/// AlphaLights section version (per-section, distinct from the PRL header
/// `CURRENT_VERSION`; mirrors the `SH_VOLUME_VERSION` precedent). Bumped when
/// the record layout changes so the loader decodes the right fields and
/// rejects stale layouts with a clear error.
///
/// - v4 (current): the dead `cast_shadows` byte was removed (every authored
///   light cast shadows; the flag was never read at runtime). 73-byte records,
///   trailing `shadow_type` byte. This is a BREAKING wire change — any `.prl`
///   predating v4 must be rebuilt via `prl-build`; there is no in-place
///   migration. The dynamic-tier distinction rides the separate `is_dynamic`
///   field, not the `shadow_type` byte.
pub const ALPHA_LIGHTS_VERSION: u32 = 4;

/// AlphaLights section (ID 18).
///
/// On-disk layout (little-endian throughout):
///   u32  version  (= ALPHA_LIGHTS_VERSION)
///   u32  light_count
///   AlphaLightRecord[light_count]  (`ALPHA_LIGHT_RECORD_SIZE` bytes each)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AlphaLightsSection {
    pub lights: Vec<AlphaLightRecord>,
}

impl AlphaLightsSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let count = self.lights.len() as u32;
        let mut buf = Vec::with_capacity(8 + self.lights.len() * ALPHA_LIGHT_RECORD_SIZE);

        buf.extend_from_slice(&ALPHA_LIGHTS_VERSION.to_le_bytes());
        buf.extend_from_slice(&count.to_le_bytes());

        for l in &self.lights {
            buf.extend_from_slice(&l.origin[0].to_le_bytes());
            buf.extend_from_slice(&l.origin[1].to_le_bytes());
            buf.extend_from_slice(&l.origin[2].to_le_bytes());
            buf.push(l.light_type as u8);
            buf.extend_from_slice(&l.intensity.to_le_bytes());
            buf.extend_from_slice(&l.color[0].to_le_bytes());
            buf.extend_from_slice(&l.color[1].to_le_bytes());
            buf.extend_from_slice(&l.color[2].to_le_bytes());
            buf.push(l.falloff_model as u8);
            buf.extend_from_slice(&l.falloff_range.to_le_bytes());
            buf.extend_from_slice(&l.cone_angle_inner.to_le_bytes());
            buf.extend_from_slice(&l.cone_angle_outer.to_le_bytes());
            buf.extend_from_slice(&l.cone_direction[0].to_le_bytes());
            buf.extend_from_slice(&l.cone_direction[1].to_le_bytes());
            buf.extend_from_slice(&l.cone_direction[2].to_le_bytes());
            buf.push(if l.is_dynamic { 1 } else { 0 });
            buf.push(if l.casts_entity_shadows { 1 } else { 0 });
            buf.extend_from_slice(&l.leaf_index.to_le_bytes());
            buf.push(l.shadow_type as u8);
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 8 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "alpha lights section too short for header",
            )));
        }

        // On-disk layout: version(u32) + count(u32) + count×ALPHA_LIGHT_RECORD_SIZE.
        // The leading version word gates the layout; a foreign version is rejected
        // with a clear "recompile" error. The `cast_shadows` byte was removed in
        // v4 (it was never read at runtime), so there is no in-place migration —
        // pre-v4 `.prl` files must be rebuilt via `prl-build`.
        let version = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if version != ALPHA_LIGHTS_VERSION {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "unsupported AlphaLights section version {} (expected {}); recompile the map",
                    version, ALPHA_LIGHTS_VERSION,
                ),
            )));
        }

        let mut o = 8usize;
        let count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;

        let expected_len = o + count * ALPHA_LIGHT_RECORD_SIZE;
        if data.len() < expected_len {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "alpha lights section truncated: need {expected_len} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut lights = Vec::with_capacity(count);

        for i in 0..count {
            let ox = read_f64_le(&data[o..o + 8]);
            let oy = read_f64_le(&data[o + 8..o + 16]);
            let oz = read_f64_le(&data[o + 16..o + 24]);
            let light_type_raw = data[o + 24];
            let light_type = AlphaLightType::from_u8(light_type_raw).ok_or_else(|| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("alpha light {i}: invalid light_type {light_type_raw}"),
                ))
            })?;
            let intensity = read_f32_le(&data[o + 25..o + 29]);
            let cr = read_f32_le(&data[o + 29..o + 33]);
            let cg = read_f32_le(&data[o + 33..o + 37]);
            let cb = read_f32_le(&data[o + 37..o + 41]);
            let falloff_raw = data[o + 41];
            let falloff_model = AlphaFalloffModel::from_u8(falloff_raw).ok_or_else(|| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("alpha light {i}: invalid falloff_model {falloff_raw}"),
                ))
            })?;
            let falloff_range = read_f32_le(&data[o + 42..o + 46]);
            let cone_angle_inner = read_f32_le(&data[o + 46..o + 50]);
            let cone_angle_outer = read_f32_le(&data[o + 50..o + 54]);
            let cdx = read_f32_le(&data[o + 54..o + 58]);
            let cdy = read_f32_le(&data[o + 58..o + 62]);
            let cdz = read_f32_le(&data[o + 62..o + 66]);
            let is_dynamic = data[o + 66] != 0;
            let casts_entity_shadows = data[o + 67] != 0;
            let leaf_index =
                u32::from_le_bytes([data[o + 68], data[o + 69], data[o + 70], data[o + 71]]);
            let raw = data[o + 72];
            let shadow_type = AlphaShadowType::from_u8(raw).ok_or_else(|| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("alpha light {i}: invalid shadow_type {raw}"),
                ))
            })?;

            lights.push(AlphaLightRecord {
                origin: [ox, oy, oz],
                light_type,
                intensity,
                color: [cr, cg, cb],
                falloff_model,
                falloff_range,
                cone_angle_inner,
                cone_angle_outer,
                cone_direction: [cdx, cdy, cdz],
                is_dynamic,
                casts_entity_shadows,
                leaf_index,
                shadow_type,
            });

            o += ALPHA_LIGHT_RECORD_SIZE;
        }

        Ok(Self { lights })
    }
}

fn read_f32_le(s: &[u8]) -> f32 {
    f32::from_le_bytes([s[0], s[1], s[2], s[3]])
}

fn read_f64_le(s: &[u8]) -> f64 {
    f64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> AlphaLightRecord {
        AlphaLightRecord {
            origin: [1.5, -2.25, 3.0],
            light_type: AlphaLightType::Spot,
            intensity: 250.0,
            color: [1.0, 0.5, 0.25],
            falloff_model: AlphaFalloffModel::InverseSquared,
            falloff_range: 104.0576,
            cone_angle_inner: std::f32::consts::FRAC_PI_6, // 30 deg
            cone_angle_outer: std::f32::consts::FRAC_PI_4, // 45 deg
            cone_direction: [0.0, -1.0, 0.0],
            is_dynamic: false,
            casts_entity_shadows: false,
            leaf_index: 7,
            shadow_type: AlphaShadowType::Sdf,
        }
    }

    #[test]
    fn round_trip_empty() {
        let section = AlphaLightsSection::default();
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), 8); // version + count
        let restored = AlphaLightsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_single_record() {
        let section = AlphaLightsSection {
            lights: vec![sample_record()],
        };
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), 8 + ALPHA_LIGHT_RECORD_SIZE);
        let restored = AlphaLightsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    /// The shadow-type tag survives a serialize → deserialize round-trip across
    /// both values. (PRL → runtime contract seam.) Two values only — the
    /// dynamic tier rides `is_dynamic`, asserted alongside here.
    #[test]
    fn shadow_type_and_tier_survive_round_trip() {
        for (ty, is_dynamic) in [
            (AlphaShadowType::StaticLightMap, false),
            (AlphaShadowType::Sdf, false),
            // The dynamic tier is orthogonal to shadow type; it rides
            // `is_dynamic`, which round-trips independently of the u8.
            (AlphaShadowType::StaticLightMap, true),
        ] {
            let mut rec = sample_record();
            rec.shadow_type = ty;
            rec.is_dynamic = is_dynamic;
            let section = AlphaLightsSection { lights: vec![rec] };
            let restored = AlphaLightsSection::from_bytes(&section.to_bytes()).unwrap();
            assert_eq!(restored.lights[0].shadow_type, ty);
            assert_eq!(restored.lights[0].is_dynamic, is_dynamic);
        }
    }

    #[test]
    fn round_trip_multiple_records() {
        let section = AlphaLightsSection {
            lights: vec![
                AlphaLightRecord {
                    origin: [0.0, 0.0, 0.0],
                    light_type: AlphaLightType::Point,
                    intensity: 300.0,
                    color: [1.0, 1.0, 1.0],
                    falloff_model: AlphaFalloffModel::Linear,
                    falloff_range: 50.0,
                    cone_angle_inner: 0.0,
                    cone_angle_outer: 0.0,
                    cone_direction: [0.0, 0.0, 0.0],
                    is_dynamic: false,
                    casts_entity_shadows: false,
                    leaf_index: 0,
                    shadow_type: AlphaShadowType::StaticLightMap,
                },
                sample_record(),
                AlphaLightRecord {
                    origin: [10.0, 20.0, -30.0],
                    light_type: AlphaLightType::Directional,
                    intensity: 200.0,
                    color: [0.7, 0.8, 1.0],
                    falloff_model: AlphaFalloffModel::Linear,
                    falloff_range: 0.0,
                    cone_angle_inner: 0.0,
                    cone_angle_outer: 0.0,
                    cone_direction: [
                        0.0,
                        -std::f32::consts::FRAC_1_SQRT_2,
                        -std::f32::consts::FRAC_1_SQRT_2,
                    ],
                    is_dynamic: true,
                    casts_entity_shadows: false,
                    leaf_index: ALPHA_LIGHT_LEAF_UNASSIGNED,
                    shadow_type: AlphaShadowType::StaticLightMap,
                },
            ],
        };
        let bytes = section.to_bytes();
        let restored = AlphaLightsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = AlphaLightsSection::from_bytes(&[0u8; 3]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("too short"), "unexpected: {msg}");
    }

    #[test]
    fn rejects_truncated_body() {
        // Valid version word + a count claiming 1 light, but no record body.
        let mut buf = Vec::new();
        buf.extend_from_slice(&ALPHA_LIGHTS_VERSION.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes()); // claim 1 light
        let err = AlphaLightsSection::from_bytes(&buf).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("truncated"), "unexpected: {msg}");
    }

    /// A section whose leading version word is not `ALPHA_LIGHTS_VERSION` was
    /// written by a different compiler version. The loader must reject it with a
    /// clear version-mismatch error instructing a recompile rather than decoding
    /// garbage — there is no in-place migration.
    #[test]
    fn rejects_section_with_non_current_version() {
        let non_current_version: u32 = ALPHA_LIGHTS_VERSION + 1;
        let count: u32 = 1;
        let mut buf = Vec::new();
        buf.extend_from_slice(&non_current_version.to_le_bytes());
        buf.extend_from_slice(&count.to_le_bytes());
        buf.extend(std::iter::repeat_n(0u8, ALPHA_LIGHT_RECORD_SIZE));
        assert_eq!(buf.len(), 8 + ALPHA_LIGHT_RECORD_SIZE);

        let err = AlphaLightsSection::from_bytes(&buf).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unsupported AlphaLights section version"),
            "expected version-mismatch error, got: {msg}"
        );
        assert!(
            msg.contains(&non_current_version.to_string()),
            "error should name the bad version number, got: {msg}"
        );
        assert!(
            msg.contains("recompile the map"),
            "error should instruct the user to recompile, got: {msg}"
        );
    }

    #[test]
    fn rejects_unknown_light_type_byte() {
        let section = AlphaLightsSection {
            lights: vec![sample_record()],
        };
        let mut bytes = section.to_bytes();
        // light_type byte: version(4) + count(4) + 24 (origin) = 32.
        bytes[32] = 99;
        let err = AlphaLightsSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid light_type"), "unexpected: {msg}");
    }

    #[test]
    fn rejects_unknown_falloff_model_byte() {
        let section = AlphaLightsSection {
            lights: vec![sample_record()],
        };
        let mut bytes = section.to_bytes();
        // falloff_model byte: version(4) + count(4) + 24 + 1 + 4 + 12 = 49.
        bytes[49] = 99;
        let err = AlphaLightsSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid falloff_model"), "unexpected: {msg}");
    }

    #[test]
    fn rejects_unknown_shadow_type_byte() {
        let section = AlphaLightsSection {
            lights: vec![sample_record()],
        };
        let mut bytes = section.to_bytes();
        // shadow_type is the last byte of the (only) record.
        *bytes.last_mut().unwrap() = 99;
        let err = AlphaLightsSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid shadow_type"), "unexpected: {msg}");
    }
}
