// Bakes per-texture mip pyramids into `.prm` sidecar files.
// See: context/lib/build_pipeline.md §Baked texture mips

use std::collections::{HashMap, HashSet};
use std::path::Path;

use postretro_level_format::prm::{
    PORTABLE_MAX_TEXTURE_ARRAY_LAYERS, PrmFile, PrmFormat, PrmHeader, PrmSlot, PrmSlots,
    STAGE_VERSION, bc5_level_count, cache_filename_for_key, expected_level_count,
};
use postretro_level_format::prm_accounting::{MaterialBytes, TextureByteSummary, slot_label};
use postretro_level_format::sprite_collection::{
    SpriteSlot, collection_frame_paths, sprite_collection_key_from_frame_bytes,
};

mod bake;
mod cache;
mod resolution;

use bake::{
    build_diffuse_chain, build_normal_bc5_chain, build_specular_chain, build_surface_chain,
};
use cache::{
    SLOT_MASK_BITS, bundle_hash_for, cache_entry_has_valid_declared_slots, filename_key_for,
};
use resolution::{
    build_name_to_path_map, normalize_map_texture_name, resolve_texture_bundle_paths,
};

// -- Gamma helpers --------------------------------------------------------

/// 256-entry sRGB → linear lookup. Built once per world-bundle bake.
fn build_srgb_to_linear_lut() -> [f32; 256] {
    let mut lut = [0.0f32; 256];
    for (i, slot) in lut.iter_mut().enumerate() {
        let c = (i as f32) / 255.0;
        *slot = if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        };
    }
    lut
}

