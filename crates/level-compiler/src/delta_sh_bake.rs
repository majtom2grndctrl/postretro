// Per-animated-light sparse octahedral delta irradiance baker (CSR / affinity-cell form).
//
// For each animated light, bakes the light's **indirect-only** (bounced)
// unit-radiance transport contribution into octahedral irradiance tiles — but
// ONLY at base-grid probes that fall
// inside an affinity cell the light reaches (from `affinity_grid::decompose_affinity`).
//
// Indirect-only is a deliberate split: the animated light's DIRECT contribution
// already lives in `lm_anim` (the animated weight-map bake, occlusion-tested);
// folding it into the delta too would double-count it. Base and delta
// irradiance both store bounce only. See `context/lib/rendering_pipeline.md §4`
// (Animated SH delta volumes).
// The result is stored as a CSR index keyed by affinity cell:
//
//   - `affinity_offsets[c]..affinity_offsets[c+1]` is cell `c`'s slice of
//     `affinity_lights` (the animated-light indices touching that cell).
//   - `delta_subblocks` is index-parallel to `affinity_lights`: one dense
//     64-probe sub-block per CSR entry, x-fastest in-cell order
//     `local = lx + ly*4 + lz*16`; each probe slot is one row-major RGBA16F
//     octahedral tile matching the base irradiance atlas tile geometry.
//
// The runtime compose pass reads one per-cell light list per workgroup and adds
// `delta × authored_radiance × curve_modulation(t)` to the base irradiance atlas.
//
// Sub-blocks are COINCIDENT with base SH probes 1:1: the base-probe global coords
// of in-cell probe `local` in cell `(cx,cy,cz)` are `(cx*4+lx, cy*4+ly, cz*4+lz)`
// and its world position is `base_origin + global_coords * base_spacing` — the
// same origin/spacing the base `bake_sh_volume` uses.
//
// Index-space contract: `affinity_lights[i]` and
// `animation_descriptor_indices[affinity_lights[i]]` both index
// `AnimatedBakedLights.entries()` — same iteration order, no remap.
//
// Wire format: `context/lib/build_pipeline.md §PRL section IDs`

use std::collections::HashSet;

use crate::bake_control::BakeControl;
use bvh::bvh::Bvh;
use glam::{DVec3, Vec3};
use postretro_level_format::delta_sh_volumes::{
    AFFINITY_FACTOR as FORMAT_AFFINITY_FACTOR, DEFAULT_DELTA_PROBE_F16_STRIDE,
    DeltaShVolumesSection, PROBES_PER_CELL,
};
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
};

use crate::affinity_grid::{
    AFFINITY_FACTOR, AffinityInputs, build_csr, csr_entry_cells, decompose_affinity,
};
use crate::bvh_build::BvhPrimitive;
use crate::cache::StageCache;
use crate::delta_sh_cache::{DeltaShCacheInputs, DeltaShCacheTally, bake_or_load_delta_subblocks};
use crate::geometry::GeometryResult;
use crate::light_namespaces::AnimatedBakedLights;
use crate::map_data::{LightType, MapLight};
use crate::partition::BspTree;
use crate::portals::Portal;
use crate::sh_bake::{
    RaytracingCtx, bake_probe_indirect_rgb, pack_octahedral_irradiance_tile, probe_is_valid_pub,
};

// The compiler-side affinity factor (cell geometry) and the format-side
// factor (written into the section, validated by the loader) must agree, or
// the bake would emit a section whose stored factor matches the engine while
// its actual cell geometry used a different one.
const _: () = assert!(AFFINITY_FACTOR as u8 == FORMAT_AFFINITY_FACTOR);

/// AABB padding past the light's falloff sphere, meters. Extends coverage
/// slightly beyond the falloff radius so boundary probes inside included
/// cells aren't dropped from their sub-block. Mirrors `affinity_grid`.
const AABB_PADDING_METERS: f64 = 0.5;

/// Hard cap on directional-light AABB size, meters. Directional lights
/// nominally cover the whole world, but the delta volume only needs to
/// span the playable geometry — the world AABB substitutes for the
/// "infinite" influence sphere.
const DIRECTIONAL_FALLBACK_RANGE_METERS: f64 = 100.0;

/// Cache stage id for raw indirect-delta `(affinity cell, light)` sub-blocks.
/// It must remain distinct from the animated-direct and entity-shadow delta
/// stages because their payloads share the same dense f16 shape.
pub(crate) const INDIRECT_DELTA_SH_STAGE_ID: &str = "indirect_delta_sh_subblock";

/// Bump when the indirect-delta sub-block computation or its key inputs change.
pub(crate) const INDIRECT_DELTA_SH_STAGE_VERSION: u32 = 1;

