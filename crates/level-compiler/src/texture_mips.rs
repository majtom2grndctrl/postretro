// Bakes per-texture mip pyramids into `.prm` sidecar files.
// See: context/lib/build_pipeline.md §Baked texture mips

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use postretro_level_format::prm::{
    PrmFile, PrmFormat, PrmHeader, PrmSlot, PrmSlots, STAGE_VERSION, bc5_level_count,
    cache_filename_for_key, expected_level_count,
};

/// Normalize a texture name read verbatim from a `.map` into the canonical
/// lookup form used by [`build_name_to_path_map`]: lowercase, backslashes
/// converted to forward slashes, and a leading `textures/` stripped (a no-op
/// when absent). TrenchBroom may emit either `collection/stem` or the
/// root-inclusive `textures/collection/stem`; both normalize to
/// `collection/stem`. A bare `stem` normalizes to itself.
fn normalize_map_texture_name(name: &str) -> String {
    let lowered = name.to_lowercase().replace('\\', "/");
    lowered
        .strip_prefix("textures/")
        .map(str::to_owned)
        .unwrap_or(lowered)
}

/// The bare last path segment of an already-normalized name (the substring
/// after the last `/`), used as the back-compat bare-stem lookup fallback.
fn bare_segment(normalized: &str) -> &str {
    match normalized.rsplit_once('/') {
        Some((_, stem)) => stem,
        None => normalized,
    }
}

/// Build a case-insensitive lookup from texture name to PNG path, scanning
/// every collection directory under `texture_root`. The compiler owns this
/// helper so it does not depend on the runtime crate.
///
/// TrenchBroom identifies materials by their path relative to the textures
/// root, so the `.map` may carry a **collection-qualified** name (e.g.
/// `50-free-textures/concrete_pavement_036`) rather than the bare stem. Each
/// PNG is therefore indexed under its path **relative to `texture_root`**,
/// forward-slashed, lowercased, with the `.png` extension stripped (e.g.
/// `50-free-textures/concrete_pavement_036`).
///
/// For back-compat with hand-authored maps that use bare stems, a **bare-stem
/// alias** is also inserted — but only when that stem is unique across all
/// collections. On a stem collision the alias is dropped (and a warning logged
/// naming both paths) so a bare name never silently resolves to the wrong
/// collection. The collection-qualified key always resolves unambiguously.
fn build_name_to_path_map(texture_root: &Path) -> HashMap<String, PathBuf> {
    let mut map: HashMap<String, PathBuf> = HashMap::new();
    // Tracks bare stems that are ambiguous (seen in more than one collection)
    // so we can avoid inserting a misleading bare-stem alias.
    let mut stem_owner: HashMap<String, PathBuf> = HashMap::new();
    let mut ambiguous_stems: std::collections::HashSet<String> = std::collections::HashSet::new();

    let collections = match std::fs::read_dir(texture_root) {
        Ok(entries) => entries,
        Err(err) => {
            log::warn!(
                "[prl-build] cannot read texture root {}: {err}",
                texture_root.display()
            );
            return map;
        }
    };

    // Collect collection dirs, sorted for deterministic warning order.
    let mut collection_dirs: Vec<PathBuf> = collections
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    collection_dirs.sort();

    for collection_path in collection_dirs {
        let collection_name = match collection_path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_lowercase(),
            None => continue,
        };

        let files = match std::fs::read_dir(&collection_path) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        // Sort files for deterministic relative-key collision resolution.
        let mut file_paths: Vec<PathBuf> = files.flatten().map(|e| e.path()).collect();
        file_paths.sort();

        for file_path in file_paths {
            let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !ext.eq_ignore_ascii_case("png") {
                continue;
            }
            let stem = match file_path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_lowercase(),
                None => continue,
            };

            // Primary key: path relative to texture_root, forward-slashed,
            // lowercased, no extension (e.g. `collection/stem`).
            let rel_key = format!("{collection_name}/{stem}");
            if let Some(existing) = map.get(&rel_key) {
                log::warn!(
                    "[prl-build] duplicate texture path '{rel_key}': found in {} and {}, using first found",
                    existing.display(),
                    file_path.display(),
                );
            } else {
                map.insert(rel_key, file_path.clone());
            }

            // Track bare-stem ownership for the back-compat alias. A stem seen
            // in two or more collections is ambiguous and gets no alias.
            match stem_owner.get(&stem) {
                Some(first) => {
                    if !ambiguous_stems.contains(&stem) {
                        log::warn!(
                            "[prl-build] bare texture name '{stem}' exists in multiple collections \
                             ({} and {}); the bare-stem alias is disabled — qualify it as \
                             'collection/{stem}' to resolve",
                            first.display(),
                            file_path.display(),
                        );
                        ambiguous_stems.insert(stem.clone());
                    }
                }
                None => {
                    stem_owner.insert(stem.clone(), file_path.clone());
                }
            }
        }
    }

    // Insert bare-stem aliases only for stems unique across all collections.
    // A relative key already occupying a bare-stem slot (a PNG sitting at the
    // texture root with no collection — not the documented layout) is left
    // intact rather than overwritten.
    for (stem, path) in stem_owner {
        if ambiguous_stems.contains(&stem) {
            continue;
        }
        map.entry(stem).or_insert(path);
    }

    map
}

