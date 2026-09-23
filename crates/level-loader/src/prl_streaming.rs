//! PRL streaming-mode dispatch, id-49/id-50 validation, and legacy fallback.
//! See: context/lib/build_pipeline.md §PRL Compilation.

use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES;
use postretro_level_format::cluster_directory::{
    CLUSTER_DIRECTORY_CONTAINER_VERSION, ClusterDirectoryError, ClusterDirectorySection,
};
use postretro_level_format::{self as prl_format, SectionId};

use crate::prl::{LevelWorld, PrlLoadError};
use crate::prl_container::PrlContainer;
use crate::prl_loader::{MAX_DELTA_SECTION_BINDING_BYTES, load_prl_from_container};
use crate::sh_stream::{
    ShStreamManifest, ShStreamingMode, load_manifest_positionally, read_container_positionally,
    read_section_positionally, read_vec_at, requested_streaming_mode,
};

type StreamingManifestLoad = (
    std::sync::Arc<std::fs::File>,
    prl_format::ContainerMeta,
    Option<std::sync::Arc<ShStreamManifest>>,
);

pub fn load_prl(path: &str) -> Result<LevelWorld, PrlLoadError> {
    let limits = (
        MAX_DELTA_SECTION_BINDING_BYTES,
        MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
    );
    let (file, metadata, manifest) = load_stream_manifest_if_present(path)?;
    match manifest {
        // A pre-streaming PRL bypasses the environment gate entirely and
        // preserves the legacy decode path on this same opened file.
        None => load_prl_from_retained_file(file, metadata, limits.0, limits.1),
        Some(manifest) => match requested_streaming_mode()? {
            ShStreamingMode::Off => load_prl_from_retained_file(file, metadata, limits.0, limits.1),
            ShStreamingMode::SyncProof | ShStreamingMode::Async => load_prl_from_container(
                PrlContainer::from_positional(file, metadata),
                limits.0,
                limits.1,
                Some(manifest),
            ),
        },
    }
}

/// Internal load entry point with an injectable per-section binding floor.
/// Production always supplies the desktop floor above; tests use a tiny value
/// to exercise the complete resolution path without a 128 MiB fixture.
#[cfg(test)]
pub(crate) fn load_prl_with_delta_binding_limit(
    path: &str,
    max_delta_section_binding_bytes: u64,
) -> Result<LevelWorld, PrlLoadError> {
    load_prl_with_section_limits(
        path,
        max_delta_section_binding_bytes,
        MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
    )
}

#[cfg(test)]
pub(crate) fn load_prl_with_scatter_pack_limit(
    path: &str,
    max_scatter_section_bytes: u64,
) -> Result<LevelWorld, PrlLoadError> {
    load_prl_with_section_limits(
        path,
        MAX_DELTA_SECTION_BINDING_BYTES,
        max_scatter_section_bytes,
    )
}

/// Test-only mode injection that preserves the production ordering: id-50 is
/// opened and validated before the selected mode can choose streaming or the
/// retained-handle legacy fallback. Avoid process-global environment mutation
/// so these tests remain parallel-safe.
#[cfg(test)]
pub(crate) fn load_prl_with_streaming_mode_for_test(
    path: &str,
    mode: ShStreamingMode,
) -> Result<LevelWorld, PrlLoadError> {
    let (file, metadata, manifest) = load_stream_manifest_if_present(path)?;
    match manifest {
        None => load_prl_from_retained_file(
            file,
            metadata,
            MAX_DELTA_SECTION_BINDING_BYTES,
            MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
        ),
        Some(_) if matches!(mode, ShStreamingMode::Off) => load_prl_from_retained_file(
            file,
            metadata,
            MAX_DELTA_SECTION_BINDING_BYTES,
            MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
        ),
        Some(manifest) => load_prl_from_container(
            PrlContainer::from_positional(file, metadata),
            MAX_DELTA_SECTION_BINDING_BYTES,
            MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
            Some(manifest),
        ),
    }
}

#[cfg(test)]
fn load_prl_with_section_limits(
    path: &str,
    max_delta_section_binding_bytes: u64,
    max_scatter_section_bytes: u64,
) -> Result<LevelWorld, PrlLoadError> {
    let container = PrlContainer::open(path)?;
    load_prl_from_container(
        container,
        max_delta_section_binding_bytes,
        max_scatter_section_bytes,
        None,
    )
}

fn load_prl_from_retained_file(
    file: std::sync::Arc<std::fs::File>,
    metadata: prl_format::ContainerMeta,
    max_delta_section_binding_bytes: u64,
    max_scatter_section_bytes: u64,
) -> Result<LevelWorld, PrlLoadError> {
    let file_len = file.metadata()?.len();
    let file_data = read_vec_at(&file, 0, file_len, "legacy PRL image")?;
    load_prl_from_container(
        PrlContainer::from_whole_bytes(file_data, metadata),
        max_delta_section_binding_bytes,
        max_scatter_section_bytes,
        None,
    )
}

/// Validate the id-49/id-50 pair before consulting the mode gate. The returned
/// handle owns the exact file opened for table parsing; the streaming branch
/// must not reopen `path` or call the legacy whole-file constructor.
fn load_stream_manifest_if_present(path: &str) -> Result<StreamingManifestLoad, PrlLoadError> {
    let (file, container) = read_container_positionally(path)?;
    let id50_entries: Vec<_> = container
        .sections
        .iter()
        .filter(|entry| entry.section_id == SectionId::ClusterShPayloads as u32)
        .collect();
    if id50_entries.is_empty() {
        return Ok((file, container, None));
    }
    if id50_entries.len() != 1 {
        return Err(
            postretro_level_format::cluster_sh_payloads::ClusterShPayloadsError::SourceMismatch(
                format!("PRL table has {} id-50 entries", id50_entries.len()),
            )
            .into(),
        );
    }
    let id49_entries: Vec<_> = container
        .sections
        .iter()
        .filter(|entry| entry.section_id == SectionId::ClusterDirectory as u32)
        .collect();
    let [directory_entry] = id49_entries.as_slice() else {
        return Err(
            postretro_level_format::cluster_sh_payloads::ClusterShPayloadsError::SourceMismatch(
                "id 50 requires exactly one id 49 ClusterDirectory entry".into(),
            )
            .into(),
        );
    };
    if directory_entry.version != CLUSTER_DIRECTORY_CONTAINER_VERSION {
        return Err(ClusterDirectoryError::VersionMismatch {
            version: u32::from(directory_entry.version),
            expected: u32::from(CLUSTER_DIRECTORY_CONTAINER_VERSION),
        }
        .into());
    }
    let directory_bytes =
        read_section_positionally(&file, &container, SectionId::ClusterDirectory)?.ok_or_else(
            || {
                postretro_level_format::cluster_sh_payloads::ClusterShPayloadsError::SourceMismatch(
                    "id 49 table entry could not be read".into(),
                )
            },
        )?;
    let directory = ClusterDirectorySection::from_bytes(&directory_bytes)?;
    let manifest = std::sync::Arc::new(load_manifest_positionally(
        file.clone(),
        std::path::PathBuf::from(path),
        container.clone(),
        directory,
        &directory_bytes,
    )?);
    Ok((file, container, Some(manifest)))
}
