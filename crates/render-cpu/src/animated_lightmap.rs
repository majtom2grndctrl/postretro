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

pub fn validate_cross_section(
    section: &postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection,
    animated_chunks: Option<
        &postretro_level_format::animated_light_chunks::AnimatedLightChunksSection,
    >,
    animated_light_count: u32,
    slot_to_static_layer: &[u32],
    atlas_dimensions: (u32, u32),
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

    let (atlas_width, atlas_height) = atlas_dimensions;
    let mut running: u32 = 0;
    for (i, rect) in section.chunk_rects.iter().enumerate() {
        if slot_to_static_layer.binary_search(&rect.layer).is_err() {
            return Err(format!(
                "chunk_rects[{i}].layer ({}) is absent from the animated slot table",
                rect.layer,
            ));
        }

        let atlas_x_end = rect.atlas_x.checked_add(rect.width).ok_or_else(|| {
            format!(
                "chunk_rects[{i}] atlas x range overflows ({} + {})",
                rect.atlas_x, rect.width,
            )
        })?;
        let atlas_y_end = rect.atlas_y.checked_add(rect.height).ok_or_else(|| {
            format!(
                "chunk_rects[{i}] atlas y range overflows ({} + {})",
                rect.atlas_y, rect.height,
            )
        })?;
        if rect.atlas_x >= atlas_width
            || rect.atlas_y >= atlas_height
            || atlas_x_end > atlas_width
            || atlas_y_end > atlas_height
        {
            return Err(format!(
                "chunk_rects[{i}] atlas rectangle ({}, {}) {}x{} exceeds static atlas {}x{}",
                rect.atlas_x, rect.atlas_y, rect.width, rect.height, atlas_width, atlas_height,
            ));
        }
        if rect.texel_offset != running {
            return Err(format!(
                "chunk_rects[{}].texel_offset ({}) != prefix sum ({})",
                i, rect.texel_offset, running,
            ));
        }
        running = running
            .checked_add(rect.width.checked_mul(rect.height).ok_or_else(|| {
                format!(
                    "chunk_rects[{}] width*height overflow ({} * {})",
                    i, rect.width, rect.height,
                )
            })?)
            .ok_or_else(|| format!("chunk_rects prefix sum overflow at index {i}"))?;
    }
    if section.offset_counts.len() as u32 != running {
        return Err(format!(
            "offset_counts.len() ({}) != Σ width×height ({})",
            section.offset_counts.len(),
            running,
        ));
    }

    for (i, tl) in section.texel_lights.iter().enumerate() {
        if tl.light_index >= animated_light_count {
            return Err(format!(
                "texel_lights[{}].light_index ({}) >= animated_light_count ({})",
                i, tl.light_index, animated_light_count,
            ));
        }
    }
    for (i, oc) in section.offset_counts.iter().enumerate() {
        let end = (oc.offset as usize)
            .checked_add(oc.count as usize)
            .ok_or_else(|| format!("offset_counts[{i}] end overflow"))?;
        if end > section.texel_lights.len() {
            return Err(format!(
                "offset_counts[{}] range {}..{} exceeds texel_lights.len() ({})",
                i,
                oc.offset,
                end,
                section.texel_lights.len(),
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
        AnimatedLightWeightMapsSection, ChunkAtlasRect, TexelLight, TexelLightEntry,
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
            atlas_x: 0,
            atlas_y: 0,
            width: w,
            height: h,
            texel_offset: offset,
            layer: 0,
        }
    }

    fn mk_section(
        chunk_rects: Vec<ChunkAtlasRect>,
        offset_counts: Vec<TexelLightEntry>,
        texel_lights: Vec<TexelLight>,
    ) -> AnimatedLightWeightMapsSection {
        AnimatedLightWeightMapsSection {
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

    #[test]
    fn validate_cross_section_accepts_valid_section() {
        let section = mk_section(
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
        );
        let chunks = mk_chunks(1);
        assert!(validate_cross_section(&section, Some(&chunks), 1, &[0], (8, 8)).is_ok());
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
        let err = validate_cross_section(&section, Some(&chunks), 0, &[0], (8, 8)).unwrap_err();
        assert!(err.contains("prefix sum"), "unexpected error: {err}");
    }

    #[test]
    fn validate_cross_section_rejects_out_of_range_light_index() {
        let section = mk_section(
            vec![mk_rect(1, 1, 0)],
            vec![TexelLightEntry {
                offset: 0,
                count: 1,
            }],
            vec![TexelLight {
                light_index: 42,
                weight: 1.0,
                direction_oct: [32768, 65535],
            }],
        );
        let chunks = mk_chunks(1);
        let err = validate_cross_section(&section, Some(&chunks), 5, &[0], (8, 8)).unwrap_err();
        assert!(err.contains("light_index"), "unexpected error: {err}");
    }

    #[test]
    fn validate_cross_section_rejects_offset_count_out_of_range() {
        let section = mk_section(
            vec![mk_rect(1, 1, 0)],
            vec![TexelLightEntry {
                offset: 0,
                count: 5,
            }],
            vec![TexelLight {
                light_index: 0,
                weight: 1.0,
                direction_oct: [32768, 65535],
            }],
        );
        let chunks = mk_chunks(1);
        let err = validate_cross_section(&section, Some(&chunks), 1, &[0], (8, 8)).unwrap_err();
        assert!(err.contains("texel_lights.len"), "unexpected error: {err}");
    }

    #[test]
    fn validate_cross_section_rejects_offset_counts_length_mismatch() {
        let section = mk_section(
            vec![mk_rect(2, 2, 0)],
            vec![
                TexelLightEntry {
                    offset: 0,
                    count: 0,
                };
                3
            ],
            vec![],
        );
        let chunks = mk_chunks(1);
        let err = validate_cross_section(&section, Some(&chunks), 0, &[0], (8, 8)).unwrap_err();
        assert!(err.contains("offset_counts.len"), "unexpected error: {err}");
    }

    #[test]
    fn validate_cross_section_rejects_missing_chunks_when_weight_maps_present() {
        let section = mk_section(
            vec![mk_rect(1, 1, 0)],
            vec![TexelLightEntry {
                offset: 0,
                count: 0,
            }],
            vec![],
        );
        let err = validate_cross_section(&section, None, 0, &[0], (8, 8)).unwrap_err();
        assert!(err.contains("AnimatedLightChunks") && err.contains("malformed"));
    }

    #[test]
    fn validate_cross_section_accepts_empty_weight_maps_without_chunks() {
        let section = mk_section(vec![], vec![], vec![]);
        assert!(validate_cross_section(&section, None, 0, &[], (8, 8)).is_ok());
    }

    #[test]
    fn validate_cross_section_rejects_rect_layer_absent_from_slot_table() {
        let mut rect = mk_rect(1, 1, 0);
        rect.layer = 4;
        let section = mk_section(
            vec![rect],
            vec![TexelLightEntry {
                offset: 0,
                count: 0,
            }],
            vec![],
        );
        let chunks = mk_chunks(1);

        let err = validate_cross_section(&section, Some(&chunks), 0, &[0], (8, 8)).unwrap_err();
        assert!(err.contains("absent from the animated slot table"));
    }

    #[test]
    fn validate_cross_section_rejects_rect_outside_static_atlas_bounds() {
        let mut rect = mk_rect(2, 1, 0);
        rect.atlas_x = 7;
        let section = mk_section(
            vec![rect],
            vec![
                TexelLightEntry {
                    offset: 0,
                    count: 0,
                };
                2
            ],
            vec![],
        );
        let chunks = mk_chunks(1);

        let err = validate_cross_section(&section, Some(&chunks), 0, &[0], (8, 8)).unwrap_err();
        assert!(err.contains("exceeds static atlas"));
    }
}
