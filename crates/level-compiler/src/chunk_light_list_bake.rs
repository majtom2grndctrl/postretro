// Per-chunk static-light list builder (ChunkLightList section).
// See: context/lib/build_pipeline.md (PRL sections table)

use bvh::bvh::Bvh;
use bvh::ray::Ray;
use glam::{DVec3, IVec3, Vec3};
use nalgebra::{Point3, Vector3};
use postretro_level_format::chunk_light_list::{
    ChunkEntry, ChunkLightListSection, DEFAULT_PER_CHUNK_CAP,
};
use std::collections::{HashMap, HashSet, VecDeque};
use thiserror::Error;

use crate::bvh_build::BvhPrimitive;
use crate::cache::{CacheKey, StageCache};
use crate::geometry::GeometryResult;
use crate::geometry_utils::clip_winding_to_half_spaces;
use crate::light_namespaces::AlphaLightsNs;
use crate::lightmap_bake;
use crate::map_data::{LightType, MapLight, ShadowType};
use crate::partition::{BspChild, BspTree, find_leaf_for_point};
use crate::portals::Portal;

/// Default chunk edge length in meters. Small enough that per-chunk buckets
/// stay sparse; large enough that the grid does not explode on larger maps.
pub const DEFAULT_CELL_SIZE_METERS: f32 = 8.0;

pub const DEFAULT_PER_CHUNK_LIGHT_CAP: u32 = DEFAULT_PER_CHUNK_CAP;

/// Shared stage-cache identity for the whole ChunkLightList section memo.
///
/// Bump when the bake algorithm, compacted-light slot contract, cache-key
/// inputs, or section payload interpretation changes. This is independent of
/// the section's on-disk version: a cache epoch invalidates disposable local
/// entries, while the section version governs persisted PRL compatibility.
pub const CHUNK_LIGHT_LIST_STAGE_ID: &str = "chunk_light_list";
pub const CHUNK_LIGHT_LIST_STAGE_VERSION: u32 = 3;

/// Cap total `offset table + index list` memory at 16 MB.
pub const MAX_SECTION_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

/// Offset along ray direction to avoid self-intersection on the emitting surface.
const RAY_EPSILON: f32 = 1.0e-3;

/// Move a receiver sample slightly into the light-facing side of its surface
/// before tracing, so the receiving triangle is not mistaken for an occluder.
/// This stays far below the normal 8 m chunk edge while meeting the ray's
/// numerical separation floor.
const RECEIVER_NORMAL_OFFSET_METERS: f32 = 2.0e-3;

/// A hit within this distance of a visibility sample does not count as
/// occlusion. Samples are proxies for receiver SURFACES, so the closing
/// centimeters of a segment are expected to touch the receiving geometry
/// itself — a sample lying on a floor or wall plane would otherwise read as
/// "occluded by its own floor" (with only `RAY_EPSILON` of slack the outcome
/// was float luck, which is exactly how ground-layer cells were randomly
/// dropped: the grid's half-cell pad puts a ground cell's midheight ON the
/// floor plane of any map whose geometry AABB min sits at the floor).
const SAMPLE_END_TOLERANCE_METERS: f32 = 0.02;

/// Per-fragment SDF-shadow budget. Runtime traces at most K `sdf`-tagged lights
/// per `chunk_grid` cell; extras are dropped (treated lit). The half-res shadow
/// target has four RGBA channels — slot i maps to channel i. Compiler warns when
/// a cell exceeds K. See `context/lib/rendering_pipeline.md` §4.
///
/// Must equal `SDF_SELECT_K` in `sdf_light_select.wgsl` — that constant drives
/// runtime selection and the half-res texture layout. Raising K also requires
/// updating `indices: array<u32, N>` in `sdf_light_select.wgsl` and the channel
/// mapping in `forward.wgsl`.
pub const SDF_SHADOW_K: usize = 4;

#[derive(Debug, Error)]
pub enum ChunkLightListError {
    #[error(
        "ChunkLightList payload {actual} bytes exceeds {max} byte cap. \
         Raise `cell_size_meters` or subdivide the map."
    )]
    PayloadTooLarge { actual: usize, max: usize },
}

pub struct ChunkLightListInputs<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    pub lights: &'a AlphaLightsNs<'a>,
    pub tree: &'a BspTree,
    pub portals: &'a [Portal],
    pub exterior_leaves: &'a HashSet<usize>,
}

/// A static triangle that can supply a shaded fragment to one or more chunks.
#[derive(Clone, Copy)]
struct ReceiverTriangle {
    vertices: [Vec3; 3],
}

/// Static lights in exactly the compacted `spec_lights` order used by this
/// bake's output indices. AlphaLights source indices deliberately do not cross
/// this seam: inserting or reordering dynamic lights must leave both the
/// compacted slots and the cache key unchanged.
fn compacted_static_lights<'a>(inputs: &ChunkLightListInputs<'a>) -> Vec<&'a MapLight> {
    inputs
        .lights
        .entries()
        .iter()
        .filter(|entry| !entry.light.is_dynamic)
        .map(|entry| entry.light)
        .collect()
}

/// Fold the parts of the BSP input the chunk-light bake actually consumes.
///
/// Point location depends only on each split plane and its front/back child
/// references; the bake additionally reads leaf solidity and the exterior set.
/// Face lists, bounds, defining planes, and node parents are intentionally not
/// folded because they cannot affect this bake once the geometry hash is fixed.
fn update_bsp_fingerprint(
    hasher: &mut blake3::Hasher,
    tree: &BspTree,
    exterior_leaves: &HashSet<usize>,
) {
    fn update_child(hasher: &mut blake3::Hasher, child: &BspChild) {
        match child {
            BspChild::Node(index) => {
                hasher.update(&[0]);
                hasher.update(&(*index as u64).to_le_bytes());
            }
            BspChild::Leaf(index) => {
                hasher.update(&[1]);
                hasher.update(&(*index as u64).to_le_bytes());
            }
        }
    }

    hasher.update(&(tree.nodes.len() as u64).to_le_bytes());
    for node in &tree.nodes {
        for component in [
            node.plane_normal.x,
            node.plane_normal.y,
            node.plane_normal.z,
            node.plane_distance,
        ] {
            hasher.update(&component.to_le_bytes());
        }
        update_child(hasher, &node.front);
        update_child(hasher, &node.back);
    }

    hasher.update(&(tree.leaves.len() as u64).to_le_bytes());
    for (index, leaf) in tree.leaves.iter().enumerate() {
        hasher.update(&[u8::from(leaf.is_solid)]);
        hasher.update(&[u8::from(exterior_leaves.contains(&index))]);
    }
}

/// Build the whole-section cache key for [`bake_chunk_light_list`].
///
/// The fold order is explicit and only includes data that can affect emitted
/// section bytes: geometry for ray tracing/grid bounds, the point-location BSP
/// partition and leaf state, portal leaf-pair adjacency, compacted static
/// lights, and the two bake controls. Dynamic lights and AlphaLights source
/// indices are excluded because the bake never reads them for its output.
pub(crate) fn chunk_light_list_cache_key(
    inputs: &ChunkLightListInputs<'_>,
    cell_size_meters: f32,
    per_chunk_cap: u32,
) -> CacheKey {
    chunk_light_list_cache_key_with_version(
        inputs,
        cell_size_meters,
        per_chunk_cap,
        CHUNK_LIGHT_LIST_STAGE_VERSION,
    )
}

/// Version-parameterized key builder used by the cache-epoch regression test.
/// The production wrapper always supplies the current stage version.
pub(crate) fn chunk_light_list_cache_key_with_version(
    inputs: &ChunkLightListInputs<'_>,
    cell_size_meters: f32,
    per_chunk_cap: u32,
    stage_version: u32,
) -> CacheKey {
    let mut hasher = blake3::Hasher::new();

    hasher.update(&crate::sh_group::geometry_content_hash(inputs.geometry));
    update_bsp_fingerprint(&mut hasher, inputs.tree, inputs.exterior_leaves);

    // Portal polygon vertices never affect the flood: only the directed leaf
    // pairs become adjacency entries, in generated portal order.
    hasher.update(&(inputs.portals.len() as u64).to_le_bytes());
    for portal in inputs.portals {
        hasher.update(&(portal.front_leaf as u64).to_le_bytes());
        hasher.update(&(portal.back_leaf as u64).to_le_bytes());
    }

    let static_lights = compacted_static_lights(inputs);
    let encoded =
        postcard::to_allocvec(&static_lights).expect("postcard serialize chunk static lights");
    hasher.update(&(encoded.len() as u64).to_le_bytes());
    hasher.update(&encoded);

    hasher.update(&cell_size_meters.to_le_bytes());
    hasher.update(&per_chunk_cap.to_le_bytes());

    CacheKey::new(
        CHUNK_LIGHT_LIST_STAGE_ID,
        stage_version,
        hasher.finalize().as_bytes(),
    )
}

/// Bake the ChunkLightList section or load a validated whole-section memo.
/// `None` bypasses the cache entirely: it neither reads nor writes a cache
/// entry, preserving the exact `--no-cache` path and all bake errors.
pub fn bake_chunk_light_list_cached(
    inputs: &ChunkLightListInputs<'_>,
    cell_size_meters: f32,
    per_chunk_cap: u32,
    cache: Option<&StageCache>,
) -> Result<ChunkLightListSection, ChunkLightListError> {
    let Some(cache) = cache else {
        return bake_chunk_light_list(inputs, cell_size_meters, per_chunk_cap);
    };

    let key = chunk_light_list_cache_key(inputs, cell_size_meters, per_chunk_cap);
    if let Some(bytes) = cache.get(&key) {
        match ChunkLightListSection::from_bytes(&bytes) {
            Ok(section) => match validate_cached_chunk_light_list(
                &section,
                inputs,
                cell_size_meters,
                per_chunk_cap,
            ) {
                Ok(()) => {
                    log::info!("[cache] chunk_light_list hit");
                    return Ok(section);
                }
                Err(reason) => {
                    log::warn!("[cache] corrupt chunk_light_list entry, re-baking: {reason}");
                }
            },
            Err(error) => {
                log::warn!("[cache] corrupt chunk_light_list entry, re-baking: {error}");
            }
        }
    }
    log::info!("[cache] chunk_light_list miss");

    // Keep the baker's error surface intact: a genuine PayloadTooLarge miss
    // remains a compiler error and is never written as a soft cache result.
    let section = bake_chunk_light_list(inputs, cell_size_meters, per_chunk_cap)?;
    cache.put(&key, &section.to_bytes());
    Ok(section)
}

/// Check decoded cache bytes against the bake context that produced their key.
/// The section codec validates byte bounds only. This check rejects shapes that
/// decode but cannot be emitted for the current geometry, light set, or bake
/// controls, so cache corruption remains a soft miss rather than stale output.
fn validate_cached_chunk_light_list(
    section: &ChunkLightListSection,
    inputs: &ChunkLightListInputs<'_>,
    cell_size_meters: f32,
    per_chunk_cap: u32,
) -> Result<(), &'static str> {
    let static_light_count = inputs
        .lights
        .entries()
        .iter()
        .filter(|entry| !entry.light.is_dynamic)
        .count();

    if inputs.geometry.geometry.vertices.is_empty() || static_light_count == 0 {
        if section != &ChunkLightListSection::placeholder() {
            return Err("expected the canonical placeholder");
        }
        return Ok(());
    }

    let (expected_origin, expected_cell_size, expected_dimensions) =
        chunk_grid_layout(inputs.geometry, cell_size_meters);
    let expected_cap = per_chunk_cap;
    if section.has_grid != 1 {
        return Err("expected a populated grid");
    }
    if section.grid_origin != expected_origin.to_array()
        || section.cell_size != expected_cell_size
        || section.grid_dimensions != expected_dimensions
        || section.per_chunk_cap != expected_cap
    {
        return Err("grid shape does not match current bake inputs");
    }

    let expected_chunk_count = expected_dimensions
        .iter()
        .try_fold(1usize, |count, &dimension| {
            count.checked_mul(dimension as usize)
        })
        .ok_or("expected chunk count overflows usize")?;
    if section.offsets.len() != expected_chunk_count {
        return Err("offset table length does not match expected grid");
    }

    let mut next_offset = 0usize;
    let mut seen_in_chunk = vec![usize::MAX; static_light_count];
    for (chunk_index, entry) in section.offsets.iter().enumerate() {
        if entry.offset as usize != next_offset {
            return Err("offset table is not canonically packed");
        }
        if entry.count > expected_cap {
            return Err("chunk count exceeds the current cap");
        }
        let end = next_offset
            .checked_add(entry.count as usize)
            .ok_or("chunk index range overflows usize")?;
        let indices = section
            .light_indices
            .get(next_offset..end)
            .ok_or("chunk index range exceeds the flat list")?;
        for &light_index in indices {
            let light_index = light_index as usize;
            if light_index >= static_light_count {
                return Err("chunk references a missing compacted static-light slot");
            }
            if seen_in_chunk[light_index] == chunk_index {
                return Err("chunk repeats a compacted static-light slot");
            }
            seen_in_chunk[light_index] = chunk_index;
        }
        next_offset = end;
    }
    if next_offset != section.light_indices.len() {
        return Err("offset table does not consume the flat index list");
    }
    Ok(())
}

