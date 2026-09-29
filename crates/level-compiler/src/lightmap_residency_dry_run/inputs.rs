//! PRL → `DryRunInput`: parses only the sections the dry run needs and
//! recovers each face chart's padded atlas rectangle from stored vertex UVs.

use std::collections::VecDeque;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::{Context, bail, ensure};
use postretro_level_format::bvh::{BvhLeaf, BvhSection};
use postretro_level_format::cell_visibility::CellVisibilitySection;
use postretro_level_format::cells::CellsSection;
use postretro_level_format::cluster_directory::{
    CLUSTER_HINT_FLAG_PINNED, ClusterDirectorySection,
};
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::shadowmask_atlas::{
    SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE, ShadowmaskAtlasSection,
};
use postretro_level_format::{SectionId, read_container, read_section_data};

use super::portal_distance::{HubCell, HubPortal, PortalGraphInput};
use super::{
    AtlasFormats, CellInfo, ChartRect, DryRunInput, FaceSlot, ReconstructionStats,
    ShadowmaskFormat, ShadowmaskState,
};
use crate::chart_raster::CHART_PADDING_TEXELS;

const UV_QUANT_MAX: u16 = u16::MAX;

pub(crate) fn read_dry_run_input(path: &Path) -> anyhow::Result<DryRunInput> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let meta = read_container(&mut reader).context("read PRL container")?;
    let mut optional = |id: SectionId| -> anyhow::Result<Option<Vec<u8>>> {
        read_section_data(&mut reader, &meta, id as u32)
            .with_context(|| format!("read section {id:?}"))
    };

    // Each payload section is parsed, reduced to its header facts, and
    // dropped before the next is read, bounding peak memory to one section.
    let lightmap_bytes = optional(SectionId::Lightmap)?.context("missing Lightmap (id 22)")?;
    let lightmap = LightmapSection::from_bytes(&lightmap_bytes).context("parse Lightmap")?;
    drop(lightmap_bytes);
    let shadowmask = match optional(SectionId::ShadowmaskAtlas)? {
        Some(bytes) => {
            let section =
                ShadowmaskAtlasSection::from_bytes(&bytes).context("parse ShadowmaskAtlas")?;
            ensure!(
                section.format == SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE,
                "unexpected ShadowmaskAtlas format tag {:#x}",
                section.format
            );
            ShadowmaskState::Stored(ShadowmaskFormat {
                width: section.width,
                height: section.height,
                layer_count: section.layer_count,
                payload_bytes: section.data.len() as u64,
            })
        }
        None => {
            // The bake emits id 42 for every usable id-40 selection unless
            // the doubled layer width exceeds the texture limit.
            let selected = match optional(SectionId::EntityShadowLights)? {
                Some(bytes) => !EntityShadowLightsSection::from_bytes(&bytes)
                    .map_err(|error| anyhow::anyhow!("parse EntityShadowLights: {error:?}"))?
                    .light_indices
                    .is_empty(),
                None => false,
            };
            if selected {
                ShadowmaskState::OmittedForWidth
            } else {
                ShadowmaskState::Absent
            }
        }
    };
    let formats = AtlasFormats {
        layer_count: lightmap.layer_count,
        irr_width: lightmap.irr_width,
        irr_height: lightmap.irr_height,
        irr_format: lightmap.irradiance_format,
        irr_payload_bytes: lightmap.irradiance.len() as u64,
        dir_width: lightmap.dir_width,
        dir_height: lightmap.dir_height,
        dir_format: lightmap.direction_format,
        dir_payload_bytes: lightmap.direction.len() as u64,
        shadowmask,
    };
    drop(lightmap);
    if let Some(sm) = formats.stored_shadowmask() {
        ensure!(
            sm.width == formats.irr_width
                && sm.height == formats.irr_height
                && sm.layer_count == formats.layer_count,
            "ShadowmaskAtlas dimensions {}x{}x{} disagree with Lightmap {}x{}x{}",
            sm.width,
            sm.height,
            sm.layer_count,
            formats.irr_width,
            formats.irr_height,
            formats.layer_count
        );
    }

    let cells = CellsSection::from_bytes(&optional(SectionId::Cells)?.context("missing Cells")?)
        .context("parse Cells")?;
    let cell_count = u32::try_from(cells.cells.len()).context("cell count exceeds u32")?;
    let geometry =
        GeometrySection::from_bytes(&optional(SectionId::Geometry)?.context("missing Geometry")?)
            .context("parse Geometry")?;
    let bvh = BvhSection::from_bytes(&optional(SectionId::Bvh)?.context("missing Bvh")?)
        .context("parse Bvh")?;
    let directory = ClusterDirectorySection::from_bytes(
        &optional(SectionId::ClusterDirectory)?.context("missing ClusterDirectory (id 49)")?,
    )
    .map_err(|error| anyhow::anyhow!("parse ClusterDirectory: {error:?}"))?;
    let visibility = match optional(SectionId::CellVisibility)? {
        Some(bytes) => Some(
            CellVisibilitySection::from_bytes(&bytes, cell_count)
                .context("parse CellVisibility")?,
        ),
        None => None,
    };
    let portals =
        PortalsSection::from_bytes(&optional(SectionId::Portals)?.context("missing Portals")?)
            .context("parse Portals")?;

    let (charts, faces, reconstruction) =
        reconstruct_charts(&geometry, &bvh, &formats, cell_count)?;
    drop(geometry);

    let mut cell_cluster = vec![u32::MAX; cells.cells.len()];
    for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
        let start = cluster.member_start as usize;
        let end = start + cluster.member_count as usize;
        for &cell in &directory.members[start..end] {
            let slot = cell_cluster
                .get_mut(cell as usize)
                .context("cluster member outside the cell range")?;
            ensure!(*slot == u32::MAX, "cell {cell} belongs to two clusters");
            *slot = cluster_id as u32;
        }
    }
    if let Some(cell) = cell_cluster.iter().position(|&c| c == u32::MAX) {
        bail!("cell {cell} belongs to no cluster");
    }
    let cell_infos = cells
        .cells
        .iter()
        .zip(&cell_cluster)
        .map(|(record, &cluster)| CellInfo {
            center: [
                0.5 * (record.bounds_min[0] + record.bounds_max[0]),
                0.5 * (record.bounds_min[1] + record.bounds_max[1]),
                0.5 * (record.bounds_min[2] + record.bounds_max[2]),
            ],
            cluster,
            camera_candidate: !record.is_solid() && !record.is_exterior(),
        })
        .collect();

    let mut pinned_clusters: Vec<u32> = directory
        .cluster_hints
        .iter()
        .filter(|hint| hint.flags & CLUSTER_HINT_FLAG_PINNED != 0)
        .map(|hint| hint.cluster_id)
        .collect();
    pinned_clusters.sort_unstable();
    pinned_clusters.dedup();

    let portal_graph = PortalGraphInput {
        cells: cells
            .cells
            .iter()
            .map(|record| HubCell {
                bounds_min: record.bounds_min,
                bounds_max: record.bounds_max,
                solid: record.is_solid(),
            })
            .collect(),
        portals: portals
            .portals
            .iter()
            .map(|record| {
                let start = record.vertex_start as usize;
                let end = start + record.vertex_count as usize;
                HubPortal {
                    front: record.front_leaf,
                    back: record.back_leaf,
                    vertices: portals.vertices[start..end].to_vec(),
                }
            })
            .collect(),
    };

    let cell_visibility_present = visibility.is_some();
    let (component_ids, coupled_pairs) = match visibility {
        Some(section) => (section.component_ids, section.coupled_pairs),
        // Missing id 46 is the conservative all-perceivable fallback.
        None => (vec![0; cells.cells.len()], Vec::new()),
    };

    Ok(DryRunInput {
        formats,
        charts,
        faces,
        cells: cell_infos,
        cluster_count: directory.clusters.len() as u32,
        pinned_clusters,
        cell_visibility_present,
        component_ids,
        coupled_pairs,
        portal_graph: Some(portal_graph),
        reconstruction,
    })
}

