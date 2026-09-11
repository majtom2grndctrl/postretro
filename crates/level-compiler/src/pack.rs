// Pack and write: serialize sections to .prl binary, validate via read-back.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs4::FileExt;
use glam::Vec3;
use postretro_level_format::alpha_lights::{
    ALPHA_LIGHT_LEAF_UNASSIGNED, AlphaFalloffModel, AlphaLightRecord, AlphaLightType,
    AlphaLightsSection, AlphaShadowType,
};
use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::{
    AnimatedBillboardDirectScatterDeltaVolumesSection,
    MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
};
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection;
use postretro_level_format::bsp::BspLeavesSection;
use postretro_level_format::bvh::BvhSection;
use postretro_level_format::cell_draw_index::CellDrawIndexSection;
use postretro_level_format::cell_locator::{
    CellLocatorChild, CellLocatorNodeRecord, CellLocatorSection,
};
use postretro_level_format::cell_visibility::CellVisibilitySection;
use postretro_level_format::cells::{
    CELL_FLAG_DRAWABLE, CELL_FLAG_EXTERIOR, CELL_FLAG_SOLID, CellRecord, CellsSection,
};
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::data_script::DataScriptSection;
use postretro_level_format::delta_sh_volumes::{AFFINITY_FACTOR, DeltaShVolumesSection};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::fog_cell_masks::FogCellMasksSection;
use postretro_level_format::fog_volumes::{FogVolumeRecord, FogVolumesSection};
use postretro_level_format::kinematic_geometry::KinematicGeometrySection;
use postretro_level_format::light_influence::{InfluenceRecord, LightInfluenceSection};
use postretro_level_format::light_tags::LightTagsSection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::map_entity::{MapEntityRecord, MapEntitySection};
use postretro_level_format::navmesh::NavMeshSection;
use postretro_level_format::portals::{PortalRecord, PortalsSection};
use postretro_level_format::sdf_atlas::SdfAtlasSection;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::trigger_volumes::TriggerVolumesSection;
use postretro_level_format::{
    SectionDescriptor, SectionId, read_container, read_section_data, write_prl_header_and_table,
};
use same_file::Handle as FileIdentity;
use tempfile::{Builder as TempFileBuilder, NamedTempFile};

use std::collections::{HashMap, HashSet};

use crate::geometry::GeometryResult;
use crate::light_namespaces::AlphaLightsNs;
use crate::map_data::{FalloffModel, LightType, ShadowType};
use crate::partition::{BspChild, BspTree, find_leaf_for_point};
use crate::portals::Portal;

// PRL table and NavMesh body versions are independent domains.
const NAVMESH_CONTAINER_VERSION: u16 = 1;

// Windows denies an atomic replacement while an indexer, sync client, or reader
// temporarily holds the destination without delete sharing. Keep the retry brief:
// a persistent lock or ACL problem still fails and discards the staged PRL.
const WINDOWS_PUBLISH_MAX_RETRIES: u32 = 5;
const WINDOWS_PUBLISH_RETRY_BASE_DELAY: Duration = Duration::from_millis(50);
const WINDOWS_ERROR_ACCESS_DENIED: i32 = 5;
const WINDOWS_ERROR_SHARING_VIOLATION: i32 = 32;
const WINDOWS_ERROR_LOCK_VIOLATION: i32 = 33;

#[path = "pack_sections.rs"]
mod pack_sections;

use pack_sections::bvh_with_chunk_ranges;

type SectionEncoder<'a> = Box<dyn FnOnce() -> anyhow::Result<Vec<u8>> + 'a>;

struct PlannedSection<'a> {
    descriptor: SectionDescriptor,
    encode: SectionEncoder<'a>,
}

impl<'a> PlannedSection<'a> {
    fn new(
        section_id: u32,
        version: u16,
        byte_len: usize,
        encode: impl FnOnce() -> anyhow::Result<Vec<u8>> + 'a,
    ) -> Self {
        Self {
            descriptor: SectionDescriptor {
                section_id,
                version,
                byte_len: byte_len as u64,
            },
            encode: Box::new(encode),
        }
    }
}

fn scatter_section_fits_pack_cap(
    section: &AnimatedBillboardDirectScatterDeltaVolumesSection,
) -> bool {
    scatter_section_fits_pack_cap_with_limit(
        section,
        MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
    )
}

fn scatter_section_fits_pack_cap_with_limit(
    section: &AnimatedBillboardDirectScatterDeltaVolumesSection,
    max_encoded_bytes: u64,
) -> bool {
    section
        .encoded_len()
        .is_some_and(|bytes| bytes <= max_encoded_bytes)
}

/// Convert translated map lights into an `AlphaLightsSection` for the format
/// crate. Strips animation curves; the direct lighting path uses the static
/// base properties only.
pub fn encode_alpha_lights(lights: &AlphaLightsNs<'_>, tree: &BspTree) -> AlphaLightsSection {
    let records: Vec<AlphaLightRecord> = lights
        .entries()
        .iter()
        .map(|entry| {
            let src_index = entry.source_index;
            let l = entry.light;
            let light_type = match l.light_type {
                LightType::Point => AlphaLightType::Point,
                LightType::Spot => AlphaLightType::Spot,
                LightType::Directional => AlphaLightType::Directional,
            };
            let falloff_model = match l.falloff_model {
                FalloffModel::Linear => AlphaFalloffModel::Linear,
                FalloffModel::InverseDistance => AlphaFalloffModel::InverseDistance,
                FalloffModel::InverseSquared => AlphaFalloffModel::InverseSquared,
            };

            let leaf_index = if tree.leaves.is_empty() {
                ALPHA_LIGHT_LEAF_UNASSIGNED
            } else {
                let idx = find_leaf_for_point(tree, l.origin);
                if tree.leaves[idx].is_solid {
                    log::warn!(
                        "[Compiler] AlphaLights: light {src_index} at origin ({:.3}, {:.3}, {:.3}) is inside a solid leaf; marking unassigned",
                        l.origin.x,
                        l.origin.y,
                        l.origin.z,
                    );
                    ALPHA_LIGHT_LEAF_UNASSIGNED
                } else {
                    idx as u32
                }
            };

            AlphaLightRecord {
                origin: [l.origin.x, l.origin.y, l.origin.z],
                light_type,
                intensity: l.intensity,
                color: l.color,
                falloff_model,
                falloff_range: l.falloff_range,
                cone_angle_inner: l.cone_angle_inner.unwrap_or(0.0),
                cone_angle_outer: l.cone_angle_outer.unwrap_or(0.0),
                cone_direction: l.cone_direction.unwrap_or([0.0, 0.0, 0.0]),
                is_dynamic: l.is_dynamic,
                casts_entity_shadows: l.casts_entity_shadows,
                leaf_index,
                shadow_type: match l.shadow_type {
                    ShadowType::StaticLightMap => AlphaShadowType::StaticLightMap,
                    ShadowType::Sdf => AlphaShadowType::Sdf,
                },
            }
        })
        .collect();

    AlphaLightsSection { lights: records }
}

/// Encode per-light script tags, aligned with the AlphaLights record order.
/// Returns `None` when no light in the AlphaLights namespace carries a tag —
/// the caller omits the section entirely in that case so tag-less maps add
/// zero bytes.
pub fn encode_light_tags(lights: &AlphaLightsNs<'_>) -> Option<LightTagsSection> {
    if lights.entries().iter().all(|e| e.light.tags.is_empty()) {
        return None;
    }
    let tags = lights
        .entries()
        .iter()
        .map(|e| e.light.tags.join(" "))
        .collect();
    Some(LightTagsSection { tags })
}

/// Derive influence records from the AlphaLights namespace. Iteration order
/// matches AlphaLights — record `i` here corresponds to light `i` there.
pub fn encode_light_influence(lights: &AlphaLightsNs<'_>) -> LightInfluenceSection {
    let records = lights
        .entries()
        .iter()
        .map(|e| {
            let l = e.light;
            let (center, radius) = match l.light_type {
                LightType::Directional => ([0.0f32, 0.0, 0.0], f32::MAX),
                LightType::Point | LightType::Spot => {
                    let cx = l.origin.x as f32;
                    let cy = l.origin.y as f32;
                    let cz = l.origin.z as f32;
                    ([cx, cy, cz], l.falloff_range)
                }
            };
            InfluenceRecord { center, radius }
        })
        .collect();

    LightInfluenceSection { records }
}

pub(crate) fn direct_sh_delta_covers_selection(
    section: &DirectShDeltaVolumesSection,
    selected_light_count: usize,
) -> bool {
    if selected_light_count == 0 {
        return false;
    }

    let mut seen = vec![false; selected_light_count];
    for &selection_index in &section.affinity_lights {
        let Some(slot) = seen.get_mut(selection_index as usize) else {
            return false;
        };
        *slot = true;
    }

    seen.into_iter().all(|has_delta| has_delta)
}

pub(crate) fn direct_sh_delta_has_valid_csr_shape(section: &DirectShDeltaVolumesSection) -> bool {
    let Some(affinity_cell_count) = (section.affinity_dims[0] as usize)
        .checked_mul(section.affinity_dims[1] as usize)
        .and_then(|n| n.checked_mul(section.affinity_dims[2] as usize))
    else {
        return false;
    };
    let Some(expected_offsets_len) = affinity_cell_count.checked_add(1) else {
        return false;
    };
    if section.affinity_offsets.len() != expected_offsets_len {
        return false;
    }
    if section.affinity_offsets.first().copied() != Some(0) {
        return false;
    }
    if !section
        .affinity_offsets
        .windows(2)
        .all(|window| window[0] <= window[1])
    {
        return false;
    }
    if section
        .affinity_offsets
        .last()
        .and_then(|&offset| usize::try_from(offset).ok())
        != Some(section.affinity_lights.len())
    {
        return false;
    }

    section.expected_delta_subblock_f16_count() == Some(section.delta_subblocks.len())
}

pub(crate) fn direct_sh_delta_is_usable_for_selection(
    section: &DirectShDeltaVolumesSection,
    direct: &DirectShVolumeSection,
    selected_light_count: usize,
) -> bool {
    let expected_affinity_dims = [
        direct.grid_dimensions[0].div_ceil(AFFINITY_FACTOR as u32),
        direct.grid_dimensions[1].div_ceil(AFFINITY_FACTOR as u32),
        direct.grid_dimensions[2].div_ceil(AFFINITY_FACTOR as u32),
    ];

    section.affinity_factor == AFFINITY_FACTOR
        && section.affinity_dims == expected_affinity_dims
        && section.tile_dimension == direct.tile_dimension
        && section.tile_border == direct.tile_border
        && direct_sh_delta_has_valid_csr_shape(section)
        && direct_sh_delta_covers_selection(section, selected_light_count)
}

/// Encode the collected non-light, non-worldspawn map entities into a
/// `MapEntitySection` for the runtime classname dispatch. Returns `None` when
/// the map carries no such entities — the caller omits the section so empty
/// maps add zero bytes.
///
/// Origin is narrowed from `f64` (compiler precision) to `f32` (engine /
/// runtime precision) at this boundary; angles are already engine-convention
/// `f32` from the format adapter.
pub fn encode_map_entities(
    entities: &[crate::map_data::MapEntityRecord],
) -> Option<MapEntitySection> {
    if entities.is_empty() {
        return None;
    }
    let entries = entities
        .iter()
        .map(|e| MapEntityRecord {
            classname: e.classname.clone(),
            origin: [e.origin.x as f32, e.origin.y as f32, e.origin.z as f32],
            angles: e.angles,
            key_values: e.key_values.clone(),
            tags: e.tags.clone(),
        })
        .collect();
    Some(MapEntitySection { entries })
}

/// Encode the resolved fog volume entities and the worldspawn-scoped scalars
/// (`fog_pixel_scale` and `initial_gravity`) into a `FogVolumesSection`.
/// Always produces a section so the worldspawn data is honoured at runtime,
/// even when the map carries no fog brushes.
pub fn encode_fog_volumes(
    fog_volumes: &[crate::map_data::MapFogVolume],
    fog_pixel_scale: u32,
    initial_gravity: f32,
) -> FogVolumesSection {
    let volumes = fog_volumes
        .iter()
        .map(|v| {
            // Bake derived AABB metrics at compile time so the raymarch shader
            // can skip recomputing them per ray step. `half_ext` is clamped
            // away from zero to avoid infinities for degenerate volumes.
            let min = Vec3::from(v.min);
            let max = Vec3::from(v.max);
            let center = (min + max) * 0.5;
            let half_ext = ((max - min) * 0.5).max(Vec3::splat(1.0e-6));
            let inv_half_ext = Vec3::ONE / half_ext;
            // Semantic point entities (fog_lamp sphere, fog_tube capsule) have
            // no planes and use `radial_falloff` for their fade shape. For a
            // sphere (isotropic AABB), normalise by the sphere radius so
            // `radial_t == 1` at the actual sphere surface and the shader's
            // `pow(1 - radial_t, radial_falloff)` reaches 0 exactly there.
            // Using the AABB half-diagonal (= R*sqrt(3) for a sphere) would
            // push the zero-density point to the AABB corners, leaving visible
            // density at the sphere boundary and making the volume look boxy.
            // For anisotropic volumes (fog_tube) keep the half-diagonal as
            // before; improving capsule shaping requires a proper capsule SDF
            // and is left for a future pass.
            let is_sphere_semantic = v.planes.is_empty()
                && (half_ext.x - half_ext.y).abs() < 1.0e-3
                && (half_ext.y - half_ext.z).abs() < 1.0e-3;
            let half_diag = if is_sphere_semantic {
                half_ext.x
            } else {
                half_ext.length()
            };
            // Typed bool in IR → float discriminant at write time: radial-fade
            // producers (plane-bounded `fog_volume`, `fog_lamp`, `fog_tube`) → 0.0;
            // axis-aligned `fog_volume` (ellipsoid path) → 1.0.
            let shape_mode = if v.is_ellipsoid { 1.0 } else { 0.0 };

            FogVolumeRecord {
                min: v.min,
                density: v.density,
                max: v.max,
                edge_softness: v.edge_softness,
                glow: v.glow,
                radial_falloff: v.radial_falloff,
                center: center.to_array(),
                inv_half_ext: inv_half_ext.to_array(),
                half_diag,
                shape_mode,
                tint: v.tint,
                saturation: v.saturation,
                min_brightness: v.min_brightness,
                light_range: v.light_range,
                anisotropy: v.anisotropy,
                ambient_scatter: v.ambient_scatter,
                plane_count: v.planes.len() as u32,
                planes: v.planes.clone(),
                tags: v.tags.clone(),
            }
        })
        .collect();
    FogVolumesSection {
        pixel_scale: fog_pixel_scale,
        initial_gravity,
        volumes,
    }
}

