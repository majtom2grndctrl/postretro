// CellDrawIndex PRL section (ID 37): each cell's owned BVH-leaf spans, baked as
// a CSR (offset table + flat span payload) so the runtime camera cull gathers
// only visible cells' leaves as candidates.
// See: context/lib/build_pipeline.md §PRL Compilation

use crate::FormatError;

/// Current section-internal version emitted by [`CellDrawIndexSection::to_bytes`].
pub const CELL_DRAW_INDEX_VERSION: u32 = 1;

/// Header bytes before the offset table: version(4) + cell_count(4)
/// + span_count(4) + reserved(4).
pub const HEADER_SIZE: usize = 16;
/// Bytes per offset-table entry (`u32`).
pub const OFFSET_STRIDE: usize = 4;
/// Bytes per [`Span`] record: leaf_start(4) + leaf_count(4).
pub const SPAN_STRIDE: usize = 8;

/// A contiguous run of BVH leaves owned by a cell. `leaf_start` indexes the flat
/// BVH leaf array; `leaf_count` is the run length. Empty spans (`leaf_count == 0`)
/// are invalid — a cell with no owned leaves is represented by a zero-length CSR
/// row, not by a present-but-empty span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub leaf_start: u32,
    pub leaf_count: u32,
}

/// CellDrawIndex section (ID 37): per-cell BVH-leaf spans in CSR layout.
///
/// On-disk layout (little-endian):
///
/// ```text
///   Header (16 bytes):
///     u32  version       (= 1)
///     u32  cell_count
///     u32  span_count    (length of the flat spans array)
///     u32  reserved      (= 0)
///
///   Offset table (4 bytes × (cell_count + 1)):
///     u32  cell_span_offset[i]   CSR row pointer; non-decreasing,
///                                cell_span_offset[0] == 0,
///                                cell_span_offset[cell_count] == span_count
///
///   Span records (8 bytes × span_count):
///     u32  leaf_start
///     u32  leaf_count    (>= 1)
/// ```
///
/// Cell `i` owns spans `cell_span_offset[i] .. cell_span_offset[i + 1]`. The CSR
/// shape lets the runtime gather a visible cell's leaf candidates with one slice.
///
/// `from_bytes` validates only structural / self-consistent invariants. It does
/// NOT validate spans against the BVH leaf array, cell drawability, or material
/// buckets — those cross-section checks live in the runtime loader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellDrawIndexSection {
    pub cell_count: u32,
    pub span_count: u32,
    /// CSR row pointers, length `cell_count + 1`. Non-decreasing, starts at 0,
    /// ends at `span_count`.
    pub cell_span_offset: Vec<u32>,
    /// Flat span payload, length `span_count`.
    pub spans: Vec<Span>,
}

