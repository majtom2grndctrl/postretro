// PRL level loading: reads .prl files, populates LevelWorld (cells, BVH, lights,
// portals, fog volumes, scripted entities, and worldspawn metadata).
// See: context/lib/build_pipeline.md §PRL Compilation

use glam::Vec3;
use postretro_level_format as prl_format;
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::cell_draw_index::CellDrawIndexSection;
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::data_script::DataScriptSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::fog_volumes::FogVolumeRecord;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::map_entity::MapEntityRecord;
use postretro_level_format::navmesh::NavMeshSection;
use postretro_level_format::sdf_atlas::SdfAtlasSection;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use thiserror::Error;

use crate::geometry::{BvhTree, WorldVertex};
use crate::material::Material;

#[path = "prl_loader.rs"]
mod prl_loader;

pub use prl_loader::load_prl;

#[derive(Debug, Error)]
pub enum PrlLoadError {
    #[error("PRL file not found: {0}")]
    FileNotFound(String),
    #[error("failed to read PRL file: {0}")]
    IoError(#[from] std::io::Error),
    #[error("PRL format error: {0}")]
    FormatError(#[from] prl_format::FormatError),
    #[error(
        "PRL file is missing required {section} section (id {id}) — stale format; recompile with `prl-build`"
    )]
    StaleFormatMissingSection { section: &'static str, id: u32 },
    #[error(
        "PRL file contains modern Cells/CellLocator sections plus legacy runtime BSP section(s) {sections} — ambiguous stale format; recompile with `prl-build`"
    )]
    AmbiguousRuntimeBspSections { sections: String },
    #[error("{section} validation error: {message}")]
    SectionValidation {
        section: &'static str,
        message: String,
    },
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
    /// Runtime cell id for this face. Field name preserved from the `Geometry` wire type
    /// (`GeometrySection` uses `leaf_index`); the value is a runtime cell id.
    pub leaf_index: u32,
    pub texture_index: Option<u32>,
    #[allow(dead_code)]
    pub texture_dimensions: (u32, u32), // defaults to (64, 64) for missing textures
    pub texture_name: String,
    pub material: Material,
}

#[derive(Debug, Clone)]
pub struct CellData {
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub face_start: u32,
    pub face_count: u32,
    pub portal_ref_start: u32,
    pub portal_ref_count: u32,
    pub is_solid: bool,
    pub is_exterior: bool,
    pub is_drawable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellLocatorChild {
    Node(usize),
    Cell(usize),
}

#[derive(Debug, Clone)]
pub struct CellLocatorNodeData {
    pub plane_normal: Vec3,
    pub plane_distance: f32,
    pub front: CellLocatorChild,
    pub back: CellLocatorChild,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(not(any(feature = "dev-tools", test)), allow(dead_code))]
pub enum CellLocatorSide {
    Front,
    Back,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(not(any(feature = "dev-tools", test)), allow(dead_code))]
pub struct CellLocatorTraceStep {
    pub node_index: usize,
    pub signed_distance: f32,
    pub selected_side: CellLocatorSide,
    pub selected_child: CellLocatorChild,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(not(any(feature = "dev-tools", test)), allow(dead_code))]
pub struct CellLocatorTrace {
    pub root: CellLocatorChild,
    pub steps: Vec<CellLocatorTraceStep>,
    pub result_cell: usize,
}

#[derive(Debug, Clone)]
pub struct PortalData {
    pub polygon: Vec<Vec3>, // convex, world space
    pub front_cell: usize,
    pub back_cell: usize,
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
    /// Runtime cell id for portal-graph reachability and chunk light lists.
    /// `u32::MAX` (`ALPHA_LIGHT_LEAF_UNASSIGNED` on the legacy wire) means the
    /// compiler/runtime could not assign the light to a non-solid cell.
    pub cell_index: u32,
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

/// Runtime view of the `CellDrawIndex` PRL section (id 37): each cell's owned
/// BVH-leaf spans in CSR layout. Held as the format type after the loader has
/// cross-validated it against the BVH leaf array and loaded Cells section. A
/// stable runtime name so the candidate-cull GPU path consumes one type.
pub type CellDrawIndex = CellDrawIndexSection;

#[derive(Debug)]
pub struct LevelWorld {
    pub vertices: Vec<WorldVertex>,
    pub indices: Vec<u32>,
    pub face_meta: Vec<FaceMeta>,
    /// Preferred spatial contract: runtime cells preserving compiler cell ids.
    pub cells: Vec<CellData>,
    /// Flat portal-index adjacency referenced by `cells[*].portal_ref_*`.
    pub cell_portal_refs: Vec<u32>,
    pub cell_locator_root: CellLocatorChild,
    pub cell_locator_nodes: Vec<CellLocatorNodeData>,
    pub portals: Vec<PortalData>,
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
    /// `masks[C]` has bit `i` set when fog volume `i` overlaps cell `C`.
    /// `None` only when the map has no canonical fog volumes.
    pub fog_cell_masks: Option<Vec<u32>>,
    /// Baked navigation graph (PRL section 36). `None` for maps without a
    /// navmesh bake; the runtime nav query surface (`crate::nav::NavGraph`) is
    /// only built when this is present. A malformed section warns and decodes to
    /// `None` rather than failing the load. Read only by the dev-tools nav graph
    /// build today, so allowed dead in shipping builds until pathfinding lands.
    #[allow(dead_code)]
    pub navmesh: Option<NavMeshSection>,
    /// Per-cell BVH-leaf draw index (PRL section 37), cross-validated against
    /// the BVH leaf array and loaded Cells section. `None` only for empty-BVH
    /// maps, where the section must be omitted.
    pub cell_draw_index: Option<CellDrawIndex>,
}

impl LevelWorld {
    /// Locate the runtime cell containing `position`.
    ///
    /// On-plane positions choose the front child, matching the temporary
    /// compiler-side BSP traversal.
    pub fn locate_cell(&self, position: Vec3) -> usize {
        let mut current = self.cell_locator_root;

        loop {
            match current {
                CellLocatorChild::Cell(cell_idx) => return cell_idx,
                CellLocatorChild::Node(node_idx) => {
                    let node = &self.cell_locator_nodes[node_idx];
                    let side = node.plane_normal.dot(position) - node.plane_distance;
                    current = if side >= 0.0 { node.front } else { node.back };
                }
            }
        }
    }

    /// Trace the same point-to-cell descent as [`Self::locate_cell`] for
    /// diagnostics. Keeps UI code from duplicating locator traversal.
    #[cfg(any(feature = "dev-tools", test))]
    pub fn trace_locate_cell(&self, position: Vec3) -> CellLocatorTrace {
        let mut current = self.cell_locator_root;
        let mut steps = Vec::new();

        loop {
            match current {
                CellLocatorChild::Cell(result_cell) => {
                    return CellLocatorTrace {
                        root: self.cell_locator_root,
                        steps,
                        result_cell,
                    };
                }
                CellLocatorChild::Node(node_index) => {
                    let node = &self.cell_locator_nodes[node_index];
                    let signed_distance = node.plane_normal.dot(position) - node.plane_distance;
                    let (selected_side, selected_child) = if signed_distance >= 0.0 {
                        (CellLocatorSide::Front, node.front)
                    } else {
                        (CellLocatorSide::Back, node.back)
                    };
                    steps.push(CellLocatorTraceStep {
                        node_index,
                        signed_distance,
                        selected_side,
                        selected_child,
                    });
                    current = selected_child;
                }
            }
        }
    }

    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    pub fn cell_portal_count(&self, cell_idx: usize) -> usize {
        let Some((start, end)) = self.cell_portal_range(cell_idx) else {
            return 0;
        };
        self.cell_portal_refs
            .get(start..end)
            .map_or(0, <[u32]>::len)
    }

