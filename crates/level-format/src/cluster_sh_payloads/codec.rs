//! Id-50 metadata codec, section structure checks, and public worker entry points.
//! See: context/lib/build_pipeline.md §PRL section IDs.

use super::*;

impl ClusterShPayloadsSection {
    pub fn metadata_len_from_header(bytes: &[u8]) -> Result<usize, ClusterShPayloadsError> {
        Self::parse_header(bytes)?.metadata_len()
    }

    pub fn parse_header(bytes: &[u8]) -> Result<ClusterShPayloadsHeader, ClusterShPayloadsError> {
        if bytes.len() < CLUSTER_SH_PAYLOADS_HEADER_SIZE {
            return invalid(format!(
                "section too short for {CLUSTER_SH_PAYLOADS_HEADER_SIZE}-byte header: got {}",
                bytes.len()
            ));
        }
        let epoch = read_u32(bytes, 0);
        if epoch != CLUSTER_SH_PAYLOADS_VERSION {
            return Err(ClusterShPayloadsError::VersionMismatch {
                version: epoch,
                expected: CLUSTER_SH_PAYLOADS_VERSION,
            });
        }
        let header = ClusterShPayloadsHeader {
            cluster_count: read_u32(bytes, 4),
            source_count: read_u32(bytes, 8),
            grid_dimensions: [
                read_u32(bytes, 16),
                read_u32(bytes, 20),
                read_u32(bytes, 24),
            ],
            affinity_dimensions: [
                read_u32(bytes, 28),
                read_u32(bytes, 32),
                read_u32(bytes, 36),
            ],
            payload_bytes: read_u64(bytes, 56),
        };
        if read_u32(bytes, 12) != 0
            || read_u32(bytes, 40) != CLUSTER_SH_LOGICAL_TILE_DIMENSION
            || read_u32(bytes, 44) != CLUSTER_SH_LOGICAL_TILE_BORDER
            || read_u32(bytes, 48) != CLUSTER_SH_PHYSICAL_TILE_STRIDE
            || read_u32(bytes, 52) != 0
            || read_u64(bytes, 64) != 0
        {
            return invalid("header flags, geometry, or reserved fields are not canonical".into());
        }
        Ok(header)
    }

