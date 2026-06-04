// Affinity-grid decomposition for animated-light delta SH volumes.
//
// Produces, per animated light, the coarse "affinity cells" the light's delta
// contribution should be stored at. An affinity cell is a cube of
// AFFINITY_FACTOR^3 base SH probes. The compose pass later reads one per-cell
// light list per workgroup so its threads only touch lights that reach the
// region — instead of looping over every animated light at every probe.
//
// This module only computes the in-memory decomposition (`per_light_cells` +
// dims). It does not change the delta wire format; `delta_sh_bake` consumes
// this structure to build the CSR index and bake the sparse sub-blocks.
//
// Reachability reuses the SAME inline per-light portal flood-fill pattern as
// `chunk_light_list_bake` (BFS over the `Portal` adjacency graph seeded at the
// light's containing leaf), including the solid/exterior-seed bypass.
//
// See: context/plans/in-progress/perf-animated-sh-light-culling/

use std::collections::{HashMap, HashSet, VecDeque};

use glam::DVec3;

use crate::light_namespaces::AnimatedBakedLights;
use crate::map_data::{LightType, MapLight};
use crate::partition::{BspTree, find_leaf_for_point};
use crate::portals::Portal;

/// Edge length of an affinity cell, in base SH probes, per axis.
///
/// LOCKED at 4 — this is not a tuning knob. One affinity cell must map to
/// exactly one compose workgroup (`@workgroup_size(4,4,4)` = 64 threads) so the
/// cell's light list is read once per workgroup and shared across its threads.
/// Changing this would desync the bake-time cell decomposition from the
/// shader's workgroup geometry.
pub const AFFINITY_FACTOR: u32 = 4;

/// AABB padding past the light's falloff sphere, meters. Extends affinity-cell
/// coverage slightly beyond the falloff radius so boundary probes are included
/// in the sub-block — the light still contributes a small amount there, and
/// dropping those probes would silently under-cull the region. Must match
/// `delta_sh_bake`'s padding so affinity cells and baked sub-blocks cover the
/// same volume.
///
/// `pub(crate)` so the per-light lightmap layer cache (`lightmap_layer.rs`)
/// derives the same influence AABB this grid uses (Task 4 of
/// `incremental-bake-per-element`). The `delta_sh_bake.rs` copy is a separate
/// `f64` value feeding the shipped delta path and is intentionally NOT folded.
pub(crate) const AABB_PADDING_METERS: f32 = 0.5;

/// Hard cap on directional-light AABB size, meters. MUST equal `delta_sh_bake`'s
/// value — if they diverge, affinity cells cover a different volume than the
/// baked sub-blocks, producing silently wrong culling for directional lights.
const DIRECTIONAL_FALLBACK_RANGE_METERS: f32 = 100.0;

/// Inputs for the affinity decomposition. The base SH volume's AABB and probe
/// spacing must match the base `bake_sh_volume` call so affinity cells align
/// with real base probes; pass the same world vertex bounds and `probe_spacing`.
pub struct AffinityInputs<'a> {
    pub geometry_vertices: &'a [[f32; 3]],
    pub tree: &'a BspTree,
    pub exterior_leaves: &'a HashSet<usize>,
    pub portals: &'a [Portal],
    pub animated_lights: &'a AnimatedBakedLights<'a>,
    /// Base SH probe spacing, meters. Same value passed to `bake_sh_volume`.
    pub probe_spacing: f32,
}

/// Result of the affinity decomposition.
pub struct AffinityDecomposition {
    /// Affinity grid dimensions = `ceil(base_dims / AFFINITY_FACTOR)`, per axis.
    pub affinity_dims: [u32; 3],
    /// Per animated light (outer index aligned with
    /// `AnimatedBakedLights::entries()`), the affinity-cell linear indices the
    /// light reaches. Linearized x-fastest: `idx = x + y*dx + z*dx*dy`.
    pub per_light_cells: Vec<Vec<u32>>,
}

impl AffinityDecomposition {
    pub fn affinity_cell_count(&self) -> usize {
        self.affinity_dims[0] as usize
            * self.affinity_dims[1] as usize
            * self.affinity_dims[2] as usize
    }
}

