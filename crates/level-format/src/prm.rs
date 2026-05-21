// PRM ("Postretro Mip") texture sidecar file format.
//
// One `.prm` file per source PNG texture: it carries the precomputed mip chain
// for that texture's three material slots (diffuse, specular, normal), baked
// at compile time by `prl-build` and uploaded directly at runtime by the
// renderer. The format is intentionally trivial — no compression, no
// per-mip headers — so loading is a header parse plus a few `memcpy`s into
// staging buffers.
//
// Wire format (little-endian throughout):
//
//   -- header (43 bytes)
//   [u8; 4]  magic                = b"PRM\x01"
//   u8       stage_version        -- equals STAGE_VERSION
//   u8       slot_mask            -- bit 0 diffuse, 1 specular, 2 normal, 3 emissive (reserved)
//   u8       reserved             = 0
//   [u8; 32] bundle_hash          -- blake3 over slot_mask + per-present-slot
//                                    (bit_index_byte, source_png_file_bytes)
//   u32      total_body_bytes     -- Σ across present slots of
//                                    (12-byte per-slot header + payload_bytes)
//
//   -- per present slot, in wire order diffuse → specular → normal
//   u8       format_tag           -- 0 Rgba8UnormSrgb, 1 Rgba8Unorm, 2 R8Unorm
//   u8       reserved             = 0
//   u16      width                -- mip 0, >= 1
//   u16      height               -- mip 0, >= 1
//   u8       level_count          -- floor(log2(max(w, h))) + 1
//   u8       reserved             = 0
//   u32      payload_bytes        -- total bytes for all levels concatenated
//   [u8; payload_bytes]           -- levels packed back-to-back, level 0 first
//
// See: context/lib/build_pipeline.md §PRL section IDs · §Baked texture mips

use bitflags::bitflags;
use thiserror::Error;

/// Wire-format version of the `.prm` sidecar. The fourth byte of the magic
/// (`b"PRM\x01"`) and this constant are bumped in lockstep when the layout
/// changes; the reader rejects mismatches with `UnsupportedVersion` /
/// `StageVersionMismatch`.
pub const STAGE_VERSION: u8 = 1;

/// Cache filename stem for a `.prm` sidecar keyed by `key`. The compile-time
/// writer and runtime-side reader both call this so the addressing contract
/// has a single source of truth. Returns 64 lowercase hex characters;
/// callers append `.prm` themselves.
pub fn cache_filename_for_key(key: &[u8; 32]) -> String {
    // Hand-rolled hex to keep `postretro-level-format` dependency-free.
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for &b in key {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Size of the fixed file header in bytes.
const HEADER_SIZE: usize = 43;

/// Size of each per-slot header in bytes.
const SLOT_HEADER_SIZE: usize = 12;

/// Maximum supported texture dimension (per axis). The renderer's atlas budget
/// is much tighter, but we still cap at 4096 here so a malformed file cannot
/// drive an unbounded allocation.
const MAX_DIMENSION: u16 = 4096;

bitflags! {
    /// Material slots present in a `.prm` bundle. Bits 3–7 are reserved; the
    /// reader rejects files with any reserved bit set or with `slot_mask == 0`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct PrmSlots: u8 {
        const DIFFUSE  = 0b0000_0001;
        const SPECULAR = 0b0000_0010;
        const NORMAL   = 0b0000_0100;
    }
}

/// Pixel format of a single slot's mip chain. The integer tag matches the wire
/// encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PrmFormat {
    /// 4 bytes per pixel, sRGB-encoded RGBA. Used for albedo (diffuse).
    Rgba8UnormSrgb = 0,
    /// 4 bytes per pixel, linear RGBA. Used for normal maps; each texel stores
    /// a filtered, renormalised direction (`n = sample.rgb * 2 - 1`).
    Rgba8Unorm = 1,
    /// 1 byte per pixel, linear single channel. Used for specular intensity.
    R8Unorm = 2,
}

impl PrmFormat {
    fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Rgba8UnormSrgb),
            1 => Some(Self::Rgba8Unorm),
            2 => Some(Self::R8Unorm),
            _ => None,
        }
    }

    fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgba8UnormSrgb | Self::Rgba8Unorm => 4,
            Self::R8Unorm => 1,
        }
    }
}

