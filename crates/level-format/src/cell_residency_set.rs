// CellResidencySet PRL section (ID 51): each camera cell's baked mandatory
// cells, keyed by the smallest movement lead that makes each one mandatory.
// See: context/lib/build_pipeline.md §PRL section IDs, rendering_pipeline.md §4

use crate::cell_visibility::CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;
use crate::coded_plane::{BlockCodec, decode_plane_from, max_decoded_len, store_plane};
use crate::lightmap::{eof, invalid, read_u32};

/// Section version. Load rejects any other. v2: the body is stored raw or as
/// one checksummed zstd frame behind a codec and raw-length prefix.
pub const CELL_RESIDENCY_SET_VERSION: u32 = 2;

/// Default movement lead L in metres. Lives here so the compiler's bound and
/// the runtime's demand cannot differ.
pub const DEFAULT_LEAD_METRES: u32 = 16;
/// [`DEFAULT_LEAD_METRES`] in id-46 fixed-point units, the units of every
/// stored lead.
pub const DEFAULT_LEAD: u32 = DEFAULT_LEAD_METRES * CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;

/// version, codec, reserved, raw_len (u64).
const PREFIX_BYTES: usize = 20;
/// The body's camera_cell_count, max_lead, entry_count.
const HEADER_BYTES: usize = 12;
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
///   u32 codec              (BlockCodec: 0 raw, 1 one zstd frame)
///   u32 reserved           (0)
///   u64 raw_len            (the body's decoded length)
///   body, stored under `codec`:
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

    /// Whether any camera cell has a baked set. A camera cell's set always
    /// holds at least that cell, so this is the compiler's "has camera cells":
    /// a level whose cells are all solid or exterior has an empty section.
    pub fn has_camera_sets(&self) -> bool {
        !self.entries.is_empty()
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

    /// The body's decoded length; what the section stores depends on its codec.
    pub fn raw_len(&self) -> usize {
        HEADER_BYTES + self.offsets.len() * 4 + self.entries.len() * ENTRY_BYTES
    }

    /// The section as stored under `codec`. The compiler writes zstd; `raw` is
    /// its uncoded diagnostic and the form its memo holds.
    pub fn to_bytes(&self, codec: BlockCodec) -> Vec<u8> {
        let mut body = Vec::with_capacity(self.raw_len());
        body.extend_from_slice(&(self.camera_cell_count() as u32).to_le_bytes());
        body.extend_from_slice(&self.max_lead.to_le_bytes());
        body.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for offset in &self.offsets {
            body.extend_from_slice(&offset.to_le_bytes());
        }
        for entry in &self.entries {
            body.extend_from_slice(&entry.cell_id.to_le_bytes());
            body.extend_from_slice(&entry.lead.to_le_bytes());
        }
        let raw_len = body.len() as u64;
        let stored = store_plane(codec, body);
        let mut out = Vec::with_capacity(PREFIX_BYTES + stored.len());
        out.extend_from_slice(&CELL_RESIDENCY_SET_VERSION.to_le_bytes());
        out.extend_from_slice(&codec.as_u32().to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&raw_len.to_le_bytes());
        out.extend_from_slice(&stored);
        out
    }

    /// Parse and validate against the level's cell count. Rejects an other
    /// version, an unknown codec, a body that fails to decode to its raw
    /// length (a corrupt frame), a camera cell count that differs from
    /// `cell_count`, CSR offsets that don't start at 0, decrease, or pass
    /// `entry_count`, an entry naming a cell past `cell_count`, a lead past
    /// `max_lead`, and a camera cell's range that is unsorted or repeats a
    /// cell.
    pub fn from_bytes(data: &[u8], cell_count: usize) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(eof("cell residency set too short for its version"));
        }
        let version = read_u32(data, 0);
        if version != CELL_RESIDENCY_SET_VERSION {
            return Err(invalid(format!(
                "unsupported cell residency set version: {version} (expected {CELL_RESIDENCY_SET_VERSION}); re-bake the level"
            )));
        }
        if data.len() < PREFIX_BYTES {
            return Err(eof("cell residency set too short for its prefix"));
        }
        let codec_word = read_u32(data, 4);
        let codec = BlockCodec::from_u32(codec_word)
            .ok_or_else(|| invalid(format!("cell residency set codec {codec_word} is unknown")))?;
        if read_u32(data, 8) != 0 {
            return Err(invalid("cell residency set reserved word is not 0"));
        }
        let raw_len = u64::from_le_bytes(data[12..20].try_into().expect("8-byte slice"));
        let stored = &data[PREFIX_BYTES..];
        // A body holds at least its header and CSR offsets, and at most what
        // its stored bytes can decode to: checked before the decode allocates.
        let min_raw_len = (cell_count as u64 + 1)
            .checked_mul(4)
            .and_then(|offsets| offsets.checked_add(HEADER_BYTES as u64))
            .ok_or_else(|| invalid("cell residency set length overflows"))?;
        let max_raw_len = max_decoded_len(codec, stored.len());
        if raw_len < min_raw_len || raw_len > max_raw_len {
            return Err(invalid(format!(
                "cell residency set raw length {raw_len} is outside {min_raw_len}..={max_raw_len} \
                 for {cell_count} cells in {} stored bytes",
                stored.len()
            )));
        }
        let body = decode_plane_from(codec, stored, raw_len as usize)
            .map_err(|reason| invalid(format!("cell residency set frame is corrupt: {reason}")))?;
        Self::from_body(&body, cell_count)
    }

    /// Parse and validate a decoded body.
    fn from_body(data: &[u8], cell_count: usize) -> crate::Result<Self> {
        if data.len() < HEADER_BYTES {
            return Err(eof("cell residency set too short for header"));
        }
        let camera_cell_count = read_u32(data, 0) as usize;
        let max_lead = read_u32(data, 4);
        let entry_count = read_u32(data, 8) as usize;
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

    /// The section as the default (zstd) bake writes it.
    trait Coded {
        fn coded(&self) -> Vec<u8>;
    }

    impl Coded for CellResidencySetSection {
        fn coded(&self) -> Vec<u8> {
            self.to_bytes(BlockCodec::Zstd)
        }
    }

    #[test]
    fn residency_set_round_trips_with_empty_ranges_under_both_codecs() {
        let section = sample();
        for codec in [BlockCodec::Raw, BlockCodec::Zstd] {
            let bytes = section.to_bytes(codec);
            assert_eq!(read_u32(&bytes, 4), codec.as_u32());
            let raw_len = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
            assert_eq!(raw_len, section.raw_len() as u64);
            if codec == BlockCodec::Raw {
                assert_eq!(bytes.len(), PREFIX_BYTES + section.raw_len());
            }
            let restored = CellResidencySetSection::from_bytes(&bytes, 3).unwrap();
            assert_eq!(restored, section);
            assert!(restored.entries_for(1).is_empty());
            assert_eq!(restored.entries_for(0).len(), 3);
            assert!(restored.entries_for(9).is_empty());
        }
        // Both codecs store one body: the coded frame decodes to the raw one.
        let raw = section.to_bytes(BlockCodec::Raw);
        let coded = section.to_bytes(BlockCodec::Zstd);
        assert_eq!(
            crate::coded_plane::decode_plane_from(
                BlockCodec::Zstd,
                &coded[PREFIX_BYTES..],
                section.raw_len()
            )
            .unwrap(),
            raw[PREFIX_BYTES..]
        );
    }

    /// A v1 section, to prove that layout is refused: version 1, then the
    /// body uncoded with no codec prefix.
    fn v1_bytes(section: &CellResidencySetSection) -> Vec<u8> {
        let raw = section.to_bytes(BlockCodec::Raw);
        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&raw[PREFIX_BYTES..]);
        bytes
    }

    #[test]
    fn a_v1_residency_set_is_refused_with_a_rebake_message() {
        let error = message(CellResidencySetSection::from_bytes(&v1_bytes(&sample()), 3));
        assert!(error.contains("version: 1"), "{error}");
        assert!(error.contains("re-bake the level"), "{error}");
    }

    // A declared raw length is bounded by the stored bytes, linearly, before
    // anything is allocated for the decode.
    #[test]
    fn a_raw_length_past_what_the_stored_bytes_can_hold_is_refused_before_decoding() {
        let section = sample();
        let declare = |mut bytes: Vec<u8>, raw_len: u64| {
            bytes[12..20].copy_from_slice(&raw_len.to_le_bytes());
            message(CellResidencySetSection::from_bytes(&bytes, 3))
        };
        let coded = section.coded();
        let stored = (coded.len() - PREFIX_BYTES) as u64;
        let most = stored * 32 * 1024;
        let error = declare(coded.clone(), most + 1);
        assert!(
            error.contains(&format!(
                "raw length {} is outside 28..={most} for 3 cells in {stored} stored bytes",
                most + 1
            )),
            "{error}"
        );
        assert!(declare(coded.clone(), u64::MAX).contains("is outside"));
        // Within the bound, the frame's own content size refuses it.
        assert!(declare(coded.clone(), most).contains("frame is corrupt"));

        let raw = section.to_bytes(BlockCodec::Raw);
        let stored = raw.len() - PREFIX_BYTES;
        assert!(declare(raw.clone(), stored as u64 + 1).contains("is outside"));
        // Shorter than its header and one CSR offset per cell plus one.
        assert!(declare(raw, 27).contains("raw length 27 is outside 28..="));
    }

    // A corrupt frame fails by name, and never yields a different section: a
    // flipped byte anywhere in the frame, a short read, a raw length that
    // disagrees with the frame, an unknown codec and a nonzero reserved word.
    #[test]
    fn a_corrupt_residency_frame_is_refused_by_name() {
        let section = sample();
        let coded = section.coded();
        for at in PREFIX_BYTES..coded.len() {
            let mut flipped = coded.clone();
            flipped[at] ^= 0x01;
            match CellResidencySetSection::from_bytes(&flipped, 3) {
                Ok(decoded) => assert_eq!(decoded, section, "byte {at} decoded differently"),
                Err(error) => assert!(
                    format!("{error:?}").contains("cell residency set"),
                    "byte {at}: {error:?}"
                ),
            }
        }
        let short = &coded[..coded.len() - 1];
        assert!(
            message(CellResidencySetSection::from_bytes(short, 3)).contains("frame is corrupt")
        );

        let mut longer = coded.clone();
        longer[12..20].copy_from_slice(&(section.raw_len() as u64 + 8).to_le_bytes());
        assert!(
            message(CellResidencySetSection::from_bytes(&longer, 3)).contains("frame is corrupt")
        );

        let mut raw = section.to_bytes(BlockCodec::Raw);
        raw[12..20].copy_from_slice(&(section.raw_len() as u64 - 8).to_le_bytes());
        assert!(message(CellResidencySetSection::from_bytes(&raw, 3)).contains("frame is corrupt"));

        let mut codec = coded.clone();
        codec[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert!(message(CellResidencySetSection::from_bytes(&codec, 3)).contains("codec 2"));

        let mut reserved = coded;
        reserved[8] = 1;
        assert!(message(CellResidencySetSection::from_bytes(&reserved, 3)).contains("reserved"));
    }

    #[test]
    fn residency_set_rejects_an_entry_past_the_cell_count() {
        let mut section = sample();
        section.entries[3] = entry(3, 0);
        let error = message(CellResidencySetSection::from_bytes(&section.coded(), 3));
        assert!(error.contains("past the cell count"), "{error}");
    }

    #[test]
    fn residency_set_rejects_decreasing_or_overlong_csr_offsets() {
        let mut decreasing = sample();
        decreasing.offsets = vec![0, 3, 2, 4];
        let error = message(CellResidencySetSection::from_bytes(&decreasing.coded(), 3));
        assert!(error.contains("decrease"), "{error}");

        // Last offset past entry_count: hand-edit the final CSR word.
        let section = sample();
        let mut bytes = section.to_bytes(BlockCodec::Raw);
        let last = PREFIX_BYTES + HEADER_BYTES + 3 * 4;
        bytes[last..last + 4].copy_from_slice(&5u32.to_le_bytes());
        let error = message(CellResidencySetSection::from_bytes(&bytes, 3));
        assert!(error.contains("entry_count"), "{error}");
    }

    #[test]
    fn residency_set_rejects_other_versions_cell_counts_and_bad_ranges() {
        let mut bytes = sample().coded();
        bytes[0..4].copy_from_slice(&0u32.to_le_bytes());
        assert!(message(CellResidencySetSection::from_bytes(&bytes, 3)).contains("version"));

        let bytes = sample().coded();
        assert!(message(CellResidencySetSection::from_bytes(&bytes, 4)).contains("camera cells"));

        let mut unsorted = sample();
        unsorted.entries.swap(0, 2);
        assert!(
            message(CellResidencySetSection::from_bytes(&unsorted.coded(), 3))
                .contains("not sorted")
        );

        let mut repeated = sample();
        repeated.entries[2] = entry(1, 5);
        assert!(
            message(CellResidencySetSection::from_bytes(&repeated.coded(), 3)).contains("repeats")
        );

        let mut far = sample();
        far.entries[2] = entry(2, far.max_lead + 1);
        assert!(
            message(CellResidencySetSection::from_bytes(&far.coded(), 3)).contains("past max_lead")
        );
    }

    #[test]
    fn section_id_is_pinned() {
        assert_eq!(SectionId::CellResidencySet as u32, 51);
        assert_eq!(SectionId::from_u32(51), Some(SectionId::CellResidencySet));
    }
}
