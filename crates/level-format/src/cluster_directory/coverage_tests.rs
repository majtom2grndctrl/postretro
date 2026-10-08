// Oracle equivalence for the id-49 range-table derivation: the coverage
// module against the original scan kept in `coverage_oracle`.
// See: context/lib/build_pipeline.md §PRL section IDs

use std::collections::BTreeMap;

use super::coverage_oracle::{oracle_derive_ranges_with_counts, oracle_validate_range_table};
use super::*;
use crate::{
    bvh::BvhSection,
    cell_locator::{CellLocatorChild, CellLocatorNodeRecord, CellLocatorSection},
    cells::{CELL_FLAG_DRAWABLE, CELL_FLAG_EXTERIOR, CELL_FLAG_SOLID, CellRecord, CellsSection},
    delta_sh_volumes::DeltaShVolumesSection,
    portals::PortalsSection,
    sh_volume::{OCTAHEDRAL_PROBE_STRIDE, OctahedralShProbe, OctahedralShVolumeSection},
};

/// Everything the derivation reads, owned so cases can be generated.
struct Case {
    directory: ClusterDirectorySection,
    cells: CellsSection,
    locator: CellLocatorSection,
    base: OctahedralShVolumeSection,
    sparse: Option<DeltaShVolumesSection>,
}

/// Which way both derivations went, for generator-reach accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    Ranges,
    NoRanges,
    LocatorFault,
    ScaleFault,
    NodeBoundFault,
    OtherFault,
}

impl Case {
    fn with_inputs<R>(&self, run: impl FnOnce(ClusterDirectoryValidationInputs<'_>) -> R) -> R {
        let portals = PortalsSection {
            vertices: Vec::new(),
            portals: Vec::new(),
        };
        let bvh = BvhSection {
            nodes: Vec::new(),
            leaves: Vec::new(),
            root_node_index: 0,
        };
        run(ClusterDirectoryValidationInputs {
            cells: &self.cells,
            portals: &portals,
            bvh: &bvh,
            cell_locator: &self.locator,
            sh: ClusterDirectoryShInventory {
                octahedral: Some(&self.base),
                delta: self.sparse.as_ref(),
                ..Default::default()
            },
        })
    }

    /// Assert both derivations and both range-table checks agree exactly,
    /// and classify the shared outcome.
    fn assert_equivalent(&self) -> Verdict {
        let Ok(affinity_dims) = affinity_dimensions(self.base.grid_dimensions) else {
            return Verdict::OtherFault;
        };
        self.with_inputs(|inputs| {
            let new = coverage::derive_ranges_with_counts(
                &self.directory,
                inputs,
                &self.base,
                affinity_dims,
            );
            let old = oracle_derive_ranges_with_counts(
                &self.directory,
                inputs,
                &self.base,
                affinity_dims,
            );
            assert_eq!(new, old, "derivation diverged from the oracle");
            assert_eq!(
                validate_range_table(&self.directory, inputs, &self.base, affinity_dims),
                oracle_validate_range_table(&self.directory, inputs, &self.base, affinity_dims),
                "range-table check diverged from the oracle"
            );
            match new {
                Ok((ranges, _, _)) if ranges.is_empty() => Verdict::NoRanges,
                Ok(_) => Verdict::Ranges,
                Err(ClusterDirectoryError::InvalidData(message))
                    if message.starts_with("cell locator") =>
                {
                    Verdict::LocatorFault
                }
                Err(ClusterDirectoryError::ResourceMismatch(message))
                    if message.contains("node_scale") =>
                {
                    Verdict::ScaleFault
                }
                Err(ClusterDirectoryError::ResourceMismatch(message))
                    if message.contains("adaptive node") =>
                {
                    Verdict::NodeBoundFault
                }
                Err(_) => Verdict::OtherFault,
            }
        })
    }

    /// Bake this case's range table with the oracle, as the compiler did
    /// before the coverage module existed.
    fn bake_ranges_with_oracle(&mut self) {
        let affinity_dims = affinity_dimensions(self.base.grid_dimensions).unwrap();
        let (ranges, counts, _) = self
            .with_inputs(|inputs| {
                oracle_derive_ranges_with_counts(&self.directory, inputs, &self.base, affinity_dims)
            })
            .unwrap();
        let mut start = 0;
        for (cluster, count) in self.directory.clusters.iter_mut().zip(counts) {
            cluster.range_start = if count == 0 { 0 } else { start };
            cluster.range_count = count;
            start += count;
        }
        self.directory.ranges = ranges;
    }
}

/// xorshift64*: deterministic, dependency-free case generation.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: u32) -> u32 {
        (self.next() % u64::from(bound.max(1))) as u32
    }

    fn chance(&mut self, percent: u32) -> bool {
        self.below(100) < percent
    }

    fn pick<T: Copy>(&mut self, values: &[T]) -> T {
        values[self.below(values.len() as u32) as usize]
    }
}

fn probe(validity: u8, node_scale: u8) -> OctahedralShProbe {
    OctahedralShProbe {
        validity,
        node_scale,
        ..Default::default()
    }
}

