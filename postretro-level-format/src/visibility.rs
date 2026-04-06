// Cluster visibility section: per-cluster AABBs, face ranges, and compressed PVS.
// See: context/plans/ready/prl-phase-1-minimum-viable-compiler/

use crate::FormatError;

/// Per-cluster data within the visibility section.
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterInfo {
    /// Bounding volume in engine coordinates (Y-up).
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// Starting face index in the Geometry section's face metadata array.
    pub face_start: u32,
    /// Number of faces in this cluster.
    pub face_count: u32,
    /// Byte offset into the compressed PVS blob.
    pub pvs_offset: u32,
    /// Byte length of this cluster's compressed PVS data.
    pub pvs_size: u32,
}

/// Cluster visibility section: bounding volumes, face ranges, and PVS bitsets.
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterVisibilitySection {
    pub clusters: Vec<ClusterInfo>,
    /// RLE-compressed PVS blob shared by all clusters.
    pub pvs_data: Vec<u8>,
}

// On-disk layout (all little-endian):
//   u32  cluster_count
//   Per cluster (32 bytes each):
//     f32 * 3  bounds_min (x, y, z)
//     f32 * 3  bounds_max (x, y, z)
//     u32      face_start
//     u32      face_count
//     u32      pvs_offset
//     u32      pvs_size
//   u8 * N     compressed PVS blob

const CLUSTER_ENTRY_SIZE: usize = 40;

impl ClusterVisibilitySection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let cluster_count = self.clusters.len() as u32;
        let size = 4 + (self.clusters.len() * CLUSTER_ENTRY_SIZE) + self.pvs_data.len();
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(&cluster_count.to_le_bytes());

        for c in &self.clusters {
            buf.extend_from_slice(&c.bounds_min[0].to_le_bytes());
            buf.extend_from_slice(&c.bounds_min[1].to_le_bytes());
            buf.extend_from_slice(&c.bounds_min[2].to_le_bytes());
            buf.extend_from_slice(&c.bounds_max[0].to_le_bytes());
            buf.extend_from_slice(&c.bounds_max[1].to_le_bytes());
            buf.extend_from_slice(&c.bounds_max[2].to_le_bytes());
            buf.extend_from_slice(&c.face_start.to_le_bytes());
            buf.extend_from_slice(&c.face_count.to_le_bytes());
            buf.extend_from_slice(&c.pvs_offset.to_le_bytes());
            buf.extend_from_slice(&c.pvs_size.to_le_bytes());
        }

        buf.extend_from_slice(&self.pvs_data);

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "visibility section too short for header",
            )));
        }

        let cluster_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

        let table_end = 4 + cluster_count * CLUSTER_ENTRY_SIZE;
        if data.len() < table_end {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "visibility section too short for cluster table: need {table_end} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut clusters = Vec::with_capacity(cluster_count);
        for i in 0..cluster_count {
            let base = 4 + i * CLUSTER_ENTRY_SIZE;
            let f = |off: usize| -> f32 {
                f32::from_le_bytes([
                    data[base + off],
                    data[base + off + 1],
                    data[base + off + 2],
                    data[base + off + 3],
                ])
            };
            let u = |off: usize| -> u32 {
                u32::from_le_bytes([
                    data[base + off],
                    data[base + off + 1],
                    data[base + off + 2],
                    data[base + off + 3],
                ])
            };

            clusters.push(ClusterInfo {
                bounds_min: [f(0), f(4), f(8)],
                bounds_max: [f(12), f(16), f(20)],
                face_start: u(24),
                face_count: u(28),
                pvs_offset: u(32),
                pvs_size: u(36),
            });
        }

        let pvs_data = data[table_end..].to_vec();

        Ok(Self { clusters, pvs_data })
    }
}

/// Compress a PVS bitset using run-length encoding.
///
/// Zero bytes are encoded as `0x00, count` where count is the number of
/// consecutive zero bytes (1..=255). Non-zero bytes are stored verbatim.
pub fn compress_pvs(uncompressed: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < uncompressed.len() {
        if uncompressed[i] == 0 {
            let mut run = 0u8;
            while i < uncompressed.len() && uncompressed[i] == 0 && run < 255 {
                run += 1;
                i += 1;
            }
            out.push(0x00);
            out.push(run);
        } else {
            out.push(uncompressed[i]);
            i += 1;
        }
    }
    out
}

