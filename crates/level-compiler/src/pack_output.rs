// PRL output staging, publication, and read-back validation.
// Kept separate from section planning so callers can inspect descriptors before encoders are consumed.
use std::ffi::OsString;

use std::fs::{self, File, OpenOptions};
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs4::FileExt;

use postretro_level_format::cluster_directory::ClusterDirectoryValidationInputs;
use postretro_level_format::{SectionDescriptor, SectionId, write_prl_header_and_table};
use same_file::Handle as FileIdentity;
use tempfile::{Builder as TempFileBuilder, NamedTempFile};

// Windows denies an atomic replacement while an indexer, sync client, or reader
// temporarily holds the destination without delete sharing. Keep the retry brief:
// a persistent lock or ACL problem still fails and discards the staged PRL.
const WINDOWS_PUBLISH_MAX_RETRIES: u32 = 5;
const WINDOWS_PUBLISH_RETRY_BASE_DELAY: Duration = Duration::from_millis(50);
const WINDOWS_ERROR_ACCESS_DENIED: i32 = 5;
const WINDOWS_ERROR_SHARING_VIOLATION: i32 = 32;
const WINDOWS_ERROR_LOCK_VIOLATION: i32 = 33;

#[path = "pack_output/readback.rs"]
mod readback;
use readback::validate_readback;

pub(super) type SectionWriter<'a> = Box<dyn FnOnce(&mut dyn Write) -> anyhow::Result<()> + 'a>;

pub(super) struct PlannedSection<'a> {
    pub(super) descriptor: SectionDescriptor,
    write: SectionWriter<'a>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SectionFootprint {
    pub(super) section_id: u32,
    pub(super) section_name: Option<SectionId>,
    pub(super) payload_bytes: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct PrlFootprint {
    pub(super) sections: Vec<SectionFootprint>,
    pub(super) payload_bytes: u64,
}

/// Summarize the planned payloads without invoking their one-shot encoders.
pub(super) fn report_section_footprint(descriptors: &[SectionDescriptor]) -> PrlFootprint {
    let sections: Vec<_> = descriptors
        .iter()
        .map(|descriptor| SectionFootprint {
            section_id: descriptor.section_id,
            section_name: SectionId::from_u32(descriptor.section_id),
            payload_bytes: descriptor.byte_len,
        })
        .collect();
    let payload_bytes = sections.iter().map(|section| section.payload_bytes).sum();

    PrlFootprint {
        sections,
        payload_bytes,
    }
}

impl<'a> PlannedSection<'a> {
    /// Wrap an existing one-shot byte encoder for legacy section bodies.
    ///
    /// New payloads that already live in a spool may use [`Self::with_writer`]
    /// to write directly to the staged PRL without first materializing a second
    /// full `Vec<u8>`.
    pub(super) fn new(
        section_id: u32,
        version: u16,
        byte_len: usize,
        encode: impl FnOnce() -> anyhow::Result<Vec<u8>> + 'a,
    ) -> Self {
        let byte_len = u64::try_from(byte_len)
            .expect("usize section byte lengths must fit the PRL u64 descriptor field");
        Self::with_writer(section_id, version, byte_len, move |writer| {
            let bytes = encode()?;
            writer.write_all(&bytes)?;
            Ok(())
        })
    }

    /// Plan one exact-length payload that writes itself once to the staged PRL.
    pub(super) fn with_writer(
        section_id: u32,
        version: u16,
        byte_len: u64,
        write: impl FnOnce(&mut dyn Write) -> anyhow::Result<()> + 'a,
    ) -> Self {
        Self {
            descriptor: SectionDescriptor {
                section_id,
                version,
                byte_len,
            },
            write: Box::new(write),
        }
    }
}

#[cfg(test)]
pub(super) fn write_and_validate_sections(
    output: &Path,
    sections: Vec<PlannedSection<'_>>,
) -> anyhow::Result<()> {
    write_and_validate_sections_with_cluster_directory_validation(output, sections, None)
}

