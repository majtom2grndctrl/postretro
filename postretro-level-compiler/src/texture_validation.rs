// Compile-time PNG color-space validation for texture siblings.
//
// Per `context/lib/resource_management.md` §4 and the normal-maps plan
// (`context/plans/in-progress/normal-maps/index.md` Task 2), surface map
// siblings must be authored in linear color space:
//
// | Suffix     | Required color space | Action on violation |
// |------------|----------------------|---------------------|
// | `_n.png`   | Linear               | Fail build          |
// | `_s.png`   | Linear               | Fail build          |
// | (diffuse)  | sRGB (no enforcement)| —                   |
//
// "Linear" is defined as: no `sRGB` PNG chunk, no `iCCP` chunk, and either
// no `gAMA` chunk or a `gAMA` chunk with value approximately 1.0.
//
// A misconfigured `_n.png` is the worst case — sRGB gamma applied to raw XYZ
// normals shifts directions non-linearly and silently breaks shading. This
// pass turns the silent bug into a compile-time diagnostic.
//
// The `image` crate (used elsewhere) does not surface PNG `sRGB` / `gAMA` /
// `iCCP` chunks through its public API. The lower-level `png` crate exposes
// them on `png::Info`. We read just the metadata (no pixel decode).

use std::path::{Path, PathBuf};

/// Tolerance for treating a `gAMA` chunk as "linear (1.0)".
///
/// PNG gAMA values are stored as `ScaledFloat` with 1e-5 quantization; allow
/// a small slack so a value like `1.00000` round-tripped through `100000`
/// reads as linear. Anything outside this band — including the canonical
/// sRGB display gamma `0.45455` (~1/2.2) — is rejected.
const LINEAR_GAMMA_EPSILON: f32 = 0.01;

/// Detected color space for an inspected PNG.
#[derive(Debug, Clone, PartialEq)]
enum DetectedColorSpace {
    /// PNG has an `sRGB` chunk — explicitly tagged sRGB.
    Srgb,
    /// PNG has an `iCCP` chunk — embeds an ICC profile, treated as non-linear.
    /// Most exporters that emit `iCCP` are tagging an sRGB or display profile;
    /// authoring a linear surface map should not include an embedded profile.
    IccProfile { profile_name: String },
    /// PNG has a `gAMA` chunk with value not within `LINEAR_GAMMA_EPSILON` of 1.0.
    NonLinearGamma { gamma: f32 },
    /// No color-space metadata, or a `gAMA` chunk approximately equal to 1.0.
    Linear,
}

impl DetectedColorSpace {
    fn describe(&self) -> String {
        match self {
            Self::Srgb => "sRGB (sRGB chunk present)".to_string(),
            Self::IccProfile { profile_name } => {
                format!("ICC profile '{profile_name}' (iCCP chunk present)")
            }
            Self::NonLinearGamma { gamma } => format!("non-linear gamma {gamma:.5}"),
            Self::Linear => "linear".to_string(),
        }
    }

    fn is_linear(&self) -> bool {
        matches!(self, Self::Linear)
    }
}

/// Inspect a PNG's color-space metadata without decoding pixel data.
///
/// Returns an error only on I/O failure or when the file is not a valid
/// PNG header. A valid PNG with no color-space chunks reads as `Linear`.
fn detect_color_space(path: &Path) -> anyhow::Result<DetectedColorSpace> {
    let file = std::fs::File::open(path)
        .map_err(|e| anyhow::anyhow!("failed to open {}: {e}", path.display()))?;
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let reader = decoder
        .read_info()
        .map_err(|e| anyhow::anyhow!("failed to read PNG header for {}: {e}", path.display()))?;
    let info = reader.info();

    if info.srgb.is_some() {
        return Ok(DetectedColorSpace::Srgb);
    }

    if let Some(icc) = info.icc_profile.as_ref() {
        // The iCCP chunk leads with a 1–79 byte Latin-1 profile name terminated
        // by a NUL, but `png` already strips that and exposes the decompressed
        // profile body. We don't have the original name, so report a stable
        // marker and the profile size.
        let _ = icc; // body content not used; presence is the signal
        return Ok(DetectedColorSpace::IccProfile {
            profile_name: "embedded".to_string(),
        });
    }

    if let Some(gama) = info.gama_chunk {
        let gamma: f32 = gama.into_value();
        if (gamma - 1.0).abs() > LINEAR_GAMMA_EPSILON {
            return Ok(DetectedColorSpace::NonLinearGamma { gamma });
        }
    }

    Ok(DetectedColorSpace::Linear)
}

