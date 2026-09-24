// PRL container framing, metadata, and section I/O.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::io::{self, Read, Seek, SeekFrom, Write};

use thiserror::Error;

pub const MAGIC: [u8; 4] = *b"PRL\0";
pub const CURRENT_VERSION: u16 = 4;

const HEADER_SIZE: usize = 8;
const SECTION_ENTRY_SIZE: usize = 22;

#[derive(Debug, Error)]
pub enum FormatError {
    #[error("invalid magic bytes: expected PRL\\0, got {found:?}")]
    InvalidMagic { found: [u8; 4] },

    #[error("unsupported format version {version} (expected {CURRENT_VERSION})")]
    UnsupportedVersion { version: u16 },

    #[error("truncated header: need {HEADER_SIZE} bytes, got {available}")]
    TruncatedHeader { available: usize },

    #[error("truncated section table: need {needed} bytes, got {available}")]
    TruncatedSectionTable { needed: usize, available: usize },

    #[error("section count {count} exceeds the PRL container maximum {max}")]
    TooManySections { count: usize, max: usize },

    #[error("PRL container size overflow")]
    ContainerSizeOverflow,

    #[error("failed to allocate {size} bytes for PRL container metadata")]
    ContainerMetadataAllocationFailed { size: usize },

    #[error("section {section_id} offset {offset} + size {size} overflows the PRL container")]
    SectionOffsetOverflow {
        section_id: u32,
        offset: u64,
        size: u64,
    },

    #[error("section offset {offset} + size {size} exceeds file length {file_len}")]
    SectionOutOfBounds {
        offset: u64,
        size: u64,
        file_len: u64,
    },

    #[error(
        "section {section_id} offset {offset} points before the end of the container metadata at {table_end}"
    )]
    SectionOverlapsContainerMetadata {
        section_id: u32,
        offset: u64,
        table_end: u64,
    },

    #[error("failed to allocate {size} bytes for section {section_id}")]
    SectionAllocationFailed { section_id: u32, size: u64 },

    #[error(transparent)]
    Io(#[from] io::Error),
}

pub type Result<T> = std::result::Result<T, FormatError>;

/// File header (8 bytes).
#[derive(Debug, Clone)]
pub struct Header {
    pub version: u16,
    pub section_count: u16,
}

/// One entry in the section table (22 bytes on disk).
#[derive(Debug, Clone)]
pub struct SectionEntry {
    pub section_id: u32,
    pub offset: u64,
    pub size: u64,
    pub version: u16,
}

/// Result of reading a PRL file's container metadata.
#[derive(Debug, Clone)]
pub struct ContainerMeta {
    pub header: Header,
    pub sections: Vec<SectionEntry>,
}

impl ContainerMeta {
    /// Find a section entry by its raw ID. Returns None if absent.
    pub fn find_section(&self, id: u32) -> Option<&SectionEntry> {
        self.sections.iter().find(|s| s.section_id == id)
    }
}

// -- Writing --

/// Section data to be written: an ID, per-section version, and raw bytes.
pub struct SectionBlob {
    pub section_id: u32,
    pub version: u16,
    pub data: Vec<u8>,
}

/// Section metadata used to write a PRL container header and table before its
/// payloads are available. The table's payload lengths must exactly match the
/// bytes subsequently written by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionDescriptor {
    pub section_id: u32,
    pub version: u16,
    pub byte_len: u64,
}

