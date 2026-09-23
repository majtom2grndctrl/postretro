//! Validated SH streaming manifest assembly and named cluster-payload decoding.
//! See: context/lib/build_pipeline.md §PRL Compilation.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use postretro_level_format::SectionEntry;
use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_format::cluster_sh_payloads::{
    ClusterShPayloadsError, ClusterShPayloadsSection, ClusterShPayloadsValidationInputs,
    DecodedClusterShPayload,
};
use postretro_level_format::{ContainerMeta, SectionId};

use super::boundary::{ShDrainBatch, sorted_lists_intersect, validate_sorted_cluster_ids};
use super::metadata_base::read_base_metadata;
use super::metadata_sparse::{read_optional_direct_metadata, read_optional_sparse_metadata};
use super::positional_io::{read_vec_at, validate_positional_entry_bounds};
use super::projection::{ShStreamBaseMetadata, ShStreamSourceMetadata, validate_projected_sources};
use super::{PrlLoadError, stream_error};
/// Immutable, validated handle for a streaming PRL session. The file is opened
/// exactly once at load; worker jobs use positional reads against this handle
/// and never reopen the diagnostic path.
#[derive(Debug)]
pub struct ShStreamManifest {
    file: Arc<File>,
    diagnostic_path: PathBuf,
    container: ContainerMeta,
    cluster_directory: ClusterDirectorySection,
    payloads: ClusterShPayloadsSection,
    base: ShStreamBaseMetadata,
    sources: ShStreamSourceMetadata,
    content_tag: [u8; 32],
    cluster_adjacency: std::sync::OnceLock<Vec<Vec<u32>>>,
}

impl ShStreamManifest {
    pub fn diagnostic_path(&self) -> &Path {
        &self.diagnostic_path
    }

    pub fn cluster_count(&self) -> u32 {
        self.payloads.header.cluster_count
    }

    pub fn cluster_directory(&self) -> &ClusterDirectorySection {
        &self.cluster_directory
    }

    pub fn payloads(&self) -> &ClusterShPayloadsSection {
        &self.payloads
    }

    pub fn base(&self) -> &ShStreamBaseMetadata {
        &self.base
    }

    pub fn sources(&self) -> &ShStreamSourceMetadata {
        &self.sources
    }

    pub fn content_tag(&self) -> [u8; 32] {
        self.content_tag
    }

    pub fn container(&self) -> &ContainerMeta {
        &self.container
    }

    /// Canonical cluster-neighbor graph derived from the validated portal
    /// topology. It is installed exactly once during the successful level
    /// load, after directory semantic validation.
    pub fn cluster_adjacency(&self) -> &[Vec<u32>] {
        self.cluster_adjacency
            .get()
            .expect("successful streaming manifest installs validated cluster adjacency")
            .as_slice()
    }

    pub(crate) fn install_cluster_adjacency(
        &self,
        adjacency: Vec<Vec<u32>>,
    ) -> Result<(), PrlLoadError> {
        self.cluster_adjacency
            .set(adjacency)
            .map_err(|_| stream_error("cluster adjacency was installed more than once"))
    }

    /// Read and validate one indexed payload chunk through the retained file.
    /// The read is cursor-independent on every supported host.
    pub fn read_and_decode_cluster(
        &self,
        cluster_id: u32,
    ) -> Result<DecodedClusterShPayload, PrlLoadError> {
        let bytes = self.read_encoded_cluster_with(cluster_id, |file, offset, len| {
            read_vec_at(file, offset, len, "id-50 chunk")
        })?;
        self.decode_encoded_cluster(cluster_id, bytes)
    }

    /// Production positional read for a single encoded chunk.
    pub fn read_encoded_cluster(&self, cluster_id: u32) -> Result<Vec<u8>, PrlLoadError> {
        self.read_encoded_cluster_with(cluster_id, |file, offset, len| {
            read_vec_at(file, offset, len, "id-50 chunk")
        })
    }

