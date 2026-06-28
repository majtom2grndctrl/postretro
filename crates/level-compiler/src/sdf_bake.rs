// SDF static-occluder atlas bake.
//
// Builds a sparse brick atlas of signed distances from each voxel to the
// nearest static-world triangle, packaged as a `SdfAtlasSection` for the PRL.
// Foundation plan: `context/plans/done/sdf-static-occluder-shadows/index.md`.
// Current apron/version change: `context/plans/in-progress/sdf-filterable-atlas/index.md`.
//
// Algorithm (deterministic, single-threaded inside each brick — accumulation
// order is stable so identical inputs produce byte-identical output):
//
// 1. Compute the world AABB from the geometry vertices (matches SH/lightmap
//    bakes' definition). Snap the min to a voxel grid; size the grid so the
//    max is enclosed. Brick dims are `ceil(grid_voxel_dims / brick_size)`.
// 2. For each brick, in z-major / y / x order, compute the per-voxel unsigned
//    distance to the nearest triangle by brute-force triangle iteration
//    (gated by an AABB-vs-brick rejection). Sign comes from BSP leaf
//    classification: a voxel-center inside a solid leaf is negative.
// 3. Classify each brick: all samples > `voxel_size_m` in magnitude → empty
//    or interior (sign tells which), no atlas entry; otherwise it is a
//    surface brick — emit it into the atlas in the order the bake encountered
//    it. Surface bricks store a 1-voxel apron on every side: the stored block
//    is `(brick_size + 2)^3` samples, z-major-within-brick, with interior
//    voxels at stored indices `[1, brick_size]` and apron voxels at `0` and
//    `brick_size + 1`. The apron carries the true signed field at the neighbor
//    positions it mirrors (edge-extended at the world-AABB boundary) so the
//    runtime can sample the fine field with hardware trilinear filtering
//    without seams across brick boundaries. Classification (empty/interior vs.
//    surface) uses only the interior voxels.
// 4. Pack the surface bricks into a 3D `atlas_bricks_per_axis` arrangement
//    sized as a near-cube to keep texture aspect reasonable. The packing
//    coordinate is implicit from the linear order, so the bake records only
//    the per-axis dimensions; the runtime decodes by the same convention.
//
// Distance quantisation: `voxel_size_m / 256` per `i16` step, clamped to the
// `i16` range. The SDF only needs to distinguish near-surface gradients;
// distances larger than the voxel size are uninteresting (the runtime falls
// back to the coarse `f32` per-brick distance there).

use std::time::Instant;

use glam::{DVec3, Vec3};
use postretro_level_format::sdf_atlas::{BRICK_SLOT_EMPTY, BRICK_SLOT_INTERIOR, SdfAtlasSection};

use crate::geometry::GeometryResult;
use crate::partition::{BspTree, find_leaf_for_point};

/// Bump this when the bake algorithm or input layout changes. The cache key
/// folds it in so a stale on-disk entry from a prior version is automatically
/// invalidated (first build after the bump is a miss; the second is a hit —
/// matches the SH and lightmap stage pattern).
// v2: empty-brick coarse value changed from the per-brick MEAN of signed
// distances to a conservative MINIMUM clearance (`min_unsigned − safety
// margin`). The mean is not a valid sphere-trace lower bound and let the shadow
// march overstep/tunnel through sub-brick geometry (Hart 1996). Bumped so stale
// caches don't serve the old mean-based coarse field. See `COARSE_SAFETY_MARGIN_VOXELS`.
// v3: surface bricks now store a 1-voxel apron on every side (stored sample
// count `(brick_size + 2)^3`, z-major) so the runtime can sample the fine field
// with hardware trilinear filtering without seams at brick boundaries.
pub const STAGE_VERSION: u32 = 3;

/// Default voxel edge length in meters. Sized to give a usable shadow
/// resolution for retro-scale interiors without exploding atlas memory.
pub const DEFAULT_VOXEL_SIZE_METERS: f32 = 0.5;

/// Default voxels per brick edge. 8^3 = 512 samples per brick — the standard
/// brick size for sparse SDF atlases, balanced against the 3D-texture sample
/// cost.
pub const DEFAULT_BRICK_SIZE_VOXELS: u32 = 8;

/// Padding (in voxels) added around the geometry AABB so the surface band on
/// the very edge of the world stays inside the grid.
const GRID_VOXEL_PADDING: u32 = 2;

/// Above this many voxels of distance from the surface, a brick contributes
/// nothing useful to the runtime tracer — drop it (mark empty/interior).
/// One voxel of slack gives the runtime a smooth fallback across brick
/// boundaries via the coarse field.
const SURFACE_BAND_VOXELS: f32 = 1.0;

/// Half the unit-cube diagonal (`sqrt(3) / 2 ≈ 0.866`), in voxel-size units.
/// The per-brick coarse clearance (`min_unsigned`) is the minimum unsigned
/// distance measured at VOXEL CENTERS, spaced `voxel_size` apart. A ray point
/// between centers can be closer to a surface than any center; the worst case is
/// a point at a corner of the voxel-center lattice cell, which sits up to half a
/// voxel diagonal (`voxel_size · sqrt(3) / 2`) from the nearest center. Since
/// the SDF is 1-Lipschitz, subtracting this margin from `min_unsigned` yields a
/// provable lower bound on the true clearance for ANY point inside the brick —
/// the invariant sphere tracing requires (Hart 1996). See the `coarse_clearance`
/// computation below.
const COARSE_SAFETY_MARGIN_VOXELS: f32 = 0.866_025_4;

/// Owned, serialisable snapshot of the bake's inputs. Hashed into the cache
/// key via postcard: a per-stage `*Inputs` struct holds exactly the data the
/// bake reads, serialised deterministically so the digest captures every input
/// the outputs depend on.
#[derive(serde::Serialize)]
pub struct SdfInputs {
    pub geometry: GeometryResult,
}

/// CLI-driven configuration. Hashed alongside `SdfInputs` so the cache key
/// changes if the voxel/brick sizing changes.
#[derive(serde::Serialize, Clone, Copy)]
pub struct SdfConfig {
    pub voxel_size_m: f32,
    pub brick_size_voxels: u32,
}

impl Default for SdfConfig {
    fn default() -> Self {
        Self {
            voxel_size_m: DEFAULT_VOXEL_SIZE_METERS,
            brick_size_voxels: DEFAULT_BRICK_SIZE_VOXELS,
        }
    }
}