/// Write a staged PRL and validate its cluster directory against the finalized
/// compiler metadata that produced it.
pub(super) fn write_and_validate_sections_with_cluster_directory_validation(
    output: &Path,
    sections: Vec<PlannedSection<'_>>,
    cluster_directory_validation: Option<ClusterDirectoryValidationInputs<'_>>,
) -> anyhow::Result<()> {
    // Validate output directory exists before writing
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            anyhow::bail!("output directory does not exist: {}", parent.display());
        }
    }
    let original_output = OutputIdentity::capture(output)?;

    let descriptors: Vec<_> = sections
        .iter()
        .map(|section| section.descriptor.clone())
        .collect();
    let file_name = output
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("output path has no file name: {}", output.display()))?;
    let mut temporary_output = StagedPrl::create(output, file_name)?;
    let write_result = (|| -> anyhow::Result<u64> {
        write_prl_header_and_table(temporary_output.file_mut(), &descriptors)?;
        for section in sections {
            let start = temporary_output.file_mut().stream_position()?;
            (section.write)(temporary_output.file_mut())?;
            let end = temporary_output.file_mut().stream_position()?;
            let actual_len = end.checked_sub(start).ok_or_else(|| {
                anyhow::anyhow!(
                    "section {} writer moved the staged PRL cursor backward",
                    section.descriptor.section_id,
                )
            })?;
            if actual_len != section.descriptor.byte_len {
                match SectionId::from_u32(section.descriptor.section_id) {
                    Some(section_id) => anyhow::bail!(
                        "section {section_id:?} (id {}) wrote {} bytes but its table declares {} bytes",
                        section.descriptor.section_id,
                        actual_len,
                        section.descriptor.byte_len,
                    ),
                    None => anyhow::bail!(
                        "unknown section {} wrote {} bytes but its table declares {} bytes",
                        section.descriptor.section_id,
                        actual_len,
                        section.descriptor.byte_len,
                    ),
                }
            }
        }
        temporary_output.file_mut().flush()?;
        let total_size = temporary_output.file().metadata()?.len();
        validate_readback(
            temporary_output.file_mut(),
            &descriptors,
            cluster_directory_validation,
        )?;
        Ok(total_size)
    })();
    let total_size = match write_result {
        Ok(total_size) => total_size,
        Err(error) => {
            return Err(failed_staging_error(
                temporary_output,
                format!("{error} after write failure"),
            ));
        }
    };
    publish_validated_output(temporary_output, output, original_output)?;
    log::info!("Wrote {} ({} bytes)", output.display(), total_size);
    log::info!("Read-back validation passed.");

    Ok(())
}

struct StagedPrl {
    temporary: NamedTempFile,
    identity: FileIdentity,
}

impl StagedPrl {
    fn create(output: &Path, file_name: &std::ffi::OsStr) -> anyhow::Result<Self> {
        let parent = output
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut prefix = OsString::from(".");
        prefix.push(file_name);
        prefix.push(".pack-");
        let mut temporary = TempFileBuilder::new()
            .prefix(&prefix)
            .suffix(".tmp")
            .tempfile_in(parent)
            .map_err(|error| {
                anyhow::anyhow!(
                    "failed to create temporary PRL beside {}: {error}",
                    output.display(),
                )
            })?;

        // Disable pathname-based Drop cleanup. Failed staging paths remain because
        // a checked name can be replaced before an unlink reaches the filesystem.
        temporary.disable_cleanup(true);
        let identity = FileIdentity::from_file(temporary.reopen()?).map_err(|error| {
            anyhow::anyhow!(
                "failed to identify temporary PRL {}: {error}",
                temporary.path().display(),
            )
        })?;

        Ok(Self {
            temporary,
            identity,
        })
    }

    fn path(&self) -> &Path {
        self.temporary.path()
    }

    fn file(&self) -> &File {
        self.temporary.as_file()
    }

    fn file_mut(&mut self) -> &mut File {
        self.temporary.as_file_mut()
    }

    fn ensure_path_is_owned(&self) -> anyhow::Result<()> {
        ensure_path_has_identity(self.path(), &self.identity, "temporary PRL")
    }

    fn discard(self) -> anyhow::Result<()> {
        // Cleanup is best effort. Re-check before unlink so a known replacement
        // remains intact; a failed check leaves the path for manual inspection.
        let path = self.path().to_path_buf();
        self.ensure_path_is_owned()?;
        self.temporary.close().map_err(|error| {
            anyhow::anyhow!("failed to remove temporary PRL {}: {error}", path.display(),)
        })
    }
}

fn failed_staging_error(staged: StagedPrl, failure: impl std::fmt::Display) -> anyhow::Error {
    let temporary_path = staged.path().to_path_buf();
    match staged.discard() {
        Ok(()) => anyhow::anyhow!("{failure}; temporary PRL discarded"),
        Err(cleanup_error) => anyhow::anyhow!(
            "{failure}; temporary PRL remains at {} because cleanup failed: {cleanup_error}",
            temporary_path.display(),
        ),
    }
}

fn retry_windows_publication<T, U>(
    mut staged: T,
    is_windows: bool,
    mut persist: impl FnMut(T) -> Result<U, (T, std::io::Error)>,
    mut sleep: impl FnMut(Duration),
) -> Result<U, (T, std::io::Error, u32)> {
    let mut retries = 0;
    loop {
        match persist(staged) {
            Ok(persisted) => return Ok(persisted),
            Err((returned_staged, error))
                if is_retryable_windows_publish_error(&error, is_windows)
                    && retries < WINDOWS_PUBLISH_MAX_RETRIES =>
            {
                let delay = WINDOWS_PUBLISH_RETRY_BASE_DELAY * (1_u32 << retries);
                retries += 1;
                sleep(delay);
                staged = returned_staged;
            }
            Err((returned_staged, error)) => return Err((returned_staged, error, retries)),
        }
    }
}

fn is_retryable_windows_publish_error(error: &std::io::Error, is_windows: bool) -> bool {
    is_windows
        && matches!(
            error.raw_os_error(),
            Some(
                WINDOWS_ERROR_ACCESS_DENIED
                    | WINDOWS_ERROR_SHARING_VIOLATION
                    | WINDOWS_ERROR_LOCK_VIOLATION
            )
        )
}

