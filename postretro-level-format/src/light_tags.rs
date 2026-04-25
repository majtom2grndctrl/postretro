// LightTags PRL section (ID 26): per-light author-supplied script tag.
// See: context/lib/build_pipeline.md §PRL section IDs

use crate::FormatError;

/// One tag per AlphaLights record, in the same order. An empty string means
/// "no tag" so every on-disk entry is length-prefixed + UTF-8 bytes.
///
/// On-disk layout (little-endian):
///   u32  tag_count
///   repeat tag_count:
///     u32   byte_len
///     u8[]  utf8_bytes   (no terminator, length == byte_len)
///
/// The section exists only when at least one tag is author-supplied; the
/// compiler omits it entirely for tag-less maps, and the runtime treats the
/// absence as "all lights have no tag". A `tag_count` of 0 is also legal (the
/// section is present but all entries are absent); the loader accepts both.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LightTagsSection {
    pub tags: Vec<String>,
}

impl LightTagsSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        let count = self.tags.len() as u32;
        buf.extend_from_slice(&count.to_le_bytes());
        for tag in &self.tags {
            let bytes = tag.as_bytes();
            buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(bytes);
        }
        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "light tags section too short for header",
            )));
        }
        let count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let mut tags = Vec::with_capacity(count);
        let mut o = 4usize;
        for i in 0..count {
            if o + 4 > data.len() {
                return Err(FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("light tags entry {i}: truncated length prefix"),
                )));
            }
            let byte_len = u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]])
                as usize;
            o += 4;
            if o + byte_len > data.len() {
                return Err(FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("light tags entry {i}: truncated payload"),
                )));
            }
            let s = std::str::from_utf8(&data[o..o + byte_len]).map_err(|_| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("light tags entry {i}: invalid UTF-8"),
                ))
            })?;
            tags.push(s.to_string());
            o += byte_len;
        }
        Ok(Self { tags })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let section = LightTagsSection::default();
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), 4);
        let restored = LightTagsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_mixed_tags() {
        let section = LightTagsSection {
            tags: vec![
                "hallway_wave".to_string(),
                String::new(),
                "boss_strobe".to_string(),
            ],
        };
        let bytes = section.to_bytes();
        let restored = LightTagsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_unicode() {
        let section = LightTagsSection {
            tags: vec!["écran_de_néon".to_string()],
        };
        let bytes = section.to_bytes();
        let restored = LightTagsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = LightTagsSection::from_bytes(&[0u8; 3]).unwrap_err();
        assert!(err.to_string().contains("too short"));
    }

    #[test]
    fn rejects_truncated_length_prefix() {
        let mut buf = vec![0u8; 4];
        buf[0] = 1; // claim 1 tag
        let err = LightTagsSection::from_bytes(&buf).unwrap_err();
        assert!(err.to_string().contains("truncated length prefix"));
    }

    #[test]
    fn rejects_truncated_payload() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes()); // 1 tag
        buf.extend_from_slice(&5u32.to_le_bytes()); // claim 5 bytes
        buf.extend_from_slice(b"abc"); // only 3 bytes
        let err = LightTagsSection::from_bytes(&buf).unwrap_err();
        assert!(err.to_string().contains("truncated payload"));
    }

    #[test]
    fn rejects_invalid_utf8() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.push(0xFF);
        buf.push(0xFE);
        let err = LightTagsSection::from_bytes(&buf).unwrap_err();
        assert!(err.to_string().contains("invalid UTF-8"));
    }
}