/// Recover one padded chart rectangle per face, in the bake's chart order.
///
/// The bake plans one chart per Geometry face record, in record order. A BVH
/// leaf is one face's index range; faces with no indices have no leaf. Each
/// face record takes the next leaf of its cell in index-offset order, or
/// becomes a 1×1 placeholder when its cell has none left. Placement order
/// within a cell only breaks ties between equal-area charts, and every
/// placeholder is 1×1 while a real chart is at least `2 × padding + 1` wide,
/// so the recovered order packs exactly as the bake's did.
///
/// The bake writes a vertex UV as `(placement + padding + local · scale) /
/// atlas`, and the face's extreme vertices land exactly on the interior's
/// integer edges, so rounding the quantized UV bounds (1/8 texel worst case
/// at 8192²) recovers the placement and padded size exactly.
pub(super) fn reconstruct_charts(
    geometry: &GeometrySection,
    bvh: &BvhSection,
    formats: &AtlasFormats,
    cell_count: u32,
) -> anyhow::Result<(Vec<ChartRect>, Vec<FaceSlot>, ReconstructionStats)> {
    let mut leaves_by_cell: Vec<VecDeque<&BvhLeaf>> = vec![VecDeque::new(); cell_count as usize];
    let mut sorted: Vec<&BvhLeaf> = bvh.leaves.iter().collect();
    sorted.sort_by_key(|leaf| (leaf.index_offset, leaf.index_count));
    for leaf in sorted {
        leaves_by_cell
            .get_mut(leaf.cell_id as usize)
            .with_context(|| format!("BVH leaf cell {} outside the cell range", leaf.cell_id))?
            .push_back(leaf);
    }

    let mut stats = ReconstructionStats {
        faces: geometry.faces.len(),
        ..Default::default()
    };
    let mut charts = Vec::with_capacity(bvh.leaves.len());
    let mut faces = Vec::with_capacity(geometry.faces.len());
    for record in &geometry.faces {
        let cell = record.leaf_index;
        let leaf = leaves_by_cell
            .get_mut(cell as usize)
            .with_context(|| format!("face cell {cell} outside the cell range"))?
            .pop_front();
        let slot = match leaf {
            None => {
                stats.zero_index_faces += 1;
                FaceSlot::Placeholder { cell }
            }
            Some(leaf) => match recover_rect(geometry, leaf, formats, &mut stats)? {
                Some(rect) => {
                    charts.push(rect);
                    FaceSlot::Chart(charts.len() - 1)
                }
                None => FaceSlot::Placeholder { cell },
            },
        };
        faces.push(slot);
    }
    stats.unmatched_bvh_faces = leaves_by_cell.iter().map(VecDeque::len).sum();
    stats.charts = charts.len();
    Ok((charts, faces, stats))
}

