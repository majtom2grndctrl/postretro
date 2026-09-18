// PRL output staging, publication, and read-back validation.
// Kept separate from section planning so callers can inspect descriptors before encoders are consumed.
use std::ffi::OsString;

use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs4::FileExt;

use postretro_level_format::{
    SectionDescriptor, SectionId, read_container, read_section_data, write_prl_header_and_table,
};
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

pub(super) type SectionEncoder<'a> = Box<dyn FnOnce() -> anyhow::Result<Vec<u8>> + 'a>;

pub(super) struct PlannedSection<'a> {
    pub(super) descriptor: SectionDescriptor,
    encode: SectionEncoder<'a>,
}

impl<'a> PlannedSection<'a> {
    pub(super) fn new(
        section_id: u32,
        version: u16,
        byte_len: usize,
        encode: impl FnOnce() -> anyhow::Result<Vec<u8>> + 'a,
    ) -> Self {
        Self {
            descriptor: SectionDescriptor {
                section_id,
                version,
                byte_len: byte_len as u64,
            },
            encode: Box::new(encode),
        }
    }
}

pub(super) fn write_and_validate_sections(
    output: &Path,
    sections: Vec<PlannedSection<'_>>,
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
            let bytes = (section.encode)()?;
            if bytes.len() as u64 != section.descriptor.byte_len {
                match SectionId::from_u32(section.descriptor.section_id) {
                    Some(section_id) => anyhow::bail!(
                        "section {section_id:?} (id {}) wrote {} bytes but its table declares {} bytes",
                        section.descriptor.section_id,
                        bytes.len(),
                        section.descriptor.byte_len,
                    ),
                    None => anyhow::bail!(
                        "unknown section {} wrote {} bytes but its table declares {} bytes",
                        section.descriptor.section_id,
                        bytes.len(),
                        section.descriptor.byte_len,
                    ),
                }
            }
            temporary_output.file_mut().write_all(&bytes)?;
        }
        temporary_output.file_mut().flush()?;
        let total_size = temporary_output.file().metadata()?.len();
        validate_readback(temporary_output.file_mut(), &descriptors)?;
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

/// Validate the flushed PRL through the handle that received the bytes.
fn validate_readback(
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

    fn remove_publication_test_artifacts(output: &Path) {
        for path in staging_artifacts(output) {
            std::fs::remove_file(path).expect("publish artifact should be removable");
        }
        let lock_path = output_lock_path(output).expect("test output should have a lock path");
        if lock_path.exists() {
            std::fs::remove_file(lock_path).expect("publication lock should be removable");
        }
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
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
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
    fn streamed_write_rejects_declared_payload_length_mismatch() {
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-mismatch-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
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
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-replace-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
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
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-preserve-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
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
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-directory-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
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
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-cleanup-identity-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
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
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-staged-identity-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
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

        validate_readback(staged.file_mut(), std::slice::from_ref(&descriptor))
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

        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-symlink-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
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
        let output = std::env::temp_dir().join(format!(
            "postretro-streamed-pack-output-race-{}-{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
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
}

// Regression: publication overwrote an output installed after its precondition check.
#[test]
fn publication_rejects_regular_output_replaced_after_precondition() {
    let output = std::env::temp_dir().join(format!(
        "postretro-streamed-pack-regular-race-{}-{}.prl",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
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
