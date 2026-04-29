// Texture names section: flat list of texture name strings.
// See: context/lib/build_pipeline.md §PRL

use crate::FormatError;

/// Texture names section: flat list of texture name strings indexed by FaceMeta.texture_index.
///
/// On-disk layout (all little-endian):
///   u32           count  -- number of texture names
///   Per name:
///     u32         length -- byte length of name string (no null terminator)
///     [u8; length] data  -- UTF-8 bytes
#[derive(Debug, Clone, PartialEq)]
pub struct TextureNamesSection {
    pub names: Vec<String>,
}

impl TextureNamesSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let count = self.names.len() as u32;

        // Pre-compute total size: 4 (count) + for each name: 4 (length) + len(bytes)
        let size: usize = 4 + self.names.iter().map(|n| 4 + n.len()).sum::<usize>();
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(&count.to_le_bytes());

        for name in &self.names {
            let len = name.len() as u32;
            buf.extend_from_slice(&len.to_le_bytes());
            buf.extend_from_slice(name.as_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "texture names section too short for header",
            )));
        }

        let count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

        let mut offset = 4;
        let mut names = Vec::with_capacity(count);

        for i in 0..count {
            if offset + 4 > data.len() {
                return Err(FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!(
                        "texture names section truncated at name {i}: need length field at offset {offset}"
                    ),
                )));
            }

            let len = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4;

            if offset + len > data.len() {
                return Err(FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!(
                        "texture names section truncated at name {i}: need {len} bytes at offset {offset}, got {}",
                        data.len() - offset
                    ),
                )));
            }

            let name = String::from_utf8(data[offset..offset + len].to_vec()).map_err(|e| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("texture name {i} is not valid UTF-8: {e}"),
                ))
            })?;

            names.push(name);
            offset += len;
        }

        Ok(Self { names })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let section = TextureNamesSection {
            names: vec![
                "metal/floor_01".to_string(),
                "concrete/wall_03".to_string(),
                "trim/baseboard".to_string(),
            ],
        };

        let bytes = section.to_bytes();
        let restored = TextureNamesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn empty_round_trip() {
        let section = TextureNamesSection { names: vec![] };
        let bytes = section.to_bytes();
        let restored = TextureNamesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn preserves_order() {
        let section = TextureNamesSection {
            names: vec!["c".to_string(), "a".to_string(), "b".to_string()],
        };

        let bytes = section.to_bytes();
        let restored = TextureNamesSection::from_bytes(&bytes).unwrap();

        assert_eq!(restored.names[0], "c");
        assert_eq!(restored.names[1], "a");
        assert_eq!(restored.names[2], "b");
    }

    #[test]
    fn byte_layout_header() {
        let section = TextureNamesSection {
            names: vec!["abc".to_string(), "de".to_string()],
        };
        let bytes = section.to_bytes();

        // count = 2
        let count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(count, 2);

        // First name: length 3, then "abc"
        let len0 = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        assert_eq!(len0, 3);
        assert_eq!(&bytes[8..11], b"abc");

        // Second name: length 2, then "de"
        let len1 = u32::from_le_bytes([bytes[11], bytes[12], bytes[13], bytes[14]]);
        assert_eq!(len1, 2);
        assert_eq!(&bytes[15..17], b"de");
    }

    #[test]
    fn single_name_round_trip() {
        let section = TextureNamesSection {
            names: vec!["textures/metal/floor_01".to_string()],
        };
        let bytes = section.to_bytes();
        let restored = TextureNamesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn empty_string_name_round_trip() {
        let section = TextureNamesSection {
            names: vec!["".to_string(), "nonempty".to_string(), "".to_string()],
        };
        let bytes = section.to_bytes();
        let restored = TextureNamesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn rejects_truncated_header() {
        let result = TextureNamesSection::from_bytes(&[0; 2]);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_truncated_name_length() {
        // Header says 1 name but no length field follows
        let mut data = vec![0u8; 4];
        data[0] = 1; // count = 1
        let result = TextureNamesSection::from_bytes(&data);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_truncated_name_data() {
        // Header says 1 name, length says 10 but only 2 bytes follow
        let mut data = vec![0u8; 10];
        data[0..4].copy_from_slice(&1u32.to_le_bytes()); // count = 1
        data[4..8].copy_from_slice(&10u32.to_le_bytes()); // length = 10
        let result = TextureNamesSection::from_bytes(&data);
        assert!(result.is_err());
    }
}