/// Build the per-texture bundle hash: covers the slot-mask byte plus, for each
/// present slot in diffuse→specular→normal order, a `(0x00 | 0x01 | 0x02)`
/// disambiguator byte followed by the raw PNG file bytes.
///
/// Hash input starts with the `slot_mask` byte so slot deletion is an
/// unambiguous fingerprint change, followed by `(bit_index_byte, png_bytes)`
/// for every present slot in canonical order.
///
/// The `.prm` filename key (computed separately) intentionally uses a cheaper
/// recipe so files with identical diffuse content collide; this bundle hash
/// changes whenever any sibling changes, forcing a rebake even on a filename
/// hit.
fn bundle_hash_for(
    diffuse: Option<&[u8]>,
    specular: Option<&[u8]>,
    normal: Option<&[u8]>,
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
    let mut h = blake3::Hasher::new();
    h.update(&[mask]);
    if let Some(b) = diffuse {
        h.update(&[0x00]);
        h.update(b);
    }
    if let Some(b) = specular {
        h.update(&[0x01]);
        h.update(b);
    }
    if let Some(b) = normal {
        h.update(&[0x02]);
        h.update(b);
    }
    *h.finalize().as_bytes()
}

/// Filename-key recipe (NOT the bundle hash). Distinct from the bundle hash by
/// design — see module-level docs.
fn filename_key_for(
    diffuse: Option<&[u8]>,
    specular: Option<&[u8]>,
    normal: Option<&[u8]>,
) -> [u8; 32] {
    match (diffuse, specular, normal) {
        (Some(d), _, _) => *blake3::hash(d).as_bytes(),
        (None, Some(s), _) => {
            let mut h = blake3::Hasher::new();
            h.update(&[0x01]);
            h.update(s);
            *h.finalize().as_bytes()
        }
        (None, None, Some(n)) => {
            let mut h = blake3::Hasher::new();
            h.update(&[0x02]);
            h.update(n);
            *h.finalize().as_bytes()
        }
        (None, None, None) => [0u8; 32],
    }
}

// -- Gamma helpers --------------------------------------------------------

