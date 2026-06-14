// Navmesh bake: rasterize walkable surfaces into a span grid, then decompose
// into rectangular regions joined by portals.
// See: context/lib/build_pipeline.md §Navigation bake

use glam::Vec3;
use postretro_level_format::navmesh::{NAVMESH_VERSION, NavMeshSection, NavPortal, NavRegion};

use crate::geometry::GeometryResult;
use crate::map_data::NavParams;

/// Bump when the bake algorithm or its input layout changes. Folded into the
/// stage cache key so a stale on-disk entry from a prior version is a miss on
/// the first build after the bump and a hit on the second (matches the SDF and
/// SH stage pattern).
pub const NAVMESH_STAGE_VERSION: u32 = 2;

/// Tolerance on every `step_height` comparison. A floor delta exactly equal to
/// `step_height` must count as a climbable step (a one-step riser is reachable),
/// but f32 round-trips through interpolation and subtraction leave a delta of,
/// e.g., 0.5 a hair above the f32 `step_height`. This slack keeps the
/// region-merge and portal step-rules agreeing on the boundary case so a shared
/// edge at exactly `step_height` always yields a portal.
const STEP_EPS: f32 = 1.0e-4;

/// One merged vertical span in a grid column: a walkable (or non-walkable)
/// floor band plus the vertical clearance above it. Bake-internal only — never
/// serialized.
#[derive(Debug, Clone, Copy)]
struct Span {
    /// Floor height of the span (world meters): the top of the supporting
    /// surface, used as the standable height.
    floor_y: f32,
    /// Top of the merged solid band (world meters). Clearance is measured from
    /// here to the next span's floor.
    top_y: f32,
    /// Whether the supporting surface passed the slope filter.
    walkable: bool,
    /// Vertical gap to the next span above (open sky = +inf).
    clearance: f32,
}

/// A raw span fragment from a single triangle before per-column merging.
#[derive(Debug, Clone, Copy)]
struct Fragment {
    min_y: f32,
    max_y: f32,
    walkable: bool,
}

/// The walkable grid: per `(x, z)` column, the ascending-sorted floor heights of
/// every surviving walkable span (slope + clearance + erosion passed). A column
/// holds more than one entry only over stacked floors. Bake-internal; only the
/// grid header is serialized.
struct WalkGrid {
    origin: Vec3,
    cell_size: f32,
    dim_x: u32,
    dim_z: u32,
    /// Row-major (`z * dim_x + x`) list of walkable floor heights for the
    /// column, ascending. Empty where no walkable span exists.
    cells: Vec<Vec<f32>>,
}

impl WalkGrid {
    #[inline]
    fn idx(&self, x: u32, z: u32) -> usize {
        (z as usize) * (self.dim_x as usize) + (x as usize)
    }

    #[inline]
    fn heights_at(&self, x: u32, z: u32) -> &[f32] {
        &self.cells[self.idx(x, z)]
    }

    /// The neighbor column's walkable floor height closest to `reference` and
    /// within `step` of it. `None` when the neighbor is out of bounds or has no
    /// walkable span reachable by a climbable step — a true non-walkable
    /// boundary for the span at `reference`.
    fn neighbor_floor_within_step(
        &self,
        nx: i64,
        nz: i64,
        reference: f32,
        step: f32,
    ) -> Option<f32> {
        if nx < 0 || nz < 0 || nx >= self.dim_x as i64 || nz >= self.dim_z as i64 {
            return None;
        }
        let mut best: Option<f32> = None;
        for &h in self.heights_at(nx as u32, nz as u32) {
            let delta = (h - reference).abs();
            if delta <= step + STEP_EPS {
                match best {
                    Some(b) if (b - reference).abs() <= delta => {}
                    _ => best = Some(h),
                }
            }
        }
        best
    }
}

/// Bake a navmesh from the extracted geometry's triangles plus the resolved
/// nav parameters. Returns `None` when no walkable region survives (then the
/// caller emits no section and the build still succeeds).
///
/// Sequential and allocation-stable: no `HashMap`-iteration-ordered output and
/// no parallel reductions, so identical inputs produce byte-identical section
/// bytes (the stage cache keys on those bytes).
pub fn bake_navmesh(geo: &GeometryResult, params: &NavParams) -> Option<NavMeshSection> {
    let triangles = collect_triangles(geo);
    if triangles.is_empty() {
        return None;
    }

    let grid = rasterize(&triangles, params)?;
    let regions = decompose_regions(&grid);
    if regions.is_empty() {
        return None;
    }
    let portals = extract_portals(&grid, &regions, params);

    log::info!(
        "[Compiler] NavMesh: {} regions, {} portals ({}x{} grid @ {} m)",
        regions.len(),
        portals.len(),
        grid.dim_x,
        grid.dim_z,
        grid.cell_size,
    );

    Some(NavMeshSection {
        version: NAVMESH_VERSION,
        origin: grid.origin.to_array(),
        cell_size: grid.cell_size,
        dim_x: grid.dim_x,
        dim_z: grid.dim_z,
        agent_radius: params.agent_radius,
        agent_height: params.agent_height,
        step_height: params.step_height,
        max_slope_deg: params.max_slope_deg,
        regions,
        portals,
    })
}

/// One source triangle in world space.
struct Triangle {
    a: Vec3,
    b: Vec3,
    c: Vec3,
}

/// Pull the geometry section's triangles into world-space triangles. The
/// geometry is already filtered to empty, non-exterior leaf faces at
/// extraction, so no further leaf filtering is needed.
fn collect_triangles(geo: &GeometryResult) -> Vec<Triangle> {
    let verts = &geo.geometry.vertices;
    let indices = &geo.geometry.indices;
    let mut triangles = Vec::with_capacity(indices.len() / 3);
    for tri in indices.chunks_exact(3) {
        let a = Vec3::from(verts[tri[0] as usize].position);
        let b = Vec3::from(verts[tri[1] as usize].position);
        let c = Vec3::from(verts[tri[2] as usize].position);
        triangles.push(Triangle { a, b, c });
    }
    triangles
}