/// Inputs for the delta SH bake. Mirrors `sh_bake::ShBakeCtx` (same BVH,
/// same geometry, same BSP tree) plus the animated-light envelope and the
/// portal graph the affinity decomposition floods over.
pub struct DeltaBakeInputs<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    pub tree: &'a BspTree,
    pub exterior_leaves: &'a HashSet<usize>,
    pub portals: &'a [Portal],
    pub animated_lights: &'a AnimatedBakedLights<'a>,
}

/// Bake the sparse delta SH section. Returns `None` when the envelope is empty —
/// the caller should omit the `DeltaShVolumes` section entirely in that case (an
/// empty section is wasted bytes).
///
/// One pass: invert the affinity decomposition into a CSR index, then bake one
/// 64-probe sub-block per CSR entry. CSR entries and sub-blocks are produced
/// together so `affinity_lights` and `delta_subblocks` stay index-parallel.
pub fn bake_delta_sh_volumes(
    inputs: &DeltaBakeInputs<'_>,
    config: &crate::sh_bake::ShConfig,
) -> Option<DeltaShVolumesSection> {
    bake_delta_sh_volumes_controlled(inputs, config, None, &BakeControl::unrestricted())
}

pub fn bake_delta_sh_volumes_controlled(
    inputs: &DeltaBakeInputs<'_>,
    config: &crate::sh_bake::ShConfig,
    cache: Option<&StageCache>,
    control: &BakeControl,
) -> Option<DeltaShVolumesSection> {
    bake_delta_sh_volumes_controlled_with_tally(inputs, config, cache, control).0
}

/// Test-facing cache accounting for the indirect delta bake. The public bake
/// API intentionally remains section-only; pipeline diagnostics do not need
/// cache-locality data, while the cross-bake regression suite does.
pub(crate) fn bake_delta_sh_volumes_controlled_with_tally(
    inputs: &DeltaBakeInputs<'_>,
    config: &crate::sh_bake::ShConfig,
    cache: Option<&StageCache>,
    control: &BakeControl,
) -> (Option<DeltaShVolumesSection>, DeltaShCacheTally) {
    if inputs.animated_lights.is_empty() {
        return (None, DeltaShCacheTally::default());
    }
    if inputs.geometry.geometry.vertices.is_empty() {
        return (None, DeltaShCacheTally::default());
    }
    let probe_spacing = config.probe_spacing;
    let animated_light_count = inputs.animated_lights.len();

    // Base SH grid origin/spacing — these MUST match `bake_sh_volume` so the
    // affinity cells (and thus the baked sub-blocks) land on real base probes.
    let base_origin = world_aabb_min(inputs);

    // Affinity decomposition: per-light affinity-cell lists + grid dims. Shares
    // the same world AABB / spacing as the base grid (the `geometry_vertices`
    // and `probe_spacing` are identical to `bake_sh_volume`'s).
    let geometry_vertices = vertex_positions(inputs);
    let decomposition = decompose_affinity(&AffinityInputs {
        geometry_vertices: &geometry_vertices,
        tree: inputs.tree,
        exterior_leaves: inputs.exterior_leaves,
        portals: inputs.portals,
        animated_lights: inputs.animated_lights,
        probe_spacing,
    });
    let affinity_dims = decomposition.affinity_dims;
    let affinity_cell_count = decomposition.affinity_cell_count();

    // --- CSR index: invert `per_light_cells` (light → cells) into cell → lights.
    let (affinity_offsets, affinity_lights) =
        build_csr(&decomposition.per_light_cells, affinity_cell_count);
    control.publish_total(affinity_lights.len());

    // Bake-time invariants the loader also enforces.
    assert_eq!(
        affinity_offsets.len(),
        affinity_cell_count + 1,
        "affinity_offsets must hold one entry per cell plus a trailing total"
    );
    debug_assert_eq!(
        *affinity_offsets.last().expect("offsets non-empty") as usize,
        affinity_lights.len(),
        "trailing CSR offset must equal the flat light-list length"
    );
    for &light in &affinity_lights {
        assert!(
            (light as usize) < animated_light_count,
            "affinity_lights entry {light} out of range (animated_light_count = {animated_light_count})"
        );
    }

    // --- Per-light logging (AC #2): emitted-cell vs full-AABB probe counts.
    log_per_light_culling(
        inputs,
        &decomposition.per_light_cells,
        base_origin,
        probe_spacing,
    );

    // --- Sub-blocks: one dense 64-probe block per CSR entry, index-parallel to
    // `affinity_lights`. The shared cache holds this raw pre-drop payload, so
    // every downstream delta pass remains identical for hit and miss paths.
    let entries = inputs.animated_lights.entries();
    let csr_cells = csr_entry_cells(&affinity_offsets);
    let valid_probe_masks = valid_probe_masks(inputs, affinity_dims, base_origin, probe_spacing);
    let keyed_lights: Vec<MapLight> = entries
        .iter()
        .map(|entry| unit_radiance_light(entry.light))
        .collect();
    // Indirect soft-visibility is seeded from stable cell/probe coordinates,
    // already folded in the entry key. Unlike the two direct-delta bakes it has
    // no light-global seed axis to add.
    let light_seed_axes = vec![0; keyed_lights.len()];
    let cached_subblocks = bake_or_load_delta_subblocks(
        &DeltaShCacheInputs {
            stage_id: INDIRECT_DELTA_SH_STAGE_ID,
            stage_version: INDIRECT_DELTA_SH_STAGE_VERSION,
            geometry_hash: crate::sh_group::geometry_content_hash(inputs.geometry),
            affinity_dims,
            affinity_lights: &affinity_lights,
            csr_entry_cells: &csr_cells,
            valid_probe_masks: &valid_probe_masks,
            probe_spacing,
            light_seed_axes: &light_seed_axes,
            expected_subblock_f16_len: PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE,
        },
        &keyed_lights,
        cache,
        control,
        |light_index, cell| {
            bake_subblock(
                inputs,
                &keyed_lights[light_index as usize],
                cell,
                affinity_dims,
                base_origin,
                probe_spacing,
            )
        },
    );

    let animation_descriptor_indices: Vec<u32> = (0..animated_light_count as u32).collect();
    let section = DeltaShVolumesSection {
        affinity_factor: FORMAT_AFFINITY_FACTOR,
        affinity_dims,
        tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
        tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
        animation_descriptor_indices,
        // The indirect bake writes a dense L0 block for every probe (including
        // invalid zeros), preserving the pre-existing wire contract. These
        // all-valid masks are intentionally unrelated to the cache's private
        // per-cell validity fold above; downstream compaction owns the section
        // masks as before.
        valid_probe_masks: vec![u64::MAX; affinity_cell_count],
        cell_levels: vec![0u8; affinity_cell_count],
        affinity_offsets,
        affinity_lights,
        delta_subblocks: cached_subblocks.subblocks,
    };
    debug_assert_eq!(
        section.expected_delta_subblock_f16_count(),
        Some(section.delta_subblocks.len()),
        "the dense bake must still satisfy the section's descriptor-driven payload identity"
    );

    (Some(section), cached_subblocks.tally)
}