fn base_volume(
    grid_dimensions: [u32; 3],
    grid_origin: [f32; 3],
    cell_size: [f32; 3],
    probes: Vec<OctahedralShProbe>,
) -> OctahedralShVolumeSection {
    OctahedralShVolumeSection {
        grid_origin,
        cell_size,
        grid_dimensions,
        probe_stride: OCTAHEDRAL_PROBE_STRIDE,
        tile_dimension: 6,
        tile_border: 1,
        atlas_dimensions: [0, 0],
        layer_count: 0,
        tiles_per_layer: 0,
        atlas_tiles_per_row: 0,
        probes,
        irradiance_format: 1,
        compact_atlas: Vec::new(),
        animation_descriptors: Vec::new(),
        slot_for_map_light: Vec::new(),
    }
}

fn cell(bounds_min: [f32; 3], bounds_max: [f32; 3], flags: u32) -> CellRecord {
    CellRecord {
        bounds_min,
        bounds_max,
        flags,
        face_start: 0,
        face_count: 0,
        portal_ref_start: 0,
        portal_ref_count: 0,
    }
}

fn sparse_section(affinity_dims: [u32; 3], offsets: Vec<u32>) -> DeltaShVolumesSection {
    let count = offsets.len() - 1;
    DeltaShVolumesSection {
        affinity_factor: 4,
        affinity_dims,
        tile_dimension: 6,
        tile_border: 1,
        animation_descriptor_indices: Vec::new(),
        valid_probe_masks: vec![0; count],
        cell_levels: vec![0; count],
        affinity_offsets: offsets,
        affinity_lights: Vec::new(),
        delta_subblocks: Vec::new(),
    }
}

/// Directory over `cell_count` cells with the given cluster member lists and
/// resource rows; ranges left empty.
fn directory(
    cell_count: u32,
    clusters: &[Vec<u32>],
    resources: &[ClusterResourceDomain],
    affinity_dims: [u32; 3],
    grid: [u32; 3],
) -> ClusterDirectorySection {
    let mut members = Vec::new();
    let records = clusters
        .iter()
        .map(|cluster| {
            let member_start = members.len() as u32;
            members.extend_from_slice(cluster);
            ClusterRecord {
                bounds_min: [0.0; 3],
                bounds_max: [0.0; 3],
                member_start,
                member_count: cluster.len() as u32,
                range_start: 0,
                range_count: 0,
                primitive_count: 0,
                flags: 0,
            }
        })
        .collect();
    ClusterDirectorySection {
        runtime_cell_count: cell_count,
        primitive_limit: 64,
        cell_limit: 32,
        clusters: records,
        resources: resources
            .iter()
            .map(|&domain| ClusterResourceRecord {
                section_id: if domain == ClusterResourceDomain::DenseProbe {
                    34
                } else {
                    27
                },
                domain,
                dimensions: if domain == ClusterResourceDomain::DenseProbe {
                    grid
                } else {
                    affinity_dims
                },
            })
            .collect(),
        members,
        ranges: Vec::new(),
        seam_portal_ids: Vec::new(),
        cluster_hints: Vec::new(),
    }
}

/// World-space lower support bound of brick coordinate `brick` on `axis`:
/// the original expression.
fn support_min(base: &OctahedralShVolumeSection, axis: usize, brick: u32) -> f32 {
    let center = base.grid_origin[axis] + (brick * 4) as f32 * base.cell_size[axis];
    center - 0.5 * base.cell_size[axis]
}

fn support_max(base: &OctahedralShVolumeSection, axis: usize, brick: u32) -> f32 {
    let probe = (brick * 4 + 3).min(base.grid_dimensions[axis] - 1);
    let center = base.grid_origin[axis] + probe as f32 * base.cell_size[axis];
    center + 0.5 * base.cell_size[axis]
}

fn probe_world(base: &OctahedralShVolumeSection, axis: usize, coord: u32) -> f32 {
    base.grid_origin[axis] + coord as f32 * base.cell_size[axis]
}