/// Rasterize triangles into column spans, merge per column, then erode walkable
/// cells against true non-walkable boundaries. Returns the surviving walkable
/// grid, or `None` if the geometry has no finite XZ extent.
fn rasterize(triangles: &[Triangle], params: &NavParams) -> Option<WalkGrid> {
    let cell = params.cell_size;
    let cos_max_slope = params.max_slope_deg.to_radians().cos();

    // World XZ extent across all triangle vertices defines the grid footprint.
    let mut min_x = f32::INFINITY;
    let mut min_z = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_z = f32::NEG_INFINITY;
    for t in triangles {
        for v in [t.a, t.b, t.c] {
            min_x = min_x.min(v.x);
            min_z = min_z.min(v.z);
            min_y = min_y.min(v.y);
            max_x = max_x.max(v.x);
            max_z = max_z.max(v.z);
        }
    }
    if !(min_x.is_finite() && min_z.is_finite() && max_x.is_finite() && max_z.is_finite()) {
        return None;
    }

    // Grid origin snaps to a cell-size lattice anchored at the world min corner
    // so decoded region coords map back to world via `origin + cell_size * i`.
    let origin = Vec3::new(min_x, min_y, min_z);
    let dim_x = (((max_x - min_x) / cell).ceil() as i64).max(1) as u32;
    let dim_z = (((max_z - min_z) / cell).ceil() as i64).max(1) as u32;
    let cells = (dim_x as usize) * (dim_z as usize);

    // Per-column fragment lists, indexed row-major (z * dim_x + x).
    let mut fragments: Vec<Vec<Fragment>> = vec![Vec::new(); cells];

    for t in triangles {
        // Upward normal in normal-Y space; degenerate triangles (zero-area)
        // produce a zero normal and are skipped (no walkable contribution).
        let normal = (t.b - t.a).cross(t.c - t.a);
        let len = normal.length();
        if len <= f32::EPSILON {
            continue;
        }
        let normal_y = normal.y / len;
        // A downward-facing surface (ceiling) is never walkable; flip is not
        // applied — only upward-facing surfaces support the agent.
        let walkable = normal_y >= cos_max_slope;

        // Clip the triangle's XZ footprint to the overlapped columns. The
        // per-column span uses the triangle's interpolated Y at the column
        // center, clamped to the triangle's own Y range.
        let tmin_x = t.a.x.min(t.b.x).min(t.c.x);
        let tmax_x = t.a.x.max(t.b.x).max(t.c.x);
        let tmin_z = t.a.z.min(t.b.z).min(t.c.z);
        let tmax_z = t.a.z.max(t.b.z).max(t.c.z);

        let x0 = column_index_floor(tmin_x - origin.x, cell, dim_x);
        let x1 = column_index_ceil(tmax_x - origin.x, cell, dim_x);
        let z0 = column_index_floor(tmin_z - origin.z, cell, dim_z);
        let z1 = column_index_ceil(tmax_z - origin.z, cell, dim_z);

        for z in z0..z1 {
            for x in x0..x1 {
                let cx = origin.x + (x as f32 + 0.5) * cell;
                let cz = origin.z + (z as f32 + 0.5) * cell;
                let Some(y) = sample_triangle_y(t, cx, cz) else {
                    continue;
                };
                let idx = (z as usize) * (dim_x as usize) + (x as usize);
                fragments[idx].push(Fragment {
                    min_y: y,
                    max_y: y,
                    walkable,
                });
            }
        }
    }

    // Merge fragments per column bottom-up into spans, then keep every walkable
    // span whose clearance admits the agent (ascending by floor height). A
    // column with two stacked floors keeps both, so the decomposition can build
    // a region on each level.
    let mut grid_cells: Vec<Vec<f32>> = Vec::with_capacity(cells);
    for frags in &mut fragments {
        let spans = merge_column(frags, params.agent_height);
        let heights: Vec<f32> = spans
            .iter()
            .filter(|s| s.walkable && s.clearance >= params.agent_height)
            .map(|s| s.floor_y)
            .collect();
        grid_cells.push(heights);
    }

    let grid = WalkGrid {
        origin,
        cell_size: cell,
        dim_x,
        dim_z,
        cells: grid_cells,
    };
    Some(erode(grid, params))
}

/// Floor of the column index for a coordinate offset from the grid origin,
/// clamped to `[0, dim)`.
fn column_index_floor(offset: f32, cell: f32, dim: u32) -> u32 {
    if offset <= 0.0 {
        return 0;
    }
    ((offset / cell).floor() as i64).clamp(0, dim as i64 - 1) as u32
}

/// One past the last column index a coordinate offset touches, clamped to
/// `[1, dim]` (an exclusive upper bound for iteration).
fn column_index_ceil(offset: f32, cell: f32, dim: u32) -> u32 {
    if offset <= 0.0 {
        return 1;
    }
    (((offset / cell).floor() as i64) + 1).clamp(1, dim as i64) as u32
}

/// Interpolate the triangle's Y at world `(x, z)` via barycentric coordinates
/// in the XZ plane. Returns `None` when the point lies outside the triangle or
/// the triangle is XZ-degenerate (vertical wall — contributes no floor).
fn sample_triangle_y(t: &Triangle, x: f32, z: f32) -> Option<f32> {
    let (ax, az) = (t.a.x, t.a.z);
    let (bx, bz) = (t.b.x, t.b.z);
    let (cx, cz) = (t.c.x, t.c.z);

    let det = (bz - cz) * (ax - cx) + (cx - bx) * (az - cz);
    if det.abs() <= f32::EPSILON {
        return None;
    }
    let l1 = ((bz - cz) * (x - cx) + (cx - bx) * (z - cz)) / det;
    let l2 = ((cz - az) * (x - cx) + (ax - cx) * (z - cz)) / det;
    let l3 = 1.0 - l1 - l2;
    // Small epsilon tolerance so a column center exactly on a shared edge is
    // attributed to at least one triangle.
    const BARY_EPS: f32 = 1.0e-4;
    if l1 < -BARY_EPS || l2 < -BARY_EPS || l3 < -BARY_EPS {
        return None;
    }
    Some(l1 * t.a.y + l2 * t.b.y + l3 * t.c.y)
}

