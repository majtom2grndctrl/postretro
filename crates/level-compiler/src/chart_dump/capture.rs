//! Capture sites: copy one chart's texels out of the buffer an encoder is
//! about to consume, in chart-local row-major order. Read-only on the bake.

use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::shadowmask_atlas::{FaceOwners, MaskOffset, NO_FACE_OWNERS};

use super::ChartDumpConfig;
use super::format::{ChartKey, ChartMeta, DumpKind, Placement, PlaneKind, SharedDumpWriter};
use crate::animated_light_weight_maps::WeightMapInputs;
use crate::chart_raster::{CHART_PADDING_TEXELS, ChartPlacement};
use crate::lightmap_bake::{BlockLayout, Chart, CompositedAtlas};
use crate::lightmap_layer::SharedAtlas;

/// blake3 of the chart's grid, folded to 64 bits: what places its texels
/// in the world, never where it was packed.
pub(crate) fn chart_fingerprint(chart: &Chart) -> u64 {
    let mut hasher = blake3::Hasher::new();
    for v in [chart.origin, chart.u_axis, chart.v_axis, chart.normal] {
        for c in v.to_array() {
            hasher.update(&c.to_bits().to_le_bytes());
        }
    }
    for c in chart.uv_min.iter().chain(&chart.uv_extent) {
        hasher.update(&c.to_bits().to_le_bytes());
    }
    hasher.update(&chart.width_texels.to_le_bytes());
    hasher.update(&chart.height_texels.to_le_bytes());
    if let Some(window) = chart.window {
        for c in window.grid_uv_min.iter().chain(&window.grid_uv_extent) {
            hasher.update(&c.to_bits().to_le_bytes());
        }
        for v in window.grid_interior.iter().chain(&window.origin) {
            hasher.update(&v.to_le_bytes());
        }
    }
    let hash = hasher.finalize();
    u64::from_le_bytes(hash.as_bytes()[..8].try_into().expect("eight bytes"))
}

fn placement_of(layout: &BlockLayout, face: usize, placement: &ChartPlacement) -> Placement {
    let (block_x, block_y) = layout.local_placement(face, placement);
    Placement {
        block: layout.chart_blocks[face],
        block_x,
        block_y,
        layer: placement.layer,
        layer_x: placement.x,
        layer_y: placement.y,
        transposed: placement.transposed,
    }
}

/// A `width × height` chart-local rect read out of a row-major buffer
/// `buffer_width` texels wide, its placed top-left at buffer texel `(x, y)`,
/// in the chart's own row-major order: a transposed placement's rows are
/// read down the buffer's columns.
fn copy_chart_rect(
    buffer: &[u8],
    buffer_width: u32,
    texel_bytes: usize,
    (x, y): (u32, u32),
    (width, height): (u32, u32),
    placement: &ChartPlacement,
) -> Vec<u8> {
    let row_bytes = width as usize * texel_bytes;
    let mut out = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height {
        if !placement.transposed {
            let start = ((y + row) as usize * buffer_width as usize + x as usize) * texel_bytes;
            out.extend_from_slice(&buffer[start..start + row_bytes]);
            continue;
        }
        for column in 0..width {
            let (dx, dy) = placement.orient(column, row);
            let start =
                ((y + dy) as usize * buffer_width as usize + (x + dx) as usize) * texel_bytes;
            out.extend_from_slice(&buffer[start..start + texel_bytes]);
        }
    }
    out
}

/// `BlockSectionBuilder::push_layer`'s capture: every chart on the layer,
/// from the dilated plane BC6H encodes and the Rg8 direction it slices.
pub(crate) struct LightmapCapture<'a> {
    writer: SharedDumpWriter,
    charts: &'a [Chart],
    placements: &'a [ChartPlacement],
}

impl<'a> LightmapCapture<'a> {
    pub(crate) fn new(
        writer: SharedDumpWriter,
        charts: &'a [Chart],
        placements: &'a [ChartPlacement],
    ) -> Self {
        debug_assert_eq!(charts.len(), placements.len());
        Self {
            writer,
            charts,
            placements,
        }
    }

