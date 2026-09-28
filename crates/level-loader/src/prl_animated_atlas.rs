// Load-time checks of section 25 against the sections it relies on: the page
// size against section 22's layer, and each vertex's block id (section 17)
// against the block table.
// See: context/lib/build_pipeline.md §PRL section IDs (AnimatedLightWeightMaps)

use postretro_level_format::animated_light_weight_maps::{
    AnimatedBlock, AnimatedLightWeightMapsSection,
};
use postretro_level_format::animated_lightmap_atlas::validate_animated_page_size;
use postretro_level_format::lightmap::LightmapSection;
use postretro_render_data::geometry::WorldVertex;

use crate::prl::PrlLoadError;

/// Reject a section whose page size breaks the paging rules, then apply the
/// block-id mismatch policy. Returns the section to install, or `None` when a
/// player release build disables animated light for a mismatched level.
pub(crate) fn check_animated_atlas(
    weight_maps: Option<AnimatedLightWeightMapsSection>,
    lightmap: Option<&LightmapSection>,
    vertices: &[WorldVertex],
) -> Result<Option<AnimatedLightWeightMapsSection>, PrlLoadError> {
    if let Some(section) = &weight_maps {
        check_page_size(section, lightmap)?;
    }
    let static_layer_size = usable_static_layer_size(lightmap);
    match first_block_id_mismatch(vertices, weight_maps.as_ref(), static_layer_size) {
        None => Ok(weight_maps),
        Some(message) => block_id_mismatch_policy(message).map(|()| None),
    }
}

/// The real static layer size, or `None` for an absent, placeholder or
/// non-square section 22, whose static-relative checks are skipped — the
/// renderer takes the no-animated-light path for all three.
fn usable_static_layer_size(lightmap: Option<&LightmapSection>) -> Option<u32> {
    lightmap
        .filter(|lightmap| !lightmap.is_placeholder() && lightmap.irr_width == lightmap.irr_height)
        .map(|lightmap| lightmap.irr_width)
}

/// The page size must be a power of two within the paging bounds. With no
/// real static lightmap the static-layer bounds are skipped: that level keeps
/// the no-animated-light path, whatever its pages.
fn check_page_size(
    section: &AnimatedLightWeightMapsSection,
    lightmap: Option<&LightmapSection>,
) -> Result<(), PrlLoadError> {
    if section.chunk_rects.is_empty() {
        return Ok(());
    }
    let static_layer_size = usable_static_layer_size(lightmap);
    validate_animated_page_size(
        section.page_size,
        section.largest_block_side(),
        static_layer_size,
    )
    .map_err(|message| PrlLoadError::AnimatedAtlasLayout { message })
}

/// First vertex whose block id names a block past the table, a block on a
/// different static layer than the vertex samples, or — with a real static
/// lightmap — a block whose static rect does not contain the vertex's static
/// texel. Id 0 means no block.
fn first_block_id_mismatch(
    vertices: &[WorldVertex],
    section: Option<&AnimatedLightWeightMapsSection>,
    static_layer_size: Option<u32>,
) -> Option<String> {
    let blocks = section.map_or(&[][..], |section| section.blocks.as_slice());
    vertices.iter().enumerate().find_map(|(index, vertex)| {
        let block = usize::from(vertex.animated_block.checked_sub(1)?);
        match blocks.get(block) {
            None => Some(format!(
                "vertex {index} names animated block {block} past the {}-block table",
                blocks.len()
            )),
            Some(entry) if entry.static_layer != u32::from(vertex.lightmap_layer) => Some(format!(
                "vertex {index} samples static layer {} but names animated block {block} \
                     on layer {}",
                vertex.lightmap_layer, entry.static_layer
            )),
            Some(entry) => static_layer_size
                .filter(|&size| !static_texel_inside_block(vertex, entry, size))
                .map(|_| {
                    format!(
                        "vertex {index} lightmap UV {:?} lies outside animated block {block}'s \
                         static rect ({}, {}) {}x{}",
                        vertex.lightmap_uv,
                        entry.static_x,
                        entry.static_y,
                        entry.width,
                        entry.height
                    )
                }),
        }
    })
}

