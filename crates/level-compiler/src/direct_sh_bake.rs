// Direct-at-probe octahedral SH baker: baked DIRECT light from STATIC lights for
// dynamic objects (entities / billboards).
//
// Sibling to the indirect octahedral SH bake (`sh_bake.rs`). The runtime symmetry
// target is: static surface = lightmap-direct + SH-indirect; entity/billboard =
// SH-direct + SH-indirect. This module bakes the SH-direct half.
//
// Per probe, this consumes the SAME `StaticBakedLights` set (in the same global
// order) the indirect/lightmap bakes consume, additionally restricted to
// `shadow_type == StaticLightMap` — an `Sdf`-typed light's DIRECT term is traced
// at runtime, so baking it here too would double-count it (the same disjoint-direct
// rule the static lightmap consumer applies). Animated/dynamic lights are already
// excluded by the `StaticBakedLights` filter (`!is_dynamic && animation.is_none()`);
// animated direct is owned by `lm_anim`.
//
// For each reaching static light at a probe, the validated D3 assembly (Task 0):
//   radiance = incident_radiance_at_point(light, probe) * soft_visibility(probe, light)
//   accumulate_sh_rgb along probe→light
// then after all lights apply_cosine_lobe_rgb and pack into the octahedral atlas.
//
// Per-probe light-reach culling (D9): before the per-probe shadow-ray pass, each
// probe's reaching-light set is derived from the SAME two-stage reach test the
// delta bake uses (falloff-sphere AABB clip + portal-reachability flood, via
// `affinity_grid::decompose_affinity_for_lights`). That yields a per-LIGHT→cell
// CSR; this module inverts it ONCE into cell→lights, then maps each probe to its
// affinity cell to read its reaching-light set. A culled light is provably
// zero-contribution at that probe, so baked coefficients stay byte-identical to an
// unculled bake.
//
// See: context/lib/build_pipeline.md, context/lib/rendering_pipeline.md §4

use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION, irradiance_atlas_dimensions,
    irradiance_atlas_tiles_per_row, irradiance_tile_origin,
};
use postretro_level_format::sh_volume::OctahedralAtlasTexel;
use rayon::prelude::*;

use crate::affinity_grid::{AFFINITY_FACTOR, AffinityReachInputs, decompose_affinity_for_lights};
use crate::bc6h;
use crate::cache::{CacheKey, StageCache};
use crate::map_data::{MapLight, ShadowType};
use crate::portals::Portal;
use crate::sh_bake::{
    ProbeGridLayout, RaytracingCtx, ShBakeCtx, ShConfig, bake_probe_direct_rgb,
    pack_octahedral_irradiance_tile, probe_grid_layout, static_light_refs, vec3_from,
};
use crate::sh_group::geometry_content_hash;

/// Cache stage id for the whole-section direct SH bake on the shared `StageCache`.
pub const DIRECT_SH_STAGE_ID: &str = "direct_sh_volume";

/// Bump when the direct SH bake's output computation changes (the D3 assembly,
/// the cull, the octahedral packing, or the section layout). Versions
/// independently from the indirect SH stages and from the section-internal
/// `DIRECT_SH_VOLUME_VERSION` (which guards the on-disk format).
pub const DIRECT_SH_STAGE_VERSION: u32 = 1;

const TILE_DIMENSION: u32 = DEFAULT_IRRADIANCE_TILE_DIMENSION;
const TILE_BORDER: u32 = DEFAULT_IRRADIANCE_TILE_BORDER;

/// Inputs for the direct-at-probe SH bake. Mirrors `delta_sh_bake::DeltaBakeInputs`
/// (same BVH/geometry/BSP tree + the portal graph the reach cull floods over),
/// but carries the FULL `ShBakeCtx` so the probe grid is byte-identical in
/// position/validity to the indirect bake (NO second grid).
pub struct DirectBakeInputs<'a, 'b> {
    pub sh_ctx: &'a ShBakeCtx<'b>,
    pub portals: &'a [Portal],
}

/// Build the empty (`grid_dimensions == [0,0,0]`) direct section for empty
/// geometry, matching the "no SH section" degradation path at runtime.
fn empty_section() -> DirectShVolumeSection {
    DirectShVolumeSection {
        grid_origin: [0.0, 0.0, 0.0],
        cell_size: [0.0, 0.0, 0.0],
        grid_dimensions: [0, 0, 0],
        tile_dimension: TILE_DIMENSION,
        tile_border: TILE_BORDER,
        atlas_dimensions: [0, 0],
        atlas_tiles_per_row: 0,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        atlas: Vec::new(),
    }
}

/// The static-direct light set: `StaticBakedLights` (global order) further
/// restricted to `shadow_type == StaticLightMap`. `Sdf`-typed lights are dropped
/// — their direct term is runtime-traced, so baking it here would double-count.
/// Animated/dynamic lights are already excluded by the `StaticBakedLights` filter.
/// Returns `(lights, global_indices)`: `global_indices[i]` is `lights[i]`'s
/// position in the full `static_lights` slice (the soft-visibility seed axis that
/// keeps culled and unculled bakes byte-identical).
fn static_direct_lights<'a>(static_lights: &[&'a MapLight]) -> (Vec<&'a MapLight>, Vec<u64>) {
    let mut lights = Vec::new();
    let mut global_indices = Vec::new();
    for (i, &light) in static_lights.iter().enumerate() {
        if light.shadow_type == ShadowType::StaticLightMap {
            lights.push(light);
            global_indices.push(i as u64);
        }
    }
    (lights, global_indices)
}

/// Per-probe reaching-light index lists, derived from the D9 two-stage reach test.
///
/// `decompose_affinity_for_lights` produces a per-LIGHT→affinity-cell CSR
/// (`per_light_cells`). We invert it ONCE into cell→light-list, then map each
/// probe to its affinity cell so the per-probe bake reads its cell's light list.
/// A light reaches a probe iff the light reaches the probe's affinity cell.
///
/// Indices in the returned lists index the `direct_lights` slice passed to
/// `decompose_affinity_for_lights` — i.e. the post-`static_direct_lights` set —
/// NOT the global `static_lights` slice. The caller maps them back to global
/// indices for the soft-visibility seed.
struct ReachIndex {
    /// `cell_lights[c]` is the ascending list of `direct_lights` indices reaching
    /// affinity cell `c` (linear x-fastest index in the affinity grid).
    cell_lights: Vec<Vec<u32>>,
    affinity_dims: [u32; 3],
}

impl ReachIndex {
    /// Affinity-cell linear index (x-fastest) for a probe at grid coords
    /// `(px, py, pz)`. One affinity cell spans `AFFINITY_FACTOR` probes per axis.
    fn cell_for_probe(&self, px: u32, py: u32, pz: u32) -> usize {
        let cx = px / AFFINITY_FACTOR;
        let cy = py / AFFINITY_FACTOR;
        let cz = pz / AFFINITY_FACTOR;
        let nx = self.affinity_dims[0] as usize;
        let ny = self.affinity_dims[1] as usize;
        cx as usize + cy as usize * nx + cz as usize * nx * ny
    }
}

