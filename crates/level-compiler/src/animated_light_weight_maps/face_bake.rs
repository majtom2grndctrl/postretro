// One animated face's light-independent setup, and one light's weight-map texels and texel-step scan on it.
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
use crate::lightmap_bake::{Chart, light_contribution_and_direction, segment_clear};
use crate::map_data::ShadowType;
use crate::texel_footprint::{
    ClipBand, ClipReach, TexelFrame, TexelSampler, TexelSite, lead_in_clear,
};
use crate::texel_step_detector::{ChartSteps, CutSites, TexelRect, scan_chart};

/// One light slot's `(rect texel, entry)` pairs, in rect order.
type SlotEntries = Vec<(u32, TexelLight)>;

/// One chunk of a face, as every light's bake on it reads it.
struct ChunkLayout<'a> {
    rect: ChunkAtlasRect,
    layer: u32,
    lights: &'a [u32],
    /// The rect's first texel in chart interior coordinates.
    interior_origin: [i32; 2],
    /// The rect's texels inside the chart interior, in interior coordinates:
    /// `[x0, x1, y0, y1]`, ends exclusive. Empty for a degenerate chart.
    span: [i32; 4],
}

impl ChunkLayout<'_> {
    fn is_empty(&self) -> bool {
        self.span[0] >= self.span[1] || self.span[2] >= self.span[3]
    }

    fn holds(&self, light: u32) -> bool {
        self.lights.contains(&light)
    }

    /// Per-texel offset/count entries and the light pool from `entries`, one
    /// list per light slot: texels in row-major rect order, each texel's
    /// lights in the chunk's list order.
    fn into_result(self, entries: Vec<SlotEntries>) -> ChunkBakeResult {
        let texel_count = (self.rect.width * self.rect.height) as usize;
        let mut offset_counts = Vec::with_capacity(texel_count);
        let mut texel_lights = Vec::new();
        let mut cursors = vec![0usize; entries.len()];
        for texel in 0..texel_count as u32 {
            let offset = texel_lights.len() as u32;
            for (slot, entries) in entries.iter().enumerate() {
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

/// What every light's bake on one face shares: the face's chart, its chunks
/// in the order it was given them, the lights they list that bake here, and
/// the light-independent clip pre-pass.
pub(super) struct FaceSetup<'a> {
    face: usize,
    chart: &'a Chart,
    chunks: Vec<ChunkLayout<'a>>,
    /// Distinct non-`sdf` lights the face's chunks list, ascending.
    pub(super) lights: Vec<u32>,
    frame: TexelFrame,
    band: ClipBand,
    /// `None` with the floor off or on a degenerate chart.
    clip_reach: Option<ClipReach>,
}

/// One light's bake on one face.
pub(super) struct LightBake {
    /// Per chunk that lists the light: its position among the face's chunks,
    /// the light's slot in its list, and its `(rect texel, entry)` pairs in
    /// rect order.
    entries: Vec<(usize, usize, SlotEntries)>,
    pub(super) steps: Option<ChartSteps>,
    /// Time spent laying out and scanning the visibility grid.
    pub(super) detector: Duration,
}

impl<'a> FaceSetup<'a> {
    /// Lay out the chunks `chunk_indices` (into the chunk section, all on one
    /// face) and run the face's clip pre-pass.
    pub(super) fn new(inputs: &WeightMapInputs<'a>, chunk_indices: &[usize]) -> Self {
        let chunks = &inputs.chunk_section.chunks;
        let face = chunks[chunk_indices[0]].face_index as usize;
        let chart = &inputs.face_charts[face];
        let placement = inputs.face_placements[face];
        let (interior_w, interior_h) = chart_interior_dims(chart);
        let chart_usable = chart.uv_extent[0] > 0.0 && chart.uv_extent[1] > 0.0;
        let padding = CHART_PADDING_TEXELS as i32;

        let layouts: Vec<ChunkLayout<'a>> = chunk_indices
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
                // The chunk builder lists each candidate light once, so a
                // light owns one slot and one task's entries fill it.
                debug_assert!(
                    lights
                        .iter()
                        .enumerate()
                        .all(|(i, light)| !lights[..i].contains(light)),
                    "chunk {index} lists a light twice: {lights:?}"
                );
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
                ChunkLayout {
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
                }
            })
            .collect();

        // Disjoint-direct exclusion at the direct weight-map consumer: `sdf`
        // lights resolve their direct term at runtime, so they contribute no
        // baked `lm_anim` weight. They stay in `inputs.lights`, so indices
        // align with the chunk section and the delta-SH bake.
        let mut lights: Vec<u32> = layouts
            .iter()
            .flat_map(|chunk| chunk.lights.iter().copied())
            .filter(|&light| inputs.lights[light as usize].shadow_type != ShadowType::Sdf)
            .collect();
        lights.sort_unstable();
        lights.dedup();

        // Light-independent, so one pre-pass serves every light on the face.
        let clip_reach = (inputs.penumbra_floor && chart_usable && !lights.is_empty())
            .then(|| ClipReach::rasterized(chart, inputs.bvh, inputs.primitives, inputs.geometry));
        Self {
            face,
            chart,
            chunks: layouts,
            lights,
            frame: TexelFrame::for_chart(chart),
            band: ClipBand::for_chart(chart),
            clip_reach,
        }
    }

    /// Bake `light_index` over every chunk of the face that lists it, then
    /// scan its visibility for texel steps.
    ///
    /// The grid holds the light's raw visibility before weighting over the
    /// texels those chunks cover, absent where it contributes no weight. The
    /// weight stores visibility times the weight and drops texels below the
    /// weight epsilon, so no such grid survives the bake; this task's is the
    /// only one it holds, dropped on return.
    pub(super) fn bake_light(
        &self,
        inputs: &WeightMapInputs<'_>,
        sites: &CutSites<'_>,
        light_index: u32,
        embedded_emitter: bool,
    ) -> LightBake {
        let chart = self.chart;
        let light = &inputs.lights[light_index as usize];
        let mut detector = Duration::ZERO;
        // The grid spans the texels of the chunks that list this light.
        let Some(region) = self
            .chunks
            .iter()
            .filter(|chunk| chunk.holds(light_index) && !chunk.is_empty())
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
            // Every chunk listing the light is empty; its slots stay empty.
            return LightBake {
                entries: Vec::new(),
                steps: None,
                detector,
            };
        };
        let clear =
            |from, to| segment_clear(inputs.bvh, inputs.primitives, inputs.geometry, from, to);
        let lead_in = |from, to| {
            lead_in_clear(
                inputs.bvh,
                inputs.primitives,
                inputs.geometry,
                &self.band,
                from,
                to,
            )
        };

        let started = Instant::now();
        let mut grid = vec![f32::NAN; (region.width * region.height) as usize];
        detector += started.elapsed();

        let sampler = TexelSampler::new(
            light,
            inputs.area_sample_count,
            inputs.penumbra_floor,
            embedded_emitter,
        );
        let mut entries = Vec::new();
        for (position, chunk) in self.chunks.iter().enumerate() {
            let Some(slot) = chunk.lights.iter().position(|&l| l == light_index) else {
                continue;
            };
            let mut chunk_entries = Vec::new();
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
                        frame: self.frame,
                        may_clip: self
                            .clip_reach
                            .as_ref()
                            .is_none_or(|reach| reach.may_clip(tx, ty)),
                    };
                    let visibility = sampler.visibility(light, &site, clear, clear, lead_in);
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
                    chunk_entries.push((at, entry));
                }
            }
            entries.push((position, slot, chunk_entries));
        }

        let started = Instant::now();
        let steps = scan_chart(
            chart,
            self.face,
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
        LightBake {
            entries,
            steps,
            detector,
        }
    }

    /// The face's chunk results, parallel to the chunk indices it was given,
    /// from its lights' bakes in any order: each fills its own slots.
    pub(super) fn into_chunks(
        self,
        bakes: impl IntoIterator<Item = LightBake>,
    ) -> Vec<ChunkBakeResult> {
        let mut entries: Vec<Vec<SlotEntries>> = self
            .chunks
            .iter()
            .map(|chunk| vec![Vec::new(); chunk.lights.len()])
            .collect();
        for bake in bakes {
            for (position, slot, chunk_entries) in bake.entries {
                entries[position][slot] = chunk_entries;
            }
        }
        self.chunks
            .into_iter()
            .zip(entries)
            .map(|(chunk, entries)| chunk.into_result(entries))
            .collect()
    }
}