/// Log emitted-cell vs full-AABB probe counts per animated light (AC #2). The
/// emitted count is the affinity-clipped probe count actually baked; the
/// full-AABB count is what the old dense per-light grid would have produced.
pub fn log_stats(section: &DeltaShVolumesSection) {
    let total_probes = section.affinity_lights.len() * PROBES_PER_CELL;
    log::info!(
        "[Compiler] DeltaShVolumes: {} animated light(s), {} CSR entries, {total_probes} emitted probes, affinity_dims {}x{}x{}",
        section.animation_descriptor_indices.len(),
        section.affinity_lights.len(),
        section.affinity_dims[0],
        section.affinity_dims[1],
        section.affinity_dims[2],
    );
}

// ---------------------------------------------------------------------------
// Per-entry sub-block bake

/// Bake one dense 64-probe sub-block for `(cell, light)`. Returns the
/// flat probe payload (`PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE` halves):
/// one RGBA16F octahedral tile per probe, x-fastest in-cell order.
///
/// Per-probe clip (see module doc): cell inclusion is portal-granular; affinity
/// decomposition already dropped cells the light can't reach through portals.
/// Within an included cell each probe is additionally clipped by the
/// light's AABB and the solid/exterior validity gate (`probe_is_valid_pub`, the
/// same gate the dense bake used). Probes outside the AABB or in an invalid leaf
/// are written all-zero. The portal-reachability clip stays cell-granular and
/// uses the affinity-cell centroid rather than per-probe portal tests.
fn bake_subblock(
    inputs: &DeltaBakeInputs<'_>,
    unit_light: &MapLight,
    cell: u32,
    affinity_dims: [u32; 3],
    base_origin: DVec3,
    base_spacing: f32,
) -> Vec<u16> {
    let (light_min, light_max) = light_aabb(unit_light, world_aabb_for_directional(inputs));

    let ctx = RaytracingCtx {
        bvh: inputs.bvh,
        primitives: inputs.primitives,
        geometry: inputs.geometry,
    };

    let mut out = vec![0u16; PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE];
    for local in 0..PROBES_PER_CELL {
        // In-cell local coords, x-fastest: local = lx + ly*4 + lz*16. Matches the
        // compose shader's `local_invocation_index` for @workgroup_size(4,4,4).
        // Base-probe global coords, then world position (coincident with the
        // base SH probe at the same global coords). This shared helper is also
        // what derives the cache key's per-cell validity mask.
        let pos_d = delta_probe_position(cell, local, affinity_dims, base_origin, base_spacing);

        // Per-probe AABB clip + solid/exterior validity gate. Out-of-region probes
        // inside an included cell stay zero (their slot is already zero-filled).
        if !point_in_aabb(pos_d, light_min, light_max) {
            continue;
        }
        if !probe_is_valid_pub(inputs.tree, inputs.exterior_leaves, pos_d) {
            continue;
        }

        let pos = Vec3::new(pos_d.x as f32, pos_d.y as f32, pos_d.z as f32);
        let lights_slice: [&MapLight; 1] = [unit_light];
        // Indirect-only: the same bounce math as the base bake. The animated
        // light's DIRECT contribution lives in `lm_anim` (occlusion-tested),
        // so baking it here too would double-count. Delta irradiance is indirect-only.
        // Stable per-probe index for the soft-visibility seed: the affinity cell
        // plus the in-cell local slot uniquely (and deterministically) identify
        // this delta probe across separate processes — no RNG, no hash order.
        // Base-vs-delta numeric collisions are harmless: each bake writes a
        // separate PRL section (DeltaShVolumes vs OctahedralShVolume) and the
        // two are never combined — a shared seed just reuses the same lattice
        // rotation, never mixing values between the bakes.
        let probe_index = cell as u64 * PROBES_PER_CELL as u64 + local as u64;
        let indirect = bake_probe_indirect_rgb(&ctx, pos, &lights_slice, probe_index);
        let tile = pack_octahedral_irradiance_tile(
            &indirect,
            true,
            DEFAULT_IRRADIANCE_TILE_DIMENSION,
            DEFAULT_IRRADIANCE_TILE_BORDER,
        );

        let base = local * DEFAULT_DELTA_PROBE_F16_STRIDE;
        for (texel_index, texel) in tile.iter().enumerate() {
            let dst = base + texel_index * 4;
            // alpha channel (texel.rgba[3]) is set to f16(1.0) for packing-path symmetry with
            // base tiles; sh_compose.wgsl reads only delta.rgb (alpha unused downstream).
            out[dst..dst + 4].copy_from_slice(&texel.rgba);
        }
    }

    out
}

