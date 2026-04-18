// SDF atlas baker: brick-indexed sparse signed distance field over all static
// world geometry. Output feeds the runtime sphere-trace shadow path.
//
// See: context/plans/in-progress/lighting-foundation/8-sdf-shadows.md

use std::collections::VecDeque;

use bvh::aabb::Aabb;
use bvh::bvh::Bvh;
use glam::Vec3;
use nalgebra::Point3;
use postretro_level_format::sdf_atlas::{
    BRICK_SLOT_EMPTY, BRICK_SLOT_INTERIOR, SDF_FALLBACK_DISTANCE_M, SdfAtlasSection,
};
use rayon::prelude::*;

use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;

/// Default voxel edge length in meters.
pub const DEFAULT_VOXEL_SIZE_M: f32 = 0.08;
/// Default brick edge in voxels; brick volume = 8³ = 512 voxels.
pub const DEFAULT_BRICK_SIZE_VOXELS: u32 = 8;
/// Default world-bounds margin in meters.
pub const DEFAULT_MARGIN_M: f32 = 1.0;

/// Quantization scale: 1 i16 unit = voxel_size_m / 256.0 meters.
/// At 0.08 m voxels → 0.078/256 ≈ 0.31 mm per unit.
/// The representable range is ±(32767 / 256) * voxel_size_m.
const QUANT_UNITS_PER_METER_FACTOR: f32 = 256.0;

/// Max representable distance in i16 (saturate at this value).
const MAX_QUANT: i16 = i16::MAX;
const MIN_QUANT: i16 = i16::MIN;

/// BVH-backed inputs for the SDF baker.
pub struct SdfBakeInputs<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    /// World-space origin of the player start entity. The flood-fill that
    /// classifies empty bricks as EMPTY vs INTERIOR seeds from this point:
    /// bricks reachable from here through air are EMPTY (open air the player
    /// can occupy); unreachable bricks are INTERIOR (inside solid geometry or
    /// in the exterior margin band — both correctly hard-occlude shadow rays).
    ///
    /// Must be inside the playable area of a watertight level. If the level
    /// has a leak, the flood will reach the grid boundary and a warning is
    /// logged; shadows will be absent rather than incorrect.
    pub player_start_world: Vec3,
}

