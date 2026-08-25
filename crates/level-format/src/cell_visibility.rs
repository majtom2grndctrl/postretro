// CellVisibility PRL section (ID 46): static cell-to-cell portal-graph coupling.
// See: context/plans/in-progress/cell-visibility-relation/index.md

use crate::FormatError;

pub const CELL_VISIBILITY_VERSION: u32 = 1;
/// Fixed-point units per world metre for `CoupledPairRecord::distance`.
pub const CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE: u32 = 1024;
/// Fixed-point units per world metre for `CoupledPairRecord::aperture`.
pub const CELL_VISIBILITY_APERTURE_FIXED_POINT_SCALE: u32 = 1024;
/// Maximum fixed-point distance stored in the graded side table (16 km).
///
/// Distances at or below this cap are stored; greater values remain
/// perceivable but have no graded detail.
pub const CELL_VISIBILITY_DISTANCE_CAP: u32 = 16 * 1024 * 1024;
/// Maximum number of directed graded selections retained for one source cell.
///
/// The stored table is the union of these directed selections, so this bounds
/// the total pair count at `cell_count * K` but does not bound a cell's final
/// undirected degree.
pub const CELL_VISIBILITY_FANOUT_K: usize = 32;

const HEADER_SIZE: usize = 8;
const PAIR_COUNT_SIZE: usize = 4;
const PAIR_RECORD_SIZE: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoupledPairRecord {
    pub cell_a: u32,
    pub cell_b: u32,
    pub distance: u32,
    pub aperture: u32,
}

/// The version-one cell coupling payload.
///
/// `component_ids[cell]` is the conservative reachability gate. Graded
/// records are canonical unordered pairs, sorted by `(cell_a, cell_b)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellVisibilitySection {
    pub cell_count: u32,
    pub component_ids: Vec<u32>,
    pub coupled_pairs: Vec<CoupledPairRecord>,
}

impl CellVisibilitySection {
    /// Serialize the complete version-one layout.
    ///
    /// Count and byte-size conversions are checked before allocating so an
    /// oversized side table aborts the bake instead of truncating its wire
    /// count or wrapping the requested capacity.
    pub fn to_bytes(&self) -> crate::Result<Vec<u8>> {
        self.validate()?;
        let pair_count = checked_pair_count(self.coupled_pairs.len())?;
        let output_len = checked_section_len(self.cell_count, pair_count)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(output_len).map_err(|error| {
            invalid(format!(
                "CellVisibility section cannot reserve {output_len} output bytes: {error}"
            ))
        })?;

        bytes.extend_from_slice(&CELL_VISIBILITY_VERSION.to_le_bytes());
        bytes.extend_from_slice(&self.cell_count.to_le_bytes());
        for component_id in &self.component_ids {
            bytes.extend_from_slice(&component_id.to_le_bytes());
        }
        bytes.extend_from_slice(&pair_count.to_le_bytes());
        for pair in &self.coupled_pairs {
            bytes.extend_from_slice(&pair.cell_a.to_le_bytes());
            bytes.extend_from_slice(&pair.cell_b.to_le_bytes());
            bytes.extend_from_slice(&pair.distance.to_le_bytes());
            bytes.extend_from_slice(&pair.aperture.to_le_bytes());
        }
        Ok(bytes)
    }

