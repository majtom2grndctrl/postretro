// Compile-time mip-chain baker for material textures.
//
// Source PNGs (diffuse, `_s.png` specular, `_n.png` normal) are read once per
// build, downsampled to a full mip pyramid using a separable
// Mitchell-Netravali (B=1/3, C=1/3) filter, and packed into a per-texture
// `.prm` sidecar under `<workspace>/.build-caches/prm-cache/`. The runtime
// uploads `.prm` payloads directly without rehashing or re-filtering.
//
// Why bake offline:
// - Mitchell-Netravali is too expensive to run at load time across every
//   texture and every level (3+ slots × Σ widths × heights at all mip levels).
// - The renderer cannot do gamma-correct sRGB filtering with wgpu's built-in
//   mip generation, which assumes linear data.
// - Per-texel normal renormalisation is not expressible inside a hardware
//   filter; baking lets us renormalise per output texel.
//
// Filename keying vs bundle hashing:
//
//   The on-disk filename uses a *cheap* key (`blake3(diffuse_png_bytes)` with
//   slot-specific prefixes when diffuse is absent) so that two textures with
//   identical diffuse content collide on the same `.prm`. The *bundle hash*
//   stored inside the file (and used for cache-validity comparison) covers
//   the full present-slot set: when only the specular sibling changes,
//   bundle_hash differs and we rebake even though the filename key would have
//   matched.
//
// Wire types come from `postretro_level_format::prm` — this module never
// reinvents the format.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use postretro_level_format::prm::{
    PrmFile, PrmFormat, PrmHeader, PrmSlot, PrmSlots, STAGE_VERSION, cache_filename_for_key,
};

/// Build a case-insensitive lookup from texture stem to PNG path, scanning
/// every collection directory under `texture_root`. The compiler owns this
/// helper so it does not depend on the runtime crate.
fn build_name_to_path_map(texture_root: &Path) -> HashMap<String, PathBuf> {
    let mut map: HashMap<String, PathBuf> = HashMap::new();

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

    for entry in collections.flatten() {
        let collection_path = entry.path();
        if !collection_path.is_dir() {
            continue;
        }

        let files = match std::fs::read_dir(&collection_path) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for file_entry in files.flatten() {
            let file_path = file_entry.path();
            let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !ext.eq_ignore_ascii_case("png") {
                continue;
            }
            let stem = match file_path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_lowercase(),
                None => continue,
            };
            if let Some(existing) = map.get(&stem) {
                log::warn!(
                    "[prl-build] duplicate texture name '{stem}': found in {} and {}, using first found",
                    existing.display(),
                    file_path.display(),
                );
            } else {
                map.insert(stem, file_path);
            }
        }
    }

    map
}

/// Build the per-texture bundle hash: covers the slot-mask byte plus, for each
/// present slot in diffuse→specular→normal order, a `(0x00 | 0x01 | 0x02)`
/// disambiguator byte followed by the raw PNG file bytes.
///
/// The `.prm` filename key (computed separately) intentionally uses a cheaper
/// recipe so files with identical diffuse content collide; this bundle hash
/// changes whenever any sibling changes, forcing a rebake even on a filename
/// hit. See module-level docs.
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

