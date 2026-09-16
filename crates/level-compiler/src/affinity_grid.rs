// Shared affinity decomposition for SH sections ids 27, 35, 41, and 45.
// See: context/lib/build_pipeline.md §PRL section IDs

use std::collections::{HashMap, HashSet, VecDeque};

use glam::{DVec3, Vec3};

use crate::light_namespaces::AnimatedBakedLights;
use crate::map_data::{LightType, MapLight};
use crate::partition::{BspTree, find_leaf_for_point};
use crate::portals::Portal;
use crate::sh_bake::spot_cone_parameters;

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
/// derives the same influence AABB this grid uses, ensuring cache-key locality
/// matches the affinity cells (see `lightmap_layer::layer_influence_aabb`). The
/// `delta_sh_bake.rs` copy is a separate `f64` value feeding the shipped delta
/// path and is intentionally NOT folded.
pub(crate) const AABB_PADDING_METERS: f32 = 0.5;

/// Hard cap on directional-light AABB size, meters. MUST equal `delta_sh_bake`'s
/// value — if they diverge, affinity cells cover a different volume than the
/// baked sub-blocks, producing silently wrong culling for directional lights.
const DIRECTIONAL_FALLBACK_RANGE_METERS: f32 = 100.0;

/// Outward cone slack for f32 coordinate and transcendental roundoff.
const CONE_BOUNDARY_TOLERANCE_RADIANS: f32 = 1.0e-4;

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

/// Light-list-generic inputs for the affinity decomposition: the same world /
/// portal / spacing context as [`AffinityInputs`], but with the light set passed
/// as a plain `&[&MapLight]` slice instead of an `AnimatedBakedLights` envelope.
///
/// The shared reach test clips by falloff, floods through portals, and may
/// reject direct spotlight cells outside the authored cone. The direct-at-probe
/// SH bake (`direct_sh_bake.rs`) reuses this core for its static light set.
pub struct AffinityReachInputs<'a> {
    pub geometry_vertices: &'a [[f32; 3]],
    pub tree: &'a BspTree,
    pub exterior_leaves: &'a HashSet<usize>,
    pub portals: &'a [Portal],
    pub probe_spacing: f32,
}

/// Transport semantics for one affinity decomposition. Direct transport may
/// discard cells provably outside a spotlight's authored outer cone; indirect
/// transport must retain the falloff cube because bounced light leaves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AffinityTransport {
    Indirect,
    Direct,
}

/// Reach policy layered over the shared falloff/portal decomposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AffinityReachPolicy {
    pub transport: AffinityTransport,
    /// Script-mutable animated-direct slots preserve their established cube
    /// reach so exact-zero records reserved by the post-bake policy remain
    /// byte-identical. Other direct transports enable the cone clamp.
    cone_cull_spots: bool,
    /// Id 41 must keep the same final cube-reach cell that the exact-zero drop
    /// policy uses as a selected light's canonical representation.
    pub retain_canonical_cell: bool,
}

impl AffinityReachPolicy {
    pub const INDIRECT: Self = Self {
        transport: AffinityTransport::Indirect,
        cone_cull_spots: false,
        retain_canonical_cell: false,
    };
    pub const DIRECT_UNCLIPPED: Self = Self {
        transport: AffinityTransport::Direct,
        cone_cull_spots: false,
        retain_canonical_cell: false,
    };
    pub const DIRECT: Self = Self {
        transport: AffinityTransport::Direct,
        cone_cull_spots: true,
        retain_canonical_cell: false,
    };
    pub const SELECTED_DIRECT: Self = Self {
        transport: AffinityTransport::Direct,
        cone_cull_spots: true,
        retain_canonical_cell: true,
    };
}

/// Result of the affinity decomposition.
pub struct AffinityDecomposition {
    /// Affinity grid dimensions = `ceil(base_dims / AFFINITY_FACTOR)`, per axis.
    pub affinity_dims: [u32; 3],
    /// Per input light, in the caller's light-index space, the affinity-cell
    /// linear indices it reaches. Linearized x-fastest: `idx = x + y*dx + z*dx*dy`.
    pub per_light_cells: Vec<Vec<u32>>,
}

