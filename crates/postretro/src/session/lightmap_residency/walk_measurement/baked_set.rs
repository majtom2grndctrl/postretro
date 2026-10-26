//! The camera cell's baked set as the walks read it: blocks per cell, the
//! mandatory set at lead L, and which bake a run read.
//! See: context/lib/rendering_pipeline.md §4

use std::ops::Range;

use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_loader::LevelWorld;

/// Each cell's blocks from the owning cell of every block in block order: a
/// contiguous run, as the loader validates, empty for a cell without charts.
pub(super) fn cell_block_ranges(
    block_cells: impl IntoIterator<Item = u32>,
    cell_count: usize,
) -> Vec<Range<u32>> {
    let mut ranges = vec![0..0; cell_count];
    for (block, cell) in (0u32..).zip(block_cells) {
        let run = &mut ranges[cell as usize];
        if run.start == run.end {
            *run = block..block + 1;
        } else {
            assert_eq!(run.end, block, "cell {cell}'s blocks are contiguous");
            run.end += 1;
        }
    }
    ranges
}

/// The camera cell's mandatory cells at `lead` (fixed point), sorted.
pub(super) fn mandatory_cells(
    residency_set: &CellResidencySetSection,
    camera_cell: u32,
    lead: u32,
    out: &mut Vec<u32>,
) {
    out.clear();
    out.extend(
        residency_set
            .entries_for(camera_cell as usize)
            .iter()
            .filter(|entry| entry.lead <= lead)
            .map(|entry| entry.cell_id),
    );
    out.sort_unstable();
    out.dedup();
}

/// Every block of the camera cell's mandatory cells at `lead` (fixed point),
/// sorted.
pub(super) fn mandatory_blocks(
    residency_set: &CellResidencySetSection,
    cell_blocks: &[Range<u32>],
    camera_cell: u32,
    lead: u32,
    out: &mut Vec<u32>,
) {
    out.clear();
    out.extend(
        residency_set
            .entries_for(camera_cell as usize)
            .iter()
            .filter(|entry| entry.lead <= lead)
            .flat_map(|entry| {
                cell_blocks
                    .get(entry.cell_id as usize)
                    .cloned()
                    .unwrap_or(0..0)
            }),
    );
    out.sort_unstable();
    out.dedup();
}

/// Blocks in every camera cell's baked mandatory set at `lead`: p50, p95,
/// max.
pub(super) fn mandatory_set_sizes(
    world: &LevelWorld,
    residency_set: &CellResidencySetSection,
    cell_blocks: &[Range<u32>],
    lead: u32,
) -> [u64; 3] {
    let mut blocks = Vec::new();
    let mut sizes: Vec<u64> = (0..residency_set.camera_cell_count().min(world.cells.len()))
        .filter(|&cell| !world.cells[cell].is_solid && !world.cells[cell].is_exterior)
        .map(|cell| {
            mandatory_blocks(residency_set, cell_blocks, cell as u32, lead, &mut blocks);
            blocks.len() as u64
        })
        .collect();
    if sizes.is_empty() {
        return [0; 3];
    }
    sizes.sort_unstable();
    let at = |q: f64| sizes[((sizes.len() as f64 * q).ceil() as usize).clamp(1, sizes.len()) - 1];
    [at(0.5), at(0.95), at(1.0)]
}

/// The PRL and id 51 a run read (O14), as `key=value` tokens. The walk
/// epoch is this build's; the runtime never reads a bake's.
pub(super) fn bake_identity(path: &str, world: &LevelWorld) -> String {
    let (size, mtime) = std::fs::metadata(path).map_or((0, 0), |meta| {
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_secs());
        (meta.len(), mtime)
    });
    let id51 = world.cell_residency_set.as_ref().map_or_else(
        || "none".to_string(),
        |set| blake3::hash(&set.to_bytes()).to_hex()[..16].to_string(),
    );
    format!(
        "id51={id51} walk_epoch={} size={size} mtime={mtime} prl={path}",
        postretro_visibility::PORTAL_WALK_EPOCH,
    )
}

pub(super) fn bake_token<'a>(bake: &'a str, key: &str) -> Option<&'a str> {
    bake.split_ascii_whitespace()
        .find_map(|token| token.strip_prefix(key)?.strip_prefix('='))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mandatory_blocks_include_every_block_of_a_multi_block_cell() {
        use crate::lightmap_streaming::test_fixtures::{M, residency_set};

        // Cell 0 owns blocks 0 and 1, cell 1 owns block 2, cell 2 owns none.
        let cell_blocks = cell_block_ranges([0, 0, 1], 3);
        assert_eq!(cell_blocks, [0..2, 2..3, 0..0]);

        // Camera 1 reaches cell 1 at 0 m, cell 0 at 8 m and cell 2 at 20 m.
        let set = residency_set(3, &[(1, 1, 0), (1, 0, 8), (1, 2, 20)], 32);
        let mut blocks = Vec::new();
        mandatory_blocks(&set, &cell_blocks, 1, 0, &mut blocks);
        assert_eq!(blocks, [2]);
        mandatory_blocks(&set, &cell_blocks, 1, 8 * M, &mut blocks);
        assert_eq!(blocks, [0, 1, 2], "cell 0 brings both of its blocks");
        mandatory_blocks(&set, &cell_blocks, 1, 32 * M, &mut blocks);
        assert_eq!(blocks, [0, 1, 2], "a chartless cell adds none");
    }
}
