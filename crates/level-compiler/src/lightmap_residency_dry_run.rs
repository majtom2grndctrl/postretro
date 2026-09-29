//! Read-only lightmap residency dry run over a compiled PRL.
//!
//! Attributes Lightmap (id 22) and ShadowmaskAtlas (id 42) bytes to the cells
//! and clusters whose charts own them, then measures each camera cell's
//! mandatory resident bytes under distance bounds and three atlas layouts:
//! today's stored packing and soft cluster-ordered packing at two layer caps.
//! A second pass bounds the mandatory set by sampled visibility instead of
//! distance: everything visible from the cells a movement lead reaches, and
//! costs those sets under fixed-size tiles owned by one cell or cluster each,
//! and under per-cell blocks allocated into a pool of `POOL_LAYER_EDGE²` layers.
//! A third pass measures the problem brief's own set: a baked lead map with
//! one-hop dilation and no camera-cluster term, its prefetch band, and pool
//! walks under the brief's miss policy.
//! Measurement only; nothing here feeds a bake.
//! See: context/plans/large-map-spatial-residency.md ·
//! context/lib/build_pipeline.md §PRL section IDs

mod attribution;
mod band_pool_sim;
mod block_pool_sim;
mod brief_set;
mod brief_set_render;
mod brief_set_residency;
mod camera_walks;
mod cell_block_residency;
mod cell_blocks;
mod cell_blocks_render;
mod inputs;
mod layouts;
mod mandatory;
mod portal_distance;
mod pvs_sampling;
mod render;
mod report;
mod tiles;
mod tiles_render;
mod visible_set;
mod visible_set_render;

#[cfg(test)]
mod brief_set_tests;
#[cfg(test)]
mod cell_blocks_tests;
#[cfg(test)]
mod dry_run_test_fixtures;
#[cfg(test)]
mod real_prl_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tiles_tests;
#[cfg(test)]
mod visible_set_tests;

use postretro_level_format::cell_visibility::CoupledPairRecord;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::shadowmask_atlas::SHADOWMASK_GROUP_COUNT;
use postretro_level_loader::LevelWorld;

use crate::chart_raster::CHART_PADDING_TEXELS;
use crate::shadowmask_bake::MAX_SHADOWMASK_TEXTURE_WIDTH;

pub(crate) use inputs::read_dry_run_input;
pub(crate) use portal_distance::PortalGraphInput;
pub(crate) use report::run_dry_run;

/// Bytes of one 4×4 BC block (BC5, BC6H).
const BC_BLOCK_BYTES: u64 = 16;
const BC_BLOCK_EDGE: u32 = 4;

/// One grid unit of a stored encoding: `width × height` atlas texels cost
/// `bytes`. BC formats attribute at block granularity, raw formats per texel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EncodingUnit {
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

/// Shadowmask (id 42) payload as its block records declare it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShadowmaskFormat {
    /// Group A plus group B bytes of each stored block, in block order.
    pub block_bytes: Vec<u64>,
    pub payload_bytes: u64,
}

/// Whether a level carries id 42, and why not when it doesn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShadowmaskState {
    Stored(ShadowmaskFormat),
    /// EntityShadowLights (id 40) selects lights but the bake omitted id 42.
    /// Pre-block PRLs did so when a layer could not double; a block PRL never
    /// should, so on one this flags a compiler fault.
    OmittedForWidth,
    /// No selected shadow lights: the level has no shadowmask to stream.
    Absent,
}

/// One stored id-22 cell block: its extent and blob bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoredBlock {
    pub cell: u32,
    pub width: u32,
    pub height: u32,
    pub irradiance_bytes: u64,
    pub direction_bytes: u64,
}

/// Stored encodings of ids 22 and 42: the header's formats and every block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AtlasFormats {
    pub irr_format: u32,
    /// Irradiance texels per direction texel along each axis.
    pub direction_texel_scale: u32,
    /// Bytes of one direction texel (2 for Rg8).
    pub direction_texel_bytes: u64,
    /// Stored blocks in block-id order. A chart's `layer` is its block id.
    pub blocks: Vec<StoredBlock>,
    pub shadowmask: ShadowmaskState,
}

impl AtlasFormats {
    pub(crate) fn irradiance_unit(&self) -> EncodingUnit {
        match self.irr_format {
            IRRADIANCE_FORMAT_BC6H => EncodingUnit {
                width: BC_BLOCK_EDGE,
                height: BC_BLOCK_EDGE,
                bytes: BC_BLOCK_BYTES,
            },
            IRRADIANCE_FORMAT_RGBA16F => EncodingUnit {
                width: 1,
                height: 1,
                bytes: 8,
            },
            other => panic!("unknown lightmap irradiance format tag {other}"),
        }
    }

