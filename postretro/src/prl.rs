// PRL level loading: read .prl files, produce BSP tree + BVH runtime data.
// See: context/lib/build_pipeline.md §PRL
// See: context/plans/in-progress/bvh-foundation/2-runtime-bvh.md

use std::collections::HashSet;
use std::path::Path;

use glam::Vec3;
use postretro_level_format::alpha_lights::{AlphaFalloffModel, AlphaLightType, AlphaLightsSection};
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::bsp::{BspLeavesSection, BspNodesSection};
use postretro_level_format::bvh::{BVH_NODE_FLAG_LEAF, BvhSection};
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::geometry::{GeometrySection, NO_TEXTURE};
use postretro_level_format::leaf_pvs::LeafPvsSection;
use postretro_level_format::light_influence::LightInfluenceSection;
use postretro_level_format::light_tags::LightTagsSection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::sh_volume::ShVolumeSection;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_level_format::visibility::decompress_pvs;
use postretro_level_format::{self as prl_format, SectionId};
use thiserror::Error;

use crate::geometry::{BvhLeaf, BvhNode, BvhTree, WorldVertex};
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
    #[error(
        "PRL file has no BVH section — pre-BVH maps are not supported; recompile with `prl-build`"
    )]
    NoBvh,
}

/// Per-face draw metadata. Face → index-range mapping now lives on BVH
/// leaves; `FaceMeta` only carries the per-face attributes that downstream
/// CPU code still needs (texture name, cell id, material class). These
/// fields are not read yet — they stay around for the lighting baker and
/// editor diagnostics slated for Milestone 5+.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FaceMeta {
    /// BSP leaf index this face belongs to (also the runtime cell id).
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
    /// Decompressed PVS: `pvs[i]` = leaf `i` is visible from this leaf.
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

/// Runtime shape discriminant for engine-side lights. Mirrors
/// `postretro-level-compiler::map_data::LightType` at the wire boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightType {
    Point,
    Spot,
    Directional,
}

/// Runtime falloff discriminant. Mirrors
/// `postretro-level-compiler::map_data::FalloffModel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FalloffModel {
    Linear,
    InverseDistance,
    InverseSquared,
}

/// Engine-side light loaded from the interim AlphaLights PRL section (ID 18).
///
/// **INTERIM** — this type and the AlphaLights section it comes from will be
/// replaced by an entity-system serialisation in Milestone 6+. Sub-plan 3 of
/// the Lighting Foundation plan uploads these to the direct-lighting GPU
/// buffer; sub-plan 1 only guarantees parsing round-trips cleanly.
/// See `context/plans/in-progress/lighting-foundation/1-fgd-canonical.md`.
#[derive(Debug, Clone, PartialEq)]
pub struct MapLight {
    pub origin: [f64; 3],
    pub light_type: LightType,
    pub intensity: f32,
    pub color: [f32; 3],
    pub falloff_model: FalloffModel,
    pub falloff_range: f32,
    pub cone_angle_inner: f32,
    pub cone_angle_outer: f32,
    pub cone_direction: [f32; 3],
    /// Reserved for sub-plan 4 (shadow maps). The direct-lighting path
    /// built in sub-plan 3 does not consume this; the shader's
    /// `shadow_info` slot is zeroed at upload time.
    pub cast_shadows: bool,
    /// Runtime counterpart of the compiler's `MapLight.is_dynamic`. The
    /// AlphaLights wire format does not yet carry this flag — it's always
    /// `false` at load time until `lighting-dynamic-flag/` plumbs it
    /// through. `pack_spec_lights` filters on `!is_dynamic` so that when
    /// the field goes live, dynamic lights stop appearing in the static
    /// spec buffer (they are already driven by the dynamic `GpuLight`
    /// loop).
    pub is_dynamic: bool,
    /// Optional author-supplied script tag loaded from the `LightTags`
    /// section (ID 26). Consumed by the scripting bridge at level load to
    /// populate the entity registry's tag column so scripts can call
    /// `world.query({ component: "light", tag: "<tag>" })`.
    pub tag: Option<String>,
}