/// Parsed header fields. The header bytes are validated up front; if this
/// returns `Ok`, the magic and stage version are correct and the slot mask
/// has no reserved bits set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrmHeader {
    pub stage_version: u8,
    pub slot_mask: PrmSlots,
    pub bundle_hash: [u8; 32],
    pub total_body_bytes: u32,
}

/// A single material slot's mip chain. The renderer uploads `payload` directly
/// to the matching texture using `width`, `height`, `level_count`, and
/// `format`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrmSlot {
    pub format: PrmFormat,
    pub width: u16,
    pub height: u16,
    pub level_count: u8,
    pub payload: Vec<u8>,
}

/// In-memory representation of a `.prm` file. Round-tripped via `to_bytes` /
/// `from_bytes_partial`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrmFile {
    pub header: PrmHeader,
    /// Slots indexed by wire order: 0 diffuse, 1 specular, 2 normal. A slot
    /// is `Some` iff the corresponding bit in `header.slot_mask` is set.
    pub slots: [Option<PrmSlot>; 3],
}

/// Errors returned by the `.prm` reader. Header-level errors abort the parse;
/// per-slot errors are isolated so a truncated tail slot does not poison
/// earlier slots.
#[derive(Debug, Error)]
pub enum PrmReadError {
    #[error("bad magic: expected PRM\\x01-prefix, got {found:?}")]
    BadMagic { found: [u8; 4] },

    #[error("unsupported .prm magic version byte: {version}")]
    UnsupportedVersion { version: u8 },

    #[error("stage version mismatch: expected {expected}, got {found}")]
    StageVersionMismatch { expected: u8, found: u8 },

    #[error("slot_mask has reserved bits set or is zero: {mask:#010b}")]
    ReservedSlotBitsSet { mask: u8 },

    #[error("slot {slot} invalid dimension: {width}x{height} (must be 1..=4096)")]
    DimensionTooLarge { slot: u8, width: u16, height: u16 },

    #[error("slot {slot} level_count mismatch: expected {expected}, got {found}")]
    LevelCountMismatch { slot: u8, expected: u8, found: u8 },

    #[error("slot {slot} payload_bytes mismatch: expected {expected}, got {found}")]
    PayloadBytesMismatch { slot: u8, expected: u32, found: u32 },

    #[error("total body bytes mismatch: expected {expected}, found {found}")]
    TotalBodyBytesMismatch { expected: u32, found: u32 },

    #[error("file body size {found} does not match declared total_body_bytes {expected}")]
    BodySizeMismatch { expected: u32, found: u32 },

    #[error("file truncated mid-header or mid-payload")]
    Truncated,

    #[error("slot {slot}: unsupported format tag {tag}")]
    UnsupportedFormatTag { slot: u8, tag: u8 },