/// Write a PRL header and complete section table, leaving the writer positioned
/// at the first section payload. This supports callers that serialize each
/// payload incrementally after all table lengths are known.
pub fn write_prl_header_and_table<W: Write>(
    writer: &mut W,
    sections: &[SectionDescriptor],
) -> Result<()> {
    let section_count =
        u16::try_from(sections.len()).map_err(|_| FormatError::TooManySections {
            count: sections.len(),
            max: usize::from(u16::MAX),
        })?;
    let table_size = sections
        .len()
        .checked_mul(SECTION_ENTRY_SIZE)
        .ok_or(FormatError::ContainerSizeOverflow)?;
    let data_start = HEADER_SIZE
        .checked_add(table_size)
        .ok_or(FormatError::ContainerSizeOverflow)?;
    let mut current_offset =
        u64::try_from(data_start).map_err(|_| FormatError::ContainerSizeOverflow)?;
    let mut section_offsets = Vec::with_capacity(sections.len());
    for section in sections {
        section_offsets.push(current_offset);
        current_offset = current_offset.checked_add(section.byte_len).ok_or(
            FormatError::SectionOffsetOverflow {
                section_id: section.section_id,
                offset: current_offset,
                size: section.byte_len,
            },
        )?;
    }

    writer.write_all(&MAGIC)?;
    writer.write_all(&CURRENT_VERSION.to_le_bytes())?;
    writer.write_all(&section_count.to_le_bytes())?;

    for (section, offset) in sections.iter().zip(section_offsets) {
        writer.write_all(&section.section_id.to_le_bytes())?;
        writer.write_all(&offset.to_le_bytes())?;
        writer.write_all(&section.byte_len.to_le_bytes())?;
        writer.write_all(&section.version.to_le_bytes())?;
    }

    Ok(())
}

/// Write a complete PRL file: header, section table, then section data blobs.
pub fn write_prl<W: Write>(writer: &mut W, sections: &[SectionBlob]) -> Result<()> {
    let descriptors: Vec<_> = sections
        .iter()
        .map(|blob| {
            Ok(SectionDescriptor {
                section_id: blob.section_id,
                version: blob.version,
                byte_len: u64::try_from(blob.data.len())
                    .map_err(|_| FormatError::ContainerSizeOverflow)?,
            })
        })
        .collect::<Result<_>>()?;
    write_prl_header_and_table(writer, &descriptors)?;

    for blob in sections {
        writer.write_all(&blob.data)?;
    }

    Ok(())
}

// -- Reading --

/// Read the container metadata (header + section table) from a reader.
pub fn read_container<R: Read>(reader: &mut R) -> Result<ContainerMeta> {
    // Header
    let mut header_buf = [0u8; HEADER_SIZE];
    let bytes_read = read_exact_or_short(reader, &mut header_buf)?;
    if bytes_read < HEADER_SIZE {
        return Err(FormatError::TruncatedHeader {
            available: bytes_read,
        });
    }

    let mut magic = [0u8; 4];
    magic.copy_from_slice(&header_buf[0..4]);
    if magic != MAGIC {
        return Err(FormatError::InvalidMagic { found: magic });
    }

    let version = u16::from_le_bytes([header_buf[4], header_buf[5]]);
    if version != CURRENT_VERSION {
        return Err(FormatError::UnsupportedVersion { version });
    }

    let section_count = u16::from_le_bytes([header_buf[6], header_buf[7]]);

    // Section table
    let section_count_usize = usize::from(section_count);
    let table_size = section_count_usize * SECTION_ENTRY_SIZE;
    let mut table_buf = Vec::new();
    try_reserve_container_metadata(&mut table_buf, table_size, table_size)?;
    table_buf.resize(table_size, 0);
    let bytes_read = read_exact_or_short(reader, &mut table_buf)?;
    if bytes_read < table_size {
        return Err(FormatError::TruncatedSectionTable {
            needed: table_size,
            available: bytes_read,
        });
    }

    let entries_size = section_count_usize * std::mem::size_of::<SectionEntry>();
    let mut sections = Vec::new();
    try_reserve_container_metadata(&mut sections, section_count_usize, entries_size)?;
    for i in 0..section_count_usize {
        let base = i * SECTION_ENTRY_SIZE;
        let section_id = u32::from_le_bytes([
            table_buf[base],
            table_buf[base + 1],
            table_buf[base + 2],
            table_buf[base + 3],
        ]);
        let offset = u64::from_le_bytes([
            table_buf[base + 4],
            table_buf[base + 5],
            table_buf[base + 6],
            table_buf[base + 7],
            table_buf[base + 8],
            table_buf[base + 9],
            table_buf[base + 10],
            table_buf[base + 11],
        ]);
        let size = u64::from_le_bytes([
            table_buf[base + 12],
            table_buf[base + 13],
            table_buf[base + 14],
            table_buf[base + 15],
            table_buf[base + 16],
            table_buf[base + 17],
            table_buf[base + 18],
            table_buf[base + 19],
        ]);
        let version = u16::from_le_bytes([table_buf[base + 20], table_buf[base + 21]]);

        sections.push(SectionEntry {
            section_id,
            offset,
            size,
            version,
        });
    }

    Ok(ContainerMeta {
        header: Header {
            version: CURRENT_VERSION,
            section_count,
        },
        sections,
    })
}

