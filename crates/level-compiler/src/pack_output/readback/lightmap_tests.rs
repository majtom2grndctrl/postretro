use super::*;
use std::io::{self, Cursor, Write};

use postretro_level_format::lightmap::{
    IRRADIANCE_FORMAT_RGBA16F, LightmapBlock, LightmapMode, LightmapSection,
};
use postretro_level_format::shadowmask_atlas::{NO_FACE_OWNERS, ShadowmaskAtlasSection};
use postretro_level_format::{SectionBlob, write_prl};

fn sections() -> (LightmapSection, ShadowmaskAtlasSection) {
    let lightmap = LightmapSection {
        direction_texel_scale: 2,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        mode: LightmapMode::Shadowed,
        blocks: (0..3)
            .map(|cell_id| LightmapBlock {
                cell_id,
                width: 64,
                height: 64,
                irradiance: vec![cell_id as u8; 64 * 64 * 8],
                direction: vec![7 + cell_id as u8; 32 * 32 * 2],
            })
            .collect(),
    };
    let shadowmask = ShadowmaskAtlasSection {
        owners: vec![NO_FACE_OWNERS; 3],
        blocks: (0..3)
            .map(|seed| [vec![seed; 4096], vec![seed + 1; 4096]])
            .collect(),
    };
    (lightmap, shadowmask)
}

/// Fail at the actual read seam if validation requests a whole payload.
struct BoundedReader {
    bytes: Cursor<Vec<u8>>,
    max_read: usize,
    total_read: usize,
}

impl Read for BoundedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        assert!(buffer.len() <= SECTION_READBACK_SCRATCH_BYTES);
        self.max_read = self.max_read.max(buffer.len());
        let count = self.bytes.read(buffer)?;
        self.total_read += count;
        Ok(count)
    }
}

impl Seek for BoundedReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.bytes.seek(from)
    }
}

struct BoundedWriter<'a> {
    writer: &'a mut dyn Write,
    limit: usize,
}

impl Write for BoundedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        assert!(
            bytes.len() <= self.limit,
            "whole-section write exceeds the block bound"
        );
        self.writer.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

fn entry(section_id: SectionId, bytes: &[u8]) -> SectionEntry {
    SectionEntry {
        section_id: section_id as u32,
        offset: 0,
        size: bytes.len() as u64,
        version: 1,
    }
}

#[test]
fn lightmap_family_readback_scans_bodies_with_fixed_scratch_and_validates_ranges() {
    let (lightmap, shadowmask) = sections();
    let bytes = lightmap.to_bytes();
    let frame = entry(SectionId::Lightmap, &bytes);
    let mut reader = BoundedReader {
        bytes: Cursor::new(bytes),
        max_read: 0,
        total_read: 0,
    };
    let index = validate_lightmap_readback(&mut reader, &frame, frame.size).unwrap();
    assert_eq!(reader.total_read as u64, frame.size);
    assert_eq!(reader.max_read, SECTION_READBACK_SCRATCH_BYTES);
    assert!(frame.size > SECTION_READBACK_SCRATCH_BYTES as u64);

    // Corrupt the first irradiance offset while preserving the framed length.
    reader.bytes.get_mut()[LIGHTMAP_HEADER_BYTES + 8..LIGHTMAP_HEADER_BYTES + 16]
        .copy_from_slice(&0u64.to_le_bytes());
    assert!(validate_lightmap_readback(&mut reader, &frame, frame.size).is_err());

    let bytes = shadowmask.to_bytes();
    let frame = entry(SectionId::ShadowmaskAtlas, &bytes);
    let mut reader = BoundedReader {
        bytes: Cursor::new(bytes),
        max_read: 0,
        total_read: 0,
    };
    validate_shadowmask_readback(&mut reader, &frame, frame.size, &index).unwrap();
    assert_eq!(reader.total_read as u64, frame.size);
    assert_eq!(reader.max_read, SECTION_READBACK_SCRATCH_BYTES);
    assert!(frame.size > SECTION_READBACK_SCRATCH_BYTES as u64);
    // Group A starts in the first record, past the owner table and count.
    reader.bytes.get_mut()[8 + 3 * 8 + 4..8 + 3 * 8 + 12].copy_from_slice(&0u64.to_le_bytes());
    assert!(validate_shadowmask_readback(&mut reader, &frame, frame.size, &index).is_err());
}

#[test]
fn streamed_lightmap_family_prl_matches_contiguous_container_bytes() {
    use super::super::{PlannedSection, write_and_validate_sections};
    let (lightmap, shadowmask) = sections();
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("streamed.prl");
    let lightmap_bound = lightmap.index_bytes().len().max(64 * 64 * 8 + 32 * 32 * 2);
    let shadowmask_bound = shadowmask.index_bytes().len().max(2 * 4096);
    assert!(lightmap_bound < lightmap.byte_len());
    assert!(shadowmask_bound < shadowmask.byte_len());
    let mut expected = Vec::new();
    write_prl(
        &mut expected,
        &[
            SectionBlob {
                section_id: SectionId::Lightmap as u32,
                version: 1,
                data: lightmap.to_bytes(),
            },
            SectionBlob {
                section_id: SectionId::ShadowmaskAtlas as u32,
                version: 2,
                data: shadowmask.to_bytes(),
            },
        ],
    )
    .unwrap();
    write_and_validate_sections(
        &output,
        vec![
            PlannedSection::with_writer(
                SectionId::Lightmap as u32,
                1,
                lightmap.byte_len() as u64,
                |writer| {
                    Ok(lightmap.write_to(&mut BoundedWriter {
                        writer,
                        limit: lightmap_bound,
                    })?)
                },
            ),
            PlannedSection::with_writer(
                SectionId::ShadowmaskAtlas as u32,
                2,
                shadowmask.byte_len() as u64,
                |writer| {
                    Ok(shadowmask.write_to(&mut BoundedWriter {
                        writer,
                        limit: shadowmask_bound,
                    })?)
                },
            ),
        ],
    )
    .unwrap();
    assert_eq!(std::fs::read(output).unwrap(), expected);
}
