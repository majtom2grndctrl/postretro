// TextureCacheKeys PRL section (ID 32): per-texture blake3 cache keys.
//
// One 32-byte key per entry in `TextureNames`, in the same order. Each key is
// the blake3 of the raw PNG source bytes for that texture. The runtime uses
// these keys to find the matching baked `.prm` mip sidecar (see the `prm`
// module) without rehashing PNGs at load time. prl-build (texture_mips.rs)
// writes the section; the runtime (render/loaded_texture.rs) reads it.
//
// See: context/lib/build_pipeline.md §PRL section IDs

use crate::FormatError;

/// Flat array of 32-byte blake3 cache keys, one per texture name (same index
/// order as `TextureNamesSection.names`). Empty when no textures are
/// referenced; the compiler still emits the section so the runtime can rely
/// on a fixed presence model.
///
/// On-disk layout (little-endian):
///   u32           count   -- number of keys (must equal TextureNames.count)
///   [u8; 32] × count      -- raw blake3 digests, no padding
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TextureCacheKeysSection {
    pub keys: Vec<[u8; 32]>,
}

impl TextureCacheKeysSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let count = self.keys.len() as u32;
        let size = 4 + self.keys.len() * 32;
        let mut buf = Vec::with_capacity(size);
        buf.extend_from_slice(&count.to_le_bytes());
        for key in &self.keys {
            buf.extend_from_slice(key);
        }
        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "texture cache keys section too short for header",
            )));
        }
        let count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let needed = 4 + count * 32;
        if data.len() < needed {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "texture cache keys section truncated: need {needed} bytes for {count} keys, got {}",
                    data.len()
                ),
            )));
        }
        let mut keys = Vec::with_capacity(count);
        let mut offset = 4;
        for _ in 0..count {
            let mut key = [0u8; 32];
            key.copy_from_slice(&data[offset..offset + 32]);
            keys.push(key);
            offset += 32;
        }
        Ok(Self { keys })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let section = TextureCacheKeysSection {
            keys: vec![[0u8; 32], [0xAB; 32], {
                let mut k = [0u8; 32];
                for (i, b) in k.iter_mut().enumerate() {
                    *b = i as u8;
                }
                k
            }],
        };
        let bytes = section.to_bytes();
        let restored = TextureCacheKeysSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn empty_round_trip() {
        let section = TextureCacheKeysSection { keys: vec![] };
        let bytes = section.to_bytes();
        assert_eq!(bytes, vec![0, 0, 0, 0]);
        let restored = TextureCacheKeysSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn byte_layout_header() {
        let section = TextureCacheKeysSection {
            keys: vec![[0xCD; 32]],
        };
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), 4 + 32);
        let count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(count, 1);
        assert_eq!(&bytes[4..36], &[0xCD; 32]);
    }

    #[test]
    fn rejects_truncated_header() {
        let result = TextureCacheKeysSection::from_bytes(&[0; 2]);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_truncated_keys() {
        // Header says 2 keys (= 64 bytes) but only 32 follow.
        let mut data = vec![0u8; 4 + 32];
        data[0..4].copy_from_slice(&2u32.to_le_bytes());
        let result = TextureCacheKeysSection::from_bytes(&data);
        assert!(result.is_err());
    }
}