/// Decompose each animated light into the affinity cells its (AABB ∩
/// portal-reachable region) overlaps.
///
/// The base SH volume covers the world vertex AABB at `probe_spacing`; the
/// affinity grid covers that same AABB at `AFFINITY_FACTOR ×` coarser. For each
/// light we walk the affinity cells overlapping the light AABB and keep a cell
/// when its centroid lands in a portal-reachable leaf (with the same
/// solid/exterior-seed bypass `chunk_light_list_bake` uses).
pub fn decompose_affinity(inputs: &AffinityInputs<'_>) -> AffinityDecomposition {
    let (base_min, base_max) = world_aabb(inputs.geometry_vertices);
    let base_dims = grid_dimensions(base_min, base_max, inputs.probe_spacing);
    let affinity_dims = [
        base_dims[0].div_ceil(AFFINITY_FACTOR).max(1),
        base_dims[1].div_ceil(AFFINITY_FACTOR).max(1),
        base_dims[2].div_ceil(AFFINITY_FACTOR).max(1),
    ];

    // No geometry → no base grid → no affinity cells. Return aligned-but-empty.
    if !base_min.x.is_finite() {
        return AffinityDecomposition {
            affinity_dims,
            per_light_cells: vec![Vec::new(); inputs.animated_lights.len()],
        };
    }

    // One affinity cell spans AFFINITY_FACTOR base probes; base probes are
    // `probe_spacing` apart, so an affinity cell is this many meters per axis.
    let affinity_cell_meters = inputs.probe_spacing.max(1.0e-4) as f64 * AFFINITY_FACTOR as f64;

    // Portal adjacency graph — identical construction to chunk_light_list_bake.
    let mut adjacency: HashMap<usize, Vec<usize>> = HashMap::new();
    for p in inputs.portals {
        adjacency.entry(p.front_leaf).or_default().push(p.back_leaf);
        adjacency.entry(p.back_leaf).or_default().push(p.front_leaf);
    }

    let world_aabb_d = world_aabb_for_directional(inputs.geometry_vertices);

    let per_light_cells = inputs
        .animated_lights
        .entries()
        .iter()
        .map(|entry| {
            let light = entry.light;
            let reachable = reachable_leaves(light, inputs, &adjacency);
            cells_for_light(
                light,
                world_aabb_d,
                base_min,
                affinity_dims,
                affinity_cell_meters,
                inputs,
                reachable.as_ref(),
            )
        })
        .collect();

    AffinityDecomposition {
        affinity_dims,
        per_light_cells,
    }
}

// ---------------------------------------------------------------------------
// Reachability — inline BFS, mirrors chunk_light_list_bake.rs:98-135.

/// `None` means the portal filter is bypassed (directional source, or origin in
/// a solid/exterior leaf) — every overlapping cell is kept. `Some(set)` is the
/// set of leaves reachable from the light's leaf through non-exterior portals.
fn reachable_leaves(
    light: &MapLight,
    inputs: &AffinityInputs<'_>,
    adjacency: &HashMap<usize, Vec<usize>>,
) -> Option<HashSet<usize>> {
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
}

// ---------------------------------------------------------------------------
// Cell decomposition

#[allow(clippy::too_many_arguments)]
fn cells_for_light(
    light: &MapLight,
    world_aabb_d: (DVec3, DVec3),
    base_min: DVec3,
    affinity_dims: [u32; 3],
    affinity_cell_meters: f64,
    inputs: &AffinityInputs<'_>,
    reachable: Option<&HashSet<usize>>,
) -> Vec<u32> {
    let (light_min, light_max) = light_aabb(light, world_aabb_d);

    // Translate the light AABB into affinity-cell index ranges, clamped to the
    // grid. A cell `c` on an axis spans `[base_min + c*cell, base_min +
    // (c+1)*cell]`; the light AABB overlaps cells `floor((lo-base_min)/cell)`
    // through `floor((hi-base_min)/cell)`.
    let lo = cell_range(light_min, base_min, affinity_cell_meters, affinity_dims);
    let hi = cell_range(light_max, base_min, affinity_cell_meters, affinity_dims);

    let mut cells = Vec::new();
    let nx = affinity_dims[0] as usize;
    let ny = affinity_dims[1] as usize;
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                // Keep the cell unless the portal filter rejects it. Filter is
                // bypassed (cell kept) when reachable is None, or when the
                // cell centroid lands in a solid/exterior/out-of-range leaf —
                // exactly the bypass chunk_light_list_bake applies per chunk.
                if let Some(set) = reachable {
                    let centroid = DVec3::new(
                        base_min.x + (x as f64 + 0.5) * affinity_cell_meters,
                        base_min.y + (y as f64 + 0.5) * affinity_cell_meters,
                        base_min.z + (z as f64 + 0.5) * affinity_cell_meters,
                    );
                    let leaf = find_leaf_for_point(inputs.tree, centroid);
                    let bypass = leaf >= inputs.tree.leaves.len()
                        || inputs.tree.leaves[leaf].is_solid
                        || inputs.exterior_leaves.contains(&leaf);
                    if !bypass && !set.contains(&leaf) {
                        continue;
                    }
                }
                cells.push((x + y * nx + z * nx * ny) as u32);
            }
        }
    }
    cells
}

