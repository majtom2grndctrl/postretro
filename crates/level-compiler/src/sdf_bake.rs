// SDF static-occluder atlas bake.
//
// Builds a sparse brick atlas of signed distances from each voxel to the
// nearest static-world triangle, packaged as a `SdfAtlasSection` for the PRL.
// Drives the runtime SDF static-occluder shadow pass (see
// `context/plans/in-progress/sdf-static-occluder-shadows/index.md`).
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
//    it. Atlas brick layout is z-major-within-brick, mirroring the runtime
//    sampler convention noted in `sdf_atlas.rs`.
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
pub const STAGE_VERSION: u32 = 2;

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
/// key via postcard (mirrors `ShInputs` / `LightmapInputs`).
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
    let voxels_per_brick = (brick_size * brick_size * brick_size) as usize;

    let (world_min, world_max) = world_aabb(ctx);
    let (grid_origin, brick_dims) = grid_extents(world_min, world_max, voxel_size, brick_size);
    let total_bricks = brick_dims[0] as usize * brick_dims[1] as usize * brick_dims[2] as usize;

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
    // The ordering here is `z*nx*ny + y*nx + x` over the brick voxel grid, so
    // the bake's atlas layout is z-major-within-brick.
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

                for vz in 0..brick_size {
                    for vy in 0..brick_size {
                        for vx in 0..brick_size {
                            let p = voxel_center(
                                grid_origin,
                                bx,
                                by,
                                bz,
                                vx,
                                vy,
                                vz,
                                brick_size,
                                voxel_size,
                            );
                            let unsigned = nearest_triangle_distance(&triangles, p);
                            min_unsigned = min_unsigned.min(unsigned);

                            let inside_solid = point_in_solid(ctx.tree, p);
                            if inside_solid {
                                all_empty_air = false;
                            } else {
                                all_solid = false;
                            }
                            let signed = if inside_solid { -unsigned } else { unsigned };

                            let q_raw = (signed * inv_quant_step).round();
                            let q_clamped = q_raw.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                            brick_samples.push(q_clamped);
                            max_abs_quant = max_abs_quant.max(q_clamped.unsigned_abs() as f32);
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

#[allow(clippy::too_many_arguments)]
fn voxel_center(
    origin: DVec3,
    bx: u32,
    by: u32,
    bz: u32,
    vx: u32,
    vy: u32,
    vz: u32,
    brick_size: u32,
    voxel_size: f32,
) -> Vec3 {
    let voxel = voxel_size as f64;
    let world_vx = bx * brick_size + vx;
    let world_vy = by * brick_size + vy;
    let world_vz = bz * brick_size + vz;
    let p = origin
        + DVec3::new(
            (world_vx as f64 + 0.5) * voxel,
            (world_vy as f64 + 0.5) * voxel,
            (world_vz as f64 + 0.5) * voxel,
        );
    Vec3::new(p.x as f32, p.y as f32, p.z as f32)
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
        let voxels_per_brick =
            (cfg.brick_size_voxels * cfg.brick_size_voxels * cfg.brick_size_voxels) as usize;
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
                                    bx,
                                    by,
                                    bz,
                                    vx,
                                    vy,
                                    vz,
                                    brick_size,
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
}
