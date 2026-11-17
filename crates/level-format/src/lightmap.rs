// Lightmap PRL section (ID 22): per-cell static lightmap blocks.
// See: context/lib/build_pipeline.md §PRL section IDs, rendering_pipeline.md §4

use crate::FormatError;
use crate::coded_plane::{self, BlockCodec};

mod record;

pub use record::LIGHTMAP_BLOCK_RECORD_BYTES;

/// Irradiance texel byte width for the uncompressed `Rgba16Float` path.
pub const IRRADIANCE_TEXEL_BYTES: usize = 8;
/// Direction texel byte width: two octahedral `Rg8Unorm` components.
pub const DIRECTION_TEXEL_BYTES: usize = 2;

/// v4: each plane is stored coded ([`BlockCodec`]), its record carrying the
/// codec and both raw lengths. v3 stored per-cell blocks raw; v2 was
/// layer-major whole atlases. `from_bytes` rejects every version but this one.
pub const LIGHTMAP_SECTION_VERSION: u32 = 4;

/// Fixed header: version, block_count, direction_texel_scale,
/// irradiance_format, mode.
pub const LIGHTMAP_HEADER_BYTES: usize = 20;

/// Edge of one runtime pool layer. No block may exceed it on either axis;
/// the compiler rejects such a level and the loader rejects such a record.
/// Why 2048: a pool layer then costs 14 MiB (BC6H, scale 2, shadowmask), the
/// step the pool grows by, and the 4096-wide two-group shadowmask layer stays
/// inside WebGPU's default 8192 texture dimension.
pub const LIGHTMAP_POOL_LAYER_EDGE: u32 = 2048;

/// Minimum array-layer limit requested by the renderer. A level that cannot
/// stream must fit its all-resident lightmap pool within this runtime floor.
/// Internal compiler bake layers are working planes and do not use this limit.
pub const RUNTIME_MAX_TEXTURE_ARRAY_LAYERS: u32 = 256;

/// Most blocks a level may carry. Block id + 1 must fit a vertex's `u16`;
/// 0 means no lightmap. So ids run 0..=65534.
pub const MAX_LIGHTMAP_BLOCKS: u32 = u16::MAX as u32;

/// BC texel block edge. Every block edge is a multiple of it and of the
/// direction texel scale.
const BC_BLOCK_EDGE: u32 = 4;
const BC6H_BLOCK_BYTES: u64 = 16;

/// Format tag for uncompressed `Rgba16Float` irradiance (debug bake and tests).
pub const IRRADIANCE_FORMAT_RGBA16F: u32 = 0;

/// Format tag for `Bc6hRgbUfloat` irradiance: 4×4 blocks, 16 bytes each.
/// RGB only; the shader reads `.rgb`.
pub const IRRADIANCE_FORMAT_BC6H: u32 = 1;

/// Selects how the lightmap was baked.
///
/// - `Shadowed` (default): static-light shadows are folded into irradiance.
/// - `Unshadowed`: full static-light irradiance with no visibility term. The
///   runtime records this mode but does not honour it: no pass multiplies SDF
///   visibility into the static term, so the compiler writes only `Shadowed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LightmapMode {
    #[default]
    Shadowed,
    Unshadowed,
}

impl LightmapMode {
    pub const fn as_u32(self) -> u32 {
        match self {
            LightmapMode::Shadowed => 0,
            LightmapMode::Unshadowed => 1,
        }
    }

    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::Shadowed),
            1 => Some(Self::Unshadowed),
            _ => None,
        }
    }
}

/// A byte range inside a section payload, measured from the payload start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionByteRange {
    pub offset: u64,
    pub len: u32,
}

impl SectionByteRange {
    /// One past the last byte, or `None` on overflow.
    pub fn end(&self) -> Option<u64> {
        self.offset.checked_add(u64::from(self.len))
    }
}

