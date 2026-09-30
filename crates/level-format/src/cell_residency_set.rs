// CellResidencySet PRL section (ID 51): each camera cell's baked mandatory
// cells, keyed by the smallest movement lead that makes each one mandatory.
// See: context/lib/build_pipeline.md §PRL section IDs, rendering_pipeline.md §4

use crate::lightmap::{eof, invalid, read_u32};

/// Section version. Load rejects any other.
pub const CELL_RESIDENCY_SET_VERSION: u32 = 1;

/// version, camera_cell_count, max_lead, entry_count.
const HEADER_BYTES: usize = 16;
/// cell_id, lead.
const ENTRY_BYTES: usize = 8;

/// One mandatory cell for a camera cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResidencyEntry {
    /// Smallest lead, in id-46 fixed-point units
    /// (`cell_visibility::CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE` per
    /// metre), at which `cell_id` becomes mandatory. 0 for the camera cell's
    /// own dilated visible set.
    pub lead: u32,
    pub cell_id: u32,
}

/// The streaming-owned residency relation: for every camera cell, the cells
/// within portal-path lead L, each such cell's sampled visible set dilated one
/// portal hop, each tagged with its smallest lead. Pins are not baked here;
/// they come from id 49. Keyed by cell, never by a resource unit.
///
/// On-disk layout (little-endian):
///
/// ```text
///   u32 version            (= CELL_RESIDENCY_SET_VERSION)
///   u32 camera_cell_count  (= the level's cell count)
///   u32 max_lead           (fixed-point; every entry's lead <= it)
///   u32 entry_count
///   u32 offsets[camera_cell_count + 1]   CSR, indexed by camera cell id
///   entry_count × { u32 cell_id, u32 lead }
///     each camera cell's range sorted by (lead, cell_id), one entry per cell
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellResidencySetSection {
    pub max_lead: u32,
    /// `camera_cell_count + 1` offsets into `entries`.
    pub offsets: Vec<u32>,
    pub entries: Vec<ResidencyEntry>,
}

impl CellResidencySetSection {
    pub fn camera_cell_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// The camera cell's entries, sorted by (lead, cell id). Entries with
    /// lead <= L form its mandatory set at lead L. Empty past the table.
    pub fn entries_for(&self, camera_cell: usize) -> &[ResidencyEntry] {
        match (
            self.offsets.get(camera_cell),
            self.offsets.get(camera_cell + 1),
        ) {
            (Some(&start), Some(&end)) => &self.entries[start as usize..end as usize],
            _ => &[],
        }
    }

