// PRL level loading: read .prl files, produce BSP tree + cell-chunk engine data structures.
// See: context/lib/build_pipeline.md §PRL

use std::collections::HashSet;
use std::path::Path;

use glam::Vec3;
use postretro_level_format::bsp::{BspLeavesSection, BspNodesSection};
use postretro_level_format::cell_chunks::CellChunksSection;
use postretro_level_format::geometry::{
    GeometrySection, GeometrySectionV2, GeometrySectionV3, NO_TEXTURE,
};
use postretro_level_format::leaf_pvs::LeafPvsSection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_level_format::visibility::decompress_pvs;
use postretro_level_format::{self as prl_format, SectionId};
use thiserror::Error;

use crate::geometry::{CellChunkTable, CellRange, DrawChunk, WorldVertex};
use crate::material::{self, Material};

#[derive(Debug, Error)]
pub enum PrlLoadError {
    #[error("PRL file not found: {0}")]
    FileNotFound(String),
    #[error("failed to read PRL file: {0}")]
    IoError(#[from] std::io::Error),
    #[error("PRL format error: {0}")]
    FormatError(#[from] prl_format::FormatError),
    #[error("PRL file has no geometry section")]
    NoGeometry,
}

/// Per-face draw metadata for PRL levels.
/// Mirrors BSP's `FaceMeta` fields for renderer compatibility.
#[derive(Debug, Clone)]
pub struct FaceMeta {
    pub index_offset: u32,
    pub index_count: u32,
    /// BSP leaf index this face belongs to.
    pub leaf_index: u32,
    /// Index into the texture names list. `None` for untextured faces.
    pub texture_index: Option<u32>,
    /// Texture dimensions (width, height). Default (64, 64) for missing textures.
    #[allow(dead_code)]
    pub texture_dimensions: (u32, u32),
    /// Texture name from PRL data. Empty string if no texture data.
    pub texture_name: String,
    /// Material type derived from texture name prefix.
    pub material: Material,
}

/// A BSP tree child reference: either an interior node or a leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BspChild {
    Node(usize),
    Leaf(usize),
}

/// BSP interior node: splitting plane + front/back children.
#[derive(Debug, Clone)]
pub struct NodeData {
    pub plane_normal: Vec3,
    pub plane_distance: f32,
    pub front: BspChild,
    pub back: BspChild,
}

/// BSP leaf: contains face range, bounds, PVS, and solid flag.
#[derive(Debug, Clone)]
pub struct LeafData {
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub face_start: u32,
    pub face_count: u32,
    /// Decompressed PVS: pvs[i] = leaf i is visible from this leaf.
    pub pvs: Vec<bool>,
    pub is_solid: bool,
}

/// A portal connecting two adjacent BSP leaves, loaded from the Portals section.
#[derive(Debug, Clone)]
pub struct PortalData {
    /// Convex polygon vertices in world space.
    pub polygon: Vec<Vec3>,
    pub front_leaf: usize,
    pub back_leaf: usize,
}

/// BSP tree + cell-chunk level data loaded from a .prl file.
#[derive(Debug)]
pub struct LevelWorld {
    pub vertices: Vec<WorldVertex>,
    pub indices: Vec<u32>,
    pub face_meta: Vec<FaceMeta>,
    pub leaves: Vec<LeafData>,
    pub nodes: Vec<NodeData>,
    /// Root of the BSP tree. For a single-leaf tree (no nodes), this is BspChild::Leaf(0).
    pub root: BspChild,
    /// Whether PVS data was present in the file.
    pub has_pvs: bool,
    /// Portal polygons loaded from the Portals section.
    pub portals: Vec<PortalData>,
    /// Portal indices per leaf (adjacency list). `leaf_portals[i]` lists all
    /// portal indices touching leaf `i`.
    pub leaf_portals: Vec<Vec<usize>>,
    /// Whether portal data was present in the file.
    pub has_portals: bool,
    /// Texture names from the TextureNames section, indexed by face texture_index.
    pub texture_names: Vec<String>,
    /// Per-cell draw chunk table loaded from the CellChunks section.
    /// `None` when loading legacy PRL files without a CellChunks section.
    pub cell_chunk_table: Option<CellChunkTable>,
}

impl LevelWorld {
    /// Find which BSP leaf contains the given position via BSP tree descent.
    ///
    /// At each node, tests the position against the splitting plane and descends
    /// into the appropriate child. Returns the leaf index.
    ///
    /// Fallback behavior:
    /// - If position is on the plane (within epsilon), chooses front.
    /// - If the tree is empty (no nodes), returns leaf 0.
    pub fn find_leaf(&self, position: Vec3) -> usize {
        let mut current = self.root;

        loop {
            match current {
                BspChild::Leaf(leaf_idx) => return leaf_idx,
                BspChild::Node(node_idx) => {
                    let node = &self.nodes[node_idx];
                    let side = node.plane_normal.dot(position) - node.plane_distance;
                    // side >= 0.0 means front (on-plane chooses front).
                    if side >= 0.0 {
                        current = node.front;
                    } else {
                        current = node.back;
                    }
                }
            }
        }
    }

