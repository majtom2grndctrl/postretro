// PRL container ownership and section reads for the runtime loader.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::fs::File;
use std::io::Cursor;
#[cfg(test)]
use std::path::Path;
use std::sync::Arc;

use postretro_level_format as prl_format;

use crate::prl::PrlLoadError;
use crate::sh_stream::read_vec_at;

/// One fully validated PRL container image plus its parsed table.
///
/// The loader still owns decoding and cross-section policy. This type owns
/// only the shared file image and the container-level inventory/read seam so
/// a later positional-reader path has one place to replace.
pub(crate) struct PrlContainer {
    backing: PrlBacking,
    metadata: prl_format::ContainerMeta,
}

enum PrlBacking {
    Whole(Vec<u8>),
    Positional(Arc<File>),
}

impl PrlContainer {
    #[cfg(test)]
    pub(crate) fn open(path: &str) -> Result<Self, PrlLoadError> {
        let path_ref = Path::new(path);
        if !path_ref.exists() {
            return Err(PrlLoadError::FileNotFound(path.to_string()));
        }

        let file_data = std::fs::read(path_ref)?;
        let mut cursor = Cursor::new(&file_data);
        let metadata = prl_format::read_container(&mut cursor)?;
        Ok(Self {
            backing: PrlBacking::Whole(file_data),
            metadata,
        })
    }

    /// Construct the streaming reader after the caller has already parsed and
    /// bounds-validated the table through positional reads. This deliberately
    /// retains the same file handle that the manifest will use for chunks.
    pub(crate) fn from_positional(file: Arc<File>, metadata: prl_format::ContainerMeta) -> Self {
        Self {
            backing: PrlBacking::Positional(file),
            metadata,
        }
    }

    /// Preserve a previously validated table while reading the complete legacy
    /// image from the same retained file handle. This avoids reopening a path
    /// between the id-50 validation pass and a legacy/off-mode decode.
    pub(crate) fn from_whole_bytes(
        file_data: Vec<u8>,
        metadata: prl_format::ContainerMeta,
    ) -> Self {
        Self {
            backing: PrlBacking::Whole(file_data),
            metadata,
        }
    }

    pub(crate) fn metadata(&self) -> &prl_format::ContainerMeta {
        &self.metadata
    }

    pub(crate) fn data(&self) -> &[u8] {
        match &self.backing {
            PrlBacking::Whole(file_data) => file_data,
            PrlBacking::Positional(_) => &[],
        }
    }

    /// Read a section by raw id, preserving the format crate's allocation and
    /// bounds-validation behavior.
    pub(crate) fn read_section(&self, section_id: u32) -> Result<Option<Vec<u8>>, PrlLoadError> {
        let Some(entry) = self.metadata.find_section(section_id) else {
            return Ok(None);
        };
        match &self.backing {
            PrlBacking::Whole(file_data) => {
                let mut cursor = Cursor::new(file_data);
                Ok(prl_format::read_section_data(
                    &mut cursor,
                    &self.metadata,
                    section_id,
                )?)
            }
            PrlBacking::Positional(file) => {
                if matches!(
                    section_id,
                    id if id == prl_format::SectionId::DeltaShVolumes as u32
                        || id == prl_format::SectionId::OctahedralShVolume as u32
                        || id == prl_format::SectionId::DirectShVolume as u32
                        || id == prl_format::SectionId::DirectShDeltaVolumes as u32
                        || id == prl_format::SectionId::AnimatedDirectShDeltaVolumes as u32
                ) {
                    return Err(PrlLoadError::SectionValidation {
                        section: "SH streaming",
                        message: format!(
                            "streaming attempted a forbidden whole-body read of section {section_id}"
                        ),
                    });
                }
                prl_format::validate_container_entry_bounds(
                    &self.metadata,
                    entry,
                    file.metadata()?.len(),
                )?;
                Ok(Some(read_vec_at(
                    file,
                    entry.offset,
                    entry.size,
                    "PRL non-streamed section",
                )?))
            }
        }
    }

    pub(crate) fn has_section(&self, section_id: u32) -> bool {
        self.metadata.find_section(section_id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positional_reader_refuses_large_streamed_atlas_before_file_io() {
        let path = std::env::temp_dir().join(format!(
            "postretro_streamed_atlas_guard_{}_{}.prl",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(&path, b"only a tiny backing file").unwrap();
        let file = Arc::new(File::open(&path).unwrap());
        let container = PrlContainer::from_positional(
            file,
            prl_format::ContainerMeta {
                header: prl_format::Header {
                    version: prl_format::CURRENT_VERSION,
                    section_count: 1,
                },
                sections: vec![prl_format::SectionEntry {
                    section_id: prl_format::SectionId::OctahedralShVolume as u32,
                    offset: 64,
                    size: 512 * 1024 * 1024,
                    version: 1,
                }],
            },
        );

        let error = container
            .read_section(prl_format::SectionId::OctahedralShVolume as u32)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("forbidden whole-body read of section 34")
        );
        let _ = std::fs::remove_file(path);
    }
}
