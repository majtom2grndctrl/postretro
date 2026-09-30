// Lightmap PRL section (ID 22): per-cell static lightmap blocks.
// See: context/lib/build_pipeline.md §PRL section IDs, rendering_pipeline.md §4

use crate::FormatError;

/// Irradiance texel byte width for the uncompressed `Rgba16Float` path.
pub const IRRADIANCE_TEXEL_BYTES: usize = 8;
/// Direction texel byte width: two octahedral `Rg8Unorm` components.
pub const DIRECTION_TEXEL_BYTES: usize = 2;

/// v3: per-cell blocks. v2 was layer-major whole atlases; `from_bytes` rejects
/// every version but this one.
pub const LIGHTMAP_SECTION_VERSION: u32 = 3;

/// Fixed header: version, block_count, direction_texel_scale,
/// irradiance_format, mode.
pub const LIGHTMAP_HEADER_BYTES: usize = 20;

/// One block record: cell_id u32, width u16, height u16, irradiance_offset
/// u64, irradiance_len u32, direction_offset u64, direction_len u32,
/// reserved u32.
pub const LIGHTMAP_BLOCK_RECORD_BYTES: usize = 36;

/// Edge of one runtime pool layer. No block may exceed it on either axis;
/// the compiler rejects such a level and the loader rejects such a record.
/// Why 2048: a pool layer then costs 14 MiB (BC6H, scale 2, shadowmask), the
/// step the pool grows by, and the 4096-wide two-group shadowmask layer stays
/// inside WebGPU's default 8192 texture dimension.
pub const LIGHTMAP_POOL_LAYER_EDGE: u32 = 2048;

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

/// Fixed header of a v3 lightmap section.
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
    /// Runtime cell whose charts this block holds.
    pub cell_id: u32,
    pub width: u16,
    pub height: u16,
    pub irradiance: SectionByteRange,
    pub direction: SectionByteRange,
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
    /// Rejects: a record with a zero, misaligned, or pool-layer-oversize
    /// extent; a blob length that disagrees with the block's extent and
    /// format; a blob range that overlaps the index or leaves the section; a
    /// direction blob that does not start where its irradiance blob ends; a
    /// block whose blobs start before the previous block's end; a nonzero
    /// reserved field.
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
            let record = LightmapBlockRecord {
                cell_id: read_u32(prefix, at),
                width: read_u16(prefix, at + 4),
                height: read_u16(prefix, at + 6),
                irradiance: SectionByteRange {
                    offset: read_u64(prefix, at + 8),
                    len: read_u32(prefix, at + 16),
                },
                direction: SectionByteRange {
                    offset: read_u64(prefix, at + 20),
                    len: read_u32(prefix, at + 28),
                },
            };
            let reserved = read_u32(prefix, at + 32);
            if reserved != 0 {
                return Err(invalid(format!(
                    "lightmap block {block} has nonzero reserved field {reserved:#x}"
                )));
            }
            validate_record(&header, block, &record, align, index_len, section_len)?;
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
}

fn validate_record(
    header: &LightmapHeader,
    block: usize,
    record: &LightmapBlockRecord,
    align: u32,
    index_len: u64,
    section_len: u64,
) -> crate::Result<()> {
    let (w, h) = (u32::from(record.width), u32::from(record.height));
    if w == 0 || h == 0 {
        return Err(invalid(format!(
            "lightmap block {block} has zero extent {w}x{h}"
        )));
    }
    if w > LIGHTMAP_POOL_LAYER_EDGE || h > LIGHTMAP_POOL_LAYER_EDGE {
        return Err(invalid(format!(
            "lightmap block {block} extent {w}x{h} exceeds the {LIGHTMAP_POOL_LAYER_EDGE}² pool layer"
        )));
    }
    if w % align != 0 || h % align != 0 {
        return Err(invalid(format!(
            "lightmap block {block} extent {w}x{h} is not a multiple of {align}"
        )));
    }
    let expected_irr = header
        .irradiance_len(w, h)
        .ok_or_else(|| invalid("lightmap irradiance length overflows"))?;
    let expected_dir = header
        .direction_len(w, h)
        .ok_or_else(|| invalid("lightmap direction length overflows"))?;
    check_blob(
        "lightmap",
        block,
        "irradiance",
        record.irradiance,
        expected_irr,
        index_len,
        section_len,
    )?;
    check_blob(
        "lightmap",
        block,
        "direction",
        record.direction,
        expected_dir,
        index_len,
        section_len,
    )?;
    check_adjacent(
        "lightmap",
        block,
        ("irradiance", record.irradiance),
        ("direction", record.direction),
    )
}