impl CellDrawIndexSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(
            HEADER_SIZE
                + self.cell_span_offset.len() * OFFSET_STRIDE
                + self.spans.len() * SPAN_STRIDE,
        );

        buf.extend_from_slice(&CELL_DRAW_INDEX_VERSION.to_le_bytes());
        buf.extend_from_slice(&self.cell_count.to_le_bytes());
        buf.extend_from_slice(&self.span_count.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // reserved

        for offset in &self.cell_span_offset {
            buf.extend_from_slice(&offset.to_le_bytes());
        }

        for span in &self.spans {
            buf.extend_from_slice(&span.leaf_start.to_le_bytes());
            buf.extend_from_slice(&span.leaf_count.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(invalid_data(
                "cell draw index section too short for header",
            ));
        }

        let version = read_u32(data, 0);
        if version != CELL_DRAW_INDEX_VERSION {
            return Err(invalid_data(format!(
                "cell draw index section version {version}, expected {CELL_DRAW_INDEX_VERSION}"
            )));
        }

        let cell_count = read_u32(data, 4);
        let span_count = read_u32(data, 8);
        let reserved = read_u32(data, 12);
        if reserved != 0 {
            return Err(invalid_data(format!(
                "cell draw index section reserved field must be 0, got {reserved}"
            )));
        }

        // Offset table has cell_count + 1 entries. Compute the exact expected
        // section length and require an exact match (reject both truncation and
        // trailing bytes). usize math on u32-derived counts cannot overflow on
        // any 64-bit target the engine builds for.
        let offset_entries = cell_count as usize + 1;
        let expected_len =
            HEADER_SIZE + offset_entries * OFFSET_STRIDE + span_count as usize * SPAN_STRIDE;
        if data.len() != expected_len {
            return Err(invalid_data(format!(
                "cell draw index section length mismatch: expected {expected_len} bytes for \
                 cell_count {cell_count} and span_count {span_count}, got {}",
                data.len()
            )));
        }

        let mut cell_span_offset = Vec::with_capacity(offset_entries);
        let mut cursor = HEADER_SIZE;
        for _ in 0..offset_entries {
            cell_span_offset.push(read_u32(data, cursor));
            cursor += OFFSET_STRIDE;
        }

        // CSR invariants on the offset table.
        if cell_span_offset[0] != 0 {
            return Err(invalid_data(format!(
                "cell draw index offset table must start at 0, got {}",
                cell_span_offset[0]
            )));
        }
        for window in cell_span_offset.windows(2) {
            if window[1] < window[0] {
                return Err(invalid_data(format!(
                    "cell draw index offset table must be non-decreasing, got {} after {}",
                    window[1], window[0]
                )));
            }
        }
        // offset_entries == cell_count + 1 >= 1, so the last index is in bounds.
        let last_offset = cell_span_offset[offset_entries - 1];
        if last_offset != span_count {
            return Err(invalid_data(format!(
                "cell draw index final offset {last_offset} must equal span_count {span_count}"
            )));
        }

        let mut spans = Vec::with_capacity(span_count as usize);
        for _ in 0..span_count {
            let leaf_start = read_u32(data, cursor);
            let leaf_count = read_u32(data, cursor + 4);
            cursor += SPAN_STRIDE;

            if leaf_count == 0 {
                return Err(invalid_data(
                    "cell draw index span has leaf_count 0 (empty spans are invalid)",
                ));
            }
            if leaf_start.checked_add(leaf_count).is_none() {
                return Err(invalid_data(format!(
                    "cell draw index span leaf_start {leaf_start} + leaf_count {leaf_count} \
                     overflows u32"
                )));
            }

            spans.push(Span {
                leaf_start,
                leaf_count,
            });
        }

        Ok(Self {
            cell_count,
            span_count,
            cell_span_offset,
            spans,
        })
    }
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn invalid_data(msg: impl Into<String>) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        msg.into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-formed multi-cell section: 3 cells, cell 1 owns no leaves.
    fn sample_section() -> CellDrawIndexSection {
        CellDrawIndexSection {
            cell_count: 3,
            span_count: 3,
            // cell 0 -> spans [0,2), cell 1 -> [2,2) (empty row), cell 2 -> [2,3).
            cell_span_offset: vec![0, 2, 2, 3],
            spans: vec![
                Span {
                    leaf_start: 0,
                    leaf_count: 4,
                },
                Span {
                    leaf_start: 4,
                    leaf_count: 1,
                },
                Span {
                    leaf_start: 10,
                    leaf_count: 7,
                },
            ],
        }
    }

    /// The minimal valid section: zero cells, zero spans. The offset table still
    /// carries one entry (`cell_count + 1`), which must be 0 == span_count.
    fn empty_section() -> CellDrawIndexSection {
        CellDrawIndexSection {
            cell_count: 0,
            span_count: 0,
            cell_span_offset: vec![0],
            spans: vec![],
        }
    }

    #[test]
    fn round_trip_byte_identical() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = CellDrawIndexSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
        let rebytes = restored.to_bytes();
        assert_eq!(bytes, rebytes);
    }

    #[test]
    fn round_trip_empty_section() {
        let section = empty_section();
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), HEADER_SIZE + OFFSET_STRIDE);
        let restored = CellDrawIndexSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn to_bytes_matches_expected_length_formula() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let expected = 16 + 4 * (section.cell_count as usize + 1) + 8 * section.span_count as usize;
        assert_eq!(bytes.len(), expected);
    }

    fn assert_invalid_data(err: FormatError) {
        match err {
            FormatError::Io(e) => {
                assert_eq!(e.kind(), std::io::ErrorKind::InvalidData, "wrong io kind");
            }
            other => panic!("expected FormatError::Io(InvalidData), got {other:?}"),
        }
    }

    #[test]
    fn rejects_too_short_for_header() {
        let err = CellDrawIndexSection::from_bytes(&[0u8; HEADER_SIZE - 1]).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn rejects_unsupported_version() {
        let mut bytes = sample_section().to_bytes();
        bytes[0..4].copy_from_slice(&2u32.to_le_bytes());
        let err = CellDrawIndexSection::from_bytes(&bytes).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn rejects_nonzero_reserved() {
        let mut bytes = sample_section().to_bytes();
        bytes[12..16].copy_from_slice(&1u32.to_le_bytes());
        let err = CellDrawIndexSection::from_bytes(&bytes).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn rejects_truncated_body() {
        // Drop the last span's worth of bytes: declared counts no longer fit.
        let mut bytes = sample_section().to_bytes();
        bytes.truncate(bytes.len() - SPAN_STRIDE);
        let err = CellDrawIndexSection::from_bytes(&bytes).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut bytes = sample_section().to_bytes();
        bytes.push(0xFF);
        let err = CellDrawIndexSection::from_bytes(&bytes).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn rejects_non_monotonic_offsets() {
        // cell_span_offset = [0, 2, 1, 3] — entry 2 < entry 1.
        let section = CellDrawIndexSection {
            cell_count: 3,
            span_count: 3,
            cell_span_offset: vec![0, 2, 1, 3],
            spans: vec![
                Span {
                    leaf_start: 0,
                    leaf_count: 1,
                },
                Span {
                    leaf_start: 1,
                    leaf_count: 1,
                },
                Span {
                    leaf_start: 2,
                    leaf_count: 1,
                },
            ],
        };
        let err = CellDrawIndexSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn rejects_nonzero_first_offset() {
        let section = CellDrawIndexSection {
            cell_count: 1,
            span_count: 1,
            cell_span_offset: vec![1, 1],
            spans: vec![Span {
                leaf_start: 0,
                leaf_count: 1,
            }],
        };
        let err = CellDrawIndexSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn rejects_final_offset_not_equal_span_count() {
        // Final offset 2 but span_count declared 3 (and 3 spans present, so the
        // length still matches — the mismatch is purely the CSR terminator).
        let section = CellDrawIndexSection {
            cell_count: 2,
            span_count: 3,
            cell_span_offset: vec![0, 1, 2],
            spans: vec![
                Span {
                    leaf_start: 0,
                    leaf_count: 1,
                },
                Span {
                    leaf_start: 1,
                    leaf_count: 1,
                },
                Span {
                    leaf_start: 2,
                    leaf_count: 1,
                },
            ],
        };
        let err = CellDrawIndexSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn rejects_span_overflow() {
        let section = CellDrawIndexSection {
            cell_count: 1,
            span_count: 1,
            cell_span_offset: vec![0, 1],
            spans: vec![Span {
                leaf_start: u32::MAX,
                leaf_count: 1,
            }],
        };
        let err = CellDrawIndexSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn rejects_empty_span() {
        let section = CellDrawIndexSection {
            cell_count: 1,
            span_count: 1,
            cell_span_offset: vec![0, 1],
            spans: vec![Span {
                leaf_start: 5,
                leaf_count: 0,
            }],
        };
        let err = CellDrawIndexSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert_invalid_data(err);
    }
}