/// Build a `DataScriptSection` from already-compiled bytes and the resolved
/// source path. The compiler reads the source, runs `scripts-build` for `.ts`
/// inputs (or passes Luau through unchanged), then hands the result here for
/// embedding in the PRL.
pub fn encode_data_script(compiled_bytes: Vec<u8>, source_path: String) -> DataScriptSection {
    DataScriptSection {
        compiled_bytes,
        source_path,
    }
}

/// Convert compiler portal data into a `PortalsSection` for the format crate.
pub fn encode_portals(portals: &[Portal]) -> PortalsSection {
    let mut vertices = Vec::new();
    let mut records = Vec::new();

    for portal in portals {
        let vertex_start = vertices.len() as u32;
        let vertex_count = portal.polygon.len() as u32;

        // Output precision boundary: narrow portal vertices from f64 to f32
        // at the PRL format write site.
        for v in &portal.polygon {
            vertices.push([v.x as f32, v.y as f32, v.z as f32]);
        }

        records.push(PortalRecord {
            vertex_start,
            vertex_count,
            front_leaf: portal.front_leaf as u32,
            back_leaf: portal.back_leaf as u32,
        });
    }

    PortalsSection {
        vertices,
        portals: records,
    }
}

/// Encode runtime cells from BSP leaf records plus explicit exterior
/// classification. Cell ids stay one-to-one with BSP leaf ids. The one-to-one
/// mapping avoids remapping portal endpoints, BVH leaf `cell_id`, fog masks,
/// and diagnostics — all downstream consumers index by leaf id directly.
pub fn encode_cells(
    leaves: &BspLeavesSection,
    portals: &PortalsSection,
    exterior_leaves: &HashSet<usize>,
) -> anyhow::Result<CellsSection> {
    if leaves.leaves.is_empty() {
        anyhow::bail!("cannot encode Cells: source BspLeavesSection is empty");
    }

    let mut portal_refs_by_cell: Vec<Vec<u32>> = vec![Vec::new(); leaves.leaves.len()];
    for (portal_idx, portal) in portals.portals.iter().enumerate() {
        let portal_idx = portal_idx as u32;
        let front = portal.front_leaf as usize;
        let back = portal.back_leaf as usize;
        if front >= leaves.leaves.len() || back >= leaves.leaves.len() {
            anyhow::bail!(
                "Cells portal adjacency references leaf out of range: portal {portal_idx} \
                 front={} back={} leaf_count={}",
                portal.front_leaf,
                portal.back_leaf,
                leaves.leaves.len()
            );
        }
        portal_refs_by_cell[front].push(portal_idx);
        portal_refs_by_cell[back].push(portal_idx);
    }
    for refs in &mut portal_refs_by_cell {
        refs.sort_unstable();
        refs.dedup();
    }

    let mut portal_refs = Vec::new();
    let mut cells = Vec::with_capacity(leaves.leaves.len());
    for (cell_idx, leaf) in leaves.leaves.iter().enumerate() {
        validate_cell_bounds(cell_idx, leaf)?;

        let solid = leaf.is_solid != 0;
        let exterior = exterior_leaves.contains(&cell_idx);
        if solid && exterior {
            anyhow::bail!("Cells cell {cell_idx} cannot be both solid and exterior");
        }
        if (solid || exterior) && leaf.face_count != 0 {
            anyhow::bail!(
                "Cells cell {cell_idx} is solid/exterior but has face_count {}",
                leaf.face_count
            );
        }

        let drawable = !solid && !exterior && leaf.face_count > 0;
        let flags = (u32::from(solid) * CELL_FLAG_SOLID)
            | (u32::from(exterior) * CELL_FLAG_EXTERIOR)
            | (u32::from(drawable) * CELL_FLAG_DRAWABLE);

        let refs = &portal_refs_by_cell[cell_idx];
        let (portal_ref_start, portal_ref_count) = if refs.is_empty() {
            (0, 0)
        } else {
            let start = portal_refs.len() as u32;
            portal_refs.extend_from_slice(refs);
            (start, refs.len() as u32)
        };

        cells.push(CellRecord {
            bounds_min: leaf.bounds_min,
            bounds_max: leaf.bounds_max,
            flags,
            face_start: if leaf.face_count == 0 {
                0
            } else {
                leaf.face_start
            },
            face_count: leaf.face_count,
            portal_ref_start,
            portal_ref_count,
        });
    }

    let section = CellsSection { cells, portal_refs };
    CellsSection::from_bytes(&section.to_bytes())?;
    Ok(section)
}

fn validate_cell_bounds(
    cell_idx: usize,
    leaf: &postretro_level_format::bsp::BspLeafRecord,
) -> anyhow::Result<()> {
    for axis in 0..3 {
        let min = leaf.bounds_min[axis];
        let max = leaf.bounds_max[axis];
        if !min.is_finite() || !max.is_finite() {
            anyhow::bail!(
                "Cells cell {cell_idx} has non-finite bounds on axis {axis}: min {min}, max {max}"
            );
        }
        if min > max {
            anyhow::bail!(
                "Cells cell {cell_idx} has inverted bounds on axis {axis}: min {min} > max {max}"
            );
        }
    }
    Ok(())
}

/// Encode the point-to-cell locator from the final BSP tree. Cell ids preserve
/// the BSP leaf id space, but the wire format names them as cells rather than
/// using the legacy negative leaf sentinel.
pub fn encode_cell_locator(tree: &BspTree) -> anyhow::Result<CellLocatorSection> {
    if tree.leaves.is_empty() {
        anyhow::bail!("cannot encode CellLocator: source BspLeavesSection is empty");
    }

    let root = if tree.nodes.is_empty() {
        CellLocatorChild::Cell(0)
    } else {
        CellLocatorChild::Node(0)
    };
    let mut nodes = Vec::with_capacity(tree.nodes.len());
    for node in &tree.nodes {
        nodes.push(CellLocatorNodeRecord {
            plane_normal: [
                node.plane_normal.x as f32,
                node.plane_normal.y as f32,
                node.plane_normal.z as f32,
            ],
            plane_distance: node.plane_distance as f32,
            front: locator_child(&node.front),
            back: locator_child(&node.back),
        });
    }

    let section = CellLocatorSection { root, nodes };
    CellLocatorSection::from_bytes(&section.to_bytes(), tree.leaves.len() as u32)?;
    Ok(section)
}

fn locator_child(child: &BspChild) -> CellLocatorChild {
    match child {
        BspChild::Node(index) => CellLocatorChild::Node(*index as u32),
        BspChild::Leaf(index) => CellLocatorChild::Cell(*index as u32),
    }
}

/// Compatibility entry point for callers without billboard scatter sections.
#[allow(clippy::too_many_arguments)]
pub fn pack_and_write_portals(
    output: &Path,
    geo_result: &GeometryResult,
    texture_cache_keys: &HashMap<String, [u8; 32]>,
    leaves: &BspLeavesSection,
    tree: &BspTree,
    portals: &PortalsSection,
    exterior_leaves: &HashSet<usize>,
    bvh: &BvhSection,
    bvh_chunk_ranges: &[(u32, u32)],
    alpha_lights: &AlphaLightsSection,
    light_influence: &LightInfluenceSection,
    sh_volume: &OctahedralShVolumeSection,
    direct_sh_volume: Option<&DirectShVolumeSection>,
    entity_shadow_lights: Option<&EntityShadowLightsSection>,
    direct_sh_delta_volumes: Option<&DirectShDeltaVolumesSection>,
    shadowmask_atlas: Option<&ShadowmaskAtlasSection>,
    lightmap: &LightmapSection,
    chunk_light_list: &ChunkLightListSection,
    animated_light_chunks: Option<&AnimatedLightChunksSection>,
    animated_light_weight_maps: Option<&AnimatedLightWeightMapsSection>,
    light_tags: Option<&LightTagsSection>,
    delta_sh_volumes: Option<&DeltaShVolumesSection>,
    data_script: Option<&DataScriptSection>,
    map_entities: Option<&MapEntitySection>,
    fog_volumes: &FogVolumesSection,
    fog_cell_masks: Option<&FogCellMasksSection>,
    sdf_atlas: Option<&SdfAtlasSection>,
    navmesh: Option<&NavMeshSection>,
    kinematic_geometry: Option<&KinematicGeometrySection>,
    trigger_volumes: Option<&TriggerVolumesSection>,
    cell_draw_index_section: Option<&CellDrawIndexSection>,
    cell_visibility_section: Option<&CellVisibilitySection>,
    animated_direct_sh_delta_volumes: Option<&AnimatedDirectShDeltaVolumesSection>,
) -> anyhow::Result<()> {
    pack_and_write_portals_with_billboard_scatter(
        output,
        geo_result,
        texture_cache_keys,
        leaves,
        tree,
        portals,
        exterior_leaves,
        bvh,
        bvh_chunk_ranges,
        alpha_lights,
        light_influence,
        sh_volume,
        direct_sh_volume,
        entity_shadow_lights,
        direct_sh_delta_volumes,
        shadowmask_atlas,
        lightmap,
        chunk_light_list,
        animated_light_chunks,
        animated_light_weight_maps,
        light_tags,
        delta_sh_volumes,
        data_script,
        map_entities,
        fog_volumes,
        fog_cell_masks,
        sdf_atlas,
        navmesh,
        kinematic_geometry,
        trigger_volumes,
        cell_draw_index_section,
        cell_visibility_section,
        animated_direct_sh_delta_volumes,
        None,
        None,
    )
}