/// One random cell bound pair on `axis`, biased toward exact brick-support
/// contact, one-ulp misses, zero volume, and the outside of the grid.
fn random_bounds(rng: &mut Rng, base: &OctahedralShVolumeSection, axis: usize) -> (f32, f32) {
    let size = base.cell_size[axis];
    let bricks = base.grid_dimensions[axis].div_ceil(4).max(1);
    let extent = base.grid_dimensions[axis] as f32 * size;
    let origin = base.grid_origin[axis];
    let quantum = size * 0.25;
    let anywhere = |rng: &mut Rng| {
        origin - 2.0 * size
            + rng.below(((extent + 4.0 * size) / quantum) as u32 + 1) as f32 * quantum
    };
    match rng.below(10) {
        // Expanded max lands exactly on a brick's support minimum.
        0 => {
            let touch = support_min(base, axis, rng.below(bricks)) - size;
            (touch - rng.below(3) as f32 * size, touch)
        }
        // Expanded min lands exactly on a brick's support maximum.
        1 => {
            let touch = support_max(base, axis, rng.below(bricks)) + size;
            (touch, touch + rng.below(3) as f32 * size)
        }
        // One ulp short of contact on either side.
        2 => {
            let touch = (support_min(base, axis, rng.below(bricks)) - size).next_down();
            (touch - size, touch)
        }
        3 => {
            let touch = (support_max(base, axis, rng.below(bricks)) + size).next_up();
            (touch, touch + size)
        }
        // Zero volume, on a probe or a contact plane.
        4 => {
            let point = if rng.chance(50) {
                probe_world(base, axis, rng.below(base.grid_dimensions[axis].max(1)))
            } else {
                support_min(base, axis, rng.below(bricks)) - size
            };
            (point, point)
        }
        // Entirely outside the grid.
        5 => {
            if rng.chance(50) {
                let high = origin - 3.0 * size;
                (high - size, high)
            } else {
                let low = origin + extent + 3.0 * size;
                (low, low + size)
            }
        }
        // Inverted.
        6 => {
            let a = anywhere(rng);
            (a + size, a)
        }
        _ => {
            let a = anywhere(rng);
            let b = anywhere(rng);
            (a.min(b), a.max(b))
        }
    }
}

fn random_cells(
    rng: &mut Rng,
    base: &OctahedralShVolumeSection,
    count: u32,
    hostile: bool,
) -> CellsSection {
    let cells = (0..count)
        .map(|_| {
            let mut bounds_min = [0.0; 3];
            let mut bounds_max = [0.0; 3];
            // A shared draw on every axis makes corner-only contact.
            let corner = rng.chance(15);
            let corner_kind = rng.below(4);
            for axis in 0..3 {
                let (low, high) = if corner {
                    let bricks = base.grid_dimensions[axis].div_ceil(4).max(1);
                    let size = base.cell_size[axis];
                    match corner_kind {
                        0 => {
                            let touch = support_min(base, axis, rng.below(bricks)) - size;
                            (touch - size, touch)
                        }
                        1 => {
                            let touch = support_max(base, axis, rng.below(bricks)) + size;
                            (touch, touch + size)
                        }
                        2 => {
                            let touch =
                                (support_min(base, axis, rng.below(bricks)) - size).next_down();
                            (touch - size, touch)
                        }
                        _ => {
                            let touch =
                                (support_max(base, axis, rng.below(bricks)) + size).next_up();
                            (touch, touch + size)
                        }
                    }
                } else {
                    random_bounds(rng, base, axis)
                };
                bounds_min[axis] = low;
                bounds_max[axis] = high;
            }
            if hostile && rng.chance(5) {
                let axis = rng.below(3) as usize;
                let value = rng.pick(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.0]);
                if rng.chance(50) {
                    bounds_min[axis] = value;
                } else {
                    bounds_max[axis] = value;
                }
            }
            let flags = match rng.below(10) {
                0 => CELL_FLAG_SOLID,
                1 => CELL_FLAG_EXTERIOR | CELL_FLAG_DRAWABLE,
                _ => CELL_FLAG_DRAWABLE,
            };
            cell(bounds_min, bounds_max, flags)
        })
        .collect();
    CellsSection {
        cells,
        portal_refs: Vec::new(),
    }
}

/// A random locator tree. Planes often pass exactly through probe rows or
/// cut a brick; hostile trees add faults, cycles and non-finite planes.
fn random_locator(
    rng: &mut Rng,
    base: &OctahedralShVolumeSection,
    cell_count: u32,
    hostile: bool,
) -> CellLocatorSection {
    let node_count = rng.below(14);
    let grid = base.grid_dimensions;
    let terminal = |rng: &mut Rng| {
        if hostile && rng.chance(4) {
            CellLocatorChild::Cell(cell_count + rng.below(3))
        } else {
            CellLocatorChild::Cell(rng.below(cell_count))
        }
    };
    let nodes = (0..node_count)
        .map(|index| {
            let axis = rng.below(3) as usize;
            let mut plane_normal = [0.0f32; 3];
            let mut plane_distance;
            match rng.below(6) {
                // Axis plane exactly through a probe row: signed distance 0.
                0 | 1 => {
                    let sign = rng.pick(&[1.0f32, -1.0]);
                    plane_normal[axis] = sign;
                    plane_distance = sign * probe_world(base, axis, rng.below(grid[axis].max(1)));
                }
                // Axis plane between probes.
                2 => {
                    plane_normal[axis] = 1.0;
                    plane_distance = probe_world(base, axis, rng.below(grid[axis].max(1)))
                        + 0.5 * base.cell_size[axis];
                }
                // Oblique, dyadic.
                3 | 4 => {
                    for value in &mut plane_normal {
                        *value = rng.pick(&[-1.0, -0.5, -0.25, 0.0, 0.25, 0.5, 1.0, 0.1, -0.3]);
                    }
                    let point: [f32; 3] = std::array::from_fn(|axis| {
                        probe_world(base, axis, rng.below(grid[axis].max(1)))
                    });
                    plane_distance = plane_normal[0] * point[0]
                        + plane_normal[1] * point[1]
                        + plane_normal[2] * point[2];
                }
                _ => {
                    plane_normal = [1.0, 0.0, 0.0];
                    plane_distance = rng.pick(&[-1e6f32, 1e6, 0.0, -0.0]);
                }
            }
            if hostile && rng.chance(8) {
                match rng.below(4) {
                    0 => plane_normal[axis] = f32::NAN,
                    1 => plane_normal[axis] = 3e38,
                    2 => plane_distance = f32::INFINITY,
                    _ => plane_distance = f32::NAN,
                }
            }
            let child = |rng: &mut Rng| {
                if hostile && rng.chance(4) {
                    // Cycle or missing node.
                    CellLocatorChild::Node(rng.below(node_count + 2))
                } else if index + 1 < node_count && rng.chance(60) {
                    CellLocatorChild::Node(index + 1 + rng.below(node_count - index - 1))
                } else {
                    terminal(rng)
                }
            };
            let front = child(rng);
            let back = child(rng);
            CellLocatorNodeRecord {
                plane_normal,
                plane_distance,
                front,
                back,
            }
        })
        .collect();
    let root = if node_count > 0 && rng.chance(85) {
        CellLocatorChild::Node(0)
    } else {
        terminal(rng)
    };
    CellLocatorSection { root, nodes }
}

