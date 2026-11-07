// Forward vertex-stage lightmap block table: one `vec4<u32>` per entry,
// indexed by `WorldVertex::lightmap_block` (block id + 1, 0 = no lightmap).
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use super::BlockPlacement;

/// Bytes of one entry: a WGSL `vec4<u32>`.
pub const BLOCK_TABLE_ENTRY_BYTES: usize = 16;
/// The entry's texels are in the pool at its address.
pub const BLOCK_FLAG_RESIDENT: u32 = 1 << 0;
/// The "no lightmap" entry: zero static irradiance, neutral direction,
/// all-visible shadowmask. A real block with neither flag is a miss.
pub const BLOCK_FLAG_NONE: u32 = 1 << 1;
/// Plane A sits at the entry's `y` address. Clear, it reads zero.
pub const BLOCK_FLAG_PLANE_A: u32 = 1 << 2;
/// Plane B sits at the entry's `z` address. Clear, it reads zero.
pub const BLOCK_FLAG_PLANE_B: u32 = 1 << 3;
/// The level has no mask pool at all: the shadowmask reads fully visible.
/// Distinct from an absent plane, which reads zero.
pub const BLOCK_FLAG_NO_MASK_POOL: u32 = 1 << 4;

/// An offset's bits: texels / 4, since `block_alignment()` is at least 4.
const OFFSET_BITS: u32 = 9;
const OFFSET_MASK: u32 = (1 << OFFSET_BITS) - 1;
const OFFSET_UNIT: u32 = 4;
/// An address's layer bits, past any device's array limit.
const LAYER_BITS: u32 = 14;
const LAYER_SHIFT: u32 = OFFSET_BITS * 2;
/// An extent's bits: a block is at most a 2048-texel layer on a side.
const EXTENT_BITS: u32 = 12;
const EXTENT_MASK: u32 = (1 << EXTENT_BITS) - 1;
const FLAGS_SHIFT: u32 = EXTENT_BITS * 2;

/// Where a resident block's masks sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskAddresses {
    /// The level has no mask pool.
    NoPool,
    /// Plane A then plane B; `None` for an absent plane.
    Planes([Option<BlockPlacement>; 2]),
}

/// Every address of a resident block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockAddresses {
    pub lightmap: BlockPlacement,
    pub masks: MaskAddresses,
}

impl BlockAddresses {
    /// A block at `lightmap` in a level with no mask pool.
    pub fn without_masks(lightmap: BlockPlacement) -> Self {
        Self {
            lightmap,
            masks: MaskAddresses::NoPool,
        }
    }
}

/// One table entry. Layout, mirrored by `lightmap_block_table` in
/// forward.wgsl and decoded by `resolve_lightmap_block` in
/// lightmap_sample.wgsl, as native-endian `u32`s:
///
/// ```text
///   x  lightmap address   offset x/4 (9) | offset y/4 (9) << 9 | layer (14) << 18
///   y  plane A address    same
///   z  plane B address    same
///   w  extent w (12) | extent h (12) << 12 | flags (8) << 24
/// ```
///
/// The three addresses are three pool places: a block's lightmap and its two
/// planes need not share a layer or an offset. `block_alignment()` is at least
/// 4, so an offset is whole in texels / 4, and a pool layer is at most
/// `LIGHTMAP_POOL_LAYER_EDGE` (2048) texels on a side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockTableEntry {
    /// Entry 0 of a level with blocks.
    None,
    /// Entry 0 of a level without blocks: the 1×1 neutral placeholder
    /// textures, which read the same at any coordinate.
    Placeholder,
    /// A block whose texels are in the pools.
    Resident {
        addresses: BlockAddresses,
        width: u32,
        height: u32,
    },
    /// A real block whose texels are not in the pool. The extent stays so
    /// the block-local texel (and the animated lookup keyed on it) resolves.
    Missing { width: u32, height: u32 },
}

fn pack_address(placement: BlockPlacement) -> u32 {
    debug_assert!(
        placement.x.is_multiple_of(OFFSET_UNIT) && placement.y.is_multiple_of(OFFSET_UNIT),
        "a pool offset is a multiple of 4: {placement:?}"
    );
    debug_assert!(placement.layer < 1 << LAYER_BITS, "14-bit table layer");
    let (x, y) = (placement.x / OFFSET_UNIT, placement.y / OFFSET_UNIT);
    debug_assert!(x <= OFFSET_MASK && y <= OFFSET_MASK, "9-bit table offset");
    x | (y << OFFSET_BITS) | (placement.layer << LAYER_SHIFT)
}

fn unpack_address(word: u32) -> BlockPlacement {
    BlockPlacement {
        layer: word >> LAYER_SHIFT,
        x: (word & OFFSET_MASK) * OFFSET_UNIT,
        y: ((word >> OFFSET_BITS) & OFFSET_MASK) * OFFSET_UNIT,
    }
}