    /// Read one encoded chunk through an injectable positional reader. Worker
    /// tests can delay this seam without introducing a cursor or reopening the
    /// file; production uses `read_vec_at` and platform `FileExt`.
    pub fn read_encoded_cluster_with(
        &self,
        cluster_id: u32,
        reader: impl FnOnce(&File, u64, u64) -> Result<Vec<u8>, PrlLoadError>,
    ) -> Result<Vec<u8>, PrlLoadError> {
        let range = self.payloads.payload_range(cluster_id)?;
        let payload_entry = self
            .container
            .find_section(SectionId::ClusterShPayloads as u32)
            .ok_or_else(|| stream_error("id 50 disappeared from validated table"))?;
        let metadata_len = u64::try_from(self.payloads.header.metadata_len()?)
            .map_err(|_| ClusterShPayloadsError::SizeOverflow("id-50 metadata length"))?;
        let start = payload_entry
            .offset
            .checked_add(metadata_len)
            .and_then(|offset| offset.checked_add(range.start))
            .ok_or(ClusterShPayloadsError::SizeOverflow(
                "id-50 chunk file offset",
            ))?;
        reader(&self.file, start, range.end - range.start)
    }

    /// Decode and verify the chunk against the validated manifest, including
    /// its per-chunk BLAKE3. Ownership of the encoded vector moves into the
    /// codec rather than being cloned for the renderer handoff.
    pub fn decode_encoded_cluster(
        &self,
        cluster_id: u32,
        bytes: Vec<u8>,
    ) -> Result<DecodedClusterShPayload, PrlLoadError> {
        let sources = self.sources.codec_sources(&self.base);
        Ok(self.payloads.decode_chunk(
            cluster_id,
            bytes,
            ClusterShPayloadsValidationInputs {
                directory: &self.cluster_directory,
                base: self.base.codec_metadata(),
                sources: &sources,
            },
        )?)
    }

    /// Validate the renderer handoff before it crosses the loader boundary.
    /// This is deliberately CPU-only: target policy stays in the planner and
    /// GPU installation stays in the renderer.
    pub fn validate_drain_batch(&self, batch: &ShDrainBatch) -> Result<(), PrlLoadError> {
        if batch.generation == 0 {
            return Err(stream_error("drain batch generation must be nonzero"));
        }
        if batch.content_tag != self.content_tag {
            return Err(stream_error(
                "drain batch content tag does not match manifest",
            ));
        }
        if batch.ready.len() > 2 {
            return Err(stream_error(
                "drain batch exceeds the two-ready-cluster cap",
            ));
        }
        let words = usize::try_from(self.cluster_count().div_ceil(64))
            .map_err(|_| stream_error("cluster bitset word count exceeds usize"))?;
        if let Some(reset) = &batch.target_reset {
            if reset.len() != words {
                return Err(stream_error(
                    "target reset bitset length disagrees with cluster count",
                ));
            }
            if let (Some(last), remainder) = (reset.last(), self.cluster_count() % 64)
                && remainder != 0
                && (*last >> remainder) != 0
            {
                return Err(stream_error(
                    "target reset bitset names an out-of-range cluster",
                ));
            }
        }
        validate_sorted_cluster_ids(&batch.target_add, self.cluster_count(), "target-add")?;
        validate_sorted_cluster_ids(&batch.target_remove, self.cluster_count(), "target-remove")?;
        if sorted_lists_intersect(&batch.target_add, &batch.target_remove) {
            return Err(stream_error(
                "target-add and target-remove must not name the same cluster",
            ));
        }
        validate_sorted_cluster_ids(&batch.evictions, self.cluster_count(), "evictions")?;
        let mut ready_ids = std::collections::BTreeSet::new();
        for prepared in &batch.ready {
            if prepared.generation != batch.generation || prepared.content_tag != batch.content_tag
            {
                return Err(stream_error(
                    "ready cluster identity does not match drain batch",
                ));
            }
            if prepared.chunk.cluster_id >= self.cluster_count() {
                return Err(stream_error(
                    "ready cluster id exceeds manifest cluster count",
                ));
            }
            if !ready_ids.insert(prepared.chunk.cluster_id) {
                return Err(stream_error(
                    "drain batch contains more than one ready chunk for a cluster",
                ));
            }
        }
        Ok(())
    }

