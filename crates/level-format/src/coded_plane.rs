// Coded planes: one stored plane of a streamed section, raw or one checksummed
// zstd frame, and the codec identity build caches fold.
// See: context/lib/build_pipeline.md §PRL section IDs

use std::cell::RefCell;

/// How a plane's bytes are stored. The wire value is a `u32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BlockCodec {
    /// The plane's bytes, unchanged. Written only by the compiler's
    /// `--section-codec raw` diagnostic; every loader reads it.
    Raw,
    /// One zstd frame per plane at [`ZSTD_LEVEL`], recording its content size
    /// and a checksum.
    #[default]
    Zstd,
}

impl BlockCodec {
    pub const fn as_u32(self) -> u32 {
        match self {
            Self::Raw => 0,
            Self::Zstd => 1,
        }
    }

    pub fn from_u32(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Raw),
            1 => Some(Self::Zstd),
            _ => None,
        }
    }

    /// The compiler flag's spelling.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Zstd => "zstd",
        }
    }
}

/// The zstd level every coded plane is written at. Level 7 decodes as fast as
/// lower levels; higher levels decode slower for little size.
pub const ZSTD_LEVEL: i32 = 7;

/// What a memo holding coded bytes keys on: the codec, the zstd library
/// version and the level. zstd output may change across library versions, so
/// a cached plane is reused only by the build that would write the same bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecIdentity {
    pub codec: BlockCodec,
    pub zstd_version: u32,
    pub level: i32,
}

impl CodecIdentity {
    /// This build's identity for `codec`.
    pub fn current(codec: BlockCodec) -> Self {
        Self {
            codec,
            zstd_version: zstd::zstd_safe::version_number(),
            level: ZSTD_LEVEL,
        }
    }

    /// Folds the identity into a memo key hash, fixed layout: codec, version,
    /// level, each little-endian.
    pub fn fold_into(&self, hasher: &mut blake3::Hasher) {
        hasher.update(&self.codec.as_u32().to_le_bytes());
        hasher.update(&self.zstd_version.to_le_bytes());
        hasher.update(&self.level.to_le_bytes());
    }
}

thread_local! {
    static COMPRESSOR: RefCell<Option<zstd::bulk::Compressor<'static>>> =
        const { RefCell::new(None) };
    static DECOMPRESSOR: RefCell<Option<zstd::bulk::Decompressor<'static>>> =
        const { RefCell::new(None) };
}

/// The stored form of `raw` under `codec`. A zstd frame records its content
/// size and a checksum. An empty plane is absent and stays empty under every
/// codec: it has no frame. Infallible for any input; a compressor failure is
/// an out-of-memory condition and panics.
pub fn encode_plane(codec: BlockCodec, raw: &[u8]) -> Vec<u8> {
    match codec {
        _ if raw.is_empty() => Vec::new(),
        BlockCodec::Raw => raw.to_vec(),
        BlockCodec::Zstd => compress(raw),
    }
}

/// [`encode_plane`] taking the raw plane by value: a raw or absent plane is
/// stored without a copy, a coded one drops its raw bytes once framed.
pub fn store_plane(codec: BlockCodec, raw: Vec<u8>) -> Vec<u8> {
    match codec {
        BlockCodec::Zstd if !raw.is_empty() => compress(&raw),
        _ => raw,
    }
}

/// Most bytes one stored zstd byte can decode to. A zstd block decodes to at
/// most 128 KiB, and the smallest block that does, an RLE block, stores 4
/// bytes: its 3-byte header and the repeated byte.
const ZSTD_MAX_EXPANSION: u64 = 128 * 1024 / 4;

/// The most bytes `stored_len` stored bytes can decode to under `codec`. A
/// reader bounds a declared raw length by it before decoding allocates, so a
/// corrupt length costs at most a fixed multiple of the bytes read.
pub fn max_decoded_len(codec: BlockCodec, stored_len: usize) -> u64 {
    match codec {
        BlockCodec::Raw => stored_len as u64,
        BlockCodec::Zstd => (stored_len as u64).saturating_mul(ZSTD_MAX_EXPANSION),
    }
}

fn compress(raw: &[u8]) -> Vec<u8> {
    COMPRESSOR.with(|cell| {
        let mut slot = cell.borrow_mut();
        let compressor = slot.get_or_insert_with(|| {
            let mut compressor = zstd::bulk::Compressor::new(ZSTD_LEVEL).expect("zstd compressor");
            compressor
                .include_checksum(true)
                .expect("zstd checksum parameter");
            compressor
                .include_contentsize(true)
                .expect("zstd content-size parameter");
            compressor
        });
        let mut stored = compressor.compress(raw).expect("zstd compress");
        stored.shrink_to_fit();
        stored
    })
}

/// zstd frame magic number, little-endian at offset 0.
const ZSTD_MAGIC: u32 = 0xFD2F_B528;
/// Frame header descriptor bit 2: the frame ends in a content checksum.
const ZSTD_CHECKSUM_FLAG: u8 = 1 << 2;

/// Decodes one stored plane to its `raw_len` bytes. An absent plane (empty,
/// raw length 0) stays empty; a raw plane moves through unchanged; a zstd
/// plane must be exactly one frame that records a checksum and a content size
/// of `raw_len`, and must decode, checksum verified, to exactly `raw_len`
/// bytes. The error is the reason alone; callers name the section, block and
/// plane.
pub fn decode_plane(codec: BlockCodec, stored: Vec<u8>, raw_len: usize) -> Result<Vec<u8>, String> {
    match codec {
        _ if stored.is_empty() && raw_len == 0 => Ok(stored),
        BlockCodec::Raw => {
            if stored.len() != raw_len {
                return Err(format!(
                    "raw plane is {} bytes, but its raw length is {raw_len}",
                    stored.len()
                ));
            }
            Ok(stored)
        }
        BlockCodec::Zstd => decode_zstd(&stored, raw_len),
    }
}

