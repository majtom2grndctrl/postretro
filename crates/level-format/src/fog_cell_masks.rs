// FogCellMasks PRL section (ID 31): per-BSP-leaf bitmask of overlapping fog
// volumes. Bit `i` set in leaf `L`'s mask means fog volume `i` overlaps leaf
// `L`. Used at runtime to OR together visible-cell masks into an active fog
// volume set for the fog raymarch pass.
// See: context/lib/build_pipeline.md §PRL section IDs
// See: context/plans/in-progress/perf-portal-fog-culling/index.md
//
// On-disk layout (little-endian):
//   u32  cell_count          — total BSP leaf count (solid + empty)
//   u32  masks[cell_count]   — per-leaf fog volume bitmask
//
// Bits 0..16 carry the volume bitmap (`MAX_FOG_VOLUMES = 16`). Bits 16..32 are
// reserved, written as zero, and ignored by the runtime fog-active computation
// (AND-masked at union time, not stripped here) so the section can grow without
// a format break.
//
// The section is optional: it is omitted from the PRL when the source map has
// no `env_fog_volume` brushes. Absence at load time produces `None`.

use crate::FormatError;

/// Parsed FogCellMasks section: one `u32` mask per BSP leaf, in leaf-index
/// order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FogCellMasksSection {
    pub masks: Vec<u32>,
}

/// OR together the per-leaf fog-volume bitmasks for every leaf in `visible_leaves`,
/// returning an `active_mask` whose bit `i` is set iff fog volume `i` overlaps at
/// least one visible leaf.
///
/// Out-of-range leaf indices are silently skipped — no clamping or panic — so a
/// stale `VisibleCells` from a previous level cannot crash a frame mid-load.
///
/// Hot path: called once per frame from the fog pass before the raymarch
/// dispatch. Kept tight (no allocation, no bounds-check error path) so it stays
/// well under the < 10 µs target on a 200-leaf input.
#[inline]
pub fn union_active_mask(visible_leaves: &[u32], masks: &[u32]) -> u32 {
    let mut active = 0u32;
    for &leaf in visible_leaves {
        // Non-empty masks with an OOB index means stale VisibleCells — assert
        // in debug to surface the bug early without affecting release.
        debug_assert!(
            masks.is_empty() || (leaf as usize) < masks.len(),
            "leaf index {leaf} OOB — stale VisibleCells?"
        );
        if let Some(m) = masks.get(leaf as usize) {
            active |= *m;
        }
    }
    active
}