fn random_case(seed: u64, hostile: bool) -> Case {
    let mut rng = Rng::new(seed);
    let grid: [u32; 3] = std::array::from_fn(|_| {
        let limit = if rng.chance(20) { 24 } else { 13 };
        1 + rng.below(limit)
    });
    let grid_origin: [f32; 3] =
        std::array::from_fn(|_| rng.pick(&[0.0f32, -3.25, 0.5, -0.0, 1.0e-3, 17.0]));
    let cell_size: [f32; 3] = std::array::from_fn(|_| rng.pick(&[1.0f32, 0.5, 2.0, 0.75, 0.1]));
    let affinity_dims = affinity_dimensions(grid).unwrap();
    let brick_count = affinity_dims.iter().product::<u32>();
    let validity_percent = rng.pick(&[0, 5, 30, 70, 100]);
    let scaled = rng.chance(30);
    let mut brick_scale = vec![0u8; brick_count as usize];
    if scaled {
        for scale in &mut brick_scale {
            *scale = rng.pick(&[0, 0, 0, 1, 1, 2, 3]);
        }
    }
    let probe_count = grid[0] * grid[1] * grid[2];
    let bad_scale_probe = (hostile && rng.chance(15)).then(|| rng.below(probe_count));
    let mut probes = Vec::with_capacity(probe_count as usize);
    for z in 0..grid[2] {
        for y in 0..grid[1] {
            for x in 0..grid[0] {
                let brick = x / 4 + affinity_dims[0] * (y / 4 + affinity_dims[1] * (z / 4));
                let mut scale = brick_scale[brick as usize];
                if bad_scale_probe == Some(probes.len() as u32) {
                    scale = 4 + rng.below(3) as u8;
                }
                probes.push(probe(u8::from(rng.chance(validity_percent)), scale));
            }
        }
    }
    let mut base = base_volume(grid, grid_origin, cell_size, probes);
    if hostile && rng.chance(5) {
        let axis = rng.below(3) as usize;
        base.grid_origin[axis] = rng.pick(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY]);
    }
    let cell_count = 1 + rng.below(12);
    let cells = random_cells(&mut rng, &base, cell_count, hostile);
    let locator = random_locator(&mut rng, &base, cell_count, hostile);
    if hostile && rng.chance(2) {
        base.cell_size[rng.below(3) as usize] = rng.pick(&[0.0, -1.0, f32::NAN]);
    }
    let cluster_count = 1 + rng.below(4);
    let mut clusters = vec![Vec::new(); cluster_count as usize];
    for cell_id in 0..cell_count {
        clusters[rng.below(cluster_count) as usize].push(cell_id);
    }
    // Duplicate members and cells shared between clusters.
    if rng.chance(20) {
        let cluster = rng.below(cluster_count) as usize;
        let member = rng.below(cell_count);
        clusters[cluster].push(member);
        clusters[cluster].push(member);
    }
    if rng.chance(20) {
        clusters[rng.below(cluster_count) as usize].push(rng.below(cell_count));
    }
    let resource_rows = rng.below(5);
    let resources: Vec<_> = (0..resource_rows)
        .map(|_| {
            rng.pick(&[
                ClusterResourceDomain::DenseProbe,
                ClusterResourceDomain::AffinityCell,
            ])
        })
        .collect();
    let sparse = rng.chance(50).then(|| {
        let mut offsets = vec![0u32];
        let mut total = 0;
        for _ in 0..brick_count {
            if rng.chance(15) {
                total += 1 + rng.below(3);
            }
            offsets.push(total);
        }
        sparse_section(affinity_dims, offsets)
    });
    Case {
        directory: directory(cell_count, &clusters, &resources, affinity_dims, grid),
        cells,
        locator,
        base,
        sparse,
    }
}

