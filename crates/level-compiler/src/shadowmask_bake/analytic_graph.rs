// Analytic (unshadowed) selected-light overlap graph over the shared atlas.
// See: context/lib/build_pipeline.md §PRL (ShadowmaskAtlas)

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AnalyticGraphStats {
    pub(super) light_chart_pairs: usize,
    pub(super) candidate_light_chart_pairs: usize,
}

pub(super) fn build_analytic_overlap_graph(
    selected: &[(usize, u32, &MapLight)],
    shared: &SharedAtlas<'_>,
    geometry: &GeometryResult,
    control: &BakeControl,
) -> OverlapGraph {
    let chart_order: Vec<usize> = (0..shared.placements.len()).collect();
    let (graph, stats) = build_analytic_overlap_graph_in_order(
        selected,
        shared,
        geometry,
        control,
        &chart_order,
        true,
    );
    log::debug!(
        "[ShadowmaskAtlas] analytic graph prune kept {}/{} light-chart pairs; adjacency {} bytes",
        stats.candidate_light_chart_pairs,
        stats.light_chart_pairs,
        graph.storage_bytes()
    );
    graph
}

pub(super) fn build_analytic_overlap_graph_in_order(
    selected: &[(usize, u32, &MapLight)],
    shared: &SharedAtlas<'_>,
    geometry: &GeometryResult,
    control: &BakeControl,
    chart_order: &[usize],
    prune: bool,
) -> (OverlapGraph, AnalyticGraphStats) {
    let graph = OverlapGraph::new(selected.len());
    let world_aabb = lightmap_layer::geometry_world_aabb(geometry);
    let candidate_pairs = AtomicUsize::new(0);

    chart_order.par_iter().for_each(|&chart_index| {
        // The prune, texel walk, and edge union are one governed chart item.
        let _permit = control.governor().enter();
        let chart = &shared.charts[chart_index];
        let chart_aabb = chart_world_aabb(chart);
        let candidates: Vec<usize> = selected
            .iter()
            .enumerate()
            .filter_map(|(compact_index, &(_, _, light))| {
                (!prune || chart_may_receive_light(light, chart_aabb, world_aabb))
                    .then_some(compact_index)
            })
            .collect();
        candidate_pairs.fetch_add(candidates.len(), Ordering::Relaxed);

        let mut covered_lights = Vec::with_capacity(candidates.len());
        lightmap_layer::for_each_light_layer_chart_texel(shared, chart_index, |sample| {
            covered_lights.clear();
            for &compact_index in &candidates {
                let light = selected[compact_index].2;
                if lightmap_bake::light_texel_is_covered(
                    light,
                    sample.world_p,
                    sample.surface_normal,
                ) {
                    covered_lights.push(compact_index);
                }
            }
            graph.record_texel_overlap(covered_lights.len());
            for (position, &a) in covered_lights.iter().enumerate() {
                for &b in &covered_lights[position + 1..] {
                    graph.mark_overlap(a, b);
                }
            }
        });
        control.advance(1);
    });

    let stats = AnalyticGraphStats {
        light_chart_pairs: selected.len().saturating_mul(chart_order.len()),
        candidate_light_chart_pairs: candidate_pairs.load(Ordering::Relaxed),
    };
    (graph, stats)
}

pub(super) fn chart_world_aabb(chart: &lightmap_bake::Chart) -> Option<(DVec3, DVec3)> {
    if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
        return None;
    }

    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for u in [chart.uv_min[0], chart.uv_min[0] + chart.uv_extent[0]] {
        for v in [chart.uv_min[1], chart.uv_min[1] + chart.uv_extent[1]] {
            let point = chart.origin + chart.u_axis * u + chart.v_axis * v;
            let point = DVec3::new(point.x as f64, point.y as f64, point.z as f64);
            min = min.min(point);
            max = max.max(point);
        }
    }
    Some((min, max))
}

pub(super) fn chart_may_receive_light(
    light: &MapLight,
    chart_aabb: Option<(DVec3, DVec3)>,
    world_aabb: (DVec3, DVec3),
) -> bool {
    let Some((chart_min, chart_max)) = chart_aabb else {
        return false;
    };
    if matches!(light.light_type, LightType::Directional) {
        return true;
    }
    let (mut light_min, mut light_max) = affinity_grid::light_aabb(light, world_aabb);
    // Coverage casts the light origin to f32 before subtracting the f32 chart
    // sample. Pruning must use that same coordinate domain or it can reject a
    // chart that coverage retains at large finite coordinates.
    let coverage_origin = DVec3::new(
        light.origin.x as f32 as f64,
        light.origin.y as f32 as f64,
        light.origin.z as f32 as f64,
    );
    let origin_rounding = coverage_origin - light.origin;
    light_min += origin_rounding;
    light_max += origin_rounding;
    chart_min.x <= light_max.x
        && chart_max.x >= light_min.x
        && chart_min.y <= light_max.y
        && chart_max.y >= light_min.y
        && chart_min.z <= light_max.z
        && chart_max.z >= light_min.z
}