    pub fn from_bytes(data: &[u8], expected_cell_count: u32) -> crate::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(invalid(format!(
                "CellVisibility section too short for header: need {HEADER_SIZE} bytes, got {}",
                data.len()
            )));
        }
        if expected_cell_count == 0 {
            return Err(invalid(
                "CellVisibility section requires expected_cell_count greater than zero",
            ));
        }

        let version = read_u32(data, 0);
        if version != CELL_VISIBILITY_VERSION {
            return Err(invalid(format!(
                "CellVisibility section version {version}, expected {CELL_VISIBILITY_VERSION}"
            )));
        }
        let cell_count = read_u32(data, 4);
        if cell_count == 0 {
            return Err(invalid(
                "CellVisibility section cell_count must be greater than zero",
            ));
        }
        if cell_count != expected_cell_count {
            return Err(invalid(format!(
                "CellVisibility section cell_count {cell_count} does not match Cells count {expected_cell_count}"
            )));
        }

        let component_bytes = checked_bytes(cell_count, std::mem::size_of::<u32>(), "cell_count")?;
        let pair_count_offset = HEADER_SIZE
            .checked_add(component_bytes)
            .ok_or_else(|| invalid("CellVisibility component-id offset overflow"))?;
        let pair_count_end = pair_count_offset
            .checked_add(PAIR_COUNT_SIZE)
            .ok_or_else(|| invalid("CellVisibility pair-count offset overflow"))?;
        if data.len() < pair_count_end {
            return Err(invalid(format!(
                "CellVisibility section truncated before pair count: need {pair_count_end} bytes, got {}",
                data.len()
            )));
        }

        let pair_count = read_u32(data, pair_count_offset);
        let pair_count_usize = usize_from_u32(pair_count, "pair_count")?;
        let max_pair_count = max_pair_count(cell_count)?;
        if pair_count_usize > max_pair_count {
            return Err(invalid(format!(
                "CellVisibility pair count {pair_count} exceeds {} cells × fanout {} = {max_pair_count}",
                cell_count, CELL_VISIBILITY_FANOUT_K
            )));
        }
        let pair_bytes = checked_bytes(pair_count, PAIR_RECORD_SIZE, "pair_count")?;
        let expected_len = pair_count_end
            .checked_add(pair_bytes)
            .ok_or_else(|| invalid("CellVisibility pair-table length overflow"))?;
        if data.len() != expected_len {
            return Err(invalid(format!(
                "CellVisibility section length mismatch: expected {expected_len} bytes for {pair_count} pair record(s), got {}",
                data.len()
            )));
        }

        let component_count = usize_from_u32(cell_count, "cell_count")?;
        let component_ids = (0..component_count)
            .map(|index| read_u32(data, HEADER_SIZE + index * std::mem::size_of::<u32>()))
            .collect();
        let mut coupled_pairs = Vec::new();
        coupled_pairs
            .try_reserve_exact(pair_count_usize)
            .map_err(|error| {
                invalid(format!(
                    "CellVisibility section cannot reserve {pair_count} pair records: {error}"
                ))
            })?;
        let mut cursor = pair_count_end;
        for _ in 0..pair_count {
            coupled_pairs.push(CoupledPairRecord {
                cell_a: read_u32(data, cursor),
                cell_b: read_u32(data, cursor + 4),
                distance: read_u32(data, cursor + 8),
                aperture: read_u32(data, cursor + 12),
            });
            cursor += PAIR_RECORD_SIZE;
        }

        let section = Self {
            cell_count,
            component_ids,
            coupled_pairs,
        };
        section.validate()?;
        Ok(section)
    }

    /// Largest valid encoded byte length for a known Cells count.
    ///
    /// Loaders use this before materializing the optional section so a bogus
    /// container-table size cannot bypass the `N × K` pair-table bound.
    pub fn max_encoded_len(cell_count: u32) -> crate::Result<u64> {
        if cell_count == 0 {
            return Err(invalid(
                "CellVisibility section cell_count must be greater than zero",
            ));
        }
        let pair_count = u64::from(cell_count)
            .checked_mul(u64::try_from(CELL_VISIBILITY_FANOUT_K).expect("fanout fits u64"))
            .ok_or_else(|| invalid("CellVisibility maximum pair count overflow"))?;
        u64::try_from(HEADER_SIZE)
            .expect("header size fits u64")
            .checked_add(
                u64::from(cell_count)
                    .checked_mul(
                        u64::try_from(std::mem::size_of::<u32>()).expect("u32 size fits u64"),
                    )
                    .ok_or_else(|| invalid("CellVisibility component table size overflow"))?,
            )
            .and_then(|length| {
                length
                    .checked_add(u64::try_from(PAIR_COUNT_SIZE).expect("pair-count size fits u64"))
            })
            .and_then(|length| {
                length.checked_add(pair_count.checked_mul(
                    u64::try_from(PAIR_RECORD_SIZE).expect("pair record size fits u64"),
                )?)
            })
            .ok_or_else(|| invalid("CellVisibility maximum section length overflow"))
    }

    fn validate(&self) -> crate::Result<()> {
        if self.cell_count == 0 {
            return Err(invalid(
                "CellVisibility section cell_count must be greater than zero",
            ));
        }
        if self.component_ids.len() != usize_from_u32(self.cell_count, "cell_count")? {
            return Err(invalid(format!(
                "CellVisibility has {} component ids for cell_count {}",
                self.component_ids.len(),
                self.cell_count
            )));
        }
        let max_pair_count = max_pair_count(self.cell_count)?;
        if self.coupled_pairs.len() > max_pair_count {
            return Err(invalid(format!(
                "CellVisibility pair count {} exceeds {} cells × fanout {} = {max_pair_count}",
                self.coupled_pairs.len(),
                self.cell_count,
                CELL_VISIBILITY_FANOUT_K
            )));
        }

        let mut first_member = Vec::new();
        for (cell, &component) in self.component_ids.iter().enumerate() {
            let component = component as usize;
            if component > first_member.len() {
                return Err(invalid(format!(
                    "CellVisibility component id {component} at cell {cell} is not dense"
                )));
            }
            if component == first_member.len() {
                first_member.push(cell);
            }
        }

        let mut previous = None;
        for pair in &self.coupled_pairs {
            if pair.cell_a >= pair.cell_b {
                return Err(invalid(format!(
                    "CellVisibility pair ({}, {}) must satisfy cell_a < cell_b",
                    pair.cell_a, pair.cell_b
                )));
            }
            if pair.cell_b >= self.cell_count {
                return Err(invalid(format!(
                    "CellVisibility pair ({}, {}) references cell outside cell_count {}",
                    pair.cell_a, pair.cell_b, self.cell_count
                )));
            }
            if self.component_ids[pair.cell_a as usize] != self.component_ids[pair.cell_b as usize]
            {
                return Err(invalid(format!(
                    "CellVisibility pair ({}, {}) spans different components",
                    pair.cell_a, pair.cell_b
                )));
            }
            if pair.distance > CELL_VISIBILITY_DISTANCE_CAP {
                return Err(invalid(format!(
                    "CellVisibility pair ({}, {}) distance {} exceeds cap {}",
                    pair.cell_a, pair.cell_b, pair.distance, CELL_VISIBILITY_DISTANCE_CAP
                )));
            }
            let key = (pair.cell_a, pair.cell_b);
            if previous.is_some_and(|previous| previous >= key) {
                return Err(invalid(
                    "CellVisibility pair table must be strictly ascending by (cell_a, cell_b)",
                ));
            }
            previous = Some(key);
        }
        Ok(())
    }
}

