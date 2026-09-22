// PRL container ownership and section reads for the runtime loader.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::io::Cursor;
use std::path::Path;

use postretro_level_format as prl_format;

use crate::prl::PrlLoadError;

/// One fully validated PRL container image plus its parsed table.
///
/// The loader still owns decoding and cross-section policy. This type owns
/// only the shared file image and the container-level inventory/read seam so
/// a later positional-reader path has one place to replace.
pub(crate) struct PrlContainer {
    file_data: Vec<u8>,
    metadata: prl_format::ContainerMeta,
}

impl PrlContainer {
    pub(crate) fn open(path: &str) -> Result<Self, PrlLoadError> {
        let path_ref = Path::new(path);
        if !path_ref.exists() {
            return Err(PrlLoadError::FileNotFound(path.to_string()));
        }

        let file_data = std::fs::read(path_ref)?;
        let mut cursor = Cursor::new(&file_data);
        let metadata = prl_format::read_container(&mut cursor)?;
        Ok(Self {
            file_data,
            metadata,
        })
    }

    pub(crate) fn metadata(&self) -> &prl_format::ContainerMeta {
        &self.metadata
    }

    pub(crate) fn data(&self) -> &[u8] {
        &self.file_data
    }

    /// Read a section by raw id, preserving the format crate's allocation and
    /// bounds-validation behavior.
    pub(crate) fn read_section(&self, section_id: u32) -> Result<Option<Vec<u8>>, PrlLoadError> {
        let mut cursor = Cursor::new(&self.file_data);
        Ok(prl_format::read_section_data(
            &mut cursor,
            &self.metadata,
            section_id,
        )?)
    }

    pub(crate) fn has_section(&self, section_id: u32) -> bool {
        self.metadata.find_section(section_id).is_some()
    }
}
