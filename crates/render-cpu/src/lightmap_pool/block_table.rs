// Forward vertex-stage lightmap block table: one `vec4<u32>` per entry,
// indexed by `WorldVertex::lightmap_block` (block id + 1, 0 = no lightmap).
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use super::BlockPlacement;

/// Bytes of one entry: a WGSL `vec4<u32>`.
pub const BLOCK_TABLE_ENTRY_BYTES: usize = 16;
/// The entry's texels are in the pool at its layer and offset.
pub const BLOCK_FLAG_RESIDENT: u32 = 1 << 0;
/// The "no lightmap" entry: zero static irradiance, neutral direction,
/// all-visible shadowmask. A real block with neither flag is a miss.
pub const BLOCK_FLAG_NONE: u32 = 1 << 1;

/// One table entry. Layout, mirrored by `lightmap_block_table` in
/// forward.wgsl and decoded by `resolve_lightmap_block` in
/// lightmap_sample.wgsl, as native-endian `u32`s:
///
/// ```text
///   x  pool layer
///   y  pool offset x | pool offset y << 16
///   z  block extent w | block extent h << 16
///   w  flags (BLOCK_FLAG_RESIDENT, BLOCK_FLAG_NONE)
/// ```
///
/// Offsets and extents fit 16 bits: a pool layer is at most
/// `LIGHTMAP_POOL_LAYER_EDGE` (2048) texels on a side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockTableEntry {
    /// Entry 0 of a level with blocks.
    None,
    /// Entry 0 of a level without blocks: the 1×1 neutral placeholder
    /// textures, which read the same at any coordinate.
    Placeholder,
    /// A block whose texels are in the pool.
    Resident {
        placement: BlockPlacement,
        width: u32,
        height: u32,
    },
    /// A real block whose texels are not in the pool. The extent stays so
    /// the block-local texel (and the animated lookup keyed on it) resolves.
    Missing { width: u32, height: u32 },
}

impl BlockTableEntry {
    pub fn to_words(self) -> [u32; 4] {
        let pack = |low: u32, high: u32| {
            debug_assert!(low <= 0xffff && high <= 0xffff, "16-bit table field");
            low | (high << 16)
        };
        match self {
            Self::None => [0, 0, 0, BLOCK_FLAG_NONE],
            Self::Placeholder => [0, 0, pack(1, 1), BLOCK_FLAG_RESIDENT],
            Self::Resident {
                placement,
                width,
                height,
            } => [
                placement.layer,
                pack(placement.x, placement.y),
                pack(width, height),
                BLOCK_FLAG_RESIDENT,
            ],
            Self::Missing { width, height } => [0, 0, pack(width, height), 0],
        }
    }

    /// Inverse of [`Self::to_words`]. A resident entry whose extent is 1×1 at
    /// the origin of layer 0 reads back as [`Self::Placeholder`].
    pub fn from_words([layer, offset, extent, flags]: [u32; 4]) -> Self {
        let (width, height) = (extent & 0xffff, extent >> 16);
        if flags & BLOCK_FLAG_NONE != 0 {
            return Self::None;
        }
        if flags & BLOCK_FLAG_RESIDENT == 0 {
            return Self::Missing { width, height };
        }
        let placement = BlockPlacement {
            layer,
            x: offset & 0xffff,
            y: offset >> 16,
        };
        if placement
            == (BlockPlacement {
                layer: 0,
                x: 0,
                y: 0,
            })
            && (width, height) == (1, 1)
        {
            return Self::Placeholder;
        }
        Self::Resident {
            placement,
            width,
            height,
        }
    }

    /// Pool texel `(layer, x, y)` a block-local texel reads, as the fragment
    /// stage resolves it: the placement plus the local texel. `None` for an
    /// entry that samples no pool block, or a texel outside the extent.
    pub fn pool_texel(self, local_x: u32, local_y: u32) -> Option<(u32, u32, u32)> {
        match self {
            Self::Resident {
                placement,
                width,
                height,
            } if local_x < width && local_y < height => Some((
                placement.layer,
                placement.x + local_x,
                placement.y + local_y,
            )),
            _ => None,
        }
    }
}

fn push_entry(bytes: &mut Vec<u8>, entry: BlockTableEntry) {
    for word in entry.to_words() {
        bytes.extend_from_slice(&word.to_ne_bytes());
    }
}

/// The table of a level without blocks: entry 0 samples the placeholders,
/// and the vertex stage resolves any id past the table to entry 0.
pub fn placeholder_block_table() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(BLOCK_TABLE_ENTRY_BYTES);
    push_entry(&mut bytes, BlockTableEntry::Placeholder);
    bytes
}

/// The table of a level with blocks: entry 0 is "no lightmap", then one entry
/// per block id, resident where `placements[id]` is `Some` and a miss where it
/// is `None`.
pub fn block_table_bytes(extents: &[(u32, u32)], placements: &[Option<BlockPlacement>]) -> Vec<u8> {
    assert_eq!(
        extents.len(),
        placements.len(),
        "one placement slot per block"
    );
    let mut bytes = Vec::with_capacity((extents.len() + 1) * BLOCK_TABLE_ENTRY_BYTES);
    push_entry(&mut bytes, BlockTableEntry::None);
    for (&(width, height), placement) in extents.iter().zip(placements) {
        let entry = match *placement {
            Some(placement) => BlockTableEntry::Resident {
                placement,
                width,
                height,
            },
            None => BlockTableEntry::Missing { width, height },
        };
        push_entry(&mut bytes, entry);
    }
    bytes
}
