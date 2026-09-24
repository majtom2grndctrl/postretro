//! Same-handle validation for staged PRL output.
//! See: context/lib/build_pipeline.md §PRL Compilation

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use postretro_level_format::cluster_directory::{
    ClusterDirectorySection, ClusterDirectoryValidationInputs,
};
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
    cluster_directory_validation: Option<ClusterDirectoryValidationInputs<'_>>,
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
    if let Some(inputs) = cluster_directory_validation {
        validate_cluster_directory_semantics_readback(file, &meta, inputs)?;
    }

    Ok(())
}

/// Validate staged id 49 with the finalized metadata that produced it.
///
/// Structural decode alone cannot prove canonical cell membership, seam
/// endpoint cuts, or resource ranges. The compiler already retains those
/// finalized sections through publication, so borrow them instead of decoding
/// duplicate SH payloads from the staged file.
fn validate_cluster_directory_semantics_readback(
    file: &mut File,
    meta: &postretro_level_format::ContainerMeta,
    inputs: ClusterDirectoryValidationInputs<'_>,
) -> anyhow::Result<()> {
    let directory_bytes = read_section_data(file, meta, SectionId::ClusterDirectory as u32)?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "section 49 is required when finalized ClusterDirectory metadata is supplied"
            )
        })?;
    let directory = ClusterDirectorySection::from_bytes(&directory_bytes).map_err(|error| {
        anyhow::anyhow!("section 49 failed same-handle structural read-back: {error}")
    })?;
    directory.validate_semantics(inputs).map_err(|error| {
        anyhow::anyhow!("section 49 failed same-handle semantic read-back: {error}")
    })
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

#[cfg(test)]
mod tests {
    use std::io::Write;

    use postretro_level_format::bvh::BvhSection;
    use postretro_level_format::cell_locator::{CellLocatorChild, CellLocatorSection};
    use postretro_level_format::cells::{CellRecord, CellsSection};
    use postretro_level_format::cluster_directory::{
        CLUSTER_DIRECTORY_CONTAINER_VERSION, ClusterDirectoryShInventory,
    };
    use postretro_level_format::portals::PortalsSection;
    use postretro_level_format::{SectionBlob, write_prl};
    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn cluster_directory_semantics_borrow_finalized_metadata() {
        let directory = ClusterDirectorySection {
            runtime_cell_count: 0,
            primitive_limit: 1,
            cell_limit: 1,
            clusters: Vec::new(),
            resources: Vec::new(),
            members: Vec::new(),
            ranges: Vec::new(),
            seam_portal_ids: Vec::new(),
            cluster_hints: Vec::new(),
        };
        let directory_bytes = directory
            .try_to_bytes()
            .expect("empty directory should encode");
        let descriptors = [SectionDescriptor {
            section_id: SectionId::ClusterDirectory as u32,
            version: CLUSTER_DIRECTORY_CONTAINER_VERSION,
            byte_len: u64::try_from(directory_bytes.len()).expect("directory length fits u64"),
        }];
        let mut staged = NamedTempFile::new().expect("staged PRL should open");
        write_prl(
            staged.as_file_mut(),
            &[SectionBlob {
                section_id: SectionId::ClusterDirectory as u32,
                version: CLUSTER_DIRECTORY_CONTAINER_VERSION,
                data: directory_bytes,
            }],
        )
        .expect("directory-only PRL should encode");
        staged
            .as_file_mut()
            .flush()
            .expect("staged PRL should flush");

        let empty_cells = CellsSection {
            cells: Vec::new(),
            portal_refs: Vec::new(),
        };
        let portals = PortalsSection {
            vertices: Vec::new(),
            portals: Vec::new(),
        };
        let bvh = BvhSection {
            nodes: Vec::new(),
            leaves: Vec::new(),
            root_node_index: 0,
        };
        let locator = CellLocatorSection {
            root: CellLocatorChild::Cell(0),
            nodes: Vec::new(),
        };
        let empty_inputs = ClusterDirectoryValidationInputs {
            cells: &empty_cells,
            portals: &portals,
            bvh: &bvh,
            cell_locator: &locator,
            sh: ClusterDirectoryShInventory::default(),
        };
        validate_readback(staged.as_file_mut(), &descriptors, Some(empty_inputs))
            .expect("borrowed metadata validates without staged SH companion sections");

        let mut missing_directory = NamedTempFile::new().expect("staged PRL should open");
        write_prl(missing_directory.as_file_mut(), &[])
            .expect("empty PRL should encode for missing-directory coverage");
        let missing_error =
            validate_readback(missing_directory.as_file_mut(), &[], Some(empty_inputs))
                .expect_err("finalized ClusterDirectory metadata requires staged id 49");
        assert!(
            missing_error.to_string().contains(
                "section 49 is required when finalized ClusterDirectory metadata is supplied"
            ),
            "unexpected missing-directory error: {missing_error:#}",
        );

        let cells_with_one_runtime_cell = CellsSection {
            cells: vec![CellRecord {
                bounds_min: [0.0, 0.0, 0.0],
                bounds_max: [1.0, 1.0, 1.0],
                flags: 0,
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
            }],
            portal_refs: Vec::new(),
        };
        let mismatched_inputs = ClusterDirectoryValidationInputs {
            cells: &cells_with_one_runtime_cell,
            portals: &portals,
            bvh: &bvh,
            cell_locator: &locator,
            sh: ClusterDirectoryShInventory::default(),
        };
        let error = validate_readback(staged.as_file_mut(), &descriptors, Some(mismatched_inputs))
            .expect_err("same-handle id 49 validation must use borrowed finalized cells");
        assert!(
            error
                .to_string()
                .contains("runtime_cell_count 0 disagrees with Cells count 1"),
            "unexpected semantic read-back error: {error:#}",
        );
    }
}