/// Merge a column's raw fragments into bottom-up spans and compute clearance.
/// Fragments whose floors are within `merge_eps` collapse into one span; a span
/// is walkable iff any contributing fragment is walkable (a walkable surface is
/// not masked by a coincident non-walkable one). A merged span's floor tracks
/// the lowest WALKABLE surface — the height an agent stands on. Non-walkable
/// fragments (e.g. a thin deck's underside) do not drag the floor down: if they
/// did, the region would sink a slab-thickness below the walk surface and the
/// step delta to flush ground would exceed `step_height`, leaving it a
/// disconnected island. Clearance is the gap from a span's top to the next
/// span's floor; the topmost span has open sky (+inf).
fn merge_column(fragments: &mut [Fragment], agent_height: f32) -> Vec<Span> {
    if fragments.is_empty() {
        return Vec::new();
    }
    // Stable sort by floor so identical inputs merge identically.
    fragments.sort_by(|a, b| a.min_y.total_cmp(&b.min_y));

    // Merge threshold: fragments closer than a small fraction of agent height
    // are the same physical surface sampled by adjacent triangles.
    let merge_eps = (agent_height * 0.25).max(1.0e-3);

    let mut spans: Vec<Span> = Vec::new();
    for frag in fragments.iter() {
        if let Some(last) = spans.last_mut() {
            if frag.min_y - last.top_y <= merge_eps {
                last.top_y = last.top_y.max(frag.max_y);
                if frag.walkable {
                    // The standable floor is the lowest walkable fragment; a
                    // walkable fragment merging onto a non-walkable span (a thin
                    // deck's top over its own underside) lifts the floor to it.
                    last.floor_y = if last.walkable {
                        last.floor_y.min(frag.min_y)
                    } else {
                        frag.min_y
                    };
                    last.walkable = true;
                }
                continue;
            }
        }
        spans.push(Span {
            floor_y: frag.min_y,
            top_y: frag.max_y,
            walkable: frag.walkable,
            clearance: f32::INFINITY,
        });
    }

    for i in 0..spans.len() {
        spans[i].clearance = if i + 1 < spans.len() {
            spans[i + 1].floor_y - spans[i].top_y
        } else {
            f32::INFINITY
        };
    }
    spans
}

/// One walkable cell instance: a column position plus the specific span height
/// that survived filtering. Stacked floors over a column produce distinct
/// `NavCell`s (one per height). The decomposition and portal passes operate on
/// these so a region never spans two floor levels.
#[derive(Debug, Clone, Copy)]
struct NavCell {
    x: u32,
    z: u32,
    floor_y: f32,
}

/// Flatten the grid's per-column span heights into one `NavCell` per walkable
/// span, ascending by `(z, x, floor_y)` for a deterministic scan order.
fn flatten_cells(grid: &WalkGrid) -> Vec<NavCell> {
    let mut cells = Vec::new();
    for z in 0..grid.dim_z {
        for x in 0..grid.dim_x {
            for &h in grid.heights_at(x, z) {
                cells.push(NavCell { x, z, floor_y: h });
            }
        }
    }
    cells
}

/// Erode walkable spans by `agent_radius` against TRUE non-walkable boundaries
/// only. A span is a boundary when a 4-neighbor column has no walkable span
/// within `step_height` of it (a wall, an unclimbable drop, or the grid edge).
/// A climbable-step neighbor (a walkable span within `step_height`) is NOT a
/// boundary and does not erode its neighbors — this keeps doorway-width paths
/// beside steps connected. Removes every walkable span within `radius_cells`
/// (Chebyshev, matching floor heights within `step_height`) of a boundary span.
fn erode(grid: WalkGrid, params: &NavParams) -> WalkGrid {
    let radius_cells = (params.agent_radius / grid.cell_size).ceil() as i64;
    if radius_cells <= 0 {
        return grid;
    }
    let step = params.step_height;

    // Boundary spans, recorded as (x, z, floor_y).
    let mut boundary: Vec<NavCell> = Vec::new();
    for cell in flatten_cells(&grid) {
        if is_boundary_span(&grid, &cell, step) {
            boundary.push(cell);
        }
    }

    // Build the surviving span set: drop any span within `radius_cells`
    // (Chebyshev) of a boundary span at a compatible height.
    let mut eroded: Vec<Vec<f32>> = vec![Vec::new(); grid.cells.len()];
    let mut eroded_count = 0usize;
    for cell in flatten_cells(&grid) {
        if near_boundary(&boundary, &cell, radius_cells, step) {
            eroded_count += 1;
            continue;
        }
        let idx = grid.idx(cell.x, cell.z);
        eroded[idx].push(cell.floor_y);
    }
    // Heights stay ascending (flatten visited them ascending per column).

    log::info!("[Compiler] NavMesh: eroded {eroded_count} cells against non-walkable boundaries");

    WalkGrid {
        cells: eroded,
        ..grid
    }
}

/// Whether `cell`'s span borders a true non-walkable boundary: any 4-neighbor
/// column lacks a walkable span within `step` of this span's floor.
fn is_boundary_span(grid: &WalkGrid, cell: &NavCell, step: f32) -> bool {
    for (dx, dz) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
        let nx = cell.x as i64 + dx;
        let nz = cell.z as i64 + dz;
        if grid
            .neighbor_floor_within_step(nx, nz, cell.floor_y, step)
            .is_none()
        {
            return true;
        }
    }
    false
}

/// Whether `cell` lies within `agent_radius` of a true non-walkable boundary.
/// A boundary cell is itself the wall-adjacent ring and is always eroded
/// (distance 0); cells further in erode only while strictly inside the radius
/// reach. With `radius_cells == 1` (radius == cell_size) this removes exactly
/// the one outermost ring — the boundary cells themselves — and no interior
/// ring, matching the spec's "within `agent_radius` of a boundary" measured
/// from the boundary cell outward.
fn near_boundary(boundary: &[NavCell], cell: &NavCell, radius_cells: i64, step: f32) -> bool {
    boundary.iter().any(|b| {
        let dx = (b.x as i64 - cell.x as i64).abs();
        let dz = (b.z as i64 - cell.z as i64).abs();
        dx < radius_cells && dz < radius_cells && (b.floor_y - cell.floor_y).abs() <= step
    })
}