    fn validate_payloads(&self) -> Result<(), ClusterShPayloadsError> {
        let sources = self.sources.codec_sources(&self.base);
        self.payloads
            .validate_against(ClusterShPayloadsValidationInputs {
                directory: &self.cluster_directory,
                base: self.base.codec_metadata(),
                sources: &sources,
            })
    }
}

/// Build a manifest using only the PRL table, id-49/id-50 metadata, and the
/// codec-defined metadata ranges of streamed source sections. The caller
/// continues loading non-streamed sections through the same retained file.
pub(crate) fn load_manifest_positionally(
    file: Arc<File>,
    diagnostic_path: PathBuf,
    container: ContainerMeta,
    cluster_directory: ClusterDirectorySection,
    cluster_directory_bytes: &[u8],
) -> Result<ShStreamManifest, PrlLoadError> {
    validate_streamed_entry_cardinality(&container)?;
    let payload_entry = required_entry(&container, SectionId::ClusterShPayloads)?;
    validate_positional_entry_bounds(&file, &container, payload_entry)?;
    if payload_entry.version
        != postretro_level_format::cluster_sh_payloads::CLUSTER_SH_PAYLOADS_CONTAINER_VERSION
    {
        return Err(ClusterShPayloadsError::VersionMismatch {
            version: u32::from(payload_entry.version),
            expected: u32::from(
                postretro_level_format::cluster_sh_payloads::CLUSTER_SH_PAYLOADS_CONTAINER_VERSION,
            ),
        }
        .into());
    }
    let header_len =
        u64::try_from(postretro_level_format::cluster_sh_payloads::CLUSTER_SH_PAYLOADS_HEADER_SIZE)
            .expect("id-50 header length fits u64");
    ensure_section_floor(payload_entry, header_len, "id-50 header")?;
    let header_bytes = read_vec_at(&file, payload_entry.offset, header_len, "id-50 header")?;
    let header = ClusterShPayloadsSection::parse_header(&header_bytes)?;
    const MAX_STREAMED_SOURCES: u32 = 5;
    if header.source_count > MAX_STREAMED_SOURCES {
        return Err(stream_error(format!(
            "id-50 source count {} exceeds the {MAX_STREAMED_SOURCES}-source streaming inventory",
            header.source_count
        )));
    }
    let directory_cluster_count = u32::try_from(cluster_directory.clusters.len())
        .map_err(|_| stream_error("id-49 cluster count exceeds u32"))?;
    if header.cluster_count != directory_cluster_count {
        return Err(stream_error(format!(
            "id-50 cluster count {} disagrees with id-49 cluster count {directory_cluster_count}",
            header.cluster_count
        )));
    }
    let metadata_len = header.metadata_len()?;
    let metadata_len = u64::try_from(metadata_len)
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("id-50 metadata length"))?;
    ensure_section_floor(payload_entry, metadata_len, "id-50 metadata")?;
    let metadata_bytes = read_vec_at(&file, payload_entry.offset, metadata_len, "id-50 metadata")?;
    let payloads =
        ClusterShPayloadsSection::from_metadata_bytes(&metadata_bytes, payload_entry.size)?;

    let base_entry = container
        .find_section(SectionId::OctahedralShVolume as u32)
        .ok_or_else(|| {
            ClusterShPayloadsError::SourceMismatch(
                "id 50 is present but required id 34 is absent".into(),
            )
        })?;
    validate_positional_entry_bounds(&file, &container, base_entry)?;
    let base = read_base_metadata(&file, base_entry)?;
    let indirect_delta_entry = optional_entry(&container, SectionId::DeltaShVolumes)?;
    let direct_entry = optional_entry(&container, SectionId::DirectShVolume)?;
    let direct_delta_entry = optional_entry(&container, SectionId::DirectShDeltaVolumes)?;
    let animated_direct_delta_entry =
        optional_entry(&container, SectionId::AnimatedDirectShDeltaVolumes)?;
    for entry in [
        indirect_delta_entry,
        direct_entry,
        direct_delta_entry,
        animated_direct_delta_entry,
    ]
    .into_iter()
    .flatten()
    {
        validate_positional_entry_bounds(&file, &container, entry)?;
    }
    let sources = ShStreamSourceMetadata {
        indirect_delta: read_optional_sparse_metadata(
            &file,
            indirect_delta_entry,
            SectionId::DeltaShVolumes,
            &base,
        )?,
        direct: read_optional_direct_metadata(&file, direct_entry)?,
        direct_delta: read_optional_sparse_metadata(
            &file,
            direct_delta_entry,
            SectionId::DirectShDeltaVolumes,
            &base,
        )?,
        animated_direct_delta: read_optional_sparse_metadata(
            &file,
            animated_direct_delta_entry,
            SectionId::AnimatedDirectShDeltaVolumes,
            &base,
        )?,
    };
    validate_projected_sources(&base, &sources)?;
    let content_tag = streaming_content_tag(&container, cluster_directory_bytes, &metadata_bytes);
    let manifest = ShStreamManifest {
        file,
        diagnostic_path,
        container,
        cluster_directory,
        payloads,
        base,
        sources,
        content_tag,
        cluster_adjacency: std::sync::OnceLock::new(),
    };
    manifest.validate_payloads()?;
    Ok(manifest)
}

