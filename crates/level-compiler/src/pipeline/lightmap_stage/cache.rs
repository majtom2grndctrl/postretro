//! Persist the encoded lightmap blocks without a second section-sized buffer.
//! See: context/lib/development_guide.md §1.4 (resource lifetime).

use postretro_level_format::lightmap::LightmapSection;

use super::TexelStepReport;
use crate::cache::{CacheKey, StageCache};

/// Store the texel-step report, then the section's wire bytes, as one entry,
/// so a memo hit repeats the cold build's warnings and can never lose them
/// apart from the section.
pub(super) fn cache_lightmap_section(
    cache: &StageCache,
    key: &CacheKey,
    section: &LightmapSection,
    texel_steps: &TexelStepReport,
) {
    let report = texel_steps.to_bytes();
    let entry_len = report.len() + section.byte_len();
    cache.put_streamed(key, entry_len as u64, |writer| {
        writer.write_all(&report)?;
        section.write_to(writer)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::lightmap::{
        IRRADIANCE_FORMAT_RGBA16F, LightmapBlock, LightmapMode,
    };

    #[test]
    fn streamed_lightmap_cache_matches_contiguous_entry_including_report_and_header() {
        let directory = tempfile::tempdir().unwrap();
        let cache = StageCache::new(directory.path()).unwrap();
        let section = LightmapSection {
            direction_texel_scale: 2,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            mode: LightmapMode::Shadowed,
            blocks: (0..3)
                .map(|cell_id| LightmapBlock {
                    cell_id,
                    width: 16,
                    height: 16,
                    irradiance: vec![cell_id as u8 + 1; 16 * 16 * 8],
                    direction: vec![cell_id as u8 + 7; 8 * 8 * 2],
                })
                .collect(),
        };
        let streamed = CacheKey::new("lightmap_section", 1, b"streamed");
        let contiguous = CacheKey::new("lightmap_section_reference", 1, b"contiguous");
        let report = TexelStepReport::default();
        cache_lightmap_section(&cache, &streamed, &section, &report);
        cache.put(
            &contiguous,
            &[report.to_bytes(), section.to_bytes()].concat(),
        );
        assert_eq!(
            std::fs::read(directory.path().join(streamed.as_filename())).unwrap(),
            std::fs::read(directory.path().join(contiguous.as_filename())).unwrap(),
        );
        let access = cache.test_access("lightmap_section");
        assert_eq!(access.streamed_writes, 1);
        let bound = section.index_bytes().len().max(
            section
                .blocks
                .iter()
                .map(|block| block.irradiance.len() + block.direction.len())
                .max()
                .unwrap(),
        );
        assert!(access.max_streamed_write_bytes <= bound);
        assert!(bound < section.byte_len());
    }
}