/// Floor-delta tolerance that defines "same level" for region growth. A region
/// must carry a near-uniform floor (a rectangle is one walkable level), so the
/// greedy merge only absorbs cells within this tight band — NOT within
/// `step_height`. A `step_height` riser between two flat floors therefore splits
/// into two distinct regions joined by a portal (the portal step-rule, which
/// uses `step_height`, then bridges them). Sized like the column merge_eps floor
/// so f32 jitter on a flat surface never spuriously splits a region.
const LEVEL_EPS: f32 = 1.0e-3;

/// Greedy rectangular decomposition of the surviving walkable spans into
/// disjoint axis-aligned rectangles, one per floor level. Deterministic scan
/// order: ascending z then x; at each unclaimed span grow the x-run, then grow
/// z. A span merges into the growing rectangle only while every interior
/// adjacent-cell floor delta stays within `LEVEL_EPS` (a near-uniform level), so
/// a region carries a single floor level and a step riser splits regions.
/// Returns regions sorted by the section's invariant order.
fn decompose_regions(grid: &WalkGrid) -> Vec<NavRegion> {
    let dim_x = grid.dim_x;
    let dim_z = grid.dim_z;
    // Region growth uses the tight same-level band, not `step_height`: a region
    // is one floor level, so a climbable step becomes a region boundary (and a
    // portal), not an in-region delta.
    let step = LEVEL_EPS;
    // Per-column claimed span heights (claiming a span removes it from
    // consideration). A column may have spans on multiple levels.
    let mut claimed: Vec<Vec<f32>> = vec![Vec::new(); grid.cells.len()];
    let mut regions: Vec<NavRegion> = Vec::new();

    for seed in flatten_cells(grid) {
        if is_claimed(&claimed, grid, seed.x, seed.z, seed.floor_y) {
            continue;
        }

        // Grow the x-run from the seed: include the next column only while it
        // carries an unclaimed span within step of the run's previous height.
        let mut prev_h = seed.floor_y;
        let mut x1 = seed.x + 1;
        while x1 < dim_x {
            match unclaimed_span_within_step(grid, &claimed, x1, seed.z, prev_h, step) {
                Some(h) => {
                    prev_h = h;
                    x1 += 1;
                }
                None => break,
            }
        }

        // The seed-row span heights, recomputed by the same left-to-right
        // chaining the x-grow used (each column references its left neighbor's
        // height) so a climbing x-run carries the right reference per column.
        let mut row_heights: Vec<f32> = Vec::with_capacity((x1 - seed.x) as usize);
        let mut chain_h = seed.floor_y;
        for x in seed.x..x1 {
            chain_h = span_height_at(grid, x, seed.z, chain_h, step).expect("seed-row span exists");
            row_heights.push(chain_h);
        }

        // Grow z: accept the next row only if every column in [seed.x, x1)
        // carries an unclaimed span within step of the cell directly below.
        let mut z1 = seed.z + 1;
        while z1 < dim_z {
            let Some(next_heights) =
                row_extends(grid, &claimed, seed.x, x1, z1, &row_heights, step)
            else {
                break;
            };
            row_heights = next_heights;
            z1 += 1;
        }

        // Claim every span in the rectangle and accumulate floor extent. Each
        // column references the seed-row height first, then chains downward in z
        // — the same reference the grow phase accepted the rectangle under.
        let mut fmin = f32::INFINITY;
        let mut fmax = f32::NEG_INFINITY;
        let mut ref_below = row_heights.clone();
        for z in seed.z..z1 {
            for (col, x) in (seed.x..x1).enumerate() {
                let h =
                    span_height_at(grid, x, z, ref_below[col], step).expect("claimed span exists");
                ref_below[col] = h;
                claimed[grid.idx(x, z)].push(h);
                fmin = fmin.min(h);
                fmax = fmax.max(h);
            }
        }

        regions.push(NavRegion {
            x0: seed.x,
            z0: seed.z,
            x1,
            z1,
            floor_y_min: fmin,
            floor_y_max: fmax,
        });
    }

    // Section invariant: regions sorted ascending by (z0, x0, x1, z1,
    // floor_y_min) and unique.
    regions.sort_by(|a, b| {
        a.z0.cmp(&b.z0)
            .then(a.x0.cmp(&b.x0))
            .then(a.x1.cmp(&b.x1))
            .then(a.z1.cmp(&b.z1))
            .then(a.floor_y_min.total_cmp(&b.floor_y_min))
    });
    regions.dedup();
    regions
}

/// The walkable span height at `(x, z)` closest to `reference` and within
/// `step` of it, or `None` when the column has no such span.
fn span_height_at(grid: &WalkGrid, x: u32, z: u32, reference: f32, step: f32) -> Option<f32> {
    grid.neighbor_floor_within_step(x as i64, z as i64, reference, step)
}

/// Whether the span at `(x, z, height)` has already been claimed by a region
/// (matched within a tight epsilon so float jitter never double-claims).
fn is_claimed(claimed: &[Vec<f32>], grid: &WalkGrid, x: u32, z: u32, height: f32) -> bool {
    claimed[grid.idx(x, z)]
        .iter()
        .any(|&c| (c - height).abs() <= 1.0e-4)
}

/// The unclaimed span at `(x, z)` within `step` of `reference`, or `None`.
fn unclaimed_span_within_step(
    grid: &WalkGrid,
    claimed: &[Vec<f32>],
    x: u32,
    z: u32,
    reference: f32,
    step: f32,
) -> Option<f32> {
    let h = span_height_at(grid, x, z, reference, step)?;
    if is_claimed(claimed, grid, x, z, h) {
        return None;
    }
    Some(h)
}

