// Shared runtime LevelWorld data model for slim visibility-only worlds and
// full PRL loads. File decoding lives in prl_loader.rs behind `load-prl`.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::error::Error as StdError;
use std::fmt;

use glam::Vec3;
#[cfg(feature = "load-prl")]
use postretro_level_format as prl_format;
#[cfg(feature = "load-prl")]
use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::cell_draw_index::CellDrawIndexSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::chunk_light_list::ChunkLightListSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::data_script::DataScriptSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::fog_volumes::FogVolumeRecord;
#[cfg(feature = "load-prl")]
use postretro_level_format::geometry::Vertex as PrlVertex;
#[cfg(feature = "load-prl")]
use postretro_level_format::kinematic_geometry::{
    KinematicMoverRecord, KinematicWaypointRecord, MemberLight,
};
#[cfg(feature = "load-prl")]
use postretro_level_format::lightmap::LightmapSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::map_entity::MapEntityRecord;
#[cfg(feature = "load-prl")]
use postretro_level_format::navmesh::NavMeshSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::sdf_atlas::SdfAtlasSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
#[cfg(feature = "load-prl")]
use postretro_level_format::trigger_volumes::TriggerVolumeRecord;
#[cfg(feature = "load-prl")]
use thiserror::Error;

#[cfg(feature = "load-prl")]
use postretro_render_data::geometry::{BvhTree, WorldVertex};
#[cfg(feature = "load-prl")]
use postretro_render_data::influence::LightInfluence;
#[cfg(feature = "load-prl")]
use postretro_render_data::material::Material;

/// Stable runtime cell identifier. It is the compiler BSP leaf index.
pub type CellId = usize;

/// One canonical, unordered coupled cell pair with baked graded details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoupledCellPair {
    pub cell_a: CellId,
    pub cell_b: CellId,
    pub distance: u32,
    pub aperture: u32,
}

/// Consumer-neutral cell-to-cell coupling result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CouplingTuple {
    pub perceivable: bool,
    pub distance: Option<u32>,
    pub aperture: Option<u32>,
}

/// Loaded static portal-graph coupling data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellVisibility {
    component_ids: Vec<u32>,
    coupled_pairs: Vec<CoupledCellPair>,
}

impl CellVisibility {
    pub(crate) fn new(component_ids: Vec<u32>, coupled_pairs: Vec<CoupledCellPair>) -> Self {
        Self {
            component_ids,
            coupled_pairs,
        }
    }

    /// One conservative portal-reachability component ID per cell.
    pub fn component_ids(&self) -> &[u32] {
        &self.component_ids
    }

    /// Canonically sorted coupled off-diagonal pairs with graded detail.
    pub fn coupled_pairs(&self) -> std::slice::Iter<'_, CoupledCellPair> {
        self.coupled_pairs.iter()
    }

    fn perceivable(&self, a: CellId, b: CellId) -> bool {
        self.component_ids.get(a) == self.component_ids.get(b)
    }

    fn coupled_pair(&self, a: CellId, b: CellId) -> Option<CoupledCellPair> {
        if a == b {
            return None;
        }
        let (cell_a, cell_b) = (a.min(b), a.max(b));
        self.coupled_pairs
            .binary_search_by_key(&(cell_a, cell_b), |pair| (pair.cell_a, pair.cell_b))
            .ok()
            .map(|index| self.coupled_pairs[index])
    }
}

#[cfg(feature = "load-prl")]
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
    #[error(
        "DirectShDeltaVolumes affinity_factor {found} != engine AFFINITY_FACTOR {expected} — recompile the .prl with the current `prl-build`"
    )]
    DirectShDeltaAffinityFactorMismatch { found: u8, expected: u8 },
    #[error(
        "DirectShDeltaVolumes affinity_dims {found:?} != ceil(base DirectShVolume dims {base_dims:?} / {factor}) = {expected:?} — recompile the .prl with the current `prl-build`"
    )]
    DirectShDeltaAffinityDimsMismatch {
        found: [u32; 3],
        base_dims: [u32; 3],
        factor: u32,
        expected: [u32; 3],
    },
    #[error(
        "DirectShDeltaVolumes tile geometry {found_dimension}+border {found_border} does not match base DirectShVolume tile geometry {base_dimension}+border {base_border} — recompile the .prl with the current `prl-build`"
    )]
    DirectShDeltaTileGeometryMismatch {
        found_dimension: u32,
        found_border: u32,
        base_dimension: u32,
        base_border: u32,
    },
    #[error(
        "AnimatedDirectShDeltaVolumes valid_probe_masks[{cell}] {found:#018x} disagrees with OctahedralShVolume (id 34) validity {expected:#018x} — recompile the .prl with the current `prl-build`"
    )]
    AnimatedDirectShDeltaValidityMismatch {
        cell: usize,
        found: u64,
        expected: u64,
    },
}

/// Face → index-range mapping lives on BVH leaves; `FaceMeta` carries only
/// the per-face attributes CPU code still needs (lighting baker, editor diagnostics).
#[cfg(feature = "load-prl")]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelWorldValidationError {
    message: String,
}

impl LevelWorldValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for LevelWorldValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl StdError for LevelWorldValidationError {}

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
/// `StaticLightMap`. See `context/lib/build_pipeline.md`.
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
    /// the animated-baked descriptor path, not here. Legacy PRLs retain their
    /// stored value on parse.
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
    /// table.
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
/// for runtime SDF visibility to multiply in.
///
/// Current bakes load as `Shadowed`: a missing lightmap-mode marker decodes
/// that way, and shadowed bakes omit the marker for wire compatibility.
/// `Unshadowed` remains for legacy wire compatibility, not as current bake
/// output.
#[cfg(feature = "load-prl")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LightmapMode {
    /// Static-light visibility folded into the bake. Forward must NOT multiply
    /// by SDF visibility.
    #[default]
    Shadowed,
    /// Visibility term removed from the bake. Forward MUST multiply by SDF
    /// visibility to recover shadowed lighting. Retained for legacy wire
    /// compatibility.
    #[allow(dead_code)]
    Unshadowed,
}

/// Runtime view of the `CellDrawIndex` PRL section (id 37): each cell's owned
/// BVH-leaf spans in CSR layout. Held as the format type after the loader has
/// cross-validated it against the BVH leaf array and loaded Cells section. A
/// stable runtime name so the candidate-cull GPU path consumes one type.
#[cfg(feature = "load-prl")]
pub type CellDrawIndex = CellDrawIndexSection;

#[cfg(feature = "load-prl")]
#[derive(Debug, Clone, Default)]
pub struct KinematicGeometry {
    pub movers: Vec<LoadedKinematicMover>,
    pub waypoints: Vec<LoadedKinematicWaypoint>,
}

#[cfg(feature = "load-prl")]
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedKinematicMover {
    pub mover_id: u32,
    pub name: String,
    pub tags: Vec<String>,
    pub origin: Vec3,
    pub path: String,
    pub speed_mps: f32,
    pub wait_ms: f32,
    pub move_mode: u8,
    pub start_on_spawn: bool,
    pub vertices: Vec<PrlVertex>,
    pub indices: Vec<u32>,
    pub face_meta: Vec<postretro_level_format::geometry::FaceMeta>,
    pub spin_axis: Vec3,
    pub spin_speed_deg_s: f32,
    pub spin_accel_deg_s2: f32,
    pub carry_yaw: bool,
    pub block_policy: String,
    pub crush_damage: f32,
    pub crush_interval_ms: f32,
    pub auto_close_ms: Option<f32>,
    pub open_event: Option<String>,
    pub close_event: Option<String>,
    pub blocked_event: Option<String>,
    pub crush_event: Option<String>,
    pub sealed_portal_ids: Vec<u32>,
    pub carried_lights: Vec<LoadedMemberLight>,
}

/// Runtime copy of a KinematicGeometry member-light relation. Alpha-light
/// indices are positional in the loaded `LevelWorld::lights` table.
#[cfg(feature = "load-prl")]
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedMemberLight {
    pub alpha_light_index: u32,
    pub local_offset: Vec3,
}

#[cfg(feature = "load-prl")]
impl From<MemberLight> for LoadedMemberLight {
    fn from(record: MemberLight) -> Self {
        Self {
            alpha_light_index: record.alpha_light_index,
            local_offset: Vec3::from(record.local_offset),
        }
    }
}

#[cfg(feature = "load-prl")]
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedKinematicWaypoint {
    pub name: String,
    pub next: String,
    pub origin: Vec3,
}

#[cfg(feature = "load-prl")]
impl From<KinematicMoverRecord> for LoadedKinematicMover {
    fn from(record: KinematicMoverRecord) -> Self {
        Self {
            mover_id: record.mover_id,
            name: record.name,
            tags: record.tags,
            origin: Vec3::from(record.origin),
            path: record.path,
            speed_mps: record.speed,
            wait_ms: record.wait_ms,
            move_mode: record.move_mode,
            start_on_spawn: record.start_on_spawn,
            vertices: record.vertices,
            indices: record.indices,
            face_meta: record.face_meta,
            spin_axis: Vec3::from(record.spin_axis),
            spin_speed_deg_s: record.spin_speed_deg_s,
            spin_accel_deg_s2: record.spin_accel_deg_s2,
            carry_yaw: record.carry_yaw,
            block_policy: record.block_policy,
            crush_damage: record.crush_damage,
            crush_interval_ms: record.crush_interval_ms,
            auto_close_ms: record.auto_close_ms,
            open_event: record.open_event,
            close_event: record.close_event,
            blocked_event: record.blocked_event,
            crush_event: record.crush_event,
            sealed_portal_ids: record.sealed_portal_ids,
            carried_lights: record.carried_lights.into_iter().map(Into::into).collect(),
        }
    }
}

#[cfg(feature = "load-prl")]
impl From<KinematicWaypointRecord> for LoadedKinematicWaypoint {
    fn from(record: KinematicWaypointRecord) -> Self {
        Self {
            name: record.name,
            next: record.next,
            origin: Vec3::from(record.origin),
        }
    }
}

