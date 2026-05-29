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

/// Which tech casts this light's shadow. Wire-level `u8`; matches the semantic
/// `ShadowTech` enum in `postretro-level-compiler::map_data`. The three sets
/// are disjoint — a light is shadowed by exactly one — so no contribution is
/// double-counted. See `context/plans/in-progress/sdf-per-light-shadows/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum AlphaShadowTech {
    /// Shadow baked into the lightmap (free, fixed). The default.
    #[default]
    Baked = 0,
    /// Runtime SDF-traced per-light shadow (sparse, tweakable, no re-bake).
    Sdf = 1,
    /// Shadow-map path (spots / moving / hero); also sets `is_dynamic`.
    Dynamic = 2,
}

impl AlphaShadowTech {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Baked),
            1 => Some(Self::Sdf),
            2 => Some(Self::Dynamic),
            _ => None,
        }
    }
}

/// One serialised light record. Fixed-size on disk: 72 bytes per record.
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
    pub cast_shadows: bool,
    /// Routes this light onto the shadow-map path. Set by the `_shadow_tech
    /// dynamic` authoring key; `false` for `baked` and `sdf`. Intensity-only
    /// animation does **not** set this flag — that stays on the animated-baked
    /// path and needs no per-frame shadow re-render.
    pub is_dynamic: bool,
    /// Per-light opt-in for shadow-map-pool eligibility for dynamic entities
    /// (enemies / moving meshes). FGD `_cast_entity_shadows`. Default `false`.
    /// Enemy / dynamic-occluder shadows are strictly opt-in.
    pub casts_entity_shadows: bool,
    /// BSP leaf index containing the light origin, baked at compile time for
    /// the runtime PVS cull. `u32::MAX` is the reserved sentinel for
    /// "unassigned / cannot determine leaf" (e.g. the light origin landed in
    /// a solid leaf — a map-authoring error). Runtime culls these and warns.
    pub leaf_index: u32,
    /// Which tech casts this light's shadow (FGD `_shadow_tech`). The three
    /// sets are disjoint, so no light's contribution is double-counted.
    /// Records from a `.prl` predating the shadow-tech field decode `Baked`.
    pub shadow_tech: AlphaShadowTech,
}

/// Sentinel `leaf_index` for lights whose origin could not be assigned to a
/// non-solid leaf at compile time. Runtime consumers cull these and emit a
/// warning at load.
pub const ALPHA_LIGHT_LEAF_UNASSIGNED: u32 = u32::MAX;

/// Byte size of a single serialised `AlphaLightRecord` in the current layout.
/// 24 (origin) + 1 (type) + 4 (intensity) + 12 (color) + 1 (falloff model)
/// + 4 (range) + 4 + 4 (cone angles) + 12 (cone dir) + 1 (cast shadows)
/// + 1 (is_dynamic) + 1 (casts_entity_shadows) + 4 (leaf_index)
/// + 1 (shadow_tech) = 74.
pub const ALPHA_LIGHT_RECORD_SIZE: usize = 74;

/// AlphaLights section version (per-section, distinct from the PRL header
/// `CURRENT_VERSION`; mirrors the `SH_VOLUME_VERSION` precedent). Bumped when
/// the record layout changes so the loader decodes the right fields.
///
/// - v1 (legacy): no `shadow_tech` byte — 73-byte records. Decodes
///   `shadow_tech = Baked`.
/// - v2 (current): trailing `shadow_tech` byte — 74-byte records.
///
/// A version-less `.prl` written before this field existed is treated as v1.
pub const ALPHA_LIGHTS_VERSION: u32 = 2;