/// Borrowed bake context — matches the `ShBakeCtx` pattern.
pub struct SdfBakeCtx<'a> {
    pub geometry: &'a GeometryResult,
    pub tree: &'a BspTree,
}

/// Cached per-triangle data: vertices + AABB. Pre-computed once so the
/// per-voxel distance loop is a tight numeric kernel.
struct Triangle {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    aabb_min: Vec3,
    aabb_max: Vec3,
}

/// Returns an empty section (`grid_dims == [0,0,0]`, empty arrays) for
/// empty geometry — matches the "no SDF" degradation path the runtime
/// already handles.
pub fn bake_sdf_atlas(ctx: &SdfBakeCtx<'_>, config: &SdfConfig) -> SdfAtlasSection {
    let geom = &ctx.geometry.geometry;
    if geom.vertices.is_empty() || geom.indices.is_empty() {
        return SdfAtlasSection::empty();
    }

    let triangles = collect_triangles(ctx.geometry);
    if triangles.is_empty() {
        return SdfAtlasSection::empty();
    }

    let voxel_size = config.voxel_size_m.max(1.0e-4);
    let brick_size = config.brick_size_voxels.max(1);
    // Surface bricks store a 1-voxel apron on every side for hardware trilinear
    // filtering, so the stored block is `(brick_size + 2)^3`, z-major.
    let stored_brick_edge = brick_size + 2;
    let voxels_per_brick = (stored_brick_edge * stored_brick_edge * stored_brick_edge) as usize;

    let (world_min, world_max) = world_aabb(ctx);
    let (grid_origin, brick_dims) = grid_extents(world_min, world_max, voxel_size, brick_size);
    let total_bricks = brick_dims[0] as usize * brick_dims[1] as usize * brick_dims[2] as usize;

    // Total interior-voxel extent of the whole grid per axis. Apron voxels that
    // fall outside `[0, interior_voxels - 1]` have no neighbor brick, so they
    // edge-extend: clamp the world voxel index into this range before eval.
    let interior_voxels = [
        brick_dims[0] * brick_size,
        brick_dims[1] * brick_size,
        brick_dims[2] * brick_size,
    ];

    let inv_quant_step = 256.0 / voxel_size;
    let surface_band_m = voxel_size * SURFACE_BAND_VOXELS;

    // Outputs.
    let mut top_level: Vec<u32> = Vec::with_capacity(total_bricks);
    let mut atlas: Vec<i16> = Vec::new();
    let mut coarse: Vec<f32> = Vec::with_capacity(total_bricks);
    let mut surface_brick_count: u32 = 0;

    // Stats counters (diagnostic-only).
    let mut empty_count = 0u32;
    let mut interior_count = 0u32;
    let bake_started = Instant::now();

    // Per-brick scratch buffer; reused across bricks to avoid reallocation.
    // The ordering is z-major over the apron'd `(brick_size + 2)^3` block:
    // `sz*edge^2 + sy*edge + sx` with `edge = brick_size + 2`. Stored indices
    // `[1, brick_size]` per axis hold interior voxels; `0` and `brick_size + 1`
    // hold the apron. The runtime/shader address this same layout.
    let mut brick_samples: Vec<i16> = Vec::with_capacity(voxels_per_brick);

    // Iterate bricks in z-major / y-major / x order — the same order the
    // top-level index uses on disk. The traversal order pins atlas-slot
    // assignment, which is what gives the bake byte-identical output across
    // runs on identical input.
    for bz in 0..brick_dims[2] {
        for by in 0..brick_dims[1] {
            for bx in 0..brick_dims[0] {
                brick_samples.clear();
                let mut min_unsigned = f32::INFINITY;
                let mut all_solid = true;
                let mut all_empty_air = true;
                let mut max_abs_quant: f32 = 0.0;

                // Sample the apron'd `(brick_size + 2)^3` block, z-major. Stored
                // index `s` maps to world voxel index `brick_origin + s - 1`, so
                // `s = 0` and `s = brick_size + 1` are the apron (a neighbor
                // brick's space, or edge-extended at the world boundary) and
                // `s in [1, brick_size]` are this brick's interior. Only interior
                // voxels feed the empty/interior/surface classification.
                for sz in 0..stored_brick_edge {
                    let wz = clamp_world_voxel(bz, sz, brick_size, interior_voxels[2]);
                    let interior_z = sz >= 1 && sz <= brick_size;
                    for sy in 0..stored_brick_edge {
                        let wy = clamp_world_voxel(by, sy, brick_size, interior_voxels[1]);
                        let interior_y = sy >= 1 && sy <= brick_size;
                        for sx in 0..stored_brick_edge {
                            let wx = clamp_world_voxel(bx, sx, brick_size, interior_voxels[0]);
                            let interior_x = sx >= 1 && sx <= brick_size;

                            let p = voxel_center(grid_origin, wx, wy, wz, voxel_size);
                            let unsigned = nearest_triangle_distance(&triangles, p);
                            let inside_solid = point_in_solid(ctx.tree, p);
                            let signed = if inside_solid { -unsigned } else { unsigned };

                            let q_raw = (signed * inv_quant_step).round();
                            let q_clamped = q_raw.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                            brick_samples.push(q_clamped);

                            // Classification reads interior voxels only — the
                            // apron is filler for trilinear continuity, not part
                            // of this brick's surface/empty decision.
                            if interior_x && interior_y && interior_z {
                                min_unsigned = min_unsigned.min(unsigned);
                                if inside_solid {
                                    all_empty_air = false;
                                } else {
                                    all_solid = false;
                                }
                                max_abs_quant = max_abs_quant.max(q_clamped.unsigned_abs() as f32);
                            }
                        }
                    }
                }

                // Classify the brick. A brick is "near a surface" if any voxel
                // sample lies within the surface band — that's the only case
                // where the fine atlas data carries non-redundant info.
                let near_surface = min_unsigned <= surface_band_m;

                // Per-brick coarse clearance: a CONSERVATIVE LOWER BOUND on the
                // unsigned distance-to-surface for ANY point a ray could occupy
                // inside this brick. The runtime uses this as the sphere-trace
                // step for non-surface (EMPTY) bricks (`sample_coarse_distance`
                // in `sdf_shadow.wgsl`, step `t += max(d, voxel*0.5)`).
                //
                // WHY MIN, NOT MEAN: sphere tracing requires the step to be a
                // lower bound on distance-to-surface (Hart 1996). The old mean
                // of per-voxel distances is NOT a lower bound — it overstates
                // clearance near the brick's closest approach, so the ray
                // oversteps and tunnels through sub-brick occluders (the
                // secondary ~4 m-granularity banding). Take the minimum instead.
                //
                // WHY THE MARGIN: `min_unsigned` is the minimum measured at
                // voxel CENTERS (spaced `voxel_size` apart). A point between
                // centers can be closer to a surface; the worst case is half a
                // voxel diagonal from the nearest center. Subtracting that margin
                // (1-Lipschitz SDF) makes the value a provable lower bound for
                // every interior point. Under-stepping is safe-but-slower;
                // over-stepping is the bug. Clamp at >= 0 (the runtime clamps
                // too, but keep the stored value honest).
                let coarse_clearance =
                    (min_unsigned - voxel_size * COARSE_SAFETY_MARGIN_VOXELS).max(0.0);

                if !near_surface {
                    // Pure-empty or pure-solid brick — no atlas slot. Pick
                    // sentinel by which side of the surface the brick sits.
                    let slot = if all_solid {
                        interior_count += 1;
                        BRICK_SLOT_INTERIOR
                    } else {
                        // Default: treat anything not strictly all-solid as
                        // empty space when the brick is far from a surface.
                        // (`all_empty_air` covers the clean case; mixed bricks
                        // never reach this branch because they'd have a near
                        // sample.)
                        let _ = all_empty_air;
                        empty_count += 1;
                        BRICK_SLOT_EMPTY
                    };
                    top_level.push(slot);
                } else {
                    let slot = surface_brick_count;
                    top_level.push(slot);
                    atlas.extend_from_slice(&brick_samples);
                    surface_brick_count += 1;
                }
                coarse.push(coarse_clearance);
            }
        }
    }

    let atlas_bricks_per_axis = pack_atlas_dimensions(surface_brick_count);

    let section = SdfAtlasSection {
        world_min: [
            grid_origin.x as f32,
            grid_origin.y as f32,
            grid_origin.z as f32,
        ],
        world_max: [
            (grid_origin.x + brick_dims[0] as f64 * brick_size as f64 * voxel_size as f64) as f32,
            (grid_origin.y + brick_dims[1] as f64 * brick_size as f64 * voxel_size as f64) as f32,
            (grid_origin.z + brick_dims[2] as f64 * brick_size as f64 * voxel_size as f64) as f32,
        ],
        voxel_size_m: voxel_size,
        brick_size_voxels: brick_size,
        grid_dims: brick_dims,
        atlas_bricks_per_axis,
        surface_brick_count,
        top_level,
        atlas,
        coarse_distances: coarse,
    };

    log::info!(
        "SdfAtlas: brick grid {}x{}x{} = {} bricks ({} surface, {} empty, {} interior) — \
         voxel {voxel_size}m / brick {brick_size} vox, atlas pack {}x{}x{}, baked in {:.2}s",
        brick_dims[0],
        brick_dims[1],
        brick_dims[2],
        total_bricks,
        surface_brick_count,
        empty_count,
        interior_count,
        atlas_bricks_per_axis[0],
        atlas_bricks_per_axis[1],
        atlas_bricks_per_axis[2],
        bake_started.elapsed().as_secs_f32(),
    );

    section
}

