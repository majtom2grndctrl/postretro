//! Per-camera-cell mandatory sets and their resident bytes.
//!
//! Texels belong to their receiver cell, so the mandatory set needs no owner
//! closure: the camera cell's own cluster, every pinned cluster, and every
//! cell coupled to the camera cell within the distance bound (or, unbounded,
//! its whole reachability component).

use postretro_level_format::cell_visibility::{
    CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE, CoupledPairRecord,
};

use super::DryRunInput;
use super::layouts::Layout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DistanceBound {
    Meters(u32),
    /// Reachability only: the camera cell's whole id-46 component.
    Unbounded,
}

impl DistanceBound {
    pub(crate) const ALL: [DistanceBound; 5] = [
        DistanceBound::Meters(16),
        DistanceBound::Meters(32),
        DistanceBound::Meters(64),
        DistanceBound::Meters(128),
        DistanceBound::Unbounded,
    ];

    /// Largest bounded distance, in id-46 fixed point.
    pub(crate) fn max_fixed() -> u32 {
        Self::ALL
            .iter()
            .filter_map(|bound| bound.fixed())
            .max()
            .unwrap_or(0)
    }

    /// Inclusive bound in id-46 fixed-point units.
    pub(crate) fn fixed(self) -> Option<u32> {
        match self {
            DistanceBound::Meters(meters) => {
                Some(meters * CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE)
            }
            DistanceBound::Unbounded => None,
        }
    }

    pub(crate) fn label(self) -> String {
        match self {
            DistanceBound::Meters(meters) => format!("{meters}m"),
            DistanceBound::Unbounded => "reach".to_string(),
        }
    }
}

/// Coupled partners of each cell with their fixed-point distance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Neighbors {
    per_cell: Vec<Vec<(u32, u32)>>,
}

impl Neighbors {
    pub(crate) fn from_pairs(cell_count: usize, pairs: &[CoupledPairRecord]) -> Self {
        let mut per_cell = vec![Vec::new(); cell_count];
        for pair in pairs {
            per_cell[pair.cell_a as usize].push((pair.cell_b, pair.distance));
            per_cell[pair.cell_b as usize].push((pair.cell_a, pair.distance));
        }
        for partners in &mut per_cell {
            partners.sort_unstable_by_key(|&(cell, distance)| (distance, cell));
        }
        Self { per_cell }
    }

    /// Partners within `fixed` (inclusive), nearest first.
    pub(crate) fn within(&self, cell: u32, fixed: u32) -> impl Iterator<Item = u32> + '_ {
        self.within_distances(cell, fixed).map(|(other, _)| other)
    }

    /// Partners within `fixed` (inclusive) with their fixed-point distance,
    /// nearest first.
    pub(crate) fn within_distances(
        &self,
        cell: u32,
        fixed: u32,
    ) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.per_cell[cell as usize]
            .iter()
            .take_while(move |&&(_, distance)| distance <= fixed)
            .copied()
    }
}

/// Precomputed membership the set builder reads for every camera cell.
pub(crate) struct MandatoryContext {
    cluster_members: Vec<Vec<u32>>,
    pinned_cells: Vec<u32>,
    component_members: Vec<Vec<u32>>,
    cell_cluster: Vec<u32>,
    component_ids: Vec<u32>,
    stamp: Vec<u32>,
    cluster_stamp: Vec<u32>,
    generation: u32,
}

impl MandatoryContext {
    pub(crate) fn new(input: &DryRunInput) -> Self {
        let cluster_members = input.cluster_members();
        let mut pinned_cells: Vec<u32> = input
            .pinned_clusters
            .iter()
            .flat_map(|&cluster| cluster_members[cluster as usize].iter().copied())
            .collect();
        pinned_cells.sort_unstable();
        let component_count = input
            .component_ids
            .iter()
            .max()
            .map_or(0, |&max| max as usize + 1);
        let mut component_members = vec![Vec::new(); component_count];
        for (cell, &component) in input.component_ids.iter().enumerate() {
            component_members[component as usize].push(cell as u32);
        }
        Self {
            cluster_members,
            pinned_cells,
            component_members,
            cell_cluster: input.cells.iter().map(|info| info.cluster).collect(),
            component_ids: input.component_ids.clone(),
            stamp: vec![0; input.cell_count()],
            cluster_stamp: vec![0; input.cluster_count as usize],
            generation: 0,
        }
    }

    /// Mandatory cells for `camera`, ascending.
    pub(crate) fn cell_set(
        &mut self,
        camera: u32,
        bound: DistanceBound,
        neighbors: &Neighbors,
        granularity: Granularity,
    ) -> Vec<u32> {
        let mut reached = Vec::new();
        reached.push(camera);
        match bound.fixed() {
            Some(fixed) => reached.extend(neighbors.within(camera, fixed)),
            None => {
                let component = self.component_ids[camera as usize] as usize;
                reached.extend_from_slice(&self.component_members[component]);
            }
        }
        self.set_from_reached(camera, &reached, granularity)
    }