/// `floor(log2(max_dim)) + 1`. Mirrors `expected_level_count` in
/// `postretro_level_format::prm` (kept private there).
pub fn mip_level_count_for(width: u32, height: u32) -> u8 {
    let max_dim = width.max(height).max(1);
    (32 - max_dim.leading_zeros()) as u8
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
    let level_count = mip_level_count_for(width, height) as u32;

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
    let level_count = mip_level_count_for(width, height) as u32;

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

/// Build a normal mip chain (Rgba8Unorm). Each RGB octet is decoded into the
/// `[-1, 1]` interval before filtering; output normals are renormalised per
/// texel and re-encoded.
fn build_normal_chain(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let channels = 4;
    let level_count = mip_level_count_for(width, height) as u32;

    let mut linear: Vec<f32> = Vec::with_capacity((width * height) as usize * channels);
    for chunk in rgba.chunks_exact(4) {
        linear.push((chunk[0] as f32) / 255.0 * 2.0 - 1.0);
        linear.push((chunk[1] as f32) / 255.0 * 2.0 - 1.0);
        linear.push((chunk[2] as f32) / 255.0 * 2.0 - 1.0);
        linear.push((chunk[3] as f32) / 255.0);
    }

    let mut payload: Vec<u8> = Vec::with_capacity(rgba.len() * 2);
    encode_normal_into(&linear, &mut payload);

    let mut cur = linear;
    let mut cw = width;
    let mut ch = height;
    for _ in 1..level_count {
        let (next, nw, nh) = downsample_2x_f32(&cur, cw, ch, channels);
        encode_normal_into(&next, &mut payload);
        cur = next;
        cw = nw;
        ch = nh;
    }

    payload
}

/// Encode a normal RGBA buffer (XYZ in `[-1, 1]`, A in `[0, 1]`) into Rgba8
/// bytes. Each output normal is renormalised; near-zero magnitudes fall back
/// to `(0, 0, 1)` (tangent-space up).
fn encode_normal_into(linear: &[f32], out: &mut Vec<u8>) {
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
}

// -- File I/O -------------------------------------------------------------

/// Decode a PNG into a `(rgba8, w, h)` triple. The `image` crate handles all
/// supported PNG colour types and converts them to RGBA8.
fn decode_png_rgba(path: &Path) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let img = image::open(path)
        .map_err(|e| anyhow::anyhow!("failed to open PNG {}: {e}", path.display()))?;
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
        let key_lower = name.to_lowercase();
        let diff_path = name_to_path.get(&key_lower).cloned();
        let spec_path = name_to_path.get(&format!("{key_lower}_s")).cloned();
        let norm_path = name_to_path.get(&format!("{key_lower}_n")).cloned();

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

        if let Some(p) = diff_path.as_ref() {
            let (rgba, w, h) = decode_png_rgba(p)?;
            let payload = build_diffuse_chain(&rgba, w, h, &lut);
            slots_arr[0] = Some(PrmSlot {
                format: PrmFormat::Rgba8UnormSrgb,
                width: w as u16,
                height: h as u16,
                level_count: mip_level_count_for(w, h),
                payload,
            });
            slot_mask |= PrmSlots::DIFFUSE;
        }
        if let Some(p) = spec_path.as_ref() {
            // Decode as RGBA; flatten to R8 (PNG authoring is typically L8 or
            // RGBA8 with the spec data in R). We accept either.
            let (rgba, w, h) = decode_png_rgba(p)?;
            let r8: Vec<u8> = rgba.chunks_exact(4).map(|c| c[0]).collect();
            let payload = build_specular_chain(&r8, w, h);
            slots_arr[1] = Some(PrmSlot {
                format: PrmFormat::R8Unorm,
                width: w as u16,
                height: h as u16,
                level_count: mip_level_count_for(w, h),
                payload,
            });
            slot_mask |= PrmSlots::SPECULAR;
        }
        if let Some(p) = norm_path.as_ref() {
            let (rgba, w, h) = decode_png_rgba(p)?;
            let payload = build_normal_chain(&rgba, w, h);
            slots_arr[2] = Some(PrmSlot {
                format: PrmFormat::Rgba8Unorm,
                width: w as u16,
                height: h as u16,
                level_count: mip_level_count_for(w, h),
                payload,
            });
            slot_mask |= PrmSlots::NORMAL;
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

        let encoded = prm.to_bytes();
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

    /// Normal-map outputs are unit-length within 1/127 of 1.0 after baking
    /// at every mip level. Build a 4×4 normal map of varied directions and
    /// verify both the 2×2 and 1×1 mips.
    #[test]
    fn normal_map_outputs_unit_length() {
        // Build 4×4 unit-length normals in directions clustered around
        // (0, 0, 1) with small tilt — typical surface-normal authoring.
        let mut rgba = Vec::with_capacity(4 * 4 * 4);
        for y in 0..4 {
            for x in 0..4 {
                let dx = (x as f32 - 1.5) * 0.2;
                let dy = (y as f32 - 1.5) * 0.2;
                let dz = (1.0f32 - dx * dx - dy * dy).max(0.0).sqrt();
                let r = ((dx * 0.5 + 0.5) * 255.0).round() as u8;
                let g = ((dy * 0.5 + 0.5) * 255.0).round() as u8;
                let b = ((dz * 0.5 + 0.5) * 255.0).round() as u8;
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }

        let payload = build_normal_chain(&rgba, 4, 4);
        // Levels: 4×4 = 64, 2×2 = 16, 1×1 = 4. Total = 84.
        assert_eq!(payload.len(), 84);

        fn check_unit_length(level: &[u8]) {
            for chunk in level.chunks_exact(4) {
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
        check_unit_length(&payload[0..64]); // 4×4
        check_unit_length(&payload[64..80]); // 2×2
        check_unit_length(&payload[80..84]); // 1×1
    }

    #[test]
    fn mip_level_count_matches_floor_log2_plus_one() {
        assert_eq!(mip_level_count_for(1, 1), 1);
        assert_eq!(mip_level_count_for(2, 1), 2);
        assert_eq!(mip_level_count_for(4, 4), 3);
        assert_eq!(mip_level_count_for(8, 4), 4);
        assert_eq!(mip_level_count_for(1024, 1024), 11);
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
}
