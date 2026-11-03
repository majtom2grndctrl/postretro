// Gating one light's lit texels in one lightmap partition, at both termini of
// every mover the light may meet.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use glam::Vec3;

use super::pose::{MoverPoses, Terminus};
use super::rows::MoverTexels;
use super::segment::{
    BOX_SLACK_M, boxes_may_touch, point_touches_brush, segment_may_touch_box, segment_touches_brush,
};
use crate::chart_raster::{CHART_PADDING_TEXELS, chart_interior_dims, chart_texel_world_position};
use crate::lightmap_bake::hard_ray_target;
use crate::lightmap_layer::{LightmapLayer, SharedAtlas};
use crate::map_data::{LightType, MapLight};
use crate::partition::Aabb;
use crate::ray_traversal::RAY_EPSILON;

/// One mover a light may meet, by index into [`MoverPoses::analysed`].
/// `inside[k]` is `None` when the light's influence misses the pose at
/// terminus `k`, else whether the light sits inside one of its brushes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CandidateMover {
    pub(crate) mover: usize,
    pub(crate) inside: [Option<bool>; 2],
}

/// The posed movers a light's influence sphere reaches; every pose for a
/// sun. Built once per light, so a partition no mover can touch costs one
/// empty check.
pub(crate) fn light_candidates(light: &MapLight, poses: &MoverPoses) -> Vec<CandidateMover> {
    poses
        .analysed
        .iter()
        .enumerate()
        .filter_map(|(mover, analysed)| {
            let inside = Terminus::BOTH.map(|terminus| {
                let pose = analysed.pose(terminus);
                if !pose.bounds.is_valid() {
                    return None;
                }
                match light.light_type {
                    LightType::Directional => Some(false),
                    LightType::Point | LightType::Spot => {
                        // A lit texel lies within the falloff range, so its
                        // whole segment lies inside the influence sphere.
                        let nearest = light.origin.clamp(pose.bounds.min, pose.bounds.max);
                        let reach = f64::from(light.falloff_range.max(1.0e-4)) + BOX_SLACK_M;
                        (light.origin.distance(nearest) <= reach).then(|| {
                            pose.brushes
                                .iter()
                                .any(|brush| point_touches_brush(light.origin, brush))
                        })
                    }
                }
            });
            inside
                .iter()
                .any(Option::is_some)
                .then_some(CandidateMover { mover, inside })
        })
        .collect()
}

/// Gate `partition`, one light's records on one bake layer, over `charts`
/// with their padded texel bounds. A texel counts at a terminus when the
/// bake lit it and its segment from the lifted centre toward the light
/// touches a posed brush, or the light sits inside one. Returns only movers
/// with a gated texel.
pub(crate) fn gate_partition(
    poses: &MoverPoses,
    light: &MapLight,
    candidates: &[CandidateMover],
    shared: &SharedAtlas<'_>,
    partition: &LightmapLayer,
    charts: impl Iterator<Item = (usize, Option<(Vec3, Vec3)>)>,
) -> Vec<MoverTexels> {
    let mut tallies: Vec<MoverTexels> = candidates.iter().map(MoverTexels::new).collect();
    // Per chart: which candidate poses its segments can reach at all.
    let mut live: Vec<[bool; 2]> = vec![[false; 2]; candidates.len()];
    for (chart_index, bounds) in charts {
        let Some(hull) = bounds.map(|bounds| segment_hull(light, bounds)) else {
            continue;
        };
        let mut any_live = false;
        for (slot, candidate) in candidates.iter().enumerate() {
            let analysed = &poses.analysed[candidate.mover];
            live[slot] = Terminus::BOTH.map(|terminus| match candidate.inside[terminus.index()] {
                None => false,
                Some(true) => true,
                Some(false) => boxes_may_touch(&hull, &analysed.pose(terminus).bounds),
            });
            any_live |= live[slot].contains(&true);
        }
        if !any_live {
            continue;
        }
        let block = shared.layout.chart_blocks[chart_index];
        for_each_lit_texel(shared, partition, chart_index, |centre, lifted| {
            let end = hard_ray_target(centre, light).as_dvec3();
            let (start, centre) = (lifted.as_dvec3(), centre.as_dvec3());
            for (slot, candidate) in candidates.iter().enumerate() {
                let analysed = &poses.analysed[candidate.mover];
                let mut mask = 0u8;
                for terminus in Terminus::BOTH {
                    let k = terminus.index();
                    if !live[slot][k] {
                        continue;
                    }
                    let pose = analysed.pose(terminus);
                    let gated = candidate.inside[k] == Some(true)
                        || (segment_may_touch_box(start, end, &pose.bounds)
                            && pose
                                .brushes
                                .iter()
                                .any(|brush| segment_touches_brush(start, end, brush)));
                    if gated {
                        mask |= terminus.bit();
                    }
                }
                if mask != 0 {
                    tallies[slot].add(mask, centre, block);
                }
            }
        });
    }
    tallies.retain(|tally| tally.gated.iter().any(|&count| count > 0));
    for tally in &mut tallies {
        tally.finish();
    }
    tallies
}

/// World bounds of every segment a chart's texels trace toward `light`,
/// lift included.
fn segment_hull(light: &MapLight, (min, max): (Vec3, Vec3)) -> Aabb {
    let lift = Vec3::splat(RAY_EPSILON);
    let (min, max) = ((min - lift).as_dvec3(), (max + lift).as_dvec3());
    let mut hull = Aabb { min, max };
    match light.light_type {
        LightType::Point | LightType::Spot => hull.expand_point(light.origin),
        LightType::Directional => {
            // A sun's target sits a fixed offset from its texel.
            let offset = hard_ray_target(Vec3::ZERO, light).as_dvec3();
            hull.expand_aabb(&Aabb {
                min: min + offset,
                max: max + offset,
            });
        }
    }
    hull
}

/// Each texel of chart `chart_index` the bake lit in `partition`, as its
/// centre and the centre lifted along the chart normal as the bake lifts it.
/// Records are sorted by atlas index, so each row is one range.
fn for_each_lit_texel(
    shared: &SharedAtlas<'_>,
    partition: &LightmapLayer,
    chart_index: usize,
    mut visit: impl FnMut(Vec3, Vec3),
) {
    let chart = &shared.charts[chart_index];
    if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
        return;
    }
    let placement = shared.placements[chart_index];
    let (width, height) = chart_interior_dims(chart);
    let lift = chart.normal * RAY_EPSILON;
    let texels = &partition.texels;
    let mut cursor = 0;
    for ty in 0..height {
        let start = (placement.y + CHART_PADDING_TEXELS + ty as u32) * shared.atlas_width
            + placement.x
            + CHART_PADDING_TEXELS;
        cursor += texels[cursor..].partition_point(|texel| texel.idx < start);
        while let Some(texel) = texels.get(cursor)
            && texel.idx < start + width as u32
        {
            if texel.raw_visibility > 0.0 {
                let centre = chart_texel_world_position(chart, (texel.idx - start) as i32, ty);
                visit(centre, centre + lift);
            }
            cursor += 1;
        }
    }
}
