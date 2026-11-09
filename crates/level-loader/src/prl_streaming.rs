//! PRL streaming-mode dispatch, id-49/id-50 validation, and legacy fallback.
//! See: context/lib/build_pipeline.md §PRL Compilation.

use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES;
use postretro_level_format::cluster_directory::{
    CLUSTER_DIRECTORY_CONTAINER_VERSION, ClusterDirectoryError, ClusterDirectorySection,
};
use postretro_level_format::{self as prl_format, SectionId};

use std::sync::Arc;

use crate::lightmap_stream::{LightmapStreamingMode, requested_lightmap_streaming_mode};
use crate::load_progress::LoadProgress;
use crate::prl::{LevelWorld, PrlLoadError};
use crate::prl_container::PrlContainer;
use crate::prl_file::PrlFile;
use crate::prl_loader::{
    HandOut, LoadCancel, LoadManifests, MAX_DELTA_SECTION_BINDING_BYTES, StagedParseOutcome,
    load_prl_from_container, load_prl_staged_from_container,
};
use crate::sh_stream::{
    ShStreamManifest, ShStreamingMode, load_manifest_positionally, read_container_positionally,
    read_section_positionally, read_vec_at_in_chunks, requested_streaming_mode,
};

/// The whole-file backing reads its image in pieces this size so progress
/// moves during the read rather than after it.
const WHOLE_IMAGE_READ_CHUNK_BYTES: u64 = 8 * 1024 * 1024;

/// Where one load's modes come from. SH's mode is consulted only for a PRL
/// with id 50, which otherwise ignores `POSTRETRO_SH_STREAMING` entirely.
#[derive(Clone, Copy)]
pub(crate) enum ModeSource {
    /// The environment gates, as production reads them.
    Environment,
    /// Test-only injection that keeps the production ordering without
    /// process-global environment mutation, so tests stay parallel-safe.
    #[cfg(test)]
    Injected {
        sh: ShStreamingMode,
        lightmap: LightmapStreamingMode,
    },
}

/// The container one load decodes, with the modes it runs under.
struct OpenedLoad {
    container: PrlContainer,
    manifests: LoadManifests,
    lightmap: LightmapStreamingMode,
}

pub fn load_prl(path: &str) -> Result<LevelWorld, PrlLoadError> {
    load_prl_with_progress(path, &Arc::new(LoadProgress::new()))
}

/// [`load_prl`], reporting to `progress` as the load consumes the file.
///
/// Units are bytes this load reads: the container header and table, the SH
/// streaming manifest's reads, every section it decodes, plus, on the
/// whole-file backing, the image read itself. Sections the load leaves on
/// disk (streamed SH bodies and payloads, streamed lightmap blocks) are not
/// counted. The plan is fixed right after the table read, before any section
/// is read, so progress moves through the manifest read; a successful return
/// leaves `done == total`.
pub fn load_prl_with_progress(
    path: &str,
    progress: &Arc<LoadProgress>,
) -> Result<LevelWorld, PrlLoadError> {
    let result = load_prl_with_modes(
        path,
        ModeSource::Environment,
        MAX_DELTA_SECTION_BINDING_BYTES,
        MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
        progress,
    );
    #[cfg(test)]
    crate::prl_loader::staged_tests::cross_check_staged(&result, |sink| {
        load_prl_staged_with_modes(
            path,
            ModeSource::Environment,
            MAX_DELTA_SECTION_BINDING_BYTES,
            MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
            &Arc::new(LoadProgress::new()),
            &std::sync::atomic::AtomicBool::new(false),
            sink,
        )
    });
    result
}

/// The staged parse of `path`: decodes in the same order as [`load_prl`] and
/// passes `sink` five ordered hand-outs (see [`HandOut`]) as each group's data
/// becomes final. The return value is the late verdict: `Ok(Complete)` after
/// the last hand-out, or the error [`load_prl`] returns for the same file,
/// possibly after hand-outs 1–4 went out. [`crate::assemble`] builds the
/// `LevelWorld` from a complete set.
///
/// `progress` is the parse's counter, credited exactly as
/// [`load_prl_with_progress`] credits it and finished on `Complete`. `cancel`
/// is polled between stages; once seen, no further hand-out goes out and the
/// parse returns `Ok(Cancelled)`.
pub fn load_prl_staged(
    path: &str,
    progress: &Arc<LoadProgress>,
    cancel: &(impl LoadCancel + ?Sized),
    mut sink: impl FnMut(HandOut),
) -> Result<StagedParseOutcome, PrlLoadError> {
    load_prl_staged_with_modes(
        path,
        ModeSource::Environment,
        MAX_DELTA_SECTION_BINDING_BYTES,
        MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
        progress,
        cancel,
        &mut sink,
    )
}

