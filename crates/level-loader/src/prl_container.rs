// PRL container ownership and section reads for the runtime loader.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::borrow::Cow;
#[cfg(test)]
use std::fs::File;
use std::io::Cursor;
#[cfg(test)]
use std::path::Path;
use std::sync::Arc;

use postretro_level_format as prl_format;

use crate::prl::PrlLoadError;
use crate::prl_file::{PrlFile, PrlReadCounters};
use crate::sh_stream::read_vec_at;

/// One fully validated PRL container image plus its parsed table.
///
/// The loader still owns decoding and cross-section policy. This type owns
/// only the backing (a whole file image, or the retained file read
/// positionally) and the container-level inventory/read seam.
pub(crate) struct PrlContainer {
    backing: PrlBacking,
    metadata: prl_format::ContainerMeta,
    reads: Option<Arc<PrlReadCounters>>,
}

enum PrlBacking {
    Whole(Vec<u8>),
    /// Sections are read on demand. `sh_bodies_streamed` forbids whole-body
    /// reads of the SH families that id 50 streams; lightmap streaming uses
    /// this backing with SH bodies read whole as legacy decode expects.
    Positional {
        file: Arc<PrlFile>,
        sh_bodies_streamed: bool,
    },
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
            reads: None,
        })
    }

    /// Construct the positional reader after the caller has already parsed
    /// and bounds-validated the table through positional reads. This retains
    /// the same file handle the SH and lightmap manifests use for chunks.
    pub(crate) fn from_positional(
        file: Arc<PrlFile>,
        metadata: prl_format::ContainerMeta,
        sh_bodies_streamed: bool,
    ) -> Self {
        let reads = Some(file.read_counters().clone());
        Self {
            backing: PrlBacking::Positional {
                file,
                sh_bodies_streamed,
            },
            metadata,
            reads,
        }
    }

    /// Preserve a previously validated table while reading the complete legacy
    /// image from the same retained file handle. This avoids reopening a path
    /// between the id-50 validation pass and a legacy/off-mode decode.
    pub(crate) fn from_whole_bytes(
        file_data: Vec<u8>,
        metadata: prl_format::ContainerMeta,
        reads: Option<Arc<PrlReadCounters>>,
    ) -> Self {
        Self {
            backing: PrlBacking::Whole(file_data),
            metadata,
            reads,
        }
    }

    pub(crate) fn metadata(&self) -> &prl_format::ContainerMeta {
        &self.metadata
    }

    /// The retained handle when sections are read positionally: the only
    /// backing a streamed resource can keep reading after load.
    pub(crate) fn retained_file(&self) -> Option<&Arc<PrlFile>> {
        match &self.backing {
            PrlBacking::Whole(_) => None,
            PrlBacking::Positional { file, .. } => Some(file),
        }
    }

    /// The counters of the file this container was read from, when it came
    /// through the positional reader.
    pub(crate) fn read_counters(&self) -> Option<&Arc<PrlReadCounters>> {
        self.reads.as_ref()
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
            PrlBacking::Positional {
                file,
                sh_bodies_streamed,
            } => {
                if *sh_bodies_streamed
                    && matches!(
                        section_id,
                        id if id == prl_format::SectionId::DeltaShVolumes as u32
                            || id == prl_format::SectionId::OctahedralShVolume as u32
                            || id == prl_format::SectionId::DirectShVolume as u32
                            || id == prl_format::SectionId::DirectShDeltaVolumes as u32
                            || id == prl_format::SectionId::AnimatedDirectShDeltaVolumes as u32
                    )
                {
                    return Err(PrlLoadError::SectionValidation {
                        section: "SH streaming",
                        message: format!(
                            "streaming attempted a forbidden whole-body read of section {section_id}"
                        ),
                    });
                }
                prl_format::validate_container_entry_bounds(&self.metadata, entry, file.len()?)?;
                Ok(Some(read_vec_at(
                    file,
                    entry.offset,
                    entry.size,
                    "PRL non-streamed section",
                )?))
            }
        }
    }

    /// A section body, borrowed from a whole image or read positionally.
    /// Bounds are validated before any read, as [`Self::read_section`] does.
    pub(crate) fn section_bytes(
        &self,
        section_id: u32,
    ) -> Result<Option<Cow<'_, [u8]>>, PrlLoadError> {
        match &self.backing {
            PrlBacking::Whole(file_data) => {
                Ok(
                    prl_format::section_data_from_bytes(file_data, &self.metadata, section_id)?
                        .map(Cow::Borrowed),
                )
            }
            PrlBacking::Positional { .. } => Ok(self.read_section(section_id)?.map(Cow::Owned)),
        }
    }

    /// A section's length after validating its container bounds, without
    /// reading its body. Lets a size policy refuse a section before it is
    /// read or allocated.
    pub(crate) fn validated_section_len(
        &self,
        section_id: u32,
    ) -> Result<Option<u64>, PrlLoadError> {
        let Some(entry) = self.metadata.find_section(section_id) else {
            return Ok(None);
        };
        let file_len = match &self.backing {
            PrlBacking::Whole(file_data) => file_data.len() as u64,
            PrlBacking::Positional { file, .. } => file.len()?,
        };
        prl_format::validate_container_entry_bounds(&self.metadata, entry, file_len)?;
        Ok(Some(entry.size))
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
        // A test thread's name holds `::`, which Windows refuses in a file
        // name, so the backing file lives in its own temporary directory.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("streamed_atlas_guard.prl");
        std::fs::write(&path, b"only a tiny backing file").unwrap();
        let file = Arc::new(PrlFile::new(File::open(&path).unwrap()));
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
            true,
        );

        let error = container
            .read_section(prl_format::SectionId::OctahedralShVolume as u32)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("forbidden whole-body read of section 34")
        );
    }
}