    /// One BC5 block position covers both mask groups.
    pub(crate) fn shadowmask_unit() -> EncodingUnit {
        EncodingUnit {
            width: BC_BLOCK_EDGE,
            height: BC_BLOCK_EDGE,
            bytes: BC_BLOCK_BYTES * u64::from(SHADOWMASK_GROUP_COUNT),
        }
    }

    pub(crate) fn stored_shadowmask(&self) -> Option<&ShadowmaskFormat> {
        match &self.shadowmask {
            ShadowmaskState::Stored(format) => Some(format),
            ShadowmaskState::OmittedForWidth | ShadowmaskState::Absent => None,
        }
    }

    pub(crate) fn irr_payload_bytes(&self) -> u64 {
        self.blocks.iter().map(|b| b.irradiance_bytes).sum()
    }

    pub(crate) fn dir_payload_bytes(&self) -> u64 {
        self.blocks.iter().map(|b| b.direction_bytes).sum()
    }

    /// Whether a layer `width` texels wide would carry id 42: the level
    /// selects shadow lights and the doubled texture fits. A stored block
    /// always fits; the width rule still bounds the simulated whole layers.
    pub(crate) fn layer_carries_shadowmask(&self, width: u32) -> bool {
        let selected = !matches!(self.shadowmask, ShadowmaskState::Absent);
        selected
            && width
                .checked_mul(SHADOWMASK_GROUP_COUNT)
                .is_some_and(|doubled| doubled <= MAX_SHADOWMASK_TEXTURE_WIDTH)
    }

    /// Grid a runtime block's origin and extent sit on: the BC block edge and
    /// the direction scale, so a block covers whole BC blocks and whole
    /// direction texels. The bake's own rule.
    pub(crate) fn block_alignment(&self) -> u32 {
        crate::lightmap_bake::BlockLayout {
            direction_texel_scale: self.direction_texel_scale,
            ..Default::default()
        }
        .alignment()
    }

    /// Id 22 plus id 42 bytes for one layer of `width × height` irradiance
    /// texels, at the stored encodings and the stored direction scale.
    pub(crate) fn layer_bytes_at(&self, width: u32, height: u32) -> u64 {
        let irr = self.irradiance_unit();
        let irradiance = u64::from(width.div_ceil(irr.width))
            * u64::from(height.div_ceil(irr.height))
            * irr.bytes;
        let scale = self.direction_texel_scale.max(1);
        assert!(
            width % scale == 0 && height % scale == 0,
            "layer extent {width}x{height} does not divide into the direction scale {scale}"
        );
        let direction =
            u64::from(width / scale) * u64::from(height / scale) * self.direction_texel_bytes;
        let shadowmask = if self.layer_carries_shadowmask(width) {
            let unit = Self::shadowmask_unit();
            u64::from(width.div_ceil(unit.width))
                * u64::from(height.div_ceil(unit.height))
                * unit.bytes
        } else {
            0
        };
        irradiance + direction + shadowmask
    }

    /// Stored id 22 plus id 42 bytes of block `block`, from its records.
    pub(crate) fn stored_block_bytes(&self, block: usize) -> u64 {
        let stored = &self.blocks[block];
        let mask = self
            .stored_shadowmask()
            .map_or(0, |sm| sm.block_bytes[block]);
        stored.irradiance_bytes + stored.direction_bytes + mask
    }

    /// Id 22 bytes per irradiance texel of chart area (irradiance + direction).
    pub(crate) fn lightmap_bytes_per_texel(&self) -> f64 {
        let irr = self.irradiance_unit();
        let irradiance = irr.bytes as f64 / f64::from(irr.width * irr.height);
        let scale = f64::from(self.direction_texel_scale.max(1));
        irradiance + self.direction_texel_bytes as f64 / (scale * scale)
    }

    /// Id 42 bytes per irradiance texel of chart area, charged only when id
    /// 42 is stored. An omitted shadowmask is charged by layouts whose layers
    /// fit it, never by the texel-exact floor.
    pub(crate) fn shadowmask_bytes_per_texel(&self) -> f64 {
        match self.shadowmask {
            ShadowmaskState::Stored(_) => {
                let unit = Self::shadowmask_unit();
                unit.bytes as f64 / f64::from(unit.width * unit.height)
            }
            ShadowmaskState::OmittedForWidth | ShadowmaskState::Absent => 0.0,
        }
    }

    /// Id 42 bytes per chart texel a level would carry if its omitted
    /// shadowmask were restored; `None` for any other state. Makes the floor
    /// comparable to capped layouts, which charge id 42 where it fits.
    pub(crate) fn omitted_shadowmask_bytes_per_texel(&self) -> Option<f64> {
        matches!(self.shadowmask, ShadowmaskState::OmittedForWidth).then(|| {
            let unit = Self::shadowmask_unit();
            unit.bytes as f64 / f64::from(unit.width * unit.height)
        })
    }