/// Fixed header of a v4 lightmap section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightmapHeader {
    pub block_count: u32,
    /// Irradiance texels per direction texel along each axis: a power of two
    /// dividing [`LIGHTMAP_POOL_LAYER_EDGE`].
    pub direction_texel_scale: u32,
    pub irradiance_format: u32,
    pub mode: LightmapMode,
}

impl LightmapHeader {
    /// Every block edge must be a multiple of this: both the BC block edge
    /// and the direction texel scale.
    pub fn block_alignment(&self) -> u32 {
        lcm(BC_BLOCK_EDGE, self.direction_texel_scale.max(1))
    }

    /// Bytes of header plus records: the prefix a streaming loader reads
    /// before any blob.
    pub fn index_byte_len(&self) -> Option<u64> {
        u64::from(self.block_count)
            .checked_mul(LIGHTMAP_BLOCK_RECORD_BYTES as u64)?
            .checked_add(LIGHTMAP_HEADER_BYTES as u64)
    }

    /// Expected irradiance blob bytes for a `width × height` block.
    pub fn irradiance_len(&self, width: u32, height: u32) -> Option<u64> {
        let (w, h) = (u64::from(width), u64::from(height));
        match self.irradiance_format {
            IRRADIANCE_FORMAT_RGBA16F => {
                w.checked_mul(h)?.checked_mul(IRRADIANCE_TEXEL_BYTES as u64)
            }
            IRRADIANCE_FORMAT_BC6H => w
                .div_ceil(4)
                .checked_mul(h.div_ceil(4))?
                .checked_mul(BC6H_BLOCK_BYTES),
            _ => None,
        }
    }

    /// Direction extent of a `width × height` block.
    pub fn direction_extent(&self, width: u32, height: u32) -> (u32, u32) {
        let scale = self.direction_texel_scale.max(1);
        (width / scale, height / scale)
    }

    /// Expected direction blob bytes for a `width × height` block.
    pub fn direction_len(&self, width: u32, height: u32) -> Option<u64> {
        let (dw, dh) = self.direction_extent(width, height);
        u64::from(dw)
            .checked_mul(u64::from(dh))?
            .checked_mul(DIRECTION_TEXEL_BYTES as u64)
    }

    /// Parse the fixed header. Rejects an older or unknown version, an unknown
    /// irradiance format or mode, a direction scale that is not a power of two
    /// dividing [`LIGHTMAP_POOL_LAYER_EDGE`], and a block count past
    /// [`MAX_LIGHTMAP_BLOCKS`].
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < LIGHTMAP_HEADER_BYTES {
            return Err(eof("lightmap section too short for header"));
        }
        let version = read_u32(data, 0);
        if version != LIGHTMAP_SECTION_VERSION {
            return Err(invalid(format!(
                "unsupported lightmap section version: {version} (expected {LIGHTMAP_SECTION_VERSION}); re-bake the level"
            )));
        }
        let block_count = read_u32(data, 4);
        let direction_texel_scale = read_u32(data, 8);
        let irradiance_format = read_u32(data, 12);
        let raw_mode = read_u32(data, 16);
        if block_count > MAX_LIGHTMAP_BLOCKS {
            return Err(invalid(format!(
                "lightmap block count {block_count} exceeds the vertex id limit {MAX_LIGHTMAP_BLOCKS}"
            )));
        }
        // The pool's direction layer is the pool edge divided by the scale;
        // any other scale leaves the renderer no direction layer to build.
        if !direction_texel_scale.is_power_of_two()
            || direction_texel_scale > LIGHTMAP_POOL_LAYER_EDGE
        {
            return Err(invalid(format!(
                "lightmap direction texel scale {direction_texel_scale} is not a power of two dividing the {LIGHTMAP_POOL_LAYER_EDGE}-texel pool layer"
            )));
        }
        if !matches!(
            irradiance_format,
            IRRADIANCE_FORMAT_RGBA16F | IRRADIANCE_FORMAT_BC6H
        ) {
            return Err(invalid(format!(
                "unsupported lightmap irradiance format: {irradiance_format}"
            )));
        }
        let mode = LightmapMode::from_u32(raw_mode)
            .ok_or_else(|| invalid(format!("unsupported lightmap mode: {raw_mode}")))?;
        Ok(Self {
            block_count,
            direction_texel_scale,
            irradiance_format,
            mode,
        })
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&LIGHTMAP_SECTION_VERSION.to_le_bytes());
        out.extend_from_slice(&self.block_count.to_le_bytes());
        out.extend_from_slice(&self.direction_texel_scale.to_le_bytes());
        out.extend_from_slice(&self.irradiance_format.to_le_bytes());
        out.extend_from_slice(&self.mode.as_u32().to_le_bytes());
    }
}

