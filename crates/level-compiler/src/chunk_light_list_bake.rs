// Per-chunk static-light list builder.
// See: lighting-chunk-lists/

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
use crate::geometry::GeometryResult;
use crate::light_namespaces::AlphaLightsNs;
use crate::map_data::{LightType, MapLight, ShadowType};
use crate::partition::{BspTree, find_leaf_for_point};
use crate::portals::Portal;

/// Default chunk edge length in meters. Small enough that per-chunk buckets
/// stay sparse; large enough that the grid does not explode on larger maps.
pub const DEFAULT_CELL_SIZE_METERS: f32 = 8.0;

pub const DEFAULT_PER_CHUNK_LIGHT_CAP: u32 = DEFAULT_PER_CHUNK_CAP;

/// Cap total `offset table + index list` memory at 16 MB.
pub const MAX_SECTION_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

/// Offset along ray direction to avoid self-intersection on the emitting surface.
const RAY_EPSILON: f32 = 1.0e-3;

/// Per-fragment SDF-shadow budget. Runtime traces at most K `sdf`-tagged lights
/// per `chunk_grid` cell; extras are dropped (treated lit). The half-res shadow
/// target has four RGBA channels — slot i maps to channel i. Compiler warns when
/// a cell exceeds K. See `context/plans/in-progress/sdf-per-light-shadows/`.
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
    let static_slots: Vec<(u32, &MapLight)> = inputs
        .lights
        .entries()
        .iter()
        .filter(|e| !e.light.is_dynamic)
        .enumerate()
        .map(|(slot, e)| (slot as u32, e.light))
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
                for (idx, &(slot, light)) in static_slots.iter().enumerate() {
                    if !overlaps_chunk(light, chunk_min, chunk_max) {
                        continue;
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
                        chunk_min,
                        chunk_max,
                        chunk_centroid,
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
/// Coverage uses the same `overlaps_chunk` metric as the runtime cull.
/// Returns over-K cell count (for tests); logging is the production effect.
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

/// Returns `true` if any of 4 shadow rays (centroid + 3 light-facing face midpoints)
/// reaches the light unoccluded. Directional lights cast from sample toward sun,
/// mirroring the `lightmap_bake::shadow_visible` pattern.
fn any_ray_unoccluded(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    light: &MapLight,
    chunk_min: Vec3,
    chunk_max: Vec3,
    chunk_centroid: Vec3,
) -> bool {
    let samples = sample_points(light, chunk_min, chunk_max, chunk_centroid);
    for sample in samples {
        if segment_clear(bvh, primitives, geometry, light, sample) {
            return true;
        }
    }
    false
}

fn sample_points(light: &MapLight, chunk_min: Vec3, chunk_max: Vec3, centroid: Vec3) -> [Vec3; 4] {
    let to_centroid = match light.light_type {
        LightType::Directional => {
            // Sun shines along cone_direction; faces the light means outward normal
            // points away from it, so `to_centroid` = `aim` (not `-aim`).
            Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0])).normalize_or_zero()
        }
        LightType::Point | LightType::Spot => {
            let origin = Vec3::new(
                light.origin.x as f32,
                light.origin.y as f32,
                light.origin.z as f32,
            );
            (centroid - origin).normalize_or_zero()
        }
    };

    // The three axis signs of `-to_centroid` pick exactly 3 of the 6 cube faces.
    let facing = -to_centroid;
    let mut pts = [centroid; 4];
    pts[1] = if facing.x >= 0.0 {
        Vec3::new(chunk_max.x, centroid.y, centroid.z)
    } else {
        Vec3::new(chunk_min.x, centroid.y, centroid.z)
    };
    pts[2] = if facing.y >= 0.0 {
        Vec3::new(centroid.x, chunk_max.y, centroid.z)
    } else {
        Vec3::new(centroid.x, chunk_min.y, centroid.z)
    };
    pts[3] = if facing.z >= 0.0 {
        Vec3::new(centroid.x, centroid.y, chunk_max.z)
    } else {
        Vec3::new(centroid.x, centroid.y, chunk_min.z)
    };
    pts
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
    let candidates = bvh.traverse(&ray, primitives);
    let max_distance = length - RAY_EPSILON;
    let geom = &geometry.geometry;
    for prim in candidates {
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
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: range,
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
            light_type: LightType::Directional,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: Some(aim),
            animation: None,
            cast_shadows: true,
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
    /// the `postretro` crate and would cross a crate boundary) by mirroring its
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

    // TEMP DIAG: pit-floor SDF light-selection probe ------------------------
    //
    // The pit floor directly under the bridge reads WHITE in the SDF shadow
    // debug viz (should be shadowed). A separate CPU probe
    // (`sdf_bake::temp_diag_floor_light_column_probe`) already PROVED the bridge
    // slab is a correctly-baked SURFACE occluder squarely on the floor→light
    // ray — so the distance field is fine. That refutes "field can't see the
    // slab" and points the finger at LIGHT SELECTION: if the pit-floor pixel
    // never SELECTS the SDF light, no shadow ray is traced and the viz defaults
    // to WHITE.
    //
    // The runtime selector `select_sdf_lights` (sdf_light_select.wgsl) iterates
    // ONLY over the baked per-cell `chunk_indices` list for the receiver's
    // `chunk_grid` cell, then applies the range cull + influence ordering
    // (`sdf_select_influence`). The per-cell list is produced by
    // `bake_chunk_light_list` in THIS module — and it is NOT a pure sphere
    // overlap: it also applies a portal-reachability filter AND a BVH
    // shadow-ray filter (`any_ray_unoccluded`). So an SDF light can be DROPPED
    // from the pit-floor cell at bake time even though its influence sphere
    // covers the floor — which would make the runtime never trace it.
    //
    // This probe bakes the chunk-light-list the SAME way `prl-build` does, then
    // for the pit-floor receiver (plus a few offsets under the bridge) reports:
    //   * the SDF light's falloff `range`, position, peak intensity, and the
    //     floor→light `dist`;
    //   * the per-pixel influence (mirroring `sdf_select_influence`: range cull
    //     + atten*peak) — culled to 0 by range, or > 0?
    //   * the receiver's chunk-grid cell, and whether the SDF light's compacted
    //     spec index is present in that cell's baked light list (and which
    //     sdf-tagged lights ARE present);
    //   * the full `select_sdf_lights`-equivalent result (count + indices),
    //     mirrored on the CPU over the baked cell list;
    //   * a VERDICT: is the SDF light selected for the pit floor? If not, is it
    //     dropped by (a) the range cull, (b) chunk-list omission, or (c) else.
    //
    // No host comparator is reused: the existing one
    // (`postretro::render::sdf_light_select_test::reference_select`) is in the
    // `postretro` crate, runs on the GPU, and scans the FULL spec buffer
    // (chunk grid disabled) — it cannot reach the level-compiler bake nor model
    // the per-cell window that is the whole point here. This test mirrors
    // `sdf_select_influence` + `select_sdf_lights` fresh against the baked
    // `ChunkLightListSection` (the actual runtime candidate window).
    //
    // Run with:
    //   cargo test -p postretro-level-compiler --release \
    //     temp_diag_pit_floor_sdf_light_selection -- --ignored --nocapture
    //
    // TEMPORARY diagnostic — delete this whole block when the investigation is
    // complete. Changes no engine/production behavior.
    #[test]
    #[ignore = "TEMP DIAG: pit-floor SDF light-selection probe — run explicitly with --ignored --nocapture"]
    fn temp_diag_pit_floor_sdf_light_selection() {
        use crate::map_data::ShadowType;
        use crate::map_format::MapFormat;
        use std::collections::HashSet;

        // Must mirror SDF_SELECT_K in sdf_light_select.wgsl (and SDF_SHADOW_K).
        const K: usize = SDF_SHADOW_K;

        // 1. Build the map + chunk-light-list the SAME way prl-build does
        //    (main.rs: parse → partition → portals → exterior → geometry → BVH
        //    → bake_chunk_light_list with the DEFAULT cell size + cap).
        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .join("content/dev/maps/campaign-test.map");
        let map_data = crate::parse::parse_map_file(&map_path, MapFormat::IdTech2)
            .expect("campaign-test.map should parse");
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

        println!("\n=== TEMP DIAG: pit-floor SDF light-selection probe ===");
        println!(
            "chunk grid: origin={:?} cell_size={:.3}m dims={:?} has_grid={} (K={K})",
            section.grid_origin, section.cell_size, section.grid_dimensions, section.has_grid,
        );

        // 2. Build the COMPACTED spec_lights view the runtime indexes into. This
        //    is exactly what pack_spec_lights (spec_buffer.rs) emits and what the
        //    baked `light_indices` index into: filter `!is_dynamic`, preserve
        //    order, no placeholder. The baked chunk list stores these compacted
        //    slots, and the runtime reads spec_lights[slot].
        struct Spec<'a> {
            slot: u32,
            light: &'a MapLight,
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
                light: l,
                pos: Vec3::new(l.origin.x as f32, l.origin.y as f32, l.origin.z as f32),
                range: l.falloff_range, // SpecLight.range = falloff_range (meters)
                peak: (l.color[0] * l.intensity)
                    .max(l.color[1] * l.intensity)
                    .max(l.color[2] * l.intensity),
                is_sdf: l.shadow_type == ShadowType::Sdf,
            })
            .collect();

        // 3. Identify the SDF light (the one with shadow_type == Sdf).
        let sdf = specs
            .iter()
            .find(|s| s.is_sdf)
            .expect("campaign-test must have an Sdf-shadow light");
        let sdf_pos = Vec3::new(
            sdf.light.origin.x as f32,
            sdf.light.origin.y as f32,
            sdf.light.origin.z as f32,
        );
        println!(
            "\nSDF light: compacted spec slot={} pos=({:.3},{:.3},{:.3}) \
             falloff_range={:.3}m intensity={:.3} color={:?} peak(color*intensity)={:.3}",
            sdf.slot, sdf_pos.x, sdf_pos.y, sdf_pos.z, sdf.range, sdf.light.intensity,
            sdf.light.color, sdf.peak,
        );
        let sdf_slots: Vec<u32> = specs.iter().filter(|s| s.is_sdf).map(|s| s.slot).collect();
        println!("all sdf-tagged compacted spec slots: {sdf_slots:?}");

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

        // Resolve the chunk-grid cell linear index for a world position, mirroring
        // `sdf_select_chunk_window`. Returns None when outside the grid.
        let cell_index = |world: Vec3| -> Option<(usize, [i32; 3])> {
            if section.has_grid == 0 {
                return None;
            }
            let origin = Vec3::from(section.grid_origin);
            let cell = section.cell_size;
            let local = world - origin;
            let cx = (local.x / cell).floor() as i32;
            let cy = (local.y / cell).floor() as i32;
            let cz = (local.z / cell).floor() as i32;
            let dims = section.grid_dimensions;
            if cx < 0
                || cy < 0
                || cz < 0
                || cx >= dims[0] as i32
                || cy >= dims[1] as i32
                || cz >= dims[2] as i32
            {
                return Some((usize::MAX, [cx, cy, cz])); // outside grid sentinel
            }
            let ci = cz as usize * dims[1] as usize * dims[0] as usize
                + cy as usize * dims[0] as usize
                + cx as usize;
            Some((ci, [cx, cy, cz]))
        };

        // CPU mirror of `select_sdf_lights` over the BAKED per-cell window: scan
        // the cell's chunk_indices, keep sdf lights with influence > 0, order by
        // influence DESC then index ASC, take K.
        let select = |world: Vec3| -> Vec<(u32, f32)> {
            let Some((ci, _)) = cell_index(world) else {
                // No grid → runtime scans the full spec buffer.
                let mut cands: Vec<(u32, f32)> = specs
                    .iter()
                    .filter(|s| s.is_sdf)
                    .map(|s| (s.slot, influence(s, world)))
                    .filter(|(_, inf)| *inf > 0.0)
                    .collect();
                cands.sort_by(|a, b| {
                    b.1.partial_cmp(&a.1).unwrap().then_with(|| a.0.cmp(&b.0))
                });
                cands.truncate(K);
                return cands;
            };
            if ci == usize::MAX {
                return Vec::new(); // outside authored grid → no static lights
            }
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
            cands
        };

        // 4. Probe the pit-floor receiver and a few offsets under the bridge so a
        //    single unlucky cell boundary cannot mislead.
        let receivers = [
            ("pit floor (canonical)", Vec3::new(-13.0, -5.5, -46.3)),
            ("+1m x", Vec3::new(-12.0, -5.5, -46.3)),
            ("-1m x", Vec3::new(-14.0, -5.5, -46.3)),
            ("+2m z", Vec3::new(-13.0, -5.5, -44.3)),
            ("-2m z", Vec3::new(-13.0, -5.5, -48.3)),
            ("+1m y (above floor)", Vec3::new(-13.0, -4.5, -46.3)),
        ];

        // Fine y-sweep at the canonical (x,z): pin exactly where the grid floor
        // (grid_origin.y) cuts off selection. The pit-floor SURFACE sits right at
        // the world-min AABB, which is also the chunk-grid origin — so a receiver
        // a hair below it falls into cell y=-1 (out of grid) and selects nothing.
        println!(
            "\n--- fine y-sweep at canonical x=-13, z=-46.3 (grid_origin.y={:.4}) ---",
            section.grid_origin[1]
        );
        let mut yy = -6.0_f32;
        while yy <= -3.0 {
            let world = Vec3::new(-13.0, yy, -46.3);
            let cell = cell_index(world);
            let sel = select(world);
            let cell_desc = match cell {
                Some((usize::MAX, c)) => format!("OUT[{},{},{}]", c[0], c[1], c[2]),
                Some((ci, c)) => format!("in[{},{},{}]#{ci}", c[0], c[1], c[2]),
                None => "nogrid".to_string(),
            };
            println!(
                "  y={yy:>6.2}  cell={cell_desc:>16}  selected_sdf_slots={:?}",
                sel.iter().map(|(i, _)| *i).collect::<Vec<_>>()
            );
            yy += 0.25;
        }

        // 4b. TRUE floor-SURFACE receiver. The canonical y=-5.5 receiver above is
        //     actually BELOW the floor surface (inside solid), so it never tests
        //     the pixel the forward shader shades. Walk UP a column from well
        //     below the floor to the FIRST non-solid (air) point — the surface
        //     receiver the runtime shades — exactly like sdf_bake's column probe
        //     finds floor_top. Then perturb it ±0.3 m vertically to stand in for
        //     the half-res depth-reconstruction error. The fix is PROVEN when the
        //     surface receiver AND both perturbations all land in a valid interior
        //     cell (index >= 0, with margin) and select the SDF light.
        let in_solid = |p: Vec3| -> bool {
            let leaf = find_leaf_for_point(&result.tree, DVec3::new(p.x as f64, p.y as f64, p.z as f64));
            result
                .tree
                .leaves
                .get(leaf)
                .map(|l| l.is_solid)
                .unwrap_or(false)
        };
        let (col_x, col_z) = (-13.0_f32, -46.3_f32);
        // Walk DOWN from a known-air point INSIDE the pit (y = -3 m; the y-sweep
        // shows this column is air and selects the light there) to the FIRST
        // air→solid transition: that is the PIT-floor top (not the upper ledge
        // above the pit, which a y=+2 start would hit first). The last air sample
        // just above it is the surface receiver the forward shader shades.
        let step = 0.05_f32;
        let mut wy = -3.0_f32;
        let bottom = Vec3::from(section.grid_origin).y; // padded grid floor
        let mut floor_surface_y: Option<f32> = None;
        let mut prev_air = !in_solid(Vec3::new(col_x, wy, col_z));
        while wy >= bottom {
            let here = Vec3::new(col_x, wy, col_z);
            let solid = in_solid(here);
            // First air→solid transition going DOWN = the floor top. The surface
            // receiver is the air sample one step above (prev iteration's y).
            if prev_air && solid {
                floor_surface_y = Some(wy + step);
                break;
            }
            prev_air = !solid;
            wy -= step;
        }

        println!("\n--- TRUE floor-surface receiver at x={col_x}, z={col_z} ---");
        match floor_surface_y {
            None => println!(
                "  (no air→solid transition found in the column — cannot locate the \
                 floor surface; check the probe column placement)"
            ),
            Some(surf_y) => {
                // `surf_y` is the lowest air sample above the solid floor — the
                // receiver the forward shader shades at the floor surface.
                let surface = Vec3::new(col_x, surf_y, col_z);
                let probes = [
                    ("floor surface", surface),
                    ("surface +0.3m (half-res error)", surface + Vec3::new(0.0, 0.3, 0.0)),
                    ("surface -0.3m (half-res error)", surface - Vec3::new(0.0, 0.3, 0.0)),
                ];
                let mut all_ok = true;
                for (label, world) in probes {
                    let cell = cell_index(world);
                    let sel = select(world);
                    let chosen: Vec<u32> = sel.iter().map(|(i, _)| *i).collect();
                    let selected = chosen.contains(&sdf.slot);
                    // Robustness criterion: the receiver must be a valid in-grid
                    // cell (index >= 0 on every axis) AND sit clear of any cell
                    // FACE — i.e. its fractional position within the cell is not
                    // hard against a boundary. Half-cell padding centers a surface
                    // flush with the AABB min in the MIDDLE of its cell, so the
                    // fractional Y here should sit near 0.5, far from 0.0/1.0.
                    let origin = Vec3::from(section.grid_origin);
                    let frac = ((world - origin) / section.cell_size).fract();
                    // Distance (in cell fractions) to the nearest cell face on the
                    // Y axis — the axis the flush-boundary leak lives on (the pit
                    // floor surface == AABB min.y). Half-cell padding should put
                    // the surface receiver near the cell CENTER (frac_y ≈ 0.5), so
                    // a ±0.3 m reconstruction error (≈ 0.04 cell at 8 m) cannot
                    // cross the Y face. We only gate on Y; the X/Z fractional
                    // position is incidental to this column's placement and not
                    // what the padding addresses.
                    let edge_dist_y = frac.y.min(1.0 - frac.y);
                    let (cell_desc, in_grid) = match cell {
                        Some((usize::MAX, c)) => {
                            (format!("OUTSIDE[{},{},{}]", c[0], c[1], c[2]), false)
                        }
                        Some((_ci, c)) => (format!("in[{},{},{}]", c[0], c[1], c[2]), true),
                        None => ("nogrid".to_string(), false),
                    };
                    // "off-boundary" margin on Y: > 0.1 cell from the nearest Y face.
                    let off_boundary = in_grid && edge_dist_y > 0.1;
                    println!(
                        "  {label:<32} world=({:.3},{:.3},{:.3}) cell={cell_desc:>14} \
                         frac_y={:.3} edge_dist_y={edge_dist_y:.3} off_boundary_y={off_boundary} \
                         count={} indices={chosen:?} sdf_selected={selected}",
                        world.x, world.y, world.z, frac.y, sel.len(),
                    );
                    if !(selected && off_boundary) {
                        all_ok = false;
                    }
                }
                println!(
                    "\n  >> FLOOR-SURFACE VERDICT: {}",
                    if all_ok {
                        "FIXED — the floor-surface receiver and both ±0.3m perturbations \
                         all land in an in-grid cell clear of the Y cell face \
                         (edge_dist_y > 0.1 cell) and select the SDF light. Before \
                         padding this same surface receiver mapped to cell y=-1 \
                         (outside grid, count 0)."
                    } else {
                        "NOT fixed — at least one of the surface receiver / perturbations \
                         still fails to select the SDF light or sits on a cell boundary."
                    }
                );
                assert!(
                    all_ok,
                    "floor-surface receiver (and ±0.3m perturbations) must land in an \
                     interior cell and select the SDF light after the grid padding fix"
                );
            }
        }

        for (label, world) in receivers {
            let dist = (sdf_pos - world).length();
            let inf = influence(sdf, world);
            let range_culled = sdf.range > 0.0 && dist > sdf.range;
            let cell = cell_index(world);
            println!(
                "\n----- RECEIVER {label}: world=({:.3},{:.3},{:.3}) -----",
                world.x, world.y, world.z
            );
            println!(
                "  floor→light dist={dist:.3}m  range={:.3}m  influence={inf:.4} \
                 (range_culled={range_culled})",
                sdf.range
            );

            match cell {
                None => println!("  chunk cell: (no grid)"),
                Some((usize::MAX, c)) => println!(
                    "  chunk cell: ({},{},{}) is OUTSIDE the authored grid \
                     dims={:?} → runtime sees NO static lights here",
                    c[0], c[1], c[2], section.grid_dimensions
                ),
                Some((ci, c)) => {
                    let entry = section.offsets[ci];
                    let start = entry.offset as usize;
                    let end = start + entry.count as usize;
                    let in_cell = &section.light_indices[start..end];
                    let sdf_in_cell: Vec<u32> = in_cell
                        .iter()
                        .copied()
                        .filter(|&slot| {
                            specs.get(slot as usize).map(|s| s.is_sdf).unwrap_or(false)
                        })
                        .collect();
                    let sdf_present = in_cell.contains(&sdf.slot);
                    println!(
                        "  chunk cell: ({},{},{}) linear={ci}  cell holds {} light slot(s): {:?}",
                        c[0], c[1], c[2], entry.count, in_cell
                    );
                    println!(
                        "  sdf-tagged slots present in this cell: {sdf_in_cell:?}  \
                         (SDF light slot {} present? {sdf_present})",
                        sdf.slot
                    );
                }
            }

            let sel = select(world);
            let chosen: Vec<u32> = sel.iter().map(|(i, _)| *i).collect();
            let selected = chosen.contains(&sdf.slot);
            println!(
                "  select_sdf_lights-equiv: count={} indices={chosen:?} \
                 (influences={:?})",
                sel.len(),
                sel.iter().map(|(_, f)| format!("{f:.4}")).collect::<Vec<_>>()
            );

            // VERDICT for this receiver.
            let verdict = if selected {
                "SELECTED — SDF light IS chosen; leak is NOT light selection for this pixel."
            } else if range_culled {
                "NOT selected: dropped by the RANGE CULL (dist > falloff_range)."
            } else {
                match cell {
                    Some((usize::MAX, _)) => {
                        "NOT selected: receiver is OUTSIDE the chunk grid (no static lights)."
                    }
                    Some((ci, _)) => {
                        let entry = section.offsets[ci];
                        let start = entry.offset as usize;
                        let end = start + entry.count as usize;
                        if section.light_indices[start..end].contains(&sdf.slot) {
                            "NOT selected: in the cell + in range, but ranked below K \
                             (other sdf lights outrank it)."
                        } else {
                            "NOT selected: dropped by CHUNK-LIST OMISSION — the bake's \
                             portal-reachability / BVH-shadow-ray filter removed the SDF \
                             light from this cell's list, so the runtime never traces it."
                        }
                    }
                    None => "NOT selected: no grid (full-buffer scan) yet still not chosen.",
                }
            };
            println!("  >> VERDICT: {verdict}");
        }
        println!("=== END TEMP DIAG ===\n");
    }
}