/// Bake an SDF atlas at the requested voxel and brick sizes.
///
/// Returns an `SdfAtlasSection` ready for serialization into the PRL file.
/// If the geometry is empty, returns a minimal degenerate section (zero
/// top-level cells, zero atlas bytes).
pub fn bake_sdf_atlas(
    inputs: &SdfBakeInputs<'_>,
    voxel_size_m: f32,
    brick_size_voxels: u32,
    margin_m: f32,
) -> SdfAtlasSection {
    let geom = &inputs.geometry.geometry;
    if geom.vertices.is_empty() {
        return SdfAtlasSection {
            world_min: [0.0; 3],
            world_max: [0.0; 3],
            voxel_size_m,
            brick_size_voxels,
            grid_dims: [0, 0, 0],
            top_level: Vec::new(),
            atlas: Vec::new(),
            coarse_distances: Vec::new(),
        };
    }

    let (raw_min, raw_max) = world_aabb(geom);
    let world_min = [
        raw_min[0] - margin_m,
        raw_min[1] - margin_m,
        raw_min[2] - margin_m,
    ];
    let world_max = [
        raw_max[0] + margin_m,
        raw_max[1] + margin_m,
        raw_max[2] + margin_m,
    ];

    // Brick world-space size in meters.
    let brick_m = voxel_size_m * brick_size_voxels as f32;
    let brick_vol = (brick_size_voxels as usize).pow(3);

    // Grid dimensions (number of bricks along each axis).
    let extents = [
        world_max[0] - world_min[0],
        world_max[1] - world_min[1],
        world_max[2] - world_min[2],
    ];
    let grid_dims = [
        ((extents[0] / brick_m).ceil() as u32).max(1),
        ((extents[1] / brick_m).ceil() as u32).max(1),
        ((extents[2] / brick_m).ceil() as u32).max(1),
    ];

    let total_bricks = (grid_dims[0] as usize) * (grid_dims[1] as usize) * (grid_dims[2] as usize);

    // Empty/interior classification is done by flood-fill from the grid
    // boundary (see `flood_fill_exterior` below). Correctness depends on the
    // boundary being guaranteed exterior, which in turn requires the margin
    // to be strictly larger than one brick edge: if `margin_m > brick_m`,
    // every boundary brick sits fully inside the margin band, outside the
    // raw geometry AABB, and therefore contains no triangles. Enforce it
    // here so a caller cannot silently shrink the margin below that floor.
    assert!(
        margin_m > brick_m,
        "sdf_bake margin_m ({margin_m}) must exceed brick_m ({brick_m}) so boundary bricks are guaranteed exterior"
    );

    log::info!(
        "[SdfBake] Grid {}x{}x{} = {} bricks, voxel_size={:.3}m, brick_size={}",
        grid_dims[0], grid_dims[1], grid_dims[2],
        total_bricks, voxel_size_m, brick_size_voxels
    );

    // Phase 1 (parallel): per-brick, either compute per-voxel SDF data (if
    // the brick overlaps any geometry) or mark it as "no surface" — empty
    // air that still needs exterior/interior classification in phase 2.
    let phase1: Vec<Phase1> = (0..total_bricks)
        .into_par_iter()
        .map(|linear| {
            let (bx, by, bz) = linear_to_brick(linear, grid_dims);
            let brick_origin = [
                world_min[0] + bx as f32 * brick_m,
                world_min[1] + by as f32 * brick_m,
                world_min[2] + bz as f32 * brick_m,
            ];
            classify_brick(
                inputs,
                brick_origin,
                voxel_size_m,
                brick_size_voxels,
                brick_vol,
            )
        })
        .collect();

    // Phase 2 (sequential): BFS flood-fill from the player-start brick
    // through no-surface bricks. Reached bricks are open air the player can
    // occupy (→ EMPTY sentinel); unreached no-surface bricks are either
    // inside solid geometry or in the exterior margin band (→ INTERIOR
    // sentinel — both correctly hard-occlude shadow rays in the tracer).
    //
    // This is the opposite of seeding from the grid boundary: that approach
    // marked playable rooms INTERIOR because walls blocked the boundary flood
    // from entering them, making the tracer treat all rooms as solid.
    let reachable_air = flood_fill_reachable_air(
        &phase1,
        grid_dims,
        world_min,
        inputs.player_start_world,
        voxel_size_m,
        brick_size_voxels,
    );

    // Phase 3: assemble the top-level index and pack surface bricks into
    // the atlas. Surface-brick slot indices are assigned in linear order.
    let mut top_level = Vec::with_capacity(total_bricks);
    let mut atlas: Vec<i16> = Vec::new();
    let mut surface_slot: u32 = 0;

    for (linear, result) in phase1.into_iter().enumerate() {
        match result {
            Phase1::NotSurface => {
                if reachable_air[linear] {
                    top_level.push(BRICK_SLOT_EMPTY);
                } else {
                    top_level.push(BRICK_SLOT_INTERIOR);
                }
            }
            Phase1::Surface(data) => {
                top_level.push(surface_slot);
                atlas.extend_from_slice(&data);
                surface_slot += 1;
            }
        }
    }

    // Phase 4 (parallel): compute a coarse signed distance at every brick
    // CENTER. This populates a grid-resolution SDF that the sphere tracer
    // samples trilinearly when it's in a non-SURFACE brick. Without this,
    // EMPTY bricks return a huge-positive sentinel and the tracer jumps the
    // full light distance in one step, missing every occluder it hasn't
    // already entered a SURFACE brick of.
    //
    // Sign convention: positive for reachable air, negative for interior /
    // unreachable. For SURFACE bricks we use the winding-consistent nearest-
    // face test (same as per-voxel in classify_brick). Magnitude = distance
    // from brick center to the nearest triangle, found via iterative BVH
    // AABB expansion starting at one brick radius.
    let coarse_distances: Vec<f32> = (0..total_bricks)
        .into_par_iter()
        .map(|linear| {
            let (bx, by, bz) = linear_to_brick(linear, grid_dims);
            let brick_center = Vec3::new(
                world_min[0] + (bx as f32 + 0.5) * brick_m,
                world_min[1] + (by as f32 + 0.5) * brick_m,
                world_min[2] + (bz as f32 + 0.5) * brick_m,
            );
            coarse_brick_distance(
                inputs,
                brick_center,
                brick_m,
                top_level[linear],
                reachable_air[linear],
            )
        })
        .collect();

    let surface_count = surface_slot;
    log::info!(
        "[SdfBake] Atlas: {surface_count} surface bricks, {} atlas bytes, {} top-level bytes, \
         {} coarse bytes",
        atlas.len() * 2,
        top_level.len() * 4,
        coarse_distances.len() * 4,
    );

    SdfAtlasSection {
        world_min,
        world_max,
        voxel_size_m,
        brick_size_voxels,
        grid_dims,
        top_level,
        atlas,
        coarse_distances,
    }
}