/// One block's index record. Block id is the record's index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightmapBlockRecord {
    /// Runtime cell some of whose charts this block holds.
    pub cell_id: u32,
    pub width: u16,
    pub height: u16,
    /// The stored irradiance plane.
    pub irradiance: SectionByteRange,
    /// The stored direction plane, starting where the irradiance plane ends.
    pub direction: SectionByteRange,
    /// How both planes are stored.
    pub codec: BlockCodec,
    /// Irradiance then direction bytes once decoded: the lengths the block's
    /// extent and format give.
    pub raw_len: [u32; 2],
}

impl LightmapBlockRecord {
    /// Both planes' bytes once decoded: what the block costs in hand and to
    /// upload.
    pub fn decoded_bytes(&self) -> u64 {
        u64::from(self.raw_len[0]) + u64::from(self.raw_len[1])
    }
}

/// Header plus every block record: what a loaded level keeps CPU-side for
/// its lifetime, and all a streaming loader reads before blobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightmapBlockIndex {
    pub header: LightmapHeader,
    pub records: Vec<LightmapBlockRecord>,
}

impl LightmapBlockIndex {
    /// Parse and validate the index from the section prefix. `prefix` must
    /// hold at least [`LightmapHeader::index_byte_len`] bytes; `section_len`
    /// is the whole section's length, which every blob range must fit inside.
    ///
    /// Rejects: an unknown codec; a record with a zero, misaligned, or
    /// pool-layer-oversize extent; a raw length that disagrees with the
    /// block's extent and format; a raw plane stored at another length, or a
    /// coded plane stored empty; a blob range that overlaps the index or
    /// leaves the section; a direction blob that does not start where its
    /// irradiance blob ends; a block whose blobs start before the previous
    /// block's end; a nonzero reserved field.
    pub fn from_prefix(prefix: &[u8], section_len: u64) -> crate::Result<Self> {
        let header = LightmapHeader::from_bytes(prefix)?;
        let index_len = header
            .index_byte_len()
            .ok_or_else(|| invalid("lightmap index length overflows"))?;
        if (prefix.len() as u64) < index_len {
            return Err(eof(format!(
                "lightmap index truncated: need {index_len} bytes, got {}",
                prefix.len()
            )));
        }
        if section_len < index_len {
            return Err(eof(format!(
                "lightmap section of {section_len} bytes cannot hold its {index_len}-byte index"
            )));
        }
        let align = header.block_alignment();
        let mut records = Vec::with_capacity(header.block_count as usize);
        let mut blobs_end = index_len;
        for block in 0..header.block_count as usize {
            let at = LIGHTMAP_HEADER_BYTES + block * LIGHTMAP_BLOCK_RECORD_BYTES;
            let record = record::read_record(prefix, at, block)?;
            record::validate_record(&header, block, &record, align, index_len, section_len)?;
            blobs_end = check_after_previous_block(
                "lightmap",
                block,
                ("irradiance", record.irradiance),
                record.direction,
                blobs_end,
            )?;
            records.push(record);
        }
        Ok(Self { header, records })
    }

    /// Block extent in texels.
    pub fn extent(&self, block: usize) -> Option<(u32, u32)> {
        self.records
            .get(block)
            .map(|r| (u32::from(r.width), u32::from(r.height)))
    }