struct OutputPublishLock {
    _file: File,
}

impl OutputPublishLock {
    fn acquire(output: &Path) -> anyhow::Result<Self> {
        let lock_path = output_lock_path(output)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| {
                anyhow::anyhow!(
                    "failed to open PRL publication lock {}: {error}",
                    lock_path.display(),
                )
            })?;
        anyhow::ensure!(
            file.metadata()?.file_type().is_file(),
            "PRL publication lock {} is not a regular file",
            lock_path.display(),
        );
        let identity = FileIdentity::from_file(file.try_clone()?)?;
        ensure_path_has_identity(&lock_path, &identity, "PRL publication lock")?;
        FileExt::lock(&file).map_err(|error| {
            anyhow::anyhow!(
                "failed to lock PRL publication lock {}: {error}",
                lock_path.display(),
            )
        })?;
        ensure_path_has_identity(&lock_path, &identity, "PRL publication lock")?;
        // Keep the lock pathname between runs. Removing it would let a waiter
        // hold the old inode while a new compiler locks a newly created inode.
        Ok(Self { _file: file })
    }
}

fn output_lock_path(output: &Path) -> anyhow::Result<PathBuf> {
    let file_name = output
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("output path has no file name: {}", output.display()))?;
    let mut lock_name = OsString::from(".");
    lock_name.push(file_name);
    lock_name.push(".pack.lock");
    Ok(output.with_file_name(lock_name))
}

enum OutputIdentity {
    Absent,
    Regular(FileIdentity),
}

impl OutputIdentity {
    fn capture(output: &Path) -> anyhow::Result<Self> {
        match fs::symlink_metadata(output) {
            Ok(metadata) if metadata.file_type().is_file() => {
                let identity = open_regular_path_identity(output, "existing output")?;
                Ok(Self::Regular(identity))
            }
            Ok(_) => anyhow::bail!(
                "refusing to replace non-regular output {}",
                output.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::Absent),
            Err(error) => Err(anyhow::anyhow!(
                "failed to inspect existing output {}: {error}",
                output.display(),
            )),
        }
    }

    fn ensure_unchanged(&self, output: &Path) -> anyhow::Result<()> {
        match self {
            Self::Absent => match fs::symlink_metadata(output) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Ok(_) => anyhow::bail!(
                    "refusing to publish because output {} appeared during compilation",
                    output.display(),
                ),
                Err(error) => Err(anyhow::anyhow!(
                    "failed to re-inspect output {} before publication: {error}",
                    output.display(),
                )),
            },
            Self::Regular(identity) => {
                ensure_path_has_identity(output, identity, "existing output").map_err(|error| {
                    anyhow::anyhow!(
                        "refusing to publish because output {} changed during compilation: {error}",
                        output.display(),
                    )
                })
            }
        }
    }
}

fn open_regular_path_identity(path: &Path, description: &str) -> anyhow::Result<FileIdentity> {
    let before = fs::symlink_metadata(path).map_err(|error| {
        anyhow::anyhow!(
            "failed to inspect {description} {}: {error}",
            path.display()
        )
    })?;
    anyhow::ensure!(
        before.file_type().is_file(),
        "{description} {} is not a regular file",
        path.display(),
    );

    let file = File::open(path).map_err(|error| {
        anyhow::anyhow!("failed to open {description} {}: {error}", path.display())
    })?;
    anyhow::ensure!(
        file.metadata()?.file_type().is_file(),
        "{description} {} did not open as a regular file",
        path.display(),
    );
    let identity = FileIdentity::from_file(file)?;

    let after = fs::symlink_metadata(path).map_err(|error| {
        anyhow::anyhow!(
            "failed to re-inspect {description} {}: {error}",
            path.display()
        )
    })?;
    anyhow::ensure!(
        after.file_type().is_file(),
        "{description} {} changed while it was inspected",
        path.display(),
    );
    let confirmed = FileIdentity::from_path(path).map_err(|error| {
        anyhow::anyhow!(
            "failed to confirm {description} {} identity: {error}",
            path.display(),
        )
    })?;
    anyhow::ensure!(
        confirmed == identity,
        "{description} {} changed while its identity was captured",
        path.display(),
    );
    Ok(identity)
}

fn ensure_path_has_identity(
    path: &Path,
    expected: &FileIdentity,
    description: &str,
) -> anyhow::Result<()> {
    let actual = open_regular_path_identity(path, description)?;
    anyhow::ensure!(
        &actual == expected,
        "{description} {} no longer names the file owned by this invocation",
        path.display(),
    );
    Ok(())
}

/// Publish a validated staged file while excluding cooperating prl-build writers.
fn publish_validated_output(
    temporary_output: StagedPrl,
    output: &Path,
    original_output: OutputIdentity,
) -> anyhow::Result<()> {
    publish_validated_output_with_hook(temporary_output, output, original_output, || Ok(()))
}