/// Signed distance from a brick center to the nearest triangle in the BVH.
///
/// Uses iterative AABB expansion: starts at a 1-brick radius and doubles the
/// query radius until at least one primitive is found (or we exceed a large
/// cap).
///
/// Correctness of the AABB query: `gather_triangles` returns every triangle
/// whose AABB overlaps the query cube. A triangle whose AABB is entirely
/// outside a cube of half-extent `radius` centered on `brick_center` has
/// L∞ distance > radius from the center, and since L2 ≥ L∞, its L2 distance
/// is also > radius. So once `unsigned < radius`, we know no unseen triangle
/// can be closer. When `unsigned >= radius` (the nearest found sits on or
/// past the query boundary), we re-query at `radius = unsigned` before
/// returning — cheap safety net for edge cases.
///
/// Sign rules:
///   - SURFACE brick, first iteration: winding-consistent nearest-face normal
///     test (same as per-voxel baking in `classify_brick`). Only reliable
///     when geometry is genuinely adjacent, which the first-iteration guard
///     enforces — "radius == brick_m" means the triangle is within one brick
///     of the center.
///   - All other cases (non-SURFACE, or SURFACE whose geometry was only
///     found after expansion): sign from flood-fill classification
///     (`is_reachable_air`). Positive for open air (EMPTY), negative for
///     interior (INTERIOR). Robust across rooms where a single face normal
///     can't answer "which side am I on".
fn coarse_brick_distance(
    inputs: &SdfBakeInputs<'_>,
    brick_center: Vec3,
    brick_m: f32,
    top_slot: u32,
    is_reachable_air: bool,
) -> f32 {
    let geom = &inputs.geometry.geometry;
    // Cap expansion at ~1024 bricks on either side — more than covers any
    // reasonable level. If we never find a triangle, fall back to a large
    // signed value.
    const MAX_ITERS: u32 = 11;

    let mut radius = brick_m;
    let mut iter: u32 = 0;
    while iter < MAX_ITERS {
        let query_min = Vec3::new(
            brick_center.x - radius,
            brick_center.y - radius,
            brick_center.z - radius,
        );
        let query_max = Vec3::new(
            brick_center.x + radius,
            brick_center.y + radius,
            brick_center.z + radius,
        );
        let tris = gather_triangles(inputs, query_min, query_max);
        if !tris.is_empty() {
            let (unsigned, (face_normal, closest_point)) =
                closest_triangle(geom, &tris, brick_center);

            // Nearest-competitor safety check: if the best distance sits on
            // or past the query boundary, a triangle just outside the box
            // could be closer. Expand to `unsigned` (slight epsilon to clear
            // equality) and retry. Only fires on edge cases; most bricks
            // terminate on the first iteration with unsigned << radius.
            if unsigned >= radius && iter + 1 < MAX_ITERS {
                radius = unsigned * 1.0001;
                iter += 1;
                continue;
            }

            let is_surface = top_slot != BRICK_SLOT_EMPTY && top_slot != BRICK_SLOT_INTERIOR;
            let sign = if is_surface && iter == 0 {
                // SURFACE brick with adjacent geometry — winding test is
                // reliable here. `iter == 0` means radius never exceeded
                // brick_m, so the triangle is within one brick edge.
                let to_surface = brick_center - closest_point;
                if face_normal.dot(to_surface) < 0.0 {
                    -1.0
                } else {
                    1.0
                }
            } else if is_reachable_air {
                1.0
            } else {
                -1.0
            };
            return sign * unsigned;
        }
        radius *= 2.0;
        iter += 1;
    }
    // No geometry anywhere — degenerate case. Return a large magnitude so the
    // tracer takes big steps; sign follows flood classification.
    if is_reachable_air {
        SDF_FALLBACK_DISTANCE_M
    } else {
        -SDF_FALLBACK_DISTANCE_M
    }
}

