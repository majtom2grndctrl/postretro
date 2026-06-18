// PRL level loading: reads .prl files, populates LevelWorld (BSP, BVH, lights,
// portals, fog volumes, scripted entities, and worldspawn metadata).
// See: context/lib/build_pipeline.md §PRL Compilation

use std::collections::HashSet;
use std::path::Path;

use glam::Vec3;
use postretro_level_format::alpha_lights::{
    AlphaFalloffModel, AlphaLightType, AlphaLightsSection, AlphaShadowType,
};
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::bsp::{BspLeavesSection, BspNodesSection};
use postretro_level_format::bvh::{BVH_NODE_FLAG_LEAF, BvhSection};
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::data_script::DataScriptSection;
use postretro_level_format::delta_sh_volumes::{AFFINITY_FACTOR, DeltaShVolumesSection};
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::fog_cell_masks::FogCellMasksSection;
use postretro_level_format::fog_volumes::{FogVolumeRecord, FogVolumesSection};
use postretro_level_format::geometry::{GeometrySection, NO_TEXTURE};
use postretro_level_format::light_influence::LightInfluenceSection;
use postretro_level_format::light_tags::LightTagsSection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::map_entity::{MapEntityRecord, MapEntitySection};
use postretro_level_format::navmesh::NavMeshSection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::sdf_atlas::SdfAtlasSection;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::texture_names::TextureNamesSection;
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
    #[error(
        "PRL file is missing the worldspawn `initialGravity` value (carried in the FogVolumes section, required since M7); recompile with `prl-build`"
    )]
    NoWorldspawnGravity,
    #[error(
        "PRL file has no TextureCacheKeys section (section 32) — file is corrupt or was produced by a writer that omits the section; recompile with `prl-build`"
    )]
    NoTextureCacheKeys,
    #[error(
        "PRL file has no OctahedralShVolume section (section 34) — pre-migration SH volume maps are not supported; recompile with `prl-build`"
    )]
    NoOctahedralShVolume,
    #[error(
        "DeltaShVolumes affinity_factor {found} != engine AFFINITY_FACTOR {expected} — recompile the .prl with the current `prl-build`"
    )]
    DeltaShAffinityFactorMismatch { found: u8, expected: u8 },
    #[error(
        "DeltaShVolumes affinity_dims {found:?} != ceil(base ShVolume dims {base_dims:?} / {factor}) = {expected:?} — recompile the .prl with the current `prl-build`"
    )]
    DeltaShAffinityDimsMismatch {
        found: [u32; 3],
        base_dims: [u32; 3],
        factor: u32,
        expected: [u32; 3],
    },
    #[error(
        "DeltaShVolumes tile geometry {found_dimension}+border {found_border} does not match base OctahedralShVolume tile geometry {base_dimension}+border {base_border} — recompile the .prl with the current `prl-build`"
    )]
    DeltaShTileGeometryMismatch {
        found_dimension: u32,
        found_border: u32,
        base_dimension: u32,
        base_border: u32,
    },
    #[error(
        "PRL file has a DeltaShVolumes section (id 27) but no base OctahedralShVolume section (id 34) — the compose pass cannot derive affinity dims without the base grid; recompile with `prl-build`"
    )]
    DeltaShMissingBaseVolume,
}

/// Face → index-range mapping lives on BVH leaves; `FaceMeta` carries only
/// the per-face attributes CPU code still needs (lighting baker, editor diagnostics).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FaceMeta {
    pub leaf_index: u32,
    pub texture_index: Option<u32>,
    #[allow(dead_code)]
    pub texture_dimensions: (u32, u32), // defaults to (64, 64) for missing textures
    pub texture_name: String,
    pub material: Material,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BspChild {
    Node(usize),
    Leaf(usize),
}

#[derive(Debug, Clone)]
pub struct NodeData {
    pub plane_normal: Vec3,
    pub plane_distance: f32,
    pub front: BspChild,
    pub back: BspChild,
}

#[derive(Debug, Clone)]
pub struct LeafData {
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub face_start: u32,
    pub face_count: u32,
    pub is_solid: bool,
}

#[derive(Debug, Clone)]
pub struct PortalData {
    pub polygon: Vec<Vec3>, // convex, world space
    pub front_leaf: usize,
    pub back_leaf: usize,
}

/// Mirrors `postretro-level-compiler::map_data::LightType` at the wire boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightType {
    Point,
    Spot,
    Directional,
}

/// Mirrors `postretro-level-compiler::map_data::FalloffModel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FalloffModel {
    Linear,
    InverseDistance,
    InverseSquared,
}

/// How a baked-tier light's **direct** shadow resolves. Mirrors the
/// compiler-side `ShadowType` and the wire-level `AlphaShadowType`. Two values
/// only — the dynamic tier is NOT a shadow-type value; it reaches the runtime
/// via the separate `is_dynamic` field (set by classname). The direct
/// techniques are disjoint, so the forward pass routes each light's direct
/// shadow to exactly one of lightmap (`StaticLightMap`) / runtime SDF trace
/// (`Sdf`) — no double-count. Legacy PRLs without the wire field decode
/// `StaticLightMap`. See `context/plans/in-progress/sdf-per-light-shadows/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShadowType {
    #[default]
    StaticLightMap,
    Sdf,
}

/// From PRL section 18. FGD-authored; script-registered entity types arrive via
/// `ModManifest.entities`, drained into `DataRegistry` at boot.
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
    /// Internal/seam-only flag for the geometry-moving light class
    /// (position/aim animation). v1 has no authoring surface — every
    /// authored light parses `false`. Intensity-only animation lives on
    /// the animated-baked path (Task 2c), not here. Legacy PRLs retain
    /// their stored value on parse.
    pub is_dynamic: bool,
    /// Whether this light casts shadows from dynamic ENTITIES (enemies / moving
    /// meshes). Mirrors FGD `_cast_entity_shadows`. Only ever `true` on
    /// dynamic-tier lights (`is_dynamic`) — the compiler warn-clears it on baked
    /// lights — so it is the second half of the entity-occluder gate
    /// (`entity_occluder_eligible` ≡ `casts_entity_shadows && is_dynamic`). The
    /// light's own WORLD-shadow pool eligibility rides `is_dynamic` alone, so a
    /// dynamic light with this `false` still casts its world shadow, it just
    /// draws no entity occluders.
    pub casts_entity_shadows: bool,
    /// Slot into the SH-volume animated-light descriptor table when the
    /// compiler reserved one for this map light, else `None`. Resolved once
    /// at load from `ShVolumeSection.slot_for_map_light` and cached on the
    /// runtime `LightComponent` so `setLightAnimation` can write the
    /// descriptor through the compose-side buffer without a per-call lookup.
    /// `None` for non-animated lights and for legacy PRLs that lack the slot
    /// table. Task 2c of `sdf-static-occluder-shadows`.
    pub animated_slot: Option<u32>,
    /// From LightTags section (ID 26). Space-delimited on wire; split here.
    /// `world.query({ tag: "t" })` matches when any tag equals `"t"`.
    pub tags: Vec<String>,
    /// `u32::MAX` (`ALPHA_LIGHT_LEAF_UNASSIGNED`) = couldn't assign to a non-solid leaf;
    /// excluded from portal-graph reachability and chunk light lists.
    pub leaf_index: u32,
    /// How this baked-tier light's **direct** shadow resolves (FGD
    /// `_shadow_type`). `Sdf`-typed lights take the runtime per-light SDF
    /// visibility + diffuse path in the forward shader (flagged via
    /// `spec_lights`); `StaticLightMap` is shadowed by the lightmap. The dynamic
    /// tier rides the separate `is_dynamic` field (shadow-map path), not this
    /// value. Legacy PRLs decode `StaticLightMap`.
    pub shadow_type: ShadowType,
}

