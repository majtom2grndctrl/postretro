//! PRL → `DryRunInput`: parses only the sections the dry run needs and
//! recovers each face chart's padded block-local rectangle from stored vertex
//! block ids and UVs.

use std::collections::VecDeque;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::{Context, bail, ensure};
use glam::Vec3;
use postretro_level_format::bvh::{BvhLeaf, BvhSection};
use postretro_level_format::cell_locator::{self, CellLocatorSection};
use postretro_level_format::cell_visibility::CellVisibilitySection;
use postretro_level_format::cells::CellsSection;
use postretro_level_format::cluster_directory::{
    CLUSTER_HINT_FLAG_PINNED, ClusterDirectorySection, ClusterHintRecord,
};
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::lightmap::{DIRECTION_TEXEL_BYTES, LightmapBlockIndex};
use postretro_level_format::portals::{PortalRecord, PortalsSection};
use postretro_level_format::shadowmask_atlas::ShadowmaskBlockIndex;
use postretro_level_format::{SectionId, read_container, read_section_data};
use postretro_level_loader::{
    CellData, CellLocatorChild, CellLocatorNodeData, LevelWorld, PortalData,
};

use super::portal_distance::{HubCell, HubPortal, PortalGraphInput};
use super::{
    AtlasFormats, CellInfo, ChartRect, DryRunInput, FaceSlot, ReconstructionStats,
    ShadowmaskFormat, ShadowmaskState, StoredBlock,
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

    // Each payload section is reduced to its block index and dropped before
    // the next is read, bounding peak memory to one section. Only the index
    // is parsed; blob bytes are never copied.
    let lightmap_bytes = optional(SectionId::Lightmap)?.context("missing Lightmap (id 22)")?;
    let lightmap = LightmapBlockIndex::from_prefix(&lightmap_bytes, lightmap_bytes.len() as u64)
        .context("parse Lightmap")?;
    drop(lightmap_bytes);
    let shadowmask = match optional(SectionId::ShadowmaskAtlas)? {
        Some(bytes) => {
            let index = ShadowmaskBlockIndex::from_prefix(&bytes, bytes.len() as u64, &lightmap)
                .context("parse ShadowmaskAtlas")?;
            let block_bytes: Vec<u64> = index
                .records
                .iter()
                .map(|r| u64::from(r.group_a.len) + u64::from(r.group_b.len))
                .collect();
            ShadowmaskState::Stored(ShadowmaskFormat {
                payload_bytes: block_bytes.iter().sum(),
                block_bytes,
            })
        }
        None => {
            // The bake emits id 42 for every usable id-40 selection.
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
        irr_format: lightmap.header.irradiance_format,
        direction_texel_scale: lightmap.header.direction_texel_scale,
        direction_texel_bytes: DIRECTION_TEXEL_BYTES as u64,
        blocks: lightmap
            .records
            .iter()
            .map(|record| StoredBlock {
                cell: record.cell_id,
                width: u32::from(record.width),
                height: u32::from(record.height),
                irradiance_bytes: u64::from(record.irradiance.len),
                direction_bytes: u64::from(record.direction.len),
            })
            .collect(),
        shadowmask,
    };
    drop(lightmap);

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
    let locator = CellLocatorSection::from_bytes(
        &optional(SectionId::CellLocator)?.context("missing CellLocator")?,
        cell_count,
    )
    .context("parse CellLocator")?;

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
            exterior: record.is_exterior(),
        })
        .collect();

    let pinned_clusters = pinned_clusters(&directory.cluster_hints);

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
            .map(|record| HubPortal {
                front: record.front_leaf,
                back: record.back_leaf,
                vertices: portal_vertices(&portals, record)
                    .unwrap_or_default()
                    .to_vec(),
            })
            .collect(),
    };
    let loader_rejected_portals = portals
        .portals
        .iter()
        .filter(|record| loader_rejects_portal(portal_vertices(&portals, record)))
        .count();

    let visibility_world = visibility_world(&cells, &portals, locator)?;

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
        visibility_world: Some(visibility_world),
        loader_rejected_portals,
        reconstruction,
    })
}

/// Clusters an id-49 hint flags pinned, ascending. The flag alone decides:
/// a priority region's hint ranks prefetch and pins nothing, and SH's owner
/// closure is SH-only.
pub(super) fn pinned_clusters(hints: &[ClusterHintRecord]) -> Vec<u32> {
    let mut pinned: Vec<u32> = hints
        .iter()
        .filter(|hint| hint.flags & CLUSTER_HINT_FLAG_PINNED != 0)
        .map(|hint| hint.cluster_id)
        .collect();
    pinned.sort_unstable();
    pinned.dedup();
    pinned
}

/// A portal record's vertices, or `None` for a range outside the vertex
/// buffer, so a bad record degrades to an empty polygon instead of panicking.
fn portal_vertices<'a>(
    portals: &'a PortalsSection,
    record: &PortalRecord,
) -> Option<&'a [[f32; 3]]> {
    let start = record.vertex_start as usize;
    let end = start.checked_add(record.vertex_count as usize)?;
    portals.vertices.get(start..end)
}