fn checked_bytes(count: u32, stride: usize, name: &'static str) -> crate::Result<usize> {
    usize_from_u32(count, name)?.checked_mul(stride).ok_or_else(|| {
        invalid(format!(
            "CellVisibility section count multiplication overflow for {name} {count} * stride {stride}"
        ))
    })
}

fn checked_pair_count(pair_count: usize) -> crate::Result<u32> {
    u32::try_from(pair_count).map_err(|_| {
        invalid(format!(
            "CellVisibility section pair count {pair_count} exceeds the u32 wire limit"
        ))
    })
}

fn max_pair_count(cell_count: u32) -> crate::Result<usize> {
    usize_from_u32(cell_count, "cell_count")?
        .checked_mul(CELL_VISIBILITY_FANOUT_K)
        .ok_or_else(|| invalid("CellVisibility maximum pair count overflow"))
}

fn checked_section_len(cell_count: u32, pair_count: u32) -> crate::Result<usize> {
    let component_bytes = checked_bytes(cell_count, std::mem::size_of::<u32>(), "cell_count")?;
    let pair_bytes = checked_bytes(pair_count, PAIR_RECORD_SIZE, "pair_count")?;
    HEADER_SIZE
        .checked_add(component_bytes)
        .and_then(|length| length.checked_add(PAIR_COUNT_SIZE))
        .and_then(|length| length.checked_add(pair_bytes))
        .ok_or_else(|| invalid("CellVisibility section output length overflow"))
}