/// The monolithic load under `modes`: open, decode, finish the counter.
pub(crate) fn load_prl_with_modes(
    path: &str,
    modes: ModeSource,
    max_delta_section_binding_bytes: u64,
    max_scatter_section_bytes: u64,
    progress: &Arc<LoadProgress>,
) -> Result<LevelWorld, PrlLoadError> {
    let opened = open_load(path, modes, progress)?;
    let world = load_prl_from_container(
        opened.container,
        max_delta_section_binding_bytes,
        max_scatter_section_bytes,
        opened.manifests,
        opened.lightmap,
    )?;
    progress.finish();
    Ok(world)
}

/// The staged load under `modes`. Opening the file (table, manifest, backing)
/// is one stage: the cancel flag is polled before and after it, and between
/// the decode stages after that.
pub(crate) fn load_prl_staged_with_modes(
    path: &str,
    modes: ModeSource,
    max_delta_section_binding_bytes: u64,
    max_scatter_section_bytes: u64,
    progress: &Arc<LoadProgress>,
    cancel: &(impl LoadCancel + ?Sized),
    sink: &mut dyn FnMut(HandOut),
) -> Result<StagedParseOutcome, PrlLoadError> {
    if cancel.is_cancelled() {
        return Ok(StagedParseOutcome::Cancelled);
    }
    let opened = open_load(path, modes, progress)?;
    let outcome = load_prl_staged_from_container(
        opened.container,
        max_delta_section_binding_bytes,
        max_scatter_section_bytes,
        opened.manifests,
        opened.lightmap,
        cancel,
        sink,
    )?;
    if outcome == StagedParseOutcome::Complete {
        progress.finish();
    }
    Ok(outcome)
}

/// Read the table, plan and credit the parse's progress, validate the
/// id-49/id-50 pair, resolve the modes, and pick the container backing.
///
/// The plan is fixed right after the table read so the table and the SH
/// manifest's reads are credited parse work: on a streamed-manifest map the
/// bar moves before the first section read. The mode gates are evaluated
/// before the manifest read to size the plan, but their errors still surface
/// after the manifest's, in the order the gates have always run.
///
/// The whole-file image is the legacy path: it holds every section, ids
/// 22/42 included. Lightmap streaming therefore forces the positional
/// container whenever the table could stream (id 22, id 51 and portals
/// present), even with SH `off` or no id 50; whether it actually streams is
/// decided inside the load once portals and id 51 have been validated.
fn open_load(
    path: &str,
    modes: ModeSource,
    progress: &Arc<LoadProgress>,
) -> Result<OpenedLoad, PrlLoadError> {
    let (file, metadata) = read_container_positionally(path)?;
    let has_id50 = metadata
        .sections
        .iter()
        .any(|entry| entry.section_id == SectionId::ClusterShPayloads as u32);
    let (sh_mode, lightmap_mode) = match modes {
        ModeSource::Environment => (
            has_id50.then(requested_streaming_mode),
            lightmap_mode_for_table(&metadata, requested_lightmap_streaming_mode),
        ),
        #[cfg(test)]
        ModeSource::Injected { sh, lightmap } => (has_id50.then_some(Ok(sh)), Ok(lightmap)),
    };

    let sh_streams = matches!(
        sh_mode,
        Some(Ok(ShStreamingMode::SyncProof | ShStreamingMode::Async))
    );
    let lightmap_may_stream = matches!(lightmap_mode, Ok(LightmapStreamingMode::Stream))
        && table_may_stream_lightmaps(&metadata);
    let whole_file = !sh_streams && !lightmap_may_stream;
    let planned = planned_section_reads(&metadata, sh_streams);
    let image_len = if whole_file { file.len()? } else { 0 };
    let table_bytes = file.read_counters().total_bytes();
    let manifest_allotment = if has_id50 {
        manifest_read_bound(&metadata)
    } else {
        0
    };
    progress.begin(
        table_bytes
            + manifest_allotment
            + image_len
            + planned.iter().map(|(_, size)| size).sum::<u64>(),
    );
    progress.advance(table_bytes);

    let manifest = load_stream_manifest_if_present(path, &file, &metadata);
    let manifest_bytes = file
        .read_counters()
        .total_bytes()
        .saturating_sub(table_bytes)
        .min(manifest_allotment);
    progress.advance(manifest_bytes);
    progress.forgo(manifest_allotment - manifest_bytes);
    let manifest = manifest?;
    let sh_mode = sh_mode.transpose()?;
    let lightmap = lightmap_mode?;

    let sh_manifest = match sh_mode {
        Some(ShStreamingMode::SyncProof | ShStreamingMode::Async) => manifest.clone(),
        Some(ShStreamingMode::Off) | None => None,
    };
    debug_assert_eq!(
        whole_file,
        sh_manifest.is_none()
            && !(lightmap == LightmapStreamingMode::Stream
                && table_may_stream_lightmaps(&metadata)),
        "the plan was sized for the backing the modes choose"
    );
    let container = if whole_file {
        // The image read is real work here, unlike the positional backing, so
        // it shares the plan with the section decodes that follow it.
        let file_data = read_vec_at_in_chunks(
            &file,
            0,
            image_len,
            WHOLE_IMAGE_READ_CHUNK_BYTES,
            "legacy PRL image",
            |piece| progress.advance(piece),
        )?;
        let reads = Some(file.read_counters().clone());
        PrlContainer::from_whole_bytes(file_data, metadata, reads)
    } else {
        PrlContainer::from_positional(file, metadata, sh_streams)
    };
    Ok(OpenedLoad {
        container: container.with_progress(progress.clone(), planned),
        manifests: LoadManifests {
            streaming: sh_manifest,
            id50: manifest,
        },
        lightmap,
    })
}