    /// Decodes block `block`'s stored planes to its irradiance and direction
    /// bytes, as the pool uploads them. A raw plane moves through uncopied.
    /// Fails by name on a frame that does not decode, checksum verified, to
    /// exactly its declared raw length.
    pub fn decode_block(
        &self,
        block: usize,
        irradiance: Vec<u8>,
        direction: Vec<u8>,
    ) -> crate::Result<(Vec<u8>, Vec<u8>)> {
        let record = self
            .records
            .get(block)
            .ok_or_else(|| invalid(format!("lightmap block {block} is past the index")))?;
        let decode = |what: &str, stored: Vec<u8>, raw_len: u32| {
            coded_plane::decode_plane(record.codec, stored, raw_len as usize).map_err(|reason| {
                invalid(format!(
                    "lightmap block {block} {what} plane failed to decode: {reason}"
                ))
            })
        };
        Ok((
            decode("irradiance", irradiance, record.raw_len[0])?,
            decode("direction", direction, record.raw_len[1])?,
        ))
    }
}

/// Shared by ids 22 and 42: a stored blob lies after the index, inside the
/// section. Its length is the record's to check against its codec.
pub(crate) fn check_blob_range(
    section: &str,
    block: usize,
    what: &str,
    range: SectionByteRange,
    index_len: u64,
    section_len: u64,
) -> crate::Result<()> {
    let end = range
        .end()
        .ok_or_else(|| invalid(format!("{section} block {block} {what} range overflows")))?;
    if range.offset < index_len || end > section_len {
        return Err(invalid(format!(
            "{section} block {block} {what} range {}..{end} lies outside the section blobs {index_len}..{section_len}",
            range.offset
        )));
    }
    Ok(())
}

/// Shared by ids 22 and 42: a block's second blob starts where its first
/// ends. A streamed block then reads each section's half as one range.
pub(crate) fn check_adjacent(
    section: &str,
    block: usize,
    (first_what, first): (&str, SectionByteRange),
    (second_what, second): (&str, SectionByteRange),
) -> crate::Result<()> {
    if first.end() == Some(second.offset) {
        return Ok(());
    }
    Err(invalid(format!(
        "{section} block {block} {second_what} blob at {} does not start where its {first_what} blob ends",
        second.offset
    )))
}

/// Shared by ids 22 and 42: a block's adjacent blob pair starts at or after
/// `previous_end`, where the previous block's pair (or the index) ends, so
/// blobs are disjoint and in record order. The compiler writes them so, and
/// id 42's slot-table bound counts each blob byte once. Returns this pair's
/// end. Callers have already checked both ranges with [`check_blob_range`].
pub(crate) fn check_after_previous_block(
    section: &str,
    block: usize,
    (first_what, first): (&str, SectionByteRange),
    second: SectionByteRange,
    previous_end: u64,
) -> crate::Result<u64> {
    if first.offset < previous_end {
        return Err(invalid(format!(
            "{section} block {block} {first_what} blob at {} starts before the previous block's blobs end at {previous_end}; blobs must be disjoint and in record order",
            first.offset
        )));
    }
    Ok(second.offset + u64::from(second.len))
}

/// One block's baked planes in their stored form: BC6H (or RGBA16F)
/// irradiance and Rg8 direction, each coded by `codec`. Read the texels
/// through [`LightmapSection::decode_block`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightmapBlock {
    pub cell_id: u32,
    pub width: u16,
    pub height: u16,
    pub codec: BlockCodec,
    pub irradiance: Vec<u8>,
    pub direction: Vec<u8>,
}

impl LightmapBlock {
    /// A block whose raw planes are stored under `codec`.
    pub fn coded(
        cell_id: u32,
        width: u16,
        height: u16,
        codec: BlockCodec,
        irradiance: &[u8],
        direction: &[u8],
    ) -> Self {
        Self {
            cell_id,
            width,
            height,
            codec,
            irradiance: coded_plane::encode_plane(codec, irradiance),
            direction: coded_plane::encode_plane(codec, direction),
        }
    }
}

