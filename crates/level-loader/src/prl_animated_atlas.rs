// Load-time checks of section 25 against the sections it relies on: each
// animated block against its section-22 cell block, the page size, and each
// vertex's animated block id (section 17) against the block table.
// See: context/lib/build_pipeline.md §PRL section IDs (AnimatedLightWeightMaps)

use postretro_level_format::animated_light_weight_maps::{
    AnimatedBlock, AnimatedLightWeightMapsSection,
};
use postretro_level_format::animated_lightmap_atlas::validate_animated_page_size;
use postretro_level_format::lightmap::{LightmapBlockIndex, LightmapBlockRecord};
use postretro_render_data::geometry::WorldVertex;

use crate::prl::PrlLoadError;

/// Reject a section whose blocks leave their cell blocks or whose page size
/// breaks the paging rules, then apply the block-id mismatch policy. Returns
/// the section to install, or `None` when a player release build disables
/// animated light for a mismatched level.
pub(crate) fn check_animated_atlas(
    weight_maps: Option<AnimatedLightWeightMapsSection>,
    lightmap: Option<&LightmapBlockIndex>,
    vertices: &[WorldVertex],
) -> Result<Option<AnimatedLightWeightMapsSection>, PrlLoadError> {
    let cell_blocks = usable_cell_blocks(lightmap);
    if let Some(section) = &weight_maps {
        check_page_size(section)?;
        if let Some(cell_blocks) = cell_blocks {
            check_blocks_inside_cell_blocks(section, cell_blocks)?;
        }
    }
    match first_block_id_mismatch(vertices, weight_maps.as_ref(), cell_blocks) {
        None => Ok(weight_maps),
        Some(message) => block_id_mismatch_policy(message).map(|()| None),
    }
}

/// The id-22 cell blocks, or `None` in placeholder mode (id 22 absent or
/// holding no blocks). Placeholder mode gives the animated atlas no static
/// frame, so the renderer takes the no-animated-light path and the
/// block-frame checks are skipped.
fn usable_cell_blocks(lightmap: Option<&LightmapBlockIndex>) -> Option<&[LightmapBlockRecord]> {
    lightmap
        .map(|index| index.records.as_slice())
        .filter(|records| !records.is_empty())
}

/// The page size must be a power of two that holds the largest block. The
/// static-layer bounds retired with the static atlas; the renderer still
/// checks the atlas against its VRAM budget.
fn check_page_size(section: &AnimatedLightWeightMapsSection) -> Result<(), PrlLoadError> {
    if section.chunk_rects.is_empty() {
        return Ok(());
    }
    validate_animated_page_size(section.page_size, section.largest_block_side(), None)
        .map_err(|message| PrlLoadError::AnimatedAtlasLayout { message })
}

/// Every animated block names a cell block in the id-22 table and lies
/// inside that block's extent. A hard error in every build: compose would
/// otherwise write texels that belong to a neighbouring cell block.
fn check_blocks_inside_cell_blocks(
    section: &AnimatedLightWeightMapsSection,
    cell_blocks: &[LightmapBlockRecord],
) -> Result<(), PrlLoadError> {
    for (index, block) in section.blocks.iter().enumerate() {
        let Some(cell) = cell_blocks.get(block.lightmap_block as usize) else {
            return Err(PrlLoadError::AnimatedAtlasLayout {
                message: format!(
                    "animated block {index} names lightmap block {} past the {}-block Lightmap \
                     (id 22) table",
                    block.lightmap_block,
                    cell_blocks.len()
                ),
            });
        };
        let fits = |origin: u16, extent: u32, cell_extent: u16| {
            u64::from(origin) + u64::from(extent) <= u64::from(cell_extent)
        };
        if !fits(block.block_x, block.width, cell.width)
            || !fits(block.block_y, block.height, cell.height)
        {
            return Err(PrlLoadError::AnimatedAtlasLayout {
                message: format!(
                    "animated block {index} rect ({}, {}) {}x{} lies outside lightmap block {}'s \
                     {}x{} extent",
                    block.block_x,
                    block.block_y,
                    block.width,
                    block.height,
                    block.lightmap_block,
                    cell.width,
                    cell.height
                ),
            });
        }
    }
    Ok(())
}