/// An upper bound on the bytes the SH manifest reads: id 49 whole, plus
/// prefixes of id 50 (header and metadata) and of the SH sections whose
/// metadata it projects. What it does not read leaves the plan once it
/// returns.
fn manifest_read_bound(metadata: &prl_format::ContainerMeta) -> u64 {
    [
        SectionId::ClusterDirectory,
        SectionId::ClusterShPayloads,
        SectionId::OctahedralShVolume,
        SectionId::DeltaShVolumes,
        SectionId::DirectShVolume,
        SectionId::DirectShDeltaVolumes,
        SectionId::AnimatedDirectShDeltaVolumes,
        SectionId::MoverProbeChannel,
    ]
    .into_iter()
    .filter_map(|section| metadata.find_section(section as u32))
    .map(|entry| entry.size)
    .sum()
}

/// `(id, size)` of every section the decode may read whole: each registered
/// id once, as the table lookup finds it. Left out are ids no decode reads
/// (retired BSP and SH volume ids, unknown ids), the id-50 payloads only the
/// streaming manifest touches, and, when SH streams, the SH bodies it leaves
/// on disk plus the id-49 directory the manifest already parsed. Sections the
/// decode turns out to skip leave the plan when the load settles or finishes.
pub(crate) fn planned_section_reads(
    metadata: &prl_format::ContainerMeta,
    sh_bodies_streamed: bool,
) -> Vec<(u32, u64)> {
    let mut planned: Vec<(u32, u64)> = Vec::new();
    for entry in &metadata.sections {
        let Some(id) = SectionId::from_u32(entry.section_id) else {
            continue;
        };
        let never_read_whole = matches!(
            id,
            SectionId::BspNodes
                | SectionId::BspLeaves
                | SectionId::ShVolume
                | SectionId::ClusterShPayloads
        );
        let streamed = sh_bodies_streamed
            && matches!(
                id,
                SectionId::DeltaShVolumes
                    | SectionId::OctahedralShVolume
                    | SectionId::DirectShVolume
                    | SectionId::DirectShDeltaVolumes
                    | SectionId::AnimatedDirectShDeltaVolumes
                    | SectionId::ClusterDirectory
            );
        let already_planned = planned
            .iter()
            .any(|(planned_id, _)| *planned_id == entry.section_id);
        if never_read_whole || streamed || already_planned {
            continue;
        }
        planned.push((entry.section_id, entry.size));
    }
    planned
}

/// A table that can never stream lightmaps bypasses the lightmap environment
/// gate, as a PRL without id 50 bypasses SH's, so a bad value cannot fail it.
/// The unset default stands in, and the load's residency rule logs why the
/// level stays all-resident.
pub(crate) fn lightmap_mode_for_table(
    metadata: &prl_format::ContainerMeta,
    requested: impl FnOnce() -> Result<LightmapStreamingMode, PrlLoadError>,
) -> Result<LightmapStreamingMode, PrlLoadError> {
    if table_may_stream_lightmaps(metadata) {
        requested()
    } else {
        Ok(LightmapStreamingMode::Stream)
    }
}