/// A whole v4 lightmap section in memory, planes in their stored form: the
/// compiler's output and its memo. Loaders never build one: they keep a
/// [`LightmapBlockIndex`] and decode each block's planes as they read them.
///
/// On-disk layout (little-endian; offsets from the payload start):
///
/// ```text
///   Header (20 bytes):
///     u32 version                (= 4)
///     u32 block_count            (<= MAX_LIGHTMAP_BLOCKS)
///     u32 direction_texel_scale  (power of two dividing LIGHTMAP_POOL_LAYER_EDGE)
///     u32 irradiance_format      (0 = Rgba16Float, 1 = Bc6hRgbUfloat)
///     u32 mode                   (LightmapMode: 0 = shadowed, 1 = unshadowed)
///   block_count records (48 bytes each), block id = record index:
///     u32 cell_id
///     u16 width, u16 height      (multiples of lcm(4, direction_texel_scale),
///                                 each <= LIGHTMAP_POOL_LAYER_EDGE)
///     u64 irradiance_offset, u32 irradiance_stored_len
///     u64 direction_offset,  u32 direction_stored_len
///     u32 codec                  (0 = stored raw, 1 = one zstd frame per plane)
///     u32 irradiance_raw_len, u32 direction_raw_len  (the extent's lengths;
///                                 direction is Rg8 at extent / scale)
///     u32 reserved (= 0)
///   Blobs, in record order: each block's stored irradiance, then its stored
///   direction, adjacent.
/// ```
///
/// Records and blobs are sorted by owning cluster, then cell id, then
/// sub-block; the compiler owns that order. A cell owns zero or more
/// contiguous blocks (the loader rejects interleaving). Zero blocks is a valid header with no records:
/// a map without static baked light.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightmapSection {
    pub direction_texel_scale: u32,
    pub irradiance_format: u32,
    pub mode: LightmapMode,
    pub blocks: Vec<LightmapBlock>,
}

impl LightmapSection {
    /// A section with no blocks: every vertex carries block 0 ("no lightmap").
    pub fn empty(direction_texel_scale: u32) -> Self {
        Self {
            direction_texel_scale,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
            mode: LightmapMode::Shadowed,
            blocks: Vec::new(),
        }
    }

    pub fn header(&self) -> LightmapHeader {
        LightmapHeader {
            block_count: self.blocks.len() as u32,
            direction_texel_scale: self.direction_texel_scale,
            irradiance_format: self.irradiance_format,
            mode: self.mode,
        }
    }

    /// The index this section serializes with: records carry the blob ranges
    /// `to_bytes` writes.
    pub fn index(&self) -> LightmapBlockIndex {
        let header = self.header();
        let mut cursor = header.index_byte_len().unwrap_or(0);
        let records = self
            .blocks
            .iter()
            .map(|block| {
                let irradiance = SectionByteRange {
                    offset: cursor,
                    len: block.irradiance.len() as u32,
                };
                cursor += block.irradiance.len() as u64;
                let direction = SectionByteRange {
                    offset: cursor,
                    len: block.direction.len() as u32,
                };
                cursor += block.direction.len() as u64;
                let (w, h) = (u32::from(block.width), u32::from(block.height));
                let raw_len = |len: Option<u64>| {
                    len.and_then(|len| u32::try_from(len).ok())
                        .expect("a block's plane fits u32 bytes")
                };
                LightmapBlockRecord {
                    cell_id: block.cell_id,
                    width: block.width,
                    height: block.height,
                    irradiance,
                    direction,
                    codec: block.codec,
                    raw_len: [
                        raw_len(header.irradiance_len(w, h)),
                        raw_len(header.direction_len(w, h)),
                    ],
                }
            })
            .collect();
        LightmapBlockIndex { header, records }
    }