/// Build the per-cell reaching-light index by running the reach decomposition over
/// `direct_lights` and inverting the per-light CSR into a cell-keyed one.
fn build_reach_index(
    inputs: &DirectBakeInputs<'_, '_>,
    direct_lights: &[&MapLight],
    probe_spacing: f32,
) -> ReachIndex {
    let geometry_vertices: Vec<[f32; 3]> = inputs
        .sh_ctx
        .geometry
        .geometry
        .vertices
        .iter()
        .map(|v| v.position)
        .collect();
    let reach = AffinityReachInputs {
        geometry_vertices: &geometry_vertices,
        tree: inputs.sh_ctx.tree,
        exterior_leaves: inputs.sh_ctx.exterior_leaves,
        portals: inputs.portals,
        probe_spacing,
    };
    let decomposition = decompose_affinity_for_lights(&reach, direct_lights);
    let cell_count = decomposition.affinity_cell_count();

    // Invert per_light_cells (light → cells) into cell → lights, once. Iterating
    // lights in ascending order keeps each cell's list ascending (deterministic).
    let mut cell_lights: Vec<Vec<u32>> = vec![Vec::new(); cell_count];
    for (light, cells) in decomposition.per_light_cells.iter().enumerate() {
        for &cell in cells {
            cell_lights[cell as usize].push(light as u32);
        }
    }

    ReachIndex {
        cell_lights,
        affinity_dims: decomposition.affinity_dims,
    }
}

/// Bake the dense direct SH atlas without touching the cache. Returns the
/// uncompressed-debug section (`IRRADIANCE_FORMAT_RGBA16F`); Task 3 BC6H-encodes
/// the atlas at emit time. Empty geometry yields the empty-section degradation
/// path (`grid_dimensions == [0,0,0]`).
pub fn bake_direct_sh_volume(
    inputs: &DirectBakeInputs<'_, '_>,
    config: &ShConfig,
) -> DirectShVolumeSection {
    let layout = probe_grid_layout(inputs.sh_ctx, config);
    if layout.is_empty() {
        return empty_section();
    }

    let static_lights = static_light_refs(inputs.sh_ctx);
    let (direct_lights, global_indices) = static_direct_lights(&static_lights);
    let reach = build_reach_index(inputs, &direct_lights, config.probe_spacing);

    let dims = layout.dims;
    let total = layout.total_probes();
    let nx = dims[0] as usize;
    let ny = dims[1] as usize;

    // Per-probe direct SH tiles, order-preserving fan-out (no float reduce, no
    // HashMap iteration) so the encoded atlas is byte-identical across runs.
    let tiles: Vec<Vec<OctahedralAtlasTexel>> = (0..total)
        .into_par_iter()
        .map(|probe_index| {
            bake_probe_tile(
                inputs,
                &layout,
                &direct_lights,
                &global_indices,
                &reach,
                probe_index,
                nx,
                ny,
            )
        })
        .collect();

    let atlas_dimensions = irradiance_atlas_dimensions(dims, TILE_DIMENSION);
    let atlas_tiles_per_row = irradiance_atlas_tiles_per_row(dims)
        .expect("non-empty SH probe grid should have a valid atlas tile row count");
    let atlas = pack_atlas(&tiles, atlas_dimensions, atlas_tiles_per_row);

    DirectShVolumeSection {
        grid_origin: [
            layout.world_min.x as f32,
            layout.world_min.y as f32,
            layout.world_min.z as f32,
        ],
        cell_size: layout.cell_size,
        grid_dimensions: dims,
        tile_dimension: TILE_DIMENSION,
        tile_border: TILE_BORDER,
        atlas_dimensions,
        atlas_tiles_per_row,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        atlas,
    }
}

/// Bake one probe's packed octahedral direct-SH tile. Invalid probes pack the
/// all-zero tile (matching the indirect bake). The probe's reaching-light set is
/// read from its affinity cell; culled lights never enter the radiance sum.
#[allow(clippy::too_many_arguments)]
fn bake_probe_tile(
    inputs: &DirectBakeInputs<'_, '_>,
    layout: &ProbeGridLayout,
    direct_lights: &[&MapLight],
    global_indices: &[u64],
    reach: &ReachIndex,
    probe_index: usize,
    nx: usize,
    ny: usize,
) -> Vec<OctahedralAtlasTexel> {
    let valid = layout.validity[probe_index] != 0;
    if !valid {
        return pack_octahedral_irradiance_tile(&[0.0; 27], false, TILE_DIMENSION, TILE_BORDER);
    }

    // Probe grid coords (z-major linear order, matching `sh_bake`).
    let pz = (probe_index / (nx * ny)) as u32;
    let rem = probe_index - pz as usize * nx * ny;
    let py = (rem / nx) as u32;
    let px = (rem - py as usize * nx) as u32;

    let cell = reach.cell_for_probe(px, py, pz);
    let reaching = reach
        .cell_lights
        .get(cell)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    // Gather the reaching subset (light refs + their global static_lights index).
    let lights: Vec<&MapLight> = reaching
        .iter()
        .map(|&i| direct_lights[i as usize])
        .collect();
    let seed_indices: Vec<u64> = reaching
        .iter()
        .map(|&i| global_indices[i as usize])
        .collect();

    let ctx = RaytracingCtx {
        bvh: inputs.sh_ctx.bvh,
        primitives: inputs.sh_ctx.primitives,
        geometry: inputs.sh_ctx.geometry,
    };
    let probe_pos = vec3_from(layout.probe_positions[probe_index]);
    let coefficients =
        bake_probe_direct_rgb(&ctx, probe_pos, &lights, &seed_indices, probe_index as u64);
    pack_octahedral_irradiance_tile(&coefficients, true, TILE_DIMENSION, TILE_BORDER)
}

/// Pack per-probe octahedral tiles into the dense near-square atlas, then
/// serialize to the row-major `Rgba16Float` byte blob (the uncompressed-debug
/// variant). Byte layout matches the indirect `OctahedralShVolumeSection` atlas
/// block, so Task 3's BC6H encoder reads a familiar input.
fn pack_atlas(
    tiles: &[Vec<OctahedralAtlasTexel>],
    atlas_dimensions: [u32; 2],
    atlas_tiles_per_row: u32,
) -> Vec<u8> {
    let atlas_texel_count = atlas_dimensions[0] as usize * atlas_dimensions[1] as usize;
    let mut atlas = vec![OctahedralAtlasTexel::default(); atlas_texel_count];
    for (probe_index, tile) in tiles.iter().enumerate() {
        let origin = irradiance_tile_origin(probe_index, TILE_DIMENSION, atlas_tiles_per_row);
        for tile_y in 0..TILE_DIMENSION {
            for tile_x in 0..TILE_DIMENSION {
                let texel = tile[(tile_y * TILE_DIMENSION + tile_x) as usize];
                let atlas_x = origin[0] + tile_x;
                let atlas_y = origin[1] + tile_y;
                let off = (atlas_y * atlas_dimensions[0] + atlas_x) as usize;
                atlas[off] = texel;
            }
        }
    }

    // Row-major f16×4 RGBA, little-endian — byte-identical to the indirect atlas
    // texel block. Stored verbatim under `irradiance_len`.
    let mut bytes = Vec::with_capacity(atlas_texel_count * 8);
    for texel in &atlas {
        for channel in &texel.rgba {
            bytes.extend_from_slice(&channel.to_le_bytes());
        }
    }
    bytes
}

// ---------------------------------------------------------------------------
// Cache