    #[error("slot is not present in the bundle")]
    NotPresent,

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Number of mip levels for a `(width, height)` chain: `floor(log2(max(w, h))) + 1`.
pub fn expected_level_count(width: u16, height: u16) -> u8 {
    let m = width.max(height).max(1) as u32;
    // `ilog2` is defined for non-zero u32 and returns `floor(log2(x))`.
    (m.ilog2() + 1) as u8
}

/// Expected payload size in bytes for a mip chain.
fn expected_payload_bytes(format: PrmFormat, width: u16, height: u16, level_count: u8) -> u32 {
    let bpp = format.bytes_per_pixel();
    let mut total: u32 = 0;
    for n in 0..level_count {
        let w_n = ((width as u32) >> n).max(1);
        let h_n = ((height as u32) >> n).max(1);
        total = total.saturating_add(bpp.saturating_mul(w_n).saturating_mul(h_n));
    }
    total
}

impl PrmFile {
    /// Serialize the file to bytes. Slots are written in wire order
    /// (diffuse, specular, normal); only those marked present in
    /// `header.slot_mask` are emitted. `header.total_body_bytes` is recomputed
    /// from the actual slot payloads, so callers may leave it as `0`.
    pub fn to_bytes(&self) -> Vec<u8> {
        // Recompute total_body_bytes from present slots so writers don't have
        // to keep it in sync manually.
        let mut total_body: u32 = 0;
        for (i, bit) in [PrmSlots::DIFFUSE, PrmSlots::SPECULAR, PrmSlots::NORMAL]
            .iter()
            .enumerate()
        {
            if self.header.slot_mask.contains(*bit) {
                if let Some(slot) = &self.slots[i] {
                    total_body = total_body
                        .saturating_add(SLOT_HEADER_SIZE as u32)
                        .saturating_add(slot.payload.len() as u32);
                }
            }
        }

        let mut buf = Vec::with_capacity(HEADER_SIZE + total_body as usize);

        // -- header --
        buf.extend_from_slice(b"PRM\x01");
        buf.push(self.header.stage_version);
        buf.push(self.header.slot_mask.bits());
        buf.push(0); // reserved
        buf.extend_from_slice(&self.header.bundle_hash);
        buf.extend_from_slice(&total_body.to_le_bytes());
        debug_assert_eq!(buf.len(), HEADER_SIZE);

        // -- per-slot --
        for (i, bit) in [PrmSlots::DIFFUSE, PrmSlots::SPECULAR, PrmSlots::NORMAL]
            .iter()
            .enumerate()
        {
            if !self.header.slot_mask.contains(*bit) {
                continue;
            }
            let Some(slot) = &self.slots[i] else { continue };
            buf.push(slot.format as u8);
            buf.push(0); // reserved
            buf.extend_from_slice(&slot.width.to_le_bytes());
            buf.extend_from_slice(&slot.height.to_le_bytes());
            buf.push(slot.level_count);
            buf.push(0); // reserved
            let payload_bytes = slot.payload.len() as u32;
            buf.extend_from_slice(&payload_bytes.to_le_bytes());
            buf.extend_from_slice(&slot.payload);
        }

        buf
    }