    pub fn byte_len(&self) -> usize {
        LIGHTMAP_HEADER_BYTES
            + self.blocks.len() * LIGHTMAP_BLOCK_RECORD_BYTES
            + self
                .blocks
                .iter()
                .map(|b| b.irradiance.len() + b.direction.len())
                .sum::<usize>()
    }

    /// Header and block records, without copying any encoded block payload.
    pub fn index_bytes(&self) -> Vec<u8> {
        let index = self.index();
        let mut out = Vec::with_capacity(index.header.index_byte_len().unwrap_or(0) as usize);
        index.header.write(&mut out);
        for record in &index.records {
            record::write_record(&mut out, record);
        }
        out
    }

    /// Stream the wire bytes, retaining only the index beside the block blobs.
    pub fn write_to(&self, writer: &mut (impl std::io::Write + ?Sized)) -> std::io::Result<()> {
        writer.write_all(&self.index_bytes())?;
        for block in &self.blocks {
            writer.write_all(&block.irradiance)?;
            writer.write_all(&block.direction)?;
        }
        Ok(())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.index_bytes();
        out.reserve_exact(self.byte_len() - out.len());
        for block in &self.blocks {
            out.extend_from_slice(&block.irradiance);
            out.extend_from_slice(&block.direction);
        }
        out
    }

    /// Parse a whole section, validating the index against its own length.
    /// Planes stay in their stored form.
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        let index = LightmapBlockIndex::from_prefix(data, data.len() as u64)?;
        let blocks = index
            .records
            .iter()
            .map(|record| LightmapBlock {
                cell_id: record.cell_id,
                width: record.width,
                height: record.height,
                codec: record.codec,
                irradiance: slice_range(data, record.irradiance).to_vec(),
                direction: slice_range(data, record.direction).to_vec(),
            })
            .collect();
        Ok(Self {
            direction_texel_scale: index.header.direction_texel_scale,
            irradiance_format: index.header.irradiance_format,
            mode: index.header.mode,
            blocks,
        })
    }

    /// This section with every block's planes decoded and stored raw: the
    /// texels a compiler-side reader or a test inspects byte for byte.
    pub fn decoded(&self) -> crate::Result<Self> {
        let blocks = (0..self.blocks.len())
            .map(|block| {
                let (irradiance, direction) = self.decode_block(block)?;
                Ok(LightmapBlock {
                    codec: BlockCodec::Raw,
                    irradiance,
                    direction,
                    ..self.blocks[block]
                })
            })
            .collect::<crate::Result<_>>()?;
        Ok(Self { blocks, ..*self })
    }

    /// Block `block`'s irradiance and direction bytes, decoded from their
    /// stored form, as the pool uploads them. Fails by name like
    /// [`LightmapBlockIndex::decode_block`].
    pub fn decode_block(&self, block: usize) -> crate::Result<(Vec<u8>, Vec<u8>)> {
        let header = self.header();
        let stored = self
            .blocks
            .get(block)
            .ok_or_else(|| invalid(format!("lightmap block {block} is past the section")))?;
        let (w, h) = (u32::from(stored.width), u32::from(stored.height));
        let decode = |what: &str, plane: &[u8], raw_len: Option<u64>| {
            let raw_len = raw_len.ok_or_else(|| invalid("lightmap plane length overflows"))?;
            coded_plane::decode_plane_from(stored.codec, plane, raw_len as usize).map_err(
                |reason| {
                    invalid(format!(
                        "lightmap block {block} {what} plane failed to decode: {reason}"
                    ))
                },
            )
        };
        Ok((
            decode(
                "irradiance",
                &stored.irradiance,
                header.irradiance_len(w, h),
            )?,
            decode("direction", &stored.direction, header.direction_len(w, h))?,
        ))
    }
}

/// One block's texels as the pool installs them, decoded and split per plane:
/// the lightmap pair plus its shadowmask groups when id 42 is present. The
/// all-resident loader and the streaming decode pool both produce this.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LightmapBlockPayload {
    pub irradiance: Vec<u8>,
    pub direction: Vec<u8>,
    /// Shadowmask group A then group B, BC5 at the block extent. An absent
    /// plane is empty: it takes no placement, no upload and no bytes.
    pub shadowmask: Option<[Vec<u8>; 2]>,
}