#[derive(Debug)]
pub struct LevelWorld {
    #[cfg(feature = "load-prl")]
    pub vertices: Vec<WorldVertex>,
    #[cfg(feature = "load-prl")]
    pub indices: Vec<u32>,
    #[cfg(feature = "load-prl")]
    pub face_meta: Vec<FaceMeta>,
    /// Preferred spatial contract: runtime cells preserving compiler cell ids.
    pub cells: Vec<CellData>,
    /// Flat portal-index adjacency referenced by `cells[*].portal_ref_*`.
    pub cell_portal_refs: Vec<u32>,
    pub cell_locator_root: CellLocatorChild,
    pub cell_locator_nodes: Vec<CellLocatorNodeData>,
    pub portals: Vec<PortalData>,
    pub has_portals: bool,
    /// Optional baked CellVisibility section. `None` is the conservative
    /// all-perceivable fallback for maps compiled before the section existed.
    pub cell_visibility: Option<CellVisibility>,
    #[cfg(feature = "load-prl")]
    pub texture_names: Vec<String>,
    /// Per-texture blake3 cache keys (PRL section 32), parallel to `texture_names`.
    /// Required — loader rejects files where the section is absent.
    #[cfg(feature = "load-prl")]
    pub texture_cache_keys: TextureCacheKeysSection,
    /// Always present — loader rejects files without a BVH section.
    #[cfg(feature = "load-prl")]
    pub bvh: BvhTree,
    /// Empty when section 18 is absent (maps predating lighting foundation).
    #[cfg(feature = "load-prl")]
    pub lights: Vec<MapLight>,
    /// Index `i` corresponds to `lights[i]`. Empty → all lights treated as infinite-bound.
    #[cfg(feature = "load-prl")]
    pub light_influences: Vec<LightInfluence>,
    /// Required octahedral irradiance atlas. Empty geometry uses a present
    /// section with zero grid dimensions; missing section means stale PRL.
    #[cfg(feature = "load-prl")]
    pub sh_volume: Option<OctahedralShVolumeSection>,
    /// `None` → 1×1 white placeholder; bumped-Lambert degrades to flat white.
    #[cfg(feature = "load-prl")]
    pub lightmap: Option<LightmapSection>,
    /// Whether the lightmap bake includes static-light visibility (`Shadowed`)
    /// or carries unshadowed irradiance that requires runtime SDF visibility
    /// multiplication (`Unshadowed`). Legacy PRLs without the on-disk marker
    /// parse as `Shadowed`.
    #[cfg(feature = "load-prl")]
    pub lightmap_mode: LightmapMode,
    /// `None` → no static-occluder SDF atlas (legacy PRL or empty-geometry
    /// bake). Runtime SDF shadowing is skipped.
    #[cfg(feature = "load-prl")]
    pub sdf_atlas: Option<SdfAtlasSection>,
    /// `None` → full spec-buffer scan fallback. See `ChunkGrid::fallback`.
    #[cfg(feature = "load-prl")]
    pub chunk_light_list: Option<ChunkLightListSection>,
    /// Emitted by `prl-build` for animated-light maps; cross-checked against weight-map chunk count.
    #[cfg(feature = "load-prl")]
    pub animated_light_chunks: Option<AnimatedLightChunksSection>,
    /// `None` when no animated lights — renderer binds a 1×1 zero atlas.
    #[cfg(feature = "load-prl")]
    pub animated_light_weight_maps: Option<AnimatedLightWeightMapsSection>,
    /// Sparse, affinity-cell-indexed (CSR) per-animated-light SH deltas at peak
    /// brightness. `None` when no animated lights — compose pass falls back to
    /// base→total copy.
    #[cfg(feature = "load-prl")]
    pub delta_sh_volumes: Option<DeltaShVolumesSection>,
    /// Dense baked DIRECT static-light octahedral atlas for dynamic objects
    /// (mesh entities + billboards). `None` when the map has no static direct
    /// SH/static lights, or when the section is unusable — dynamic objects fall
    /// back to indirect-only (the renderer binds a 4×4 BC6H zero dummy). Tile
    /// geometry is byte-identical to `sh_volume`; the runtime reuses that
    /// section's grid uniform + depth moments.
    #[cfg(feature = "load-prl")]
    pub direct_sh_volume: Option<DirectShVolumeSection>,
    /// Sparse direct-SH deltas for selected static lights. `None` when no
    /// selected lights were emitted, or when deltas are missing/unusable.
    /// Missing or unusable deltas also clear `entity_shadow_lights`.
    #[cfg(feature = "load-prl")]
    pub direct_sh_delta_volumes: Option<DirectShDeltaVolumesSection>,
    /// Sparse direct-SH deltas for animated baked lights. `None` when the map
    /// has no animated baked lights or the optional section is malformed.
    #[cfg(feature = "load-prl")]
    pub animated_direct_sh_delta_volumes: Option<AnimatedDirectShDeltaVolumesSection>,
    /// Normal-free static direct scatter for billboard shading. `None` selects
    /// legacy billboard direct lighting. A usable animated companion (id 48)
    /// is required whenever id 45 is present.
    #[cfg(feature = "load-prl")]
    pub billboard_direct_scatter_volume: Option<BillboardDirectScatterVolumeSection>,
    /// Dense animated billboard direct-scatter deltas. This is exposed only
    /// with a usable `billboard_direct_scatter_volume` base.
    #[cfg(feature = "load-prl")]
    pub animated_billboard_direct_scatter_delta_volumes:
        Option<AnimatedBillboardDirectScatterDeltaVolumesSection>,
    /// Runtime level-light indices selected by the compiler for static-light
    /// entity-shadow promotion. Empty for maps without direct SH/static lights,
    /// maps whose compiler selection found no eligible lights, or maps whose
    /// direct-SH deltas are missing/unusable.
    #[cfg(feature = "load-prl")]
    pub entity_shadow_lights: Vec<u32>,
    /// Per-selected-light world visibility masks for entity→world static-light
    /// shadows. `channels[i]` aligns with `entity_shadow_lights[i]`.
    #[cfg(feature = "load-prl")]
    pub shadowmask_atlas: Option<ShadowmaskAtlasSection>,
    /// `None` when level has no `data_script` worldspawn KVP.
    /// See: context/lib/scripting.md §2 (Data context lifecycle)
    #[cfg(feature = "load-prl")]
    pub data_script: Option<DataScriptSection>,
    /// Held as wire type — loader doesn't depend on scripting tree.
    /// Dispatch entry point converts to `scripting::map_entity::MapEntity`.
    #[cfg(feature = "load-prl")]
    pub map_entities: Vec<MapEntityRecord>,
    /// Runtime-loaded kinematic brush mover records (PRL section 43). Empty
    /// when the section is absent or contains no movers.
    #[cfg(feature = "load-prl")]
    pub kinematic_geometry: KinematicGeometry,
    /// Invisible trigger AABBs and declarative command records (section 44).
    #[cfg(feature = "load-prl")]
    pub trigger_volumes: Vec<TriggerVolumeRecord>,
    /// Empty when section absent or no `fog_volume` brushes authored.
    #[cfg(feature = "load-prl")]
    pub fog_volumes: Vec<FogVolumeRecord>,
    /// Downscale factor (1=full-res, 8=coarsest). Defaults to 4 when absent.
    #[cfg(feature = "load-prl")]
    pub fog_pixel_scale: u32,
    /// Seeds `App::current_gravity` so `world.getGravity()` sees the authored value before scripts run.
    #[cfg(feature = "load-prl")]
    pub initial_gravity: f32,
    /// `masks[C]` has bit `i` set when fog volume `i` overlaps cell `C`.
    /// `None` only when the map has no canonical fog volumes.
    #[cfg(feature = "load-prl")]
    pub fog_cell_masks: Option<Vec<u32>>,
    /// Baked navigation graph (PRL section 36). `None` for maps without a
    /// navmesh bake. Startup builds a runtime `NavGraph` from this section;
    /// pathfinding and the dev-tools overlay consume that graph. A malformed
    /// section warns and decodes to `None` rather than failing the load.
    #[cfg(feature = "load-prl")]
    #[allow(dead_code)]
    pub navmesh: Option<NavMeshSection>,
    /// Per-cell BVH-leaf draw index (PRL section 37), cross-validated against
    /// the BVH leaf array and loaded Cells section. `None` only for empty-BVH
    /// maps, where the section must be omitted.
    #[cfg(feature = "load-prl")]
    pub cell_draw_index: Option<CellDrawIndex>,
}

impl LevelWorld {
    pub fn new_visibility_only(
        cells: Vec<CellData>,
        cell_portal_refs: Vec<u32>,
        cell_locator_root: CellLocatorChild,
        cell_locator_nodes: Vec<CellLocatorNodeData>,
        portals: Vec<PortalData>,
        has_portals: bool,
    ) -> Result<Self, LevelWorldValidationError> {
        validate_visibility_only_world(
            &cells,
            &cell_portal_refs,
            cell_locator_root,
            &cell_locator_nodes,
            &portals,
            has_portals,
        )?;

        #[cfg(feature = "load-prl")]
        let face_meta_count: usize = cells.iter().map(|cell| cell.face_count as usize).sum();

        Ok(Self {
            #[cfg(feature = "load-prl")]
            vertices: vec![],
            #[cfg(feature = "load-prl")]
            indices: vec![],
            #[cfg(feature = "load-prl")]
            face_meta: (0..face_meta_count)
                .map(|_| FaceMeta {
                    leaf_index: 0,
                    texture_index: None,
                    texture_dimensions: (64, 64),
                    texture_name: String::new(),
                    material: Material::Default,
                })
                .collect(),
            cells,
            cell_portal_refs,
            cell_locator_root,
            cell_locator_nodes,
            portals,
            has_portals,
            cell_visibility: None,
            #[cfg(feature = "load-prl")]
            texture_names: vec![],
            #[cfg(feature = "load-prl")]
            texture_cache_keys: TextureCacheKeysSection { keys: vec![] },
            #[cfg(feature = "load-prl")]
            bvh: BvhTree {
                nodes: vec![],
                leaves: vec![],
                root_node_index: 0,
            },
            #[cfg(feature = "load-prl")]
            lights: vec![],
            #[cfg(feature = "load-prl")]
            light_influences: vec![],
            #[cfg(feature = "load-prl")]
            sh_volume: None,
            #[cfg(feature = "load-prl")]
            lightmap: None,
            #[cfg(feature = "load-prl")]
            lightmap_mode: LightmapMode::Shadowed,
            #[cfg(feature = "load-prl")]
            sdf_atlas: None,
            #[cfg(feature = "load-prl")]
            chunk_light_list: None,
            #[cfg(feature = "load-prl")]
            animated_light_chunks: None,
            #[cfg(feature = "load-prl")]
            animated_light_weight_maps: None,
            #[cfg(feature = "load-prl")]
            delta_sh_volumes: None,
            #[cfg(feature = "load-prl")]
            direct_sh_volume: None,
            #[cfg(feature = "load-prl")]
            direct_sh_delta_volumes: None,
            #[cfg(feature = "load-prl")]
            animated_direct_sh_delta_volumes: None,
            #[cfg(feature = "load-prl")]
            billboard_direct_scatter_volume: None,
            #[cfg(feature = "load-prl")]
            animated_billboard_direct_scatter_delta_volumes: None,
            #[cfg(feature = "load-prl")]
            entity_shadow_lights: Vec::new(),
            #[cfg(feature = "load-prl")]
            shadowmask_atlas: None,
            #[cfg(feature = "load-prl")]
            data_script: None,
            #[cfg(feature = "load-prl")]
            map_entities: Vec::new(),
            #[cfg(feature = "load-prl")]
            kinematic_geometry: KinematicGeometry::default(),
            #[cfg(feature = "load-prl")]
            trigger_volumes: Vec::new(),
            #[cfg(feature = "load-prl")]
            fog_volumes: Vec::new(),
            #[cfg(feature = "load-prl")]
            fog_pixel_scale: 4,
            #[cfg(feature = "load-prl")]
            initial_gravity: -9.81,
            #[cfg(feature = "load-prl")]
            fog_cell_masks: None,
            #[cfg(feature = "load-prl")]
            navmesh: None,
            #[cfg(feature = "load-prl")]
            cell_draw_index: None,
        })
    }

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

    /// Whether two cells share the conservative portal-reachability component.
    /// Missing baked data deliberately treats every pair as perceivable.
    pub fn perceivable(&self, a: CellId, b: CellId) -> bool {
        self.cell_visibility
            .as_ref()
            .map(|visibility| visibility.perceivable(a, b))
            .unwrap_or(true)
    }

    /// Baked consumer-neutral coupling axes for two cells.
    ///
    /// Graded axes are absent when the optional section or its pair record is
    /// absent.
    pub fn coupling(&self, a: CellId, b: CellId) -> CouplingTuple {
        let perceivable = self.perceivable(a, b);
        let pair = self
            .cell_visibility
            .as_ref()
            .and_then(|visibility| visibility.coupled_pair(a, b));
        CouplingTuple {
            perceivable,
            distance: pair.map(|pair| pair.distance),
            aperture: pair.map(|pair| pair.aperture),
        }
    }