/// Decompress an RLE-compressed PVS bitset.
///
/// `output_len` is the expected uncompressed size in bytes.
pub fn decompress_pvs(compressed: &[u8], output_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(output_len);
    let mut i = 0;
    while i < compressed.len() && out.len() < output_len {
        if compressed[i] == 0 {
            i += 1;
            let count = if i < compressed.len() {
                compressed[i] as usize
            } else {
                0
            };
            i += 1;
            let zeros = count.min(output_len - out.len());
            out.extend(std::iter::repeat_n(0u8, zeros));
        } else {
            out.push(compressed[i]);
            i += 1;
        }
    }
    // Pad if decompressed data is shorter than expected
    out.resize(output_len, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- RLE round-trip tests --

    #[test]
    fn rle_all_zeros() {
        let input = vec![0u8; 32];
        let compressed = compress_pvs(&input);
        let decompressed = decompress_pvs(&compressed, input.len());
        assert_eq!(decompressed, input);
        // Should be much shorter than the input
        assert!(compressed.len() < input.len());
    }

    #[test]
    fn rle_all_ones() {
        let input = vec![0xFF; 16];
        let compressed = compress_pvs(&input);
        let decompressed = decompress_pvs(&compressed, input.len());
        assert_eq!(decompressed, input);
    }

    #[test]
    fn rle_sparse() {
        // Mostly zeros with a few set bits
        let mut input = vec![0u8; 64];
        input[7] = 0x01;
        input[31] = 0x80;
        input[63] = 0x42;
        let compressed = compress_pvs(&input);
        let decompressed = decompress_pvs(&compressed, input.len());
        assert_eq!(decompressed, input);
    }

    #[test]
    fn rle_dense() {
        // Mostly non-zero
        let input: Vec<u8> = (0..64)
            .map(|i| if i % 3 == 0 { 0 } else { (i + 1) as u8 })
            .collect();
        let compressed = compress_pvs(&input);
        let decompressed = decompress_pvs(&compressed, input.len());
        assert_eq!(decompressed, input);
    }

    #[test]
    fn rle_empty() {
        let input: Vec<u8> = vec![];
        let compressed = compress_pvs(&input);
        let decompressed = decompress_pvs(&compressed, 0);
        assert_eq!(decompressed, input);
    }

    #[test]
    fn rle_long_zero_run() {
        // More than 255 consecutive zeros
        let input = vec![0u8; 300];
        let compressed = compress_pvs(&input);
        let decompressed = decompress_pvs(&compressed, input.len());
        assert_eq!(decompressed, input);
    }

    #[test]
    fn rle_single_nonzero_byte() {
        let input = vec![0xAB];
        let compressed = compress_pvs(&input);
        let decompressed = decompress_pvs(&compressed, input.len());
        assert_eq!(decompressed, input);
    }

    // -- Section serialization round-trip --

    #[test]
    fn section_round_trip() {
        let section = ClusterVisibilitySection {
            clusters: vec![
                ClusterInfo {
                    bounds_min: [1.0, 2.0, 3.0],
                    bounds_max: [4.0, 5.0, 6.0],
                    face_start: 0,
                    face_count: 10,
                    pvs_offset: 0,
                    pvs_size: 3,
                },
                ClusterInfo {
                    bounds_min: [7.0, 8.0, 9.0],
                    bounds_max: [10.0, 11.0, 12.0],
                    face_start: 10,
                    face_count: 5,
                    pvs_offset: 3,
                    pvs_size: 2,
                },
            ],
            pvs_data: vec![0xFF, 0x00, 0x01, 0xFF, 0x80],
        };

        let bytes = section.to_bytes();
        let restored = ClusterVisibilitySection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn empty_section_round_trip() {
        let section = ClusterVisibilitySection {
            clusters: vec![],
            pvs_data: vec![],
        };
        let bytes = section.to_bytes();
        let restored = ClusterVisibilitySection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn rejects_truncated_data() {
        let result = ClusterVisibilitySection::from_bytes(&[0; 2]);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_short_cluster_table() {
        // Header says 1 cluster but not enough data for the table entry
        let mut data = vec![0u8; 8];
        data[0] = 1; // cluster_count = 1
        let result = ClusterVisibilitySection::from_bytes(&data);
        assert!(result.is_err());
    }
}
