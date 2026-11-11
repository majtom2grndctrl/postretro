// Where a gated light's per-probe term T comes from when an id-57 record
// borrows it: id 41 or id 45, as resident metadata and per-cell tile rows,
// the same in a whole load and a streamed one.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use postretro_level_format::mover_probe_channel::MOVER_IRRADIANCE_TILE_F16_COUNT;
use postretro_level_format::sh_reconstruct::{Level, kept_mask, stored_delta_tiles};

use super::layout::MoverChannelError;

const TILE_HALVES: usize = MOVER_IRRADIANCE_TILE_F16_COUNT;

fn fail<T>(message: String) -> Result<T, MoverChannelError> {
    Err(MoverChannelError(message))
}

/// One sparse delta family (id 41 or id 45) as resident metadata: the same
/// in a whole load and a streamed one, where only its tiles stream.
#[derive(Clone, Copy, Debug)]
pub(in crate::render) struct TermFamily<'a> {
    pub(in crate::render) name: &'static str,
    pub(in crate::render) valid_probe_masks: &'a [u64],
    pub(in crate::render) cell_levels: &'a [u8],
    /// CSR, one offset per affinity cell plus a trailing total.
    pub(in crate::render) affinity_offsets: &'a [u32],
    pub(in crate::render) affinity_lights: &'a [u32],
}

impl TermFamily<'_> {
    /// The cell-local entry holding `light`'s per-probe term. The channel
    /// never rebuilds T from corners, so the cell must sit at L0 and keep
    /// every probe `mask` names.
    pub(super) fn entry_in_cell(
        &self,
        cell: usize,
        light: u32,
        mask: u64,
    ) -> Result<usize, MoverChannelError> {
        let name = self.name;
        let (Some(&level), Some(&valid), Some(range)) = (
            self.cell_levels.get(cell),
            self.valid_probe_masks.get(cell),
            self.affinity_offsets.get(cell..cell + 2),
        ) else {
            return fail(format!("affinity cell {cell} is outside {name}"));
        };
        if Level::from_u8(level) != Some(Level::L0) {
            return fail(format!(
                "{name} stores affinity cell {cell} at level {level}, not L0, under gated probes"
            ));
        }
        if mask & !kept_mask(Level::L0, valid) != 0 {
            return fail(format!(
                "a gated probe of affinity cell {cell} is not a valid {name} probe"
            ));
        }
        let lights = self
            .affinity_lights
            .get(range[0] as usize..range[1] as usize)
            .unwrap_or(&[]);
        match lights.iter().position(|&entry| entry == light) {
            Some(entry) => Ok(entry),
            None => fail(format!(
                "{name} holds no entry for light {light} at affinity cell {cell}, and the record carries no tile of its own"
            )),
        }
    }
}

/// One affinity cell's tiles in a delta family: each entry's first f16 half
/// in `tiles`, and the cell's valid probes, which an L0 cell keeps in order.
#[derive(Clone, Copy, Debug)]
pub(in crate::render) struct TermRow<'a> {
    pub(in crate::render) entry_offsets: &'a [u32],
    pub(in crate::render) tiles: &'a [u16],
    pub(in crate::render) valid_probe_mask: u64,
}

impl<'a> TermRow<'a> {
    pub(super) fn tile(&self, entry: usize, local_probe: u32) -> Option<&'a [u16]> {
        let below = (1u64 << local_probe) - 1;
        let rank = (self.valid_probe_mask & below).count_ones() as usize;
        let start = *self.entry_offsets.get(entry)? as usize + rank * TILE_HALVES;
        self.tiles.get(start..start + TILE_HALVES)
    }
}

/// A whole-loaded delta family's rows: each CSR entry's first f16 half in
/// the family's compact payload, entries in CSR order.
pub(in crate::render) struct WholeTermRows<'a> {
    family: TermFamily<'a>,
    tiles: &'a [u16],
    entry_offsets: Vec<u32>,
}

impl<'a> WholeTermRows<'a> {
    pub(in crate::render) fn new(family: TermFamily<'a>, tiles: &'a [u16]) -> Self {
        let cells = family.affinity_offsets.len().saturating_sub(1);
        let mut entry_offsets = Vec::with_capacity(family.affinity_lights.len());
        let mut next = 0u32;
        for cell in 0..cells {
            let level = Level::from_u8(family.cell_levels[cell]).unwrap_or(Level::L0);
            let kept = stored_delta_tiles(level, family.valid_probe_masks[cell]) as u32;
            for _ in family.affinity_offsets[cell]..family.affinity_offsets[cell + 1] {
                entry_offsets.push(next);
                next = next.saturating_add(kept * TILE_HALVES as u32);
            }
        }
        Self {
            family,
            tiles,
            entry_offsets,
        }
    }

    pub(in crate::render) fn row(&self, cell: u32) -> Option<TermRow<'_>> {
        let cell = cell as usize;
        let range = self.family.affinity_offsets.get(cell..cell + 2)?;
        Some(TermRow {
            entry_offsets: self
                .entry_offsets
                .get(range[0] as usize..range[1] as usize)?,
            tiles: self.tiles,
            valid_probe_mask: *self.family.valid_probe_masks.get(cell)?,
        })
    }
}