    /// `direction` is the layer's encoded Rg8 plane, `atlas_width /
    /// direction_texel_scale` texels wide.
    pub(crate) fn capture_layer(
        &self,
        layer: u32,
        layout: &BlockLayout,
        plane: &CompositedAtlas,
        direction: &[u8],
    ) {
        let scale = layout.direction_texel_scale.max(1);
        let direction_width = plane.atlas_width / scale;
        for (face, (chart, placement)) in self.charts.iter().zip(self.placements).enumerate() {
            if placement.layer != layer {
                continue;
            }
            // The f32 RGBA texels the BC6H encoder reads, little-endian, in
            // the chart's own row-major order.
            let row_floats = chart.width_texels as usize * 4;
            let mut irradiance = Vec::with_capacity(row_floats * 4 * chart.height_texels as usize);
            for row in 0..chart.height_texels {
                for column in 0..chart.width_texels {
                    let (x, y) = placement.padded_texel(column, row);
                    let start = (y as usize * plane.atlas_width as usize + x as usize) * 4;
                    for c in &plane.irradiance[start..start + 4] {
                        irradiance.extend_from_slice(&c.to_le_bytes());
                    }
                }
            }
            // Whole direction texels only: a texel straddling the padded
            // rect's edge also groups a neighbour's (or empty) texels, so it
            // is not this chart's value. Found on the placed rect, recorded
            // in the chart's own orientation.
            let (placed_w, placed_h) =
                placement.placed_extent(chart.width_texels, chart.height_texels);
            let x0 = placement.x.div_ceil(scale);
            let y0 = placement.y.div_ceil(scale);
            let x1 = (placement.x + placed_w) / scale;
            let y1 = (placement.y + placed_h) / scale;
            let placed_dims = (x1.saturating_sub(x0), y1.saturating_sub(y0));
            let (dims_w, dims_h) = placement.orient(placed_dims.0, placed_dims.1);
            let dims = [dims_w, dims_h];
            let dir = copy_chart_rect(
                direction,
                direction_width,
                2,
                (x0, y0),
                (dims_w, dims_h),
                placement,
            );
            let (offset_x, offset_y) =
                placement.orient(x0 * scale - placement.x, y0 * scale - placement.y);
            let meta = ChartMeta {
                key: ChartKey {
                    face: face as u32,
                    sub: 0,
                },
                chart_fingerprint: chart_fingerprint(chart),
                width: chart.width_texels,
                height: chart.height_texels,
                padding: CHART_PADDING_TEXELS,
                origin: [0, 0],
                placement: placement_of(layout, face, placement),
                direction_texel_scale: scale,
                direction_offset: [offset_x, offset_y],
                direction_dims: dims,
                owners: NO_FACE_OWNERS,
            };
            self.writer.push(
                meta,
                &[
                    (PlaneKind::Irradiance, &irradiance),
                    (PlaneKind::Direction, &dir),
                ],
            );
        }
    }
}

/// `OwnershipFill::close_layer`'s capture: each mask-owning chart of a block
/// at the moment its mask block goes to BC5, after owner ranking, padding,
/// dilation and the copy into the mask layout. Each chart is read back
/// through its face's stamped mask offset, the way the runtime finds it, so a
/// dump compares against the lightmap-layout mask of an older bake. A block
/// carried past its layer close is captured when it is finally encoded, so
/// every chart is recorded once, from its final values.
pub(crate) struct MaskCapture {
    writer: SharedDumpWriter,
}

impl MaskCapture {
    pub(crate) fn new(writer: SharedDumpWriter) -> Self {
        Self { writer }
    }