/// Normalize a delta's single light to unit radiance. The runtime descriptor
/// supplies authored color and intensity during composition, so neither may
/// affect this transport payload or its cache key.
fn unit_radiance_light(light: &MapLight) -> MapLight {
    let mut unit_light = light.clone();
    unit_light.color = [1.0, 1.0, 1.0];
    unit_light.intensity = 1.0;
    unit_light
}

/// Per-cell validity for cache keys. This mirrors the precise `pos_d` and
/// `probe_is_valid_pub` gate in `bake_subblock`; BSP/exterior changes are not
/// represented by the render-geometry content hash alone.
fn valid_probe_masks(
    inputs: &DeltaBakeInputs<'_>,
    affinity_dims: [u32; 3],
    base_origin: DVec3,
    base_spacing: f32,
) -> Vec<u64> {
    let affinity_cell_count = affinity_dims[0] * affinity_dims[1] * affinity_dims[2];
    (0..affinity_cell_count)
        .map(|cell| {
            let mut mask = 0u64;
            for local in 0..PROBES_PER_CELL {
                let pos_d =
                    delta_probe_position(cell, local, affinity_dims, base_origin, base_spacing);
                if probe_is_valid_pub(inputs.tree, inputs.exterior_leaves, pos_d) {
                    mask |= 1u64 << local;
                }
            }
            mask
        })
        .collect()
}

fn delta_probe_position(
    cell: u32,
    local: usize,
    affinity_dims: [u32; 3],
    base_origin: DVec3,
    base_spacing: f32,
) -> DVec3 {
    let nx = affinity_dims[0];
    let ny = affinity_dims[1];
    let cell_x = cell % nx;
    let cell_y = (cell / nx) % ny;
    let cell_z = cell / (nx * ny);
    let lx = (local % AFFINITY_FACTOR as usize) as u32;
    let ly = ((local / AFFINITY_FACTOR as usize) % AFFINITY_FACTOR as usize) as u32;
    let lz = (local / (AFFINITY_FACTOR as usize * AFFINITY_FACTOR as usize)) as u32;
    let spacing = base_spacing as f64;

    DVec3::new(
        base_origin.x + (cell_x * AFFINITY_FACTOR + lx) as f64 * spacing,
        base_origin.y + (cell_y * AFFINITY_FACTOR + ly) as f64 * spacing,
        base_origin.z + (cell_z * AFFINITY_FACTOR + lz) as f64 * spacing,
    )
}

// ---------------------------------------------------------------------------
// Per-light culling logging (AC #2)