/// 256-entry sRGB → linear lookup. Built once per call to `bake_texture_mips`.
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
fn build_diffuse_chain(rgba: &[u8], width: u32, height: u32, lut: &[f32; 256]) -> Vec<u8> {
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
fn build_specular_chain(r8: &[u8], width: u32, height: u32) -> Vec<u8> {
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
fn build_normal_bc5_chain(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
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

/// Bake per-texture mip pyramids into `.prm` sidecars under `cache_root`.
/// Returns a map from texture name → 32-byte cache key (the `.prm` filename
/// stem in hex). Names whose slots are all missing get a `[0u8; 32]` key and
/// no `.prm` is written; callers flag the all-zero key in
/// `TextureCacheKeysSection`. The runtime treats zero keys as 'no source PNG'
/// and substitutes placeholders silently by design — missing PNGs are not an
/// error in maps that don't use every named texture slot.
pub fn bake_texture_mips(
    texture_names: &[String],
    texture_root: &Path,
    cache_root: &Path,
) -> anyhow::Result<HashMap<String, [u8; 32]>> {
    let name_to_path = build_name_to_path_map(texture_root);
    let lut = build_srgb_to_linear_lut();

    let mut out: HashMap<String, [u8; 32]> = HashMap::with_capacity(texture_names.len());

    for name in texture_names {
        // Normalize the incoming map name: lowercase, backslashes → forward
        // slashes, and strip a leading `textures/` (a no-op when absent) so a
        // root-inclusive TrenchBroom name (`textures/collection/stem`) matches
        // the relative keys (`collection/stem`).
        let normalized = normalize_map_texture_name(name);

        // Resolve the diffuse against the normalized relative name first; on a
        // miss, fall back to the bare last path segment (back-compat alias).
        // Sibling `_s`/`_n` keys append to the SAME form that resolved the
        // diffuse, so siblings come from the same collection.
        let (diff_path, resolved_base) = match name_to_path.get(&normalized) {
            Some(p) => (Some(p.clone()), normalized.clone()),
            None => {
                let bare = bare_segment(&normalized).to_string();
                (name_to_path.get(&bare).cloned(), bare)
            }
        };
        let spec_path = name_to_path.get(&format!("{resolved_base}_s")).cloned();
        let norm_path = name_to_path.get(&format!("{resolved_base}_n")).cloned();

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

        let filename_key = filename_key_for(
            diff_bytes.as_deref(),
            spec_bytes.as_deref(),
            norm_bytes.as_deref(),
        );

        // All-absent: nothing to bake.
        if diff_bytes.is_none() && spec_bytes.is_none() && norm_bytes.is_none() {
            out.insert(name.clone(), [0u8; 32]);
            continue;
        }

        let bundle_hash = bundle_hash_for(
            diff_bytes.as_deref(),
            spec_bytes.as_deref(),
            norm_bytes.as_deref(),
        );

        let prm_path = cache_root.join(format!("{}.prm", cache_filename_for_key(&filename_key)));

        // Cache hit: header parses and bundle_hash matches.
        if prm_path.exists() {
            if let Ok(bytes) = std::fs::read(&prm_path) {
                let (hdr_result, _slots) = PrmFile::from_bytes_partial(&bytes);
                if let Ok(hdr) = hdr_result {
                    if hdr.bundle_hash == bundle_hash {
                        out.insert(name.clone(), filename_key);
                        continue;
                    }
                }
            }
        }

        // Build slots. We only emit a slot for the diffuse if the source PNG
        // actually exists; same for specular and normal. Dimensions across
        // slots are not required to match here (the runtime checks).
        let mut slots_arr: [Option<PrmSlot>; 3] = [None, None, None];
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

        let prm = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask,
                bundle_hash,
                total_body_bytes: 0, // recomputed by to_bytes
            },
            slots: slots_arr,
        };

        let encoded = prm
            .to_bytes()
            .map_err(|e| anyhow::anyhow!("encoding .prm for texture {name:?}: {e}"))?;
        atomic_write(&prm_path, &encoded)?;

        out.insert(name.clone(), filename_key);
    }

    Ok(out)
}

// -- Tests ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
            },
            slots: [None, None, Some(slot)],
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
            },
            slots: [None, None, Some(slot)],
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
        let a = bundle_hash_for(None, Some(b"alpha"), Some(b"beta"));
        let b = bundle_hash_for(None, Some(b"beta"), Some(b"alpha"));
        assert_ne!(a, b);
    }

    /// Filename key falls back to specular when diffuse is missing, but the
    /// 0x01 prefix prevents a collision with a diffuse PNG whose bytes
    /// happen to equal the specular bytes.
    #[test]
    fn filename_key_specular_fallback_does_not_collide_with_diffuse() {
        let bytes: &[u8] = b"identical-payload";
        let diff_only = filename_key_for(Some(bytes), None, None);
        let spec_only = filename_key_for(None, Some(bytes), None);
        assert_ne!(diff_only, spec_only);
    }

    #[test]
    fn all_absent_key_is_zero() {
        assert_eq!(filename_key_for(None, None, None), [0u8; 32]);
    }

    /// Resolver coverage: a collection subdir with a diffuse, a `_s` specular
    /// sibling, and a `_n` normal sibling must resolve through ALL three name
    /// forms TrenchBroom might emit — bare `stem`, `collection/stem`, and
    /// root-inclusive `textures/collection/stem` — to the same PNG, and the
    /// resolved form must carry the `_s`/`_n` siblings from that collection.
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
        std::fs::write(&diff, png_bytes(1)).unwrap();
        std::fs::write(&spec, png_bytes(2)).unwrap();
        std::fs::write(&norm, png_bytes(3)).unwrap();

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
            let (diff_path, base) = match map.get(&normalized) {
                Some(p) => (Some(p.clone()), normalized.clone()),
                None => {
                    let bare = bare_segment(&normalized).to_string();
                    (map.get(&bare).cloned(), bare)
                }
            };
            assert_eq!(
                diff_path.as_ref(),
                Some(&diff),
                "diffuse for '{incoming}' should resolve to {}",
                diff.display()
            );
            assert_eq!(
                map.get(&format!("{base}_s")),
                Some(&spec),
                "specular sibling for '{incoming}' should resolve from the same collection"
            );
            assert_eq!(
                map.get(&format!("{base}_n")),
                Some(&norm),
                "normal sibling for '{incoming}' should resolve from the same collection"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
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
        let got = bundle_hash_for(Some(&[0xAAu8, 0xBB]), None, None);
        assert_eq!(got, expected);
    }
}
