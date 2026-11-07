// The lightmap pool and the mask pool as one pair: their names, a figure for
// each, and how a mask placement names its block's plane.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::ops::{AddAssign, Index, IndexMut, SubAssign};

/// One of a level's two block pools. The lightmap pool holds a block's
/// irradiance and direction; the mask pool holds each plane the block stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PoolKind {
    Lightmap,
    Mask,
}

impl PoolKind {
    pub const ALL: [Self; 2] = [Self::Lightmap, Self::Mask];

    /// The block that owns `item` of this pool. A lightmap pool item is its
    /// block id; a mask pool item is [`mask_item`].
    pub fn block_of(self, item: u32) -> u32 {
        match self {
            Self::Lightmap => item,
            Self::Mask => item / 2,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Lightmap => "lightmap",
            Self::Mask => "mask",
        }
    }
}

/// `block`'s plane `plane` (0 for group A, 1 for group B) as a mask pool item:
/// the two planes of one block stay two items, told apart everywhere a
/// placement is.
pub fn mask_item(block: u32, plane: usize) -> u32 {
    debug_assert!(plane < 2, "a block has two planes");
    block * 2 + plane as u32
}

/// The plane a mask pool item names.
pub fn mask_item_plane(item: u32) -> usize {
    (item % 2) as usize
}

/// One figure for each pool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PerPool<T> {
    pub lightmap: T,
    pub mask: T,
}

impl<T> PerPool<T> {
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> PerPool<U> {
        PerPool {
            lightmap: f(self.lightmap),
            mask: f(self.mask),
        }
    }
}

impl<T: Clone> PerPool<T> {
    pub fn both(value: T) -> Self {
        Self {
            lightmap: value.clone(),
            mask: value,
        }
    }
}

impl PerPool<bool> {
    pub fn any(self) -> bool {
        self.lightmap || self.mask
    }
}

impl<T> Index<PoolKind> for PerPool<T> {
    type Output = T;

    fn index(&self, kind: PoolKind) -> &T {
        match kind {
            PoolKind::Lightmap => &self.lightmap,
            PoolKind::Mask => &self.mask,
        }
    }
}

impl<T> IndexMut<PoolKind> for PerPool<T> {
    fn index_mut(&mut self, kind: PoolKind) -> &mut T {
        match kind {
            PoolKind::Lightmap => &mut self.lightmap,
            PoolKind::Mask => &mut self.mask,
        }
    }
}

impl AddAssign for PerPool<u64> {
    fn add_assign(&mut self, other: Self) {
        self.lightmap += other.lightmap;
        self.mask += other.mask;
    }
}

impl SubAssign for PerPool<u64> {
    fn sub_assign(&mut self, other: Self) {
        self.lightmap -= other.lightmap;
        self.mask -= other.mask;
    }
}