/// Whether the lightmap section's baked irradiance already includes the
/// static-light visibility (shadow) term, or carries unshadowed irradiance
/// for runtime SDF visibility to multiply in (Task 2a).
///
/// Task 2a will add an on-disk marker to the lightmap section. Until that
/// lands, every legacy PRL parses as `Shadowed` — matching `main`-equivalent
/// behavior so the forward pass (Task 5) knows not to multiply the SDF
/// visibility factor into an already-shadowed term and double-shadow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LightmapMode {
    /// Static-light visibility folded into the bake (today's `main`). Forward
    /// must NOT multiply by SDF visibility.
    #[default]
    Shadowed,
    /// Visibility term removed from the bake. Forward MUST multiply by SDF
    /// visibility to recover shadowed lighting. Set by Task 2a's bake flag.
    #[allow(dead_code)]
    Unshadowed,
}

#[derive(Debug)]
pub struct LevelWorld {
    pub vertices: Vec<WorldVertex>,
    pub indices: Vec<u32>,
    pub face_meta: Vec<FaceMeta>,
    pub leaves: Vec<LeafData>,
    pub nodes: Vec<NodeData>,
    /// Single-leaf tree → `BspChild::Leaf(0)`.
    pub root: BspChild,
    pub portals: Vec<PortalData>,
    /// `leaf_portals[i]` = all portal indices touching leaf `i`.
    pub leaf_portals: Vec<Vec<usize>>,
    pub has_portals: bool,
    pub texture_names: Vec<String>,
    /// Per-texture blake3 cache keys (PRL section 32), parallel to `texture_names`.
    /// Required — loader rejects files where the section is absent.
    pub texture_cache_keys: TextureCacheKeysSection,
    /// Always present — loader rejects files without a BVH section.
    pub bvh: BvhTree,
    /// Empty when section 18 is absent (maps predating lighting foundation).
    pub lights: Vec<MapLight>,
    /// Index `i` corresponds to `lights[i]`. Empty → all lights treated as infinite-bound.
    pub light_influences: Vec<crate::lighting::influence::LightInfluence>,
    /// Required octahedral irradiance atlas. Empty geometry uses a present
    /// section with zero grid dimensions; missing section means stale PRL.
    pub sh_volume: Option<OctahedralShVolumeSection>,
    /// `None` → 1×1 white placeholder; bumped-Lambert degrades to flat white.
    pub lightmap: Option<LightmapSection>,
    /// Whether the lightmap bake includes static-light visibility (Shadowed,
    /// today's `main`) or is unshadowed irradiance awaiting runtime SDF
    /// multiplication (Task 2a). Legacy PRLs without the on-disk marker
    /// parse as `Shadowed` — Task 5's forward shader uses this to decide
    /// whether to multiply the SDF visibility factor into the static term.
    pub lightmap_mode: LightmapMode,
    /// `None` → no static-occluder SDF atlas (legacy PRL or empty-geometry
    /// bake). The runtime SDF shadow pass (Task 4) is skipped entirely.
    pub sdf_atlas: Option<SdfAtlasSection>,
    /// `None` → full spec-buffer scan fallback. See `ChunkGrid::fallback`.
    pub chunk_light_list: Option<ChunkLightListSection>,
    /// Emitted by `prl-build` for animated-light maps; cross-checked against weight-map chunk count.
    pub animated_light_chunks: Option<AnimatedLightChunksSection>,
    /// `None` when no animated lights — renderer binds a 1×1 zero atlas.
    pub animated_light_weight_maps: Option<AnimatedLightWeightMapsSection>,
    /// Sparse, affinity-cell-indexed (CSR) per-animated-light SH deltas at peak
    /// brightness. `None` when no animated lights — compose pass falls back to
    /// base→total copy.
    pub delta_sh_volumes: Option<DeltaShVolumesSection>,
    /// Dense baked DIRECT static-light octahedral atlas for dynamic objects
    /// (mesh entities + billboards). `None` for legacy v7 maps / maps with no
    /// static lights — dynamic objects fall back to indirect-only (the renderer
    /// binds a 4×4 BC6H zero dummy). Tile geometry is byte-identical to
    /// `sh_volume`; the runtime reuses that section's grid uniform + depth
    /// moments.
    pub direct_sh_volume: Option<DirectShVolumeSection>,
    /// `None` when level has no `data_script` worldspawn KVP.
    /// See: context/lib/scripting.md §2 (Data context lifecycle)
    pub data_script: Option<DataScriptSection>,
    /// Held as wire type — loader doesn't depend on scripting tree.
    /// Dispatch entry point converts to `scripting::map_entity::MapEntity`.
    pub map_entities: Vec<MapEntityRecord>,
    /// Empty when section absent or no `fog_volume` brushes authored.
    pub fog_volumes: Vec<FogVolumeRecord>,
    /// Downscale factor (1=full-res, 8=coarsest). Defaults to 4 when absent.
    pub fog_pixel_scale: u32,
    /// Seeds `App::current_gravity` so `world.getGravity()` sees the authored value before scripts run.
    pub initial_gravity: f32,
    /// `masks[L]` has bit `i` set when fog volume `i` overlaps leaf `L`.
    /// `None` = legacy PRL without section 31: `compute_fog_cell_mask` treats
    /// `(Culled, None)` as all canonical slots active. Section 30 may be
    /// present without section 31.
    pub fog_cell_masks: Option<Vec<u32>>,
    /// Baked navigation graph (PRL section 36). `None` for maps without a
    /// navmesh bake; the runtime nav query surface (`crate::nav::NavGraph`) is
    /// only built when this is present. A malformed section warns and decodes to
    /// `None` rather than failing the load. Read only by the dev-tools nav graph
    /// build today, so allowed dead in shipping builds until pathfinding lands.
    #[allow(dead_code)]
    pub navmesh: Option<NavMeshSection>,
}