fn table_may_stream_lightmaps(metadata: &prl_format::ContainerMeta) -> bool {
    [
        SectionId::Lightmap,
        SectionId::CellResidencySet,
        SectionId::Portals,
    ]
    .into_iter()
    .all(|section| metadata.find_section(section as u32).is_some())
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

/// Test-only SH mode injection that preserves the production ordering: id-50
/// is opened and validated before the selected mode can choose streaming or
/// the retained-handle legacy fallback. Lightmaps load all-resident. Avoid
/// process-global environment mutation so these tests remain parallel-safe.
#[cfg(test)]
pub(crate) fn load_prl_with_streaming_mode_for_test(
    path: &str,
    mode: ShStreamingMode,
) -> Result<LevelWorld, PrlLoadError> {
    load_prl_with_modes_for_test(path, mode, LightmapStreamingMode::AllResident)
}

/// Test-only injection of both modes, through the production dispatch.
#[cfg(test)]
pub(crate) fn load_prl_with_modes_for_test(
    path: &str,
    sh: ShStreamingMode,
    lightmap: LightmapStreamingMode,
) -> Result<LevelWorld, PrlLoadError> {
    load_prl_with_modes_reporting_for_test(path, sh, lightmap, &Arc::new(LoadProgress::new()))
}

/// [`load_prl_with_modes_for_test`], reporting to `progress`. Every load
/// through it is cross-checked against the staged parse.
#[cfg(test)]
pub(crate) fn load_prl_with_modes_reporting_for_test(
    path: &str,
    sh: ShStreamingMode,
    lightmap: LightmapStreamingMode,
    progress: &Arc<LoadProgress>,
) -> Result<LevelWorld, PrlLoadError> {
    let modes = ModeSource::Injected { sh, lightmap };
    let result = load_prl_with_modes(
        path,
        modes,
        MAX_DELTA_SECTION_BINDING_BYTES,
        MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
        progress,
    );
    crate::prl_loader::staged_tests::cross_check_staged(&result, |sink| {
        load_prl_staged_with_modes(
            path,
            modes,
            MAX_DELTA_SECTION_BINDING_BYTES,
            MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
            &Arc::new(LoadProgress::new()),
            &std::sync::atomic::AtomicBool::new(false),
            sink,
        )
    });
    result
}

#[cfg(test)]
fn load_prl_with_section_limits(
    path: &str,
    max_delta_section_binding_bytes: u64,
    max_scatter_section_bytes: u64,
) -> Result<LevelWorld, PrlLoadError> {
    let result = PrlContainer::open(path).and_then(|container| {
        load_prl_from_container(
            container,
            max_delta_section_binding_bytes,
            max_scatter_section_bytes,
            LoadManifests::default(),
            LightmapStreamingMode::AllResident,
        )
    });
    crate::prl_loader::staged_tests::cross_check_staged(&result, |sink| {
        load_prl_staged_from_container(
            PrlContainer::open(path)?,
            max_delta_section_binding_bytes,
            max_scatter_section_bytes,
            LoadManifests::default(),
            LightmapStreamingMode::AllResident,
            &std::sync::atomic::AtomicBool::new(false),
            sink,
        )
    });
    result
}

/// Validate the id-49/id-50 pair before consulting the mode gate. The
/// manifest owns the exact file opened for table parsing; the streaming
/// branch must not reopen `path` or call the legacy whole-file constructor.
fn load_stream_manifest_if_present(
    path: &str,
    file: &Arc<PrlFile>,
    container: &prl_format::ContainerMeta,
) -> Result<Option<Arc<ShStreamManifest>>, PrlLoadError> {
    let id50_entries: Vec<_> = container
        .sections
        .iter()
        .filter(|entry| entry.section_id == SectionId::ClusterShPayloads as u32)
        .collect();
    if id50_entries.is_empty() {
        return Ok(None);
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
    let directory_bytes = read_section_positionally(file, container, SectionId::ClusterDirectory)?
        .ok_or_else(|| {
            postretro_level_format::cluster_sh_payloads::ClusterShPayloadsError::SourceMismatch(
                "id 49 table entry could not be read".into(),
            )
        })?;
    let directory = ClusterDirectorySection::from_bytes(&directory_bytes)?;
    let manifest = Arc::new(load_manifest_positionally(
        file.clone(),
        std::path::PathBuf::from(path),
        container.clone(),
        directory,
        &directory_bytes,
    )?);
    Ok(Some(manifest))
}