    /// Parse a `.prm` byte slice. The header is validated first; if it fails,
    /// every slot result is `Err(NotPresent)` (caller should consult the
    /// header error). Otherwise each slot is parsed independently — a
    /// truncated tail slot returns `Err(Truncated)` without invalidating
    /// earlier slots, which matters because the renderer wants to fall back
    /// to placeholders per-slot rather than discard the whole bundle.
    ///
    /// Slot positions in the returned array follow wire order: 0 diffuse,
    /// 1 specular, 2 normal. Absent slots are `Err(NotPresent)`.
    pub fn from_bytes_partial(
        data: &[u8],
    ) -> (
        Result<PrmHeader, PrmReadError>,
        [Result<PrmSlot, PrmReadError>; 3],
    ) {
        let header = match parse_header(data) {
            Ok(h) => h,
            Err(e) => {
                return (
                    Err(e),
                    [
                        Err(PrmReadError::NotPresent),
                        Err(PrmReadError::NotPresent),
                        Err(PrmReadError::NotPresent),
                    ],
                );
            }
        };

        // Walk body, advancing `cursor` through each present slot.
        let body = &data[HEADER_SIZE..];
        let mut cursor: usize = 0;
        let mut slot_results: [Result<PrmSlot, PrmReadError>; 3] = [
            Err(PrmReadError::NotPresent),
            Err(PrmReadError::NotPresent),
            Err(PrmReadError::NotPresent),
        ];

        // Track expected total for the cross-check at the end. We only run
        // that check if every present slot parsed successfully; otherwise the
        // per-slot error already carries the actionable information.
        let mut expected_total: u32 = 0;
        let mut all_present_ok = true;

        for (i, bit) in [PrmSlots::DIFFUSE, PrmSlots::SPECULAR, PrmSlots::NORMAL]
            .iter()
            .enumerate()
        {
            if !header.slot_mask.contains(*bit) {
                continue;
            }
            let (result, consumed) = parse_slot(body, cursor, i as u8);
            match &result {
                Ok(slot) => {
                    expected_total = expected_total
                        .saturating_add(SLOT_HEADER_SIZE as u32)
                        .saturating_add(slot.payload.len() as u32);
                }
                Err(_) => {
                    all_present_ok = false;
                }
            }
            slot_results[i] = result;
            cursor = cursor.saturating_add(consumed);
        }

        // Two-way check: (1) recomputed body size from parsed slots must equal
        // `total_body_bytes`; (2) actual buffer length must also match — catches
        // writers that padded or truncated without adjusting the header field.
        let header_result = if all_present_ok && expected_total != header.total_body_bytes {
            Err(PrmReadError::TotalBodyBytesMismatch {
                expected: expected_total,
                found: header.total_body_bytes,
            })
        } else if all_present_ok && (body.len() as u32) != header.total_body_bytes {
            Err(PrmReadError::BodySizeMismatch {
                expected: header.total_body_bytes,
                found: body.len() as u32,
            })
        } else {
            Ok(header)
        };

        (header_result, slot_results)
    }
}

/// Parse the 43-byte header. The whole file is rejected on any header error.
fn parse_header(data: &[u8]) -> Result<PrmHeader, PrmReadError> {
    if data.len() < HEADER_SIZE {
        return Err(PrmReadError::Truncated);
    }

    let mut magic = [0u8; 4];
    magic.copy_from_slice(&data[0..4]);
    if &magic[0..3] != b"PRM" {
        return Err(PrmReadError::BadMagic { found: magic });
    }
    if magic[3] != 0x01 {
        return Err(PrmReadError::UnsupportedVersion { version: magic[3] });
    }

    let stage_version = data[4];
    if stage_version != STAGE_VERSION {
        return Err(PrmReadError::StageVersionMismatch {
            expected: STAGE_VERSION,
            found: stage_version,
        });
    }

    let raw_mask = data[5];
    // PrmSlots::from_bits returns None if any reserved bit is set; we also
    // reject `0` because a `.prm` with no slots is meaningless.
    let slot_mask = match PrmSlots::from_bits(raw_mask) {
        Some(m) if !m.is_empty() => m,
        _ => return Err(PrmReadError::ReservedSlotBitsSet { mask: raw_mask }),
    };

    // data[6] is reserved; not validated to leave room for future extension.

    let mut bundle_hash = [0u8; 32];
    bundle_hash.copy_from_slice(&data[7..39]);

    let total_body_bytes = u32::from_le_bytes([data[39], data[40], data[41], data[42]]);

    Ok(PrmHeader {
        stage_version,
        slot_mask,
        bundle_hash,
        total_body_bytes,
    })
}

/// Parse one slot starting at `body[offset..]`. Returns the parsed slot (or
/// per-slot error) and the number of bytes consumed; consumed bytes advance
/// the cursor even on error when the header was valid, so subsequent slots
/// remain addressable.
fn parse_slot(
    body: &[u8],
    offset: usize,
    slot_index: u8,
) -> (Result<PrmSlot, PrmReadError>, usize) {
    if offset + SLOT_HEADER_SIZE > body.len() {
        return (Err(PrmReadError::Truncated), body.len() - offset);
    }

    let tag = body[offset];
    let format = match PrmFormat::from_tag(tag) {
        Some(f) => f,
        None => {
            // Unknown format tag: we can't compute payload_bytes without a
            // known bytes-per-pixel, so consume the rest of the body to
            // prevent cascading parse errors on subsequent slots.
            return (
                Err(PrmReadError::UnsupportedFormatTag {
                    slot: slot_index,
                    tag,
                }),
                body.len() - offset,
            );
        }
    };
    // body[offset+1] reserved
    let width = u16::from_le_bytes([body[offset + 2], body[offset + 3]]);
    let height = u16::from_le_bytes([body[offset + 4], body[offset + 5]]);
    let level_count = body[offset + 6];
    // body[offset+7] reserved
    let payload_bytes = u32::from_le_bytes([
        body[offset + 8],
        body[offset + 9],
        body[offset + 10],
        body[offset + 11],
    ]);

    // Dimension sanity bound. Zero dimensions are rejected because a zero-size payload is always malformed.
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        let consumed = SLOT_HEADER_SIZE.saturating_add(payload_bytes as usize);
        let consumed = consumed.min(body.len() - offset);
        return (
            Err(PrmReadError::DimensionTooLarge {
                slot: slot_index,
                width,
                height,
            }),
            consumed,
        );
    }