/// Single whole-section cache key for the direct SH bake. Folds the geometry
/// content hash, `probe_spacing`, the probe-grid layout sub-key (origin / cell
/// size / dims — the SAME grid-derivation input the indirect key uses), the
/// per-probe validity bytes (derived from `tree`/`exterior_leaves`, not covered by
/// the geometry hash), and the static-direct light set (postcard-encoded, in
/// global order, each paired with its global index). A grid/geometry/static-light
/// change invalidates this entry in lockstep with the indirect SH entries; the
/// indirect entry is otherwise untouched (distinct `stage_id`).
fn direct_cache_key(
    inputs: &DirectBakeInputs<'_, '_>,
    layout: &ProbeGridLayout,
    direct_lights: &[&MapLight],
    global_indices: &[u64],
    probe_spacing: f32,
    geom_hash: &[u8; 32],
) -> CacheKey {
    let mut hasher = blake3::Hasher::new();

    // Geometry content hash — SH shadow rays trace full geometry.
    hasher.update(geom_hash);

    // Probe-grid layout descriptor (origin / cell size / dims).
    hasher.update(&probe_spacing.to_le_bytes());
    for v in [layout.world_min.x, layout.world_min.y, layout.world_min.z] {
        hasher.update(&v.to_le_bytes());
    }
    for v in &layout.cell_size {
        hasher.update(&v.to_le_bytes());
    }
    for v in &layout.dims {
        hasher.update(&v.to_le_bytes());
    }

    // Per-probe validity (derives from tree / exterior_leaves, which the geometry
    // content hash does not cover — same rationale as the per-group SH key).
    for &v in &layout.validity {
        hasher.update(&[v]);
    }

    // Static-direct light set, in global order, each with its global index so a
    // reorder (which would change the per-probe sum order) invalidates.
    hasher.update(&(direct_lights.len() as u32).to_le_bytes());
    for (light, &global_index) in direct_lights.iter().zip(global_indices.iter()) {
        hasher.update(&(global_index as u32).to_le_bytes());
        let encoded = postcard::to_allocvec(light).expect("postcard serialize MapLight");
        hasher.update(&(encoded.len() as u32).to_le_bytes());
        hasher.update(&encoded);
    }

    // The portal graph drives the reach cull; fold the leaf-pair adjacency (the
    // only portal data the flood reads) so a portal change re-bakes. The portal
    // polygon geometry never affects the cull, so it is intentionally excluded.
    hasher.update(&(inputs.portals.len() as u32).to_le_bytes());
    for portal in inputs.portals {
        hasher.update(&(portal.front_leaf as u64).to_le_bytes());
        hasher.update(&(portal.back_leaf as u64).to_le_bytes());
    }

    let digest = hasher.finalize();
    CacheKey::new(
        DIRECT_SH_STAGE_ID,
        DIRECT_SH_STAGE_VERSION,
        digest.as_bytes(),
    )
}

/// Bake the direct SH section, going through the shared `StageCache` when one is
/// supplied. This is the SEAM Task 3 wires into `main.rs`: it produces the
/// section + atlas (uncompressed `IRRADIANCE_FORMAT_RGBA16F` variant) and handles
/// the whole-section cache get/put around the full bake. The cold `--no-cache`
/// path passes `cache == None` and runs the exact uncached bake (matching the
/// indirect cold path).
///
/// The cull is the STRICT provably-zero falloff+portal test in BOTH warm and cold
/// modes — it does NOT inherit warm SH's lossy bounded-light dilation — so the
/// bake output is byte-identical whether or not a cache is present.
pub fn bake_direct_sh_volume_cached(
    inputs: &DirectBakeInputs<'_, '_>,
    config: &ShConfig,
    cache: Option<&StageCache>,
) -> DirectShVolumeSection {
    let Some(cache) = cache else {
        return bake_direct_sh_volume(inputs, config);
    };

    let layout = probe_grid_layout(inputs.sh_ctx, config);
    if layout.is_empty() {
        // Empty geometry: nothing to cache; the section is trivial.
        return empty_section();
    }
    let static_lights = static_light_refs(inputs.sh_ctx);
    let (direct_lights, global_indices) = static_direct_lights(&static_lights);
    let geom_hash = geometry_content_hash(inputs.sh_ctx.geometry);
    let key = direct_cache_key(
        inputs,
        &layout,
        &direct_lights,
        &global_indices,
        config.probe_spacing,
        &geom_hash,
    );

    if let Some(bytes) = cache.get(&key) {
        match DirectShVolumeSection::from_bytes(&bytes) {
            Ok(section) => {
                log::info!("[cache] direct_sh_volume hit");
                return section;
            }
            Err(e) => {
                log::warn!("[cache] corrupt direct_sh_volume entry, re-baking: {e}");
            }
        }
    } else {
        log::info!("[cache] direct_sh_volume miss");
    }

    let section = bake_direct_sh_volume(inputs, config);
    cache.put(&key, &section.to_bytes());
    section
}

/// Log a per-light cull-savings summary mirroring `delta_sh_bake::log_per_light_culling`:
/// for each static-direct light, the probe count reachable through the cull (its
/// affinity cells × probes-per-cell) vs the full probe grid. Returns the number of
/// lights culled to zero probes (used by tests to assert AC 11's culled-count > 0).
pub fn log_cull_savings(inputs: &DirectBakeInputs<'_, '_>, config: &ShConfig) -> usize {
    let layout = probe_grid_layout(inputs.sh_ctx, config);
    if layout.is_empty() {
        return 0;
    }
    let static_lights = static_light_refs(inputs.sh_ctx);
    let (direct_lights, _) = static_direct_lights(&static_lights);
    let geometry_vertices: Vec<[f32; 3]> = inputs
        .sh_ctx
        .geometry
        .geometry
        .vertices
        .iter()
        .map(|v| v.position)
        .collect();
    let reach = AffinityReachInputs {
        geometry_vertices: &geometry_vertices,
        tree: inputs.sh_ctx.tree,
        exterior_leaves: inputs.sh_ctx.exterior_leaves,
        portals: inputs.portals,
        probe_spacing: config.probe_spacing,
    };
    let decomposition = decompose_affinity_for_lights(&reach, &direct_lights);

    let total_cells = decomposition.affinity_cell_count();
    let probes_per_cell = (AFFINITY_FACTOR * AFFINITY_FACTOR * AFFINITY_FACTOR) as usize;
    let full_probes = layout.total_probes();
    // A light is "culled" when the falloff-AABB + portal reach test drops at least
    // one affinity cell for it — the probes in the dropped cells are PROVABLY
    // zero-contribution (out of falloff range or unreachable through the portal
    // graph), so the baked coefficients match an unculled bake there exactly.
    let mut culled_count = 0usize;
    for (i, cells) in decomposition.per_light_cells.iter().enumerate() {
        let reach_probes = cells.len() * probes_per_cell;
        if cells.len() < total_cells {
            culled_count += 1;
        }
        log::info!(
            "[Compiler]   direct light {i}: reaches {} of {total_cells} cells (~{reach_probes} probes) vs {full_probes} grid probes",
            cells.len(),
        );
    }
    log::info!(
        "[Compiler] DirectShVolume cull: {} of {} static-direct light(s) had cells dropped by the reach test",
        culled_count,
        decomposition.per_light_cells.len(),
    );
    culled_count
}

