// Gating one light's lit texels at both termini of every mover the light may
// meet; here, over one static lightmap partition.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use glam::{DVec3, Vec3};

use super::pose::{MoverPoses, Terminus};
use super::rows::MoverTexels;
use super::segment::{
    BOX_SLACK_M, boxes_may_touch, point_inside_brush, point_touches_brush, sphere_touches_box,
};
use crate::chart_raster::{CHART_PADDING_TEXELS, chart_interior_dims, chart_texel_world_position};
use crate::lightmap_bake::hard_ray_target;
use crate::lightmap_layer::{LightmapLayer, SharedAtlas};
use crate::map_data::{LightType, MapLight};
use crate::partition::Aabb;
use crate::ray_traversal::RAY_EPSILON;

/// One mover a light may meet, by index into [`MoverPoses::analysed`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CandidateMover {
    pub(crate) mover: usize,
    /// Per terminus: `None` when the light's influence misses the pose,
    /// else whether the light touches one of its brushes, inside or on a
    /// face. Touching, every lit texel counts: each segment ends on the
    /// light, so it touches that brush.
    pub(crate) inside: [Option<bool>; 2],
    /// Per terminus: whether the light lies in a posed brush's interior,
    /// past the contact band. The row's inside flag; a light on a face is
    /// not flagged, though its texels count as above.
    pub(crate) light_inside: [bool; 2],
}

impl CandidateMover {
    /// The gate mask of the segment `start..=end`: one [`Terminus::bit`] per
    /// terminus where `live` holds and the light touches the pose or the
    /// segment does. `live` lets a caller skip termini a cheaper bound
    /// already rules out.
    pub(crate) fn mask(&self, poses: &MoverPoses, live: [bool; 2], start: DVec3, end: DVec3) -> u8 {
        let analysed = &poses.analysed[self.mover];
        Terminus::BOTH
            .into_iter()
            .filter(|&terminus| {
                let k = terminus.index();
                live[k]
                    && match self.inside[k] {
                        None => false,
                        Some(true) => true,
                        Some(false) => analysed.pose(terminus).segment_touches(start, end),
                    }
            })
            .fold(0, |mask, terminus| mask | terminus.bit())
    }
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
                        // The bake records a texel only where the falloff
                        // at its centre is above zero and N·L > 0: the centre
                        // lies within range r, with the light on the side its
                        // normal faces. The lift moves the segment's start
                        // toward that side, so the start stays within r plus
                        // a vanishing ε²/2r of the light. The segment ends on
                        // the light, so by the ball's convexity all of it
                        // lies within that reach. It can touch a brush only
                        // inside the bounds grown by the box slack, so the
                        // slack widens the reach; it also absorbs the f32
                        // rounding of centres and of the light's target.
                        let reach = f64::from(light.falloff_range.max(1.0e-4)) + BOX_SLACK_M;
                        sphere_touches_box(light.origin, reach, &pose.bounds).then(|| {
                            pose.brushes
                                .iter()
                                .any(|brush| point_touches_brush(light.origin, brush))
                        })
                    }
                }
            });
            let light_inside = Terminus::BOTH.map(|terminus| {
                inside[terminus.index()] == Some(true)
                    && analysed
                        .pose(terminus)
                        .brushes
                        .iter()
                        .any(|brush| point_inside_brush(light.origin, brush))
            });
            inside
                .iter()
                .any(Option::is_some)
                .then_some(CandidateMover {
                    mover,
                    inside,
                    light_inside,
                })
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
    let mut gate = TexelGate::new(poses, light, candidates);
    for (chart_index, bounds) in charts {
        if !bounds.is_some_and(|bounds| gate.begin_chart(bounds)) {
            continue;
        }
        let block = shared.layout.chart_blocks[chart_index];
        for_each_lit_texel(shared, partition, chart_index, |centre, lifted| {
            gate.add_texel(centre, lifted, block);
        });
    }
    gate.finish()
}

/// One light's texel gate over the charts it lit, shared by the static
/// partitions and the animated weights so both tiers test a texel alike.
pub(crate) struct TexelGate<'a> {
    poses: &'a MoverPoses,
    light: &'a MapLight,
    candidates: &'a [CandidateMover],
    tallies: Vec<MoverTexels>,
    /// Per candidate, for the current chart: which termini its segments can
    /// reach at all.
    live: Vec<[bool; 2]>,
}

impl<'a> TexelGate<'a> {
    pub(crate) fn new(
        poses: &'a MoverPoses,
        light: &'a MapLight,
        candidates: &'a [CandidateMover],
    ) -> Self {
        Self {
            poses,
            light,
            candidates,
            tallies: candidates.iter().map(MoverTexels::new).collect(),
            live: vec![[false; 2]; candidates.len()],
        }
    }

    /// Start a chart whose padded texel bounds are `bounds`; `false` when no
    /// candidate pose can meet its segments, so its texels need no test.
    pub(crate) fn begin_chart(&mut self, bounds: (Vec3, Vec3)) -> bool {
        let hull = segment_hull(self.light, bounds);
        let mut any_live = false;
        for (live, candidate) in self.live.iter_mut().zip(self.candidates) {
            let analysed = &self.poses.analysed[candidate.mover];
            *live = Terminus::BOTH.map(|terminus| match candidate.inside[terminus.index()] {
                None => false,
                Some(true) => true,
                Some(false) => boxes_may_touch(&hull, &analysed.pose(terminus).bounds),
            });
            any_live |= live.contains(&true);
        }
        any_live
    }

    /// Gate one lit texel of the current chart, in cell block `block`:
    /// `centre` is where the bake anchored it, `lifted` where its segments
    /// start.
    pub(crate) fn add_texel(&mut self, centre: Vec3, lifted: Vec3, block: u32) {
        let end = hard_ray_target(centre, self.light).as_dvec3();
        let start = lifted.as_dvec3();
        for (slot, candidate) in self.candidates.iter().enumerate() {
            let mask = candidate.mask(self.poses, self.live[slot], start, end);
            if mask != 0 {
                self.tallies[slot].add(mask, centre.as_dvec3(), block);
            }
        }
    }

    /// The movers with a gated texel, block lists made distinct.
    pub(crate) fn finish(mut self) -> Vec<MoverTexels> {
        self.tallies
            .retain(|tally| tally.gated.iter().any(|&count| count > 0));
        for tally in &mut self.tallies {
            tally.finish();
        }
        self.tallies
    }
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
pub(super) fn for_each_lit_texel(
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
