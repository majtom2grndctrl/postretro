// Where a mod's save files live and what each numbered slot holds.
// See: context/lib/scripting.md §5 (Durable State Store)

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{LedgerDocument, SaveFileError, read_document};

const CHARACTERS_DIR: &str = "characters";
const RUNS_DIR: &str = "runs";
const WORLD_FILE: &str = "world.json";
const SELECTION_FILE: &str = "selection.json";
const SELECTION_VERSION: u32 = 1;

/// A mod's save directory: `<data_dir>/<mod_id>/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveDir {
    root: PathBuf,
}

impl SaveDir {
    pub fn new(data_dir: &Path, mod_id: &str) -> Self {
        Self {
            root: data_dir.join(mod_id),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn character_path(&self, index: usize) -> PathBuf {
        self.root.join(CHARACTERS_DIR).join(format!("{index}.json"))
    }

    pub fn run_path(&self, index: usize) -> PathBuf {
        self.root.join(RUNS_DIR).join(format!("{index}.json"))
    }

    pub fn world_path(&self) -> PathBuf {
        self.root.join(WORLD_FILE)
    }

    pub fn selection_path(&self) -> PathBuf {
        self.root.join(SELECTION_FILE)
    }
}

/// What one numbered slot holds on disk.
#[derive(Clone, Debug, PartialEq)]
pub enum SlotFile {
    Empty,
    Occupied(LedgerDocument),
    /// A file this build cannot read. It stays on disk and occupies its slot
    /// until deleted; nothing but a delete touches it.
    Unreadable,
}

impl SlotFile {
    pub fn is_occupied(&self) -> bool {
        !matches!(self, Self::Empty)
    }

    pub fn document(&self) -> Option<&LedgerDocument> {
        match self {
            Self::Occupied(document) => Some(document),
            Self::Empty | Self::Unreadable => None,
        }
    }
}

/// Read one slot's file. An unreadable file warns once here, naming its path.
pub fn read_slot(path: &Path) -> SlotFile {
    match read_document(path) {
        Ok(Some(document)) => SlotFile::Occupied(document),
        Ok(None) => SlotFile::Empty,
        Err(error) => {
            log::warn!(
                "[State] {} cannot be read and will not be written: {error}",
                path.display()
            );
            SlotFile::Unreadable
        }
    }
}

/// Delete one slot's file. A missing file is already deleted.
pub fn delete_slot(path: &Path) -> Result<(), SaveFileError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// The last selected character and run, `-1` for none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub character: i32,
    pub run: i32,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            character: -1,
            run: -1,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SelectionFile {
    version: u32,
    #[serde(flatten)]
    selection: Selection,
}

/// Read the stored selection. A missing file is no selection; an unreadable
/// one warns and is no selection.
pub fn read_selection(path: &Path) -> Selection {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Selection::default(),
        Err(error) => {
            log::warn!("[State] cannot read {}: {error}", path.display());
            return Selection::default();
        }
    };
    match serde_json::from_slice::<SelectionFile>(&bytes) {
        Ok(file) if file.version == SELECTION_VERSION => file.selection,
        Ok(file) => {
            log::warn!(
                "[State] {} has unsupported version {}; no selection restored",
                path.display(),
                file.version
            );
            Selection::default()
        }
        Err(error) => {
            log::warn!("[State] cannot parse {}: {error}", path.display());
            Selection::default()
        }
    }
}

/// Store the selection atomically.
pub fn write_selection(path: &Path, selection: Selection) -> Result<(), SaveFileError> {
    let bytes = serde_json::to_vec_pretty(&SelectionFile {
        version: SELECTION_VERSION,
        selection,
    })?;
    super::write_bytes_atomically(path, &bytes)
}