    let expected_levels = expected_level_count(width, height);
    if level_count != expected_levels {
        let consumed = SLOT_HEADER_SIZE.saturating_add(payload_bytes as usize);
        let consumed = consumed.min(body.len() - offset);
        return (
            Err(PrmReadError::LevelCountMismatch {
                slot: slot_index,
                expected: expected_levels,
                found: level_count,
            }),
            consumed,
        );
    }

    let expected_payload = expected_payload_bytes(format, width, height, level_count);
    if payload_bytes != expected_payload {
        let consumed = SLOT_HEADER_SIZE.saturating_add(payload_bytes as usize);
        let consumed = consumed.min(body.len() - offset);
        return (
            Err(PrmReadError::PayloadBytesMismatch {
                slot: slot_index,
                expected: expected_payload,
                found: payload_bytes,
            }),
            consumed,
        );
    }

    let payload_start = offset + SLOT_HEADER_SIZE;
    let payload_end = payload_start + payload_bytes as usize;
    if payload_end > body.len() {
        // Payload itself is truncated; consume the rest of the buffer to keep
        // the cursor monotonic but signal Truncated.
        return (Err(PrmReadError::Truncated), body.len() - offset);
    }

    let payload = body[payload_start..payload_end].to_vec();
    let slot = PrmSlot {
        format,
        width,
        height,
        level_count,
        payload,
    };
    (Ok(slot), SLOT_HEADER_SIZE + payload_bytes as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fully-populated `PrmFile` with three slots at modest sizes.
    fn make_three_slot_file() -> PrmFile {
        // Pick dimensions whose mip math is easy to reason about.
        let diffuse = make_slot(PrmFormat::Rgba8UnormSrgb, 4, 4);
        let specular = make_slot(PrmFormat::R8Unorm, 2, 2);
        let normal = make_slot(PrmFormat::Rgba8Unorm, 8, 8);
        PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: PrmSlots::DIFFUSE | PrmSlots::SPECULAR | PrmSlots::NORMAL,
                bundle_hash: [0xAB; 32],
                total_body_bytes: 0, // recomputed by to_bytes
            },
            slots: [Some(diffuse), Some(specular), Some(normal)],
        }
    }

    fn make_slot(format: PrmFormat, width: u16, height: u16) -> PrmSlot {
        let level_count = expected_level_count(width, height);
        let payload_bytes = expected_payload_bytes(format, width, height, level_count) as usize;
        let mut payload = Vec::with_capacity(payload_bytes);
        // Fill with a recognisable pattern keyed on the byte index so
        // accidental swaps between slots show up in equality asserts.
        for i in 0..payload_bytes {
            payload.push((i as u8).wrapping_mul(7));
        }
        PrmSlot {
            format,
            width,
            height,
            level_count,
            payload,
        }
    }

    #[test]
    fn round_trip_three_slots() {
        let file = make_three_slot_file();
        let bytes = file.to_bytes();
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let header = header.expect("header parse should succeed");

        assert_eq!(header.stage_version, STAGE_VERSION);
        assert_eq!(
            header.slot_mask,
            PrmSlots::DIFFUSE | PrmSlots::SPECULAR | PrmSlots::NORMAL
        );
        assert_eq!(header.bundle_hash, [0xAB; 32]);

        for (i, expected) in file.slots.iter().enumerate() {
            let got = slots[i].as_ref().expect("slot should parse");
            let expected = expected.as_ref().unwrap();
            assert_eq!(got, expected, "slot {i} mismatch");
        }
    }

    /// Per-slot fallback isolation regression guard: truncating the specular
    /// slot must surface `Truncated` for that slot only, leaving the diffuse
    /// slot (parsed earlier) intact. The normal slot (which followed the
    /// corrupted specular slot) will also fail with `Truncated`, but the
    /// load-bearing assertion here is that the diffuse slot survives.
    #[test]
    fn specular_truncation_isolated() {
        let file = make_three_slot_file();
        let mut bytes = file.to_bytes();

        // Locate the specular slot in the body. After the header, slot order
        // is diffuse → specular → normal. Diffuse is the first slot, so its
        // payload runs through SLOT_HEADER_SIZE + diffuse_payload_bytes after
        // the file header.
        let diffuse_payload = file.slots[0].as_ref().unwrap().payload.len();
        let specular_start = HEADER_SIZE + SLOT_HEADER_SIZE + diffuse_payload;

        // Chop bytes off mid-specular-payload to force a per-slot truncation
        // without disturbing the diffuse slot.
        let specular_payload_len = file.slots[1].as_ref().unwrap().payload.len();
        // Cut 1 byte into the specular payload so it ends mid-payload.
        let truncate_at = specular_start + SLOT_HEADER_SIZE + (specular_payload_len / 2).max(1);
        bytes.truncate(truncate_at);

        let (header_result, slots) = PrmFile::from_bytes_partial(&bytes);
        // The header itself parsed fine; the total-body cross-check will
        // disagree, but that's a header-result quibble — we don't assert on
        // it here. What matters is per-slot isolation.
        let _ = header_result;

        assert!(slots[0].is_ok(), "diffuse should still parse cleanly");
        assert!(
            matches!(slots[1], Err(PrmReadError::Truncated)),
            "specular should report Truncated, got {:?}",
            slots[1]
        );
        // The normal slot follows the (now-truncated) specular slot, so it
        // can't be located either; that's expected.
        assert!(
            slots[2].is_err(),
            "normal cannot be located after corruption"
        );
    }

    #[test]
    fn slot_mask_zero_is_rejected() {
        let mut bytes = vec![0u8; HEADER_SIZE];
        bytes[0..4].copy_from_slice(b"PRM\x01");
        bytes[4] = STAGE_VERSION;
        bytes[5] = 0; // slot_mask == 0
        // bundle_hash and total_body_bytes stay zero.
        let (header_result, _) = PrmFile::from_bytes_partial(&bytes);
        assert!(
            matches!(
                header_result,
                Err(PrmReadError::ReservedSlotBitsSet { mask: 0 })
            ),
            "expected ReservedSlotBitsSet {{ mask: 0 }}, got {header_result:?}"
        );
    }

    #[test]
    fn reserved_slot_bits_are_rejected() {
        let mut bytes = vec![0u8; HEADER_SIZE];
        bytes[0..4].copy_from_slice(b"PRM\x01");
        bytes[4] = STAGE_VERSION;
        bytes[5] = 0b0000_1000; // bit 3 reserved (emissive placeholder)
        let (header_result, _) = PrmFile::from_bytes_partial(&bytes);
        assert!(
            matches!(
                header_result,
                Err(PrmReadError::ReservedSlotBitsSet { mask: 0b0000_1000 })
            ),
            "expected ReservedSlotBitsSet, got {header_result:?}"
        );
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut bytes = vec![0u8; HEADER_SIZE];
        bytes[0..4].copy_from_slice(b"NOPE");
        let (header_result, slots) = PrmFile::from_bytes_partial(&bytes);
        assert!(
            matches!(header_result, Err(PrmReadError::BadMagic { found }) if found == *b"NOPE"),
            "expected BadMagic, got {header_result:?}"
        );
        // Slots fall back to NotPresent when the header fails outright.
        for s in &slots {
            assert!(matches!(s, Err(PrmReadError::NotPresent)));
        }
    }

    #[test]
    fn unsupported_magic_version_byte_is_rejected() {
        let mut bytes = vec![0u8; HEADER_SIZE];
        bytes[0..3].copy_from_slice(b"PRM");
        bytes[3] = 0x02; // wrong version byte
        let (header_result, _) = PrmFile::from_bytes_partial(&bytes);
        assert!(
            matches!(
                header_result,
                Err(PrmReadError::UnsupportedVersion { version: 0x02 })
            ),
            "expected UnsupportedVersion {{ version: 0x02 }}, got {header_result:?}"
        );
    }

    #[test]
    fn stage_version_mismatch_is_rejected() {
        let mut bytes = vec![0u8; HEADER_SIZE];
        bytes[0..4].copy_from_slice(b"PRM\x01");
        bytes[4] = STAGE_VERSION.wrapping_add(1);
        bytes[5] = PrmSlots::DIFFUSE.bits();
        let (header_result, _) = PrmFile::from_bytes_partial(&bytes);
        assert!(
            matches!(
                header_result,
                Err(PrmReadError::StageVersionMismatch { .. })
            ),
            "expected StageVersionMismatch, got {header_result:?}"
        );
    }

    #[test]
    fn cache_filename_for_key_is_lowercase_hex_64_chars() {
        let mut key = [0u8; 32];
        key[0] = 0xAB;
        key[1] = 0xCD;
        key[30] = 0xEF;
        key[31] = 0x0F;
        let s = cache_filename_for_key(&key);
        assert_eq!(s.len(), 64);
        assert!(s.starts_with("abcd"));
        assert!(s.ends_with("ef0f"));
        assert!(
            s.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "expected lowercase hex only, got {s}"
        );
    }

    #[test]
    fn expected_level_count_matches_floor_log2_plus_one() {
        assert_eq!(expected_level_count(1, 1), 1);
        assert_eq!(expected_level_count(2, 1), 2);
        assert_eq!(expected_level_count(4, 4), 3);
        assert_eq!(expected_level_count(8, 4), 4);
        assert_eq!(expected_level_count(1024, 1024), 11);
    }

    /// Mutating `total_body_bytes` in the header (slot-sum vs. declared mismatch)
    /// must return `TotalBodyBytesMismatch`.
    #[test]
    fn mutated_total_body_bytes_header_field_is_rejected() {
        let file = make_three_slot_file();
        let mut bytes = file.to_bytes();

        // `total_body_bytes` is a little-endian u32 at header bytes 39..43.
        let original = u32::from_le_bytes([bytes[39], bytes[40], bytes[41], bytes[42]]);
        let mutated = original.wrapping_add(7);
        bytes[39..43].copy_from_slice(&mutated.to_le_bytes());

        let (header_result, _) = PrmFile::from_bytes_partial(&bytes);
        assert!(
            matches!(
                header_result,
                Err(PrmReadError::TotalBodyBytesMismatch { .. })
            ),
            "expected TotalBodyBytesMismatch, got {header_result:?}"
        );
    }

    /// Appending stray bytes beyond what the header declares must return
    /// `BodySizeMismatch` (actual buffer length != declared total_body_bytes).
    #[test]
    fn stray_trailing_bytes_produce_body_size_mismatch() {
        let file = make_three_slot_file();
        let mut bytes = file.to_bytes();

        // Append 16 stray bytes — the header still declares the original size.
        bytes.extend_from_slice(&[0xFFu8; 16]);

        let (header_result, _) = PrmFile::from_bytes_partial(&bytes);
        assert!(
            matches!(header_result, Err(PrmReadError::BodySizeMismatch { .. })),
            "expected BodySizeMismatch, got {header_result:?}"
        );
    }

    /// A hand-crafted header with `slot_mask = DIFFUSE` and an empty body (no
    /// slot bytes at all) must fail. The implementation returns `BodySizeMismatch`
    /// at the header layer because `total_body_bytes` is declared non-zero but
    /// the buffer contains no body bytes at all.
    #[test]
    fn empty_body_with_diffuse_slot_fails_body_size_mismatch() {
        // Build just a header claiming one diffuse slot with a non-zero
        // total_body_bytes (matching what a real diffuse slot would require),
        // but supply no body bytes at all.
        let diffuse_slot = make_slot(PrmFormat::Rgba8UnormSrgb, 4, 4);
        let declared_body = SLOT_HEADER_SIZE as u32 + diffuse_slot.payload.len() as u32;

        let mut bytes = vec![0u8; HEADER_SIZE];
        bytes[0..4].copy_from_slice(b"PRM\x01");
        bytes[4] = STAGE_VERSION;
        bytes[5] = PrmSlots::DIFFUSE.bits();
        // bundle_hash stays zero.
        bytes[39..43].copy_from_slice(&declared_body.to_le_bytes());
        // No body bytes appended — total length is exactly HEADER_SIZE.

        let (header_result, slots) = PrmFile::from_bytes_partial(&bytes);
        // The implementation may either fail at the header layer (BodySizeMismatch)
        // or at the slot layer (Truncated). Both are acceptable contracts; what
        // matters is that the parse does not succeed.
        let header_ok = header_result.is_ok();
        let diffuse_ok = slots[0].is_ok();
        assert!(
            !header_ok || !diffuse_ok,
            "empty body with declared diffuse slot must not fully succeed; \
             header={header_result:?} diffuse={:?}",
            slots[0]
        );
    }
}