/// Public alias so `main.rs` can log stats post-cache-hit without re-baking.
pub fn log_stats(section: &SdfAtlasSection) {
    let total = section.total_bricks();
    let surface = section.surface_brick_count;
    let empty = section
        .top_level
        .iter()
        .filter(|s| **s == BRICK_SLOT_EMPTY)
        .count();
    let interior = section
        .top_level
        .iter()
        .filter(|s| **s == BRICK_SLOT_INTERIOR)
        .count();
    log::info!(
        "SdfAtlas: brick grid {}x{}x{} = {total} bricks ({surface} surface, {empty} empty, \
         {interior} interior) — voxel {}m / brick {} vox, atlas pack {}x{}x{}",
        section.grid_dims[0],
        section.grid_dims[1],
        section.grid_dims[2],
        section.voxel_size_m,
        section.brick_size_voxels,
        section.atlas_bricks_per_axis[0],
        section.atlas_bricks_per_axis[1],
        section.atlas_bricks_per_axis[2],
    );
}

fn collect_triangles(geo: &GeometryResult) -> Vec<Triangle> {
    let g = &geo.geometry;
    let mut tris = Vec::with_capacity(g.indices.len() / 3);
    let mut i = 0;
    while i + 3 <= g.indices.len() {
        let i0 = g.indices[i] as usize;
        let i1 = g.indices[i + 1] as usize;
        let i2 = g.indices[i + 2] as usize;
        i += 3;
        let a = Vec3::from(g.vertices[i0].position);
        let b = Vec3::from(g.vertices[i1].position);
        let c = Vec3::from(g.vertices[i2].position);
        let aabb_min = a.min(b).min(c);
        let aabb_max = a.max(b).max(c);
        tris.push(Triangle {
            a,
            b,
            c,
            aabb_min,
            aabb_max,
        });
    }
    tris
}

fn world_aabb(ctx: &SdfBakeCtx<'_>) -> (DVec3, DVec3) {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for v in &ctx.geometry.geometry.vertices {
        let p = DVec3::new(
            v.position[0] as f64,
            v.position[1] as f64,
            v.position[2] as f64,
        );
        min = min.min(p);
        max = max.max(p);
    }
    (min, max)
}

/// Compute the grid origin (snapped to a voxel grid so re-baking the same map
/// with a different output offset is irrelevant) and the brick-grid dimensions
/// big enough to enclose the world AABB plus a one-voxel padding band.
fn grid_extents(
    world_min: DVec3,
    world_max: DVec3,
    voxel_size: f32,
    brick_size: u32,
) -> (DVec3, [u32; 3]) {
    let voxel = voxel_size as f64;
    let bricks_size = voxel * brick_size as f64;
    let padding = voxel * GRID_VOXEL_PADDING as f64;

    let origin = DVec3::new(
        (world_min.x - padding).div_euclid(voxel) * voxel,
        (world_min.y - padding).div_euclid(voxel) * voxel,
        (world_min.z - padding).div_euclid(voxel) * voxel,
    );

    // Expand the far corner by the same padding before measuring the span.
    let span = (world_max + DVec3::splat(padding)) - origin;
    let brick_dims = [
        ((span.x / bricks_size).ceil().max(1.0) as u32),
        ((span.y / bricks_size).ceil().max(1.0) as u32),
        ((span.z / bricks_size).ceil().max(1.0) as u32),
    ];
    (origin, brick_dims)
}

