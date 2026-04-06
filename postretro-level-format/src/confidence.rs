// Visibility confidence section: per-cluster-pair ray pass-through ratios.
// See: context/lib/development_guide.md

use crate::FormatError;

/// Per-cluster-pair visibility confidence: ratio of unblocked rays.
///
/// Stored as a flat `cluster_count * cluster_count` f32 matrix where
/// `data[i * cluster_count + j]` is the confidence for cluster pair (i, j).
/// Values range from 0.0 (fully blocked) to 1.0 (wide open / adjacent).
#[derive(Debug, Clone, PartialEq)]
pub struct VisibilityConfidenceSection {
    pub cluster_count: u32,
    /// Flat row-major matrix of f32 confidence values.
    pub data: Vec<f32>,
}

// On-disk layout (all little-endian):
//   u32  cluster_count
//   f32 * (cluster_count * cluster_count)  confidence matrix

impl VisibilityConfidenceSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let float_count = self.cluster_count as usize * self.cluster_count as usize;
        let size = 4 + float_count * 4;
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(&self.cluster_count.to_le_bytes());
        for &val in &self.data {
            buf.extend_from_slice(&val.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "confidence section too short for header",
            )));
        }

        let cluster_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let float_count = cluster_count as usize * cluster_count as usize;
        let expected_len = 4 + float_count * 4;

        if data.len() < expected_len {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "confidence section too short: need {} bytes, got {}",
                    expected_len,
                    data.len()
                ),
            )));
        }

        let mut values = Vec::with_capacity(float_count);
        for i in 0..float_count {
            let offset = 4 + i * 4;
            let val = f32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            values.push(val);
        }

        Ok(Self {
            cluster_count,
            data: values,
        })
    }

    /// Look up confidence for a specific cluster pair.
    pub fn get(&self, from: usize, to: usize) -> f32 {
        let idx = from * self.cluster_count as usize + to;
        self.data.get(idx).copied().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let section = VisibilityConfidenceSection {
            cluster_count: 0,
            data: vec![],
        };
        let bytes = section.to_bytes();
        let restored = VisibilityConfidenceSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_2x2() {
        let section = VisibilityConfidenceSection {
            cluster_count: 2,
            data: vec![1.0, 0.75, 0.75, 1.0],
        };
        let bytes = section.to_bytes();
        let restored = VisibilityConfidenceSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn get_returns_correct_value() {
        let section = VisibilityConfidenceSection {
            cluster_count: 3,
            data: vec![
                1.0, 0.5, 0.3, // row 0
                0.5, 1.0, 0.8, // row 1
                0.3, 0.8, 1.0, // row 2
            ],
        };
        assert!((section.get(0, 1) - 0.5).abs() < 1e-6);
        assert!((section.get(1, 2) - 0.8).abs() < 1e-6);
        assert!((section.get(2, 0) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn get_out_of_bounds_returns_zero() {
        let section = VisibilityConfidenceSection {
            cluster_count: 1,
            data: vec![1.0],
        };
        assert!((section.get(5, 5)).abs() < 1e-6);
    }

    #[test]
    fn rejects_truncated_data() {
        let result = VisibilityConfidenceSection::from_bytes(&[0; 2]);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_short_matrix() {
        // Header says 2 clusters but not enough data for 4 floats
        let mut data = vec![0u8; 8];
        data[0] = 2; // cluster_count = 2
        let result = VisibilityConfidenceSection::from_bytes(&data);
        assert!(result.is_err());
    }
}
