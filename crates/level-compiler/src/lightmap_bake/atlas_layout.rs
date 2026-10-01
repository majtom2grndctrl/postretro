// Lightmap atlas preparation: vertex splitting, the oversize-face cut, packing, and lightmap UV write-back.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::collections::HashSet;
use std::ops::Range;

use glam::Vec3;

use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;

use super::block_layout::{BlockLayout, BlockOrdering, pack_cell_blocks_within};
use super::charts::{Chart, plan_charts};
use super::face_cut::{FaceCut, apply_face_cuts, plan_face_cuts, planned_chart_area};
use super::{CompositedAtlas, LightmapBakeError, MAX_ATLAS_LAYERS};
use crate::bake_control::BakeControl;
use crate::chart_raster::{CHART_PADDING_TEXELS, ChartPlacement};
use crate::geometry::GeometryResult;
use crate::light_namespaces::StaticBakedLights;
use crate::map_data::MapLightmapScaleRegion;

/// Cheap pre-bake setup: chart planning, per-cell block packing, bake-layer
/// placement, and writing block ids and block-local lightmap UVs back into
/// geometry. Returned by [`prepare_atlas`] and consumed by both the warm
/// per-light composite path and the cold per-layer bake — called once before
/// either branch, so the layout is shared.
#[derive(Debug)]
pub struct PreparedAtlas {
    pub charts: Vec<Chart>,
    /// Bake-layer placement of each chart.
    pub placements: Vec<ChartPlacement>,
    /// Edge of every internal bake layer.
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Internal bake layers the blocks were packed into.
    pub layer_count: u32,
    /// Cell blocks in block-id order and each chart's block.
    pub layout: BlockLayout,
}

/// [`prepare_atlas_ordered`] with blocks in cell-id order at the default
/// direction scale, for callers without a cluster partition.
pub fn prepare_atlas(
    geom: &mut GeometryResult,
    static_lights: &StaticBakedLights<'_>,
    texel_density: f32,
    scale_regions: &[MapLightmapScaleRegion],
) -> Result<PreparedAtlas, LightmapBakeError> {
    prepare_atlas_ordered(
        geom,
        static_lights,
        texel_density,
        scale_regions,
        BlockOrdering::by_cell_id(super::DIRECTION_TEXEL_SCALE),
        &BakeControl::unrestricted(),
    )
}

/// Prepare charts and cell blocks, and assign each vertex its block id and
/// block-local lightmap UV: [`plan_cut_charts`], then [`pack_cut_charts`]
/// at the runtime pool edge. Does NOT run the per-texel ray casting.
///
/// The compiler pipeline runs the two phases itself, rebuilding the
/// face-identity set and the cell partition between them; this entry point
/// keeps the cut geometry and drops the face remap, so a BVH or face index
/// built before the call is stale once a face was cut.
pub fn prepare_atlas_ordered(
    geom: &mut GeometryResult,
    static_lights: &StaticBakedLights<'_>,
    texel_density: f32,
    scale_regions: &[MapLightmapScaleRegion],
    ordering: BlockOrdering<'_>,
    control: &BakeControl,
) -> Result<PreparedAtlas, LightmapBakeError> {
    prepare_atlas_within(
        geom,
        static_lights,
        texel_density,
        scale_regions,
        ordering,
        LIGHTMAP_POOL_LAYER_EDGE,
        control,
    )
}

/// [`prepare_atlas_ordered`] against `pool_edge` rather than the runtime's,
/// so tests can cut faces and bake multi-block cells from small charts.
pub(crate) fn prepare_atlas_within(
    geom: &mut GeometryResult,
    static_lights: &StaticBakedLights<'_>,
    texel_density: f32,
    scale_regions: &[MapLightmapScaleRegion],
    ordering: BlockOrdering<'_>,
    pool_edge: u32,
    control: &BakeControl,
) -> Result<PreparedAtlas, LightmapBakeError> {
    let cut = plan_cut_charts(geom, static_lights, texel_density, scale_regions, pool_edge)?;
    pack_cut_charts(
        geom,
        static_lights,
        cut.charts,
        ordering,
        pool_edge,
        control,
    )
}