fn try_reserve_container_metadata<T>(
    buffer: &mut Vec<T>,
    additional: usize,
    size: usize,
) -> Result<()> {
    buffer
        .try_reserve_exact(additional)
        .map_err(|_| FormatError::ContainerMetadataAllocationFailed { size })
}

/// Borrow a specific section's raw bytes by section ID from a complete PRL
/// image. Returns `None` if the section ID is not present in the file.
///
/// This is the allocation-free counterpart to [`read_section_data`]. Loaders
/// that already retain the complete file can inspect a raw section size before
/// invoking a decoder whose tables would allocate from untrusted input.
pub fn section_data_from_bytes<'a>(
    file_data: &'a [u8],
    meta: &ContainerMeta,
    section_id: u32,
) -> Result<Option<&'a [u8]>> {
    let entry = match meta.find_section(section_id) {
        Some(entry) => entry,
        None => return Ok(None),
    };

    let file_len = file_data.len() as u64;
    let end = validate_section_bounds(meta, entry, file_len)?;

    let start = usize::try_from(entry.offset).map_err(|_| FormatError::SectionOutOfBounds {
        offset: entry.offset,
        size: entry.size,
        file_len,
    })?;
    let end = usize::try_from(end).map_err(|_| FormatError::SectionOutOfBounds {
        offset: entry.offset,
        size: entry.size,
        file_len,
    })?;
    Ok(Some(&file_data[start..end]))
}

/// Read a specific section's raw bytes by section ID.
/// Returns None if the section ID is not present in the file.
/// Validates offset+size against the actual file length.
pub fn read_section_data<R: Read + Seek>(
    reader: &mut R,
    meta: &ContainerMeta,
    section_id: u32,
) -> Result<Option<Vec<u8>>> {
    let entry = match meta.find_section(section_id) {
        Some(e) => e,
        None => return Ok(None),
    };

    let file_len = reader.seek(SeekFrom::End(0))?;
    validate_section_bounds(meta, entry, file_len)?;

    let size = usize::try_from(entry.size).map_err(|_| FormatError::SectionAllocationFailed {
        section_id: entry.section_id,
        size: entry.size,
    })?;
    reader.seek(SeekFrom::Start(entry.offset))?;
    let mut buf = Vec::new();
    buf.try_reserve_exact(size)
        .map_err(|_| FormatError::SectionAllocationFailed {
            section_id: entry.section_id,
            size: entry.size,
        })?;
    buf.resize(size, 0);
    reader.read_exact(&mut buf)?;
    Ok(Some(buf))
}

/// Validate every entry in a parsed container table against a known file
/// length without reading any section body.
///
/// Positional readers use this after fetching only the header and table. It
/// keeps their bounds checks identical to [`read_section_data`] without
/// requiring a seek cursor over the full PRL image.
pub fn validate_container_bounds(meta: &ContainerMeta, file_len: u64) -> Result<()> {
    for entry in &meta.sections {
        validate_container_entry_bounds(meta, entry, file_len)?;
    }
    Ok(())
}

/// Validate one table entry against a known file length. Positional consumers
/// use this for the sections they actually read, so optional legacy sections
/// can retain their established soft-failure policy without weakening bounds
/// checks for streamed metadata or required payloads.
pub fn validate_container_entry_bounds(
    meta: &ContainerMeta,
    entry: &SectionEntry,
    file_len: u64,
) -> Result<()> {
    validate_section_bounds(meta, entry, file_len).map(|_| ())
}