    pub fn cell_portal_index(&self, cell_idx: usize, offset: usize) -> Option<usize> {
        let (start, end) = self.cell_portal_range(cell_idx)?;
        let idx = start.checked_add(offset)?;
        if idx >= end {
            return None;
        }
        self.cell_portal_refs
            .get(idx)
            .map(|&portal| portal as usize)
    }

    pub fn cell_is_solid(&self, cell_idx: usize) -> bool {
        self.cells
            .get(cell_idx)
            .map(|cell| cell.is_solid)
            .unwrap_or(false)
    }

    pub fn cell_face_count(&self, cell_idx: usize) -> u32 {
        self.cells
            .get(cell_idx)
            .map(|cell| cell.face_count)
            .unwrap_or(0)
    }

    pub fn cell_bounds(&self, cell_idx: usize) -> Option<(Vec3, Vec3)> {
        self.cells
            .get(cell_idx)
            .map(|cell| (cell.bounds_min, cell.bounds_max))
    }

    pub fn spawn_position(&self) -> Vec3 {
        let mut mins = Vec3::splat(f32::MAX);
        let mut maxs = Vec3::splat(f32::MIN);
        for cell in &self.cells {
            if cell.is_solid || cell.face_count == 0 {
                continue;
            }
            mins = mins.min(cell.bounds_min);
            maxs = maxs.max(cell.bounds_max);
        }
        (mins + maxs) * 0.5
    }

    fn cell_portal_range(&self, cell_idx: usize) -> Option<(usize, usize)> {
        let cell = self.cells.get(cell_idx)?;
        let start = cell.portal_ref_start as usize;
        let count = cell.portal_ref_count as usize;
        let end = start.checked_add(count)?;
        Some((start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::prl_loader::{expected_affinity_dims, validate_cell_draw_index, validate_delta_sh};
    use super::*;
    use crate::geometry::BvhLeaf;
    use postretro_level_format::SectionId;
    use postretro_level_format::alpha_lights::{
        ALPHA_LIGHT_LEAF_UNASSIGNED, AlphaFalloffModel, AlphaLightType, AlphaLightsSection,
        AlphaShadowType,
    };
    use postretro_level_format::bvh::{
        BVH_NODE_FLAG_LEAF, BvhLeaf as FormatBvhLeaf, BvhNode as FormatBvhNode, BvhSection,
    };
    use postretro_level_format::cell_locator::{
        CellLocatorChild as FormatCellLocatorChild, CellLocatorNodeRecord, CellLocatorSection,
    };
    use postretro_level_format::cells::{
        CELL_FLAG_DRAWABLE, CELL_FLAG_EXTERIOR, CELL_FLAG_SOLID, CellRecord, CellsSection,
    };
    use postretro_level_format::fog_volumes::{FogVolumeRecord, FogVolumesSection};
    use postretro_level_format::geometry::NO_TEXTURE;
    use postretro_level_format::geometry::{FaceMeta as FormatFaceMeta, GeometrySection, Vertex};
    use postretro_level_format::portals::{PortalRecord, PortalsSection};

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

    fn simple_cell(
        bounds_min: Vec3,
        bounds_max: Vec3,
        face_start: u32,
        face_count: u32,
        is_solid: bool,
    ) -> CellData {
        CellData {
            bounds_min,
            bounds_max,
            face_start,
            face_count,
            portal_ref_start: 0,
            portal_ref_count: 0,
            is_solid,
            is_exterior: false,
            is_drawable: !is_solid && face_count > 0,
        }
    }

    fn two_leaf_world() -> LevelWorld {
        LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            cells: vec![
                simple_cell(
                    Vec3::new(0.0, -100.0, -100.0),
                    Vec3::new(100.0, 100.0, 100.0),
                    0,
                    1,
                    false,
                ),
                simple_cell(
                    Vec3::new(-100.0, -100.0, -100.0),
                    Vec3::new(0.0, 100.0, 100.0),
                    1,
                    1,
                    false,
                ),
            ],
            cell_portal_refs: vec![],
            cell_locator_root: CellLocatorChild::Node(0),
            cell_locator_nodes: vec![CellLocatorNodeData {
                plane_normal: Vec3::X,
                plane_distance: 0.0,
                front: CellLocatorChild::Cell(0),
                back: CellLocatorChild::Cell(1),
            }],
            portals: vec![],
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
            cell_draw_index: None,
        }
    }

    #[test]
    fn locate_cell_front_side() {
        let world = two_leaf_world();
        assert_eq!(world.locate_cell(Vec3::new(10.0, 0.0, 0.0)), 0);
    }

    #[test]
    fn locate_cell_back_side() {
        let world = two_leaf_world();
        assert_eq!(world.locate_cell(Vec3::new(-10.0, 0.0, 0.0)), 1);
    }

    #[test]
    fn locate_cell_on_plane_goes_front() {
        let world = two_leaf_world();
        assert_eq!(world.locate_cell(Vec3::ZERO), 0);
    }

    #[test]
    fn locate_cell_returns_expected_probe_cells() {
        let world = two_leaf_world();
        for (probe, expected) in [
            (Vec3::new(10.0, 0.0, 0.0), 0),
            (Vec3::new(-10.0, 0.0, 0.0), 1),
            (Vec3::ZERO, 0),
        ] {
            assert_eq!(world.locate_cell(probe), expected);
        }
    }

    #[test]
    fn trace_locate_cell_reports_descent_path_and_result() {
        let world = two_leaf_world();
        let trace = world.trace_locate_cell(Vec3::new(-10.0, 0.0, 0.0));

        assert_eq!(trace.root, CellLocatorChild::Node(0));
        assert_eq!(trace.result_cell, 1);
        assert_eq!(trace.steps.len(), 1);
        assert_eq!(trace.steps[0].node_index, 0);
        assert!(trace.steps[0].signed_distance < 0.0);
        assert_eq!(trace.steps[0].selected_side, CellLocatorSide::Back);
        assert_eq!(trace.steps[0].selected_child, CellLocatorChild::Cell(1));
    }

    #[test]
    fn locate_cell_single_cell_tree() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            cells: vec![simple_cell(
                Vec3::splat(-100.0),
                Vec3::splat(100.0),
                0,
                0,
                false,
            )],
            cell_portal_refs: vec![],
            cell_locator_root: CellLocatorChild::Cell(0),
            cell_locator_nodes: vec![],
            portals: vec![],
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
            cell_draw_index: None,
        };
        assert_eq!(world.locate_cell(Vec3::new(50.0, 50.0, 50.0)), 0);
    }