/// IEC 61966-2-1 piecewise sRGB encode. Input is clamped to [0, 1] before
/// quantising; output is [0, 1].
fn linear_to_srgb(linear: f32) -> f32 {
    let x = linear.clamp(0.0, 1.0);
    if x < 0.0031308 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

/// `linear_to_srgb` then quantise to u8. Re-clamped after the encode in case
/// the polynomial leaves an out-of-range value at the seam.
fn linear_to_srgb_u8(linear: f32) -> u8 {
    (linear_to_srgb(linear).clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Clamp + quantise. Shared between alpha (sRGB images), specular, and normal
/// alpha.
fn linear_to_unorm_u8(linear: f32) -> u8 {
    (linear.clamp(0.0, 1.0) * 255.0).round() as u8
}

// -- Mitchell-Netravali filter --------------------------------------------

/// Mitchell-Netravali kernel with parameters `(b, c)`. Returns the weight at
/// `x` (already in destination texel space — caller does the scaling).
fn mitchell_netravali(x: f32, b: f32, c: f32) -> f32 {
    let x = x.abs();
    if x < 1.0 {
        (1.0 / 6.0)
            * ((12.0 - 9.0 * b - 6.0 * c) * x * x * x
                + (-18.0 + 12.0 * b + 6.0 * c) * x * x
                + (6.0 - 2.0 * b))
    } else if x < 2.0 {
        (1.0 / 6.0)
            * ((-b - 6.0 * c) * x * x * x
                + (6.0 * b + 30.0 * c) * x * x
                + (-12.0 * b - 48.0 * c) * x
                + (8.0 * b + 24.0 * c))
    } else {
        0.0
    }
}

const MN_B: f32 = 1.0 / 3.0;
const MN_C: f32 = 1.0 / 3.0;

/// One destination texel's set of source taps with renormalised weights.
struct Tap {
    /// Source-texel indices into a 1-D row/column. Always clamped to
    /// `[0, src_len - 1]` — out-of-bounds taps replicate the nearest edge.
    indices: Vec<i32>,
    weights: Vec<f32>,
}

/// Precompute per-destination-texel taps for a 1-D resample
/// `src_len → dst_len`. Each Tap's weights are renormalised to sum to exactly
/// 1.0 so a constant input stays constant after filtering (no DC shift from
/// the polynomial's tail truncation).
///
/// Scale is fixed at 2× (one mip step). The destination texel at index `i`
/// sits at source coordinate `(i + 0.5) * 2 - 0.5 = 2i + 0.5`, with the kernel
/// evaluated over a ±2-source-texel support (i.e. `(x - sample) / 2`).
fn precompute_taps_2x(src_len: u32) -> Vec<Tap> {
    let dst_len = (src_len / 2).max(1);
    let mut taps = Vec::with_capacity(dst_len as usize);

    // Filter scale: source samples-per-output-texel. For a 2× downsample, the
    // kernel domain is 2 source texels wide on each side of the centre.
    let scale: f32 = 2.0;
    let support: f32 = 2.0; // Mitchell-Netravali support radius
    let filter_radius_src = support * scale;

    let src_max = (src_len as i32) - 1;

    for i in 0..dst_len {
        // Centre of dst texel `i` in source coordinates.
        let centre = (i as f32 + 0.5) * scale - 0.5;
        let first = (centre - filter_radius_src).ceil() as i32;
        let last = (centre + filter_radius_src).floor() as i32;

        let mut indices = Vec::with_capacity((last - first + 1).max(0) as usize);
        let mut weights = Vec::with_capacity((last - first + 1).max(0) as usize);
        let mut wsum = 0.0f32;
        for s in first..=last {
            let x = (s as f32 - centre) / scale;
            let w = mitchell_netravali(x, MN_B, MN_C);
            if w == 0.0 {
                continue;
            }
            let clamped = s.clamp(0, src_max);
            indices.push(clamped);
            weights.push(w);
            wsum += w;
        }

        // Renormalise. With Mitchell-Netravali (B=C=1/3) the analytic
        // integral is 1.0, but the discrete sum drifts slightly off; explicit
        // renormalisation keeps the per-texel sum exact and is cheap.
        if wsum != 0.0 {
            let inv = 1.0 / wsum;
            for w in &mut weights {
                *w *= inv;
            }
        }

        taps.push(Tap { indices, weights });
    }

    taps
}

/// Separable 2× downsample of an interleaved-channel image buffer.
///
/// `src` is a flat row-major buffer of `src_w * src_h * channels` f32 samples
/// (interleaved per pixel). Returns a buffer of `dst_w * dst_h * channels`
/// samples. Behaviour:
/// - Horizontal pass writes into a scratch `f32` buffer of `dst_w * src_h *
///   channels`, vertical pass writes the final result.
/// - Clamp-to-edge: out-of-bounds taps replicate the nearest source sample
///   (already baked into the precomputed indices).
fn downsample_2x_f32(src: &[f32], src_w: u32, src_h: u32, channels: usize) -> (Vec<f32>, u32, u32) {
    let dst_w = (src_w / 2).max(1);
    let dst_h = (src_h / 2).max(1);

    let x_taps = precompute_taps_2x(src_w);
    let y_taps = precompute_taps_2x(src_h);

    // Horizontal pass: src_w → dst_w, same height.
    let mut h_buf = vec![0.0f32; (dst_w * src_h) as usize * channels];
    for y in 0..src_h {
        let src_row_base = (y as usize) * (src_w as usize) * channels;
        let dst_row_base = (y as usize) * (dst_w as usize) * channels;
        for (dx, tap) in x_taps.iter().enumerate() {
            for ch in 0..channels {
                let mut acc = 0.0f32;
                for (idx, w) in tap.indices.iter().zip(tap.weights.iter()) {
                    let sx = *idx as usize;
                    acc += src[src_row_base + sx * channels + ch] * w;
                }
                h_buf[dst_row_base + dx * channels + ch] = acc;
            }
        }
    }

    // Vertical pass: src_h → dst_h, width is dst_w.
    let dst_w_usize = dst_w as usize;
    let mut dst = vec![0.0f32; dst_w_usize * (dst_h as usize) * channels];
    for (dy, tap) in y_taps.iter().enumerate() {
        for x in 0..dst_w_usize {
            for ch in 0..channels {
                let mut acc = 0.0f32;
                for (idx, w) in tap.indices.iter().zip(tap.weights.iter()) {
                    let sy = *idx as usize;
                    acc += h_buf[sy * dst_w_usize * channels + x * channels + ch] * w;
                }
                dst[dy * dst_w_usize * channels + x * channels + ch] = acc;
            }
        }
    }

    (dst, dst_w, dst_h)
}

// -- Per-slot mip generation ---------------------------------------------

/// Build a diffuse mip chain (RGBA8, sRGB-tagged). Filtering happens in linear
/// space via the supplied sRGB → linear LUT; alpha is filtered linearly
/// without LUT application.
pub(super) fn build_diffuse_chain_impl(
    rgba: &[u8],
    width: u32,
    height: u32,
    lut: &[f32; 256],
) -> Vec<u8> {
    let channels = 4;
    let level_count = expected_level_count(width as u16, height as u16) as u32;

    // Decode source PNG into linear-f32 buffer (RGB through LUT, A direct).
    let mut linear: Vec<f32> = Vec::with_capacity((width * height) as usize * channels);
    for chunk in rgba.chunks_exact(4) {
        linear.push(lut[chunk[0] as usize]);
        linear.push(lut[chunk[1] as usize]);
        linear.push(lut[chunk[2] as usize]);
        linear.push((chunk[3] as f32) / 255.0);
    }

    let mut payload: Vec<u8> = Vec::new();
    // Re-encode mip 0 from the linear buffer for symmetry with downstream
    // mips (sRGB → linear → sRGB is lossless within rounding given the LUT
    // is byte-exact and the encode reverses it byte-exactly for the 256
    // table entries — verified empirically by the unit test).
    encode_diffuse_into(&linear, &mut payload);

    let mut cur = linear;
    let mut cw = width;
    let mut ch = height;
    for _ in 1..level_count {
        let (next, nw, nh) = downsample_2x_f32(&cur, cw, ch, channels);
        encode_diffuse_into(&next, &mut payload);
        cur = next;
        cw = nw;
        ch = nh;
    }

    payload
}

/// Encode a linear-RGBA `f32` buffer to sRGB-tagged Rgba8 bytes, appending to
/// the supplied payload.
fn encode_diffuse_into(linear: &[f32], out: &mut Vec<u8>) {
    for chunk in linear.chunks_exact(4) {
        out.push(linear_to_srgb_u8(chunk[0]));
        out.push(linear_to_srgb_u8(chunk[1]));
        out.push(linear_to_srgb_u8(chunk[2]));
        out.push(linear_to_unorm_u8(chunk[3]));
    }
}

/// Build a specular mip chain (R8Unorm). Input bytes are interpreted as the
/// red channel of an authored PNG (we accept either L8 or the R channel of
/// RGBA8 — the caller flattens before calling this).
pub(super) fn build_specular_chain_impl(r8: &[u8], width: u32, height: u32) -> Vec<u8> {
    let channels = 1;
    let level_count = expected_level_count(width as u16, height as u16) as u32;

    let mut linear: Vec<f32> = r8.iter().map(|b| (*b as f32) / 255.0).collect();
    let mut payload: Vec<u8> = Vec::with_capacity(r8.len() * 2);
    for &v in &linear {
        payload.push(linear_to_unorm_u8(v));
    }

    let mut cw = width;
    let mut ch = height;
    for _ in 1..level_count {
        let (next, nw, nh) = downsample_2x_f32(&linear, cw, ch, channels);
        for &v in &next {
            payload.push(linear_to_unorm_u8(v));
        }
        linear = next;
        cw = nw;
        ch = nh;
    }

    payload
}

/// Build a two-channel surface-map mip chain (`PrmFormat::Rg8Unorm`).
///
/// `rg` is interleaved `[specular, depth]` per texel at `width * height`
/// texels. Both channels are already linear and already in their stored
/// sense — in particular the caller has inverted the authored height map to
/// depth (`255 - height`) before interleaving, so mip 0 is a straight copy.
///
/// Filtering is the same Mitchell-Netravali (B = C = 1/3) separable path the
/// single-channel specular chain uses, applied to both channels at once, so a
/// surface map's R channel filters identically to the `R8Unorm` chain it
/// replaces. Mitchell-Netravali has negative lobes and can overshoot outside
/// `[0, 1]`; `linear_to_unorm_u8` clamps every output texel, which is what
/// keeps an overshoot near a hard mortar edge from wrapping to the opposite
/// extreme.
pub(super) fn build_surface_chain_impl(rg: &[u8], width: u32, height: u32) -> Vec<u8> {
    let channels = 2;
    let level_count = expected_level_count(width as u16, height as u16) as u32;

    let mut linear: Vec<f32> = rg.iter().map(|b| (*b as f32) / 255.0).collect();
    let mut payload: Vec<u8> = Vec::with_capacity(rg.len() * 2);
    for &v in &linear {
        payload.push(linear_to_unorm_u8(v));
    }

    let mut cw = width;
    let mut ch = height;
    for _ in 1..level_count {
        let (next, nw, nh) = downsample_2x_f32(&linear, cw, ch, channels);
        for &v in &next {
            payload.push(linear_to_unorm_u8(v));
        }
        linear = next;
        cw = nw;
        ch = nh;
    }

    payload
}

/// Build a BC5 normal mip chain (`PrmFormat::Bc5RgUnorm`). Each RGB octet is
/// decoded into the `[-1, 1]` interval before filtering; per level the normals
/// are renormalised, re-encoded to Rgba8, padded up to 4×4 block alignment
/// (clamp-to-edge), and BC5-compressed. Only R and G survive the BC5 encode;
/// the shader reconstructs n.z at runtime.
///
/// The chain is truncated at `bc5_level_count(w, h)` — BC5 needs both dims ≥ 4
/// per level, so sub-4 mips are dropped. The concatenated output exactly
/// matches the reader's `expected_payload_bytes(Bc5RgUnorm, w, h, level_count)`
/// contract: `ceil(w_n/4) * ceil(h_n/4) * 16` bytes per level.
pub(super) fn build_normal_bc5_chain_impl(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let channels = 4;
    let level_count = bc5_level_count(width as u16, height as u16) as u32;

    // Decode source RGB into the [-1, 1] interval (alpha kept in [0, 1]).
    let mut linear: Vec<f32> = Vec::with_capacity((width * height) as usize * channels);
    for chunk in rgba.chunks_exact(4) {
        linear.push((chunk[0] as f32) / 255.0 * 2.0 - 1.0);
        linear.push((chunk[1] as f32) / 255.0 * 2.0 - 1.0);
        linear.push((chunk[2] as f32) / 255.0 * 2.0 - 1.0);
        linear.push((chunk[3] as f32) / 255.0);
    }

    let mut payload: Vec<u8> = Vec::new();

    let mut cur = linear;
    let mut cw = width;
    let mut ch = height;
    for level in 0..level_count {
        if level > 0 {
            let (next, nw, nh) = downsample_2x_f32(&cur, cw, ch, channels);
            cur = next;
            cw = nw;
            ch = nh;
        }

        // Renormalised Rgba8 scratch for this level; the BC5 encoder reads
        // only R and G (B/A written for a valid Rgba8 layout).
        let rgba8 = renormalize_to_rgba8(&cur);

        // BC5 needs 4×4 block alignment. Power-of-two levels are already
        // aligned (the common case); non-power-of-two sources can yield a
        // level that is ≥ 4 yet not a multiple of 4, so pad up to the next
        // multiple of 4 by replicating edge texels (clamp-to-edge, matching
        // the downsampler's edge behaviour).
        let padded_w = cw.div_ceil(4) * 4;
        let padded_h = ch.div_ceil(4) * 4;
        let block_rgba = if padded_w == cw && padded_h == ch {
            rgba8
        } else {
            pad_rgba8_clamp_edge(&rgba8, cw, ch, padded_w, padded_h)
        };

        payload.extend_from_slice(&crate::bc5::encode_bc5_rg(&block_rgba, padded_w, padded_h));
    }

    payload
}

/// Renormalise a normal RGBA buffer (XYZ in `[-1, 1]`, A in `[0, 1]`) into
/// Rgba8 bytes. Each output normal is renormalised; near-zero magnitudes fall
/// back to `(0, 0, 1)` (tangent-space up). The BC5 encoder reads only R and G,
/// but B and A are still written so the buffer is a valid Rgba8 level.
fn renormalize_to_rgba8(linear: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(linear.len());
    for chunk in linear.chunks_exact(4) {
        let mut n = [chunk[0], chunk[1], chunk[2]];
        let len_sq = n[0] * n[0] + n[1] * n[1] + n[2] * n[2];
        let len = len_sq.sqrt();
        if len < 1e-4 {
            n = [0.0, 0.0, 1.0];
        } else {
            let inv = 1.0 / len;
            n[0] *= inv;
            n[1] *= inv;
            n[2] *= inv;
        }
        out.push(((n[0] * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8);
        out.push(((n[1] * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8);
        out.push(((n[2] * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8);
        out.push(linear_to_unorm_u8(chunk[3]));
    }
    out
}

/// Pad a tightly-packed `src_w × src_h` Rgba8 level up to `dst_w × dst_h` by
/// replicating edge texels (clamp-to-edge). `dst_w >= src_w` and
/// `dst_h >= src_h` are required; the source occupies the top-left corner and
/// padded rows/columns repeat the nearest in-bounds texel.
fn pad_rgba8_clamp_edge(src: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<u8> {
    debug_assert!(dst_w >= src_w && dst_h >= src_h);
    let mut out = vec![0u8; (dst_w * dst_h * 4) as usize];
    let src_max_x = src_w - 1;
    let src_max_y = src_h - 1;
    for y in 0..dst_h {
        let sy = y.min(src_max_y);
        for x in 0..dst_w {
            let sx = x.min(src_max_x);
            let s = ((sy * src_w + sx) * 4) as usize;
            let d = ((y * dst_w + x) * 4) as usize;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    out
}

// -- File I/O -------------------------------------------------------------

/// Decode PNG bytes into a `(rgba8, w, h)` triple. The `image` crate handles
/// all supported PNG colour types and converts them to RGBA8. Accepts an
/// already-read byte slice so callers that hash the bytes first can reuse them
/// without a second read.
fn decode_png_rgba(bytes: &[u8], path: &Path) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    use anyhow::Context as _;
    let img = image::load_from_memory(bytes)
        .with_context(|| format!("decoding PNG {}", path.display()))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((rgba.into_raw(), w, h))
}

fn png_dimensions(bytes: &[u8], path: &Path) -> anyhow::Result<(u32, u32)> {
    use anyhow::Context as _;
    use image::ImageDecoder as _;

    let decoder = image::codecs::png::PngDecoder::new(std::io::Cursor::new(bytes))
        .with_context(|| format!("reading PNG header {}", path.display()))?;
    Ok(decoder.dimensions())
}

/// Atomic write: write to `<target>.tmp.<pid>`, then `rename` to `target`.
fn atomic_write(target: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("target {} has no parent directory", target.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| anyhow::anyhow!("failed to create cache dir {}: {e}", parent.display()))?;
    let tmp_name = format!(
        "{}.tmp.{}",
        target.file_name().and_then(|s| s.to_str()).unwrap_or("prm"),
        std::process::id()
    );
    let tmp_path = parent.join(tmp_name);
    std::fs::write(&tmp_path, bytes)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, target).map_err(|e| {
        anyhow::anyhow!(
            "failed to rename {} -> {}: {e}",
            tmp_path.display(),
            target.display()
        )
    })?;
    Ok(())
}

struct SpriteFrameSource {
    path: std::path::PathBuf,
    bytes: Vec<u8>,
}

struct DecodedSpriteFrame {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

fn read_sprite_frame_sources(
    paths: &[std::path::PathBuf],
) -> anyhow::Result<Vec<SpriteFrameSource>> {
    paths
        .iter()
        .map(|path| {
            let bytes = std::fs::read(path)
                .map_err(|error| anyhow::anyhow!("reading PNG {}: {error}", path.display()))?;
            Ok(SpriteFrameSource {
                path: path.clone(),
                bytes,
            })
        })
        .collect()
}

fn sprite_collection_key_for_sources(
    diffuse_sources: &[SpriteFrameSource],
    spec_sources: &[SpriteFrameSource],
    normal_sources: &[SpriteFrameSource],
) -> [u8; 32] {
    let diffuse_bytes = diffuse_sources
        .iter()
        .map(|source| source.bytes.as_slice())
        .collect::<Vec<_>>();
    let spec_bytes = spec_sources
        .iter()
        .map(|source| source.bytes.as_slice())
        .collect::<Vec<_>>();
    let normal_bytes = normal_sources
        .iter()
        .map(|source| source.bytes.as_slice())
        .collect::<Vec<_>>();
    sprite_collection_key_from_frame_bytes(&diffuse_bytes, &spec_bytes, &normal_bytes)
}

fn decode_sprite_frames(sources: &[SpriteFrameSource]) -> anyhow::Result<Vec<DecodedSpriteFrame>> {
    sources
        .iter()
        .map(|source| {
            let (rgba, width, height) = decode_png_rgba(&source.bytes, &source.path)?;
            Ok(DecodedSpriteFrame {
                rgba,
                width,
                height,
            })
        })
        .collect()
}

fn frames_share_dimensions(frames: &[DecodedSpriteFrame], width: u32, height: u32) -> bool {
    frames
        .iter()
        .all(|frame| (frame.width, frame.height) == (width, height))
}

fn companions_match_diffuse_paths(
    diffuse_paths: &[std::path::PathBuf],
    companion_paths: &[std::path::PathBuf],
    companion_suffix: &str,
) -> bool {
    diffuse_paths.len() == companion_paths.len()
        && diffuse_paths
            .iter()
            .zip(companion_paths)
            .all(|(diffuse, companion)| {
                let Some(diffuse_stem) = diffuse.file_stem().and_then(|stem| stem.to_str()) else {
                    return false;
                };
                let Some(companion_stem) = companion.file_stem().and_then(|stem| stem.to_str())
                else {
                    return false;
                };
                companion_stem == format!("{diffuse_stem}_{companion_suffix}")
            })
}

/// Bake one map-visible sprite collection into a layered `.prm` sidecar.
///
/// Sprite frame scans are shared with the runtime fallback. A successful bake's
/// layer payload and cache filename derive from one source-byte snapshot.
/// Invalid source collections deliberately degrade to no sidecar: runtime
/// retains its PNG decode fallback.
pub fn bake_sprite_collection(
    texture_root: &Path,
    collection: &str,
    cache_root: &Path,
) -> Option<[u8; 32]> {
    let diffuse_paths = collection_frame_paths(texture_root, collection, SpriteSlot::Diffuse);
    if diffuse_paths.is_empty() {
        log::warn!(
            "[prl-build] sprite collection '{collection}' has no diffuse frames — skipping .prm bake"
        );
        return None;
    }
    if diffuse_paths.len() > PORTABLE_MAX_TEXTURE_ARRAY_LAYERS as usize {
        log::warn!(
            "[prl-build] sprite collection '{collection}' has {} diffuse frames, exceeding the portable array-layer limit of {PORTABLE_MAX_TEXTURE_ARRAY_LAYERS} — skipping .prm bake",
            diffuse_paths.len()
        );
        return None;
    }

    let spec_paths = collection_frame_paths(texture_root, collection, SpriteSlot::Spec);
    let normal_paths = collection_frame_paths(texture_root, collection, SpriteSlot::Normal);
    let diffuse_sources = match read_sprite_frame_sources(&diffuse_paths) {
        Ok(sources) => sources,
        Err(error) => {
            log::warn!(
                "[prl-build] failed to snapshot diffuse frames for sprite collection '{collection}' — skipping .prm bake: {error}"
            );
            return None;
        }
    };
    let spec_sources = match read_sprite_frame_sources(&spec_paths) {
        Ok(sources) => sources,
        Err(error) => {
            log::warn!(
                "[prl-build] failed to snapshot specular frames for sprite collection '{collection}' — skipping .prm bake: {error}"
            );
            return None;
        }
    };
    let normal_sources = match read_sprite_frame_sources(&normal_paths) {
        Ok(sources) => sources,
        Err(error) => {
            log::warn!(
                "[prl-build] failed to snapshot normal frames for sprite collection '{collection}' — skipping .prm bake: {error}"
            );
            return None;
        }
    };
    let filename_key =
        sprite_collection_key_for_sources(&diffuse_sources, &spec_sources, &normal_sources);

    let diffuse_frames = match decode_sprite_frames(&diffuse_sources) {
        Ok(frames) => frames,
        Err(error) => {
            log::warn!(
                "[prl-build] failed to decode diffuse frames for sprite collection '{collection}' — skipping .prm bake: {error}"
            );
            return None;
        }
    };
    let Some(first_diffuse) = diffuse_frames.first() else {
        log::warn!(
            "[prl-build] sprite collection '{collection}' has no decodable diffuse frames — skipping .prm bake"
        );
        return None;
    };
    let (width, height) = (first_diffuse.width, first_diffuse.height);
    if !frames_share_dimensions(&diffuse_frames, width, height) {
        log::warn!(
            "[prl-build] sprite collection '{collection}' has ragged diffuse frame dimensions — skipping .prm bake"
        );
        return None;
    }
    let (width_u16, height_u16) = match (u16::try_from(width), u16::try_from(height)) {
        (Ok(width), Ok(height)) => (width, height),
        _ => {
            log::warn!(
                "[prl-build] sprite collection '{collection}' frame dimensions {width}x{height} exceed the .prm representation — skipping .prm bake"
            );
            return None;
        }
    };

    let layer_count = diffuse_paths.len() as u16;
    let prm_path = cache_root.join(format!("{}.prm", cache_filename_for_key(&filename_key)));

    let mut slots: [Option<PrmSlot>; 4] = [None, None, None, None];
    let mut slot_mask = PrmSlots::DIFFUSE;
    let lut = build_srgb_to_linear_lut();
    let mut diffuse_payload = Vec::new();
    for frame in &diffuse_frames {
        diffuse_payload.extend_from_slice(&build_diffuse_chain(
            &frame.rgba,
            frame.width,
            frame.height,
            &lut,
        ));
    }
    slots[0] = Some(PrmSlot {
        format: PrmFormat::Rgba8UnormSrgb,
        width: width_u16,
        height: height_u16,
        level_count: expected_level_count(width_u16, height_u16),
        payload: diffuse_payload,
    });

    if !spec_paths.is_empty() {
        if !companions_match_diffuse_paths(&diffuse_paths, &spec_paths, "spec") {
            log::warn!(
                "[prl-build] sprite collection '{collection}' has incomplete specular companion frames — omitting the specular slot"
            );
        } else {
            match decode_sprite_frames(&spec_sources) {
                Ok(spec_frames) if frames_share_dimensions(&spec_frames, width, height) => {
                    let mut payload = Vec::new();
                    for frame in spec_frames {
                        let r8: Vec<u8> =
                            frame.rgba.chunks_exact(4).map(|pixel| pixel[0]).collect();
                        payload.extend_from_slice(&build_specular_chain(&r8, width, height));
                    }
                    slots[1] = Some(PrmSlot {
                        format: PrmFormat::R8Unorm,
                        width: width_u16,
                        height: height_u16,
                        level_count: expected_level_count(width_u16, height_u16),
                        payload,
                    });
                    slot_mask |= PrmSlots::SPECULAR;
                }
                Ok(_) => {
                    log::warn!(
                        "[prl-build] sprite collection '{collection}' has specular companion dimensions that do not match diffuse — omitting the specular slot"
                    );
                }
                Err(error) => {
                    log::warn!(
                        "[prl-build] failed to decode specular companion frames for sprite collection '{collection}' — omitting the specular slot: {error}"
                    );
                }
            }
        }
    }

    if !normal_paths.is_empty() {
        if !companions_match_diffuse_paths(&diffuse_paths, &normal_paths, "normal") {
            log::warn!(
                "[prl-build] sprite collection '{collection}' has incomplete normal companion frames — omitting the normal slot"
            );
        } else {
            match decode_sprite_frames(&normal_sources) {
                Ok(normal_frames) if !frames_share_dimensions(&normal_frames, width, height) => {
                    log::warn!(
                        "[prl-build] sprite collection '{collection}' has normal companion dimensions that do not match diffuse — omitting the normal slot"
                    );
                }
                Ok(normal_frames) => {
                    let level_count = bc5_level_count(width_u16, height_u16);
                    if level_count == 0 {
                        log::warn!(
                            "[prl-build] sprite collection '{collection}' normal companion frames are below the BC5 4x4 minimum — omitting the normal slot"
                        );
                    } else {
                        let mut payload = Vec::new();
                        for frame in normal_frames {
                            payload.extend_from_slice(&build_normal_bc5_chain(
                                &frame.rgba,
                                width,
                                height,
                            ));
                        }
                        slots[2] = Some(PrmSlot {
                            format: PrmFormat::Bc5RgUnorm,
                            width: width_u16,
                            height: height_u16,
                            level_count,
                            payload,
                        });
                        slot_mask |= PrmSlots::NORMAL;
                    }
                }
                Err(error) => {
                    log::warn!(
                        "[prl-build] failed to decode normal companion frames for sprite collection '{collection}' — omitting the normal slot: {error}"
                    );
                }
            }
        }
    }

    // Cache validation includes the collection's source-set key, array depth,
    // slot mask, and complete parsing of every declared slot. The key has a
    // sprite domain discriminator, so it cannot collide with world/model keys.
    if let Ok(bytes) = std::fs::read(&prm_path) {
        let (header, parsed_slots) = PrmFile::from_bytes_partial(&bytes);
        if let Ok(header) = header {
            if header.bundle_hash == filename_key
                && header.layer_count == layer_count
                && header.slot_mask == slot_mask
                && cache_entry_has_valid_declared_slots(&header, &parsed_slots)
            {
                return Some(filename_key);
            }
        }
    }

    let prm = PrmFile {
        header: PrmHeader {
            stage_version: STAGE_VERSION,
            slot_mask,
            bundle_hash: filename_key,
            total_body_bytes: 0,
            layer_count,
        },
        slots,
    };
    let encoded = match prm.to_bytes() {
        Ok(bytes) => bytes,
        Err(error) => {
            log::warn!(
                "[prl-build] failed to encode sprite collection '{collection}' .prm — skipping bake: {error}"
            );
            return None;
        }
    };
    if let Err(error) = atomic_write(&prm_path, &encoded) {
        log::warn!(
            "[prl-build] failed to write sprite collection '{collection}' .prm — skipping bake: {error}"
        );
        return None;
    }

    Some(filename_key)
}

/// Bake one base-color PNG as a diffuse-only `.prm` under `cache_root`.
/// Returns the 32-byte filename key used for the cache sidecar.
pub fn bake_diffuse_texture(diffuse_path: &Path, cache_root: &Path) -> anyhow::Result<[u8; 32]> {
    let diffuse_bytes = std::fs::read(diffuse_path).map_err(|e| {
        anyhow::anyhow!(
            "failed to read diffuse texture {}: {e}",
            diffuse_path.display()
        )
    })?;
    let filename_key = filename_key_for(Some(&diffuse_bytes), None, None, None, None);
    let bundle_hash = bundle_hash_for(Some(&diffuse_bytes), None, None, None, None);
    let prm_path = cache_root.join(format!("{}.prm", cache_filename_for_key(&filename_key)));

    // A legacy richer world bundle may still occupy this pre-change
    // diffuse-addressed filename. Keep a structurally valid one intact because
    // model loading consumes only its diffuse slot.
    if prm_path.exists() {
        if let Ok(bytes) = std::fs::read(&prm_path) {
            let (hdr_result, slots) = PrmFile::from_bytes_partial(&bytes);
            if let Ok(hdr) = hdr_result {
                let valid_slots =
                    hdr.layer_count == 1 && cache_entry_has_valid_declared_slots(&hdr, &slots);
                let matching_diffuse_only = hdr.slot_mask == PrmSlots::DIFFUSE
                    && hdr.bundle_hash == bundle_hash
                    && valid_slots;
                let valid_richer_world_bundle = hdr.slot_mask.contains(PrmSlots::DIFFUSE)
                    && hdr.slot_mask != PrmSlots::DIFFUSE
                    && valid_slots;
                if matching_diffuse_only || valid_richer_world_bundle {
                    return Ok(filename_key);
                }
            }
        }
    }

    let (rgba, width, height) = decode_png_rgba(&diffuse_bytes, diffuse_path)?;
    let lut = build_srgb_to_linear_lut();
    let payload = build_diffuse_chain(&rgba, width, height, &lut);
    let prm = PrmFile {
        header: PrmHeader {
            stage_version: STAGE_VERSION,
            slot_mask: PrmSlots::DIFFUSE,
            bundle_hash,
            total_body_bytes: 0,
            layer_count: 1,
        },
        slots: [
            Some(PrmSlot {
                format: PrmFormat::Rgba8UnormSrgb,
                width: width as u16,
                height: height as u16,
                level_count: expected_level_count(width as u16, height as u16),
                payload,
            }),
            None,
            None,
            None,
        ],
    };

    let encoded = prm.to_bytes().map_err(|e| {
        anyhow::anyhow!(
            "encoding diffuse-only .prm for {}: {e}",
            diffuse_path.display()
        )
    })?;
    atomic_write(&prm_path, &encoded)?;

    Ok(filename_key)
}

/// Bake the world's material bundles — per-texture mip pyramids in `.prm`
/// sidecars under `cache_root` — and report what they cost.
///
/// Returns a map from texture name → 32-byte cache key (the `.prm` filename
/// stem in hex). Names whose slots are all missing get a `[0u8; 32]` key and
/// no `.prm` is written; callers flag the all-zero key in
/// `TextureCacheKeysSection`. The runtime treats zero keys as 'no source PNG'
/// and substitutes placeholders silently by design — missing PNGs are not an
/// error in maps that don't use every named texture slot.
///
/// The byte accounting is handed back rather than logged, because the
/// compiler's TextureMips stage bakes model sidecars and sprite collections
/// too and the report has to cover all three. Callers fold those in with
/// [`StageTextureBytes::account_baked_sidecar`] and flush the whole report
/// once with [`log_texture_byte_summary`].
pub fn bake_world_texture_mips(
    texture_names: &[String],
    texture_root: &Path,
    cache_root: &Path,
) -> anyhow::Result<(HashMap<String, [u8; 32]>, StageTextureBytes)> {
    let name_to_path = build_name_to_path_map(texture_root);
    let lut = build_srgb_to_linear_lut();

    let mut out: HashMap<String, [u8; 32]> = HashMap::with_capacity(texture_names.len());
    // Per-material, per-slot, per-mip byte accounting for the stage report.
    // `out` is a HashMap, so the summary keeps its own name-sorted ordering
    // rather than borrowing hash iteration order — the report has to be
    // identical across runs for identical inputs, like everything else
    // `prl-build` emits.
    let mut byte_summary = StageTextureBytes::new();

    for name in texture_names {
        // Normalize the incoming map name: lowercase, backslashes → forward
        // slashes, and strip a leading `textures/` (a no-op when absent) so a
        // root-inclusive TrenchBroom name (`textures/collection/stem`) matches
        // the relative keys (`collection/stem`).
        let normalized = normalize_map_texture_name(name);

        // Preserve a requested qualified base when any of its slots exists.
        // Only an entirely missing qualified bundle may fall back to the
        // unique bare-stem aliases.
        let resolved = resolve_texture_bundle_paths(&name_to_path, &normalized);
        let diff_path = resolved.diffuse;
        let spec_path = resolved.specular;
        let norm_path = resolved.normal;
        let emissive_path = resolved.emissive;
        let height_path = resolved.height;

        // Read raw bytes (needed for both filename key and bundle hash).
        let diff_bytes = match diff_path.as_ref() {
            Some(p) => Some(std::fs::read(p).map_err(|e| {
                anyhow::anyhow!("failed to read diffuse {} for '{name}': {e}", p.display())
            })?),
            None => None,
        };
        let spec_bytes = match spec_path.as_ref() {
            Some(p) => Some(std::fs::read(p).map_err(|e| {
                anyhow::anyhow!("failed to read specular {} for '{name}': {e}", p.display())
            })?),
            None => None,
        };
        let norm_bytes = match norm_path.as_ref() {
            Some(p) => Some(std::fs::read(p).map_err(|e| {
                anyhow::anyhow!("failed to read normal {} for '{name}': {e}", p.display())
            })?),
            None => None,
        };
        let emissive_bytes = match emissive_path.as_ref() {
            Some(p) => Some(std::fs::read(p).map_err(|e| {
                anyhow::anyhow!("failed to read emissive {} for '{name}': {e}", p.display())
            })?),
            None => None,
        };
        let height_bytes = match height_path.as_ref() {
            Some(p) => Some(std::fs::read(p).map_err(|e| {
                anyhow::anyhow!("failed to read height {} for '{name}': {e}", p.display())
            })?),
            None => None,
        };

        let filename_key = filename_key_for(
            diff_bytes.as_deref(),
            spec_bytes.as_deref(),
            norm_bytes.as_deref(),
            emissive_bytes.as_deref(),
            height_bytes.as_deref(),
        );

        // All-absent: nothing to bake.
        if diff_bytes.is_none()
            && spec_bytes.is_none()
            && norm_bytes.is_none()
            && emissive_bytes.is_none()
            && height_bytes.is_none()
        {
            out.insert(name.clone(), [0u8; 32]);
            continue;
        }

        if let (Some(diffuse), Some(emissive), Some(diffuse_path), Some(emissive_path)) = (
            diff_bytes.as_deref(),
            emissive_bytes.as_deref(),
            diff_path.as_ref(),
            emissive_path.as_ref(),
        ) {
            let (diffuse_width, diffuse_height) = png_dimensions(diffuse, diffuse_path)?;
            let (emissive_width, emissive_height) = png_dimensions(emissive, emissive_path)?;
            if (emissive_width, emissive_height) != (diffuse_width, diffuse_height) {
                anyhow::bail!(
                    "emissive texture {} is {emissive_width}x{emissive_height}, but diffuse texture {} for '{name}' is {diffuse_width}x{diffuse_height}; _e.png dimensions must match diffuse",
                    emissive_path.display(),
                    diffuse_path.display(),
                );
            }
        }

        // `_h.png` is packed into the specular slot's G channel, so its
        // dimensions are load-bearing twice over: they must match the diffuse
        // the shader marches against, and they must match `_s.png` or the two
        // channels cannot be interleaved at all. Both are hard bails, the way
        // `_e` already is. (`_s`/`_n` versus diffuse remains documented-only
        // and unenforced — out of scope here.)
        if let (Some(height), Some(height_path)) = (height_bytes.as_deref(), height_path.as_ref()) {
            let (height_width, height_height) = png_dimensions(height, height_path)?;
            if let (Some(diffuse), Some(diffuse_path)) = (diff_bytes.as_deref(), diff_path.as_ref())
            {
                let (diffuse_width, diffuse_height) = png_dimensions(diffuse, diffuse_path)?;
                if (height_width, height_height) != (diffuse_width, diffuse_height) {
                    anyhow::bail!(
                        "height texture {} is {height_width}x{height_height}, but diffuse texture {} for '{name}' is {diffuse_width}x{diffuse_height}; _h.png dimensions must match diffuse",
                        height_path.display(),
                        diffuse_path.display(),
                    );
                }
            }
            if let (Some(specular), Some(spec_path)) = (spec_bytes.as_deref(), spec_path.as_ref()) {
                let (spec_width, spec_height) = png_dimensions(specular, spec_path)?;
                if (height_width, height_height) != (spec_width, spec_height) {
                    anyhow::bail!(
                        "height texture {} is {height_width}x{height_height}, but specular texture {} for '{name}' is {spec_width}x{spec_height}; _h.png and _s.png share one Rg8Unorm slot and must have identical dimensions",
                        height_path.display(),
                        spec_path.display(),
                    );
                }
            }
        }

        let bundle_hash = bundle_hash_for(
            diff_bytes.as_deref(),
            spec_bytes.as_deref(),
            norm_bytes.as_deref(),
            emissive_bytes.as_deref(),
            height_bytes.as_deref(),
        );

        let prm_path = cache_root.join(format!("{}.prm", cache_filename_for_key(&filename_key)));

        // Cache hit: header and every declared slot parse, and bundle_hash matches.
        if prm_path.exists() {
            if let Ok(bytes) = std::fs::read(&prm_path) {
                let (hdr_result, slots) = PrmFile::from_bytes_partial(&bytes);
                if let Ok(hdr) = hdr_result {
                    if hdr.layer_count == 1
                        && hdr.bundle_hash == bundle_hash
                        && cache_entry_has_valid_declared_slots(&hdr, &slots)
                    {
                        // Account the reused sidecar too: the report describes
                        // what this level costs, not what this run rebaked.
                        byte_summary.record_bundle(
                            filename_key,
                            name.clone(),
                            MaterialBytes::from_parsed_slots(&slots, hdr.layer_count),
                        );
                        out.insert(name.clone(), filename_key);
                        continue;
                    }
                }
            }
        }

        // Build only the slots whose source PNGs exist. Emissive dimensions
        // are checked against diffuse before the bundle is encoded.
        let mut slots_arr: [Option<PrmSlot>; 4] = [None, None, None, None];
        let mut slot_mask = PrmSlots::empty();

        if let (Some(b), Some(p)) = (diff_bytes.as_deref(), diff_path.as_ref()) {
            let (rgba, w, h) = decode_png_rgba(b, p)?;
            let payload = build_diffuse_chain(&rgba, w, h, &lut);
            slots_arr[0] = Some(PrmSlot {
                format: PrmFormat::Rgba8UnormSrgb,
                width: w as u16,
                height: h as u16,
                level_count: expected_level_count(w as u16, h as u16),
                payload,
            });
            slot_mask |= PrmSlots::DIFFUSE;
        }
        // Slot 1 is the specular slot in both of its forms. Without a height
        // sibling it stays single-channel `R8Unorm`, byte-identical to what it
        // has always baked. With one it becomes the two-channel surface map:
        // R specular (or 0 when `_s.png` is absent), G depth. The SPECULAR
        // slot-mask bit is set if EITHER sibling is present.
        match (height_bytes.as_deref(), height_path.as_ref()) {
            (Some(hb), Some(hp)) => {
                let (height_rgba, w, h) = decode_png_rgba(hb, hp)?;
                // Authors write a conventional height map: white = raised.
                // The stored channel is depth BELOW the surface, so invert
                // here — that is what makes "no height sibling" and "depth 0"
                // the same thing at sample time.
                let depth: Vec<u8> = height_rgba.chunks_exact(4).map(|c| 255 - c[0]).collect();

                let specular: Vec<u8> = match spec_bytes.as_deref().zip(spec_path.as_ref()) {
                    Some((sb, sp)) => {
                        let (spec_rgba, _, _) = decode_png_rgba(sb, sp)?;
                        spec_rgba.chunks_exact(4).map(|c| c[0]).collect()
                    }
                    None => vec![0u8; depth.len()],
                };

                let mut rg: Vec<u8> = Vec::with_capacity(depth.len() * 2);
                for (s, d) in specular.iter().zip(depth.iter()) {
                    rg.push(*s);
                    rg.push(*d);
                }

                let payload = build_surface_chain(&rg, w, h);
                slots_arr[1] = Some(PrmSlot {
                    format: PrmFormat::Rg8Unorm,
                    width: w as u16,
                    height: h as u16,
                    level_count: expected_level_count(w as u16, h as u16),
                    payload,
                });
                slot_mask |= PrmSlots::SPECULAR;
            }
            _ => {
                if let (Some(b), Some(p)) = (spec_bytes.as_deref(), spec_path.as_ref()) {
                    // Decode as RGBA; flatten to R8 (PNG authoring is typically L8 or
                    // RGBA8 with the spec data in R). We accept either.
                    let (rgba, w, h) = decode_png_rgba(b, p)?;
                    let r8: Vec<u8> = rgba.chunks_exact(4).map(|c| c[0]).collect();
                    let payload = build_specular_chain(&r8, w, h);
                    slots_arr[1] = Some(PrmSlot {
                        format: PrmFormat::R8Unorm,
                        width: w as u16,
                        height: h as u16,
                        level_count: expected_level_count(w as u16, h as u16),
                        payload,
                    });
                    slot_mask |= PrmSlots::SPECULAR;
                }
            }
        }
        if let (Some(b), Some(p)) = (norm_bytes.as_deref(), norm_path.as_ref()) {
            let (rgba, w, h) = decode_png_rgba(b, p)?;
            // BC5 needs both dims ≥ 4. A normal map smaller than 4×4 has no
            // valid BC5 level, so emitting the slot would write level_count = 0
            // with an empty payload — which the runtime cannot upload. Drop the
            // slot instead; the runtime substitutes its neutral-normal
            // placeholder for an absent NORMAL slot.
            let level_count = bc5_level_count(w as u16, h as u16);
            if level_count == 0 {
                log::warn!(
                    "[prl-build] normal map for '{name}' is {w}x{h}, below the BC5 4x4 minimum — \
                     dropping the normal slot; the runtime neutral-normal placeholder will be used"
                );
            } else {
                let payload = build_normal_bc5_chain(&rgba, w, h);
                slots_arr[2] = Some(PrmSlot {
                    format: PrmFormat::Bc5RgUnorm,
                    width: w as u16,
                    height: h as u16,
                    level_count,
                    payload,
                });
                slot_mask |= PrmSlots::NORMAL;
            }
        }
        if let (Some(b), Some(p)) = (emissive_bytes.as_deref(), emissive_path.as_ref()) {
            let (rgba, w, h) = decode_png_rgba(b, p)?;
            let payload = build_diffuse_chain(&rgba, w, h, &lut);
            slots_arr[3] = Some(PrmSlot {
                format: PrmFormat::Rgba8UnormSrgb,
                width: w as u16,
                height: h as u16,
                level_count: expected_level_count(w as u16, h as u16),
                payload,
            });
            slot_mask |= PrmSlots::EMISSIVE;
        }

        let prm = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask,
                bundle_hash,
                total_body_bytes: 0, // recomputed by to_bytes
                layer_count: 1,
            },
            slots: slots_arr,
        };

        let encoded = prm
            .to_bytes()
            .map_err(|e| anyhow::anyhow!("encoding .prm for texture {name:?}: {e}"))?;
        atomic_write(&prm_path, &encoded)?;

        byte_summary.record_bundle(
            filename_key,
            name.clone(),
            MaterialBytes::from_slots(&prm.slots, prm.header.layer_count),
        );
        out.insert(name.clone(), filename_key);
    }

    Ok((out, byte_summary))
}

/// Which of a sidecar's slots the caller that asked for it actually uploads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarSlots {
    /// Every slot the file declares. The sprite path uploads the whole bundle.
    All,
    /// The diffuse slot alone. Model loading consumes only its diffuse slot,
    /// and [`bake_diffuse_texture`] deliberately accepts a richer world bundle
    /// already parked at the diffuse-only address — charging its specular,
    /// normal and emissive slots to the model would bill the report for slots
    /// the runtime never binds for it.
    DiffuseOnly,
}

/// The `TextureMips` stage's byte report under construction.
///
/// Charges each baked bundle **once per cache key**, not once per name. A
/// `.prm` is content-addressed, so one file is reachable under several names
/// at once: two map names that normalize to the same texture, two glTFs
/// carrying byte-identical base-color PNGs at different paths, a diffuse-only
/// world material sitting at the very address a model bake derives, two sprite
/// collections with identical frame sets. [`TextureByteSummary::record`] can
/// only dedup by name, so without this guard every extra walk adds a whole
/// second charge to the grand total for memory the runtime uploads once.
#[derive(Debug, Default)]
pub struct StageTextureBytes {
    summary: TextureByteSummary,
    /// Cache keys already charged. First name to reach a key owns its report
    /// line; the stage bakes world bundles first, so a shared address reports
    /// under its world material name.
    accounted: HashSet<[u8; 32]>,
}

impl StageTextureBytes {
    pub fn new() -> Self {
        Self::default()
    }

    /// The report accumulated so far.
    pub fn summary(&self) -> &TextureByteSummary {
        &self.summary
    }

    /// Charge a bundle this stage baked or found cached, unless its cache key
    /// is already charged.
    fn record_bundle(&mut self, key: [u8; 32], name: impl Into<String>, bytes: MaterialBytes) {
        if self.accounted.insert(key) {
            self.summary.record(name, bytes);
        }
    }

    /// Account a sidecar another bake path just wrote.
    ///
    /// The model and sprite bakes write their own `.prm` files and hand back
    /// only the cache key, so the stage report reads the file back rather than
    /// threading a slot table out of each one. The read is cheap next to the
    /// bake, and it keeps the accounting honest about what actually landed on
    /// disk.
    ///
    /// Every way this can fail is logged: a byte report that silently shrinks
    /// is the exact failure this accounting exists to rule out.
    pub fn account_baked_sidecar(
        &mut self,
        name: impl Into<String>,
        cache_root: &Path,
        key: &[u8; 32],
        charged: SidecarSlots,
    ) {
        if !self.accounted.insert(*key) {
            return;
        }
        let name = name.into();
        let prm_path = cache_root.join(format!("{}.prm", cache_filename_for_key(key)));
        let bytes = match std::fs::read(&prm_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!(
                    "[prl-build] byte report: cannot read baked sidecar {} for '{name}': \
                     {error} — its bytes are missing from the texture total",
                    prm_path.display()
                );
                return;
            }
        };
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let header = match header {
            Ok(header) => header,
            Err(error) => {
                log::warn!(
                    "[prl-build] byte report: cannot parse baked sidecar {} for '{name}': \
                     {error} — its bytes are missing from the texture total",
                    prm_path.display()
                );
                return;
            }
        };
        // A slot the header declares but that failed to parse contributes
        // nothing, so the bundle would otherwise be reported light while
        // looking perfectly healthy.
        for (index, declared) in SLOT_MASK_BITS.iter().enumerate() {
            if header.slot_mask.contains(*declared) {
                if let Err(error) = &slots[index] {
                    log::warn!(
                        "[prl-build] byte report: baked sidecar {} for '{name}' declares a \
                         {} slot that does not parse: {error} — that slot is missing from \
                         the texture total",
                        prm_path.display(),
                        slot_label(index as u8)
                    );
                }
            }
        }

        let mut charged_slots = slots.map(|slot| slot.ok());
        if charged == SidecarSlots::DiffuseOnly {
            for slot in &mut charged_slots[1..] {
                *slot = None;
            }
        }
        self.summary.record(
            name,
            MaterialBytes::from_slots(&charged_slots, header.layer_count),
        );
    }
}

/// How many of the heaviest materials the stage report names individually.
const BYTE_REPORT_LARGEST: usize = 5;

/// Report the `TextureMips` stage's texture-memory accounting.
///
/// This is the only texture byte report the engine has; an asset streaming
/// system is the anticipated consumer of the underlying per-mip primitive
/// (`postretro_level_format::prm_accounting`). Reporting only — no budget, no
/// cap, no eviction.
pub fn log_texture_byte_summary(summary: &TextureByteSummary) {
    for line in summary.report_lines(BYTE_REPORT_LARGEST) {
        log::info!("[prl-build] {line}");
    }
}

// -- Tests ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use log::Level;
    use postretro_level_format::prm::PrmReadError;
    use postretro_level_format::sprite_collection::sprite_collection_filename_key;
    use postretro_test_log_capture::LogCapture;

    /// Drive the world-bundle bake and flush its report, the way the compiler
    /// stage does for the bakes it owns. Most tests here only bake world
    /// bundles, and several assert on the logged report.
    fn bake_texture_mips(
        texture_names: &[String],
        texture_root: &Path,
        cache_root: &Path,
    ) -> anyhow::Result<HashMap<String, [u8; 32]>> {
        let (keys, byte_summary) =
            bake_world_texture_mips(texture_names, texture_root, cache_root)?;
        log_texture_byte_summary(byte_summary.summary());
        Ok(keys)
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "prl-build-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([
                (x * 37 + y * 11) as u8,
                (x * 13 + y * 29) as u8,
                (x * 7 + y * 43) as u8,
                255,
            ])
        });
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    fn solid_png_bytes(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(color));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    fn duplicate_prm_layers(bytes: &[u8], layer_count: u16) -> Vec<u8> {
        let (header, slots) = PrmFile::from_bytes_partial(bytes);
        let mut header = header.expect("source header parses");
        header.layer_count = layer_count;
        let slots = slots.map(|slot| {
            slot.ok().map(|mut slot| {
                slot.payload = slot.payload.repeat(usize::from(layer_count));
                slot
            })
        });
        PrmFile { header, slots }
            .to_bytes()
            .expect("layered cache fixture serializes")
    }

    fn write_sprite_frame(texture_root: &Path, collection: &str, filename: &str, bytes: &[u8]) {
        let collection_dir = texture_root.join(collection);
        std::fs::create_dir_all(&collection_dir).unwrap();
        std::fs::write(collection_dir.join(filename), bytes).unwrap();
    }

    #[test]
    fn sprite_collection_bake_writes_layer_major_diffuse_and_companion_slots() {
        let root = unique_temp_dir("sprite-layered-slots");
        let texture_root = root.join("textures");
        let cache_root = root.join("cache");
        write_sprite_frame(
            &texture_root,
            "smoke",
            "smoke_00.png",
            &solid_png_bytes(4, 4, [255, 0, 0, 255]),
        );
        write_sprite_frame(
            &texture_root,
            "smoke",
            "smoke_01.png",
            &solid_png_bytes(4, 4, [0, 0, 255, 255]),
        );
        write_sprite_frame(
            &texture_root,
            "smoke",
            "smoke_00_spec.png",
            &solid_png_bytes(4, 4, [32, 0, 0, 255]),
        );
        write_sprite_frame(
            &texture_root,
            "smoke",
            "smoke_01_spec.png",
            &solid_png_bytes(4, 4, [224, 0, 0, 255]),
        );
        write_sprite_frame(
            &texture_root,
            "smoke",
            "smoke_00_normal.png",
            &solid_png_bytes(4, 4, [128, 128, 255, 255]),
        );
        write_sprite_frame(
            &texture_root,
            "smoke",
            "smoke_01_normal.png",
            &solid_png_bytes(4, 4, [255, 128, 128, 255]),
        );

        let key = bake_sprite_collection(&texture_root, "smoke", &cache_root)
            .expect("valid sprite collection should bake");
        let runtime_key = sprite_collection_filename_key(&texture_root, "smoke");
        assert_eq!(key, runtime_key, "compiler and runtime share the cache key");
        let cache_path = cache_root.join(format!("{}.prm", cache_filename_for_key(&runtime_key)));
        assert!(cache_path.is_file(), "shared key names the emitted sidecar");

        let bytes = std::fs::read(cache_path).unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let header = header.expect("sprite .prm header parses");
        assert_eq!(header.stage_version, STAGE_VERSION);
        assert_eq!(header.bundle_hash, runtime_key);
        assert_eq!(header.layer_count, 2);
        assert_eq!(
            header.slot_mask,
            PrmSlots::DIFFUSE | PrmSlots::SPECULAR | PrmSlots::NORMAL
        );

        let diffuse = slots[0].as_ref().expect("diffuse slot parses");
        assert_eq!(diffuse.format, PrmFormat::Rgba8UnormSrgb);
        assert_eq!(
            (diffuse.width, diffuse.height, diffuse.level_count),
            (4, 4, 3)
        );
        // Each layer is an independent complete 4x4 → 2x2 → 1x1 chain.
        // Uniform red and blue frames must therefore retain their own color at
        // every level, proving the payload is layer-major rather than a
        // cross-frame strip mip chain.
        let per_layer_bytes = 4 * 4 * 4 + 2 * 2 * 4 + 4;
        assert_eq!(diffuse.payload.len(), per_layer_bytes * 2);
        for (layer, color) in [[255, 0, 0, 255], [0, 0, 255, 255]].iter().enumerate() {
            for pixel in diffuse.payload[layer * per_layer_bytes..(layer + 1) * per_layer_bytes]
                .chunks_exact(4)
            {
                assert_eq!(pixel, color, "layer {layer} lost its frame identity");
            }
        }

        let specular = slots[1].as_ref().expect("specular slot parses");
        assert_eq!(specular.format, PrmFormat::R8Unorm);
        assert_eq!(
            (specular.width, specular.height, specular.level_count),
            (4, 4, 3)
        );
        assert_eq!(specular.payload.len(), (16 + 4 + 1) * 2);

        let normal = slots[2].as_ref().expect("normal slot parses");
        assert_eq!(normal.format, PrmFormat::Bc5RgUnorm);
        assert_eq!((normal.width, normal.height, normal.level_count), (4, 4, 1));
        assert_eq!(normal.payload.len(), 16 * 2, "one BC5 block per layer");
        assert!(slots[3].is_err(), "emissive is absent from sprite bundles");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sprite_collection_bake_key_uses_the_retained_source_snapshot() {
        // Regression: the payload decoded one read while a later key scan
        // could observe different bytes for the same frame.
        let root = unique_temp_dir("sprite-source-snapshot");
        let texture_root = root.join("textures");
        write_sprite_frame(
            &texture_root,
            "smoke",
            "smoke_00.png",
            &solid_png_bytes(4, 4, [255, 0, 0, 255]),
        );
        let paths = collection_frame_paths(&texture_root, "smoke", SpriteSlot::Diffuse);
        let sources = read_sprite_frame_sources(&paths).expect("source snapshot reads");
        let snapshot_key = sprite_collection_key_for_sources(&sources, &[], &[]);

        write_sprite_frame(
            &texture_root,
            "smoke",
            "smoke_00.png",
            &solid_png_bytes(4, 4, [0, 0, 255, 255]),
        );

        assert_ne!(
            snapshot_key,
            sprite_collection_filename_key(&texture_root, "smoke"),
            "a later filesystem version must not rename the retained bake snapshot"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sprite_collection_incomplete_or_mismatched_companions_omit_only_their_slots() {
        let root = unique_temp_dir("sprite-incomplete-companions");
        let texture_root = root.join("textures");
        let cache_root = root.join("cache");
        for frame in ["00", "01"] {
            write_sprite_frame(
                &texture_root,
                "smoke",
                &format!("smoke_{frame}.png"),
                &png_bytes(4, 4),
            );
        }
        // Missing the _01 spec frame makes the specular set incomplete.
        write_sprite_frame(
            &texture_root,
            "smoke",
            "smoke_00_spec.png",
            &png_bytes(4, 4),
        );
        // Both normal frames exist, but their geometry cannot form the diffuse
        // array's layers and must be omitted independently of specular.
        for frame in ["00", "01"] {
            write_sprite_frame(
                &texture_root,
                "smoke",
                &format!("smoke_{frame}_normal.png"),
                &png_bytes(2, 4),
            );
        }

        let capture = LogCapture::start();
        let key = bake_sprite_collection(&texture_root, "smoke", &cache_root)
            .expect("diffuse still bakes when companions are invalid");
        capture.assert_logged_once(Level::Warn, "incomplete specular companion frames");
        capture.assert_logged_once(
            Level::Warn,
            "normal companion dimensions that do not match diffuse",
        );

        let bytes = std::fs::read(cache_root.join(format!("{}.prm", cache_filename_for_key(&key))))
            .unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        assert_eq!(header.unwrap().slot_mask, PrmSlots::DIFFUSE);
        assert!(slots[0].is_ok());
        assert!(slots[1].is_err());
        assert!(slots[2].is_err());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sprite_collection_sub_four_normal_companions_are_omitted_with_a_warning() {
        let root = unique_temp_dir("sprite-sub-four-normal");
        let texture_root = root.join("textures");
        let cache_root = root.join("cache");
        for frame in ["00", "01"] {
            write_sprite_frame(
                &texture_root,
                "sparks",
                &format!("sparks_{frame}.png"),
                &png_bytes(2, 2),
            );
            write_sprite_frame(
                &texture_root,
                "sparks",
                &format!("sparks_{frame}_normal.png"),
                &png_bytes(2, 2),
            );
        }

        let capture = LogCapture::start();
        let key = bake_sprite_collection(&texture_root, "sparks", &cache_root)
            .expect("diffuse remains valid below the BC5 floor");
        capture.assert_logged_once(Level::Warn, "below the BC5 4x4 minimum");

        let bytes = std::fs::read(cache_root.join(format!("{}.prm", cache_filename_for_key(&key))))
            .unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        assert_eq!(header.unwrap().slot_mask, PrmSlots::DIFFUSE);
        assert!(slots[2].is_err());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sprite_collection_without_diffuse_frames_refuses_the_bake() {
        let root = unique_temp_dir("sprite-empty-diffuse");
        let capture = LogCapture::start();
        assert_eq!(
            bake_sprite_collection(&root, "smoke", &root.join("cache")),
            None
        );
        capture.assert_logged_once(Level::Warn, "has no diffuse frames");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sprite_collection_ragged_diffuse_frames_refuse_the_bake() {
        let root = unique_temp_dir("sprite-ragged-diffuse");
        let texture_root = root.join("textures");
        write_sprite_frame(&texture_root, "smoke", "smoke_00.png", &png_bytes(4, 4));
        write_sprite_frame(&texture_root, "smoke", "smoke_01.png", &png_bytes(2, 4));

        let capture = LogCapture::start();
        assert_eq!(
            bake_sprite_collection(&texture_root, "smoke", &root.join("cache")),
            None
        );
        capture.assert_logged_once(Level::Warn, "ragged diffuse frame dimensions");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sprite_collection_over_portable_layer_cap_refuses_the_bake() {
        let root = unique_temp_dir("sprite-layer-cap");
        let texture_root = root.join("textures");
        let frame = png_bytes(1, 1);
        for index in 0..=PORTABLE_MAX_TEXTURE_ARRAY_LAYERS {
            write_sprite_frame(
                &texture_root,
                "smoke",
                &format!("smoke_{index:03}.png"),
                &frame,
            );
        }

        let capture = LogCapture::start();
        assert_eq!(
            bake_sprite_collection(&texture_root, "smoke", &root.join("cache")),
            None
        );
        capture.assert_logged_once(Level::Warn, "exceeding the portable array-layer limit");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn single_diffuse_bake_matches_world_diffuse_only_output() {
        let root = unique_temp_dir("single-diffuse-equivalence");
        let texture_root = root.join("textures");
        let collection = texture_root.join("models");
        let world_cache = root.join("world-cache");
        let model_cache = root.join("model-cache");
        std::fs::create_dir_all(&collection).unwrap();

        let diffuse_path = collection.join("base_color.png");
        let source_bytes = png_bytes(4, 2);
        std::fs::write(&diffuse_path, &source_bytes).unwrap();

        let world_keys = bake_texture_mips(
            &["models/base_color".to_string()],
            &texture_root,
            &world_cache,
        )
        .unwrap();
        let world_key = world_keys["models/base_color"];
        let model_key = bake_diffuse_texture(&diffuse_path, &model_cache).unwrap();

        assert_eq!(model_key, *blake3::hash(&source_bytes).as_bytes());
        assert_eq!(model_key, world_key);

        let filename = format!("{}.prm", cache_filename_for_key(&model_key));
        let world_bytes = std::fs::read(world_cache.join(&filename)).unwrap();
        let model_bytes = std::fs::read(model_cache.join(&filename)).unwrap();
        assert_eq!(model_bytes, world_bytes);

        let (header, slots) = PrmFile::from_bytes_partial(&model_bytes);
        let header = header.unwrap();
        assert_eq!(header.slot_mask, PrmSlots::DIFFUSE);
        let diffuse = slots[0].as_ref().unwrap();
        assert_eq!(diffuse.format, PrmFormat::Rgba8UnormSrgb);
        assert!(slots[1].is_err());
        assert!(slots[2].is_err());
        assert!(slots[3].is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn single_diffuse_bake_preserves_matching_bundle_hash_cache_entry() {
        let root = unique_temp_dir("single-diffuse-cache-hit");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&root).unwrap();

        let diffuse_path = root.join("base_color.png");
        let source_bytes = png_bytes(4, 4);
        std::fs::write(&diffuse_path, &source_bytes).unwrap();

        let key = filename_key_for(Some(&source_bytes), None, None, None, None);
        let cached = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: PrmSlots::DIFFUSE,
                bundle_hash: bundle_hash_for(Some(&source_bytes), None, None, None, None),
                total_body_bytes: 0,
                layer_count: 1,
            },
            slots: [
                Some(PrmSlot {
                    format: PrmFormat::Rgba8UnormSrgb,
                    width: 1,
                    height: 1,
                    level_count: 1,
                    payload: vec![1, 2, 3, 4],
                }),
                None,
                None,
                None,
            ],
        }
        .to_bytes()
        .unwrap();
        let cache_path = cache_root.join(format!("{}.prm", cache_filename_for_key(&key)));
        atomic_write(&cache_path, &cached).unwrap();

        let returned_key = bake_diffuse_texture(&diffuse_path, &cache_root).unwrap();

        assert_eq!(returned_key, key);
        assert_eq!(std::fs::read(&cache_path).unwrap(), cached);

        let _ = std::fs::remove_dir_all(&root);
    }

    // Regression: model cache hits accepted array PRMs even though runtime
    // model textures still use the legacy single-layer D2 upload path.
    #[test]
    fn single_diffuse_bake_rebuilds_multi_layer_cache_entry() {
        let root = unique_temp_dir("single-diffuse-rebuilds-multi-layer");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&root).unwrap();

        let diffuse_path = root.join("base_color.png");
        std::fs::write(&diffuse_path, png_bytes(4, 4)).unwrap();
        let key = bake_diffuse_texture(&diffuse_path, &cache_root).unwrap();
        let cache_path = cache_root.join(format!("{}.prm", cache_filename_for_key(&key)));
        let layered = duplicate_prm_layers(&std::fs::read(&cache_path).unwrap(), 2);
        std::fs::write(&cache_path, &layered).unwrap();

        bake_diffuse_texture(&diffuse_path, &cache_root).unwrap();

        let rebuilt = std::fs::read(&cache_path).unwrap();
        assert_ne!(rebuilt, layered, "multi-layer model cache must be rebuilt");
        let (header, slots) = PrmFile::from_bytes_partial(&rebuilt);
        assert_eq!(header.expect("rebuilt header parses").layer_count, 1);
        assert!(slots[0].is_ok(), "rebuilt diffuse slot parses");

        let _ = std::fs::remove_dir_all(&root);
    }

    // Regression: richer world bundles must not share the model's diffuse-only
    // filename, or another material with the same diffuse can overwrite them.
    #[test]
    fn single_diffuse_bake_uses_distinct_filename_from_richer_world_bundle() {
        let root = unique_temp_dir("single-diffuse-distinct-from-world-bundle");
        let texture_root = root.join("textures");
        let collection = texture_root.join("shared");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&collection).unwrap();

        let diffuse_path = collection.join("surface.png");
        std::fs::write(&diffuse_path, png_bytes(8, 8)).unwrap();
        std::fs::write(collection.join("surface_s.png"), png_bytes(8, 8)).unwrap();
        std::fs::write(collection.join("surface_n.png"), png_bytes(8, 8)).unwrap();
        std::fs::write(collection.join("surface_e.png"), png_bytes(8, 8)).unwrap();

        let world_keys =
            bake_texture_mips(&["shared/surface".to_string()], &texture_root, &cache_root).unwrap();
        let world_key = world_keys["shared/surface"];
        let world_path = cache_root.join(format!("{}.prm", cache_filename_for_key(&world_key)));
        let world_bytes = std::fs::read(&world_path).unwrap();

        let model_key = bake_diffuse_texture(&diffuse_path, &cache_root).unwrap();
        let model_path = cache_root.join(format!("{}.prm", cache_filename_for_key(&model_key)));

        assert_ne!(model_key, world_key);
        assert_eq!(std::fs::read(&world_path).unwrap(), world_bytes);
        let (header, slots) = PrmFile::from_bytes_partial(&world_bytes);
        let header = header.unwrap();
        assert_eq!(
            header.slot_mask,
            PrmSlots::DIFFUSE | PrmSlots::SPECULAR | PrmSlots::NORMAL | PrmSlots::EMISSIVE
        );
        assert!(slots.iter().all(Result::is_ok));

        let model_bytes = std::fs::read(model_path).unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&model_bytes);
        assert_eq!(header.unwrap().slot_mask, PrmSlots::DIFFUSE);
        assert!(slots[0].is_ok());
        assert!(slots[1..].iter().all(Result::is_err));

        let _ = std::fs::remove_dir_all(&root);
    }

    // Regression: a matching header alone is not a cache hit when the declared
    // diffuse payload is truncated.
    #[test]
    fn single_diffuse_bake_rebuilds_matching_header_with_truncated_diffuse() {
        let root = unique_temp_dir("single-diffuse-repairs-truncated");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&root).unwrap();

        let diffuse_path = root.join("base_color.png");
        let source_bytes = png_bytes(8, 8);
        std::fs::write(&diffuse_path, &source_bytes).unwrap();
        let key = bake_diffuse_texture(&diffuse_path, &cache_root).unwrap();
        let cache_path = cache_root.join(format!("{}.prm", cache_filename_for_key(&key)));

        let mut corrupt = std::fs::read(&cache_path).unwrap();
        corrupt.pop();
        std::fs::write(&cache_path, &corrupt).unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&corrupt);
        assert!(header.is_ok(), "header remains parseable");
        assert!(slots[0].is_err(), "declared diffuse payload is corrupt");

        bake_diffuse_texture(&diffuse_path, &cache_root).unwrap();

        let repaired = std::fs::read(&cache_path).unwrap();
        assert_ne!(repaired, corrupt);
        let (header, slots) = PrmFile::from_bytes_partial(&repaired);
        assert!(header.is_ok());
        assert!(slots[0].is_ok());

        let _ = std::fs::remove_dir_all(&root);
    }

    // Regression: world baking must also reject a matching bundle hash when a
    // declared slot failed partial parsing, so the model preservation pass
    // cannot retain a corrupt richer bundle.
    #[test]
    fn world_bake_rebuilds_matching_header_with_truncated_declared_slot() {
        let root = unique_temp_dir("world-repairs-truncated-slot");
        let texture_root = root.join("textures");
        let collection = texture_root.join("shared");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&collection).unwrap();

        std::fs::write(collection.join("surface.png"), png_bytes(8, 8)).unwrap();
        std::fs::write(collection.join("surface_s.png"), png_bytes(8, 8)).unwrap();
        std::fs::write(collection.join("surface_e.png"), png_bytes(8, 8)).unwrap();
        let names = ["shared/surface".to_string()];
        let keys = bake_texture_mips(&names, &texture_root, &cache_root).unwrap();
        let cache_path = cache_root.join(format!(
            "{}.prm",
            cache_filename_for_key(&keys["shared/surface"])
        ));

        let mut corrupt = std::fs::read(&cache_path).unwrap();
        corrupt.pop();
        std::fs::write(&cache_path, &corrupt).unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&corrupt);
        assert!(header.is_ok(), "header remains parseable");
        assert!(
            slots.iter().any(Result::is_err),
            "a declared slot must fail partial parsing"
        );

        bake_texture_mips(&names, &texture_root, &cache_root).unwrap();

        let repaired = std::fs::read(&cache_path).unwrap();
        assert_ne!(repaired, corrupt, "the corrupt bundle must be rebuilt");
        let (header, slots) = PrmFile::from_bytes_partial(&repaired);
        let header = header.expect("rebuilt header parses");
        assert!(
            cache_entry_has_valid_declared_slots(&header, &slots),
            "every slot declared by the rebuilt world bundle must parse"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    // Regression: world cache hits accepted array PRMs even though runtime
    // world textures still use the legacy single-layer D2 upload path.
    #[test]
    fn world_bake_rebuilds_multi_layer_cache_entry() {
        let root = unique_temp_dir("world-rebuilds-multi-layer");
        let texture_root = root.join("textures");
        let collection = texture_root.join("shared");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&collection).unwrap();

        std::fs::write(collection.join("surface.png"), png_bytes(4, 4)).unwrap();
        std::fs::write(collection.join("surface_s.png"), png_bytes(4, 4)).unwrap();
        let names = ["shared/surface".to_string()];
        let keys = bake_texture_mips(&names, &texture_root, &cache_root).unwrap();
        let cache_path = cache_root.join(format!(
            "{}.prm",
            cache_filename_for_key(&keys["shared/surface"])
        ));
        let layered = duplicate_prm_layers(&std::fs::read(&cache_path).unwrap(), 2);
        std::fs::write(&cache_path, &layered).unwrap();

        bake_texture_mips(&names, &texture_root, &cache_root).unwrap();

        let rebuilt = std::fs::read(&cache_path).unwrap();
        assert_ne!(rebuilt, layered, "multi-layer world cache must be rebuilt");
        let (header, slots) = PrmFile::from_bytes_partial(&rebuilt);
        let header = header.expect("rebuilt header parses");
        assert_eq!(header.layer_count, 1);
        assert!(cache_entry_has_valid_declared_slots(&header, &slots));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Hand-computed gamma-correct downsample sanity: a uniform sRGB image
    /// down-filters to itself (the filter sums to 1.0 and a constant input
    /// must yield the same constant output, even through the sRGB → linear →
    /// sRGB round-trip). This pins the gamma path and the renormalisation
    /// step against drift simultaneously.
    #[test]
    fn gamma_correct_constant_input_is_invariant() {
        let lut = build_srgb_to_linear_lut();
        // 4×4 uniform mid-grey sRGB.
        let src: Vec<u8> = (0..16).flat_map(|_| [128u8, 128, 128, 255]).collect();
        let payload = build_diffuse_chain(&src, 4, 4, &lut);

        // mip 0 is first 4*4*4 = 64 bytes; mip 1 (2x2) follows; mip 2 (1x1)
        // last 4 bytes. All texels should still read approximately 128 in
        // sRGB (the LUT round-trip introduces ±1 LSB on quantisation).
        // Total length should be 64 + 16 + 4 = 84 bytes.
        assert_eq!(payload.len(), 64 + 16 + 4);
        for chunk in payload.chunks_exact(4) {
            for &c in &chunk[0..3] {
                assert!(
                    (c as i32 - 128).abs() <= 1,
                    "uniform sRGB drifted: got {c}, expected ~128"
                );
            }
            assert_eq!(chunk[3], 255, "alpha should be preserved");
        }
    }

    /// A 2×2 black/white checker downsamples to a 1×1 texel that, in linear
    /// space, equals 0.5. Re-encoded to sRGB this is ~187/255, NOT the naive
    /// byte midpoint ~128/255. This is the load-bearing test for "filter in
    /// linear, not in sRGB".
    #[test]
    fn checker_downsample_uses_gamma_midpoint() {
        let lut = build_srgb_to_linear_lut();
        let src: Vec<u8> = vec![
            0u8, 0, 0, 255, // (0,0) black
            255, 255, 255, 255, // (1,0) white
            255, 255, 255, 255, // (0,1) white
            0, 0, 0, 255, // (1,1) black
        ];
        let payload = build_diffuse_chain(&src, 2, 2, &lut);
        // mip 0 = 16 bytes, mip 1 = 4 bytes (1×1).
        assert_eq!(payload.len(), 16 + 4);
        let last = &payload[16..20];
        // 0.5 linear → sRGB ≈ 187.5/255.
        for &c in &last[0..3] {
            assert!(
                (c as i32 - 187).abs() <= 1,
                "expected ~187, got {c} (naive midpoint 128 would indicate sRGB filtering)"
            );
        }
        assert_eq!(last[3], 255);
    }

    /// The renormalisation helper produces unit-length normals (within 1/127
    /// of 1.0). This pins the per-level renormalise step the BC5 chain feeds
    /// into the encoder. Build a 4×4 normal map of varied directions, decode
    /// to the `[-1, 1]` linear buffer, renormalise to Rgba8, and verify length.
    #[test]
    fn renormalize_to_rgba8_outputs_unit_length() {
        // Build 4×4 unit-length normals in directions clustered around
        // (0, 0, 1) with small tilt — typical surface-normal authoring.
        let mut linear = Vec::with_capacity(4 * 4 * 4);
        for y in 0..4 {
            for x in 0..4 {
                let dx = (x as f32 - 1.5) * 0.2;
                let dy = (y as f32 - 1.5) * 0.2;
                let dz = (1.0f32 - dx * dx - dy * dy).max(0.0).sqrt();
                linear.extend_from_slice(&[dx, dy, dz, 1.0]);
            }
        }

        let rgba8 = renormalize_to_rgba8(&linear);
        assert_eq!(rgba8.len(), 4 * 4 * 4);

        for chunk in rgba8.chunks_exact(4) {
            let nx = (chunk[0] as f32) / 255.0 * 2.0 - 1.0;
            let ny = (chunk[1] as f32) / 255.0 * 2.0 - 1.0;
            let nz = (chunk[2] as f32) / 255.0 * 2.0 - 1.0;
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            assert!(
                (len - 1.0).abs() <= 1.0 / 127.0,
                "non-unit normal: len = {len}"
            );
        }
    }

    /// Helper: build a synthetic tangent-space normal map of `w × h` texels
    /// tilting gently away from (0, 0, 1), encoded as Rgba8 (typical authoring).
    fn synthetic_normal_rgba(w: u32, h: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let nx = (x as f32 / (w.max(2) - 1) as f32 - 0.5) * 0.8;
                let ny = (y as f32 / (h.max(2) - 1) as f32 - 0.5) * 0.8;
                let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
                let len = (nx * nx + ny * ny + nz * nz).sqrt();
                let r = ((nx / len * 0.5 + 0.5) * 255.0).round() as u8;
                let g = ((ny / len * 0.5 + 0.5) * 255.0).round() as u8;
                let b = ((nz / len * 0.5 + 0.5) * 255.0).round() as u8;
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }
        rgba
    }

    /// SEAM-CROSSING: the baker's BC5 normal output must satisfy the format
    /// reader's contract. Bake a synthetic 8×8 normal level into a BC5 normal
    /// `PrmSlot`, wrap it in a `PrmFile`, serialize with `to_bytes`, and parse
    /// back with `from_bytes_partial`. The normal slot must parse WITHOUT error
    /// (no `LevelCountMismatch` / `PayloadBytesMismatch`), with the truncated
    /// `level_count` and `Bc5RgUnorm` format. 8×8 → bc5_level_count == 2.
    #[test]
    fn baked_bc5_normal_slot_round_trips_through_reader() {
        let (w, h) = (8u32, 8u32);
        let rgba = synthetic_normal_rgba(w, h);
        let payload = build_normal_bc5_chain(&rgba, w, h);

        // The baked payload must be exactly the size the reader expects.
        let expected_bytes = expected_payload_bytes_pub(PrmFormat::Bc5RgUnorm, w as u16, h as u16);
        assert_eq!(
            payload.len() as u32,
            expected_bytes,
            "BC5 payload size must match the reader's expected_payload_bytes"
        );

        let level_count = bc5_level_count(w as u16, h as u16);
        assert_eq!(level_count, 2, "8×8 BC5 truncates to 2 levels (8×8, 4×4)");

        let slot = PrmSlot {
            format: PrmFormat::Bc5RgUnorm,
            width: w as u16,
            height: h as u16,
            level_count,
            payload,
        };
        let file = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: PrmSlots::NORMAL,
                bundle_hash: [0x42; 32],
                total_body_bytes: 0,
                layer_count: 1,
            },
            slots: [None, None, Some(slot), None],
        };

        let bytes = file.to_bytes().expect("BC5 normal slot should serialize");
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        header.expect("header should parse");
        let parsed = slots[2]
            .as_ref()
            .expect("normal slot must parse without LevelCountMismatch/PayloadBytesMismatch");
        assert_eq!(parsed.format, PrmFormat::Bc5RgUnorm);
        assert_eq!(parsed.level_count, level_count);
        assert_eq!(parsed.width, w as u16);
        assert_eq!(parsed.height, h as u16);
    }

    /// SEAM-CROSSING (padding case): a non-power-of-two source (12×12) exercises
    /// the edge-replication padding path — level 1 is 6×6, which is ≥ 4 but not
    /// a multiple of 4, so the baker pads it to 8×8 before BC5 encoding. The
    /// reader sizes that level with ceil(6/4)*ceil(6/4)=4 blocks, so the baked
    /// payload must still match. 12×12 → levels 12×12 and 6×6 (bc5_level_count 2).
    #[test]
    fn baked_bc5_normal_slot_with_padding_round_trips() {
        let (w, h) = (12u32, 12u32);
        let rgba = synthetic_normal_rgba(w, h);
        let payload = build_normal_bc5_chain(&rgba, w, h);

        let expected_bytes = expected_payload_bytes_pub(PrmFormat::Bc5RgUnorm, w as u16, h as u16);
        assert_eq!(
            payload.len() as u32,
            expected_bytes,
            "padded BC5 payload size must match the reader's expected_payload_bytes"
        );

        let level_count = bc5_level_count(w as u16, h as u16);
        assert_eq!(
            level_count, 2,
            "12×12 BC5 truncates to 2 levels (12×12, 6×6)"
        );

        let slot = PrmSlot {
            format: PrmFormat::Bc5RgUnorm,
            width: w as u16,
            height: h as u16,
            level_count,
            payload,
        };
        let file = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: PrmSlots::NORMAL,
                bundle_hash: [0x7; 32],
                total_body_bytes: 0,
                layer_count: 1,
            },
            slots: [None, None, Some(slot), None],
        };

        let bytes = file
            .to_bytes()
            .expect("padded BC5 normal slot should serialize");
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        header.expect("header should parse");
        let parsed = slots[2]
            .as_ref()
            .expect("padded normal slot must parse without payload-size errors");
        assert_eq!(parsed.format, PrmFormat::Bc5RgUnorm);
        assert_eq!(parsed.level_count, level_count);
    }

    /// Independent restatement of the reader's BC5 payload-size contract:
    /// `bc5_level_count` levels, each `ceil(w/4) * ceil(h/4) * 16` bytes. The
    /// round-trip tests assert the baker's emitted payload matches this, pinning
    /// the seam between what `prl-build` writes and what the reader expects.
    fn expected_payload_bytes_pub(format: PrmFormat, width: u16, height: u16) -> u32 {
        assert_eq!(format, PrmFormat::Bc5RgUnorm);
        let level_count = bc5_level_count(width, height);
        let mut total = 0u32;
        for n in 0..level_count {
            let w_n = ((width as u32) >> n).max(1);
            let h_n = ((height as u32) >> n).max(1);
            total += w_n.div_ceil(4) * h_n.div_ceil(4) * 16;
        }
        total
    }

    /// A sub-4×4 normal source has no valid BC5 level: `bc5_level_count` is 0
    /// and the chain builder emits an empty payload. `bake_texture_mips` keys
    /// its drop-the-slot decision on exactly this `level_count == 0` condition,
    /// so emitting the slot would write a zero-level payload the runtime cannot
    /// upload. Pinning the precondition keeps the baker's guard honest.
    #[test]
    fn sub_four_normal_source_has_no_bc5_level() {
        for (w, h) in [(2u32, 2u32), (3, 8), (4, 2)] {
            assert_eq!(
                bc5_level_count(w as u16, h as u16),
                0,
                "{w}x{h} should have no BC5 level (needs both dims ≥ 4)"
            );
            let rgba = synthetic_normal_rgba(w, h);
            assert!(
                build_normal_bc5_chain(&rgba, w, h).is_empty(),
                "{w}x{h} normal chain must be empty so the baker drops the slot"
            );
        }
    }

    #[test]
    fn mip_level_count_matches_floor_log2_plus_one() {
        assert_eq!(expected_level_count(1, 1), 1);
        assert_eq!(expected_level_count(2, 1), 2);
        assert_eq!(expected_level_count(4, 4), 3);
        assert_eq!(expected_level_count(8, 4), 4);
        assert_eq!(expected_level_count(1024, 1024), 11);
    }

    /// Bundle hash includes only present slots, in canonical order. Changing
    /// the slot order in source bytes (e.g. swapping specular and normal)
    /// must yield a different hash because the per-slot prefix byte
    /// (0x00/0x01/0x02) tags which slot the bytes belong to.
    #[test]
    fn bundle_hash_distinguishes_slot_assignment() {
        let a = bundle_hash_for(None, Some(b"alpha"), Some(b"beta"), None, None);
        let b = bundle_hash_for(None, Some(b"beta"), Some(b"alpha"), None, None);
        assert_ne!(a, b);
    }

    #[test]
    fn bake_emissive_sibling_writes_srgb_fourth_slot_and_changes_bundle_hash() {
        let root = unique_temp_dir("emissive-slot");
        let texture_root = root.join("textures");
        let collection = texture_root.join("neon");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&collection).unwrap();
        std::fs::write(collection.join("neon_panel.png"), png_bytes(4, 4)).unwrap();
        std::fs::write(collection.join("neon_panel_e.png"), png_bytes(4, 4)).unwrap();

        let keys = bake_texture_mips(&["neon/neon_panel".to_string()], &texture_root, &cache_root)
            .unwrap();
        let key = keys["neon/neon_panel"];
        let bytes = std::fs::read(cache_root.join(format!("{}.prm", cache_filename_for_key(&key))))
            .unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let header = header.expect("emissive bundle parses");
        assert!(header.slot_mask.contains(PrmSlots::EMISSIVE));
        assert_eq!(
            slots[3].as_ref().expect("emissive slot parses").format,
            PrmFormat::Rgba8UnormSrgb,
        );

        let diffuse = std::fs::read(collection.join("neon_panel.png")).unwrap();
        let emissive = std::fs::read(collection.join("neon_panel_e.png")).unwrap();
        assert_ne!(
            header.bundle_hash,
            bundle_hash_for(Some(&diffuse), None, None, None, None),
            "the emissive sibling must participate in the bundle hash",
        );
        assert_eq!(
            header.bundle_hash,
            bundle_hash_for(Some(&diffuse), None, None, Some(&emissive), None),
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Regression: diffuse-only filename addressing collapsed materials whose
    // diffuse bytes matched but whose emissive siblings differed.
    #[test]
    fn same_diffuse_with_different_emissive_writes_distinct_runtime_bundles() {
        let root = unique_temp_dir("distinct-emissive-bundles");
        let texture_root = root.join("textures");
        let alpha = texture_root.join("alpha");
        let beta = texture_root.join("beta");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::create_dir_all(&beta).unwrap();

        let diffuse = solid_png_bytes(4, 4, [40, 50, 60, 255]);
        std::fs::write(alpha.join("panel.png"), &diffuse).unwrap();
        std::fs::write(beta.join("panel.png"), &diffuse).unwrap();
        std::fs::write(
            alpha.join("panel_e.png"),
            solid_png_bytes(4, 4, [255, 0, 0, 255]),
        )
        .unwrap();
        std::fs::write(
            beta.join("panel_e.png"),
            solid_png_bytes(4, 4, [0, 0, 255, 255]),
        )
        .unwrap();

        let names = ["alpha/panel".to_string(), "beta/panel".to_string()];
        let keys = bake_texture_mips(&names, &texture_root, &cache_root).unwrap();
        let alpha_key = keys["alpha/panel"];
        let beta_key = keys["beta/panel"];
        assert_ne!(
            alpha_key, beta_key,
            "the complete optional bundle must participate in its filename key"
        );

        let load_bundle = |key: [u8; 32]| {
            let path = cache_root.join(format!("{}.prm", cache_filename_for_key(&key)));
            let bytes = std::fs::read(path).expect("key must address a baked runtime sidecar");
            let (header, slots) = PrmFile::from_bytes_partial(&bytes);
            let header = header.expect("runtime header parses");
            assert_eq!(header.slot_mask, PrmSlots::DIFFUSE | PrmSlots::EMISSIVE);
            assert!(slots[0].is_ok(), "diffuse slot parses");
            assert!(slots[3].is_ok(), "emissive slot parses");
            (
                slots[0].as_ref().unwrap().to_owned(),
                slots[3].as_ref().unwrap().to_owned(),
            )
        };
        let (alpha_diffuse, alpha_emissive) = load_bundle(alpha_key);
        let (beta_diffuse, beta_emissive) = load_bundle(beta_key);
        assert_eq!(alpha_diffuse, beta_diffuse);
        assert_ne!(alpha_emissive, beta_emissive);

        let _ = std::fs::remove_dir_all(&root);
    }

    // Regression: resolving siblings from the diffuse fallback base dropped a
    // qualified emissive-only material when no diffuse PNG existed.
    #[test]
    fn qualified_emissive_only_material_resolves_from_requested_collection() {
        let root = unique_temp_dir("qualified-emissive-only");
        let texture_root = root.join("textures");
        let alpha = texture_root.join("alpha");
        let beta = texture_root.join("beta");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::create_dir_all(&beta).unwrap();
        std::fs::write(
            alpha.join("panel_e.png"),
            solid_png_bytes(2, 2, [240, 20, 10, 255]),
        )
        .unwrap();
        std::fs::write(
            beta.join("panel_e.png"),
            solid_png_bytes(2, 2, [10, 20, 240, 255]),
        )
        .unwrap();

        let keys =
            bake_texture_mips(&["alpha/panel".to_string()], &texture_root, &cache_root).unwrap();
        let key = keys["alpha/panel"];
        assert_ne!(key, [0u8; 32]);
        let path = cache_root.join(format!("{}.prm", cache_filename_for_key(&key)));
        let bytes = std::fs::read(path).unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        assert_eq!(header.unwrap().slot_mask, PrmSlots::EMISSIVE);
        assert!(matches!(&slots[0], Err(PrmReadError::NotPresent)));
        assert_eq!(
            &slots[3]
                .as_ref()
                .expect("qualified emissive parses")
                .payload[0..4],
            &[240, 20, 10, 255],
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    // Regression: a mismatched emissive image reached runtime even though the
    // authoring contract requires it to share the diffuse dimensions.
    #[test]
    fn emissive_dimensions_must_match_diffuse_at_compile_time() {
        let root = unique_temp_dir("emissive-dimension-mismatch");
        let texture_root = root.join("textures");
        let collection = texture_root.join("neon");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&collection).unwrap();
        std::fs::write(collection.join("panel.png"), png_bytes(4, 4)).unwrap();
        std::fs::write(collection.join("panel_e.png"), png_bytes(2, 4)).unwrap();

        let error = bake_texture_mips(&["neon/panel".to_string()], &texture_root, &cache_root)
            .expect_err("mismatched emissive dimensions must fail the map build");
        let message = error.to_string();
        assert!(message.contains("panel_e.png"));
        assert!(message.contains("2x4"));
        assert!(message.contains("panel.png"));
        assert!(message.contains("4x4"));
        assert!(message.contains("_e.png dimensions must match diffuse"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A height-only material is a real bundle: `_h` joins the suffix list the
    /// resolver uses to decide a qualified base exists, so it neither falls
    /// back to a bare stem nor collapses to the all-absent zero key.
    #[test]
    fn qualified_height_only_material_resolves_and_bakes() {
        let root = unique_temp_dir("qualified-height-only");
        let texture_root = root.join("textures");
        let alpha = texture_root.join("alpha");
        let beta = texture_root.join("beta");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::create_dir_all(&beta).unwrap();
        std::fs::write(
            alpha.join("cobble_h.png"),
            solid_png_bytes(2, 2, [10, 10, 10, 255]),
        )
        .unwrap();
        // A same-stem diffuse in another collection must not be picked up.
        std::fs::write(beta.join("cobble.png"), png_bytes(2, 2)).unwrap();

        let map = build_name_to_path_map(&texture_root);
        let paths = resolve_texture_bundle_paths(&map, "alpha/cobble");
        assert_eq!(paths.height, Some(alpha.join("cobble_h.png")));
        assert_eq!(paths.diffuse, None, "the bare-stem fallback must stay off");

        let keys =
            bake_texture_mips(&["alpha/cobble".to_string()], &texture_root, &cache_root).unwrap();
        let key = keys["alpha/cobble"];
        assert_ne!(
            key, [0u8; 32],
            "a height-only bundle is not 'no source PNG'"
        );

        let path = cache_root.join(format!("{}.prm", cache_filename_for_key(&key)));
        let (header, slots) = PrmFile::from_bytes_partial(&std::fs::read(&path).unwrap());
        assert_eq!(header.expect("header parses").slot_mask, PrmSlots::SPECULAR);
        let slot = slots[1].as_ref().expect("surface-map slot parses");
        assert_eq!(slot.format, PrmFormat::Rg8Unorm);
        for texel in slot.payload[..2 * 2 * 2].chunks_exact(2) {
            assert_eq!(texel[0], 0);
            assert_eq!(texel[1], 245, "depth = 255 - 10");
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The bundle-hash inputs for a height-less bundle must be *exactly* what
    /// they were before the height sibling existed. This replays the
    /// pre-surface-depth algorithm — 4-bit mask, tags 0x00..0x03, nothing else
    /// — and demands equality across every combination of present slots.
    ///
    /// This is the no-rebake property: `baked/materials/` is content-addressed
    /// on these bytes, so a single changed byte here would silently invalidate
    /// and re-bake every `.prm` already on disk.
    #[test]
    fn height_absent_bundle_hash_matches_the_pre_height_algorithm() {
        fn pre_height_bundle_hash(
            diffuse: Option<&[u8]>,
            specular: Option<&[u8]>,
            normal: Option<&[u8]>,
            emissive: Option<&[u8]>,
        ) -> [u8; 32] {
            let mut mask: u8 = 0;
            if diffuse.is_some() {
                mask |= 0b001;
            }
            if specular.is_some() {
                mask |= 0b010;
            }
            if normal.is_some() {
                mask |= 0b100;
            }
            if emissive.is_some() {
                mask |= 0b1000;
            }
            let mut h = blake3::Hasher::new();
            h.update(&[mask]);
            for (tag, bytes) in [
                (0x00u8, diffuse),
                (0x01, specular),
                (0x02, normal),
                (0x03, emissive),
            ] {
                if let Some(b) = bytes {
                    h.update(&[tag]);
                    h.update(b);
                }
            }
            *h.finalize().as_bytes()
        }

        let d: &[u8] = b"diffuse-png";
        let sp: &[u8] = b"specular-png";
        let n: &[u8] = b"normal-png";
        let e: &[u8] = b"emissive-png";

        for bits in 0u8..16 {
            let diffuse = (bits & 1 != 0).then_some(d);
            let specular = (bits & 2 != 0).then_some(sp);
            let normal = (bits & 4 != 0).then_some(n);
            let emissive = (bits & 8 != 0).then_some(e);
            assert_eq!(
                bundle_hash_for(diffuse, specular, normal, emissive, None),
                pre_height_bundle_hash(diffuse, specular, normal, emissive),
                "height-less bundle {bits:04b} must hash exactly as it did before \
                 the height sibling existed"
            );
        }
    }

    /// The same promise for the `.prm` filename stem: without a height
    /// sibling, every bundle keeps the address it already has on disk, so no
    /// existing sidecar is orphaned or rewritten.
    #[test]
    fn height_absent_filename_key_is_unchanged() {
        let d: &[u8] = b"diffuse-png";
        let sp: &[u8] = b"specular-png";
        let n: &[u8] = b"normal-png";
        let e: &[u8] = b"emissive-png";

        // Diffuse-only keeps the bare content hash that model loading derives
        // at runtime — the one address that is not a bundle hash.
        assert_eq!(
            filename_key_for(Some(d), None, None, None, None),
            *blake3::hash(d).as_bytes(),
        );
        // Every multi-slot height-less bundle keeps the bundle-hash address.
        for bits in 0u8..16 {
            let diffuse = (bits & 1 != 0).then_some(d);
            let specular = (bits & 2 != 0).then_some(sp);
            let normal = (bits & 4 != 0).then_some(n);
            let emissive = (bits & 8 != 0).then_some(e);
            if bits.count_ones() > 1 {
                assert_eq!(
                    filename_key_for(diffuse, specular, normal, emissive, None),
                    bundle_hash_for(diffuse, specular, normal, emissive, None),
                    "height-less bundle {bits:04b}"
                );
            }
        }
        // And a present height sibling does move the address — otherwise two
        // different bundles would share one `.prm`.
        assert_ne!(
            filename_key_for(Some(d), Some(sp), None, None, Some(b"height-png")),
            filename_key_for(Some(d), Some(sp), None, None, None),
        );
    }

    /// End-to-end no-rebake proof. A `.prm` written by the pre-surface-depth
    /// baker — placed at its historical address, carrying its historical
    /// bundle hash and a payload nothing here could reproduce — must be
    /// recognised as a cache hit and left byte-for-byte alone.
    #[test]
    fn heightless_bundle_is_a_cache_hit_and_is_not_rewritten() {
        let root = unique_temp_dir("heightless-no-rebake");
        let texture_root = root.join("textures");
        let collection = texture_root.join("stone");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&collection).unwrap();

        let diffuse = png_bytes(4, 4);
        let specular = solid_png_bytes(4, 4, [90, 90, 90, 255]);
        std::fs::write(collection.join("cobble.png"), &diffuse).unwrap();
        std::fs::write(collection.join("cobble_s.png"), &specular).unwrap();

        let key = filename_key_for(Some(&diffuse), Some(&specular), None, None, None);
        // A deliberately wrong-but-structurally-valid payload: if the baker
        // rebaked, these bytes would be replaced by real mip chains.
        let sentinel = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: PrmSlots::DIFFUSE | PrmSlots::SPECULAR,
                bundle_hash: bundle_hash_for(Some(&diffuse), Some(&specular), None, None, None),
                total_body_bytes: 0,
                layer_count: 1,
            },
            slots: [
                Some(PrmSlot {
                    format: PrmFormat::Rgba8UnormSrgb,
                    width: 1,
                    height: 1,
                    level_count: 1,
                    payload: vec![0xDE, 0xAD, 0xBE, 0xEF],
                }),
                Some(PrmSlot {
                    format: PrmFormat::R8Unorm,
                    width: 1,
                    height: 1,
                    level_count: 1,
                    payload: vec![0x5A],
                }),
                None,
                None,
            ],
        }
        .to_bytes()
        .unwrap();
        let cache_path = cache_root.join(format!("{}.prm", cache_filename_for_key(&key)));
        atomic_write(&cache_path, &sentinel).unwrap();

        let keys =
            bake_texture_mips(&["stone/cobble".to_string()], &texture_root, &cache_root).unwrap();

        assert_eq!(keys["stone/cobble"], key, "the address must not move");
        assert_eq!(
            std::fs::read(&cache_path).unwrap(),
            sentinel,
            "a height-less bundle must not rebake"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A height sibling turns the specular slot into the two-channel surface
    /// map: R is the authored specular, G is depth — the *inverse* of the
    /// authored height, because the stored channel measures how far the
    /// surface is carved below its true plane.
    #[test]
    fn height_sibling_bakes_inverted_depth_into_the_specular_green_channel() {
        let root = unique_temp_dir("height-inverts-to-depth");
        let texture_root = root.join("textures");
        let collection = texture_root.join("stone");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&collection).unwrap();

        std::fs::write(collection.join("cobble.png"), png_bytes(4, 4)).unwrap();
        std::fs::write(
            collection.join("cobble_s.png"),
            solid_png_bytes(4, 4, [200, 0, 0, 255]),
        )
        .unwrap();
        // Authored height: 60 = mostly recessed. Depth must read 255 - 60.
        std::fs::write(
            collection.join("cobble_h.png"),
            solid_png_bytes(4, 4, [60, 60, 60, 255]),
        )
        .unwrap();

        let keys =
            bake_texture_mips(&["stone/cobble".to_string()], &texture_root, &cache_root).unwrap();
        let path = cache_root.join(format!(
            "{}.prm",
            cache_filename_for_key(&keys["stone/cobble"])
        ));
        let bytes = std::fs::read(&path).unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let header = header.expect("surface-map header parses");
        assert!(
            header.slot_mask.contains(PrmSlots::SPECULAR),
            "the SPECULAR bit carries the surface map"
        );
        // Reserved slot-mask bits stay reserved: height is not a fifth slot.
        assert_eq!(
            header.slot_mask,
            PrmSlots::DIFFUSE | PrmSlots::SPECULAR,
            "height must not claim a slot-mask bit of its own"
        );

        let slot = slots[1].as_ref().expect("surface-map slot parses");
        assert_eq!(slot.format, PrmFormat::Rg8Unorm);
        assert_eq!((slot.width, slot.height), (4, 4));
        assert_eq!(slot.level_count, expected_level_count(4, 4));

        // Mip 0 is the source interleave, untouched by filtering.
        for texel in slot.payload[..4 * 4 * 2].chunks_exact(2) {
            assert_eq!(texel[0], 200, "R keeps the authored specular");
            assert_eq!(texel[1], 255 - 60, "G is depth = 255 - authored height");
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A height sibling with no `_s.png` still bakes the surface map; the
    /// specular channel reads zero, matching the shared black placeholder a
    /// specular-less material gets today.
    #[test]
    fn height_without_specular_bakes_zero_red_and_still_sets_the_specular_bit() {
        let root = unique_temp_dir("height-without-specular");
        let texture_root = root.join("textures");
        let collection = texture_root.join("stone");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&collection).unwrap();

        std::fs::write(collection.join("cobble.png"), png_bytes(4, 4)).unwrap();
        std::fs::write(
            collection.join("cobble_h.png"),
            solid_png_bytes(4, 4, [255, 255, 255, 255]),
        )
        .unwrap();

        let keys =
            bake_texture_mips(&["stone/cobble".to_string()], &texture_root, &cache_root).unwrap();
        let path = cache_root.join(format!(
            "{}.prm",
            cache_filename_for_key(&keys["stone/cobble"])
        ));
        let (header, slots) = PrmFile::from_bytes_partial(&std::fs::read(&path).unwrap());
        assert!(
            header
                .expect("header parses")
                .slot_mask
                .contains(PrmSlots::SPECULAR),
            "the SPECULAR bit is set if EITHER _s or _h is present"
        );
        let slot = slots[1].as_ref().expect("surface-map slot parses");
        assert_eq!(slot.format, PrmFormat::Rg8Unorm);
        for texel in slot.payload[..4 * 4 * 2].chunks_exact(2) {
            assert_eq!(texel[0], 0, "absent _s.png reads as zero specular");
            // Fully white height (raised) carves to zero depth: flat.
            assert_eq!(texel[1], 0, "white height is depth 0 — a true no-op");
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Without a height sibling the specular slot must stay exactly what it
    /// was: single-channel `R8Unorm`, same payload bytes. Adding the format
    /// must not perturb any material that does not use it.
    #[test]
    fn specular_without_height_still_bakes_single_channel_r8() {
        let root = unique_temp_dir("specular-stays-r8");
        let texture_root = root.join("textures");
        let collection = texture_root.join("stone");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&collection).unwrap();

        std::fs::write(collection.join("cobble.png"), png_bytes(4, 4)).unwrap();
        std::fs::write(
            collection.join("cobble_s.png"),
            solid_png_bytes(4, 4, [200, 0, 0, 255]),
        )
        .unwrap();

        let keys =
            bake_texture_mips(&["stone/cobble".to_string()], &texture_root, &cache_root).unwrap();
        let path = cache_root.join(format!(
            "{}.prm",
            cache_filename_for_key(&keys["stone/cobble"])
        ));
        let (_, slots) = PrmFile::from_bytes_partial(&std::fs::read(&path).unwrap());
        let slot = slots[1].as_ref().expect("specular slot parses");
        assert_eq!(slot.format, PrmFormat::R8Unorm);
        assert_eq!(
            slot.payload,
            build_specular_chain(&[200u8; 16], 4, 4),
            "the R8 chain is unchanged by the surface-map addition"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// `_h.png` shares one texture with `_s.png` and is marched against the
    /// diffuse, so a dimension mismatch is a hard compile-time bail rather
    /// than a silently wrong surface.
    #[test]
    fn height_dimensions_must_match_diffuse_at_compile_time() {
        let root = unique_temp_dir("height-dimension-mismatch-diffuse");
        let texture_root = root.join("textures");
        let collection = texture_root.join("stone");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&collection).unwrap();
        std::fs::write(collection.join("cobble.png"), png_bytes(4, 4)).unwrap();
        std::fs::write(collection.join("cobble_h.png"), png_bytes(2, 4)).unwrap();

        let error = bake_texture_mips(&["stone/cobble".to_string()], &texture_root, &cache_root)
            .expect_err("mismatched height dimensions must fail the map build");
        let message = error.to_string();
        assert!(message.contains("cobble_h.png"), "{message}");
        assert!(message.contains("2x4"), "{message}");
        assert!(message.contains("4x4"), "{message}");
        assert!(
            message.contains("_h.png dimensions must match diffuse"),
            "{message}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The same bail when the mismatch is against `_s.png`: the two siblings
    /// are interleaved into one `Rg8Unorm` texture, so unequal dimensions
    /// have no meaning at all.
    #[test]
    fn height_dimensions_must_match_specular_at_compile_time() {
        let root = unique_temp_dir("height-dimension-mismatch-specular");
        let texture_root = root.join("textures");
        let collection = texture_root.join("stone");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&collection).unwrap();
        std::fs::write(collection.join("cobble_s.png"), png_bytes(2, 2)).unwrap();
        std::fs::write(collection.join("cobble_h.png"), png_bytes(4, 4)).unwrap();

        let error = bake_texture_mips(&["stone/cobble".to_string()], &texture_root, &cache_root)
            .expect_err("a height/specular dimension mismatch must fail the map build");
        let message = error.to_string();
        assert!(message.contains("cobble_h.png"), "{message}");
        assert!(message.contains("cobble_s.png"), "{message}");
        assert!(
            message.contains("must have identical dimensions"),
            "{message}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Both channels of the surface map go through the same
    /// Mitchell-Netravali path the single-channel specular chain uses, so the
    /// R channel of a surface map must equal the `R8Unorm` chain built from
    /// the same R data, level for level.
    #[test]
    fn surface_chain_filters_red_identically_to_the_specular_chain() {
        let (w, h) = (8u32, 8u32);
        // High-contrast checkerboards on both channels: this is where
        // Mitchell-Netravali's negative lobes overshoot hardest.
        let mut r8 = Vec::new();
        let mut rg = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let r = if (x + y) % 2 == 0 { 255u8 } else { 0 };
                let g = if x % 2 == 0 { 0u8 } else { 255 };
                r8.push(r);
                rg.push(r);
                rg.push(g);
            }
        }

        let specular_chain = build_specular_chain_impl(&r8, w, h);
        let surface_chain = build_surface_chain_impl(&rg, w, h);

        assert_eq!(surface_chain.len(), specular_chain.len() * 2);
        let reds: Vec<u8> = surface_chain.chunks_exact(2).map(|t| t[0]).collect();
        assert_eq!(
            reds, specular_chain,
            "the surface map's R channel must filter exactly as R8 specular does"
        );
    }

    /// Mitchell-Netravali (B = C = 1/3) has negative lobes that overshoot
    /// outside [0, 1]. A one-texel bright spike is the worst case: the ringing
    /// around it must clamp to 0 rather than wrap to 255.
    #[test]
    fn surface_chain_clamps_filter_overshoot_per_texel() {
        let (w, h) = (8u32, 8u32);
        let mut rg = vec![0u8; (w * h) as usize * 2];
        // Single white spike in both channels.
        let centre = ((3 * w + 3) * 2) as usize;
        rg[centre] = 255;
        rg[centre + 1] = 255;

        let chain = build_surface_chain_impl(&rg, w, h);
        // Mip 0 is a verbatim copy.
        assert_eq!(&chain[..(w * h) as usize * 2], &rg[..]);
        // Ringing is negative around the spike; unclamped it would wrap high.
        // Every byte outside mip 0 must stay well below the spike.
        let mip_rest = &chain[(w * h) as usize * 2..];
        assert!(!mip_rest.is_empty(), "an 8x8 source has mips below level 0");
        assert!(
            mip_rest.iter().all(|&b| b < 200),
            "clamped ringing must not wrap to the top of the range"
        );
    }

    /// Filename key falls back to specular when diffuse is missing, but the
    /// 0x01 prefix prevents a collision with a diffuse PNG whose bytes
    /// happen to equal the specular bytes.
    #[test]
    fn filename_key_specular_fallback_does_not_collide_with_diffuse() {
        let bytes: &[u8] = b"identical-payload";
        let diff_only = filename_key_for(Some(bytes), None, None, None, None);
        let spec_only = filename_key_for(None, Some(bytes), None, None, None);
        assert_ne!(diff_only, spec_only);
    }

    #[test]
    fn all_absent_key_is_zero() {
        assert_eq!(filename_key_for(None, None, None, None, None), [0u8; 32]);
    }

    /// Resolver coverage: a collection subdir with a diffuse and all optional
    /// siblings must resolve through every name form TrenchBroom might emit.
    #[test]
    fn resolver_matches_bare_qualified_and_root_inclusive_forms() {
        let root = std::env::temp_dir().join(format!(
            "prl-build-resolver-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let collection = root.join("50-free-textures");
        std::fs::create_dir_all(&collection).unwrap();

        // Minimal valid 1×1 PNGs (content is irrelevant to path resolution).
        let png_bytes = |label: u8| -> Vec<u8> {
            let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([label, 0, 0, 255]));
            let mut buf = std::io::Cursor::new(Vec::new());
            img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
            buf.into_inner()
        };
        let diff = collection.join("concrete_pavement_036.png");
        let spec = collection.join("concrete_pavement_036_s.png");
        let norm = collection.join("concrete_pavement_036_n.png");
        let emissive = collection.join("concrete_pavement_036_e.png");
        let height = collection.join("concrete_pavement_036_h.png");
        std::fs::write(&diff, png_bytes(1)).unwrap();
        std::fs::write(&spec, png_bytes(2)).unwrap();
        std::fs::write(&norm, png_bytes(3)).unwrap();
        std::fs::write(&emissive, png_bytes(4)).unwrap();
        std::fs::write(&height, png_bytes(5)).unwrap();

        let map = build_name_to_path_map(&root);

        // Diffuse resolves via the relative key and the bare-stem alias.
        assert_eq!(
            map.get("50-free-textures/concrete_pavement_036"),
            Some(&diff)
        );
        assert_eq!(map.get("concrete_pavement_036"), Some(&diff));

        // Siblings resolve under the relative collection key.
        assert_eq!(
            map.get("50-free-textures/concrete_pavement_036_s"),
            Some(&spec)
        );
        assert_eq!(
            map.get("50-free-textures/concrete_pavement_036_n"),
            Some(&norm)
        );
        assert_eq!(
            map.get("50-free-textures/concrete_pavement_036_e"),
            Some(&emissive)
        );
        assert_eq!(
            map.get("50-free-textures/concrete_pavement_036_h"),
            Some(&height)
        );

        // All three incoming name forms normalize to the relative key and
        // therefore resolve to the diffuse and its siblings.
        for incoming in [
            "concrete_pavement_036",
            "50-free-textures/concrete_pavement_036",
            "textures/50-free-textures/concrete_pavement_036",
            // Backslash + mixed case must also normalize.
            "Textures\\50-Free-Textures\\Concrete_Pavement_036",
        ] {
            let normalized = normalize_map_texture_name(incoming);
            let paths = resolve_texture_bundle_paths(&map, &normalized);
            assert_eq!(
                paths.diffuse.as_ref(),
                Some(&diff),
                "diffuse for '{incoming}' should resolve to {}",
                diff.display()
            );
            assert_eq!(
                paths.specular.as_ref(),
                Some(&spec),
                "specular sibling for '{incoming}' should resolve from the same collection"
            );
            assert_eq!(
                paths.normal.as_ref(),
                Some(&norm),
                "normal sibling for '{incoming}' should resolve from the same collection"
            );
            assert_eq!(
                paths.emissive.as_ref(),
                Some(&emissive),
                "emissive sibling for '{incoming}' should resolve from the same collection"
            );
            assert_eq!(
                paths.height.as_ref(),
                Some(&height),
                "height sibling for '{incoming}' should resolve from the same collection"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A space-containing collection name normalizes to the lowercased,
    /// relative, space-preserving key that `build_name_to_path_map` indexes on
    /// disk.
    #[test]
    fn normalize_preserves_spaces_in_collection_name() {
        let normalized =
            normalize_map_texture_name("Level Eleven Games/Metal-Panel-002_Section-001-3");
        assert_eq!(
            normalized, "level eleven games/metal-panel-002_section-001-3",
            "spaces must survive normalization to match the on-disk relative key"
        );
    }

    /// Bare-stem alias is disabled when the same stem exists in two
    /// collections: only the collection-qualified keys resolve, and the bare
    /// stem misses (no silent wrong-collection match).
    #[test]
    fn resolver_drops_ambiguous_bare_stem_alias() {
        let root = std::env::temp_dir().join(format!(
            "prl-build-resolver-ambig-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let coll_a = root.join("alpha");
        let coll_b = root.join("beta");
        std::fs::create_dir_all(&coll_a).unwrap();
        std::fs::create_dir_all(&coll_b).unwrap();

        let png = {
            let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([9, 0, 0, 255]));
            let mut buf = std::io::Cursor::new(Vec::new());
            img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
            buf.into_inner()
        };
        let a = coll_a.join("metal_panel.png");
        let b = coll_b.join("metal_panel.png");
        std::fs::write(&a, &png).unwrap();
        std::fs::write(&b, &png).unwrap();

        let map = build_name_to_path_map(&root);

        // Both qualified keys present and unambiguous.
        assert_eq!(map.get("alpha/metal_panel"), Some(&a));
        assert_eq!(map.get("beta/metal_panel"), Some(&b));
        // Bare-stem alias dropped: the bare name misses.
        assert_eq!(map.get("metal_panel"), None);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Pins the bundle-hash wire format to a known byte sequence. A diffuse-only
    /// bundle with `mask = PrmSlots::DIFFUSE.bits()` (0x01) and PNG bytes `[0xAA, 0xBB]`
    /// must hash the byte stream `[0x01, 0x00, 0xAA, 0xBB]` (slot_mask byte, then
    /// bit_index_byte 0x00 for diffuse, then the two PNG bytes). Any refactor that
    /// reorders those prefix bytes or drops the slot_mask byte would silently
    /// invalidate every existing `.prm` cache — this test catches that.
    #[test]
    fn bundle_hash_for_pins_wire_format() {
        // Computed offline: blake3([0x01, 0x00, 0xAA, 0xBB])
        let expected: [u8; 32] = [
            0x73, 0x7e, 0xb8, 0x89, 0x4d, 0xa5, 0x47, 0x24, 0x8d, 0xb5, 0xd4, 0x9e, 0xdb, 0xd5,
            0xd0, 0x01, 0x49, 0xe8, 0x68, 0xc3, 0x89, 0xd5, 0xa9, 0xcb, 0x57, 0xc8, 0xb2, 0x04,
            0x7c, 0xc1, 0x7b, 0xbe,
        ];
        let got = bundle_hash_for(Some(&[0xAAu8, 0xBB]), None, None, None, None);
        assert_eq!(got, expected);
    }

    /// The `TextureMips` stage reports texture memory, and the report must
    /// reconcile with the bytes actually written to disk. This is the engine's
    /// only texture byte accounting; a report that drifts from the `.prm` it
    /// describes is worse than none.
    #[test]
    fn stage_byte_report_reconciles_with_the_baked_prm_on_disk() {
        let root = unique_temp_dir("byte-accounting-report");
        let texture_root = root.join("textures");
        let collection = texture_root.join("stone");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&collection).unwrap();

        // One plain bundle and one carrying a height sibling, so the report
        // spans both specular slot formats.
        std::fs::write(collection.join("plain.png"), png_bytes(8, 8)).unwrap();
        std::fs::write(
            collection.join("plain_s.png"),
            solid_png_bytes(8, 8, [128, 0, 0, 255]),
        )
        .unwrap();
        std::fs::write(collection.join("carved.png"), png_bytes(8, 8)).unwrap();
        std::fs::write(
            collection.join("carved_h.png"),
            solid_png_bytes(8, 8, [90, 90, 90, 255]),
        )
        .unwrap();

        let names = vec!["stone/carved".to_string(), "stone/plain".to_string()];
        // One capture at a time per thread: scope the cold-build capture so the
        // warm rebuild below can start its own.
        let (keys, logged) = {
            let capture = LogCapture::start();
            let keys = bake_texture_mips(&names, &texture_root, &cache_root).unwrap();
            let records = capture.records();
            let logged: Vec<String> = records
                .iter()
                .filter(|record| record.message.contains("texture bytes"))
                .map(|record| record.message.clone())
                .collect();
            (keys, logged)
        };

        // Rebuild the accounting from the sidecars the stage just wrote.
        let mut expected = TextureByteSummary::new();
        for name in &names {
            let path = cache_root.join(format!("{}.prm", cache_filename_for_key(&keys[name])));
            let bytes = std::fs::read(&path).unwrap();
            let (header, slots) = PrmFile::from_bytes_partial(&bytes);
            let header = header.unwrap();
            expected.record(
                name.clone(),
                MaterialBytes::from_parsed_slots(&slots, header.layer_count),
            );

            // Every slot's accounting must equal its real payload length.
            for index in 0..4u8 {
                let accounted = expected
                    .entries()
                    .find(|(entry, _)| *entry == name.as_str())
                    .map(|(_, material)| material)
                    .and_then(|material| material.slot(index))
                    .map(|slot| slot.total_bytes());
                let actual = slots[usize::from(index)]
                    .as_ref()
                    .ok()
                    .map(|slot| slot.payload.len() as u64);
                assert_eq!(accounted, actual, "{name} slot {index}");
            }
        }

        // An 8x8 chain: 4bpp diffuse = 340, 2bpp surface map = 170, 1bpp
        // specular = 85.
        assert_eq!(expected.slot_totals(), [680, 255, 0, 0]);
        assert_eq!(expected.total_bytes(), 935);
        assert_eq!(
            expected.mip_totals().iter().sum::<u64>(),
            expected.total_bytes(),
        );

        // The logged report must be exactly that summary, in name order.
        assert_eq!(
            logged,
            vec![format!(
                "[prl-build] {}",
                expected.report_lines(BYTE_REPORT_LARGEST)[0]
            )],
            "the stage must report the same totals the sidecars carry"
        );

        // A warm rebuild takes the cache-hit path but must report identically —
        // the report describes the level, not what this run happened to rebake.
        let warm = LogCapture::start();
        bake_texture_mips(&names, &texture_root, &cache_root).unwrap();
        let warm_records = warm.records();
        let warm_logged: Vec<String> = warm_records
            .iter()
            .filter(|record| record.message.contains("texture bytes"))
            .map(|record| record.message.clone())
            .collect();
        assert_eq!(warm_logged, logged, "warm and cold reports must agree");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Sum every slot payload actually on disk for the given cache keys,
    /// counting each `.prm` once however many keys point at it.
    fn prm_payload_bytes_on_disk(cache_root: &Path, keys: &[[u8; 32]]) -> u64 {
        let mut seen = HashSet::new();
        let mut total = 0u64;
        for key in keys {
            if !seen.insert(*key) {
                continue;
            }
            let path = cache_root.join(format!("{}.prm", cache_filename_for_key(key)));
            let bytes = std::fs::read(&path).unwrap();
            let (_, slots) = PrmFile::from_bytes_partial(&bytes);
            total += slots
                .iter()
                .filter_map(|slot| slot.as_ref().ok())
                .map(|slot| slot.payload.len() as u64)
                .sum::<u64>();
        }
        total
    }

    /// The `TextureMips` stage bakes world bundles, model sidecars and sprite
    /// collections, and flushes one report covering all three. The grand total
    /// is what a streaming system will size residency against, so it must equal
    /// the bytes on disk — and a `.prm` two callers reach must be charged once,
    /// because the runtime uploads it once. Identity here is the
    /// content-addressed cache key, never the report name.
    #[test]
    fn stage_byte_report_spans_all_three_bakes_and_charges_each_prm_once() {
        let root = unique_temp_dir("byte-accounting-three-bakes");
        let texture_root = root.join("textures");
        let cache_root = root.join("cache");
        let world_dir = texture_root.join("stone");
        std::fs::create_dir_all(&world_dir).unwrap();

        // A rich world bundle, at a complete-bundle address of its own.
        std::fs::write(world_dir.join("plain.png"), png_bytes(8, 8)).unwrap();
        std::fs::write(
            world_dir.join("plain_s.png"),
            solid_png_bytes(8, 8, [128, 0, 0, 255]),
        )
        .unwrap();

        // A diffuse-only world material and a model base-color PNG at a
        // different path with byte-identical content: both resolve to the same
        // `.prm`, and the level pays for it once.
        let shared_png = solid_png_bytes(8, 8, [10, 200, 30, 255]);
        std::fs::write(world_dir.join("shared.png"), &shared_png).unwrap();
        let model_dir = root.join("models");
        std::fs::create_dir_all(&model_dir).unwrap();
        let model_png = model_dir.join("base-color.png");
        std::fs::write(&model_png, &shared_png).unwrap();

        // Two sprite frames: layered payloads are the heaviest bundles the
        // format emits, and the report used to omit them entirely.
        write_sprite_frame(
            &texture_root,
            "puff",
            "puff_00.png",
            &solid_png_bytes(4, 4, [255, 0, 0, 255]),
        );
        write_sprite_frame(
            &texture_root,
            "puff",
            "puff_01.png",
            &solid_png_bytes(4, 4, [0, 0, 255, 255]),
        );

        let names = vec!["stone/plain".to_string(), "stone/shared".to_string()];
        let capture = LogCapture::start();

        let (world_keys, mut stage_bytes) =
            bake_world_texture_mips(&names, &texture_root, &cache_root).unwrap();
        let model_key = bake_diffuse_texture(&model_png, &cache_root).unwrap();
        stage_bytes.account_baked_sidecar(
            "model:models/base-color.png",
            &cache_root,
            &model_key,
            SidecarSlots::DiffuseOnly,
        );
        let sprite_key = bake_sprite_collection(&texture_root, "puff", &cache_root)
            .expect("two valid frames bake a collection");
        stage_bytes.account_baked_sidecar(
            "sprite:puff",
            &cache_root,
            &sprite_key,
            SidecarSlots::All,
        );

        assert_eq!(
            model_key, world_keys["stone/shared"],
            "identical diffuse bytes must address one .prm"
        );

        // One flush, after all three bakes.
        log_texture_byte_summary(stage_bytes.summary());
        let records = capture.records();
        let logged: Vec<String> = records
            .iter()
            .filter(|record| record.message.contains("texture bytes"))
            .map(|record| record.message.clone())
            .collect();

        let expected_total = prm_payload_bytes_on_disk(
            &cache_root,
            &[
                world_keys["stone/plain"],
                world_keys["stone/shared"],
                model_key,
                sprite_key,
            ],
        );
        let summary = stage_bytes.summary();
        assert_eq!(
            summary.total_bytes(),
            expected_total,
            "the report must equal the bytes the three bakes actually left on disk"
        );
        assert_eq!(
            summary.material_count(),
            3,
            "three distinct .prm files, three report lines"
        );
        assert!(
            summary
                .entries()
                .all(|(name, _)| name != "model:models/base-color.png"),
            "a bundle already charged under its world material name must not be charged again"
        );

        // The sprite collection's layered bytes reach the report.
        let (_, sprite_bytes) = summary
            .entries()
            .find(|(name, _)| *name == "sprite:puff")
            .expect("the sprite collection is reported");
        assert_eq!(
            sprite_bytes.slot(0).map(|slot| slot.layer_count),
            Some(2),
            "both sprite frames are charged"
        );
        assert_eq!(
            sprite_bytes.total_bytes(),
            prm_payload_bytes_on_disk(&cache_root, &[sprite_key]),
        );

        assert_eq!(
            logged,
            vec![format!(
                "[prl-build] {}",
                summary.report_lines(BYTE_REPORT_LARGEST)[0]
            )],
            "the stage reports once, after every bake it owns"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// `bake_diffuse_texture` deliberately keeps a richer world bundle that
    /// already occupies the diffuse-only address, because model loading binds
    /// only the diffuse slot. The report must charge the model the same way —
    /// billing it for specular and normal slots the runtime never uploads for
    /// it inflates both its line and the grand total.
    #[test]
    fn model_sidecar_is_charged_only_the_diffuse_slot_it_uploads() {
        let root = unique_temp_dir("byte-accounting-model-slots");
        let texture_root = root.join("textures");
        let world_dir = texture_root.join("stone");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&world_dir).unwrap();

        let diffuse = png_bytes(8, 8);
        std::fs::write(world_dir.join("panel.png"), &diffuse).unwrap();
        std::fs::write(
            world_dir.join("panel_s.png"),
            solid_png_bytes(8, 8, [128, 0, 0, 255]),
        )
        .unwrap();
        let (rich_keys, _) =
            bake_world_texture_mips(&["stone/panel".to_string()], &texture_root, &cache_root)
                .unwrap();

        // Park that rich bundle at the diffuse-only address a model bake
        // derives, the way a pre-change cache still can.
        let model_png = root.join("base-color.png");
        std::fs::write(&model_png, &diffuse).unwrap();
        let diffuse_only_key = *blake3::hash(&diffuse).as_bytes();
        let rich_path = cache_root.join(format!(
            "{}.prm",
            cache_filename_for_key(&rich_keys["stone/panel"])
        ));
        let legacy_path =
            cache_root.join(format!("{}.prm", cache_filename_for_key(&diffuse_only_key)));
        std::fs::copy(&rich_path, &legacy_path).unwrap();

        let key = bake_diffuse_texture(&model_png, &cache_root).unwrap();
        assert_eq!(key, diffuse_only_key, "the richer bundle is kept in place");

        let mut stage_bytes = StageTextureBytes::new();
        stage_bytes.account_baked_sidecar(
            "model:base-color.png",
            &cache_root,
            &key,
            SidecarSlots::DiffuseOnly,
        );

        let (_, model_bytes) = stage_bytes
            .summary()
            .entries()
            .find(|(name, _)| *name == "model:base-color.png")
            .expect("the model sidecar is reported");
        let (_, slots) = PrmFile::from_bytes_partial(&std::fs::read(&legacy_path).unwrap());
        assert!(
            slots[1].is_ok(),
            "the parked bundle really does carry a specular slot"
        );
        assert_eq!(
            model_bytes.slot(1).map(|slot| slot.total_bytes()),
            None,
            "a model must not be charged the specular slot it never binds"
        );
        assert_eq!(
            model_bytes.total_bytes(),
            slots[0].as_ref().unwrap().payload.len() as u64,
            "a model is charged its diffuse slot and nothing else"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every way the byte report can lose a bundle is a silent shrink of the
    /// grand total — the exact failure the stage-boundary flush exists to
    /// prevent. Each one warns and names the material and the `.prm`.
    #[test]
    fn byte_report_warns_when_a_baked_sidecar_cannot_be_accounted() {
        let root = unique_temp_dir("byte-accounting-degradations");
        let cache_root = root.join("cache");
        std::fs::create_dir_all(&cache_root).unwrap();

        let capture = LogCapture::start();
        let mut stage_bytes = StageTextureBytes::new();

        // 1. No file at the key's address.
        let missing_key = [7u8; 32];
        stage_bytes.account_baked_sidecar(
            "model:missing.png",
            &cache_root,
            &missing_key,
            SidecarSlots::DiffuseOnly,
        );
        capture.assert_logged_once(Level::Warn, "cannot read baked sidecar");

        // 2. A file whose header does not parse.
        let bad_header_key = [8u8; 32];
        std::fs::write(
            cache_root.join(format!("{}.prm", cache_filename_for_key(&bad_header_key))),
            b"not a prm file at all",
        )
        .unwrap();
        stage_bytes.account_baked_sidecar(
            "sprite:garbage",
            &cache_root,
            &bad_header_key,
            SidecarSlots::All,
        );
        capture.assert_logged_once(Level::Warn, "cannot parse baked sidecar");

        // 3. A parseable header declaring a slot whose payload is truncated.
        // `from_parsed_slots` skips it by design, so the bundle is recorded
        // light while looking perfectly healthy.
        let diffuse_path = root.join("base-color.png");
        std::fs::write(&diffuse_path, png_bytes(8, 8)).unwrap();
        let truncated_key = bake_diffuse_texture(&diffuse_path, &cache_root).unwrap();
        let truncated_path =
            cache_root.join(format!("{}.prm", cache_filename_for_key(&truncated_key)));
        let mut truncated = std::fs::read(&truncated_path).unwrap();
        truncated.pop();
        std::fs::write(&truncated_path, &truncated).unwrap();
        stage_bytes.account_baked_sidecar(
            "model:base-color.png",
            &cache_root,
            &truncated_key,
            SidecarSlots::DiffuseOnly,
        );
        capture.assert_logged_once(Level::Warn, "declares a diffuse slot that does not parse");

        assert_eq!(
            stage_bytes.summary().total_bytes(),
            0,
            "nothing usable was read, and the report says so out loud"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