fn pack_extent(width: u32, height: u32, flags: u32) -> u32 {
    debug_assert!(
        width <= EXTENT_MASK && height <= EXTENT_MASK,
        "12-bit extent"
    );
    width | (height << EXTENT_BITS) | (flags << FLAGS_SHIFT)
}

impl BlockTableEntry {
    pub fn to_words(self) -> [u32; 4] {
        match self {
            Self::None => [0, 0, 0, pack_extent(0, 0, BLOCK_FLAG_NONE)],
            Self::Placeholder => [
                0,
                0,
                0,
                pack_extent(1, 1, BLOCK_FLAG_RESIDENT | BLOCK_FLAG_NO_MASK_POOL),
            ],
            Self::Resident {
                addresses,
                width,
                height,
            } => {
                let (planes, mask_flags) = match addresses.masks {
                    MaskAddresses::NoPool => ([None; 2], BLOCK_FLAG_NO_MASK_POOL),
                    MaskAddresses::Planes(planes) => {
                        let present =
                            |plane: Option<BlockPlacement>, flag| plane.map_or(0, |_| flag);
                        let flags = present(planes[0], BLOCK_FLAG_PLANE_A)
                            | present(planes[1], BLOCK_FLAG_PLANE_B);
                        (planes, flags)
                    }
                };
                [
                    pack_address(addresses.lightmap),
                    planes[0].map_or(0, pack_address),
                    planes[1].map_or(0, pack_address),
                    pack_extent(width, height, BLOCK_FLAG_RESIDENT | mask_flags),
                ]
            }
            Self::Missing { width, height } => [0, 0, 0, pack_extent(width, height, 0)],
        }
    }

    /// Inverse of [`Self::to_words`]. A resident entry whose extent is 1×1 at
    /// the origin of layer 0 with no mask pool reads back as
    /// [`Self::Placeholder`].
    pub fn from_words([lightmap, plane_a, plane_b, packed]: [u32; 4]) -> Self {
        let (width, height) = (packed & EXTENT_MASK, (packed >> EXTENT_BITS) & EXTENT_MASK);
        let flags = packed >> FLAGS_SHIFT;
        if flags & BLOCK_FLAG_NONE != 0 {
            return Self::None;
        }
        if flags & BLOCK_FLAG_RESIDENT == 0 {
            return Self::Missing { width, height };
        }
        let lightmap = unpack_address(lightmap);
        let plane = |word, flag| (flags & flag != 0).then(|| unpack_address(word));
        let masks = match flags & BLOCK_FLAG_NO_MASK_POOL != 0 {
            true => MaskAddresses::NoPool,
            false => MaskAddresses::Planes([
                plane(plane_a, BLOCK_FLAG_PLANE_A),
                plane(plane_b, BLOCK_FLAG_PLANE_B),
            ]),
        };
        let origin = BlockPlacement {
            layer: 0,
            x: 0,
            y: 0,
        };
        if lightmap == origin && (width, height) == (1, 1) && masks == MaskAddresses::NoPool {
            return Self::Placeholder;
        }
        Self::Resident {
            addresses: BlockAddresses { lightmap, masks },
            width,
            height,
        }
    }

    /// Lightmap pool texel `(layer, x, y)` a block-local texel reads, as the
    /// fragment stage resolves it: the placement plus the local texel. `None`
    /// for an entry that samples no pool block, or a texel outside the extent.
    pub fn pool_texel(self, local_x: u32, local_y: u32) -> Option<(u32, u32, u32)> {
        match self {
            Self::Resident {
                addresses,
                width,
                height,
            } if local_x < width && local_y < height => {
                let placement = addresses.lightmap;
                Some((
                    placement.layer,
                    placement.x + local_x,
                    placement.y + local_y,
                ))
            }
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
/// per block id, resident where `addresses[id]` is `Some` and a miss where it
/// is `None`.
pub fn block_table_bytes(extents: &[(u32, u32)], addresses: &[Option<BlockAddresses>]) -> Vec<u8> {
    assert_eq!(extents.len(), addresses.len(), "one address slot per block");
    let mut bytes = Vec::with_capacity((extents.len() + 1) * BLOCK_TABLE_ENTRY_BYTES);
    push_entry(&mut bytes, BlockTableEntry::None);
    for (&(width, height), address) in extents.iter().zip(addresses) {
        let entry = match *address {
            Some(addresses) => BlockTableEntry::Resident {
                addresses,
                width,
                height,
            },
            None => BlockTableEntry::Missing { width, height },
        };
        push_entry(&mut bytes, entry);
    }
    bytes
}