/// World-space center of the voxel at global voxel index `(world_vx, _vy, _vz)`
/// — the historical center-sampling convention `(world_idx + 0.5) * voxel`.
fn voxel_center(
    origin: DVec3,
    world_vx: u32,
    world_vy: u32,
    world_vz: u32,
    voxel_size: f32,
) -> Vec3 {
    let voxel = voxel_size as f64;
    let p = origin
        + DVec3::new(
            (world_vx as f64 + 0.5) * voxel,
            (world_vy as f64 + 0.5) * voxel,
            (world_vz as f64 + 0.5) * voxel,
        );
    Vec3::new(p.x as f32, p.y as f32, p.z as f32)
}

/// Map a stored apron index `s in [0, brick_size + 1]` to a global voxel index.
/// The interior occupies `s in [1, brick_size]`; the apron at `s = 0` /
/// `s = brick_size + 1` reaches one voxel past the brick. World voxel index is
/// `brick_origin * brick_size + s - 1`; at the world-AABB boundary (no neighbor
/// brick) the index is edge-extended by clamping into `[0, interior_voxels - 1]`.
fn clamp_world_voxel(brick_origin: u32, s: u32, brick_size: u32, interior_voxels: u32) -> u32 {
    // `brick_origin * brick_size + s` is always >= 0; subtracting 1 can underflow
    // only at the very first apron voxel (brick_origin = 0, s = 0), which clamps
    // to 0 anyway, so saturate the subtraction.
    let world = (brick_origin * brick_size + s).saturating_sub(1);
    world.min(interior_voxels.saturating_sub(1))
}

fn point_in_solid(tree: &BspTree, p: Vec3) -> bool {
    if tree.leaves.is_empty() {
        return false;
    }
    let leaf_idx = find_leaf_for_point(tree, DVec3::new(p.x as f64, p.y as f64, p.z as f64));
    tree.leaves
        .get(leaf_idx)
        .map(|l| l.is_solid)
        .unwrap_or(false)
}

/// Brute-force nearest unsigned distance from `p` to any triangle.
///
/// An AABB-distance lower bound rejects triangles that cannot possibly beat
/// the current best — the bake stays correct without a BVH because each voxel
/// only needs the *nearest* hit, not all hits.
fn nearest_triangle_distance(triangles: &[Triangle], p: Vec3) -> f32 {
    let mut best_sq = f32::INFINITY;
    for tri in triangles {
        let aabb_lower_sq = point_aabb_distance_sq(p, tri.aabb_min, tri.aabb_max);
        if aabb_lower_sq >= best_sq {
            continue;
        }
        let d_sq = point_triangle_distance_sq(p, tri.a, tri.b, tri.c);
        if d_sq < best_sq {
            best_sq = d_sq;
        }
    }
    best_sq.sqrt()
}

fn point_aabb_distance_sq(p: Vec3, mn: Vec3, mx: Vec3) -> f32 {
    let dx = (mn.x - p.x).max(0.0).max(p.x - mx.x);
    let dy = (mn.y - p.y).max(0.0).max(p.y - mx.y);
    let dz = (mn.z - p.z).max(0.0).max(p.z - mx.z);
    dx * dx + dy * dy + dz * dz
}

/// Squared distance from `p` to triangle `(a,b,c)`. Standard barycentric
/// clamp — projects onto the triangle plane, then clamps into the triangle
/// interior along the closest edge if outside.
fn point_triangle_distance_sq(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.length_squared();
    }

    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return bp.length_squared();
    }

    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        let q = a + ab * v;
        return (p - q).length_squared();
    }

    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return cp.length_squared();
    }

    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        let q = a + ac * w;
        return (p - q).length_squared();
    }

    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        let q = b + (c - b) * w;
        return (p - q).length_squared();
    }

    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    let q = a + ab * v + ac * w;
    (p - q).length_squared()
}