impl LevelWorld {
    /// On-plane position → front child. Empty tree → leaf 0.
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

// Positive → Node(v); negative → Leaf(-1 - v).
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
            let shadow_type = match r.shadow_type {
                AlphaShadowType::StaticLightMap => ShadowType::StaticLightMap,
                AlphaShadowType::Sdf => ShadowType::Sdf,
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
                is_dynamic: r.is_dynamic,
                casts_entity_shadows: r.casts_entity_shadows,
                animated_slot: None, // populated from ShVolume slot table later in load
                tags: vec![],        // populated by LightTags section pass below
                leaf_index: r.leaf_index,
                shadow_type,
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

/// Expected DeltaShVolumes affinity grid dims for a given base SH grid:
/// `ceil(base_dims / factor)` along each axis. The compiler bakes the affinity
/// grid this way; the loader rejects any section whose stored dims disagree.
/// Pure so the validation rule is unit-testable without a `.prl` file.
pub(crate) fn expected_affinity_dims(base_dims: [u32; 3], factor: u8) -> [u32; 3] {
    let f = factor as u32;
    [
        base_dims[0].div_ceil(f),
        base_dims[1].div_ceil(f),
        base_dims[2].div_ceil(f),
    ]
}

/// Validate a loaded DeltaShVolumes section against the engine's invariants.
/// `base` is the base OctahedralShVolume (id 34), or `None` if that section was
/// absent. Pure so the reject paths are unit-testable.
///
/// Rejects (clear typed error, no panic):
/// - `affinity_factor` != the engine's compiled-in `AFFINITY_FACTOR`,
/// - base ShVolume absent while a delta section is present,
/// - `affinity_dims` != `ceil(base_dims / affinity_factor)`,
/// - delta tile geometry differs from the base atlas tile geometry.
pub(crate) fn validate_delta_sh(
    section: &DeltaShVolumesSection,
    base: Option<&OctahedralShVolumeSection>,
) -> Result<(), PrlLoadError> {
    // affinity_factor is locked to the compose pass `@workgroup_size(4,4,4)`.
    if section.affinity_factor != AFFINITY_FACTOR {
        return Err(PrlLoadError::DeltaShAffinityFactorMismatch {
            found: section.affinity_factor,
            expected: AFFINITY_FACTOR,
        });
    }

    // The base grid's dims derive the expected affinity dims; the compose pass
    // cannot run without it.
    let Some(base) = base else {
        return Err(PrlLoadError::DeltaShMissingBaseVolume);
    };
    let base_dims = base.grid_dimensions;

    let expected = expected_affinity_dims(base_dims, AFFINITY_FACTOR);
    if section.affinity_dims != expected {
        return Err(PrlLoadError::DeltaShAffinityDimsMismatch {
            found: section.affinity_dims,
            base_dims,
            factor: AFFINITY_FACTOR as u32,
            expected,
        });
    }

    if section.tile_dimension != base.tile_dimension || section.tile_border != base.tile_border {
        return Err(PrlLoadError::DeltaShTileGeometryMismatch {
            found_dimension: section.tile_dimension,
            found_border: section.tile_border,
            base_dimension: base.tile_dimension,
            base_border: base.tile_border,
        });
    }

    Ok(())
}

pub fn load_prl(path: &str) -> Result<LevelWorld, PrlLoadError> {
    let path_ref = Path::new(path);
    if !path_ref.exists() {
        return Err(PrlLoadError::FileNotFound(path.to_string()));
    }

    let file_data = std::fs::read(path_ref)?;
    let mut cursor = std::io::Cursor::new(&file_data);

    let meta = prl_format::read_container(&mut cursor)?;

    let geom_data = prl_format::read_section_data(&mut cursor, &meta, SectionId::Geometry as u32)?
        .ok_or(PrlLoadError::NoGeometry)?;
    let geom = GeometrySection::from_bytes(&geom_data)?;

    let texture_names_data =
        prl_format::read_section_data(&mut cursor, &meta, SectionId::TextureNames as u32)?;
    let texture_names_section = match texture_names_data {
        Some(data) => Some(TextureNamesSection::from_bytes(&data)?),
        None => None,
    };
    let texture_names: Vec<String> = texture_names_section.map(|s| s.names).unwrap_or_default();

    // Required. Absence means the file is corrupt or was produced by a writer
    // that omitted section 32; reject so the texture cache never silently
    // degrades every surface to a placeholder on a bad file.
    let texture_cache_keys_data =
        prl_format::read_section_data(&mut cursor, &meta, SectionId::TextureCacheKeys as u32)?
            .ok_or(PrlLoadError::NoTextureCacheKeys)?;
    let texture_cache_keys = TextureCacheKeysSection::from_bytes(&texture_cache_keys_data)?;

    let mut warned_prefixes = HashSet::new();
    let vertices: Vec<WorldVertex> = geom
        .vertices
        .iter()
        .map(|v| WorldVertex {
            position: v.position,
            base_uv: v.uv, // raw texel-space; normalized after texture dimensions are known
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

    // Required. Pre-BVH maps must be rebuilt with `prl-build`.
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

    // Absent for single-leaf trees.
    let nodes_section =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::BspNodes as u32)? {
            Some(data) => Some(BspNodesSection::from_bytes(&data)?),
            None => None,
        };

    let leaves_section =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::BspLeaves as u32)? {
            Some(data) => Some(BspLeavesSection::from_bytes(&data)?),
            None => None,
        };

    let portals_section =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::Portals as u32)? {
            Some(data) => Some(PortalsSection::from_bytes(&data)?),
            None => None,
        };

    // Optional — older maps fall back to empty with a warning.
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

    // 1:1 with AlphaLights; count mismatch = format error. Absence = no tags.
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
        for (light, tag_str) in lights.iter_mut().zip(section.tags) {
            let tag_list: Vec<String> = tag_str.split_whitespace().map(|t| t.to_string()).collect();
            if !tag_list.is_empty() {
                tagged += 1;
                light.tags = tag_list;
            }
        }
        log::info!("[PRL] LightTags: {tagged} tagged lights");
    }

    // Optional — absent → all lights treated as infinite-bound.
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