/// Whether the runtime loader would reject this portal. Must match
/// `convert_usable_portals` in `crates/level-loader/src/prl_loader.rs`: a bad
/// vertex range, fewer than 3 vertices, a non-finite vertex, or zero area.
pub(super) fn loader_rejects_portal(vertices: Option<&[[f32; 3]]>) -> bool {
    let Some(vertices) = vertices else {
        return true;
    };
    if vertices.len() < 3 || !vertices.iter().flatten().all(|c| c.is_finite()) {
        return true;
    }
    let area = vertices
        .iter()
        .zip(vertices.iter().cycle().skip(1))
        .take(vertices.len())
        .fold(Vec3::ZERO, |sum, (a, b)| {
            sum + Vec3::from(*a).cross(Vec3::from(*b))
        });
    area.length_squared() <= 1.0e-12
}

/// The runtime visibility world, built from ids 38, 15 and the cell locator
/// with the field mapping `postretro-level-loader` applies on load. Loading
/// the whole PRL would read every payload, over a gigabyte on large maps.
fn visibility_world(
    cells: &CellsSection,
    portals: &PortalsSection,
    locator: CellLocatorSection,
) -> anyhow::Result<LevelWorld> {
    let cell_data = cells
        .cells
        .iter()
        .map(|record| CellData {
            bounds_min: record.bounds_min.into(),
            bounds_max: record.bounds_max.into(),
            face_start: record.face_start,
            face_count: record.face_count,
            portal_ref_start: record.portal_ref_start,
            portal_ref_count: record.portal_ref_count,
            is_solid: record.is_solid(),
            is_exterior: record.is_exterior(),
            is_drawable: record.is_drawable(),
        })
        .collect();
    let portal_data: Vec<PortalData> = portals
        .portals
        .iter()
        .map(|record| PortalData {
            polygon: portal_vertices(portals, record)
                .unwrap_or_default()
                .iter()
                .map(|&v| v.into())
                .collect(),
            front_cell: record.front_leaf as usize,
            back_cell: record.back_leaf as usize,
        })
        .collect();
    let child = |child: cell_locator::CellLocatorChild| match child {
        cell_locator::CellLocatorChild::Cell(index) => CellLocatorChild::Cell(index as usize),
        cell_locator::CellLocatorChild::Node(index) => CellLocatorChild::Node(index as usize),
    };
    let nodes = locator
        .nodes
        .iter()
        .map(|node| CellLocatorNodeData {
            plane_normal: node.plane_normal.into(),
            plane_distance: node.plane_distance,
            front: child(node.front),
            back: child(node.back),
        })
        .collect();
    let has_portals = !portal_data.is_empty();
    LevelWorld::new_visibility_only(
        cell_data,
        cells.portal_refs.clone(),
        child(locator.root),
        nodes,
        portal_data,
        has_portals,
    )
    .map_err(|error| anyhow::anyhow!("build visibility world: {error}"))
}

/// Recover one padded block-local chart rectangle per face, in the bake's
/// chart order.
///
/// The bake plans one chart per Geometry face record, in record order. A BVH
/// leaf is one face's index range; faces with no indices have no leaf. Each
/// face record takes the next leaf of its cell in index-offset order, or
/// becomes a 1×1 placeholder when its cell has none left. Placement order
/// within a cell only breaks ties between equal-area charts, and every
/// placeholder is 1×1 while a real chart is at least `2 × padding + 1` wide,
/// so the recovered order packs exactly as the bake's did.
///
/// The bake writes a vertex UV as `(block-local placement + padding + local ·
/// scale) / block extent`, and the face's extreme vertices land exactly on
/// the interior's integer edges, so rounding the quantized UV bounds (1/32
/// texel worst case at a 2048-texel block) recovers the placement and padded
/// size exactly.
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
    let mut block_slot = None;
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
        match block_slot {
            None => block_slot = Some(vertex.lightmap_block),
            Some(existing) => mixed_layer |= existing != vertex.lightmap_block,
        }
    }
    // Slot 0 names no block; a real slot is `block id + 1`.
    let block_slot = block_slot.unwrap_or(0);
    if (u.1 == 0 && v.1 == 0) || block_slot == 0 {
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
    let layer = u32::from(block_slot) - 1;
    let Some(block) = formats.blocks.get(layer as usize) else {
        stats.out_of_bounds_faces += 1;
        return Ok(None);
    };
    let pad = i64::from(CHART_PADDING_TEXELS);
    let texel = |quantized: u16, extent: u32| {
        (f64::from(quantized) / f64::from(UV_QUANT_MAX) * f64::from(extent)).round() as i64
    };
    let x0 = texel(u.0, block.width) - pad;
    let y0 = texel(v.0, block.height) - pad;
    // A face thinner than one texel still owns a one-texel interior.
    let x1 = (texel(u.1, block.width) + pad).max(x0 + 2 * pad + 1);
    let y1 = (texel(v.1, block.height) + pad).max(y0 + 2 * pad + 1);
    if x0 < 0
        || y0 < 0
        || x1 > i64::from(block.width)
        || y1 > i64::from(block.height)
        || block.cell != face.cell_id
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