    /// Compute a reasonable spawn position: center of the level's geometry bounds.
    pub fn spawn_position(&self) -> Vec3 {
        let mut mins = Vec3::splat(f32::MAX);
        let mut maxs = Vec3::splat(f32::MIN);
        for leaf in &self.leaves {
            if leaf.is_solid || leaf.face_count == 0 {
                continue;
            }
            mins = mins.min(leaf.bounds_min);
            maxs = maxs.max(leaf.bounds_max);
        }
        (mins + maxs) * 0.5
    }
}

/// Build a per-face leaf index mapping from leaf face ranges.
///
/// Returns a Vec where entry `i` is the leaf index that face `i` belongs to.
/// Used by the renderer to assign per-leaf wireframe colors.
#[allow(dead_code)]
pub fn face_leaf_indices(world: &LevelWorld) -> Vec<u32> {
    let mut indices = vec![0u32; world.face_meta.len()];
    for (leaf_idx, leaf) in world.leaves.iter().enumerate() {
        let start = leaf.face_start as usize;
        let count = leaf.face_count as usize;
        for face_idx in start..start + count {
            if let Some(slot) = indices.get_mut(face_idx) {
                *slot = leaf_idx as u32;
            }
        }
    }
    indices
}

/// Sentinel value for faces with no texture assignment.
const NO_TEXTURE_INDEX: u32 = u32::MAX;

/// Sort the index buffer by (leaf_index, texture_index) and rebuild face_meta offsets.
/// After sorting, faces within each leaf are grouped by texture, enabling efficient
/// draw call batching. Same logic as BSP's `sort_indices_by_leaf_and_texture`.
fn sort_indices_by_leaf_and_texture(indices: &mut Vec<u32>, face_meta: &mut Vec<FaceMeta>) {
    let mut sorted_faces: Vec<usize> = (0..face_meta.len()).collect();
    sorted_faces.sort_by(|&a, &b| {
        let fa = &face_meta[a];
        let fb = &face_meta[b];
        let leaf_cmp = fa.leaf_index.cmp(&fb.leaf_index);
        if leaf_cmp != std::cmp::Ordering::Equal {
            return leaf_cmp;
        }
        let tex_a = fa.texture_index.unwrap_or(NO_TEXTURE_INDEX);
        let tex_b = fb.texture_index.unwrap_or(NO_TEXTURE_INDEX);
        tex_a.cmp(&tex_b)
    });

    let old_indices = indices.clone();
    let mut new_indices = Vec::with_capacity(old_indices.len());
    let mut new_face_meta = Vec::with_capacity(face_meta.len());

    for &orig_idx in &sorted_faces {
        let old_face = &face_meta[orig_idx];
        let old_offset = old_face.index_offset as usize;
        let old_count = old_face.index_count as usize;

        let new_offset = new_indices.len() as u32;
        if old_offset + old_count <= old_indices.len() {
            new_indices.extend_from_slice(&old_indices[old_offset..old_offset + old_count]);
        }

        new_face_meta.push(FaceMeta {
            index_offset: new_offset,
            index_count: old_face.index_count,
            leaf_index: old_face.leaf_index,
            texture_index: old_face.texture_index,
            texture_dimensions: old_face.texture_dimensions,
            texture_name: old_face.texture_name.clone(),
            material: old_face.material,
        });
    }

    *indices = new_indices;
    *face_meta = new_face_meta;
}

/// Decode a PRL sentinel-encoded child reference.
///
/// Positive values are node indices; negative values encode leaves as `(-1 - leaf_index)`.
fn decode_child(value: i32) -> BspChild {
    if value >= 0 {
        BspChild::Node(value as usize)
    } else {
        BspChild::Leaf((-1 - value) as usize)
    }
}

pub fn load_prl(path: &str) -> Result<LevelWorld, PrlLoadError> {
    let path_ref = Path::new(path);
    if !path_ref.exists() {
        return Err(PrlLoadError::FileNotFound(path.to_string()));
    }

    let file_data = std::fs::read(path_ref)?;
    let mut cursor = std::io::Cursor::new(&file_data);

    let meta = prl_format::read_container(&mut cursor)?;

    // Try GeometryV3 first (packed normals/tangents), then V2 (UVs + texture indices),
    // then legacy V1. Each supersedes the prior.
    let geom_v3_data =
        prl_format::read_section_data(&mut cursor, &meta, SectionId::GeometryV3 as u32)?;
    let geom_v2_data =
        prl_format::read_section_data(&mut cursor, &meta, SectionId::GeometryV2 as u32)?;
    let geom_v1_data =
        prl_format::read_section_data(&mut cursor, &meta, SectionId::Geometry as u32)?;

    // TextureNames section (optional, meaningful with GeometryV2 and V3).
    let texture_names_data =
        prl_format::read_section_data(&mut cursor, &meta, SectionId::TextureNames as u32)?;
    let texture_names_section = match texture_names_data {
        Some(data) => Some(TextureNamesSection::from_bytes(&data)?),
        None => None,
    };

    // CellChunks section (optional, present alongside GeometryV3).
    let cell_chunks_data =
        prl_format::read_section_data(&mut cursor, &meta, SectionId::CellChunks as u32)?;

    // Build vertices, indices, face_meta, and texture_names from whichever geometry section is present.
    let (vertices, mut indices, mut face_meta, texture_names) =
        if let Some(v3_data) = geom_v3_data {
            let geom_v3 = GeometrySectionV3::from_bytes(&v3_data)?;
            let tex_names: Vec<String> =
                texture_names_section.map(|s| s.names).unwrap_or_default();
            let mut warned_prefixes = HashSet::new();

            let verts: Vec<WorldVertex> = geom_v3
                .vertices
                .iter()
                .map(|v| WorldVertex {
                    position: v.position,
                    // Store raw texel-space UVs; normalized in main.rs after texture dimensions are known.
                    base_uv: v.uv,
                    normal_oct: v.normal_oct,
                    tangent_packed: v.tangent_packed,
                })
                .collect();

            let fm: Vec<FaceMeta> = geom_v3
                .faces
                .iter()
                .map(|f| {
                    let (tex_idx, tex_name) = if f.texture_index == NO_TEXTURE {
                        (None, String::new())
                    } else {
                        let name = tex_names
                            .get(f.texture_index as usize)
                            .cloned()
                            .unwrap_or_default();
                        (Some(f.texture_index), name)
                    };
                    let mat = material::derive_material(&tex_name, &mut warned_prefixes);
                    FaceMeta {
                        index_offset: f.index_offset,
                        index_count: f.index_count,
                        leaf_index: f.leaf_index,
                        texture_index: tex_idx,
                        texture_dimensions: (64, 64),
                        texture_name: tex_name,
                        material: mat,
                    }
                })
                .collect();

            log::info!(
                "[PRL] GeometryV3: {} vertices, {} textures referenced",
                verts.len(),
                tex_names.len()
            );

            (verts, geom_v3.indices, fm, tex_names)
        } else if let Some(v2_data) = geom_v2_data {
            let geom_v2 = GeometrySectionV2::from_bytes(&v2_data)?;
            let tex_names: Vec<String> =
                texture_names_section.map(|s| s.names).unwrap_or_default();
            let mut warned_prefixes = HashSet::new();

            // V2 has no normal/tangent data; fill with +Z normal and +X tangent.
            let default_normal = postretro_level_format::octahedral::encode(0.0, 0.0, 1.0);
            let default_tangent_oct = postretro_level_format::octahedral::encode(1.0, 0.0, 0.0);
            let default_tangent_packed = {
                let v_15bit =
                    (default_tangent_oct[1] as u32 * 32767 / 65535) as u16;
                [default_tangent_oct[0], v_15bit | 0x8000] // positive bitangent sign
            };

            let verts: Vec<WorldVertex> = geom_v2
                .vertices
                .iter()
                .map(|v| WorldVertex {
                    position: [v[0], v[1], v[2]],
                    base_uv: [v[3], v[4]],
                    normal_oct: default_normal,
                    tangent_packed: default_tangent_packed,
                })
                .collect();

            let fm: Vec<FaceMeta> = geom_v2
                .faces
                .iter()
                .map(|f| {
                    let (tex_idx, tex_name) = if f.texture_index == NO_TEXTURE {
                        (None, String::new())
                    } else {
                        let name = tex_names
                            .get(f.texture_index as usize)
                            .cloned()
                            .unwrap_or_default();
                        (Some(f.texture_index), name)
                    };
                    let mat = material::derive_material(&tex_name, &mut warned_prefixes);
                    FaceMeta {
                        index_offset: f.index_offset,
                        index_count: f.index_count,
                        leaf_index: f.leaf_index,
                        texture_index: tex_idx,
                        texture_dimensions: (64, 64),
                        texture_name: tex_name,
                        material: mat,
                    }
                })
                .collect();

            log::info!("[PRL] GeometryV2: {} textures referenced", tex_names.len());

            (verts, geom_v2.indices, fm, tex_names)
        } else if let Some(v1_data) = geom_v1_data {
            let geom = GeometrySection::from_bytes(&v1_data)?;

            let default_normal = postretro_level_format::octahedral::encode(0.0, 0.0, 1.0);
            let default_tangent_oct = postretro_level_format::octahedral::encode(1.0, 0.0, 0.0);
            let default_tangent_packed = {
                let v_15bit =
                    (default_tangent_oct[1] as u32 * 32767 / 65535) as u16;
                [default_tangent_oct[0], v_15bit | 0x8000]
            };

            let verts: Vec<WorldVertex> = geom
                .vertices
                .iter()
                .map(|pos| WorldVertex {
                    position: *pos,
                    base_uv: [0.0, 0.0],
                    normal_oct: default_normal,
                    tangent_packed: default_tangent_packed,
                })
                .collect();

            let fm: Vec<FaceMeta> = geom
                .faces
                .iter()
                .map(|f| FaceMeta {
                    index_offset: f.index_offset,
                    index_count: f.index_count,
                    leaf_index: f.leaf_index,
                    texture_index: None,
                    texture_dimensions: (64, 64),
                    texture_name: String::new(),
                    material: Material::Default,
                })
                .collect();

            log::info!("[PRL] Legacy Geometry section (no texture data)");

            (verts, geom.indices, fm, Vec::new())
        } else {
            return Err(PrlLoadError::NoGeometry);
        };

    // Parse CellChunks section into engine types.
    let cell_chunk_table = match cell_chunks_data {
        Some(data) => {
            let section = CellChunksSection::from_bytes(&data)?;
            let cell_ranges = section
                .cell_ranges
                .iter()
                .map(|cr| CellRange {
                    cell_id: cr.cell_id,
                    chunk_start: cr.chunk_start,
                    chunk_count: cr.chunk_count,
                })
                .collect();
            let chunks = section
                .chunks
                .iter()
                .map(|c| DrawChunk {
                    cell_id: c.cell_id,
                    aabb_min: c.aabb_min,
                    aabb_max: c.aabb_max,
                    index_offset: c.index_offset,
                    index_count: c.index_count,
                    material_bucket_id: c.material_bucket_id,
                })
                .collect();
            log::info!(
                "[PRL] CellChunks: {} cells, {} chunks",
                section.cell_ranges.len(),
                section.chunks.len()
            );
            Some(CellChunkTable {
                cell_ranges,
                chunks,
            })
        }
        None => {
            log::info!("[PRL] No CellChunks section — using legacy leaf-based draw path");
            None
        }
    };

    // BSP nodes section (optional — absent if tree is a single leaf).
    let nodes_section =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::BspNodes as u32)? {
            Some(data) => Some(BspNodesSection::from_bytes(&data)?),
            None => None,
        };