/// Shared by ids 22 and 42: a blob has its exact expected length and lies
/// after the index, inside the section.
pub(crate) fn check_blob(
    section: &str,
    block: usize,
    what: &str,
    range: SectionByteRange,
    expected_len: u64,
    index_len: u64,
    section_len: u64,
) -> crate::Result<()> {
    if u64::from(range.len) != expected_len {
        return Err(invalid(format!(
            "{section} block {block} {what} blob is {} bytes, expected {expected_len}",
            range.len
        )));
    }
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
/// end. Callers have already checked both ranges with [`check_blob`].
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

/// One block's baked texels in their stored formats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightmapBlock {
    pub cell_id: u32,
    pub width: u16,
    pub height: u16,
    pub irradiance: Vec<u8>,
    pub direction: Vec<u8>,
}

/// A whole v3 lightmap section in memory: the compiler's output, and the
/// all-resident loader's input. A streaming loader never builds one; it keeps
/// a [`LightmapBlockIndex`] and reads blob ranges.
///
/// On-disk layout (little-endian; offsets from the payload start):
///
/// ```text
///   Header (20 bytes):
///     u32 version                (= 3)
///     u32 block_count            (<= MAX_LIGHTMAP_BLOCKS)
///     u32 direction_texel_scale  (power of two dividing LIGHTMAP_POOL_LAYER_EDGE)
///     u32 irradiance_format      (0 = Rgba16Float, 1 = Bc6hRgbUfloat)
///     u32 mode                   (LightmapMode: 0 = shadowed, 1 = unshadowed)
///   block_count records (36 bytes each), block id = record index:
///     u32 cell_id
///     u16 width, u16 height      (multiples of lcm(4, direction_texel_scale),
///                                 each <= LIGHTMAP_POOL_LAYER_EDGE)
///     u64 irradiance_offset, u32 irradiance_len
///     u64 direction_offset,  u32 direction_len   (Rg8, extent / scale)
///     u32 reserved (= 0)
///   Blobs, in record order: each block's irradiance, then its direction,
///   adjacent.
/// ```
///
/// Records and blobs are sorted by owning cluster, then cell id; the
/// compiler owns that order. Zero blocks is a valid header with no records:
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
                LightmapBlockRecord {
                    cell_id: block.cell_id,
                    width: block.width,
                    height: block.height,
                    irradiance,
                    direction,
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

    pub fn to_bytes(&self) -> Vec<u8> {
        let index = self.index();
        let mut out = Vec::with_capacity(self.byte_len());
        index.header.write(&mut out);
        for record in &index.records {
            out.extend_from_slice(&record.cell_id.to_le_bytes());
            out.extend_from_slice(&record.width.to_le_bytes());
            out.extend_from_slice(&record.height.to_le_bytes());
            out.extend_from_slice(&record.irradiance.offset.to_le_bytes());
            out.extend_from_slice(&record.irradiance.len.to_le_bytes());
            out.extend_from_slice(&record.direction.offset.to_le_bytes());
            out.extend_from_slice(&record.direction.len.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        for block in &self.blocks {
            out.extend_from_slice(&block.irradiance);
            out.extend_from_slice(&block.direction);
        }
        out
    }

    /// Parse a whole section, validating the index against its own length.
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        let index = LightmapBlockIndex::from_prefix(data, data.len() as u64)?;
        let blocks = index
            .records
            .iter()
            .map(|record| LightmapBlock {
                cell_id: record.cell_id,
                width: record.width,
                height: record.height,
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
}

/// One block's texels as the pool installs them: the lightmap pair plus its
/// shadowmask groups when id 42 is present. The all-resident loader and the
/// streaming issuer both produce this.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LightmapBlockPayload {
    pub irradiance: Vec<u8>,
    pub direction: Vec<u8>,
    /// Shadowmask group A then group B, BC5 at the block extent.
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
mod tests {
    use super::*;

    fn block(cell_id: u32, width: u16, height: u16, seed: u8, scale: u32) -> LightmapBlock {
        let header = LightmapHeader {
            block_count: 0,
            direction_texel_scale: scale,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
            mode: LightmapMode::Shadowed,
        };
        let (w, h) = (u32::from(width), u32::from(height));
        let irr_len = header.irradiance_len(w, h).unwrap() as usize;
        let dir_len = header.direction_len(w, h).unwrap() as usize;
        LightmapBlock {
            cell_id,
            width,
            height,
            irradiance: (0..irr_len).map(|i| seed.wrapping_add(i as u8)).collect(),
            direction: (0..dir_len).map(|i| seed ^ (i as u8)).collect(),
        }
    }

    fn two_block_section() -> LightmapSection {
        LightmapSection {
            direction_texel_scale: 2,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
            mode: LightmapMode::Unshadowed,
            blocks: vec![block(7, 8, 4, 3, 2), block(2, 4, 12, 9, 2)],
        }
    }

    fn error_message(result: crate::Result<impl std::fmt::Debug>) -> String {
        format!("{:?}", result.expect_err("expected a rejection"))
    }

    /// Byte offset of block `block`'s record field at `field` within it.
    fn record_at(block: usize, field: usize) -> usize {
        LIGHTMAP_HEADER_BYTES + block * LIGHTMAP_BLOCK_RECORD_BYTES + field
    }

    #[test]
    fn lightmap_blocks_round_trip_with_header_mode_and_scale() {
        let section = two_block_section();
        let bytes = section.to_bytes();
        assert_eq!(section.byte_len(), bytes.len());
        assert_eq!(LightmapSection::from_bytes(&bytes).unwrap(), section);
    }

    #[test]
    fn zero_block_section_is_a_bare_header() {
        let section = LightmapSection::empty(2);
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), LIGHTMAP_HEADER_BYTES);
        let index = LightmapBlockIndex::from_prefix(&bytes, bytes.len() as u64).unwrap();
        assert!(index.records.is_empty());
        assert_eq!(LightmapSection::from_bytes(&bytes).unwrap(), section);
    }

    #[test]
    fn index_parses_from_the_prefix_alone() {
        let section = two_block_section();
        let bytes = section.to_bytes();
        let index_len = section.header().index_byte_len().unwrap() as usize;
        let index = LightmapBlockIndex::from_prefix(&bytes[..index_len], bytes.len() as u64)
            .expect("the index needs no blob bytes");
        assert_eq!(index, section.index());
        // Blobs follow the index in record order: irradiance, then direction.
        let first = index.records[0];
        assert_eq!(first.irradiance.offset, index_len as u64);
        assert_eq!(
            first.direction.offset,
            first.irradiance.offset + u64::from(first.irradiance.len)
        );
        assert_eq!(index.extent(1), Some((4, 12)));
    }

    #[test]
    fn block_alignment_is_lcm_of_bc_edge_and_direction_scale() {
        let mut header = two_block_section().header();
        for (scale, align) in [(1, 4), (2, 4), (4, 4), (8, 8), (3, 12)] {
            header.direction_texel_scale = scale;
            assert_eq!(header.block_alignment(), align, "scale {scale}");
        }
    }

    #[test]
    fn rejects_older_section_versions_with_a_rebake_hint() {
        let mut bytes = two_block_section().to_bytes();
        bytes[0..4].copy_from_slice(&2u32.to_le_bytes());
        let message = error_message(LightmapSection::from_bytes(&bytes));
        assert!(message.contains("version: 2"), "{message}");
        assert!(message.contains("re-bake"), "{message}");
    }

    #[test]
    fn rejects_nonzero_reserved_field() {
        let mut bytes = two_block_section().to_bytes();
        let at = record_at(1, 32);
        bytes[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
        let message = error_message(LightmapSection::from_bytes(&bytes));
        assert!(message.contains("reserved"), "{message}");
    }

    #[test]
    fn rejects_blob_range_outside_the_section() {
        let mut bytes = two_block_section().to_bytes();
        let at = record_at(1, 20);
        let past_end = bytes.len() as u64;
        bytes[at..at + 8].copy_from_slice(&past_end.to_le_bytes());
        let message = error_message(LightmapSection::from_bytes(&bytes));
        assert!(message.contains("outside the section"), "{message}");
    }

    #[test]
    fn rejects_blob_range_overlapping_the_index() {
        let mut bytes = two_block_section().to_bytes();
        let at = record_at(0, 8);
        bytes[at..at + 8].copy_from_slice(&0u64.to_le_bytes());
        let message = error_message(LightmapSection::from_bytes(&bytes));
        assert!(message.contains("outside the section"), "{message}");
    }

    #[test]
    fn rejects_block_larger_than_a_pool_layer() {
        let edge = LIGHTMAP_POOL_LAYER_EDGE as u16 + 4;
        let section = LightmapSection {
            blocks: vec![block(0, edge, 4, 1, 2)],
            ..two_block_section()
        };
        let message = error_message(LightmapSection::from_bytes(&section.to_bytes()));
        assert!(message.contains("pool layer"), "{message}");
    }

    #[test]
    fn accepts_a_block_exactly_one_pool_layer() {
        let edge = LIGHTMAP_POOL_LAYER_EDGE as u16;
        let section = LightmapSection {
            blocks: vec![block(0, edge, 4, 1, 2)],
            ..two_block_section()
        };
        assert!(LightmapSection::from_bytes(&section.to_bytes()).is_ok());
    }

    #[test]
    fn rejects_misaligned_extent() {
        let mut bytes = two_block_section().to_bytes();
        let at = record_at(0, 4);
        bytes[at..at + 2].copy_from_slice(&6u16.to_le_bytes());
        let message = error_message(LightmapSection::from_bytes(&bytes));
        assert!(message.contains("not a multiple of 4"), "{message}");
    }

    #[test]
    fn rejects_blob_length_that_disagrees_with_extent() {
        let mut section = two_block_section();
        section.blocks[1].direction.pop();
        let message = error_message(LightmapSection::from_bytes(&section.to_bytes()));
        assert!(message.contains("direction blob"), "{message}");
    }

    #[test]
    fn rejects_block_count_past_the_vertex_id_limit() {
        let mut bytes = LightmapSection::empty(2).to_bytes();
        bytes[4..8].copy_from_slice(&(MAX_LIGHTMAP_BLOCKS + 1).to_le_bytes());
        let message = error_message(LightmapHeader::from_bytes(&bytes));
        assert!(message.contains("vertex id limit"), "{message}");
        bytes[4..8].copy_from_slice(&MAX_LIGHTMAP_BLOCKS.to_le_bytes());
        assert!(LightmapHeader::from_bytes(&bytes).is_ok());
    }

    #[test]
    fn block_limit_is_u16_max_so_the_last_block_id_plus_one_fits_a_vertex() {
        assert_eq!(MAX_LIGHTMAP_BLOCKS, 65_535);
        let last_block_id = MAX_LIGHTMAP_BLOCKS - 1;
        assert_eq!(u16::try_from(last_block_id + 1), Ok(u16::MAX));
    }

    #[test]
    fn rejects_direction_scale_that_is_not_a_power_of_two_dividing_the_pool_layer() {
        for scale in [3u32, 6, 12, LIGHTMAP_POOL_LAYER_EDGE * 2] {
            let mut bytes = LightmapSection::empty(2).to_bytes();
            bytes[8..12].copy_from_slice(&scale.to_le_bytes());
            let message = error_message(LightmapHeader::from_bytes(&bytes));
            assert!(
                message.contains("power of two dividing"),
                "{scale}: {message}"
            );
        }
        for scale in [1u32, 2, 8, LIGHTMAP_POOL_LAYER_EDGE] {
            let mut bytes = LightmapSection::empty(2).to_bytes();
            bytes[8..12].copy_from_slice(&scale.to_le_bytes());
            assert!(LightmapHeader::from_bytes(&bytes).is_ok(), "{scale}");
        }
    }

    #[test]
    fn rejects_direction_blob_that_does_not_start_where_its_irradiance_ends() {
        let mut bytes = two_block_section().to_bytes();
        // Block 0's direction moved one byte on: still inside the section
        // and the right length, but no longer adjacent to its irradiance.
        let at = record_at(0, 20);
        let offset = read_u64(&bytes, at) + 1;
        bytes[at..at + 8].copy_from_slice(&offset.to_le_bytes());
        let message = error_message(LightmapSection::from_bytes(&bytes));
        let expected = format!(
            "lightmap block 0 direction blob at {offset} does not start where its irradiance blob ends"
        );
        assert!(message.contains(&expected), "{message}");
    }

    #[test]
    fn rejects_blobs_that_overlap_or_leave_record_order() {
        let section = two_block_section();
        let index = section.index();
        let (first, second) = (index.records[0], index.records[1]);
        let first_len = u64::from(first.irradiance.len + first.direction.len);
        let second_len = u64::from(second.irradiance.len + second.direction.len);
        let start = first.irradiance.offset;
        let place = |bytes: &mut Vec<u8>, block: usize, record: &LightmapBlockRecord, at: u64| {
            let irradiance = record_at(block, 8);
            bytes[irradiance..irradiance + 8].copy_from_slice(&at.to_le_bytes());
            let direction = record_at(block, 20);
            let direction_at = at + u64::from(record.irradiance.len);
            bytes[direction..direction + 8].copy_from_slice(&direction_at.to_le_bytes());
        };

        // Block 1's pair laid over block 0's: each pair adjacent and inside
        // the section, but the two share bytes.
        let mut overlapping = section.to_bytes();
        place(&mut overlapping, 1, &second, start);
        let message = error_message(LightmapSection::from_bytes(&overlapping));
        let expected = format!(
            "lightmap block 1 irradiance blob at {start} starts before the previous block's blobs end at {}",
            start + first_len
        );
        assert!(message.contains(&expected), "{message}");

        // The same bytes with the two pairs swapped: disjoint, but block 1's
        // blobs come first.
        let mut reversed = section.to_bytes();
        place(&mut reversed, 0, &first, start + second_len);
        place(&mut reversed, 1, &second, start);
        let message = error_message(LightmapSection::from_bytes(&reversed));
        assert!(
            message.contains("disjoint and in record order"),
            "{message}"
        );
    }

    #[test]
    fn rejects_unknown_mode_format_and_zero_scale() {
        for (at, value, expect) in [
            (16, 9u32, "mode"),
            (12, 9u32, "irradiance format"),
            (8, 0u32, "scale"),
        ] {
            let mut bytes = LightmapSection::empty(2).to_bytes();
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            let message = error_message(LightmapSection::from_bytes(&bytes));
            assert!(message.contains(expect), "{expect}: {message}");
        }
    }

    #[test]
    fn rejects_truncated_index() {
        let bytes = two_block_section().to_bytes();
        let index_len = LIGHTMAP_HEADER_BYTES + 2 * LIGHTMAP_BLOCK_RECORD_BYTES;
        assert!(
            LightmapBlockIndex::from_prefix(&bytes[..index_len - 1], bytes.len() as u64).is_err()
        );
    }

    #[test]
    fn encode_direction_axis_round_trip() {
        let enc = encode_direction_oct([0.0, 1.0, 0.0]);
        assert_eq!(enc[0], 128);
        assert_eq!(enc[1], 255);
    }

    #[test]
    fn f32_to_f16_known_values() {
        assert_eq!(f32_to_f16_bits(0.0), 0x0000);
        assert_eq!(f32_to_f16_bits(1.0), 0x3c00);
        assert_eq!(f32_to_f16_bits(-1.0), 0xbc00);
    }
}