/// Atlas preparation's first phase: the charts after any oversize-face cut.
pub struct CutCharts {
    /// One chart per face of the (possibly cut) geometry.
    pub charts: Vec<Chart>,
    /// `Some` when a face was cut: the new face range of every old face.
    pub face_remap: Option<Vec<Range<usize>>>,
    /// `Some` when a face was cut: the geometry as it stood before the cut,
    /// faces sharing no vertices and no lightmap attributes written.
    pub pre_cut: Option<GeometryResult>,
}

/// Plan charts on the uncut geometry and cut every face whose chart exceeds
/// `pool_edge` into sub-faces whose charts window its grid. A map without
/// static light cuts nothing: its charts never ship in blocks. Before any
/// geometry is cut, an atlas whose cut charts could not fit the bake-layer
/// cap fails by name.
pub fn plan_cut_charts(
    geom: &mut GeometryResult,
    static_lights: &StaticBakedLights<'_>,
    texel_density: f32,
    scale_regions: &[MapLightmapScaleRegion],
    pool_edge: u32,
) -> Result<CutCharts, LightmapBakeError> {
    let uncut = |charts| CutCharts {
        charts,
        face_remap: None,
        pre_cut: None,
    };
    if geom.geometry.vertices.is_empty() || geom.geometry.faces.is_empty() {
        return Ok(uncut(Vec::new()));
    }
    if static_lights.is_empty() {
        return Ok(uncut(plan_charts(geom, texel_density, scale_regions)?));
    }

    // Ensure no vertex index is shared across faces — each face must own its own lightmap UV slot.
    split_shared_vertices(geom);
    let mut charts = plan_charts(geom, texel_density, scale_regions)?;
    let cuts = plan_face_cuts(&charts, pool_edge);
    check_planned_area(&charts, &cuts, pool_edge)?;
    if cuts.is_empty() {
        return Ok(uncut(charts));
    }
    for cut in &cuts {
        let chart = &charts[cut.face];
        log::info!(
            "[Compiler] cutting face {} (cell {}, at {:.2?} facing {:.2?}): {}x{} texels into {}x{} sub-charts",
            cut.face,
            chart.leaf_index,
            chart.origin.to_array(),
            chart.normal.to_array(),
            chart.width_texels,
            chart.height_texels,
            cut.u_lines.len() - 1,
            cut.v_lines.len() - 1,
        );
    }
    let pre_cut = geom.clone();
    let cut = apply_face_cuts(geom, &mut charts, &cuts);
    Ok(CutCharts {
        charts,
        face_remap: Some(cut.face_remap),
        pre_cut: Some(pre_cut),
    })
}

/// Fail by name, before cutting, when the cut charts' area alone needs more
/// bake layers than the compiler allows.
fn check_planned_area(
    charts: &[Chart],
    cuts: &[FaceCut],
    pool_edge: u32,
) -> Result<(), LightmapBakeError> {
    let layer_area = u64::from(pool_edge) * u64::from(pool_edge);
    let layers = planned_chart_area(charts, cuts).div_ceil(layer_area);
    if layers > u64::from(MAX_ATLAS_LAYERS) {
        return Err(LightmapBakeError::LayerOverflow {
            layer_count: u32::try_from(layers).unwrap_or(u32::MAX),
            max: MAX_ATLAS_LAYERS,
        });
    }
    Ok(())
}