/// Log SDF atlas statistics for the compiler output.
pub fn log_stats(section: &SdfAtlasSection) {
    let total_bricks = section.top_level.len();
    let surface_bricks = section.surface_brick_count();
    let atlas_mb = (section.atlas.len() * 2) as f64 / (1024.0 * 1024.0);
    log::info!(
        "SdfAtlas: grid {}x{}x{} = {} bricks, {} surface ({:.1} MB atlas), voxel={:.3}m",
        section.grid_dims[0],
        section.grid_dims[1],
        section.grid_dims[2],
        total_bricks,
        surface_bricks,
        atlas_mb,
        section.voxel_size_m,
    );
}

// ---------------------------------------------------------------------------
// Brick classification

/// Phase-1 per-brick result: either a surface brick (with packed SDF data
/// for every voxel) or a "no-surface" brick whose empty/interior status
/// phase 2 decides by flood-fill.
enum Phase1 {
    NotSurface,
    Surface(Vec<i16>),
}

fn classify_brick(
    inputs: &SdfBakeInputs<'_>,
    brick_origin: [f32; 3],
    voxel_size_m: f32,
    brick_size_voxels: u32,
    brick_vol: usize,
) -> Phase1 {
    let geom = &inputs.geometry.geometry;
    let brick_m = voxel_size_m * brick_size_voxels as f32;
    let bsv = brick_size_voxels as usize;

    // AABB for BVH traversal: expand by one voxel in each direction so
    // triangles just outside the brick contribute to boundary voxels.
    let query_min = Vec3::new(
        brick_origin[0] - voxel_size_m,
        brick_origin[1] - voxel_size_m,
        brick_origin[2] - voxel_size_m,
    );
    let query_max = Vec3::new(
        brick_origin[0] + brick_m + voxel_size_m,
        brick_origin[1] + brick_m + voxel_size_m,
        brick_origin[2] + brick_m + voxel_size_m,
    );

    // Gather all triangles overlapping the expanded brick AABB.
    let tris = gather_triangles(inputs, query_min, query_max);

    if tris.is_empty() {
        return Phase1::NotSurface;
    }

    // Compute signed distances for every voxel.
    let quant_scale = QUANT_UNITS_PER_METER_FACTOR / voxel_size_m;
    let mut data: Vec<i16> = Vec::with_capacity(brick_vol);

    for vz in 0..bsv {
        for vy in 0..bsv {
            for vx in 0..bsv {
                let world = Vec3::new(
                    brick_origin[0] + (vx as f32 + 0.5) * voxel_size_m,
                    brick_origin[1] + (vy as f32 + 0.5) * voxel_size_m,
                    brick_origin[2] + (vz as f32 + 0.5) * voxel_size_m,
                );

                let (unsigned_dist, nearest_normal) = closest_triangle(geom, &tris, world);

                // Determine sign: if the voxel center projects inside the
                // nearest face (dot product < 0), the voxel is inside solid.
                let to_surface = world - nearest_normal.1; // nearest_normal.1 = closest point
                let sign = if nearest_normal.0.dot(to_surface) < 0.0 {
                    -1.0f32
                } else {
                    1.0
                };

                let signed_m = sign * unsigned_dist;
                let quant = (signed_m * quant_scale).round();
                let clamped = quant.clamp(MIN_QUANT as f32, MAX_QUANT as f32) as i16;
                data.push(clamped);
            }
        }
    }

    Phase1::Surface(data)
}

