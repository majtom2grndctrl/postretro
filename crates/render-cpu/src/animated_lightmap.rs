// Animated lightmap debug configuration and CPU packing.
// See: context/lib/rendering_pipeline.md §4

const DEBUG_MAX_LIGHTS_PER_CHUNK: u32 = 4;
const DEBUG_ENV_VAR: &str = "POSTRETRO_ANIMATED_LM_DEBUG";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnimatedLmDebugConfig {
    pub mode: u32,
    pub isolate_slot: u32,
}

impl AnimatedLmDebugConfig {
    pub fn from_env() -> Self {
        let Ok(raw) = std::env::var(DEBUG_ENV_VAR) else {
            return Self::default();
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Self::default();
        }
        if trimmed.eq_ignore_ascii_case("count") {
            log::info!("[Renderer] Animated LM debug: count heatmap (mode 1)");
            return Self {
                mode: 1,
                isolate_slot: 0,
            };
        }
        if let Some(rest) = trimmed.strip_prefix("isolate=") {
            match rest.parse::<u32>() {
                Ok(slot) => {
                    log::info!("[Renderer] Animated LM debug: isolate slot {slot} (mode 2)");
                    return Self {
                        mode: 2,
                        isolate_slot: slot,
                    };
                }
                Err(err) => {
                    log::warn!(
                        "[Renderer] {DEBUG_ENV_VAR}='{raw}' has invalid slot: {err}; debug off",
                    );
                    return Self::default();
                }
            }
        }
        log::warn!(
            "[Renderer] {DEBUG_ENV_VAR}='{raw}' not recognized (expected 'count' or \
             'isolate=<u32>'); debug off",
        );
        Self::default()
    }

    pub fn to_uniform_bytes(self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0..4].copy_from_slice(&self.mode.to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.isolate_slot.to_ne_bytes());
        bytes[8..12].copy_from_slice(&DEBUG_MAX_LIGHTS_PER_CHUNK.to_ne_bytes());
        bytes
    }

    pub const fn disabled() -> Self {
        Self {
            mode: 0,
            isolate_slot: 0,
        }
    }
}

