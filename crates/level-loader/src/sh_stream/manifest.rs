//! Validated SH streaming manifest assembly and named cluster-payload decoding.
//! See: context/lib/build_pipeline.md §PRL Compilation.

use std::fs::File;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use postretro_level_format::SectionEntry;
use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_format::cluster_sh_payloads::{
    ClusterShPayloadsError, ClusterShPayloadsSection, ClusterShPayloadsValidationInputs,
    DecodedClusterShPayload, ValidatedClusterShPayloadsSection,
};
use postretro_level_format::{ContainerMeta, SectionId};

use super::boundary::ShDrainBatch;
use super::metadata_base::read_base_metadata;
use super::metadata_sparse::{read_optional_direct_metadata, read_optional_sparse_metadata};
use super::positional_io::{read_vec_at, validate_positional_entry_bounds};
use super::projection::{ShStreamBaseMetadata, ShStreamSourceMetadata, validate_projected_sources};
use super::{PrlLoadError, stream_error};

/// One validated id-49 seam portal projected to the clusters at its two
/// endpoints. The controller needs this alongside ordinary cluster adjacency:
/// a closed door may remove the far cell from render visibility while an
/// authored seam still warms its lighting cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShStreamSeamPortal {
    pub portal_id: u32,
    pub front_cluster_id: u32,
    pub back_cluster_id: u32,
}

#[derive(Debug)]
struct ClusterPortalTopology {
    adjacency: Vec<Vec<u32>>,
    seam_portals: Vec<ShStreamSeamPortal>,
}
/// Immutable, validated handle for a streaming PRL session. The file is opened
/// exactly once at load; worker jobs use positional reads against this handle
/// and never reopen the diagnostic path.
#[derive(Debug)]
pub struct ShStreamManifest {
    file: Arc<File>,
    diagnostic_path: PathBuf,
    container: ContainerMeta,
    cluster_directory: ClusterDirectorySection,
    payloads: ValidatedClusterShPayloadsSection,
    base: ShStreamBaseMetadata,
    sources: ShStreamSourceMetadata,
    content_tag: [u8; 32],
    cluster_portal_topology: std::sync::OnceLock<ClusterPortalTopology>,
}

impl ShStreamManifest {
    pub fn diagnostic_path(&self) -> &Path {
        &self.diagnostic_path
    }

    pub fn cluster_count(&self) -> u32 {
        self.payloads.section().header.cluster_count
    }

    pub fn cluster_directory(&self) -> &ClusterDirectorySection {
        &self.cluster_directory
    }

    pub fn payloads(&self) -> &ClusterShPayloadsSection {
        self.payloads.section()
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
        &self
            .cluster_portal_topology
            .get()
            .expect("successful streaming manifest installs validated cluster adjacency")
            .adjacency
    }

    /// Marked portal IDs with their validated endpoint cluster IDs. This is
    /// intentionally distinct from adjacency: it preserves authored seams
    /// without changing the visibility graph used by gameplay and rendering.
    pub fn seam_portals(&self) -> &[ShStreamSeamPortal] {
        &self
            .cluster_portal_topology
            .get()
            .expect("successful streaming manifest installs validated seam portals")
            .seam_portals
    }

    pub(crate) fn install_cluster_portal_topology(
        &self,
        adjacency: Vec<Vec<u32>>,
        seam_portals: Vec<ShStreamSeamPortal>,
    ) -> Result<(), PrlLoadError> {
        self.cluster_portal_topology
            .set(ClusterPortalTopology {
                adjacency,
                seam_portals,
            })
            .map_err(|_| stream_error("cluster portal topology was installed more than once"))
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

    /// Absolute PRL file range of one cluster's encoded chunk, from the id-50
    /// index. A canonical empty cluster yields an empty range and needs no read.
    pub fn chunk_file_range(&self, cluster_id: u32) -> Result<Range<u64>, PrlLoadError> {
        let local = self.payloads.section().payload_range(cluster_id)?;
        let region = self.payload_file_region()?;
        let offset = |local: u64| {
            region
                .start
                .checked_add(local)
                .ok_or(ClusterShPayloadsError::SizeOverflow(
                    "id-50 chunk file offset",
                ))
        };
        Ok(offset(local.start)?..offset(local.end)?)
    }

    /// One positional read of an absolute file span that may cover several
    /// chunks and the gaps between them. Any span reaching outside the id-50
    /// payload region is rejected before allocation.
    pub fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        let region = self.payload_file_region()?;
        if range.start > range.end || range.start < region.start || range.end > region.end {
            return Err(stream_error(format!(
                "file span {}..{} is outside the id-50 payload region {}..{}",
                range.start, range.end, region.start, region.end
            )));
        }
        read_vec_at(
            &self.file,
            range.start,
            range.end - range.start,
            "id-50 span",
        )
    }

    /// Read one encoded chunk through an injectable positional reader. Tests
    /// can delay this seam without introducing a cursor or reopening the file;
    /// production uses `read_vec_at` and platform `FileExt`.
    pub fn read_encoded_cluster_with(
        &self,
        cluster_id: u32,
        reader: impl FnOnce(&File, u64, u64) -> Result<Vec<u8>, PrlLoadError>,
    ) -> Result<Vec<u8>, PrlLoadError> {
        let range = self.chunk_file_range(cluster_id)?;
        reader(&self.file, range.start, range.end - range.start)
    }

    /// Absolute file range of the id-50 chunk bodies: the section minus its
    /// metadata prefix.
    fn payload_file_region(&self) -> Result<Range<u64>, PrlLoadError> {
        let payload_entry = self
            .container
            .find_section(SectionId::ClusterShPayloads as u32)
            .ok_or_else(|| stream_error("id 50 disappeared from validated table"))?;
        let metadata_len = u64::try_from(self.payloads.section().header.metadata_len()?)
            .map_err(|_| ClusterShPayloadsError::SizeOverflow("id-50 metadata length"))?;
        let start = payload_entry
            .offset
            .checked_add(metadata_len)
            .ok_or(ClusterShPayloadsError::SizeOverflow("id-50 payload offset"))?;
        let end = payload_entry
            .offset
            .checked_add(payload_entry.size)
            .ok_or(ClusterShPayloadsError::SizeOverflow("id-50 section end"))?;
        Ok(start..end)
    }

    /// Decode and verify the chunk against the validated manifest, including
    /// its per-chunk BLAKE3. Ownership of the encoded vector moves into the
    /// codec rather than being cloned for the renderer handoff.
    pub fn decode_encoded_cluster(
        &self,
        cluster_id: u32,
        bytes: Vec<u8>,
    ) -> Result<DecodedClusterShPayload, PrlLoadError> {
        Ok(self.payloads.decode_chunk(cluster_id, bytes)?)
    }

    /// Validate the renderer handoff before it crosses the loader boundary.
    /// This is deliberately CPU-only: target policy stays in the planner and
    /// GPU installation stays in the renderer.
    pub fn validate_drain_batch(&self, batch: &ShDrainBatch) -> Result<(), PrlLoadError> {
        batch.validate_contract(self.cluster_count(), self.content_tag)
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
    let codec_sources = sources.codec_sources(&base);
    let payloads = payloads.into_validated(ClusterShPayloadsValidationInputs {
        directory: &cluster_directory,
        base: base.codec_metadata(),
        sources: &codec_sources,
    })?;
    let manifest = ShStreamManifest {
        file,
        diagnostic_path,
        container,
        cluster_directory,
        payloads,
        base,
        sources,
        content_tag,
        cluster_portal_topology: std::sync::OnceLock::new(),
    };
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
