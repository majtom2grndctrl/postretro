// LightmapPoolBound PRL section (ID 58): the worst mandatory pool layers of any
// camera cell, per pool, which a streamed level's pools are sized from.
// Governing context: context/lib/build_pipeline.md §PRL section IDs

use crate::lightmap::{eof, invalid, read_u32};

/// The one layout this build reads.
pub const LIGHTMAP_POOL_BOUND_VERSION: u32 = 1;

/// version, worst_lightmap_layers, worst_mask_layers.
const SECTION_BYTES: usize = 12;

/// On-disk layout (little-endian): `u32 version` (= 1),
/// `u32 worst_lightmap_layers`, `u32 worst_mask_layers`. Each is a maximum over
/// every camera cell, taken per pool, so the two can come from different cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightmapPoolBoundSection {
    pub worst_lightmap_layers: u32,
    pub worst_mask_layers: u32,
}

impl LightmapPoolBoundSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SECTION_BYTES);
        out.extend_from_slice(&LIGHTMAP_POOL_BOUND_VERSION.to_le_bytes());
        out.extend_from_slice(&self.worst_lightmap_layers.to_le_bytes());
        out.extend_from_slice(&self.worst_mask_layers.to_le_bytes());
        out
    }

    /// Rejects an unknown version first, so a later layout of any length reads
    /// as a re-bake; then any length but this version's.
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(eof("LightmapPoolBound section too short for its version"));
        }
        let version = read_u32(data, 0);
        if version != LIGHTMAP_POOL_BOUND_VERSION {
            return Err(invalid(format!(
                "LightmapPoolBound version {version} is unknown; this build reads version \
                 {LIGHTMAP_POOL_BOUND_VERSION}; re-bake the level"
            )));
        }
        if data.len() != SECTION_BYTES {
            return Err(invalid(format!(
                "LightmapPoolBound section is {} bytes, expected {SECTION_BYTES}",
                data.len()
            )));
        }
        Ok(Self {
            worst_lightmap_layers: read_u32(data, 4),
            worst_mask_layers: read_u32(data, 8),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SectionId;

    fn error_message(result: crate::Result<LightmapPoolBoundSection>) -> String {
        format!("{:?}", result.expect_err("expected a rejection"))
    }

    #[test]
    fn bound_round_trips_as_three_little_endian_words() {
        let section = LightmapPoolBoundSection {
            worst_lightmap_layers: 0x0102_0304,
            worst_mask_layers: 15,
        };
        let bytes = section.to_bytes();
        assert_eq!(
            bytes,
            [1, 0, 0, 0, 4, 3, 2, 1, 15, 0, 0, 0],
            "version, lightmap layers, mask layers"
        );
        assert_eq!(
            LightmapPoolBoundSection::from_bytes(&bytes).unwrap(),
            section
        );
    }

    #[test]
    fn rejects_unknown_id_58_version_by_name_at_any_length() {
        let mut bytes = LightmapPoolBoundSection {
            worst_lightmap_layers: 1,
            worst_mask_layers: 1,
        }
        .to_bytes();
        bytes[0..4].copy_from_slice(&2u32.to_le_bytes());
        for len in [bytes.len(), 4] {
            let message = error_message(LightmapPoolBoundSection::from_bytes(&bytes[..len]));
            assert!(
                message.contains("LightmapPoolBound version 2 is unknown")
                    && message.contains("re-bake"),
                "{message}"
            );
        }
    }

    #[test]
    fn rejects_a_bound_of_the_wrong_length() {
        let bytes = LightmapPoolBoundSection {
            worst_lightmap_layers: 1,
            worst_mask_layers: 1,
        }
        .to_bytes();
        let message = error_message(LightmapPoolBoundSection::from_bytes(&bytes[..11]));
        assert!(message.contains("is 11 bytes, expected 12"), "{message}");
        let message = error_message(LightmapPoolBoundSection::from_bytes(&bytes[..3]));
        assert!(message.contains("too short"), "{message}");
    }

    #[test]
    fn section_id_is_pinned() {
        assert_eq!(SectionId::LightmapPoolBound as u32, 58);
        assert_eq!(SectionId::from_u32(58), Some(SectionId::LightmapPoolBound));
    }
}