    pub(crate) fn bytes_per_texel(&self) -> f64 {
        self.lightmap_bytes_per_texel() + self.shadowmask_bytes_per_texel()
    }
}

/// One face chart's padded rectangle in irradiance texel space, recovered
/// from the stored per-vertex block id and block-local lightmap UVs. In a
/// simulated layout `layer` is that layout's layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChartRect {
    pub cell: u32,
    /// Stored block holding the chart; `x`/`y` are block-local.
    pub layer: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl ChartRect {
    pub(crate) fn area(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// Padded area with the interior halved per axis and the padding kept.
    pub(crate) fn half_res_area(&self) -> u64 {
        let pad = 2 * CHART_PADDING_TEXELS;
        let half = |padded: u32| padded.saturating_sub(pad).max(1).div_ceil(2) + pad;
        u64::from(half(self.width)) * u64::from(half(self.height))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CellInfo {
    /// World-space AABB center, for locating a cell in the editor.
    pub center: [f32; 3],
    pub cluster: u32,
    /// Non-solid and non-exterior per the Cells (id 38) flags: a cell the
    /// camera can stand in, charted or not.
    pub camera_candidate: bool,
    /// Exterior per the Cells (id 38) flags.
    pub exterior: bool,
}

/// One entry per Geometry (id 17) face, in the bake's chart order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FaceSlot {
    /// Index into `DryRunInput::charts`.
    Chart(usize),
    /// A face the bake planned as a 1×1 degenerate-face chart. The packer
    /// still places it, so the stored-order repack must too.
    Placeholder { cell: u32 },
}

/// How recovered faces split between placed charts and 1×1 placeholders.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ReconstructionStats {
    pub faces: usize,
    pub charts: usize,
    /// No index range (no BVH leaf): the bake packs a 1×1 placeholder.
    pub zero_index_faces: usize,
    /// One or two indices: the bake packs a 1×1 placeholder.
    pub short_index_faces: usize,
    /// Every vertex UV is zero or names no block: a placeholder chart whose
    /// UVs clamped to the block origin, or no baked chart at all (a section
    /// without blocks).
    pub uncharted_faces: usize,
    /// A vertex UV sits on the block edge, which no padded chart interior can
    /// reach: a zero-extent placeholder chart whose UVs clamped there.
    pub edge_clamped_faces: usize,
    /// Vertices of one face name different blocks.
    pub mixed_layer_faces: usize,
    /// The rect leaves its block, or the block belongs to another cell.
    pub out_of_bounds_faces: usize,
    /// BVH faces with no matching Geometry face record; nonzero means the
    /// face order could not be recovered.
    pub unmatched_bvh_faces: usize,
}

/// Everything the dry run reads from a PRL, decoupled from section types so
/// synthetic fixtures can build it directly.
#[derive(Debug)]
pub(crate) struct DryRunInput {
    pub formats: AtlasFormats,
    /// Recovered charts in face order.
    pub charts: Vec<ChartRect>,
    /// Every face in bake order, charts and placeholders alike.
    pub faces: Vec<FaceSlot>,
    pub cells: Vec<CellInfo>,
    pub cluster_count: u32,
    pub pinned_clusters: Vec<u32>,
    pub cell_visibility_present: bool,
    pub component_ids: Vec<u32>,
    pub coupled_pairs: Vec<CoupledPairRecord>,
    /// Portal hub geometry for the untruncated distance recompute; absent in
    /// fixtures that exercise only the stored id-46 records.
    pub portal_graph: Option<PortalGraphInput>,
    /// Runtime visibility world (cells, locator, portals) for the sampled
    /// visible-set pass; absent in fixtures that skip it.
    pub visibility_world: Option<LevelWorld>,
    /// Portals the runtime loader would reject. It rejects all portals when
    /// any one is bad, so a nonzero count means the shipped runtime takes its
    /// no-portals fallback instead of the walks sampled here.
    pub loader_rejected_portals: usize,
    pub reconstruction: ReconstructionStats,
}

impl DryRunInput {
    pub(crate) fn cell_count(&self) -> usize {
        self.cells.len()
    }

    /// Cell ids of each cluster, ascending.
    pub(crate) fn cluster_members(&self) -> Vec<Vec<u32>> {
        let mut members = vec![Vec::new(); self.cluster_count as usize];
        for (cell, info) in self.cells.iter().enumerate() {
            members[info.cluster as usize].push(cell as u32);
        }
        members
    }
}
