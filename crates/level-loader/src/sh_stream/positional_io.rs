//! Bounds-checked positional reads over the single retained PRL file handle.
//! See: context/lib/build_pipeline.md §PRL Compilation.

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;
#[cfg(test)]
use std::{cell::RefCell, ops::Range};

use postretro_level_format::SectionEntry;
use postretro_level_format::{ContainerMeta, SectionId};

use super::{PrlLoadError, stream_error};
pub(crate) fn read_container_positionally(
    path: &str,
) -> Result<(Arc<File>, ContainerMeta), PrlLoadError> {
    let path_ref = Path::new(path);
    if !path_ref.exists() {
        return Err(PrlLoadError::FileNotFound(path.to_owned()));
    }
    let file = Arc::new(File::open(path_ref)?);
    let header = read_vec_at(&file, 0, 8, "PRL header")?;
    let found_magic = [header[0], header[1], header[2], header[3]];
    if found_magic != postretro_level_format::MAGIC {
        return Err(
            postretro_level_format::FormatError::InvalidMagic { found: found_magic }.into(),
        );
    }
    let section_count = u16::from_le_bytes([header[6], header[7]]);
    let table_len = 8u64
        .checked_add(
            u64::from(section_count)
                .checked_mul(22)
                .ok_or_else(|| stream_error("PRL section table length overflows"))?,
        )
        .ok_or_else(|| stream_error("PRL section table end overflows"))?;
    let table = read_vec_at(&file, 0, table_len, "PRL header and table")?;
    let mut cursor = io::Cursor::new(table);
    let container = postretro_level_format::read_container(&mut cursor)?;
    Ok((file, container))
}

pub(crate) fn read_section_positionally(
    file: &Arc<File>,
    container: &ContainerMeta,
    section: SectionId,
) -> Result<Option<Vec<u8>>, PrlLoadError> {
    let Some(entry) = container.find_section(section as u32) else {
        return Ok(None);
    };
    validate_positional_entry_bounds(file, container, entry)?;
    Ok(Some(read_vec_at(
        file,
        entry.offset,
        entry.size,
        "PRL section",
    )?))
}

pub(crate) fn read_vec_at(
    file: &File,
    offset: u64,
    len: u64,
    what: &'static str,
) -> Result<Vec<u8>, PrlLoadError> {
    #[cfg(test)]
    record_positional_read(offset, len);
    let len = usize::try_from(len).map_err(|_| stream_error("positional read exceeds usize"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len)
        .map_err(|_| stream_error("positional read allocation failed"))?;
    bytes.resize(len, 0);
    read_exact_at(file, offset, &mut bytes).map_err(PrlLoadError::IoError)?;
    let _ = what;
    Ok(bytes)
}

/// Per-thread report from [`observe_positional_reads`]. Test-only instrumentation
/// lets a fixture prove that the metadata-first loader never reads legacy SH
/// atlas/delta body ranges while it constructs a streaming manifest.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct PositionalReadReport {
    pub reads: Vec<Range<u64>>,
    pub forbidden_reads: Vec<Range<u64>>,
}

#[cfg(test)]
struct PositionalReadObservation {
    forbidden: Vec<Range<u64>>,
    report: PositionalReadReport,
}

#[cfg(test)]
thread_local! {
    static POSITIONAL_READ_OBSERVER: RefCell<Option<PositionalReadObservation>> = const { RefCell::new(None) };
}

/// Run one synchronous positional load while recording requested read ranges.
/// Observers are thread-local so parallel test workers do not contaminate one
/// another; callers must not nest this helper.
#[cfg(test)]
pub(crate) fn observe_positional_reads<T>(
    forbidden: Vec<Range<u64>>,
    operation: impl FnOnce() -> T,
) -> (T, PositionalReadReport) {
    POSITIONAL_READ_OBSERVER.with(|observer| {
        assert!(
            observer.borrow().is_none(),
            "positional read observers must not be nested"
        );
        observer.replace(Some(PositionalReadObservation {
            forbidden,
            report: PositionalReadReport::default(),
        }));
    });
    let result = operation();
    let report = POSITIONAL_READ_OBSERVER.with(|observer| {
        observer
            .replace(None)
            .expect("installed positional read observer")
            .report
    });
    (result, report)
}

#[cfg(test)]
fn record_positional_read(offset: u64, len: u64) {
    let end = offset.checked_add(len).unwrap_or(u64::MAX);
    let read = offset..end;
    POSITIONAL_READ_OBSERVER.with(|observer| {
        let mut observation = observer.borrow_mut();
        let Some(observation) = observation.as_mut() else {
            return;
        };
        observation.report.reads.push(read.clone());
        if observation
            .forbidden
            .iter()
            .any(|range| read.start < range.end && range.start < read.end)
        {
            observation.report.forbidden_reads.push(read);
        }
    });
}

#[cfg(unix)]
fn read_exact_at(file: &File, offset: u64, bytes: &mut [u8]) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let cursor_u64 = u64::try_from(cursor)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "read cursor exceeds u64"))?;
        let at = offset
            .checked_add(cursor_u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "read offset overflow"))?;
        let count = file.read_at(&mut bytes[cursor..], at)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "short positional read",
            ));
        }
        cursor += count;
    }
    Ok(())
}

#[cfg(windows)]
fn read_exact_at(file: &File, offset: u64, bytes: &mut [u8]) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let cursor_u64 = u64::try_from(cursor)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "read cursor exceeds u64"))?;
        let at = offset
            .checked_add(cursor_u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "read offset overflow"))?;
        let count = file.seek_read(&mut bytes[cursor..], at)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "short positional read",
            ));
        }
        cursor += count;
    }
    Ok(())
}
pub(super) fn validate_positional_entry_bounds(
    file: &File,
    container: &ContainerMeta,
    entry: &SectionEntry,
) -> Result<(), PrlLoadError> {
    postretro_level_format::validate_container_entry_bounds(
        container,
        entry,
        file.metadata()?.len(),
    )
    .map_err(PrlLoadError::FormatError)
}
