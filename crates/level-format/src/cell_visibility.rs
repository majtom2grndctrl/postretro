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
    pub fn to_bytes(&self) -> Vec<u8> {
        debug_assert!(self.validate().is_ok());

        let mut bytes = Vec::with_capacity(
            HEADER_SIZE
                + self.component_ids.len() * std::mem::size_of::<u32>()
                + PAIR_COUNT_SIZE
                + self.coupled_pairs.len() * PAIR_RECORD_SIZE,
        );
        bytes.extend_from_slice(&CELL_VISIBILITY_VERSION.to_le_bytes());
        bytes.extend_from_slice(&self.cell_count.to_le_bytes());
        for component_id in &self.component_ids {
            bytes.extend_from_slice(&component_id.to_le_bytes());
        }
        bytes.extend_from_slice(&(self.coupled_pairs.len() as u32).to_le_bytes());
        for pair in &self.coupled_pairs {
            bytes.extend_from_slice(&pair.cell_a.to_le_bytes());
            bytes.extend_from_slice(&pair.cell_b.to_le_bytes());
            bytes.extend_from_slice(&pair.distance.to_le_bytes());
            bytes.extend_from_slice(&pair.aperture.to_le_bytes());
        }
        bytes
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

        let component_ids = (0..cell_count as usize)
            .map(|index| read_u32(data, HEADER_SIZE + index * std::mem::size_of::<u32>()))
            .collect();
        let mut coupled_pairs = Vec::with_capacity(pair_count as usize);
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

    fn validate(&self) -> crate::Result<()> {
        if self.cell_count == 0 {
            return Err(invalid(
                "CellVisibility section cell_count must be greater than zero",
            ));
        }
        if self.component_ids.len() != self.cell_count as usize {
            return Err(invalid(format!(
                "CellVisibility has {} component ids for cell_count {}",
                self.component_ids.len(),
                self.cell_count
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
    (count as usize).checked_mul(stride).ok_or_else(|| {
        invalid(format!(
            "CellVisibility section count multiplication overflow for {name} {count} * stride {stride}"
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
    fn round_trip_preserves_fixed_layout() {
        let section = valid_section();
        let bytes = section.to_bytes();
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
            CellVisibilitySection::from_bytes(&section.to_bytes(), 2).unwrap(),
            section
        );
    }

    #[test]
    fn rejects_version_cell_count_truncation_and_trailing_data() {
        let section = valid_section();
        let mut bad_version = section.to_bytes();
        bad_version[..4].copy_from_slice(&2u32.to_le_bytes());
        assert_invalid_data(CellVisibilitySection::from_bytes(&bad_version, 4).unwrap_err());
        assert_invalid_data(CellVisibilitySection::from_bytes(&section.to_bytes(), 3).unwrap_err());

        let bytes = section.to_bytes();
        assert_invalid_data(
            CellVisibilitySection::from_bytes(&bytes[..bytes.len() - 1], 4).unwrap_err(),
        );
        let mut trailing = bytes;
        trailing.push(0);
        assert_invalid_data(CellVisibilitySection::from_bytes(&trailing, 4).unwrap_err());
    }

    #[test]
    fn rejects_noncanonical_pair_table() {
        let mut bytes = valid_section().to_bytes();
        // `to_bytes` is intentionally a serialization primitive; mutate a valid
        // payload so decoding remains the strict external-data boundary.
        bytes[28..32].copy_from_slice(&1u32.to_le_bytes());
        bytes[32..36].copy_from_slice(&0u32.to_le_bytes());
        assert_invalid_data(CellVisibilitySection::from_bytes(&bytes, 4).unwrap_err());
    }

    #[test]
    fn rejects_graded_pair_beyond_distance_cap() {
        let mut bytes = valid_section().to_bytes();
        // Header (8) + four component IDs (16) + pair count (4) + pair
        // endpoints (8) leaves the distance at byte offset 36.
        bytes[36..40].copy_from_slice(&(CELL_VISIBILITY_DISTANCE_CAP + 1).to_le_bytes());
        assert_invalid_data(CellVisibilitySection::from_bytes(&bytes, 4).unwrap_err());
    }
}