// ---------------------------------------------------------------------------
// Emit-side BC6H encode (Task 3)

/// Byte size of the pre-compression dense RGBA-f32 buffer that
/// [`encode_direct_section_bc6h`] feeds to the BC6H encoder for a section with
/// the given (padded) atlas dimensions: `padded_w · padded_h · 16` bytes (4 f32
/// channels per texel). This is the AC-14 "pre-compression dense" figure, and it
/// is always defined — even on the debug bypass — so the footprint log can report
/// it alongside the post-compression size.
pub fn direct_dense_atlas_byte_size(section: &DirectShVolumeSection) -> usize {
    let (padded_w, padded_h) = bc6h_padded_atlas_dimensions(section.atlas_dimensions);
    padded_w as usize * padded_h as usize * 16
}

/// Round each atlas axis up to the next multiple of 4 so the BC6H encoder's
/// `≥4 / 4-aligned` rule holds. The LOGICAL atlas geometry (`atlas_dimensions`)
/// is unchanged — only the encoded buffer is padded — mirroring the lightmap
/// atlas builder's power-of-two ≥64 rounding (per Task 1's noted approach). Empty
/// grids yield `[0, 0]`; the caller short-circuits those.
fn bc6h_padded_atlas_dimensions(atlas_dimensions: [u32; 2]) -> (u32, u32) {
    (
        atlas_dimensions[0].div_ceil(4) * 4,
        atlas_dimensions[1].div_ceil(4) * 4,
    )
}