/// Write all required sections (geometry, texture names, texture cache keys,
/// cells, cell locator, portals, BVH, alpha lights, light influence,
/// lightmap, chunk light list, SH volume, and FogVolumes) and conditionally
/// write optional sections (direct SH volume, animated-light chunks and weight
/// maps, light tags, delta SH volumes, data script, map entities, and fog cell
/// masks, and animated direct-SH deltas) when their arguments are non-`None`. The direct SH volume is `None`
/// only when the map has no static (baked) lights at all — the loader treats
/// absence as direct = 0, so animated-only maps emit no direct section. A map
/// whose static-baked lights are all `ShadowType::Sdf` still emits a PRESENT
/// all-zero section: `Sdf` lights are dropped by `static_direct_lights` (their
/// direct term is runtime-traced), but the section itself is not omitted.
///
/// `texture_cache_keys` maps each texture name (as it appears in
/// `geo_result.texture_names.names`) to the 32-byte `.prm` filename key
/// produced by the texture-mip baker. Names absent from the map (no
/// authored PNG slots found) get an all-zero key, matching the baker's
/// "nothing to bake" sentinel.
#[allow(clippy::too_many_arguments)]
pub fn pack_and_write_portals_with_billboard_scatter(
    output: &Path,
    geo_result: &GeometryResult,
    texture_cache_keys: &HashMap<String, [u8; 32]>,
    leaves: &BspLeavesSection,
    tree: &BspTree,
    portals: &PortalsSection,
    exterior_leaves: &HashSet<usize>,
    bvh: &BvhSection,
    bvh_chunk_ranges: &[(u32, u32)],
    alpha_lights: &AlphaLightsSection,
    light_influence: &LightInfluenceSection,
    sh_volume: &OctahedralShVolumeSection,
    direct_sh_volume: Option<&DirectShVolumeSection>,
    entity_shadow_lights: Option<&EntityShadowLightsSection>,
    direct_sh_delta_volumes: Option<&DirectShDeltaVolumesSection>,
    shadowmask_atlas: Option<&ShadowmaskAtlasSection>,
    lightmap: &LightmapSection,
    chunk_light_list: &ChunkLightListSection,
    animated_light_chunks: Option<&AnimatedLightChunksSection>,
    animated_light_weight_maps: Option<&AnimatedLightWeightMapsSection>,
    light_tags: Option<&LightTagsSection>,
    delta_sh_volumes: Option<&DeltaShVolumesSection>,
    data_script: Option<&DataScriptSection>,
    map_entities: Option<&MapEntitySection>,
    fog_volumes: &FogVolumesSection,
    fog_cell_masks: Option<&FogCellMasksSection>,
    sdf_atlas: Option<&SdfAtlasSection>,
    navmesh: Option<&NavMeshSection>,
    kinematic_geometry: Option<&KinematicGeometrySection>,
    trigger_volumes: Option<&TriggerVolumesSection>,
    // CellDrawIndex (id 37), or `None` for zero-leaf maps. Emission is
    // independent of portal presence.
    cell_draw_index_section: Option<&CellDrawIndexSection>,
    // CellVisibility (id 46). The section stays optional for old PRLs; current
    // compiler output always provides it.
    cell_visibility_section: Option<&CellVisibilitySection>,
    animated_direct_sh_delta_volumes: Option<&AnimatedDirectShDeltaVolumesSection>,
    billboard_direct_scatter_volume: Option<&BillboardDirectScatterVolumeSection>,
    animated_billboard_direct_scatter_delta_volumes: Option<
        &AnimatedBillboardDirectScatterDeltaVolumesSection,
    >,
) -> anyhow::Result<()> {
    let scatter_pair_required =
        billboard_direct_scatter_volume.is_some() && animated_direct_sh_delta_volumes.is_some();
    anyhow::ensure!(
        animated_billboard_direct_scatter_delta_volumes.is_some() == scatter_pair_required,
        "BillboardDirectScatterVolume requires AnimatedBillboardDirectScatterDeltaVolumes exactly when AnimatedDirectShDeltaVolumes is present"
    );
    if let (Some(direct), Some(scatter)) = (
        animated_direct_sh_delta_volumes,
        animated_billboard_direct_scatter_delta_volumes,
    ) {
        anyhow::ensure!(
            scatter.animation_descriptor_indices == direct.animation_descriptor_indices
                && scatter.affinity_factor == direct.affinity_factor
                && scatter.affinity_dims == direct.affinity_dims
                && scatter.affinity_offsets == direct.affinity_offsets
                && scatter.affinity_lights == direct.affinity_lights,
            "AnimatedBillboardDirectScatterDeltaVolumes must duplicate AnimatedDirectShDeltaVolumes descriptor and CSR layout"
        );
    }
    let scatter_pair_fits_pack_cap =
        animated_billboard_direct_scatter_delta_volumes.is_none_or(scatter_section_fits_pack_cap);
    if !scatter_pair_fits_pack_cap {
        log::warn!(
            "[Compiler] Billboard direct scatter sections 47/48 withheld during packing: section 48 exceeds the {} byte encoded pack cap",
            MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
        );
    }
    let (billboard_direct_scatter_volume, animated_billboard_direct_scatter_delta_volumes) =
        if scatter_pair_fits_pack_cap {
            (
                billboard_direct_scatter_volume,
                animated_billboard_direct_scatter_delta_volumes,
            )
        } else {
            (None, None)
        };
    let texture_cache_keys_section = TextureCacheKeysSection {
        keys: geo_result
            .texture_names
            .names
            .iter()
            .map(|name| texture_cache_keys.get(name).copied().unwrap_or([0u8; 32]))
            .collect(),
    };
    let cells_section = encode_cells(leaves, portals, exterior_leaves)?;
    let locator_section = encode_cell_locator(tree)?;
    let bvh_section = bvh_with_chunk_ranges(bvh, bvh_chunk_ranges);
    anyhow::ensure!(
        bvh.leaves.is_empty() || cell_draw_index_section.is_some(),
        "CellDrawIndex section is required when Bvh contains {} leaf/leaves",
        bvh.leaves.len()
    );
    anyhow::ensure!(
        !bvh.leaves.is_empty() || cell_draw_index_section.is_none(),
        "CellDrawIndex section must be omitted when Bvh has no leaves"
    );
    let sh_volume_len = sh_volume.try_byte_len().map_err(|error| {
        anyhow::anyhow!("OctahedralShVolume violates its v10 wire contract: {error}")
    })?;
    let entity_shadow_light_count = entity_shadow_lights
        .map(|section| section.light_indices.len())
        .unwrap_or(0);
    let has_usable_direct_sh_deltas =
        if let (Some(direct), Some(deltas)) = (direct_sh_volume, direct_sh_delta_volumes) {
            direct_sh_delta_is_usable_for_selection(deltas, direct, entity_shadow_light_count)
        } else {
            false
        };
    let mut sections = Vec::new();
    sections.push(PlannedSection::new(
        SectionId::Geometry as u32,
        1,
        geo_result.geometry.byte_len(),
        || Ok(geo_result.geometry.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::TextureNames as u32,
        1,
        geo_result.texture_names.byte_len(),
        || Ok(geo_result.texture_names.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::TextureCacheKeys as u32,
        1,
        texture_cache_keys_section.byte_len(),
        || Ok(texture_cache_keys_section.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::Cells as u32,
        1,
        cells_section.byte_len(),
        || Ok(cells_section.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::CellLocator as u32,
        1,
        locator_section.byte_len(),
        || Ok(locator_section.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::Portals as u32,
        1,
        portals.byte_len(),
        || Ok(portals.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::ChunkLightList as u32,
        1,
        chunk_light_list.byte_len(),
        || Ok(chunk_light_list.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::Bvh as u32,
        1,
        bvh_section.byte_len(),
        || Ok(bvh_section.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::AlphaLights as u32,
        1,
        alpha_lights.byte_len(),
        || Ok(alpha_lights.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::LightInfluence as u32,
        1,
        light_influence.byte_len(),
        || Ok(light_influence.to_bytes()),
    ));
    sections.push(PlannedSection::new(
        SectionId::OctahedralShVolume as u32,
        1,
        sh_volume_len,
        || {
            sh_volume.try_to_bytes().map_err(|error| {
                anyhow::anyhow!("OctahedralShVolume violates its v10 wire contract: {error}")
            })
        },
    ));
    sections.push(PlannedSection::new(
        SectionId::Lightmap as u32,
        1,
        lightmap.byte_len(),
        || Ok(lightmap.to_bytes()),
    ));
    if let Some(section) = direct_sh_volume {
        let len = section.try_byte_len().map_err(|error| {
            anyhow::anyhow!("DirectShVolume violates its wire contract: {error}")
        })?;
        sections.push(PlannedSection::new(
            SectionId::DirectShVolume as u32,
            1,
            len,
            || {
                section.try_to_bytes().map_err(|error| {
                    anyhow::anyhow!("DirectShVolume violates its wire contract: {error}")
                })
            },
        ));
    }
    if has_usable_direct_sh_deltas {
        if let Some(section) =
            entity_shadow_lights.filter(|section| !section.light_indices.is_empty())
        {
            sections.push(PlannedSection::new(
                SectionId::EntityShadowLights as u32,
                1,
                section.byte_len(),
                || Ok(section.to_bytes()),
            ));
        }
        if let Some(section) = direct_sh_delta_volumes {
            sections.push(PlannedSection::new(
                SectionId::DirectShDeltaVolumes as u32,
                1,
                section.byte_len(),
                || Ok(section.to_bytes()),
            ));
        }
        if let Some(section) = shadowmask_atlas.filter(|section| !section.channels.is_empty()) {
            sections.push(PlannedSection::new(
                SectionId::ShadowmaskAtlas as u32,
                1,
                section.byte_len(),
                || Ok(section.to_bytes()),
            ));
        }
    }
    if let Some(section) = animated_light_chunks {
        sections.push(PlannedSection::new(
            SectionId::AnimatedLightChunks as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = animated_light_weight_maps {
        sections.push(PlannedSection::new(
            SectionId::AnimatedLightWeightMaps as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = light_tags {
        sections.push(PlannedSection::new(
            SectionId::LightTags as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = delta_sh_volumes {
        sections.push(PlannedSection::new(
            SectionId::DeltaShVolumes as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = animated_direct_sh_delta_volumes {
        let len = section.try_byte_len().map_err(|error| {
            anyhow::anyhow!("AnimatedDirectShDeltaVolumes violates its wire contract: {error}")
        })?;
        sections.push(PlannedSection::new(
            SectionId::AnimatedDirectShDeltaVolumes as u32,
            1,
            len,
            || {
                section.try_to_bytes().map_err(|error| {
                    anyhow::anyhow!(
                        "AnimatedDirectShDeltaVolumes violates its wire contract: {error}"
                    )
                })
            },
        ));
    }
    if let Some(section) = billboard_direct_scatter_volume {
        let len = section.try_byte_len().map_err(|error| {
            anyhow::anyhow!("BillboardDirectScatterVolume violates its wire contract: {error}")
        })?;
        sections.push(PlannedSection::new(
            SectionId::BillboardDirectScatterVolume as u32,
            1,
            len,
            || {
                section.try_to_bytes().map_err(|error| {
                    anyhow::anyhow!(
                        "BillboardDirectScatterVolume violates its wire contract: {error}"
                    )
                })
            },
        ));
    }
    if let Some(section) = animated_billboard_direct_scatter_delta_volumes {
        let len = section.try_byte_len().map_err(|error| {
            anyhow::anyhow!(
                "AnimatedBillboardDirectScatterDeltaVolumes violates its wire contract: {error}"
            )
        })?;
        sections.push(PlannedSection::new(SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32, 1, len, || section.try_to_bytes().map_err(|error| anyhow::anyhow!("AnimatedBillboardDirectScatterDeltaVolumes violates its wire contract: {error}"))));
    }
    if let Some(section) = data_script {
        sections.push(PlannedSection::new(
            SectionId::DataScript as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = map_entities {
        sections.push(PlannedSection::new(
            SectionId::MapEntity as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    sections.push(PlannedSection::new(
        SectionId::FogVolumes as u32,
        1,
        fog_volumes.byte_len(),
        || Ok(fog_volumes.to_bytes()),
    ));
    if let Some(section) = fog_cell_masks {
        sections.push(PlannedSection::new(
            SectionId::FogCellMasks as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = sdf_atlas {
        sections.push(PlannedSection::new(
            SectionId::SdfAtlas as u32,
            postretro_level_format::sdf_atlas::SDF_ATLAS_VERSION as u16,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = navmesh {
        sections.push(PlannedSection::new(
            SectionId::NavMesh as u32,
            NAVMESH_CONTAINER_VERSION,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = kinematic_geometry {
        sections.push(PlannedSection::new(
            SectionId::KinematicGeometry as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = trigger_volumes {
        sections.push(PlannedSection::new(
            SectionId::TriggerVolumes as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = cell_draw_index_section {
        sections.push(PlannedSection::new(
            SectionId::CellDrawIndex as u32,
            1,
            section.byte_len(),
            || Ok(section.to_bytes()),
        ));
    }
    if let Some(section) = cell_visibility_section {
        let len = section.try_byte_len().map_err(|error| {
            anyhow::anyhow!("CellVisibility violates its wire contract: {error}")
        })?;
        sections.push(PlannedSection::new(
            SectionId::CellVisibility as u32,
            1,
            len,
            || {
                section.to_bytes().map_err(|error| {
                    anyhow::anyhow!("CellVisibility violates its wire contract: {error}")
                })
            },
        ));
    }

    let section_count = sections.len();
    let declared_payload_bytes: u64 = sections
        .iter()
        .map(|section| section.descriptor.byte_len)
        .sum();
    let largest_payload_bytes = sections
        .iter()
        .map(|section| section.descriptor.byte_len)
        .max()
        .unwrap_or(0);
    write_and_validate_sections(output, sections)?;

    log::info!(
        "[Compiler] Serialize footprint: {section_count} sections, {declared_payload_bytes} payload bytes, {largest_payload_bytes} byte largest payload; writes materialize one payload at a time"
    );

    Ok(())
}

/// Write one section payload at a time, then validate the flushed file.
fn write_and_validate_sections(
    output: &Path,
    sections: Vec<PlannedSection<'_>>,
) -> anyhow::Result<()> {
    // Validate output directory exists before writing
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            anyhow::bail!("output directory does not exist: {}", parent.display());
        }
    }
    let original_output = OutputIdentity::capture(output)?;

    let descriptors: Vec<_> = sections
        .iter()
        .map(|section| section.descriptor.clone())
        .collect();
    let file_name = output
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("output path has no file name: {}", output.display()))?;
    let mut temporary_output = StagedPrl::create(output, file_name)?;
    let write_result = (|| -> anyhow::Result<u64> {
        write_prl_header_and_table(temporary_output.file_mut(), &descriptors)?;
        for section in sections {
            let bytes = (section.encode)()?;
            if bytes.len() as u64 != section.descriptor.byte_len {
                match SectionId::from_u32(section.descriptor.section_id) {
                    Some(section_id) => anyhow::bail!(
                        "section {section_id:?} (id {}) wrote {} bytes but its table declares {} bytes",
                        section.descriptor.section_id,
                        bytes.len(),
                        section.descriptor.byte_len,
                    ),
                    None => anyhow::bail!(
                        "unknown section {} wrote {} bytes but its table declares {} bytes",
                        section.descriptor.section_id,
                        bytes.len(),
                        section.descriptor.byte_len,
                    ),
                }
            }
            temporary_output.file_mut().write_all(&bytes)?;
        }
        temporary_output.file_mut().flush()?;
        let total_size = temporary_output.file().metadata()?.len();
        validate_readback(temporary_output.file_mut(), &descriptors)?;
        Ok(total_size)
    })();
    let total_size = match write_result {
        Ok(total_size) => total_size,
        Err(error) => {
            return Err(failed_staging_error(
                temporary_output,
                format!("{error} after write failure"),
            ));
        }
    };
    publish_validated_output(temporary_output, output, &original_output)?;
    log::info!("Wrote {} ({} bytes)", output.display(), total_size);
    log::info!("Read-back validation passed.");

    Ok(())
}

struct StagedPrl {
    temporary: NamedTempFile,
    identity: FileIdentity,
}

impl StagedPrl {
    fn create(output: &Path, file_name: &std::ffi::OsStr) -> anyhow::Result<Self> {
        let parent = output
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut prefix = OsString::from(".");
        prefix.push(file_name);
        prefix.push(".pack-");
        let mut temporary = TempFileBuilder::new()
            .prefix(&prefix)
            .suffix(".tmp")
            .tempfile_in(parent)
            .map_err(|error| {
                anyhow::anyhow!(
                    "failed to create temporary PRL beside {}: {error}",
                    output.display(),
                )
            })?;

        // Disable pathname-based Drop cleanup. Failed staging paths remain because
        // a checked name can be replaced before an unlink reaches the filesystem.
        temporary.disable_cleanup(true);
        let identity = FileIdentity::from_file(temporary.reopen()?).map_err(|error| {
            anyhow::anyhow!(
                "failed to identify temporary PRL {}: {error}",
                temporary.path().display(),
            )
        })?;

        Ok(Self {
            temporary,
            identity,
        })
    }

    fn path(&self) -> &Path {
        self.temporary.path()
    }

    fn file(&self) -> &File {
        self.temporary.as_file()
    }

    fn file_mut(&mut self) -> &mut File {
        self.temporary.as_file_mut()
    }

    fn ensure_path_is_owned(&self) -> anyhow::Result<()> {
        ensure_path_has_identity(self.path(), &self.identity, "temporary PRL")
    }

    fn discard(self) -> anyhow::Result<()> {
        // Cleanup is best effort. Re-check before unlink so a known replacement
        // remains intact; a failed check leaves the path for manual inspection.
        let path = self.path().to_path_buf();
        self.ensure_path_is_owned()?;
        self.temporary.close().map_err(|error| {
            anyhow::anyhow!("failed to remove temporary PRL {}: {error}", path.display(),)
        })
    }
}

fn failed_staging_error(staged: StagedPrl, failure: impl std::fmt::Display) -> anyhow::Error {
    let temporary_path = staged.path().to_path_buf();
    match staged.discard() {
        Ok(()) => anyhow::anyhow!("{failure}; temporary PRL discarded"),
        Err(cleanup_error) => anyhow::anyhow!(
            "{failure}; temporary PRL remains at {} because cleanup failed: {cleanup_error}",
            temporary_path.display(),
        ),
    }
}

fn retry_windows_publication<T, U>(
    mut staged: T,
    is_windows: bool,
    mut persist: impl FnMut(T) -> Result<U, (T, std::io::Error)>,
    mut sleep: impl FnMut(Duration),
) -> Result<U, (T, std::io::Error, u32)> {
    let mut retries = 0;
    loop {
        match persist(staged) {
            Ok(persisted) => return Ok(persisted),
            Err((returned_staged, error))
                if is_retryable_windows_publish_error(&error, is_windows)
                    && retries < WINDOWS_PUBLISH_MAX_RETRIES =>
            {
                let delay = WINDOWS_PUBLISH_RETRY_BASE_DELAY * (1_u32 << retries);
                retries += 1;
                sleep(delay);
                staged = returned_staged;
            }
            Err((returned_staged, error)) => return Err((returned_staged, error, retries)),
        }
    }
}

fn is_retryable_windows_publish_error(error: &std::io::Error, is_windows: bool) -> bool {
    is_windows
        && matches!(
            error.raw_os_error(),
            Some(
                WINDOWS_ERROR_ACCESS_DENIED
                    | WINDOWS_ERROR_SHARING_VIOLATION
                    | WINDOWS_ERROR_LOCK_VIOLATION
            )
        )
}

struct OutputPublishLock {
    _file: File,
}

impl OutputPublishLock {
    fn acquire(output: &Path) -> anyhow::Result<Self> {
        let lock_path = output_lock_path(output)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| {
                anyhow::anyhow!(
                    "failed to open PRL publication lock {}: {error}",
                    lock_path.display(),
                )
            })?;
        anyhow::ensure!(
            file.metadata()?.file_type().is_file(),
            "PRL publication lock {} is not a regular file",
            lock_path.display(),
        );
        let identity = FileIdentity::from_file(file.try_clone()?)?;
        ensure_path_has_identity(&lock_path, &identity, "PRL publication lock")?;
        FileExt::lock(&file).map_err(|error| {
            anyhow::anyhow!(
                "failed to lock PRL publication lock {}: {error}",
                lock_path.display(),
            )
        })?;
        ensure_path_has_identity(&lock_path, &identity, "PRL publication lock")?;
        // Keep the lock pathname between runs. Removing it would let a waiter
        // hold the old inode while a new compiler locks a newly created inode.
        Ok(Self { _file: file })
    }
}

fn output_lock_path(output: &Path) -> anyhow::Result<PathBuf> {
    let file_name = output
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("output path has no file name: {}", output.display()))?;
    let mut lock_name = OsString::from(".");
    lock_name.push(file_name);
    lock_name.push(".pack.lock");
    Ok(output.with_file_name(lock_name))
}

enum OutputIdentity {
    Absent,
    Regular(FileIdentity),
}

impl OutputIdentity {
    fn capture(output: &Path) -> anyhow::Result<Self> {
        match fs::symlink_metadata(output) {
            Ok(metadata) if metadata.file_type().is_file() => {
                let identity = open_regular_path_identity(output, "existing output")?;
                Ok(Self::Regular(identity))
            }
            Ok(_) => anyhow::bail!(
                "refusing to replace non-regular output {}",
                output.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::Absent),
            Err(error) => Err(anyhow::anyhow!(
                "failed to inspect existing output {}: {error}",
                output.display(),
            )),
        }
    }

    fn ensure_unchanged(&self, output: &Path) -> anyhow::Result<()> {
        match self {
            Self::Absent => match fs::symlink_metadata(output) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Ok(_) => anyhow::bail!(
                    "refusing to publish because output {} appeared during compilation",
                    output.display(),
                ),
                Err(error) => Err(anyhow::anyhow!(
                    "failed to re-inspect output {} before publication: {error}",
                    output.display(),
                )),
            },
            Self::Regular(identity) => {
                ensure_path_has_identity(output, identity, "existing output").map_err(|error| {
                    anyhow::anyhow!(
                        "refusing to publish because output {} changed during compilation: {error}",
                        output.display(),
                    )
                })
            }
        }
    }
}

fn open_regular_path_identity(path: &Path, description: &str) -> anyhow::Result<FileIdentity> {
    let before = fs::symlink_metadata(path).map_err(|error| {
        anyhow::anyhow!(
            "failed to inspect {description} {}: {error}",
            path.display()
        )
    })?;
    anyhow::ensure!(
        before.file_type().is_file(),
        "{description} {} is not a regular file",
        path.display(),
    );

    let file = File::open(path).map_err(|error| {
        anyhow::anyhow!("failed to open {description} {}: {error}", path.display())
    })?;
    anyhow::ensure!(
        file.metadata()?.file_type().is_file(),
        "{description} {} did not open as a regular file",
        path.display(),
    );
    let identity = FileIdentity::from_file(file)?;

    let after = fs::symlink_metadata(path).map_err(|error| {
        anyhow::anyhow!(
            "failed to re-inspect {description} {}: {error}",
            path.display()
        )
    })?;
    anyhow::ensure!(
        after.file_type().is_file(),
        "{description} {} changed while it was inspected",
        path.display(),
    );
    let confirmed = FileIdentity::from_path(path).map_err(|error| {
        anyhow::anyhow!(
            "failed to confirm {description} {} identity: {error}",
            path.display(),
        )
    })?;
    anyhow::ensure!(
        confirmed == identity,
        "{description} {} changed while its identity was captured",
        path.display(),
    );
    Ok(identity)
}

fn ensure_path_has_identity(
    path: &Path,
    expected: &FileIdentity,
    description: &str,
) -> anyhow::Result<()> {
    let actual = open_regular_path_identity(path, description)?;
    anyhow::ensure!(
        &actual == expected,
        "{description} {} no longer names the file owned by this invocation",
        path.display(),
    );
    Ok(())
}

/// Publish a validated staged file while excluding cooperating prl-build writers.
fn publish_validated_output(
    temporary_output: StagedPrl,
    output: &Path,
    original_output: &OutputIdentity,
) -> anyhow::Result<()> {
    publish_validated_output_with_hook(temporary_output, output, original_output, || Ok(()))
}

fn publish_validated_output_with_hook(
    temporary_output: StagedPrl,
    output: &Path,
    original_output: &OutputIdentity,
    after_precondition: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let _publication_lock = match OutputPublishLock::acquire(output) {
        Ok(lock) => lock,
        Err(error) => {
            return Err(failed_staging_error(
                temporary_output,
                format!("{error} after lock failure"),
            ));
        }
    };
    let temporary_path = temporary_output.path().to_path_buf();
    let precondition = temporary_output
        .ensure_path_is_owned()
        .and_then(|()| original_output.ensure_unchanged(output));
    if let Err(error) = precondition {
        return Err(failed_staging_error(
            temporary_output,
            format!("{error} after publication refusal"),
        ));
    }

    if let Err(error) = after_precondition() {
        return Err(failed_staging_error(
            temporary_output,
            format!("{error} after publication was interrupted"),
        ));
    }

    // The lock closes this window for cooperating prl-build processes. This
    // late check also catches noncooperating changes observed before rename;
    // an external writer can still race the final check and path-based rename.
    let precondition = temporary_output
        .ensure_path_is_owned()
        .and_then(|()| original_output.ensure_unchanged(output));
    if let Err(error) = precondition {
        return Err(failed_staging_error(
            temporary_output,
            format!("{error} after a late publication race"),
        ));
    }

    let StagedPrl {
        temporary,
        identity,
    } = temporary_output;
    match retry_windows_publication(
        StagedPrl {
            temporary,
            identity,
        },
        cfg!(windows),
        |StagedPrl {
             temporary,
             identity,
         }| match temporary.persist(output) {
            Ok(persisted_file) => Ok((persisted_file, identity)),
            Err(tempfile::PersistError {
                error,
                file: temporary,
            }) => Err((
                StagedPrl {
                    temporary,
                    identity,
                },
                error,
            )),
        },
        std::thread::sleep,
    ) {
        Ok((persisted_file, identity)) => {
            let result = ensure_path_has_identity(output, &identity, "published output");
            drop(persisted_file);
            result
        }
        Err((staged, publish_error, retries)) => {
            let retry_context = if retries == 0 {
                String::new()
            } else {
                format!(
                    " after {retries} Windows replacement retries; close processes reading {} or check its attributes and permissions",
                    output.display(),
                )
            };
            Err(failed_staging_error(
                staged,
                format!(
                    "failed to publish temporary PRL {} to {}: {publish_error}{retry_context}",
                    temporary_path.display(),
                    output.display(),
                ),
            ))
        }
    }
}

/// Validate the flushed PRL through the handle that received the bytes.
fn validate_readback(
    file: &mut File,
    expected_sections: &[SectionDescriptor],
) -> anyhow::Result<()> {
    file.seek(SeekFrom::Start(0))?;
    let meta = read_container(&mut *file)?;

    anyhow::ensure!(
        meta.header.section_count as usize == expected_sections.len(),
        "expected {} sections, got {}",
        expected_sections.len(),
        meta.header.section_count
    );

    let mut expected_offset = 8 + expected_sections.len() as u64 * 22;
    for (index, expected) in expected_sections.iter().enumerate() {
        let entry = meta.sections.get(index).ok_or_else(|| {
            anyhow::anyhow!("section ID {} missing from read-back", expected.section_id)
        })?;
        anyhow::ensure!(
            entry.section_id == expected.section_id,
            "section table order differs at index {index}: expected ID {}, got {}",
            expected.section_id,
            entry.section_id,
        );
        anyhow::ensure!(
            entry.offset == expected_offset,
            "section ID {} offset {} does not match expected {}",
            expected.section_id,
            entry.offset,
            expected_offset,
        );
        anyhow::ensure!(
            entry.size == expected.byte_len && entry.version == expected.version,
            "section ID {} table entry differs from the declared length or version",
            expected.section_id,
        );
        let actual =
            read_section_data(&mut *file, &meta, expected.section_id)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "section ID {} data missing from read-back",
                    expected.section_id
                )
            })?;
        anyhow::ensure!(
            actual.len() as u64 == expected.byte_len,
            "section ID {} framed payload length {} does not match expected {}",
            expected.section_id,
            actual.len(),
            expected.byte_len,
        );
        expected_offset += expected.byte_len;
    }
    let file_len = file.metadata()?.len();
    anyhow::ensure!(
        expected_offset == file_len,
        "section table ends at {expected_offset}, but the file is {file_len} bytes",
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::bsp::BspLeafRecord;
    use postretro_level_format::bvh::{BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode as FlatBvhNode};
    use postretro_level_format::cell_draw_index::{CellDrawIndexSection, Span};
    use postretro_level_format::cell_visibility::CellVisibilitySection;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;
    use std::io::Cursor;

    fn staging_artifacts(output: &Path) -> Vec<PathBuf> {
        let parent = output.parent().expect("test output has a parent");
        let prefix = format!(
            ".{}.pack-",
            output
                .file_name()
                .expect("test output has a file name")
                .to_string_lossy()
        );
        std::fs::read_dir(parent)
            .expect("test output parent should be readable")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(&prefix))
            })
            .collect()
    }

    fn remove_publication_test_artifacts(output: &Path) {
        for path in staging_artifacts(output) {
            std::fs::remove_file(path).expect("publish artifact should be removable");
        }
        let lock_path = output_lock_path(output).expect("test output should have a lock path");
        if lock_path.exists() {
            std::fs::remove_file(lock_path).expect("publication lock should be removable");
        }
    }

    #[test]
    fn windows_publication_retry_retries_transient_replacement_errors() {
        let mut attempts = 0;
        let mut delays = Vec::new();
        let result: std::result::Result<(), ((), std::io::Error, u32)> = retry_windows_publication(
            (),
            true,
            |_| {
                attempts += 1;
                if attempts <= 2 {
                    Err((
                        (),
                        std::io::Error::from_raw_os_error(WINDOWS_ERROR_ACCESS_DENIED),
                    ))
                } else {
                    Ok(())
                }
            },
            |delay| delays.push(delay),
        );

        result.expect("transient Windows replacement failures should retry");
        assert_eq!(attempts, 3);
        assert_eq!(
            delays,
            vec![
                WINDOWS_PUBLISH_RETRY_BASE_DELAY,
                WINDOWS_PUBLISH_RETRY_BASE_DELAY * 2,
            ]
        );
    }

    #[test]
    fn non_windows_publication_retry_returns_first_replace_failure() {
        let mut attempts = 0;
        let result: std::result::Result<(), ((), std::io::Error, u32)> = retry_windows_publication(
            (),
            false,
            |_| {
                attempts += 1;
                Err((
                    (),
                    std::io::Error::from_raw_os_error(WINDOWS_ERROR_ACCESS_DENIED),
                ))
            },
            |_| panic!("non-Windows publication must not sleep and retry"),
        );

        let (_, error, retries) = result.expect_err("non-Windows failure should return directly");
        assert_eq!(attempts, 1);
        assert_eq!(error.raw_os_error(), Some(WINDOWS_ERROR_ACCESS_DENIED));
        assert_eq!(retries, 0);
    }

    #[test]
    fn streamed_write_matches_legacy_container_bytes_and_file_readback() {
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let legacy_sections = vec![
            postretro_level_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: vec![0x01, 0x02, 0x03],
            },
            postretro_level_format::SectionBlob {
                section_id: SectionId::Lightmap as u32,
                version: 1,
                data: vec![0xAA, 0xBB],
            },
        ];
        let mut expected = Vec::new();
        postretro_level_format::write_prl(&mut expected, &legacy_sections)
            .expect("legacy PRL should serialize");

        write_and_validate_sections(
            &output,
            vec![
                PlannedSection::new(SectionId::Geometry as u32, 1, 3, || {
                    Ok(vec![0x01, 0x02, 0x03])
                }),
                PlannedSection::new(SectionId::Lightmap as u32, 1, 2, || Ok(vec![0xAA, 0xBB])),
            ],
        )
        .expect("streamed PRL should write and read back");

        assert_eq!(
            std::fs::read(&output).expect("streamed output should exist"),
            expected
        );
        std::fs::remove_file(&output).expect("output should be removable");
        remove_publication_test_artifacts(&output);
    }

    #[test]
    fn streamed_write_rejects_declared_payload_length_mismatch() {
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-mismatch-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let error = write_and_validate_sections(
            &output,
            vec![PlannedSection::new(
                SectionId::Geometry as u32,
                1,
                3,
                || Ok(vec![0x01, 0x02]),
            )],
        )
        .expect_err("writer must reject a declared length mismatch");

        assert!(
            error
                .to_string()
                .contains("section Geometry (id 17) wrote 2 bytes but its table declares 3")
        );
        assert!(
            !output.exists(),
            "a declared-length mismatch must not leave a malformed final PRL"
        );
        remove_publication_test_artifacts(&output);
    }

    #[test]
    fn streamed_write_replaces_existing_output_without_staging_artifacts() {
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-replace-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(&output, b"previous valid PRL").expect("should create previous output");

        write_and_validate_sections(
            &output,
            vec![PlannedSection::new(
                SectionId::Geometry as u32,
                1,
                3,
                || Ok(vec![0x01, 0x02, 0x03]),
            )],
        )
        .expect("streamed PRL should replace the previous output");

        assert_ne!(
            std::fs::read(&output).expect("replacement output must exist"),
            b"previous valid PRL"
        );
        assert!(
            staging_artifacts(&output).is_empty(),
            "a successful publish must not leave temporary or backup files"
        );
        assert!(
            output_lock_path(&output)
                .expect("test output should have a lock path")
                .is_file(),
            "the stable lock inode must remain for later compiler invocations"
        );
        std::fs::remove_file(&output).expect("output should be removable");
        remove_publication_test_artifacts(&output);
    }

    #[test]
    fn streamed_write_failure_preserves_existing_output_and_cleans_staging() {
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-preserve-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let previous_bytes = b"previous valid PRL";
        std::fs::write(&output, previous_bytes).expect("should create previous output");

        let error = write_and_validate_sections(
            &output,
            vec![PlannedSection::new(
                SectionId::Geometry as u32,
                1,
                3,
                || Ok(vec![0x01, 0x02]),
            )],
        )
        .expect_err("writer must reject a declared length mismatch");

        assert!(
            error
                .to_string()
                .contains("wrote 2 bytes but its table declares 3")
        );
        assert_eq!(
            std::fs::read(&output).expect("previous output must remain readable"),
            previous_bytes,
            "a write failure must not replace the previous output"
        );
        assert!(
            staging_artifacts(&output).is_empty(),
            "failed staging should be cleaned up"
        );
        std::fs::remove_file(&output).expect("output should be removable");
        remove_publication_test_artifacts(&output);
    }

    // Regression: failed compilation replaced a directory at the requested output path.
    #[test]
    fn streamed_write_rejects_directory_output_before_staging() {
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-directory-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::create_dir(&output).expect("should create directory output fixture");

        let error = write_and_validate_sections(
            &output,
            vec![PlannedSection::new(
                SectionId::Geometry as u32,
                1,
                1,
                || panic!("non-regular output must be rejected before encoding"),
            )],
        )
        .expect_err("a directory cannot be replaced with a PRL");

        assert!(error.to_string().contains("non-regular output"));
        assert!(output.is_dir(), "the existing directory must remain intact");
        assert!(
            staging_artifacts(&output).is_empty(),
            "rejection before staging must not create a temporary file"
        );
        std::fs::remove_dir(output).expect("directory fixture should be removable");
    }

    // Regression: cleanup must not unlink a replacement installed before its identity check.
    #[test]
    fn failed_staging_cleanup_preserves_replacement_swapped_before_identity_check() {
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-cleanup-identity-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let file_name = output.file_name().expect("test output has a file name");
        let staged = StagedPrl::create(&output, file_name).expect("staging should succeed");
        let staged_path = staged.path().to_path_buf();
        let displaced = output.with_extension("owned-staging");
        staged
            .ensure_path_is_owned()
            .expect("the pre-cleanup identity check should pass");
        std::fs::rename(&staged_path, &displaced).expect("should displace owned staging file");
        std::fs::write(&staged_path, b"later replacement")
            .expect("should install replacement at staging path");

        let error = staged
            .discard()
            .expect_err("cleanup must reject a replacement staging path");

        assert!(error.to_string().contains("no longer names the file owned"));
        assert_eq!(
            std::fs::read(&staged_path).expect("replacement must remain readable"),
            b"later replacement"
        );
        std::fs::remove_file(staged_path).expect("replacement should be removable");
        std::fs::remove_file(displaced).expect("owned staging file should be removable");
    }

    // Regression: read-back reopened a replaced staging pathname and could publish its valid bytes.
    #[test]
    fn staged_readback_and_publish_remain_bound_to_original_file() {
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-staged-identity-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let file_name = output.file_name().expect("test output has a file name");
        let original_output = OutputIdentity::capture(&output).expect("output should be absent");
        let mut staged = StagedPrl::create(&output, file_name).expect("staging should succeed");
        let descriptor = SectionDescriptor {
            section_id: SectionId::Geometry as u32,
            version: 1,
            byte_len: 3,
        };
        write_prl_header_and_table(staged.file_mut(), std::slice::from_ref(&descriptor))
            .expect("header should write");
        staged
            .file_mut()
            .write_all(&[1, 2, 3])
            .expect("payload should write");
        staged.file_mut().flush().expect("staging should flush");

        let staged_path = staged.path().to_path_buf();
        let displaced = output.with_extension("owned-staging");
        std::fs::rename(&staged_path, &displaced).expect("should displace owned staging file");
        let mut replacement = Vec::new();
        postretro_level_format::write_prl(
            &mut replacement,
            &[postretro_level_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: vec![9, 9, 9],
            }],
        )
        .expect("replacement PRL should encode");
        std::fs::write(&staged_path, replacement).expect("replacement PRL should write");

        validate_readback(staged.file_mut(), std::slice::from_ref(&descriptor))
            .expect("read-back should validate the original open file");
        let error = publish_validated_output(staged, &output, &original_output)
            .expect_err("publication must reject the replacement staging identity");

        assert!(error.to_string().contains("no longer names the file owned"));
        assert!(!output.exists(), "replacement bytes must not be published");
        assert!(
            staged_path.exists(),
            "replacement staging file must not be deleted"
        );
        std::fs::remove_file(staged_path).expect("replacement staging should be removable");
        std::fs::remove_file(displaced).expect("owned staging file should be removable");
        remove_publication_test_artifacts(&output);
    }

    // Regression: symlink outputs previously changed target semantics during publication.
    #[cfg(unix)]
    #[test]
    fn streamed_write_rejects_symlink_output_before_staging() {
        use std::os::unix::fs::symlink;

        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-symlink-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let target = output.with_extension("target");
        std::fs::write(&target, b"symlink target").expect("should create symlink target");
        symlink(&target, &output).expect("should create output symlink");

        let error = write_and_validate_sections(
            &output,
            vec![PlannedSection::new(
                SectionId::Geometry as u32,
                1,
                1,
                || panic!("symlink output must be rejected before encoding"),
            )],
        )
        .expect_err("a symlink cannot be replaced with a PRL");

        assert!(error.to_string().contains("non-regular output"));
        assert!(
            std::fs::symlink_metadata(&output)
                .expect("output symlink should remain")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read(&target).expect("symlink target should remain readable"),
            b"symlink target"
        );
        assert!(staging_artifacts(&output).is_empty());
        std::fs::remove_file(output).expect("output symlink should be removable");
        std::fs::remove_file(target).expect("symlink target should be removable");
    }

    // Regression: Windows backup publication could strand the original after an output race.
    #[test]
    fn publication_rejects_nonregular_output_replacement_without_moving_original() {
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-output-race-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let original = output.with_extension("original");
        let previous_bytes = b"previous valid PRL";
        std::fs::write(&output, previous_bytes).expect("should create previous output");
        let original_output = OutputIdentity::capture(&output).expect("output should be regular");
        let file_name = output.file_name().expect("test output has a file name");
        let mut staged = StagedPrl::create(&output, file_name).expect("staging should succeed");
        staged
            .file_mut()
            .write_all(b"validated replacement")
            .expect("staging should write");
        staged.file_mut().flush().expect("staging should flush");
        std::fs::rename(&output, &original).expect("should preserve original fixture");
        std::fs::create_dir(&output).expect("should install raced directory output");

        let error = publish_validated_output(staged, &output, &original_output)
            .expect_err("publishing over a raced directory must fail");

        assert!(error.to_string().contains("changed during compilation"));
        assert!(output.is_dir(), "raced directory must remain intact");
        assert_eq!(
            std::fs::read(&original).expect("original output must remain readable"),
            previous_bytes
        );
        assert!(staging_artifacts(&output).is_empty());
        std::fs::remove_dir(&output).expect("raced directory should be removable");
        std::fs::remove_file(original).expect("original fixture should be removable");
        remove_publication_test_artifacts(&output);
    }

    // Regression: publication overwrote an output installed after its precondition check.
    #[test]
    fn publication_rejects_regular_output_replaced_after_precondition() {
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-regular-race-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let original = output.with_extension("original");
        std::fs::write(&output, b"original output").expect("should create original output");
        let original_output = OutputIdentity::capture(&output).expect("output should be regular");
        let file_name = output.file_name().expect("test output has a file name");
        let mut staged = StagedPrl::create(&output, file_name).expect("staging should succeed");
        staged
            .file_mut()
            .write_all(b"validated replacement")
            .expect("staging should write");
        staged.file_mut().flush().expect("staging should flush");
        let error = publish_validated_output_with_hook(staged, &output, &original_output, || {
            let lock_path = output_lock_path(&output)?;
            let competing_lock = OpenOptions::new().read(true).write(true).open(lock_path)?;
            assert!(matches!(
                FileExt::try_lock(&competing_lock),
                Err(fs4::TryLockError::WouldBlock)
            ));
            // Model an external writer that ignores the compiler's advisory lock.
            std::fs::rename(&output, &original).expect("should preserve original fixture");
            std::fs::write(&output, b"concurrent writer").expect("should install raced output");
            Ok(())
        })
        .expect_err("publishing over a raced regular file must fail");

        assert!(error.to_string().contains("changed during compilation"));
        assert_eq!(
            std::fs::read(&output).expect("concurrent output must remain readable"),
            b"concurrent writer"
        );
        assert_eq!(
            std::fs::read(&original).expect("original output must remain readable"),
            b"original output"
        );
        assert!(staging_artifacts(&output).is_empty());
        std::fs::remove_file(&output).expect("concurrent output should be removable");
        std::fs::remove_file(original).expect("original fixture should be removable");
        remove_publication_test_artifacts(&output);
    }

    fn sample_geo_result() -> GeometryResult {
        GeometryResult {
            geometry: GeometrySection {
                vertices: vec![
                    Vertex::new(
                        [1.0, 2.0, 3.0],
                        [0.25, 0.75],
                        [0.0, 1.0, 0.0],
                        [1.0, 0.0, 0.0],
                        true,
                        [0.0, 0.0],
                        0,
                    ),
                    Vertex::new(
                        [4.0, 5.0, 6.0],
                        [0.5, 0.0],
                        [0.0, 1.0, 0.0],
                        [1.0, 0.0, 0.0],
                        true,
                        [0.0, 0.0],
                        0,
                    ),
                    Vertex::new(
                        [7.0, 8.0, 9.0],
                        [1.0, 1.0],
                        [0.0, 1.0, 0.0],
                        [1.0, 0.0, 0.0],
                        true,
                        [0.0, 0.0],
                        0,
                    ),
                ],
                indices: vec![0, 1, 2],
                faces: vec![FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                }],
            },
            texture_names: TextureNamesSection {
                names: vec!["test_texture".to_string()],
            },
            face_index_ranges: vec![crate::geometry::FaceIndexRange {
                index_offset: 0,
                index_count: 3,
            }],
        }
    }

    fn empty_geo_result() -> GeometryResult {
        GeometryResult {
            geometry: GeometrySection {
                vertices: Vec::new(),
                indices: Vec::new(),
                faces: Vec::new(),
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: Vec::new(),
        }
    }

    fn sample_leaves() -> BspLeavesSection {
        BspLeavesSection {
            leaves: vec![
                BspLeafRecord {
                    face_start: 0,
                    face_count: 1,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [32.0, 64.0, 64.0],
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 0,
                    face_count: 0,
                    bounds_min: [32.0, 0.0, 0.0],
                    bounds_max: [64.0, 64.0, 64.0],
                    is_solid: 1,
                },
            ],
        }
    }

    fn empty_draw_leaves() -> BspLeavesSection {
        let mut leaves = sample_leaves();
        leaves.leaves[0].face_count = 0;
        leaves
    }

    fn sample_tree() -> BspTree {
        BspTree {
            nodes: vec![crate::partition::BspNode {
                plane_normal: glam::DVec3::X,
                plane_distance: 32.0,
                front: crate::partition::BspChild::Leaf(0),
                back: crate::partition::BspChild::Leaf(1),
                parent: None,
            }],
            leaves: vec![
                crate::partition::BspLeaf {
                    face_indices: vec![0],
                    bounds: crate::partition::Aabb {
                        min: glam::DVec3::new(0.0, 0.0, 0.0),
                        max: glam::DVec3::new(32.0, 64.0, 64.0),
                    },
                    is_solid: false,
                    defining_planes: Vec::new(),
                },
                crate::partition::BspLeaf {
                    face_indices: Vec::new(),
                    bounds: crate::partition::Aabb {
                        min: glam::DVec3::new(32.0, 0.0, 0.0),
                        max: glam::DVec3::new(64.0, 64.0, 64.0),
                    },
                    is_solid: true,
                    defining_planes: Vec::new(),
                },
            ],
        }
    }

    fn sample_bvh() -> BvhSection {
        BvhSection {
            nodes: vec![FlatBvhNode {
                aabb_min: [0.0, 0.0, 0.0],
                skip_index: 1,
                aabb_max: [1.0, 1.0, 1.0],
                left_child_or_leaf_index: 0,
                flags: BVH_NODE_FLAG_LEAF,
                _padding: 0,
            }],
            leaves: vec![BvhLeaf {
                aabb_min: [0.0, 0.0, 0.0],
                material_bucket_id: 0,
                aabb_max: [1.0, 1.0, 1.0],
                index_offset: 0,
                index_count: 3,
                cell_id: 0,
                chunk_range_start: 0,
                chunk_range_count: 0,
            }],
            root_node_index: 0,
        }
    }

    fn empty_bvh() -> BvhSection {
        BvhSection {
            nodes: Vec::new(),
            leaves: Vec::new(),
            root_node_index: 0,
        }
    }

    fn sample_cell_draw_index_section() -> CellDrawIndexSection {
        CellDrawIndexSection {
            cell_count: 2,
            span_count: 1,
            cell_span_offset: vec![0, 1, 1],
            spans: vec![Span {
                leaf_start: 0,
                leaf_count: 1,
            }],
        }
    }

    fn minimal_kinematic_geometry_section() -> KinematicGeometrySection {
        use postretro_level_format::kinematic_geometry::{
            KINEMATIC_GEOMETRY_VERSION, KinematicMoverRecord, KinematicWaypointRecord,
        };

        let geometry = sample_geo_result().geometry;
        KinematicGeometrySection {
            version: KINEMATIC_GEOMETRY_VERSION,
            movers: vec![KinematicMoverRecord {
                mover_id: 0,
                name: "lift_a".to_string(),
                tags: vec!["platform".to_string()],
                origin: [0.0, 0.0, 0.0],
                path: "wp_a".to_string(),
                speed: 1.0,
                wait_ms: 0.0,
                move_mode: 0,
                start_on_spawn: true,
                vertices: geometry.vertices,
                indices: geometry.indices,
                face_meta: geometry.faces,
                spin_axis: [0.0; 3],
                spin_speed_deg_s: 0.0,
                spin_accel_deg_s2: 0.0,
                carry_yaw: false,
                block_policy: "displace".to_string(),
                crush_damage: 0.0,
                crush_interval_ms: 0.0,
                auto_close_ms: None,
                open_event: None,
                close_event: None,
                blocked_event: None,
                crush_event: None,
                sealed_portal_ids: Vec::new(),
                carried_lights: Vec::new(),
            }],
            waypoints: vec![
                KinematicWaypointRecord {
                    name: "wp_a".to_string(),
                    next: "wp_b".to_string(),
                    origin: [0.0, 0.0, 0.0],
                },
                KinematicWaypointRecord {
                    name: "wp_b".to_string(),
                    next: String::new(),
                    origin: [0.0, 1.0, 0.0],
                },
            ],
        }
    }

    fn empty_alpha_lights() -> AlphaLightsSection {
        AlphaLightsSection::default()
    }

    fn empty_light_influence() -> LightInfluenceSection {
        LightInfluenceSection::default()
    }

    fn empty_sh_volume() -> OctahedralShVolumeSection {
        use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;

        OctahedralShVolumeSection {
            grid_origin: [0.0, 0.0, 0.0],
            cell_size: [1.0, 1.0, 1.0],
            grid_dimensions: [0, 0, 0],
            probe_stride: postretro_level_format::sh_volume::OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: postretro_level_format::octahedral::DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: postretro_level_format::octahedral::DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions: [0, 0],
            layer_count: 0,
            tiles_per_layer: 0,
            atlas_tiles_per_row: 0,
            probes: Vec::new(),
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            compact_atlas: Vec::new(),
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    fn minimal_direct_sh_volume() -> DirectShVolumeSection {
        use postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;
        use postretro_level_format::octahedral::{
            DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
            irradiance_atlas_array_layout,
        };

        let grid = [1, 1, 1];
        let tile_dimension = DEFAULT_IRRADIANCE_TILE_DIMENSION;
        let atlas_layout = irradiance_atlas_array_layout(grid, tile_dimension, 8192).unwrap();
        let atlas_dimensions = [atlas_layout.atlas_width, atlas_layout.atlas_height];
        let padded_w = atlas_dimensions[0].div_ceil(4) * 4;
        let padded_h = atlas_dimensions[1].div_ceil(4) * 4;
        let atlas_len =
            atlas_layout.layer_count as usize * (padded_w / 4 * padded_h / 4) as usize * 16;

        DirectShVolumeSection {
            grid_origin: [0.0, 0.0, 0.0],
            cell_size: [1.0, 1.0, 1.0],
            grid_dimensions: grid,
            tile_dimension,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions,
            layer_count: atlas_layout.layer_count,
            tiles_per_layer: atlas_layout.tiles_per_layer,
            atlas_tiles_per_row: atlas_layout.atlas_tiles_per_row,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
            atlas: vec![0; atlas_len],
        }
    }

    fn minimal_direct_sh_delta_volumes() -> DirectShDeltaVolumesSection {
        use postretro_level_format::delta_sh_volumes::{
            AFFINITY_FACTOR, DEFAULT_DELTA_PROBE_F16_STRIDE, PROBES_PER_CELL,
        };
        use postretro_level_format::octahedral::{
            DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
        };

        DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: vec![u64::MAX],
            cell_levels: vec![0u8; 1],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: vec![0; PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE],
        }
    }

    fn minimal_animated_direct_sh_delta_volumes() -> AnimatedDirectShDeltaVolumesSection {
        use postretro_level_format::delta_sh_volumes::{
            DEFAULT_DELTA_PROBE_F16_STRIDE, PROBES_PER_CELL,
        };

        AnimatedDirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: postretro_level_format::octahedral::DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: postretro_level_format::octahedral::DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![0],
            valid_probe_masks: vec![u64::MAX],
            cell_levels: vec![0u8; 1],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: vec![0; PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE],
        }
    }

    #[test]
    fn direct_delta_csr_shape_uses_the_per_cell_valid_probe_masks() {
        use postretro_level_format::delta_sh_volumes::DEFAULT_DELTA_PROBE_F16_STRIDE;

        let mut section = minimal_direct_sh_delta_volumes();
        section.affinity_dims = [2, 1, 1];
        section.valid_probe_masks = vec![(1u64 << 1) | (1u64 << 63), 0];
        section.cell_levels = vec![0u8; 2];
        section.affinity_offsets = vec![0, 1, 2];
        section.affinity_lights = vec![0, 0];
        section.delta_subblocks = vec![0; 2 * DEFAULT_DELTA_PROBE_F16_STRIDE];

        assert!(direct_sh_delta_has_valid_csr_shape(&section));

        section.delta_subblocks.push(0);
        assert!(
            !direct_sh_delta_has_valid_csr_shape(&section),
            "the pack guard must reject a dense-64 payload when the descriptor stores only two tiles"
        );
    }

    #[test]
    fn scatter_pack_guard_withholds_encoded_section_above_cap() {
        let section = AnimatedBillboardDirectScatterDeltaVolumesSection {
            animation_descriptor_indices: vec![0],
            affinity_factor: 4,
            affinity_dims: [1, 1, 1],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_rgba: vec![0; 64 * 4],
        };
        let encoded_bytes = section.encoded_len().expect("fixture encoded size");

        assert!(scatter_section_fits_pack_cap_with_limit(
            &section,
            encoded_bytes,
        ));
        assert!(!scatter_section_fits_pack_cap_with_limit(
            &section,
            encoded_bytes - 1,
        ));
    }

    fn placeholder_lightmap() -> LightmapSection {
        LightmapSection::placeholder()
    }

    fn minimal_shadowmask_atlas() -> ShadowmaskAtlasSection {
        ShadowmaskAtlasSection {
            width: 1,
            height: 1,
            layer_count: 1,
            channels: vec![0],
            data: vec![255; 4],
        }
    }

    fn placeholder_chunk_light_list() -> ChunkLightListSection {
        ChunkLightListSection::placeholder()
    }

    // Regression: changing the NavMesh body epoch accidentally changed the PRL table epoch.
    #[test]
    fn packed_navmesh_keeps_container_version_one_and_body_version_two() {
        let navmesh = NavMeshSection {
            version: postretro_level_format::navmesh::NAVMESH_VERSION,
            origin: [0.0, 0.0, 0.0],
            cell_size: 0.25,
            dim_x: 1,
            dim_z: 1,
            agent_radius: 0.4,
            agent_height: 1.8,
            step_height: 0.5,
            max_slope_deg: 45.0,
            regions: vec![postretro_level_format::navmesh::NavRegion {
                x0: 0,
                z0: 0,
                x1: 1,
                z1: 1,
                floor_y_min: 0.0,
                floor_y_max: 0.0,
            }],
            portals: Vec::new(),
        };
        let sections = vec![postretro_level_format::SectionBlob {
            section_id: SectionId::NavMesh as u32,
            version: NAVMESH_CONTAINER_VERSION,
            data: navmesh.to_bytes(),
        }];

        let mut prl_bytes = Vec::new();
        postretro_level_format::write_prl(&mut prl_bytes, &sections)
            .expect("navmesh PRL should serialize");

        let mut cursor = Cursor::new(&prl_bytes);
        let meta = read_container(&mut cursor).expect("navmesh PRL table should decode");
        let entry = meta
            .find_section(SectionId::NavMesh as u32)
            .expect("navmesh table entry should exist");
        assert_eq!(entry.version, 1, "E10 must not change the PRL table epoch");

        let body = read_section_data(&mut cursor, &meta, SectionId::NavMesh as u32)
            .expect("navmesh body should be readable")
            .expect("navmesh body should exist");
        let decoded = NavMeshSection::from_bytes(&body).expect("navmesh v2 body should decode");
        assert_eq!(decoded.version, 2, "E10 must pin portal handedness at v2");
    }

    #[test]
    fn encode_cells_preserves_leaf_ids_and_derives_sorted_unique_portal_refs() {
        let leaves = BspLeavesSection {
            leaves: vec![
                BspLeafRecord {
                    face_start: 5,
                    face_count: 2,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [1.0, 1.0, 1.0],
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 0,
                    face_count: 0,
                    bounds_min: [1.0, 0.0, 0.0],
                    bounds_max: [2.0, 1.0, 1.0],
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 99,
                    face_count: 0,
                    bounds_min: [2.0, 0.0, 0.0],
                    bounds_max: [3.0, 1.0, 1.0],
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 42,
                    face_count: 0,
                    bounds_min: [3.0, 0.0, 0.0],
                    bounds_max: [4.0, 1.0, 1.0],
                    is_solid: 1,
                },
            ],
        };
        let portals = PortalsSection {
            vertices: Vec::new(),
            portals: vec![
                PortalRecord {
                    vertex_start: 0,
                    vertex_count: 0,
                    front_leaf: 0,
                    back_leaf: 1,
                },
                PortalRecord {
                    vertex_start: 0,
                    vertex_count: 0,
                    front_leaf: 1,
                    back_leaf: 2,
                },
                PortalRecord {
                    vertex_start: 0,
                    vertex_count: 0,
                    front_leaf: 1,
                    back_leaf: 1,
                },
            ],
        };
        let exterior = HashSet::from([2usize]);

        let section = encode_cells(&leaves, &portals, &exterior).unwrap();

        assert_eq!(section.cells.len(), 4);
        assert_eq!(section.cells[0].flags, CELL_FLAG_DRAWABLE);
        assert_eq!(section.cells[0].face_start, 5);
        assert_eq!(section.cells[1].flags, 0, "empty interior is not exterior");
        assert_eq!(section.cells[2].flags, CELL_FLAG_EXTERIOR);
        assert_eq!(section.cells[2].face_start, 0);
        assert_eq!(section.cells[3].flags, CELL_FLAG_SOLID);
        assert_eq!(section.cells[3].face_start, 0);

        let cell_1_refs = &section.portal_refs[section.cells[1].portal_ref_start as usize
            ..(section.cells[1].portal_ref_start + section.cells[1].portal_ref_count) as usize];
        assert_eq!(cell_1_refs, &[0, 1, 2]);
    }

    #[test]
    fn pack_write_portals_produces_valid_prl_file() {
        let dir = std::env::temp_dir().join("postretro_test_pack");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pack_portals.prl");

        let geo_result = sample_geo_result();
        let leaves = sample_leaves();
        let portals = PortalsSection {
            vertices: vec![[32.0, 0.0, 0.0], [32.0, 64.0, 0.0], [32.0, 64.0, 64.0]],
            portals: vec![PortalRecord {
                vertex_start: 0,
                vertex_count: 3,
                front_leaf: 0,
                back_leaf: 1,
            }],
        };
        let bvh = sample_bvh();

        let alpha_lights = empty_alpha_lights();
        let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();
        let animated_direct_sh_delta_volumes = minimal_animated_direct_sh_delta_volumes();
        let cell_visibility_section = CellVisibilitySection {
            cell_count: 2,
            component_ids: vec![0, 0],
            coupled_pairs: vec![],
        };
        pack_and_write_portals(
            &output,
            &geo_result,
            &texture_cache_keys,
            &leaves,
            &sample_tree(),
            &portals,
            &HashSet::new(),
            &bvh,
            &[],
            &alpha_lights,
            &empty_light_influence(),
            &empty_sh_volume(),
            None,
            None,
            None,
            None,
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
            None,
            None,
            &FogVolumesSection::default(),
            None,
            None,
            None,
            None,
            None,
            Some(&sample_cell_draw_index_section()),
            Some(&cell_visibility_section),
            Some(&animated_direct_sh_delta_volumes),
        )
        .expect("pack_and_write_portals should succeed");

        let data = std::fs::read(&output).expect("should read output file");
        assert_eq!(&data[0..4], b"PRL\0");

        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        // Baseline modern sections plus CellVisibility, section 45,
        // always-emitted FogVolumes, and the required CellDrawIndex.
        assert_eq!(meta.header.section_count, 16);

        assert!(meta.find_section(SectionId::Geometry as u32).is_some());
        assert!(meta.find_section(SectionId::TextureNames as u32).is_some());
        assert!(
            meta.find_section(SectionId::TextureCacheKeys as u32)
                .is_some()
        );
        assert!(meta.find_section(SectionId::BspNodes as u32).is_none());
        assert!(meta.find_section(SectionId::BspLeaves as u32).is_none());
        assert!(meta.find_section(SectionId::Cells as u32).is_some());
        assert!(meta.find_section(SectionId::CellLocator as u32).is_some());
        assert!(meta.find_section(SectionId::Portals as u32).is_some());
        assert!(meta.find_section(SectionId::Bvh as u32).is_some());
        assert!(meta.find_section(SectionId::CellDrawIndex as u32).is_some());
        assert!(
            meta.find_section(SectionId::CellVisibility as u32)
                .is_some()
        );
        assert!(meta.find_section(SectionId::AlphaLights as u32).is_some());
        assert!(
            meta.find_section(SectionId::LightInfluence as u32)
                .is_some()
        );
        assert!(
            meta.find_section(SectionId::OctahedralShVolume as u32)
                .is_some()
        );
        assert!(meta.find_section(SectionId::Lightmap as u32).is_some());
        assert!(
            meta.find_section(SectionId::AnimatedDirectShDeltaVolumes as u32)
                .is_some(),
            "the baked animated direct delta must reach PRL serialization"
        );
        assert!(
            meta.find_section(SectionId::DirectShVolume as u32)
                .is_none(),
            "the section-45-only path must not require a static direct-SH base"
        );

        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn pack_write_emits_kinematic_geometry_section_when_present() {
        let dir = std::env::temp_dir().join("postretro_test_pack");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pack_kinematic_geometry.prl");

        let geo_result = sample_geo_result();
        let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();
        let kinematic = minimal_kinematic_geometry_section();
        pack_and_write_portals(
            &output,
            &geo_result,
            &texture_cache_keys,
            &sample_leaves(),
            &sample_tree(),
            &PortalsSection {
                vertices: vec![],
                portals: vec![],
            },
            &HashSet::new(),
            &sample_bvh(),
            &[],
            &empty_alpha_lights(),
            &empty_light_influence(),
            &empty_sh_volume(),
            None,
            None,
            None,
            None,
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
            None,
            None,
            &FogVolumesSection::default(),
            None,
            None,
            None,
            Some(&kinematic),
            None,
            Some(&sample_cell_draw_index_section()),
            None,
            None,
        )
        .expect("pack should succeed");

        let data = std::fs::read(&output).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert!(
            meta.find_section(SectionId::KinematicGeometry as u32)
                .is_some()
        );

        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn pack_write_emits_entity_shadow_lights_only_with_direct_sh_and_usable_deltas() {
        fn write_with(
            output: &Path,
            direct_sh_volume: Option<&DirectShVolumeSection>,
            entity_shadow_lights: Option<&EntityShadowLightsSection>,
            direct_sh_delta_volumes: Option<&DirectShDeltaVolumesSection>,
        ) {
            let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();
            pack_and_write_portals(
                output,
                &sample_geo_result(),
                &texture_cache_keys,
                &sample_leaves(),
                &sample_tree(),
                &PortalsSection {
                    vertices: vec![],
                    portals: vec![],
                },
                &HashSet::new(),
                &sample_bvh(),
                &[],
                &empty_alpha_lights(),
                &empty_light_influence(),
                &empty_sh_volume(),
                direct_sh_volume,
                entity_shadow_lights,
                direct_sh_delta_volumes,
                None,
                &placeholder_lightmap(),
                &placeholder_chunk_light_list(),
                None,
                None,
                None,
                None,
                None,
                None,
                &FogVolumesSection::default(),
                None,
                None,
                None,
                None,
                None,
                Some(&sample_cell_draw_index_section()),
                None,
                None,
            )
            .expect("pack should succeed");
        }

        let dir = std::env::temp_dir().join("postretro_test_pack");
        let _ = std::fs::create_dir_all(&dir);
        let direct = minimal_direct_sh_volume();
        let selected = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let delta = minimal_direct_sh_delta_volumes();

        let output_with_direct = dir.join("test_pack_entity_shadow_with_direct.prl");
        write_with(
            &output_with_direct,
            Some(&direct),
            Some(&selected),
            Some(&delta),
        );
        let data = std::fs::read(&output_with_direct).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert!(
            meta.find_section(SectionId::EntityShadowLights as u32)
                .is_some()
        );

        let output_without_delta = dir.join("test_pack_entity_shadow_without_delta.prl");
        write_with(&output_without_delta, Some(&direct), Some(&selected), None);
        let data = std::fs::read(&output_without_delta).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert!(
            meta.find_section(SectionId::EntityShadowLights as u32)
                .is_none()
        );

        let output_without_direct = dir.join("test_pack_entity_shadow_without_direct.prl");
        write_with(&output_without_direct, None, Some(&selected), Some(&delta));
        let data = std::fs::read(&output_without_direct).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert!(
            meta.find_section(SectionId::EntityShadowLights as u32)
                .is_none()
        );

        let empty_selection = EntityShadowLightsSection {
            light_indices: Vec::new(),
        };
        let output_empty = dir.join("test_pack_entity_shadow_empty_selection.prl");
        write_with(&output_empty, Some(&direct), Some(&empty_selection), None);
        let data = std::fs::read(&output_empty).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert!(
            meta.find_section(SectionId::EntityShadowLights as u32)
                .is_none()
        );

        let _ = std::fs::remove_file(&output_with_direct);
        let _ = std::fs::remove_file(&output_without_delta);
        let _ = std::fs::remove_file(&output_without_direct);
        let _ = std::fs::remove_file(&output_empty);
    }

    #[test]
    fn pack_write_emits_direct_sh_delta_only_with_direct_sh_and_selection() {
        fn write_with(
            output: &Path,
            direct_sh_volume: Option<&DirectShVolumeSection>,
            entity_shadow_lights: Option<&EntityShadowLightsSection>,
            direct_sh_delta_volumes: Option<&DirectShDeltaVolumesSection>,
        ) {
            let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();
            pack_and_write_portals(
                output,
                &sample_geo_result(),
                &texture_cache_keys,
                &sample_leaves(),
                &sample_tree(),
                &PortalsSection {
                    vertices: vec![],
                    portals: vec![],
                },
                &HashSet::new(),
                &sample_bvh(),
                &[],
                &empty_alpha_lights(),
                &empty_light_influence(),
                &empty_sh_volume(),
                direct_sh_volume,
                entity_shadow_lights,
                direct_sh_delta_volumes,
                None,
                &placeholder_lightmap(),
                &placeholder_chunk_light_list(),
                None,
                None,
                None,
                None,
                None,
                None,
                &FogVolumesSection::default(),
                None,
                None,
                None,
                None,
                None,
                Some(&sample_cell_draw_index_section()),
                None,
                None,
            )
            .expect("pack should succeed");
        }

        let dir = std::env::temp_dir().join("postretro_test_pack");
        let _ = std::fs::create_dir_all(&dir);
        let direct = minimal_direct_sh_volume();
        let selected = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let delta = minimal_direct_sh_delta_volumes();

        let output_all = dir.join("test_pack_direct_delta_all_inputs.prl");
        write_with(&output_all, Some(&direct), Some(&selected), Some(&delta));
        let data = std::fs::read(&output_all).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert!(
            meta.find_section(SectionId::DirectShDeltaVolumes as u32)
                .is_some()
        );

        let output_no_selection = dir.join("test_pack_direct_delta_no_selection.prl");
        write_with(&output_no_selection, Some(&direct), None, Some(&delta));
        let data = std::fs::read(&output_no_selection).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert!(
            meta.find_section(SectionId::DirectShDeltaVolumes as u32)
                .is_none()
        );

        let output_no_direct = dir.join("test_pack_direct_delta_no_direct.prl");
        write_with(&output_no_direct, None, Some(&selected), Some(&delta));
        let data = std::fs::read(&output_no_direct).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert!(
            meta.find_section(SectionId::DirectShDeltaVolumes as u32)
                .is_none()
        );

        let partial_selection = EntityShadowLightsSection {
            light_indices: vec![0, 1],
        };
        let output_partial = dir.join("test_pack_direct_delta_partial_selection.prl");
        write_with(
            &output_partial,
            Some(&direct),
            Some(&partial_selection),
            Some(&delta),
        );
        let data = std::fs::read(&output_partial).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert!(
            meta.find_section(SectionId::DirectShDeltaVolumes as u32)
                .is_none()
        );
        assert!(
            meta.find_section(SectionId::EntityShadowLights as u32)
                .is_none()
        );

        let _ = std::fs::remove_file(&output_all);
        let _ = std::fs::remove_file(&output_no_selection);
        let _ = std::fs::remove_file(&output_no_direct);
        let _ = std::fs::remove_file(&output_partial);
    }

    #[test]
    fn pack_write_emits_shadowmask_only_with_usable_entity_shadow_selection() {
        fn write_with(
            output: &Path,
            direct_sh_volume: Option<&DirectShVolumeSection>,
            entity_shadow_lights: Option<&EntityShadowLightsSection>,
            direct_sh_delta_volumes: Option<&DirectShDeltaVolumesSection>,
            shadowmask_atlas: Option<&ShadowmaskAtlasSection>,
        ) {
            let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();
            pack_and_write_portals(
                output,
                &sample_geo_result(),
                &texture_cache_keys,
                &sample_leaves(),
                &sample_tree(),
                &PortalsSection {
                    vertices: vec![],
                    portals: vec![],
                },
                &HashSet::new(),
                &sample_bvh(),
                &[],
                &empty_alpha_lights(),
                &empty_light_influence(),
                &empty_sh_volume(),
                direct_sh_volume,
                entity_shadow_lights,
                direct_sh_delta_volumes,
                shadowmask_atlas,
                &placeholder_lightmap(),
                &placeholder_chunk_light_list(),
                None,
                None,
                None,
                None,
                None,
                None,
                &FogVolumesSection::default(),
                None,
                None,
                None,
                None,
                None,
                Some(&sample_cell_draw_index_section()),
                None,
                None,
            )
            .expect("pack should succeed");
        }

        let dir = std::env::temp_dir().join("postretro_test_pack");
        let _ = std::fs::create_dir_all(&dir);
        let direct = minimal_direct_sh_volume();
        let selected = EntityShadowLightsSection {
            light_indices: vec![0],
        };
        let delta = minimal_direct_sh_delta_volumes();
        let shadowmask = minimal_shadowmask_atlas();

        let output_valid = dir.join("test_pack_shadowmask_valid.prl");
        write_with(
            &output_valid,
            Some(&direct),
            Some(&selected),
            Some(&delta),
            Some(&shadowmask),
        );
        let data = std::fs::read(&output_valid).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert!(
            meta.find_section(SectionId::ShadowmaskAtlas as u32)
                .is_some()
        );

        let output_no_delta = dir.join("test_pack_shadowmask_no_delta.prl");
        write_with(
            &output_no_delta,
            Some(&direct),
            Some(&selected),
            None,
            Some(&shadowmask),
        );
        let data = std::fs::read(&output_no_delta).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert!(
            meta.find_section(SectionId::ShadowmaskAtlas as u32)
                .is_none()
        );

        let _ = std::fs::remove_file(&output_valid);
        let _ = std::fs::remove_file(&output_no_delta);
    }

    #[test]
    fn pack_write_omits_entity_shadow_sections_for_malformed_direct_delta_csr() {
        fn write_with_delta(output: &Path, delta: &DirectShDeltaVolumesSection) {
            let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();
            let direct = minimal_direct_sh_volume();
            let selected = EntityShadowLightsSection {
                light_indices: vec![0],
            };
            pack_and_write_portals(
                output,
                &sample_geo_result(),
                &texture_cache_keys,
                &sample_leaves(),
                &sample_tree(),
                &PortalsSection {
                    vertices: vec![],
                    portals: vec![],
                },
                &HashSet::new(),
                &sample_bvh(),
                &[],
                &empty_alpha_lights(),
                &empty_light_influence(),
                &empty_sh_volume(),
                Some(&direct),
                Some(&selected),
                Some(delta),
                None,
                &placeholder_lightmap(),
                &placeholder_chunk_light_list(),
                None,
                None,
                None,
                None,
                None,
                None,
                &FogVolumesSection::default(),
                None,
                None,
                None,
                None,
                None,
                Some(&sample_cell_draw_index_section()),
                None,
                None,
            )
            .expect("pack should succeed");
        }

        fn assert_shadow_sections_omitted(output: &Path) {
            let data = std::fs::read(output).expect("should read output file");
            let mut cursor = Cursor::new(&data);
            let meta = read_container(&mut cursor).expect("should read container");
            assert!(
                meta.find_section(SectionId::EntityShadowLights as u32)
                    .is_none()
            );
            assert!(
                meta.find_section(SectionId::DirectShDeltaVolumes as u32)
                    .is_none()
            );
        }

        let dir = std::env::temp_dir().join("postretro_test_pack");
        let _ = std::fs::create_dir_all(&dir);
        let base_delta = minimal_direct_sh_delta_volumes();

        let cases = [
            ("bad_offset_len", {
                let mut delta = base_delta.clone();
                delta.affinity_offsets = vec![0];
                delta
            }),
            ("bad_first_offset", {
                let mut delta = base_delta.clone();
                delta.affinity_offsets = vec![1, 1];
                delta
            }),
            ("bad_trailing_offset", {
                let mut delta = base_delta.clone();
                delta.affinity_offsets = vec![0, 2];
                delta
            }),
            ("bad_subblock_len", {
                let mut delta = base_delta.clone();
                delta.delta_subblocks.pop();
                delta
            }),
        ];

        for (name, delta) in cases {
            let output = dir.join(format!("test_pack_direct_delta_{name}.prl"));
            write_with_delta(&output, &delta);
            assert_shadow_sections_omitted(&output);
            let _ = std::fs::remove_file(&output);
        }
    }

    #[test]
    fn pack_write_rejects_missing_cell_draw_index_for_non_empty_bvh() {
        let dir = std::env::temp_dir().join("postretro_test_pack");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pack_missing_cell_draw_index.prl");

        let geo_result = sample_geo_result();
        let leaves = sample_leaves();
        let portals = PortalsSection {
            vertices: vec![],
            portals: vec![],
        };
        let bvh = sample_bvh();
        let alpha_lights = empty_alpha_lights();
        let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();

        let result = pack_and_write_portals(
            &output,
            &geo_result,
            &texture_cache_keys,
            &leaves,
            &sample_tree(),
            &portals,
            &HashSet::new(),
            &bvh,
            &[],
            &alpha_lights,
            &empty_light_influence(),
            &empty_sh_volume(),
            None,
            None,
            None,
            None,
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
            None,
            None,
            &FogVolumesSection::default(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        let msg = result.expect_err("non-empty BVH without CellDrawIndex must fail");
        assert!(
            msg.to_string()
                .contains("CellDrawIndex section is required"),
            "got: {msg}"
        );
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn pack_write_allows_empty_bvh_without_cell_draw_index() {
        let dir = std::env::temp_dir().join("postretro_test_pack");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pack_empty_bvh_no_cell_draw_index.prl");

        let geo_result = empty_geo_result();
        let leaves = empty_draw_leaves();
        let portals = PortalsSection {
            vertices: vec![],
            portals: vec![],
        };
        let bvh = empty_bvh();
        let alpha_lights = empty_alpha_lights();
        let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();

        pack_and_write_portals(
            &output,
            &geo_result,
            &texture_cache_keys,
            &leaves,
            &sample_tree(),
            &portals,
            &HashSet::new(),
            &bvh,
            &[],
            &alpha_lights,
            &empty_light_influence(),
            &empty_sh_volume(),
            None,
            None,
            None,
            None,
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
            None,
            None,
            &FogVolumesSection::default(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("empty BVH may omit CellDrawIndex");

        let data = std::fs::read(&output).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert!(meta.find_section(SectionId::Bvh as u32).is_some());
        assert!(meta.find_section(SectionId::CellDrawIndex as u32).is_none());
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn pack_write_rejects_nonexistent_directory() {
        let output = Path::new("/nonexistent/deeply/nested/dir/test.prl");
        let geo_result = sample_geo_result();
        let leaves = sample_leaves();
        let portals = PortalsSection {
            vertices: vec![],
            portals: vec![],
        };
        let bvh = sample_bvh();
        let alpha_lights = empty_alpha_lights();
        let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();

        let result = pack_and_write_portals(
            output,
            &geo_result,
            &texture_cache_keys,
            &leaves,
            &sample_tree(),
            &portals,
            &HashSet::new(),
            &bvh,
            &[],
            &alpha_lights,
            &empty_light_influence(),
            &empty_sh_volume(),
            None,
            None,
            None,
            None,
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
            None,
            None,
            &FogVolumesSection::default(),
            None,
            None,
            None,
            None,
            None,
            Some(&sample_cell_draw_index_section()),
            None,
            None,
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("output directory does not exist"),
            "expected directory error, got: {msg}"
        );
    }

    #[test]
    fn full_pipeline_portal_mode_produces_valid_prl() {
        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .join("content/dev/maps/campaign-test.map");

        let map_data =
            crate::parse::parse_map_file(&map_path, crate::map_format::MapFormat::IdTech2)
                .expect("campaign-test.map should parse");
        let result =
            crate::partition::partition(&map_data.brush_volumes).expect("partition should succeed");

        let exterior = std::collections::HashSet::new();
        let geo_result = crate::geometry::extract_geometry(&result.faces, &result.tree, &exterior);
        let generated_portals = crate::portals::generate_portals(&result.tree);
        let vis_result = crate::visibility::encode_vis(&result.tree, &exterior);

        let (bvh, primitives, bvh_section) =
            crate::bvh_build::build_bvh(&geo_result).expect("bvh build should succeed");

        let static_lights =
            crate::light_namespaces::StaticBakedLights::from_lights(&map_data.lights);
        let animated_lights =
            crate::light_namespaces::AnimatedBakedLights::from_lights(&map_data.lights);
        let alpha_ns = crate::light_namespaces::AlphaLightsNs::from_lights(&map_data.lights);
        let sh_inputs = crate::sh_bake::ShBakeCtx {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geo_result,
            tree: &result.tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: map_data.lights.len(),
        };
        let sh_volume = crate::sh_bake::bake_sh_volume(
            &sh_inputs,
            &crate::sh_bake::ShConfig { probe_spacing: 4.0 },
        );

        let portals_section = encode_portals(&generated_portals);

        let dir = std::env::temp_dir().join("postretro_test_pipeline");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pipeline_portals.prl");

        let alpha_lights = encode_alpha_lights(&alpha_ns, &result.tree);
        let light_influence = encode_light_influence(&alpha_ns);
        let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();
        let cell_draw_index_section = crate::cell_draw_index_bake::bake_cell_draw_index(
            &bvh_section.leaves,
            &vis_result.leaves_section.leaves,
        );
        pack_and_write_portals(
            &output,
            &geo_result,
            &texture_cache_keys,
            &vis_result.leaves_section,
            &result.tree,
            &portals_section,
            &exterior,
            &bvh_section,
            &[],
            &alpha_lights,
            &light_influence,
            &sh_volume,
            None,
            None,
            None,
            None,
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
            None,
            None,
            &FogVolumesSection::default(),
            None,
            None,
            None,
            None,
            None,
            cell_draw_index_section.as_ref(),
            None,
            None,
        )
        .expect("full pipeline portal pack should succeed");

        let data = std::fs::read(&output).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");

        // Baseline modern sections plus always-emitted FogVolumes and required CellDrawIndex.
        assert_eq!(meta.header.section_count, 14);
        assert!(meta.find_section(SectionId::Geometry as u32).is_some());
        assert!(meta.find_section(SectionId::TextureNames as u32).is_some());
        assert!(
            meta.find_section(SectionId::TextureCacheKeys as u32)
                .is_some()
        );
        assert!(meta.find_section(SectionId::Portals as u32).is_some());
        assert!(meta.find_section(SectionId::Bvh as u32).is_some());
        assert!(meta.find_section(SectionId::CellDrawIndex as u32).is_some());
        assert!(meta.find_section(SectionId::AlphaLights as u32).is_some());
        assert!(
            meta.find_section(SectionId::LightInfluence as u32)
                .is_some()
        );
        assert!(
            meta.find_section(SectionId::OctahedralShVolume as u32)
                .is_some()
        );
        assert!(meta.find_section(SectionId::Lightmap as u32).is_some());
        assert!(meta.find_section(SectionId::BspNodes as u32).is_none());
        assert!(meta.find_section(SectionId::BspLeaves as u32).is_none());
        assert!(meta.find_section(SectionId::Cells as u32).is_some());
        assert!(meta.find_section(SectionId::CellLocator as u32).is_some());
        assert!(meta.find_section(SectionId::FogVolumes as u32).is_some());

        let _ = std::fs::remove_file(&output);
    }

    // Regression: the closet fixture's compiler-generated portal ids were never
    // exercised across PRL packing and runtime loading, so portal-order drift
    // could silently make a closed door block the wrong portal.
    #[test]
    fn closet_reveal_compiler_loader_portal_ids_block_and_restore_interior() {
        use glam::{Mat4, Vec3};
        use postretro_visibility::VisibleCells;

        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|path| path.parent())
            .expect("workspace root")
            .join("content/dev/maps/closet-reveal.map");
        let map_data =
            crate::parse::parse_map_file(&map_path, crate::map_format::MapFormat::IdTech2)
                .expect("closet-reveal.map should parse");
        let result =
            crate::partition::partition(&map_data.brush_volumes).expect("partition should succeed");
        let generated_portals = crate::portals::generate_portals(&result.tree);
        let exterior = crate::visibility::find_exterior_leaves(&result.tree, &generated_portals);
        let vis_result = crate::visibility::encode_vis(&result.tree, &exterior);
        let mut geo_result =
            crate::geometry::extract_geometry(&result.faces, &result.tree, &exterior);
        let kinematic_geometry = crate::kinematic_geometry::encode_kinematic_geometry_section(
            &map_data.kinematic_movers,
            &map_data.kinematic_waypoints,
            &[],
            &generated_portals,
            &mut geo_result.texture_names,
        )
        .expect("closet fixture should emit kinematic geometry");
        let (_, _, bvh_section) =
            crate::bvh_build::build_bvh(&geo_result).expect("BVH build should succeed");
        let cell_draw_index_section = crate::cell_draw_index_bake::bake_cell_draw_index(
            &bvh_section.leaves,
            &vis_result.leaves_section.leaves,
        );
        let alpha_ns = crate::light_namespaces::AlphaLightsNs::from_lights(&map_data.lights);
        let alpha_lights = encode_alpha_lights(&alpha_ns, &result.tree);
        let light_influence = encode_light_influence(&alpha_ns);
        let map_entities = encode_map_entities(&map_data.map_entities);
        let portals = encode_portals(&generated_portals);
        let texture_cache_keys = HashMap::new();

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should follow Unix epoch")
            .as_nanos();
        let output = std::env::temp_dir().join(format!(
            "postretro-closet-reveal-{}-{unique}.prl",
            std::process::id()
        ));
        // Lighting is unrelated to this seam. Existing valid placeholders keep
        // the regression on the production geometry/portal/pack/load path
        // without turning it into a cold-bake test.
        pack_and_write_portals(
            &output,
            &geo_result,
            &texture_cache_keys,
            &vis_result.leaves_section,
            &result.tree,
            &portals,
            &exterior,
            &bvh_section,
            &[],
            &alpha_lights,
            &light_influence,
            &empty_sh_volume(),
            None,
            None,
            None,
            None,
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
            None,
            map_entities.as_ref(),
            &FogVolumesSection::default(),
            None,
            None,
            None,
            Some(&kinematic_geometry),
            None,
            cell_draw_index_section.as_ref(),
            None,
            None,
        )
        .expect("closet fixture should pack into a loader-valid PRL");

        let world = postretro_level_loader::load_prl(
            output
                .to_str()
                .expect("temporary PRL path should be valid UTF-8"),
        )
        .expect("production loader should read the compiled closet PRL");
        std::fs::remove_file(&output).expect("temporary closet PRL should be removable");

        let mover = world
            .kinematic_geometry
            .movers
            .iter()
            .find(|mover| mover.name == "closet_door")
            .expect("loaded closet door mover");
        assert!(
            !mover.sealed_portal_ids.is_empty(),
            "the production compiler must associate the closed closet door with a portal"
        );
        assert!(
            mover
                .sealed_portal_ids
                .iter()
                .all(|&portal_id| (portal_id as usize) < world.portals.len()),
            "every loaded association must index the loaded portal array"
        );

        let player_position = Vec3::from(
            world
                .map_entities
                .iter()
                .find(|entity| entity.classname == "player_spawn")
                .expect("loaded player spawn")
                .origin,
        );
        let closet_position = Vec3::from(
            world
                .map_entities
                .iter()
                .find(|entity| entity.classname == "reference_enemy")
                .expect("loaded closet enemy")
                .origin,
        );
        let player_cell = world.locate_cell(player_position) as u32;
        let closet_cell = world.locate_cell(closet_position) as u32;
        assert_ne!(
            player_cell, closet_cell,
            "fixture must straddle the closet door"
        );

        let view_direction = (closet_position - player_position).normalize();
        let view = Mat4::look_at_rh(player_position, player_position + view_direction, Vec3::Y);
        let view_proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.01, 100.0) * view;
        let visible_ids = |blocked_portals: &[bool]| {
            let (result, _) = postretro_visibility::determine_visible_cells(
                player_position,
                view_proj,
                &world,
                blocked_portals,
                false,
                &mut Vec::new(),
            );
            match result.visible_cells {
                VisibleCells::Culled(ids) => ids,
                VisibleCells::DrawAll => panic!("closet fixture must use portal visibility"),
            }
        };

        let open_visible = visible_ids(&[]);
        assert!(
            open_visible.contains(&closet_cell),
            "the loaded closet interior must be visible through the unblocked portal"
        );

        let mut blocked_portals = vec![false; world.portals.len()];
        for &portal_id in &mover.sealed_portal_ids {
            blocked_portals[portal_id as usize] = true;
        }
        let closed_visible = visible_ids(&blocked_portals);
        assert!(
            closed_visible.contains(&player_cell),
            "blocking the closet door must retain the camera cell"
        );
        assert!(
            !closed_visible.contains(&closet_cell),
            "blocking compiler-associated portal ids must hide the loaded closet interior"
        );
    }

    /// A curated set of small fixture maps must compile end-to-end and emit an
    /// SH volume section. The bake uses a coarse spacing (4 m) to keep test
    /// time bounded — the probe count is a design parameter, not what this test
    /// is exercising.
    ///
    /// This is a fixture smoke test, not full-map coverage. The large perf/stress
    /// maps (`stress-warren*`, `campaign-test`, `occlusion-test`) are deliberately
    /// absent: they stress the runtime, and baking them here costs minutes
    /// without adding SH-bake coverage these small maps don't already give.
    #[test]
    fn small_fixture_maps_compile_with_sh_section() {
        // Small fixtures that exercise the SH bake quickly. Add new small maps
        // here as needed; do NOT add perf/stress maps — their bake time would
        // dominate this smoke test.
        const FIXTURE_MAPS: &[&str] = &[
            "combat-demo.map",
            "gate-heavily-lit.map",
            "soft_shadow_test.map",
            "anim-demo.map",
            "test_animated_weight_maps_single.map",
            "test_animated_weight_maps_mixed.map",
            "test_animated_weight_maps_cap.map",
            "test_animated_weight_maps_occluded.map",
        ];

        let maps_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .join("content/dev/maps");

        for map_name in FIXTURE_MAPS {
            let path = maps_dir.join(map_name);
            assert!(
                path.exists(),
                "fixture map {} is missing; update FIXTURE_MAPS",
                path.display()
            );
            let map_data =
                crate::parse::parse_map_file(&path, crate::map_format::MapFormat::IdTech2)
                    .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
            let result = crate::partition::partition(&map_data.brush_volumes)
                .unwrap_or_else(|e| panic!("failed to partition {}: {e}", path.display()));
            let exterior = std::collections::HashSet::new();
            let geo_result =
                crate::geometry::extract_geometry(&result.faces, &result.tree, &exterior);
            let (bvh, primitives, _) = crate::bvh_build::build_bvh(&geo_result)
                .unwrap_or_else(|e| panic!("bvh build failed on {}: {e}", path.display()));

            let static_lights =
                crate::light_namespaces::StaticBakedLights::from_lights(&map_data.lights);
            let animated_lights =
                crate::light_namespaces::AnimatedBakedLights::from_lights(&map_data.lights);
            let sh_inputs = crate::sh_bake::ShBakeCtx {
                bvh: &bvh,
                primitives: &primitives,
                geometry: &geo_result,
                tree: &result.tree,
                exterior_leaves: &exterior,
                static_lights: &static_lights,
                animated_lights: &animated_lights,
                total_light_count: map_data.lights.len(),
            };
            let section = crate::sh_bake::bake_sh_volume(
                &sh_inputs,
                &crate::sh_bake::ShConfig { probe_spacing: 4.0 },
            );

            // Every real test map has geometry, so the grid must have at
            // least 1 probe along each axis, and the section must round-trip.
            let dims = section.grid_dimensions;
            assert!(
                dims[0] > 0 && dims[1] > 0 && dims[2] > 0,
                "{} produced an empty SH grid",
                path.display()
            );
            let bytes = section.to_bytes();
            let restored =
                postretro_level_format::sh_volume::OctahedralShVolumeSection::from_bytes(&bytes)
                    .unwrap_or_else(|e| {
                        panic!("sh volume round-trip failed for {}: {e}", path.display())
                    });
            assert_eq!(section, restored);
        }
    }

    #[test]
    fn encode_light_influence_derives_correct_bounds() {
        use crate::map_data::{FalloffModel, LightType, MapLight};
        use glam::DVec3;

        let lights = vec![
            MapLight {
                origin: DVec3::new(10.0, 20.0, 30.0),
                carrier: String::new(),
                light_type: LightType::Point,
                intensity: 1.0,
                color: [1.0, 1.0, 1.0],
                falloff_model: FalloffModel::InverseSquared,
                falloff_range: 50.0,
                light_size: 0.0,
                angular_diameter: 0.0,
                cone_angle_inner: None,
                cone_angle_outer: None,
                cone_direction: None,
                animation: None,
                bake_only: false,
                is_dynamic: false,
                casts_entity_shadows: false,
                is_animated: false,
                tags: vec![],
                shadow_type: crate::map_data::ShadowType::StaticLightMap,
            },
            MapLight {
                origin: DVec3::new(-4.0, 1.0, 0.5),
                carrier: String::new(),
                light_type: LightType::Spot,
                intensity: 1.5,
                color: [1.0, 0.8, 0.6],
                falloff_model: FalloffModel::Linear,
                falloff_range: 25.0,
                light_size: 0.0,
                angular_diameter: 0.0,
                cone_angle_inner: Some(0.5),
                cone_angle_outer: Some(0.8),
                cone_direction: Some([0.0, -1.0, 0.0]),
                animation: None,
                bake_only: false,
                is_dynamic: false,
                casts_entity_shadows: false,
                is_animated: false,
                tags: vec![],
                shadow_type: crate::map_data::ShadowType::StaticLightMap,
            },
            MapLight {
                origin: DVec3::new(0.0, 100.0, 0.0),
                carrier: String::new(),
                light_type: LightType::Directional,
                intensity: 0.9,
                color: [0.9, 0.95, 1.0],
                falloff_model: FalloffModel::Linear,
                falloff_range: 0.0,
                light_size: 0.0,
                angular_diameter: 0.0,
                cone_angle_inner: None,
                cone_angle_outer: None,
                cone_direction: Some([0.0, -1.0, 0.0]),
                animation: None,
                bake_only: false,
                is_dynamic: false,
                casts_entity_shadows: false,
                is_animated: false,
                tags: vec![],
                shadow_type: crate::map_data::ShadowType::StaticLightMap,
            },
        ];

        let alpha_ns = crate::light_namespaces::AlphaLightsNs::from_lights(&lights);
        let section = encode_light_influence(&alpha_ns);
        assert_eq!(section.records.len(), 3);

        // Point: center = position (f64→f32), radius = falloff_range.
        assert_eq!(section.records[0].center, [10.0, 20.0, 30.0]);
        assert_eq!(section.records[0].radius, 50.0);

        // Spot: same derivation as Point.
        assert_eq!(section.records[1].center, [-4.0, 1.0, 0.5]);
        assert_eq!(section.records[1].radius, 25.0);

        // Directional: center zeroed, radius = f32::MAX sentinel.
        assert_eq!(section.records[2].center, [0.0, 0.0, 0.0]);
        assert_eq!(section.records[2].radius, f32::MAX);
    }

    #[test]
    fn encode_alpha_lights_assigns_leaf_indices_and_flags_solid_leaf_lights() {
        use crate::map_data::{FalloffModel, LightType, MapLight};
        use crate::partition::{Aabb, BspChild, BspLeaf, BspNode, BspTree};
        use glam::DVec3;

        // Trivial tree: split on X = 0; back leaf (0) is empty, front leaf (1)
        // is solid. A light at +X lands in the solid leaf (sentinel); a light
        // at -X lands in the empty leaf (real index).
        let tree = BspTree {
            nodes: vec![BspNode {
                plane_normal: DVec3::X,
                plane_distance: 0.0,
                front: BspChild::Leaf(1),
                back: BspChild::Leaf(0),
                parent: None,
            }],
            leaves: vec![
                BspLeaf {
                    face_indices: vec![],
                    bounds: Aabb::empty(),
                    is_solid: false,
                    defining_planes: Vec::new(),
                },
                BspLeaf {
                    face_indices: vec![],
                    bounds: Aabb::empty(),
                    is_solid: true,
                    defining_planes: Vec::new(),
                },
            ],
        };

        let mk = |origin: DVec3| MapLight {
            origin,
            carrier: String::new(),
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 10.0,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        };

        let lights = vec![
            mk(DVec3::new(-5.0, 0.0, 0.0)),
            mk(DVec3::new(5.0, 0.0, 0.0)),
        ];

        let alpha_ns = crate::light_namespaces::AlphaLightsNs::from_lights(&lights);
        let section = encode_alpha_lights(&alpha_ns, &tree);
        assert_eq!(section.lights.len(), 2);
        assert_eq!(section.lights[0].leaf_index, 0);
        assert_eq!(
            section.lights[1].leaf_index,
            postretro_level_format::alpha_lights::ALPHA_LIGHT_LEAF_UNASSIGNED
        );
    }
}