/// Clamp a world coordinate to an inclusive affinity-cell index per axis.
fn cell_range(p: DVec3, base_min: DVec3, cell_meters: f64, dims: [u32; 3]) -> [usize; 3] {
    let idx = |v: f64, lo: f64, n: u32| -> usize {
        let c = ((v - lo) / cell_meters).floor();
        if c < 0.0 {
            0
        } else {
            (c as usize).min(n as usize - 1)
        }
    };
    [
        idx(p.x, base_min.x, dims[0]),
        idx(p.y, base_min.y, dims[1]),
        idx(p.z, base_min.z, dims[2]),
    ]
}

// ---------------------------------------------------------------------------
// Shared geometry helpers — mirror delta_sh_bake / sh_bake so the affinity grid
// lines up with the base SH volume and the per-light delta AABBs.

/// Influence AABB of a single light. Point/Spot → a cube of half-extent
/// `falloff_range + AABB_PADDING_METERS` about the origin; Directional →
/// the whole-world AABB (parallel light reaches everywhere).
///
/// `pub(crate)` so the per-light lightmap layer cache reuses the exact same
/// influence bound the affinity grid uses (Task 4 of
/// `incremental-bake-per-element`). This f32-falloff/f64-origin copy is
/// authoritative for the lightmap layer key; the `delta_sh_bake.rs` copy
/// (f64 padding, different cast order) stays separate.
pub(crate) fn light_aabb(light: &MapLight, world_aabb: (DVec3, DVec3)) -> (DVec3, DVec3) {
    match light.light_type {
        LightType::Directional => world_aabb,
        LightType::Point | LightType::Spot => {
            let r = (light.falloff_range + AABB_PADDING_METERS).max(0.01) as f64;
            let center = DVec3::new(light.origin.x, light.origin.y, light.origin.z);
            (center - DVec3::splat(r), center + DVec3::splat(r))
        }
    }
}

fn world_aabb(vertices: &[[f32; 3]]) -> (DVec3, DVec3) {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for v in vertices {
        let p = DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64);
        min = min.min(p);
        max = max.max(p);
    }
    (min, max)
}

fn world_aabb_for_directional(vertices: &[[f32; 3]]) -> (DVec3, DVec3) {
    let (min, max) = world_aabb(vertices);
    if !min.x.is_finite() {
        let r = DIRECTIONAL_FALLBACK_RANGE_METERS as f64;
        return (DVec3::splat(-r), DVec3::splat(r));
    }
    (min, max)
}