    /// Parse a header/source/index prefix that was fetched separately from the
    /// payload. `section_len` is the container entry's bounded byte length.
    pub fn from_metadata_bytes(
        bytes: &[u8],
        section_len: u64,
    ) -> Result<Self, ClusterShPayloadsError> {
        let header = Self::parse_header(bytes)?;
        let metadata_len = header.metadata_len()?;
        if bytes.len() != metadata_len {
            return Err(ClusterShPayloadsError::RangeOutOfBounds(format!(
                "metadata prefix is {} bytes, expected exactly {metadata_len}",
                bytes.len()
            )));
        }
        let total_len = u64::try_from(metadata_len)
            .map_err(|_| ClusterShPayloadsError::SizeOverflow("metadata length exceeds u64"))?
            .checked_add(header.payload_bytes)
            .ok_or(ClusterShPayloadsError::SizeOverflow("section length"))?;
        if total_len != section_len {
            return Err(ClusterShPayloadsError::RangeOutOfBounds(format!(
                "metadata plus payload is {total_len} bytes, container section is {section_len}"
            )));
        }

        let source_count = usize::try_from(header.source_count)
            .map_err(|_| ClusterShPayloadsError::SizeOverflow("source count exceeds usize"))?;
        let index_count = usize::try_from(header.cluster_count)
            .map_err(|_| ClusterShPayloadsError::SizeOverflow("cluster count exceeds usize"))?;
        let mut sources = try_vec(source_count, "source records")?;
        let mut cursor = CLUSTER_SH_PAYLOADS_HEADER_SIZE;
        for _ in 0..source_count {
            let reserved = read_u32(bytes, cursor + 12);
            if reserved != 0 {
                return invalid(format!(
                    "source reserved field is {reserved}, expected zero"
                ));
            }
            sources.push(ClusterShPayloadsSourceRecord {
                section_id: read_u32(bytes, cursor),
                internal_version: read_u32(bytes, cursor + 4),
                kind: ClusterShPayloadsSourceKind::parse(read_u32(bytes, cursor + 8))?,
            });
            cursor += CLUSTER_SH_PAYLOADS_SOURCE_RECORD_SIZE;
        }
        let mut index = try_vec(index_count, "cluster index records")?;
        for _ in 0..index_count {
            if read_u32(bytes, cursor + 44) != 0 {
                return invalid("cluster index flags must be zero".into());
            }
            let mut hash = [0; 32];
            hash.copy_from_slice(&bytes[cursor + 48..cursor + 80]);
            index.push(ClusterShPayloadsIndexRecord {
                payload_offset: read_u64(bytes, cursor),
                payload_len: read_u64(bytes, cursor + 8),
                decoded_bytes: read_u64(bytes, cursor + 16),
                requested_resident_bytes: read_u64(bytes, cursor + 24),
                stored_tile_count: read_u32(bytes, cursor + 32),
                dense_patch_count: read_u32(bytes, cursor + 36),
                affinity_patch_count: read_u32(bytes, cursor + 40),
                hash,
            });
            cursor += CLUSTER_SH_PAYLOADS_INDEX_RECORD_SIZE;
        }
        debug_assert_eq!(cursor, metadata_len);
        let section = Self {
            header,
            sources,
            index,
        };
        section.validate_structure()?;
        Ok(section)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ClusterShPayloadsError> {
        let section_len = u64::try_from(bytes.len())
            .map_err(|_| ClusterShPayloadsError::SizeOverflow("section length exceeds u64"))?;
        let metadata_len = Self::metadata_len_from_header(bytes)?;
        if metadata_len > bytes.len() {
            return Err(ClusterShPayloadsError::RangeOutOfBounds(format!(
                "section has {} bytes but metadata requires {metadata_len}",
                bytes.len()
            )));
        }
        let section = Self::from_metadata_bytes(&bytes[..metadata_len], section_len)?;
        section.validate_payload_hashes(&bytes[metadata_len..])?;
        Ok(section)
    }

    pub fn metadata_bytes(&self) -> Result<Vec<u8>, ClusterShPayloadsError> {
        self.validate_structure()?;
        let len = self.header.metadata_len()?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(len)
            .map_err(|_| ClusterShPayloadsError::AllocationFailed("metadata bytes"))?;
        push_u32(&mut bytes, CLUSTER_SH_PAYLOADS_VERSION);
        push_u32(&mut bytes, self.header.cluster_count);
        push_u32(&mut bytes, self.header.source_count);
        push_u32(&mut bytes, 0);
        for dimension in self.header.grid_dimensions {
            push_u32(&mut bytes, dimension);
        }
        for dimension in self.header.affinity_dimensions {
            push_u32(&mut bytes, dimension);
        }
        push_u32(&mut bytes, CLUSTER_SH_LOGICAL_TILE_DIMENSION);
        push_u32(&mut bytes, CLUSTER_SH_LOGICAL_TILE_BORDER);
        push_u32(&mut bytes, CLUSTER_SH_PHYSICAL_TILE_STRIDE);
        push_u32(&mut bytes, 0);
        push_u64(&mut bytes, self.header.payload_bytes);
        push_u64(&mut bytes, 0);
        for source in &self.sources {
            push_u32(&mut bytes, source.section_id);
            push_u32(&mut bytes, source.internal_version);
            push_u32(&mut bytes, source.kind as u32);
            push_u32(&mut bytes, 0);
        }
        for entry in &self.index {
            push_u64(&mut bytes, entry.payload_offset);
            push_u64(&mut bytes, entry.payload_len);
            push_u64(&mut bytes, entry.decoded_bytes);
            push_u64(&mut bytes, entry.requested_resident_bytes);
            push_u32(&mut bytes, entry.stored_tile_count);
            push_u32(&mut bytes, entry.dense_patch_count);
            push_u32(&mut bytes, entry.affinity_patch_count);
            push_u32(&mut bytes, 0);
            bytes.extend_from_slice(&entry.hash);
        }
        debug_assert_eq!(bytes.len(), len);
        Ok(bytes)
    }

    pub fn try_to_bytes(&self, payload: &[u8]) -> Result<Vec<u8>, ClusterShPayloadsError> {
        self.validate_structure()?;
        self.validate_payload_hashes(payload)?;
        let metadata = self.metadata_bytes()?;
        let total = metadata.len().checked_add(payload.len()).ok_or(
            ClusterShPayloadsError::SizeOverflow("encoded section length"),
        )?;
        let mut bytes = metadata;
        bytes
            .try_reserve_exact(payload.len())
            .map_err(|_| ClusterShPayloadsError::AllocationFailed("encoded section"))?;
        bytes.extend_from_slice(payload);
        debug_assert_eq!(bytes.len(), total);
        Ok(bytes)
    }

    /// Check an already-buffered payload without decoding it. The startup path
    /// intentionally does not call this; worker jobs call [`decode_chunk`] on
    /// one selected range instead.
    pub fn validate_payload_hashes(&self, payload: &[u8]) -> Result<(), ClusterShPayloadsError> {
        self.validate_structure()?;
        if u64::try_from(payload.len())
            .map_err(|_| ClusterShPayloadsError::SizeOverflow("payload length exceeds u64"))?
            != self.header.payload_bytes
        {
            return invalid("payload byte length disagrees with header".into());
        }
        for (cluster_id, entry) in self.index.iter().enumerate() {
            let range = self.payload_range(cluster_id as u32)?;
            let start = usize::try_from(range.start).map_err(|_| {
                ClusterShPayloadsError::SizeOverflow("payload range start exceeds usize")
            })?;
            let end = usize::try_from(range.end).map_err(|_| {
                ClusterShPayloadsError::SizeOverflow("payload range end exceeds usize")
            })?;
            if end > payload.len() {
                return Err(ClusterShPayloadsError::RangeOutOfBounds(format!(
                    "cluster {cluster_id} payload range {start}..{end} exceeds buffered payload {}",
                    payload.len()
                )));
            }
            if *blake3::hash(&payload[start..end]).as_bytes() != entry.hash {
                return Err(ClusterShPayloadsError::HashMismatch {
                    cluster_id: cluster_id as u32,
                });
            }
        }
        Ok(())
    }

    pub fn payload_range(
        &self,
        cluster_id: u32,
    ) -> Result<std::ops::Range<u64>, ClusterShPayloadsError> {
        let entry = self.index.get(cluster_id as usize).ok_or_else(|| {
            ClusterShPayloadsError::RangeOutOfBounds(format!(
                "cluster {cluster_id} is outside {} index records",
                self.index.len()
            ))
        })?;
        let end = entry
            .payload_offset
            .checked_add(entry.payload_len)
            .ok_or(ClusterShPayloadsError::SizeOverflow("chunk payload range"))?;
        Ok(entry.payload_offset..end)
    }

    /// Validate the prefix against id 49 and metadata projections. This is the
    /// loader-startup entry point and intentionally never receives payload bytes.
    pub fn validate_against(
        &self,
        inputs: ClusterShPayloadsValidationInputs<'_>,
    ) -> Result<(), ClusterShPayloadsError> {
        self.validation_plan(inputs).map(drop)
    }

    /// Consume this metadata prefix after validating its complete directory,
    /// source inventory, and index. The returned wrapper retains the canonical
    /// per-cluster plans so worker decodes never rebuild whole-level state.
    pub fn into_validated(
        self,
        inputs: ClusterShPayloadsValidationInputs<'_>,
    ) -> Result<ValidatedClusterShPayloadsSection, ClusterShPayloadsError> {
        let expected_chunks = self.validation_plan(inputs)?;
        Ok(ValidatedClusterShPayloadsSection {
            section: self,
            expected_chunks,
        })
    }

    fn validation_plan(
        &self,
        inputs: ClusterShPayloadsValidationInputs<'_>,
    ) -> Result<Vec<ExpectedChunk>, ClusterShPayloadsError> {
        self.validate_structure()?;
        validate_inputs(inputs)?;
        if self.header.cluster_count != inputs.directory.clusters.len() as u32 {
            return source_mismatch(format!(
                "header cluster_count {} disagrees with id 49 count {}",
                self.header.cluster_count,
                inputs.directory.clusters.len()
            ));
        }
        if self.header.grid_dimensions != inputs.base.grid_dimensions {
            return source_mismatch("header grid dimensions disagree with id 34".into());
        }
        let affinity_dimensions = affinity_dimensions(inputs.base.grid_dimensions)?;
        if self.header.affinity_dimensions != affinity_dimensions {
            return source_mismatch("header affinity dimensions disagree with id 34".into());
        }
        validate_source_inventory(&self.sources, inputs.sources)?;

        let plan = ValidationPlan::new(inputs)?;
        let mut expected_chunks = try_vec(
            usize::try_from(self.header.cluster_count)
                .map_err(|_| ClusterShPayloadsError::SizeOverflow("cluster count exceeds usize"))?,
            "validated cluster plans",
        )?;
        for cluster_id in 0..self.header.cluster_count {
            let expected = plan.expected_chunk(cluster_id)?;
            let actual = &self.index[cluster_id as usize];
            validate_index_against_expected(cluster_id, actual, &expected)?;
            expected_chunks.push(expected);
        }
        Ok(expected_chunks)
    }

    /// Verify one previously-read chunk and retain its single allocation for a
    /// later renderer handoff. The caller must pass exactly this index entry's
    /// payload bytes; no other chunk or payload data is inspected.
    pub fn decode_chunk(
        &self,
        cluster_id: u32,
        bytes: Vec<u8>,
        inputs: ClusterShPayloadsValidationInputs<'_>,
    ) -> Result<DecodedClusterShPayload, ClusterShPayloadsError> {
        let expected_chunks = self.validation_plan(inputs)?;
        self.decode_chunk_with_expected(cluster_id, bytes, &expected_chunks)
    }

    fn decode_chunk_with_expected(
        &self,
        cluster_id: u32,
        bytes: Vec<u8>,
        expected_chunks: &[ExpectedChunk],
    ) -> Result<DecodedClusterShPayload, ClusterShPayloadsError> {
        let entry = self.index.get(cluster_id as usize).ok_or_else(|| {
            ClusterShPayloadsError::RangeOutOfBounds(format!("unknown cluster {cluster_id}"))
        })?;
        if u64::try_from(bytes.len())
            .map_err(|_| ClusterShPayloadsError::SizeOverflow("chunk byte length exceeds u64"))?
            != entry.payload_len
        {
            return Err(ClusterShPayloadsError::RangeOutOfBounds(format!(
                "cluster {cluster_id} read {} bytes, index requires {}",
                bytes.len(),
                entry.payload_len
            )));
        }
        if *blake3::hash(&bytes).as_bytes() != entry.hash {
            return Err(ClusterShPayloadsError::HashMismatch { cluster_id });
        }
        let expected = expected_chunks.get(cluster_id as usize).ok_or_else(|| {
            ClusterShPayloadsError::RangeOutOfBounds(format!("unknown cluster {cluster_id}"))
        })?;
        let blocks = validate_chunk_bytes(cluster_id, &bytes, expected)?;
        Ok(DecodedClusterShPayload {
            cluster_id,
            bytes,
            blocks,
        })
    }

    pub(super) fn validate_structure(&self) -> Result<(), ClusterShPayloadsError> {
        if self.header.source_count as usize != self.sources.len()
            || self.header.cluster_count as usize != self.index.len()
        {
            return invalid("header counts disagree with source/index table lengths".into());
        }
        let mut previous_source = None;
        for source in &self.sources {
            if !is_streamed_section(source.section_id) {
                return source_mismatch(format!(
                    "source table names unsupported section {}",
                    source.section_id
                ));
            }
            if let Some(previous) = previous_source
                && previous >= source.section_id
            {
                return invalid("source records must be unique and sorted by section id".into());
            }
            previous_source = Some(source.section_id);
        }
        let mut expected_offset = 0u64;
        for (cluster_id, entry) in self.index.iter().enumerate() {
            if entry.payload_offset != expected_offset {
                return Err(ClusterShPayloadsError::RangeOutOfBounds(format!(
                    "cluster {cluster_id} payload offset {} is not ascending contiguous offset {expected_offset}",
                    entry.payload_offset
                )));
            }
            let end = entry
                .payload_offset
                .checked_add(entry.payload_len)
                .ok_or(ClusterShPayloadsError::SizeOverflow("chunk payload range"))?;
            if end > self.header.payload_bytes {
                return Err(ClusterShPayloadsError::RangeOutOfBounds(format!(
                    "cluster {cluster_id} payload end {end} exceeds payload bytes {}",
                    self.header.payload_bytes
                )));
            }
            let empty = entry.payload_len == 0;
            if empty
                && (entry.decoded_bytes != 0
                    || entry.requested_resident_bytes != 0
                    || entry.stored_tile_count != 0
                    || entry.dense_patch_count != 0
                    || entry.affinity_patch_count != 0
                    || entry.hash != *blake3::hash(&[]).as_bytes())
            {
                return invalid(format!(
                    "empty cluster {cluster_id} has non-empty counts or hash"
                ));
            }
            expected_offset = end;
        }
        if expected_offset != self.header.payload_bytes {
            return Err(ClusterShPayloadsError::RangeOutOfBounds(format!(
                "cluster payload ranges end at {expected_offset}, header declares {}",
                self.header.payload_bytes
            )));
        }
        Ok(())
    }
}

/// Fully validated id-50 metadata plus immutable per-cluster decode plans.
/// Construction is only available through
/// [`ClusterShPayloadsSection::into_validated`].
#[derive(Debug)]
pub struct ValidatedClusterShPayloadsSection {
    section: ClusterShPayloadsSection,
    expected_chunks: Vec<ExpectedChunk>,
}

impl ValidatedClusterShPayloadsSection {
    pub fn section(&self) -> &ClusterShPayloadsSection {
        &self.section
    }

    /// Verify only the selected chunk's length, hash, body layout, and semantic
    /// records against the startup-retained validation plan.
    pub fn decode_chunk(
        &self,
        cluster_id: u32,
        bytes: Vec<u8>,
    ) -> Result<DecodedClusterShPayload, ClusterShPayloadsError> {
        self.section
            .decode_chunk_with_expected(cluster_id, bytes, &self.expected_chunks)
    }
}

/// One validated block inside [`DecodedClusterShPayload::bytes`]. Its byte
/// range is local to the retained chunk allocation and always excludes the
/// block-table metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedClusterShBlock {
    pub section_id: u32,
    pub kind: u32,
    pub element_count: u32,
    pub body: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedClusterShPayload {
    pub cluster_id: u32,
    pub bytes: Vec<u8>,
    pub blocks: Vec<DecodedClusterShBlock>,
}

impl DecodedClusterShPayload {
    pub fn block_bytes(&self, block: &DecodedClusterShBlock) -> &[u8] {
        &self.bytes[block.body.clone()]
    }
}
