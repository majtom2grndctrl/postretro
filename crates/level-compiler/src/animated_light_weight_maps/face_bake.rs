// One animated face's weight-map texels, baked light by light, with each light's texel-step scan.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::time::{Duration, Instant};

use postretro_level_format::animated_light_chunks::AnimatedLightChunk;
use postretro_level_format::animated_light_weight_maps::{
    ChunkAtlasRect, TexelLight, TexelLightEntry,
};

use super::static_atlas_frame::chunk_atlas_rect;
use super::{ChunkBakeResult, WEIGHT_EPSILON, WeightMapInputs, contribution_to_weight};
use crate::chart_raster::{
    CHART_PADDING_TEXELS, chart_interior_dims, chart_texel_seed, chart_texel_world_position,
};
use crate::lightmap_bake::{light_contribution_and_direction, segment_clear};
use crate::map_data::ShadowType;
use crate::texel_footprint::{ClipReach, TexelFrame, TexelSampler, TexelSite};
use crate::texel_step_detector::{ChartSteps, CutSites, TexelRect, scan_chart};

/// One face's bake: its chunks' results, parallel to the chunk indices it
/// was given, and each stepped light's scan in light order.
pub(super) struct FaceBake {
    pub(super) chunks: Vec<ChunkBakeResult>,
    pub(super) steps: Vec<(usize, ChartSteps)>,
    /// Time spent laying out and scanning visibility grids.
    pub(super) detector: Duration,
}

/// One chunk as the face bake walks it.
struct ChunkTexels<'a> {
    rect: ChunkAtlasRect,
    layer: u32,
    lights: &'a [u32],
    /// The rect's first texel in chart interior coordinates.
    interior_origin: [i32; 2],
    /// The rect's texels inside the chart interior, in interior coordinates:
    /// `[x0, x1, y0, y1]`, ends exclusive. Empty for a degenerate chart.
    span: [i32; 4],
    /// Per light slot, its `(rect texel, entry)` pairs in rect order.
    entries: Vec<Vec<(u32, TexelLight)>>,
}

impl ChunkTexels<'_> {
    fn is_empty(&self) -> bool {
        self.span[0] >= self.span[1] || self.span[2] >= self.span[3]
    }

    /// Per-texel offset/count entries and the light pool, texels in row-major
    /// rect order and each texel's lights in the chunk's list order.
    fn into_result(self) -> ChunkBakeResult {
        let texel_count = (self.rect.width * self.rect.height) as usize;
        let mut offset_counts = Vec::with_capacity(texel_count);
        let mut texel_lights = Vec::new();
        let mut cursors = vec![0usize; self.entries.len()];
        for texel in 0..texel_count as u32 {
            let offset = texel_lights.len() as u32;
            for (slot, entries) in self.entries.iter().enumerate() {
                if let Some(&(at, entry)) = entries.get(cursors[slot])
                    && at == texel
                {
                    texel_lights.push(entry);
                    cursors[slot] += 1;
                }
            }
            offset_counts.push(TexelLightEntry {
                offset,
                count: texel_lights.len() as u32 - offset,
            });
        }
        ChunkBakeResult {
            rect: self.rect,
            layer: self.layer,
            offset_counts,
            texel_lights,
        }
    }
}

