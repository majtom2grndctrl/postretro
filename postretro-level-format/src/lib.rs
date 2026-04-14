// PRL binary container format: header, section table, read/write API.
// See: context/lib/build_pipeline.md §PRL

pub mod alpha_lights;
pub mod bsp;
pub mod bvh;
pub mod geometry;
pub mod leaf_pvs;
pub mod octahedral;
pub mod portals;
pub mod sh_volume;
pub mod texture_names;
pub mod visibility;

use std::io::{self, Read, Seek, SeekFrom, Write};

use thiserror::Error;

pub const MAGIC: [u8; 4] = *b"PRL\0";
pub const CURRENT_VERSION: u16 = 1;

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

    #[error("section offset {offset} + size {size} exceeds file length {file_len}")]
    SectionOutOfBounds {
        offset: u64,
        size: u64,
        file_len: u64,
    },

    #[error(transparent)]
    Io(#[from] io::Error),
}

pub type Result<T> = std::result::Result<T, FormatError>;

/// Known section type IDs.
///
/// Retired IDs are intentionally omitted so they cannot be re-used by accident.
/// The loader skips unknown IDs gracefully; older `.prl` files produced before
/// the BVH refactor will fail to decode because the geometry format changed
/// and a `Bvh` section is now required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SectionId {
    /// Flat array of BSP interior nodes (splitting planes + child references).
    BspNodes = 12,

    /// Flat array of BSP leaf records (face ranges, bounds, PVS references).
    BspLeaves = 13,

    /// Per-leaf RLE-compressed PVS bitsets (concatenated blob).
    LeafPvs = 14,

    /// Portal graph for runtime portal traversal.
    Portals = 15,

    /// Flat list of texture name strings, indexed by `FaceMeta.texture_index`.
    TextureNames = 16,

    /// Geometry section: 28-byte vertices (position + UV + octahedral normal
    /// + octahedral tangent with bitangent sign) and 8-byte `FaceMeta`.
    Geometry = 17,

    /// AlphaLights section (interim). Flat per-light record array for the
    /// direct-lighting path. Will be replaced by an entity-system
    /// serialisation in Milestone 6+.
    /// See `alpha_lights::AlphaLightsSection`.
    AlphaLights = 18,

    /// Global BVH: flat node + leaf arrays. See `bvh::BvhSection`.
    Bvh = 19,

    /// SH irradiance volume: regular-grid L2 probes plus optional per-animated
    /// light monochrome layers. See `sh_volume::ShVolumeSection`.
    ShVolume = 20,
}

impl SectionId {
    pub fn from_u32(value: u32) -> Option<Self> {
        match value {
            12 => Some(Self::BspNodes),
            13 => Some(Self::BspLeaves),
            14 => Some(Self::LeafPvs),
            15 => Some(Self::Portals),
            16 => Some(Self::TextureNames),
            17 => Some(Self::Geometry),
            18 => Some(Self::AlphaLights),
            19 => Some(Self::Bvh),
            20 => Some(Self::ShVolume),
            _ => None,
        }
    }
}

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

/// Write a complete PRL file: header, section table, then section data blobs.
pub fn write_prl<W: Write>(writer: &mut W, sections: &[SectionBlob]) -> Result<()> {
    let section_count = sections.len() as u16;

    // Header
    writer.write_all(&MAGIC)?;
    writer.write_all(&CURRENT_VERSION.to_le_bytes())?;
    writer.write_all(&section_count.to_le_bytes())?;

    // Compute offsets: data starts after header + section table
    let data_start = HEADER_SIZE + (sections.len() * SECTION_ENTRY_SIZE);
    let mut current_offset = data_start as u64;

    // Section table
    for blob in sections {
        writer.write_all(&blob.section_id.to_le_bytes())?;
        writer.write_all(&current_offset.to_le_bytes())?;
        writer.write_all(&(blob.data.len() as u64).to_le_bytes())?;
        writer.write_all(&blob.version.to_le_bytes())?;
        current_offset += blob.data.len() as u64;
    }

    // Section data
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
    let table_size = section_count as usize * SECTION_ENTRY_SIZE;
    let mut table_buf = vec![0u8; table_size];
    let bytes_read = read_exact_or_short(reader, &mut table_buf)?;
    if bytes_read < table_size {
        return Err(FormatError::TruncatedSectionTable {
            needed: table_size,
            available: bytes_read,
        });
    }

    let mut sections = Vec::with_capacity(section_count as usize);
    for i in 0..section_count as usize {
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
    if entry.offset + entry.size > file_len {
        return Err(FormatError::SectionOutOfBounds {
            offset: entry.offset,
            size: entry.size,
            file_len,
        });
    }

    reader.seek(SeekFrom::Start(entry.offset))?;
    let mut buf = vec![0u8; entry.size as usize];
    reader.read_exact(&mut buf)?;
    Ok(Some(buf))
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
    use std::io::Cursor;

    fn make_test_sections() -> Vec<SectionBlob> {
        vec![
            SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: vec![0xDE, 0xAD, 0xBE, 0xEF],
            },
            SectionBlob {
                section_id: SectionId::LeafPvs as u32,
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

        assert_eq!(meta.header.version, 1);
        assert_eq!(meta.header.section_count, 2);
        assert_eq!(meta.sections.len(), 2);
        assert_eq!(meta.sections[0].section_id, SectionId::Geometry as u32);
        assert_eq!(meta.sections[1].section_id, SectionId::LeafPvs as u32);

        let geom = read_section_data(&mut cursor, &meta, SectionId::Geometry as u32)
            .unwrap()
            .unwrap();
        assert_eq!(geom, vec![0xDE, 0xAD, 0xBE, 0xEF]);

        let pvs = read_section_data(&mut cursor, &meta, SectionId::LeafPvs as u32)
            .unwrap()
            .unwrap();
        assert_eq!(pvs, vec![0xCA, 0xFE]);
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
        let result = read_section_data(&mut cursor, &meta, SectionId::LeafPvs as u32).unwrap();
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

        // Version (u16 LE) at offset 4: value 1 => [0x01, 0x00]
        assert_eq!(buf[4], 0x01);
        assert_eq!(buf[5], 0x00);

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
        buf[4..6].copy_from_slice(&1u16.to_le_bytes());
        buf[6..8].copy_from_slice(&1u16.to_le_bytes()); // 1 section claimed

        let mut cursor = Cursor::new(&buf);
        let err = read_container(&mut cursor).unwrap_err();
        assert!(matches!(err, FormatError::TruncatedSectionTable { .. }));
    }
}