/// BSP tree + BVH level data loaded from a .prl file.
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
    /// Global BVH loaded from the `Bvh` section. Always present — the loader
    /// rejects files that lack a BVH section.
    pub bvh: BvhTree,
    /// Lights loaded from the interim AlphaLights section (ID 18). Empty
    /// `Vec` if the section is absent (e.g. maps compiled before this
    /// milestone). Consumed by the renderer's direct-lighting path.
    pub lights: Vec<MapLight>,
    /// Per-light influence volumes loaded from the LightInfluence section
    /// (ID 21). Index `i` corresponds to `lights[i]`. Empty if the section
    /// is absent — the renderer treats all lights as infinite-bound.
    pub light_influences: Vec<crate::lighting::influence::LightInfluence>,
    /// Baked SH L2 irradiance volume loaded from the ShVolume section
    /// (ID 20). `None` for maps without baked indirect — the renderer
    /// degrades to `ambient_floor + direct_sum`.
    pub sh_volume: Option<ShVolumeSection>,
    /// Baked directional lightmap atlas loaded from the Lightmap section
    /// (ID 22). `None` for maps without baked direct — the renderer binds a
    /// 1×1 white placeholder and bumped-Lambert degrades to flat white.
    pub lightmap: Option<LightmapSection>,
    /// Chunk light list (ID 23). `None` for maps compiled before Task A of
    /// `lighting-chunk-lists/` — the runtime falls back to iterating the
    /// full spec buffer. See `chunk_light_list::ChunkGrid::fallback`.
    pub chunk_light_list: Option<ChunkLightListSection>,
    /// Per-face animated-light chunks (ID 24). Produced by the
    /// `animated-light-chunks/` plan; consumed by the weight-map compose
    /// pass to cross-check `AnimatedLightWeightMaps.chunk_rects.len()`.
    pub animated_light_chunks: Option<AnimatedLightChunksSection>,
    /// Per-chunk atlas rectangles + per-texel weight lists (ID 25). Baked
    /// by the `animated-light-weight-maps/` plan. `None` when the map has
    /// no animated lights — the renderer falls back to a 1×1 zero atlas
    /// for the animated-contribution slot.
    pub animated_light_weight_maps: Option<AnimatedLightWeightMapsSection>,
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

/// Decode a PRL sentinel-encoded child reference.
fn decode_child(value: i32) -> BspChild {
    if value >= 0 {
        BspChild::Node(value as usize)
    } else {
        BspChild::Leaf((-1 - value) as usize)
    }
}

fn convert_alpha_lights(section: AlphaLightsSection) -> Vec<MapLight> {
    section
        .lights
        .into_iter()
        .map(|r| {
            let light_type = match r.light_type {
                AlphaLightType::Point => LightType::Point,
                AlphaLightType::Spot => LightType::Spot,
                AlphaLightType::Directional => LightType::Directional,
            };
            let falloff_model = match r.falloff_model {
                AlphaFalloffModel::Linear => FalloffModel::Linear,
                AlphaFalloffModel::InverseDistance => FalloffModel::InverseDistance,
                AlphaFalloffModel::InverseSquared => FalloffModel::InverseSquared,
            };
            MapLight {
                origin: r.origin,
                light_type,
                intensity: r.intensity,
                color: r.color,
                falloff_model,
                falloff_range: r.falloff_range,
                cone_angle_inner: r.cone_angle_inner,
                cone_angle_outer: r.cone_angle_outer,
                cone_direction: r.cone_direction,
                cast_shadows: r.cast_shadows,
                is_dynamic: false,
                tag: None,
            }
        })
        .collect()
}