/// Bake every chunk of one face, `chunk_indices` into the chunk section.
///
/// The face walks light by light rather than texel by texel. A texel's
/// entries come out in its chunk's light order all the same, and each
/// `(light, texel)` value depends on nothing else, so the bytes match a
/// texel-major walk. Walking a light at a time keeps one visibility grid in
/// flight: the light's raw visibility before weighting over the texels its
/// chunks cover, absent where it contributes no weight, which the texel-step
/// detector scans before the next light. The weight stores visibility times
/// the weight and drops texels below the weight epsilon, so no such grid
/// survives the bake.
pub(super) fn bake_face(
    inputs: &WeightMapInputs<'_>,
    sites: &CutSites<'_>,
    chunk_indices: &[usize],
) -> FaceBake {
    let chunks = &inputs.chunk_section.chunks;
    let face = chunks[chunk_indices[0]].face_index as usize;
    let chart = &inputs.face_charts[face];
    let placement = inputs.face_placements[face];
    let (interior_w, interior_h) = chart_interior_dims(chart);
    let chart_usable = chart.uv_extent[0] > 0.0 && chart.uv_extent[1] > 0.0;
    let padding = CHART_PADDING_TEXELS as i32;

    let mut walked: Vec<ChunkTexels<'_>> = chunk_indices
        .iter()
        .map(|&index| {
            let chunk: &AnimatedLightChunk = &chunks[index];
            debug_assert_eq!(chunk.face_index as usize, face);
            let (x, y, width, height) = chunk_atlas_rect(
                chart,
                placement,
                chunk.uv_min,
                chunk.uv_max,
                inputs.atlas_width,
                inputs.atlas_height,
            );
            let start = chunk.index_offset as usize;
            let lights =
                &inputs.chunk_section.light_indices[start..start + chunk.index_count as usize];
            // Texels past the chart's interior (outward rounding) stay empty.
            let interior_origin = [
                x as i32 - placement.x as i32 - padding,
                y as i32 - placement.y as i32 - padding,
            ];
            let span = if chart_usable {
                [
                    interior_origin[0].max(0),
                    (interior_origin[0] + width as i32).min(interior_w),
                    interior_origin[1].max(0),
                    (interior_origin[1] + height as i32).min(interior_h),
                ]
            } else {
                [0; 4]
            };
            ChunkTexels {
                rect: ChunkAtlasRect {
                    compact_x: x,
                    compact_y: y,
                    width,
                    height,
                    texel_offset: 0, // filled by the caller
                    block: 0,        // filled by the caller
                },
                layer: placement.layer,
                lights,
                interior_origin,
                span,
                entries: vec![Vec::new(); lights.len()],
            }
        })
        .collect();

    // Disjoint-direct exclusion at the direct weight-map consumer: `sdf`
    // lights resolve their direct term at runtime, so they contribute no
    // baked `lm_anim` weight. They stay in `inputs.lights`, so indices align
    // with the chunk section and the delta-SH bake.
    let mut face_lights: Vec<u32> = walked
        .iter()
        .flat_map(|chunk| chunk.lights.iter().copied())
        .filter(|&light| inputs.lights[light as usize].shadow_type != ShadowType::Sdf)
        .collect();
    face_lights.sort_unstable();
    face_lights.dedup();

    let frame = TexelFrame::for_chart(chart);
    // Light-independent, so one pre-pass serves every light on the face.
    let clip_reach = (inputs.penumbra_floor && chart_usable && !face_lights.is_empty())
        .then(|| ClipReach::rasterized(chart, inputs.bvh, inputs.primitives, inputs.geometry));
    let clear = |from, to| segment_clear(inputs.bvh, inputs.primitives, inputs.geometry, from, to);

    let mut grid: Vec<f32> = Vec::new();
    let mut steps = Vec::new();
    let mut detector = Duration::ZERO;
    for light_index in face_lights {
        let light = &inputs.lights[light_index as usize];
        let holds_light = |chunk: &ChunkTexels<'_>| chunk.lights.contains(&light_index);
        // The grid spans the texels of the chunks that list this light.
        let Some(region) = walked
            .iter()
            .filter(|chunk| holds_light(chunk) && !chunk.is_empty())
            .map(|chunk| chunk.span)
            .reduce(|a, b| {
                [
                    a[0].min(b[0]),
                    a[1].max(b[1]),
                    a[2].min(b[2]),
                    a[3].max(b[3]),
                ]
            })
            .map(|[x0, x1, y0, y1]| TexelRect {
                x: x0 as u32,
                y: y0 as u32,
                width: (x1 - x0) as u32,
                height: (y1 - y0) as u32,
            })
        else {
            continue;
        };
        let started = Instant::now();
        grid.clear();
        grid.resize((region.width * region.height) as usize, f32::NAN);
        detector += started.elapsed();

        let sampler = TexelSampler::new(light, inputs.area_sample_count, inputs.penumbra_floor);
        for chunk in walked.iter_mut().filter(|chunk| holds_light(chunk)) {
            let first_slot = chunk
                .lights
                .iter()
                .position(|&slot_light| slot_light == light_index)
                .expect("the chunk lists the light");
            let [x0, x1, y0, y1] = chunk.span;
            for ty in y0..y1 {
                for tx in x0..x1 {
                    let world_p = chart_texel_world_position(chart, tx, ty);
                    let (contribution, dir) =
                        light_contribution_and_direction(light, world_p, chart.normal);
                    let weight = contribution_to_weight(contribution, light.color, light.intensity);
                    if weight <= WEIGHT_EPSILON {
                        continue;
                    }
                    // The static bake's seed and footprint for this chart
                    // texel, so the animated weights and the static
                    // lightmap draw the same samples there. N·L, falloff and
                    // weight stay at the centre; visibility is filtered over
                    // the footprint unless the floor is off.
                    let site = TexelSite {
                        world_p,
                        normal: chart.normal,
                        seed: chart_texel_seed(chart, tx, ty),
                        frame,
                        may_clip: clip_reach
                            .as_ref()
                            .is_none_or(|reach| reach.may_clip(tx, ty)),
                    };
                    let visibility = sampler.visibility(light, &site, clear, clear);
                    let cell = (ty as u32 - region.y) * region.width + (tx as u32 - region.x);
                    grid[cell as usize] = visibility;
                    // The visibility fraction scales the weight, so a
                    // penumbra texel carries a fraction and the shadow's
                    // shape holds as the light's intensity animates; a fully
                    // occluded texel emits no entry.
                    if visibility <= 0.0 {
                        continue;
                    }
                    // The compose pass weights the incoming direction by the
                    // light's per-frame radiance to fuse a runtime
                    // dominant-direction atlas. `weight > EPSILON` already
                    // gated out the degenerate zero-distance case.
                    let entry = TexelLight {
                        light_index,
                        weight: weight * visibility,
                        direction_oct: postretro_level_format::octahedral::encode(
                            dir.x, dir.y, dir.z,
                        ),
                    };
                    let rect_x = tx - chunk.interior_origin[0];
                    let rect_y = ty - chunk.interior_origin[1];
                    let at = rect_y as u32 * chunk.rect.width + rect_x as u32;
                    chunk.entries[first_slot].push((at, entry));
                }
            }
            // A light listed twice emits twice, as the texel-major walk did.
            for slot in first_slot + 1..chunk.lights.len() {
                if chunk.lights[slot] == light_index {
                    chunk.entries[slot] = chunk.entries[first_slot].clone();
                }
            }
        }

        let started = Instant::now();
        let found = scan_chart(
            chart,
            face,
            sites,
            region,
            |y, row| {
                let start = (y * region.width) as usize;
                for (texel, &visibility) in row
                    .iter_mut()
                    .zip(&grid[start..start + region.width as usize])
                {
                    *texel = (!visibility.is_nan()).then_some(visibility);
                }
            },
            clear,
            inputs.step_threshold,
        );
        detector += started.elapsed();
        if let Some(found) = found {
            steps.push((light_index as usize, found));
        }
    }

    FaceBake {
        chunks: walked.into_iter().map(ChunkTexels::into_result).collect(),
        steps,
        detector,
    }
}