impl AffinityDecomposition {
    pub fn affinity_cell_count(&self) -> usize {
        self.affinity_dims[0] as usize
            * self.affinity_dims[1] as usize
            * self.affinity_dims[2] as usize
    }
}

/// Decompose each animated light into affinity cells selected by the indirect
/// falloff-and-portal policy.
///
/// The base SH volume covers the world vertex AABB at `probe_spacing`; the
/// affinity grid covers that same AABB at `AFFINITY_FACTOR ×` coarser. For each
/// light we walk the affinity cells overlapping the light AABB and keep a cell
/// when its centroid lands in a portal-reachable leaf (with the same
/// solid/exterior-seed bypass `chunk_light_list_bake` uses).
pub fn decompose_affinity(inputs: &AffinityInputs<'_>) -> AffinityDecomposition {
    let reach = AffinityReachInputs {
        geometry_vertices: inputs.geometry_vertices,
        tree: inputs.tree,
        exterior_leaves: inputs.exterior_leaves,
        portals: inputs.portals,
        probe_spacing: inputs.probe_spacing,
    };
    let lights: Vec<&MapLight> = inputs
        .animated_lights
        .entries()
        .iter()
        .map(|e| e.light)
        .collect();
    decompose_affinity_for_lights(&reach, &lights, AffinityReachPolicy::INDIRECT)
}

/// Light-list-generic affinity decomposition over an arbitrary `&[&MapLight]`
/// slice in caller order. Policy selects indirect falloff-and-portal reach or
/// direct reach with spotlight-cone rejection. `per_light_cells[i]` preserves
/// `lights[i]`'s caller-defined index space.
pub fn decompose_affinity_for_lights(
    inputs: &AffinityReachInputs<'_>,
    lights: &[&MapLight],
    policy: AffinityReachPolicy,
) -> AffinityDecomposition {
    decompose_affinity_for_lights_with_policies(inputs, lights, &vec![policy; lights.len()])
}

/// Per-light policy variant used by animated direct SH: script-mutable slots
/// retain the historical cube reach while immutable slots use cone reach.
pub fn decompose_affinity_for_lights_with_policies(
    inputs: &AffinityReachInputs<'_>,
    lights: &[&MapLight],
    policies: &[AffinityReachPolicy],
) -> AffinityDecomposition {
    assert_eq!(
        lights.len(),
        policies.len(),
        "every affinity light needs one reach policy"
    );
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
            per_light_cells: vec![Vec::new(); lights.len()],
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

    let per_light_cells = lights
        .iter()
        .zip(policies)
        .map(|(&light, &policy)| {
            let reachable = reachable_leaves(light, inputs, &adjacency);
            cells_for_light(
                light,
                world_aabb_d,
                base_min,
                affinity_dims,
                affinity_cell_meters,
                inputs,
                reachable.as_ref(),
                policy,
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
    inputs: &AffinityReachInputs<'_>,
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
    inputs: &AffinityReachInputs<'_>,
    reachable: Option<&HashSet<usize>>,
    policy: AffinityReachPolicy,
) -> Vec<u32> {
    let (light_min, light_max) = light_aabb(light, world_aabb_d);

    // Translate the light AABB into affinity-cell index ranges, clamped to the
    // grid. A cell `c` on an axis spans `[base_min + c*cell, base_min +
    // (c+1)*cell]`; the light AABB overlaps cells `floor((lo-base_min)/cell)`
    // through `floor((hi-base_min)/cell)`.
    let lo = cell_range(light_min, base_min, affinity_cell_meters, affinity_dims);
    let hi = cell_range(light_max, base_min, affinity_cell_meters, affinity_dims);

    let mut cells = Vec::new();
    let mut canonical = None;
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
                let cell = (x + y * nx + z * nx * ny) as u32;
                if policy.retain_canonical_cell {
                    canonical = Some(cell);
                }
                if policy.cone_cull_spots {
                    let cell_min = DVec3::new(
                        base_min.x + x as f64 * affinity_cell_meters,
                        base_min.y + y as f64 * affinity_cell_meters,
                        base_min.z + z as f64 * affinity_cell_meters,
                    );
                    let cell_max = cell_min + DVec3::splat(affinity_cell_meters);
                    if !direct_light_may_reach_cell(light, cell_min, cell_max) {
                        continue;
                    }
                }
                cells.push(cell);
            }
        }
    }

    if let Some(canonical) = canonical
        && cells.last().copied() != Some(canonical)
    {
        cells.push(canonical);
    }
    cells
}