/// First vertex whose animated block id names a block past the table, a
/// block in a different cell block than the vertex samples, or — with real
/// cell blocks — a block whose rect does not contain the vertex's
/// block-local texel. Id 0 means no block.
fn first_block_id_mismatch(
    vertices: &[WorldVertex],
    section: Option<&AnimatedLightWeightMapsSection>,
    cell_blocks: Option<&[LightmapBlockRecord]>,
) -> Option<String> {
    let blocks = section.map_or(&[][..], |section| section.blocks.as_slice());
    vertices.iter().enumerate().find_map(|(index, vertex)| {
        let block = usize::from(vertex.animated_block.checked_sub(1)?);
        let Some(entry) = blocks.get(block) else {
            return Some(format!(
                "vertex {index} names animated block {block} past the {}-block table",
                blocks.len()
            ));
        };
        if u64::from(entry.lightmap_block) + 1 != u64::from(vertex.lightmap_block) {
            return Some(format!(
                "vertex {index} samples {} but names animated block {block} in lightmap block {}",
                describe_cell_block(vertex.lightmap_block),
                entry.lightmap_block
            ));
        }
        let cell = cell_blocks?.get(entry.lightmap_block as usize)?;
        (!block_texel_inside(vertex, entry, cell)).then(|| {
            format!(
                "vertex {index} lightmap UV {:?} lies outside animated block {block}'s rect \
                 ({}, {}) {}x{} in lightmap block {}",
                vertex.lightmap_uv,
                entry.block_x,
                entry.block_y,
                entry.width,
                entry.height,
                entry.lightmap_block
            )
        })
    })
}

fn describe_cell_block(vertex_block: u16) -> String {
    match vertex_block.checked_sub(1) {
        Some(block) => format!("lightmap block {block}"),
        None => "no lightmap block".to_owned(),
    }
}

/// The vertex's block-local texel, decoded as the forward shader does
/// (`q / 65535 × block extent`), lies inside the animated block's rect,
/// gutter included. The compiler's footprint guard keeps it at least a texel
/// inside.
fn block_texel_inside(
    vertex: &WorldVertex,
    block: &AnimatedBlock,
    cell: &LightmapBlockRecord,
) -> bool {
    let texel = |quantized: u16, extent: u16| f32::from(quantized) / 65535.0 * f32::from(extent);
    let inside = |t: f32, start: u16, extent: u32| {
        t >= f32::from(start) && t <= (u64::from(start) + u64::from(extent)) as f32
    };
    inside(
        texel(vertex.lightmap_uv[0], cell.width),
        block.block_x,
        block.width,
    ) && inside(
        texel(vertex.lightmap_uv[1], cell.height),
        block.block_y,
        block.height,
    )
}