// ---------------------------------------------------------------------------
// Reachable-air classification via player-start-seeded flood-fill.

/// BFS flood-fill over the brick grid starting from the brick that contains
/// `player_start_world`. Returns a per-brick boolean where `true` means the
/// brick is reachable open air (→ `BRICK_SLOT_EMPTY` in the atlas). Surface
/// bricks block the flood and remain `false`; no-surface bricks walled off
/// from the seed also remain `false` (→ `BRICK_SLOT_INTERIOR` — used for
/// bricks both inside solid geometry and in the exterior margin band).
///
/// Seeding from the player start is essential: seeding from the grid boundary
/// produces the opposite result for a sealed level — it marks playable rooms
/// INTERIOR because the wall shell blocks the flood from entering them, making
/// the sphere tracer treat every room as solid and fully occlude all shadows.
///
/// If the seed brick is out of bounds or is a surface brick (player start is
/// inside geometry), falls back to seeding from all boundary bricks and logs
/// a warning. Relies on watertight geometry; a map leak lets the flood exit
/// through holes and mark the margin band as reachable air, which is detected
/// and warned — see `context/plans/drafts/map-leak-diagnostics/`.
fn flood_fill_reachable_air(
    phase1: &[Phase1],
    dims: [u32; 3],
    world_min: [f32; 3],
    player_start_world: Vec3,
    voxel_size_m: f32,
    brick_size_voxels: u32,
) -> Vec<bool> {
    let total = phase1.len();
    let mut reachable = vec![false; total];

    let nx = dims[0];
    let ny = dims[1];
    let nz = dims[2];
    if nx == 0 || ny == 0 || nz == 0 {
        return reachable;
    }

    let brick_m = voxel_size_m * brick_size_voxels as f32;

    let mut queue: VecDeque<(u32, u32, u32)> = VecDeque::new();

    let enqueue = |x: u32, y: u32, z: u32,
                       reachable: &mut Vec<bool>,
                       queue: &mut VecDeque<(u32, u32, u32)>| {
        let linear = brick_to_linear(x, y, z, dims);
        if !reachable[linear] && matches!(phase1[linear], Phase1::NotSurface) {
            reachable[linear] = true;
            queue.push_back((x, y, z));
        }
    };

    // Compute the brick coordinate containing the player start.
    let seed_bx = ((player_start_world.x - world_min[0]) / brick_m).floor() as i64;
    let seed_by = ((player_start_world.y - world_min[1]) / brick_m).floor() as i64;
    let seed_bz = ((player_start_world.z - world_min[2]) / brick_m).floor() as i64;

    let seed_in_bounds = seed_bx >= 0
        && seed_by >= 0
        && seed_bz >= 0
        && (seed_bx as u32) < nx
        && (seed_by as u32) < ny
        && (seed_bz as u32) < nz;

    let seed_brick = if seed_in_bounds {
        let (sx, sy, sz) = (seed_bx as u32, seed_by as u32, seed_bz as u32);
        let linear = brick_to_linear(sx, sy, sz, dims);
        if matches!(phase1[linear], Phase1::NotSurface) {
            Some((sx, sy, sz))
        } else {
            log::warn!(
                "[SdfBake] player_start brick ({sx},{sy},{sz}) is a surface brick — \
                 falling back to boundary seeding (shadows may be incorrect)"
            );
            None
        }
    } else {
        log::warn!(
            "[SdfBake] player_start ({},{},{}) is outside the bake grid — \
             falling back to boundary seeding (shadows may be incorrect)",
            player_start_world.x,
            player_start_world.y,
            player_start_world.z,
        );
        None
    };

    match seed_brick {
        Some((sx, sy, sz)) => {
            enqueue(sx, sy, sz, &mut reachable, &mut queue);
        }
        None => {
            // Fallback: seed from every boundary brick (old behavior).
            for z in 0..nz {
                for y in 0..ny {
                    for x in 0..nx {
                        let on_boundary = x == 0
                            || y == 0
                            || z == 0
                            || x == nx - 1
                            || y == ny - 1
                            || z == nz - 1;
                        if on_boundary {
                            enqueue(x, y, z, &mut reachable, &mut queue);
                        }
                    }
                }
            }
        }
    }

    while let Some((x, y, z)) = queue.pop_front() {
        // 6-connected neighbors. Diagonal connectivity would let the flood
        // squeeze through corner-touching walls and misclassify enclosed
        // rooms as exterior.
        let neighbors: [(i32, i32, i32); 6] = [
            (-1, 0, 0),
            (1, 0, 0),
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
        ];
        for (dx, dy, dz) in neighbors {
            let nxi = x as i32 + dx;
            let nyi = y as i32 + dy;
            let nzi = z as i32 + dz;
            if nxi < 0 || nyi < 0 || nzi < 0 {
                continue;
            }
            let (nxu, nyu, nzu) = (nxi as u32, nyi as u32, nzi as u32);
            if nxu >= nx || nyu >= ny || nzu >= nz {
                continue;
            }
            enqueue(nxu, nyu, nzu, &mut reachable, &mut queue);
        }
    }

    // Warn if the flood reached any boundary brick — that indicates a map
    // leak where a hole in the world shell let the flood escape into the
    // margin band. Shadows will be absent (everything EMPTY) rather than
    // over-dark. Fix the leak in the source map.
    let leaked = (0..nz).any(|z| {
        (0..ny).any(|y| {
            (0..nx).any(|x| {
                let on_boundary = x == 0
                    || y == 0
                    || z == 0
                    || x == nx - 1
                    || y == ny - 1
                    || z == nz - 1;
                on_boundary && reachable[brick_to_linear(x, y, z, dims)]
            })
        })
    });
    if leaked {
        log::warn!(
            "[SdfBake] flood-fill reached the grid boundary — map geometry has a leak. \
             SDF empty/interior classification is unreliable. Fix the leak and rebake."
        );
    }

    reachable
}