/// Build a flat list of PNG file paths under `texture_root` whose stem
/// matches one of the surface-map suffixes (`_s`, `_n`).
///
/// Walks one collection level deep (`<root>/<collection>/<file>.png`),
/// matching the authoring convention described in
/// `context/lib/resource_management.md` §1.1.
fn collect_sibling_pngs(texture_root: &Path) -> std::io::Result<Vec<(PathBuf, &'static str)>> {
    let mut out = Vec::new();
    if !texture_root.is_dir() {
        return Ok(out);
    }
    for collection in std::fs::read_dir(texture_root)? {
        let collection = match collection {
            Ok(c) => c,
            Err(_) => continue,
        };
        let cpath = collection.path();
        if !cpath.is_dir() {
            continue;
        }
        let files = match std::fs::read_dir(&cpath) {
            Ok(f) => f,
            Err(_) => continue,
        };
        for file in files.flatten() {
            let p = file.path();
            if p.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase())
                != Some("png".to_string())
            {
                continue;
            }
            let stem = match p.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s,
                None => continue,
            };
            let suffix = if stem.ends_with("_n") {
                "_n.png"
            } else if stem.ends_with("_s") {
                "_s.png"
            } else {
                continue;
            };
            out.push((p, suffix));
        }
    }
    Ok(out)
}