/// Debug builds and any `dev-tools` build reject the level so the stale or
/// corrupt PRL is noticed; a player release build loads it with animated light
/// disabled and logs once.
fn block_id_mismatch_policy(message: String) -> Result<(), PrlLoadError> {
    if cfg!(any(debug_assertions, feature = "dev-tools")) {
        Err(PrlLoadError::AnimatedAtlasLayout { message })
    } else {
        log::error!(
            "[PRL] AnimatedLightWeightMaps (id 25): {message}; animated light disabled for this \
             level — recompile the .prl with the current `prl-build`"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::animated_light_weight_maps::{ChunkAtlasRect, TexelLightEntry};
    use postretro_level_format::lightmap::{
        IRRADIANCE_FORMAT_RGBA16F, LightmapBlock, LightmapMode, LightmapSection,
    };

    fn vertex(lightmap_block: u16, animated_block: u16) -> WorldVertex {
        WorldVertex {
            position: [0.0; 3],
            base_uv: [0.0; 2],
            normal_oct: [0; 2],
            tangent_packed: [0; 2],
            lightmap_uv: [0; 2],
            lightmap_block,
            animated_block,
        }
    }

    /// One 600×300 animated block at the origin of lightmap cell block 2, on
    /// one page of `page_size`.
    fn section(page_size: u32) -> AnimatedLightWeightMapsSection {
        AnimatedLightWeightMapsSection {
            page_size,
            compact_layers: 1,
            blocks: vec![AnimatedBlock {
                lightmap_block: 2,
                block_x: 0,
                block_y: 0,
                compact_x: 0,
                compact_y: 0,
                compact_layer: 0,
                width: 600,
                height: 300,
            }],
            chunk_rects: vec![ChunkAtlasRect {
                compact_x: 2,
                compact_y: 2,
                width: 1,
                height: 1,
                texel_offset: 0,
                block: 0,
            }],
            offset_counts: vec![TexelLightEntry {
                offset: 0,
                count: 0,
            }],
            texel_lights: Vec::new(),
        }
    }

    /// The index an id-22 section of these cell-block extents carries.
    fn cell_blocks(extents: &[(u16, u16)]) -> LightmapBlockIndex {
        LightmapSection {
            direction_texel_scale: 2,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            mode: LightmapMode::Shadowed,
            blocks: extents
                .iter()
                .enumerate()
                .map(|(cell, &(width, height))| {
                    let texels = usize::from(width) * usize::from(height);
                    LightmapBlock {
                        cell_id: cell as u32,
                        width,
                        height,
                        irradiance: vec![0; texels * 8],
                        direction: vec![0; texels / 4 * 2],
                    }
                })
                .collect(),
        }
        .index()
    }

    /// Cell block 2 is 640×320: it holds the fixture's animated block.
    fn lightmap() -> LightmapBlockIndex {
        cell_blocks(&[(8, 8), (8, 8), (640, 320)])
    }

    fn layout_error(
        result: Result<Option<AnimatedLightWeightMapsSection>, PrlLoadError>,
    ) -> String {
        match result {
            Err(error @ PrlLoadError::AnimatedAtlasLayout { .. }) => error.to_string(),
            other => panic!("expected an animated atlas layout error, got {other:?}"),
        }
    }

    #[test]
    fn page_size_within_bounds_loads() {
        let loaded = check_animated_atlas(Some(section(1024)), Some(&lightmap()), &[])
            .expect("a legal page size loads");
        assert!(loaded.is_some());
    }

    #[test]
    fn page_size_that_is_not_a_power_of_two_is_rejected_with_a_recompile_error() {
        let message = layout_error(check_animated_atlas(
            Some(section(1000)),
            Some(&lightmap()),
            &[],
        ));
        assert!(message.contains("power of two"), "{message}");
        assert!(message.contains("recompile"), "{message}");
    }

    #[test]
    fn page_size_below_the_largest_block_is_rejected() {
        let message = layout_error(check_animated_atlas(
            Some(section(512)),
            Some(&lightmap()),
            &[],
        ));
        assert!(message.contains("below the lower bound"), "{message}");
    }

    // AC 6 (loader half): an animated block outside its cell block rejects in
    // every build, not only under the vertex mismatch policy.
    #[test]
    fn animated_block_outside_its_cell_block_is_rejected_in_every_build() {
        let mut past_edge = section(1024);
        past_edge.blocks[0].block_x = 44; // 44 + 600 passes the 640-wide cell block
        let message = layout_error(check_animated_atlas(
            Some(past_edge),
            Some(&lightmap()),
            &[],
        ));
        assert!(
            message.contains("lies outside lightmap block 2's 640x320 extent"),
            "{message}"
        );

        let mut too_tall = section(1024);
        too_tall.blocks[0].height = 324;
        layout_error(check_animated_atlas(Some(too_tall), Some(&lightmap()), &[]));

        let mut flush = section(1024);
        flush.blocks[0].block_x = 40; // ends exactly at the cell block's edge
        flush.blocks[0].block_y = 20;
        check_animated_atlas(Some(flush), Some(&lightmap()), &[])
            .expect("a block flush with its cell block's edge loads");
    }

    #[test]
    fn animated_block_naming_a_lightmap_block_past_the_table_is_rejected_in_every_build() {
        let mut past = section(1024);
        past.blocks[0].lightmap_block = 3;
        let message = layout_error(check_animated_atlas(Some(past), Some(&lightmap()), &[]));
        assert!(
            message.contains("names lightmap block 3 past the 3-block Lightmap"),
            "{message}"
        );
    }

    #[test]
    fn placeholder_lightmap_skips_the_block_frame_checks() {
        // No cell blocks means no static frame: the level still loads and the
        // renderer takes the no-animated-light path.
        let mut past = section(1024);
        past.blocks[0].lightmap_block = 40;
        let loaded = check_animated_atlas(Some(past.clone()), Some(&cell_blocks(&[])), &[])
            .expect("a zero-block lightmap skips the block-frame checks");
        assert!(loaded.is_some());
        check_animated_atlas(Some(past), None, &[])
            .expect("an absent lightmap skips the block-frame checks");
    }

    #[test]
    fn vertices_without_a_block_id_always_agree() {
        let loaded = check_animated_atlas(
            Some(section(1024)),
            Some(&lightmap()),
            &[vertex(0, 0), vertex(3, 1), vertex(5, 0)],
        )
        .expect("matching ids load");
        assert!(loaded.is_some());
        assert_eq!(first_block_id_mismatch(&[vertex(3, 0)], None, None), None);
    }

    #[test]
    fn mismatch_detection_names_a_block_past_the_table_or_in_another_cell_block() {
        let section = section(1024);
        let past = first_block_id_mismatch(&[vertex(3, 2)], Some(&section), None).unwrap();
        assert!(past.contains("past the 1-block table"), "{past}");
        let other =
            first_block_id_mismatch(&[vertex(0, 0), vertex(4, 1)], Some(&section), None).unwrap();
        assert!(
            other.contains(
                "vertex 1 samples lightmap block 3 but names animated block 0 in lightmap block 2"
            ),
            "{other}"
        );
        let unlit = first_block_id_mismatch(&[vertex(0, 1)], Some(&section), None).unwrap();
        assert!(unlit.contains("samples no lightmap block"), "{unlit}");
        let absent = first_block_id_mismatch(&[vertex(3, 1)], None, None).unwrap();
        assert!(absent.contains("past the 0-block table"), "{absent}");
    }

    fn uv(texel: f32, extent: u16) -> u16 {
        (texel / f32::from(extent) * 65535.0).round() as u16
    }

    /// An id naming the right cell block can still name a neighbouring
    /// animated block: its rect does not contain the vertex, so it is a
    /// mismatch too. The texel decodes against the cell block's extent.
    #[test]
    fn mismatch_detection_names_a_block_whose_rect_misses_the_vertex_texel() {
        let section = section(1024);
        let index = lightmap();
        let cells = Some(index.records.as_slice());
        // Block 0 spans block-local texels [0, 600] × [0, 300] of a 640×320
        // cell block.
        let mut inside = vertex(3, 1);
        inside.lightmap_uv = [uv(599.0, 640), uv(299.0, 320)];
        assert_eq!(
            first_block_id_mismatch(&[inside], Some(&section), cells),
            None
        );
        let mut outside = vertex(3, 1);
        outside.lightmap_uv = [uv(620.0, 640), uv(100.0, 320)];
        let message = first_block_id_mismatch(&[outside], Some(&section), cells).unwrap();
        assert!(
            message.contains("outside animated block 0's rect"),
            "{message}"
        );
        // Without real cell blocks there is no block frame to check.
        assert_eq!(
            first_block_id_mismatch(&[outside], Some(&section), None),
            None
        );
    }

    #[cfg(any(debug_assertions, feature = "dev-tools"))]
    #[test]
    fn block_id_mismatch_fails_the_load_with_a_recompile_error_in_debug_or_dev_tools() {
        let message = layout_error(check_animated_atlas(
            Some(section(1024)),
            Some(&lightmap()),
            &[vertex(4, 1)],
        ));
        assert!(message.contains("recompile"), "{message}");
    }

    #[cfg(not(any(debug_assertions, feature = "dev-tools")))]
    #[test]
    fn block_id_mismatch_loads_without_animated_light_and_logs_once_in_player_release() {
        use postretro_test_log_capture::LogCapture;
        let capture = LogCapture::start();
        let loaded = check_animated_atlas(
            Some(section(1024)),
            Some(&lightmap()),
            &[vertex(4, 1), vertex(3, 9)],
        )
        .expect("player release builds load a mismatched level");
        assert!(loaded.is_none(), "animated light is disabled");
        capture.assert_logged_once(log::Level::Error, "animated light disabled for this level");
    }
}