#[test]
fn coverage_derivation_matches_oracle_on_random_directories() {
    let mut verdicts = BTreeMap::new();
    for seed in 0..3000 {
        let verdict = random_case(seed, false).assert_equivalent();
        *verdicts.entry(verdict).or_insert(0u32) += 1;
    }
    assert!(
        verdicts.get(&Verdict::Ranges).copied().unwrap_or(0) > 1000,
        "{verdicts:?}"
    );
    assert!(
        verdicts.get(&Verdict::NodeBoundFault).copied().unwrap_or(0) > 20,
        "{verdicts:?}"
    );
}

#[test]
fn coverage_derivation_matches_oracle_on_hostile_directories() {
    let mut verdicts = BTreeMap::new();
    for seed in 0..3000 {
        let verdict = random_case(seed ^ 0xA5A5_0000, true).assert_equivalent();
        *verdicts.entry(verdict).or_insert(0u32) += 1;
    }
    for verdict in [
        Verdict::Ranges,
        Verdict::LocatorFault,
        Verdict::ScaleFault,
        Verdict::NodeBoundFault,
    ] {
        assert!(
            verdicts.get(&verdict).copied().unwrap_or(0) > 20,
            "{verdicts:?}"
        );
    }
}

#[test]
fn coverage_matches_oracle_when_baked_table_differs_in_one_range() {
    let mut mutated = 0;
    for seed in 0..1500 {
        let mut case = random_case(seed ^ 0x5EED_0000, false);
        if case.assert_equivalent() != Verdict::Ranges {
            continue;
        }
        case.bake_ranges_with_oracle();
        case.with_inputs(|inputs| {
            let dims = affinity_dimensions(case.base.grid_dimensions).unwrap();
            assert_eq!(
                validate_range_table(&case.directory, inputs, &case.base, dims),
                Ok(())
            );
        });
        let mut rng = Rng::new(seed);
        let index = rng.below(case.directory.ranges.len() as u32) as usize;
        let range = &mut case.directory.ranges[index];
        match rng.below(5) {
            0 => range.start ^= 1,
            1 => range.count += 1,
            2 => range.owner_cluster_id = range.owner_cluster_id.wrapping_add(1),
            3 => {
                range.role = match range.role {
                    ClusterRangeRole::Owned => ClusterRangeRole::Halo,
                    ClusterRangeRole::Halo => ClusterRangeRole::Dense,
                    ClusterRangeRole::Dense => ClusterRangeRole::Owned,
                }
            }
            _ => range.resource_index += 1,
        }
        case.with_inputs(|inputs| {
            let dims = affinity_dimensions(case.base.grid_dimensions).unwrap();
            let new = validate_range_table(&case.directory, inputs, &case.base, dims);
            assert!(new.is_err());
            assert_eq!(
                new,
                oracle_validate_range_table(&case.directory, inputs, &case.base, dims)
            );
        });
        // A slice-length fault wins over the table comparison in both.
        let cluster = rng.below(case.directory.clusters.len() as u32) as usize;
        case.directory.clusters[cluster].range_count += 1;
        case.assert_equivalent();
        mutated += 1;
    }
    assert!(mutated > 300, "{mutated}");
}

/// A 12×4×4 grid (bricks x = 0, 1, 2 with x supports [-0.5, 3.5],
/// [3.5, 7.5], [7.5, 11.5]; y and z [-0.5, 3.5]), every probe valid. Every
/// probe locates to solid cell 1 in cluster 1, so cluster 0 covers a brick
/// only through the bounds of its one cell, cell 0.
fn single_cell_case(bounds_min: [f32; 3], bounds_max: [f32; 3]) -> Case {
    let grid = [12, 4, 4];
    Case {
        directory: directory(
            2,
            &[vec![0], vec![1]],
            &[
                ClusterResourceDomain::AffinityCell,
                ClusterResourceDomain::DenseProbe,
            ],
            [3, 1, 1],
            grid,
        ),
        cells: CellsSection {
            cells: vec![
                cell(bounds_min, bounds_max, CELL_FLAG_DRAWABLE),
                cell([0.0; 3], [0.0; 3], CELL_FLAG_SOLID),
            ],
            portal_refs: Vec::new(),
        },
        locator: CellLocatorSection {
            root: CellLocatorChild::Cell(1),
            nodes: Vec::new(),
        },
        base: base_volume(grid, [0.0; 3], [1.0; 3], vec![probe(1, 0); 192]),
        sparse: None,
    }
}

/// Bricks cluster 0 covers, from its affinity ranges.
fn covered_bricks(case: &Case) -> Vec<u32> {
    assert_eq!(case.assert_equivalent(), Verdict::Ranges);
    let dims = affinity_dimensions(case.base.grid_dimensions).unwrap();
    let (ranges, counts, _) = case
        .with_inputs(|inputs| {
            coverage::derive_ranges_with_counts(&case.directory, inputs, &case.base, dims)
        })
        .unwrap();
    ranges[..counts[0] as usize]
        .iter()
        .filter(|range| range.resource_index == 0)
        .flat_map(|range| range.start..range.start + range.count)
        .collect()
}