fn log_per_light_culling(
    inputs: &DeltaBakeInputs<'_>,
    per_light_cells: &[Vec<u32>],
    base_origin: DVec3,
    base_spacing: f32,
) {
    let world = world_aabb_for_directional(inputs);
    for (i, (entry, cells)) in inputs
        .animated_lights
        .entries()
        .iter()
        .zip(per_light_cells.iter())
        .enumerate()
    {
        let emitted_probes = cells.len() * PROBES_PER_CELL;
        let full_aabb_probes = full_aabb_probe_count(entry.light, world, base_spacing);
        let _ = base_origin;
        log::info!(
            "[Compiler]   delta light {i}: {emitted_probes} emitted probes ({} cells) vs {full_aabb_probes} full-AABB probes",
            cells.len(),
        );
    }
}

/// Probe count the OLD dense per-light grid would have produced for this light's
/// full AABB (one probe per base-spacing cell over the padded AABB extents).
/// Mirrors the retired dense `grid_dimensions` so the log compares like for like.
fn full_aabb_probe_count(light: &MapLight, world: (DVec3, DVec3), spacing: f32) -> usize {
    let (min, max) = light_aabb(light, world);
    let extents = (max - min).max(DVec3::splat(0.0));
    let spacing = spacing.max(1.0e-4) as f64;
    let dim = |e: f64| ((e / spacing).ceil() as usize + 1).max(1);
    dim(extents.x) * dim(extents.y) * dim(extents.z)
}

// ---------------------------------------------------------------------------
// Geometry helpers — mirror affinity_grid / sh_bake so the sub-blocks land on
// real base probes and the AABBs match the affinity decomposition.

fn light_aabb(light: &MapLight, world_aabb: (DVec3, DVec3)) -> (DVec3, DVec3) {
    match light.light_type {
        LightType::Directional => world_aabb,
        LightType::Point | LightType::Spot => {
            let r = ((light.falloff_range as f64) + AABB_PADDING_METERS).max(0.01);
            let center = DVec3::new(light.origin.x, light.origin.y, light.origin.z);
            (center - DVec3::splat(r), center + DVec3::splat(r))
        }
    }
}

fn point_in_aabb(p: DVec3, min: DVec3, max: DVec3) -> bool {
    p.x >= min.x && p.x <= max.x && p.y >= min.y && p.y <= max.y && p.z >= min.z && p.z <= max.z
}

/// Vertex positions as `[[f32;3]]` for the affinity decomposition.
fn vertex_positions(inputs: &DeltaBakeInputs<'_>) -> Vec<[f32; 3]> {
    inputs
        .geometry
        .geometry
        .vertices
        .iter()
        .map(|v| v.position)
        .collect()
}

/// Base SH grid origin = geometry vertex AABB min. Identical to
/// `sh_bake::world_aabb`'s min so sub-blocks are coincident with base probes.
/// Returns the origin alone; an empty-geometry origin is unused (the empty-light
/// guard already returned `None` only when there ARE animated lights, but a map
/// with animated lights and no geometry yields a degenerate but valid section).
fn world_aabb_min(inputs: &DeltaBakeInputs<'_>) -> DVec3 {
    let mut min = DVec3::splat(f64::INFINITY);
    for v in &inputs.geometry.geometry.vertices {
        min = min.min(DVec3::new(
            v.position[0] as f64,
            v.position[1] as f64,
            v.position[2] as f64,
        ));
    }
    if !min.x.is_finite() { DVec3::ZERO } else { min }
}

