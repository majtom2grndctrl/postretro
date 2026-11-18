// Game save files: the durable ledger document, its loadout record and atomic file I/O.
// See: context/lib/scripting.md §5 (Durable State Store) · context/lib/networking.md §Session-state ledger

mod document;
mod loadout;
mod slots;

#[cfg(test)]
mod tests;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub use document::{LedgerDocument, LevelRecord, RunRecord, SeatRecord};
pub use loadout::{LayerRecord, LoadoutRecord};
pub use slots::{
    SaveDir, Selection, SlotFile, delete_slot, read_selection, read_slot, write_selection,
};

/// The ledger document format this build writes.
pub const CURRENT_SAVE_VERSION: u32 = 1;
/// The oldest ledger document format this build reads.
pub const OLDEST_SUPPORTED_SAVE_VERSION: u32 = 1;

/// Why a save file could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum SaveFileError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("not a ledger document: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("save version {0} is outside the supported range {OLDEST_SUPPORTED_SAVE_VERSION}..={CURRENT_SAVE_VERSION}")]
    UnsupportedVersion(u32),
    #[error("section `{section}` is malformed: {error}")]
    MalformedSection {
        section: &'static str,
        error: serde_json::Error,
    },
}

/// Read a ledger document. `Ok(None)` when no file exists; an unsupported
/// version, a parse failure or a malformed known section is an error, and the
/// caller must treat the slot as unreadable rather than overwrite it.
pub fn read_document(path: &Path) -> Result<Option<LedgerDocument>, SaveFileError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    LedgerDocument::from_slice(&bytes).map(Some)
}

/// Write a ledger document atomically: the whole document lands in a sibling
/// temporary file, then a rename replaces the previous file.
pub fn write_document(path: &Path, document: &LedgerDocument) -> Result<(), SaveFileError> {
    stage_document(path, document)?.commit()
}

/// A document written to its temporary file and not yet renamed over its
/// destination. Dropping it uncommitted is what a kill before the rename
/// leaves behind: the previous file is untouched.
#[must_use = "an uncommitted staged write leaves the previous file in place"]
pub struct StagedWrite {
    tmp_path: PathBuf,
    path: PathBuf,
}

impl StagedWrite {
    /// Rename the staged file over its destination.
    pub fn commit(self) -> Result<(), SaveFileError> {
        if let Err(error) = fs::rename(&self.tmp_path, &self.path) {
            let _ = fs::remove_file(&self.tmp_path);
            return Err(error.into());
        }
        Ok(())
    }
}

/// Write `document` to the temporary sibling of `path`, creating parent
/// directories, without replacing `path`.
pub fn stage_document(path: &Path, document: &LedgerDocument) -> Result<StagedWrite, SaveFileError> {
    stage_bytes(path, &document.to_vec()?)
}

fn write_bytes_atomically(path: &Path, bytes: &[u8]) -> Result<(), SaveFileError> {
    stage_bytes(path, bytes)?.commit()
}

fn stage_bytes(path: &Path, bytes: &[u8]) -> Result<StagedWrite, SaveFileError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = tmp_path_for(path);
    if let Err(error) = fs::write(&tmp_path, bytes) {
        let _ = fs::remove_file(&tmp_path);
        return Err(error.into());
    }
    Ok(StagedWrite {
        tmp_path,
        path: path.to_path_buf(),
    })
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}