fn brick_to_linear(x: u32, y: u32, z: u32, dims: [u32; 3]) -> usize {
    let nx = dims[0] as usize;
    let ny = dims[1] as usize;
    (z as usize) * nx * ny + (y as usize) * nx + (x as usize)
}

// ---------------------------------------------------------------------------
// Triangle gathering and distance computation

/// Pre-extracted triangle for SDF computation.
struct Triangle {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    /// Face geometric normal (normalized).
    normal: Vec3,
}

/// Collect all triangles from BVH primitives whose AABBs overlap
/// `[query_min, query_max]`.
///
/// Uses the BVH's AABB-vs-AABB traversal: `Aabb<f32, 3>` implements
/// `IntersectsAabb`, so `bvh.traverse(&query, primitives)` returns every
/// primitive whose AABB intersects the query box. This is the correct
/// spatial query — ray-based traversal would miss triangles whose AABBs
/// enclose the brick without straddling any axis ray through the center.
fn gather_triangles(
    inputs: &SdfBakeInputs<'_>,
    query_min: Vec3,
    query_max: Vec3,
) -> Vec<Triangle> {
    let geom = &inputs.geometry.geometry;

    let query_aabb = Aabb::with_bounds(
        Point3::new(query_min.x, query_min.y, query_min.z),
        Point3::new(query_max.x, query_max.y, query_max.z),
    );
    let candidates = inputs.bvh.traverse(&query_aabb, inputs.primitives);

    let mut tris = Vec::new();
    for prim in &candidates {
        // Emit each triangle.
        let start = prim.index_offset as usize;
        let end = start + prim.index_count as usize;
        let mut tri = start;
        while tri + 3 <= end {
            let i0 = geom.indices[tri] as usize;
            let i1 = geom.indices[tri + 1] as usize;
            let i2 = geom.indices[tri + 2] as usize;
            tri += 3;

            let a = Vec3::from(geom.vertices[i0].position);
            let b = Vec3::from(geom.vertices[i1].position);
            let c = Vec3::from(geom.vertices[i2].position);
            let normal = (b - a).cross(c - a).normalize_or_zero();
            tris.push(Triangle { a, b, c, normal });
        }
    }
    tris
}