/// Pack `n` surface bricks into a near-cube 3D arrangement. Used only as a
/// hint for the runtime to size the 3D atlas texture — the on-disk linear
/// brick order is what defines slot indices.
fn pack_atlas_dimensions(n: u32) -> [u32; 3] {
    if n == 0 {
        return [0, 0, 0];
    }
    let cbrt = (n as f32).cbrt().ceil() as u32;
    let x = cbrt.max(1);
    let y = cbrt.max(1);
    let z = n.div_ceil(x * y).max(1);
    [x, y, z]
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    use crate::geometry::FaceIndexRange;
    use crate::partition::{Aabb, BspLeaf, BspTree};

    /// Construct a tiny scene: one axis-aligned wall (a quad on the X=0 plane)
    /// between two empty half-spaces, plus a trivial BSP tree marking
    /// `x < 0` as solid. This is the structural assertion fixture for the
    /// wall-straddling-brick check.
    fn wall_scene() -> (GeometryResult, BspTree) {
        // A quad at x=0, spanning y in [-1,1] and z in [-1,1]. Two triangles.
        let v = |x: f32, y: f32, z: f32| Vertex {
            position: [x, y, z],
            uv: [0.0, 0.0],
            normal_oct: [0, 0],
            tangent_packed: [0, 0],
            lightmap_uv: [0, 0],
            lightmap_layer: 0,
            _padding: 0,
        };
        // Wall geometry (referenced by triangles) plus four bounds-only
        // "phantom" vertices that push the world AABB out so far-empty
        // bricks exist in the grid. They contribute no triangles — the
        // index buffer never references indices 4..7.
        let vertices = vec![
            v(0.0, -1.0, -1.0),
            v(0.0, 1.0, -1.0),
            v(0.0, 1.0, 1.0),
            v(0.0, -1.0, 1.0),
            v(3.0, -3.0, -3.0),
            v(3.0, 3.0, -3.0),
            v(3.0, 3.0, 3.0),
            v(3.0, -3.0, 3.0),
        ];
        let indices = vec![0u32, 1, 2, 0, 2, 3];
        let faces = vec![FaceMeta {
            leaf_index: 1,
            texture_index: 0,
        }];
        let geometry = GeometrySection {
            vertices,
            indices,
            faces,
        };
        let texture_names = TextureNamesSection {
            names: vec!["dev/wall".to_string()],
        };
        let face_index_ranges = vec![FaceIndexRange {
            index_offset: 0,
            index_count: 6,
        }];
        let geo = GeometryResult {
            geometry,
            texture_names,
            face_index_ranges,
        };

        // Minimal BSP: root node splitting on the YZ plane (normal = +X).
        // Left child (x < 0) → solid leaf; right child (x >= 0) → empty leaf.
        use crate::partition::{BspChild, BspNode};
        let nodes = vec![BspNode {
            plane_normal: DVec3::new(1.0, 0.0, 0.0),
            plane_distance: 0.0,
            front: BspChild::Leaf(1), // x >= 0 → empty
            back: BspChild::Leaf(0),  // x < 0 → solid
            parent: None,
        }];
        let leaves = vec![
            BspLeaf {
                face_indices: vec![],
                bounds: Aabb {
                    min: DVec3::new(-4.0, -4.0, -4.0),
                    max: DVec3::new(0.0, 4.0, 4.0),
                },
                is_solid: true,
                defining_planes: vec![],
            },
            BspLeaf {
                face_indices: vec![0],
                bounds: Aabb {
                    min: DVec3::new(0.0, -4.0, -4.0),
                    max: DVec3::new(4.0, 4.0, 4.0),
                },
                is_solid: false,
                defining_planes: vec![],
            },
        ];
        let tree = BspTree { nodes, leaves };
        (geo, tree)
    }

    #[test]
    fn empty_geometry_yields_empty_section() {
        let geo = GeometryResult {
            geometry: GeometrySection {
                vertices: vec![],
                indices: vec![],
                faces: vec![],
            },
            texture_names: TextureNamesSection { names: vec![] },
            face_index_ranges: vec![],
        };
        let tree = BspTree {
            nodes: vec![],
            leaves: vec![],
        };
        let section = bake_sdf_atlas(
            &SdfBakeCtx {
                geometry: &geo,
                tree: &tree,
            },
            &SdfConfig::default(),
        );
        assert_eq!(section, SdfAtlasSection::empty());
    }

    #[test]
    fn rebake_is_byte_identical() {
        let (geo, tree) = wall_scene();
        let cfg = SdfConfig::default();
        let a = bake_sdf_atlas(
            &SdfBakeCtx {
                geometry: &geo,
                tree: &tree,
            },
            &cfg,
        )
        .to_bytes();
        let b = bake_sdf_atlas(
            &SdfBakeCtx {
                geometry: &geo,
                tree: &tree,
            },
            &cfg,
        )
        .to_bytes();
        assert_eq!(a, b, "SDF bake must be byte-identical on identical inputs");
    }

    /// Structural assertion: a brick straddling the wall stores a near-zero
    /// signed distance at the surface and increasing magnitudes away from it;
    /// an all-open brick is marked empty in the top-level index, distinguishable
    /// from a surface slot. Anchors the AC for [T2].
    #[test]
    fn wall_straddling_brick_has_zero_distance_at_surface() {
        let (geo, tree) = wall_scene();
        // Tight grid so the wall is sampled densely.
        let cfg = SdfConfig {
            voxel_size_m: 0.25,
            brick_size_voxels: 4,
        };
        let section = bake_sdf_atlas(
            &SdfBakeCtx {
                geometry: &geo,
                tree: &tree,
            },
            &cfg,
        );
        // At least one surface brick must exist — the wall is inside the grid.
        assert!(
            section.surface_brick_count >= 1,
            "wall scene must produce at least one surface brick, got {}",
            section.surface_brick_count,
        );
        // At least one empty-sentinel slot must exist somewhere far from the
        // wall — the open half-space far from x=0 has bricks beyond the
        // surface band.
        let empty_slots = section
            .top_level
            .iter()
            .filter(|s| **s == BRICK_SLOT_EMPTY)
            .count();
        assert!(
            empty_slots >= 1,
            "expected at least one empty-sentinel top-level slot (open space far \
             from the wall), got {empty_slots}",
        );

        // Pick the first surface brick's sample range and check it spans
        // near-zero. i16 step is voxel_size/256, so "very close to the wall"
        // shows up as a small |quant| value. Voxel centres are offset half a
        // voxel from the grid origin, so the closest possible sample to a
        // perfectly-aligned wall is ~half-voxel away — quant ≈ 128. Allow
        // up to 200 quant steps (~78% of a voxel) so any voxel adjacent to
        // the wall qualifies as "near-surface".
        let stored_edge = cfg.brick_size_voxels + 2;
        let voxels_per_brick = (stored_edge * stored_edge * stored_edge) as usize;
        let any_near_surface = section
            .atlas
            .iter()
            .take(voxels_per_brick)
            .any(|q| q.unsigned_abs() <= 200);
        assert!(
            any_near_surface,
            "first surface brick must contain a near-surface sample (|q| <= 64), \
             got samples: {:?}",
            &section.atlas[..voxels_per_brick.min(section.atlas.len())],
        );

        // Sign axis: the brick must contain both negative (solid side) and
        // positive (open side) samples — a true "straddling" brick.
        let has_negative = section.atlas.iter().take(voxels_per_brick).any(|q| *q < 0);
        let has_positive = section.atlas.iter().take(voxels_per_brick).any(|q| *q > 0);
        assert!(
            has_negative && has_positive,
            "wall-straddling brick must contain samples on both sides of the \
             surface (negative and positive). got negative={has_negative}, \
             positive={has_positive}",
        );

        // The brick's coarse clearance is now a conservative MIN of unsigned
        // per-voxel distances (minus a half-voxel-diagonal margin), clamped >= 0.
        // A wall-straddling brick has voxels right on the wall, so its clearance
        // is small (well under a brick edge). Order-of-magnitude check.
        let first_surface_brick_idx = section
            .top_level
            .iter()
            .position(|s| *s == 0)
            .expect("surface-brick slot 0 must exist in top_level");
        let coarse_at = section.coarse_distances[first_surface_brick_idx];
        assert!(
            (0.0..cfg.voxel_size_m * cfg.brick_size_voxels as f32).contains(&coarse_at),
            "coarse clearance at a wall-straddling brick should be in \
             [0, brick edge length), got {coarse_at}",
        );
    }

    /// Guards the sphere-trace lower-bound invariant (Hart 1996): the per-brick
    /// coarse value must be a CONSERVATIVE MIN clearance, not the per-voxel MEAN.
    /// The mean overstates distance-to-surface and lets the shadow march overstep
    /// / tunnel through sub-brick geometry. We pick an EMPTY brick whose per-voxel
    /// distances span a gradient (so min ≠ mean), then assert the stored value
    /// equals `(min − half-voxel-diagonal margin).max(0)` and is NOT the mean.
    #[test]
    fn coarse_value_stores_conservative_min_not_mean() {
        let (geo, tree) = wall_scene();
        let cfg = SdfConfig {
            voxel_size_m: 0.5,
            brick_size_voxels: 4,
        };
        let ctx = SdfBakeCtx {
            geometry: &geo,
            tree: &tree,
        };
        let section = bake_sdf_atlas(&ctx, &cfg);

        // Independently reconstruct per-voxel distances so the assertion does not
        // round-trip the bake's own aggregate (would prove nothing).
        let triangles = collect_triangles(&geo);
        let voxel_size = cfg.voxel_size_m;
        let brick_size = cfg.brick_size_voxels;
        let voxels_per_brick = (brick_size * brick_size * brick_size) as usize;
        let (world_min, world_max) = world_aabb(&ctx);
        let (grid_origin, brick_dims) = grid_extents(world_min, world_max, voxel_size, brick_size);

        // Find an EMPTY brick whose voxel-center distances form a gradient, so
        // the minimum and the mean are meaningfully different. Bricks set back
        // from the x=0 wall have a clear min-to-mean spread.
        let mut found: Option<(usize, f32, f32)> = None; // (brick_idx, min, mean)
        'outer: for bz in 0..brick_dims[2] {
            for by in 0..brick_dims[1] {
                for bx in 0..brick_dims[0] {
                    let brick_idx =
                        (bz * brick_dims[1] * brick_dims[0] + by * brick_dims[0] + bx) as usize;
                    if section.top_level[brick_idx] != BRICK_SLOT_EMPTY {
                        continue;
                    }
                    let mut min_unsigned = f32::INFINITY;
                    let mut sum = 0.0f64;
                    for vz in 0..brick_size {
                        for vy in 0..brick_size {
                            for vx in 0..brick_size {
                                let p = voxel_center(
                                    grid_origin,
                                    bx * brick_size + vx,
                                    by * brick_size + vy,
                                    bz * brick_size + vz,
                                    voxel_size,
                                );
                                let d = nearest_triangle_distance(&triangles, p);
                                min_unsigned = min_unsigned.min(d);
                                sum += d as f64;
                            }
                        }
                    }
                    let mean = (sum / voxels_per_brick as f64) as f32;
                    // Require a real spread so the "not the mean" check has teeth.
                    if mean - min_unsigned > 0.5 * voxel_size {
                        found = Some((brick_idx, min_unsigned, mean));
                        break 'outer;
                    }
                }
            }
        }

        let (brick_idx, min_unsigned, mean) =
            found.expect("expected an EMPTY brick with a min-to-mean distance gradient");

        let expected = (min_unsigned - voxel_size * COARSE_SAFETY_MARGIN_VOXELS).max(0.0);
        let stored = section.coarse_distances[brick_idx];

        // Stored value matches the conservative-min formula.
        assert!(
            (stored - expected).abs() < 1.0e-4,
            "coarse value must be (min − margin).max(0) = {expected}, got {stored} \
             (min_unsigned={min_unsigned}, mean={mean})",
        );
        // ...and is a valid lower bound: never above the minimum sample.
        assert!(
            stored <= min_unsigned + 1.0e-4,
            "coarse value {stored} must not exceed the minimum sample {min_unsigned}",
        );
        // ...and is explicitly NOT the mean (this would fail under old behavior).
        assert!(
            (stored - mean).abs() > 0.5 * voxel_size,
            "coarse value {stored} must NOT equal the per-voxel mean {mean} \
             (the old, sphere-trace-unsafe behavior)",
        );
    }

    /// Construct a wide horizontal floor (a quad on the Y=0 plane) with empty
    /// space above and solid below. The distance field near the floor is the
    /// continuous unsigned distance to the plane, so every brick along the
    /// floor is a surface brick — giving the x-adjacent surface bricks the
    /// apron/seam tests need.
    fn floor_scene() -> (GeometryResult, BspTree) {
        let v = |x: f32, y: f32, z: f32| Vertex {
            position: [x, y, z],
            uv: [0.0, 0.0],
            normal_oct: [0, 0],
            tangent_packed: [0, 0],
            lightmap_uv: [0, 0],
            lightmap_layer: 0,
            _padding: 0,
        };
        // Floor quad at y=0 spanning x,z in [-6, 6]. Two triangles.
        let vertices = vec![
            v(-6.0, 0.0, -6.0),
            v(6.0, 0.0, -6.0),
            v(6.0, 0.0, 6.0),
            v(-6.0, 0.0, 6.0),
        ];
        let indices = vec![0u32, 1, 2, 0, 2, 3];
        let faces = vec![FaceMeta {
            leaf_index: 1,
            texture_index: 0,
        }];
        let geometry = GeometrySection {
            vertices,
            indices,
            faces,
        };
        let texture_names = TextureNamesSection {
            names: vec!["dev/floor".to_string()],
        };
        let face_index_ranges = vec![FaceIndexRange {
            index_offset: 0,
            index_count: 6,
        }];
        let geo = GeometryResult {
            geometry,
            texture_names,
            face_index_ranges,
        };

        // Minimal BSP: split on the XZ plane (normal = +Y). y >= 0 → empty,
        // y < 0 → solid.
        use crate::partition::{BspChild, BspNode};
        let nodes = vec![BspNode {
            plane_normal: DVec3::new(0.0, 1.0, 0.0),
            plane_distance: 0.0,
            front: BspChild::Leaf(1), // y >= 0 → empty
            back: BspChild::Leaf(0),  // y < 0 → solid
            parent: None,
        }];
        let leaves = vec![
            BspLeaf {
                face_indices: vec![],
                bounds: Aabb {
                    min: DVec3::new(-8.0, -8.0, -8.0),
                    max: DVec3::new(8.0, 0.0, 8.0),
                },
                is_solid: true,
                defining_planes: vec![],
            },
            BspLeaf {
                face_indices: vec![0],
                bounds: Aabb {
                    min: DVec3::new(-8.0, 0.0, -8.0),
                    max: DVec3::new(8.0, 8.0, 8.0),
                },
                is_solid: false,
                defining_planes: vec![],
            },
        ];
        let tree = BspTree { nodes, leaves };
        (geo, tree)
    }

    /// Re-derive the quantized stored value the bake would write for an
    /// arbitrary global voxel index, using the same eval the bake uses. Lets
    /// the apron tests assert against an independently-computed field value
    /// rather than round-tripping the bake's own output.
    fn quantized_field_at(
        geo: &GeometryResult,
        tree: &BspTree,
        grid_origin: DVec3,
        world_v: [u32; 3],
        voxel_size: f32,
    ) -> i16 {
        let triangles = collect_triangles(geo);
        let p = voxel_center(grid_origin, world_v[0], world_v[1], world_v[2], voxel_size);
        let unsigned = nearest_triangle_distance(&triangles, p);
        let signed = if point_in_solid(tree, p) {
            -unsigned
        } else {
            unsigned
        };
        let inv_quant_step = 256.0 / voxel_size;
        (signed * inv_quant_step)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16
    }

    /// Apron contract: an apron voxel stores the true field value at the
    /// neighbor position it mirrors (interior of an adjacent brick), within
    /// quant epsilon. We take the +x apron column of a surface brick and assert
    /// it equals the independently-evaluated field at world voxel index
    /// `(bx+1)*brick_size` — the voxel the apron mirrors.
    #[test]
    fn apron_voxel_mirrors_neighbor_field_value() {
        let (geo, tree) = floor_scene();
        let cfg = SdfConfig {
            voxel_size_m: 0.5,
            brick_size_voxels: 4,
        };
        let ctx = SdfBakeCtx {
            geometry: &geo,
            tree: &tree,
        };
        let section = bake_sdf_atlas(&ctx, &cfg);

        let brick_size = cfg.brick_size_voxels;
        let edge = brick_size + 2;
        let voxels_per_brick = (edge * edge * edge) as usize;
        let (world_min, world_max) = world_aabb(&ctx);
        let (grid_origin, brick_dims) =
            grid_extents(world_min, world_max, cfg.voxel_size_m, brick_size);

        // Find a surface brick that has a +x neighbor inside the grid, so its
        // +x apron mirrors a real neighbor voxel (not the edge-extended case).
        let (bx, by, bz) = find_surface_brick_with_x_neighbor(&section, brick_dims);
        let slot = section.top_level
            [(bz * brick_dims[1] * brick_dims[0] + by * brick_dims[0] + bx) as usize];
        let brick_base = slot as usize * voxels_per_brick;

        // +x apron column: sx = brick_size + 1, mirrors world voxel index
        // (bx+1)*brick_size. Check the whole column across sy, sz interior range.
        for sz in 1..=brick_size {
            for sy in 1..=brick_size {
                let sx = brick_size + 1;
                let vox_idx = (sz * edge * edge + sy * edge + sx) as usize;
                let stored = section.atlas[brick_base + vox_idx];

                let world_v = [
                    (bx + 1) * brick_size,
                    by * brick_size + (sy - 1),
                    bz * brick_size + (sz - 1),
                ];
                let expected =
                    quantized_field_at(&geo, &tree, grid_origin, world_v, cfg.voxel_size_m);
                // Same eval + same quant → exact equality (within ±1 quant step
                // for floating-point rounding determinism across the two paths).
                assert!(
                    (stored as i32 - expected as i32).abs() <= 1,
                    "apron voxel at sx={sx},sy={sy},sz={sz} stored {stored} but the \
                     mirrored neighbor field is {expected}",
                );
            }
        }
    }

    /// Seam continuity: a brick's +x apron column equals the +x-neighbor
    /// brick's first interior column, voxel-for-voxel. Both reference the same
    /// world voxel index `(bx+1)*brick_size`, so a hardware-trilinear sampler
    /// reading across the seam sees no discontinuity. Requires >=2 x-adjacent
    /// surface bricks — the wide floor fixture provides them.
    #[test]
    fn plus_x_apron_matches_neighbor_first_interior_column() {
        let (geo, tree) = floor_scene();
        let cfg = SdfConfig {
            voxel_size_m: 0.5,
            brick_size_voxels: 4,
        };
        let ctx = SdfBakeCtx {
            geometry: &geo,
            tree: &tree,
        };
        let section = bake_sdf_atlas(&ctx, &cfg);

        let brick_size = cfg.brick_size_voxels;
        let edge = brick_size + 2;
        let voxels_per_brick = (edge * edge * edge) as usize;
        let (world_min, world_max) = world_aabb(&ctx);
        let (_grid_origin, brick_dims) =
            grid_extents(world_min, world_max, cfg.voxel_size_m, brick_size);

        let (bx, by, bz) = find_surface_brick_with_x_neighbor(&section, brick_dims);
        let cell = |x: u32, y: u32, z: u32| {
            (z * brick_dims[1] * brick_dims[0] + y * brick_dims[0] + x) as usize
        };
        let slot = section.top_level[cell(bx, by, bz)];
        let neighbor_slot = section.top_level[cell(bx + 1, by, bz)];
        assert!(
            neighbor_slot != BRICK_SLOT_EMPTY && neighbor_slot != BRICK_SLOT_INTERIOR,
            "the +x-neighbor must also be a surface brick for the seam test",
        );
        let base = slot as usize * voxels_per_brick;
        let neighbor_base = neighbor_slot as usize * voxels_per_brick;

        // Compact stored-brick voxel index: vox_idx = sz*edge^2 + sy*edge + sx,
        // z-major within the apron'd brick.
        let vox_idx = |sx: u32, sy: u32, sz: u32| (sz * edge * edge + sy * edge + sx) as usize;

        for sz in 1..=brick_size {
            for sy in 1..=brick_size {
                // Brick's +x apron column (sx = brick_size + 1).
                let apron = section.atlas[base + vox_idx(brick_size + 1, sy, sz)];
                // Neighbor's first interior column (sx = 1).
                let neighbor_interior = section.atlas[neighbor_base + vox_idx(1, sy, sz)];
                assert_eq!(
                    apron, neighbor_interior,
                    "+x apron column at sy={sy},sz={sz} ({apron}) must equal the \
                     +x-neighbor's first interior column ({neighbor_interior}) — \
                     no seam discontinuity",
                );
            }
        }
    }

    /// Quake-unit -> engine-meter transform (mirrors `parse::quake_to_engine`
    /// plus the IdTech2 0.0254 m scale): `ex = -qy·s, ey = qz·s, ez = -qx·s`.
    /// Lets the SDF-fixture tests state probe points in the map's own Quake
    /// coordinates and compare against the baked, engine-space field.
    #[cfg(test)]
    fn quake_to_engine(qx: f32, qy: f32, qz: f32) -> Vec3 {
        const S: f32 = 0.0254;
        Vec3::new(-qy * S, qz * S, -qx * S)
    }

    /// Verifies the baked SDF static-occluder atlas represents the occluding
    /// geometry between a floor receiver and an `_shadow_type "sdf"` light,
    /// asserted against the known geometry of the purpose-built
    /// `sdf-shadow-test.map` fixture (generated by
    /// `tools/gen_sdf_shadow_fixture.py`).
    ///
    /// By construction the fixture has a solid occluder slab floating over the
    /// low-X half of the floor and an SDF light directly above it, so:
    ///   * a point inside the slab classifies as solid (negative SDF) and a
    ///     point in the open air classifies as empty;
    ///   * the SHADOWED floor point's floor->light ray passes through the solid
    ///     slab, while the LIT floor point's ray (far across the room, in the
    ///     open) never enters solid.
    ///
    /// The small fixture bakes in well under a second in-process, so this is a
    /// normal (non-`#[ignore]`) test.
    #[test]
    fn sdf_atlas_marks_occluder_between_floor_and_sdf_light() {
        use crate::map_data::ShadowType;
        use crate::map_format::MapFormat;
        use std::collections::HashSet;

        // 1. Build geometry + BSP exactly like the real bake (main.rs) from the
        //    committed fixture.
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
        let triangles = collect_triangles(&geo_result);
        assert!(
            !triangles.is_empty(),
            "fixture must produce occluder + room triangles"
        );

        // The fixture authors exactly one `_shadow_type "sdf"` light — true by
        // construction, so no `.expect` on drifting content.
        let sdf_light = map_data
            .lights
            .iter()
            .find(|l| l.shadow_type == ShadowType::Sdf)
            .expect("fixture must author one Sdf-shadow light");
        let light = Vec3::new(
            sdf_light.origin.x as f32,
            sdf_light.origin.y as f32,
            sdf_light.origin.z as f32,
        );
        // The light should sit roughly where the fixture authored it (above the
        // occluder); compare against the Quake->engine transform of (416,256,352).
        let expected_light = quake_to_engine(416.0, 256.0, 352.0);
        assert!(
            (light - expected_light).length() < 0.05,
            "Sdf light at {light:?}, expected ~{expected_light:?}",
        );

        let ctx = SdfBakeCtx {
            geometry: &geo_result,
            tree: &result.tree,
        };
        let section = bake_sdf_atlas(&ctx, &SdfConfig::default());
        let tree = ctx.tree;
        assert!(
            section.surface_brick_count >= 1,
            "fixture must produce surface bricks around the room + occluder",
        );

        // 2. Classification: a point inside the occluder slab is solid; a point
        //    in the open air (same height, far across the room) is empty.
        let occluder_center = quake_to_engine(416.0, 256.0, 192.0);
        let open_air = quake_to_engine(896.0, 256.0, 192.0);
        assert!(
            point_in_solid(tree, occluder_center),
            "occluder-center {occluder_center:?} must classify as solid (inside the slab)",
        );
        assert!(
            !point_in_solid(tree, open_air),
            "open-air point {open_air:?} must classify as empty",
        );

        // The atlas's signed field agrees: the occluder center is at negative
        // (interior) signed distance, the open-air point at positive distance.
        // Quantized step is voxel_size/256, so a deep-interior voxel reads a
        // strongly negative quant. We re-derive the field with the bake's own
        // eval to assert the SIGN, the observable the runtime tracer keys on.
        let signed_at = |p: Vec3| -> f32 {
            let unsigned = nearest_triangle_distance(&triangles, p);
            if point_in_solid(tree, p) {
                -unsigned
            } else {
                unsigned
            }
        };
        const EPS: f32 = 1.0e-4;
        assert!(
            signed_at(occluder_center) < -EPS,
            "signed field inside the occluder must be negative, got {}",
            signed_at(occluder_center),
        );
        assert!(
            signed_at(open_air) > EPS,
            "signed field in open air must be positive, got {}",
            signed_at(open_air),
        );

        // 3. March the floor->light ray for the SHADOWED column (under the
        //    occluder) and the LIT column (in the open). The shadowed ray must
        //    pass through solid occluder geometry; the lit ray must not.
        let march_hits_solid = |floor: Vec3| -> bool {
            let to_light = light - floor;
            let max_t = to_light.length();
            let dir = to_light / max_t;
            let step = section.voxel_size_m * 0.5;
            // Start just off the floor surface so the floor slab itself does not
            // count as the occluder.
            let mut t = section.voxel_size_m;
            while t <= max_t - section.voxel_size_m {
                if point_in_solid(tree, floor + dir * t) {
                    return true;
                }
                t += step;
            }
            false
        };
        let shadowed_floor = quake_to_engine(416.0, 256.0, 4.0);
        let lit_floor = quake_to_engine(896.0, 256.0, 4.0);
        assert!(
            march_hits_solid(shadowed_floor),
            "SHADOWED floor->light ray ({shadowed_floor:?} -> {light:?}) must pass \
             through the solid occluder slab",
        );
        assert!(
            !march_hits_solid(lit_floor),
            "LIT floor->light ray ({lit_floor:?} -> {light:?}) must be unobstructed",
        );
    }

    /// Scan the top-level index for a surface brick whose +x neighbor (`bx+1`)
    /// is also a surface brick. Panics if none exists — the floor fixture is
    /// sized to guarantee a run of x-adjacent surface bricks.
    fn find_surface_brick_with_x_neighbor(
        section: &SdfAtlasSection,
        brick_dims: [u32; 3],
    ) -> (u32, u32, u32) {
        let is_surface = |x: u32, y: u32, z: u32| {
            let slot = section.top_level
                [(z * brick_dims[1] * brick_dims[0] + y * brick_dims[0] + x) as usize];
            slot != BRICK_SLOT_EMPTY && slot != BRICK_SLOT_INTERIOR
        };
        for bz in 0..brick_dims[2] {
            for by in 0..brick_dims[1] {
                for bx in 0..brick_dims[0].saturating_sub(1) {
                    if is_surface(bx, by, bz) && is_surface(bx + 1, by, bz) {
                        return (bx, by, bz);
                    }
                }
            }
        }
        panic!("floor fixture must contain >=2 x-adjacent surface bricks");
    }
}