    /// `image` is the block's mask image, `image_width` texels per row, and
    /// `block_origin` the lightmap block's bake-layer origin. Each owned face
    /// of `faces` sits at its block-local lightmap texel plus its
    /// `mask_offsets` entry. Faces no light owns store no mask and are not
    /// recorded.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn capture_mask_block(
        &self,
        faces: &[u32],
        image: &[u8],
        image_width: u32,
        block_origin: [u32; 2],
        mask_offsets: &[MaskOffset],
        owners: &[FaceOwners],
        shared: &SharedAtlas<'_>,
    ) {
        for &face in faces {
            let face = face as usize;
            if owners[face] == NO_FACE_OWNERS {
                continue;
            }
            let chart = &shared.charts[face];
            let placement = &shared.placements[face];
            let at = |placed: u32, origin: u32, offset: i16| {
                u32::try_from(i64::from(placed - origin) + i64::from(offset))
                    .expect("a mask chart lies inside its mask block")
            };
            let [dx, dy] = mask_offsets[face];
            let rect = (
                at(placement.x, block_origin[0], dx),
                at(placement.y, block_origin[1], dy),
            );
            let texels = copy_chart_rect(
                image,
                image_width,
                4,
                rect,
                (chart.width_texels, chart.height_texels),
                placement,
            );
            let meta = ChartMeta {
                key: ChartKey {
                    face: face as u32,
                    sub: 0,
                },
                chart_fingerprint: chart_fingerprint(chart),
                width: chart.width_texels,
                height: chart.height_texels,
                padding: CHART_PADDING_TEXELS,
                origin: [0, 0],
                placement: placement_of(shared.layout, face, placement),
                direction_texel_scale: 0,
                direction_offset: [0, 0],
                direction_dims: [0, 0],
                owners: owners[face],
            };
            self.writer.push(meta, &[(PlaneKind::Mask, &texels)]);
        }
    }
}

/// Record every animated chunk's weights as the weight-map stage hands them
/// on: each chunk keyed by face and its ordinal among that face's chunks.
pub(crate) fn dump_animated_weight_maps(
    config: &ChartDumpConfig,
    inputs: &WeightMapInputs<'_>,
    section: &AnimatedLightWeightMapsSection,
) -> anyhow::Result<()> {
    let writer = SharedDumpWriter::create(config, DumpKind::Animated)?;
    let chunks = &inputs.chunk_section.chunks;
    anyhow::ensure!(
        chunks.len() == section.chunk_rects.len(),
        "chart dump: {} animated chunks but {} weight-map rects",
        chunks.len(),
        section.chunk_rects.len()
    );
    let mut ordinal_of_face = std::collections::HashMap::<u32, u32>::new();
    let mut texels = Vec::new();
    for (chunk, rect) in chunks.iter().zip(&section.chunk_rects) {
        let ordinal = ordinal_of_face.entry(chunk.face_index).or_insert(0);
        let sub = *ordinal;
        *ordinal += 1;
        let face = chunk.face_index as usize;
        let block = &section.blocks[rect.block as usize];
        let chart = &inputs.face_charts[face];
        texels.clear();
        let placement = &inputs.face_placements[face];
        let entries = &section.offset_counts
            [rect.texel_offset as usize..(rect.texel_offset + rect.width * rect.height) as usize];
        // The chunk in its chart's own orientation: a transposed chart's
        // rect rows are its columns.
        let (width, height) = placement.orient(rect.width, rect.height);
        let chart_order = (0..height).flat_map(|row| {
            (0..width).map(move |column| {
                let (x, y) = placement.orient(column, row);
                &entries[(y * rect.width + x) as usize]
            })
        });
        for entry in chart_order {
            texels.extend_from_slice(&entry.count.to_le_bytes());
            let lights =
                &section.texel_lights[entry.offset as usize..(entry.offset + entry.count) as usize];
            for light in lights {
                texels.extend_from_slice(&light.light_index.to_le_bytes());
                texels.extend_from_slice(&light.weight.to_bits().to_le_bytes());
                texels.extend_from_slice(&light.direction_oct[0].to_le_bytes());
                texels.extend_from_slice(&light.direction_oct[1].to_le_bytes());
            }
        }
        // Block and chunk rects share one translation into the compact
        // atlas, so their difference is the chunk's place in the chart.
        let (origin_x, origin_y) = placement.orient(
            rect.compact_x as i32 - block.compact_x as i32,
            rect.compact_y as i32 - block.compact_y as i32,
        );
        let origin = [origin_x, origin_y];
        let meta = ChartMeta {
            key: ChartKey {
                face: chunk.face_index,
                sub,
            },
            chart_fingerprint: chart_fingerprint(chart),
            width,
            height,
            padding: 0,
            origin,
            placement: placement_of(inputs.layout, face, placement),
            direction_texel_scale: 0,
            direction_offset: [0, 0],
            direction_dims: [0, 0],
            owners: NO_FACE_OWNERS,
        };
        writer.push(meta, &[(PlaneKind::AnimatedWeights, &texels)]);
    }
    writer.finish()
}