/// Re-encode the Task-2 `IRRADIANCE_FORMAT_RGBA16F` direct section into the
/// production `IRRADIANCE_FORMAT_BC6H` at-rest section (mirroring
/// `lightmap_bake::CompositedAtlas::encode_section`'s BC6H route).
///
/// The atlas axes (multiples of the tile dimension, 6) are NOT guaranteed
/// 4-aligned, so the encoded buffer is padded up to a multiple of 4 per axis
/// before encoding; the padded fringe carries zeros (decoded but never sampled —
/// the logical tile geometry stays at `atlas_dimensions`). The stored blob is the
/// padded BC6H block payload, carried verbatim in `irradiance_len`.
///
/// When `uncompressed_irradiance` is set (the same debug bypass the lightmap path
/// honors, for A/B + determinism baselines) the RGBA16F section is returned as-is.
/// Empty sections (no probe grid) pass through unchanged — there is nothing to
/// compress.
pub fn encode_direct_section_bc6h(
    section: &DirectShVolumeSection,
    uncompressed_irradiance: bool,
) -> DirectShVolumeSection {
    if uncompressed_irradiance || section.grid_dimensions == [0, 0, 0] {
        return section.clone();
    }
    debug_assert_eq!(
        section.irradiance_format, IRRADIANCE_FORMAT_RGBA16F,
        "direct BC6H encode expects the uncompressed RGBA16F section from the bake",
    );

    let aw = section.atlas_dimensions[0];
    let ah = section.atlas_dimensions[1];
    let (padded_w, padded_h) = bc6h_padded_atlas_dimensions(section.atlas_dimensions);

    // Decode the row-major f16×4 RGBA atlas into the padded RGBA-f32 buffer the
    // encoder expects (`padded_w · padded_h · 4` floats). The padded fringe stays
    // zero. Source texels are 8 bytes (4 × f16); RGB feeds the encoder, A drops.
    let mut rgba_f32 = vec![0.0f32; padded_w as usize * padded_h as usize * 4];
    for y in 0..ah {
        for x in 0..aw {
            let src = ((y * aw + x) * 8) as usize;
            let dst = ((y * padded_w + x) * 4) as usize;
            for c in 0..4 {
                let bits = u16::from_le_bytes([
                    section.atlas[src + c * 2],
                    section.atlas[src + c * 2 + 1],
                ]);
                rgba_f32[dst + c] = crate::sh_bake::f16_bits_to_f32(bits);
            }
        }
    }

    let bc6h_bytes = bc6h::encode_bc6h_rgb_from_f32_rgba(&rgba_f32, padded_w, padded_h);

    DirectShVolumeSection {
        irradiance_format: IRRADIANCE_FORMAT_BC6H,
        atlas: bc6h_bytes,
        ..section.clone()
    }
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh_build::build_bvh;
    use crate::geometry::{FaceIndexRange, GeometryResult};
    use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
    use crate::map_data::{FalloffModel, LightAnimation, LightType, ShadowType};
    use crate::partition::{Aabb as CompilerAabb, BspChild, BspLeaf, BspNode, BspTree};
    use crate::portals::Portal;
    use glam::DVec3;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::octahedral::irradiance_interior_texel_direction;
    use postretro_level_format::texture_names::TextureNamesSection;
    use std::collections::HashSet;

    fn tri_vertex(pos: [f32; 3]) -> Vertex {
        Vertex::new(
            pos,
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
        )
    }

    fn multi_triangle_geometry(triangles: &[[[f32; 3]; 3]]) -> GeometryResult {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut faces = Vec::new();
        let mut face_index_ranges = Vec::new();
        for (i, tri) in triangles.iter().enumerate() {
            let base = (i * 3) as u32;
            for &p in tri {
                vertices.push(tri_vertex(p));
            }
            indices.extend_from_slice(&[base, base + 1, base + 2]);
            faces.push(FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            });
            face_index_ranges.push(FaceIndexRange {
                index_offset: base,
                index_count: 3,
            });
        }
        GeometryResult {
            geometry: GeometrySection {
                vertices,
                indices,
                faces,
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges,
        }
    }

    /// Open-topped box (floor + three walls) spanning ~4 m → a 5×4×5 probe grid at
    /// 1 m spacing, enough work for rayon to schedule across several threads.
    fn floor_and_walls_geometry() -> GeometryResult {
        let floor_a = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 0.0, 4.0]];
        let floor_b = [[0.0, 0.0, 0.0], [4.0, 0.0, 4.0], [0.0, 0.0, 4.0]];
        let wall_near = [[0.0, 0.0, 0.0], [0.0, 0.0, 4.0], [0.0, 3.0, 0.0]];
        let wall_far = [[4.0, 0.0, 0.0], [4.0, 0.0, 4.0], [4.0, 3.0, 0.0]];
        let wall_side = [[0.0, 0.0, 4.0], [4.0, 0.0, 4.0], [0.0, 3.0, 4.0]];
        multi_triangle_geometry(&[floor_a, floor_b, wall_near, wall_far, wall_side])
    }

    /// A long floor strip spanning `len` m along x (3 m wide along z). At 1 m
    /// spacing the grid is `(len+1)×1×4`, so affinity cells (4 probes each) cleanly
    /// separate distant x-regions — coarse cells don't straddle a mid-strip split.
    fn long_floor_geometry(len: f32) -> GeometryResult {
        let floor_a = [[0.0, 0.0, 0.0], [len, 0.0, 0.0], [len, 0.0, 3.0]];
        let floor_b = [[0.0, 0.0, 0.0], [len, 0.0, 3.0], [0.0, 0.0, 3.0]];
        multi_triangle_geometry(&[floor_a, floor_b])
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

    fn static_point_light(origin: DVec3, range: f32, color: [f32; 3]) -> MapLight {
        MapLight {
            origin,
            light_type: LightType::Point,
            intensity: 2.0,
            color,
            falloff_model: FalloffModel::Linear,
            falloff_range: range,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            cast_shadows: true,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    /// Decode an RGBA16F atlas blob back into f16-bit RGBA texels.
    fn decode_atlas(section: &DirectShVolumeSection) -> Vec<[u16; 4]> {
        section
            .atlas
            .chunks_exact(8)
            .map(|c| {
                [
                    u16::from_le_bytes([c[0], c[1]]),
                    u16::from_le_bytes([c[2], c[3]]),
                    u16::from_le_bytes([c[4], c[5]]),
                    u16::from_le_bytes([c[6], c[7]]),
                ]
            })
            .collect()
    }

    /// Decode f16 bits → f32 (finite non-negative magnitudes; the irradiance
    /// atlas is always non-negative).
    fn f16_to_f32(bits: u16) -> f32 {
        crate::sh_bake::f16_bits_to_f32(bits)
    }

    /// Reconstruct direct irradiance at a probe for a receiver normal `n` by
    /// sampling the nearest interior octahedral texel whose direction is closest
    /// to `n`. Returns the per-channel f32 irradiance.
    fn reconstruct_irradiance(
        section: &DirectShVolumeSection,
        atlas: &[[u16; 4]],
        probe_index: usize,
        n: glam::Vec3,
    ) -> glam::Vec3 {
        let origin = irradiance_tile_origin(
            probe_index,
            section.tile_dimension,
            section.atlas_tiles_per_row,
        );
        let interior = section.tile_dimension - 2 * section.tile_border;
        let aw = section.atlas_dimensions[0] as usize;
        let mut best_dot = f32::NEG_INFINITY;
        let mut best = glam::Vec3::ZERO;
        for iy in 0..interior {
            for ix in 0..interior {
                let dir = glam::Vec3::from(irradiance_interior_texel_direction(
                    ix,
                    iy,
                    section.tile_dimension,
                    section.tile_border,
                ));
                let d = dir.dot(n);
                if d > best_dot {
                    best_dot = d;
                    let ax = origin[0] + section.tile_border + ix;
                    let ay = origin[1] + section.tile_border + iy;
                    let texel = atlas[ay as usize * aw + ax as usize];
                    best = glam::Vec3::new(
                        f16_to_f32(texel[0]),
                        f16_to_f32(texel[1]),
                        f16_to_f32(texel[2]),
                    );
                }
            }
        }
        best
    }

    fn flat_index(x: usize, y: usize, z: usize, dims: [u32; 3]) -> usize {
        z * dims[0] as usize * dims[1] as usize + y * dims[0] as usize + x
    }

    /// AC 9: byte-identical output across two runs on identical inputs.
    #[test]
    fn direct_sh_bake_produces_byte_identical_output_on_repeated_runs() {
        let geo = floor_and_walls_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights = vec![
            static_point_light(DVec3::new(0.5, 1.0, 0.5), 8.0, [1.0, 0.5, 0.25]),
            static_point_light(DVec3::new(3.0, 2.0, 3.0), 8.0, [0.25, 0.5, 1.0]),
        ];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let animated_lights = AnimatedBakedLights::from_lights(&lights);
        let sh_ctx = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: lights.len(),
        };
        let inputs = DirectBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
        };
        let config = ShConfig { probe_spacing: 1.0 };

        let a = bake_direct_sh_volume(&inputs, &config).to_bytes();
        let b = bake_direct_sh_volume(&inputs, &config).to_bytes();
        assert_eq!(
            a, b,
            "direct SH bake output drifted between runs; the build-stage cache requires \
             byte-identical output for identical inputs",
        );
    }

    /// AC 11: culling drops at least one provably-zero light, AND culled vs
    /// unculled bytes are EQUAL.
    ///
    /// The two bakes share the SAME compact geometry and the SAME two-leaf BSP
    /// split at x=2. They differ ONLY in the portal list: the unculled reference
    /// adds a portal connecting the two leaves (so the reach flood keeps every
    /// cell and every light is considered at every probe), while the culled bake
    /// has no portals (leaf-1-only lights drop from leaf-0 cells). The bytes must
    /// still match because every culled light contributes exactly zero where it
    /// was dropped: the far out-of-range light is zero by falloff everywhere, and
    /// the +x leaf-1 light's short falloff range does not reach any leaf-0 probe,
    /// so even when the unculled reference considers it there it adds zero. A far
    /// out-of-range light is culled to zero cells by the falloff AABB, giving
    /// `culled_count > 0`.
    ///
    /// Leaning byte-equality on falloff (rather than occlusion) keeps the
    /// provably-zero guarantee robust against grazing rays around a finite wall,
    /// while still exercising the portal reach path through
    /// `decompose_affinity_for_lights`.
    #[test]
    fn direct_sh_culled_equals_unculled_and_culls_at_least_one_light() {
        // A 16 m floor strip → a 17×1×4 grid, affinity cells 5×1×1 (4 probes per
        // x-cell). The split at x=12 sits on the cx=3 boundary, so the coarse
        // cells separate the two leaves cleanly (no straddling cell entangles
        // leaf-0 and leaf-1 probes).
        let geo = long_floor_geometry(16.0);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let exterior: HashSet<usize> = HashSet::new();

        // A reaching light near x=2, a far out-of-range light (its clamped AABB
        // covers far fewer than all cells → dropped cells), and a +x (leaf 1)
        // light at x=14 whose short range (1.5) never reaches any leaf-0 probe —
        // dropped from leaf-0 cells by portal reachability in the culled bake, and
        // contributing zero there by falloff in the unculled reference.
        let lights = vec![
            static_point_light(DVec3::new(2.0, 1.5, 1.5), 8.0, [1.0, 0.8, 0.6]),
            static_point_light(DVec3::new(500.0, 500.0, 500.0), 2.0, [1.0, 1.0, 1.0]),
            static_point_light(DVec3::new(14.0, 1.5, 1.5), 1.5, [0.5, 0.5, 1.0]),
        ];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let animated_lights = AnimatedBakedLights::from_lights(&lights);

        // Shared BSP split at x=12: back (x<12) = leaf 0, front (x>12) = leaf 1.
        let tree = BspTree {
            nodes: vec![BspNode {
                plane_normal: DVec3::X,
                plane_distance: 12.0,
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
        let sh_ctx = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: lights.len(),
        };
        let config = ShConfig { probe_spacing: 1.0 };

        // Culled bake: NO portals → leaf-1-only lights drop from leaf-0 cells.
        let inputs_culled = DirectBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
        };
        // Unculled reference: a portal connects the two leaves → the reach flood
        // keeps every cell, so every light is considered at every probe. The
        // portal polygon is irrelevant to the cull (only the leaf pair matters).
        let portals = vec![Portal {
            polygon: Vec::new(),
            front_leaf: 0,
            back_leaf: 1,
        }];
        let inputs_unculled = DirectBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &portals,
        };

        let culled_count = log_cull_savings(&inputs_culled, &config);
        assert!(
            culled_count > 0,
            "AC 11 requires the cull scene to drop at least one provably-zero light",
        );

        let culled = bake_direct_sh_volume(&inputs_culled, &config).to_bytes();
        let unculled = bake_direct_sh_volume(&inputs_unculled, &config).to_bytes();
        assert_eq!(
            culled, unculled,
            "culled vs unculled baked bytes must be equal — a culled light is \
             provably zero-contribution at the probes it was dropped from",
        );
    }

    /// AC 2: a shadowed probe bakes strictly less direct irradiance than a lit
    /// probe. A solid wall between a light and one probe occludes its direct term.
    #[test]
    fn direct_sh_shadowed_probe_is_dimmer_than_lit_probe() {
        // Floor plus a solid vertical wall at x=2 spanning the box, so a light on
        // the -x side directly lights probes there but is occluded for +x probes.
        let floor_a = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 0.0, 4.0]];
        let floor_b = [[0.0, 0.0, 0.0], [4.0, 0.0, 4.0], [0.0, 0.0, 4.0]];
        // Tall wall at x=2 (two triangles), blocking the light from the +x side.
        let wall_a = [[2.0, 0.0, 0.0], [2.0, 0.0, 4.0], [2.0, 3.0, 0.0]];
        let wall_b = [[2.0, 3.0, 0.0], [2.0, 0.0, 4.0], [2.0, 3.0, 4.0]];
        let geo = multi_triangle_geometry(&[floor_a, floor_b, wall_a, wall_b]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();

        // Light on the -x side, high enough to be above the floor but the wall
        // shadows the +x side. Large range so falloff doesn't confound the test.
        let lights = vec![static_point_light(
            DVec3::new(0.5, 2.0, 2.0),
            50.0,
            [1.0, 1.0, 1.0],
        )];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let animated_lights = AnimatedBakedLights::from_lights(&lights);
        let sh_ctx = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: lights.len(),
        };
        let inputs = DirectBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
        };
        let config = ShConfig { probe_spacing: 1.0 };
        let section = bake_direct_sh_volume(&inputs, &config);
        let atlas = decode_atlas(&section);
        let dims = section.grid_dimensions;

        // Lit probe: -x side, near the light. Shadowed probe: +x side, behind the
        // wall. Both one step off the floor (y=1) so neither is invalid.
        let lit = flat_index(1, 1, 2, dims);
        let shadowed = flat_index(3, 1, 2, dims);
        assert_eq!(section.tile_dimension, TILE_DIMENSION);

        // Mid-cone receiver normal (per Task 0 outcome — never the exact lobe
        // axis). Point the normal generally toward the light (-x, +y) for both.
        let n = glam::Vec3::new(-0.6, 0.8, 0.0).normalize();
        let lit_irr = reconstruct_irradiance(&section, &atlas, lit, n);
        let shadowed_irr = reconstruct_irradiance(&section, &atlas, shadowed, n);

        assert!(
            lit_irr.length() > 0.0,
            "lit probe must receive direct light, got {lit_irr}",
        );
        assert!(
            shadowed_irr.length() < lit_irr.length(),
            "shadowed probe ({shadowed_irr}) must be dimmer than lit probe ({lit_irr})",
        );
    }

    /// AC 7 support: the static-direct filter excludes animated and dynamic
    /// lights (and `Sdf`-typed lights, whose direct term is runtime-traced).
    #[test]
    fn static_direct_filter_excludes_animated_dynamic_and_sdf() {
        let geo = floor_and_walls_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();

        // 0: static StaticLightMap (kept). 1: animated (excluded by StaticBakedLights).
        // 2: dynamic (excluded by StaticBakedLights). 3: Sdf static (excluded by
        // the direct shadow-type filter — runtime-traced direct).
        let mut animated = static_point_light(DVec3::new(1.0, 1.0, 1.0), 8.0, [1.0; 3]);
        animated.animation = Some(LightAnimation {
            period: 1.0,
            phase: 0.0,
            brightness: Some(vec![0.0, 1.0]),
            color: None,
            direction: None,
            start_active: true,
        });
        let mut dynamic = static_point_light(DVec3::new(2.0, 1.0, 2.0), 8.0, [1.0; 3]);
        dynamic.is_dynamic = true;
        let mut sdf = static_point_light(DVec3::new(3.0, 1.0, 3.0), 8.0, [1.0; 3]);
        sdf.shadow_type = ShadowType::Sdf;

        let lights = vec![
            static_point_light(DVec3::new(0.5, 1.0, 0.5), 8.0, [1.0; 3]),
            animated,
            dynamic,
            sdf,
        ];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let animated_lights = AnimatedBakedLights::from_lights(&lights);
        let sh_ctx = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: lights.len(),
        };
        let static_refs = static_light_refs(&sh_ctx);

        // StaticBakedLights already drops animated (1) and dynamic (2): refs = [0, 3].
        assert_eq!(
            static_refs.len(),
            2,
            "StaticBakedLights must drop animated + dynamic"
        );

        // The direct filter then drops the Sdf light, leaving only light 0, and its
        // global index must remain its position in the static_lights slice (0).
        let (direct_lights, global_indices) = static_direct_lights(&static_refs);
        assert_eq!(
            direct_lights.len(),
            1,
            "direct filter must drop the Sdf light"
        );
        assert_eq!(
            global_indices,
            vec![0],
            "kept light keeps its global static index"
        );
    }

    /// Empty geometry yields the empty-section degradation path.
    #[test]
    fn empty_geometry_returns_empty_direct_section() {
        let geo = GeometryResult {
            geometry: GeometrySection {
                vertices: Vec::new(),
                indices: Vec::new(),
                faces: Vec::new(),
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: Vec::new(),
        };
        let bvh = bvh::bvh::Bvh { nodes: Vec::new() };
        let prims: Vec<crate::bvh_build::BvhPrimitive> = Vec::new();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights: &[MapLight] = &[];
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let sh_ctx = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        let inputs = DirectBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
        };
        let section = bake_direct_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
        assert_eq!(section.grid_dimensions, [0, 0, 0]);
        assert_eq!(section.atlas_dimensions, [0, 0]);
        assert!(section.atlas.is_empty());
    }

    /// AC 13: the production emit produces a BC6H-tagged section whose atlas blob
    /// is the padded 4×4-block payload, while the logical tile geometry is
    /// unchanged. Decoding the blocks back reproduces the bake's irradiance within
    /// the BC6H round-trip tolerance.
    #[test]
    fn encode_direct_section_bc6h_tags_and_pads_to_block_size() {
        let geo = floor_and_walls_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights = vec![static_point_light(
            DVec3::new(2.0, 1.5, 2.0),
            8.0,
            [1.0, 0.8, 0.6],
        )];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let animated_lights = AnimatedBakedLights::from_lights(&lights);
        let sh_ctx = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: lights.len(),
        };
        let inputs = DirectBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
        };
        let config = ShConfig { probe_spacing: 1.0 };

        let raw = bake_direct_sh_volume(&inputs, &config);
        assert_eq!(raw.irradiance_format, IRRADIANCE_FORMAT_RGBA16F);

        let encoded = encode_direct_section_bc6h(&raw, false);
        assert_eq!(encoded.irradiance_format, IRRADIANCE_FORMAT_BC6H);
        // Logical tile geometry is unchanged — only the encoded buffer is padded.
        assert_eq!(encoded.grid_dimensions, raw.grid_dimensions);
        assert_eq!(encoded.atlas_dimensions, raw.atlas_dimensions);
        assert_eq!(encoded.atlas_tiles_per_row, raw.atlas_tiles_per_row);

        // Blob length equals the padded 4×4-block payload size.
        let padded_w = raw.atlas_dimensions[0].div_ceil(4) * 4;
        let padded_h = raw.atlas_dimensions[1].div_ceil(4) * 4;
        let expected_len = (padded_w / 4) as usize * (padded_h / 4) as usize * 16;
        assert_eq!(encoded.atlas.len(), expected_len);

        // The encoded section round-trips through the format codec.
        let restored = DirectShVolumeSection::from_bytes(&encoded.to_bytes()).unwrap();
        assert_eq!(restored, encoded);
    }

    /// The debug bypass returns the RGBA16F section verbatim (A/B + determinism
    /// baseline), and the pre-compression dense figure is defined either way.
    #[test]
    fn encode_direct_section_bc6h_debug_bypass_is_passthrough() {
        let geo = floor_and_walls_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights = vec![static_point_light(
            DVec3::new(2.0, 1.5, 2.0),
            8.0,
            [1.0, 0.8, 0.6],
        )];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let animated_lights = AnimatedBakedLights::from_lights(&lights);
        let sh_ctx = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: lights.len(),
        };
        let inputs = DirectBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
        };
        let config = ShConfig { probe_spacing: 1.0 };

        let raw = bake_direct_sh_volume(&inputs, &config);
        let bypassed = encode_direct_section_bc6h(&raw, true);
        assert_eq!(
            bypassed, raw,
            "debug bypass must return the RGBA16F section as-is"
        );

        // Pre-compression dense figure is the padded RGBA-f32 buffer size.
        let padded_w = raw.atlas_dimensions[0].div_ceil(4) * 4;
        let padded_h = raw.atlas_dimensions[1].div_ceil(4) * 4;
        assert_eq!(
            direct_dense_atlas_byte_size(&raw),
            padded_w as usize * padded_h as usize * 16,
        );
    }

    /// An empty section (no probe grid) passes through the encoder untouched —
    /// there is nothing to compress.
    #[test]
    fn encode_direct_section_bc6h_empty_section_passthrough() {
        let empty = empty_section();
        let encoded = encode_direct_section_bc6h(&empty, false);
        assert_eq!(encoded, empty);
        assert_eq!(direct_dense_atlas_byte_size(&empty), 0);
    }

    /// Build the padded *physical* RGBA-f32 atlas the BC6H emitter/uploader feeds
    /// the GPU: each axis padded up to a multiple of 4 (`bc6h_padded_atlas_dimensions`),
    /// tiles at the SAME texel positions as the logical atlas, fringe zeroed. This
    /// mirrors `encode_direct_section_bc6h`'s decode-into-padded-buffer step (without
    /// the lossy BC6H block round-trip, so the comparison isolates the layout/UV
    /// bug rather than codec error). Returns `(rgba_f32, padded_w, padded_h)`.
    fn build_padded_physical_atlas(section: &DirectShVolumeSection) -> (Vec<f32>, u32, u32) {
        let aw = section.atlas_dimensions[0];
        let ah = section.atlas_dimensions[1];
        let (padded_w, padded_h) = bc6h_padded_atlas_dimensions(section.atlas_dimensions);
        let mut rgba = vec![0.0f32; padded_w as usize * padded_h as usize * 4];
        for y in 0..ah {
            for x in 0..aw {
                let src = ((y * aw + x) * 8) as usize;
                let dst = ((y * padded_w + x) * 4) as usize;
                for c in 0..4 {
                    let bits = u16::from_le_bytes([
                        section.atlas[src + c * 2],
                        section.atlas[src + c * 2 + 1],
                    ]);
                    rgba[dst + c] = f16_to_f32(bits);
                }
            }
        }
        (rgba, padded_w, padded_h)
    }

    /// Model the WGSL sampler's UV math (`sh_sample.wgsl::sample_probe_atlas_tex`)
    /// for an interior texel direction, reading the padded physical buffer the GPU
    /// actually samples. `divisor` is the atlas extent used to normalize the UV:
    /// the SHADER FIX passes the buffer's PHYSICAL dims (`textureDimensions`); the
    /// BUG passed the LOGICAL `atlas_dimensions`.
    ///
    /// For an interior texel center, `oct_encode(dir) * interior == interior_xy + 0.5`,
    /// so the shader's continuous `texel` coordinate lands exactly on the center of
    /// atlas pixel `(origin + border + interior_xy)`. A bilinear tap at an exact
    /// texel center returns that texel, so we recover the sampled texel by
    /// `floor(uv * physical_dims)` — the integer pixel the center UV resolves to.
    fn sample_padded_like_shader(
        section: &DirectShVolumeSection,
        rgba: &[f32],
        padded_w: u32,
        padded_h: u32,
        probe_index: usize,
        n: glam::Vec3,
    ) -> glam::Vec3 {
        let origin = irradiance_tile_origin(
            probe_index,
            section.tile_dimension,
            section.atlas_tiles_per_row,
        );
        let interior = section.tile_dimension - 2 * section.tile_border;
        // Pick the interior texel whose direction best matches `n` (same selection
        // the logical-layout `reconstruct_irradiance` uses, so the two reads target
        // the same tile texel — only the divisor under test differs).
        let mut best_dot = f32::NEG_INFINITY;
        let mut best_ix = 0u32;
        let mut best_iy = 0u32;
        for iy in 0..interior {
            for ix in 0..interior {
                let dir = glam::Vec3::from(irradiance_interior_texel_direction(
                    ix,
                    iy,
                    section.tile_dimension,
                    section.tile_border,
                ));
                let d = dir.dot(n);
                if d > best_dot {
                    best_dot = d;
                    best_ix = ix;
                    best_iy = iy;
                }
            }
        }

        // Shader's continuous texel-center coordinate for that interior texel.
        let texel_x = origin[0] as f32 + section.tile_border as f32 + best_ix as f32 + 0.5;
        let texel_y = origin[1] as f32 + section.tile_border as f32 + best_iy as f32 + 0.5;

        // Normalize by the divisor under test, then resolve to the physical pixel
        // the center UV lands on (matching the GPU's bilinear-at-center behavior).
        let uv = glam::Vec2::new(texel_x / padded_w as f32, texel_y / padded_h as f32);
        let px = (uv.x * padded_w as f32).floor() as i64;
        let py = (uv.y * padded_h as f32).floor() as i64;
        let px = px.clamp(0, padded_w as i64 - 1) as usize;
        let py = py.clamp(0, padded_h as i64 - 1) as usize;
        let off = (py * padded_w as usize + px) * 4;
        glam::Vec3::new(rgba[off], rgba[off + 1], rgba[off + 2])
    }

    /// Same as `sample_padded_like_shader` but normalizes the UV by the LOGICAL
    /// `atlas_dimensions` (the BUG) while still reading the PHYSICAL buffer — the
    /// exact mismatch the shader fix removed. Returns the texel that the stretched
    /// UV resolves to in the padded buffer.
    fn sample_padded_with_logical_divisor(
        section: &DirectShVolumeSection,
        rgba: &[f32],
        padded_w: u32,
        padded_h: u32,
        probe_index: usize,
        n: glam::Vec3,
    ) -> glam::Vec3 {
        let origin = irradiance_tile_origin(
            probe_index,
            section.tile_dimension,
            section.atlas_tiles_per_row,
        );
        let interior = section.tile_dimension - 2 * section.tile_border;
        let mut best_dot = f32::NEG_INFINITY;
        let mut best_ix = 0u32;
        let mut best_iy = 0u32;
        for iy in 0..interior {
            for ix in 0..interior {
                let dir = glam::Vec3::from(irradiance_interior_texel_direction(
                    ix,
                    iy,
                    section.tile_dimension,
                    section.tile_border,
                ));
                let d = dir.dot(n);
                if d > best_dot {
                    best_dot = d;
                    best_ix = ix;
                    best_iy = iy;
                }
            }
        }
        let texel_x = origin[0] as f32 + section.tile_border as f32 + best_ix as f32 + 0.5;
        let texel_y = origin[1] as f32 + section.tile_border as f32 + best_iy as f32 + 0.5;
        let aw = section.atlas_dimensions[0] as f32;
        let ah = section.atlas_dimensions[1] as f32;
        // BUG: divide by logical, sample physical → progressive stretch.
        let uv = glam::Vec2::new(texel_x / aw, texel_y / ah);
        let px = ((uv.x * padded_w as f32).floor() as i64).clamp(0, padded_w as i64 - 1) as usize;
        let py = ((uv.y * padded_h as f32).floor() as i64).clamp(0, padded_h as i64 - 1) as usize;
        let off = (py * padded_w as usize + px) * 4;
        glam::Vec3::new(rgba[off], rgba[off + 1], rgba[off + 2])
    }

    /// Regression (lighting--entity-direct-sh): the direct BC6H atlas is uploaded at
    /// a 4-padded PHYSICAL extent, but the shared sampler normalized the UV by the
    /// LOGICAL `sh_grid.atlas_dimensions` (multiples of tile-dim 6, almost never
    /// 4-aligned). That divides the texel coordinate by the wrong extent, so a
    /// logical texel `t` was sampled at physical `t·(padded/logical)` — a progressive
    /// stretch that pulled the zeroed fringe into the tap and broke brightness parity
    /// (AC 1). The fix normalizes by the atlas's OWN physical extent
    /// (`textureDimensions`).
    ///
    /// This test models the shader's UV math on the as-uploaded padded physical
    /// buffer: reading it with the SAMPLER's physical-extent normalization recovers
    /// the same irradiance as the logical-layout reconstruction, AND the
    /// logical-divisor read (the bug) DIVERGES — pinning the regression.
    #[test]
    fn direct_sh_padded_physical_sample_matches_logical_reconstruction() {
        // A 16 m floor strip → a 17×1×4 = 68-probe grid → ceil_sqrt(68) = 9 tiles per
        // row → logical atlas axis 9·6 = 54, which is NOT 4-aligned (54 % 4 == 2), so
        // the BC6H emitter pads it to 56 and the physical extent strictly exceeds the
        // logical — the precondition that triggered the stretch.
        let geo = long_floor_geometry(16.0);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights = vec![static_point_light(
            DVec3::new(8.0, 2.0, 1.5),
            50.0,
            [1.0, 0.8, 0.6],
        )];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let animated_lights = AnimatedBakedLights::from_lights(&lights);
        let sh_ctx = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: lights.len(),
        };
        let inputs = DirectBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
        };
        let config = ShConfig { probe_spacing: 1.0 };

        let section = bake_direct_sh_volume(&inputs, &config);
        let logical_atlas = decode_atlas(&section);

        // Precondition: the padded physical extent must exceed the logical extent on
        // at least one axis, or there is no stretch to catch.
        let (padded_w, padded_h) = bc6h_padded_atlas_dimensions(section.atlas_dimensions);
        assert!(
            padded_w > section.atlas_dimensions[0] || padded_h > section.atlas_dimensions[1],
            "test scene must produce a non-4-aligned logical atlas so the physical \
             padding differs (logical {:?}, padded {padded_w}x{padded_h})",
            section.atlas_dimensions,
        );

        let (padded_rgba, pw, ph) = build_padded_physical_atlas(&section);
        assert_eq!((pw, ph), (padded_w, padded_h));

        let dims = section.grid_dimensions;
        // A lit probe with a HIGH linear index (large x), so its tile sits far from
        // the atlas origin where the progressive stretch is largest — the buggy
        // logical-divisor read lands on a different physical pixel there. Light is
        // overhead at x=8, z=1.5; pick a probe directly under it.
        let probe = flat_index(8, 0, 1, dims);

        // Mid-cone receiver normal (~20°-45° off the probe→light axis, per Task 0 —
        // NOT the exact lobe peak, which carries ~6.25% intrinsic L2 overshoot). The
        // light is overhead (+y) and slightly +z of the probe.
        let n = glam::Vec3::new(0.0, 0.85, 0.5).normalize();

        let logical = reconstruct_irradiance(&section, &logical_atlas, probe, n);
        assert!(
            logical.length() > 1.0e-3,
            "lit probe must carry direct irradiance for the comparison to be meaningful, got {logical}",
        );

        // The FIX: physical-extent normalization recovers the logical-layout value.
        let physical = sample_padded_like_shader(&section, &padded_rgba, pw, ph, probe, n);
        let eps = 1.0e-4_f32;
        assert!(
            (physical - logical).length() <= eps,
            "physical-divisor sample {physical} of the padded buffer must match the \
             logical-layout reconstruction {logical} within {eps} (the shader fix)",
        );

        // The BUG: logical-divisor normalization of the padded buffer DIVERGES. This
        // pins the regression — if someone reintroduced `sh_grid.atlas_dimensions` as
        // the direct divisor, this assertion would fail.
        let bugged = sample_padded_with_logical_divisor(&section, &padded_rgba, pw, ph, probe, n);
        assert!(
            (bugged - logical).length() > eps,
            "logical-divisor sample {bugged} of the padded buffer must DIVERGE from the \
             correct reconstruction {logical} (the stretch the fix removed)",
        );
    }

    /// Cache round-trip: a second build with a warm cache reproduces the first
    /// build byte-for-byte, and matches the uncached bake.
    #[test]
    fn direct_sh_cache_round_trip_matches_uncached() {
        let geo = floor_and_walls_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let tree = tree_all_empty();
        let exterior: HashSet<usize> = HashSet::new();
        let lights = vec![static_point_light(
            DVec3::new(2.0, 1.5, 2.0),
            8.0,
            [1.0, 0.8, 0.6],
        )];
        let static_lights = StaticBakedLights::from_lights(&lights);
        let animated_lights = AnimatedBakedLights::from_lights(&lights);
        let sh_ctx = ShBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: lights.len(),
        };
        let inputs = DirectBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
        };
        let config = ShConfig { probe_spacing: 1.0 };

        let dir = std::env::temp_dir().join(format!(
            "postretro_direct_sh_cache_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = StageCache::new(&dir).expect("cache dir");

        let uncached = bake_direct_sh_volume(&inputs, &config).to_bytes();
        let first = bake_direct_sh_volume_cached(&inputs, &config, Some(&cache)).to_bytes();
        let second = bake_direct_sh_volume_cached(&inputs, &config, Some(&cache)).to_bytes();

        assert_eq!(uncached, first, "cached miss must match the uncached bake");
        assert_eq!(first, second, "cached hit must reproduce the first build");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