    // BSP leaves section (optional).
    let leaves_section =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::BspLeaves as u32)? {
            Some(data) => Some(BspLeavesSection::from_bytes(&data)?),
            None => None,
        };

    // Leaf PVS section (optional).
    let pvs_section =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::LeafPvs as u32)? {
            Some(data) => Some(LeafPvsSection::from_bytes(&data)?),
            None => None,
        };

    // Portals section (optional).
    let portals_section =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::Portals as u32)? {
            Some(data) => Some(PortalsSection::from_bytes(&data)?),
            None => None,
        };

    let has_pvs = pvs_section.is_some();
    let has_portals = portals_section.is_some();

    // Build runtime nodes from the nodes section.
    let nodes: Vec<NodeData> = match &nodes_section {
        Some(section) => section
            .nodes
            .iter()
            .map(|n| NodeData {
                plane_normal: Vec3::from(n.plane_normal),
                plane_distance: n.plane_distance,
                front: decode_child(n.front),
                back: decode_child(n.back),
            })
            .collect(),
        None => Vec::new(),
    };

    // Build runtime leaves from the leaves section + PVS data.
    let leaves: Vec<LeafData> = match &leaves_section {
        Some(leaf_sec) => {
            let leaf_count = leaf_sec.leaves.len();
            let pvs_byte_count = leaf_count.div_ceil(8);

            leaf_sec
                .leaves
                .iter()
                .map(|lr| {
                    let pvs = if let Some(pvs_sec) = &pvs_section {
                        if lr.pvs_size > 0 && lr.is_solid == 0 {
                            let start = lr.pvs_offset as usize;
                            let end = start + lr.pvs_size as usize;
                            let pvs_slice = if end <= pvs_sec.pvs_data.len() {
                                &pvs_sec.pvs_data[start..end]
                            } else {
                                &[]
                            };

                            let decompressed = decompress_pvs(pvs_slice, pvs_byte_count);

                            // Convert byte bitfield to per-leaf bool vec.
                            let mut pvs_bools = Vec::with_capacity(leaf_count);
                            for leaf_idx in 0..leaf_count {
                                let byte_idx = leaf_idx / 8;
                                let bit_idx = leaf_idx % 8;
                                let visible = byte_idx < decompressed.len()
                                    && (decompressed[byte_idx] & (1 << bit_idx)) != 0;
                                pvs_bools.push(visible);
                            }
                            pvs_bools
                        } else {
                            // Solid leaf or no PVS data for this leaf.
                            vec![false; leaf_count]
                        }
                    } else {
                        // No PVS section at all — all leaves visible.
                        vec![true; leaf_count]
                    };

                    LeafData {
                        bounds_min: Vec3::from(lr.bounds_min),
                        bounds_max: Vec3::from(lr.bounds_max),
                        face_start: lr.face_start,
                        face_count: lr.face_count,
                        pvs,
                        is_solid: lr.is_solid != 0,
                    }
                })
                .collect()
        }
        None => {
            // No BSP leaves section — derive a single leaf from geometry.
            log::warn!("[PRL] No BSP leaves section — creating single-leaf fallback");
            let mut mins = Vec3::splat(f32::MAX);
            let mut maxs = Vec3::splat(f32::MIN);
            for v in &vertices {
                let pos = Vec3::from(v.position);
                mins = mins.min(pos);
                maxs = maxs.max(pos);
            }
            vec![LeafData {
                bounds_min: mins,
                bounds_max: maxs,
                face_start: 0,
                face_count: face_meta.len() as u32,
                pvs: vec![true],
                is_solid: false,
            }]
        }
    };

    // Determine BSP root. If nodes exist, root is node 0. Otherwise, leaf 0.
    let root = if nodes.is_empty() {
        BspChild::Leaf(0)
    } else {
        BspChild::Node(0)
    };

    // Load portal data and build adjacency list.
    let (portals, leaf_portals) = if let Some(ps) = &portals_section {
        let portal_data: Vec<PortalData> = ps
            .portals
            .iter()
            .map(|pr| {
                let start = pr.vertex_start as usize;
                let count = pr.vertex_count as usize;
                let end = start + count;
                if end > ps.vertices.len() {
                    return Err(PrlLoadError::FormatError(prl_format::FormatError::Io(
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!(
                                "portal vertex range {}..{} exceeds vertex count {}",
                                start,
                                end,
                                ps.vertices.len()
                            ),
                        ),
                    )));
                }
                let polygon: Vec<Vec3> = ps.vertices[start..end]
                    .iter()
                    .map(|v| Vec3::from(*v))
                    .collect();
                Ok(PortalData {
                    polygon,
                    front_leaf: pr.front_leaf as usize,
                    back_leaf: pr.back_leaf as usize,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Build per-leaf adjacency.
        let mut adjacency = vec![Vec::new(); leaves.len()];
        for (portal_idx, portal) in portal_data.iter().enumerate() {
            if portal.front_leaf < adjacency.len() {
                adjacency[portal.front_leaf].push(portal_idx);
            }
            if portal.back_leaf < adjacency.len() {
                adjacency[portal.back_leaf].push(portal_idx);
            }
        }

        (portal_data, adjacency)
    } else {
        (Vec::new(), vec![Vec::new(); leaves.len()])
    };

    // When CellChunks are present, the index buffer is already organized by
    // (cell, material_bucket). The old sort-by-(leaf, texture) path is only
    // needed when falling back to legacy geometry without CellChunks.
    if cell_chunk_table.is_none() {
        sort_indices_by_leaf_and_texture(&mut indices, &mut face_meta);
    }

    let chunks_summary = match &cell_chunk_table {
        Some(t) => format!("{} cells, {} chunks", t.cell_ranges.len(), t.chunks.len()),
        None => "none (legacy)".to_string(),
    };
    log::info!(
        "[PRL] Loaded: {} vertices, {} indices ({} triangles), {} faces, {} nodes, {} leaves, cell_chunks=[{}], pvs={}, portals={}, textures={}",
        vertices.len(),
        indices.len(),
        indices.len() / 3,
        face_meta.len(),
        nodes.len(),
        leaves.len(),
        chunks_summary,
        has_pvs,
        portals.len(),
        texture_names.len(),
    );

    Ok(LevelWorld {
        vertices,
        indices,
        face_meta,
        leaves,
        nodes,
        root,
        has_pvs,
        portals,
        leaf_portals,
        has_portals,
        texture_names,
        cell_chunk_table,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::bsp::{
        BspLeafRecord, BspLeavesSection, BspNodeRecord, BspNodesSection,
    };
    use postretro_level_format::geometry::{FaceMeta as FormatFaceMeta, GeometrySection};
    use postretro_level_format::leaf_pvs::LeafPvsSection;
    use postretro_level_format::visibility::compress_pvs;

    // -- find_leaf tests --

    /// Helper to build a default FaceMeta with only index_offset and index_count.
    fn simple_face_meta(index_offset: u32, index_count: u32) -> FaceMeta {
        FaceMeta {
            index_offset,
            index_count,
            leaf_index: 0,
            texture_index: None,
            texture_dimensions: (64, 64),
            texture_name: String::new(),
            material: Material::Default,
        }
    }

    /// Helper to build a default LeafData.
    fn simple_leaf(
        bounds_min: Vec3,
        bounds_max: Vec3,
        face_start: u32,
        face_count: u32,
        pvs: Vec<bool>,
        is_solid: bool,
    ) -> LeafData {
        LeafData {
            bounds_min,
            bounds_max,
            face_start,
            face_count,
            pvs,
            is_solid,
        }
    }

    /// Build a simple two-leaf BSP: one node splits space at X=0.
    /// Front (X >= 0) goes to leaf 0, back (X < 0) goes to leaf 1.
    fn two_leaf_world() -> LevelWorld {
        LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![NodeData {
                plane_normal: Vec3::X,
                plane_distance: 0.0,
                front: BspChild::Leaf(0),
                back: BspChild::Leaf(1),
            }],
            leaves: vec![
                simple_leaf(
                    Vec3::new(0.0, -100.0, -100.0),
                    Vec3::new(100.0, 100.0, 100.0),
                    0,
                    1,
                    vec![true, true],
                    false,
                ),
                simple_leaf(
                    Vec3::new(-100.0, -100.0, -100.0),
                    Vec3::new(0.0, 100.0, 100.0),
                    1,
                    1,
                    vec![true, true],
                    false,
                ),
            ],
            root: BspChild::Node(0),
            has_pvs: true,
            portals: vec![],
            leaf_portals: vec![vec![], vec![]],
            has_portals: false,
            texture_names: vec![],
            cell_chunk_table: None,
        }
    }

    #[test]
    fn find_leaf_front_side() {
        let world = two_leaf_world();
        assert_eq!(world.find_leaf(Vec3::new(10.0, 0.0, 0.0)), 0);
    }

    #[test]
    fn find_leaf_back_side() {
        let world = two_leaf_world();
        assert_eq!(world.find_leaf(Vec3::new(-10.0, 0.0, 0.0)), 1);
    }

    #[test]
    fn find_leaf_on_plane_goes_front() {
        let world = two_leaf_world();
        // Exactly on the plane (dot = 0.0 >= 0.0) should go to front.
        assert_eq!(world.find_leaf(Vec3::ZERO), 0);
    }

    #[test]
    fn find_leaf_single_leaf_tree() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![],
            leaves: vec![simple_leaf(
                Vec3::splat(-100.0),
                Vec3::splat(100.0),
                0,
                0,
                vec![true],
                false,
            )],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![vec![]],
            has_portals: false,
            texture_names: vec![],
            cell_chunk_table: None,
        };
        assert_eq!(world.find_leaf(Vec3::new(50.0, 50.0, 50.0)), 0);
    }

    #[test]
    fn find_leaf_deep_tree() {
        // 3-level tree: root splits on X=0, front splits on Y=0.
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![
                // Node 0: split on X=0
                NodeData {
                    plane_normal: Vec3::X,
                    plane_distance: 0.0,
                    front: BspChild::Node(1),
                    back: BspChild::Leaf(0),
                },
                // Node 1: split on Y=0
                NodeData {
                    plane_normal: Vec3::Y,
                    plane_distance: 0.0,
                    front: BspChild::Leaf(1),
                    back: BspChild::Leaf(2),
                },
            ],
            leaves: vec![
                simple_leaf(
                    Vec3::splat(-100.0),
                    Vec3::new(0.0, 100.0, 100.0),
                    0,
                    0,
                    vec![],
                    false,
                ),
                simple_leaf(
                    Vec3::new(0.0, 0.0, -100.0),
                    Vec3::splat(100.0),
                    0,
                    0,
                    vec![],
                    false,
                ),
                simple_leaf(
                    Vec3::new(0.0, -100.0, -100.0),
                    Vec3::new(100.0, 0.0, 100.0),
                    0,
                    0,
                    vec![],
                    false,
                ),
            ],
            root: BspChild::Node(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![vec![], vec![], vec![]],
            has_portals: false,
            texture_names: vec![],
            cell_chunk_table: None,
        };

        // X < 0 -> leaf 0
        assert_eq!(world.find_leaf(Vec3::new(-5.0, 0.0, 0.0)), 0);
        // X > 0, Y > 0 -> leaf 1
        assert_eq!(world.find_leaf(Vec3::new(5.0, 5.0, 0.0)), 1);
        // X > 0, Y < 0 -> leaf 2
        assert_eq!(world.find_leaf(Vec3::new(5.0, -5.0, 0.0)), 2);
    }

    // -- decode_child tests --

    #[test]
    fn decode_child_positive_is_node() {
        assert_eq!(decode_child(0), BspChild::Node(0));
        assert_eq!(decode_child(5), BspChild::Node(5));
    }

    #[test]
    fn decode_child_negative_is_leaf() {
        // -1 - leaf_index encoding
        assert_eq!(decode_child(-1), BspChild::Leaf(0));
        assert_eq!(decode_child(-6), BspChild::Leaf(5));
        assert_eq!(decode_child(-101), BspChild::Leaf(100));
    }

    // -- spawn_position tests --

    #[test]
    fn spawn_position_centers_non_solid_leaves() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![simple_face_meta(0, 3)],
            nodes: vec![],
            leaves: vec![
                simple_leaf(Vec3::ZERO, Vec3::splat(10.0), 0, 1, vec![], false),
                simple_leaf(Vec3::ZERO, Vec3::ZERO, 0, 0, vec![], true),
            ],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![vec![], vec![]],
            has_portals: false,
            texture_names: vec![],
            cell_chunk_table: None,
        };

        let spawn = world.spawn_position();
        assert!((spawn - Vec3::splat(5.0)).length() < 0.01);
    }

    // -- face_leaf_indices tests --

    #[test]
    fn face_leaf_indices_maps_faces_to_leaves() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![
                simple_face_meta(0, 3),
                simple_face_meta(3, 3),
                simple_face_meta(6, 3),
            ],
            nodes: vec![],
            leaves: vec![
                simple_leaf(Vec3::ZERO, Vec3::ZERO, 0, 2, vec![], false),
                simple_leaf(Vec3::ZERO, Vec3::ZERO, 2, 1, vec![], false),
            ],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![vec![], vec![]],
            has_portals: false,
            texture_names: vec![],
            cell_chunk_table: None,
        };

        let indices = face_leaf_indices(&world);
        assert_eq!(indices, vec![0, 0, 1]);
    }

    // -- load_prl error cases --

    #[test]
    fn load_prl_missing_file_returns_file_not_found() {
        let result = load_prl("nonexistent/path/to/map.prl");
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), PrlLoadError::FileNotFound(_)),
            "expected FileNotFound"
        );
    }

    // -- Round-trip: write a PRL file with BSP sections, load it --

    fn sample_geometry() -> GeometrySection {
        GeometrySection {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [10.0, 0.0, 0.0],
                [11.0, 0.0, 0.0],
                [11.0, 1.0, 0.0],
            ],
            indices: vec![0, 1, 2, 3, 4, 5],
            faces: vec![
                FormatFaceMeta {
                    index_offset: 0,
                    index_count: 3,
                    leaf_index: 0,
                },
                FormatFaceMeta {
                    index_offset: 3,
                    index_count: 3,
                    leaf_index: 1,
                },
            ],
        }
    }

    #[test]
    fn load_prl_round_trip_with_bsp_sections() {
        let geom = sample_geometry();

        // Build BSP sections: 1 node splitting two leaves.
        let nodes = BspNodesSection {
            nodes: vec![BspNodeRecord {
                plane_normal: [1.0, 0.0, 0.0],
                plane_distance: 5.0,
                front: -1 - 0, // leaf 0
                back: -1 - 1,  // leaf 1
            }],
        };

        // Build PVS: 2 leaves, each sees both.
        let pvs_uncompressed = vec![0b0000_0011u8]; // both bits set
        let compressed_0 = compress_pvs(&pvs_uncompressed);
        let compressed_1 = compress_pvs(&pvs_uncompressed);

        let mut pvs_data = Vec::new();
        let offset_0 = pvs_data.len() as u32;
        let size_0 = compressed_0.len() as u32;
        pvs_data.extend_from_slice(&compressed_0);
        let offset_1 = pvs_data.len() as u32;
        let size_1 = compressed_1.len() as u32;
        pvs_data.extend_from_slice(&compressed_1);

        let leaves = BspLeavesSection {
            leaves: vec![
                BspLeafRecord {
                    face_start: 0,
                    face_count: 1,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    pvs_offset: offset_0,
                    pvs_size: size_0,
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 1,
                    face_count: 1,
                    bounds_min: [9.0, 0.0, 0.0],
                    bounds_max: [12.0, 2.0, 2.0],
                    pvs_offset: offset_1,
                    pvs_size: size_1,
                    is_solid: 0,
                },
            ],
        };

        let pvs_section = LeafPvsSection { pvs_data };

        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geom.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::BspNodes as u32,
                version: 1,
                data: nodes.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::BspLeaves as u32,
                version: 1,
                data: leaves.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::LeafPvs as u32,
                version: 1,
                data: pvs_section.to_bytes(),
            },
        ];

        let tmp = std::env::temp_dir().join("postretro_test_bsp_round_trip.prl");
        let mut file = std::fs::File::create(&tmp).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();

        let world = load_prl(tmp.to_str().unwrap()).unwrap();
        assert_eq!(world.vertices.len(), 6);
        assert_eq!(world.indices.len(), 6);
        assert_eq!(world.face_meta.len(), 2);
        assert_eq!(world.nodes.len(), 1);
        assert_eq!(world.leaves.len(), 2);
        assert!(world.has_pvs);
        assert_eq!(world.root, BspChild::Node(0));

        // Verify leaf PVS decompression.
        assert_eq!(world.leaves[0].pvs.len(), 2);
        assert!(world.leaves[0].pvs[0]);
        assert!(world.leaves[0].pvs[1]);
        assert_eq!(world.leaves[1].pvs.len(), 2);
        assert!(world.leaves[1].pvs[0]);
        assert!(world.leaves[1].pvs[1]);

        // Verify BSP descent.
        // Node splits at X=5: front (X >= 5) -> leaf 0, back (X < 5) -> leaf 1.
        // Wait — front child is -1-0 = leaf 0, back child is -1-1 = leaf 1.
        // A point at X=10 has dot = 10 >= 5, so front -> leaf 0.
        assert_eq!(world.find_leaf(Vec3::new(10.0, 0.0, 0.0)), 0);
        // A point at X=0 has dot = 0 < 5, so back -> leaf 1.
        assert_eq!(world.find_leaf(Vec3::new(0.0, 0.0, 0.0)), 1);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_round_trip_geometry_only() {
        let geom = sample_geometry();

        let sections = vec![prl_format::SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: geom.to_bytes(),
        }];

        let tmp = std::env::temp_dir().join("postretro_test_geom_only.prl");
        let mut file = std::fs::File::create(&tmp).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();

        let world = load_prl(tmp.to_str().unwrap()).unwrap();
        assert_eq!(world.vertices.len(), 6);
        assert_eq!(world.nodes.len(), 0);
        assert_eq!(world.leaves.len(), 1); // fallback single leaf
        assert!(!world.has_pvs);
        assert_eq!(world.root, BspChild::Leaf(0));

        // Single-leaf fallback: all faces in leaf 0.
        assert_eq!(world.leaves[0].face_start, 0);
        assert_eq!(world.leaves[0].face_count, 2);
        assert!(!world.leaves[0].is_solid);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_with_solid_leaf_has_empty_pvs() {
        let geom = sample_geometry();

        let nodes = BspNodesSection {
            nodes: vec![BspNodeRecord {
                plane_normal: [1.0, 0.0, 0.0],
                plane_distance: 5.0,
                front: -1 - 0, // leaf 0 (empty)
                back: -1 - 1,  // leaf 1 (solid)
            }],
        };

        let pvs_uncompressed = vec![0b0000_0001u8]; // only self visible
        let compressed = compress_pvs(&pvs_uncompressed);

        let leaves = BspLeavesSection {
            leaves: vec![
                BspLeafRecord {
                    face_start: 0,
                    face_count: 2,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [10.0, 10.0, 10.0],
                    pvs_offset: 0,
                    pvs_size: compressed.len() as u32,
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 0,
                    face_count: 0,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [0.0, 0.0, 0.0],
                    pvs_offset: 0,
                    pvs_size: 0,
                    is_solid: 1,
                },
            ],
        };

        let pvs_section = LeafPvsSection {
            pvs_data: compressed,
        };

        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geom.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::BspNodes as u32,
                version: 1,
                data: nodes.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::BspLeaves as u32,
                version: 1,
                data: leaves.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::LeafPvs as u32,
                version: 1,
                data: pvs_section.to_bytes(),
            },
        ];

        let tmp = std::env::temp_dir().join("postretro_test_solid_leaf.prl");
        let mut file = std::fs::File::create(&tmp).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();

        let world = load_prl(tmp.to_str().unwrap()).unwrap();

        // Solid leaf should have all-false PVS.
        assert!(world.leaves[1].is_solid);
        assert!(world.leaves[1].pvs.iter().all(|&v| !v));

        // Empty leaf should have valid PVS.
        assert!(!world.leaves[0].is_solid);
        assert!(world.leaves[0].pvs[0]); // sees self

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_invalid_magic_produces_clear_error() {
        let tmp = std::env::temp_dir().join("postretro_test_bad_magic.prl");
        std::fs::write(&tmp, b"NOPE extra data for length").unwrap();

        let result = load_prl(tmp.to_str().unwrap());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("magic"),
            "error should mention magic: {err_msg}"
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_truncated_file_produces_clear_error() {
        let tmp = std::env::temp_dir().join("postretro_test_truncated.prl");
        std::fs::write(&tmp, &[0x50, 0x52, 0x4C]).unwrap(); // "PRL" only

        let result = load_prl(tmp.to_str().unwrap());
        assert!(result.is_err());

        std::fs::remove_file(&tmp).ok();
    }

    // -- GeometryV2 + TextureNames round-trip tests --

    use postretro_level_format::geometry::{FaceMetaV2, GeometrySectionV2};
    use postretro_level_format::texture_names::TextureNamesSection;

    fn sample_geometry_v2() -> GeometrySectionV2 {
        GeometrySectionV2 {
            vertices: vec![
                [0.0, 0.0, 0.0, 10.0, 20.0],
                [1.0, 0.0, 0.0, 30.0, 40.0],
                [1.0, 1.0, 0.0, 50.0, 60.0],
                [10.0, 0.0, 0.0, 100.0, 200.0],
                [11.0, 0.0, 0.0, 110.0, 210.0],
                [11.0, 1.0, 0.0, 120.0, 220.0],
            ],
            indices: vec![0, 1, 2, 3, 4, 5],
            faces: vec![
                FaceMetaV2 {
                    index_offset: 0,
                    index_count: 3,
                    leaf_index: 0,
                    texture_index: 0,
                },
                FaceMetaV2 {
                    index_offset: 3,
                    index_count: 3,
                    leaf_index: 1,
                    texture_index: 1,
                },
            ],
        }
    }

    fn sample_texture_names() -> TextureNamesSection {
        TextureNamesSection {
            names: vec!["metal/floor_01".to_string(), "concrete/wall_03".to_string()],
        }
    }

    #[test]
    fn load_prl_geometry_v2_reads_uvs_and_texture_names() {
        let geom = sample_geometry_v2();
        let tex_names = sample_texture_names();

        let leaves = BspLeavesSection {
            leaves: vec![
                BspLeafRecord {
                    face_start: 0,
                    face_count: 1,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    pvs_offset: 0,
                    pvs_size: 0,
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 1,
                    face_count: 1,
                    bounds_min: [9.0, 0.0, 0.0],
                    bounds_max: [12.0, 2.0, 2.0],
                    pvs_offset: 0,
                    pvs_size: 0,
                    is_solid: 0,
                },
            ],
        };

        let nodes = BspNodesSection {
            nodes: vec![BspNodeRecord {
                plane_normal: [1.0, 0.0, 0.0],
                plane_distance: 5.0,
                front: -1,
                back: -2,
            }],
        };

        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::GeometryV2 as u32,
                version: 1,
                data: geom.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::TextureNames as u32,
                version: 1,
                data: tex_names.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::BspNodes as u32,
                version: 1,
                data: nodes.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::BspLeaves as u32,
                version: 1,
                data: leaves.to_bytes(),
            },
        ];

        let tmp = std::env::temp_dir().join("postretro_test_geom_v2.prl");
        let mut file = std::fs::File::create(&tmp).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();

        let world = load_prl(tmp.to_str().unwrap()).unwrap();

        // Vertices should have UV data from GeometryV2.
        assert_eq!(world.vertices.len(), 6);
        // UVs are texel-space (not yet normalized — that happens in main.rs).
        // After sort, face ordering may change, so check by finding the vertex.
        let first_vert = &world.vertices[0];
        assert!(
            first_vert.base_uv[0] != 0.0
                || first_vert.base_uv[1] != 0.0
                || world.vertices.iter().any(|v| v.base_uv[0] != 0.0),
            "at least some vertices should have non-zero UVs from GeometryV2"
        );

        // Texture names should be loaded.
        assert_eq!(world.texture_names.len(), 2);
        assert_eq!(world.texture_names[0], "metal/floor_01");
        assert_eq!(world.texture_names[1], "concrete/wall_03");

        // Face meta should have texture indices.
        assert_eq!(world.face_meta.len(), 2);
        assert!(world.face_meta.iter().any(|f| f.texture_index == Some(0)));
        assert!(world.face_meta.iter().any(|f| f.texture_index == Some(1)));

        // Face meta should have texture names.
        assert!(
            world
                .face_meta
                .iter()
                .any(|f| f.texture_name == "metal/floor_01")
        );
        assert!(
            world
                .face_meta
                .iter()
                .any(|f| f.texture_name == "concrete/wall_03")
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_legacy_geometry_falls_back_to_zero_uvs() {
        let geom = sample_geometry();

        let sections = vec![prl_format::SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: geom.to_bytes(),
        }];

        let tmp = std::env::temp_dir().join("postretro_test_legacy_geom_fallback.prl");
        let mut file = std::fs::File::create(&tmp).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();

        let world = load_prl(tmp.to_str().unwrap()).unwrap();

        // Legacy geometry should produce vertices with zero UVs.
        assert_eq!(world.vertices.len(), 6);
        for vert in &world.vertices {
            assert_eq!(
                vert.base_uv,
                [0.0, 0.0],
                "legacy vertices should have zero UVs"
            );
        }

        // No texture names from legacy format.
        assert!(world.texture_names.is_empty());

        // Face meta should have no texture index.
        for face in &world.face_meta {
            assert!(
                face.texture_index.is_none(),
                "legacy faces should have no texture index"
            );
        }

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_geometry_v2_no_texture_sentinel_produces_none_index() {
        let geom = GeometrySectionV2 {
            vertices: vec![
                [0.0, 0.0, 0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0, 0.0, 0.0],
                [1.0, 1.0, 0.0, 0.0, 0.0],
            ],
            indices: vec![0, 1, 2],
            faces: vec![FaceMetaV2 {
                index_offset: 0,
                index_count: 3,
                leaf_index: 0,
                texture_index: NO_TEXTURE,
            }],
        };

        let sections = vec![prl_format::SectionBlob {
            section_id: SectionId::GeometryV2 as u32,
            version: 1,
            data: geom.to_bytes(),
        }];

        let tmp = std::env::temp_dir().join("postretro_test_no_tex_sentinel.prl");
        let mut file = std::fs::File::create(&tmp).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();

        let world = load_prl(tmp.to_str().unwrap()).unwrap();

        assert_eq!(world.face_meta.len(), 1);
        assert_eq!(world.face_meta[0].texture_index, None);
        assert_eq!(world.face_meta[0].texture_name, "");
        assert_eq!(world.face_meta[0].material, Material::Default);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_geometry_v2_preferred_over_legacy() {
        // When both Geometry and GeometryV2 are present, GeometryV2 wins.
        let v1_geom = sample_geometry();
        let v2_geom = sample_geometry_v2();
        let tex_names = sample_texture_names();

        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: v1_geom.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::GeometryV2 as u32,
                version: 1,
                data: v2_geom.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::TextureNames as u32,
                version: 1,
                data: tex_names.to_bytes(),
            },
        ];

        let tmp = std::env::temp_dir().join("postretro_test_v2_preferred.prl");
        let mut file = std::fs::File::create(&tmp).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();

        let world = load_prl(tmp.to_str().unwrap()).unwrap();

        // Should have texture names from V2 path, not zero UVs from legacy.
        assert_eq!(world.texture_names.len(), 2);
        assert!(world.face_meta.iter().any(|f| f.texture_index.is_some()));

        std::fs::remove_file(&tmp).ok();
    }
}