/// Returns a placeholder section (`has_grid == 0`) when there is nothing to bake.
/// Runtime falls back to full-buffer iteration on placeholder.
pub fn bake_chunk_light_list(
    inputs: &ChunkLightListInputs<'_>,
    cell_size_meters: f32,
    per_chunk_cap: u32,
) -> Result<ChunkLightListSection, ChunkLightListError> {
    let verts = &inputs.geometry.geometry.vertices;
    if verts.is_empty() {
        return Ok(ChunkLightListSection::placeholder());
    }

    // `light_indices` values index the COMPACTED `!is_dynamic` spec_lights array
    // (mirrors `pack_spec_lights`, `spec_buffer.rs`) — NOT AlphaLights slot space.
    // `pack_spec_lights` skips dynamic lights with no placeholder; the slot is a
    // running index over non-dynamic lights only. `enumerate()` runs AFTER the
    // `!is_dynamic` filter so the emitted index is a contiguous compacted slot.
    let static_slots: Vec<(u32, &MapLight)> = compacted_static_lights(inputs)
        .into_iter()
        .enumerate()
        .map(|(slot, light)| (slot as u32, light))
        .collect();
    if static_slots.is_empty() {
        return Ok(ChunkLightListSection::placeholder());
    }

    // Pad the grid bounds outward by HALF a cell on every side. Without padding,
    // `grid_origin` sits FLUSH with the lowest rendered surface (the geometry-AABB
    // min) — e.g. a pit floor whose surface y equals `grid_origin.y`. The full-res
    // forward shader selects SDF lights at the exact fragment position, so a
    // flush-boundary floor lands in cell 0 and is lit; but the half-res SDF shadow
    // pass selects at a depth-RECONSTRUCTED half-res position whose sub-meter error
    // can tip that same floor to cell index -1 ("outside grid → no lights"),
    // writing no shadow and leaving the floor reading fully lit. The forward
    // shader documents this full-vs-half-res disagreement at a chunk-grid cell
    // boundary.
    //
    // The padding must NOT be an integer multiple of `cell`: shifting the origin
    // by a whole cell only MOVES the flush boundary from the grid edge to the
    // first interior cell boundary, where surfaces flush with the AABB min STILL
    // straddle a cell face (a downward reconstruction error then drops into the
    // sub-floor cell, which legitimately holds no light because the floor occludes
    // it). Half a cell centers a surface flush with the AABB min in the MIDDLE of
    // cell 0 — the point maximally far (half a cell) from either neighboring cell
    // face — so the half-res reconstruction error (sub-meter; here < 0.5·cell = 4 m)
    // cannot push it across a boundary. This mirrors the intent of the SDF atlas
    // grid's `GRID_VOXEL_PADDING` band (`sdf_bake::grid_extents`), which combines
    // an outward pad with a lattice snap to keep the edge surface band inside the
    // grid; we achieve the equivalent boundary-avoidance with a fractional pad and
    // no snap (the chunk grid is recomputed identically each bake, so origin-offset
    // determinism is moot). We pad the origin (and expand `world_max` to keep
    // coverage) at this single point; `grid_origin`/`dims` are computed once here
    // and reused everywhere downstream (per-cell construction, the oversubscription
    // warning, the emitted `ChunkLightListSection`), so the shift flows through
    // consistently.
    let (world_min, cell, dims) = chunk_grid_layout(inputs.geometry, cell_size_meters);
    let nx = dims[0] as usize;
    let ny = dims[1] as usize;
    let nz = dims[2] as usize;
    let chunk_count = nx * ny * nz;

    // Receiver triangles, unlike volume-proxy points, are guaranteed to
    // represent fragments the forward shader can actually shade. Bin once so
    // the per-(chunk, light) loop only clips local candidates.
    let receiver_triangles = build_receiver_triangle_bins(inputs.geometry, world_min, cell, dims);

    let cap = per_chunk_cap as usize;

    let mut adjacency: HashMap<usize, Vec<usize>> = HashMap::new();
    for p in inputs.portals {
        adjacency.entry(p.front_leaf).or_default().push(p.back_leaf);
        adjacency.entry(p.back_leaf).or_default().push(p.front_leaf);
    }

    // `None` means portal filter is bypassed: directional sources, or origin in
    // solid/exterior leaf — fall back to spatial overlap + BVH shadow rays only.
    let light_reachable: Vec<Option<HashSet<usize>>> = static_slots
        .iter()
        .map(|&(_, light)| {
            if matches!(light.light_type, LightType::Directional) {
                return None;
            }
            let source = find_leaf_for_point(inputs.tree, light.origin);
            if source >= inputs.tree.leaves.len() {
                return None;
            }
            if inputs.tree.leaves[source].is_solid || inputs.exterior_leaves.contains(&source) {
                return None;
            }
            let mut reachable: HashSet<usize> = HashSet::new();
            reachable.insert(source);
            let mut queue: VecDeque<usize> = VecDeque::new();
            queue.push_back(source);
            while let Some(leaf) = queue.pop_front() {
                if let Some(neighbors) = adjacency.get(&leaf) {
                    for &n in neighbors {
                        if inputs.exterior_leaves.contains(&n) {
                            continue;
                        }
                        if reachable.insert(n) {
                            queue.push_back(n);
                        }
                    }
                }
            }
            Some(reachable)
        })
        .collect();

    let mut per_chunk: Vec<Vec<u32>> = vec![Vec::new(); chunk_count];
    let mut overflow_drops = 0u64;
    let mut overflow_chunks = 0u64;

    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let chunk_idx = z * nx * ny + y * nx + x;
                let chunk_min = Vec3::new(
                    world_min.x + x as f32 * cell,
                    world_min.y + y as f32 * cell,
                    world_min.z + z as f32 * cell,
                );
                let chunk_max = chunk_min + Vec3::splat(cell);
                let chunk_centroid = (chunk_min + chunk_max) * 0.5;
                let chunk_leaf = find_leaf_for_point(
                    inputs.tree,
                    DVec3::new(
                        chunk_centroid.x as f64,
                        chunk_centroid.y as f64,
                        chunk_centroid.z as f64,
                    ),
                );

                // Bypass portal filter when centroid lands in a solid leaf (wall bisects
                // the chunk — common at 8 m grid), exterior leaf, or out-of-range index.
                // The AABB still overlaps visible air; fall back to spatial + BVH only.
                let chunk_filter_bypassed = chunk_leaf >= inputs.tree.leaves.len()
                    || inputs.tree.leaves[chunk_leaf].is_solid
                    || inputs.exterior_leaves.contains(&chunk_leaf);

                let bucket = &mut per_chunk[chunk_idx];
                // Slots kept by the contains-light guard below, ascending
                // (static_slots order). Consulted at cap truncation so every
                // non-contained light is evicted before a contained one.
                let mut contained_slots: Vec<u32> = Vec::new();
                for (idx, &(slot, light)) in static_slots.iter().enumerate() {
                    if !overlaps_chunk(light, chunk_min, chunk_max) {
                        continue;
                    }
                    // A cell that CONTAINS the light keeps it: fragments right
                    // next to the light are lit no matter what the
                    // centroid-keyed portal filter or the visibility rays say —
                    // both key on proxy points that can sit in a different room
                    // (or inside geometry) than the light, and an 8 m cell
                    // routinely spans both. The keep survives the overflow cap
                    // too (contained slots rank ahead of every non-contained
                    // slot before truncation below); it yields only past `cap`
                    // contained lights in one cell.
                    if matches!(light.light_type, LightType::Point | LightType::Spot) {
                        let origin = Vec3::new(
                            light.origin.x as f32,
                            light.origin.y as f32,
                            light.origin.z as f32,
                        );
                        if origin.cmpge(chunk_min).all() && origin.cmple(chunk_max).all() {
                            contained_slots.push(slot);
                            bucket.push(slot);
                            continue;
                        }
                    }
                    if !chunk_filter_bypassed {
                        if let Some(reachable) = &light_reachable[idx] {
                            if !reachable.contains(&chunk_leaf) {
                                continue;
                            }
                        }
                    }
                    if !any_receiver_unoccluded(
                        inputs.bvh,
                        inputs.primitives,
                        inputs.geometry,
                        light,
                        &receiver_triangles[chunk_idx],
                        (chunk_min, chunk_max),
                    ) {
                        continue;
                    }
                    bucket.push(slot);
                }

                if bucket.len() > cap {
                    overflow_chunks += 1;
                    let dropped = bucket.len() - cap;
                    overflow_drops += dropped as u64;
                    log::warn!(
                        "[ChunkLightList] chunk ({x}, {y}, {z}) holds {} lights; \
                         clamping to cap {cap}, dropping {dropped}",
                        bucket.len(),
                    );
                    // Contained lights are a hard first tier: preserve them in
                    // compacted-slot order. Rank the rest by the maximum
                    // contribution they can make anywhere in the chunk. This
                    // improves strong-light continuity across neighboring
                    // chunks, though their different candidate sets and
                    // closest-approach points cannot guarantee identical sets.
                    bucket.sort_by(|left, right| {
                        let left_contained = contained_slots.binary_search(left).is_ok();
                        let right_contained = contained_slots.binary_search(right).is_ok();
                        match (left_contained, right_contained) {
                            (true, true) => left.cmp(right),
                            (true, false) => std::cmp::Ordering::Less,
                            (false, true) => std::cmp::Ordering::Greater,
                            (false, false) => {
                                let left_light = static_slots[*left as usize].1;
                                let right_light = static_slots[*right as usize].1;
                                let left_influence =
                                    chunk_light_influence(left_light, chunk_min, chunk_max);
                                let right_influence =
                                    chunk_light_influence(right_light, chunk_min, chunk_max);
                                right_influence
                                    .total_cmp(&left_influence)
                                    .then_with(|| left.cmp(right))
                            }
                        }
                    });
                    bucket.truncate(cap);
                }
            }
        }
    }

    let mut offsets = Vec::with_capacity(chunk_count);
    let total_indices: usize = per_chunk.iter().map(|v| v.len()).sum();
    let mut indices = Vec::with_capacity(total_indices);
    let mut running: u32 = 0;
    for bucket in &per_chunk {
        offsets.push(ChunkEntry {
            offset: running,
            count: bucket.len() as u32,
        });
        indices.extend_from_slice(bucket);
        running += bucket.len() as u32;
    }

    let payload_bytes = offsets.len() * 8 + indices.len() * 4;
    if payload_bytes > MAX_SECTION_PAYLOAD_BYTES {
        return Err(ChunkLightListError::PayloadTooLarge {
            actual: payload_bytes,
            max: MAX_SECTION_PAYLOAD_BYTES,
        });
    }

    let avg = if chunk_count > 0 {
        total_indices as f64 / chunk_count as f64
    } else {
        0.0
    };
    let mut max_count = 0u32;
    for e in &offsets {
        if e.count > max_count {
            max_count = e.count;
        }
    }
    log::info!(
        "[ChunkLightList] grid {}x{}x{} ({} chunks), {} static lights, \
         avg {:.2} / chunk, max {}, total indices {}, payload {} bytes",
        dims[0],
        dims[1],
        dims[2],
        chunk_count,
        static_slots.len(),
        avg,
        max_count,
        total_indices,
        payload_bytes,
    );
    if overflow_chunks > 0 {
        log::warn!(
            "[ChunkLightList] {overflow_chunks} chunks overflowed cap {cap}; \
             {overflow_drops} light entries dropped across the grid"
        );
    }

    // The runtime resolves `sdf`-tagged lights per-fragment, not from this
    // baked list — but the `chunk_grid` cell is the unit the runtime's
    // K-selection operates on, so the over-K warning is framed in cells here.
    let sdf_lights: Vec<&MapLight> = inputs
        .lights
        .entries()
        .iter()
        .map(|e| e.light)
        .filter(|l| l.shadow_type == ShadowType::Sdf)
        .collect();
    warn_oversubscribed_sdf_cells(&sdf_lights, world_min, cell, dims, SDF_SHADOW_K);

    Ok(ChunkLightListSection {
        grid_origin: world_min.to_array(),
        cell_size: cell,
        grid_dimensions: dims,
        has_grid: 1,
        per_chunk_cap,
        offsets,
        light_indices: indices,
    })
}

/// Warn when more than `k` `sdf`-tagged lights cover a single `chunk_grid` cell.
/// Runtime traces at most `k` per fragment; extras are dropped (treated lit).
/// Coverage uses the `overlaps_chunk` sphere test alone — a SUPERSET of baked
/// membership (every kept light first passes `overlaps_chunk`), so the warning
/// may over-report but never misses a cell where the runtime K could drop a
/// baked light. Returns over-K cell count (for tests); logging is the
/// production effect.
fn warn_oversubscribed_sdf_cells(
    sdf_lights: &[&MapLight],
    world_min: Vec3,
    cell: f32,
    dims: [u32; 3],
    k: usize,
) -> u64 {
    if sdf_lights.len() <= k {
        return 0; // cannot exceed k in any cell
    }

    let nx = dims[0] as usize;
    let ny = dims[1] as usize;
    let nz = dims[2] as usize;

    let mut over_cells = 0u64;
    let mut worst = (0usize, [0u32; 3]);
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let chunk_min = Vec3::new(
                    world_min.x + x as f32 * cell,
                    world_min.y + y as f32 * cell,
                    world_min.z + z as f32 * cell,
                );
                let chunk_max = chunk_min + Vec3::splat(cell);
                let covering = sdf_lights
                    .iter()
                    .filter(|l| overlaps_chunk(l, chunk_min, chunk_max))
                    .count();
                if covering > k {
                    over_cells += 1;
                    if covering > worst.0 {
                        worst = (covering, [x as u32, y as u32, z as u32]);
                    }
                }
            }
        }
    }

    if over_cells > 0 {
        log::warn!(
            "[ChunkLightList] {over_cells} chunk-grid cell(s) are covered by more than \
             K={k} `_shadow_type sdf` lights; the runtime traces only K per fragment and \
             drops the rest (treated lit). Worst cell ({}, {}, {}) is covered by {}. \
             Re-tag some lights `static_light_map` (or author them dynamic-tier) or spread \
             them out.",
            worst.1[0],
            worst.1[1],
            worst.1[2],
            worst.0,
        );
    }

    over_cells
}

fn world_aabb(geo: &GeometryResult) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for v in &geo.geometry.vertices {
        let p = Vec3::from(v.position);
        min = min.min(p);
        max = max.max(p);
    }
    (min, max)
}

fn chunk_grid_layout(geo: &GeometryResult, cell_size_meters: f32) -> (Vec3, f32, [u32; 3]) {
    let cell = cell_size_meters.max(1.0e-3);
    let (geo_min, geo_max) = world_aabb(geo);
    let pad = Vec3::splat(cell * 0.5);
    let world_min = geo_min - pad;
    let world_max = geo_max + pad;
    let extent = (world_max - world_min).max(Vec3::splat(cell));
    let dims = [
        ((extent.x / cell).ceil() as u32).max(1),
        ((extent.y / cell).ceil() as u32).max(1),
        ((extent.z / cell).ceil() as u32).max(1),
    ];
    (world_min, cell, dims)
}