/// Whether row `z` over columns `[x0, x1)` can extend the growing rectangle:
/// every column carries an unclaimed span within `step` of the cell directly
/// below it (`below_heights`, parallel to the column range). Returns the row's
/// span heights on success so the next row references them.
fn row_extends(
    grid: &WalkGrid,
    claimed: &[Vec<f32>],
    x0: u32,
    x1: u32,
    z: u32,
    below_heights: &[f32],
    step: f32,
) -> Option<Vec<f32>> {
    let mut heights = Vec::with_capacity((x1 - x0) as usize);
    for (col, x) in (x0..x1).enumerate() {
        let h = unclaimed_span_within_step(grid, claimed, x, z, below_heights[col], step)?;
        heights.push(h);
    }
    Some(heights)
}

/// Extract portals over shared region edges. A portal exists where two regions
/// share an edge run whose floor delta along the run stays within `step_height`;
/// its world-space segment spans the shared run, with Y the minimum of the two
/// regions' floor heights along it. A shared edge exceeding `step_height` yields
/// no portal (a ledge). Returns portals sorted by the section's invariant order.
fn extract_portals(grid: &WalkGrid, regions: &[NavRegion], params: &NavParams) -> Vec<NavPortal> {
    let step = params.step_height;
    let mut portals: Vec<NavPortal> = Vec::new();

    // O(n^2) over regions: deterministic, allocation-stable, and region counts
    // are small at v1 scale (logged for fragmentation feedback).
    for a in 0..regions.len() {
        for b in (a + 1)..regions.len() {
            let ra = &regions[a];
            let rb = &regions[b];

            // Vertical shared edge: ra's right edge meets rb's left edge (or
            // vice versa) at a common X, overlapping in Z.
            if let Some(portal) = shared_vertical_edge(grid, ra, rb, a as u32, b as u32, step) {
                portals.push(portal);
                continue;
            }
            // Horizontal shared edge: shared Z line, overlapping in X.
            if let Some(portal) = shared_horizontal_edge(grid, ra, rb, a as u32, b as u32, step) {
                portals.push(portal);
            }
        }
    }

    // Section invariant: sort ascending by (region_a, region_b) then
    // lexicographically by left.x, left.y, left.z under f32 total order.
    portals.sort_by(|p, q| {
        p.region_a
            .cmp(&q.region_a)
            .then(p.region_b.cmp(&q.region_b))
            .then(p.left[0].total_cmp(&q.left[0]))
            .then(p.left[1].total_cmp(&q.left[1]))
            .then(p.left[2].total_cmp(&q.left[2]))
    });
    portals
}

/// A portal across a shared vertical cell edge (constant X) between two regions,
/// or `None` when they do not abut vertically, do not overlap in Z, or the
/// floor delta along the shared run exceeds `step`. Region floor heights gate
/// the level match: the two regions must be on the same floor level (their
/// shared-run spans within `step`), so stacked floors never portal across.
fn shared_vertical_edge(
    grid: &WalkGrid,
    ra: &NavRegion,
    rb: &NavRegion,
    region_a: u32,
    region_b: u32,
    step: f32,
) -> Option<NavPortal> {
    let (edge_x, left_region, right_region) = if ra.x1 == rb.x0 {
        (ra.x1, ra, rb)
    } else if rb.x1 == ra.x0 {
        (rb.x1, rb, ra)
    } else {
        return None;
    };

    let z_lo = ra.z0.max(rb.z0);
    let z_hi = ra.z1.min(rb.z1);
    if z_lo >= z_hi {
        return None;
    }

    // The shared run is the cells on each side of `edge_x` for z in [z_lo,
    // z_hi). Match the left cell against `left_region`'s level and the right
    // against `right_region`'s level, then require the two within `step`.
    let mut seg_y = f32::INFINITY;
    let mut matched = false;
    for z in z_lo..z_hi {
        let left = span_height_at(grid, edge_x - 1, z, region_ref_height(left_region), step)?;
        let right = span_height_at(grid, edge_x, z, region_ref_height(right_region), step)?;
        if (left - right).abs() > step + STEP_EPS {
            return None;
        }
        seg_y = seg_y.min(left).min(right);
        matched = true;
    }
    if !matched {
        return None;
    }

    let world_x = grid.origin.x + edge_x as f32 * grid.cell_size;
    let world_z0 = grid.origin.z + z_lo as f32 * grid.cell_size;
    let world_z1 = grid.origin.z + z_hi as f32 * grid.cell_size;

    Some(NavPortal {
        region_a,
        region_b,
        left: [world_x, seg_y, world_z0],
        right: [world_x, seg_y, world_z1],
    })
}

/// A portal across a shared horizontal cell edge (constant Z) between two
/// regions, or `None` when they do not abut horizontally, do not overlap in X,
/// or the floor delta along the shared run exceeds `step`.
fn shared_horizontal_edge(
    grid: &WalkGrid,
    ra: &NavRegion,
    rb: &NavRegion,
    region_a: u32,
    region_b: u32,
    step: f32,
) -> Option<NavPortal> {
    let (edge_z, lower_region, upper_region) = if ra.z1 == rb.z0 {
        (ra.z1, ra, rb)
    } else if rb.z1 == ra.z0 {
        (rb.z1, rb, ra)
    } else {
        return None;
    };

    let x_lo = ra.x0.max(rb.x0);
    let x_hi = ra.x1.min(rb.x1);
    if x_lo >= x_hi {
        return None;
    }

    let mut seg_y = f32::INFINITY;
    let mut matched = false;
    for x in x_lo..x_hi {
        let below = span_height_at(grid, x, edge_z - 1, region_ref_height(lower_region), step)?;
        let above = span_height_at(grid, x, edge_z, region_ref_height(upper_region), step)?;
        if (below - above).abs() > step + STEP_EPS {
            return None;
        }
        seg_y = seg_y.min(below).min(above);
        matched = true;
    }
    if !matched {
        return None;
    }

    let world_z = grid.origin.z + edge_z as f32 * grid.cell_size;
    let world_x0 = grid.origin.x + x_lo as f32 * grid.cell_size;
    let world_x1 = grid.origin.x + x_hi as f32 * grid.cell_size;

    Some(NavPortal {
        region_a,
        region_b,
        left: [world_x0, seg_y, world_z],
        right: [world_x1, seg_y, world_z],
    })
}

