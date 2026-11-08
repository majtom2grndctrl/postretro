// Probe depth tiles (PRL id 59): the per-load probe → tile reference table the
// moment-texel writers pack G from, the tile texture's texel plan, and the
// `ShGridInfo` words the shader finds a tile with.
// See: context/lib/rendering_pipeline.md §4 · context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::probe_depth_tiles::{
    MAX_PROBE_DEPTH_TILES, PROBE_DEPTH_TILE_BORDERED_EDGE, ProbeDepthTileTextureLayout,
    ProbeDepthTilesSection, tile_reference_bits,
};

/// The three `ShGridInfo` words that locate a tile without a texture-size
/// query: the layout's tiles-per-row shift (byte 28) and the tile texture's
/// reciprocal width (44) and height (92). All zero without tiles.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DepthTileGridWords {
    pub row_shift: u32,
    pub inv_width: f32,
    pub inv_height: f32,
}

impl DepthTileGridWords {
    pub const NONE: Self = Self {
        row_shift: 0,
        inv_width: 0.0,
        inv_height: 0.0,
    };

    pub fn for_layout(layout: Option<ProbeDepthTileTextureLayout>) -> Self {
        let Some(layout) = layout else {
            return Self::NONE;
        };
        let [width, height] = layout.extent();
        Self {
            row_shift: layout.tiles_per_row_shift,
            inv_width: 1.0 / width as f32,
            inv_height: 1.0 / height as f32,
        }
    }
}

/// One loaded level's probe → tile references, built from that load's id 59
/// and owned by its SH state, so nothing carries from one load to the next.
///
/// Every moment-texel writer packs G through [`Self::second_channel`]: a
/// tiled probe's sign-flagged reference, an untiled probe's id-34 `E[d²]`
/// bits. R is never touched. [`Self::set_reference`] is the runtime rewrite
/// seam: redirecting or clearing a probe changes only this table, and a writer
/// that repacks the probe picks it up; clearing restores `E[d²]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProbeDepthTileReferences {
    /// `(dense probe index, tile)`, ascending by probe.
    entries: Vec<(u32, u16)>,
    /// Tiles the level's tile texture holds; a reference names one of them.
    tile_count: u32,
}

impl ProbeDepthTileReferences {
    /// References from a load's id 59. Absent and zero-tile id 59 both give
    /// the empty table, so every probe packs exactly as before id 59 existed.
    pub fn from_section(section: Option<&ProbeDepthTilesSection>) -> Self {
        let Some(section) = section else {
            return Self::default();
        };
        debug_assert!(section.tile_count() as u32 <= MAX_PROBE_DEPTH_TILES);
        Self {
            entries: section
                .probe_indices
                .iter()
                .enumerate()
                .map(|(tile, &probe)| (probe, tile as u16))
                .collect(),
            tile_count: section.tile_count() as u32,
        }
    }

    pub fn tile_count(&self) -> u32 {
        self.tile_count
    }

    /// Tiles in the level's texture, as its layout prices them.
    pub fn texture_layout(&self) -> Option<ProbeDepthTileTextureLayout> {
        ProbeDepthTileTextureLayout::for_tile_count(self.tile_count)
    }

    /// The tile dense probe `probe` weighs with, if any.
    pub fn tile_of(&self, probe: u32) -> Option<u16> {
        self.entries
            .binary_search_by_key(&probe, |&(entry, _)| entry)
            .ok()
            .map(|at| self.entries[at].1)
    }

    /// The moment texel's G for valid dense probe `probe` whose id-34
    /// `E[d²]` bits are `mean_sq_distance`.
    pub fn second_channel(&self, probe: u32, mean_sq_distance: u16) -> u16 {
        self.tile_of(probe)
            .map_or(mean_sq_distance, tile_reference_bits)
    }

    /// Point `probe` at `tile`, or clear its reference with `None`. A tile
    /// past the level's texture is refused and changes nothing.
    pub fn set_reference(
        &mut self,
        probe: u32,
        tile: Option<u16>,
    ) -> Result<(), DepthTileReferenceError> {
        if let Some(tile) = tile
            && u32::from(tile) >= self.tile_count
        {
            return Err(DepthTileReferenceError::TileOutOfRange {
                tile,
                tile_count: self.tile_count,
            });
        }
        match (
            self.entries
                .binary_search_by_key(&probe, |&(entry, _)| entry),
            tile,
        ) {
            (Ok(at), Some(tile)) => self.entries[at].1 = tile,
            (Ok(at), None) => {
                self.entries.remove(at);
            }
            (Err(at), Some(tile)) => self.entries.insert(at, (probe, tile)),
            (Err(_), None) => {}
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepthTileReferenceError {
    TileOutOfRange { tile: u16, tile_count: u32 },
}

impl std::fmt::Display for DepthTileReferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TileOutOfRange { tile, tile_count } => write!(
                f,
                "depth tile {tile} is past the level's {tile_count} tile(s)"
            ),
        }
    }
}

/// The tile texture's extent and `Rg16Float` texels, row-major: each tile's
/// bordered square at its layout origin, unused texels zero. No tiles is the
/// 1×1 zero dummy the binding still needs.
pub struct DepthTileTexturePlan {
    pub extent: [u32; 2],
    pub texels: Vec<[u16; 2]>,
}

pub fn depth_tile_texture_plan(section: Option<&ProbeDepthTilesSection>) -> DepthTileTexturePlan {
    let Some((section, layout)) =
        section.and_then(|section| section.texture_layout().map(|layout| (section, layout)))
    else {
        return DepthTileTexturePlan {
            extent: [1, 1],
            texels: vec![[0, 0]],
        };
    };
    let [width, height] = layout.extent();
    let mut texels = vec![[0u16; 2]; width as usize * height as usize];
    let edge = PROBE_DEPTH_TILE_BORDERED_EDGE as usize;
    for tile in 0..section.tile_count() {
        let [ox, oy] = layout.tile_origin(tile as u32);
        let source = section.tile(tile);
        for row in 0..edge {
            let at = (oy as usize + row) * width as usize + ox as usize;
            texels[at..at + edge].copy_from_slice(&source[row * edge..(row + 1) * edge]);
        }
    }
    DepthTileTexturePlan {
        extent: [width, height],
        texels,
    }
}

#[cfg(test)]
#[path = "sh_depth_tiles_tests.rs"]
mod tests;