/// Caller has validated `range` against `data.len()`.
pub(crate) fn slice_range(data: &[u8], range: SectionByteRange) -> &[u8] {
    let start = range.offset as usize;
    &data[start..start + range.len as usize]
}

fn lcm(a: u32, b: u32) -> u32 {
    fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    a / gcd(a, b) * b
}

pub(crate) fn invalid(msg: impl Into<String>) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        msg.into(),
    ))
}

pub(crate) fn eof(msg: impl Into<String>) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        msg.into(),
    ))
}

pub(crate) fn read_u16(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

pub(crate) fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}

pub(crate) fn read_u64(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().unwrap())
}

/// Round-to-nearest-even f32 → IEEE 754 binary16. Shared with the runtime's
/// SH upload path; kept here as a small dedicated helper so the compiler can
/// write lightmap data without pulling a renderer module in.
pub fn f32_to_f16_bits(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 31) & 0x1) as u16;
    let exp32 = ((bits >> 23) & 0xff) as i32;
    let mant32 = bits & 0x7fffff;

    if exp32 == 0xff {
        let mant16 = if mant32 != 0 { 0x200 } else { 0 };
        return (sign << 15) | (0x1f << 10) | mant16;
    }
    let exp16 = exp32 - 127 + 15;
    if exp16 >= 0x1f {
        return (sign << 15) | (0x1f << 10);
    }
    if exp16 <= 0 {
        if exp16 < -10 {
            return sign << 15;
        }
        let mant = mant32 | 0x800000;
        let shift = 14 - exp16;
        let rounded = mant >> shift;
        let rem = mant & ((1 << shift) - 1);
        let half = 1 << (shift - 1);
        let add = if rem > half || (rem == half && (rounded & 1) != 0) {
            1
        } else {
            0
        };
        return (sign << 15) | ((rounded + add) as u16);
    }
    let mant16 = mant32 >> 13;
    let rem = mant32 & 0x1fff;
    let half = 0x1000;
    let add = if rem > half || (rem == half && (mant16 & 1) != 0) {
        1
    } else {
        0
    };
    let mut mant16 = mant16 + add;
    let mut exp16 = exp16;
    if mant16 >= 0x400 {
        mant16 = 0;
        exp16 += 1;
        if exp16 >= 0x1f {
            return (sign << 15) | (0x1f << 10);
        }
    }
    (sign << 15) | ((exp16 as u16) << 10) | (mant16 as u16)
}

/// Encode a unit direction as two 8-bit octahedral components.
/// Matches the WGSL decoder: `oct * 2 - 1`, recover z via `1 - |x| - |y|`.
pub fn encode_direction_oct(dir: [f32; 3]) -> [u8; 2] {
    let mut d = [dir[0], dir[1], dir[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1.0e-6);
    d[0] /= len;
    d[1] /= len;
    d[2] /= len;

    // Octahedral projection: project onto the L1 sphere, then map to [-1,1]^2.
    let abs_sum = d[0].abs() + d[1].abs() + d[2].abs();
    let inv = if abs_sum > 1.0e-6 { 1.0 / abs_sum } else { 0.0 };
    let mut ox = d[0] * inv;
    let mut oy = d[1] * inv;
    if d[2] < 0.0 {
        let rx = (1.0 - oy.abs()) * signum_nonzero(ox);
        let ry = (1.0 - ox.abs()) * signum_nonzero(oy);
        ox = rx;
        oy = ry;
    }

    // Quantize [-1, 1] → [0, 255] with round-to-nearest.
    let qx = ((ox * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let qy = ((oy * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    [qx, qy]
}

fn signum_nonzero(v: f32) -> f32 {
    if v >= 0.0 { 1.0 } else { -1.0 }
}

#[cfg(test)]
mod tests;