/// Base SH grid dims — identical to `sh_bake::grid_dimensions` so the affinity
/// grid is derived from the same probe count the base volume bakes.
fn grid_dimensions(min: DVec3, max: DVec3, spacing: f32) -> [u32; 3] {
    let extents = (max - min).max(DVec3::splat(0.0));
    let spacing = spacing.max(1.0e-4) as f64;
    [
        ((extents.x / spacing).ceil() as u32 + 1).max(1),
        ((extents.y / spacing).ceil() as u32 + 1).max(1),
        ((extents.z / spacing).ceil() as u32 + 1).max(1),
    ]
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::{FalloffModel, LightAnimation, LightType};
    use crate::partition::{Aabb, BspChild, BspLeaf, BspNode};

    fn animated_point_light(origin: DVec3, range: f32) -> MapLight {
        MapLight {
            origin,
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
            cast_shadows: true,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        }
    }

    fn empty_tree() -> BspTree {
        BspTree {
            nodes: Vec::new(),
            leaves: Vec::new(),
        }
    }

    // A 16m cube of world geometry: vertices span [-8, 8] on every axis.
    fn cube_vertices() -> Vec<[f32; 3]> {
        let s = 8.0;
        vec![
            [-s, -s, -s],
            [s, -s, -s],
            [s, s, -s],
            [-s, s, -s],
            [-s, -s, s],
            [s, -s, s],
            [s, s, s],
            [-s, s, s],
        ]
    }

    #[test]
    fn affinity_dims_is_base_dims_div_ceil_four() {
        // World AABB [-8,8] = 16m extent. At 1m spacing base dims = ceil(16/1)+1
        // = 17 per axis. ceil(17/4) = 5 per axis.
        let verts = cube_vertices();
        let lights: Vec<MapLight> = Vec::new();
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let exterior: HashSet<usize> = HashSet::new();
        let inputs = AffinityInputs {
            geometry_vertices: &verts,
            tree: &empty_tree(),
            exterior_leaves: &exterior,
            portals: &[],
            animated_lights: &envelope,
            probe_spacing: 1.0,
        };
        let result = decompose_affinity(&inputs);
        assert_eq!(result.affinity_dims, [5, 5, 5]);
        assert_eq!(result.affinity_cell_count(), 125);
        assert!(result.per_light_cells.is_empty());
    }

    #[test]
    fn light_cells_are_subset_of_full_grid_and_x_fastest_linearized() {
        // Light at origin, range 2 → AABB half-extent 2.5m. With empty tree the
        // portal filter is bypassed, so we get exactly the cells the AABB
        // overlaps — a small box around the grid center, strictly fewer than all
        // 125 cells. Verifies the AABB→cell mapping and linearization.
        let verts = cube_vertices();
        let lights = vec![animated_point_light(DVec3::ZERO, 2.0)];
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let exterior: HashSet<usize> = HashSet::new();
        let inputs = AffinityInputs {
            geometry_vertices: &verts,
            tree: &empty_tree(),
            exterior_leaves: &exterior,
            portals: &[],
            animated_lights: &envelope,
            probe_spacing: 1.0,
        };
        let result = decompose_affinity(&inputs);
        assert_eq!(result.per_light_cells.len(), 1);
        let cells = &result.per_light_cells[0];

        let total = result.affinity_cell_count() as u32;
        assert!(!cells.is_empty(), "light at origin must reach some cells");
        assert!(
            (cells.len() as u32) < total,
            "AABB clip must exclude far cells (got {} of {})",
            cells.len(),
            total,
        );
        // Every index in range and unique.
        let unique: HashSet<u32> = cells.iter().copied().collect();
        assert_eq!(unique.len(), cells.len(), "cell indices must be unique");
        for &c in cells {
            assert!(c < total, "cell index {c} out of range");
        }

        // Affinity cell = 4 base probes = 4m. base_min = (-8,-8,-8). The light
        // AABB [-2.5, 2.5] spans cells floor((-2.5+8)/4)=1 .. floor((2.5+8)/4)=2
        // on each axis → a 2×2×2 block = 8 cells.
        assert_eq!(cells.len(), 8);
        let nx = result.affinity_dims[0] as u32;
        let ny = result.affinity_dims[1] as u32;
        let mut expected: Vec<u32> = Vec::new();
        for z in 1..=2u32 {
            for y in 1..=2u32 {
                for x in 1..=2u32 {
                    expected.push(x + y * nx + z * nx * ny);
                }
            }
        }
        let mut got = cells.clone();
        got.sort_unstable();
        expected.sort_unstable();
        assert_eq!(got, expected);
    }

    #[test]
    fn portal_filter_drops_unreachable_cells() {
        // Two non-solid leaves split at x=0, NO portals between them. A light in
        // leaf 0 (x<0) must not reach cells whose centroid lands in leaf 1.
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
        let verts = cube_vertices();
        // Range 50 so the AABB spans the whole world (cells on both sides).
        let lights = vec![animated_point_light(DVec3::new(-4.0, 0.0, 0.0), 50.0)];
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let exterior: HashSet<usize> = HashSet::new();
        let inputs = AffinityInputs {
            geometry_vertices: &verts,
            tree: &tree,
            exterior_leaves: &exterior,
            portals: &[],
            animated_lights: &envelope,
            probe_spacing: 1.0,
        };
        let result = decompose_affinity(&inputs);
        let cells = &result.per_light_cells[0];
        assert!(!cells.is_empty());

        // No cell may have a centroid with x >= 0 (that lands in unreachable
        // leaf 1). cell centroid x = base_min.x + (cx+0.5)*4.
        let base_min_x = -8.0_f64;
        let cell_m = 4.0_f64;
        let nx = result.affinity_dims[0] as u32;
        let ny = result.affinity_dims[1] as u32;
        for &c in cells {
            let cx = c % nx;
            let cy = (c / nx) % ny;
            let _ = cy;
            let centroid_x = base_min_x + (cx as f64 + 0.5) * cell_m;
            assert!(
                centroid_x < 0.0,
                "cell {c} (cx={cx}) has centroid x={centroid_x} in unreachable leaf 1",
            );
        }
    }

    #[test]
    fn solid_seed_leaf_bypasses_filter() {
        // Light origin lands in a solid leaf → filter bypassed → every cell the
        // AABB overlaps is kept (same as empty-tree behavior).
        let tree = BspTree {
            nodes: Vec::new(),
            leaves: vec![BspLeaf {
                face_indices: Vec::new(),
                bounds: Aabb::empty(),
                is_solid: true,
                defining_planes: Vec::new(),
            }],
        };
        let verts = cube_vertices();
        let lights = vec![animated_point_light(DVec3::ZERO, 2.0)];
        let envelope = AnimatedBakedLights::from_lights(&lights);
        let exterior: HashSet<usize> = HashSet::new();
        let inputs = AffinityInputs {
            geometry_vertices: &verts,
            tree: &tree,
            exterior_leaves: &exterior,
            portals: &[],
            animated_lights: &envelope,
            probe_spacing: 1.0,
        };
        let result = decompose_affinity(&inputs);
        // Bypassed filter → same 8-cell block as the empty-tree subset test.
        assert_eq!(result.per_light_cells[0].len(), 8);
    }
}