/// Validate every `_n.png` and `_s.png` under `texture_root` for linear
/// color-space metadata. Logs each load at `info` level.
///
/// Returns an aggregate error naming every offender. The validator surfaces
/// every violation at once rather than failing on the first, so a single
/// build run gives the asset author the full work list.
pub fn validate_sibling_color_spaces(texture_root: &Path) -> anyhow::Result<()> {
    let siblings = collect_sibling_pngs(texture_root).map_err(|e| {
        anyhow::anyhow!(
            "failed to walk textures directory {}: {e}",
            texture_root.display()
        )
    })?;

    let mut violations: Vec<String> = Vec::new();
    for (path, suffix) in &siblings {
        match detect_color_space(path) {
            Ok(cs) => {
                log::info!(
                    "[prl-build] surface-map sibling {} color space: {}",
                    path.display(),
                    cs.describe()
                );
                if !cs.is_linear() {
                    violations.push(format!(
                        "  {}: detected {}, required linear (suffix `{suffix}`)",
                        path.display(),
                        cs.describe(),
                    ));
                }
            }
            Err(e) => {
                violations.push(format!("  {}: {e}", path.display()));
            }
        }
    }

    if violations.is_empty() {
        log::info!(
            "[prl-build] color-space validation passed for {} surface-map sibling(s) under {}",
            siblings.len(),
            texture_root.display()
        );
        return Ok(());
    }

    Err(anyhow::anyhow!(
        "PNG color-space validation failed for {} surface-map sibling(s):\n{}\n\
         `_s.png` (specular) and `_n.png` (normal) textures must be authored \
         in linear color space (no sRGB chunk, no iCCP chunk, gAMA ≈ 1.0). \
         Re-export the offending files as linear PNG. \
         See context/lib/resource_management.md §4.",
        violations.len(),
        violations.join("\n"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal valid PNG (1×1 grayscale) with optional `sRGB` / `gAMA`
    /// / `iCCP` chunks for testing. Produces a real, decodable PNG so the
    /// `png` crate accepts it.
    fn build_test_png(extra_chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);

        fn crc32(chunk_type: &[u8; 4], data: &[u8]) -> u32 {
            const TABLE: [u32; 256] = {
                let mut t = [0u32; 256];
                let mut i = 0;
                while i < 256 {
                    let mut c = i as u32;
                    let mut k = 0;
                    while k < 8 {
                        c = if c & 1 != 0 { 0xedb88320 ^ (c >> 1) } else { c >> 1 };
                        k += 1;
                    }
                    t[i] = c;
                    i += 1;
                }
                t
            };
            let mut crc = 0xffffffffu32;
            for b in chunk_type.iter().chain(data.iter()) {
                crc = TABLE[((crc ^ *b as u32) & 0xff) as usize] ^ (crc >> 8);
            }
            crc ^ 0xffffffff
        }

        fn write_chunk(buf: &mut Vec<u8>, ctype: &[u8; 4], data: &[u8]) {
            buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
            buf.extend_from_slice(ctype);
            buf.extend_from_slice(data);
            buf.extend_from_slice(&crc32(ctype, data).to_be_bytes());
        }

        // IHDR: 1×1, 8-bit, grayscale, no interlace.
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.push(8); // bit depth
        ihdr.push(0); // color type: grayscale
        ihdr.push(0); // compression
        ihdr.push(0); // filter
        ihdr.push(0); // interlace
        write_chunk(&mut buf, b"IHDR", &ihdr);

        for (ctype, data) in extra_chunks {
            write_chunk(&mut buf, ctype, data);
        }

        // IDAT: zlib-wrapped deflate of one filter byte + one pixel byte.
        // Use stored block to keep it dependency-free.
        let raw = [0u8, 0u8]; // filter=None, pixel=0
        let mut zlib = Vec::new();
        zlib.extend_from_slice(&[0x78, 0x01]); // zlib header (no compression)
        // stored block: BFINAL=1, BTYPE=00
        zlib.push(0x01);
        let len = raw.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(&raw);
        // Adler-32 of `raw`.
        let mut a: u32 = 1;
        let mut b: u32 = 0;
        for &x in &raw {
            a = (a + x as u32) % 65521;
            b = (b + a) % 65521;
        }
        let adler = (b << 16) | a;
        zlib.extend_from_slice(&adler.to_be_bytes());
        write_chunk(&mut buf, b"IDAT", &zlib);

        write_chunk(&mut buf, b"IEND", &[]);
        buf
    }

    fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("prl-build-tex-validate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    #[test]
    fn detects_linear_when_no_color_chunks_present() {
        let png = build_test_png(&[]);
        let p = write_temp("plain.png", &png);
        assert!(matches!(detect_color_space(&p).unwrap(), DetectedColorSpace::Linear));
    }

    #[test]
    fn detects_srgb_chunk() {
        // sRGB chunk: 1 byte rendering intent.
        let png = build_test_png(&[(b"sRGB", vec![0u8])]);
        let p = write_temp("srgb.png", &png);
        assert!(matches!(detect_color_space(&p).unwrap(), DetectedColorSpace::Srgb));
    }

    #[test]
    fn detects_non_linear_gamma() {
        // gAMA: 4 bytes big-endian, value * 100000. 1/2.2 ≈ 45455.
        let png = build_test_png(&[(b"gAMA", 45455u32.to_be_bytes().to_vec())]);
        let p = write_temp("gama_srgb.png", &png);
        match detect_color_space(&p).unwrap() {
            DetectedColorSpace::NonLinearGamma { gamma } => {
                assert!((gamma - 0.45455).abs() < 1e-4, "got {gamma}");
            }
            other => panic!("expected NonLinearGamma, got {other:?}"),
        }
    }

    #[test]
    fn accepts_gamma_one() {
        // gAMA = 1.0 is encoded as 100000.
        let png = build_test_png(&[(b"gAMA", 100000u32.to_be_bytes().to_vec())]);
        let p = write_temp("gama_linear.png", &png);
        assert!(matches!(detect_color_space(&p).unwrap(), DetectedColorSpace::Linear));
    }

    #[test]
    fn validate_passes_on_clean_tree() {
        let dir = std::env::temp_dir().join(format!(
            "prl-build-tex-validate-clean-{}",
            std::process::id()
        ));
        let coll = dir.join("collection");
        std::fs::create_dir_all(&coll).unwrap();
        let png = build_test_png(&[]);
        std::fs::write(coll.join("wall_s.png"), &png).unwrap();
        std::fs::write(coll.join("wall_n.png"), &png).unwrap();
        validate_sibling_color_spaces(&dir).unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn validate_fails_naming_offender_and_required_space() {
        let dir = std::env::temp_dir().join(format!(
            "prl-build-tex-validate-fail-{}",
            std::process::id()
        ));
        let coll = dir.join("collection");
        std::fs::create_dir_all(&coll).unwrap();
        let bad = build_test_png(&[(b"sRGB", vec![0u8])]);
        std::fs::write(coll.join("wall_n.png"), &bad).unwrap();
        let err = validate_sibling_color_spaces(&dir).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("wall_n.png"), "missing path: {msg}");
        assert!(msg.contains("sRGB"), "missing detected space: {msg}");
        assert!(msg.contains("linear"), "missing required space: {msg}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn validate_skips_diffuse_textures() {
        // Diffuse PNGs (no `_s`/`_n` suffix) get no enforcement, even when
        // tagged sRGB — they are sampled as Rgba8UnormSrgb at runtime.
        let dir = std::env::temp_dir().join(format!(
            "prl-build-tex-validate-diffuse-{}",
            std::process::id()
        ));
        let coll = dir.join("collection");
        std::fs::create_dir_all(&coll).unwrap();
        let srgb = build_test_png(&[(b"sRGB", vec![0u8])]);
        std::fs::write(coll.join("wall.png"), &srgb).unwrap();
        validate_sibling_color_spaces(&dir).unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }
}