fn convert_bvh_section(section: BvhSection) -> BvhTree {
    let nodes = section
        .nodes
        .into_iter()
        .map(|n| BvhNode {
            aabb_min: n.aabb_min,
            skip_index: n.skip_index,
            aabb_max: n.aabb_max,
            left_child_or_leaf_index: n.left_child_or_leaf_index,
            flags: n.flags,
        })
        .collect();

    let leaves = section
        .leaves
        .into_iter()
        .map(|l| BvhLeaf {
            aabb_min: l.aabb_min,
            material_bucket_id: l.material_bucket_id,
            aabb_max: l.aabb_max,
            index_offset: l.index_offset,
            index_count: l.index_count,
            cell_id: l.cell_id,
            chunk_range_start: l.chunk_range_start,
            chunk_range_count: l.chunk_range_count,
        })
        .collect();

    BvhTree {
        nodes,
        leaves,
        root_node_index: section.root_node_index,
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

    // Geometry section — the only supported on-disk geometry shape. Pre-BVH
    // maps are rejected outright (see `NoBvh` below).
    let geom_data = prl_format::read_section_data(&mut cursor, &meta, SectionId::Geometry as u32)?
        .ok_or(PrlLoadError::NoGeometry)?;
    let geom = GeometrySection::from_bytes(&geom_data)?;

    // TextureNames section (optional).
    let texture_names_data =
        prl_format::read_section_data(&mut cursor, &meta, SectionId::TextureNames as u32)?;
    let texture_names_section = match texture_names_data {
        Some(data) => Some(TextureNamesSection::from_bytes(&data)?),
        None => None,
    };
    let texture_names: Vec<String> = texture_names_section.map(|s| s.names).unwrap_or_default();

    // Build vertices and face_meta.
    let mut warned_prefixes = HashSet::new();
    let vertices: Vec<WorldVertex> = geom
        .vertices
        .iter()
        .map(|v| WorldVertex {
            position: v.position,
            // Store raw texel-space UVs; normalized in main.rs after texture
            // dimensions are known.
            base_uv: v.uv,
            normal_oct: v.normal_oct,
            tangent_packed: v.tangent_packed,
            lightmap_uv: v.lightmap_uv,
        })
        .collect();

    let face_meta: Vec<FaceMeta> = geom
        .faces
        .iter()
        .map(|f| {
            let (tex_idx, tex_name) = if f.texture_index == NO_TEXTURE {
                (None, String::new())
            } else {
                let name = texture_names
                    .get(f.texture_index as usize)
                    .cloned()
                    .unwrap_or_default();
                (Some(f.texture_index), name)
            };
            let mat = material::derive_material(&tex_name, &mut warned_prefixes);
            FaceMeta {
                leaf_index: f.leaf_index,
                texture_index: tex_idx,
                texture_dimensions: (64, 64),
                texture_name: tex_name,
                material: mat,
            }
        })
        .collect();

    let indices = geom.indices;

    log::info!(
        "[PRL] Geometry: {} vertices, {} indices, {} faces, {} textures referenced",
        vertices.len(),
        indices.len(),
        face_meta.len(),
        texture_names.len()
    );

    // BVH section — required. Pre-BVH maps lack this section and cannot be
    // loaded; rebuild them with `prl-build`.
    let bvh_data = prl_format::read_section_data(&mut cursor, &meta, SectionId::Bvh as u32)?
        .ok_or(PrlLoadError::NoBvh)?;
    let bvh_section = BvhSection::from_bytes(&bvh_data)?;
    let bvh = convert_bvh_section(bvh_section);
    log::info!(
        "[PRL] BVH: {} nodes, {} leaves, root={}",
        bvh.nodes.len(),
        bvh.leaves.len(),
        bvh.root_node_index,
    );
    debug_assert!(
        bvh.leaves
            .windows(2)
            .all(|w| w[0].material_bucket_id <= w[1].material_bucket_id),
        "BVH leaves must be sorted by material_bucket_id",
    );
    debug_assert!(
        bvh.nodes.is_empty() || (bvh.root_node_index as usize) < bvh.nodes.len(),
        "BVH root_node_index {} out of range for {} nodes",
        bvh.root_node_index,
        bvh.nodes.len(),
    );
    // Flag-bit sanity: every node's flags must be either clean (internal) or
    // exactly the leaf bit — the compiler doesn't use the reserved bits yet.
    debug_assert!(
        bvh.nodes
            .iter()
            .all(|n| n.flags == 0 || n.flags == BVH_NODE_FLAG_LEAF),
        "BVH nodes carry unexpected flag bits",
    );

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

    // AlphaLights section (optional). Missing for maps compiled before the
    // Lighting Foundation milestone — fall back to an empty light list with
    // a warning so older maps still load.
    let mut lights: Vec<MapLight> = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::AlphaLights as u32,
    )? {
        Some(data) => {
            let section = AlphaLightsSection::from_bytes(&data)?;
            let count = section.lights.len();
            let converted = convert_alpha_lights(section);
            log::info!("[PRL] AlphaLights: {count} lights loaded");
            converted
        }
        None => {
            log::warn!(
                "[PRL] AlphaLights section missing — map predates the lighting foundation milestone; recompile with `prl-build` for lights to appear"
            );
            Vec::new()
        }
    };

    // LightTags section (optional). Entries correspond 1:1 with AlphaLights
    // records in the same order; a mismatch is a format error. Absence means
    // no light carries a tag.
    if let Some(data) =
        prl_format::read_section_data(&mut cursor, &meta, SectionId::LightTags as u32)?
    {
        let section = LightTagsSection::from_bytes(&data)?;
        if section.tags.len() != lights.len() {
            return Err(PrlLoadError::FormatError(prl_format::FormatError::Io(
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "LightTags count ({}) does not match AlphaLights count ({})",
                        section.tags.len(),
                        lights.len()
                    ),
                ),
            )));
        }
        let mut tagged = 0usize;
        for (light, tag) in lights.iter_mut().zip(section.tags.into_iter()) {
            if !tag.is_empty() {
                tagged += 1;
                light.tag = Some(tag);
            }
        }
        log::info!("[PRL] LightTags: {tagged} tagged lights");
    }

    // LightInfluence section (optional). Missing for maps compiled before
    // sub-plan 4 — fall back to empty (all lights treated as infinite-bound).
    let light_influences: Vec<crate::lighting::influence::LightInfluence> =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::LightInfluence as u32)? {
            Some(data) => {
                let section = LightInfluenceSection::from_bytes(&data)?;
                if section.records.len() != lights.len() {
                    return Err(PrlLoadError::FormatError(prl_format::FormatError::Io(
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!(
                                "LightInfluence record count ({}) does not match AlphaLights count ({})",
                                section.records.len(),
                                lights.len()
                            ),
                        ),
                    )));
                }
                let converted: Vec<_> = section
                    .records
                    .into_iter()
                    .map(|r| crate::lighting::influence::LightInfluence {
                        center: glam::Vec3::from(r.center),
                        radius: r.radius,
                    })
                    .collect();
                log::info!("[PRL] LightInfluence: {} records loaded", converted.len());
                converted
            }
            None => {
                log::warn!("[Loader] LightInfluence section missing, no spatial culling this map");
                Vec::new()
            }
        };

    // ShVolume section (optional). Missing for maps compiled before sub-plan 2
    // — the renderer falls back to `ambient_floor + direct_sum`.
    let sh_volume: Option<ShVolumeSection> =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::ShVolume as u32)? {
            Some(data) => {
                let section = ShVolumeSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] ShVolume: {}×{}×{} grid ({} probes, {} animated layers)",
                    section.grid_dimensions[0],
                    section.grid_dimensions[1],
                    section.grid_dimensions[2],
                    section.probes.len(),
                    section.animation_descriptors.len(),
                );
                Some(section)
            }
            None => {
                log::warn!(
                    "[PRL] ShVolume section missing — indirect lighting disabled for this map"
                );
                None
            }
        };

    // Lightmap section (optional). Missing for maps compiled before the
    // directional-lightmap plan shipped — the renderer falls back to a 1×1
    // white placeholder and bumped-Lambert degrades to flat white.
    let lightmap: Option<LightmapSection> =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::Lightmap as u32)? {
            Some(data) => {
                let section = LightmapSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] Lightmap: {}x{} atlas ({} B irradiance, {} B direction)",
                    section.width,
                    section.height,
                    section.irradiance.len(),
                    section.direction.len(),
                );
                Some(section)
            }
            None => {
                log::warn!(
                    "[PRL] Lightmap section missing — static direct lighting disabled for this map"
                );
                None
            }
        };

    // ChunkLightList section (optional). Missing for maps compiled before
    // Task A of `lighting-chunk-lists/` — the runtime falls back to iterating
    // the full spec-only light buffer.
    let chunk_light_list: Option<ChunkLightListSection> = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::ChunkLightList as u32,
    )? {
        Some(data) => {
            let section = ChunkLightListSection::from_bytes(&data)?;
            log::info!(
                "[PRL] ChunkLightList: {}×{}×{} grid, {} indices",
                section.grid_dimensions[0],
                section.grid_dimensions[1],
                section.grid_dimensions[2],
                section.light_indices.len(),
            );
            Some(section)
        }
        None => {
            log::info!(
                "[PRL] ChunkLightList section missing — specular path uses full-buffer fallback"
            );
            None
        }
    };

    // AnimatedLightChunks section (optional). Emitted by `prl-build` for maps
    // that carry animated lights. Used at runtime to cross-check the
    // weight-maps section count.
    let animated_light_chunks: Option<AnimatedLightChunksSection> =
        match prl_format::read_section_data(
            &mut cursor,
            &meta,
            SectionId::AnimatedLightChunks as u32,
        )? {
            Some(data) => {
                let section = AnimatedLightChunksSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] AnimatedLightChunks: {} chunks, {} flat indices",
                    section.chunks.len(),
                    section.light_indices.len(),
                );
                Some(section)
            }
            None => None,
        };

    // AnimatedLightWeightMaps section (optional). Missing for maps with zero
    // animated lights — the runtime falls back to a 1×1 zero atlas for the
    // animated-contribution slot on bind group 4.
    let animated_light_weight_maps: Option<AnimatedLightWeightMapsSection> =
        match prl_format::read_section_data(
            &mut cursor,
            &meta,
            SectionId::AnimatedLightWeightMaps as u32,
        )? {
            Some(data) => {
                let section = AnimatedLightWeightMapsSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] AnimatedLightWeightMaps: {} chunks, {} covered texels, {} weight entries",
                    section.chunk_rects.len(),
                    section.offset_counts.len(),
                    section.texel_lights.len(),
                );
                Some(section)
            }
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
                            vec![false; leaf_count]
                        }
                    } else {
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

    log::info!(
        "[PRL] Loaded: {} vertices, {} indices ({} triangles), {} faces, {} nodes, {} leaves, bvh=[{} nodes, {} leaves], pvs={}, portals={}, textures={}",
        vertices.len(),
        indices.len(),
        indices.len() / 3,
        face_meta.len(),
        nodes.len(),
        leaves.len(),
        bvh.nodes.len(),
        bvh.leaves.len(),
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
        bvh,
        lights,
        light_influences,
        sh_volume,
        lightmap,
        chunk_light_list,
        animated_light_chunks,
        animated_light_weight_maps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::bsp::{
        BspLeafRecord, BspLeavesSection, BspNodeRecord, BspNodesSection,
    };
    use postretro_level_format::bvh::{
        BvhLeaf as FormatBvhLeaf, BvhNode as FormatBvhNode, BvhSection,
    };
    use postretro_level_format::geometry::{FaceMeta as FormatFaceMeta, GeometrySection, Vertex};
    use postretro_level_format::leaf_pvs::LeafPvsSection;
    use postretro_level_format::visibility::compress_pvs;

    fn simple_face_meta() -> FaceMeta {
        FaceMeta {
            leaf_index: 0,
            texture_index: None,
            texture_dimensions: (64, 64),
            texture_name: String::new(),
            material: Material::Default,
        }
    }

    fn empty_bvh() -> BvhTree {
        BvhTree {
            nodes: vec![],
            leaves: vec![],
            root_node_index: 0,
        }
    }

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
            bvh: empty_bvh(),
            lights: vec![],
            light_influences: vec![],
            sh_volume: None,
            lightmap: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
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
            bvh: empty_bvh(),
            lights: vec![],
            light_influences: vec![],
            sh_volume: None,
            lightmap: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
        };
        assert_eq!(world.find_leaf(Vec3::new(50.0, 50.0, 50.0)), 0);
    }

    #[test]
    fn decode_child_positive_is_node() {
        assert_eq!(decode_child(0), BspChild::Node(0));
        assert_eq!(decode_child(5), BspChild::Node(5));
    }

    #[test]
    fn decode_child_negative_is_leaf() {
        assert_eq!(decode_child(-1), BspChild::Leaf(0));
        assert_eq!(decode_child(-6), BspChild::Leaf(5));
        assert_eq!(decode_child(-101), BspChild::Leaf(100));
    }

    #[test]
    fn spawn_position_centers_non_solid_leaves() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![simple_face_meta()],
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
            bvh: empty_bvh(),
            lights: vec![],
            light_influences: vec![],
            sh_volume: None,
            lightmap: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
        };

        let spawn = world.spawn_position();
        assert!((spawn - Vec3::splat(5.0)).length() < 0.01);
    }

    #[test]
    fn face_leaf_indices_maps_faces_to_leaves() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![simple_face_meta(), simple_face_meta(), simple_face_meta()],
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
            bvh: empty_bvh(),
            lights: vec![],
            light_influences: vec![],
            sh_volume: None,
            lightmap: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
        };

        let indices = face_leaf_indices(&world);
        assert_eq!(indices, vec![0, 0, 1]);
    }

    #[test]
    fn load_prl_missing_file_returns_file_not_found() {
        let result = load_prl("nonexistent/path/to/map.prl");
        assert!(matches!(result.unwrap_err(), PrlLoadError::FileNotFound(_)));
    }

    // --- Round-trip helpers ---

    fn sample_vertex(x: f32) -> Vertex {
        Vertex::new(
            [x, 0.0, 0.0],
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
        )
    }

    fn sample_geometry() -> GeometrySection {
        GeometrySection {
            vertices: vec![
                sample_vertex(0.0),
                sample_vertex(1.0),
                sample_vertex(1.5),
                sample_vertex(10.0),
                sample_vertex(11.0),
                sample_vertex(11.5),
            ],
            indices: vec![0, 1, 2, 3, 4, 5],
            faces: vec![
                FormatFaceMeta {
                    leaf_index: 0,
                    texture_index: NO_TEXTURE,
                },
                FormatFaceMeta {
                    leaf_index: 1,
                    texture_index: NO_TEXTURE,
                },
            ],
        }
    }

    fn sample_bvh_section() -> BvhSection {
        // Minimal valid BVH: one internal root + two leaves.
        BvhSection {
            nodes: vec![
                FormatBvhNode {
                    aabb_min: [0.0, 0.0, 0.0],
                    skip_index: 3,
                    aabb_max: [12.0, 2.0, 2.0],
                    left_child_or_leaf_index: 0,
                    flags: 0,
                    _padding: 0,
                },
                FormatBvhNode {
                    aabb_min: [0.0, 0.0, 0.0],
                    skip_index: 2,
                    aabb_max: [2.0, 2.0, 2.0],
                    left_child_or_leaf_index: 0,
                    flags: BVH_NODE_FLAG_LEAF,
                    _padding: 0,
                },
                FormatBvhNode {
                    aabb_min: [9.0, 0.0, 0.0],
                    skip_index: 3,
                    aabb_max: [12.0, 2.0, 2.0],
                    left_child_or_leaf_index: 1,
                    flags: BVH_NODE_FLAG_LEAF,
                    _padding: 0,
                },
            ],
            leaves: vec![
                FormatBvhLeaf {
                    aabb_min: [0.0, 0.0, 0.0],
                    material_bucket_id: 0,
                    aabb_max: [2.0, 2.0, 2.0],
                    index_offset: 0,
                    index_count: 3,
                    cell_id: 0,
                    chunk_range_start: 0,
                    chunk_range_count: 0,
                },
                FormatBvhLeaf {
                    aabb_min: [9.0, 0.0, 0.0],
                    material_bucket_id: 0,
                    aabb_max: [12.0, 2.0, 2.0],
                    index_offset: 3,
                    index_count: 3,
                    cell_id: 1,
                    chunk_range_start: 0,
                    chunk_range_count: 0,
                },
            ],
            root_node_index: 0,
        }
    }

    fn write_prl_fixture(sections: Vec<prl_format::SectionBlob>, name: &str) -> std::path::PathBuf {
        let tmp = std::env::temp_dir().join(name);
        let mut file = std::fs::File::create(&tmp).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();
        tmp
    }

    #[test]
    fn load_prl_round_trip_with_bsp_sections() {
        let geom = sample_geometry();
        let bvh = sample_bvh_section();

        let nodes = BspNodesSection {
            nodes: vec![BspNodeRecord {
                plane_normal: [1.0, 0.0, 0.0],
                plane_distance: 5.0,
                front: -1,
                back: -2,
            }],
        };

        let pvs_uncompressed = vec![0b0000_0011u8];
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
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
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

        let tmp = write_prl_fixture(sections, "postretro_test_bvh_round_trip.prl");
        let world = load_prl(tmp.to_str().unwrap()).unwrap();

        assert_eq!(world.vertices.len(), 6);
        assert_eq!(world.indices.len(), 6);
        assert_eq!(world.face_meta.len(), 2);
        assert_eq!(world.nodes.len(), 1);
        assert_eq!(world.leaves.len(), 2);
        assert_eq!(world.bvh.nodes.len(), 3);
        assert_eq!(world.bvh.leaves.len(), 2);
        assert!(world.has_pvs);
        assert_eq!(world.root, BspChild::Node(0));
        assert_eq!(world.find_leaf(Vec3::new(10.0, 0.0, 0.0)), 0);
        assert_eq!(world.find_leaf(Vec3::new(0.0, 0.0, 0.0)), 1);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_missing_bvh_section() {
        let geom = sample_geometry();
        let sections = vec![prl_format::SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: geom.to_bytes(),
        }];
        let tmp = write_prl_fixture(sections, "postretro_test_missing_bvh.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(matches!(err, PrlLoadError::NoBvh), "got {err:?}");
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_invalid_magic_produces_clear_error() {
        let tmp = std::env::temp_dir().join("postretro_test_bad_magic.prl");
        std::fs::write(&tmp, b"NOPE extra data for length").unwrap();
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("magic"));
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_truncated_file_produces_clear_error() {
        let tmp = std::env::temp_dir().join("postretro_test_truncated.prl");
        std::fs::write(&tmp, [0x50, 0x52, 0x4C]).unwrap();
        assert!(load_prl(tmp.to_str().unwrap()).is_err());
        std::fs::remove_file(&tmp).ok();
    }

    fn sample_alpha_lights() -> AlphaLightsSection {
        use postretro_level_format::alpha_lights::AlphaLightRecord;
        AlphaLightsSection {
            lights: vec![
                AlphaLightRecord {
                    origin: [1.0, 2.0, 3.0],
                    light_type: AlphaLightType::Point,
                    intensity: 300.0,
                    color: [1.0, 0.8, 0.5],
                    falloff_model: AlphaFalloffModel::InverseSquared,
                    falloff_range: 50.0,
                    cone_angle_inner: 0.0,
                    cone_angle_outer: 0.0,
                    cone_direction: [0.0, 0.0, 0.0],
                    cast_shadows: true,
                },
                AlphaLightRecord {
                    origin: [-4.0, 5.5, 6.0],
                    light_type: AlphaLightType::Spot,
                    intensity: 220.0,
                    color: [0.7, 0.9, 1.0],
                    falloff_model: AlphaFalloffModel::Linear,
                    falloff_range: 25.0,
                    cone_angle_inner: std::f32::consts::FRAC_PI_6,
                    cone_angle_outer: std::f32::consts::FRAC_PI_4,
                    cone_direction: [0.0, -1.0, 0.0],
                    cast_shadows: false,
                },
                AlphaLightRecord {
                    origin: [0.0, 10.0, 0.0],
                    light_type: AlphaLightType::Directional,
                    intensity: 180.0,
                    color: [0.9, 0.95, 1.0],
                    falloff_model: AlphaFalloffModel::Linear,
                    falloff_range: 0.0,
                    cone_angle_inner: 0.0,
                    cone_angle_outer: 0.0,
                    cone_direction: [0.0, -0.70710677, -0.70710677],
                    cast_shadows: true,
                },
            ],
        }
    }

    #[test]
    fn load_prl_parses_alpha_lights_section() {
        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        let alpha_lights = sample_alpha_lights();

        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geom.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::AlphaLights as u32,
                version: 1,
                data: alpha_lights.to_bytes(),
            },
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_alpha_lights.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("should load");

        assert_eq!(world.lights.len(), 3);

        assert_eq!(world.lights[0].light_type, LightType::Point);
        assert_eq!(world.lights[0].origin, [1.0, 2.0, 3.0]);
        assert_eq!(world.lights[0].intensity, 300.0);
        assert_eq!(world.lights[0].falloff_model, FalloffModel::InverseSquared);
        assert!((world.lights[0].falloff_range - 50.0).abs() < 1e-5);
        assert!(world.lights[0].cast_shadows);

        assert_eq!(world.lights[1].light_type, LightType::Spot);
        assert_eq!(world.lights[1].falloff_model, FalloffModel::Linear);
        assert!((world.lights[1].cone_angle_inner - std::f32::consts::FRAC_PI_6).abs() < 1e-4);
        assert!((world.lights[1].cone_angle_outer - std::f32::consts::FRAC_PI_4).abs() < 1e-4);
        assert_eq!(world.lights[1].cone_direction, [0.0, -1.0, 0.0]);
        assert!(!world.lights[1].cast_shadows);

        assert_eq!(world.lights[2].light_type, LightType::Directional);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_absent_light_influence_falls_back_to_empty() {
        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        let alpha_lights = sample_alpha_lights();

        // AlphaLights present, LightInfluence absent — should warn but load.
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geom.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::AlphaLights as u32,
                version: 1,
                data: alpha_lights.to_bytes(),
            },
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_no_light_influence.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("should load");

        assert_eq!(world.lights.len(), 3, "lights should still parse");
        assert!(
            world.light_influences.is_empty(),
            "missing LightInfluence section should give empty vec"
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_light_influence_count_mismatch_is_error() {
        use postretro_level_format::light_influence::{InfluenceRecord, LightInfluenceSection};

        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        let alpha_lights = sample_alpha_lights(); // 3 lights

        // Only 2 influence records — mismatch.
        let influence = LightInfluenceSection {
            records: vec![
                InfluenceRecord {
                    center: [1.0, 2.0, 3.0],
                    radius: 50.0,
                },
                InfluenceRecord {
                    center: [-4.0, 5.5, 6.0],
                    radius: 25.0,
                },
            ],
        };

        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geom.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::AlphaLights as u32,
                version: 1,
                data: alpha_lights.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::LightInfluence as u32,
                version: 1,
                data: influence.to_bytes(),
            },
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_influence_mismatch.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("does not match"),
            "expected count mismatch error, got: {msg}"
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_falls_back_to_empty_lights_when_section_absent() {
        let geom = sample_geometry();
        let bvh = sample_bvh_section();

        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geom.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
            },
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_no_alpha_lights.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("should load");
        assert!(world.lights.is_empty());

        std::fs::remove_file(&tmp).ok();
    }
}