#[test]
fn coverage_pins_exact_boundary_contact() {
    // The cell expands by one probe spacing (1.0) before the test. Expanded
    // max exactly on brick 1's support min (3.5) covers brick 1; one ulp
    // below misses it.
    assert_eq!(
        covered_bricks(&single_cell_case([0.0; 3], [2.5, 3.0, 3.0])),
        vec![0, 1]
    );
    assert_eq!(
        covered_bricks(&single_cell_case([0.0; 3], [2.5f32.next_down(), 3.0, 3.0])),
        vec![0]
    );
    // Expanded min exactly on brick 0's support max (3.5) covers brick 0.
    assert_eq!(
        covered_bricks(&single_cell_case([4.5, 0.0, 0.0], [5.0, 3.0, 3.0])),
        vec![0, 1]
    );
    assert_eq!(
        covered_bricks(&single_cell_case(
            [4.5f32.next_up(), 0.0, 0.0],
            [5.0, 3.0, 3.0]
        )),
        vec![1]
    );
    // Corner contact: brick 1's support minimum corner, on every axis at once.
    assert_eq!(
        covered_bricks(&single_cell_case([-1.0, -2.5, -2.5], [2.5, -1.5, -1.5])),
        vec![0, 1]
    );
    // Zero volume, exactly on the shared face of bricks 1 and 2.
    assert_eq!(
        covered_bricks(&single_cell_case([8.5, 1.0, 1.0], [8.5, 1.0, 1.0])),
        vec![1, 2]
    );
    for bounds in [
        ([2.5, 2.5, 2.5], [2.5, 2.5, 2.5]),
        ([-9.0, -9.0, -9.0], [-8.0, -8.0, -8.0]),
        ([50.0, 0.0, 0.0], [60.0, 3.0, 3.0]),
        ([3.0, 0.0, 0.0], [1.0, 3.0, 3.0]),
        ([f32::NAN, 0.0, 0.0], [3.0, 3.0, 3.0]),
        ([f32::NEG_INFINITY, 0.0, 0.0], [f32::INFINITY, 3.0, 3.0]),
    ] {
        single_cell_case(bounds.0, bounds.1).assert_equivalent();
    }
}

/// Every probe valid, two cells split by a locator plane, and a second
/// cluster whose only cell touches a brick's support at one face or corner.
#[test]
fn coverage_pins_face_and_corner_contact_between_clusters() {
    let grid = [8, 8, 8];
    let affinity_dims = [2, 2, 2];
    let base = base_volume(grid, [0.0; 3], [1.0; 3], vec![probe(1, 0); 512]);
    // Brick (0,0,0) supports [-0.5, 3.5] per axis; a cell expanded by 1.0
    // whose min is 4.5 touches it exactly at x = 3.5.
    let contact_cases = [
        ([4.5, 0.0, 0.0], [6.0, 3.0, 3.0], true),
        ([4.5, 4.5, 4.5], [6.0, 6.0, 6.0], true),
        ([4.5f32.next_up(), 4.5, 4.5], [6.0, 6.0, 6.0], false),
        ([4.5, 4.5, 4.5], [4.5, 4.5, 4.5], true),
    ];
    for (bounds_min, bounds_max, touches_brick_zero) in contact_cases {
        let case = Case {
            directory: directory(
                3,
                &[vec![0, 1], vec![2], Vec::new()],
                &[ClusterResourceDomain::AffinityCell],
                affinity_dims,
                grid,
            ),
            cells: CellsSection {
                cells: vec![
                    cell([0.0; 3], [3.5, 7.0, 7.0], CELL_FLAG_DRAWABLE),
                    cell([3.5, 0.0, 0.0], [7.0, 7.0, 7.0], CELL_FLAG_DRAWABLE),
                    cell(bounds_min, bounds_max, CELL_FLAG_DRAWABLE),
                ],
                portal_refs: Vec::new(),
            },
            locator: CellLocatorSection {
                root: CellLocatorChild::Node(0),
                nodes: vec![CellLocatorNodeRecord {
                    plane_normal: [1.0, 0.0, 0.0],
                    plane_distance: 4.0,
                    front: CellLocatorChild::Cell(1),
                    back: CellLocatorChild::Cell(0),
                }],
            },
            base: base.clone(),
            sparse: None,
        };
        assert_eq!(case.assert_equivalent(), Verdict::Ranges);
        let (ranges, counts, _) = case
            .with_inputs(|inputs| {
                coverage::derive_ranges_with_counts(
                    &case.directory,
                    inputs,
                    &case.base,
                    affinity_dims,
                )
            })
            .unwrap();
        let second_cluster = &ranges[counts[0] as usize..(counts[0] + counts[1]) as usize];
        let covers_brick_zero = second_cluster.iter().any(|range| range.start == 0);
        assert_eq!(covers_brick_zero, touches_brick_zero, "{bounds_min:?}");
        assert_eq!(counts[2], 0, "empty cluster has no ranges");
    }
}

