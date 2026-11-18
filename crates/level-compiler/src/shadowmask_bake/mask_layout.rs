// Per-cell-block mask layout: only the charts of mask-owning faces, each copied
// in its lightmap orientation, with plane B a sub-rect at the mask block's origin.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;
use postretro_level_format::shadowmask_atlas::{
    FaceOwners, MaskExtents, MaskOffset, NO_FACE_OWNERS, SHADOWMASK_OWNER_CHANNELS,
    SHADOWMASK_OWNER_EMPTY,
};

use crate::lightmap_bake::{CellBlock, PackHeuristic, PackRules, pack_cell_block_reserving};
use crate::lightmap_layer::SharedAtlas;

/// BC5 block edge: mask extents and the plane-B sub-rect are whole BC blocks,
/// so plane B trims out of the encoded mask block losslessly.
const MASK_ALIGNMENT: u32 = 4;

/// Where one cell block's owned charts sit in its mask block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaskBlockLayout {
    /// The mask block's extent; `(0, 0)` for a block with no owned face.
    pub extent: (u32, u32),
    /// Plane B's sub-rect at the mask block's origin: every chart whose face
    /// owns channel 2 or 3 lies inside it. `(0, 0)` when no face does.
    pub plane_b: (u32, u32),
    /// Each owned face, ascending, and its padded chart rect's top-left in the
    /// mask block.
    pub charts: Vec<(u32, (u32, u32))>,
    /// Owned charts that would not pack smaller than the block stay where the
    /// lightmap placed them: the mask block is the lightmap block.
    pub identity: bool,
}

impl MaskBlockLayout {
    /// The extents id 42 records for this block.
    pub(crate) fn extents(&self) -> MaskExtents {
        let edge = |value: u32| u16::try_from(value).expect("a mask extent fits a pool layer");
        MaskExtents {
            mask: (edge(self.extent.0), edge(self.extent.1)),
            plane_b: (edge(self.plane_b.0), edge(self.plane_b.1)),
        }
    }
}

/// Whether `owners` names a light on channel 2 or 3, so its chart samples
/// plane B. Owners are ascending with empties trailing, so position 2 holds
/// an owner exactly when channel 2 or 3 does.
pub(crate) fn owns_plane_b(owners: &FaceOwners) -> bool {
    owners[2] != SHADOWMASK_OWNER_EMPTY
}

/// A chart's padded extent as it sits in the lightmap block, turned when the
/// chart is stored transposed: the rect the mask copies whole, padding ring
/// included, so a bilinear tap at the chart's edge reads the same padding it
/// read in the lightmap layout.
fn placed_extent(face: usize, shared: &SharedAtlas<'_>) -> (u32, u32) {
    let chart = &shared.charts[face];
    shared.placements[face].placed_extent(chart.width_texels, chart.height_texels)
}

/// Lay out `block`'s mask block from its `faces` (ascending) and their final
/// owners. Plane-B charts pack first, tight, into the sub-rect at the origin;
/// channel-0/1 charts pack around it. The layout depends only on the owned
/// charts' extents in face order and their plane-B membership, never on worker
/// count or arrival order.
///
/// A block whose owned charts would not pack within a pool layer, or into
/// less area than the lightmap block, keeps its lightmap layout (`identity`),
/// so a mask block is never larger than its lightmap block.
pub(crate) fn layout_mask_block(
    block: &CellBlock,
    faces: &[u32],
    owners: &[FaceOwners],
    shared: &SharedAtlas<'_>,
) -> MaskBlockLayout {
    let owned: Vec<u32> = faces
        .iter()
        .copied()
        .filter(|&face| owners[face as usize] != NO_FACE_OWNERS)
        .collect();
    if owned.is_empty() {
        return MaskBlockLayout {
            extent: (0, 0),
            plane_b: (0, 0),
            charts: Vec::new(),
            identity: false,
        };
    }
    let (in_b, in_a): (Vec<u32>, Vec<u32>) = owned
        .iter()
        .partition(|&&face| owns_plane_b(&owners[face as usize]));
    let packed = pack_owned(&in_b, &in_a, shared);
    let block_area = u64::from(block.width) * u64::from(block.height);
    match packed {
        Some(layout) if u64::from(layout.extent.0) * u64::from(layout.extent.1) < block_area => {
            layout
        }
        _ => identity_layout(block, &owned, !in_b.is_empty(), shared),
    }
}

/// Mask slots never turn: a mask chart keeps its lightmap orientation.
fn origins(slots: &[crate::lightmap_bake::BlockSlot]) -> Vec<(u32, u32)> {
    slots
        .iter()
        .map(|slot| {
            debug_assert!(!slot.transposed, "mask charts pack upright");
            (slot.x, slot.y)
        })
        .collect()
}