/// Assign each static geometry triangle to every chunk whose AABB it overlaps.
/// The receiver test clips the triangle again before sampling, so these bins
/// may conservatively include a triangle that only touches a chunk boundary.
fn build_receiver_triangle_bins(
    geometry: &GeometryResult,
    world_min: Vec3,
    cell: f32,
    dims: [u32; 3],
) -> Vec<Vec<ReceiverTriangle>> {
    let nx = dims[0] as usize;
    let ny = dims[1] as usize;
    let nz = dims[2] as usize;
    let mut bins = vec![Vec::new(); nx * ny * nz];
    let vertices = &geometry.geometry.vertices;

    for indices in geometry.geometry.indices.chunks_exact(3) {
        let triangle = ReceiverTriangle {
            vertices: [
                Vec3::from(vertices[indices[0] as usize].position),
                Vec3::from(vertices[indices[1] as usize].position),
                Vec3::from(vertices[indices[2] as usize].position),
            ],
        };
        let triangle_min = triangle
            .vertices
            .iter()
            .copied()
            .fold(Vec3::splat(f32::INFINITY), Vec3::min);
        let triangle_max = triangle
            .vertices
            .iter()
            .copied()
            .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);

        let start = ((triangle_min - world_min) / cell)
            .floor()
            .as_ivec3()
            .clamp(
                IVec3::ZERO,
                IVec3::new(nx as i32 - 1, ny as i32 - 1, nz as i32 - 1),
            );
        let end = ((triangle_max - world_min) / cell)
            .floor()
            .as_ivec3()
            .clamp(
                IVec3::ZERO,
                IVec3::new(nx as i32 - 1, ny as i32 - 1, nz as i32 - 1),
            );

        for z in start.z as usize..=end.z as usize {
            for y in start.y as usize..=end.y as usize {
                for x in start.x as usize..=end.x as usize {
                    let chunk_idx = z * nx * ny + y * nx + x;
                    bins[chunk_idx].push(triangle);
                }
            }
        }
    }

    bins
}

fn overlaps_chunk(light: &MapLight, chunk_min: Vec3, chunk_max: Vec3) -> bool {
    match light.light_type {
        LightType::Directional => true,
        LightType::Point | LightType::Spot => {
            // Spot lights use a conservative sphere; cone refinement is a runtime concern.
            let center = Vec3::new(
                light.origin.x as f32,
                light.origin.y as f32,
                light.origin.z as f32,
            );
            let radius = light.falloff_range.max(0.0);
            let closest = center.clamp(chunk_min, chunk_max);
            let d = closest - center;
            d.dot(d) <= radius * radius
        }
    }
}

/// Maximum runtime-like light contribution over a chunk's AABB. The bake has
/// one retained set per chunk, so closest approach conservatively keeps a light
/// that can matter strongly anywhere in that chunk.
fn chunk_light_influence(light: &MapLight, chunk_min: Vec3, chunk_max: Vec3) -> f32 {
    let center = Vec3::new(
        light.origin.x as f32,
        light.origin.y as f32,
        light.origin.z as f32,
    );
    let distance = (center.clamp(chunk_min, chunk_max) - center).length();
    let attenuation = if light.falloff_range > 0.0 {
        lightmap_bake::falloff(light, distance)
    } else {
        1.0
    };
    let peak = light.intensity * light.color[0].max(light.color[1]).max(light.color[2]);
    attenuation * peak
}

/// Returns `true` when an actual receiver fragment in this chunk can see the
/// light. Each triangle is clipped before sampling, so a neighboring chunk
/// receives the same shared surface without treating the chunk volume as a
/// receiver.
///
/// This is deliberately not a normal-facing admission test. The normal only
/// determines which side receives the self-intersection offset; both sides of
/// a geometric receiver remain eligible to keep a light.
fn any_receiver_unoccluded(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    light: &MapLight,
    receiver_triangles: &[ReceiverTriangle],
    chunk_bounds: (Vec3, Vec3),
) -> bool {
    let (chunk_min, chunk_max) = chunk_bounds;
    let clip_planes = [
        (DVec3::X, chunk_min.x as f64),
        (-DVec3::X, -chunk_max.x as f64),
        (DVec3::Y, chunk_min.y as f64),
        (-DVec3::Y, -chunk_max.y as f64),
        (DVec3::Z, chunk_min.z as f64),
        (-DVec3::Z, -chunk_max.z as f64),
    ];

    for triangle in receiver_triangles {
        let normal = (triangle.vertices[1] - triangle.vertices[0])
            .cross(triangle.vertices[2] - triangle.vertices[0])
            .normalize_or_zero();
        if normal == Vec3::ZERO {
            continue;
        }
        let winding = triangle
            .vertices
            .map(|vertex| DVec3::new(vertex.x as f64, vertex.y as f64, vertex.z as f64))
            .to_vec();
        let Some(mut clipped) =
            clip_winding_to_half_spaces(winding, &clip_planes, RAY_EPSILON as f64)
        else {
            continue;
        };
        let centroid = clipped
            .iter()
            .copied()
            .fold(DVec3::ZERO, |sum, vertex| sum + vertex)
            / clipped.len() as f64;
        clipped.push(centroid);

        for sample in clipped {
            let sample = Vec3::new(sample.x as f32, sample.y as f32, sample.z as f32);
            if matches!(light.light_type, LightType::Point | LightType::Spot) {
                let light_origin = Vec3::new(
                    light.origin.x as f32,
                    light.origin.y as f32,
                    light.origin.z as f32,
                );
                let range = light.falloff_range.max(0.0);
                if (sample - light_origin).length_squared() > range * range {
                    continue;
                }
            }
            let to_light = receiver_to_light_direction(light, sample);
            let alignment = normal.dot(to_light);
            let sample = if alignment > RAY_EPSILON {
                sample + normal * RECEIVER_NORMAL_OFFSET_METERS
            } else if alignment < -RAY_EPSILON {
                sample - normal * RECEIVER_NORMAL_OFFSET_METERS
            } else {
                sample
            };
            if segment_clear(bvh, primitives, geometry, light, sample) {
                return true;
            }
        }
    }
    false
}

fn receiver_to_light_direction(light: &MapLight, sample: Vec3) -> Vec3 {
    match light.light_type {
        LightType::Point | LightType::Spot => (Vec3::new(
            light.origin.x as f32,
            light.origin.y as f32,
            light.origin.z as f32,
        ) - sample)
            .normalize_or_zero(),
        LightType::Directional => {
            -Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0])).normalize_or_zero()
        }
    }
}

/// The former production volume-proxy samples, retained only to prove the
/// regression coverage against the pre-receiver-cull behavior.
///
/// The former scheme (cell center + eight inset corners) sampled cell-geometry
/// landmarks that can DEGENERATE onto world geometry. Its nine rays could all
/// graze or cross geometry while a real receiver remained visible, dropping a
/// light from a cell it plainly illuminated. The influence/world clip keeps
/// every sample where visibility is actually at stake, and the inset keeps
/// corners off planes that coincide with cell or world faces.
#[cfg(test)]
fn sample_points(
    light: &MapLight,
    (chunk_min, chunk_max): (Vec3, Vec3),
    (world_min, world_max): (Vec3, Vec3),
) -> [Vec3; 9] {
    let mut lo = chunk_min.max(world_min);
    let mut hi = chunk_max.min(world_max);
    if let LightType::Point | LightType::Spot = light.light_type {
        let center = Vec3::new(
            light.origin.x as f32,
            light.origin.y as f32,
            light.origin.z as f32,
        );
        let radius = light.falloff_range.max(0.0);
        lo = lo.max(center - Vec3::splat(radius));
        hi = hi.min(center + Vec3::splat(radius));
    }
    // A clip can empty an axis (float slack on a sphere-graze, or a cell fully
    // outside the world pad). Collapse that axis to the midpoint of the raw
    // cell interval rather than inventing an inverted box.
    for axis in 0..3 {
        if lo[axis] > hi[axis] {
            let mid = (chunk_min[axis] + chunk_max[axis]) * 0.5;
            lo[axis] = mid;
            hi[axis] = mid;
        }
    }
    let center = (lo + hi) * 0.5;
    // Pull corners off the box faces (quarter-extent, capped at 0.5 m) so they
    // cannot sit exactly on a wall/floor plane flush with a box face. A
    // zero-extent axis stays collapsed — the on-plane case is what
    // `SAMPLE_END_TOLERANCE_METERS` absorbs.
    let inset = ((hi - lo) * 0.25).min(Vec3::splat(0.5));
    let (a, b) = (lo + inset, hi - inset);
    [
        center,
        Vec3::new(a.x, a.y, a.z),
        Vec3::new(b.x, a.y, a.z),
        Vec3::new(a.x, b.y, a.z),
        Vec3::new(b.x, b.y, a.z),
        Vec3::new(a.x, a.y, b.z),
        Vec3::new(b.x, a.y, b.z),
        Vec3::new(a.x, b.y, b.z),
        Vec3::new(b.x, b.y, b.z),
    ]
}

fn segment_clear(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    light: &MapLight,
    sample: Vec3,
) -> bool {
    let (from, to) = match light.light_type {
        LightType::Point | LightType::Spot => (
            Vec3::new(
                light.origin.x as f32,
                light.origin.y as f32,
                light.origin.z as f32,
            ),
            sample,
        ),
        LightType::Directional => {
            let aim =
                Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0])).normalize_or_zero();
            let to_light = -aim;
            (sample + to_light * 10_000.0, sample)
        }
    };

    let delta = to - from;
    let length = delta.length();
    if length < RAY_EPSILON {
        return true;
    }
    let dir = delta / length;
    let origin = from + dir * RAY_EPSILON;
    let ray = Ray::new(
        Point3::new(origin.x, origin.y, origin.z),
        Vector3::new(dir.x, dir.y, dir.z),
    );
    // Stop counting hits a couple of centimeters short of the sample: a graze
    // at the endpoint is the receiving surface itself, not an occluder (see
    // `SAMPLE_END_TOLERANCE_METERS`).
    let max_distance = length - SAMPLE_END_TOLERANCE_METERS.max(RAY_EPSILON);
    if max_distance <= 0.0 {
        return true;
    }
    let geom = &geometry.geometry;
    for prim in bvh.traverse_iterator(&ray, primitives) {
        let start = prim.index_offset as usize;
        let end = start + prim.index_count as usize;
        let mut tri = start;
        while tri + 3 <= end {
            let i0 = geom.indices[tri] as usize;
            let i1 = geom.indices[tri + 1] as usize;
            let i2 = geom.indices[tri + 2] as usize;
            tri += 3;
            let p0 = Vec3::from(geom.vertices[i0].position);
            let p1 = Vec3::from(geom.vertices[i1].position);
            let p2 = Vec3::from(geom.vertices[i2].position);
            if let Some(dist) = ray_triangle_hit(origin, dir, p0, p1, p2) {
                if dist > 0.0 && dist < max_distance {
                    return false;
                }
            }
        }
    }
    true
}