/// A representative floor height for a region's level, used to disambiguate
/// which stacked span an edge cell belongs to.
fn region_ref_height(region: &NavRegion) -> f32 {
    0.5 * (region.floor_y_min + region.floor_y_max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    use crate::geometry::FaceIndexRange;

    /// Build a `GeometryResult` from a flat list of world-space triangles.
    /// Every triangle gets a placeholder face/leaf — the bake reads only the
    /// vertex positions and the index triples.
    fn geo_from_triangles(triangles: &[[[f32; 3]; 3]]) -> GeometryResult {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for tri in triangles {
            let base = vertices.len() as u32;
            for &pos in tri {
                vertices.push(Vertex::new(
                    pos,
                    [0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [1.0, 0.0, 0.0],
                    true,
                    [0.0, 0.0],
                ));
            }
            indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
        GeometryResult {
            geometry: GeometrySection {
                vertices,
                indices,
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

    /// Two triangles forming an axis-aligned floor quad at height `y` over
    /// `[x0, x1] x [z0, z1]`, wound so `(b-a) x (c-a)` points up (+Y) — the bake
    /// only treats upward-facing surfaces as walkable.
    fn floor_quad(x0: f32, z0: f32, x1: f32, z1: f32, y: f32) -> [[[f32; 3]; 3]; 2] {
        [
            [[x0, y, z0], [x1, y, z1], [x1, y, z0]],
            [[x0, y, z0], [x0, y, z1], [x1, y, z1]],
        ]
    }

    /// A downward-facing (-Y normal) quad at height `y` — a ceiling. Reverses
    /// `floor_quad`'s winding so the bake never treats it as walkable.
    fn ceiling_quad(x0: f32, z0: f32, x1: f32, z1: f32, y: f32) -> [[[f32; 3]; 3]; 2] {
        [
            [[x0, y, z0], [x1, y, z0], [x1, y, z1]],
            [[x0, y, z0], [x1, y, z1], [x0, y, z1]],
        ]
    }

    /// Test params: tiny radius (no erosion) unless a test overrides it, so the
    /// rasterization/region logic is exercised in isolation. `step_height` is
    /// fixed at 0.3 (not the engine default of 0.5) — chosen to suit the
    /// thin-deck and step fixtures in this suite.
    fn no_erode_params() -> NavParams {
        NavParams {
            agent_radius: 0.0,
            agent_height: 1.8,
            step_height: 0.3,
            max_slope_deg: 45.0,
            cell_size: 0.25,
        }
    }

    #[test]
    fn flat_floor_produces_single_region_no_portals() {
        // 2 m x 2 m floor at y = 0; with zero radius and open sky, every cell
        // is walkable and merges into one rectangle.
        let tris = floor_quad(0.0, 0.0, 2.0, 2.0, 0.0);
        let geo = geo_from_triangles(&tris);
        let section = bake_navmesh(&geo, &no_erode_params()).expect("flat floor must bake");

        assert_eq!(section.regions.len(), 1);
        assert!(section.portals.is_empty());
        let r = section.regions[0];
        assert_eq!(r.x0, 0);
        assert_eq!(r.z0, 0);
        assert_eq!(r.x1, 8); // 2.0 / 0.25
        assert_eq!(r.z1, 8);
        assert!((r.floor_y_min - 0.0).abs() < 1.0e-4);
        assert!((r.floor_y_max - 0.0).abs() < 1.0e-4);
    }

    #[test]
    fn thin_deck_records_floor_at_walk_surface_not_underside() {
        // Regression: a bridge deck thinner than merge_eps (= agent_height * 0.25
        // = 0.45 m) has its walkable top and non-walkable underside merged into
        // one span. The span's floor must be the TOP (1.0), the surface an agent
        // stands on — not the underside (0.7). The old bug kept the underside,
        // sinking the region a slab-thickness below the deck (it then read as a
        // disconnected island: the step delta to flush ground exceeds step_height).
        let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
        tris.extend_from_slice(&floor_quad(0.0, 0.0, 2.0, 2.0, 1.0)); // deck top (walkable)
        tris.extend_from_slice(&ceiling_quad(0.0, 0.0, 2.0, 2.0, 0.7)); // underside, 0.3 m thick
        let geo = geo_from_triangles(&tris);
        let section = bake_navmesh(&geo, &no_erode_params()).expect("thin deck must bake");

        assert_eq!(section.regions.len(), 1);
        let r = section.regions[0];
        assert!(
            (r.floor_y_min - 1.0).abs() < 1.0e-4,
            "deck floor must sit on the walk surface (1.0), got {}",
            r.floor_y_min
        );
        assert!((r.floor_y_max - 1.0).abs() < 1.0e-4);
    }

    #[test]
    fn no_floor_geometry_emits_no_section() {
        // A single vertical wall quad (normal in XZ plane): no walkable surface.
        let tris = [
            [[0.0, 0.0, 0.0], [0.0, 2.0, 0.0], [2.0, 2.0, 0.0]],
            [[0.0, 0.0, 0.0], [2.0, 2.0, 0.0], [2.0, 0.0, 0.0]],
        ];
        let geo = geo_from_triangles(&tris);
        assert!(bake_navmesh(&geo, &no_erode_params()).is_none());
    }

    #[test]
    fn empty_geometry_emits_no_section() {
        let geo = geo_from_triangles(&[]);
        assert!(bake_navmesh(&geo, &no_erode_params()).is_none());
    }

    #[test]
    fn steep_slope_is_not_walkable() {
        // A ~60-degree ramp exceeds the 45-degree slope filter, so no cell is
        // walkable and no section is emitted.
        // Ramp rises 2 m over 1 m of XZ run: slope ~63 degrees.
        let tris = [
            [[0.0, 0.0, 0.0], [1.0, 2.0, 0.0], [1.0, 2.0, 2.0]],
            [[0.0, 0.0, 0.0], [1.0, 2.0, 2.0], [0.0, 0.0, 2.0]],
        ];
        let geo = geo_from_triangles(&tris);
        assert!(
            bake_navmesh(&geo, &no_erode_params()).is_none(),
            "a slope steeper than max_slope_deg must produce no walkable region"
        );
    }

    #[test]
    fn low_clearance_ceiling_blocks_walkability() {
        // Floor at y=0 with a ceiling 1.0 m above it (< agent_height 1.8) makes
        // the floor span non-walkable for clearance.
        let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
        tris.extend_from_slice(&floor_quad(0.0, 0.0, 2.0, 2.0, 0.0));
        // Downward-facing ceiling 1.0 m above the floor: non-walkable itself
        // (normal points -Y) and it bounds the floor span's clearance below
        // agent_height, so neither surface is walkable.
        tris.extend_from_slice(&ceiling_quad(0.0, 0.0, 2.0, 2.0, 1.0));
        let geo = geo_from_triangles(&tris);
        assert!(
            bake_navmesh(&geo, &no_erode_params()).is_none(),
            "a ceiling closer than agent_height must remove the floor's walkability"
        );
    }

    #[test]
    fn agent_radius_erodes_floor_edges() {
        // 2 m floor with agent_radius 0.25 (= 1 cell). The outer ring of cells
        // borders the grid edge (a non-walkable boundary) and erodes, leaving a
        // strictly smaller interior region.
        let tris = floor_quad(0.0, 0.0, 2.0, 2.0, 0.0);
        let geo = geo_from_triangles(&tris);
        let params = NavParams {
            agent_radius: 0.25,
            ..no_erode_params()
        };
        let section = bake_navmesh(&geo, &params).expect("interior must survive erosion");
        let r = section.regions[0];
        // 8x8 grid, one-cell erosion ring → interior 6x6 at [1,7).
        assert_eq!(r.x0, 1);
        assert_eq!(r.z0, 1);
        assert_eq!(r.x1, 7);
        assert_eq!(r.z1, 7);
    }

    #[test]
    fn doorway_beside_a_step_survives() {
        // Two coplanar floor halves split by a step_height riser, with a
        // doorway-width gap beside the riser where both halves are at the SAME
        // height (a flush walkway). The flush gap must not erode — it is not a
        // wall, and the step beside it is climbable.
        //
        // Layout (z is depth, x is width), cell_size 0.25:
        //   Lower floor at y=0 over x in [0, 2], z in [0, 3].
        //   Upper floor at y=0.3 (== step_height) over x in [0, 2], z in [3, 6],
        //     EXCEPT a doorway column-strip x in [0.5, 1.0] kept at y=0 so the
        //     two halves are flush there (a level walkway through the gap).
        let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
        tris.extend_from_slice(&floor_quad(0.0, 0.0, 2.0, 3.0, 0.0)); // lower half
        // Upper half, split around the doorway strip [0.5, 1.0] in x.
        tris.extend_from_slice(&floor_quad(0.0, 3.0, 0.5, 6.0, 0.3));
        tris.extend_from_slice(&floor_quad(1.0, 3.0, 2.0, 6.0, 0.3));
        // Flush doorway walkway at y=0 bridging the two halves.
        tris.extend_from_slice(&floor_quad(0.5, 3.0, 1.0, 6.0, 0.0));

        let geo = geo_from_triangles(&tris);
        // Use a small but non-zero radius so erosion runs; the doorway is wide
        // enough (2 cells) that with radius 0 the interior survives, and we want
        // to prove the climbable step does NOT erode the doorway cells.
        let params = NavParams {
            agent_radius: 0.0,
            step_height: 0.3,
            ..no_erode_params()
        };
        let section = bake_navmesh(&geo, &params).expect("stepped floor must bake");

        // The doorway strip cells (x in [2, 4), at the boundary z=12 between the
        // halves) must be present somewhere in a region. Confirm a cell at the
        // flush gap, on the upper-half side row, is covered by a region.
        let covered = |x: u32, z: u32| {
            section
                .regions
                .iter()
                .any(|r| x >= r.x0 && x < r.x1 && z >= r.z0 && z < r.z1)
        };
        // Doorway columns x in [2, 4); pick a row just inside the upper half
        // (z = 12 is the first upper-half row: 3.0 / 0.25 = 12).
        assert!(
            covered(2, 12) && covered(3, 12),
            "doorway cells beside the climbable step must survive (not eroded as a wall)"
        );
    }

    #[test]
    fn climbable_step_yields_a_portal() {
        // Two floor halves one step_height apart, sharing a full edge run within
        // step → exactly one portal between the two regions.
        let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
        tris.extend_from_slice(&floor_quad(0.0, 0.0, 2.0, 1.0, 0.0)); // z in [0,1)
        tris.extend_from_slice(&floor_quad(0.0, 1.0, 2.0, 2.0, 0.3)); // z in [1,2), +step
        let geo = geo_from_triangles(&tris);
        let params = NavParams {
            agent_radius: 0.0,
            step_height: 0.3,
            ..no_erode_params()
        };
        let section = bake_navmesh(&geo, &params).expect("stepped floor must bake");
        assert_eq!(
            section.regions.len(),
            2,
            "a step splits the floor into two regions"
        );
        assert_eq!(
            section.portals.len(),
            1,
            "the climbable step shares a portal"
        );
        let p = &section.portals[0];
        assert!(p.region_a < p.region_b);
        // Portal Y is the min of the two floor heights along the run (= 0.0).
        assert!((p.left[1] - 0.0).abs() < 1.0e-4);
        // Endpoints lie on the shared Z line (z = 1.0 in world).
        assert!((p.left[2] - 1.0).abs() < 1.0e-4);
        assert!((p.right[2] - 1.0).abs() < 1.0e-4);
    }

    #[test]
    fn tall_ledge_yields_no_portal() {
        // Two floor halves separated by a drop larger than step_height. They are
        // distinct regions but share no traversable edge → no portal.
        let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
        tris.extend_from_slice(&floor_quad(0.0, 0.0, 2.0, 1.0, 0.0));
        tris.extend_from_slice(&floor_quad(0.0, 1.0, 2.0, 2.0, 1.0)); // +1.0 m, a ledge
        let geo = geo_from_triangles(&tris);
        let params = NavParams {
            agent_radius: 0.0,
            step_height: 0.3,
            ..no_erode_params()
        };
        let section = bake_navmesh(&geo, &params).expect("ledged floor must bake");
        assert_eq!(section.regions.len(), 2);
        assert!(
            section.portals.is_empty(),
            "a drop taller than step_height must yield no portal"
        );
    }

    #[test]
    fn stacked_floors_produce_distinct_regions_no_portal() {
        // Two floors over the SAME XZ footprint, vertically separated by more
        // than agent_height. Both are walkable (open clearance on each), giving
        // a region on each level over the same footprint and NO portal between
        // them (no shared cell edge — they overlap, not abut).
        let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
        tris.extend_from_slice(&floor_quad(0.0, 0.0, 2.0, 2.0, 0.0));
        tris.extend_from_slice(&floor_quad(0.0, 0.0, 2.0, 2.0, 5.0)); // 5 m up
        let geo = geo_from_triangles(&tris);
        let params = no_erode_params();
        let section = bake_navmesh(&geo, &params).expect("stacked floors must bake");

        // Both floors are independently walkable: each column keeps both spans
        // (the 5 m gap gives each open clearance), so the decomposition builds a
        // region on EACH level over the same footprint. The two regions overlap
        // in XZ but do not abut at a cell edge, so there is no portal between
        // them — vertical stacking is not a traversable step.
        assert_eq!(
            section.regions.len(),
            2,
            "stacked floors yield a distinct region on each level"
        );
        // Same cell footprint, different floor heights.
        let a = section.regions[0];
        let b = section.regions[1];
        assert_eq!((a.x0, a.z0, a.x1, a.z1), (0, 0, 8, 8));
        assert_eq!((b.x0, b.z0, b.x1, b.z1), (0, 0, 8, 8));
        let mut heights = [a.floor_y_min, b.floor_y_min];
        heights.sort_by(f32::total_cmp);
        assert!((heights[0] - 0.0).abs() < 1.0e-4, "lower level at y ~= 0");
        assert!((heights[1] - 5.0).abs() < 1.0e-4, "upper level at y ~= 5");
        assert!(
            section.portals.is_empty(),
            "stacked floors over the same footprint must not portal between levels"
        );
    }

    #[test]
    fn bake_is_byte_deterministic_in_process() {
        // Two in-process bakes of the same fixture must produce byte-identical
        // section bytes (the stage cache keys on these bytes).
        let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
        tris.extend_from_slice(&floor_quad(0.0, 0.0, 3.0, 3.0, 0.0));
        tris.extend_from_slice(&floor_quad(0.0, 3.0, 3.0, 6.0, 0.2));
        let geo = geo_from_triangles(&tris);
        let params = no_erode_params();
        let a = bake_navmesh(&geo, &params).expect("bake a").to_bytes();
        let b = bake_navmesh(&geo, &params).expect("bake b").to_bytes();
        assert_eq!(a, b, "navmesh bake must be byte-deterministic in-process");
    }

    #[test]
    fn regions_are_disjoint_and_cover_walkable_cells() {
        // An L-shaped floor: greedy decomposition must tile it with disjoint
        // rectangles whose union is exactly the walkable cells.
        let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
        tris.extend_from_slice(&floor_quad(0.0, 0.0, 3.0, 1.0, 0.0)); // horizontal arm
        tris.extend_from_slice(&floor_quad(0.0, 1.0, 1.0, 3.0, 0.0)); // vertical arm
        let geo = geo_from_triangles(&tris);
        let params = no_erode_params();
        let section = bake_navmesh(&geo, &params).expect("L floor must bake");

        // No two regions overlap (cell-space rectangles are disjoint).
        for i in 0..section.regions.len() {
            for j in (i + 1)..section.regions.len() {
                let a = section.regions[i];
                let b = section.regions[j];
                let overlap_x = a.x0 < b.x1 && b.x0 < a.x1;
                let overlap_z = a.z0 < b.z1 && b.z0 < a.z1;
                assert!(
                    !(overlap_x && overlap_z),
                    "regions {i} and {j} overlap in cell space"
                );
            }
        }
        // Total covered cell count equals the walkable cell count: an L over a
        // 12x12 grid (3 m / 0.25). Horizontal arm 12x4 + vertical arm 4x8.
        let covered: u32 = section
            .regions
            .iter()
            .map(|r| (r.x1 - r.x0) * (r.z1 - r.z0))
            .sum();
        assert_eq!(covered, 12 * 4 + 4 * 8);
    }

    #[test]
    fn cache_key_changes_with_each_nav_param() {
        // Changing any nav param must change the stage cache key (and the
        // unchanged case must reproduce the same key). Mirrors the SDF stage's
        // input-hash construction: blake3(postcard(geo) || postcard(params)).
        use crate::cache::CacheKey;

        let geo = geo_from_triangles(&floor_quad(0.0, 0.0, 2.0, 2.0, 0.0));
        let base = no_erode_params();

        let key_for = |params: &NavParams| -> String {
            let mut buf = postcard::to_allocvec(&geo).unwrap();
            buf.extend_from_slice(&postcard::to_allocvec(params).unwrap());
            let input_hash = blake3::hash(&buf);
            CacheKey::new("navmesh", NAVMESH_STAGE_VERSION, input_hash.as_bytes()).as_filename()
        };

        let base_key = key_for(&base);
        assert_eq!(
            base_key,
            key_for(&base),
            "unchanged params reproduce the key"
        );

        let mutated = [
            NavParams {
                agent_radius: base.agent_radius + 0.1,
                ..base
            },
            NavParams {
                agent_height: base.agent_height + 0.1,
                ..base
            },
            NavParams {
                step_height: base.step_height + 0.1,
                ..base
            },
            NavParams {
                max_slope_deg: base.max_slope_deg + 1.0,
                ..base
            },
            NavParams {
                cell_size: base.cell_size + 0.05,
                ..base
            },
        ];
        for m in &mutated {
            assert_ne!(
                base_key,
                key_for(m),
                "changing a nav param must change the cache key"
            );
        }
    }
}