    #[test]
    fn spawn_position_centers_non_solid_cells() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![simple_face_meta()],
            cells: vec![
                simple_cell(Vec3::ZERO, Vec3::splat(10.0), 0, 1, false),
                simple_cell(Vec3::ZERO, Vec3::ZERO, 0, 0, true),
            ],
            cell_portal_refs: vec![],
            cell_locator_root: CellLocatorChild::Cell(0),
            cell_locator_nodes: vec![],
            portals: vec![],
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
            cell_draw_index: None,
        };

        let spawn = world.spawn_position();
        assert!((spawn - Vec3::splat(5.0)).length() < 0.01);
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

    fn empty_geometry() -> GeometrySection {
        GeometrySection {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
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
            .any(|section| section.section_id == SectionId::Cells as u32)
        {
            sections.push(default_cells_blob());
        }
        if !sections
            .iter()
            .any(|section| section.section_id == SectionId::CellLocator as u32)
        {
            sections.push(default_cell_locator_blob());
        }
        let fixture_bvh_has_leaves = sections
            .iter()
            .find(|section| section.section_id == SectionId::Bvh as u32)
            .and_then(|section| BvhSection::from_bytes(&section.data).ok())
            .is_some_and(|section| !section.leaves.is_empty());
        if fixture_bvh_has_leaves
            && !sections
                .iter()
                .any(|section| section.section_id == SectionId::CellDrawIndex as u32)
        {
            sections.push(default_cell_draw_index_blob());
        }
        if !sections
            .iter()
            .any(|section| section.section_id == SectionId::OctahedralShVolume as u32)
        {
            sections.push(default_octahedral_sh_volume_blob());
        }
        write_prl_fixture_raw(sections, name)
    }

    fn default_cells_blob() -> prl_format::SectionBlob {
        let section = CellsSection {
            cells: vec![
                CellRecord {
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 0,
                    face_count: 1,
                    portal_ref_start: 0,
                    portal_ref_count: 0,
                },
                CellRecord {
                    bounds_min: [9.0, 0.0, 0.0],
                    bounds_max: [12.0, 2.0, 2.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 1,
                    face_count: 1,
                    portal_ref_start: 0,
                    portal_ref_count: 0,
                },
            ],
            portal_refs: Vec::new(),
        };
        prl_format::SectionBlob {
            section_id: SectionId::Cells as u32,
            version: 1,
            data: section.to_bytes(),
        }
    }

    fn default_cell_locator_blob() -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::CellLocator as u32,
            version: 1,
            data: two_leaf_locator_section(5.0).to_bytes(),
        }
    }

    fn default_cell_draw_index_blob() -> prl_format::SectionBlob {
        let section = postretro_level_format::cell_draw_index::CellDrawIndexSection {
            cell_count: 2,
            span_count: 2,
            cell_span_offset: vec![0, 1, 2],
            spans: vec![
                postretro_level_format::cell_draw_index::Span {
                    leaf_start: 0,
                    leaf_count: 1,
                },
                postretro_level_format::cell_draw_index::Span {
                    leaf_start: 1,
                    leaf_count: 1,
                },
            ],
        };
        prl_format::SectionBlob {
            section_id: SectionId::CellDrawIndex as u32,
            version: 1,
            data: section.to_bytes(),
        }
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

    fn fog_volumes_blob_with_count(count: usize) -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::FogVolumes as u32,
            version: 1,
            data: FogVolumesSection {
                pixel_scale: 4,
                initial_gravity: -9.81,
                volumes: vec![FogVolumeRecord::default(); count],
            }
            .to_bytes(),
        }
    }