    pub fn byte_len(&self) -> usize {
        HEADER_BYTES + self.offsets.len() * 4 + self.entries.len() * ENTRY_BYTES
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.byte_len());
        out.extend_from_slice(&CELL_RESIDENCY_SET_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.camera_cell_count() as u32).to_le_bytes());
        out.extend_from_slice(&self.max_lead.to_le_bytes());
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for offset in &self.offsets {
            out.extend_from_slice(&offset.to_le_bytes());
        }
        for entry in &self.entries {
            out.extend_from_slice(&entry.cell_id.to_le_bytes());
            out.extend_from_slice(&entry.lead.to_le_bytes());
        }
        out
    }

    /// Parse and validate against the level's cell count. Rejects an other
    /// version, a camera cell count that differs from `cell_count`, CSR
    /// offsets that don't start at 0, decrease, or pass `entry_count`, an
    /// entry naming a cell past `cell_count`, a lead past `max_lead`, and a
    /// camera cell's range that is unsorted or repeats a cell.
    pub fn from_bytes(data: &[u8], cell_count: usize) -> crate::Result<Self> {
        if data.len() < HEADER_BYTES {
            return Err(eof("cell residency set too short for header"));
        }
        let version = read_u32(data, 0);
        if version != CELL_RESIDENCY_SET_VERSION {
            return Err(invalid(format!(
                "unsupported cell residency set version: {version} (expected {CELL_RESIDENCY_SET_VERSION}); re-bake the level"
            )));
        }
        let camera_cell_count = read_u32(data, 4) as usize;
        let max_lead = read_u32(data, 8);
        let entry_count = read_u32(data, 12) as usize;
        if camera_cell_count != cell_count {
            return Err(invalid(format!(
                "cell residency set covers {camera_cell_count} camera cells, level has {cell_count}"
            )));
        }
        let expected = (camera_cell_count as u64 + 1)
            .checked_mul(4)
            .and_then(|n| n.checked_add((entry_count as u64).checked_mul(ENTRY_BYTES as u64)?))
            .and_then(|n| n.checked_add(HEADER_BYTES as u64))
            .ok_or_else(|| invalid("cell residency set length overflows"))?;
        if data.len() as u64 != expected {
            return Err(invalid(format!(
                "cell residency set is {} bytes, expected {expected}",
                data.len()
            )));
        }
        let offsets: Vec<u32> = (0..=camera_cell_count)
            .map(|i| read_u32(data, HEADER_BYTES + i * 4))
            .collect();
        if offsets[0] != 0 {
            return Err(invalid(format!(
                "cell residency set CSR starts at {}, not 0",
                offsets[0]
            )));
        }
        if let Some(i) = offsets.windows(2).position(|w| w[1] < w[0]) {
            return Err(invalid(format!(
                "cell residency set CSR offsets decrease at camera cell {i}"
            )));
        }
        if offsets[camera_cell_count] as usize != entry_count {
            return Err(invalid(format!(
                "cell residency set CSR ends at {}, entry_count is {entry_count}",
                offsets[camera_cell_count]
            )));
        }
        let entries_at = HEADER_BYTES + offsets.len() * 4;
        let entries: Vec<ResidencyEntry> = (0..entry_count)
            .map(|i| {
                let at = entries_at + i * ENTRY_BYTES;
                ResidencyEntry {
                    cell_id: read_u32(data, at),
                    lead: read_u32(data, at + 4),
                }
            })
            .collect();
        let section = Self {
            max_lead,
            offsets,
            entries,
        };
        for camera in 0..camera_cell_count {
            let range = section.entries_for(camera);
            if let Some(entry) = range.iter().find(|e| e.cell_id as usize >= cell_count) {
                return Err(invalid(format!(
                    "cell residency set entry for camera cell {camera} names cell {} past the cell count {cell_count}",
                    entry.cell_id
                )));
            }
            if let Some(entry) = range.iter().find(|e| e.lead > max_lead) {
                return Err(invalid(format!(
                    "cell residency set entry for camera cell {camera} has lead {} past max_lead {max_lead}",
                    entry.lead
                )));
            }
            if range.windows(2).any(|w| w[1] <= w[0]) {
                return Err(invalid(format!(
                    "cell residency set range for camera cell {camera} is not sorted by (lead, cell)"
                )));
            }
            let mut cells: Vec<u32> = range.iter().map(|e| e.cell_id).collect();
            cells.sort_unstable();
            if cells.windows(2).any(|w| w[0] == w[1]) {
                return Err(invalid(format!(
                    "cell residency set range for camera cell {camera} repeats a cell"
                )));
            }
        }
        Ok(section)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SectionId;

    fn entry(cell_id: u32, lead: u32) -> ResidencyEntry {
        ResidencyEntry { lead, cell_id }
    }

    /// Three cells: camera 0 sees {0, 1} at lead 0 and 2 at lead 5; camera 1
    /// has an empty range; camera 2 sees itself.
    fn sample() -> CellResidencySetSection {
        CellResidencySetSection {
            max_lead: 32 * 1024,
            offsets: vec![0, 3, 3, 4],
            entries: vec![entry(0, 0), entry(1, 0), entry(2, 5), entry(2, 0)],
        }
    }

    fn message(result: crate::Result<CellResidencySetSection>) -> String {
        format!("{:?}", result.expect_err("expected a rejection"))
    }

    #[test]
    fn residency_set_round_trips_with_empty_ranges() {
        let section = sample();
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), section.byte_len());
        let restored = CellResidencySetSection::from_bytes(&bytes, 3).unwrap();
        assert_eq!(restored, section);
        assert!(restored.entries_for(1).is_empty());
        assert_eq!(restored.entries_for(0).len(), 3);
        assert!(restored.entries_for(9).is_empty());
    }

    #[test]
    fn residency_set_rejects_an_entry_past_the_cell_count() {
        let mut section = sample();
        section.entries[3] = entry(3, 0);
        let error = message(CellResidencySetSection::from_bytes(&section.to_bytes(), 3));
        assert!(error.contains("past the cell count"), "{error}");
    }

    #[test]
    fn residency_set_rejects_decreasing_or_overlong_csr_offsets() {
        let mut decreasing = sample();
        decreasing.offsets = vec![0, 3, 2, 4];
        let error = message(CellResidencySetSection::from_bytes(
            &decreasing.to_bytes(),
            3,
        ));
        assert!(error.contains("decrease"), "{error}");

        // Last offset past entry_count: hand-edit the final CSR word.
        let section = sample();
        let mut bytes = section.to_bytes();
        let last = HEADER_BYTES + 3 * 4;
        bytes[last..last + 4].copy_from_slice(&5u32.to_le_bytes());
        let error = message(CellResidencySetSection::from_bytes(&bytes, 3));
        assert!(error.contains("entry_count"), "{error}");
    }

    #[test]
    fn residency_set_rejects_other_versions_cell_counts_and_bad_ranges() {
        let mut bytes = sample().to_bytes();
        bytes[0..4].copy_from_slice(&0u32.to_le_bytes());
        assert!(message(CellResidencySetSection::from_bytes(&bytes, 3)).contains("version"));

        let bytes = sample().to_bytes();
        assert!(message(CellResidencySetSection::from_bytes(&bytes, 4)).contains("camera cells"));

        let mut unsorted = sample();
        unsorted.entries.swap(0, 2);
        assert!(
            message(CellResidencySetSection::from_bytes(&unsorted.to_bytes(), 3))
                .contains("not sorted")
        );

        let mut repeated = sample();
        repeated.entries[2] = entry(1, 5);
        assert!(
            message(CellResidencySetSection::from_bytes(&repeated.to_bytes(), 3))
                .contains("repeats")
        );

        let mut far = sample();
        far.entries[2] = entry(2, far.max_lead + 1);
        assert!(
            message(CellResidencySetSection::from_bytes(&far.to_bytes(), 3))
                .contains("past max_lead")
        );
    }

    #[test]
    fn section_id_is_pinned() {
        assert_eq!(SectionId::CellResidencySet as u32, 51);
        assert_eq!(SectionId::from_u32(51), Some(SectionId::CellResidencySet));
    }
}