fn validate_section_bounds(
    meta: &ContainerMeta,
    entry: &SectionEntry,
    file_len: u64,
) -> Result<u64> {
    let table_end =
        HEADER_SIZE as u64 + u64::from(meta.header.section_count) * SECTION_ENTRY_SIZE as u64;
    if entry.offset < table_end {
        return Err(FormatError::SectionOverlapsContainerMetadata {
            section_id: entry.section_id,
            offset: entry.offset,
            table_end,
        });
    }

    let end = entry
        .offset
        .checked_add(entry.size)
        .ok_or(FormatError::SectionOffsetOverflow {
            section_id: entry.section_id,
            offset: entry.offset,
            size: entry.size,
        })?;
    if end > file_len {
        return Err(FormatError::SectionOutOfBounds {
            offset: entry.offset,
            size: entry.size,
            file_len,
        });
    }

    Ok(end)
}

/// Read into buf, returning actual bytes read instead of erroring on EOF.
fn read_exact_or_short<R: Read>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match reader.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SectionId;
    use std::io::Cursor;

    struct MaxLengthReader;

    impl Read for MaxLengthReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    impl Seek for MaxLengthReader {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            match position {
                SeekFrom::End(0) => Ok(u64::MAX),
                SeekFrom::Start(offset) => Ok(offset),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unsupported test seek",
                )),
            }
        }
    }

    fn make_test_sections() -> Vec<SectionBlob> {
        vec![
            SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: vec![0xDE, 0xAD, 0xBE, 0xEF],
            },
            SectionBlob {
                section_id: SectionId::Portals as u32,
                version: 1,
                data: vec![0xCA, 0xFE],
            },
        ]
    }

    #[test]
    fn round_trip() {
        let sections = make_test_sections();
        let mut buf = Vec::new();
        write_prl(&mut buf, &sections).unwrap();

        let mut cursor = Cursor::new(&buf);
        let meta = read_container(&mut cursor).unwrap();

        assert_eq!(meta.header.version, CURRENT_VERSION);
        assert_eq!(meta.header.section_count, 2);
        assert_eq!(meta.sections.len(), 2);
        assert_eq!(meta.sections[0].section_id, SectionId::Geometry as u32);
        assert_eq!(meta.sections[1].section_id, SectionId::Portals as u32);

        let geom = read_section_data(&mut cursor, &meta, SectionId::Geometry as u32)
            .unwrap()
            .unwrap();
        assert_eq!(geom, vec![0xDE, 0xAD, 0xBE, 0xEF]);

        let portals = read_section_data(&mut cursor, &meta, SectionId::Portals as u32)
            .unwrap()
            .unwrap();
        assert_eq!(portals, vec![0xCA, 0xFE]);
    }

    #[test]
    fn table_first_write_matches_complete_writer_bytes() {
        let sections = make_test_sections();
        let mut complete = Vec::new();
        write_prl(&mut complete, &sections).unwrap();

        let descriptors: Vec<_> = sections
            .iter()
            .map(|section| SectionDescriptor {
                section_id: section.section_id,
                version: section.version,
                byte_len: section.data.len() as u64,
            })
            .collect();
        let mut table_first = Vec::new();
        write_prl_header_and_table(&mut table_first, &descriptors).unwrap();
        for section in &sections {
            table_first.extend_from_slice(&section.data);
        }

        assert_eq!(table_first, complete);
    }

    #[test]
    fn write_prl_preserves_legacy_container_bytes() {
        let mut actual = Vec::new();
        write_prl(&mut actual, &make_test_sections()).unwrap();

        let expected = [
            0x50, 0x52, 0x4c, 0x00, 0x04, 0x00, 0x02, 0x00, // Header.
            0x11, 0x00, 0x00, 0x00, // Geometry id.
            0x34, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Geometry offset.
            0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Geometry size.
            0x01, 0x00, // Geometry version.
            0x0f, 0x00, 0x00, 0x00, // Portals id.
            0x38, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Portals offset.
            0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Portals size.
            0x01, 0x00, // Portals version.
            0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe,
        ];

        assert_eq!(actual, expected);
    }

    #[test]
    fn table_writer_rejects_65_536_sections_before_writing() {
        let descriptors = vec![
            SectionDescriptor {
                section_id: SectionId::Geometry as u32,
                version: 1,
                byte_len: 0,
            };
            usize::from(u16::MAX) + 1
        ];
        let mut output = Vec::new();

        let error = write_prl_header_and_table(&mut output, &descriptors)
            .expect_err("the u16 section count must not truncate");

        assert!(matches!(
            error,
            FormatError::TooManySections {
                count: 65_536,
                max: 65_535,
            }
        ));
        assert!(
            output.is_empty(),
            "validation must precede the header write"
        );
    }

    #[test]
    fn table_writer_rejects_section_offset_overflow_before_writing() {
        let descriptors = [SectionDescriptor {
            section_id: SectionId::Geometry as u32,
            version: 1,
            byte_len: u64::MAX,
        }];
        let mut output = Vec::new();

        let error = write_prl_header_and_table(&mut output, &descriptors)
            .expect_err("the section end must not wrap around u64");

        assert!(matches!(
            error,
            FormatError::SectionOffsetOverflow {
                section_id,
                offset: 30,
                size: u64::MAX,
            } if section_id == SectionId::Geometry as u32
        ));
        assert!(
            output.is_empty(),
            "validation must precede the header write"
        );
    }

    #[test]
    fn borrows_section_data_from_complete_prl_image() {
        let sections = make_test_sections();
        let mut file_data = Vec::new();
        write_prl(&mut file_data, &sections).unwrap();

        let mut cursor = Cursor::new(&file_data);
        let meta = read_container(&mut cursor).unwrap();
        let geometry = section_data_from_bytes(&file_data, &meta, SectionId::Geometry as u32)
            .unwrap()
            .expect("geometry section is present");

        assert_eq!(geometry, [0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn rejects_invalid_magic() {
        let mut buf = vec![0u8; 64];
        buf[0..4].copy_from_slice(b"NOPE");
        buf[4..6].copy_from_slice(&1u16.to_le_bytes());

        let mut cursor = Cursor::new(&buf);
        let err = read_container(&mut cursor).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid magic"), "unexpected error: {msg}");
    }

    #[test]
    fn rejects_unsupported_version() {
        let mut buf = vec![0u8; 64];
        buf[0..4].copy_from_slice(&MAGIC);
        buf[4..6].copy_from_slice(&99u16.to_le_bytes());

        let mut cursor = Cursor::new(&buf);
        let err = read_container(&mut cursor).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("99"), "error should include version: {msg}");
    }

    /// Pinned regression guard for the v3 → v4 bump made by the baked-texture-mips
    /// plan (Task 1). Once `CURRENT_VERSION` was bumped, every previously-shipped
    /// v3 PRL must be rejected with a specific `UnsupportedVersion { version: 3 }`
    /// error. The generic `rejects_unsupported_version` test above uses `99` and
    /// derives the expected version from `CURRENT_VERSION`, so it does not pin
    /// the v3-specific rejection. This test pins `3` literally so a future
    /// accidental rollback of `CURRENT_VERSION` to `3` would surface here.
    #[test]
    fn rejects_v3_after_bump() {
        let mut buf = vec![0u8; HEADER_SIZE];
        buf[0..4].copy_from_slice(&MAGIC);
        buf[4..6].copy_from_slice(&3u16.to_le_bytes());
        buf[6..8].copy_from_slice(&0u16.to_le_bytes());

        let mut cursor = Cursor::new(&buf);
        let err = read_container(&mut cursor).unwrap_err();
        assert!(
            matches!(err, FormatError::UnsupportedVersion { version: 3 }),
            "expected UnsupportedVersion {{ version: 3 }}, got {err:?}"
        );
    }

    #[test]
    fn skips_unknown_section_ids() {
        let sections = vec![
            SectionBlob {
                section_id: 999,
                version: 1,
                data: vec![0x01],
            },
            SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: vec![0x02],
            },
        ];
        let mut buf = Vec::new();
        write_prl(&mut buf, &sections).unwrap();

        let mut cursor = Cursor::new(&buf);
        let meta = read_container(&mut cursor).unwrap();
        // Unknown section is present in the table but SectionId::from_u32 returns None
        assert!(SectionId::from_u32(meta.sections[0].section_id).is_none());
        // Known section still readable
        let geom = read_section_data(&mut cursor, &meta, SectionId::Geometry as u32)
            .unwrap()
            .unwrap();
        assert_eq!(geom, vec![0x02]);
    }

    #[test]
    fn absent_section_returns_none() {
        let sections = vec![SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: vec![0x01],
        }];
        let mut buf = Vec::new();
        write_prl(&mut buf, &sections).unwrap();

        let mut cursor = Cursor::new(&buf);
        let meta = read_container(&mut cursor).unwrap();
        let result = read_section_data(&mut cursor, &meta, SectionId::BspNodes as u32).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn validates_section_bounds() {
        let sections = vec![SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: vec![0x01],
        }];
        let mut buf = Vec::new();
        write_prl(&mut buf, &sections).unwrap();

        // Tamper: inflate the section size beyond the file
        let mut cursor = Cursor::new(&buf);
        let mut meta = read_container(&mut cursor).unwrap();
        meta.sections[0].size = 9999;

        let err = read_section_data(&mut cursor, &meta, SectionId::Geometry as u32).unwrap_err();
        assert!(matches!(err, FormatError::SectionOutOfBounds { .. }));
    }

    #[test]
    fn section_readers_reject_offset_plus_size_overflow() {
        let meta = ContainerMeta {
            header: Header {
                version: CURRENT_VERSION,
                section_count: 1,
            },
            sections: vec![SectionEntry {
                section_id: SectionId::Geometry as u32,
                offset: u64::MAX,
                size: 1,
                version: 1,
            }],
        };
        let file_data = [];

        let borrowed_error = section_data_from_bytes(&file_data, &meta, SectionId::Geometry as u32)
            .expect_err("borrowed section bounds must reject overflow");
        let mut cursor = Cursor::new(file_data);
        let owned_error = read_section_data(&mut cursor, &meta, SectionId::Geometry as u32)
            .expect_err("owned section bounds must reject overflow");

        assert!(matches!(
            borrowed_error,
            FormatError::SectionOffsetOverflow {
                section_id,
                offset: u64::MAX,
                size: 1,
            } if section_id == SectionId::Geometry as u32
        ));
        assert!(matches!(
            owned_error,
            FormatError::SectionOffsetOverflow {
                section_id,
                offset: u64::MAX,
                size: 1,
            } if section_id == SectionId::Geometry as u32
        ));
    }

    // Regression: container metadata reservations could abort instead of returning
    // FormatError.
    #[test]
    fn container_metadata_reservation_reports_allocation_failure() {
        let mut metadata = Vec::<u8>::new();

        let error = try_reserve_container_metadata(&mut metadata, usize::MAX, usize::MAX)
            .expect_err("an impossible metadata allocation must return a format error");

        assert!(matches!(
            error,
            FormatError::ContainerMetadataAllocationFailed { size: usize::MAX }
        ));
    }

    // Regression: an in-file section offset could point back into the header or table.
    #[test]
    fn section_readers_reject_payloads_inside_container_metadata() {
        let sections = make_test_sections();
        let mut file_data = Vec::new();
        write_prl(&mut file_data, &sections).unwrap();
        file_data[12..20].copy_from_slice(&(HEADER_SIZE as u64).to_le_bytes());

        let mut cursor = Cursor::new(&file_data);
        let meta = read_container(&mut cursor).unwrap();

        let borrowed_error = section_data_from_bytes(&file_data, &meta, SectionId::Geometry as u32)
            .expect_err("borrowed reads must reject payloads inside the section table");
        let owned_error = read_section_data(&mut cursor, &meta, SectionId::Geometry as u32)
            .expect_err("owned reads must reject payloads inside the section table");
        let expected_table_end = (HEADER_SIZE + sections.len() * SECTION_ENTRY_SIZE) as u64;

        assert!(matches!(
            borrowed_error,
            FormatError::SectionOverlapsContainerMetadata {
                section_id,
                offset,
                table_end,
            } if section_id == SectionId::Geometry as u32
                && offset == HEADER_SIZE as u64
                && table_end == expected_table_end
        ));
        assert!(matches!(
            owned_error,
            FormatError::SectionOverlapsContainerMetadata {
                section_id,
                offset,
                table_end,
            } if section_id == SectionId::Geometry as u32
                && offset == HEADER_SIZE as u64
                && table_end == expected_table_end
        ));
    }

    #[test]
    fn owned_section_reader_rejects_unallocatable_u64_size() {
        let offset = (HEADER_SIZE + SECTION_ENTRY_SIZE) as u64;
        let meta = ContainerMeta {
            header: Header {
                version: CURRENT_VERSION,
                section_count: 1,
            },
            sections: vec![SectionEntry {
                section_id: SectionId::Geometry as u32,
                offset,
                size: u64::MAX - offset,
                version: 1,
            }],
        };
        let mut reader = MaxLengthReader;

        let error = read_section_data(&mut reader, &meta, SectionId::Geometry as u32)
            .expect_err("an unrepresentable or unallocatable section size must be rejected");

        assert!(matches!(
            error,
            FormatError::SectionAllocationFailed { section_id, size }
                if section_id == SectionId::Geometry as u32 && size == u64::MAX - offset
        ));
    }

    #[test]
    fn little_endian_byte_order() {
        let sections = vec![SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: vec![0xAA],
        }];
        let mut buf = Vec::new();
        write_prl(&mut buf, &sections).unwrap();

        // Magic: b"PRL\0" at offset 0
        assert_eq!(&buf[0..4], b"PRL\0");

        // Version (u16 LE) at offset 4 reflects CURRENT_VERSION.
        let version_bytes = CURRENT_VERSION.to_le_bytes();
        assert_eq!(buf[4], version_bytes[0]);
        assert_eq!(buf[5], version_bytes[1]);

        // Section count (u16 LE) at offset 6: value 1 => [0x01, 0x00]
        assert_eq!(buf[6], 0x01);
        assert_eq!(buf[7], 0x00);

        // First section entry starts at offset 8 (HEADER_SIZE)
        // section_id (u32 LE): Geometry=17 => [0x11, 0x00, 0x00, 0x00]
        assert_eq!(&buf[8..12], &[0x11, 0x00, 0x00, 0x00]);

        // offset (u64 LE): data starts at 8 + 22 = 30 => 0x1E
        assert_eq!(buf[12], 0x1E);
        assert_eq!(buf[13..20], [0x00; 7]);

        // size (u64 LE): 1 byte => [0x01, 0x00, ...]
        assert_eq!(buf[20], 0x01);
        assert_eq!(buf[21..28], [0x00; 7]);

        // version (u16 LE): 1 => [0x01, 0x00]
        assert_eq!(buf[28], 0x01);
        assert_eq!(buf[29], 0x00);

        // Data at offset 30
        assert_eq!(buf[30], 0xAA);
    }

    #[test]
    fn truncated_header() {
        let buf = vec![0x50, 0x52, 0x4C]; // "PRL" without null and rest
        let mut cursor = Cursor::new(&buf);
        let err = read_container(&mut cursor).unwrap_err();
        assert!(matches!(err, FormatError::TruncatedHeader { .. }));
    }

    #[test]
    fn truncated_section_table() {
        let mut buf = vec![0u8; HEADER_SIZE + 5]; // header + partial entry
        buf[0..4].copy_from_slice(&MAGIC);
        buf[4..6].copy_from_slice(&CURRENT_VERSION.to_le_bytes());
        buf[6..8].copy_from_slice(&1u16.to_le_bytes()); // 1 section claimed

        let mut cursor = Cursor::new(&buf);
        let err = read_container(&mut cursor).unwrap_err();
        assert!(matches!(err, FormatError::TruncatedSectionTable { .. }));
    }
}
