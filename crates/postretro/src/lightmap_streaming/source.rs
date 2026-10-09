//! Where the lightmap controller and route read block facts and bytes.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::ops::Range;
use std::sync::Arc;

use postretro_level_format::lightmap::LightmapBlockPayload;
use postretro_level_loader::{LightmapBlockFileRanges, LightmapStreamManifest, PrlLoadError};

/// One block's index facts: its owning cell and texel extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockSummary {
    pub(crate) cell_id: u32,
    pub(crate) width: u16,
    pub(crate) height: u16,
}

/// A level's streamed lightmap blocks. Production reads the retained
/// [`LightmapStreamManifest`]; tests inject in-memory sources that hold chosen
/// reads by offset.
///
/// Contract: `block_file_ranges` returns the same ranges for a block for the
/// source's lifetime; `read_file_span` returns exactly the span's length or
/// an error, may cover several byte-contiguous block ranges (across the
/// id-22/42 boundary when the sections touch), and is called on the issuer
/// thread, or by a synchronous preload that runs before the issuer starts;
/// `payload_from_pair_bytes` rejects buffers whose lengths disagree with the
/// block's ranges.
pub(crate) trait LightmapBlockSource: Send + Sync {
    fn block_count(&self) -> u32;
    fn block_summary(&self, block: u32) -> Option<BlockSummary>;
    /// The pool slot alignment every block edge rounds up to.
    fn block_alignment(&self) -> u32;
    fn block_file_ranges(&self, block: u32) -> Result<LightmapBlockFileRanges, PrlLoadError>;
    fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError>;
    fn payload_from_pair_bytes(
        &self,
        block: u32,
        lightmap: Vec<u8>,
        shadowmask: Option<Vec<u8>>,
    ) -> Result<LightmapBlockPayload, PrlLoadError>;
    /// The level's content identity for stale-completion checks.
    fn content_tag(&self) -> [u8; 32];

    /// One pair read synchronously on the calling thread: install preload and
    /// capture, never the frame path.
    #[cfg_attr(
        not(feature = "capture"),
        allow(dead_code, reason = "capture and tests preload synchronously")
    )]
    fn read_block_pair(&self, block: u32) -> Result<LightmapBlockPayload, PrlLoadError> {
        let ranges = self.block_file_ranges(block)?;
        let lightmap = self.read_file_span(ranges.lightmap)?;
        let shadowmask = ranges
            .shadowmask
            .map(|range| self.read_file_span(range))
            .transpose()?;
        self.payload_from_pair_bytes(block, lightmap, shadowmask)
    }
}

impl std::fmt::Debug for dyn LightmapBlockSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LightmapBlockSource")
            .field("blocks", &self.block_count())
            .finish_non_exhaustive()
    }
}

/// The production source: the level's retained streaming manifest.
#[derive(Debug)]
pub(crate) struct ManifestBlockSource {
    manifest: Arc<LightmapStreamManifest>,
}

impl ManifestBlockSource {
    pub(crate) fn new(manifest: Arc<LightmapStreamManifest>) -> Self {
        Self { manifest }
    }
}

impl LightmapBlockSource for ManifestBlockSource {
    fn block_count(&self) -> u32 {
        self.manifest.block_count()
    }

    fn block_summary(&self, block: u32) -> Option<BlockSummary> {
        self.manifest
            .lightmap_index()
            .records
            .get(block as usize)
            .map(|record| BlockSummary {
                cell_id: record.cell_id,
                width: record.width,
                height: record.height,
            })
    }

    fn block_alignment(&self) -> u32 {
        self.manifest.lightmap_index().header.block_alignment()
    }

    fn block_file_ranges(&self, block: u32) -> Result<LightmapBlockFileRanges, PrlLoadError> {
        self.manifest.block_file_ranges(block)
    }

    fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        self.manifest.read_file_span(range)
    }

    fn payload_from_pair_bytes(
        &self,
        block: u32,
        lightmap: Vec<u8>,
        shadowmask: Option<Vec<u8>>,
    ) -> Result<LightmapBlockPayload, PrlLoadError> {
        self.manifest
            .payload_from_pair_bytes(block, lightmap, shadowmask)
    }

    fn content_tag(&self) -> [u8; 32] {
        self.manifest.content_tag()
    }

    /// The manifest's own counted positional pair read.
    fn read_block_pair(&self, block: u32) -> Result<LightmapBlockPayload, PrlLoadError> {
        self.manifest.read_block_pair(block)
    }
}
