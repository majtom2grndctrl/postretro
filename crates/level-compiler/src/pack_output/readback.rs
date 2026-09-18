//! Same-handle validation for staged PRL output.
//! See: context/lib/build_pipeline.md §PRL Compilation

use std::fs::File;
use std::io::{Seek, SeekFrom};

use postretro_level_format::{SectionDescriptor, read_container, read_section_data};

/// Validate the flushed PRL through the handle that received the bytes.
pub(super) fn validate_readback(
    file: &mut File,
    expected_sections: &[SectionDescriptor],
) -> anyhow::Result<()> {
    file.seek(SeekFrom::Start(0))?;
    let meta = read_container(&mut *file)?;

    anyhow::ensure!(
        meta.header.section_count as usize == expected_sections.len(),
        "expected {} sections, got {}",
        expected_sections.len(),
        meta.header.section_count
    );

    let mut expected_offset = 8 + expected_sections.len() as u64 * 22;
    for (index, expected) in expected_sections.iter().enumerate() {
        let entry = meta.sections.get(index).ok_or_else(|| {
            anyhow::anyhow!("section ID {} missing from read-back", expected.section_id)
        })?;
        anyhow::ensure!(
            entry.section_id == expected.section_id,
            "section table order differs at index {index}: expected ID {}, got {}",
            expected.section_id,
            entry.section_id,
        );
        anyhow::ensure!(
            entry.offset == expected_offset,
            "section ID {} offset {} does not match expected {}",
            expected.section_id,
            entry.offset,
            expected_offset,
        );
        anyhow::ensure!(
            entry.size == expected.byte_len && entry.version == expected.version,
            "section ID {} table entry differs from the declared length or version",
            expected.section_id,
        );
        let actual =
            read_section_data(&mut *file, &meta, expected.section_id)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "section ID {} data missing from read-back",
                    expected.section_id
                )
            })?;
        anyhow::ensure!(
            actual.len() as u64 == expected.byte_len,
            "section ID {} framed payload length {} does not match expected {}",
            expected.section_id,
            actual.len(),
            expected.byte_len,
        );
        expected_offset += expected.byte_len;
    }
    let file_len = file.metadata()?.len();
    anyhow::ensure!(
        expected_offset == file_len,
        "section table ends at {expected_offset}, but the file is {file_len} bytes",
    );

    Ok(())
}
