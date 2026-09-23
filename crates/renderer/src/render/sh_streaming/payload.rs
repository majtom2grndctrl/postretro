//! Parsed sparse cluster payloads and renderer-local upload plans.
//!
//! These types are intentionally CPU-only. They borrow no wgpu state and
//! exist only until the owning drain has written its validated row data.

use super::*;

#[derive(Debug, Clone)]
pub(super) struct ParsedSparseRow {
    pub(super) row: u32,
    pub(super) entry_count: u32,
    pub(super) tile_f16_count: u32,
    pub(super) role: u32,
    pub(super) lights: Vec<u32>,
    pub(super) entry_tile_f16_offsets: Vec<u32>,
    pub(super) tile_f16: Vec<u16>,
}

/// A fully parsed, ownership-checked sparse row with its renderer-local
/// first-fit ranges. It owns only the transient decoded values needed for the
/// immediate queue upload; no cluster payload is retained after installation.
#[derive(Debug)]
pub(super) struct SparseInstallPlan {
    pub(super) section_id: u32,
    pub(super) payload: ParsedSparseRow,
    pub(super) entries: PoolRange,
    pub(super) tiles: PoolRange,
}

impl SparseInstallPlan {
    pub(super) fn direct_upload(&self) -> Result<DirectSparseRowUpload<'_>, ShResidencyDrainError> {
        let entry_end = self
            .entries
            .start
            .checked_add(self.entries.len)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let offsets = self
            .payload
            .entry_tile_f16_offsets
            .iter()
            .map(|offset| {
                self.tiles
                    .start
                    .checked_add(*offset)
                    .ok_or(ShResidencyDrainError::SlotOverflow)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(DirectSparseRowUpload {
            row: self.payload.row,
            role: self.payload.role,
            entry_start: self.entries.start,
            entry_end,
            tile_f16_start: self.tiles.start,
            entry_tile_f16_offsets: offsets,
            // The queue packer reads these directly from the already-parsed
            // ready chunk. Only absolute entry offsets are derived here; do
            // not clone an entire id41/id45 row merely to validate it.
            lights: &self.payload.lights,
            tile_f16: &self.payload.tile_f16,
        })
    }
}

pub(super) fn parse_sparse_rows(
    cluster_id: u32,
    bytes: &[u8],
) -> Result<Vec<ParsedSparseRow>, ShResidencyDrainError> {
    let row_count = u32_at(bytes, 0).ok_or(malformed(cluster_id, "sparse header is truncated"))?;
    let entry_count =
        u32_at(bytes, 4).ok_or(malformed(cluster_id, "sparse header is truncated"))?;
    let tile_f16_count =
        u32_at(bytes, 8).ok_or(malformed(cluster_id, "sparse header is truncated"))?;
    if u32_at(bytes, 12) != Some(0) {
        return Err(malformed(
            cluster_id,
            "sparse header reserved word is nonzero",
        ));
    }
    let rows_bytes = (row_count as usize)
        .checked_mul(16)
        .and_then(|count| count.checked_add(16))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let entries_bytes = (entry_count as usize)
        .checked_mul(16)
        .and_then(|count| count.checked_add(rows_bytes))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let tiles_bytes = (tile_f16_count as usize)
        .checked_mul(2)
        .and_then(|count| count.checked_add(entries_bytes))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    if bytes.len() != tiles_bytes {
        return Err(malformed(cluster_id, "sparse block length is invalid"));
    }
    let mut rows = Vec::with_capacity(row_count as usize);
    let mut expected_entry = 0u32;
    for row_index in 0..row_count as usize {
        let offset = 16 + row_index * 16;
        let row = u32_at(bytes, offset).ok_or(malformed(cluster_id, "truncated sparse row"))?;
        let first_entry =
            u32_at(bytes, offset + 4).ok_or(malformed(cluster_id, "truncated sparse row"))?;
        let count =
            u32_at(bytes, offset + 8).ok_or(malformed(cluster_id, "truncated sparse row"))?;
        let role =
            u32_at(bytes, offset + 12).ok_or(malformed(cluster_id, "truncated sparse row"))?;
        if first_entry != expected_entry || role > 2 {
            return Err(malformed(
                cluster_id,
                "sparse row records are not canonical",
            ));
        }
        expected_entry = expected_entry
            .checked_add(count)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        rows.push(ParsedSparseRow {
            row,
            entry_count: count,
            tile_f16_count: 0,
            role,
            lights: Vec::with_capacity(count as usize),
            entry_tile_f16_offsets: Vec::with_capacity(count as usize),
            tile_f16: Vec::new(),
        });
    }
    if expected_entry != entry_count {
        return Err(malformed(
            cluster_id,
            "sparse row count disagrees with entry count",
        ));
    }
    let mut expected_tile = 0u32;
    let entry_base = rows_bytes;
    let mut row_index = 0usize;
    let mut row_entry_end = rows.first().map_or(0, |row| row.entry_count);
    for entry in 0..entry_count as usize {
        let offset = entry_base + entry * 16;
        let light = u32_at(bytes, offset).ok_or(malformed(cluster_id, "truncated sparse entry"))?;
        let first_tile =
            u32_at(bytes, offset + 4).ok_or(malformed(cluster_id, "truncated sparse entry"))?;
        let tile_count =
            u32_at(bytes, offset + 8).ok_or(malformed(cluster_id, "truncated sparse entry"))?;
        if u32_at(bytes, offset + 12) != Some(0) || first_tile != expected_tile {
            return Err(malformed(cluster_id, "sparse entries are not canonical"));
        }
        expected_tile = expected_tile
            .checked_add(tile_count)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        while entry as u32 >= row_entry_end {
            row_index = row_index
                .checked_add(1)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            let Some(row) = rows.get(row_index) else {
                return Err(malformed(cluster_id, "sparse entry has no row"));
            };
            row_entry_end = row_entry_end
                .checked_add(row.entry_count)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
        }
        let row = rows
            .get_mut(row_index)
            .ok_or(malformed(cluster_id, "sparse entry has no row"))?;
        let local_tile_offset = row.tile_f16_count;
        row.tile_f16_count = row
            .tile_f16_count
            .checked_add(tile_count)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        row.lights.push(light);
        row.entry_tile_f16_offsets.push(local_tile_offset);
    }
    if expected_tile != tile_f16_count {
        return Err(malformed(
            cluster_id,
            "sparse tile count disagrees with entries",
        ));
    }
    let tile_payload = bytes
        .get(entries_bytes..)
        .ok_or(malformed(cluster_id, "sparse tile payload is truncated"))?;
    let mut row_tile_start = 0u32;
    for row in &mut rows {
        let row_tile_end = row_tile_start
            .checked_add(row.tile_f16_count)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let start = usize::try_from(row_tile_start)
            .ok()
            .and_then(|value| value.checked_mul(2))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let end = usize::try_from(row_tile_end)
            .ok()
            .and_then(|value| value.checked_mul(2))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        row.tile_f16 = tile_payload
            .get(start..end)
            .ok_or(malformed(cluster_id, "sparse tile payload is truncated"))?
            .chunks_exact(2)
            .map(|half| u16::from_le_bytes([half[0], half[1]]))
            .collect();
        row_tile_start = row_tile_end;
    }

    Ok(rows)
}

pub(super) fn sparse_offsets_fit_allocation(
    offsets: &[u32],
    allocation_start: u32,
    tile_f16_count: u32,
) -> bool {
    let Some(allocation_end) = allocation_start.checked_add(tile_f16_count) else {
        return false;
    };
    if offsets.is_empty() {
        return tile_f16_count == 0;
    }
    if offsets[0] != allocation_start {
        return false;
    }
    offsets
        .windows(2)
        .all(|pair| pair[0] <= pair[1] && pair[1] <= allocation_end)
        && offsets
            .last()
            .is_none_or(|&offset| offset <= allocation_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_zero_tile_entry_at_allocation_end_is_valid_but_positive_overrun_is_not() {
        // Regression: compiler-emitted id-50 rows can retain a light with no kept tiles.
        let mut block = Vec::new();
        for word in [1u32, 2, 108, 0, 8, 0, 2, 1, 7, 0, 108, 0, 9, 108, 0, 0] {
            block.extend_from_slice(&word.to_le_bytes());
        }
        block.resize(block.len() + 108 * 2, 0);
        let rows = parse_sparse_rows(0, &block).unwrap();
        let row = &rows[0];
        assert_eq!(row.entry_tile_f16_offsets, [0, 108]);
        assert_eq!(row.tile_f16_count, 108);
        assert!(sparse_offsets_fit_allocation(
            &[4, 112],
            4,
            row.tile_f16_count
        ));
        // Moving the second start to 113 would extend the positive first entry past 108 halves.
        assert!(!sparse_offsets_fit_allocation(
            &[4, 113],
            4,
            row.tile_f16_count
        ));

        let mut empty_block = Vec::new();
        for word in [1u32, 1, 0, 0, 9, 0, 1, 1, 7, 0, 0, 0] {
            empty_block.extend_from_slice(&word.to_le_bytes());
        }
        let empty_row = parse_sparse_rows(0, &empty_block).unwrap();
        assert_eq!(empty_row[0].tile_f16_count, 0);
        assert_eq!(empty_row[0].entry_tile_f16_offsets, [0]);
        assert!(sparse_offsets_fit_allocation(&[0], 0, 0));
        assert!(!sparse_offsets_fit_allocation(&[], 0, 1));
    }
}