fn ray_triangle_hit(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let edge1 = b - a;
    let edge2 = c - a;
    let h = dir.cross(edge2);
    let det = edge1.dot(h);
    if det.abs() < 1.0e-8 {
        return None;
    }
    let inv_det = 1.0 / det;
    let s = origin - a;
    let u = inv_det * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(edge1);
    let v = inv_det * dir.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = inv_det * edge2.dot(q);
    if t <= 0.0 { None } else { Some(t) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh_build::build_bvh;
    use crate::fixture_pipeline::load_fixture;
    use crate::geometry::FaceIndexRange;
    use crate::light_namespaces::AlphaLightsNs;
    use crate::map_data::{FalloffModel, LightType, MapLight};
    use crate::portals::generate_portals;
    use glam::DVec3;
    use log::Level;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;
    use postretro_test_log_capture::LogCapture;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn fresh_cache_dir(label: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "postretro_chunk_light_list_cache_{label}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn point_light(origin: DVec3, range: f32) -> MapLight {
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
            animation: None,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        }
    }

    fn dynamic_point_light(origin: DVec3, range: f32) -> MapLight {
        let mut l = point_light(origin, range);
        l.is_dynamic = true;
        l
    }

    fn sdf_point_light(origin: DVec3, range: f32) -> MapLight {
        let mut l = point_light(origin, range);
        l.shadow_type = ShadowType::Sdf;
        l
    }

    /// More than K `sdf`-tagged lights' influence overlapping one cell trips
    /// the over-budget warning (the runtime traces only K per fragment).
    #[test]
    fn warns_when_more_than_k_sdf_lights_cover_a_cell() {
        // A single 8 m cell at the origin; K+1 sdf lights, all centered inside
        // it with generous range, so each overlaps the one cell.
        let world_min = Vec3::ZERO;
        let cell = 8.0;
        let dims = [1u32, 1, 1];
        let k = SDF_SHADOW_K;

        let lights: Vec<MapLight> = (0..=k)
            .map(|i| sdf_point_light(DVec3::new(4.0, 4.0, 4.0 + i as f64 * 0.1), 100.0))
            .collect();
        let refs: Vec<&MapLight> = lights.iter().collect();

        let over = warn_oversubscribed_sdf_cells(&refs, world_min, cell, dims, k);
        assert_eq!(over, 1, "the single cell should be over budget");
    }

    /// Exactly K sdf lights in a cell is within budget — no warning.
    #[test]
    fn does_not_warn_at_exactly_k_sdf_lights() {
        let lights: Vec<MapLight> = (0..SDF_SHADOW_K)
            .map(|i| sdf_point_light(DVec3::new(4.0, 4.0, 4.0 + i as f64 * 0.1), 100.0))
            .collect();
        let refs: Vec<&MapLight> = lights.iter().collect();

        let over = warn_oversubscribed_sdf_cells(&refs, Vec3::ZERO, 8.0, [1, 1, 1], SDF_SHADOW_K);
        assert_eq!(over, 0);
    }

    /// A light whose influence sphere does not reach the cell is not counted,
    /// so the warning uses the same overlap metric as the runtime cull.
    #[test]
    fn distant_sdf_lights_do_not_oversubscribe_a_cell() {
        // K+1 sdf lights but each far from the cell with small range.
        let lights: Vec<MapLight> = (0..=SDF_SHADOW_K)
            .map(|i| sdf_point_light(DVec3::new(1000.0 + i as f64 * 50.0, 0.0, 0.0), 1.0))
            .collect();
        let refs: Vec<&MapLight> = lights.iter().collect();

        let over = warn_oversubscribed_sdf_cells(&refs, Vec3::ZERO, 8.0, [1, 1, 1], SDF_SHADOW_K);
        assert_eq!(over, 0);
    }

    fn directional_light(aim: [f32; 3]) -> MapLight {
        MapLight {
            origin: DVec3::ZERO,
            carrier: String::new(),
            light_type: LightType::Directional,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 0.0,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: Some(aim),
            animation: None,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        }
    }

    fn bake_single_quad_chunk_lights(
        lights: &[MapLight],
        cell_size_meters: f32,
        per_chunk_cap: u32,
    ) -> ChunkLightListSection {
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let alpha_lights = AlphaLightsNs::from_lights(lights);
        let tree = empty_tree();
        let exterior = HashSet::new();
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };
        bake_chunk_light_list(&inputs, cell_size_meters, per_chunk_cap).unwrap()
    }

    fn chunk_slots_and_bounds_at(
        section: &ChunkLightListSection,
        point: Vec3,
    ) -> (Vec<u32>, Vec3, Vec3) {
        let origin = Vec3::from(section.grid_origin);
        let cell = section.cell_size;
        let coords = ((point - origin) / cell).floor().as_uvec3();
        let [nx, ny, nz] = section.grid_dimensions;
        assert!(coords.x < nx && coords.y < ny && coords.z < nz);
        let linear = (coords.z * ny * nx + coords.y * nx + coords.x) as usize;
        let entry = section.offsets[linear];
        let start = entry.offset as usize;
        let end = start + entry.count as usize;
        let chunk_min = origin + coords.as_vec3() * cell;
        (
            section.light_indices[start..end].to_vec(),
            chunk_min,
            chunk_min + Vec3::splat(cell),
        )
    }

    fn is_contained_by_chunk(light: &MapLight, chunk_min: Vec3, chunk_max: Vec3) -> bool {
        matches!(light.light_type, LightType::Point | LightType::Spot) && {
            let origin = Vec3::new(
                light.origin.x as f32,
                light.origin.y as f32,
                light.origin.z as f32,
            );
            origin.cmpge(chunk_min).all() && origin.cmple(chunk_max).all()
        }
    }

    fn reference_influence(light: &MapLight, chunk_min: Vec3, chunk_max: Vec3) -> f32 {
        let origin = Vec3::new(
            light.origin.x as f32,
            light.origin.y as f32,
            light.origin.z as f32,
        );
        let distance = (origin.clamp(chunk_min, chunk_max) - origin).length();
        let attenuation = if light.falloff_range > 0.0 {
            crate::lightmap_bake::falloff(light, distance)
        } else {
            1.0
        };
        let peak = light.intensity * light.color[0].max(light.color[1]).max(light.color[2]);
        attenuation * peak
    }

    fn reference_top_cap_slots(
        lights: &[MapLight],
        chunk_min: Vec3,
        chunk_max: Vec3,
        cap: usize,
    ) -> Vec<u32> {
        let mut slots: Vec<u32> = (0..lights.len() as u32).collect();
        slots.sort_by(|left, right| {
            let left_light = &lights[*left as usize];
            let right_light = &lights[*right as usize];
            match (
                is_contained_by_chunk(left_light, chunk_min, chunk_max),
                is_contained_by_chunk(right_light, chunk_min, chunk_max),
            ) {
                (true, true) => left.cmp(right),
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                (false, false) => reference_influence(right_light, chunk_min, chunk_max)
                    .total_cmp(&reference_influence(left_light, chunk_min, chunk_max))
                    .then_with(|| left.cmp(right)),
            }
        });
        slots.truncate(cap);
        slots
    }

    fn single_quad_geometry() -> GeometryResult {
        // 16 × 16 m floor quad on XZ plane, centered at origin.
        let s = 8.0;
        let v = |x: f32, z: f32| {
            Vertex::new(
                [x, 0.0, z],
                [0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            )
        };
        GeometryResult {
            geometry: GeometrySection {
                vertices: vec![v(-s, -s), v(s, -s), v(s, s), v(-s, s)],
                indices: vec![0, 1, 2, 0, 2, 3],
                faces: vec![FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                }],
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: vec![FaceIndexRange {
                index_offset: 0,
                index_count: 6,
            }],
        }
    }

    fn two_room_geometry() -> GeometryResult {
        // Two floor strips (Room A: x ∈ [-10,-1], Room B: x ∈ [1,10]) separated by
        // a solid wall at x ≈ 0 (x ∈ [-0.5,0.5], y ∈ [0,10], z ∈ [-10,10]).
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut faces = Vec::new();
        let mut ranges = Vec::new();

        let mut push_quad = |vs: [[f32; 3]; 4], n: [f32; 3]| {
            let base = vertices.len() as u32;
            for p in vs.iter() {
                vertices.push(Vertex::new(
                    *p,
                    [0.0, 0.0],
                    n,
                    [1.0, 0.0, 0.0],
                    true,
                    [0.0, 0.0],
                    0,
                ));
            }
            let start = indices.len() as u32;
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            faces.push(FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            });
            ranges.push(FaceIndexRange {
                index_offset: start,
                index_count: 6,
            });
        };

        push_quad(
            // Floor A
            [
                [-10.0, 0.0, -10.0],
                [-1.0, 0.0, -10.0],
                [-1.0, 0.0, 10.0],
                [-10.0, 0.0, 10.0],
            ],
            [0.0, 1.0, 0.0],
        );
        push_quad(
            // Floor B
            [
                [1.0, 0.0, -10.0],
                [10.0, 0.0, -10.0],
                [10.0, 0.0, 10.0],
                [1.0, 0.0, 10.0],
            ],
            [0.0, 1.0, 0.0],
        );
        // Wall faces — seal the gap so rays cannot pass between rooms
        push_quad(
            [
                [-0.5, 0.0, -10.0],
                [-0.5, 10.0, -10.0],
                [-0.5, 10.0, 10.0],
                [-0.5, 0.0, 10.0],
            ],
            [-1.0, 0.0, 0.0],
        );
        push_quad(
            [
                [0.5, 0.0, -10.0],
                [0.5, 0.0, 10.0],
                [0.5, 10.0, 10.0],
                [0.5, 10.0, -10.0],
            ],
            [1.0, 0.0, 0.0],
        );
        push_quad(
            [
                [-0.5, 10.0, -10.0],
                [0.5, 10.0, -10.0],
                [0.5, 10.0, 10.0],
                [-0.5, 10.0, 10.0],
            ],
            [0.0, 1.0, 0.0],
        );

        GeometryResult {
            geometry: GeometrySection {
                vertices,
                indices,
                faces,
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: ranges,
        }
    }

    fn triangle_geometry(triangles: &[[[f32; 3]; 3]]) -> GeometryResult {
        let mut vertices = Vec::with_capacity(triangles.len() * 3);
        let mut indices = Vec::with_capacity(triangles.len() * 3);
        let mut faces = Vec::with_capacity(triangles.len());
        let mut face_index_ranges = Vec::with_capacity(triangles.len());

        for triangle in triangles {
            let base = vertices.len() as u32;
            for position in triangle {
                vertices.push(Vertex::new(
                    *position,
                    [0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [1.0, 0.0, 0.0],
                    true,
                    [0.0, 0.0],
                    0,
                ));
            }
            let index_offset = indices.len() as u32;
            indices.extend_from_slice(&[base, base + 1, base + 2]);
            faces.push(FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            });
            face_index_ranges.push(FaceIndexRange {
                index_offset,
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

    fn push_quad(triangles: &mut Vec<[[f32; 3]; 3]>, corners: [[f32; 3]; 4]) {
        triangles.push([corners[0], corners[1], corners[2]]);
        triangles.push([corners[0], corners[2], corners[3]]);
    }

    fn section_chunk_slots(section: &ChunkLightListSection, x: u32, y: u32, z: u32) -> &[u32] {
        let [nx, ny, nz] = section.grid_dimensions;
        assert!(x < nx && y < ny && z < nz);
        let chunk_idx = (z * nx * ny + y * nx + x) as usize;
        let entry = section.offsets[chunk_idx];
        let start = entry.offset as usize;
        &section.light_indices[start..start + entry.count as usize]
    }

    #[test]
    fn empty_geometry_returns_placeholder() {
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
        let lights = vec![point_light(DVec3::ZERO, 10.0)];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &[],
            geometry: &geo,
            lights: &alpha_lights,
            tree: &BspTree {
                nodes: Vec::new(),
                leaves: Vec::new(),
            },
            portals: &[],
            exterior_leaves: &HashSet::new(),
        };
        let section = bake_chunk_light_list(
            &inputs,
            DEFAULT_CELL_SIZE_METERS,
            DEFAULT_PER_CHUNK_LIGHT_CAP,
        )
        .unwrap();
        assert_eq!(section.has_grid, 0);
    }

    #[test]
    fn no_static_lights_returns_placeholder() {
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![dynamic_point_light(DVec3::ZERO, 10.0)];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &BspTree {
                nodes: Vec::new(),
                leaves: Vec::new(),
            },
            portals: &[],
            exterior_leaves: &HashSet::new(),
        };
        let section = bake_chunk_light_list(
            &inputs,
            DEFAULT_CELL_SIZE_METERS,
            DEFAULT_PER_CHUNK_LIGHT_CAP,
        )
        .unwrap();
        assert_eq!(section.has_grid, 0);
    }

    #[test]
    fn cache_key_uses_compacted_static_lights_and_structural_inputs() {
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let static_light = point_light(DVec3::new(0.0, 1.0, 0.0), 4.0);
        let static_only = vec![static_light.clone()];
        // This insertion shifts the static light's AlphaLights position but not
        // its compacted spec-light slot, so the whole-section key must still hit.
        let dynamic_then_static = vec![
            dynamic_point_light(DVec3::new(99.0, 1.0, 0.0), 1.0),
            static_light.clone(),
        ];
        let static_ns = AlphaLightsNs::from_lights(&static_only);
        let dynamic_ns = AlphaLightsNs::from_lights(&dynamic_then_static);
        let tree = two_leaf_tree_no_portals();
        let exterior = HashSet::new();
        let static_inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &static_ns,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };
        let dynamic_inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &dynamic_ns,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };
        let baseline = chunk_light_list_cache_key(&static_inputs, 8.0, 64).as_filename();
        assert_eq!(
            baseline,
            chunk_light_list_cache_key(&dynamic_inputs, 8.0, 64).as_filename(),
            "dynamic lights and AlphaLights source positions must not invalidate the compacted bake"
        );

        let mut changed_static_light = static_light.clone();
        changed_static_light.intensity = 2.0;
        let changed_static_lights = vec![changed_static_light];
        let changed_static_ns = AlphaLightsNs::from_lights(&changed_static_lights);
        let changed_static_inputs = ChunkLightListInputs {
            lights: &changed_static_ns,
            ..static_inputs
        };
        assert_ne!(
            baseline,
            chunk_light_list_cache_key(&changed_static_inputs, 8.0, 64).as_filename(),
            "a static-light edit must invalidate the section"
        );

        let solid_tree = two_leaf_tree_solid_back();
        let solid_tree_inputs = ChunkLightListInputs {
            tree: &solid_tree,
            ..static_inputs
        };
        assert_ne!(
            baseline,
            chunk_light_list_cache_key(&solid_tree_inputs, 8.0, 64).as_filename(),
            "leaf solidity changes the portal-filter bypass and must invalidate"
        );

        let (portal_tree, portals) = two_leaf_tree_with_portal();
        let portal_inputs = ChunkLightListInputs {
            tree: &portal_tree,
            portals: &portals,
            ..static_inputs
        };
        assert_ne!(
            baseline,
            chunk_light_list_cache_key(&portal_inputs, 8.0, 64).as_filename(),
            "portal leaf-pair adjacency must invalidate the reachability bake"
        );
    }

    #[test]
    fn cached_chunk_light_list_matches_uncached_and_rebakes_corruption() {
        let dir = fresh_cache_dir("round_trip");
        let cache = StageCache::new(&dir).expect("create cache directory");

        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light(DVec3::new(0.0, 1.0, 0.0), 4.0)];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let tree = empty_tree();
        let exterior = HashSet::new();
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };

        let uncached = bake_chunk_light_list_cached(&inputs, 8.0, 64, None)
            .expect("uncached bake must succeed")
            .to_bytes();
        let first = bake_chunk_light_list_cached(&inputs, 8.0, 64, Some(&cache))
            .expect("cached miss must succeed")
            .to_bytes();
        let second = bake_chunk_light_list_cached(&inputs, 8.0, 64, Some(&cache))
            .expect("cached hit must succeed")
            .to_bytes();
        assert_eq!(uncached, first, "cached miss must match uncached bytes");
        assert_eq!(
            first, second,
            "cached hit must reproduce exact section bytes"
        );

        let key = chunk_light_list_cache_key(&inputs, 8.0, 64);
        cache.put(&key, b"corrupt chunk-light-list cache payload");
        let capture = LogCapture::start();
        let rebaked = bake_chunk_light_list_cached(&inputs, 8.0, 64, Some(&cache))
            .expect("corrupt cache entry must be a soft miss")
            .to_bytes();
        assert_eq!(
            rebaked, uncached,
            "corrupt cache entry must re-bake exact bytes"
        );
        capture.assert_logged_once(Level::Warn, "[cache] corrupt chunk_light_list entry");
        capture.assert_logged_once(Level::Info, "[cache] chunk_light_list miss");
        capture.assert_not_logged(Level::Info, "[cache] chunk_light_list hit");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Regression: a placeholder is codec-valid but stale for populated input,
    // and previously bypassed the bake as a false cache hit.
    #[test]
    fn populated_context_rejects_cached_placeholder_and_rebakes() {
        let dir = fresh_cache_dir("semantic_placeholder_corruption");
        let cache = StageCache::new(&dir).expect("create cache directory");
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light(DVec3::new(0.0, 1.0, 0.0), 4.0)];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let tree = empty_tree();
        let exterior = HashSet::new();
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };

        let reference = bake_chunk_light_list_cached(&inputs, 8.0, 64, None)
            .expect("uncached bake must succeed");
        let key = chunk_light_list_cache_key(&inputs, 8.0, 64);
        cache.put(&key, &ChunkLightListSection::placeholder().to_bytes());

        let capture = LogCapture::start();
        let rebaked = bake_chunk_light_list_cached(&inputs, 8.0, 64, Some(&cache))
            .expect("semantically stale cache entry must be a soft miss");
        assert_eq!(rebaked, reference);
        capture.assert_logged_once(Level::Warn, "[cache] corrupt chunk_light_list entry");
        capture.assert_logged_once(Level::Info, "[cache] chunk_light_list miss");
        capture.assert_not_logged(Level::Info, "[cache] chunk_light_list hit");

        capture.clear();
        let warm = bake_chunk_light_list_cached(&inputs, 8.0, 64, Some(&cache))
            .expect("rebaked section must replace corrupt cache entry");
        assert_eq!(warm, reference);
        capture.assert_logged_once(Level::Info, "[cache] chunk_light_list hit");
        capture.assert_not_logged(Level::Info, "[cache] chunk_light_list miss");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zero_static_lights_placeholder_hits_warm_cache() {
        let dir = fresh_cache_dir("zero_static_placeholder");
        let cache = StageCache::new(&dir).expect("create cache directory");
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![dynamic_point_light(DVec3::ZERO, 10.0)];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let tree = empty_tree();
        let exterior = HashSet::new();
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };

        let first = bake_chunk_light_list_cached(&inputs, 8.0, 64, Some(&cache))
            .expect("zero-static cache miss must return a placeholder");
        assert_eq!(first, ChunkLightListSection::placeholder());

        let capture = LogCapture::start();
        let warm = bake_chunk_light_list_cached(&inputs, 8.0, 64, Some(&cache))
            .expect("zero-static warm hit must return a placeholder");
        assert_eq!(warm, ChunkLightListSection::placeholder());
        capture.assert_logged_once(Level::Info, "[cache] chunk_light_list hit");
        capture.assert_not_logged(Level::Info, "[cache] chunk_light_list miss");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dynamic_light_insertion_ahead_of_static_light_hits_cache() {
        let dir = fresh_cache_dir("dynamic_insert");
        let cache = StageCache::new(&dir).expect("create cache directory");
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let static_light = point_light(DVec3::new(0.0, 1.0, 0.0), 4.0);
        let dynamic_light = dynamic_point_light(DVec3::new(20.0, 1.0, 0.0), 2.0);
        let first_lights = vec![static_light.clone(), dynamic_light.clone()];
        let edited_lights = vec![
            dynamic_point_light(DVec3::new(-20.0, 1.0, 0.0), 3.0),
            static_light,
            dynamic_light,
        ];
        let first_ns = AlphaLightsNs::from_lights(&first_lights);
        let edited_ns = AlphaLightsNs::from_lights(&edited_lights);
        let tree = empty_tree();
        let exterior = HashSet::new();
        let first_inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &first_ns,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };
        let edited_inputs = ChunkLightListInputs {
            lights: &edited_ns,
            ..first_inputs
        };

        let first = bake_chunk_light_list_cached(&first_inputs, 8.0, 64, Some(&cache))
            .expect("initial cache miss must bake");
        let capture = LogCapture::start();
        let warm = bake_chunk_light_list_cached(&edited_inputs, 8.0, 64, Some(&cache))
            .expect("dynamic-light insertion must hit");
        assert_eq!(warm, first);
        capture.assert_logged_once(Level::Info, "[cache] chunk_light_list hit");
        capture.assert_not_logged(Level::Info, "[cache] chunk_light_list miss");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dynamic_light_parameter_edit_hits_cache() {
        let dir = fresh_cache_dir("dynamic_param_edit");
        let cache = StageCache::new(&dir).expect("create cache directory");
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let static_light = point_light(DVec3::new(0.0, 1.0, 0.0), 4.0);
        let dynamic_light = dynamic_point_light(DVec3::new(20.0, 1.0, 0.0), 2.0);
        let first_lights = vec![static_light.clone(), dynamic_light.clone()];
        let mut edited_dynamic = dynamic_light;
        edited_dynamic.intensity = 9.0;
        let edited_lights = vec![static_light, edited_dynamic];
        let first_ns = AlphaLightsNs::from_lights(&first_lights);
        let edited_ns = AlphaLightsNs::from_lights(&edited_lights);
        let tree = empty_tree();
        let exterior = HashSet::new();
        let first_inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &first_ns,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };
        let edited_inputs = ChunkLightListInputs {
            lights: &edited_ns,
            ..first_inputs
        };

        let first = bake_chunk_light_list_cached(&first_inputs, 8.0, 64, Some(&cache))
            .expect("initial cache miss must bake");
        let capture = LogCapture::start();
        let warm = bake_chunk_light_list_cached(&edited_inputs, 8.0, 64, Some(&cache))
            .expect("dynamic-light parameter edit must hit");
        assert_eq!(warm, first);
        capture.assert_logged_once(Level::Info, "[cache] chunk_light_list hit");
        capture.assert_not_logged(Level::Info, "[cache] chunk_light_list miss");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn in_range_light_lands_in_containing_chunks() {
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light(DVec3::new(7.0, 1.0, 7.0), 4.0)];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &BspTree {
                nodes: Vec::new(),
                leaves: Vec::new(),
            },
            portals: &[],
            exterior_leaves: &HashSet::new(),
        };
        let section = bake_chunk_light_list(&inputs, 4.0, 64).unwrap();
        assert_eq!(section.has_grid, 1);
        let total: u32 = section.offsets.iter().map(|e| e.count).sum();
        assert!(total >= 1, "expected at least one chunk to hold the light");
        assert!(
            total < section.chunk_count() as u32,
            "expected the sphere-AABB filter to exclude some chunks (total {} of {} chunks)",
            total,
            section.chunk_count(),
        );
    }

    #[test]
    fn directional_light_populates_every_chunk() {
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![directional_light([0.0, -1.0, 0.0])];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &BspTree {
                nodes: Vec::new(),
                leaves: Vec::new(),
            },
            portals: &[],
            exterior_leaves: &HashSet::new(),
        };
        let section = bake_chunk_light_list(&inputs, 8.0, 64).unwrap();
        assert_eq!(section.has_grid, 1);
        for e in &section.offsets {
            assert_eq!(e.count, 1);
        }
    }

    #[test]
    fn fully_occluded_receiver_behind_two_room_wall_is_dropped() {
        let geo = two_room_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let light_pos = DVec3::new(-5.0, 2.0, 0.0);
        let lights = vec![point_light(light_pos, 50.0)];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &BspTree {
                nodes: Vec::new(),
                leaves: Vec::new(),
            },
            portals: &[],
            exterior_leaves: &HashSet::new(),
        };
        let section = bake_chunk_light_list(&inputs, 4.0, 64).unwrap();
        assert_eq!(section.has_grid, 1);

        let far_point = Vec3::new(5.0, 0.1, 0.0); // floor receiver deep in Room B
        let origin = Vec3::from(section.grid_origin);
        let cell = section.cell_size;
        let cx = ((far_point.x - origin.x) / cell).floor() as i32;
        let cy = ((far_point.y - origin.y) / cell).floor() as i32;
        let cz = ((far_point.z - origin.z) / cell).floor() as i32;
        let nx = section.grid_dimensions[0] as i32;
        let ny = section.grid_dimensions[1] as i32;
        assert!(cx >= 0 && cy >= 0 && cz >= 0);
        let linear = (cz * ny * nx + cy * nx + cx) as usize;
        let entry = section.offsets[linear];
        let count = entry.count;
        assert_eq!(
            count, 0,
            "expected the floor-receiver chunk to see no lights through the wall, got {count}"
        );
    }

    // Regression: a thin floor receiver under a small ceiling aperture was
    // previously dropped because every volume-proxy ray crossed the ceiling.
    #[test]
    fn receiver_sampling_keeps_thin_ceiling_sliver_when_all_volume_proxies_are_occluded() {
        let mut triangles = vec![[[1.22, 0.0, 1.22], [1.25, 0.0, 1.28], [1.28, 0.0, 1.22]]];
        // A thin ceiling with only the small aperture over the receiver.
        push_quad(
            &mut triangles,
            [
                [-2.0, 1.0, -2.0],
                [1.20, 1.0, -2.0],
                [1.20, 1.0, 2.0],
                [-2.0, 1.0, 2.0],
            ],
        );
        push_quad(
            &mut triangles,
            [
                [1.30, 1.0, -2.0],
                [2.0, 1.0, -2.0],
                [2.0, 1.0, 2.0],
                [1.30, 1.0, 2.0],
            ],
        );
        push_quad(
            &mut triangles,
            [
                [1.20, 1.0, -2.0],
                [1.30, 1.0, -2.0],
                [1.30, 1.0, 1.20],
                [1.20, 1.0, 1.20],
            ],
        );
        push_quad(
            &mut triangles,
            [
                [1.20, 1.0, 1.30],
                [1.30, 1.0, 1.30],
                [1.30, 1.0, 2.0],
                [1.20, 1.0, 2.0],
            ],
        );
        let geo = triangle_geometry(&triangles);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let light = point_light(DVec3::new(1.25, 3.0, 1.25), 10.0);
        let lights = vec![light.clone()];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let tree = empty_tree();
        let exterior = HashSet::new();
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };

        let section = bake_chunk_light_list(&inputs, 4.0, 64).unwrap();
        let (slots, chunk_min, chunk_max) =
            chunk_slots_and_bounds_at(&section, Vec3::new(1.25, 0.1, 1.25));
        assert_eq!(slots, vec![0], "the aperture receiver must keep its light");

        let former_proxy_samples = sample_points(&light, (chunk_min, chunk_max), world_aabb(&geo));
        assert!(
            former_proxy_samples
                .into_iter()
                .all(|sample| !segment_clear(&bvh, &prims, &geo, &light, sample)),
            "the former nine volume proxy segments must all be blocked by the ceiling"
        );
    }

    // Regression: a finite light grazing a chunk used to admit a clear receiver
    // at the opposite corner even though runtime range culling makes it dead.
    #[test]
    fn receiver_sampling_rejects_clear_out_of_range_receiver_in_grazed_chunk() {
        let in_range = ReceiverTriangle {
            vertices: [
                Vec3::new(0.00, 0.0, 1.98),
                Vec3::new(0.03, 0.0, 1.98),
                Vec3::new(0.015, 0.0, 2.02),
            ],
        };
        let out_of_range = ReceiverTriangle {
            vertices: [
                Vec3::new(3.60, 0.0, 1.90),
                Vec3::new(3.80, 0.0, 1.90),
                Vec3::new(3.70, 0.0, 2.10),
            ],
        };
        let geo = triangle_geometry(&[
            in_range.vertices.map(|vertex| vertex.to_array()),
            out_of_range.vertices.map(|vertex| vertex.to_array()),
        ]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let light = point_light(DVec3::new(-1.0, 0.4, 2.0), 1.1);
        let chunk_bounds = (Vec3::ZERO, Vec3::splat(4.0));

        assert!(
            overlaps_chunk(&light, chunk_bounds.0, chunk_bounds.1),
            "the finite-light sphere must graze the chunk"
        );
        assert!(
            any_receiver_unoccluded(&bvh, &prims, &geo, &light, &[in_range], chunk_bounds,),
            "an in-range clear receiver must keep the light"
        );
        assert!(
            segment_clear(&bvh, &prims, &geo, &light, out_of_range.vertices[0],),
            "the opposite-corner receiver is clear; range, not occlusion, rejects it"
        );
        assert!(
            !any_receiver_unoccluded(&bvh, &prims, &geo, &light, &[out_of_range], chunk_bounds,),
            "a clear receiver beyond a finite light's range must not admit it"
        );
    }

    #[test]
    fn receiver_triangle_spanning_chunk_plane_keeps_light_on_both_neighbors() {
        // Its reverse winding proves admission does not require a receiver
        // normal to face the light; the normal only picks the offset side.
        let geo = triangle_geometry(&[[[-1.0, 0.0, -1.0], [5.0, 0.0, -1.0], [2.0, 0.0, 3.0]]]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light(DVec3::new(8.0, 3.0, -0.5), 30.0)];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let tree = empty_tree();
        let exterior = HashSet::new();
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };

        let section = bake_chunk_light_list(&inputs, 4.0, 64).unwrap();
        let (left, _, _) = chunk_slots_and_bounds_at(&section, Vec3::new(0.0, 0.1, -0.5));
        let (right, _, _) = chunk_slots_and_bounds_at(&section, Vec3::new(2.0, 0.1, -0.5));
        assert_eq!(left, vec![0]);
        assert_eq!(right, vec![0]);
    }

    #[test]
    fn empty_air_chunk_overlapped_by_light_omits_light() {
        // The distant marker only expands the grid. The intervening chunk is
        // inside the light sphere but has no binned receiver triangle.
        let geo = triangle_geometry(&[
            [[-0.25, 0.0, -0.25], [0.25, 0.0, -0.25], [0.0, 0.0, 0.25]],
            [[20.0, 0.0, -0.25], [20.5, 0.0, -0.25], [20.25, 0.0, 0.25]],
        ]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let light = point_light(DVec3::new(-1.0, 3.0, 0.0), 10.0);
        let lights = vec![light.clone()];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let tree = empty_tree();
        let exterior = HashSet::new();
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };

        let section = bake_chunk_light_list(&inputs, 4.0, 64).unwrap();
        let (slots, chunk_min, chunk_max) =
            chunk_slots_and_bounds_at(&section, Vec3::new(4.0, 0.1, 0.0));
        assert!(
            overlaps_chunk(&light, chunk_min, chunk_max),
            "the light sphere must reach the empty-air chunk"
        );
        assert!(
            slots.is_empty(),
            "an empty receiver bin must not keep a light"
        );
    }

    #[test]
    fn receiver_binning_uses_z_y_x_linearization_on_asymmetric_grid() {
        // Bounds produce nx=4, ny=1, nz=3. Only the small receiver is in
        // range; the three distant markers establish the asymmetric grid.
        let geo = triangle_geometry(&[
            [[1.75, 0.0, 3.25], [2.25, 0.0, 3.25], [2.0, 0.0, 3.75]],
            [[0.0, 0.0, 0.0], [0.1, 0.0, 0.0], [0.0, 0.0, 0.1]],
            [[6.0, 0.0, 0.0], [5.9, 0.0, 0.0], [6.0, 0.0, 0.1]],
            [[0.0, 0.0, 4.0], [0.1, 0.0, 4.0], [0.0, 0.0, 3.9]],
        ]);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let light = point_light(DVec3::new(2.0, 3.0, 7.0), 6.0);
        let lights = vec![light.clone()];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let tree = empty_tree();
        let exterior = HashSet::new();
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };

        let section = bake_chunk_light_list(&inputs, 2.0, 64).unwrap();
        assert_eq!(section.grid_dimensions, [4, 1, 3]);
        let (receiver_x, receiver_y, receiver_z) = (1u32, 0u32, 2u32);
        let [nx, ny, _] = section.grid_dimensions;
        let correct_index = (receiver_z * nx * ny + receiver_y * nx + receiver_x) as usize;
        let transposed_index = (receiver_x * nx * ny + receiver_y * nx + receiver_z) as usize;
        assert_eq!(section.offsets[correct_index].count, 1);
        assert_eq!(
            section.light_indices[section.offsets[correct_index].offset as usize],
            0
        );
        assert_eq!(
            section.offsets[transposed_index].count, 0,
            "the x/z-transposed cell must not receive the receiver triangle"
        );

        let transposed_x = transposed_index as u32 % nx;
        let transposed_z = transposed_index as u32 / (nx * ny);
        let transposed_slots = section_chunk_slots(&section, transposed_x, 0, transposed_z);
        assert!(transposed_slots.is_empty());
    }

    #[test]
    fn per_chunk_cap_clamps_overflow() {
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let mut lights = Vec::new();
        for _ in 0..70 {
            lights.push(point_light(DVec3::new(0.0, 1.0, 0.0), 4.0));
        }
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &BspTree {
                nodes: Vec::new(),
                leaves: Vec::new(),
            },
            portals: &[],
            exterior_leaves: &HashSet::new(),
        };
        let section = bake_chunk_light_list(&inputs, 8.0, 64).unwrap();
        for entry in &section.offsets {
            assert!(
                entry.count <= 64,
                "chunk retained {} lights; expected <= cap 64",
                entry.count
            );
        }
    }

    #[test]
    fn overflow_retains_independent_influence_top_cap_reference() {
        let mut dim_low_slot = point_light(DVec3::new(6.0, 2.0, 0.0), 30.0);
        dim_low_slot.intensity = 0.01;

        let mut inverse_square = point_light(DVec3::new(6.0, 2.0, 0.0), 30.0);
        inverse_square.intensity = 4.0;
        inverse_square.falloff_model = FalloffModel::InverseSquared;

        let mut dim_contained = point_light(DVec3::new(0.0, 2.0, 0.0), 3.0);
        dim_contained.intensity = 0.0001;

        let mut inverse_distance = point_light(DVec3::new(8.0, 2.0, 0.0), 30.0);
        inverse_distance.intensity = 8.0;
        inverse_distance.falloff_model = FalloffModel::InverseDistance;

        let mut mid_strength = point_light(DVec3::new(10.0, 2.0, 0.0), 30.0);
        mid_strength.intensity = 1.0;

        let lights = vec![
            dim_low_slot,
            inverse_square,
            dim_contained,
            inverse_distance,
            mid_strength,
        ];
        let section = bake_single_quad_chunk_lights(&lights, 8.0, 3);
        let (kept, chunk_min, chunk_max) =
            chunk_slots_and_bounds_at(&section, Vec3::new(0.0, 2.0, 0.0));
        let expected = reference_top_cap_slots(&lights, chunk_min, chunk_max, 3);

        assert_eq!(
            kept, expected,
            "overflow must retain the reference top-cap set"
        );
        assert_ne!(
            kept,
            vec![0, 1, 2],
            "overflow must not fall back to lowest-slot truncation"
        );
    }

    #[test]
    fn overflow_keeps_bright_high_slot_over_dim_low_slots() {
        let mut lights: Vec<MapLight> = (0..4)
            .map(|_| {
                let mut light = point_light(DVec3::new(6.0, 2.0, 0.0), 30.0);
                light.intensity = 0.01;
                light
            })
            .collect();
        let mut bright_high_slot = point_light(DVec3::new(6.0, 2.0, 0.0), 30.0);
        bright_high_slot.intensity = 10.0;
        lights.push(bright_high_slot);

        let section = bake_single_quad_chunk_lights(&lights, 8.0, 1);
        let (kept, _, _) = chunk_slots_and_bounds_at(&section, Vec3::new(0.0, 2.0, 0.0));
        assert_eq!(kept, vec![4]);
    }

    #[test]
    fn boundary_spanning_bright_light_survives_in_adjacent_overflow_chunks() {
        let mut lights: Vec<MapLight> = (0..4)
            .map(|index| {
                let mut light = point_light(DVec3::new(index as f64, 10.0, 0.0), 24.0);
                light.intensity = 1.0;
                light
            })
            .collect();
        // Just left of the shared x=4 chunk face and above both chunks. It is
        // therefore a non-contained candidate on each side, while its large
        // range and intensity make it rank above the cap in both.
        let mut boundary_light = point_light(DVec3::new(3.5, 10.0, 0.0), 24.0);
        boundary_light.intensity = 100.0;
        lights.push(boundary_light);
        let boundary_slot = 4_u32;
        let cap = 2;

        let section = bake_single_quad_chunk_lights(&lights, 8.0, cap);
        let (left_kept, left_min, left_max) =
            chunk_slots_and_bounds_at(&section, Vec3::new(0.0, 0.0, 0.0));
        let (right_kept, right_min, right_max) =
            chunk_slots_and_bounds_at(&section, Vec3::new(8.0, 0.0, 0.0));

        for (chunk_min, chunk_max) in [(left_min, left_max), (right_min, right_max)] {
            assert!(
                lights
                    .iter()
                    .all(|light| overlaps_chunk(light, chunk_min, chunk_max)),
                "each checked chunk must be over-cap from these candidates"
            );
            assert!(
                !is_contained_by_chunk(&lights[boundary_slot as usize], chunk_min, chunk_max),
                "the boundary light must exercise influence ranking, not the contained tier"
            );
        }

        let left_expected = reference_top_cap_slots(&lights, left_min, left_max, cap as usize);
        let right_expected = reference_top_cap_slots(&lights, right_min, right_max, cap as usize);
        assert!(left_expected.contains(&boundary_slot));
        assert!(right_expected.contains(&boundary_slot));
        assert_ne!(left_expected, vec![0, 1]);
        assert_ne!(right_expected, vec![0, 1]);

        assert_eq!(left_kept, left_expected);
        assert_eq!(right_kept, right_expected);
    }

    #[test]
    fn dim_contained_light_survives_over_brighter_non_contained_candidates() {
        let mut lights: Vec<MapLight> = (0..4)
            .map(|_| {
                let mut light = point_light(DVec3::new(6.0, 2.0, 0.0), 30.0);
                light.intensity = 10.0;
                light
            })
            .collect();
        let mut dim_contained = point_light(DVec3::new(0.0, 2.0, 0.0), 3.0);
        dim_contained.intensity = 0.0001;
        lights.push(dim_contained);
        let contained_slot = 4;

        let section = bake_single_quad_chunk_lights(&lights, 8.0, 2);
        let (kept, chunk_min, chunk_max) =
            chunk_slots_and_bounds_at(&section, Vec3::new(0.0, 2.0, 0.0));
        let contained_influence =
            reference_influence(&lights[contained_slot], chunk_min, chunk_max);
        assert!(
            (0..contained_slot).all(|slot| {
                reference_influence(&lights[slot], chunk_min, chunk_max) > contained_influence
            }),
            "the contained light must genuinely be dimmer than every non-contained candidate"
        );
        assert!(kept.contains(&(contained_slot as u32)));
        assert_eq!(
            kept.iter()
                .filter(|&&slot| slot != contained_slot as u32)
                .count(),
            1,
            "the cap must drop brighter non-contained candidates after retaining the contained light"
        );
    }

    #[test]
    fn overflow_of_contained_lights_keeps_lowest_slots_deterministically() {
        let lights: Vec<MapLight> = (0..5)
            .map(|index| point_light(DVec3::new(-3.0 + index as f64 * 0.5, 1.0, -3.0), 3.0))
            .collect();

        let section = bake_single_quad_chunk_lights(&lights, 8.0, 3);
        let (kept, _, _) = chunk_slots_and_bounds_at(&section, Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(kept, vec![0, 1, 2]);
    }

    #[test]
    fn default_cap_bake_records_raised_header_value() {
        let lights = vec![point_light(DVec3::new(0.0, 2.0, 0.0), 3.0)];
        let section = bake_single_quad_chunk_lights(
            &lights,
            DEFAULT_CELL_SIZE_METERS,
            DEFAULT_PER_CHUNK_LIGHT_CAP,
        );

        assert_eq!(DEFAULT_PER_CHUNK_LIGHT_CAP, 256);
        assert_eq!(section.per_chunk_cap, DEFAULT_PER_CHUNK_LIGHT_CAP);
    }

    #[test]
    fn explicit_zero_cap_records_zero_and_emits_empty_chunk_lists() {
        let lights = vec![
            point_light(DVec3::new(0.0, 2.0, 0.0), 3.0),
            point_light(DVec3::new(6.0, 2.0, 0.0), 30.0),
        ];

        let section = bake_single_quad_chunk_lights(&lights, 8.0, 0);

        assert_eq!(section.per_chunk_cap, 0);
        assert!(section.offsets.iter().all(|entry| entry.count == 0));
        assert!(section.light_indices.is_empty());
    }

    #[test]
    fn section_payload_cap_fails_bake() {
        // 16 × 16 m at 0.01 m/cell = 1600×1600×1 = 2.56M chunks × 8 bytes = ~20 MB > cap.
        let dir = fresh_cache_dir("payload_too_large");
        let cache = StageCache::new(&dir).expect("create cache directory");
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light(DVec3::new(0.0, 1.0, 0.0), 4.0)];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &BspTree {
                nodes: Vec::new(),
                leaves: Vec::new(),
            },
            portals: &[],
            exterior_leaves: &HashSet::new(),
        };
        let key = chunk_light_list_cache_key(&inputs, 0.01, 64);
        let err = bake_chunk_light_list_cached(&inputs, 0.01, 64, Some(&cache)).unwrap_err();
        match err {
            ChunkLightListError::PayloadTooLarge { actual, max } => {
                assert!(actual > max);
            }
        }
        assert!(
            cache.get(&key).is_none(),
            "a genuine miss-path bake error must not be cached"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn empty_tree() -> BspTree {
        BspTree {
            nodes: Vec::new(),
            leaves: Vec::new(),
        }
    }

    fn two_leaf_tree_no_portals() -> BspTree {
        // Plane at x = 0: leaf 0 = back (x < 0), leaf 1 = front (x > 0), no portals.
        use crate::partition::{Aabb, BspChild, BspLeaf, BspNode};
        BspTree {
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
                    bounds: Aabb::empty(),
                    is_solid: false,
                    defining_planes: Vec::new(),
                },
                BspLeaf {
                    face_indices: Vec::new(),
                    bounds: Aabb::empty(),
                    is_solid: false,
                    defining_planes: Vec::new(),
                },
            ],
        }
    }

    #[test]
    fn portal_cull_drops_light_from_unreachable_leaf() {
        // BFS reachable set is {0}; chunks in leaf 1 must be dropped. The solid
        // wall also blocks BVH rays here — both filters agree on this case.
        let geo = two_room_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light(DVec3::new(-5.0, 2.0, 0.0), 50.0)];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let tree = two_leaf_tree_no_portals();
        let exterior: HashSet<usize> = HashSet::new();
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };
        let section = bake_chunk_light_list(&inputs, 4.0, 64).unwrap();
        assert_eq!(section.has_grid, 1);

        let origin = Vec3::from(section.grid_origin);
        let cell = section.cell_size;
        let far_point = Vec3::new(5.0, 2.0, 0.0);
        let cx = ((far_point.x - origin.x) / cell).floor() as i32;
        let cy = ((far_point.y - origin.y) / cell).floor() as i32;
        let cz = ((far_point.z - origin.z) / cell).floor() as i32;
        let nx = section.grid_dimensions[0] as i32;
        let ny = section.grid_dimensions[1] as i32;
        let linear = (cz * ny * nx + cy * nx + cx) as usize;
        let count = section.offsets[linear].count;
        assert_eq!(
            count, 0,
            "portal filter must drop the light from the unreachable leaf-1 chunk (got {count})"
        );
    }

    fn two_leaf_tree_with_portal() -> (BspTree, Vec<Portal>) {
        // Plane at x = 0: leaf 0 = back (x < 0), leaf 1 = front (x > 0), one portal.
        use crate::partition::{Aabb, BspChild, BspLeaf, BspNode};
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
                    bounds: Aabb::empty(),
                    is_solid: false,
                    defining_planes: Vec::new(),
                },
                BspLeaf {
                    face_indices: Vec::new(),
                    bounds: Aabb::empty(),
                    is_solid: false,
                    defining_planes: Vec::new(),
                },
            ],
        };
        let portal = Portal {
            polygon: vec![
                DVec3::new(0.0, 0.0, -10.0),
                DVec3::new(0.0, 10.0, -10.0),
                DVec3::new(0.0, 10.0, 10.0),
                DVec3::new(0.0, 0.0, 10.0),
            ],
            front_leaf: 1,
            back_leaf: 0,
        };
        (tree, vec![portal])
    }

    fn two_leaf_tree_solid_back() -> BspTree {
        // Plane at x = 0: leaf 0 = back (x < 0, SOLID), leaf 1 = front (x > 0).
        // Drives a chunk centroid into a solid leaf to exercise the bypass path.
        use crate::partition::{Aabb, BspChild, BspLeaf, BspNode};
        BspTree {
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
                    bounds: Aabb::empty(),
                    is_solid: true,
                    defining_planes: Vec::new(),
                },
                BspLeaf {
                    face_indices: Vec::new(),
                    bounds: Aabb::empty(),
                    is_solid: false,
                    defining_planes: Vec::new(),
                },
            ],
        }
    }

    #[test]
    fn chunk_centroid_in_solid_leaf_bypasses_portal_filter() {
        // Light in leaf 1 (x > 0); BFS reachable set is {1}. Without solid-leaf
        // bypass the portal filter would reject the chunk whose centroid falls in
        // solid leaf 0 (x < 0), even though sphere overlap and BVH rays are clear.
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let light = point_light(DVec3::new(4.0, 4.0, -4.0), 50.0);
        let lights = vec![light];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let tree = two_leaf_tree_solid_back();
        let exterior: HashSet<usize> = HashSet::new();
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };
        let section = bake_chunk_light_list(&inputs, 8.0, 64).unwrap();
        assert_eq!(section.has_grid, 1);

        let probe = Vec3::new(-4.0, 4.0, -4.0); // centroid in solid leaf 0
        let origin = Vec3::from(section.grid_origin);
        let cell = section.cell_size;
        let cx = ((probe.x - origin.x) / cell).floor() as i32;
        let cy = ((probe.y - origin.y) / cell).floor() as i32;
        let cz = ((probe.z - origin.z) / cell).floor() as i32;
        let nx = section.grid_dimensions[0] as i32;
        let ny = section.grid_dimensions[1] as i32;
        assert!(cx >= 0 && cy >= 0 && cz >= 0);
        let linear = (cz * ny * nx + cy * nx + cx) as usize;
        let entry = section.offsets[linear];
        assert_eq!(
            entry.count, 1,
            "solid-leaf bypass must let the spatial+BVH-clear light through (got {})",
            entry.count
        );
        let slot = section.light_indices[entry.offset as usize];
        assert_eq!(
            slot, 0,
            "expected the only static light's slot in the bucket"
        );
    }

    #[test]
    fn light_reaches_chunk_in_adjacent_leaf_through_portal() {
        // Open geometry (floor quad only, no wall) isolates the portal-BFS path
        // from the wall-occlusion path tested in portal_cull_drops_light_from_unreachable_leaf.
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let light = point_light(DVec3::new(-4.0, 4.0, -4.0), 50.0);
        let lights = vec![light];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let (tree, portals) = two_leaf_tree_with_portal();
        let exterior: HashSet<usize> = HashSet::new();
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &tree,
            portals: &portals,
            exterior_leaves: &exterior,
        };
        let section = bake_chunk_light_list(&inputs, 8.0, 64).unwrap();
        assert_eq!(section.has_grid, 1);

        let probe = Vec3::new(4.0, 4.0, -4.0); // centroid in leaf 1
        let origin = Vec3::from(section.grid_origin);
        let cell = section.cell_size;
        let cx = ((probe.x - origin.x) / cell).floor() as i32;
        let cy = ((probe.y - origin.y) / cell).floor() as i32;
        let cz = ((probe.z - origin.z) / cell).floor() as i32;
        let nx = section.grid_dimensions[0] as i32;
        let ny = section.grid_dimensions[1] as i32;
        assert!(cx >= 0 && cy >= 0 && cz >= 0);
        let linear = (cz * ny * nx + cy * nx + cx) as usize;
        let entry = section.offsets[linear];
        assert_eq!(
            entry.count, 1,
            "portal BFS must reach the adjacent leaf (got {})",
            entry.count
        );
        let slot = section.light_indices[entry.offset as usize];
        assert_eq!(slot, 0);
    }

    /// Regression for the dynamic-light index-skew bug: the baker must emit
    /// `light_indices` in the COMPACTED `!is_dynamic` slot space that
    /// `pack_spec_lights` produces — NOT the AlphaLights slot space (which
    /// counts dynamic lights). When a dynamic light precedes a static/SDF light,
    /// the AlphaLights slot of the SDF light is 1, but its compacted spec_lights
    /// slot is 0 (the dynamic light is skipped with no placeholder). The runtime
    /// indexes `spec_lights[light_idx]`, so the baker must emit the compacted
    /// index or every light after a dynamic one reads the wrong record (SDF
    /// lights dropped from selection, static specular mis-read).
    ///
    /// The contract is pinned by reconstructing the same compaction
    /// `pack_spec_lights` applies (`!is_dynamic`, iteration order, no
    /// placeholder) over the runtime light list, then asserting the emitted
    /// index lands on the intended SDF light in that compacted array. We pin at
    /// the baker level (rather than calling `pack_spec_lights`, which lives in
    /// `postretro-lighting` and would cross a crate boundary) by mirroring its
    /// filter here; `pack_spec_lights` has its own `skips_dynamic_lights` test
    /// holding up the other half of the seam.
    #[test]
    fn emitted_index_is_compacted_spec_slot_when_dynamic_precedes_sdf() {
        // AlphaLights order: [dynamic, sdf-static]. AlphaLights slot of the SDF
        // light is 1; its compacted (!is_dynamic) spec_lights slot is 0.
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![
            dynamic_point_light(DVec3::new(0.0, 1.0, 0.0), 4.0),
            sdf_point_light(DVec3::new(0.0, 1.0, 0.0), 4.0),
        ];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &empty_tree(),
            portals: &[],
            exterior_leaves: &HashSet::new(),
        };
        let section = bake_chunk_light_list(&inputs, 8.0, 64).unwrap();
        assert_eq!(section.has_grid, 1);

        // Mirror `pack_spec_lights`: the compacted spec_lights view of the
        // runtime light list (filter !is_dynamic, preserve order, no placeholder).
        let spec_lights: Vec<&MapLight> = lights.iter().filter(|l| !l.is_dynamic).collect();

        // Every emitted index must point at the SDF light through the compacted
        // array — i.e. `spec_lights[emitted].shadow_type == Sdf`. The pre-fix
        // baker emitted AlphaLights slot 1, which is out of range of the
        // single-entry compacted array (the bug), or in larger sets the wrong
        // record.
        assert!(
            !section.light_indices.is_empty(),
            "the SDF light should land in at least one chunk"
        );
        for &emitted in &section.light_indices {
            let slot = emitted as usize;
            assert!(
                slot < spec_lights.len(),
                "emitted index {slot} is out of range of the compacted spec_lights \
                 array (len {}) — this is the AlphaLights-vs-compacted skew bug",
                spec_lights.len(),
            );
            assert_eq!(
                spec_lights[slot].shadow_type,
                ShadowType::Sdf,
                "emitted compacted index {slot} must resolve to the SDF light, \
                 not a different spec_lights record"
            );
        }
    }

    /// The baker emits a contiguous compacted index sequence over the
    /// non-dynamic subset: with [static, dynamic, sdf] the only valid
    /// spec_lights slots are 0 and 1 (the two non-dynamic lights), never 2.
    #[test]
    fn emitted_indices_are_contiguous_over_non_dynamic_subset() {
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![
            point_light(DVec3::new(0.0, 1.0, 0.0), 4.0), // compacted slot 0
            dynamic_point_light(DVec3::new(0.0, 1.0, 0.0), 4.0), // skipped
            sdf_point_light(DVec3::new(0.0, 1.0, 0.0), 4.0), // compacted slot 1
        ];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &empty_tree(),
            portals: &[],
            exterior_leaves: &HashSet::new(),
        };
        let section = bake_chunk_light_list(&inputs, 8.0, 64).unwrap();
        let non_dynamic_count = lights.iter().filter(|l| !l.is_dynamic).count() as u32;
        for &emitted in &section.light_indices {
            assert!(
                emitted < non_dynamic_count,
                "emitted index {emitted} exceeds the {non_dynamic_count} non-dynamic \
                 spec_lights slots (AlphaLights slot would have been 2 for the SDF light)"
            );
        }
    }

    #[test]
    fn portal_filter_bypassed_for_empty_tree() {
        // Empty BspTree: find_leaf_for_point returns 0 everywhere, BFS reachable
        // set = {0}, every chunk centroid maps to 0 — no chunk is filtered.
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light(DVec3::new(0.0, 1.0, 0.0), 4.0)];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let tree = empty_tree();
        let exterior: HashSet<usize> = HashSet::new();
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &tree,
            portals: &[],
            exterior_leaves: &exterior,
        };
        let section = bake_chunk_light_list(&inputs, 4.0, 64).unwrap();
        assert_eq!(section.has_grid, 1);
        let total: u32 = section.offsets.iter().map(|e| e.count).sum();
        assert!(
            total >= 1,
            "no-portal degenerate case must not filter out all assignments"
        );
    }

    /// Quake-unit -> engine-meter transform (mirrors `parse::quake_to_engine`
    /// plus the IdTech2 0.0254 m scale): `ex = -qy·s, ey = qz·s, ez = -qx·s`.
    #[cfg(test)]
    fn quake_to_engine(qx: f32, qy: f32, qz: f32) -> Vec3 {
        const S: f32 = 0.0254;
        Vec3::new(-qy * S, qz * S, -qx * S)
    }

    /// Verifies the per-chunk SDF light SELECTION lists the `_shadow_type "sdf"`
    /// light for the floor it shadows, asserted against the known geometry of the
    /// purpose-built `sdf-shadow-test.map` fixture (generated by
    /// `tools/gen_sdf_shadow_fixture.py`).
    ///
    /// The bake's chunk light list is not a pure influence-sphere overlap: it
    /// also applies a portal-reachability filter and a BVH shadow-ray filter, so
    /// an SDF light can be DROPPED from a floor cell even when its range covers
    /// the floor. The fixture's light and floor sit in one sealed room within
    /// range, so the SDF light MUST survive into the shadowed-floor cell and be
    /// selected there. The tiny fixture bakes in-process quickly, so this is a
    /// normal (non-`#[ignore]`) test.
    #[test]
    fn chunk_light_list_includes_sdf_light_for_shadowed_floor() {
        use crate::map_format::MapFormat;
        use std::collections::HashSet;

        const K: usize = SDF_SHADOW_K;

        // 1. Build the map + chunk-light-list the SAME way prl-build does.
        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .join("content/dev/maps/sdf-shadow-test.map");
        let map_data = crate::parse::parse_map_file(&map_path, MapFormat::IdTech2)
            .expect("sdf-shadow-test.map should parse");
        let result =
            crate::partition::partition(&map_data.brush_volumes).expect("partition should succeed");
        let portals = crate::portals::generate_portals(&result.tree);
        let exterior: HashSet<usize> =
            crate::visibility::find_exterior_leaves(&result.tree, &portals);
        let geo_result = crate::geometry::extract_geometry(&result.faces, &result.tree, &exterior);
        let (bvh, prims, _) = build_bvh(&geo_result).expect("bvh build should succeed");
        let alpha_lights = AlphaLightsNs::from_lights(&map_data.lights);

        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo_result,
            lights: &alpha_lights,
            tree: &result.tree,
            portals: &portals,
            exterior_leaves: &exterior,
        };
        let section = bake_chunk_light_list(
            &inputs,
            DEFAULT_CELL_SIZE_METERS,
            DEFAULT_PER_CHUNK_LIGHT_CAP,
        )
        .expect("chunk light list bake should succeed");
        assert_eq!(
            section.has_grid, 1,
            "fixture must produce an authored chunk grid",
        );

        // 2. Build the COMPACTED spec_lights view the runtime indexes into:
        //    filter `!is_dynamic`, preserve order (matches pack_spec_lights).
        struct Spec {
            slot: u32,
            pos: Vec3,
            range: f32,
            peak: f32,
            is_sdf: bool,
        }
        let specs: Vec<Spec> = map_data
            .lights
            .iter()
            .filter(|l| !l.is_dynamic)
            .enumerate()
            .map(|(slot, l)| Spec {
                slot: slot as u32,
                pos: Vec3::new(l.origin.x as f32, l.origin.y as f32, l.origin.z as f32),
                range: l.falloff_range,
                peak: (l.color[0] * l.intensity)
                    .max(l.color[1] * l.intensity)
                    .max(l.color[2] * l.intensity),
                is_sdf: l.shadow_type == ShadowType::Sdf,
            })
            .collect();
        let sdf = specs
            .iter()
            .find(|s| s.is_sdf)
            .expect("fixture must author one Sdf-shadow light");

        // CPU mirror of `sdf_select_influence`: range cull then atten*peak.
        let influence = |s: &Spec, world: Vec3| -> f32 {
            let dist = (s.pos - world).length();
            if s.range > 0.0 && dist > s.range {
                return 0.0;
            }
            let atten = if s.range > 0.0 {
                (1.0 - dist / s.range.max(0.001)).max(0.0)
            } else {
                1.0
            };
            atten * s.peak
        };

        // Resolve the chunk-grid cell linear index for a world position.
        let cell_index = |world: Vec3| -> Option<usize> {
            let origin = Vec3::from(section.grid_origin);
            let local = world - origin;
            let cx = (local.x / section.cell_size).floor() as i32;
            let cy = (local.y / section.cell_size).floor() as i32;
            let cz = (local.z / section.cell_size).floor() as i32;
            let dims = section.grid_dimensions;
            if cx < 0
                || cy < 0
                || cz < 0
                || cx >= dims[0] as i32
                || cy >= dims[1] as i32
                || cz >= dims[2] as i32
            {
                return None;
            }
            Some(
                cz as usize * dims[1] as usize * dims[0] as usize
                    + cy as usize * dims[0] as usize
                    + cx as usize,
            )
        };

        // CPU mirror of `select_sdf_lights` over the BAKED per-cell window.
        let select = |world: Vec3| -> Vec<u32> {
            let ci = cell_index(world).expect("shadowed floor must lie inside the chunk grid");
            let entry = section.offsets[ci];
            let start = entry.offset as usize;
            let end = start + entry.count as usize;
            let mut cands: Vec<(u32, f32)> = section.light_indices[start..end]
                .iter()
                .filter_map(|&slot| {
                    let s = specs.get(slot as usize)?;
                    if !s.is_sdf {
                        return None;
                    }
                    let inf = influence(s, world);
                    if inf <= 0.0 { None } else { Some((slot, inf)) }
                })
                .collect();
            cands.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then_with(|| a.0.cmp(&b.0)));
            cands.truncate(K);
            cands.into_iter().map(|(slot, _)| slot).collect()
        };

        // 3. The shadowed floor surface receiver (just above the floor, under the
        //    occluder). Walk down from above the floor to the first air->solid
        //    transition; the air sample above it is the floor-surface receiver
        //    the forward shader shades — robust to a half-voxel offset.
        let in_solid = |p: Vec3| -> bool {
            let leaf =
                find_leaf_for_point(&result.tree, DVec3::new(p.x as f64, p.y as f64, p.z as f64));
            result
                .tree
                .leaves
                .get(leaf)
                .map(|l| l.is_solid)
                .unwrap_or(false)
        };
        let step = 0.05_f32;
        let down = Vec3::new(0.0, -step, 0.0);
        let floor_top = quake_to_engine(416.0, 256.0, 0.0).y;
        let mut surface = quake_to_engine(416.0, 256.0, 40.0);
        let mut prev_air = !in_solid(surface);
        let mut p = surface;
        while p.y >= floor_top - 1.0 {
            let solid = in_solid(p);
            if prev_air && solid {
                surface = p - down; // the air sample one step above the floor
                break;
            }
            prev_air = !solid;
            p += down;
        }
        assert!(
            !in_solid(surface),
            "floor-surface receiver {surface:?} must be in air",
        );

        // 4. The SDF light's compacted slot must be present in the shadowed
        //    floor cell's baked list, and the influence selection must choose it.
        let ci = cell_index(surface).expect("shadowed floor must lie inside the chunk grid");
        let entry = section.offsets[ci];
        let start = entry.offset as usize;
        let end = start + entry.count as usize;
        let in_cell = &section.light_indices[start..end];
        assert!(
            in_cell.contains(&sdf.slot),
            "SDF light slot {} must survive the bake's portal/BVH filter into the \
             shadowed-floor cell (cell holds {in_cell:?})",
            sdf.slot,
        );

        let selected = select(surface);
        assert!(
            selected.contains(&sdf.slot),
            "the per-chunk SDF selection must include the SDF light for the shadowed \
             floor receiver {surface:?} (selected {selected:?})",
        );

        // Sanity: the light is genuinely in range (selection is not vacuous).
        let dist = (sdf.pos - surface).length();
        const EPS: f32 = 1.0e-4;
        assert!(
            sdf.range > EPS && dist < sdf.range,
            "SDF light range {} must cover the floor->light distance {dist}",
            sdf.range,
        );
    }

    /// Every cell a light's influence sphere touches must KEEP the light on an
    /// open floor — even though the grid's half-cell pad puts every ground
    /// cell's midheight (and thus the sample-box y, clamped to the floor-only
    /// geometry AABB) exactly ON the floor plane. Under the old center + eight
    /// inset-corner sampling those grazing rays could read as occlusion by
    /// float luck, randomly cutting cell-sized box holes out of the light
    /// (observed on combat-demo: the sdf spot was dropped from the cell
    /// containing it).
    /// `SAMPLE_END_TOLERANCE_METERS` makes an endpoint graze read as the
    /// receiving surface, not an occluder.
    #[test]
    fn open_floor_cells_in_range_always_keep_the_light() {
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        // Ceiling-spot geometry from the observed repro: light 2.5 m above the
        // floor, short falloff, nothing occluding anywhere.
        let light_pos = DVec3::new(0.0, 2.5, 0.0);
        let range = 3.8_f32;
        let lights = vec![point_light(light_pos, range)];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &empty_tree(),
            portals: &[],
            exterior_leaves: &HashSet::new(),
        };
        let section = bake_chunk_light_list(&inputs, 4.0, 64).unwrap();
        assert_eq!(section.has_grid, 1);

        let origin = Vec3::from(section.grid_origin);
        let cell = section.cell_size;
        let center = Vec3::new(light_pos.x as f32, light_pos.y as f32, light_pos.z as f32);
        let [nx, ny, nz] = section.grid_dimensions;
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    let cmin = origin + Vec3::new(x as f32, y as f32, z as f32) * cell;
                    let cmax = cmin + Vec3::splat(cell);
                    let closest = center.clamp(cmin, cmax);
                    if (closest - center).length_squared() > range * range {
                        continue; // outside influence — not required to hold it
                    }
                    let linear = (z * ny * nx + y * nx + x) as usize;
                    let count = section.offsets[linear].count;
                    assert_eq!(
                        count, 1,
                        "unoccluded in-range cell ({x},{y},{z}) [{cmin:?}..{cmax:?}] \
                         must keep the light"
                    );
                }
            }
        }
    }

    /// The contains-light guard survives per-chunk cap truncation: plain
    /// `truncate(cap)` is slot-index biased and would evict the highest slots
    /// first — including a contained light, re-cutting the box hole the guard
    /// prevents. Contained slots are partitioned to the front before the cut.
    #[test]
    fn contained_light_survives_per_chunk_cap_truncation() {
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        // 69 lights OUTSIDE the target cell (origins at x = 6 > cell max 4)
        // whose range overlaps it, then the contained light LAST — the highest
        // compacted slot, first in line for a slot-biased eviction.
        let mut lights: Vec<MapLight> = (0..69)
            .map(|_| point_light(DVec3::new(6.0, 2.0, 0.0), 30.0))
            .collect();
        lights.push(point_light(DVec3::new(0.0, 2.0, 0.0), 3.0));
        let contained_slot = 69u32;
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &empty_tree(),
            portals: &[],
            exterior_leaves: &HashSet::new(),
        };
        let section = bake_chunk_light_list(&inputs, 8.0, 64).unwrap();
        assert_eq!(section.has_grid, 1);

        let origin = Vec3::from(section.grid_origin);
        let cell = section.cell_size;
        let center = Vec3::new(0.0, 2.0, 0.0);
        let cx = ((center.x - origin.x) / cell).floor() as u32;
        let cy = ((center.y - origin.y) / cell).floor() as u32;
        let cz = ((center.z - origin.z) / cell).floor() as u32;
        let [nx, ny, _] = section.grid_dimensions;
        let linear = (cz * ny * nx + cy * nx + cx) as usize;
        let entry = section.offsets[linear];
        assert_eq!(entry.count, 64, "cell must be clamped to the cap");
        let start = entry.offset as usize;
        let kept = &section.light_indices[start..start + entry.count as usize];
        assert!(
            kept.contains(&contained_slot),
            "the contained light (slot {contained_slot}) must survive cap truncation"
        );
    }

    /// The cell CONTAINING a light keeps it even when every visibility sample
    /// is occluded: a light boxed in by geometry still lights the box's own
    /// interior surfaces, which live in that same cell. The former all-rays
    /// cull keyed only on proxy points outside the enclosure and dropped the
    /// light from its own cell.
    #[test]
    fn cell_containing_the_light_survives_full_sample_occlusion() {
        // Floor plus a closed 1 m cube shell centered on the light. Every
        // sample point of the containing cell lies outside the shell, so all
        // 9 rays are blocked.
        let mut geo = single_quad_geometry();
        {
            let geom = &mut geo.geometry;
            let (lo, hi) = (Vec3::new(-0.5, 1.5, -0.5), Vec3::new(0.5, 2.5, 0.5));
            let corners = [
                Vec3::new(lo.x, lo.y, lo.z),
                Vec3::new(hi.x, lo.y, lo.z),
                Vec3::new(hi.x, hi.y, lo.z),
                Vec3::new(lo.x, hi.y, lo.z),
                Vec3::new(lo.x, lo.y, hi.z),
                Vec3::new(hi.x, lo.y, hi.z),
                Vec3::new(hi.x, hi.y, hi.z),
                Vec3::new(lo.x, hi.y, hi.z),
            ];
            // 6 faces as corner-index quads (winding irrelevant: the ray test
            // is double-sided).
            let quads: [[usize; 4]; 6] = [
                [0, 1, 2, 3], // -z
                [4, 5, 6, 7], // +z
                [0, 3, 7, 4], // -x
                [1, 2, 6, 5], // +x
                [0, 1, 5, 4], // -y
                [3, 2, 6, 7], // +y
            ];
            for q in quads {
                let base = geom.vertices.len() as u32;
                for &ci in &q {
                    let p = corners[ci];
                    geom.vertices.push(Vertex::new(
                        [p.x, p.y, p.z],
                        [0.0, 0.0],
                        [0.0, 1.0, 0.0],
                        [1.0, 0.0, 0.0],
                        true,
                        [0.0, 0.0],
                        0,
                    ));
                }
                let start = geom.indices.len() as u32;
                geom.indices.extend_from_slice(&[
                    base,
                    base + 1,
                    base + 2,
                    base,
                    base + 2,
                    base + 3,
                ]);
                geom.faces.push(FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                });
                geo.face_index_ranges.push(FaceIndexRange {
                    index_offset: start,
                    index_count: 6,
                });
            }
        }
        let (bvh, prims, _) = build_bvh(&geo).unwrap();

        let light_pos = DVec3::new(0.0, 2.0, 0.0); // inside the shell
        let lights = vec![point_light(light_pos, 3.0)];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &alpha_lights,
            tree: &empty_tree(),
            portals: &[],
            exterior_leaves: &HashSet::new(),
        };
        let section = bake_chunk_light_list(&inputs, 8.0, 64).unwrap();
        assert_eq!(section.has_grid, 1);

        let origin = Vec3::from(section.grid_origin);
        let cell = section.cell_size;
        let center = Vec3::new(0.0, 2.0, 0.0);
        let cx = ((center.x - origin.x) / cell).floor() as u32;
        let cy = ((center.y - origin.y) / cell).floor() as u32;
        let cz = ((center.z - origin.z) / cell).floor() as u32;
        let [nx, ny, _] = section.grid_dimensions;
        let linear = (cz * ny * nx + cy * nx + cx) as usize;
        assert_eq!(
            section.offsets[linear].count, 1,
            "the cell containing the boxed-in light must keep it"
        );
    }

    // Regression: diagnosis found six pre-fix pairs under volume proxies; this
    // oracle covers vertex-in-chunk cases, while Phase 1 clipping also covers
    // spanning triangles without an in-chunk vertex.
    #[test]
    #[ignore = "campaign-test is a slow, independent receiver-cull oracle"]
    fn campaign_test_vertex_receivers_in_range_are_not_omitted_from_chunk_light_lists() {
        let fixture = load_fixture("campaign-test");
        let portals = generate_portals(&fixture.tree);
        let alpha_lights = AlphaLightsNs::from_lights(&fixture.lights);
        let inputs = ChunkLightListInputs {
            bvh: &fixture.bvh,
            primitives: &fixture.primitives,
            geometry: &fixture.geometry,
            lights: &alpha_lights,
            tree: &fixture.tree,
            portals: &portals,
            exterior_leaves: &fixture.exterior_leaves,
        };
        let section = bake_chunk_light_list(
            &inputs,
            DEFAULT_CELL_SIZE_METERS,
            DEFAULT_PER_CHUNK_LIGHT_CAP,
        )
        .expect("campaign-test chunk light list bake should succeed");
        assert_eq!(
            section.has_grid, 1,
            "campaign-test must produce a chunk grid"
        );

        // This must stay in the bake's compacted static-light slot order, not
        // AlphaLights source order: `light_indices` addresses this exact view.
        let static_lights = compacted_static_lights(&inputs);
        let vertices = &fixture.geometry.geometry.vertices;
        let indices = &fixture.geometry.geometry.indices;
        let origin = Vec3::from(section.grid_origin);
        let [nx, ny, nz] = section.grid_dimensions;
        let mut false_negatives = Vec::new();

        for (slot, light) in static_lights.into_iter().enumerate() {
            if !matches!(light.light_type, LightType::Point | LightType::Spot)
                || light.falloff_range <= 0.0
            {
                continue;
            }
            let light_position = Vec3::new(
                light.origin.x as f32,
                light.origin.y as f32,
                light.origin.z as f32,
            );
            let range_squared = light.falloff_range * light.falloff_range;

            for z in 0..nz {
                for y in 0..ny {
                    for x in 0..nx {
                        let chunk_index = (z * nx * ny + y * nx + x) as usize;
                        let entry = section.offsets[chunk_index];
                        let start = entry.offset as usize;
                        let baked_slots =
                            &section.light_indices[start..start + entry.count as usize];
                        if baked_slots.contains(&(slot as u32)) {
                            continue;
                        }

                        let chunk_min =
                            origin + Vec3::new(x as f32, y as f32, z as f32) * section.cell_size;
                        let chunk_max = chunk_min + Vec3::splat(section.cell_size);
                        if !overlaps_chunk(light, chunk_min, chunk_max) {
                            continue;
                        }

                        let mut clear_receiver_found = false;
                        for triangle_indices in indices.chunks_exact(3) {
                            let triangle = [
                                Vec3::from(vertices[triangle_indices[0] as usize].position),
                                Vec3::from(vertices[triangle_indices[1] as usize].position),
                                Vec3::from(vertices[triangle_indices[2] as usize].position),
                            ];
                            let normal = (triangle[1] - triangle[0])
                                .cross(triangle[2] - triangle[0])
                                .normalize_or_zero();
                            if normal == Vec3::ZERO {
                                continue;
                            }

                            for receiver in triangle {
                                if !receiver.cmpge(chunk_min).all()
                                    || !receiver.cmple(chunk_max).all()
                                    || (light_position - receiver).length_squared() > range_squared
                                {
                                    continue;
                                }
                                if normal.dot((light_position - receiver).normalize_or_zero())
                                    <= RAY_EPSILON
                                {
                                    continue;
                                }

                                // Independent oracle: brute-force every geometry
                                // triangle, with the same end-graze allowance that
                                // prevents the receiver's own triangle blocking it.
                                let delta = receiver - light_position;
                                let length = delta.length();
                                if length <= RAY_EPSILON {
                                    clear_receiver_found = true;
                                    break;
                                }
                                let direction = delta / length;
                                let ray_origin = light_position + direction * RAY_EPSILON;
                                let max_distance =
                                    length - SAMPLE_END_TOLERANCE_METERS.max(RAY_EPSILON);
                                let blocked = max_distance > 0.0
                                    && indices.chunks_exact(3).any(|occluder_indices| {
                                        let a = Vec3::from(
                                            vertices[occluder_indices[0] as usize].position,
                                        );
                                        let b = Vec3::from(
                                            vertices[occluder_indices[1] as usize].position,
                                        );
                                        let c = Vec3::from(
                                            vertices[occluder_indices[2] as usize].position,
                                        );
                                        ray_triangle_hit(ray_origin, direction, a, b, c)
                                            .is_some_and(|distance| distance < max_distance)
                                    });
                                if !blocked {
                                    clear_receiver_found = true;
                                    break;
                                }
                            }
                            if clear_receiver_found {
                                break;
                            }
                        }

                        if clear_receiver_found {
                            false_negatives.push((slot, x, y, z));
                        }
                    }
                }
            }
        }

        assert!(
            false_negatives.is_empty(),
            "campaign-test omitted visible static-light receiver pairs: {false_negatives:?}"
        );
    }
}