/// A plane exactly through a probe row splits a brick: the row on the plane
/// goes front (`>= 0`), so both cells cover the brick.
#[test]
fn coverage_pins_locator_plane_through_probe_row() {
    let grid = [4, 4, 4];
    for (normal, distance) in [
        ([1.0f32, 0.0, 0.0], 2.0f32),
        ([-1.0, 0.0, 0.0], -2.0),
        ([0.0, 0.0, 1.0], 3.0),
        ([0.5, 0.25, 0.25], 1.0),
        ([1.0, 0.0, 0.0], 3.0f32.next_up()),
        ([3e38, 0.0, 0.0], 1.0),
        ([f32::NAN, 0.0, 0.0], 1.0),
    ] {
        let case = Case {
            directory: directory(
                2,
                &[vec![0], vec![1]],
                &[
                    ClusterResourceDomain::DenseProbe,
                    ClusterResourceDomain::AffinityCell,
                ],
                [1, 1, 1],
                grid,
            ),
            cells: CellsSection {
                cells: vec![
                    cell([0.0; 3], [0.0; 3], CELL_FLAG_SOLID),
                    cell([0.0; 3], [0.0; 3], CELL_FLAG_SOLID),
                ],
                portal_refs: Vec::new(),
            },
            locator: CellLocatorSection {
                root: CellLocatorChild::Node(0),
                nodes: vec![CellLocatorNodeRecord {
                    plane_normal: normal,
                    plane_distance: distance,
                    front: CellLocatorChild::Cell(1),
                    back: CellLocatorChild::Cell(0),
                }],
            },
            base: base_volume(grid, [0.0; 3], [1.0; 3], vec![probe(1, 0); 64]),
            sparse: None,
        };
        case.assert_equivalent();
    }
}

#[test]
fn coverage_reports_the_first_locator_fault_in_probe_order() {
    // Probes with x >= 2 reach a missing node; probes with x < 2 reach an
    // out-of-range cell. Probe 0 (x = 0) comes first, so the cell fault wins
    // even though the batched walk meets the node fault in the same brick.
    let grid = [4, 4, 4];
    let case = Case {
        directory: directory(
            1,
            &[vec![0]],
            &[ClusterResourceDomain::DenseProbe],
            [1, 1, 1],
            grid,
        ),
        cells: CellsSection {
            cells: vec![cell([0.0; 3], [0.0; 3], CELL_FLAG_SOLID)],
            portal_refs: Vec::new(),
        },
        locator: CellLocatorSection {
            root: CellLocatorChild::Node(0),
            nodes: vec![CellLocatorNodeRecord {
                plane_normal: [1.0, 0.0, 0.0],
                plane_distance: 2.0,
                front: CellLocatorChild::Node(7),
                back: CellLocatorChild::Cell(5),
            }],
        },
        base: base_volume(grid, [0.0; 3], [1.0; 3], vec![probe(1, 0); 64]),
        sparse: None,
    };
    assert_eq!(case.assert_equivalent(), Verdict::LocatorFault);
    let mut first_invalid = case;
    // With probe 0 invalid, probe 1 (x = 1) still reaches the cell fault first.
    first_invalid.base.probes[0].validity = 0;
    assert_eq!(first_invalid.assert_equivalent(), Verdict::LocatorFault);
    // A fault only invalid probes reach is not an error.
    for (index, probe) in first_invalid.base.probes.iter_mut().enumerate() {
        probe.validity = u8::from(index % 4 >= 2);
    }
    first_invalid.locator.nodes[0].front = CellLocatorChild::Cell(0);
    assert_eq!(first_invalid.assert_equivalent(), Verdict::Ranges);
}

#[test]
fn coverage_matches_oracle_on_a_large_lit_like_grid() {
    // A wider grid than the random sweep: many bricks, a deeper locator, so
    // whole-brick boxes resolve high in the tree and split near the leaves.
    for seed in 0..12 {
        let mut rng = Rng::new(seed + 900);
        let grid = [40 + rng.below(9), 28 + rng.below(9), 12 + rng.below(9)];
        let affinity_dims = affinity_dimensions(grid).unwrap();
        let probe_count = (grid[0] * grid[1] * grid[2]) as usize;
        let probes = (0..probe_count)
            .map(|_| probe(u8::from(rng.chance(80)), 0))
            .collect();
        let base = base_volume(grid, [-8.0, -4.0, 1.5], [0.5, 0.5, 0.75], probes);
        let cell_count = 40;
        let cells = random_cells(&mut rng, &base, cell_count, false);
        // A balanced kd-tree of axis planes through probe rows.
        let mut nodes = Vec::new();
        fn build(
            nodes: &mut Vec<CellLocatorNodeRecord>,
            rng: &mut Rng,
            base: &OctahedralShVolumeSection,
            depth: u32,
            cell_count: u32,
        ) -> CellLocatorChild {
            if depth == 0 {
                return CellLocatorChild::Cell(rng.below(cell_count));
            }
            let index = nodes.len();
            let axis = (depth % 3) as usize;
            nodes.push(CellLocatorNodeRecord {
                plane_normal: std::array::from_fn(|a| if a == axis { 1.0 } else { 0.0 }),
                plane_distance: probe_world(base, axis, rng.below(base.grid_dimensions[axis])),
                front: CellLocatorChild::Cell(0),
                back: CellLocatorChild::Cell(0),
            });
            let front = build(nodes, rng, base, depth - 1, cell_count);
            let back = build(nodes, rng, base, depth - 1, cell_count);
            nodes[index].front = front;
            nodes[index].back = back;
            CellLocatorChild::Node(index as u32)
        }
        let root = build(&mut nodes, &mut rng, &base, 9, cell_count);
        let clusters: Vec<Vec<u32>> = (0..cell_count)
            .collect::<Vec<_>>()
            .chunks(5)
            .map(<[u32]>::to_vec)
            .collect();
        let case = Case {
            directory: directory(
                cell_count,
                &clusters,
                &[
                    ClusterResourceDomain::AffinityCell,
                    ClusterResourceDomain::DenseProbe,
                    ClusterResourceDomain::DenseProbe,
                    ClusterResourceDomain::AffinityCell,
                ],
                affinity_dims,
                grid,
            ),
            cells,
            locator: CellLocatorSection { root, nodes },
            base,
            sparse: None,
        };
        assert_eq!(case.assert_equivalent(), Verdict::Ranges);
    }
}