/// Atlas preparation's second phase: pack `charts` into cell blocks in
/// `ordering` and write every vertex's block id and block-local lightmap UV.
/// With static lights, every chart must fit `pool_edge`, which
/// [`plan_cut_charts`] guarantees.
///
/// Without static lights the section has no blocks, so vertices keep block
/// 0, but placements are still returned for the animated-light passes; when a
/// chart, the block count or the layer count exceeds the runtime limits,
/// placements come back empty instead of failing the build. Empty geometry
/// returns an empty layout without mutating anything.
pub fn pack_cut_charts(
    geom: &mut GeometryResult,
    static_lights: &StaticBakedLights<'_>,
    charts: Vec<Chart>,
    ordering: BlockOrdering<'_>,
    pool_edge: u32,
    control: &BakeControl,
) -> Result<PreparedAtlas, LightmapBakeError> {
    let unplaced = |charts| PreparedAtlas {
        charts,
        placements: Vec::new(),
        atlas_width: 1,
        atlas_height: 1,
        layer_count: 1,
        layout: empty_layout(ordering),
    };
    if geom.geometry.vertices.is_empty() || geom.geometry.faces.is_empty() {
        return Ok(unplaced(charts));
    }
    let fits = |chart: &Chart| chart.width_texels <= pool_edge && chart.height_texels <= pool_edge;
    if static_lights.is_empty() {
        // The animated-light-chunks builder needs placements even without
        // static lights; a layout the runtime could not hold just plans no
        // chunks. The empty layout keeps the direction scale, which the
        // placeholder id-22 header carries.
        if !charts.iter().all(fits) {
            return Ok(unplaced(charts));
        }
        return Ok(
            match pack_cell_blocks_within(&charts, ordering, pool_edge, control) {
                Ok(pack) => PreparedAtlas {
                    charts,
                    placements: pack.placements,
                    atlas_width: pack.layer_dim,
                    atlas_height: pack.layer_dim,
                    layer_count: pack.layer_count,
                    layout: pack.layout,
                },
                Err(_) => unplaced(charts),
            },
        );
    }

    debug_assert!(charts.iter().all(fits), "the cut bounds every chart");
    let pack = pack_cell_blocks_within(&charts, ordering, pool_edge, control)?;
    if !pack.placements.is_empty() {
        assign_lightmap_uvs(geom, &charts, &pack.placements, &pack.layout);
    }
    Ok(PreparedAtlas {
        charts,
        placements: pack.placements,
        atlas_width: pack.layer_dim,
        atlas_height: pack.layer_dim,
        layer_count: pack.layer_count,
        layout: pack.layout,
    })
}

/// A layout with no blocks at the ordering's normalized direction scale.
fn empty_layout(ordering: BlockOrdering<'_>) -> BlockLayout {
    BlockLayout {
        direction_texel_scale: super::encode::normalized_direction_texel_scale(
            ordering.direction_texel_scale,
        ),
        ..BlockLayout::default()
    }
}

fn split_shared_vertices(geom: &mut GeometryResult) {
    let face_count = geom.face_index_ranges.len();
    if face_count <= 1 {
        return;
    }

    // First-seen face owns the original vertex; subsequent faces get duplicates.
    let mut owner: Vec<u32> = vec![u32::MAX; geom.geometry.vertices.len()];
    let ranges = geom.face_index_ranges.clone();
    let mut face_remap: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();

    for (face_idx, range) in ranges.iter().enumerate() {
        let face_idx_u32 = face_idx as u32;
        let start = range.index_offset as usize;
        let end = start + range.index_count as usize;
        face_remap.clear();

        for i in start..end {
            let vi = geom.geometry.indices[i];
            let cur = owner[vi as usize];
            if cur == u32::MAX {
                owner[vi as usize] = face_idx_u32;
            } else if cur != face_idx_u32 {
                let new_index = if let Some(&dup) = face_remap.get(&vi) {
                    dup
                } else {
                    let dup_vertex = geom.geometry.vertices[vi as usize].clone();
                    let new_index = geom.geometry.vertices.len() as u32;
                    geom.geometry.vertices.push(dup_vertex);
                    owner.push(face_idx_u32);
                    face_remap.insert(vi, new_index);
                    new_index
                };
                geom.geometry.indices[i] = new_index;
            }
        }
    }
}

/// Continuous texel position of `world_p` inside `chart` placed with its
/// padded top-left at `(origin_x, origin_y)`: the interior starts one padding
/// in, and the chart's UV extent spans the interior. Shared by UV assignment
/// (block-local origin) and the rebase proof (bake-layer origin).
pub(crate) fn chart_texel_position(
    chart: &Chart,
    origin_x: u32,
    origin_y: u32,
    world_p: Vec3,
) -> (f32, f32) {
    let padding = CHART_PADDING_TEXELS as f32;
    // A sub-chart maps through its parent's grid, then shifts by its window,
    // so a vertex on a cut lands on the same grid texel from both sides.
    let (uv_min, uv_extent, interior, window) = match chart.window {
        None => (
            chart.uv_min,
            chart.uv_extent,
            [
                ((chart.width_texels as f32) - 2.0 * padding).max(1.0),
                ((chart.height_texels as f32) - 2.0 * padding).max(1.0),
            ],
            [0.0, 0.0],
        ),
        Some(window) => (
            window.grid_uv_min,
            window.grid_uv_extent,
            [
                window.grid_interior[0] as f32,
                window.grid_interior[1] as f32,
            ],
            [window.origin[0] as f32, window.origin[1] as f32],
        ),
    };
    let scale_u = interior[0] / uv_extent[0].max(1.0e-6);
    let scale_v = interior[1] / uv_extent[1].max(1.0e-6);
    let rel = world_p - chart.origin;
    let local_u = rel.dot(chart.u_axis) - uv_min[0];
    let local_v = rel.dot(chart.v_axis) - uv_min[1];
    (
        (origin_x as f32 + padding - window[0]) + local_u * scale_u,
        (origin_y as f32 + padding - window[1]) + local_v * scale_v,
    )
}