    fn default_texture_cache_keys_blob() -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::TextureCacheKeys as u32,
            version: 1,
            data: TextureCacheKeysSection::default().to_bytes(),
        }
    }

    fn two_leaf_locator_section(plane_distance: f32) -> CellLocatorSection {
        CellLocatorSection {
            root: FormatCellLocatorChild::Node(0),
            nodes: vec![CellLocatorNodeRecord {
                plane_normal: [1.0, 0.0, 0.0],
                plane_distance,
                front: FormatCellLocatorChild::Cell(0),
                back: FormatCellLocatorChild::Cell(1),
            }],
        }
    }

    #[test]
    fn load_prl_round_trip_with_cells_and_locator() {
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
                section_id: SectionId::CellLocator as u32,
                version: 1,
                data: two_leaf_locator_section(5.0).to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_bvh_round_trip.prl");
        let world = load_prl(tmp.to_str().unwrap()).unwrap();

        assert_eq!(world.vertices.len(), 6);
        assert_eq!(world.indices.len(), 6);
        assert_eq!(world.face_meta.len(), 2);
        assert_eq!(world.bvh.nodes.len(), 3);
        assert_eq!(world.bvh.leaves.len(), 2);
        assert_eq!(world.cells.len(), 2);
        assert_eq!(world.cell_locator_root, CellLocatorChild::Node(0));
        assert_eq!(world.cell_locator_nodes.len(), 1);
        for (probe, expected) in [
            (Vec3::new(10.0, 0.0, 0.0), 0),
            (Vec3::new(0.0, 0.0, 0.0), 1),
            (Vec3::new(5.0, 0.0, 0.0), 0),
        ] {
            assert_eq!(world.locate_cell(probe), expected);
        }

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_decodes_cells_section_into_level_world() {
        let geom = sample_geometry();
        let bvh = sample_bvh_section();

        let cells = CellsSection {
            cells: vec![
                CellRecord {
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 0,
                    face_count: 1,
                    portal_ref_start: 0,
                    portal_ref_count: 2,
                },
                CellRecord {
                    bounds_min: [9.0, 0.0, 0.0],
                    bounds_max: [12.0, 2.0, 2.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 1,
                    face_count: 1,
                    portal_ref_start: 0,
                    portal_ref_count: 0,
                },
            ],
            portal_refs: vec![4, 7],
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
                section_id: SectionId::Cells as u32,
                version: 1,
                data: cells.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_cells_load.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("should load");

        assert_eq!(world.cells.len(), 2);
        assert_eq!(world.cell_portal_refs, vec![4, 7]);
        assert!(world.cells[0].is_drawable);
        assert!(!world.cells[0].is_solid);
        assert_eq!(world.cells[0].portal_ref_count, 2);
        assert!(!world.cells[1].is_solid);
        assert!(world.cells[1].is_drawable);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_malformed_portals_use_no_portals_fallback() {
        let portals = PortalsSection {
            vertices: Vec::new(),
            portals: vec![PortalRecord {
                vertex_start: 0,
                vertex_count: 3,
                front_leaf: 0,
                back_leaf: 1,
            }],
        };
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Portals as u32,
                version: 1,
                data: portals.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_malformed_portals_fallback.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("malformed portals should fall back");
        assert!(!world.has_portals);
        assert!(world.portals.is_empty());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_portals_trailing_bytes_use_no_portals_fallback() {
        let mut portals = PortalsSection {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            portals: vec![PortalRecord {
                vertex_start: 0,
                vertex_count: 3,
                front_leaf: 0,
                back_leaf: 1,
            }],
        }
        .to_bytes();
        portals.extend_from_slice(&[0xab, 0xcd]);
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Portals as u32,
                version: 1,
                data: portals,
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_portals_trailing_fallback.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("malformed portals should fall back");
        assert!(!world.has_portals);
        assert!(world.portals.is_empty());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_degenerate_two_vertex_portal_uses_no_portals_fallback() {
        let portals = PortalsSection {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            portals: vec![PortalRecord {
                vertex_start: 0,
                vertex_count: 2,
                front_leaf: 0,
                back_leaf: 1,
            }],
        };
        let cells = CellsSection {
            cells: vec![
                CellRecord {
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 0,
                    face_count: 1,
                    portal_ref_start: 0,
                    portal_ref_count: 1,
                },
                CellRecord {
                    bounds_min: [9.0, 0.0, 0.0],
                    bounds_max: [12.0, 2.0, 2.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 1,
                    face_count: 1,
                    portal_ref_start: 1,
                    portal_ref_count: 1,
                },
            ],
            portal_refs: vec![0, 0],
        };
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Cells as u32,
                version: 1,
                data: cells.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Portals as u32,
                version: 1,
                data: portals.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_two_vertex_portal_fallback.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("degenerate portal should fall back");
        assert!(!world.has_portals);
        assert!(world.portals.is_empty());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_collapsed_portal_polygon_uses_no_portals_fallback() {
        let portals = PortalsSection {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
            portals: vec![PortalRecord {
                vertex_start: 0,
                vertex_count: 3,
                front_leaf: 0,
                back_leaf: 1,
            }],
        };
        let cells = CellsSection {
            cells: vec![
                CellRecord {
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 0,
                    face_count: 1,
                    portal_ref_start: 0,
                    portal_ref_count: 1,
                },
                CellRecord {
                    bounds_min: [9.0, 0.0, 0.0],
                    bounds_max: [12.0, 2.0, 2.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 1,
                    face_count: 1,
                    portal_ref_start: 1,
                    portal_ref_count: 1,
                },
            ],
            portal_refs: vec![0, 0],
        };
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Cells as u32,
                version: 1,
                data: cells.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Portals as u32,
                version: 1,
                data: portals.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_collapsed_portal_fallback.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("collapsed portal should fall back");
        assert!(!world.has_portals);
        assert!(world.portals.is_empty());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_same_endpoint_portal_when_graph_is_usable() {
        let portals = PortalsSection {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            portals: vec![PortalRecord {
                vertex_start: 0,
                vertex_count: 3,
                front_leaf: 0,
                back_leaf: 0,
            }],
        };
        let cells = CellsSection {
            cells: vec![
                CellRecord {
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 0,
                    face_count: 1,
                    portal_ref_start: 0,
                    portal_ref_count: 1,
                },
                CellRecord {
                    bounds_min: [9.0, 0.0, 0.0],
                    bounds_max: [12.0, 2.0, 2.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 1,
                    face_count: 1,
                    portal_ref_start: 0,
                    portal_ref_count: 0,
                },
            ],
            portal_refs: vec![0],
        };
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Cells as u32,
                version: 1,
                data: cells.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Portals as u32,
                version: 1,
                data: portals.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_same_endpoint_portal.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "Portals",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_portal_adjacency_mismatch_when_graph_is_usable() {
        let portals = PortalsSection {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            portals: vec![PortalRecord {
                vertex_start: 0,
                vertex_count: 3,
                front_leaf: 0,
                back_leaf: 1,
            }],
        };
        let cells = CellsSection {
            cells: vec![
                CellRecord {
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 0,
                    face_count: 1,
                    portal_ref_start: 0,
                    portal_ref_count: 1,
                },
                CellRecord {
                    bounds_min: [9.0, 0.0, 0.0],
                    bounds_max: [12.0, 2.0, 2.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 1,
                    face_count: 1,
                    portal_ref_start: 0,
                    portal_ref_count: 0,
                },
            ],
            portal_refs: vec![0],
        };
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Cells as u32,
                version: 1,
                data: cells.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Portals as u32,
                version: 1,
                data: portals.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_portal_adjacency_mismatch.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "Portals",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_missing_geometry_section_as_stale_format() {
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            default_cell_draw_index_blob(),
            default_texture_cache_keys_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_missing_geometry.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::StaleFormatMissingSection {
                    section: "Geometry",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_malformed_geometry_section_as_section_validation() {
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: vec![0],
            },
            default_texture_cache_keys_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_malformed_geometry.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "Geometry",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_geometry_trailing_bytes_as_section_validation() {
        let mut geometry = sample_geometry().to_bytes();
        geometry.extend_from_slice(&[0xab, 0xcd]);
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geometry,
            },
            default_texture_cache_keys_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_geometry_trailing.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "Geometry",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_geometry_index_past_vertex_count() {
        let mut geometry = sample_geometry();
        geometry.indices[2] = 999;
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geometry.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_geometry_bad_index.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "Geometry",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_non_finite_geometry_position() {
        let mut geometry = sample_geometry();
        geometry.vertices[0].position[1] = f32::INFINITY;
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geometry.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_geometry_non_finite.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "Geometry",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_missing_bvh_section_as_stale_format() {
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
        assert!(
            matches!(
                err,
                PrlLoadError::StaleFormatMissingSection { section: "Bvh", .. }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_malformed_bvh_section_as_section_validation() {
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: vec![0],
            },
            default_texture_cache_keys_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_malformed_bvh.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_bvh_non_finite_aabb() {
        let mut bvh = sample_bvh_section();
        bvh.nodes[0].aabb_max[2] = f32::NAN;
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_bvh_non_finite_aabb.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_bvh_inverted_leaf_aabb() {
        let mut bvh = sample_bvh_section();
        bvh.leaves[1].aabb_min[0] = 20.0;
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_bvh_inverted_aabb.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_bvh_nonzero_node_padding() {
        let mut bvh = sample_bvh_section().to_bytes();
        let node_0_padding_offset = postretro_level_format::bvh::HEADER_SIZE + 36;
        bvh[node_0_padding_offset..node_0_padding_offset + 4].copy_from_slice(&1u32.to_le_bytes());
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh,
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_bvh_node_padding.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_bvh_skip_index_at_current_node_as_section_validation() {
        let mut bvh = sample_bvh_section().to_bytes();
        let node_1_skip_offset = postretro_level_format::bvh::HEADER_SIZE
            + postretro_level_format::bvh::NODE_STRIDE
            + 12;
        bvh[node_1_skip_offset..node_1_skip_offset + 4].copy_from_slice(&1u32.to_le_bytes());
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh,
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_bvh_bad_skip.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_bvh_leaves_without_nodes_as_section_validation() {
        let mut bvh = Vec::new();
        bvh.extend_from_slice(&0u32.to_le_bytes());
        bvh.extend_from_slice(&1u32.to_le_bytes());
        bvh.extend_from_slice(&0u32.to_le_bytes());
        bvh.extend_from_slice(&0u32.to_le_bytes());
        bvh.extend_from_slice(&[0u8; postretro_level_format::bvh::LEAF_STRIDE]);
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh,
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_bvh_leaves_without_nodes.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_bvh_leaf_index_range_past_geometry_indices() {
        let mut bvh = sample_bvh_section();
        bvh.leaves[0].index_offset = 5;
        bvh.leaves[0].index_count = 3;
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_bvh_leaf_index_oob.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_bvh_leaf_index_offset_inside_triangle() {
        let mut bvh = sample_bvh_section();
        bvh.leaves[0].index_offset = 1;
        bvh.leaves[0].index_count = 3;
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(
            sections,
            "postretro_test_bvh_leaf_index_offset_mid_triangle.prl",
        );
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_bvh_leaf_partial_triangle_index_count() {
        let mut bvh = sample_bvh_section();
        bvh.leaves[0].index_offset = 0;
        bvh.leaves[0].index_count = 2;
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(
            sections,
            "postretro_test_bvh_leaf_partial_triangle_count.prl",
        );
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_bvh_leaf_index_range_overflow() {
        let mut bvh = sample_bvh_section();
        bvh.leaves[0].index_offset = u32::MAX;
        bvh.leaves[0].index_count = 3;
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_bvh_leaf_index_overflow.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_bvh_leaf_node_reference_out_of_range() {
        let mut bvh = sample_bvh_section();
        bvh.nodes[1].left_child_or_leaf_index = 99;
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_bvh_leaf_node_ref.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_duplicate_bvh_leaf_node_reference() {
        let mut bvh = sample_bvh_section();
        bvh.nodes[2].left_child_or_leaf_index = 0;
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_bvh_duplicate_leaf_node_ref.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_bvh_node_unknown_flags() {
        let mut bvh = sample_bvh_section();
        bvh.nodes[0].flags = 0x2;
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_bvh_node_flags.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_missing_cells_section_as_stale_format() {
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            default_cell_locator_blob(),
            default_cell_draw_index_blob(),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
            default_octahedral_sh_volume_blob(),
        ];
        let tmp = write_prl_fixture_raw(sections, "postretro_test_missing_cells.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::StaleFormatMissingSection {
                    section: "Cells",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_malformed_cells_section_as_section_validation() {
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Cells as u32,
                version: 1,
                data: vec![0],
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
            default_octahedral_sh_volume_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_malformed_cells.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "Cells",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_missing_cell_locator_section_as_stale_format() {
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            default_cells_blob(),
            default_cell_draw_index_blob(),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
            default_octahedral_sh_volume_blob(),
        ];
        let tmp = write_prl_fixture_raw(sections, "postretro_test_missing_cell_locator.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::StaleFormatMissingSection {
                    section: "CellLocator",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_malformed_cell_locator_section_as_section_validation() {
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            default_cells_blob(),
            prl_format::SectionBlob {
                section_id: SectionId::CellLocator as u32,
                version: 1,
                data: vec![0],
            },
            default_cell_draw_index_blob(),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
            default_octahedral_sh_volume_blob(),
        ];
        let tmp = write_prl_fixture_raw(sections, "postretro_test_malformed_cell_locator.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "CellLocator",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_modern_prl_with_legacy_bsp_nodes_as_ambiguous() {
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            default_cells_blob(),
            default_cell_locator_blob(),
            default_cell_draw_index_blob(),
            prl_format::SectionBlob {
                section_id: SectionId::BspNodes as u32,
                version: 1,
                data: vec![0],
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
            default_octahedral_sh_volume_blob(),
        ];
        let tmp = write_prl_fixture_raw(sections, "postretro_test_ambiguous_bsp_nodes.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::AmbiguousRuntimeBspSections { .. }),
            "got {err:?}"
        );
        assert!(err.to_string().contains("BspNodes"));
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_modern_prl_with_legacy_bsp_leaves_as_ambiguous() {
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            default_cells_blob(),
            default_cell_locator_blob(),
            default_cell_draw_index_blob(),
            prl_format::SectionBlob {
                section_id: SectionId::BspLeaves as u32,
                version: 1,
                data: vec![0],
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
            default_octahedral_sh_volume_blob(),
        ];
        let tmp = write_prl_fixture_raw(sections, "postretro_test_ambiguous_bsp_leaves.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::AmbiguousRuntimeBspSections { .. }),
            "got {err:?}"
        );
        assert!(err.to_string().contains("BspLeaves"));
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
            default_cells_blob(),
            default_cell_locator_blob(),
            default_cell_draw_index_blob(),
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
                    leaf_index: ALPHA_LIGHT_LEAF_UNASSIGNED,
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
        assert_eq!(world.lights[0].cell_index, 0);

        assert_eq!(world.lights[1].light_type, LightType::Spot);
        assert_eq!(world.lights[1].falloff_model, FalloffModel::Linear);
        assert!((world.lights[1].cone_angle_inner - std::f32::consts::FRAC_PI_6).abs() < 1e-4);
        assert!((world.lights[1].cone_angle_outer - std::f32::consts::FRAC_PI_4).abs() < 1e-4);
        assert_eq!(world.lights[1].cone_direction, [0.0, -1.0, 0.0]);
        assert_eq!(world.lights[1].cell_index, 1);

        assert_eq!(world.lights[2].light_type, LightType::Directional);
        assert_eq!(world.lights[2].cell_index, ALPHA_LIGHT_LEAF_UNASSIGNED);

        std::fs::remove_file(&tmp).ok();
    }

    fn empty_bvh_blob() -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::Bvh as u32,
            version: 1,
            data: BvhSection {
                nodes: Vec::new(),
                leaves: Vec::new(),
                root_node_index: 0,
            }
            .to_bytes(),
        }
    }

    fn cells_with_second_flag(second_flags: u32) -> CellsSection {
        CellsSection {
            cells: vec![
                CellRecord {
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    flags: 0,
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 0,
                    portal_ref_count: 0,
                },
                CellRecord {
                    bounds_min: [9.0, 0.0, 0.0],
                    bounds_max: [12.0, 2.0, 2.0],
                    flags: second_flags,
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 0,
                    portal_ref_count: 0,
                },
            ],
            portal_refs: Vec::new(),
        }
    }

    #[test]
    fn load_prl_allows_light_in_exterior_cell() {
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: empty_geometry().to_bytes(),
            },
            empty_bvh_blob(),
            prl_format::SectionBlob {
                section_id: SectionId::Cells as u32,
                version: 1,
                data: cells_with_second_flag(CELL_FLAG_EXTERIOR).to_bytes(),
            },
            default_cell_locator_blob(),
            prl_format::SectionBlob {
                section_id: SectionId::AlphaLights as u32,
                version: 1,
                data: sample_alpha_lights().to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_exterior_light.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("exterior light placement is valid");
        assert_eq!(world.lights[1].cell_index, 1);
        assert!(world.cells[1].is_exterior);
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_light_in_solid_cell() {
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: empty_geometry().to_bytes(),
            },
            empty_bvh_blob(),
            prl_format::SectionBlob {
                section_id: SectionId::Cells as u32,
                version: 1,
                data: cells_with_second_flag(CELL_FLAG_SOLID).to_bytes(),
            },
            default_cell_locator_blob(),
            prl_format::SectionBlob {
                section_id: SectionId::AlphaLights as u32,
                version: 1,
                data: sample_alpha_lights().to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_solid_light.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "AlphaLights",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_zero_leaf_bvh_with_drawable_geometry() {
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            empty_bvh_blob(),
            default_cells_blob(),
            default_cell_locator_blob(),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_empty_bvh_with_geometry.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
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
        let masks = FogCellMasksSection {
            masks: vec![0x0000_0001, 0x0000_0001],
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
                section_id: SectionId::FogCellMasks as u32,
                version: 1,
                data: masks.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            fog_volumes_blob_with_count(1),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_fog_cell_masks.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("should load");

        assert_eq!(
            world.fog_cell_masks,
            Some(vec![0x0000_0001u32, 0x0000_0001])
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_fog_cell_masks_when_length_mismatches_cells() {
        use postretro_level_format::fog_cell_masks::FogCellMasksSection;

        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        // Two cells but only one mask: Task 5 makes this fatal and validates
        // against Cells.cell_count, not removed runtime BSP sections.
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
                section_id: SectionId::FogCellMasks as u32,
                version: 1,
                data: masks.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            fog_volumes_blob_with_count(1),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_fog_cell_masks_truncated.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "FogCellMasks",
                    ..
                }
            ),
            "got {err:?}"
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_malformed_fog_cell_masks_as_section_validation() {
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::FogCellMasks as u32,
                version: 1,
                data: vec![0],
            },
            default_texture_cache_keys_blob(),
            fog_volumes_blob_with_count(1),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_malformed_fog_cell_masks.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "FogCellMasks",
                    ..
                }
            ),
            "got {err:?}"
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_fog_cell_masks_when_masks_longer_than_cells() {
        use postretro_level_format::fog_cell_masks::FogCellMasksSection;

        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        // Two cells but three masks: Task 5 makes this fatal and validates
        // against Cells.cell_count, not removed runtime BSP sections.
        let masks = FogCellMasksSection {
            masks: vec![0x0000_0001, 0x0000_0001, 0x0000_0001],
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
                section_id: SectionId::FogCellMasks as u32,
                version: 1,
                data: masks.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            fog_volumes_blob_with_count(1),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_fog_cell_masks_oversized.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "FogCellMasks",
                    ..
                }
            ),
            "got {err:?}"
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_fog_cell_masks_bits_outside_canonical_slots() {
        use postretro_level_format::fog_cell_masks::FogCellMasksSection;

        let masks = FogCellMasksSection {
            // One canonical fog slot means bit 1 is outside all_slots_mask.
            masks: vec![0x0000_0001, 0x0000_0002],
        };
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::FogCellMasks as u32,
                version: 1,
                data: masks.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            fog_volumes_blob_with_count(1),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_fog_cell_masks_extra_bits.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "FogCellMasks",
                    ..
                }
            ),
            "got {err:?}"
        );
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

    // --- CellDrawIndex (section 37) cross-validation + load ---

    use postretro_level_format::cell_draw_index::{
        CELL_DRAW_INDEX_VERSION, CellDrawIndexSection, Span,
    };

    /// A runtime BVH leaf with the fields the cross-validation reads. Other
    /// fields are filler; the validator only inspects `material_bucket_id`,
    /// `index_count`, and `cell_id`.
    fn rt_bvh_leaf(material_bucket_id: u32, index_count: u32, cell_id: u32) -> BvhLeaf {
        BvhLeaf {
            aabb_min: [0.0; 3],
            material_bucket_id,
            aabb_max: [1.0; 3],
            index_offset: 0,
            index_count,
            cell_id,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }
    }

    /// Cell with only the drawability-relevant flags set.
    fn draw_index_cell(is_solid: bool, face_count: u32) -> CellData {
        simple_cell(Vec3::ZERO, Vec3::splat(1.0), 0, face_count, is_solid)
    }

    /// Two drawable cells, one drawable BVH leaf each, all in bucket 0.
    /// cell 0 → bvh leaf 0, cell 1 → bvh leaf 1.
    fn two_cell_setup() -> (Vec<BvhLeaf>, Vec<CellData>) {
        let bvh_leaves = vec![rt_bvh_leaf(0, 3, 0), rt_bvh_leaf(0, 3, 1)];
        let cells = vec![draw_index_cell(false, 1), draw_index_cell(false, 1)];
        (bvh_leaves, cells)
    }

    fn valid_two_cell_section() -> CellDrawIndexSection {
        CellDrawIndexSection {
            cell_count: 2,
            span_count: 2,
            cell_span_offset: vec![0, 1, 2],
            spans: vec![
                Span {
                    leaf_start: 0,
                    leaf_count: 1,
                },
                Span {
                    leaf_start: 1,
                    leaf_count: 1,
                },
            ],
        }
    }

    #[test]
    fn validate_cell_draw_index_accepts_valid_section() {
        let (bvh_leaves, leaves) = two_cell_setup();
        let section = valid_two_cell_section();
        assert!(
            validate_cell_draw_index(&section, &bvh_leaves, &leaves, CELL_DRAW_INDEX_VERSION)
                .is_ok()
        );
    }

    #[test]
    fn validate_cell_draw_index_rejects_unsupported_version() {
        let (bvh_leaves, leaves) = two_cell_setup();
        let section = valid_two_cell_section();
        let err =
            validate_cell_draw_index(&section, &bvh_leaves, &leaves, CELL_DRAW_INDEX_VERSION + 1)
                .unwrap_err();
        assert!(err.contains("version"), "got: {err}");
    }

    #[test]
    fn validate_cell_draw_index_rejects_wrong_cell_count() {
        let (bvh_leaves, _) = two_cell_setup();
        // Only one cell but the section declares two cells.
        let leaves = vec![draw_index_cell(false, 1)];
        let section = valid_two_cell_section();
        let err = validate_cell_draw_index(&section, &bvh_leaves, &leaves, CELL_DRAW_INDEX_VERSION)
            .unwrap_err();
        assert!(err.contains("cell_count"), "got: {err}");
    }

    #[test]
    fn validate_cell_draw_index_rejects_span_out_of_bounds() {
        let (bvh_leaves, leaves) = two_cell_setup();
        let mut section = valid_two_cell_section();
        // cell 1's span runs past the 2-leaf BVH array.
        section.spans[1] = Span {
            leaf_start: 1,
            leaf_count: 5,
        };
        let err = validate_cell_draw_index(&section, &bvh_leaves, &leaves, CELL_DRAW_INDEX_VERSION)
            .unwrap_err();
        assert!(err.contains("exceeds total BVH leaves"), "got: {err}");
    }

    #[test]
    fn validate_cell_draw_index_rejects_wrong_cell_span() {
        // bvh leaf 1 belongs to cell 1, but here cell 0 claims [0,2).
        let bvh_leaves = vec![rt_bvh_leaf(0, 3, 0), rt_bvh_leaf(0, 3, 1)];
        let leaves = vec![draw_index_cell(false, 1), draw_index_cell(false, 1)];
        let section = CellDrawIndexSection {
            cell_count: 2,
            span_count: 1,
            cell_span_offset: vec![0, 1, 1],
            spans: vec![Span {
                leaf_start: 0,
                leaf_count: 2,
            }],
        };
        let err = validate_cell_draw_index(&section, &bvh_leaves, &leaves, CELL_DRAW_INDEX_VERSION)
            .unwrap_err();
        assert!(err.contains("wrong cell"), "got: {err}");
    }

    #[test]
    fn validate_cell_draw_index_rejects_non_drawable_leaf_coverage() {
        // cell 1's BVH leaf has zero indices — not drawable, but the index
        // tries to cover it.
        let bvh_leaves = vec![rt_bvh_leaf(0, 3, 0), rt_bvh_leaf(0, 0, 1)];
        let leaves = vec![draw_index_cell(false, 1), draw_index_cell(false, 1)];
        let section = valid_two_cell_section();
        let err = validate_cell_draw_index(&section, &bvh_leaves, &leaves, CELL_DRAW_INDEX_VERSION)
            .unwrap_err();
        assert!(err.contains("non-drawable BVH leaf"), "got: {err}");
    }

    #[test]
    fn validate_cell_draw_index_rejects_span_on_non_drawable_cell() {
        // cell 1 is solid (non-drawable) but the index gives it a span.
        let bvh_leaves = vec![rt_bvh_leaf(0, 3, 0), rt_bvh_leaf(0, 3, 1)];
        let leaves = vec![draw_index_cell(false, 1), draw_index_cell(true, 0)];
        let section = valid_two_cell_section();
        let err = validate_cell_draw_index(&section, &bvh_leaves, &leaves, CELL_DRAW_INDEX_VERSION)
            .unwrap_err();
        assert!(err.contains("non-drawable BVH leaf"), "got: {err}");
    }

    // Regression guard for the review question: a span covering a leaf with
    // `index_count > 0` whose cell is non-drawable BECAUSE it is zero-face (but
    // NOT solid). `rejects_span_on_non_drawable_cell` covers the solid+zero-face
    // case; this isolates the other non-drawable sub-case so both halves of
    // `cell_is_drawable = !is_solid && face_count > 0` are pinned. The validator
    // already enforces this via the in-span `!leaf_is_drawable` check (a leaf is
    // drawable only if its cell is), so it must return Err.
    #[test]
    fn validate_cell_draw_index_rejects_index_count_leaf_on_zero_face_cell() {
        // cell 1: non-solid but zero faces → non-drawable. Its BVH leaf has
        // index_count == 3 (> 0), yet the index tries to cover it.
        let bvh_leaves = vec![rt_bvh_leaf(0, 3, 0), rt_bvh_leaf(0, 3, 1)];
        let leaves = vec![draw_index_cell(false, 1), draw_index_cell(false, 0)];
        let section = valid_two_cell_section();
        let err = validate_cell_draw_index(&section, &bvh_leaves, &leaves, CELL_DRAW_INDEX_VERSION)
            .unwrap_err();
        assert!(err.contains("non-drawable BVH leaf"), "got: {err}");
    }

    #[test]
    fn validate_cell_draw_index_rejects_bucket_crossing_span() {
        // cell 0 owns two BVH leaves in different buckets; one span can't cover
        // both.
        let bvh_leaves = vec![rt_bvh_leaf(0, 3, 0), rt_bvh_leaf(1, 3, 0)];
        let leaves = vec![draw_index_cell(false, 1)];
        let section = CellDrawIndexSection {
            cell_count: 1,
            span_count: 1,
            cell_span_offset: vec![0, 1],
            spans: vec![Span {
                leaf_start: 0,
                leaf_count: 2,
            }],
        };
        let err = validate_cell_draw_index(&section, &bvh_leaves, &leaves, CELL_DRAW_INDEX_VERSION)
            .unwrap_err();
        assert!(err.contains("material bucket"), "got: {err}");
    }

    #[test]
    fn validate_cell_draw_index_rejects_non_maximal_run() {
        // cell 0 owns leaves 0,1 in the same bucket but splits them into two
        // abutting spans that should have been one.
        let bvh_leaves = vec![rt_bvh_leaf(0, 3, 0), rt_bvh_leaf(0, 3, 0)];
        let leaves = vec![draw_index_cell(false, 1)];
        let section = CellDrawIndexSection {
            cell_count: 1,
            span_count: 2,
            cell_span_offset: vec![0, 2],
            spans: vec![
                Span {
                    leaf_start: 0,
                    leaf_count: 1,
                },
                Span {
                    leaf_start: 1,
                    leaf_count: 1,
                },
            ],
        };
        let err = validate_cell_draw_index(&section, &bvh_leaves, &leaves, CELL_DRAW_INDEX_VERSION)
            .unwrap_err();
        assert!(err.contains("non-maximal"), "got: {err}");
    }

    #[test]
    fn validate_cell_draw_index_rejects_missing_drawable_leaf() {
        // cell 1's drawable leaf is never covered (cell 1 row is empty).
        let (bvh_leaves, leaves) = two_cell_setup();
        let section = CellDrawIndexSection {
            cell_count: 2,
            span_count: 1,
            cell_span_offset: vec![0, 1, 1],
            spans: vec![Span {
                leaf_start: 0,
                leaf_count: 1,
            }],
        };
        let err = validate_cell_draw_index(&section, &bvh_leaves, &leaves, CELL_DRAW_INDEX_VERSION)
            .unwrap_err();
        assert!(err.contains("missing from the draw index"), "got: {err}");
    }

    #[test]
    fn validate_cell_draw_index_rejects_non_drawable_cell_with_nonempty_row() {
        // cell 0 drawable; cell 1 solid (non-drawable) yet carries a span over a
        // bvh leaf that names cell 1. The leaf is non-drawable (solid cell), so
        // the in-span drawability check fires first — both surface the
        // "non-drawable cell shouldn't have a row" intent.
        let bvh_leaves = vec![rt_bvh_leaf(0, 3, 0), rt_bvh_leaf(0, 0, 1)];
        let leaves = vec![draw_index_cell(false, 1), draw_index_cell(true, 0)];
        let section = valid_two_cell_section();
        let err = validate_cell_draw_index(&section, &bvh_leaves, &leaves, CELL_DRAW_INDEX_VERSION)
            .unwrap_err();
        assert!(
            err.contains("non-drawable") || err.contains("non-empty CSR row"),
            "got: {err}"
        );
    }

    #[test]
    fn validate_cell_draw_index_rejects_overlapping_coverage() {
        // Two cells, but both spans cover bvh leaf 0 (cell 1's span re-covers).
        // Construct directly to bypass structural CSR checks at this layer.
        let bvh_leaves = vec![rt_bvh_leaf(0, 3, 0), rt_bvh_leaf(0, 3, 1)];
        let leaves = vec![draw_index_cell(false, 1), draw_index_cell(false, 1)];
        // cell 0 → [0,1); cell 1 → also [0,1) (wrong + overlap). The wrong-cell
        // check trips first, which is itself a rejection — assert it fails.
        let section = CellDrawIndexSection {
            cell_count: 2,
            span_count: 2,
            cell_span_offset: vec![0, 1, 2],
            spans: vec![
                Span {
                    leaf_start: 0,
                    leaf_count: 1,
                },
                Span {
                    leaf_start: 0,
                    leaf_count: 1,
                },
            ],
        };
        assert!(
            validate_cell_draw_index(&section, &bvh_leaves, &leaves, CELL_DRAW_INDEX_VERSION)
                .is_err()
        );
    }

    fn cell_draw_index_blob(section: &CellDrawIndexSection) -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::CellDrawIndex as u32,
            version: CELL_DRAW_INDEX_VERSION as u16,
            data: section.to_bytes(),
        }
    }

    fn base_cell_draw_index_sections() -> Vec<prl_format::SectionBlob> {
        vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: sample_bvh_section().to_bytes(),
            },
            default_cells_blob(),
            default_cell_locator_blob(),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ]
    }

    #[test]
    fn load_prl_absent_cell_draw_index_for_nonempty_bvh_is_error() {
        let mut sections = base_cell_draw_index_sections();
        sections.push(default_octahedral_sh_volume_blob());
        let tmp = write_prl_fixture_raw(sections, "postretro_test_no_cell_draw_index.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "CellDrawIndex",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_cell_draw_index_for_empty_bvh() {
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: empty_geometry().to_bytes(),
            },
            empty_bvh_blob(),
            prl_format::SectionBlob {
                section_id: SectionId::Cells as u32,
                version: 1,
                data: cells_with_second_flag(0).to_bytes(),
            },
            default_cell_locator_blob(),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
            cell_draw_index_blob(&valid_two_cell_section()),
        ];
        let tmp = write_prl_fixture(
            sections,
            "postretro_test_empty_bvh_with_cell_draw_index.prl",
        );
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "CellDrawIndex",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_parses_valid_cell_draw_index() {
        let mut sections = base_cell_draw_index_sections();
        sections.push(cell_draw_index_blob(&valid_two_cell_section()));
        let tmp = write_prl_fixture(sections, "postretro_test_valid_cell_draw_index.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("PRL with valid index must load");
        let index = world
            .cell_draw_index
            .as_ref()
            .expect("valid CellDrawIndex must round-trip into LevelWorld");
        assert_eq!(index.cell_count, 2);
        assert_eq!(index.span_count, 2);
        std::fs::remove_file(&tmp).ok();
    }

    /// Every representative invalid section is fatal once the BVH is non-empty.
    #[test]
    fn load_prl_invalid_cell_draw_index_is_error() {
        // (label, section) pairs covering the representative reject cases.
        let cases: Vec<(&str, CellDrawIndexSection)> = vec![
            (
                "wrong cell_count",
                CellDrawIndexSection {
                    cell_count: 3, // topology has 2 cells
                    span_count: 2,
                    cell_span_offset: vec![0, 1, 2, 2],
                    spans: vec![
                        Span {
                            leaf_start: 0,
                            leaf_count: 1,
                        },
                        Span {
                            leaf_start: 1,
                            leaf_count: 1,
                        },
                    ],
                },
            ),
            (
                "span out of bounds",
                CellDrawIndexSection {
                    cell_count: 2,
                    span_count: 2,
                    cell_span_offset: vec![0, 1, 2],
                    spans: vec![
                        Span {
                            leaf_start: 0,
                            leaf_count: 1,
                        },
                        Span {
                            leaf_start: 1,
                            leaf_count: 9, // runs past the 2-leaf array
                        },
                    ],
                },
            ),
            (
                "wrong-cell span",
                CellDrawIndexSection {
                    cell_count: 2,
                    span_count: 1,
                    cell_span_offset: vec![0, 1, 1],
                    spans: vec![Span {
                        leaf_start: 0,
                        leaf_count: 2, // cell 0 claiming leaf 1 (cell 1's)
                    }],
                },
            ),
            (
                "missing drawable leaf",
                CellDrawIndexSection {
                    cell_count: 2,
                    span_count: 1,
                    cell_span_offset: vec![0, 1, 1],
                    spans: vec![Span {
                        leaf_start: 0,
                        leaf_count: 1, // cell 1's leaf never covered
                    }],
                },
            ),
        ];

        for (i, (label, section)) in cases.into_iter().enumerate() {
            let mut sections = base_cell_draw_index_sections();
            sections.push(cell_draw_index_blob(&section));
            let tmp = write_prl_fixture(
                sections,
                &format!("postretro_test_invalid_cell_draw_index_{i}.prl"),
            );
            let err = load_prl(tmp.to_str().unwrap())
                .err()
                .unwrap_or_else(|| panic!("[{label}] load should fail"));
            assert!(
                matches!(
                    err,
                    PrlLoadError::SectionValidation {
                        section: "CellDrawIndex",
                        ..
                    }
                ),
                "[{label}] expected CellDrawIndex validation error, got {err:?}"
            );
            std::fs::remove_file(&tmp).ok();
        }
    }

    /// A bucket-crossing span is fatal at the load layer too.
    #[test]
    fn load_prl_bucket_crossing_cell_draw_index_is_error() {
        // Build a BVH whose two leaves for cell 0 sit in different buckets, then
        // hand the index a single span covering both.
        let bvh = BvhSection {
            nodes: sample_bvh_section().nodes,
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
                    material_bucket_id: 1,
                    aabb_max: [12.0, 2.0, 2.0],
                    index_offset: 3,
                    index_count: 3,
                    cell_id: 0,
                    chunk_range_start: 0,
                    chunk_range_count: 0,
                },
            ],
            root_node_index: 0,
        };
        let section = CellDrawIndexSection {
            cell_count: 1,
            span_count: 1,
            cell_span_offset: vec![0, 1],
            spans: vec![Span {
                leaf_start: 0,
                leaf_count: 2, // crosses bucket 0 → 1
            }],
        };
        let cells = CellsSection {
            cells: vec![CellRecord {
                bounds_min: [0.0, 0.0, 0.0],
                bounds_max: [12.0, 2.0, 2.0],
                flags: CELL_FLAG_DRAWABLE,
                face_start: 0,
                face_count: 1,
                portal_ref_start: 0,
                portal_ref_count: 0,
            }],
            portal_refs: Vec::new(),
        };
        let locator = CellLocatorSection {
            root: FormatCellLocatorChild::Cell(0),
            nodes: Vec::new(),
        };

        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: sample_geometry().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: bvh.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Cells as u32,
                version: 1,
                data: cells.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::CellLocator as u32,
                version: 1,
                data: locator.to_bytes(),
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
            cell_draw_index_blob(&section),
        ];
        let tmp = write_prl_fixture(
            sections,
            "postretro_test_bucket_crossing_cell_draw_index.prl",
        );
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "CellDrawIndex",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(&tmp).ok();
    }
}