fn publish_validated_output_with_hook(
    temporary_output: StagedPrl,
    output: &Path,
    original_output: OutputIdentity,
    after_precondition: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let _publication_lock = match OutputPublishLock::acquire(output) {
        Ok(lock) => lock,
        Err(error) => {
            return Err(failed_staging_error(
                temporary_output,
                format!("{error} after lock failure"),
            ));
        }
    };
    let temporary_path = temporary_output.path().to_path_buf();
    let precondition = temporary_output
        .ensure_path_is_owned()
        .and_then(|()| original_output.ensure_unchanged(output));
    if let Err(error) = precondition {
        return Err(failed_staging_error(
            temporary_output,
            format!("{error} after publication refusal"),
        ));
    }

    if let Err(error) = after_precondition() {
        return Err(failed_staging_error(
            temporary_output,
            format!("{error} after publication was interrupted"),
        ));
    }

    // The lock closes this window for cooperating prl-build processes. This
    // late check also catches noncooperating changes observed before rename;
    // an external writer can still race the final check and path-based rename.
    let precondition = temporary_output
        .ensure_path_is_owned()
        .and_then(|()| original_output.ensure_unchanged(output));
    if let Err(error) = precondition {
        return Err(failed_staging_error(
            temporary_output,
            format!("{error} after a late publication race"),
        ));
    }

    // `same_file::Handle` retains the existing output's OS handle. All identity
    // checks are complete, so release it before Windows replaces the destination.
    drop(original_output);

    let StagedPrl {
        temporary,
        identity,
    } = temporary_output;
    match retry_windows_publication(
        StagedPrl {
            temporary,
            identity,
        },
        cfg!(windows),
        |StagedPrl {
             temporary,
             identity,
         }| match temporary.persist(output) {
            Ok(persisted_file) => Ok((persisted_file, identity)),
            Err(tempfile::PersistError {
                error,
                file: temporary,
            }) => Err((
                StagedPrl {
                    temporary,
                    identity,
                },
                error,
            )),
        },
        std::thread::sleep,
    ) {
        Ok((persisted_file, identity)) => {
            let result = ensure_path_has_identity(output, &identity, "published output");
            drop(persisted_file);
            result
        }
        Err((staged, publish_error, retries)) => {
            let retry_context = if retries == 0 {
                String::new()
            } else {
                format!(
                    " after {retries} Windows replacement retries; close processes reading {} or check its attributes and permissions",
                    output.display(),
                )
            };
            Err(failed_staging_error(
                staged,
                format!(
                    "failed to publish temporary PRL {} to {}: {publish_error}{retry_context}",
                    temporary_path.display(),
                    output.display(),
                ),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staging_artifacts(output: &Path) -> Vec<PathBuf> {
        let parent = output.parent().expect("test output has a parent");
        let prefix = format!(
            ".{}.pack-",
            output
                .file_name()
                .expect("test output has a file name")
                .to_string_lossy()
        );
        std::fs::read_dir(parent)
            .expect("test output parent should be readable")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(&prefix))
            })
            .collect()
    }

    /// A publication-output path under the system temp directory, unique per
    /// process and per test thread.
    ///
    /// The thread name is sanitized rather than interpolated raw. `cargo test`
    /// names each thread after the test path it runs (`pack::pack_output::tests::…`),
    /// and `::` is not legal in a Windows filename — so building a path from the
    /// raw name failed there with os error 123, for every test that used one.
    /// Every character that is not portable in a path component is replaced, not
    /// `::` specifically, so a test added later cannot reintroduce the bug with a
    /// differently illegal name.
    ///
    /// The name is kept rather than swapped for a counter because these land in
    /// a directory shared with the rest of the system: it is what identifies
    /// which test left one behind. Pass an empty `extension` for a path the test
    /// creates as a directory.
    fn temp_output_path(stem: &str, extension: &str) -> PathBuf {
        let thread = std::thread::current();
        let sanitized: String = thread
            .name()
            .unwrap_or("test")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();

        let mut name = format!("postretro-{stem}-{}-{sanitized}", std::process::id());
        if !extension.is_empty() {
            name.push('.');
            name.push_str(extension);
        }
        std::env::temp_dir().join(name)
    }

    fn remove_publication_test_artifacts(output: &Path) {
        for path in staging_artifacts(output) {
            std::fs::remove_file(path).expect("publish artifact should be removable");
        }
        let lock_path = output_lock_path(output).expect("test output should have a lock path");
        if lock_path.exists() {
            std::fs::remove_file(lock_path).expect("publication lock should be removable");
        }
    }

    fn assert_failed_one_shot_writer_preserves_output_and_cleans_staging(
        case: &str,
        declared_byte_len: u64,
        write: impl FnOnce(&mut dyn Write) -> anyhow::Result<()>,
        expected_error: &str,
    ) {
        let output = temp_output_path(&format!("one-shot-section-{case}"), "prl");
        let previous_bytes = b"previous valid PRL";
        std::fs::write(&output, previous_bytes).expect("should create previous output");

        let error = write_and_validate_sections(
            &output,
            vec![PlannedSection::with_writer(
                SectionId::Geometry as u32,
                1,
                declared_byte_len,
                write,
            )],
        )
        .expect_err("a failing one-shot writer must not publish its staging file");

        assert!(
            error.to_string().contains(expected_error),
            "unexpected one-shot writer error: {error:#}"
        );
        assert_eq!(
            std::fs::read(&output).expect("previous output must remain readable"),
            previous_bytes,
            "a failed one-shot writer must not replace the previous output"
        );
        assert!(
            staging_artifacts(&output).is_empty(),
            "failed one-shot staging should be cleaned up"
        );
        std::fs::remove_file(&output).expect("output should be removable");
        remove_publication_test_artifacts(&output);
    }

    #[test]
    fn section_footprint_names_known_ids_and_retains_unknown_ids() {
        let footprint = report_section_footprint(&[
            SectionDescriptor {
                section_id: SectionId::Geometry as u32,
                version: 1,
                byte_len: 3,
            },
            SectionDescriptor {
                section_id: 9_001,
                version: 1,
                byte_len: 5,
            },
        ]);

        assert_eq!(footprint.payload_bytes, 8);
        assert_eq!(
            footprint.sections,
            vec![
                SectionFootprint {
                    section_id: SectionId::Geometry as u32,
                    section_name: Some(SectionId::Geometry),
                    payload_bytes: 3,
                },
                SectionFootprint {
                    section_id: 9_001,
                    section_name: None,
                    payload_bytes: 5,
                },
            ]
        );
    }

    #[test]
    fn section_footprint_includes_optional_sections_only_when_emitted() {
        let required = SectionDescriptor {
            section_id: SectionId::Geometry as u32,
            version: 1,
            byte_len: 3,
        };
        let optional = SectionDescriptor {
            section_id: SectionId::Lightmap as u32,
            version: 1,
            byte_len: 5,
        };

        let without_optional = report_section_footprint(std::slice::from_ref(&required));
        let with_optional = report_section_footprint(&[required, optional]);

        assert_eq!(without_optional.payload_bytes, 3);
        assert_eq!(
            without_optional
                .sections
                .iter()
                .map(|section| section.section_id)
                .collect::<Vec<_>>(),
            vec![SectionId::Geometry as u32]
        );
        assert_eq!(with_optional.payload_bytes, 8);
        assert_eq!(
            with_optional
                .sections
                .iter()
                .map(|section| section.section_id)
                .collect::<Vec<_>>(),
            vec![SectionId::Geometry as u32, SectionId::Lightmap as u32]
        );
    }

    #[test]
    fn windows_publication_retry_retries_transient_replacement_errors() {
        let mut attempts = 0;
        let mut delays = Vec::new();
        let result: std::result::Result<(), ((), std::io::Error, u32)> = retry_windows_publication(
            (),
            true,
            |_| {
                attempts += 1;
                if attempts <= 2 {
                    Err((
                        (),
                        std::io::Error::from_raw_os_error(WINDOWS_ERROR_ACCESS_DENIED),
                    ))
                } else {
                    Ok(())
                }
            },
            |delay| delays.push(delay),
        );

        result.expect("transient Windows replacement failures should retry");
        assert_eq!(attempts, 3);
        assert_eq!(
            delays,
            vec![
                WINDOWS_PUBLISH_RETRY_BASE_DELAY,
                WINDOWS_PUBLISH_RETRY_BASE_DELAY * 2,
            ]
        );
    }

    #[test]
    fn non_windows_publication_retry_returns_first_replace_failure() {
        let mut attempts = 0;
        let result: std::result::Result<(), ((), std::io::Error, u32)> = retry_windows_publication(
            (),
            false,
            |_| {
                attempts += 1;
                Err((
                    (),
                    std::io::Error::from_raw_os_error(WINDOWS_ERROR_ACCESS_DENIED),
                ))
            },
            |_| panic!("non-Windows publication must not sleep and retry"),
        );

        let (_, error, retries) = result.expect_err("non-Windows failure should return directly");
        assert_eq!(attempts, 1);
        assert_eq!(error.raw_os_error(), Some(WINDOWS_ERROR_ACCESS_DENIED));
        assert_eq!(retries, 0);
    }

    #[test]
    fn streamed_write_matches_legacy_container_bytes_and_file_readback() {
        let output = temp_output_path("streamed-pack", "prl");
        let legacy_sections = vec![
            postretro_level_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: vec![0x01, 0x02, 0x03],
            },
            postretro_level_format::SectionBlob {
                section_id: SectionId::Lightmap as u32,
                version: 1,
                data: vec![0xAA, 0xBB],
            },
        ];
        let mut expected = Vec::new();
        postretro_level_format::write_prl(&mut expected, &legacy_sections)
            .expect("legacy PRL should serialize");

        write_and_validate_sections(
            &output,
            vec![
                PlannedSection::new(SectionId::Geometry as u32, 1, 3, || {
                    Ok(vec![0x01, 0x02, 0x03])
                }),
                PlannedSection::new(SectionId::Lightmap as u32, 1, 2, || Ok(vec![0xAA, 0xBB])),
            ],
        )
        .expect("streamed PRL should write and read back");

        assert_eq!(
            std::fs::read(&output).expect("streamed output should exist"),
            expected
        );
        std::fs::remove_file(&output).expect("output should be removable");
        remove_publication_test_artifacts(&output);
    }

    #[test]
    fn one_shot_section_writer_streams_chunks_without_a_payload_vec() {
        let output = temp_output_path("one-shot-section-writer", "prl");

        write_and_validate_sections(
            &output,
            vec![PlannedSection::with_writer(
                SectionId::Geometry as u32,
                1,
                5,
                |writer| {
                    writer.write_all(&[0x01, 0x02])?;
                    writer.write_all(&[0x03, 0x04, 0x05])?;
                    Ok(())
                },
            )],
        )
        .expect("a chunked one-shot writer should produce a valid PRL");

        let mut file = File::open(&output).expect("streamed output should exist");
        let container = postretro_level_format::read_container(&mut file)
            .expect("written PRL should have a readable container");
        assert_eq!(
            postretro_level_format::read_section_data(
                &mut file,
                &container,
                SectionId::Geometry as u32,
            )
            .expect("section read should succeed"),
            Some(vec![0x01, 0x02, 0x03, 0x04, 0x05])
        );

        std::fs::remove_file(&output).expect("output should be removable");
        remove_publication_test_artifacts(&output);
    }

    #[test]
    fn one_shot_section_writer_partial_error_preserves_output_and_cleans_staging() {
        assert_failed_one_shot_writer_preserves_output_and_cleans_staging(
            "partial-error",
            3,
            |writer| {
                writer.write_all(&[0x01])?;
                anyhow::bail!("test one-shot writer stopped after a partial payload")
            },
            "test one-shot writer stopped after a partial payload",
        );
    }

    #[test]
    fn one_shot_section_writer_short_write_preserves_output_and_cleans_staging() {
        assert_failed_one_shot_writer_preserves_output_and_cleans_staging(
            "short-write",
            3,
            |writer| {
                writer.write_all(&[0x01, 0x02])?;
                Ok(())
            },
            "section Geometry (id 17) wrote 2 bytes but its table declares 3",
        );
    }

    #[test]
    fn one_shot_section_writer_long_write_preserves_output_and_cleans_staging() {
        assert_failed_one_shot_writer_preserves_output_and_cleans_staging(
            "long-write",
            3,
            |writer| {
                writer.write_all(&[0x01, 0x02, 0x03, 0x04])?;
                Ok(())
            },
            "section Geometry (id 17) wrote 4 bytes but its table declares 3",
        );
    }

    #[test]
    fn section_footprint_matches_flushed_container_payload_portion() {
        let output = temp_output_path("section-footprint", "prl");
        let sections = vec![
            PlannedSection::new(SectionId::Geometry as u32, 1, 3, || Ok(vec![1, 2, 3])),
            PlannedSection::new(9_001, 1, 5, || Ok(vec![4, 5, 6, 7, 8])),
        ];
        let descriptors: Vec<_> = sections
            .iter()
            .map(|section| section.descriptor.clone())
            .collect();
        let footprint = report_section_footprint(&descriptors);

        write_and_validate_sections(&output, sections)
            .expect("streamed PRL should write and read back");

        let file_bytes = std::fs::metadata(&output)
            .expect("streamed output should exist")
            .len();
        let header_and_table_bytes = 8 + footprint.sections.len() as u64 * 22;
        assert_eq!(
            file_bytes - header_and_table_bytes,
            footprint.payload_bytes,
            "the report excludes only the PRL header and section table"
        );

        std::fs::remove_file(&output).expect("output should be removable");
        remove_publication_test_artifacts(&output);
    }

    #[test]
    fn streamed_write_rejects_declared_payload_length_mismatch() {
        let output = temp_output_path("streamed-pack-mismatch", "prl");
        let error = write_and_validate_sections(
            &output,
            vec![PlannedSection::new(
                SectionId::Geometry as u32,
                1,
                3,
                || Ok(vec![0x01, 0x02]),
            )],
        )
        .expect_err("writer must reject a declared length mismatch");

        assert!(
            error
                .to_string()
                .contains("section Geometry (id 17) wrote 2 bytes but its table declares 3")
        );
        assert!(
            !output.exists(),
            "a declared-length mismatch must not leave a malformed final PRL"
        );
        remove_publication_test_artifacts(&output);
    }

    #[test]
    fn streamed_write_replaces_existing_output_without_staging_artifacts() {
        let output = temp_output_path("streamed-pack-replace", "prl");
        std::fs::write(&output, b"previous valid PRL").expect("should create previous output");

        write_and_validate_sections(
            &output,
            vec![PlannedSection::new(
                SectionId::Geometry as u32,
                1,
                3,
                || Ok(vec![0x01, 0x02, 0x03]),
            )],
        )
        .expect("streamed PRL should replace the previous output");

        assert_ne!(
            std::fs::read(&output).expect("replacement output must exist"),
            b"previous valid PRL"
        );
        assert!(
            staging_artifacts(&output).is_empty(),
            "a successful publish must not leave temporary or backup files"
        );
        assert!(
            output_lock_path(&output)
                .expect("test output should have a lock path")
                .is_file(),
            "the stable lock inode must remain for later compiler invocations"
        );
        std::fs::remove_file(&output).expect("output should be removable");
        remove_publication_test_artifacts(&output);
    }

    #[test]
    fn streamed_write_failure_preserves_existing_output_and_cleans_staging() {
        let output = temp_output_path("streamed-pack-preserve", "prl");
        let previous_bytes = b"previous valid PRL";
        std::fs::write(&output, previous_bytes).expect("should create previous output");

        let error = write_and_validate_sections(
            &output,
            vec![PlannedSection::new(
                SectionId::Geometry as u32,
                1,
                3,
                || Ok(vec![0x01, 0x02]),
            )],
        )
        .expect_err("writer must reject a declared length mismatch");

        assert!(
            error
                .to_string()
                .contains("wrote 2 bytes but its table declares 3")
        );
        assert_eq!(
            std::fs::read(&output).expect("previous output must remain readable"),
            previous_bytes,
            "a write failure must not replace the previous output"
        );
        assert!(
            staging_artifacts(&output).is_empty(),
            "failed staging should be cleaned up"
        );
        std::fs::remove_file(&output).expect("output should be removable");
        remove_publication_test_artifacts(&output);
    }

    // Regression: failed compilation replaced a directory at the requested output path.
    #[test]
    fn streamed_write_rejects_directory_output_before_staging() {
        let output = temp_output_path("streamed-pack-directory", "");
        std::fs::create_dir(&output).expect("should create directory output fixture");

        let error = write_and_validate_sections(
            &output,
            vec![PlannedSection::new(
                SectionId::Geometry as u32,
                1,
                1,
                || panic!("non-regular output must be rejected before encoding"),
            )],
        )
        .expect_err("a directory cannot be replaced with a PRL");

        assert!(error.to_string().contains("non-regular output"));
        assert!(output.is_dir(), "the existing directory must remain intact");
        assert!(
            staging_artifacts(&output).is_empty(),
            "rejection before staging must not create a temporary file"
        );
        std::fs::remove_dir(output).expect("directory fixture should be removable");
    }

    // Regression: cleanup must not unlink a replacement installed before its identity check.
    #[test]
    fn failed_staging_cleanup_preserves_replacement_swapped_before_identity_check() {
        let output = temp_output_path("streamed-pack-cleanup-identity", "prl");
        let file_name = output.file_name().expect("test output has a file name");
        let staged = StagedPrl::create(&output, file_name).expect("staging should succeed");
        let staged_path = staged.path().to_path_buf();
        let displaced = output.with_extension("owned-staging");
        staged
            .ensure_path_is_owned()
            .expect("the pre-cleanup identity check should pass");
        std::fs::rename(&staged_path, &displaced).expect("should displace owned staging file");
        std::fs::write(&staged_path, b"later replacement")
            .expect("should install replacement at staging path");

        let error = staged
            .discard()
            .expect_err("cleanup must reject a replacement staging path");

        assert!(error.to_string().contains("no longer names the file owned"));
        assert_eq!(
            std::fs::read(&staged_path).expect("replacement must remain readable"),
            b"later replacement"
        );
        std::fs::remove_file(staged_path).expect("replacement should be removable");
        std::fs::remove_file(displaced).expect("owned staging file should be removable");
    }

    // Regression: read-back reopened a replaced staging pathname and could publish its valid bytes.
    #[test]
    fn staged_readback_and_publish_remain_bound_to_original_file() {
        let output = temp_output_path("streamed-pack-staged-identity", "prl");
        let file_name = output.file_name().expect("test output has a file name");
        let original_output = OutputIdentity::capture(&output).expect("output should be absent");
        let mut staged = StagedPrl::create(&output, file_name).expect("staging should succeed");
        let descriptor = SectionDescriptor {
            section_id: SectionId::Geometry as u32,
            version: 1,
            byte_len: 3,
        };
        write_prl_header_and_table(staged.file_mut(), std::slice::from_ref(&descriptor))
            .expect("header should write");
        staged
            .file_mut()
            .write_all(&[1, 2, 3])
            .expect("payload should write");
        staged.file_mut().flush().expect("staging should flush");

        let staged_path = staged.path().to_path_buf();
        let displaced = output.with_extension("owned-staging");
        std::fs::rename(&staged_path, &displaced).expect("should displace owned staging file");
        let mut replacement = Vec::new();
        postretro_level_format::write_prl(
            &mut replacement,
            &[postretro_level_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: vec![9, 9, 9],
            }],
        )
        .expect("replacement PRL should encode");
        std::fs::write(&staged_path, replacement).expect("replacement PRL should write");

        validate_readback(staged.file_mut(), std::slice::from_ref(&descriptor), None)
            .expect("read-back should validate the original open file");
        let error = publish_validated_output(staged, &output, original_output)
            .expect_err("publication must reject the replacement staging identity");

        assert!(error.to_string().contains("no longer names the file owned"));
        assert!(!output.exists(), "replacement bytes must not be published");
        assert!(
            staged_path.exists(),
            "replacement staging file must not be deleted"
        );
        std::fs::remove_file(staged_path).expect("replacement staging should be removable");
        std::fs::remove_file(displaced).expect("owned staging file should be removable");
        remove_publication_test_artifacts(&output);
    }

    // Regression: symlink outputs previously changed target semantics during publication.
    #[cfg(unix)]
    #[test]
    fn streamed_write_rejects_symlink_output_before_staging() {
        use std::os::unix::fs::symlink;

        let output = temp_output_path("streamed-pack-symlink", "prl");
        let target = output.with_extension("target");
        std::fs::write(&target, b"symlink target").expect("should create symlink target");
        symlink(&target, &output).expect("should create output symlink");

        let error = write_and_validate_sections(
            &output,
            vec![PlannedSection::new(
                SectionId::Geometry as u32,
                1,
                1,
                || panic!("symlink output must be rejected before encoding"),
            )],
        )
        .expect_err("a symlink cannot be replaced with a PRL");

        assert!(error.to_string().contains("non-regular output"));
        assert!(
            std::fs::symlink_metadata(&output)
                .expect("output symlink should remain")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read(&target).expect("symlink target should remain readable"),
            b"symlink target"
        );
        assert!(staging_artifacts(&output).is_empty());
        std::fs::remove_file(output).expect("output symlink should be removable");
        std::fs::remove_file(target).expect("symlink target should be removable");
    }

    // Regression: Windows backup publication could strand the original after an output race.
    #[test]
    fn publication_rejects_nonregular_output_replacement_without_moving_original() {
        let output = temp_output_path("streamed-pack-output-race", "prl");
        let original = output.with_extension("original");
        let previous_bytes = b"previous valid PRL";
        std::fs::write(&output, previous_bytes).expect("should create previous output");
        let original_output = OutputIdentity::capture(&output).expect("output should be regular");
        let file_name = output.file_name().expect("test output has a file name");
        let mut staged = StagedPrl::create(&output, file_name).expect("staging should succeed");
        staged
            .file_mut()
            .write_all(b"validated replacement")
            .expect("staging should write");
        staged.file_mut().flush().expect("staging should flush");
        std::fs::rename(&output, &original).expect("should preserve original fixture");
        std::fs::create_dir(&output).expect("should install raced directory output");

        let error = publish_validated_output(staged, &output, original_output)
            .expect_err("publishing over a raced directory must fail");

        assert!(error.to_string().contains("changed during compilation"));
        assert!(output.is_dir(), "raced directory must remain intact");
        assert_eq!(
            std::fs::read(&original).expect("original output must remain readable"),
            previous_bytes
        );
        assert!(staging_artifacts(&output).is_empty());
        std::fs::remove_dir(&output).expect("raced directory should be removable");
        std::fs::remove_file(original).expect("original fixture should be removable");
        remove_publication_test_artifacts(&output);
    }

    // Regression: publication overwrote an output installed after its precondition check.
    #[test]
    fn publication_rejects_regular_output_replaced_after_precondition() {
        let output = temp_output_path("streamed-pack-regular-race", "prl");
        let original = output.with_extension("original");
        std::fs::write(&output, b"original output").expect("should create original output");
        let original_output = OutputIdentity::capture(&output).expect("output should be regular");
        let file_name = output.file_name().expect("test output has a file name");
        let mut staged = StagedPrl::create(&output, file_name).expect("staging should succeed");
        staged
            .file_mut()
            .write_all(b"validated replacement")
            .expect("staging should write");
        staged.file_mut().flush().expect("staging should flush");
        let error = publish_validated_output_with_hook(staged, &output, original_output, || {
            let lock_path = output_lock_path(&output)?;
            let competing_lock = OpenOptions::new().read(true).write(true).open(lock_path)?;
            assert!(matches!(
                FileExt::try_lock(&competing_lock),
                Err(fs4::TryLockError::WouldBlock)
            ));
            // Model an external writer that ignores the compiler's advisory lock.
            std::fs::rename(&output, &original).expect("should preserve original fixture");
            std::fs::write(&output, b"concurrent writer").expect("should install raced output");
            Ok(())
        })
        .expect_err("publishing over a raced regular file must fail");

        assert!(error.to_string().contains("changed during compilation"));
        assert_eq!(
            std::fs::read(&output).expect("concurrent output must remain readable"),
            b"concurrent writer"
        );
        assert_eq!(
            std::fs::read(&original).expect("original output must remain readable"),
            b"original output"
        );
        assert!(staging_artifacts(&output).is_empty());
        std::fs::remove_file(&output).expect("concurrent output should be removable");
        std::fs::remove_file(original).expect("original fixture should be removable");
        remove_publication_test_artifacts(&output);
    }
}