/// Unsigned distance to the nearest triangle, plus the nearest point and face
/// normal for sign determination. Returns (distance, (face_normal, closest_point)).
fn closest_triangle(
    _geom: &postretro_level_format::geometry::GeometrySection,
    tris: &[Triangle],
    p: Vec3,
) -> (f32, (Vec3, Vec3)) {
    let mut best_dist = f32::INFINITY;
    let mut best_normal = Vec3::Y;
    let mut best_closest = p;

    for tri in tris {
        let (d, closest) = point_triangle_distance(p, tri.a, tri.b, tri.c);
        if d < best_dist {
            best_dist = d;
            best_normal = tri.normal;
            best_closest = closest;
        }
    }

    (best_dist, (best_normal, best_closest))
}

/// Squared distance from point `p` to the closest point on triangle `(a, b, c)`.
/// Returns `(distance, closest_point)`.
///
/// Uses the standard closest-point-on-triangle algorithm (barycentric regions).
fn point_triangle_distance(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> (f32, Vec3) {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;

    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return (p.distance(a), a);
    }

    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return (p.distance(b), b);
    }

    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        let closest = a + ab * v;
        return (p.distance(closest), closest);
    }

    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return (p.distance(c), c);
    }

    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        let closest = a + ac * w;
        return (p.distance(closest), closest);
    }

    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        let closest = b + (c - b) * w;
        return (p.distance(closest), closest);
    }

    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    let closest = a + ab * v + ac * w;
    (p.distance(closest), closest)
}

// ---------------------------------------------------------------------------
// Utilities

pub fn world_aabb_pub(
    geom: &postretro_level_format::geometry::GeometrySection,
) -> ([f32; 3], [f32; 3]) {
    world_aabb(geom)
}

fn world_aabb(
    geom: &postretro_level_format::geometry::GeometrySection,
) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for v in &geom.vertices {
        for i in 0..3 {
            min[i] = min[i].min(v.position[i]);
            max[i] = max[i].max(v.position[i]);
        }
    }
    (min, max)
}