/// [`decode_plane`] from a borrowed stored plane: a raw plane is copied.
pub fn decode_plane_from(
    codec: BlockCodec,
    stored: &[u8],
    raw_len: usize,
) -> Result<Vec<u8>, String> {
    match codec {
        BlockCodec::Zstd if !stored.is_empty() || raw_len != 0 => decode_zstd(stored, raw_len),
        _ => decode_plane(codec, stored.to_vec(), raw_len),
    }
}

/// [`decode_plane`] into `out`, whose length is the plane's raw length: a
/// caller that frames the plane inside a larger buffer decodes it in place,
/// with no copy. A raw plane is copied in.
pub fn decode_plane_into(codec: BlockCodec, stored: &[u8], out: &mut [u8]) -> Result<(), String> {
    match codec {
        _ if stored.is_empty() && out.is_empty() => Ok(()),
        BlockCodec::Raw => {
            if stored.len() != out.len() {
                return Err(format!(
                    "raw plane is {} bytes, but its raw length is {}",
                    stored.len(),
                    out.len()
                ));
            }
            out.copy_from_slice(stored);
            Ok(())
        }
        BlockCodec::Zstd => decode_zstd_into(stored, out),
    }
}

/// The checks [`decode_plane`] makes before it decodes, alone: a caller that
/// allocates the decoded plane itself refuses a malformed frame before it
/// allocates.
pub fn check_stored_plane(codec: BlockCodec, stored: &[u8], raw_len: usize) -> Result<(), String> {
    match codec {
        _ if stored.is_empty() && raw_len == 0 => Ok(()),
        BlockCodec::Raw if stored.len() != raw_len => Err(format!(
            "raw plane is {} bytes, but its raw length is {raw_len}",
            stored.len()
        )),
        BlockCodec::Raw => Ok(()),
        BlockCodec::Zstd => check_zstd_frame(stored, raw_len),
    }
}

fn decode_zstd(stored: &[u8], raw_len: usize) -> Result<Vec<u8>, String> {
    check_zstd_frame(stored, raw_len)?;
    let decoded = with_decompressor(|decompressor| {
        decompressor
            .decompress(stored, raw_len)
            .map_err(|error| format!("zstd frame failed to decode: {error}"))
    })?;
    if decoded.len() != raw_len {
        return Err(format!(
            "zstd frame decoded to {} bytes, but the plane's raw length is {raw_len}",
            decoded.len()
        ));
    }
    Ok(decoded)
}

fn decode_zstd_into(stored: &[u8], out: &mut [u8]) -> Result<(), String> {
    let raw_len = out.len();
    check_zstd_frame(stored, raw_len)?;
    let written = with_decompressor(|decompressor| {
        decompressor
            .decompress_to_buffer(stored, out)
            .map_err(|error| format!("zstd frame failed to decode: {error}"))
    })?;
    if written != raw_len {
        return Err(format!(
            "zstd frame decoded to {written} bytes, but the plane's raw length is {raw_len}"
        ));
    }
    Ok(())
}

/// One frame exactly, recording a checksum and a content size of `raw_len`,
/// which its stored bytes can decode to.
fn check_zstd_frame(stored: &[u8], raw_len: usize) -> Result<(), String> {
    if stored.len() < 6 || u32::from_le_bytes(stored[..4].try_into().unwrap()) != ZSTD_MAGIC {
        return Err(format!(
            "zstd plane of {} bytes does not start with a zstd frame",
            stored.len()
        ));
    }
    let most = max_decoded_len(BlockCodec::Zstd, stored.len());
    if raw_len as u64 > most {
        return Err(format!(
            "zstd plane of {} bytes cannot decode to its raw length {raw_len}; at most {most}",
            stored.len()
        ));
    }
    if stored[4] & ZSTD_CHECKSUM_FLAG == 0 {
        return Err("zstd frame records no checksum".into());
    }
    match zstd::zstd_safe::get_frame_content_size(stored) {
        Ok(Some(size)) if size == raw_len as u64 => {}
        Ok(Some(size)) => {
            return Err(format!(
                "zstd frame declares {size} decoded bytes, but the plane's raw length is \
                 {raw_len}"
            ));
        }
        Ok(None) => return Err("zstd frame records no content size".into()),
        Err(_) => return Err("zstd frame header is malformed".into()),
    }
    match zstd::zstd_safe::find_frame_compressed_size(stored) {
        Ok(frame) if frame == stored.len() => Ok(()),
        Ok(frame) => Err(format!(
            "zstd plane is {} bytes, but its frame ends at {frame}",
            stored.len()
        )),
        Err(code) => Err(format!(
            "zstd frame is malformed: {}",
            zstd::zstd_safe::get_error_name(code)
        )),
    }
}

fn with_decompressor<T>(
    decode: impl FnOnce(&mut zstd::bulk::Decompressor<'static>) -> Result<T, String>,
) -> Result<T, String> {
    DECOMPRESSOR.with(|cell| {
        let mut slot = cell.borrow_mut();
        let decompressor = match slot.as_mut() {
            Some(decompressor) => decompressor,
            None => slot.insert(
                zstd::bulk::Decompressor::new()
                    .map_err(|error| format!("zstd decompressor: {error}"))?,
            ),
        };
        decode(decompressor)
    })
}

#[cfg(test)]
#[path = "coded_plane_tests.rs"]
mod tests;