/// Conservative f32-cone test. The sphere encloses the complete cell AABB in
/// the same coordinate domain as the direct probe bake. Invalid state retains.
fn direct_light_may_reach_cell(light: &MapLight, cell_min: DVec3, cell_max: DVec3) -> bool {
    if light.light_type != LightType::Spot {
        return true;
    }

    if !light.origin.is_finite()
        || !light.falloff_range.is_finite()
        || !cell_min.is_finite()
        || !cell_max.is_finite()
    {
        return true;
    }
    let origin = Vec3::new(
        light.origin.x as f32,
        light.origin.y as f32,
        light.origin.z as f32,
    );
    let cell_min = Vec3::new(cell_min.x as f32, cell_min.y as f32, cell_min.z as f32);
    let cell_max = Vec3::new(cell_max.x as f32, cell_max.y as f32, cell_max.z as f32);
    if !origin.is_finite()
        || !cell_min.is_finite()
        || !cell_max.is_finite()
        || cell_min.x > cell_max.x
        || cell_min.y > cell_max.y
        || cell_min.z > cell_max.z
    {
        return true;
    }

    let (shared_axis, cos_inner, cos_outer) = spot_cone_parameters(light);
    if !shared_axis.is_finite() || !cos_inner.is_finite() || !cos_outer.is_finite() {
        return true;
    }
    let axis_length_squared = shared_axis.length_squared();
    if !axis_length_squared.is_finite() || axis_length_squared < 1.0e-12 {
        return true;
    }
    // `spot_cone_parameters` supplies the baker's f32 axis; normalize it again
    // after the shared conversion so this sphere test uses a unit cone axis.
    let axis = shared_axis / axis_length_squared.sqrt();
    if !axis.is_finite() {
        return true;
    }

    let center = (cell_min + cell_max) * 0.5;
    // The rounded midpoint can lie off-center at large coordinates. Measure
    // every rounded corner from that exact center so every f32 probe position
    // within the cell remains inside this conservative sphere.
    let radius = [
        Vec3::new(cell_min.x, cell_min.y, cell_min.z),
        Vec3::new(cell_min.x, cell_min.y, cell_max.z),
        Vec3::new(cell_min.x, cell_max.y, cell_min.z),
        Vec3::new(cell_min.x, cell_max.y, cell_max.z),
        Vec3::new(cell_max.x, cell_min.y, cell_min.z),
        Vec3::new(cell_max.x, cell_min.y, cell_max.z),
        Vec3::new(cell_max.x, cell_max.y, cell_min.z),
        Vec3::new(cell_max.x, cell_max.y, cell_max.z),
    ]
    .into_iter()
    .map(|corner| (corner - center).length())
    .fold(0.0_f32, f32::max);
    let to_center = center - origin;
    let distance = to_center.length();
    let padded_reach = (light.falloff_range + AABB_PADDING_METERS).max(0.01);
    if !center.is_finite()
        || !radius.is_finite()
        || !to_center.is_finite()
        || !distance.is_finite()
        || !padded_reach.is_finite()
    {
        return true;
    }
    if distance - radius > padded_reach {
        return false;
    }
    if distance <= radius {
        return true;
    }

    let center_angle = axis.dot(to_center / distance).clamp(-1.0, 1.0).acos();
    let angular_radius = (radius / distance).clamp(0.0, 1.0).asin();
    let outer_angle = cos_outer.clamp(-1.0, 1.0).acos();
    if !center_angle.is_finite() || !angular_radius.is_finite() || !outer_angle.is_finite() {
        return true;
    }
    center_angle <= outer_angle + angular_radius + CONE_BOUNDARY_TOLERANCE_RADIANS
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
/// `pub(crate)` so the per-light lightmap layer cache (`lightmap_layer.rs`)
/// reuses the exact same influence bound the affinity grid uses, for cache-key
/// locality (see `lightmap_layer::layer_influence_aabb`). This
/// f32-falloff/f64-origin copy is authoritative for the lightmap layer key;
/// the `delta_sh_bake.rs` copy (f64 padding, different cast order) stays
/// separate.
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
// CSR construction — shared by `delta_sh_bake.rs` (animated-light path) and
// `direct_sh_bake.rs` (static-direct path). Both invert an `AffinityDecomposition`'s
// `per_light_cells` (light → cells it reaches) into the same cell-keyed CSR shape
// the wire format and the runtime compose/lookup logic expect. "Light index" here
// is caller-defined: `delta_sh_bake` indexes into `AnimatedBakedLights::entries()`;
// `direct_sh_bake` indexes into its own selection list — either way the returned
// `affinity_lights` entries reuse whatever index space `per_light_cells` was built
// over, unchanged.

/// Invert `per_light_cells` (light → affinity cells it reaches) into a CSR index
/// keyed by affinity cell (cell → lights touching it). Returns
/// `(affinity_offsets, affinity_lights)`:
///   - `affinity_offsets` has `cell_count + 1` entries; `offsets[c]..offsets[c+1]`
///     bounds cell `c`'s slice of `affinity_lights`. The trailing entry equals
///     `affinity_lights.len()`.
///   - `affinity_lights` is the flat light-index list, grouped by cell. Within a
///     cell, lights appear in ascending light index (deterministic output).
pub(crate) fn build_csr(
    per_light_cells: &[Vec<u32>],
    affinity_cell_count: usize,
) -> (Vec<u32>, Vec<u32>) {
    // Count how many lights touch each cell (counting sort over cells).
    let mut counts = vec![0u32; affinity_cell_count];
    for cells in per_light_cells {
        for &cell in cells {
            counts[cell as usize] += 1;
        }
    }

    // Prefix-sum the counts into offsets.
    let mut affinity_offsets = vec![0u32; affinity_cell_count + 1];
    let mut running = 0u32;
    for c in 0..affinity_cell_count {
        affinity_offsets[c] = running;
        running += counts[c];
    }
    affinity_offsets[affinity_cell_count] = running;

    // Scatter lights into their cells. Iterating lights in ascending order keeps
    // each cell's slice ascending without a separate sort. `cursor` tracks the
    // next free slot per cell.
    let mut affinity_lights = vec![0u32; running as usize];
    let mut cursor: Vec<u32> = affinity_offsets[..affinity_cell_count].to_vec();
    for (light, cells) in per_light_cells.iter().enumerate() {
        for &cell in cells {
            let slot = cursor[cell as usize] as usize;
            affinity_lights[slot] = light as u32;
            cursor[cell as usize] += 1;
        }
    }

    (affinity_offsets, affinity_lights)
}

/// Expand the CSR offsets into a per-entry cell index, parallel to
/// `affinity_lights` / a per-entry sub-block array. Entry `i` belongs to cell
/// `cells[i]`.
pub(crate) fn csr_entry_cells(affinity_offsets: &[u32]) -> Vec<u32> {
    let total = *affinity_offsets.last().unwrap_or(&0) as usize;
    let mut cells = Vec::with_capacity(total);
    for cell in 0..affinity_offsets.len().saturating_sub(1) {
        let count = affinity_offsets[cell + 1] - affinity_offsets[cell];
        for _ in 0..count {
            cells.push(cell as u32);
        }
    }
    cells
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
            animation: Some(LightAnimation {
                period: 1.0,
                phase: 0.0,
                brightness: Some(vec![0.0, 1.0, 0.0]),
                color: None,
                direction: None,
                start_active: true,
            }),
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        }
    }

    fn spot_light(origin: DVec3, range: f32, direction: [f32; 3], outer: f32) -> MapLight {
        let mut light = animated_point_light(origin, range);
        light.light_type = LightType::Spot;
        light.cone_angle_inner = Some(outer * 0.5);
        light.cone_angle_outer = Some(outer);
        light.cone_direction = Some(direction);
        light
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

    #[test]
    fn direct_spot_cell_test_rejects_only_spheres_outside_the_outer_cone() {
        let light = spot_light(DVec3::ZERO, 10.0, [1.0, 0.0, 0.0], 15.0f32.to_radians());

        assert!(!direct_light_may_reach_cell(
            &light,
            DVec3::new(4.0, 4.0, 0.0),
            DVec3::new(5.0, 5.0, 1.0),
        ));
        assert!(!direct_light_may_reach_cell(
            &light,
            DVec3::new(-5.0, 0.0, 0.0),
            DVec3::new(-4.0, 1.0, 1.0),
        ));

        // The cell straddles the cone: its axis-side edge is in-cone while its
        // far corner is out. The enclosing-sphere test must conservatively keep it.
        assert!(direct_light_may_reach_cell(
            &light,
            DVec3::new(4.0, 0.0, 0.0),
            DVec3::new(5.0, 1.0, 1.0),
        ));
    }

    #[test]
    fn direct_spot_cell_test_uses_f32_probe_coordinates_at_large_offsets() {
        let light = spot_light(
            DVec3::new(100_000_000.0, 100_000_000.0, 0.0),
            20.0,
            [1.0, 0.0, 0.0],
            15.0f32.to_radians(),
        );
        // In f64 this point is 16.7 degrees from +X and lies outside the cone.
        // The direct probe bake rounds it to (origin + 8, origin, 0), on-axis.
        assert!(direct_light_may_reach_cell(
            &light,
            DVec3::new(100_000_010.0, 100_000_003.0, 0.0),
            DVec3::new(100_000_010.0, 100_000_003.0, 0.0),
        ));
    }

    #[test]
    fn direct_spot_cell_test_keeps_a_rounded_large_coordinate_cell_corner() {
        let light = spot_light(
            DVec3::new(16_777_220.0, 0.0, 0.0),
            2.4,
            [-1.0, 0.0, 0.0],
            15.0f32.to_radians(),
        );

        // This is the cone culler's full x cell. Its f32 midpoint rounds down
        // to 16_777_216, while the upper corner remains 16_777_218. That corner
        // is an on-axis f32 probe within the light's falloff and must be kept.
        assert!(direct_light_may_reach_cell(
            &light,
            DVec3::new(16_777_216.0, 0.0, 0.0),
            DVec3::new(16_777_218.0, 0.0, 0.0),
        ));
    }

    #[test]
    fn direct_spot_cell_test_retains_non_finite_origin() {
        let light = spot_light(
            DVec3::new(f64::NAN, 0.0, 0.0),
            10.0,
            [1.0, 0.0, 0.0],
            15.0f32.to_radians(),
        );

        assert!(direct_light_may_reach_cell(
            &light,
            DVec3::new(4.0, 4.0, 0.0),
            DVec3::new(5.0, 5.0, 1.0),
        ));
    }

    #[test]
    fn direct_and_indirect_spot_decompositions_use_separate_transport_reach() {
        let verts = cube_vertices();
        let exterior: HashSet<usize> = HashSet::new();
        let tree = empty_tree();
        let lights = vec![spot_light(
            DVec3::ZERO,
            20.0,
            [1.0, 0.0, 0.0],
            12.0f32.to_radians(),
        )];
        let animated = AnimatedBakedLights::from_lights(&lights);
        let indirect = decompose_affinity(&AffinityInputs {
            geometry_vertices: &verts,
            tree: &tree,
            exterior_leaves: &exterior,
            portals: &[],
            animated_lights: &animated,
            probe_spacing: 1.0,
        });
        let reach = AffinityReachInputs {
            geometry_vertices: &verts,
            tree: &tree,
            exterior_leaves: &exterior,
            portals: &[],
            probe_spacing: 1.0,
        };
        let direct_lights = [&lights[0]];
        let direct =
            decompose_affinity_for_lights(&reach, &direct_lights, AffinityReachPolicy::DIRECT);

        assert_eq!(
            indirect.per_light_cells[0].len(),
            indirect.affinity_cell_count()
        );
        assert!(direct.per_light_cells[0].len() < indirect.per_light_cells[0].len());
        assert!(!direct.per_light_cells[0].is_empty());
    }

    #[test]
    fn selected_direct_spot_retains_the_unculled_canonical_cell() {
        let verts = cube_vertices();
        let exterior: HashSet<usize> = HashSet::new();
        let tree = empty_tree();
        let reach = AffinityReachInputs {
            geometry_vertices: &verts,
            tree: &tree,
            exterior_leaves: &exterior,
            portals: &[],
            probe_spacing: 1.0,
        };
        let light = spot_light(
            DVec3::new(20.0, 0.0, 0.0),
            20.0,
            [1.0, 0.0, 0.0],
            5.0f32.to_radians(),
        );
        let lights = [&light];

        let cube = decompose_affinity_for_lights(&reach, &lights, AffinityReachPolicy::INDIRECT);
        let selected =
            decompose_affinity_for_lights(&reach, &lights, AffinityReachPolicy::SELECTED_DIRECT);

        assert_eq!(
            selected.per_light_cells[0],
            vec![*cube.per_light_cells[0].last().unwrap()]
        );
    }

    #[test]
    fn directional_direct_reach_retains_the_whole_world_grid() {
        let verts = cube_vertices();
        let exterior: HashSet<usize> = HashSet::new();
        let tree = empty_tree();
        let reach = AffinityReachInputs {
            geometry_vertices: &verts,
            tree: &tree,
            exterior_leaves: &exterior,
            portals: &[],
            probe_spacing: 1.0,
        };
        let mut light = animated_point_light(DVec3::ZERO, 1.0);
        light.light_type = LightType::Directional;
        light.cone_direction = Some([0.0, -1.0, 0.0]);
        let lights = [&light];

        let direct = decompose_affinity_for_lights(&reach, &lights, AffinityReachPolicy::DIRECT);
        assert_eq!(
            direct.per_light_cells[0].len(),
            direct.affinity_cell_count()
        );
    }

    // --- CSR inversion -----------------------------------------------------
    //
    // Shared by `delta_sh_bake` (animated-light path) and `direct_sh_bake`
    // (static-direct path); exercised here once against the shared
    // implementation rather than per caller.

    #[test]
    fn build_csr_inverts_per_light_cells_into_cell_keyed_offsets() {
        // 3 cells, 3 lights:
        //   light 0 → cells {0, 2}
        //   light 1 → cells {2}
        //   light 2 → cells {0}
        // Expected per cell: cell 0 → [0, 2], cell 1 → [], cell 2 → [0, 1].
        let per_light_cells = vec![vec![0u32, 2], vec![2], vec![0]];
        let (offsets, lights) = build_csr(&per_light_cells, 3);

        // Offsets monotonic non-decreasing, length cell_count + 1, last == len.
        assert_eq!(offsets.len(), 4);
        assert!(offsets.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(*offsets.last().unwrap() as usize, lights.len());

        // cell 0 → lights [0, 2], cell 1 → empty, cell 2 → lights [0, 1].
        let slice = |c: usize| &lights[offsets[c] as usize..offsets[c + 1] as usize];
        assert_eq!(slice(0), &[0, 2]);
        assert_eq!(slice(1), &[] as &[u32]);
        assert_eq!(slice(2), &[0, 1]);
    }

    #[test]
    fn csr_entry_cells_expands_offsets_into_a_per_entry_cell_index() {
        // offsets: cell 0 has 2 entries, cell 1 has 0, cell 2 has 1.
        let offsets = vec![0u32, 2, 2, 3];
        let cells = csr_entry_cells(&offsets);
        assert_eq!(cells, vec![0, 0, 2]);
    }
}