fn usize_from_u32(value: u32, name: &'static str) -> crate::Result<usize> {
    usize::try_from(value).map_err(|_| {
        invalid(format!(
            "CellVisibility section {name} {value} does not fit the host index size"
        ))
    })
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn invalid(message: impl Into<String>) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SectionId;

    fn valid_section() -> CellVisibilitySection {
        CellVisibilitySection {
            cell_count: 4,
            component_ids: vec![0, 0, 1, 2],
            coupled_pairs: vec![CoupledPairRecord {
                cell_a: 0,
                cell_b: 1,
                distance: 32,
                aperture: 16,
            }],
        }
    }

    fn assert_invalid_data(error: FormatError) {
        match error {
            FormatError::Io(error) => assert_eq!(error.kind(), std::io::ErrorKind::InvalidData),
            other => panic!("expected FormatError::Io(InvalidData), got {other:?}"),
        }
    }

    #[test]
    fn cell_visibility_section_id_registered() {
        assert_eq!(SectionId::CellVisibility as u32, 46);
        assert_eq!(SectionId::from_u32(46), Some(SectionId::CellVisibility));
    }

    #[test]
    fn cell_visibility_v1_constants_are_pinned() {
        assert_eq!(CELL_VISIBILITY_VERSION, 1);
        assert_eq!(CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE, 1024);
        assert_eq!(CELL_VISIBILITY_APERTURE_FIXED_POINT_SCALE, 1024);
        assert_eq!(CELL_VISIBILITY_DISTANCE_CAP, 16 * 1024 * 1024);
        assert_eq!(CELL_VISIBILITY_FANOUT_K, 32);
    }

    #[test]
    fn round_trip_preserves_fixed_layout() {
        let section = valid_section();
        let bytes = section.to_bytes().unwrap();
        assert_eq!(
            CellVisibilitySection::from_bytes(&bytes, 4).unwrap(),
            section
        );
    }

    #[test]
    fn empty_graded_table_round_trips() {
        let section = CellVisibilitySection {
            cell_count: 2,
            component_ids: vec![0, 1],
            coupled_pairs: Vec::new(),
        };
        assert_eq!(
            CellVisibilitySection::from_bytes(&section.to_bytes().unwrap(), 2).unwrap(),
            section
        );
    }

    #[test]
    fn rejects_version_cell_count_truncation_and_trailing_data() {
        let section = valid_section();
        let mut bad_version = section.to_bytes().unwrap();
        bad_version[..4].copy_from_slice(&2u32.to_le_bytes());
        assert_invalid_data(CellVisibilitySection::from_bytes(&bad_version, 4).unwrap_err());
        assert_invalid_data(
            CellVisibilitySection::from_bytes(&section.to_bytes().unwrap(), 3).unwrap_err(),
        );

        let bytes = section.to_bytes().unwrap();
        assert_invalid_data(
            CellVisibilitySection::from_bytes(&bytes[..bytes.len() - 1], 4).unwrap_err(),
        );
        let mut trailing = bytes;
        trailing.push(0);
        assert_invalid_data(CellVisibilitySection::from_bytes(&trailing, 4).unwrap_err());
    }

    #[test]
    fn rejects_noncanonical_pair_table() {
        let mut bytes = valid_section().to_bytes().unwrap();
        // `to_bytes` is intentionally a serialization primitive; mutate a valid
        // payload so decoding remains the strict external-data boundary.
        bytes[28..32].copy_from_slice(&1u32.to_le_bytes());
        bytes[32..36].copy_from_slice(&0u32.to_le_bytes());
        assert_invalid_data(CellVisibilitySection::from_bytes(&bytes, 4).unwrap_err());
    }

    #[test]
    fn rejects_graded_pair_beyond_distance_cap() {
        let mut bytes = valid_section().to_bytes().unwrap();
        // Header (8) + four component IDs (16) + pair count (4) + pair
        // endpoints (8) leaves the distance at byte offset 36.
        bytes[36..40].copy_from_slice(&(CELL_VISIBILITY_DISTANCE_CAP + 1).to_le_bytes());
        assert_invalid_data(CellVisibilitySection::from_bytes(&bytes, 4).unwrap_err());
    }

    #[test]
    fn writer_rejects_pair_count_beyond_u32_without_allocating_records() {
        // The boundary is a wire-format conversion, not an allocation test.
        // On 32-bit hosts a Vec length cannot exceed the u32 wire range.
        if let Some(too_many_pairs) = usize::try_from(u32::MAX).unwrap().checked_add(1) {
            assert_invalid_data(checked_pair_count(too_many_pairs).unwrap_err());
        }
    }

    #[test]
    fn rejects_pair_table_beyond_fanout_bound_before_allocation() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&CELL_VISIBILITY_VERSION.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&((CELL_VISIBILITY_FANOUT_K + 1) as u32).to_le_bytes());
        bytes.resize(
            bytes.len() + (CELL_VISIBILITY_FANOUT_K + 1) * PAIR_RECORD_SIZE,
            0,
        );

        assert_invalid_data(CellVisibilitySection::from_bytes(&bytes, 1).unwrap_err());
    }

    #[test]
    fn maximum_encoded_length_matches_fanout_bound() {
        assert_eq!(
            CellVisibilitySection::max_encoded_len(2).unwrap(),
            (HEADER_SIZE
                + 2 * std::mem::size_of::<u32>()
                + PAIR_COUNT_SIZE
                + 2 * CELL_VISIBILITY_FANOUT_K * PAIR_RECORD_SIZE) as u64
        );
    }
}