/// Check an entire positional read against its table entry *before* allocating
/// a buffer or calling the OS. This is deliberately phrased as a floor so it
/// protects both fixed headers and variable metadata prefixes.
pub(super) fn ensure_section_floor(
    entry: &SectionEntry,
    required_len: u64,
    what: &'static str,
) -> Result<(), PrlLoadError> {
    if required_len > entry.size {
        return Err(stream_error(format!(
            "{what} requires {required_len} bytes but its section contains only {}",
            entry.size
        )));
    }
    Ok(())
}

fn required_entry(
    container: &ContainerMeta,
    section: SectionId,
) -> Result<&SectionEntry, PrlLoadError> {
    container
        .find_section(section as u32)
        .ok_or(PrlLoadError::StaleFormatMissingSection {
            section: match section {
                SectionId::OctahedralShVolume => "OctahedralShVolume",
                SectionId::ClusterShPayloads => "ClusterShPayloads",
                _ => "streamed SH source",
            },
            id: section as u32,
        })
}

fn optional_entry(
    container: &ContainerMeta,
    section: SectionId,
) -> Result<Option<&SectionEntry>, PrlLoadError> {
    let entries: Vec<_> = container
        .sections
        .iter()
        .filter(|entry| entry.section_id == section as u32)
        .collect();
    match entries.as_slice() {
        [] => Ok(None),
        [entry] => Ok(Some(*entry)),
        _ => Err(ClusterShPayloadsError::SourceMismatch(format!(
            "PRL table has duplicate streamed section {}",
            section as u32
        ))
        .into()),
    }
}

fn validate_streamed_entry_cardinality(container: &ContainerMeta) -> Result<(), PrlLoadError> {
    for section in [
        SectionId::ClusterDirectory,
        SectionId::ClusterShPayloads,
        SectionId::DeltaShVolumes,
        SectionId::OctahedralShVolume,
        SectionId::DirectShVolume,
        SectionId::DirectShDeltaVolumes,
        SectionId::AnimatedDirectShDeltaVolumes,
    ] {
        let count = container
            .sections
            .iter()
            .filter(|entry| entry.section_id == section as u32)
            .count();
        if count > 1 {
            return Err(ClusterShPayloadsError::SourceMismatch(format!(
                "PRL table has {count} entries for streamed section {}",
                section as u32
            ))
            .into());
        }
    }
    Ok(())
}

pub(super) fn streaming_content_tag(
    container: &ContainerMeta,
    cluster_directory_bytes: &[u8],
    payload_metadata_bytes: &[u8],
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"postretro.sh-stream.v1\0");
    hasher.update(&postretro_level_format::MAGIC);
    hasher.update(&container.header.version.to_le_bytes());
    hasher.update(&container.header.section_count.to_le_bytes());
    for entry in &container.sections {
        hasher.update(&entry.section_id.to_le_bytes());
        hasher.update(&entry.offset.to_le_bytes());
        hasher.update(&entry.size.to_le_bytes());
        hasher.update(&entry.version.to_le_bytes());
    }
    hasher.update(cluster_directory_bytes);
    hasher.update(payload_metadata_bytes);
    *hasher.finalize().as_bytes()
}
