// Leaf PVS section: concatenated RLE-compressed PVS bitsets for all leaves.
// See: context/lib/build_pipeline.md §PRL

use crate::FormatError;

/// Leaf PVS section: concatenated blob of RLE-compressed PVS bitsets.
///
/// Each empty leaf's PVS bitset is RLE-compressed and stored contiguously in
/// the blob. The BspLeavesSection records reference into this blob via
/// `pvs_offset` and `pvs_size`. Solid leaves have `pvs_offset = 0` and
/// `pvs_size = 0`.
///
/// On-disk layout (all little-endian):
///   u32      total_size  (byte length of the compressed PVS blob)
///   u8 * N   compressed PVS blob
#[derive(Debug, Clone, PartialEq)]
pub struct LeafPvsSection {
    /// Concatenated RLE-compressed PVS bitsets.
    pub pvs_data: Vec<u8>,
}

impl LeafPvsSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let total_size = self.pvs_data.len() as u32;
        let size = 4 + self.pvs_data.len();
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(&total_size.to_le_bytes());
        buf.extend_from_slice(&self.pvs_data);

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "leaf PVS section too short for header",
            )));
        }

        let total_size = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

        let expected_size = 4 + total_size;
        if data.len() < expected_size {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "leaf PVS section too short: need {expected_size} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let pvs_data = data[4..4 + total_size].to_vec();

        Ok(Self { pvs_data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visibility::{compress_pvs, decompress_pvs};

    #[test]
    fn round_trip() {
        let section = LeafPvsSection {
            pvs_data: vec![0xFF, 0x00, 0x03, 0xAB, 0x00, 0x01],
        };

        let bytes = section.to_bytes();
        let restored = LeafPvsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn empty_round_trip() {
        let section = LeafPvsSection { pvs_data: vec![] };

        let bytes = section.to_bytes();
        let restored = LeafPvsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn rejects_truncated_header() {
        let result = LeafPvsSection::from_bytes(&[0; 2]);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_truncated_body() {
        // Header says 10 bytes but only 2 available
        let mut data = vec![0u8; 6];
        data[0..4].copy_from_slice(&10u32.to_le_bytes());
        let result = LeafPvsSection::from_bytes(&data);
        assert!(result.is_err());
    }

    #[test]
    fn stores_compressed_pvs_from_multiple_leaves() {
        // Simulate two leaves, each with a compressed PVS bitset.
        let bitset_0 = vec![0b0000_0011]; // leaf 0 sees leaves 0, 1
        let bitset_1 = vec![0b0000_0101]; // leaf 1 sees leaves 0, 2

        let compressed_0 = compress_pvs(&bitset_0);
        let compressed_1 = compress_pvs(&bitset_1);

        let mut pvs_data = Vec::new();
        let offset_0 = pvs_data.len() as u32;
        let size_0 = compressed_0.len() as u32;
        pvs_data.extend_from_slice(&compressed_0);

        let offset_1 = pvs_data.len() as u32;
        let size_1 = compressed_1.len() as u32;
        pvs_data.extend_from_slice(&compressed_1);

        let section = LeafPvsSection { pvs_data };

        let bytes = section.to_bytes();
        let restored = LeafPvsSection::from_bytes(&bytes).unwrap();

        // Decompress leaf 0's PVS from the restored section
        let slice_0 = &restored.pvs_data[offset_0 as usize..(offset_0 + size_0) as usize];
        let decompressed_0 = decompress_pvs(slice_0, 1);
        assert_eq!(decompressed_0, bitset_0);

        // Decompress leaf 1's PVS from the restored section
        let slice_1 = &restored.pvs_data[offset_1 as usize..(offset_1 + size_1) as usize];
        let decompressed_1 = decompress_pvs(slice_1, 1);
        assert_eq!(decompressed_1, bitset_1);
    }
}
