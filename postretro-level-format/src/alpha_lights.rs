// AlphaLights PRL section (ID 18): flat per-light record array for the
// direct-lighting path in sub-plan 3 of the Lighting Foundation plan.
//
// **INTERIM FORMAT.** This section exists to unblock direct lighting before
// the entity system lands. Do not build stable consumers against this layout
// — it will be replaced by proper entity serialization in Milestone 6+.
// See: context/plans/in-progress/lighting-foundation/1-fgd-canonical.md
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

/// One serialised light record. Fixed-size on disk: 65 bytes per record.
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
}

/// Byte size of a single serialised `AlphaLightRecord`.
/// 24 (origin) + 1 (type) + 4 (intensity) + 12 (color) + 1 (falloff model)
/// + 4 (range) + 4 + 4 (cone angles) + 12 (cone dir) + 1 (cast shadows) = 67.
pub const ALPHA_LIGHT_RECORD_SIZE: usize = 67;

/// AlphaLights section (ID 18).
///
/// On-disk layout (little-endian throughout):
///   u32  light_count
///   AlphaLightRecord[light_count]  (`ALPHA_LIGHT_RECORD_SIZE` bytes each)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AlphaLightsSection {
    pub lights: Vec<AlphaLightRecord>,
}

impl AlphaLightsSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let count = self.lights.len() as u32;
        let mut buf = Vec::with_capacity(4 + self.lights.len() * ALPHA_LIGHT_RECORD_SIZE);

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

        let count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let expected_len = 4 + count * ALPHA_LIGHT_RECORD_SIZE;
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
        let mut o = 4;

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
            cast_shadows: true,
        }
    }

    #[test]
    fn round_trip_empty() {
        let section = AlphaLightsSection::default();
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), 4);
        let restored = AlphaLightsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_single_record() {
        let section = AlphaLightsSection {
            lights: vec![sample_record()],
        };
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), 4 + ALPHA_LIGHT_RECORD_SIZE);
        let restored = AlphaLightsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
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

    #[test]
    fn rejects_unknown_light_type_byte() {
        let section = AlphaLightsSection {
            lights: vec![sample_record()],
        };
        let mut bytes = section.to_bytes();
        // light_type byte is at offset 4 + 24 = 28.
        bytes[28] = 99;
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
        // falloff_model byte at offset 4 + 24 + 1 + 4 + 12 = 45.
        bytes[45] = 99;
        let err = AlphaLightsSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid falloff_model"), "unexpected: {msg}");
    }
}