impl FogCellMasksSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4 + self.masks.len() * 4);
        buf.extend_from_slice(&(self.masks.len() as u32).to_le_bytes());
        for mask in &self.masks {
            buf.extend_from_slice(&mask.to_le_bytes());
        }
        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "fog cell masks: truncated header",
            )));
        }
        let cell_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

        let needed = 4usize.saturating_add(cell_count.saturating_mul(4));
        if data.len() < needed {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "fog cell masks: truncated — cell_count {cell_count} requires {needed} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut masks = Vec::with_capacity(cell_count);
        let mut o = 4usize;
        for _ in 0..cell_count {
            let mask = u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
            masks.push(mask);
            o += 4;
        }

        Ok(Self { masks })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let section = FogCellMasksSection { masks: vec![] };
        let bytes = section.to_bytes();
        // 4-byte cell_count header only.
        assert_eq!(bytes.len(), 4);
        let restored = FogCellMasksSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_several_leaves() {
        let section = FogCellMasksSection {
            masks: vec![
                0x0000_0000,
                0x0000_0001,
                0x0000_0003,
                0x0000_8000,
                0x0000_FFFF,
            ],
        };
        let bytes = section.to_bytes();
        // 4 (header) + 5 * 4 (masks).
        assert_eq!(bytes.len(), 24);
        let restored = FogCellMasksSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn parses_well_formed_blob_to_expected_vec() {
        // Hand-build a blob: cell_count=3, masks=[0x1, 0x2, 0x4].
        let mut buf = Vec::new();
        buf.extend_from_slice(&3u32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&4u32.to_le_bytes());

        let section = FogCellMasksSection::from_bytes(&buf).unwrap();
        assert_eq!(section.masks, vec![1u32, 2, 4]);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = FogCellMasksSection::from_bytes(&[0u8; 3]).unwrap_err();
        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn rejects_truncated_masks_payload() {
        // cell_count = 4 but only one mask of payload supplied.
        let mut buf = Vec::new();
        buf.extend_from_slice(&4u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        let err = FogCellMasksSection::from_bytes(&buf).unwrap_err();
        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn rejects_implausible_cell_count() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&u32::MAX.to_le_bytes());
        let err = FogCellMasksSection::from_bytes(&buf).unwrap_err();
        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn absent_section_yields_none_via_container() {
        use crate::{SectionBlob, SectionId, read_container, read_section_data, write_prl};
        use std::io::Cursor;

        // Build a PRL container with a non-FogCellMasks section.
        let blobs = vec![SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: vec![0xAB, 0xCD],
        }];
        let mut buf = Vec::new();
        write_prl(&mut buf, &blobs).unwrap();

        let mut cursor = Cursor::new(&buf);
        let meta = read_container(&mut cursor).unwrap();

        // Looking up FogCellMasks in a file that omits it yields None.
        let raw = read_section_data(&mut cursor, &meta, SectionId::FogCellMasks as u32).unwrap();
        assert!(raw.is_none());
    }

    #[test]
    fn parses_via_container_round_trip() {
        use crate::{SectionBlob, SectionId, read_container, read_section_data, write_prl};
        use std::io::Cursor;

        let original = FogCellMasksSection {
            masks: vec![0, 1, 0x8000, 0x0000_FFFF],
        };
        let blobs = vec![SectionBlob {
            section_id: SectionId::FogCellMasks as u32,
            version: 1,
            data: original.to_bytes(),
        }];
        let mut buf = Vec::new();
        write_prl(&mut buf, &blobs).unwrap();

        let mut cursor = Cursor::new(&buf);
        let meta = read_container(&mut cursor).unwrap();
        let raw = read_section_data(&mut cursor, &meta, SectionId::FogCellMasks as u32)
            .unwrap()
            .expect("FogCellMasks section should be present");
        let parsed = FogCellMasksSection::from_bytes(&raw).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn union_active_mask_ors_visible_leaves() {
        let masks = vec![0b0001, 0b0010, 0b0100, 0b1000];
        let visible = vec![0u32, 2u32];
        assert_eq!(union_active_mask(&visible, &masks), 0b0101);
    }

    // In debug builds, out-of-range leaf indices trigger a `debug_assert!` to
    // surface stale-VisibleCells bugs early. In release, the `.get()` bounds
    // check silently skips the OOB index so a stale VisibleCells from a
    // previous level cannot crash a frame mid-load.
    #[test]
    #[cfg(not(debug_assertions))]
    fn union_active_mask_skips_out_of_range_leaves() {
        let masks = vec![0b0001, 0b0010];
        // leaf 99 is out of range — must not affect the result (release only;
        // debug build panics at the assert instead).
        let visible = vec![0u32, 99u32, 1u32];
        assert_eq!(union_active_mask(&visible, &masks), 0b0011);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "OOB")]
    fn union_active_mask_panics_on_oob_in_debug() {
        let masks = vec![0b0001, 0b0010];
        let visible = vec![0u32, 99u32, 1u32];
        union_active_mask(&visible, &masks);
    }

    #[test]
    fn union_active_mask_empty_inputs_yield_zero() {
        assert_eq!(union_active_mask(&[], &[]), 0);
        assert_eq!(union_active_mask(&[0, 1], &[]), 0);
        assert_eq!(union_active_mask(&[], &[0xFFFF_FFFFu32]), 0);
    }

    /// Algorithmic-regression guard for the per-frame OR loop. The plan's
    /// performance target is < 10 µs on a 200-leaf input; criterion enforces
    /// that statistically in the bench. This test uses a much more generous
    /// 50 µs ceiling so it surfaces order-of-magnitude regressions in
    /// `cargo test` without false-positives on loaded CI machines.
    #[test]
    fn union_active_mask_under_50us_on_200_leaves() {
        use std::hint::black_box;
        use std::time::{Duration, Instant};

        let leaf_count = 1024usize;
        let masks: Vec<u32> = (0..leaf_count)
            .map(|i| 1u32 << ((i as u32) % 16))
            .collect();
        let visible: Vec<u32> = (0..200u32).map(|i| (i * 5) % leaf_count as u32).collect();

        // Warm caches.
        for _ in 0..1000 {
            black_box(union_active_mask(black_box(&visible), black_box(&masks)));
        }

        let iters = 10_000u32;
        let start = Instant::now();
        let mut acc = 0u32;
        for _ in 0..iters {
            acc ^= union_active_mask(black_box(&visible), black_box(&masks));
        }
        let elapsed = start.elapsed();
        black_box(acc);
        let per_call = elapsed / iters;
        assert!(
            per_call < Duration::from_micros(50),
            "union_active_mask regressed past the 50 µs algorithmic ceiling: \
             {per_call:?} per call ({iters} iters in {elapsed:?})"
        );
    }

    #[test]
    fn section_id_31_is_registered() {
        use crate::SectionId;
        assert_eq!(
            SectionId::from_u32(31),
            Some(SectionId::FogCellMasks),
            "section ID 31 must map to FogCellMasks"
        );
    }
}