/// The vertex's static texel coordinate, decoded as the forward shader does
/// (`q / 65535 × size`), lies inside the block's static rect, gutter
/// included. The compiler's footprint guard keeps it at least a texel inside.
fn static_texel_inside_block(
    vertex: &WorldVertex,
    block: &AnimatedBlock,
    static_layer_size: u32,
) -> bool {
    let texel = |quantized: u16| f32::from(quantized) / 65535.0 * static_layer_size as f32;
    let inside = |t: f32, start: u32, extent: u32| {
        t >= start as f32 && t <= (u64::from(start) + u64::from(extent)) as f32
    };
    inside(texel(vertex.lightmap_uv[0]), block.static_x, block.width)
        && inside(texel(vertex.lightmap_uv[1]), block.static_y, block.height)
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
    use postretro_level_format::animated_light_weight_maps::{
        AnimatedBlock, ChunkAtlasRect, TexelLightEntry,
    };

    fn vertex(lightmap_layer: u16, animated_block: u16) -> WorldVertex {
        WorldVertex {
            position: [0.0; 3],
            base_uv: [0.0; 2],
            normal_oct: [0; 2],
            tangent_packed: [0; 2],
            lightmap_uv: [0; 2],
            lightmap_layer,
            animated_block,
        }
    }

    /// One 600×300 block on static layer 2, on one page of `page_size`.
    fn section(page_size: u32) -> AnimatedLightWeightMapsSection {
        AnimatedLightWeightMapsSection {
            page_size,
            compact_layers: 1,
            blocks: vec![AnimatedBlock {
                static_layer: 2,
                static_x: 0,
                static_y: 0,
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

    fn lightmap(size: u32) -> LightmapSection {
        LightmapSection {
            irr_width: size,
            irr_height: size,
            ..LightmapSection::placeholder()
        }
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
        let loaded = check_animated_atlas(Some(section(1024)), Some(&lightmap(2048)), &[])
            .expect("a legal page size loads");
        assert!(loaded.is_some());
    }

    #[test]
    fn page_size_that_is_not_a_power_of_two_is_rejected_with_a_recompile_error() {
        let message = layout_error(check_animated_atlas(
            Some(section(1000)),
            Some(&lightmap(2048)),
            &[],
        ));
        assert!(message.contains("power of two"), "{message}");
        assert!(message.contains("recompile"), "{message}");
    }

    #[test]
    fn page_size_outside_the_bounds_is_rejected() {
        // Past the static layer.
        let message = layout_error(check_animated_atlas(
            Some(section(4096)),
            Some(&lightmap(2048)),
            &[],
        ));
        assert!(
            message.contains("static lightmap layer size 2048"),
            "{message}"
        );
        // Below the largest block.
        layout_error(check_animated_atlas(
            Some(section(512)),
            Some(&lightmap(2048)),
            &[],
        ));
        // Below the 1024 floor while the static layer is larger.
        let mut small_block = section(512);
        small_block.blocks[0].width = 100;
        small_block.blocks[0].height = 100;
        layout_error(check_animated_atlas(
            Some(small_block),
            Some(&lightmap(2048)),
            &[],
        ));
    }

    #[test]
    fn placeholder_static_lightmap_skips_the_upper_bound() {
        // 8192² pages exceed the 1×1 placeholder layer; the level still loads
        // and the renderer takes the no-animated-light path.
        let loaded = check_animated_atlas(
            Some(section(8192)),
            Some(&LightmapSection::placeholder()),
            &[],
        )
        .expect("placeholder static lightmap skips the static bounds");
        assert!(loaded.is_some());
        check_animated_atlas(Some(section(8192)), None, &[])
            .expect("an absent static lightmap skips the static bounds");
    }

    #[test]
    fn vertices_without_a_block_id_always_agree() {
        let loaded = check_animated_atlas(
            Some(section(1024)),
            Some(&lightmap(2048)),
            &[vertex(0, 0), vertex(2, 1), vertex(5, 0)],
        )
        .expect("matching ids load");
        assert!(loaded.is_some());
        assert_eq!(first_block_id_mismatch(&[vertex(3, 0)], None, None), None);
    }

    #[test]
    fn mismatch_detection_names_a_block_past_the_table_or_on_another_layer() {
        let section = section(1024);
        let past = first_block_id_mismatch(&[vertex(2, 2)], Some(&section), None).unwrap();
        assert!(past.contains("past the 1-block table"), "{past}");
        let layer =
            first_block_id_mismatch(&[vertex(0, 0), vertex(3, 1)], Some(&section), None).unwrap();
        assert!(layer.contains("vertex 1 samples static layer 3"), "{layer}");
        let absent = first_block_id_mismatch(&[vertex(2, 1)], None, None).unwrap();
        assert!(absent.contains("past the 0-block table"), "{absent}");
    }

    fn uv(texel: f32, size: u32) -> u16 {
        (texel / size as f32 * 65535.0).round() as u16
    }

    /// A same-layer id off by one names a neighbouring block: its static rect
    /// does not contain the vertex, so it is a mismatch too.
    #[test]
    fn mismatch_detection_names_a_block_whose_static_rect_misses_the_vertex() {
        let section = section(1024);
        // Block 0 spans static texels [0, 600] × [0, 300] of a 2048 layer.
        let mut inside = vertex(2, 1);
        inside.lightmap_uv = [uv(100.0, 2048), uv(100.0, 2048)];
        assert_eq!(
            first_block_id_mismatch(&[inside], Some(&section), Some(2048)),
            None
        );
        let mut outside = vertex(2, 1);
        outside.lightmap_uv = [uv(900.0, 2048), uv(100.0, 2048)];
        let message = first_block_id_mismatch(&[outside], Some(&section), Some(2048)).unwrap();
        assert!(message.contains("outside animated block 0"), "{message}");
        // Without a real static lightmap there is no static space to check.
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
            Some(&lightmap(2048)),
            &[vertex(3, 1)],
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
            Some(&lightmap(2048)),
            &[vertex(3, 1), vertex(2, 9)],
        )
        .expect("player release builds load a mismatched level");
        assert!(loaded.is_none(), "animated light is disabled");
        capture.assert_logged_once(log::Level::Error, "animated light disabled for this level");
    }
}