    let sh_volume: Option<OctahedralShVolumeSection> = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::OctahedralShVolume as u32,
    )? {
        Some(data) => {
            let section = OctahedralShVolumeSection::from_bytes(&data)?;
            log::info!(
                "[PRL] OctahedralShVolume: {}×{}×{} grid ({} probes, {}×{} atlas, tile {} + border {}, {} tile(s)/row, {} animated layers)",
                section.grid_dimensions[0],
                section.grid_dimensions[1],
                section.grid_dimensions[2],
                section.probes.len(),
                section.atlas_dimensions[0],
                section.atlas_dimensions[1],
                section.tile_dimension,
                section.tile_border,
                section.atlas_tiles_per_row,
                section.animation_descriptors.len(),
            );
            Some(section)
        }
        None => return Err(PrlLoadError::NoOctahedralShVolume),
    };

    // Task 2c: populate `MapLight.animated_slot` from the SH-volume slot
    // table. Resolution happens once here (load time), not per
    // `setLightAnimation` call. Legacy PRLs lack the table — every slot stays
    // `None` and the bridge takes the legacy `is_dynamic`-gated path.
    if let Some(sh) = sh_volume.as_ref()
        && !sh.slot_for_map_light.is_empty()
    {
        use postretro_level_format::sh_volume::ANIMATED_SLOT_NONE;
        if sh.slot_for_map_light.len() != lights.len() {
            log::warn!(
                "[PRL] OctahedralShVolume slot_for_map_light count ({}) != AlphaLights count ({}); skipping animated-slot resolution",
                sh.slot_for_map_light.len(),
                lights.len(),
            );
        } else {
            let mut resolved = 0usize;
            for (light, &slot) in lights.iter_mut().zip(sh.slot_for_map_light.iter()) {
                if slot != ANIMATED_SLOT_NONE {
                    light.animated_slot = Some(slot);
                    resolved += 1;
                }
            }
            log::info!("[PRL] Resolved {resolved} map-light → animated-slot mapping(s)");
        }
    }

    // Optional — absent → 1×1 white placeholder; bumped-Lambert degrades to flat white.
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

    // Optional — absent → no static-occluder SDF; runtime shadow pass disabled.
    // An empty-geometry section (zero grid dims) is also a valid "no SDF"
    // marker; the renderer collapses it to the same disabled state.
    let sdf_atlas: Option<SdfAtlasSection> = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::SdfAtlas as u32,
    )? {
        Some(data) => {
            let section = SdfAtlasSection::from_bytes(&data)?;
            log::info!(
                "[PRL] SdfAtlas: grid={}×{}×{}, voxel_size={:.4}m, brick={} voxels, {} surface bricks",
                section.grid_dims[0],
                section.grid_dims[1],
                section.grid_dims[2],
                section.voxel_size_m,
                section.brick_size_voxels,
                section.surface_brick_count,
            );
            Some(section)
        }
        None => {
            log::info!(
                "[PRL] SdfAtlas section missing — runtime SDF shadow pass disabled (legacy PRL or no SDF bake)"
            );
            None
        }
    };

    // Optional — absent → full spec-buffer scan fallback.
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

    // Optional — cross-checked against weight-map chunk count at runtime.
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

    // Optional — absent → 1×1 zero atlas on animated-contribution slot.
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

    // Optional — absent → SH compose pass falls back to base→total copy.
    let delta_sh_volumes: Option<DeltaShVolumesSection> = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::DeltaShVolumes as u32,
    )? {
        Some(data) => {
            let section = DeltaShVolumesSection::from_bytes(&data)?;

            // Validation (mirrors the section-version reject path): a mismatched
            // bake must fail the load with a clear error rather than feed the
            // compose pass garbage. `sh_volume` (id 20) was loaded above.
            validate_delta_sh(&section, sh_volume.as_ref())?;

            log::info!(
                "[PRL] DeltaShVolumes: {} animated light(s), affinity grid {}×{}×{} \
                 ({} CSR entr(y/ies), {} delta subblock halves)",
                section.animation_descriptor_indices.len(),
                section.affinity_dims[0],
                section.affinity_dims[1],
                section.affinity_dims[2],
                section.affinity_lights.len(),
                section.delta_subblocks.len(),
            );
            Some(section)
        }
        None => None,
    };

    // Optional — absent for legacy v7 maps (no `SH_VOLUME_VERSION` bump) and for
    // maps with no static lights. Dynamic objects fall back to indirect-only.
    let direct_sh_volume: Option<DirectShVolumeSection> = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::DirectShVolume as u32,
    )? {
        Some(data) => {
            let section = DirectShVolumeSection::from_bytes(&data)?;
            log::info!(
                "[PRL] DirectShVolume: {}×{}×{} grid ({} probes, {}×{} atlas, tile {} + border {}, {} tile(s)/row, format {}, {} atlas byte(s))",
                section.grid_dimensions[0],
                section.grid_dimensions[1],
                section.grid_dimensions[2],
                section.total_probes(),
                section.atlas_dimensions[0],
                section.atlas_dimensions[1],
                section.tile_dimension,
                section.tile_border,
                section.atlas_tiles_per_row,
                section.irradiance_format,
                section.atlas.len(),
            );
            Some(section)
        }
        None => None,
    };

    // Optional — absent when map has no `data_script` worldspawn KVP.
    let data_script: Option<DataScriptSection> =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::DataScript as u32)? {
            Some(data) => {
                let section = DataScriptSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] DataScript: {} bytes from `{}`",
                    section.compiled_bytes.len(),
                    section.source_path,
                );
                Some(section)
            }
            None => None,
        };

    // Optional — absent when no non-light, non-worldspawn entities exist.
    let map_entities: Vec<MapEntityRecord> =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::MapEntity as u32)? {
            Some(data) => {
                let section = MapEntitySection::from_bytes(&data)?;
                log::info!("[PRL] MapEntity: {} entities", section.entries.len());
                section.entries
            }
            None => Vec::new(),
        };

    // Required — carries `initial_gravity` alongside fog volumes. Absence = pre-gravity PRL;
    // rejected so the engine never silently falls back to a hardcoded default.
    let (fog_volumes, fog_pixel_scale, initial_gravity): (Vec<FogVolumeRecord>, u32, f32) =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::FogVolumes as u32)? {
            Some(data) => {
                let section = FogVolumesSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] FogVolumes: {} volumes, pixel_scale={}, initial_gravity={}",
                    section.volumes.len(),
                    section.pixel_scale,
                    section.initial_gravity,
                );
                (
                    section.volumes,
                    section.pixel_scale,
                    section.initial_gravity,
                )
            }
            None => return Err(PrlLoadError::NoWorldspawnGravity),
        };

    // Optional — absent for legacy PRLs or maps with no fog entities.
    // None → `compute_fog_cell_mask` treats all canonical slots active.
    let fog_cell_masks: Option<Vec<u32>> =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::FogCellMasks as u32)? {
            Some(data) => {
                let section = FogCellMasksSection::from_bytes(&data)?;
                log::info!("[PRL] FogCellMasks: {} cells", section.masks.len());
                Some(section.masks)
            }
            None => None,
        };

    // Optional — absent → no runtime navigation (logged at info, mirroring the
    // SdfAtlas precedent for the absent-section case). A malformed body warns
    // and decodes to None (softer than SdfAtlas, which propagates with `?` and
    // fails the load): nothing depends on the navmesh yet, so warn-and-continue
    // is intentional rather than making a malformed navmesh unplayable.
    let navmesh: Option<NavMeshSection> =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::NavMesh as u32)? {
            Some(data) => match NavMeshSection::from_bytes(&data) {
                Ok(section) => {
                    log::info!(
                        "[PRL] NavMesh: {}×{} grid, cell_size={:.4}m, {} region(s), {} portal(s)",
                        section.dim_x,
                        section.dim_z,
                        section.cell_size,
                        section.regions.len(),
                        section.portals.len(),
                    );
                    Some(section)
                }
                Err(err) => {
                    log::warn!("[PRL] NavMesh section malformed, ignoring: {err}");
                    None
                }
            },
            None => {
                log::info!("[PRL] NavMesh section missing — no runtime navigation for this map");
                None
            }
        };

    let has_portals = portals_section.is_some();

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

    let leaves: Vec<LeafData> = match &leaves_section {
        Some(leaf_sec) => leaf_sec
            .leaves
            .iter()
            .map(|lr| LeafData {
                bounds_min: Vec3::from(lr.bounds_min),
                bounds_max: Vec3::from(lr.bounds_max),
                face_start: lr.face_start,
                face_count: lr.face_count,
                is_solid: lr.is_solid != 0,
            })
            .collect(),
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
                is_solid: false,
            }]
        }
    };

    // FogCellMasks is indexed by leaf id; a length mismatch means the masks
    // can't be safely consulted. Drop them and let the renderer fall back to
    // "all canonical slots active" (see `compute_fog_cell_mask`).
    let fog_cell_masks = match fog_cell_masks {
        Some(masks) if masks.len() != leaves.len() => {
            log::warn!(
                "[Loader] FogCellMasks length ({}) does not match leaves length ({}); ignoring masks (all slots active)",
                masks.len(),
                leaves.len(),
            );
            None
        }
        other => other,
    };

    let root = if nodes.is_empty() {
        BspChild::Leaf(0)
    } else {
        BspChild::Node(0)
    };

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
        "[PRL] Loaded: {} vertices, {} indices ({} triangles), {} faces, {} nodes, {} leaves, bvh=[{} nodes, {} leaves], portals={}, textures={}",
        vertices.len(),
        indices.len(),
        indices.len() / 3,
        face_meta.len(),
        nodes.len(),
        leaves.len(),
        bvh.nodes.len(),
        bvh.leaves.len(),
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
        portals,
        leaf_portals,
        has_portals,
        texture_names,
        texture_cache_keys,
        bvh,
        lights,
        light_influences,
        sh_volume,
        lightmap,
        // Task 2a will read this from the lightmap section's mode marker.
        // Until then every PRL parses as Shadowed — `main`-equivalent so
        // Task 5's forward pass skips the SDF visibility multiply.
        lightmap_mode: LightmapMode::default(),
        sdf_atlas,
        chunk_light_list,
        animated_light_chunks,
        animated_light_weight_maps,
        delta_sh_volumes,
        direct_sh_volume,
        data_script,
        map_entities,
        fog_volumes,
        fog_pixel_scale,
        initial_gravity,
        fog_cell_masks,
        navmesh,
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

    use postretro_level_format::delta_sh_volumes::{
        AFFINITY_FACTOR, DEFAULT_DELTA_PROBE_F16_STRIDE, DeltaShVolumesSection, PROBES_PER_CELL,
    };
    use postretro_level_format::octahedral::{
        DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
    };

    /// A minimal valid delta section for `base_dims`, with one CSR entry.
    fn delta_section_for(affinity_dims: [u32; 3]) -> DeltaShVolumesSection {
        let cell_count = (affinity_dims[0] * affinity_dims[1] * affinity_dims[2]) as usize;
        let mut offsets = vec![0u32; cell_count + 1];
        // One light touching cell 0.
        for o in offsets.iter_mut().skip(1) {
            *o = 1;
        }
        DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims,
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![0],
            affinity_offsets: offsets,
            affinity_lights: vec![0],
            delta_subblocks: vec![0u16; PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE],
        }
    }

    fn base_octahedral_section(grid_dimensions: [u32; 3]) -> OctahedralShVolumeSection {
        let probe_count =
            grid_dimensions[0] as usize * grid_dimensions[1] as usize * grid_dimensions[2] as usize;
        let atlas_dimensions = postretro_level_format::octahedral::irradiance_atlas_dimensions(
            grid_dimensions,
            DEFAULT_IRRADIANCE_TILE_DIMENSION,
        );
        let atlas_tiles_per_row =
            postretro_level_format::octahedral::irradiance_atlas_tiles_per_row(grid_dimensions)
                .unwrap();
        let atlas_texel_count = atlas_dimensions[0] as usize * atlas_dimensions[1] as usize;
        OctahedralShVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions,
            probe_stride: postretro_level_format::sh_volume::OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions,
            atlas_tiles_per_row,
            probes: vec![
                postretro_level_format::sh_volume::OctahedralShProbe::default();
                probe_count
            ],
            atlas_texels: vec![
                postretro_level_format::sh_volume::OctahedralAtlasTexel::default();
                atlas_texel_count
            ],
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    #[test]
    fn expected_affinity_dims_ceil_divides_per_axis() {
        // factor 4: 8→2, 9→3, 1→1, 4→1, 5→2.
        assert_eq!(expected_affinity_dims([8, 9, 1], 4), [2, 3, 1]);
        assert_eq!(expected_affinity_dims([4, 5, 16], 4), [1, 2, 4]);
    }

    #[test]
    fn validate_delta_sh_accepts_matching_dims() {
        let base_dims = [8u32, 5, 1];
        let section = delta_section_for(expected_affinity_dims(base_dims, AFFINITY_FACTOR));
        let base = base_octahedral_section(base_dims);
        assert!(validate_delta_sh(&section, Some(&base)).is_ok());
    }

    #[test]
    fn validate_delta_sh_rejects_wrong_affinity_factor() {
        let base_dims = [8u32, 8, 8];
        let mut section = delta_section_for(expected_affinity_dims(base_dims, AFFINITY_FACTOR));
        section.affinity_factor = AFFINITY_FACTOR + 1;
        let base = base_octahedral_section(base_dims);
        let err = validate_delta_sh(&section, Some(&base)).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::DeltaShAffinityFactorMismatch { .. }),
            "expected affinity-factor error, got {err:?}"
        );
    }

    #[test]
    fn validate_delta_sh_rejects_affinity_dims_mismatch() {
        let base_dims = [8u32, 8, 8]; // expected affinity dims [2,2,2]
        // Build a section whose affinity_dims disagree with the base grid.
        let section = delta_section_for([3, 2, 2]);
        let base = base_octahedral_section(base_dims);
        let err = validate_delta_sh(&section, Some(&base)).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::DeltaShAffinityDimsMismatch { .. }),
            "expected affinity-dims error, got {err:?}"
        );
    }

    #[test]
    fn validate_delta_sh_rejects_tile_geometry_mismatch() {
        let base_dims = [8u32, 5, 1];
        let mut section = delta_section_for(expected_affinity_dims(base_dims, AFFINITY_FACTOR));
        section.tile_dimension += 2;
        let base = base_octahedral_section(base_dims);
        let err = validate_delta_sh(&section, Some(&base)).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::DeltaShTileGeometryMismatch { .. }),
            "expected tile-geometry error, got {err:?}"
        );
    }

    #[test]
    fn validate_delta_sh_rejects_missing_base_volume() {
        let section = delta_section_for([2, 2, 2]);
        let err = validate_delta_sh(&section, None).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::DeltaShMissingBaseVolume),
            "expected missing-base error, got {err:?}"
        );
    }

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
        is_solid: bool,
    ) -> LeafData {
        LeafData {
            bounds_min,
            bounds_max,
            face_start,
            face_count,
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
                    false,
                ),
                simple_leaf(
                    Vec3::new(-100.0, -100.0, -100.0),
                    Vec3::new(0.0, 100.0, 100.0),
                    1,
                    1,
                    false,
                ),
            ],
            root: BspChild::Node(0),
            portals: vec![],
            leaf_portals: vec![vec![], vec![]],
            has_portals: false,
            texture_names: vec![],
            texture_cache_keys: TextureCacheKeysSection { keys: vec![] },
            bvh: empty_bvh(),
            lights: vec![],
            light_influences: vec![],
            sh_volume: None,
            lightmap: None,
            lightmap_mode: LightmapMode::Shadowed,
            sdf_atlas: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            data_script: None,
            map_entities: Vec::new(),
            fog_volumes: Vec::new(),
            fog_pixel_scale: 4,
            initial_gravity: -9.81,
            fog_cell_masks: None,
            navmesh: None,
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
                false,
            )],
            root: BspChild::Leaf(0),
            portals: vec![],
            leaf_portals: vec![vec![]],
            has_portals: false,
            texture_names: vec![],
            texture_cache_keys: TextureCacheKeysSection { keys: vec![] },
            bvh: empty_bvh(),
            lights: vec![],
            light_influences: vec![],
            sh_volume: None,
            lightmap: None,
            lightmap_mode: LightmapMode::Shadowed,
            sdf_atlas: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            data_script: None,
            map_entities: Vec::new(),
            fog_volumes: Vec::new(),
            fog_pixel_scale: 4,
            initial_gravity: -9.81,
            fog_cell_masks: None,
            navmesh: None,
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
                simple_leaf(Vec3::ZERO, Vec3::splat(10.0), 0, 1, false),
                simple_leaf(Vec3::ZERO, Vec3::ZERO, 0, 0, true),
            ],
            root: BspChild::Leaf(0),
            portals: vec![],
            leaf_portals: vec![vec![], vec![]],
            has_portals: false,
            texture_names: vec![],
            texture_cache_keys: TextureCacheKeysSection { keys: vec![] },
            bvh: empty_bvh(),
            lights: vec![],
            light_influences: vec![],
            sh_volume: None,
            lightmap: None,
            lightmap_mode: LightmapMode::Shadowed,
            sdf_atlas: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            data_script: None,
            map_entities: Vec::new(),
            fog_volumes: Vec::new(),
            fog_pixel_scale: 4,
            initial_gravity: -9.81,
            fog_cell_masks: None,
            navmesh: None,
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
                simple_leaf(Vec3::ZERO, Vec3::ZERO, 0, 2, false),
                simple_leaf(Vec3::ZERO, Vec3::ZERO, 2, 1, false),
            ],
            root: BspChild::Leaf(0),
            portals: vec![],
            leaf_portals: vec![vec![], vec![]],
            has_portals: false,
            texture_names: vec![],
            texture_cache_keys: TextureCacheKeysSection { keys: vec![] },
            bvh: empty_bvh(),
            lights: vec![],
            light_influences: vec![],
            sh_volume: None,
            lightmap: None,
            lightmap_mode: LightmapMode::Shadowed,
            sdf_atlas: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            data_script: None,
            map_entities: Vec::new(),
            fog_volumes: Vec::new(),
            fog_pixel_scale: 4,
            initial_gravity: -9.81,
            fog_cell_masks: None,
            navmesh: None,
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

    fn write_prl_fixture_raw(
        sections: Vec<prl_format::SectionBlob>,
        name: &str,
    ) -> std::path::PathBuf {
        let tmp = std::env::temp_dir().join(name);
        let mut file = std::fs::File::create(&tmp).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();
        tmp
    }

    fn write_prl_fixture(
        mut sections: Vec<prl_format::SectionBlob>,
        name: &str,
    ) -> std::path::PathBuf {
        if !sections
            .iter()
            .any(|section| section.section_id == SectionId::OctahedralShVolume as u32)
        {
            sections.push(default_octahedral_sh_volume_blob());
        }
        write_prl_fixture_raw(sections, name)
    }

    fn default_octahedral_sh_volume_blob() -> prl_format::SectionBlob {
        let section = OctahedralShVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: [0, 0, 0],
            probe_stride: postretro_level_format::sh_volume::OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: postretro_level_format::octahedral::DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: postretro_level_format::octahedral::DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions: [0, 0],
            atlas_tiles_per_row: 0,
            probes: Vec::new(),
            atlas_texels: Vec::new(),
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        };
        prl_format::SectionBlob {
            section_id: SectionId::OctahedralShVolume as u32,
            version: 1,
            data: section.to_bytes(),
        }
    }

    fn default_fog_volumes_blob() -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::FogVolumes as u32,
            version: 1,
            data: FogVolumesSection::default().to_bytes(),
        }
    }

    fn default_texture_cache_keys_blob() -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::TextureCacheKeys as u32,
            version: 1,
            data: TextureCacheKeysSection::default().to_bytes(),
        }
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

        let leaves = BspLeavesSection {
            leaves: vec![
                BspLeafRecord {
                    face_start: 0,
                    face_count: 1,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 1,
                    face_count: 1,
                    bounds_min: [9.0, 0.0, 0.0],
                    bounds_max: [12.0, 2.0, 2.0],
                    is_solid: 0,
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
                section_id: SectionId::BspNodes as u32,
                version: 1,
                data: nodes.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::BspLeaves as u32,
                version: 1,
                data: leaves.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
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
        assert_eq!(world.root, BspChild::Node(0));
        assert_eq!(world.find_leaf(Vec3::new(10.0, 0.0, 0.0)), 0);
        assert_eq!(world.find_leaf(Vec3::new(0.0, 0.0, 0.0)), 1);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_missing_bvh_section() {
        let geom = sample_geometry();
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geom.to_bytes(),
            },
            default_texture_cache_keys_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_missing_bvh.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(matches!(err, PrlLoadError::NoBvh), "got {err:?}");
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_missing_octahedral_sh_volume_section() {
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
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture_raw(sections, "postretro_test_missing_oct_sh.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::NoOctahedralShVolume),
            "got {err:?}"
        );
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
                    is_dynamic: false,
                    casts_entity_shadows: false,
                    leaf_index: 0,
                    shadow_type: AlphaShadowType::StaticLightMap,
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
                    is_dynamic: true,
                    casts_entity_shadows: true,
                    leaf_index: 1,
                    shadow_type: AlphaShadowType::StaticLightMap,
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
                    is_dynamic: false,
                    casts_entity_shadows: false,
                    leaf_index: 2,
                    shadow_type: AlphaShadowType::StaticLightMap,
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
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_alpha_lights.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("should load");

        assert_eq!(world.lights.len(), 3);

        assert_eq!(world.lights[0].light_type, LightType::Point);
        assert_eq!(world.lights[0].origin, [1.0, 2.0, 3.0]);
        assert_eq!(world.lights[0].intensity, 300.0);
        assert_eq!(world.lights[0].falloff_model, FalloffModel::InverseSquared);
        assert!((world.lights[0].falloff_range - 50.0).abs() < 1e-5);
        assert_eq!(world.lights[0].leaf_index, 0);

        assert_eq!(world.lights[1].light_type, LightType::Spot);
        assert_eq!(world.lights[1].falloff_model, FalloffModel::Linear);
        assert!((world.lights[1].cone_angle_inner - std::f32::consts::FRAC_PI_6).abs() < 1e-4);
        assert!((world.lights[1].cone_angle_outer - std::f32::consts::FRAC_PI_4).abs() < 1e-4);
        assert_eq!(world.lights[1].cone_direction, [0.0, -1.0, 0.0]);
        assert_eq!(world.lights[1].leaf_index, 1);

        assert_eq!(world.lights[2].light_type, LightType::Directional);
        assert_eq!(world.lights[2].leaf_index, 2);

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
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
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
            default_texture_cache_keys_blob(),
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
    fn load_prl_parses_map_entity_section_into_world() {
        use postretro_level_format::map_entity::{MapEntityRecord, MapEntitySection};

        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        let me = MapEntitySection {
            entries: vec![MapEntityRecord {
                classname: "billboard_emitter".to_string(),
                origin: [4.0, 1.0, -2.0],
                angles: [0.0, std::f32::consts::FRAC_PI_2, 0.0],
                key_values: vec![
                    ("rate".to_string(), "12".to_string()),
                    ("wave".to_string(), "3".to_string()),
                ],
                tags: vec!["fx".to_string()],
            }],
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
                section_id: SectionId::MapEntity as u32,
                version: 1,
                data: me.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_map_entity.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("should load");

        assert_eq!(world.map_entities.len(), 1);
        let e = &world.map_entities[0];
        assert_eq!(e.classname, "billboard_emitter");
        assert!((e.origin[0] - 4.0).abs() < 1e-5);
        assert!((e.angles[1] - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        let rate = e
            .key_values
            .iter()
            .find(|(k, _)| k == "rate")
            .map(|(_, v)| v.as_str());
        let wave = e
            .key_values
            .iter()
            .find(|(k, _)| k == "wave")
            .map(|(_, v)| v.as_str());
        assert_eq!(rate, Some("12"));
        assert_eq!(wave, Some("3"));
        assert_eq!(e.tags, vec!["fx".to_string()]);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_absent_map_entity_section_yields_empty_vec() {
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
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_no_map_entity.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("should load");
        assert!(world.map_entities.is_empty());
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
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_no_alpha_lights.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("should load");
        assert!(world.lights.is_empty());

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_parses_fog_cell_masks_section() {
        use postretro_level_format::fog_cell_masks::FogCellMasksSection;

        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        let leaves = BspLeavesSection {
            leaves: vec![
                BspLeafRecord {
                    face_start: 0,
                    face_count: 1,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 1,
                    face_count: 1,
                    bounds_min: [9.0, 0.0, 0.0],
                    bounds_max: [12.0, 2.0, 2.0],
                    is_solid: 0,
                },
            ],
        };
        let masks = FogCellMasksSection {
            masks: vec![0x0000_0001, 0x0000_8000],
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
                section_id: SectionId::BspLeaves as u32,
                version: 1,
                data: leaves.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::FogCellMasks as u32,
                version: 1,
                data: masks.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_fog_cell_masks.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("should load");

        assert_eq!(
            world.fog_cell_masks,
            Some(vec![0x0000_0001u32, 0x0000_8000])
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_drops_fog_cell_masks_when_length_mismatches_leaves() {
        use postretro_level_format::fog_cell_masks::FogCellMasksSection;

        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        // Two leaves but only one mask — truncated FogCellMasks must degrade
        // to None so the renderer's "all slots active" fallback engages.
        let leaves = BspLeavesSection {
            leaves: vec![
                BspLeafRecord {
                    face_start: 0,
                    face_count: 1,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 1,
                    face_count: 1,
                    bounds_min: [9.0, 0.0, 0.0],
                    bounds_max: [12.0, 2.0, 2.0],
                    is_solid: 0,
                },
            ],
        };
        let masks = FogCellMasksSection {
            masks: vec![0x0000_0001],
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
                section_id: SectionId::BspLeaves as u32,
                version: 1,
                data: leaves.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::FogCellMasks as u32,
                version: 1,
                data: masks.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_fog_cell_masks_truncated.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("should load");
        assert!(
            world.fog_cell_masks.is_none(),
            "truncated FogCellMasks should be dropped to None"
        );
        assert_eq!(world.leaves.len(), 2);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_drops_fog_cell_masks_when_masks_longer_than_leaves() {
        use postretro_level_format::fog_cell_masks::FogCellMasksSection;

        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        // One leaf but two masks — oversized FogCellMasks must degrade
        // to None so the renderer's "all slots active" fallback engages.
        let leaves = BspLeavesSection {
            leaves: vec![BspLeafRecord {
                face_start: 0,
                face_count: 1,
                bounds_min: [0.0, 0.0, 0.0],
                bounds_max: [2.0, 2.0, 2.0],
                is_solid: 0,
            }],
        };
        let masks = FogCellMasksSection {
            masks: vec![0x0000_0001, 0x0000_0002],
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
                section_id: SectionId::BspLeaves as u32,
                version: 1,
                data: leaves.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::FogCellMasks as u32,
                version: 1,
                data: masks.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_fog_cell_masks_oversized.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("should load");
        assert!(
            world.fog_cell_masks.is_none(),
            "oversized FogCellMasks should be dropped to None"
        );
        assert_eq!(world.leaves.len(), 1);

        std::fs::remove_file(&tmp).ok();
    }

    /// AC [T3]: an old `.prl` without the SDF section loads without error;
    /// the parsed `LevelWorld` reports `sdf_atlas == None` so the renderer
    /// can degrade to the "no SDF atlas" state and skip the shadow pass.
    #[test]
    fn load_prl_absent_sdf_atlas_section_yields_none() {
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
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_no_sdf_atlas.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("legacy PRL without SDF must load");
        assert!(
            world.sdf_atlas.is_none(),
            "absent SDF atlas section should yield None (legacy / no-bake degrade path)"
        );
        // Lightmap mode defaults to Shadowed until Task 2a adds the marker,
        // so legacy PRLs degrade to `main`-equivalent forward behavior.
        assert_eq!(world.lightmap_mode, LightmapMode::Shadowed);

        std::fs::remove_file(&tmp).ok();
    }

    /// AC [T3]: an SDF section that round-trips through the PRL container
    /// is parsed by the loader and surfaced on `LevelWorld`.
    #[test]
    fn load_prl_parses_sdf_atlas_section() {
        use postretro_level_format::sdf_atlas::{
            BRICK_SLOT_EMPTY, BRICK_SLOT_INTERIOR, SDF_ATLAS_VERSION, SdfAtlasSection,
        };

        let brick_size = 4u32;
        // v2 layout: each surface brick stores an apron'd `(brick_size + 2)^3`
        // block. The fixture must satisfy the loader's per-brick invariant
        // (atlas_len == (brick_size + 2)^3 * surface_brick_count).
        let stored_edge = brick_size + 2;
        let voxels_per_brick = (stored_edge * stored_edge * stored_edge) as usize;
        let section = SdfAtlasSection {
            world_min: [-1.0, -1.0, -1.0],
            world_max: [1.0, 1.0, 1.0],
            voxel_size_m: 0.125,
            brick_size_voxels: brick_size,
            grid_dims: [1, 1, 1],
            atlas_bricks_per_axis: [1, 1, 1],
            surface_brick_count: 1,
            // One brick cell, marked as a surface brick.
            top_level: vec![0],
            atlas: vec![0i16; voxels_per_brick],
            coarse_distances: vec![0.5],
        };
        // Spot-check the sentinels round-trip — separate cell with sentinel
        // marker isn't needed for this loader test, but confirm the const
        // imports compile.
        let _: u32 = BRICK_SLOT_EMPTY;
        let _: u32 = BRICK_SLOT_INTERIOR;

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
            prl_format::SectionBlob {
                section_id: SectionId::SdfAtlas as u32,
                version: SDF_ATLAS_VERSION as u16,
                data: section.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_with_sdf_atlas.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("PRL with SDF atlas must load");
        let parsed = world
            .sdf_atlas
            .as_ref()
            .expect("SDF atlas section must round-trip into LevelWorld");
        assert_eq!(parsed.grid_dims, [1, 1, 1]);
        assert_eq!(parsed.brick_size_voxels, brick_size);
        assert_eq!(parsed.surface_brick_count, 1);
        assert_eq!(parsed.atlas.len(), voxels_per_brick);

        std::fs::remove_file(&tmp).ok();
    }

    /// AC 6: a legacy map without the new DirectShVolume section loads and
    /// surfaces `direct_sh_volume = None` (dynamic objects fall back to
    /// indirect-only; the renderer binds the 4×4 BC6H dummy).
    #[test]
    fn load_prl_absent_direct_sh_volume_section_yields_none() {
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
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_no_direct_sh_volume.prl");
        let world =
            load_prl(tmp.to_str().unwrap()).expect("legacy PRL without DirectShVolume must load");
        assert!(
            world.direct_sh_volume.is_none(),
            "absent DirectShVolume section should yield None (indirect-only fallback)"
        );

        std::fs::remove_file(&tmp).ok();
    }

    /// AC 12/13 (loader half): a DirectShVolume section round-trips through the
    /// PRL container and is surfaced on `LevelWorld`, BC6H tag preserved.
    #[test]
    fn load_prl_parses_direct_sh_volume_section() {
        use postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;
        use postretro_level_format::octahedral::{
            DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
            irradiance_atlas_dimensions, irradiance_atlas_tiles_per_row,
        };

        let grid = [3u32, 2, 4];
        let tile_dimension = DEFAULT_IRRADIANCE_TILE_DIMENSION;
        let atlas_dimensions = irradiance_atlas_dimensions(grid, tile_dimension);
        let atlas_tiles_per_row = irradiance_atlas_tiles_per_row(grid).unwrap();
        // BC6H blob length for the 4-aligned padded atlas (the emitter rounds
        // each axis up to a multiple of 4 before encoding).
        let padded_w = atlas_dimensions[0].div_ceil(4) * 4;
        let padded_h = atlas_dimensions[1].div_ceil(4) * 4;
        let block_count = (padded_w / 4) as usize * (padded_h / 4) as usize;
        let section = DirectShVolumeSection {
            grid_origin: [1.0, 2.0, 3.0],
            cell_size: [0.5, 0.5, 0.5],
            grid_dimensions: grid,
            tile_dimension,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions,
            atlas_tiles_per_row,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
            atlas: vec![0u8; block_count * 16],
        };

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
            prl_format::SectionBlob {
                section_id: SectionId::DirectShVolume as u32,
                version: 1,
                data: section.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_with_direct_sh_volume.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("PRL with DirectShVolume must load");
        let parsed = world
            .direct_sh_volume
            .as_ref()
            .expect("DirectShVolume section must round-trip into LevelWorld");
        assert_eq!(parsed.grid_dimensions, grid);
        assert_eq!(parsed.atlas_dimensions, atlas_dimensions);
        assert_eq!(parsed.irradiance_format, IRRADIANCE_FORMAT_BC6H);
        assert_eq!(parsed.atlas.len(), block_count * 16);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_absent_fog_cell_masks_yields_none() {
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
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_no_fog_cell_masks.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("should load");
        assert!(
            world.fog_cell_masks.is_none(),
            "absent FogCellMasks section should yield None"
        );

        std::fs::remove_file(&tmp).ok();
    }
}