/// One face's padded rectangle, or `None` (counted in `stats`) when its UVs
/// describe no placed chart.
fn recover_rect(
    geometry: &GeometrySection,
    face: &BvhLeaf,
    formats: &AtlasFormats,
    stats: &mut ReconstructionStats,
) -> anyhow::Result<Option<ChartRect>> {
    if face.index_count < 3 {
        stats.short_index_faces += 1;
        return Ok(None);
    }
    let start = face.index_offset as usize;
    let end = start + face.index_count as usize;
    let indices = geometry
        .indices
        .get(start..end)
        .context("BVH leaf index range outside the index buffer")?;
    let mut u = (u16::MAX, u16::MIN);
    let mut v = (u16::MAX, u16::MIN);
    let mut layer = None;
    let mut mixed_layer = false;
    let mut edge_clamped = false;
    for &index in indices {
        let vertex = geometry
            .vertices
            .get(index as usize)
            .context("index outside the vertex buffer")?;
        let [vu, vv] = vertex.lightmap_uv;
        u = (u.0.min(vu), u.1.max(vu));
        v = (v.0.min(vv), v.1.max(vv));
        edge_clamped |= vu == 0 || vv == 0 || vu == UV_QUANT_MAX || vv == UV_QUANT_MAX;
        match layer {
            None => layer = Some(vertex.lightmap_layer),
            Some(existing) => mixed_layer |= existing != vertex.lightmap_layer,
        }
    }
    if u.1 == 0 && v.1 == 0 {
        stats.uncharted_faces += 1;
        return Ok(None);
    }
    if edge_clamped {
        stats.edge_clamped_faces += 1;
        return Ok(None);
    }
    if mixed_layer {
        stats.mixed_layer_faces += 1;
        return Ok(None);
    }
    let pad = i64::from(CHART_PADDING_TEXELS);
    let texel = |quantized: u16, extent: u32| {
        (f64::from(quantized) / f64::from(UV_QUANT_MAX) * f64::from(extent)).round() as i64
    };
    let x0 = texel(u.0, formats.irr_width) - pad;
    let y0 = texel(v.0, formats.irr_height) - pad;
    // A face thinner than one texel still owns a one-texel interior.
    let x1 = (texel(u.1, formats.irr_width) + pad).max(x0 + 2 * pad + 1);
    let y1 = (texel(v.1, formats.irr_height) + pad).max(y0 + 2 * pad + 1);
    let layer = u32::from(layer.unwrap_or(0));
    if x0 < 0
        || y0 < 0
        || x1 > i64::from(formats.irr_width)
        || y1 > i64::from(formats.irr_height)
        || layer >= formats.layer_count
    {
        stats.out_of_bounds_faces += 1;
        return Ok(None);
    }
    Ok(Some(ChartRect {
        cell: face.cell_id,
        layer,
        x: x0 as u32,
        y: y0 as u32,
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
    }))
}