fn linear_to_brick(linear: usize, grid_dims: [u32; 3]) -> (u32, u32, u32) {
    let nx = grid_dims[0] as usize;
    let ny = grid_dims[1] as usize;
    let z = linear / (nx * ny);
    let rem = linear - z * nx * ny;
    let y = rem / nx;
    let x = rem - y * nx;
    (x as u32, y as u32, z as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_triangle_distance_against_vertex() {
        // Point at vertex A.
        let (d, closest) = point_triangle_distance(
            Vec3::ZERO,
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
        );
        assert!(d < 1e-5, "distance to vertex A should be ~0, got {d}");
        assert!(closest.distance(Vec3::ZERO) < 1e-5);
    }

    #[test]
    fn point_triangle_distance_against_interior() {
        // Point directly above the centroid of the XY triangle.
        let centroid = Vec3::new(1.0 / 3.0, 1.0 / 3.0, 0.0);
        let p = centroid + Vec3::Z;
        let (d, _closest) = point_triangle_distance(p, Vec3::ZERO, Vec3::X, Vec3::Y);
        assert!((d - 1.0).abs() < 1e-5, "distance should be 1.0 above centroid, got {d}");
    }

    // Helper: call flood_fill_reachable_air with a brick-unit-sized grid
    // (voxel_size_m=1, brick_size_voxels=1 so one brick = 1 m), world_min at
    // origin, and an explicit player-start in world space.
    fn flood_with_seed(
        phase1: &[Phase1],
        dims: [u32; 3],
        seed_world: Vec3,
    ) -> Vec<bool> {
        flood_fill_reachable_air(phase1, dims, [0.0; 3], seed_world, 1.0, 1)
    }

    /// A 5×5×5 grid with a 3×3×3 hollow core at the center, walled by a
    /// single-brick-thick shell of surface bricks. Seeding from inside the
    /// core marks the core as reachable (EMPTY); the surrounding exterior and
    /// the shell itself are unreachable from that seed (INTERIOR / surface).
    #[test]
    fn flood_fill_seeds_from_playable_core() {
        let dims = [5u32, 5, 5];
        let total = (dims[0] * dims[1] * dims[2]) as usize;
        let mut phase1: Vec<Phase1> = (0..total).map(|_| Phase1::NotSurface).collect();

        // Shell: every brick at index 1..=3 on all three axes that sits on
        // the boundary of the 3×3×3 block (i.e. index == 1 or 3 on some axis).
        for z in 1..=3u32 {
            for y in 1..=3u32 {
                for x in 1..=3u32 {
                    let on_shell = x == 1 || x == 3 || y == 1 || y == 3 || z == 1 || z == 3;
                    if on_shell {
                        phase1[brick_to_linear(x, y, z, dims)] = Phase1::Surface(Vec::new());
                    }
                }
            }
        }

        // Seed from inside the room (core brick at (2,2,2) → world-space center ~2.5).
        let reachable = flood_with_seed(&phase1, dims, Vec3::new(2.5, 2.5, 2.5));

        // The core brick is reachable from the seed → EMPTY (playable air).
        assert!(
            reachable[brick_to_linear(2, 2, 2, dims)],
            "core brick should be reachable (EMPTY) when seed is inside"
        );
        // Boundary brick (0,0,0) is walled off from the core → INTERIOR.
        assert!(
            !reachable[brick_to_linear(0, 0, 0, dims)],
            "boundary brick should be unreachable (INTERIOR) from inside a sealed room"
        );
        // A shell (surface) brick is never reachable — flood can't enter it.
        assert!(!reachable[brick_to_linear(1, 2, 2, dims)]);
    }

    /// When the grid has no surface bricks at all and the seed is inside it,
    /// every brick should be reachable (EMPTY).
    #[test]
    fn flood_fill_no_shell_fills_whole_grid_from_center() {
        let dims = [3u32, 3, 3];
        let total = (dims[0] * dims[1] * dims[2]) as usize;
        let phase1: Vec<Phase1> = (0..total).map(|_| Phase1::NotSurface).collect();
        let reachable = flood_with_seed(&phase1, dims, Vec3::new(1.5, 1.5, 1.5));
        assert!(reachable.iter().all(|&b| b));
    }

    #[test]
    fn linear_to_brick_first_last() {
        let dims = [3u32, 4, 5];
        assert_eq!(linear_to_brick(0, dims), (0, 0, 0));
        let last = (3 * 4 * 5) - 1;
        assert_eq!(linear_to_brick(last, dims), (2, 3, 4));
    }
}
