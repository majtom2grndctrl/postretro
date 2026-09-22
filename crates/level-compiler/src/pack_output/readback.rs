//! Same-handle validation for staged PRL output.
//! See: context/lib/build_pipeline.md §PRL Compilation

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_format::cluster_sh_payloads::{
    CLUSTER_SH_PAYLOADS_CONTAINER_VERSION, CLUSTER_SH_PAYLOADS_HEADER_SIZE,
    ClusterShPayloadsSection,
};
use postretro_level_format::{
    SectionDescriptor, SectionEntry, SectionId, read_container, read_section_data,
};

/// Validate the flushed PRL through the handle that received the bytes.
pub(super) fn validate_readback(
    file: &mut File,
    expected_sections: &[SectionDescriptor],
) -> anyhow::Result<()> {
    file.seek(SeekFrom::Start(0))?;
    let meta = read_container(&mut *file)?;
    let file_len = file.metadata()?.len();

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
        if expected.section_id == SectionId::ClusterShPayloads as u32 {
            validate_cluster_sh_payloads_readback(file, entry, file_len)?;
        } else {
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
            if expected.section_id == SectionId::ClusterDirectory as u32 {
                ClusterDirectorySection::from_bytes(&actual).map_err(|error| {
                    anyhow::anyhow!("section 49 failed same-handle semantic read-back: {error}")
                })?;
            }
        }
        expected_offset += expected.byte_len;
    }
    anyhow::ensure!(
        expected_offset == file_len,
        "section table ends at {expected_offset}, but the file is {file_len} bytes",
    );

    Ok(())
}

/// Validate id 50 while retaining at most its metadata prefix and one chunk.
///
/// The compiler validates id-50's shared metadata and index contract before
/// publishing its spool. This same-handle pass confirms that the staged writer
/// copied every exact indexed chunk without constructing a whole-payload buffer.
fn validate_cluster_sh_payloads_readback(
    file: &mut File,
    entry: &SectionEntry,
    file_len: u64,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        entry.version == CLUSTER_SH_PAYLOADS_CONTAINER_VERSION,
        "section 50 container version {} is not {}",
        entry.version,
        CLUSTER_SH_PAYLOADS_CONTAINER_VERSION,
    );
    anyhow::ensure!(
        entry.size >= CLUSTER_SH_PAYLOADS_HEADER_SIZE as u64,
        "section 50 is shorter than its fixed metadata header",
    );
    let entry_end = entry
        .offset
        .checked_add(entry.size)
        .ok_or_else(|| anyhow::anyhow!("section 50 framed range overflows u64"))?;
    anyhow::ensure!(
        entry_end <= file_len,
        "section 50 framed range ends at {entry_end}, beyond staged PRL length {file_len}",
    );

    let mut header = [0u8; CLUSTER_SH_PAYLOADS_HEADER_SIZE];
    file.seek(SeekFrom::Start(entry.offset))?;
    file.read_exact(&mut header)?;
    let metadata_len =
        ClusterShPayloadsSection::metadata_len_from_header(&header).map_err(|error| {
            anyhow::anyhow!("section 50 header failed same-handle read-back: {error}")
        })?;
    let metadata_len_u64 = u64::try_from(metadata_len)?;
    anyhow::ensure!(
        metadata_len_u64 <= entry.size,
        "section 50 metadata prefix {} exceeds its framed length {}",
        metadata_len,
        entry.size,
    );
    let mut metadata = Vec::new();
    metadata
        .try_reserve_exact(metadata_len)
        .map_err(|_| anyhow::anyhow!("section 50 metadata allocation failed"))?;
    metadata.resize(metadata_len, 0);
    metadata[..CLUSTER_SH_PAYLOADS_HEADER_SIZE].copy_from_slice(&header);
    if metadata_len > CLUSTER_SH_PAYLOADS_HEADER_SIZE {
        file.read_exact(&mut metadata[CLUSTER_SH_PAYLOADS_HEADER_SIZE..])?;
    }
    let section =
        ClusterShPayloadsSection::from_metadata_bytes(&metadata, entry.size).map_err(|error| {
            anyhow::anyhow!("section 50 metadata failed same-handle read-back: {error}")
        })?;
    let payload_start = entry
        .offset
        .checked_add(metadata_len_u64)
        .ok_or_else(|| anyhow::anyhow!("section 50 payload start overflows u64"))?;
    for (cluster_id, index) in section.index.iter().enumerate() {
        let range = section
            .payload_range(u32::try_from(cluster_id)?)
            .map_err(|error| anyhow::anyhow!("section 50 chunk range is invalid: {error}"))?;
        let chunk_offset = payload_start
            .checked_add(range.start)
            .ok_or_else(|| anyhow::anyhow!("section 50 chunk offset overflows u64"))?;
        let chunk_len = usize::try_from(index.payload_len)
            .map_err(|_| anyhow::anyhow!("section 50 chunk {cluster_id} length exceeds usize"))?;
        let mut chunk = Vec::new();
        chunk
            .try_reserve_exact(chunk_len)
            .map_err(|_| anyhow::anyhow!("section 50 chunk {cluster_id} allocation failed"))?;
        chunk.resize(chunk_len, 0);
        file.seek(SeekFrom::Start(chunk_offset))?;
        file.read_exact(&mut chunk)?;
        anyhow::ensure!(
            *blake3::hash(&chunk).as_bytes() == index.hash,
            "section 50 chunk {cluster_id} hash failed same-handle read-back",
        );
    }
    Ok(())
}