    /// Mandatory cells for `camera` given the cells it reaches, ascending:
    /// its own cluster, pinned clusters, and `reached` at `granularity`.
    /// Duplicates in `reached` are harmless.
    pub(crate) fn set_from_reached(
        &mut self,
        camera: u32,
        reached: &[u32],
        granularity: Granularity,
    ) -> Vec<u32> {
        self.generation += 1;
        let generation = self.generation;
        let mut set = Vec::new();
        let stamp = &mut self.stamp;
        let mut add = |cell: u32| {
            if stamp[cell as usize] != generation {
                stamp[cell as usize] = generation;
                set.push(cell);
            }
        };
        let own_cluster = self.cell_cluster[camera as usize] as usize;
        self.cluster_stamp[own_cluster] = generation;
        for &cell in &self.cluster_members[own_cluster] {
            add(cell);
        }
        for &cell in &self.pinned_cells {
            add(cell);
        }
        for &cell in reached {
            match granularity {
                Granularity::Cell => add(cell),
                Granularity::ClusterClosure => {
                    let cluster = self.cell_cluster[cell as usize] as usize;
                    if self.cluster_stamp[cluster] != generation {
                        self.cluster_stamp[cluster] = generation;
                        for &member in &self.cluster_members[cluster] {
                            add(member);
                        }
                    }
                }
            }
        }
        set.sort_unstable();
        set
    }
}

/// How a reached cell enters the mandatory set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Granularity {
    /// Only the reached cell itself.
    Cell,
    /// The reached cell's whole cluster: residency state is cluster-keyed.
    ClusterClosure,
}

impl Granularity {
    pub(crate) const ALL: [Granularity; 2] = [Granularity::Cell, Granularity::ClusterClosure];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Granularity::Cell => "cell-granular",
            Granularity::ClusterClosure => "cluster-closure",
        }
    }
}

/// Resident bytes of one mandatory set.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MandatoryBytes {
    pub cell_count: usize,
    /// Sum of the set's own chart bytes: the floor any granularity could reach.
    pub texel_exact: f64,
    /// `texel_exact` with each chart interior halved per axis.
    pub half_res: f64,
    /// `(texel_exact, half_res)` with an omitted-for-width id 42 restored;
    /// `None` unless the level omitted id 42 for width.
    pub with_omitted_mask: Option<(f64, f64)>,
    /// Distinct layers touched × their bytes, one entry per layout.
    pub layer: Vec<u64>,
}

impl MandatoryBytes {
    /// Texel-exact bytes with id 42 charged whenever the level selects shadow
    /// lights: the restored-mask column when the bake omitted id 42 for width. Blocks and tiles narrow enough to double charge id 42 the
    /// same way (`AtlasFormats::layer_carries_shadowmask`), so this is the
    /// matching denominator for them.
    pub(crate) fn texel_exact_charging_mask(&self) -> f64 {
        self.with_omitted_mask
            .map_or(self.texel_exact, |(texel_exact, _)| texel_exact)
    }
}

/// Per-cell chart areas and the combined id 22 + id 42 byte rate.
pub(crate) struct CellFootprint {
    pub area: Vec<u64>,
    pub half_area: Vec<u64>,
    pub bytes_per_texel: f64,
    pub omitted_mask_bytes_per_texel: Option<f64>,
}

impl CellFootprint {
    pub(crate) fn new(input: &DryRunInput) -> Self {
        let mut area = vec![0; input.cell_count()];
        let mut half_area = vec![0; input.cell_count()];
        for chart in &input.charts {
            area[chart.cell as usize] += chart.area();
            half_area[chart.cell as usize] += chart.half_res_area();
        }
        Self {
            area,
            half_area,
            bytes_per_texel: input.formats.bytes_per_texel(),
            omitted_mask_bytes_per_texel: input.formats.omitted_shadowmask_bytes_per_texel(),
        }
    }
}

pub(crate) fn mandatory_bytes(
    set: &[u32],
    footprint: &CellFootprint,
    layouts: &[Layout],
    layer_stamp: &mut Vec<bool>,
) -> MandatoryBytes {
    let area: u64 = set.iter().map(|&c| footprint.area[c as usize]).sum();
    let half_area: u64 = set.iter().map(|&c| footprint.half_area[c as usize]).sum();
    let layer = layouts
        .iter()
        .map(|layout| {
            layer_stamp.clear();
            layer_stamp.resize(layout.layer_bytes.len(), false);
            let mut bytes = 0;
            for &cell in set {
                for &layer in &layout.cell_layers[cell as usize] {
                    if !layer_stamp[layer as usize] {
                        layer_stamp[layer as usize] = true;
                        bytes += layout.layer_bytes[layer as usize];
                    }
                }
            }
            bytes
        })
        .collect();
    MandatoryBytes {
        cell_count: set.len(),
        texel_exact: area as f64 * footprint.bytes_per_texel,
        half_res: half_area as f64 * footprint.bytes_per_texel,
        with_omitted_mask: footprint.omitted_mask_bytes_per_texel.map(|mask| {
            let rate = footprint.bytes_per_texel + mask;
            (area as f64 * rate, half_area as f64 * rate)
        }),
        layer,
    }
}