/// Plane-B charts tight at the origin, then channel-0/1 charts around them.
/// `None` when either pass does not fit a pool layer.
fn pack_owned(in_b: &[u32], in_a: &[u32], shared: &SharedAtlas<'_>) -> Option<MaskBlockLayout> {
    let edge = LIGHTMAP_POOL_LAYER_EDGE;
    let sizes = |faces: &[u32]| -> Vec<(u32, u32)> {
        faces
            .iter()
            .map(|&face| placed_extent(face as usize, shared))
            .collect()
    };
    let rules = PackRules::mask(MASK_ALIGNMENT);
    let pack = |faces: &[u32], reserved| {
        pack_cell_block_reserving(&sizes(faces), reserved, rules, edge, PackHeuristic::DEFAULT)
    };
    let mut charts = Vec::with_capacity(in_b.len() + in_a.len());
    let plane_b = match pack(in_b, None) {
        Some(block) => {
            if !block.fits(edge) {
                return None;
            }
            charts.extend(in_b.iter().copied().zip(origins(&block.placements)));
            (block.width, block.height)
        }
        None => (0, 0),
    };
    let reserved = (plane_b != (0, 0)).then_some(plane_b);
    let extent = if in_a.is_empty() {
        plane_b
    } else {
        let block = pack(in_a, reserved)?;
        if !block.fits(edge) {
            return None;
        }
        charts.extend(in_a.iter().copied().zip(origins(&block.placements)));
        (block.width, block.height)
    };
    charts.sort_unstable_by_key(|&(face, _)| face);
    Some(MaskBlockLayout {
        extent,
        plane_b,
        charts,
        identity: false,
    })
}

/// Every owned chart where the lightmap placed it: the mask block is the
/// lightmap block, plane B the whole block when any face owns channel 2 or 3.
fn identity_layout(
    block: &CellBlock,
    owned: &[u32],
    any_plane_b: bool,
    shared: &SharedAtlas<'_>,
) -> MaskBlockLayout {
    let extent = (block.width, block.height);
    let charts = owned
        .iter()
        .map(|&face| {
            let placement = &shared.placements[face as usize];
            (face, (placement.x - block.x, placement.y - block.y))
        })
        .collect();
    MaskBlockLayout {
        extent,
        plane_b: if any_plane_b { extent } else { (0, 0) },
        charts,
        identity: true,
    }
}

/// `face`'s offset from its lightmap texel to its mask texel: its mask
/// origin less its block-local lightmap origin.
pub(crate) fn face_offset(
    block: &CellBlock,
    face: u32,
    mask_origin: (u32, u32),
    shared: &SharedAtlas<'_>,
) -> MaskOffset {
    let placement = &shared.placements[face as usize];
    let delta = |mask: u32, placed: u32, origin: u32| {
        let delta = i64::from(mask) - i64::from(placed - origin);
        i16::try_from(delta).expect("a mask offset stays within a pool layer")
    };
    [
        delta(mask_origin.0, placement.x, block.x),
        delta(mask_origin.1, placement.y, block.y),
    ]
}

/// The mask block's raw RGBA image: zero everywhere but each owned chart's
/// padded rect, copied row by row from `source`. `source` holds 4 mask bytes
/// per texel, `source_width` texels per row, its texel `(0, 0)` at bake-layer
/// `source_origin`. A pure translation: every value is a byte-copy.
pub(crate) fn copy_owned_charts(
    source: &[u8],
    source_width: u32,
    source_origin: [u32; 2],
    layout: &MaskBlockLayout,
    shared: &SharedAtlas<'_>,
) -> Vec<u8> {
    const TEXEL: usize = SHADOWMASK_OWNER_CHANNELS;
    let (width, height) = layout.extent;
    let mut image = vec![0u8; width as usize * height as usize * TEXEL];
    for &(face, (mask_x, mask_y)) in &layout.charts {
        let placement = &shared.placements[face as usize];
        let (chart_w, chart_h) = placed_extent(face as usize, shared);
        let row_bytes = chart_w as usize * TEXEL;
        let (src_x, src_y) = (
            (placement.x - source_origin[0]) as usize,
            (placement.y - source_origin[1]) as usize,
        );
        for row in 0..chart_h as usize {
            let from = ((src_y + row) * source_width as usize + src_x) * TEXEL;
            let to = ((mask_y as usize + row) * width as usize + mask_x as usize) * TEXEL;
            image[to..to + row_bytes].copy_from_slice(&source[from..from + row_bytes]);
        }
    }
    image
}

/// Always-on checks of a finished layout: every plane-B chart inside plane B,
/// plane B inside the mask block, every chart inside the block, and no two
/// charts overlapping. A violation would let a face sample another chart's
/// mask, so it fails the build in every profile.
pub(crate) fn assert_layout_sound(
    layout: &MaskBlockLayout,
    owners: &[FaceOwners],
    shared: &SharedAtlas<'_>,
) {
    let (width, height) = layout.extent;
    assert!(
        layout.plane_b.0 <= width && layout.plane_b.1 <= height,
        "plane B {:?} leaves the {width}x{height} mask block",
        layout.plane_b
    );
    let rects: Vec<(u32, u32, u32, u32)> = layout
        .charts
        .iter()
        .map(|&(face, (x, y))| {
            let (w, h) = placed_extent(face as usize, shared);
            (x, y, x + w, y + h)
        })
        .collect();
    for (&(face, _), &(_, _, x1, y1)) in layout.charts.iter().zip(&rects) {
        assert!(
            x1 <= width && y1 <= height,
            "face {face}'s mask chart leaves the {width}x{height} mask block"
        );
        if owns_plane_b(&owners[face as usize]) {
            assert!(
                x1 <= layout.plane_b.0 && y1 <= layout.plane_b.1,
                "face {face} owns channel 2 or 3 but its mask chart leaves plane B {:?}",
                layout.plane_b
            );
        }
    }
    if layout.identity {
        // The lightmap layout's own charts, which the lightmap packer keeps
        // disjoint.
        return;
    }
    for (i, a) in rects.iter().enumerate() {
        for b in &rects[i + 1..] {
            let apart = a.2 <= b.0 || b.2 <= a.0 || a.3 <= b.1 || b.3 <= a.1;
            assert!(apart, "two mask charts overlap: {a:?} and {b:?}");
        }
    }
}