/// Cross-check a decoded section 25 against the sections it relies on and the
/// static lightmap it lives beside: chunk count against section 24, internal
/// layout consistency, every block's static rect inside a static layer, the
/// paging bounds, and every light index inside the animated descriptor
/// buffer. `static_atlas` is the usable static layer `(size, layer_count)`.
pub fn validate_cross_section(
    section: &postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection,
    animated_chunks: Option<
        &postretro_level_format::animated_light_chunks::AnimatedLightChunksSection,
    >,
    animated_light_count: u32,
    static_atlas: (u32, u32),
) -> Result<(), String> {
    match animated_chunks {
        Some(chunks) => {
            if section.chunk_rects.len() != chunks.chunks.len() {
                return Err(format!(
                    "chunk_rects.len() ({}) != AnimatedLightChunks.chunks.len() ({})",
                    section.chunk_rects.len(),
                    chunks.chunks.len(),
                ));
            }
        }
        None => {
            if !section.chunk_rects.is_empty() {
                return Err(format!(
                    "AnimatedLightWeightMaps present ({} chunk_rects) but \
                     AnimatedLightChunks section is missing — PRL is malformed",
                    section.chunk_rects.len(),
                ));
            }
        }
    }

    if let Some(error) = section.consistency_error() {
        return Err(format!("animated light weight maps layout: {error}"));
    }

    let (static_size, static_layers) = static_atlas;
    for (i, block) in section.blocks.iter().enumerate() {
        let inside = block.static_layer < static_layers
            && u64::from(block.static_x) + u64::from(block.width) <= u64::from(static_size)
            && u64::from(block.static_y) + u64::from(block.height) <= u64::from(static_size);
        if !inside {
            return Err(format!(
                "blocks[{i}] static rect ({}, {}) {}x{} on layer {} exceeds static atlas \
                 {static_size}x{static_size}x{static_layers}",
                block.static_x, block.static_y, block.width, block.height, block.static_layer,
            ));
        }
    }
    if !section.chunk_rects.is_empty() {
        postretro_level_format::animated_lightmap_atlas::validate_animated_page_size(
            section.page_size,
            section.largest_block_side(),
            Some(static_size),
        )?;
    }

    for (i, tl) in section.texel_lights.iter().enumerate() {
        if tl.light_index >= animated_light_count {
            return Err(format!(
                "texel_lights[{}].light_index ({}) >= animated_light_count ({})",
                i, tl.light_index, animated_light_count,
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::animated_light_chunks::{
        AnimatedLightChunk, AnimatedLightChunksSection,
    };
    use postretro_level_format::animated_light_weight_maps::{
        AnimatedBlock, AnimatedLightWeightMapsSection, ChunkAtlasRect, TexelLight, TexelLightEntry,
    };

    fn mk_chunks(n: usize) -> AnimatedLightChunksSection {
        AnimatedLightChunksSection {
            chunks: (0..n)
                .map(|_| AnimatedLightChunk {
                    aabb_min: [0.0, 0.0, 0.0],
                    face_index: 0,
                    aabb_max: [1.0, 1.0, 1.0],
                    index_offset: 0,
                    uv_min: [0.0, 0.0],
                    uv_max: [1.0, 1.0],
                    index_count: 0,
                    _padding: 0,
                })
                .collect(),
            light_indices: Vec::new(),
        }
    }

    fn mk_rect(w: u32, h: u32, offset: u32) -> ChunkAtlasRect {
        ChunkAtlasRect {
            compact_x: 2,
            compact_y: 2,
            width: w,
            height: h,
            texel_offset: offset,
            block: 0,
        }
    }

    /// Chunks all inside one 8×8 block on static layer 0, identity-placed on
    /// one 8² page (the static layer size used throughout these tests).
    fn mk_section(
        chunk_rects: Vec<ChunkAtlasRect>,
        offset_counts: Vec<TexelLightEntry>,
        texel_lights: Vec<TexelLight>,
    ) -> AnimatedLightWeightMapsSection {
        let has_chunks = !chunk_rects.is_empty();
        AnimatedLightWeightMapsSection {
            page_size: if has_chunks { 8 } else { 0 },
            compact_layers: u32::from(has_chunks),
            blocks: if has_chunks {
                vec![AnimatedBlock {
                    static_layer: 0,
                    static_x: 0,
                    static_y: 0,
                    compact_x: 0,
                    compact_y: 0,
                    compact_layer: 0,
                    width: 8,
                    height: 8,
                }]
            } else {
                Vec::new()
            },
            chunk_rects,
            offset_counts,
            texel_lights,
        }
    }

    #[test]
    fn debug_config_uniform_bytes_layout() {
        let cfg = AnimatedLmDebugConfig {
            mode: 2,
            isolate_slot: 7,
        };
        let bytes = cfg.to_uniform_bytes();
        assert_eq!(&bytes[0..4], &2u32.to_ne_bytes());
        assert_eq!(&bytes[4..8], &7u32.to_ne_bytes());
        assert_eq!(&bytes[8..12], &4u32.to_ne_bytes());
        assert_eq!(&bytes[12..16], &[0, 0, 0, 0]);
    }

    fn one_lit_texel_section() -> AnimatedLightWeightMapsSection {
        mk_section(
            vec![mk_rect(2, 2, 0)],
            vec![
                TexelLightEntry {
                    offset: 0,
                    count: 1,
                },
                TexelLightEntry {
                    offset: 1,
                    count: 0,
                },
                TexelLightEntry {
                    offset: 1,
                    count: 0,
                },
                TexelLightEntry {
                    offset: 1,
                    count: 0,
                },
            ],
            vec![TexelLight {
                light_index: 0,
                weight: 0.5,
                direction_oct: [32768, 65535],
            }],
        )
    }

    #[test]
    fn validate_cross_section_accepts_valid_section() {
        let chunks = mk_chunks(1);
        assert_eq!(
            validate_cross_section(&one_lit_texel_section(), Some(&chunks), 1, (8, 1)),
            Ok(())
        );
    }

    #[test]
    fn validate_cross_section_rejects_bad_prefix_sum() {
        let section = mk_section(
            vec![mk_rect(2, 2, 0), mk_rect(1, 1, 5)],
            vec![
                TexelLightEntry {
                    offset: 0,
                    count: 0,
                };
                5
            ],
            vec![],
        );
        let chunks = mk_chunks(2);
        let err = validate_cross_section(&section, Some(&chunks), 0, (8, 1)).unwrap_err();
        assert!(err.contains("partition"), "unexpected error: {err}");
    }

    #[test]
    fn validate_cross_section_rejects_out_of_range_light_index() {
        let mut section = one_lit_texel_section();
        section.texel_lights[0].light_index = 42;
        let chunks = mk_chunks(1);
        let err = validate_cross_section(&section, Some(&chunks), 5, (8, 1)).unwrap_err();
        assert!(err.contains("light_index"), "unexpected error: {err}");
    }

    #[test]
    fn validate_cross_section_rejects_offset_count_out_of_range() {
        let mut section = one_lit_texel_section();
        section.offset_counts[0].count = 5;
        let chunks = mk_chunks(1);
        let err = validate_cross_section(&section, Some(&chunks), 1, (8, 1)).unwrap_err();
        assert!(err.contains("texel_lights"), "unexpected error: {err}");
    }

    #[test]
    fn validate_cross_section_rejects_offset_counts_length_mismatch() {
        let mut section = one_lit_texel_section();
        section.offset_counts.pop();
        let chunks = mk_chunks(1);
        let err = validate_cross_section(&section, Some(&chunks), 1, (8, 1)).unwrap_err();
        assert!(
            err.contains("offset_counts length"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_cross_section_rejects_missing_chunks_when_weight_maps_present() {
        let err = validate_cross_section(&one_lit_texel_section(), None, 1, (8, 1)).unwrap_err();
        assert!(err.contains("AnimatedLightChunks") && err.contains("malformed"));
    }

    #[test]
    fn validate_cross_section_accepts_empty_weight_maps_without_chunks() {
        let section = mk_section(vec![], vec![], vec![]);
        assert_eq!(validate_cross_section(&section, None, 0, (8, 1)), Ok(()));
    }

    #[test]
    fn validate_cross_section_rejects_a_block_outside_the_static_atlas() {
        let chunks = mk_chunks(1);
        let mut past_edge = one_lit_texel_section();
        past_edge.blocks[0].static_x = 1;
        let err = validate_cross_section(&past_edge, Some(&chunks), 1, (8, 1)).unwrap_err();
        assert!(err.contains("exceeds static atlas"), "{err}");

        let mut past_layer = one_lit_texel_section();
        past_layer.blocks[0].static_layer = 1;
        let err = validate_cross_section(&past_layer, Some(&chunks), 1, (8, 1)).unwrap_err();
        assert!(err.contains("exceeds static atlas"), "{err}");
    }

    #[test]
    fn validate_cross_section_rejects_a_chunk_naming_a_block_past_the_table() {
        let mut section = one_lit_texel_section();
        section.chunk_rects[0].block = 1;
        let err = validate_cross_section(&section, Some(&mk_chunks(1)), 1, (8, 1)).unwrap_err();
        assert!(err.contains("past the table"), "{err}");
    }

    #[test]
    fn validate_cross_section_rejects_a_page_size_outside_the_bounds() {
        let mut section = one_lit_texel_section();
        section.page_size = 16;
        let err = validate_cross_section(&section, Some(&mk_chunks(1)), 1, (8, 1)).unwrap_err();
        assert!(
            err.contains("exceeds the static lightmap layer size"),
            "{err}"
        );
    }
}
