// LightInfluence PRL section (ID 21): per-light bounding volumes for
// spatial culling in the fragment shader and CPU-side shadow-slot allocation.
// See: context/plans/in-progress/lighting-foundation/4-light-influence-volumes.md

use crate::FormatError;

/// Current section version. Bump when the record layout changes.
pub const LIGHT_INFLUENCE_VERSION: u32 = 1;

/// Byte size of one packed `InfluenceRecord` on disk.
pub const INFLUENCE_RECORD_SIZE: u32 = 16;

/// Section header size: version + record_count + record_stride + reserved.
const HEADER_SIZE: usize = 16;

/// One influence record: sphere center + radius. 16 bytes, maps to one
/// `vec4<f32>` on the GPU.
///
/// Directional lights encode as `([0,0,0], f32::MAX)` — the shader tests
/// `radius > 1.0e30` to skip the spatial test.
#[derive(Debug, Clone, PartialEq)]
pub struct InfluenceRecord {
    /// World-space sphere center (unused for directional lights).
    pub center: [f32; 3],
    /// Sphere radius in meters. `f32::MAX` signals "always active" (no bound).
    pub radius: f32,
}

/// LightInfluence section (ID 21).
///
/// On-disk layout (little-endian throughout):
///   u32  version        (= 1)
///   u32  record_count
///   u32  record_stride  (= 16)
///   u32  reserved       (= 0)
///   [record_count × record_stride bytes] packed InfluenceRecord array
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LightInfluenceSection {
    pub records: Vec<InfluenceRecord>,
}

impl LightInfluenceSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let count = self.records.len() as u32;
        let mut buf =
            Vec::with_capacity(HEADER_SIZE + self.records.len() * INFLUENCE_RECORD_SIZE as usize);

        // Header
        buf.extend_from_slice(&LIGHT_INFLUENCE_VERSION.to_le_bytes());
        buf.extend_from_slice(&count.to_le_bytes());
        buf.extend_from_slice(&INFLUENCE_RECORD_SIZE.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // reserved

        // Records
        for r in &self.records {
            buf.extend_from_slice(&r.center[0].to_le_bytes());
            buf.extend_from_slice(&r.center[1].to_le_bytes());
            buf.extend_from_slice(&r.center[2].to_le_bytes());
            buf.extend_from_slice(&r.radius.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "light influence section too short for header",
            )));
        }

        let version = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if version != LIGHT_INFLUENCE_VERSION {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "light influence section version {version}, expected {LIGHT_INFLUENCE_VERSION}"
                ),
            )));
        }

        let record_count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let record_stride = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;

        if record_stride < INFLUENCE_RECORD_SIZE as usize {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "light influence record_stride {record_stride} < minimum {}",
                    INFLUENCE_RECORD_SIZE
                ),
            )));
        }

        let expected_len = HEADER_SIZE + record_count * record_stride;
        if data.len() < expected_len {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "light influence section truncated: need {expected_len} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut records = Vec::with_capacity(record_count);
        for i in 0..record_count {
            let o = HEADER_SIZE + i * record_stride;
            let cx = f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
            let cy = f32::from_le_bytes([data[o + 4], data[o + 5], data[o + 6], data[o + 7]]);
            let cz = f32::from_le_bytes([data[o + 8], data[o + 9], data[o + 10], data[o + 11]]);
            let radius =
                f32::from_le_bytes([data[o + 12], data[o + 13], data[o + 14], data[o + 15]]);

            records.push(InfluenceRecord {
                center: [cx, cy, cz],
                radius,
            });
        }

        Ok(Self { records })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let section = LightInfluenceSection::default();
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), HEADER_SIZE);
        let restored = LightInfluenceSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_all_light_types() {
        let section = LightInfluenceSection {
            records: vec![
                // Point light
                InfluenceRecord {
                    center: [1.5, -2.25, 3.0],
                    radius: 50.0,
                },
                // Spot light (conservative sphere)
                InfluenceRecord {
                    center: [-4.0, 1.0, 0.0],
                    radius: 20.0,
                },
                // Directional light (infinite bound)
                InfluenceRecord {
                    center: [0.0, 0.0, 0.0],
                    radius: f32::MAX,
                },
            ],
        };
        let bytes = section.to_bytes();
        assert_eq!(
            bytes.len(),
            HEADER_SIZE + 3 * INFLUENCE_RECORD_SIZE as usize
        );
        let restored = LightInfluenceSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn f32_max_sentinel_round_trips() {
        let section = LightInfluenceSection {
            records: vec![InfluenceRecord {
                center: [0.0, 0.0, 0.0],
                radius: f32::MAX,
            }],
        };
        let bytes = section.to_bytes();
        let restored = LightInfluenceSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored.records[0].radius, f32::MAX);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = LightInfluenceSection::from_bytes(&[0u8; 12]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("too short"), "unexpected: {msg}");
    }

    #[test]
    fn rejects_truncated_body() {
        let mut buf = vec![0u8; HEADER_SIZE];
        // version = 1
        buf[0] = 1;
        // record_count = 1
        buf[4] = 1;
        // record_stride = 16
        buf[8] = 16;
        let err = LightInfluenceSection::from_bytes(&buf).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("truncated"), "unexpected: {msg}");
    }

    #[test]
    fn rejects_too_small_stride() {
        let mut buf = vec![0u8; HEADER_SIZE];
        buf[0] = 1; // version
        buf[4] = 0; // record_count = 0
        buf[8] = 8; // stride < 16
        let err = LightInfluenceSection::from_bytes(&buf).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("record_stride"), "unexpected: {msg}");
    }

    #[test]
    fn forward_compatible_larger_stride() {
        // Simulate a future version that writes 32-byte records. The reader
        // should consume the first 16 bytes and skip the rest.
        let section = LightInfluenceSection {
            records: vec![InfluenceRecord {
                center: [1.0, 2.0, 3.0],
                radius: 10.0,
            }],
        };
        let mut bytes = Vec::new();
        // Header with stride=32
        bytes.extend_from_slice(&1u32.to_le_bytes()); // version
        bytes.extend_from_slice(&1u32.to_le_bytes()); // count
        bytes.extend_from_slice(&32u32.to_le_bytes()); // stride
        bytes.extend_from_slice(&0u32.to_le_bytes()); // reserved
        // Record: 16 real bytes + 16 padding
        bytes.extend_from_slice(&1.0f32.to_le_bytes());
        bytes.extend_from_slice(&2.0f32.to_le_bytes());
        bytes.extend_from_slice(&3.0f32.to_le_bytes());
        bytes.extend_from_slice(&10.0f32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 16]); // future fields

        let restored = LightInfluenceSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, section);
    }

    #[test]
    fn header_encodes_version_count_stride() {
        let section = LightInfluenceSection {
            records: vec![InfluenceRecord {
                center: [0.0, 0.0, 0.0],
                radius: 5.0,
            }],
        };
        let bytes = section.to_bytes();
        let version = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let count = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let stride = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let reserved = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        assert_eq!(version, 1);
        assert_eq!(count, 1);
        assert_eq!(stride, 16);
        assert_eq!(reserved, 0);
    }
}
