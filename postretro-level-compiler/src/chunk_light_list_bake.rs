// Per-chunk static-light list builder.
//
// Partitions the world AABB into a uniform cubic grid, buckets static lights
// per chunk by sphere-vs-AABB overlap, filters each bucket with 4 BVH shadow
// rays to drop lights that cannot reach the chunk through geometry, then
// clamps each chunk to a fixed cap. Output packs into the `ChunkLightList`
// PRL section for the runtime specular path.
//
// See: lighting-chunk-lists/

use bvh::bvh::Bvh;
use bvh::ray::Ray;
use glam::Vec3;
use nalgebra::{Point3, Vector3};
use postretro_level_format::chunk_light_list::{
    ChunkEntry, ChunkLightListSection, DEFAULT_PER_CHUNK_CAP,
};
use thiserror::Error;

use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;
use crate::map_data::{LightType, MapLight};

/// Default chunk edge length in meters. 8 m fits the rendering-pipeline
/// cadence: small enough that per-chunk buckets stay sparse, large enough that
/// the grid does not explode on larger maps.
pub const DEFAULT_CELL_SIZE_METERS: f32 = 8.0;

/// Per-chunk hard cap on retained lights. Overflow logs at bake time.
pub const DEFAULT_PER_CHUNK_LIGHT_CAP: u32 = DEFAULT_PER_CHUNK_CAP;

/// Cap total `offset table + index list` memory at 16 MB. Matches the plan's
/// acceptance gate. Bake fails with a diagnostic when exceeded.
pub const MAX_SECTION_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

/// Ray start offset along the light→sample direction to avoid self-hits on
/// walls the light is mounted on.
const RAY_EPSILON: f32 = 1.0e-3;

#[derive(Debug, Error)]
pub enum ChunkLightListError {
    #[error(
        "ChunkLightList payload {actual} bytes exceeds {max} byte cap. \
         Raise `cell_size_meters` or subdivide the map."
    )]
    PayloadTooLarge { actual: usize, max: usize },
}

/// Inputs the baker pulls from the rest of the compile stages.
pub struct ChunkLightListInputs<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    pub lights: &'a [MapLight],
}