/// Substitute world AABB used as the directional-light influence volume — the
/// extracted geometry's vertex bounds, matching `affinity_grid`. Falls back to a
/// small box around the origin when the geometry is empty (test fixtures).
fn world_aabb_for_directional(inputs: &DeltaBakeInputs<'_>) -> (DVec3, DVec3) {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for v in &inputs.geometry.geometry.vertices {
        let p = DVec3::new(
            v.position[0] as f64,
            v.position[1] as f64,
            v.position[2] as f64,
        );
        min = min.min(p);
        max = max.max(p);
    }
    if !min.x.is_finite() {
        let r = DIRECTIONAL_FALLBACK_RANGE_METERS;
        return (DVec3::splat(-r), DVec3::splat(r));
    }
    (min, max)
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh_build::build_bvh;
    use crate::geometry::FaceIndexRange;
    use crate::map_data::{FalloffModel, LightAnimation, LightType};
    use crate::partition::{Aabb as CompilerAabb, BspLeaf, BspTree};
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    fn tri_vertex(pos: [f32; 3]) -> Vertex {
        Vertex::new(
            pos,
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        )
    }

    /// A 16m cube of world geometry: vertices span [-8, 8] on every axis, so the
    /// base grid origin is (-8,-8,-8) and (at 1m spacing) base dims are 17³,
    /// affinity dims ceil(17/4) = 5³ = 125 cells.
    fn cube_geometry() -> GeometryResult {
        let s = 8.0_f32;
        let corners = [
            [-s, -s, -s],
            [s, -s, -s],
            [s, s, -s],
            [-s, s, -s],
            [-s, -s, s],
            [s, -s, s],
            [s, s, s],
            [-s, s, s],
        ];
        GeometryResult {
            geometry: GeometrySection {
                vertices: corners.iter().map(|&p| tri_vertex(p)).collect(),
                indices: vec![0, 1, 2],
                faces: vec![FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                }],
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: vec![FaceIndexRange {
                index_offset: 0,
                index_count: 3,
            }],
        }
    }

    fn empty_geometry() -> GeometryResult {
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

    fn tree_all_empty() -> BspTree {
        BspTree {
            nodes: Vec::new(),
            leaves: vec![BspLeaf {
                face_indices: Vec::new(),
                bounds: CompilerAabb {
                    min: DVec3::splat(-1000.0),
                    max: DVec3::splat(1000.0),
                },
                is_solid: false,
                defining_planes: Vec::new(),
            }],
        }
    }

    fn animated_point_light(origin: DVec3, range: f32) -> MapLight {
        MapLight {
            origin,
            carrier: String::new(),
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: range,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: Some(LightAnimation {
                period: 1.0,
                phase: 0.0,
                brightness: Some(vec![0.0, 1.0, 0.0]),
                color: None,
                direction: None,
                start_active: true,
            }),
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        }
    }

    // --- Sub-block layout --------------------------------------------------

    #[test]
    fn subblock_payload_has_expected_length_and_tile_geometry() {
        let geo = cube_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        // A single light at the origin; its AABB clips to a small cell block.
        let lights = vec![animated_point_light(DVec3::ZERO, 2.0)];
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let inputs = DeltaBakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            portals: &[],
            animated_lights: &envelope,
        };
        let section =
            bake_delta_sh_volumes(&inputs, &crate::sh_bake::ShConfig { probe_spacing: 1.0 })
                .expect("expected a section");

        // Index-parallel: payload length = entries × 64 × one octahedral tile.
        assert_eq!(
            section.expected_delta_subblock_f16_count(),
            Some(section.delta_subblocks.len()),
            "the dense bake must satisfy the descriptor-driven payload identity"
        );
        assert!(!section.affinity_lights.is_empty());
        assert_eq!(section.tile_dimension, DEFAULT_IRRADIANCE_TILE_DIMENSION);
        assert_eq!(section.tile_border, DEFAULT_IRRADIANCE_TILE_BORDER);
        // NOTE: no "any probe nonzero" assertion here. The delta is
        // INDIRECT-only: with this sparse single-triangle fixture the
        // origin probes have no surfaces to bounce off, so an all-zero payload is
        // correct. The nonzero-tile contract is covered by
        // `subblock_stores_indirect_only_not_direct_plus_indirect` (bit-exact vs
        // the indirect-only reference) instead.
    }

    /// Delta irradiance is INDIRECT-ONLY. The baked sub-block at a probe must
    /// equal the indirect-only bounce tile — NOT direct + indirect. The animated
    /// light's direct term lives in `lm_anim`; folding it into the delta too
    /// would double-count.
    #[test]
    fn subblock_stores_indirect_only_not_direct_plus_indirect() {
        // Light on a base-probe position inside cell 0's block, with cube
        // geometry so there is a real bounce to register. base_origin =
        // (-8,-8,-8), spacing 1 → probe (1,1,1) is at (-7,-7,-7); cell 0 covers
        // global probes 0..3 per axis, so (1,1,1) is local lx=ly=lz=1 →
        // local = 1 + 1*4 + 1*16 = 21.
        let geo = cube_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let light_pos = DVec3::new(-7.0, -7.0, -7.0);
        let mut authored_light = animated_point_light(light_pos, 1.5);
        authored_light.color = [0.25, 0.5, 0.75];
        authored_light.intensity = 3.0;
        let lights = vec![authored_light];
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let inputs = DeltaBakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            portals: &[],
            animated_lights: &envelope,
        };
        let section =
            bake_delta_sh_volumes(&inputs, &crate::sh_bake::ShConfig { probe_spacing: 1.0 })
                .expect("expected a section");

        // The CSR entry for cell 0 must exist (the light reaches it).
        assert!(section.affinity_offsets[1] > section.affinity_offsets[0]);
        let cell = 0usize;
        let entry = section.affinity_offsets[cell] as usize;
        let local = 21usize;
        let entry_base = section.affinity_offsets[..cell]
            .windows(2)
            .zip(&section.valid_probe_masks[..cell])
            .map(|(offsets, &mask)| {
                (offsets[1] - offsets[0]) as usize
                    * mask.count_ones() as usize
                    * DEFAULT_DELTA_PROBE_F16_STRIDE
            })
            .sum::<usize>()
            + (entry - section.affinity_offsets[cell] as usize)
                * section.valid_probe_masks[cell].count_ones() as usize
                * DEFAULT_DELTA_PROBE_F16_STRIDE;
        let within_cell_rank =
            (section.valid_probe_masks[cell] & ((1u64 << local) - 1)).count_ones() as usize;
        let slot = entry_base + within_cell_rank * DEFAULT_DELTA_PROBE_F16_STRIDE;

        // Recompute the reference indirect-only SH at this probe and pack it
        // through the same octahedral tile path the base bake uses, then compare
        // bit-for-bit.
        let ctx = RaytracingCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
        };
        let pos = Vec3::new(-7.0, -7.0, -7.0);
        let mut unit_light = lights[0].clone();
        unit_light.color = [1.0; 3];
        unit_light.intensity = 1.0;
        let indirect = bake_probe_indirect_rgb(&ctx, pos, &[&unit_light], 0);
        let expected_tile = pack_octahedral_irradiance_tile(
            &indirect,
            true,
            DEFAULT_IRRADIANCE_TILE_DIMENSION,
            DEFAULT_IRRADIANCE_TILE_BORDER,
        );
        let expected: Vec<u16> = expected_tile.iter().flat_map(|texel| texel.rgba).collect();
        let stored = &section.delta_subblocks[slot..slot + DEFAULT_DELTA_PROBE_F16_STRIDE];
        assert_eq!(
            stored,
            &expected[..],
            "delta sub-block must store unit-radiance indirect transport, not authored radiance or direct light",
        );
    }

    #[test]
    fn controlled_delta_bake_reports_every_affinity_entry_and_is_deterministic() {
        let geo = cube_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights = vec![animated_point_light(DVec3::new(-7.0, -7.0, -7.0), 1.5)];
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let inputs = DeltaBakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            portals: &[],
            animated_lights: &envelope,
        };
        let config = crate::sh_bake::ShConfig { probe_spacing: 1.0 };

        let progress = crate::reporter::StageProgress::indeterminate();
        let control = BakeControl::new(
            std::sync::Arc::new(crate::governor::Governor::new(2, false)),
            &progress,
        );
        let first_section = bake_delta_sh_volumes_controlled(&inputs, &config, None, &control)
            .expect("expected first deterministic delta section");
        assert_eq!(progress.total(), Some(first_section.affinity_lights.len()));
        assert_eq!(progress.completed(), first_section.affinity_lights.len());
        let first = first_section.to_bytes();
        let second = bake_delta_sh_volumes(&inputs, &config)
            .expect("expected second deterministic delta section")
            .to_bytes();

        assert_eq!(first, second, "delta bake must be byte-identical");

        let cache_dir = std::env::temp_dir().join(format!(
            "postretro_indirect_delta_cache_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after unix epoch")
                .as_nanos(),
        ));
        let cache = StageCache::new(&cache_dir).expect("create indirect delta cache");
        let cached_progress = crate::reporter::StageProgress::indeterminate();
        let cached_control = BakeControl::new(
            std::sync::Arc::new(crate::governor::Governor::new(2, false)),
            &cached_progress,
        );
        let cached =
            bake_delta_sh_volumes_controlled(&inputs, &config, Some(&cache), &cached_control)
                .expect("expected cached delta section")
                .to_bytes();
        assert_eq!(
            cached_progress.total(),
            Some(first_section.affinity_lights.len())
        );
        assert_eq!(
            cached_progress.completed(),
            first_section.affinity_lights.len()
        );
        let warm_progress = crate::reporter::StageProgress::indeterminate();
        let warm_control = BakeControl::new(
            std::sync::Arc::new(crate::governor::Governor::new(2, false)),
            &warm_progress,
        );
        let warm = bake_delta_sh_volumes_controlled(&inputs, &config, Some(&cache), &warm_control)
            .expect("expected warm delta section")
            .to_bytes();
        assert_eq!(
            warm_progress.total(),
            Some(first_section.affinity_lights.len())
        );
        assert_eq!(
            warm_progress.completed(),
            first_section.affinity_lights.len()
        );

        assert_eq!(first, cached, "cached miss must preserve pre-drop bytes");
        assert_eq!(first, warm, "cached hit must preserve pre-drop bytes");
        let _ = std::fs::remove_dir_all(cache_dir);
    }

    #[test]
    fn indirect_delta_cache_key_uses_unit_radiance_light_and_stage_version() {
        let mut authored = animated_point_light(DVec3::ZERO, 4.0);
        authored.color = [0.25, 0.5, 0.75];
        authored.intensity = 7.0;
        let unit = unit_radiance_light(&authored);

        let key = crate::delta_sh_cache::delta_sh_entry_cache_key(
            INDIRECT_DELTA_SH_STAGE_ID,
            INDIRECT_DELTA_SH_STAGE_VERSION,
            &[11; 32],
            [2, 3, 4],
            5,
            1.0,
            u64::MAX,
            0,
            &unit,
        );
        let mut recolored = authored;
        recolored.color = [0.1, 0.2, 0.3];
        recolored.intensity = 0.1;
        let recolored_key = crate::delta_sh_cache::delta_sh_entry_cache_key(
            INDIRECT_DELTA_SH_STAGE_ID,
            INDIRECT_DELTA_SH_STAGE_VERSION,
            &[11; 32],
            [2, 3, 4],
            5,
            1.0,
            u64::MAX,
            0,
            &unit_radiance_light(&recolored),
        );
        let bumped_version_key = crate::delta_sh_cache::delta_sh_entry_cache_key(
            INDIRECT_DELTA_SH_STAGE_ID,
            INDIRECT_DELTA_SH_STAGE_VERSION + 1,
            &[11; 32],
            [2, 3, 4],
            5,
            1.0,
            u64::MAX,
            0,
            &unit,
        );

        assert_eq!(key.as_filename(), recolored_key.as_filename());
        assert_ne!(key.as_filename(), bumped_version_key.as_filename());
    }

    // --- Out-of-region drop (AC #2) ----------------------------------------

    #[test]
    fn emitted_probe_count_is_less_than_full_aabb_when_cells_are_clipped() {
        // A wide-range light in a two-leaf map with NO portal between the leaves:
        // its AABB spans the whole world, but the affinity decomposition only
        // keeps cells reachable through portals from the light's leaf. The
        // emitted-cell probe count must be strictly less than the full-AABB count.
        use crate::partition::{BspChild, BspNode};
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
                    face_indices: Vec::new(),
                    bounds: CompilerAabb {
                        min: DVec3::splat(-1000.0),
                        max: DVec3::splat(1000.0),
                    },
                    is_solid: false,
                    defining_planes: Vec::new(),
                },
                BspLeaf {
                    face_indices: Vec::new(),
                    bounds: CompilerAabb {
                        min: DVec3::splat(-1000.0),
                        max: DVec3::splat(1000.0),
                    },
                    is_solid: false,
                    defining_planes: Vec::new(),
                },
            ],
        };
        let geo = cube_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let exterior: HashSet<usize> = HashSet::new();
        // Range 50 so the AABB covers the whole world; no portals → cells on the
        // far side of x=0 are dropped.
        let lights = vec![animated_point_light(DVec3::new(-4.0, 0.0, 0.0), 50.0)];
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let inputs = DeltaBakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            portals: &[],
            animated_lights: &envelope,
        };
        let section =
            bake_delta_sh_volumes(&inputs, &crate::sh_bake::ShConfig { probe_spacing: 1.0 })
                .expect("expected a section");

        let emitted = section.affinity_lights.len() * PROBES_PER_CELL;
        let world = world_aabb_for_directional(&inputs);
        let full = full_aabb_probe_count(&lights[0], world, 1.0);
        assert!(
            emitted < full,
            "portal clip must drop probes: emitted {emitted} should be < full-AABB {full}"
        );
    }

    #[test]
    fn empty_envelope_returns_none() {
        let geo = cube_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights: Vec<MapLight> = Vec::new();
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let inputs = DeltaBakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            portals: &[],
            animated_lights: &envelope,
        };
        assert!(
            bake_delta_sh_volumes(&inputs, &crate::sh_bake::ShConfig { probe_spacing: 1.0 })
                .is_none()
        );
    }

    #[test]
    fn empty_geometry_with_animated_lights_returns_none() {
        let geo = empty_geometry();
        let bvh = bvh::bvh::Bvh { nodes: Vec::new() };
        let prims: Vec<BvhPrimitive> = Vec::new();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights = vec![animated_point_light(DVec3::ZERO, 4.0)];
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let inputs = DeltaBakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            portals: &[],
            animated_lights: &envelope,
        };

        assert!(
            bake_delta_sh_volumes(&inputs, &crate::sh_bake::ShConfig { probe_spacing: 1.0 })
                .is_none(),
            "empty geometry has no base probe grid, so animated-light deltas must be omitted",
        );
    }

    #[test]
    fn section_round_trips_through_format_crate() {
        let geo = cube_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights = vec![animated_point_light(DVec3::ZERO, 1.5)];
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let inputs = DeltaBakeInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            portals: &[],
            animated_lights: &envelope,
        };
        let section =
            bake_delta_sh_volumes(&inputs, &crate::sh_bake::ShConfig { probe_spacing: 1.0 })
                .expect("expected a section");
        let bytes = section.to_bytes();
        let restored = DeltaShVolumesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }
}
