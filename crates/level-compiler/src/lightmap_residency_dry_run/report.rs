//! Dry-run orchestration: attribution, layouts, and per-cell mandatory bytes
//! for each distance source and bound.

use postretro_level_format::cell_visibility::CELL_VISIBILITY_FANOUT_K;

use super::DryRunInput;
use super::attribution::{Attribution, SectionAttribution, attribute};
use super::cell_blocks::CellBlocks;
use super::layouts::{
    Layout, RepackCheck, cluster_ordered_layout, stored_layout, stored_repack_matches,
};
use super::mandatory::{
    CellFootprint, DistanceBound, Granularity, MandatoryBytes, MandatoryContext, mandatory_bytes,
};
use super::portal_distance::{
    DistanceValidation, VALIDATION_TOLERANCE_FIXED, validate_against_stored,
};
use super::render::{input_summary, shadowmask_policy};
use super::tiles::{TileLayout, tile_layouts};
use super::visible_set::{VisibleSetInputs, VisibleSetResult, run_visible_set};
use crate::cell_residency_bake::portal_distance::{Neighbors, recompute_pairs};

/// Layer caps simulated for soft cluster-ordered packing.
pub(crate) const SIMULATED_LAYER_CAPS: [u32; 2] = [1024, 2048];

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BoundResult {
    pub bound: DistanceBound,
    /// Playable camera cells, ascending, with their mandatory bytes.
    pub cells: Vec<(u32, MandatoryBytes)>,
    /// Cells with at least K stored partners inside the bound, whose id-46
    /// set may be missing partners the bake's top-K cut.
    pub possibly_truncated: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SourceResult {
    pub name: &'static str,
    pub granularity: Granularity,
    pub bounds: Vec<BoundResult>,
    /// Whether `possibly_truncated` was counted (stored id-46 records only).
    pub counts_truncation: bool,
    /// Why this source's bounds mean something other than their label.
    pub note: Option<&'static str>,
}

/// One distance source before granularity is applied.
struct SourceSpec<'a> {
    name: &'static str,
    neighbors: &'a Neighbors,
    counts_truncation: bool,
    /// False when the source has no distance records: every bound then
    /// resolves to the whole reachability component.
    distance_bounded: bool,
    note: Option<&'static str>,
}

/// Attributed bytes and layer reach of one chart-carrying cluster.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ClusterRow {
    pub cluster: u32,
    pub cells: usize,
    pub charts: usize,
    pub irradiance: u64,
    pub direction: u64,
    pub shadowmask: u64,
    /// Distinct layers touched, one entry per layout.
    pub layers: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DryRunReport {
    pub input_summary: String,
    pub shadowmask_policy: String,
    /// Id 42 was omitted for width; the floor gains restored-mask columns.
    pub omitted_mask: bool,
    pub attribution: Attribution,
    pub layouts: Vec<Layout>,
    pub repack: RepackCheck,
    pub stored_total_texels: u64,
    pub validation: Option<DistanceValidation>,
    pub sources: Vec<SourceResult>,
    /// Sampled visible-set bounds; needs the portal graph and the runtime
    /// visibility world.
    pub visible_set: Option<VisibleSetResult>,
    /// Per-unit fixed-size tile packings the visible-set pass costs.
    pub tile_layouts: Vec<TileLayout>,
    /// Each cell's charts packed into one or more contiguous BC-aligned blocks.
    pub cell_blocks: CellBlocks,
    /// Non-solid, non-exterior cells: every cell the camera can occupy.
    pub camera_cells: Vec<u32>,
    pub cluster_rows: Vec<ClusterRow>,
    pub cell_centers: Vec<[f32; 3]>,
    pub cell_clusters: Vec<u32>,
}