/// Build the chunk light list from the current compile stage outputs.
///
/// Returns a placeholder section (`has_grid == 0`) when there is no work to do
/// (empty geometry, or no static non-dynamic lights). The runtime treats the
/// placeholder as the signal to fall back to full-buffer iteration.
pub fn bake_chunk_light_list(
    inputs: &ChunkLightListInputs<'_>,
    cell_size_meters: f32,
    per_chunk_cap: u32,
) -> Result<ChunkLightListSection, ChunkLightListError> {
    let verts = &inputs.geometry.geometry.vertices;
    if verts.is_empty() {
        return Ok(ChunkLightListSection::placeholder());
    }

    // Static-light index table: original `MapLight` index so offsets line up
    // with consumers that keep the full light array (the baker does not
    // re-number or filter dynamic/bake-only lights — those are skipped at
    // bucketing time, not dropped from the index space).
    let static_light_indices: Vec<usize> = inputs
        .lights
        .iter()
        .enumerate()
        .filter_map(|(i, l)| if is_eligible(l) { Some(i) } else { None })
        .collect();
    if static_light_indices.is_empty() {
        return Ok(ChunkLightListSection::placeholder());
    }

    // World AABB from geometry vertices.
    let (world_min, world_max) = world_aabb(inputs.geometry);
    let cell = cell_size_meters.max(1.0e-3);
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

    // Per-chunk retained light-index lists.
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

                let bucket = &mut per_chunk[chunk_idx];
                for &li in &static_light_indices {
                    let light = &inputs.lights[li];
                    if !overlaps_chunk(light, chunk_min, chunk_max) {
                        continue;
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
                    bucket.push(li as u32);
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

    // Pack offset table + flat index list.
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
        static_light_indices.len(),
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

/// Static-and-live filter. Dynamic lights are evaluated at runtime (the spec
/// buffer they drive lives in Task B); bake-only lights never reach the
/// runtime direct path at all.
fn is_eligible(light: &MapLight) -> bool {
    !light.is_dynamic && !light.bake_only
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
            // Spot lights use a conservative sphere identical to
            // `LightInfluence` — the cone refinement is a runtime concern.
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

/// Cast up to 4 shadow rays from light→sample-points inside the chunk.
/// Samples: chunk centroid plus the 3 face midpoints whose outward normal is
/// most aligned with the (light → centroid) direction — i.e. the faces that
/// face the light. Returns `true` on the first unoccluded ray.
///
/// Directional lights have no finite origin, so the ray emanates from the
/// sample point toward the incoming light direction for a long segment — the
/// same pattern as `lightmap_bake::shadow_visible`.
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

/// Centroid + face-midpoint samples picked by alignment with the
/// light→centroid direction. For a directional light the "from light"
/// direction is `-cone_direction`; for point/spot it is `centroid - origin`.
fn sample_points(light: &MapLight, chunk_min: Vec3, chunk_max: Vec3, centroid: Vec3) -> [Vec3; 4] {
    let to_centroid = match light.light_type {
        LightType::Directional => {
            // Sun shines along `cone_direction`; light arrives from `-aim`.
            // The "face the light" criterion uses the direction pointing AT
            // the centroid, so that's `aim` itself.
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

    // Pick 3 chunk faces whose outward normal is most aligned with
    // `to_centroid` (negated — faces the light means outward normal points
    // away from the light). The three axis signs of `-to_centroid` — one per
    // axis — always pick exactly 3 of the 6 cube faces.
    let facing = -to_centroid;
    let mut pts = [centroid; 4];
    // X face
    pts[1] = if facing.x >= 0.0 {
        Vec3::new(chunk_max.x, centroid.y, centroid.z)
    } else {
        Vec3::new(chunk_min.x, centroid.y, centroid.z)
    };
    // Y face
    pts[2] = if facing.y >= 0.0 {
        Vec3::new(centroid.x, chunk_max.y, centroid.z)
    } else {
        Vec3::new(centroid.x, chunk_min.y, centroid.z)
    };
    // Z face
    pts[3] = if facing.z >= 0.0 {
        Vec3::new(centroid.x, centroid.y, chunk_max.z)
    } else {
        Vec3::new(centroid.x, centroid.y, chunk_min.z)
    };
    pts
}

/// Test whether the segment from `light` position (or directional aim, for a
/// sun) to `sample` is unobstructed. Mirrors the lightmap baker's sweep, but
/// the ray origin sits at the light end rather than the surface — we're
/// answering "can the light see the chunk", which is the same geometric
/// predicate in reverse.
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
            // Emulate "ray from infinite light" by casting from the sample in
            // the direction of the sun, far enough to clear any plausible
            // world geometry before declaring the sample lit.
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
        }
    }

    fn dynamic_point_light(origin: DVec3, range: f32) -> MapLight {
        let mut l = point_light(origin, range);
        l.is_dynamic = true;
        l
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
        }
    }

    fn single_quad_geometry() -> GeometryResult {
        // 16 m × 16 m floor quad on the XZ plane, centered at origin.
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
        // Two floor strips separated by a tall vertical wall at x = 0.
        // Room A occupies x in [-10, -1], Room B x in [1, 10].
        // Wall spans x ∈ [-0.5, 0.5], z ∈ [-10, 10], y ∈ [0, 10].
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

        // Floor A
        push_quad(
            [
                [-10.0, 0.0, -10.0],
                [-1.0, 0.0, -10.0],
                [-1.0, 0.0, 10.0],
                [-10.0, 0.0, 10.0],
            ],
            [0.0, 1.0, 0.0],
        );
        // Floor B
        push_quad(
            [
                [1.0, 0.0, -10.0],
                [10.0, 0.0, -10.0],
                [10.0, 0.0, 10.0],
                [1.0, 0.0, 10.0],
            ],
            [0.0, 1.0, 0.0],
        );
        // Wall (facing +X) — blocks rays from the A side traveling in +X
        push_quad(
            [
                [-0.5, 0.0, -10.0],
                [-0.5, 10.0, -10.0],
                [-0.5, 10.0, 10.0],
                [-0.5, 0.0, 10.0],
            ],
            [-1.0, 0.0, 0.0],
        );
        // Wall (facing -X)
        push_quad(
            [
                [0.5, 0.0, -10.0],
                [0.5, 0.0, 10.0],
                [0.5, 10.0, 10.0],
                [0.5, 10.0, -10.0],
            ],
            [1.0, 0.0, 0.0],
        );
        // Top of wall — seals the gap above so rays cannot sneak over
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
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &[],
            geometry: &geo,
            lights: &lights,
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
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &lights,
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
        // 16 × 16 m floor subdivided into 4 × 4 m chunks (4 × 4 = 16 chunks on
        // Y=0). A light with a 4 m radius placed at the +X edge touches only
        // the chunks near it — the far side chunks fail the sphere-AABB test.
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light(DVec3::new(7.0, 1.0, 7.0), 4.0)];
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &lights,
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
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &lights,
        };
        let section = bake_chunk_light_list(&inputs, 8.0, 64).unwrap();
        assert_eq!(section.has_grid, 1);
        for e in &section.offsets {
            assert_eq!(e.count, 1);
        }
    }

    #[test]
    fn occluded_chunk_drops_light() {
        // A light sits in Room A (x < 0). A solid wall at x ≈ 0 blocks rays.
        // The chunk sitting deep in Room B must not retain the light even
        // though the light's sphere reaches it.
        let geo = two_room_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let light_pos = DVec3::new(-5.0, 2.0, 0.0);
        let lights = vec![point_light(light_pos, 50.0)];
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &lights,
        };
        let section = bake_chunk_light_list(&inputs, 4.0, 64).unwrap();
        assert_eq!(section.has_grid, 1);

        // Find the chunk that contains (5, 2, 0) — deep in Room B.
        let far_point = Vec3::new(5.0, 2.0, 0.0);
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
        // Pack 70 co-located point lights. The only chunk touching the origin
        // should retain at most the cap.
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let mut lights = Vec::new();
        for _ in 0..70 {
            lights.push(point_light(DVec3::new(0.0, 1.0, 0.0), 4.0));
        }
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &lights,
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
        // Synthesize a tiny cell size over a modest map so the offset table
        // alone blows the cap. 256^3 chunks × 8 bytes = 128 MiB.
        let geo = single_quad_geometry();
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let lights = vec![point_light(DVec3::new(0.0, 1.0, 0.0), 4.0)];
        let inputs = ChunkLightListInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            lights: &lights,
        };
        // 16 m map, 0.05 m cells → 320^1 on XZ × 1 on Y. Still big enough to
        // exceed 16 MB offset table (320*320*1*8 = ~820 KB), so instead crank
        // Y as well by using a long skinny quad — keep it simple here: pick a
        // cell size that reliably breaks the cap on the single_quad map.
        //
        // 16 m × 16 m at 0.01 m/cell = 1600*1600*1 = 2.56M chunks, 8 bytes
        // each = 20 MB. Over cap.
        let err = bake_chunk_light_list(&inputs, 0.01, 64).unwrap_err();
        match err {
            ChunkLightListError::PayloadTooLarge { actual, max } => {
                assert!(actual > max);
            }
        }
    }
}