/// The batched descent reports the same `(brick, cell)` set as locating every
/// valid probe one by one, and faults exactly when some valid probe's walk
/// does. Compared per cell, not per cluster, so a probe sent to the wrong cell
/// of its cluster still fails here.
#[test]
fn batched_locator_matches_the_per_probe_walk_cell_for_cell() {
    let mut outcomes = [0u32; 2];
    for (seed, hostile) in (0..1500u64)
        .map(|seed| (seed, false))
        .chain((0..1500).map(|seed| (seed ^ 0x10C0_0000, true)))
    {
        let case = random_case(seed, hostile);
        let Ok(affinity_dims) = affinity_dimensions(case.base.grid_dimensions) else {
            continue;
        };
        let grid = case.base.grid_dimensions;
        let mut expected = std::collections::BTreeSet::new();
        let mut faulted = false;
        for (index, probe) in case.base.probes.iter().enumerate() {
            if probe.validity == 0 {
                continue;
            }
            let coords = probe_coords(index as u32, grid);
            match locate_cell(
                &case.locator,
                case.directory.runtime_cell_count,
                probe_position(coords, &case.base),
            ) {
                Ok(cell) => {
                    let brick = flatten(coords.map(|coord| coord / 4), affinity_dims).unwrap();
                    expected.insert((brick, cell));
                }
                Err(_) => faulted = true,
            }
        }
        let mut located = Vec::new();
        let batched = super::probe_locate::ProbeLocator::new(
            &case.locator,
            case.directory.runtime_cell_count,
            &case.base,
        )
        .locate_grid(affinity_dims, &mut located);
        assert_eq!(batched.is_err(), faulted, "seed {seed}: fault verdict");
        if !faulted {
            let actual: std::collections::BTreeSet<_> = located.into_iter().collect();
            assert_eq!(actual, expected, "seed {seed}: located cells");
        }
        outcomes[usize::from(faulted)] += 1;
    }
    assert!(outcomes[0] > 1000 && outcomes[1] > 50, "{outcomes:?}");
}

// Regression: the batched locator recursed once per straddled node, so a valid
// near-linear locator chain overflowed the stack where the per-probe walk
// (and the original validation) succeeded.
#[test]
fn coverage_survives_a_deep_axis_plane_chain() {
    const PROBES: u32 = 200_000;
    let grid = [PROBES, 1, 1];
    let affinity_dims = affinity_dimensions(grid).unwrap();
    let mut probes = vec![probe(0, 0); PROBES as usize];
    probes[PROBES as usize - 1].validity = 1;
    // Node i cuts between probes i and i + 1: back is cell 0, front walks on.
    let node_count = PROBES - 1;
    let nodes = (0..node_count)
        .map(|index| CellLocatorNodeRecord {
            plane_normal: [1.0, 0.0, 0.0],
            plane_distance: index as f32 + 0.5,
            front: if index + 1 < node_count {
                CellLocatorChild::Node(index + 1)
            } else {
                CellLocatorChild::Cell(1)
            },
            back: CellLocatorChild::Cell(0),
        })
        .collect();
    let case = Case {
        directory: directory(
            2,
            &[vec![0], vec![1]],
            &[
                ClusterResourceDomain::DenseProbe,
                ClusterResourceDomain::AffinityCell,
            ],
            affinity_dims,
            grid,
        ),
        cells: CellsSection {
            cells: vec![
                cell([0.0; 3], [0.0; 3], CELL_FLAG_SOLID),
                cell([0.0; 3], [0.0; 3], CELL_FLAG_SOLID),
            ],
            portal_refs: Vec::new(),
        },
        locator: CellLocatorSection {
            root: CellLocatorChild::Node(0),
            nodes,
        },
        base: base_volume(grid, [0.0; 3], [1.0; 3], probes),
        sparse: None,
    };
    assert_eq!(case.assert_equivalent(), Verdict::Ranges);
}