pub(crate) fn run_dry_run(input: &DryRunInput) -> DryRunReport {
    let attribution = attribute(&input.formats, &input.charts, input.cell_count());
    let mut layouts = vec![stored_layout(input)];
    for cap in SIMULATED_LAYER_CAPS {
        layouts.push(cluster_ordered_layout(input, cap));
    }
    let repack = stored_repack_matches(input);
    let tile_layouts = tile_layouts(input);
    let cell_blocks = CellBlocks::new(input);
    let footprint = CellFootprint::new(input);

    let mut chart_clusters = vec![false; input.cluster_count as usize];
    for chart in &input.charts {
        chart_clusters[input.cells[chart.cell as usize].cluster as usize] = true;
    }
    let camera_cells: Vec<u32> = (0..input.cell_count() as u32)
        .filter(|&cell| input.cells[cell as usize].camera_candidate)
        .collect();

    let stored_neighbors = Neighbors::from_pairs(input.cell_count(), &input.coupled_pairs);
    let mut specs = vec![SourceSpec {
        name: "id-46 stored pairs",
        neighbors: &stored_neighbors,
        counts_truncation: input.cell_visibility_present,
        distance_bounded: input.cell_visibility_present,
        note: (!input.cell_visibility_present)
            .then_some("id 46 absent: conservative all-perceivable, every bound is the whole map"),
    }];
    let mut validation = None;
    let mut visible_set = None;
    let recomputed_neighbors;
    if let Some(graph) = &input.portal_graph {
        let max_fixed = DistanceBound::max_fixed();
        // Recompute past the cut so a stored pair whose rounding straddles it
        // is still found for validation; bounds filter it again.
        let recomputed = recompute_pairs(graph, max_fixed + VALIDATION_TOLERANCE_FIXED);
        validation = Some(validate_against_stored(
            &input.coupled_pairs,
            &recomputed,
            max_fixed,
        ));
        recomputed_neighbors = Neighbors::from_pairs(input.cell_count(), &recomputed);
        if let Some(world) = &input.visibility_world {
            visible_set = Some(run_visible_set(&VisibleSetInputs {
                input,
                world,
                graph,
                neighbors: &recomputed_neighbors,
                camera_cells: &camera_cells,
                footprint: &footprint,
                layouts: &layouts,
                tile_layouts: &tile_layouts,
                cell_blocks: &cell_blocks,
            }));
        }
        specs.push(SourceSpec {
            name: "hub-metric recompute (untruncated)",
            neighbors: &recomputed_neighbors,
            counts_truncation: false,
            distance_bounded: true,
            note: (!input.cell_visibility_present)
                .then_some("id 46 absent: the unbounded row is the whole map"),
        });
    }
    let sources = specs
        .iter()
        .flat_map(|spec| {
            Granularity::ALL.map(|granularity| {
                evaluate_source(
                    spec,
                    granularity,
                    input,
                    &camera_cells,
                    &footprint,
                    &layouts,
                )
            })
        })
        .collect();

    DryRunReport {
        input_summary: input_summary(input),
        shadowmask_policy: shadowmask_policy(&input.formats),
        omitted_mask: input.formats.omitted_shadowmask_bytes_per_texel().is_some(),
        cluster_rows: cluster_rows(input, &attribution, &layouts, &chart_clusters),
        attribution,
        stored_total_texels: layouts[0].total_layer_texels(),
        layouts,
        repack,
        validation,
        sources,
        visible_set,
        tile_layouts,
        cell_blocks,
        camera_cells,
        cell_centers: input.cells.iter().map(|info| info.center).collect(),
        cell_clusters: input.cells.iter().map(|info| info.cluster).collect(),
    }
}

fn evaluate_source(
    spec: &SourceSpec<'_>,
    granularity: Granularity,
    input: &DryRunInput,
    camera_cells: &[u32],
    footprint: &CellFootprint,
    layouts: &[Layout],
) -> SourceResult {
    let mut context = MandatoryContext::new(input);
    let mut layer_stamp = Vec::new();
    let bounds = DistanceBound::ALL
        .iter()
        .map(|&bound| {
            let effective = if spec.distance_bounded {
                bound
            } else {
                DistanceBound::Unbounded
            };
            let mut possibly_truncated = 0;
            let cells = camera_cells
                .iter()
                .map(|&cell| {
                    if spec.counts_truncation
                        && let Some(fixed) = bound.fixed()
                        && spec.neighbors.within(cell, fixed).count() >= CELL_VISIBILITY_FANOUT_K
                    {
                        possibly_truncated += 1;
                    }
                    let set = context.cell_set(cell, effective, spec.neighbors, granularity);
                    (
                        cell,
                        mandatory_bytes(&set, footprint, layouts, &mut layer_stamp),
                    )
                })
                .collect();
            BoundResult {
                bound,
                cells,
                possibly_truncated,
            }
        })
        .collect();
    SourceResult {
        name: spec.name,
        granularity,
        bounds,
        counts_truncation: spec.counts_truncation,
        note: spec.note,
    }
}

fn cluster_rows(
    input: &DryRunInput,
    attribution: &Attribution,
    layouts: &[Layout],
    chart_clusters: &[bool],
) -> Vec<ClusterRow> {
    let members = input.cluster_members();
    let mut charts_per_cluster = vec![0usize; members.len()];
    for chart in &input.charts {
        charts_per_cluster[input.cells[chart.cell as usize].cluster as usize] += 1;
    }
    let sum = |section: &SectionAttribution, cells: &[u32]| -> u64 {
        cells.iter().map(|&c| section.per_cell[c as usize]).sum()
    };
    (0..members.len())
        .filter(|&cluster| chart_clusters[cluster])
        .map(|cluster| {
            let cells = &members[cluster];
            let layers = layouts
                .iter()
                .map(|layout| {
                    let mut touched: Vec<u32> = cells
                        .iter()
                        .flat_map(|&c| layout.cell_layers[c as usize].iter().copied())
                        .collect();
                    touched.sort_unstable();
                    touched.dedup();
                    touched.len()
                })
                .collect();
            ClusterRow {
                cluster: cluster as u32,
                cells: cells.len(),
                charts: charts_per_cluster[cluster],
                irradiance: sum(&attribution.irradiance, cells),
                direction: sum(&attribution.direction, cells),
                shadowmask: attribution
                    .shadowmask
                    .as_ref()
                    .map_or(0, |section| sum(section, cells)),
                layers,
            }
        })
        .collect()
}