/// v1 record stride (predating the `shadow_tech` byte). Records this length
/// decode `shadow_tech = Baked`.
const ALPHA_LIGHT_RECORD_SIZE_V1: usize = 73;

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
            buf.push(if l.cast_shadows { 1 } else { 0 });
            buf.push(if l.is_dynamic { 1 } else { 0 });
            buf.push(if l.casts_entity_shadows { 1 } else { 0 });
            buf.extend_from_slice(&l.leaf_index.to_le_bytes());
            buf.push(l.shadow_tech as u8);
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "alpha lights section too short for header",
            )));
        }

        // The section gained a leading `u32 version` (mirroring
        // `SH_VOLUME_VERSION`). A `.prl` written before that field began with
        // `u32 light_count` directly and 73-byte records. Disambiguate by
        // testing the versioned interpretation against the body length: a
        // current section is `version(4) + count(4) + count*74`. If that
        // matches, decode versioned; otherwise fall back to the version-less
        // v1 layout (`count(4) + count*73`, `shadow_tech = Baked`).
        let first = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let versioned = data.len() >= 8 && {
            let count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
            first == ALPHA_LIGHTS_VERSION && data.len() == 8 + count * ALPHA_LIGHT_RECORD_SIZE
        };

        let (mut o, count, record_size, has_shadow_tech) = if versioned {
            let count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
            (8usize, count, ALPHA_LIGHT_RECORD_SIZE, true)
        } else {
            // Version-less legacy section: `first` is the light count, records
            // are 73 bytes, and `shadow_tech` defaults to `Baked`.
            (4usize, first as usize, ALPHA_LIGHT_RECORD_SIZE_V1, false)
        };

        let expected_len = o + count * record_size;
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
            let cast_shadows = data[o + 66] != 0;
            let is_dynamic = data[o + 67] != 0;
            let casts_entity_shadows = data[o + 68] != 0;
            let leaf_index =
                u32::from_le_bytes([data[o + 69], data[o + 70], data[o + 71], data[o + 72]]);
            // `shadow_tech` trails the record only in v2+; version-less v1
            // sections default to `Baked`.
            let shadow_tech = if has_shadow_tech {
                let raw = data[o + 73];
                AlphaShadowTech::from_u8(raw).ok_or_else(|| {
                    FormatError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("alpha light {i}: invalid shadow_tech {raw}"),
                    ))
                })?
            } else {
                AlphaShadowTech::Baked
            };

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
                cast_shadows,
                is_dynamic,
                casts_entity_shadows,
                leaf_index,
                shadow_tech,
            });

            o += record_size;
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
            cast_shadows: true,
            is_dynamic: false,
            casts_entity_shadows: false,
            leaf_index: 7,
            shadow_tech: AlphaShadowTech::Sdf,
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

    /// The tech tag survives a serialize → deserialize round-trip across all
    /// three values. (PRL → runtime contract seam.)
    #[test]
    fn shadow_tech_survives_round_trip() {
        for tech in [
            AlphaShadowTech::Baked,
            AlphaShadowTech::Sdf,
            AlphaShadowTech::Dynamic,
        ] {
            let mut rec = sample_record();
            rec.shadow_tech = tech;
            let section = AlphaLightsSection { lights: vec![rec] };
            let restored = AlphaLightsSection::from_bytes(&section.to_bytes()).unwrap();
            assert_eq!(restored.lights[0].shadow_tech, tech);
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
                    cast_shadows: true,
                    is_dynamic: false,
                    casts_entity_shadows: false,
                    leaf_index: 0,
                    shadow_tech: AlphaShadowTech::Baked,
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
                    cast_shadows: false,
                    is_dynamic: false,
                    casts_entity_shadows: false,
                    leaf_index: ALPHA_LIGHT_LEAF_UNASSIGNED,
                    shadow_tech: AlphaShadowTech::Dynamic,
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
        let mut buf = vec![0u8; 4];
        buf[0] = 1; // claim 1 light
        let err = AlphaLightsSection::from_bytes(&buf).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("truncated"), "unexpected: {msg}");
    }

    /// Version-less legacy PRLs predating `_shadow_tech` write a section with
    /// no leading version field and 73-byte records (no trailing tech byte).
    /// The loader detects the absence of the version header and defaults every
    /// record's `shadow_tech` to `Baked`, preserving the stored `is_dynamic`.
    #[test]
    fn legacy_versionless_section_decodes_baked_shadow_tech() {
        // Build a version-less (count + 73-byte records) body for two records,
        // with distinct is_dynamic bytes to assert preservation.
        let mut buf = Vec::new();
        let count: u32 = 2;
        buf.extend_from_slice(&count.to_le_bytes());
        for (is_dyn_byte, leaf) in [(0u8, 5u32), (1u8, 9u32)] {
            // origin (24)
            buf.extend_from_slice(&0.0_f64.to_le_bytes());
            buf.extend_from_slice(&0.0_f64.to_le_bytes());
            buf.extend_from_slice(&0.0_f64.to_le_bytes());
            buf.push(0); // light_type Point
            buf.extend_from_slice(&1.0_f32.to_le_bytes()); // intensity
            buf.extend_from_slice(&1.0_f32.to_le_bytes()); // r
            buf.extend_from_slice(&1.0_f32.to_le_bytes()); // g
            buf.extend_from_slice(&1.0_f32.to_le_bytes()); // b
            buf.push(0); // falloff Linear
            buf.extend_from_slice(&10.0_f32.to_le_bytes()); // range
            buf.extend_from_slice(&0.0_f32.to_le_bytes()); // cone inner
            buf.extend_from_slice(&0.0_f32.to_le_bytes()); // cone outer
            buf.extend_from_slice(&0.0_f32.to_le_bytes()); // cdx
            buf.extend_from_slice(&0.0_f32.to_le_bytes()); // cdy
            buf.extend_from_slice(&0.0_f32.to_le_bytes()); // cdz
            buf.push(1); // cast_shadows
            buf.push(is_dyn_byte); // is_dynamic
            buf.push(0); // casts_entity_shadows
            buf.extend_from_slice(&leaf.to_le_bytes()); // leaf_index
        }
        assert_eq!(buf.len(), 4 + 2 * ALPHA_LIGHT_RECORD_SIZE_V1);

        let section = AlphaLightsSection::from_bytes(&buf).expect("legacy body should parse");
        assert_eq!(section.lights.len(), 2);
        assert!(!section.lights[0].is_dynamic);
        assert_eq!(section.lights[0].leaf_index, 5);
        assert_eq!(section.lights[0].shadow_tech, AlphaShadowTech::Baked);
        assert!(section.lights[1].is_dynamic);
        assert_eq!(section.lights[1].leaf_index, 9);
        assert_eq!(section.lights[1].shadow_tech, AlphaShadowTech::Baked);
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
    fn rejects_unknown_shadow_tech_byte() {
        let section = AlphaLightsSection {
            lights: vec![sample_record()],
        };
        let mut bytes = section.to_bytes();
        // shadow_tech is the last byte of the (only) record.
        *bytes.last_mut().unwrap() = 99;
        let err = AlphaLightsSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid shadow_tech"), "unexpected: {msg}");
    }
}