    pub fn total_face_count(&self) -> u32 {
        #[cfg(feature = "load-prl")]
        {
            self.face_meta.len() as u32
        }

        #[cfg(not(feature = "load-prl"))]
        {
            self.cells.iter().map(|cell| cell.face_count).sum()
        }
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

fn validate_visibility_only_world(
    cells: &[CellData],
    cell_portal_refs: &[u32],
    cell_locator_root: CellLocatorChild,
    cell_locator_nodes: &[CellLocatorNodeData],
    portals: &[PortalData],
    has_portals: bool,
) -> Result<(), LevelWorldValidationError> {
    validate_visibility_locator(cell_locator_root, cell_locator_nodes, cells.len())?;
    validate_visibility_cell_portal_refs(cells, cell_portal_refs, portals.len(), has_portals)?;

    if has_portals && portals.is_empty() {
        return Err(LevelWorldValidationError::new(
            "has_portals is true but portals is empty",
        ));
    }
    if !has_portals && !portals.is_empty() {
        return Err(LevelWorldValidationError::new(
            "has_portals is false but portals is not empty",
        ));
    }

    if has_portals {
        validate_visibility_portal_adjacency(cells, cell_portal_refs, portals)?;
    }

    Ok(())
}

fn validate_visibility_locator(
    root: CellLocatorChild,
    nodes: &[CellLocatorNodeData],
    cell_count: usize,
) -> Result<(), LevelWorldValidationError> {
    let mut active = vec![false; nodes.len()];
    validate_visibility_locator_child(root, nodes, cell_count, &mut active)
}

fn validate_visibility_locator_child(
    child: CellLocatorChild,
    nodes: &[CellLocatorNodeData],
    cell_count: usize,
    active: &mut [bool],
) -> Result<(), LevelWorldValidationError> {
    match child {
        CellLocatorChild::Cell(cell_idx) => {
            if cell_count == 0 {
                if cell_idx == 0 {
                    return Ok(());
                }
                return Err(LevelWorldValidationError::new(format!(
                    "locator references cell {cell_idx}, but the world has no cells"
                )));
            }
            if cell_idx >= cell_count {
                return Err(LevelWorldValidationError::new(format!(
                    "locator references cell {cell_idx}, but the world has {cell_count} cells"
                )));
            }
            Ok(())
        }
        CellLocatorChild::Node(node_idx) => {
            let node = nodes.get(node_idx).ok_or_else(|| {
                LevelWorldValidationError::new(format!(
                    "locator references node {node_idx}, but the world has {} locator nodes",
                    nodes.len()
                ))
            })?;
            if active[node_idx] {
                return Err(LevelWorldValidationError::new(format!(
                    "locator contains a cycle through node {node_idx}"
                )));
            }
            active[node_idx] = true;
            validate_visibility_locator_child(node.front, nodes, cell_count, active)?;
            validate_visibility_locator_child(node.back, nodes, cell_count, active)?;
            active[node_idx] = false;
            Ok(())
        }
    }
}

fn validate_visibility_cell_portal_refs(
    cells: &[CellData],
    cell_portal_refs: &[u32],
    portal_count: usize,
    has_portals: bool,
) -> Result<(), LevelWorldValidationError> {
    for (cell_idx, cell) in cells.iter().enumerate() {
        let start = cell.portal_ref_start as usize;
        let count = cell.portal_ref_count as usize;
        let end = start.checked_add(count).ok_or_else(|| {
            LevelWorldValidationError::new(format!(
                "cell {cell_idx} portal_ref_start {start} + portal_ref_count {count} overflows usize"
            ))
        })?;
        let refs = cell_portal_refs.get(start..end).ok_or_else(|| {
            LevelWorldValidationError::new(format!(
                "cell {cell_idx} portal ref range [{start}..{end}) exceeds portal_refs length {}",
                cell_portal_refs.len()
            ))
        })?;
        for window in refs.windows(2) {
            if window[1] <= window[0] {
                return Err(LevelWorldValidationError::new(format!(
                    "cell {cell_idx} portal_refs must be sorted ascending and duplicate-free, got {} then {}",
                    window[0], window[1]
                )));
            }
        }
        if has_portals {
            for &portal_ref in refs {
                if portal_ref as usize >= portal_count {
                    return Err(LevelWorldValidationError::new(format!(
                        "cell {cell_idx} portal_ref {portal_ref} out of range for {portal_count} portals"
                    )));
                }
            }
        }
    }

    Ok(())
}

fn validate_visibility_portal_adjacency(
    cells: &[CellData],
    cell_portal_refs: &[u32],
    portals: &[PortalData],
) -> Result<(), LevelWorldValidationError> {
    let mut front_seen = vec![0u8; portals.len()];
    let mut back_seen = vec![0u8; portals.len()];

    for (portal_idx, portal) in portals.iter().enumerate() {
        if portal.front_cell == portal.back_cell {
            return Err(LevelWorldValidationError::new(format!(
                "portal {portal_idx} has identical endpoints {}",
                portal.front_cell
            )));
        }
        for (label, cell_idx) in [("front", portal.front_cell), ("back", portal.back_cell)] {
            let cell = cells.get(cell_idx).ok_or_else(|| {
                LevelWorldValidationError::new(format!(
                    "portal {portal_idx} {label} endpoint cell {cell_idx} out of range for {} cells",
                    cells.len()
                ))
            })?;
            if cell.is_solid {
                return Err(LevelWorldValidationError::new(format!(
                    "portal {portal_idx} {label} endpoint cell {cell_idx} is solid"
                )));
            }
        }
    }

    for (cell_idx, cell) in cells.iter().enumerate() {
        let start = cell.portal_ref_start as usize;
        let end = start + cell.portal_ref_count as usize;
        for &portal_ref in &cell_portal_refs[start..end] {
            let portal_idx = portal_ref as usize;
            let portal = &portals[portal_idx];
            if portal.front_cell == cell_idx {
                front_seen[portal_idx] += 1;
            } else if portal.back_cell == cell_idx {
                back_seen[portal_idx] += 1;
            } else {
                return Err(LevelWorldValidationError::new(format!(
                    "cell {cell_idx} adjacency lists portal {portal_idx}, but the portal endpoints are {} and {}",
                    portal.front_cell, portal.back_cell
                )));
            }
        }
    }

    for portal_idx in 0..portals.len() {
        if front_seen[portal_idx] != 1 || back_seen[portal_idx] != 1 {
            let portal = &portals[portal_idx];
            return Err(LevelWorldValidationError::new(format!(
                "portal {portal_idx} must appear exactly once in endpoint cells {} and {}; saw front {} and back {}",
                portal.front_cell, portal.back_cell, front_seen[portal_idx], back_seen[portal_idx]
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod visibility_only_validation_tests {
    use super::*;

    fn cell(portal_ref_start: u32, portal_ref_count: u32) -> CellData {
        CellData {
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ONE,
            face_start: 0,
            face_count: 0,
            portal_ref_start,
            portal_ref_count,
            is_solid: false,
            is_exterior: false,
            is_drawable: false,
        }
    }

    #[test]
    fn new_visibility_only_rejects_missing_portal_for_portal_ref() {
        let err = LevelWorld::new_visibility_only(
            vec![cell(0, 1)],
            vec![99],
            CellLocatorChild::Cell(0),
            vec![],
            vec![],
            true,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("portal_ref 99 out of range"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn new_visibility_only_rejects_missing_locator_node() {
        let err = LevelWorld::new_visibility_only(
            vec![cell(0, 0)],
            vec![],
            CellLocatorChild::Node(0),
            vec![],
            vec![],
            false,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("references node 0"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn cell_coupling_falls_back_conservatively_when_section_is_missing() {
        let world = LevelWorld::new_visibility_only(
            vec![cell(0, 0), cell(0, 0)],
            vec![],
            CellLocatorChild::Cell(0),
            vec![],
            vec![],
            false,
        )
        .unwrap();

        assert!(world.perceivable(0, 1));
        assert_eq!(
            world.coupling(0, 1),
            CouplingTuple {
                perceivable: true,
                distance: None,
                aperture: None,
            }
        );
    }

    #[test]
    fn cell_coupling_uses_loaded_components_and_canonical_pair_lookup() {
        let mut world = LevelWorld::new_visibility_only(
            vec![cell(0, 0), cell(0, 0), cell(0, 0)],
            vec![],
            CellLocatorChild::Cell(0),
            vec![],
            vec![],
            false,
        )
        .unwrap();
        world.cell_visibility = Some(CellVisibility::new(
            vec![0, 0, 1],
            vec![CoupledCellPair {
                cell_a: 0,
                cell_b: 1,
                distance: 42,
                aperture: 7,
            }],
        ));

        assert!(world.perceivable(0, 1));
        assert!(!world.perceivable(0, 2));
        assert_eq!(
            world.coupling(1, 0),
            CouplingTuple {
                perceivable: true,
                distance: Some(42),
                aperture: Some(7),
            }
        );
        assert_eq!(
            world.coupling(0, 2),
            CouplingTuple {
                perceivable: false,
                distance: None,
                aperture: None,
            }
        );

        let visibility = world.cell_visibility.as_ref().unwrap();
        assert_eq!(visibility.component_ids(), &[0, 0, 1]);
        assert_eq!(visibility.coupled_pairs().count(), 1);
    }
}

#[cfg(all(test, not(feature = "load-prl")))]
mod slim_tests {
    use super::*;

    fn cell(face_start: u32, face_count: u32) -> CellData {
        CellData {
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ONE,
            face_start,
            face_count,
            portal_ref_start: 0,
            portal_ref_count: 0,
            is_solid: false,
            is_exterior: false,
            is_drawable: face_count > 0,
        }
    }

    #[test]
    fn total_face_count_sums_cell_face_counts_without_full_prl_loader() {
        let world = LevelWorld {
            cells: vec![cell(0, 2), cell(2, 0), cell(2, 3)],
            cell_portal_refs: vec![],
            cell_locator_root: CellLocatorChild::Cell(0),
            cell_locator_nodes: vec![],
            portals: vec![],
            has_portals: false,
            cell_visibility: None,
        };

        assert_eq!(world.total_face_count(), 5);
    }
}

#[cfg(all(test, feature = "load-prl"))]
mod tests {
    use super::*;
    use crate::load_prl;
    use crate::prl_loader::{
        convert_alpha_lights, expected_affinity_dims, valid_probe_mask_for_affinity_cell,
        validate_animated_billboard_direct_scatter_delta_volumes, validate_cell_draw_index,
        validate_delta_sh, validate_direct_sh_delta, validate_entity_shadow_light_selection,
    };
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
    use postretro_level_format::fog_volumes::{
        FogVolumeRecord, FogVolumesSection, MAX_FOG_VOLUMES,
    };
    use postretro_level_format::geometry::NO_TEXTURE;
    use postretro_level_format::geometry::{FaceMeta as FormatFaceMeta, GeometrySection, Vertex};
    use postretro_level_format::navmesh::{NAVMESH_VERSION, NavRegion};
    use postretro_level_format::portals::{PortalRecord, PortalsSection};
    use postretro_render_data::geometry::BvhLeaf;
    use postretro_test_log_capture::LogCapture;

    use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection;
    use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
    use postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection;
    use postretro_level_format::delta_sh_volumes::{
        AFFINITY_FACTOR, DEFAULT_DELTA_PROBE_F16_STRIDE, DeltaShVolumesSection, PROBES_PER_CELL,
    };
    use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
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
            valid_probe_masks: vec![u64::MAX; cell_count],
            cell_levels: vec![0u8; cell_count],
            affinity_offsets: offsets,
            affinity_lights: vec![0],
            delta_subblocks: vec![0u16; PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE],
        }
    }

    fn direct_delta_section_for(
        affinity_dims: [u32; 3],
        affinity_lights: Vec<u32>,
    ) -> DirectShDeltaVolumesSection {
        let cell_count = (affinity_dims[0] * affinity_dims[1] * affinity_dims[2]) as usize;
        let mut offsets = vec![0u32; cell_count + 1];
        for o in offsets.iter_mut().skip(1) {
            *o = affinity_lights.len() as u32;
        }
        DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims,
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            // The shared fixture base volume marks every probe invalid, so
            // retained zero-length entries are the matching id-41 spelling.
            valid_probe_masks: vec![0; cell_count],
            cell_levels: vec![0u8; cell_count],
            affinity_offsets: offsets,
            delta_subblocks: Vec::new(),
            affinity_lights,
        }
    }

    fn animated_direct_delta_section_for(
        affinity_dims: [u32; 3],
    ) -> AnimatedDirectShDeltaVolumesSection {
        let cell_count = (affinity_dims[0] * affinity_dims[1] * affinity_dims[2]) as usize;
        let mut offsets = vec![0u32; cell_count + 1];
        for offset in offsets.iter_mut().skip(1) {
            *offset = 1;
        }
        AnimatedDirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims,
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            // The shared fixture base volume marks every probe invalid, so a
            // retained zero-length id-45 entry is its matching compact form.
            animation_descriptor_indices: vec![u32::MAX],
            valid_probe_masks: vec![0; cell_count],
            cell_levels: vec![0u8; cell_count],
            affinity_offsets: offsets,
            affinity_lights: vec![0],
            delta_subblocks: Vec::new(),
        }
    }

    fn billboard_direct_scatter_section_for(
        base: &OctahedralShVolumeSection,
    ) -> BillboardDirectScatterVolumeSection {
        let mut scatter_rgba = vec![0; base.total_probes() * 4];
        for (probe, metadata) in base.probes.iter().enumerate() {
            scatter_rgba[probe * 4 + 3] = if metadata.validity == 0 { 0 } else { 0x3c00 };
        }
        BillboardDirectScatterVolumeSection {
            grid_origin: base.grid_origin,
            cell_size: base.cell_size,
            grid_dimensions: base.grid_dimensions,
            scatter_rgba,
        }
    }

    fn animated_billboard_direct_scatter_delta_section_for(
        animated_direct: &AnimatedDirectShDeltaVolumesSection,
    ) -> AnimatedBillboardDirectScatterDeltaVolumesSection {
        AnimatedBillboardDirectScatterDeltaVolumesSection {
            animation_descriptor_indices: animated_direct.animation_descriptor_indices.clone(),
            affinity_factor: animated_direct.affinity_factor,
            affinity_dims: animated_direct.affinity_dims,
            affinity_offsets: animated_direct.affinity_offsets.clone(),
            affinity_lights: animated_direct.affinity_lights.clone(),
            delta_rgba: vec![0; animated_direct.affinity_lights.len() * 64 * 4],
        }
    }

    fn base_octahedral_section(grid_dimensions: [u32; 3]) -> OctahedralShVolumeSection {
        let probe_count =
            grid_dimensions[0] as usize * grid_dimensions[1] as usize * grid_dimensions[2] as usize;
        let layout = postretro_level_format::octahedral::irradiance_atlas_array_layout(
            grid_dimensions,
            DEFAULT_IRRADIANCE_TILE_DIMENSION,
            8192,
        )
        .unwrap();
        let atlas_dimensions = [layout.atlas_width, layout.atlas_height];
        OctahedralShVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions,
            probe_stride: postretro_level_format::sh_volume::OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions,
            layer_count: layout.layer_count,
            tiles_per_layer: layout.tiles_per_layer,
            atlas_tiles_per_row: layout.atlas_tiles_per_row,
            probes: vec![
                postretro_level_format::sh_volume::OctahedralShProbe::default();
                probe_count
            ],
            compact_atlas_dimensions: [0, 0],
            compact_atlas_tiles_per_row: 0,
            compact_atlas_tiles_per_layer: 0,
            compact_atlas_layer_count: 0,
            irradiance_format: postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H,
            compact_atlas: Vec::new(),
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    fn base_octahedral_section_for_direct(
        direct: &DirectShVolumeSection,
    ) -> OctahedralShVolumeSection {
        let probe_count = direct.total_probes();
        OctahedralShVolumeSection {
            grid_origin: direct.grid_origin,
            cell_size: direct.cell_size,
            grid_dimensions: direct.grid_dimensions,
            probe_stride: postretro_level_format::sh_volume::OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: direct.tile_dimension,
            tile_border: direct.tile_border,
            atlas_dimensions: direct.atlas_dimensions,
            layer_count: direct.layer_count,
            tiles_per_layer: direct.tiles_per_layer,
            atlas_tiles_per_row: direct.atlas_tiles_per_row,
            probes: vec![
                postretro_level_format::sh_volume::OctahedralShProbe::default();
                probe_count
            ],
            compact_atlas_dimensions: [0, 0],
            compact_atlas_tiles_per_row: 0,
            compact_atlas_tiles_per_layer: 0,
            compact_atlas_layer_count: 0,
            irradiance_format: postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H,
            compact_atlas: Vec::new(),
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
    fn validate_animated_billboard_scatter_requires_id45_factor_and_dimensions() {
        let direct = animated_direct_delta_section_for([2, 1, 1]);
        let mut scatter = animated_billboard_direct_scatter_delta_section_for(&direct);
        assert!(
            validate_animated_billboard_direct_scatter_delta_volumes(&scatter, &direct).is_ok()
        );

        scatter.affinity_factor = direct.affinity_factor + 1;
        assert!(
            validate_animated_billboard_direct_scatter_delta_volumes(&scatter, &direct).is_err()
        );
        scatter.affinity_factor = direct.affinity_factor;
        scatter.affinity_dims = [1, 2, 1];
        assert!(
            validate_animated_billboard_direct_scatter_delta_volumes(&scatter, &direct).is_err()
        );
    }

    #[test]
    fn validate_delta_sh_accepts_matching_dims() {
        let base_dims = [8u32, 5, 1];
        let mut section = delta_section_for(expected_affinity_dims(base_dims, AFFINITY_FACTOR));
        let mut base = base_octahedral_section(base_dims);
        for probe in &mut base.probes {
            probe.validity = 1;
        }
        section.valid_probe_masks = (0..section.affinity_cell_count())
            .map(|cell| valid_probe_mask_for_affinity_cell(&base, section.affinity_dims, cell))
            .collect();
        assert!(validate_delta_sh(&section, Some(&base)).is_ok());
    }

    #[test]
    fn validate_delta_sh_rejects_descriptor_that_disagrees_with_id34_validity() {
        let base_dims = [4u32, 4, 4];
        let mut section = delta_section_for(expected_affinity_dims(base_dims, AFFINITY_FACTOR));
        // The descriptor and payload agree with one another (one compact tile),
        // but not with the all-valid id-34 metadata. This must fail before any
        // renderer buffer sizing can derive a tail from the wrong mask.
        section.valid_probe_masks = vec![1];
        section.delta_subblocks = vec![0; DEFAULT_DELTA_PROBE_F16_STRIDE];
        let mut base = base_octahedral_section(base_dims);
        for probe in &mut base.probes {
            probe.validity = 1;
        }

        let error = validate_delta_sh(&section, Some(&base)).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("DeltaShVolumes"));
        assert!(message.contains("id 34"));
        assert!(message.contains("recompile"));
    }

    #[test]
    fn validate_direct_sh_delta_accepts_selection_indices() {
        let direct = minimal_direct_sh_volume_section();
        let section = direct_delta_section_for(
            expected_affinity_dims(direct.grid_dimensions, AFFINITY_FACTOR),
            vec![0, 1],
        );
        let base = base_octahedral_section_for_direct(&direct);

        assert!(validate_direct_sh_delta(&section, &direct, &base, 2).is_ok());
    }

    #[test]
    fn validate_direct_sh_delta_rejects_alpha_light_indices() {
        let direct = minimal_direct_sh_volume_section();
        let section = direct_delta_section_for(
            expected_affinity_dims(direct.grid_dimensions, AFFINITY_FACTOR),
            vec![0, 2],
        );
        let base = base_octahedral_section_for_direct(&direct);

        let err = validate_direct_sh_delta(&section, &direct, &base, 2).unwrap_err();

        assert!(
            err.to_string().contains("selection index 2 out of range"),
            "expected selection-index validation error, got {err:?}",
        );
    }

    #[test]
    fn validate_direct_sh_delta_rejects_missing_selected_light_coverage() {
        let direct = minimal_direct_sh_volume_section();
        let section = direct_delta_section_for(
            expected_affinity_dims(direct.grid_dimensions, AFFINITY_FACTOR),
            vec![0],
        );
        let base = base_octahedral_section_for_direct(&direct);

        let err = validate_direct_sh_delta(&section, &direct, &base, 2).unwrap_err();

        assert!(
            err.to_string()
                .contains("missing usable delta entry for selected light index 1"),
            "expected missing selected-light coverage error, got {err:?}",
        );
    }

    #[test]
    fn validate_entity_shadow_lights_accepts_static_lightmap_point_or_spot() {
        let lights = convert_alpha_lights(sample_alpha_lights());

        assert!(validate_entity_shadow_light_selection(&[0], &lights).is_ok());
    }

    #[test]
    fn validate_entity_shadow_lights_rejects_dynamic_directional_and_sdf_lights() {
        let mut alpha = sample_alpha_lights();
        alpha.lights[0].shadow_type = AlphaShadowType::Sdf;
        let lights = convert_alpha_lights(alpha);

        let sdf = validate_entity_shadow_light_selection(&[0], &lights).unwrap_err();
        assert!(
            sdf.to_string().contains("static_light_map"),
            "expected SDF contributor rejection, got {sdf:?}",
        );

        let dynamic = validate_entity_shadow_light_selection(&[1], &lights).unwrap_err();
        assert!(
            dynamic.to_string().contains("dynamic-tier"),
            "expected dynamic-tier rejection, got {dynamic:?}",
        );

        let directional = validate_entity_shadow_light_selection(&[2], &lights).unwrap_err();
        assert!(
            directional.to_string().contains("directional"),
            "expected directional rejection, got {directional:?}",
        );
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
            cell_visibility: None,
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
            direct_sh_delta_volumes: None,
            animated_direct_sh_delta_volumes: None,
            billboard_direct_scatter_volume: None,
            animated_billboard_direct_scatter_delta_volumes: None,
            entity_shadow_lights: Vec::new(),
            shadowmask_atlas: None,
            data_script: None,
            map_entities: Vec::new(),
            kinematic_geometry: KinematicGeometry::default(),
            trigger_volumes: Vec::new(),
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
            cell_visibility: None,
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
            direct_sh_delta_volumes: None,
            animated_direct_sh_delta_volumes: None,
            billboard_direct_scatter_volume: None,
            animated_billboard_direct_scatter_delta_volumes: None,
            entity_shadow_lights: Vec::new(),
            shadowmask_atlas: None,
            data_script: None,
            map_entities: Vec::new(),
            kinematic_geometry: KinematicGeometry::default(),
            trigger_volumes: Vec::new(),
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
            cell_visibility: None,
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
            direct_sh_delta_volumes: None,
            animated_direct_sh_delta_volumes: None,
            billboard_direct_scatter_volume: None,
            animated_billboard_direct_scatter_delta_volumes: None,
            entity_shadow_lights: Vec::new(),
            shadowmask_atlas: None,
            data_script: None,
            map_entities: Vec::new(),
            kinematic_geometry: KinematicGeometry::default(),
            trigger_volumes: Vec::new(),
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
            0,
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
            if let Some(direct) = sections
                .iter()
                .find(|section| section.section_id == SectionId::DirectShVolume as u32)
                .and_then(|section| DirectShVolumeSection::from_bytes(&section.data).ok())
            {
                sections.push(octahedral_sh_volume_blob(
                    base_octahedral_section_for_direct(&direct),
                ));
            } else {
                sections.push(default_octahedral_sh_volume_blob());
            }
        }
        write_prl_fixture_raw(sections, name)
    }

    fn geometry_blob(section: GeometrySection) -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: section.to_bytes(),
        }
    }

    fn bvh_blob(section: BvhSection) -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::Bvh as u32,
            version: 1,
            data: section.to_bytes(),
        }
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
        cells_blob(section)
    }

    fn cells_blob(section: CellsSection) -> prl_format::SectionBlob {
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
            layer_count: 0,
            tiles_per_layer: 0,
            atlas_tiles_per_row: 0,
            probes: Vec::new(),
            compact_atlas_dimensions: [0, 0],
            compact_atlas_tiles_per_row: 0,
            compact_atlas_tiles_per_layer: 0,
            compact_atlas_layer_count: 0,
            irradiance_format: postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H,
            compact_atlas: Vec::new(),
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        };
        octahedral_sh_volume_blob(section)
    }

    fn octahedral_sh_volume_blob(section: OctahedralShVolumeSection) -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::OctahedralShVolume as u32,
            version: 1,
            data: section.to_bytes(),
        }
    }

    fn minimal_direct_sh_volume_section() -> DirectShVolumeSection {
        use postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;
        use postretro_level_format::octahedral::irradiance_atlas_array_layout;

        let grid = [1, 1, 1];
        let tile_dimension = DEFAULT_IRRADIANCE_TILE_DIMENSION;
        let layout = irradiance_atlas_array_layout(grid, tile_dimension, 8192).unwrap();
        let atlas_dimensions = [layout.atlas_width, layout.atlas_height];
        let padded_w = atlas_dimensions[0].div_ceil(4) * 4;
        let padded_h = atlas_dimensions[1].div_ceil(4) * 4;
        let atlas_len = layout.layer_count as usize * (padded_w / 4 * padded_h / 4) as usize * 16;

        DirectShVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: grid,
            tile_dimension,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions,
            layer_count: layout.layer_count,
            tiles_per_layer: layout.tiles_per_layer,
            atlas_tiles_per_row: layout.atlas_tiles_per_row,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
            atlas: vec![0; atlas_len],
        }
    }

    fn direct_sh_volume_blob(section: DirectShVolumeSection) -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::DirectShVolume as u32,
            version: 1,
            data: section.to_bytes(),
        }
    }

    fn entity_shadow_lights_blob(indices: Vec<u32>) -> prl_format::SectionBlob {
        let section = postretro_level_format::entity_shadow_lights::EntityShadowLightsSection {
            light_indices: indices,
        };
        prl_format::SectionBlob {
            section_id: SectionId::EntityShadowLights as u32,
            version: 1,
            data: section.to_bytes(),
        }
    }

    fn lightmap_blob(width: u32, height: u32, layer_count: u32) -> prl_format::SectionBlob {
        let texels = (width * height * layer_count) as usize;
        let section = postretro_level_format::lightmap::LightmapSection {
            layer_count,
            irr_width: width,
            irr_height: height,
            irr_texel_density: 0.04,
            irradiance: vec![0; texels * postretro_level_format::lightmap::IRRADIANCE_TEXEL_BYTES],
            irradiance_format: postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F,
            dir_width: width,
            dir_height: height,
            dir_texel_density: 0.04,
            direction: vec![255; texels * postretro_level_format::lightmap::DIRECTION_TEXEL_BYTES],
            mode: postretro_level_format::lightmap::LightmapMode::Shadowed,
        };
        prl_format::SectionBlob {
            section_id: SectionId::Lightmap as u32,
            version: 1,
            data: section.to_bytes(),
        }
    }

    fn shadowmask_blob(
        section: postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection,
    ) -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::ShadowmaskAtlas as u32,
            version: 1,
            data: section.to_bytes(),
        }
    }

    fn direct_sh_delta_blob(section: DirectShDeltaVolumesSection) -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::DirectShDeltaVolumes as u32,
            version: 1,
            data: section.to_bytes(),
        }
    }

    fn animated_direct_sh_delta_blob(
        section: AnimatedDirectShDeltaVolumesSection,
    ) -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::AnimatedDirectShDeltaVolumes as u32,
            version: 1,
            data: section.to_bytes(),
        }
    }

    fn billboard_direct_scatter_blob(
        section: BillboardDirectScatterVolumeSection,
    ) -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::BillboardDirectScatterVolume as u32,
            version: 1,
            data: section.to_bytes(),
        }
    }

    fn animated_billboard_direct_scatter_delta_blob(
        section: AnimatedBillboardDirectScatterDeltaVolumesSection,
    ) -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32,
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

    // Regression: version-1 navmesh portals used the wrong handedness at runtime.
    #[test]
    fn load_prl_stale_navmesh_version_warns_and_disables_navigation() {
        const STALE_NAVMESH_VERSION: u16 = 1;

        let stale_navmesh = NavMeshSection {
            version: STALE_NAVMESH_VERSION,
            origin: [0.0, 0.0, 0.0],
            cell_size: 0.25,
            dim_x: 1,
            dim_z: 1,
            agent_radius: 0.4,
            agent_height: 1.8,
            step_height: 0.5,
            max_slope_deg: 45.0,
            regions: vec![NavRegion {
                x0: 0,
                z0: 0,
                x1: 1,
                z1: 1,
                floor_y_min: 0.0,
                floor_y_max: 0.0,
            }],
            portals: Vec::new(),
        };
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
            prl_format::SectionBlob {
                section_id: SectionId::NavMesh as u32,
                version: 1,
                data: stale_navmesh.to_bytes(),
            },
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_stale_navmesh_version.prl");
        let capture = LogCapture::start();

        let world = load_prl(tmp.to_str().unwrap())
            .expect("stale navmesh must degrade without failing the level load");

        assert!(
            world.navmesh.is_none(),
            "stale navmesh must not reach runtime navigation"
        );
        capture.assert_logged_once(
            log::Level::Warn,
            &format!(
                "navmesh section version {STALE_NAVMESH_VERSION}, expected {NAVMESH_VERSION} — recompile the map"
            ),
        );
        std::fs::remove_file(&tmp).ok();
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
    fn load_prl_rejects_face_meta_cell_id_out_of_range() {
        let mut geometry = sample_geometry();
        geometry.faces[1].leaf_index = 99;
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
        let tmp = write_prl_fixture(sections, "postretro_test_face_meta_cell_oob.prl");
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
        assert!(err.to_string().contains("leaf_index"), "got {err}");
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_cell_face_range_with_wrong_face_owner() {
        let mut geometry = sample_geometry();
        geometry.faces[1].leaf_index = 0;
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
        let tmp = write_prl_fixture(sections, "postretro_test_cell_face_wrong_owner.prl");
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
        assert!(
            err.to_string().contains("face range includes face"),
            "got {err}"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_face_missing_from_owning_cell_range() {
        let cells = CellsSection {
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
                    flags: 0,
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 0,
                    portal_ref_count: 0,
                },
            ],
            portal_refs: Vec::new(),
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
            cells_blob(cells),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_face_missing_from_owner.prl");
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
        assert!(err.to_string().contains("owning cell range"), "got {err}");
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_duplicate_cell_face_ownership() {
        let mut geometry = sample_geometry();
        geometry.faces[1].leaf_index = 0;
        let cells = CellsSection {
            cells: vec![
                CellRecord {
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 0,
                    face_count: 2,
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
            cells_blob(cells),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        let tmp = write_prl_fixture(sections, "postretro_test_duplicate_face_owner.prl");
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
        assert!(err.to_string().contains("claimed by both"), "got {err}");
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
    fn load_prl_rejects_unsorted_bvh_material_buckets() {
        let mut bvh = sample_bvh_section();
        bvh.leaves[0].material_bucket_id = 1;
        bvh.leaves[1].material_bucket_id = 0;
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
        let tmp = write_prl_fixture(sections, "postretro_test_bvh_unsorted_buckets.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PrlLoadError::SectionValidation { section: "Bvh", .. }),
            "got {err:?}"
        );
        assert!(err.to_string().contains("material_bucket_id"), "got {err}");
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
    fn load_prl_rejects_missing_cell_locator_before_missing_octahedral_sh_volume() {
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
        ];
        let tmp = write_prl_fixture_raw(
            sections,
            "postretro_test_missing_cell_locator_before_sh.prl",
        );
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
    fn load_prl_rejects_malformed_cell_locator_before_missing_octahedral_sh_volume() {
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
        ];
        let tmp = write_prl_fixture_raw(
            sections,
            "postretro_test_malformed_cell_locator_before_sh.prl",
        );
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
    fn load_prl_rejects_bvh_leaf_without_indices_for_drawable_cell() {
        let geometry = GeometrySection {
            vertices: vec![sample_vertex(0.0), sample_vertex(1.0), sample_vertex(1.5)],
            indices: vec![0, 1, 2],
            faces: vec![FormatFaceMeta {
                leaf_index: 0,
                texture_index: NO_TEXTURE,
            }],
        };
        let bvh = BvhSection {
            nodes: vec![FormatBvhNode {
                aabb_min: [0.0, 0.0, 0.0],
                skip_index: 1,
                aabb_max: [2.0, 2.0, 2.0],
                left_child_or_leaf_index: 0,
                flags: BVH_NODE_FLAG_LEAF,
                _padding: 0,
            }],
            leaves: vec![FormatBvhLeaf {
                aabb_min: [0.0, 0.0, 0.0],
                material_bucket_id: 0,
                aabb_max: [2.0, 2.0, 2.0],
                index_offset: 0,
                index_count: 0,
                cell_id: 0,
                chunk_range_start: 0,
                chunk_range_count: 0,
            }],
            root_node_index: 0,
        };
        let cells = CellsSection {
            cells: vec![CellRecord {
                bounds_min: [0.0, 0.0, 0.0],
                bounds_max: [2.0, 2.0, 2.0],
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
        let cell_draw_index = CellDrawIndexSection {
            cell_count: 1,
            span_count: 0,
            cell_span_offset: vec![0, 0],
            spans: Vec::new(),
        };
        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geometry.to_bytes(),
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
            default_octahedral_sh_volume_blob(),
            cell_draw_index_blob(&cell_draw_index),
        ];

        // Regression: a drawable cell with real Geometry but only zero-index BVH
        // leaves used to load, leaving the renderer with no drawable work.
        let tmp = write_prl_fixture_raw(
            sections,
            "postretro_test_zero_index_bvh_leaf_for_drawable_cell.prl",
        );
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
    fn load_prl_light_influence_short_section_loads_available_records() {
        use postretro_level_format::light_influence::{InfluenceRecord, LightInfluenceSection};

        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        let alpha_lights = sample_alpha_lights(); // 3 lights

        // Only 2 influence records — the missing tail light degrades to
        // uncullable downstream instead of rejecting the map.
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
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_influence_short.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("short LightInfluence should load");
        assert_eq!(world.lights.len(), 3, "lights should still parse");
        assert_eq!(
            world.light_influences.len(),
            2,
            "only available records are loaded; missing tail entries stay absent"
        );
        assert!(
            (world.light_influences[0].center - glam::Vec3::new(1.0, 2.0, 3.0)).length() <= 1.0e-5
        );
        assert!((world.light_influences[0].radius - 50.0).abs() <= 1.0e-5);
        assert!(
            (world.light_influences[1].center - glam::Vec3::new(-4.0, 5.5, 6.0)).length() <= 1.0e-5
        );
        assert!((world.light_influences[1].radius - 25.0).abs() <= 1.0e-5);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_light_influence_extra_records_are_error() {
        use postretro_level_format::light_influence::{InfluenceRecord, LightInfluenceSection};

        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        let alpha_lights = sample_alpha_lights(); // 3 lights

        // Four influence records for three lights cannot be paired safely.
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
                InfluenceRecord {
                    center: [0.0, 0.0, 0.0],
                    radius: 10.0,
                },
                InfluenceRecord {
                    center: [8.0, 9.0, 10.0],
                    radius: 5.0,
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

        let tmp = write_prl_fixture(sections, "postretro_test_influence_extra.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("exceeds AlphaLights count"),
            "expected extra-record count error, got: {msg}"
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

    fn kinematic_section_blob(
        section: postretro_level_format::kinematic_geometry::KinematicGeometrySection,
    ) -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::KinematicGeometry as u32,
            version: 1,
            data: section.to_bytes(),
        }
    }

    fn sample_kinematic_section()
    -> postretro_level_format::kinematic_geometry::KinematicGeometrySection {
        use postretro_level_format::geometry::FaceMeta as PrlFaceMeta;
        use postretro_level_format::kinematic_geometry::{
            KINEMATIC_GEOMETRY_VERSION, KinematicGeometrySection, KinematicMoverRecord,
            KinematicWaypointRecord,
        };

        KinematicGeometrySection {
            version: KINEMATIC_GEOMETRY_VERSION,
            movers: vec![KinematicMoverRecord {
                mover_id: 7,
                name: "lift".to_string(),
                tags: vec!["platform".to_string()],
                origin: [1.0, 2.0, 3.0],
                path: "a".to_string(),
                speed: 2.0,
                wait_ms: 125.0,
                move_mode: 1,
                start_on_spawn: true,
                vertices: vec![sample_vertex(0.0), sample_vertex(1.0), sample_vertex(2.0)],
                indices: vec![0, 1, 2],
                face_meta: vec![PrlFaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                }],
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
                    name: "a".to_string(),
                    next: "b".to_string(),
                    origin: [1.0, 2.0, 3.0],
                },
                KinematicWaypointRecord {
                    name: "b".to_string(),
                    next: String::new(),
                    origin: [3.0, 2.0, 3.0],
                },
            ],
        }
    }

    #[test]
    fn loaded_kinematic_mover_preserves_rotation_authoring_values() {
        let mut record = sample_kinematic_section().movers.remove(0);
        record.spin_axis = [0.0, 3.0, 4.0];
        record.spin_speed_deg_s = -90.0;
        record.spin_accel_deg_s2 = 180.0;
        record.carry_yaw = true;
        record.block_policy = "crush".to_string();
        record.crush_damage = 25.0;
        record.crush_interval_ms = 150.0;
        record.auto_close_ms = Some(900.0);
        record.open_event = Some("opened".to_string());
        record.close_event = Some("closed".to_string());
        record.blocked_event = Some("blocked".to_string());
        record.crush_event = Some("crushed".to_string());

        let mover = LoadedKinematicMover::from(record);

        assert!((mover.spin_axis - Vec3::new(0.0, 3.0, 4.0)).length() <= 1.0e-6);
        assert!((mover.spin_speed_deg_s + 90.0).abs() <= 1.0e-6);
        assert!((mover.spin_accel_deg_s2 - 180.0).abs() <= 1.0e-6);
        assert!(mover.carry_yaw);
        assert_eq!(mover.block_policy, "crush");
        assert!((mover.crush_damage - 25.0).abs() <= 1.0e-6);
        assert!((mover.crush_interval_ms - 150.0).abs() <= 1.0e-6);
        assert_eq!(mover.auto_close_ms, Some(900.0));
        assert_eq!(mover.open_event.as_deref(), Some("opened"));
        assert_eq!(mover.close_event.as_deref(), Some("closed"));
        assert_eq!(mover.blocked_event.as_deref(), Some("blocked"));
        assert_eq!(mover.crush_event.as_deref(), Some("crushed"));
    }

    fn minimal_sections_with_kinematic(
        section: Option<postretro_level_format::kinematic_geometry::KinematicGeometrySection>,
    ) -> Vec<prl_format::SectionBlob> {
        let mut sections = vec![
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
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];
        if let Some(section) = section {
            sections.push(kinematic_section_blob(section));
        }
        sections
    }

    #[test]
    fn load_prl_absent_kinematic_geometry_section_yields_no_movers() {
        let tmp = write_prl_fixture(
            minimal_sections_with_kinematic(None),
            "postretro_test_kinematic_absent.prl",
        );
        let world = load_prl(tmp.to_str().unwrap()).expect("should load without movers");

        assert!(world.kinematic_geometry.movers.is_empty());
        assert!(world.kinematic_geometry.waypoints.is_empty());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_empty_kinematic_geometry_section_yields_no_movers() {
        let tmp = write_prl_fixture(
            minimal_sections_with_kinematic(Some(Default::default())),
            "postretro_test_kinematic_empty.prl",
        );
        let world = load_prl(tmp.to_str().unwrap()).expect("empty mover section should load");

        assert!(world.kinematic_geometry.movers.is_empty());
        assert!(world.kinematic_geometry.waypoints.is_empty());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_reads_kinematic_geometry_mover_and_waypoints() {
        let tmp = write_prl_fixture(
            minimal_sections_with_kinematic(Some(sample_kinematic_section())),
            "postretro_test_kinematic_one_mover.prl",
        );
        let world = load_prl(tmp.to_str().unwrap()).expect("kinematic mover should load");

        assert_eq!(world.kinematic_geometry.movers.len(), 1);
        assert_eq!(world.kinematic_geometry.waypoints.len(), 2);
        let mover = &world.kinematic_geometry.movers[0];
        assert_eq!(mover.mover_id, 7);
        assert_eq!(mover.name, "lift");
        assert_eq!(mover.tags, vec!["platform".to_string()]);
        assert_eq!(mover.origin, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(mover.path, "a");
        assert_eq!(mover.vertices.len(), 3);
        assert_eq!(mover.indices, vec![0, 1, 2]);
        assert_eq!(world.kinematic_geometry.waypoints[0].next, "b");
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_v1_kinematic_mover_defaults_rotation_authoring_values() {
        let mut legacy_section = sample_kinematic_section();
        legacy_section.version = 1;
        legacy_section.movers[0].spin_axis = [0.0, 3.0, 4.0];
        legacy_section.movers[0].spin_speed_deg_s = 90.0;
        legacy_section.movers[0].spin_accel_deg_s2 = 180.0;
        legacy_section.movers[0].carry_yaw = true;
        let tmp = write_prl_fixture(
            minimal_sections_with_kinematic(Some(legacy_section)),
            "postretro_test_kinematic_v1_rotation_defaults.prl",
        );

        let world = load_prl(tmp.to_str().unwrap()).expect("v1 kinematic mover should load");
        let mover = &world.kinematic_geometry.movers[0];

        assert!(mover.spin_axis.length() <= 1.0e-6);
        assert!(mover.spin_speed_deg_s.abs() <= 1.0e-6);
        assert!(mover.spin_accel_deg_s2.abs() <= 1.0e-6);
        assert!(!mover.carry_yaw);
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_invalid_kinematic_waypoint_chains() {
        let mut unknown = sample_kinematic_section();
        unknown.waypoints[0].next = "missing".to_string();
        let tmp = write_prl_fixture(
            minimal_sections_with_kinematic(Some(unknown)),
            "postretro_test_kinematic_unknown_next.prl",
        );
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err().to_string();
        assert!(err.contains("KinematicGeometry"));
        assert!(err.contains("unknown waypoint"));
        std::fs::remove_file(&tmp).ok();

        let mut cycle = sample_kinematic_section();
        cycle.waypoints[1].next = "a".to_string();
        let tmp = write_prl_fixture(
            minimal_sections_with_kinematic(Some(cycle)),
            "postretro_test_kinematic_cycle.prl",
        );
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err().to_string();
        assert!(err.contains("cycles"));
        std::fs::remove_file(&tmp).ok();

        let mut short = sample_kinematic_section();
        short.waypoints.truncate(1);
        short.waypoints[0].next.clear();
        let tmp = write_prl_fixture(
            minimal_sections_with_kinematic(Some(short)),
            "postretro_test_kinematic_short.prl",
        );
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err().to_string();
        assert!(err.contains("at least 2 required"));
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
        // Two cells but only one mask: validate against Cells.cell_count,
        // not removed runtime BSP sections.
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
        // Two cells but three masks: validate against Cells.cell_count,
        // not removed runtime BSP sections.
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

    #[test]
    fn load_prl_rejects_fog_volume_count_over_renderer_slot_cap() {
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
            default_texture_cache_keys_blob(),
            fog_volumes_blob_with_count(MAX_FOG_VOLUMES + 1),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_too_many_fog_volumes.prl");
        let err = load_prl(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "FogVolumes",
                    ..
                }
            ),
            "got {err:?}"
        );
        assert!(err.to_string().contains("MAX_FOG_VOLUMES"), "got {err}");
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
        // Legacy PRLs without a lightmap-mode marker decode as `Shadowed`.
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

    /// A map without static direct SH/static lights loads and surfaces
    /// `direct_sh_volume = None` (dynamic objects fall back to indirect-only;
    /// the renderer binds the 4×4 BC6H dummy).
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
        let world = load_prl(tmp.to_str().unwrap())
            .expect("PRL without static direct SH/static lights must load");
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
            irradiance_atlas_array_layout,
        };

        let grid = [3u32, 2, 4];
        let tile_dimension = DEFAULT_IRRADIANCE_TILE_DIMENSION;
        let layout = irradiance_atlas_array_layout(grid, tile_dimension, 8192).unwrap();
        let atlas_dimensions = [layout.atlas_width, layout.atlas_height];
        // BC6H blob length for the 4-aligned padded atlas (the emitter rounds
        // each axis up to a multiple of 4 before encoding).
        let padded_w = atlas_dimensions[0].div_ceil(4) * 4;
        let padded_h = atlas_dimensions[1].div_ceil(4) * 4;
        let block_count =
            layout.layer_count as usize * (padded_w / 4) as usize * (padded_h / 4) as usize;
        let section = DirectShVolumeSection {
            grid_origin: [1.0, 2.0, 3.0],
            cell_size: [0.5, 0.5, 0.5],
            grid_dimensions: grid,
            tile_dimension,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions,
            layer_count: layout.layer_count,
            tiles_per_layer: layout.tiles_per_layer,
            atlas_tiles_per_row: layout.atlas_tiles_per_row,
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
    fn load_prl_clears_direct_sh_volume_when_layout_mismatches_base_sh_volume() {
        // Regression: DirectShVolume reuses OctahedralShVolume layout; accepting
        // mismatched probe/tile/array fields lets shaders derive bad layers.
        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        let direct_sh = minimal_direct_sh_volume_section();
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
            octahedral_sh_volume_blob(base_octahedral_section([2, 1, 1])),
            direct_sh_volume_blob(direct_sh),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_direct_sh_volume_layout_mismatch.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("mismatched DirectShVolume layout must degrade without failing load");

        assert!(
            world.direct_sh_volume.is_none(),
            "mismatched DirectShVolume must be cleared before reaching LevelWorld"
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_parses_animated_direct_sh_deltas_without_static_direct_sh() {
        let base_dims = [1, 1, 1];
        let section =
            animated_direct_delta_section_for(expected_affinity_dims(base_dims, AFFINITY_FACTOR));
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            octahedral_sh_volume_blob(base_octahedral_section(base_dims)),
            animated_direct_sh_delta_blob(section.clone()),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_animated_direct_sh_delta_round_trip.prl",
        );
        let world =
            load_prl(tmp.to_str().unwrap()).expect("valid AnimatedDirectShDeltaVolumes must load");

        assert_eq!(
            world.animated_direct_sh_delta_volumes,
            Some(section),
            "the animated direct delta must load independently of DirectShVolume"
        );
        assert!(world.direct_sh_volume.is_none());

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_rejects_animated_direct_delta_descriptor_that_disagrees_with_id34() {
        let base_dims = [1, 1, 1];
        let mut section =
            animated_direct_delta_section_for(expected_affinity_dims(base_dims, AFFINITY_FACTOR));
        section.valid_probe_masks[0] = 1;
        section.delta_subblocks = vec![0; DEFAULT_DELTA_PROBE_F16_STRIDE];
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            octahedral_sh_volume_blob(base_octahedral_section(base_dims)),
            animated_direct_sh_delta_blob(section),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_animated_direct_sh_delta_validity_mismatch.prl",
        );
        let error = load_prl(tmp.to_str().unwrap())
            .expect_err("an id-45/id-34 descriptor mismatch must reject the entire load");

        assert!(
            matches!(
                error,
                PrlLoadError::AnimatedDirectShDeltaValidityMismatch {
                    cell: 0,
                    found: 1,
                    expected: 0,
                }
            ),
            "expected the named id-45 validity mismatch error, got {error:?}"
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_treats_empty_animated_direct_sh_delta_csr_as_absent() {
        let base_dims = [1, 1, 1];
        let mut section =
            animated_direct_delta_section_for(expected_affinity_dims(base_dims, AFFINITY_FACTOR));
        section.affinity_offsets.fill(0);
        section.affinity_lights.clear();
        section.delta_subblocks.clear();
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            octahedral_sh_volume_blob(base_octahedral_section(base_dims)),
            animated_direct_sh_delta_blob(section),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_empty_animated_direct_sh_delta.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("valid empty AnimatedDirectShDeltaVolumes must load");

        assert!(
            world.animated_direct_sh_delta_volumes.is_none(),
            "an empty CSR cannot contribute and must not select renderer Case 2"
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_soft_drops_malformed_animated_direct_sh_deltas() {
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            prl_format::SectionBlob {
                section_id: SectionId::AnimatedDirectShDeltaVolumes as u32,
                version: 1,
                data: vec![0],
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_animated_direct_sh_delta_malformed.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("malformed AnimatedDirectShDeltaVolumes must not fail level load");

        assert!(world.animated_direct_sh_delta_volumes.is_none());

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_soft_drops_partial_animated_direct_sh_deltas() {
        let base_dims = [1, 1, 1];
        let mut section =
            animated_direct_delta_section_for(expected_affinity_dims(base_dims, AFFINITY_FACTOR));
        section.valid_probe_masks[0] = 1;
        section.delta_subblocks = vec![0; DEFAULT_DELTA_PROBE_F16_STRIDE];
        let mut partial_data = section.to_bytes();
        partial_data.truncate(partial_data.len() - 2);
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            octahedral_sh_volume_blob(base_octahedral_section(base_dims)),
            prl_format::SectionBlob {
                section_id: SectionId::AnimatedDirectShDeltaVolumes as u32,
                version: 1,
                data: partial_data,
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_animated_direct_sh_delta_partial.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("partial AnimatedDirectShDeltaVolumes must not fail level load");

        assert!(world.animated_direct_sh_delta_volumes.is_none());

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_keeps_static_billboard_direct_scatter_without_animated_sections() {
        let base = base_octahedral_section([2, 1, 1]);
        let scatter = billboard_direct_scatter_section_for(&base);
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            octahedral_sh_volume_blob(base),
            billboard_direct_scatter_blob(scatter.clone()),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_static_billboard_direct_scatter.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("static-only billboard scatter must load without animated companions");

        assert_eq!(world.billboard_direct_scatter_volume, Some(scatter));
        assert!(
            world
                .animated_billboard_direct_scatter_delta_volumes
                .is_none()
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_exposes_billboard_direct_scatter_only_with_matching_animated_pair() {
        let base = base_octahedral_section([1, 1, 1]);
        let animated_direct = animated_direct_delta_section_for(expected_affinity_dims(
            base.grid_dimensions,
            AFFINITY_FACTOR,
        ));
        let animated_scatter =
            animated_billboard_direct_scatter_delta_section_for(&animated_direct);
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            octahedral_sh_volume_blob(base.clone()),
            billboard_direct_scatter_blob(billboard_direct_scatter_section_for(&base)),
            animated_direct_sh_delta_blob(animated_direct),
            animated_billboard_direct_scatter_delta_blob(animated_scatter.clone()),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_animated_billboard_direct_scatter_pair.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("matching billboard scatter companions must load");

        assert!(world.billboard_direct_scatter_volume.is_some());
        assert_eq!(
            world.animated_billboard_direct_scatter_delta_volumes,
            Some(animated_scatter)
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_retains_scatter_for_valid_empty_animated_pair() {
        let base = base_octahedral_section([1, 1, 1]);
        let affinity_dims = expected_affinity_dims(base.grid_dimensions, AFFINITY_FACTOR);
        let cell_count = affinity_dims.iter().product::<u32>() as usize;
        let animated_direct = AnimatedDirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims,
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: Vec::new(),
            valid_probe_masks: vec![0; cell_count],
            cell_levels: vec![0; cell_count],
            affinity_offsets: vec![0; cell_count + 1],
            affinity_lights: Vec::new(),
            delta_subblocks: Vec::new(),
        };
        let animated_scatter =
            animated_billboard_direct_scatter_delta_section_for(&animated_direct);
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            octahedral_sh_volume_blob(base.clone()),
            billboard_direct_scatter_blob(billboard_direct_scatter_section_for(&base)),
            animated_direct_sh_delta_blob(animated_direct),
            animated_billboard_direct_scatter_delta_blob(animated_scatter.clone()),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_empty_animated_billboard_direct_scatter_pair.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("a valid empty id-45/id-48 pair must keep billboard scatter");

        assert!(world.billboard_direct_scatter_volume.is_some());
        assert_eq!(
            world.animated_billboard_direct_scatter_delta_volumes,
            Some(animated_scatter),
            "P7 requires an animated compose path that can seed the base with an empty sum"
        );
        assert!(
            world.animated_direct_sh_delta_volumes.is_none(),
            "an empty id-45 section carries pair validity but no direct-SH runtime work"
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_soft_drops_billboard_scatter_with_invalid_version_or_base_dimensions() {
        let base = base_octahedral_section([1, 1, 1]);
        let valid_scatter = billboard_direct_scatter_section_for(&base);
        let mut stale_version = valid_scatter.to_bytes();
        stale_version[0] = 0;
        let mut truncated_payload = valid_scatter.to_bytes();
        truncated_payload.pop();
        let wrong_dimensions =
            billboard_direct_scatter_section_for(&base_octahedral_section([2, 1, 1])).to_bytes();

        for (name, scatter_data) in [
            (
                "postretro_test_billboard_scatter_stale_version.prl",
                stale_version,
            ),
            (
                "postretro_test_billboard_scatter_short_payload.prl",
                truncated_payload,
            ),
            (
                "postretro_test_billboard_scatter_wrong_dimensions.prl",
                wrong_dimensions,
            ),
        ] {
            let sections = vec![
                geometry_blob(sample_geometry()),
                bvh_blob(sample_bvh_section()),
                octahedral_sh_volume_blob(base.clone()),
                prl_format::SectionBlob {
                    section_id: SectionId::BillboardDirectScatterVolume as u32,
                    version: 1,
                    data: scatter_data,
                },
                default_texture_cache_keys_blob(),
                default_fog_volumes_blob(),
            ];
            let tmp = write_prl_fixture(sections, name);
            let world = load_prl(tmp.to_str().unwrap())
                .expect("invalid optional billboard scatter must not reject the map");
            assert!(world.billboard_direct_scatter_volume.is_none());
            assert!(
                world
                    .animated_billboard_direct_scatter_delta_volumes
                    .is_none()
            );
            std::fs::remove_file(&tmp).ok();
        }
    }

    #[test]
    fn load_prl_soft_drops_billboard_scatter_with_invalid_animated_csr_shape() {
        let base = base_octahedral_section([1, 1, 1]);
        let animated_direct = animated_direct_delta_section_for(expected_affinity_dims(
            base.grid_dimensions,
            AFFINITY_FACTOR,
        ));
        let mut malformed_scatter =
            animated_billboard_direct_scatter_delta_section_for(&animated_direct).to_bytes();
        // Header (18 bytes) + one descriptor mapping (4 bytes) → CSR's
        // leading offset. A nonzero start invalidates the optional CSR shape.
        malformed_scatter[22..26].copy_from_slice(&1u32.to_le_bytes());
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            octahedral_sh_volume_blob(base.clone()),
            billboard_direct_scatter_blob(billboard_direct_scatter_section_for(&base)),
            animated_direct_sh_delta_blob(animated_direct),
            prl_format::SectionBlob {
                section_id: SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32,
                version: 1,
                data: malformed_scatter,
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_billboard_scatter_bad_csr.prl");
        let world = load_prl(tmp.to_str().unwrap())
            .expect("malformed optional animated scatter CSR must not reject the map");
        assert!(world.billboard_direct_scatter_volume.is_none());
        assert!(
            world
                .animated_billboard_direct_scatter_delta_volumes
                .is_none()
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_soft_drops_billboard_scatter_when_descriptor_mapping_disagrees_with_id45() {
        let base = base_octahedral_section([1, 1, 1]);
        let animated_direct = animated_direct_delta_section_for(expected_affinity_dims(
            base.grid_dimensions,
            AFFINITY_FACTOR,
        ));
        let mut animated_scatter =
            animated_billboard_direct_scatter_delta_section_for(&animated_direct);
        animated_scatter.animation_descriptor_indices[0] = 0;
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            octahedral_sh_volume_blob(base.clone()),
            billboard_direct_scatter_blob(billboard_direct_scatter_section_for(&base)),
            animated_direct_sh_delta_blob(animated_direct),
            animated_billboard_direct_scatter_delta_blob(animated_scatter),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_billboard_scatter_descriptor_mapping_mismatch.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("descriptor disagreement must fall back without rejecting the map");
        assert!(world.billboard_direct_scatter_volume.is_none());
        assert!(
            world
                .animated_billboard_direct_scatter_delta_volumes
                .is_none()
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_soft_drops_billboard_scatter_with_invalid_dense_payload_length() {
        let base = base_octahedral_section([1, 1, 1]);
        let animated_direct = animated_direct_delta_section_for(expected_affinity_dims(
            base.grid_dimensions,
            AFFINITY_FACTOR,
        ));
        let mut truncated_payload =
            animated_billboard_direct_scatter_delta_section_for(&animated_direct).to_bytes();
        truncated_payload.pop();
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            octahedral_sh_volume_blob(base.clone()),
            billboard_direct_scatter_blob(billboard_direct_scatter_section_for(&base)),
            animated_direct_sh_delta_blob(animated_direct),
            prl_format::SectionBlob {
                section_id: SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32,
                version: 1,
                data: truncated_payload,
            },
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_billboard_scatter_bad_payload_length.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("bad optional animated scatter payload must not reject the map");
        assert!(world.billboard_direct_scatter_volume.is_none());
        assert!(
            world
                .animated_billboard_direct_scatter_delta_volumes
                .is_none()
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_disables_billboard_scatter_when_id45_lacks_id48() {
        let base = base_octahedral_section([1, 1, 1]);
        let animated_direct = animated_direct_delta_section_for(expected_affinity_dims(
            base.grid_dimensions,
            AFFINITY_FACTOR,
        ));
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            octahedral_sh_volume_blob(base.clone()),
            billboard_direct_scatter_blob(billboard_direct_scatter_section_for(&base)),
            animated_direct_sh_delta_blob(animated_direct),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_billboard_scatter_missing_animated_companion.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("missing optional id-48 companion must not reject the map");
        assert!(world.billboard_direct_scatter_volume.is_none());
        assert!(
            world
                .animated_billboard_direct_scatter_delta_volumes
                .is_none()
        );
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_parses_entity_shadow_lights_when_direct_sh_is_present() {
        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        let direct_sh = minimal_direct_sh_volume_section();
        let direct_sh_delta = direct_delta_section_for(
            expected_affinity_dims(direct_sh.grid_dimensions, AFFINITY_FACTOR),
            vec![0],
        );
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
                data: sample_alpha_lights().to_bytes(),
            },
            direct_sh_volume_blob(direct_sh),
            entity_shadow_lights_blob(vec![0]),
            direct_sh_delta_blob(direct_sh_delta),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_entity_shadow_lights.prl");
        let world = load_prl(tmp.to_str().unwrap())
            .expect("PRL with DirectShVolume and EntityShadowLights must load");

        assert_eq!(world.entity_shadow_lights, vec![0]);
        assert!(world.direct_sh_delta_volumes.is_some());
        assert!(
            world.shadowmask_atlas.is_none(),
            "missing ShadowmaskAtlas must not clear EntityShadowLights or direct SH deltas"
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_exposes_shadowmask_atlas_multi_layer_payload() {
        let shadowmask = postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection {
            width: 2,
            height: 1,
            layer_count: 2,
            channels: vec![0],
            data: vec![255, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        };
        let direct_sh = minimal_direct_sh_volume_section();
        let direct_sh_delta = direct_delta_section_for(
            expected_affinity_dims(direct_sh.grid_dimensions, AFFINITY_FACTOR),
            vec![0],
        );
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            prl_format::SectionBlob {
                section_id: SectionId::AlphaLights as u32,
                version: 1,
                data: sample_alpha_lights().to_bytes(),
            },
            direct_sh_volume_blob(direct_sh),
            entity_shadow_lights_blob(vec![0]),
            direct_sh_delta_blob(direct_sh_delta),
            lightmap_blob(2, 1, 2),
            shadowmask_blob(shadowmask.clone()),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_shadowmask_atlas.prl");
        let world = load_prl(tmp.to_str().unwrap()).expect("PRL with ShadowmaskAtlas must load");
        let loaded = world
            .shadowmask_atlas
            .expect("ShadowmaskAtlas section must be exposed");

        assert_eq!(loaded.layer_count, 2);
        assert_eq!(loaded.channels, shadowmask.channels);
        assert_eq!(loaded.data, shadowmask.data);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_ignores_malformed_shadowmask_without_clearing_direct_selection() {
        // Regression: malformed optional shadowmask data must disable only
        // baked world visibility; static-light entity promotion remains valid.
        let direct_sh = minimal_direct_sh_volume_section();
        let direct_sh_delta = direct_delta_section_for(
            expected_affinity_dims(direct_sh.grid_dimensions, AFFINITY_FACTOR),
            vec![0],
        );
        let mut malformed_shadowmask = shadowmask_blob(
            postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection {
                width: 2,
                height: 1,
                layer_count: 2,
                channels: vec![0],
                data: vec![255; 16],
            },
        );
        malformed_shadowmask
            .data
            .pop()
            .expect("fixture shadowmask payload must be non-empty");
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            prl_format::SectionBlob {
                section_id: SectionId::AlphaLights as u32,
                version: 1,
                data: sample_alpha_lights().to_bytes(),
            },
            direct_sh_volume_blob(direct_sh),
            entity_shadow_lights_blob(vec![0]),
            direct_sh_delta_blob(direct_sh_delta),
            lightmap_blob(2, 1, 2),
            malformed_shadowmask,
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_malformed_shadowmask_atlas.prl");
        let world = load_prl(tmp.to_str().unwrap())
            .expect("malformed ShadowmaskAtlas must degrade without failing load");

        assert_eq!(world.entity_shadow_lights, vec![0]);
        assert!(world.direct_sh_delta_volumes.is_some());
        assert!(
            world.shadowmask_atlas.is_none(),
            "malformed optional shadowmask must degrade to absence"
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_clears_direct_selection_set_when_id41_validity_disagrees_with_id34() {
        let shadowmask = postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection {
            width: 2,
            height: 1,
            layer_count: 2,
            channels: vec![0],
            data: vec![255, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        };
        let direct_sh = minimal_direct_sh_volume_section();
        let mut direct_sh_delta = direct_delta_section_for(
            expected_affinity_dims(direct_sh.grid_dimensions, AFFINITY_FACTOR),
            vec![0],
        );
        // The section validates its own compact length, but id 34 marks every
        // fixture probe invalid. This must clear 40 + 41 + 42 before renderer
        // buffer construction rather than letting the mask misalign later CSR entries.
        direct_sh_delta.valid_probe_masks[0] = 1;
        direct_sh_delta.delta_subblocks = vec![0; DEFAULT_DELTA_PROBE_F16_STRIDE];
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            prl_format::SectionBlob {
                section_id: SectionId::AlphaLights as u32,
                version: 1,
                data: sample_alpha_lights().to_bytes(),
            },
            direct_sh_volume_blob(direct_sh),
            entity_shadow_lights_blob(vec![0]),
            direct_sh_delta_blob(direct_sh_delta),
            lightmap_blob(2, 1, 2),
            shadowmask_blob(shadowmask),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_direct_delta_validity_mismatch.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("an id-41/id-34 validity mismatch should degrade only promotion");

        assert!(world.entity_shadow_lights.is_empty());
        assert!(world.direct_sh_delta_volumes.is_none());
        assert!(world.shadowmask_atlas.is_none());

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_ignores_shadowmask_atlas_without_lightmap_only() {
        // Regression: ShadowmaskAtlas without its defining Lightmap must disable
        // only the entity-to-world union term, not static-light entity receipt.
        let direct_sh = minimal_direct_sh_volume_section();
        let direct_sh_delta = direct_delta_section_for(
            expected_affinity_dims(direct_sh.grid_dimensions, AFFINITY_FACTOR),
            vec![0],
        );
        let shadowmask = postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection {
            width: 2,
            height: 1,
            layer_count: 2,
            channels: vec![0],
            data: vec![255, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        };
        let sections = vec![
            geometry_blob(sample_geometry()),
            bvh_blob(sample_bvh_section()),
            prl_format::SectionBlob {
                section_id: SectionId::AlphaLights as u32,
                version: 1,
                data: sample_alpha_lights().to_bytes(),
            },
            direct_sh_volume_blob(direct_sh),
            entity_shadow_lights_blob(vec![0]),
            direct_sh_delta_blob(direct_sh_delta),
            shadowmask_blob(shadowmask),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_shadowmask_atlas_without_lightmap.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("ShadowmaskAtlas without Lightmap must degrade without failing load");

        assert_eq!(world.entity_shadow_lights, vec![0]);
        assert!(world.direct_sh_delta_volumes.is_some());
        assert!(
            world.shadowmask_atlas.is_none(),
            "ShadowmaskAtlas depends on Lightmap dimensions and must be ignored when Lightmap is absent"
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_degrades_malformed_entity_shadow_lights_to_empty() {
        // A malformed EntityShadowLights section (non-ascending indices) must warn
        // and degrade to empty — not brick the whole level load — mirroring the
        // sibling DirectShDeltaVolumes degradation path.
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
                section_id: SectionId::AlphaLights as u32,
                version: 1,
                data: sample_alpha_lights().to_bytes(),
            },
            direct_sh_volume_blob(minimal_direct_sh_volume_section()),
            entity_shadow_lights_blob(vec![2, 1]),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_entity_shadow_lights_malformed.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("malformed EntityShadowLights must degrade to empty, not fail load");

        assert!(world.entity_shadow_lights.is_empty());

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_degrades_invalid_entity_shadow_light_selection_to_empty() {
        // A structurally valid section that selects an ineligible light (index 1
        // is dynamic-tier in `sample_alpha_lights`) must warn and degrade to
        // empty, not fail the whole load.
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
                section_id: SectionId::AlphaLights as u32,
                version: 1,
                data: sample_alpha_lights().to_bytes(),
            },
            direct_sh_volume_blob(minimal_direct_sh_volume_section()),
            entity_shadow_lights_blob(vec![1]),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_entity_shadow_lights_invalid_selection.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("invalid EntityShadowLights selection must degrade to empty, not fail load");

        assert!(world.entity_shadow_lights.is_empty());

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_clears_entity_shadow_lights_without_direct_sh_deltas() {
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
                section_id: SectionId::AlphaLights as u32,
                version: 1,
                data: sample_alpha_lights().to_bytes(),
            },
            direct_sh_volume_blob(minimal_direct_sh_volume_section()),
            entity_shadow_lights_blob(vec![0]),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(sections, "postretro_test_entity_shadow_lights_no_delta.prl");
        let world = load_prl(tmp.to_str().unwrap())
            .expect("EntityShadowLights without DirectShDeltaVolumes must degrade to empty");

        assert!(world.entity_shadow_lights.is_empty());
        assert!(world.direct_sh_delta_volumes.is_none());

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_ignores_direct_sh_deltas_without_entity_shadow_lights() {
        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        let direct_sh = minimal_direct_sh_volume_section();
        let direct_sh_delta = direct_delta_section_for(
            expected_affinity_dims(direct_sh.grid_dimensions, AFFINITY_FACTOR),
            Vec::new(),
        );
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
                data: sample_alpha_lights().to_bytes(),
            },
            direct_sh_volume_blob(direct_sh),
            direct_sh_delta_blob(direct_sh_delta),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_direct_sh_deltas_no_entity_shadow_lights.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("DirectShDeltaVolumes without EntityShadowLights must load as no promotion");

        assert!(world.entity_shadow_lights.is_empty());
        assert!(world.direct_sh_delta_volumes.is_none());

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_clears_entity_shadow_lights_when_direct_sh_deltas_are_unusable() {
        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        let direct_sh = minimal_direct_sh_volume_section();
        let direct_sh_delta = direct_delta_section_for(
            expected_affinity_dims(direct_sh.grid_dimensions, AFFINITY_FACTOR),
            vec![1],
        );
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
                data: sample_alpha_lights().to_bytes(),
            },
            direct_sh_volume_blob(direct_sh),
            entity_shadow_lights_blob(vec![0]),
            direct_sh_delta_blob(direct_sh_delta),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_entity_shadow_lights_bad_delta.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("unusable DirectShDeltaVolumes must degrade to no promotion");

        assert!(world.entity_shadow_lights.is_empty());
        assert!(world.direct_sh_delta_volumes.is_none());

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_clears_entity_shadow_lights_when_direct_sh_deltas_are_partial() {
        let geom = sample_geometry();
        let bvh = sample_bvh_section();
        let direct_sh = minimal_direct_sh_volume_section();
        let direct_sh_delta = direct_delta_section_for(
            expected_affinity_dims(direct_sh.grid_dimensions, AFFINITY_FACTOR),
            vec![0],
        );
        let mut alpha_lights = sample_alpha_lights();
        alpha_lights.lights[1].is_dynamic = false;
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
            direct_sh_volume_blob(direct_sh),
            entity_shadow_lights_blob(vec![0, 1]),
            direct_sh_delta_blob(direct_sh_delta),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_entity_shadow_lights_partial_delta.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("partial DirectShDeltaVolumes must degrade to no promotion");

        assert!(world.entity_shadow_lights.is_empty());
        assert!(world.direct_sh_delta_volumes.is_none());

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_ignores_entity_shadow_lights_without_direct_sh() {
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
                section_id: SectionId::AlphaLights as u32,
                version: 1,
                data: sample_alpha_lights().to_bytes(),
            },
            entity_shadow_lights_blob(vec![0, 2]),
            default_texture_cache_keys_blob(),
            default_fog_volumes_blob(),
        ];

        let tmp = write_prl_fixture(
            sections,
            "postretro_test_entity_shadow_lights_no_direct.prl",
        );
        let world = load_prl(tmp.to_str().unwrap())
            .expect("EntityShadowLights without DirectShVolume must degrade to empty");

        assert!(world.entity_shadow_lights.is_empty());

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
                face_count: 2,
                portal_ref_start: 0,
                portal_ref_count: 0,
            }],
            portal_refs: Vec::new(),
        };
        let locator = CellLocatorSection {
            root: FormatCellLocatorChild::Cell(0),
            nodes: Vec::new(),
        };
        let mut geometry = sample_geometry();
        for face in &mut geometry.faces {
            face.leaf_index = 0;
        }

        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geometry.to_bytes(),
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