/// Quantize a texel position over `extent` texels to the vertex's unorm16 UV.
pub(crate) fn quantize_lightmap_uv(texel: f32, extent: u32) -> u16 {
    let uv = (texel / extent as f32).clamp(0.0, 1.0);
    (uv * 65535.0 + 0.5) as u16
}

/// Write each face vertex's block (`id + 1`) and its UV over that block's
/// extent, computed from the chart's block-local placement. Vertices no face
/// references keep block 0 and UV 0.
pub(super) fn assign_lightmap_uvs(
    geom: &mut GeometryResult,
    charts: &[Chart],
    placements: &[ChartPlacement],
    layout: &BlockLayout,
) {
    for vert in &mut geom.geometry.vertices {
        vert.lightmap_uv = [0, 0];
        vert.lightmap_block = 0;
    }
    let ranges = geom.face_index_ranges.clone();

    for (face_index, chart) in charts.iter().enumerate() {
        let block_id = layout.chart_blocks[face_index];
        let block = &layout.blocks[block_id as usize];
        let (local_x, local_y) = layout.local_placement(face_index, &placements[face_index]);
        // `check_block_limits` bounds the block count to `MAX_LIGHTMAP_BLOCKS`,
        // so `id + 1` fits the vertex's `u16`.
        let vertex_block = u16::try_from(block_id + 1).expect("block count limit keeps ids in u16");
        let range = ranges[face_index];
        let start = range.index_offset as usize;
        let end = start + range.index_count as usize;

        let geom_section = &mut geom.geometry;
        let mut assigned: HashSet<usize> = HashSet::new();
        let mut tri = start;
        while tri + 3 <= end {
            for j in 0..3 {
                let vi = geom_section.indices[tri + j] as usize;
                if !assigned.insert(vi) {
                    continue;
                }
                let vert = &mut geom_section.vertices[vi];
                let (tx, ty) =
                    chart_texel_position(chart, local_x, local_y, Vec3::from(vert.position));
                vert.lightmap_uv = [
                    quantize_lightmap_uv(tx, block.width),
                    quantize_lightmap_uv(ty, block.height),
                ];
                vert.lightmap_block = vertex_block;
            }
            tri += 3;
        }
    }
}

/// Copy one chart-local bake buffer into its disjoint rectangle in the
/// layer-major atlas. `pack_layers` establishes the non-overlap contract that
/// makes the completion order of these copies irrelevant to output bytes.
pub(super) fn scatter_chart_into_atlas(
    chart_atlas: &CompositedAtlas,
    placement: &ChartPlacement,
    atlas: &mut CompositedAtlas,
) {
    debug_assert_eq!(chart_atlas.layer_count, 1);
    debug_assert!(chart_atlas.atlas_width + placement.x <= atlas.atlas_width);
    debug_assert!(chart_atlas.atlas_height + placement.y <= atlas.atlas_height);
    debug_assert!(placement.layer < atlas.layer_count);

    let chart_w = chart_atlas.atlas_width as usize;
    let chart_h = chart_atlas.atlas_height as usize;
    let atlas_w = atlas.atlas_width as usize;
    let layer_offset = placement.layer as usize * atlas_w * atlas.atlas_height as usize;

    for row in 0..chart_h {
        let source_start = row * chart_w;
        let source_end = source_start + chart_w;
        let dest_start =
            layer_offset + (placement.y as usize + row) * atlas_w + placement.x as usize;
        let dest_end = dest_start + chart_w;
        atlas.irradiance[dest_start * 4..dest_end * 4]
            .copy_from_slice(&chart_atlas.irradiance[source_start * 4..source_end * 4]);
        atlas.direction[dest_start..dest_end]
            .copy_from_slice(&chart_atlas.direction[source_start..source_end]);
        atlas.coverage[dest_start..dest_end]
            .copy_from_slice(&chart_atlas.coverage[source_start..source_end]);
    }
}
