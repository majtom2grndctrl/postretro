// Per-chunk static-light list builder (ChunkLightList section).
// See: context/lib/build_pipeline.md (PRL sections table)

use bvh::bvh::Bvh;
use bvh::ray::Ray;
use glam::{DVec3, Vec3};
use nalgebra::{Point3, Vector3};
use postretro_level_format::chunk_light_list::{
    ChunkEntry, ChunkLightListSection, DEFAULT_PER_CHUNK_CAP,
};
use std::collections::{HashMap, HashSet, VecDeque};
use thiserror::Error;

use crate::bvh_build::BvhPrimitive;
use crate::cache::{CacheKey, StageCache};
use crate::geometry::GeometryResult;
use crate::light_namespaces::AlphaLightsNs;
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
pub const CHUNK_LIGHT_LIST_STAGE_VERSION: u32 = 1;

/// Cap total `offset table + index list` memory at 16 MB.
pub const MAX_SECTION_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

/// Offset along ray direction to avoid self-intersection on the emitting surface.
const RAY_EPSILON: f32 = 1.0e-3;

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
/// a cell exceeds K. See `context/plans/done/sdf-per-light-shadows/`.
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
            Ok(section) => {
                log::info!("[cache] chunk_light_list hit");
                return Ok(section);
            }
            Err(error) => {
                log::warn!("[cache] corrupt chunk_light_list entry, re-baking: {error}");
            }
        }
    } else {
        log::info!("[cache] chunk_light_list miss");
    }

    // Keep the baker's error surface intact: a genuine PayloadTooLarge miss
    // remains a compiler error and is never written as a soft cache result.
    let section = bake_chunk_light_list(inputs, cell_size_meters, per_chunk_cap)?;
    cache.put(&key, &section.to_bytes());
    Ok(section)
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

    let cell = cell_size_meters.max(1.0e-3);

    // Pad the grid bounds outward by HALF a cell on every side. Without padding,
    // `grid_origin` sits FLUSH with the lowest rendered surface (the geometry-AABB
    // min) — e.g. a pit floor whose surface y equals `grid_origin.y`. The full-res
    // forward shader selects SDF lights at the exact fragment position, so a
    // flush-boundary floor lands in cell 0 and is lit; but the half-res SDF shadow
    // pass selects at a depth-RECONSTRUCTED half-res position whose sub-meter error
    // can tip that same floor to cell index -1 ("outside grid → no lights"),
    // writing no shadow and leaving the floor reading fully lit. (forward.wgsl's
    // "Task 4 visual check" note documents this exact full-vs-half-res selection
    // disagreement at a chunk_grid cell boundary.)
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
    let (geo_min, geo_max) = world_aabb(inputs.geometry);
    let pad = Vec3::splat(cell * 0.5);
    let world_min = geo_min - pad;
    let world_max = geo_max + pad;
    let extent = (world_max - world_min).max(Vec3::splat(cell));
    let dims = [
        ((extent.x / cell).ceil() as u32).max(1),
        ((extent.y / cell).ceil() as u32).max(1),
        ((extent.z / cell).ceil() as u32).max(1),
    ];
    let nx = dims[0] as usize;
    let ny = dims[1] as usize;
    let nz = dims[2] as usize;
    let chunk_count = nx * ny * nz;

    let cap = per_chunk_cap.max(1) as usize;

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
                // (static_slots order). Consulted at cap truncation so a
                // contained light is never the one evicted.
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
                    // too (contained slots are partitioned to the front before
                    // truncation below); it yields only past `cap` contained
                    // lights in one cell.
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
                    if !any_ray_unoccluded(
                        inputs.bvh,
                        inputs.primitives,
                        inputs.geometry,
                        light,
                        (chunk_min, chunk_max),
                        (geo_min, geo_max),
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
                    // Plain truncation is slot-index biased: it would evict the
                    // highest slots, including a light the contains-guard just
                    // promised to keep — re-cutting the box hole the guard
                    // exists to prevent. Stable-partition contained lights to
                    // the front so eviction only reaches them after every
                    // non-contained light is gone. Bucket order is otherwise
                    // free: both runtime consumers are order-independent (the
                    // sdf K-selection re-ranks by influence; the specular loop
                    // sums the whole window).
                    if !contained_slots.is_empty() {
                        // `contained_slots` is ascending — binary_search is the
                        // membership test.
                        bucket.sort_by_key(|s| contained_slots.binary_search(s).is_err());
                    }
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
        per_chunk_cap: per_chunk_cap.max(1),
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

/// Returns `true` if any of 9 shadow rays (sample-box center + 8 inset corners,
/// see `sample_points`) reaches the light unoccluded. Directional lights cast
/// from sample toward sun, mirroring the `segment_clear`-based hard-ray
/// approach used by `lightmap_bake::soft_visibility`'s zero-size short-circuit.
///
/// This is a CONSERVATIVE cull: it answers "could any receiver in this cell
/// possibly see the light". A false KEEP costs one redundant runtime candidate;
/// a false DROP is a cell-sized (8 m) hole cut out of the light — an `sdf`
/// light's runtime direct term (and every static light's specular) reads this
/// list per fragment, so a wrong drop is directly visible. Err lit.
fn any_ray_unoccluded(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    light: &MapLight,
    chunk_bounds: (Vec3, Vec3),
    world_bounds: (Vec3, Vec3),
) -> bool {
    let samples = sample_points(light, chunk_bounds, world_bounds);
    for sample in samples {
        if segment_clear(bvh, primitives, geometry, light, sample) {
            return true;
        }
    }
    false
}

/// Visibility sample points for a (light, cell) pair: the center + 8 corners of
/// the cell AABB clipped to the light's influence AABB (`origin ± falloff` —
/// the only region where the light can matter) and to the geometry AABB (the
/// only region where receivers can exist), with the corners inset off the box
/// faces.
///
/// The former scheme (cell centroid + 3 light-facing face midpoints) sampled
/// cell-geometry landmarks that routinely DEGENERATE onto world geometry: with
/// the grid's half-cell pad, a ground-layer cell's midheight sits exactly on
/// the floor plane of any map whose geometry min is the floor, face midpoints
/// sit on cell faces (which walls love to align with), and the vertical face
/// midpoint lands above low ceilings. All four rays then graze or cross
/// geometry and the light is dropped from a cell it plainly illuminates — the
/// observed failure dropped a light from the very cell CONTAINING it, cutting
/// a box-shaped hole out of the light. The influence/world clip keeps every
/// sample where visibility is actually at stake, and the inset keeps corners
/// off planes that coincide with cell or world faces.
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
    use crate::geometry::FaceIndexRange;
    use crate::map_data::{FalloffModel, LightType, MapLight};
    use glam::DVec3;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

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
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "postretro_chunk_light_list_cache_test_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&dir);
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
        let rebaked = bake_chunk_light_list_cached(&inputs, 8.0, 64, Some(&cache))
            .expect("corrupt cache entry must be a soft miss")
            .to_bytes();
        assert_eq!(
            rebaked, uncached,
            "corrupt cache entry must re-bake exact bytes"
        );

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
    fn occluded_chunk_drops_light() {
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

        let far_point = Vec3::new(5.0, 2.0, 0.0); // deep in Room B
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
            "expected the far chunk to see no lights through the wall, got {count}"
        );
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
    fn section_payload_cap_fails_bake() {
        // 16 × 16 m at 0.01 m/cell = 1600×1600×1 = 2.56M chunks × 8 bytes = ~20 MB > cap.
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
        let err = bake_chunk_light_list(&inputs, 0.01, 64).unwrap_err();
        match err {
            ChunkLightListError::PayloadTooLarge { actual, max } => {
                assert!(actual > max);
            }
        }
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
    /// geometry AABB) exactly ON the floor plane. Under the old centroid + 3
    /// face-midpoint sampling those grazing rays read as occlusion by float
    /// luck, randomly cutting cell-sized box holes out of the light (observed
    /// on combat-demo: the sdf spot was dropped from the cell containing it).
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
}
