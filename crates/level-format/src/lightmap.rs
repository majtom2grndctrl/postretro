// Directional lightmap section (ID 22): atlas-packed per-texel irradiance plus
// dominant incoming-light direction for static (non-dynamic) lights.
// See: context/lib/rendering_pipeline.md §4, context/lib/build_pipeline.md §PRL

use crate::FormatError;

/// Byte stride of one irradiance texel for the **RGBA16F path only**: 4 half-floats = 8 bytes.
/// Not applicable to BC6H, which encodes 4×4 texel blocks (16 bytes each, so
/// `ceil(w/4)·ceil(h/4)·16` bytes total). Alpha is currently unused (carries no AO
/// term yet) and is written as 1.0 so fallback samplers that read alpha don't
/// misinterpret it as transparency.
pub const IRRADIANCE_TEXEL_BYTES: usize = 8;

/// Byte stride of one direction texel on disk: RGBA8Unorm, 4 bytes.
/// `rgb` holds the octahedral-encoded unit direction (remapped 0..1 → −1..1 on
/// sample); `a` is padding and carries 0xFF so bilinear sampling of edge
/// texels into unused neighbours doesn't corrupt direction decode.
pub const DIRECTION_TEXEL_BYTES: usize = 4;

/// v2 section format version. Pre-v2 sections had no version field (their first
/// u32 was `width`); `from_bytes` rejects any value other than this.
pub const LIGHTMAP_SECTION_VERSION: u32 = 2;

/// Fixed 48-byte v2 header preceding the two layer-major texel blobs.
const HEADER_SIZE: usize = 48;

/// Directional lightmap atlas (multi-layer, format version 2).
///
/// Irradiance and direction are independent `texture_2d_array` atlases sharing
/// a single `layer_count`. Their per-layer dimensions and texel densities are
/// decoupled — the irradiance and direction atlases need not match in size.
/// Charts that overflow one layer spill into additional layers, so the blob for
/// each atlas is layer-major: layer 0's texels, then layer 1's, and so on.
///
/// On-disk layout (all little-endian):
///
/// ```text
///   Header (48 bytes):
///     u32 version            (= LIGHTMAP_SECTION_VERSION = 2)
///     u32 layer_count        (shared by both atlases; >= 1)
///     u32 irr_width          (per-layer texel width;  pow2, >= 4)
///     u32 irr_height         (per-layer texel height; pow2, >= 4)
///     f32 irr_texel_density  (m/texel at bake time; informational)
///     u32 irr_format         (0 = Rgba16Float, 1 = Bc6hRgbUfloat)
///     u32 irr_total_bytes    (byte count for ALL irradiance layers combined)
///     u32 dir_width          (per-layer texel width;  pow2, >= 4)
///     u32 dir_height         (per-layer texel height; pow2, >= 4)
///     f32 dir_texel_density  (m/texel; informational; may differ from irr)
///     u32 dir_format         (0 = Rgba8Unorm octahedral; only defined value)
///     u32 dir_total_bytes    (byte count for ALL direction layers combined)
///
///   Irradiance blob (irr_total_bytes bytes): layer-major.
///     Each layer is irr_width × irr_height texels in irr_format layout:
///       Rgba16Float:   u16 × 4 per texel, row-major (y * irr_width + x)
///       Bc6hRgbUfloat: ceil(w/4)·ceil(h/4)·16 bytes of row-major 4×4 blocks
///
///   Direction blob (dir_total_bytes bytes): layer-major.
///     Each layer is dir_width × dir_height × 4 bytes (Rgba8Unorm octahedral,
///     row-major y * dir_width + x).
///
///   Optional LMOD trailer (8 bytes; omitted when mode = Shadowed), at offset
///   48 + irr_total_bytes + dir_total_bytes:
///     u32 magic (= LIGHTMAP_MODE_TRAILER_MAGIC)
///     u32 mode  (LightmapMode discriminant — 0 = shadowed, 1 = unshadowed)
/// ```
///
/// The header carries explicit `irr_total_bytes`/`dir_total_bytes`; `from_bytes`
/// slices each blob by the stored length without recomputing per-layer block math.
///
/// Irradiance texels for atlas positions not covered by any face chart are
/// zero. Edge dilation is applied at bake time so bilinear sampling at chart
/// boundaries pulls valid neighbours instead of black.
#[derive(Debug, Clone, PartialEq)]
pub struct LightmapSection {
    /// Number of array layers shared by the irradiance and direction atlases.
    /// Always `>= 1`; single-layer bakes write `1`.
    pub layer_count: u32,
    /// Per-layer irradiance texel width.
    pub irr_width: u32,
    /// Per-layer irradiance texel height.
    pub irr_height: u32,
    /// World-space meters-per-texel used at bake time for the irradiance atlas.
    /// Informational — the runtime samples through per-vertex lightmap UVs and
    /// does not derive world-space sizes from this field.
    pub irr_texel_density: f32,
    /// Layer-major irradiance blob (all layers concatenated). Layout per layer
    /// depends on `irr_format`: `IRRADIANCE_FORMAT_RGBA16F` (0) is row-major
    /// `Rgba16Float` (`w·h·8` bytes per layer); `IRRADIANCE_FORMAT_BC6H` (1) is
    /// row-major 4×4 `Bc6hRgbUfloat` blocks (`ceil(w/4)·ceil(h/4)·16` per layer).
    pub irradiance: Vec<u8>,
    /// Format tag for `irradiance` — one of `IRRADIANCE_FORMAT_RGBA16F` or
    /// `IRRADIANCE_FORMAT_BC6H`. Written into the section header; the runtime
    /// branches its texture format on this value. `LightmapSection::placeholder`
    /// always emits RGBA16F; the bake chooses BC6H vs RGBA16F via
    /// `LightmapConfig::uncompressed_irradiance`.
    pub irradiance_format: u32,
    /// Per-layer direction texel width.
    pub dir_width: u32,
    /// Per-layer direction texel height.
    pub dir_height: u32,
    /// World-space meters-per-texel used at bake time for the direction atlas.
    /// Informational; may differ from `irr_texel_density`.
    pub dir_texel_density: f32,
    /// Layer-major direction blob (Rgba8Unorm, octahedral, row-major per layer).
    pub direction: Vec<u8>,
    /// Which bake produced the irradiance. `Shadowed` (default) folds static-light
    /// shadows into irradiance — the `main` behavior, written without a trailer so
    /// output is byte-identical to `main`. `Unshadowed` writes a trailer recording
    /// the mode so the runtime can multiply runtime SDF visibility into the static
    /// term without double-shadowing.
    pub mode: LightmapMode,
}

/// Format tag for the irradiance blob. Uncompressed `Rgba16Float` (4 half-floats
/// per texel, `w·h·8` bytes total). Used in two cases: (1) the bake's debug
/// bypass (`LightmapConfig::uncompressed_irradiance = true`), and (2) all
/// placeholder sections (`LightmapSection::placeholder` always emits RGBA16F —
/// placeholders never go through BC6H). `from_bytes`/`to_bytes` read and write
/// the blob by the stored `irr_total_bytes`, not by recomputing per-layer `w·h·8`.
pub const IRRADIANCE_FORMAT_RGBA16F: u32 = 0;

/// Format tag for the irradiance blob. Block-compressed `Bc6hRgbUfloat` — 4×4
/// texel blocks, 16 bytes each (`ceil(w/4)·ceil(h/4)·16` total). RGB-only;
/// alpha is dropped at bake time and reconstructed as 1.0 by the shader's
/// `.rgb` swizzle (the shader never reads `.a` from the irradiance atlas).
/// The runtime branches on this tag to choose `Bc6hRgbUfloat` vs `Rgba16Float`
/// at texture creation; the BGL is sample-type only so both bind to the same
/// `Float { filterable: true }` slot through the linear sampler.
pub const IRRADIANCE_FORMAT_BC6H: u32 = 1;

/// Format tag for the direction blob. Only octahedral-in-Rgba8Unorm exists
/// today. The tag is stored in the header so a future encoder can add new
/// direction encodings without breaking existing parsers that reject unknown tags.
pub const DIRECTION_FORMAT_OCT_RGBA8: u32 = 0;

/// Magic tag introducing the bake-mode trailer, ASCII `"LMOD"` little-endian.
/// Trailer layout (8 bytes total) appended *after* the direction blob:
///
/// ```text
///   u32 trailer_magic (= LIGHTMAP_MODE_TRAILER_MAGIC)
///   u32 mode          (LightmapMode discriminant — 0 = shadowed, 1 = unshadowed)
/// ```
///
/// The base `from_bytes` parser only consumes `HEADER_SIZE + irr_total_bytes +
/// dir_total_bytes` bytes; trailing bytes are silently ignored. Sections written
/// without a trailer (shadowed bake) parse
/// as `LightmapMode::Shadowed` — `main`'s baked-shadow behavior. The shadowed
/// bake writes no trailer, preserving byte-for-byte parity with `main`.
pub const LIGHTMAP_MODE_TRAILER_MAGIC: u32 = u32::from_le_bytes(*b"LMOD");

/// Selects how the lightmap atlas was baked.
///
/// - `Shadowed` (default): static-light shadows are folded into the irradiance
///   value — the `main` bake. Texels occluded from a static light are dark.
/// - `Unshadowed`: full static-light irradiance + bounce with **no** visibility
///   term. Texels occluded from a static light still receive its full irradiance.
///   Runtime SDF supplies visibility separately to avoid double-shadowing.
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

impl LightmapSection {
    /// Build an empty placeholder section: 1×1 white irradiance + neutral
    /// direction. Used by the compiler when a map has no static lights so
    /// downstream consumers always see a valid section.
    pub fn placeholder() -> Self {
        // White irradiance (1.0, 1.0, 1.0, 1.0) as four half-floats.
        let one_half = f32_to_f16_bits(1.0);
        let mut irradiance = Vec::with_capacity(IRRADIANCE_TEXEL_BYTES);
        for _ in 0..4 {
            irradiance.extend_from_slice(&one_half.to_le_bytes());
        }
        // Neutral direction: encode (0, 1, 0) octahedral = (0.5, 1.0) remapped
        // to (128, 255). Alpha 0xFF.
        let direction = vec![128u8, 255, 128, 255];
        Self {
            layer_count: 1,
            irr_width: 1,
            irr_height: 1,
            irr_texel_density: 1.0,
            irradiance,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            dir_width: 1,
            dir_height: 1,
            dir_texel_density: 1.0,
            direction,
            mode: LightmapMode::Shadowed,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let irr_total_bytes = self.irradiance.len() as u32;
        let dir_total_bytes = self.direction.len() as u32;
        // Shadowed mode is the legacy/default — write no trailer so output is
        // byte-identical to `main` for the same bake inputs.
        let trailer_bytes = if self.mode == LightmapMode::Shadowed {
            0
        } else {
            8
        };
        let mut buf = Vec::with_capacity(
            HEADER_SIZE + self.irradiance.len() + self.direction.len() + trailer_bytes,
        );
        buf.extend_from_slice(&LIGHTMAP_SECTION_VERSION.to_le_bytes());
        buf.extend_from_slice(&self.layer_count.to_le_bytes());
        buf.extend_from_slice(&self.irr_width.to_le_bytes());
        buf.extend_from_slice(&self.irr_height.to_le_bytes());
        buf.extend_from_slice(&self.irr_texel_density.to_le_bytes());
        buf.extend_from_slice(&self.irradiance_format.to_le_bytes());
        buf.extend_from_slice(&irr_total_bytes.to_le_bytes());
        buf.extend_from_slice(&self.dir_width.to_le_bytes());
        buf.extend_from_slice(&self.dir_height.to_le_bytes());
        buf.extend_from_slice(&self.dir_texel_density.to_le_bytes());
        buf.extend_from_slice(&DIRECTION_FORMAT_OCT_RGBA8.to_le_bytes());
        buf.extend_from_slice(&dir_total_bytes.to_le_bytes());
        buf.extend_from_slice(&self.irradiance);
        buf.extend_from_slice(&self.direction);
        if self.mode != LightmapMode::Shadowed {
            buf.extend_from_slice(&LIGHTMAP_MODE_TRAILER_MAGIC.to_le_bytes());
            buf.extend_from_slice(&self.mode.as_u32().to_le_bytes());
        }
        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "lightmap section too short for header",
            )));
        }
        let version = u32::from_le_bytes(data[0..4].try_into().unwrap());
        // Pre-v2 sections carried no version field — their first u32 was `width`.
        // Reject anything that isn't the v2 marker, naming the value seen on disk.
        if version != LIGHTMAP_SECTION_VERSION {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "unsupported lightmap section version: {version} (expected {LIGHTMAP_SECTION_VERSION})"
                ),
            )));
        }
        let layer_count = u32::from_le_bytes(data[4..8].try_into().unwrap());
        let irr_width = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let irr_height = u32::from_le_bytes(data[12..16].try_into().unwrap());
        let irr_texel_density = f32::from_le_bytes(data[16..20].try_into().unwrap());
        let irr_format = u32::from_le_bytes(data[20..24].try_into().unwrap());
        let irr_total_bytes = u32::from_le_bytes(data[24..28].try_into().unwrap()) as usize;
        let dir_width = u32::from_le_bytes(data[28..32].try_into().unwrap());
        let dir_height = u32::from_le_bytes(data[32..36].try_into().unwrap());
        let dir_texel_density = f32::from_le_bytes(data[36..40].try_into().unwrap());
        let dir_format = u32::from_le_bytes(data[40..44].try_into().unwrap());
        let dir_total_bytes = u32::from_le_bytes(data[44..48].try_into().unwrap()) as usize;

        // Accept both the uncompressed RGBA16F layout and the BC6H block layout.
        // `from_bytes` reads the blob by the stored byte count, not by recomputing
        // block math, so the only per-format work here is gating the tag value.
        if irr_format != IRRADIANCE_FORMAT_RGBA16F && irr_format != IRRADIANCE_FORMAT_BC6H {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported lightmap irradiance format: {irr_format}"),
            )));
        }
        if dir_format != DIRECTION_FORMAT_OCT_RGBA8 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported lightmap direction format: {dir_format}"),
            )));
        }

        let expected = HEADER_SIZE + irr_total_bytes + dir_total_bytes;
        if data.len() < expected {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "lightmap section too short: need {expected} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let irr_start = HEADER_SIZE;
        let dir_start = irr_start + irr_total_bytes;
        let irradiance = data[irr_start..dir_start].to_vec();
        let direction = data[dir_start..dir_start + dir_total_bytes].to_vec();

        // Optional bake-mode trailer. Legacy PRLs (and shadowed-bake output) omit
        // it entirely; absence reads as `Shadowed` so missing-marker behavior
        // matches `main`. A present trailer's magic must match to be honored; any
        // other trailing bytes are ignored as forward-compat slack.
        let trailer_start = dir_start + dir_total_bytes;
        let mode = if data.len() >= trailer_start + 8 {
            let magic =
                u32::from_le_bytes(data[trailer_start..trailer_start + 4].try_into().unwrap());
            if magic == LIGHTMAP_MODE_TRAILER_MAGIC {
                let raw = u32::from_le_bytes(
                    data[trailer_start + 4..trailer_start + 8]
                        .try_into()
                        .unwrap(),
                );
                LightmapMode::from_u32(raw).ok_or_else(|| {
                    FormatError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unsupported lightmap mode: {raw}"),
                    ))
                })?
            } else {
                LightmapMode::Shadowed
            }
        } else {
            LightmapMode::Shadowed
        };

        Ok(Self {
            layer_count,
            irr_width,
            irr_height,
            irr_texel_density,
            irradiance,
            irradiance_format: irr_format,
            dir_width,
            dir_height,
            dir_texel_density,
            direction,
            mode,
        })
    }
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

/// Encode a unit direction as two 8-bit octahedral components + padding.
/// Matches the WGSL decoder: `oct * 2 - 1`, recover z via `1 - |x| - |y|`.
pub fn encode_direction_oct(dir: [f32; 3]) -> [u8; 4] {
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
    [qx, qy, 128, 255]
}

fn signum_nonzero(v: f32) -> f32 {
    if v >= 0.0 { 1.0 } else { -1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_placeholder() {
        let section = LightmapSection::placeholder();
        let bytes = section.to_bytes();
        let restored = LightmapSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_real_atlas_single_layer() {
        // Single-layer 2×2 atlas with distinct per-texel values.
        let mut irradiance = Vec::new();
        for i in 0..4 {
            let v = f32_to_f16_bits(i as f32 * 0.25);
            for _ in 0..4 {
                irradiance.extend_from_slice(&v.to_le_bytes());
            }
        }
        let direction = (0..4)
            .flat_map(|i| [i as u8 * 30, i as u8 * 20, 128, 255])
            .collect();
        let section = LightmapSection {
            layer_count: 1,
            irr_width: 2,
            irr_height: 2,
            irr_texel_density: 0.04,
            irradiance,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            dir_width: 2,
            dir_height: 2,
            dir_texel_density: 0.04,
            direction,
            mode: LightmapMode::Shadowed,
        };
        let bytes = section.to_bytes();
        let restored = LightmapSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_real_atlas_multi_layer() {
        // Three layers, with irradiance and direction sized differently to
        // exercise the decoupled dimensions. Irradiance: 2×2 RGBA16F per layer
        // (2·2·8 = 32 bytes/layer). Direction: 4×2 RGBA8 per layer (4·2·4 = 32
        // bytes/layer). Blobs are layer-major.
        let layer_count = 3u32;
        let irr_w = 2u32;
        let irr_h = 2u32;
        let dir_w = 4u32;
        let dir_h = 2u32;

        let mut irradiance = Vec::new();
        for layer in 0..layer_count {
            for texel in 0..(irr_w * irr_h) {
                let v = f32_to_f16_bits((layer * 4 + texel) as f32 * 0.1);
                for _ in 0..4 {
                    irradiance.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        let mut direction = Vec::new();
        for layer in 0..layer_count {
            for texel in 0..(dir_w * dir_h) {
                let base = (layer * 8 + texel) as u8;
                direction.extend_from_slice(&[base, base.wrapping_add(7), 128, 255]);
            }
        }

        let section = LightmapSection {
            layer_count,
            irr_width: irr_w,
            irr_height: irr_h,
            irr_texel_density: 0.04,
            irradiance,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            dir_width: dir_w,
            dir_height: dir_h,
            dir_texel_density: 0.08,
            direction,
            mode: LightmapMode::Shadowed,
        };
        let bytes = section.to_bytes();
        let restored = LightmapSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
        assert_eq!(restored.layer_count, 3);
    }

    #[test]
    fn shadowed_mode_writes_no_trailer_byte_identical_to_pre_trailer_layout() {
        // The shadowed bake is `main`'s behavior. Output must be byte-identical to
        // the pre-trailer encoding (header + irradiance + direction, nothing more)
        // so a build with neither new prl-build flag set produces no output drift.
        let section = LightmapSection::placeholder();
        let bytes = section.to_bytes();
        let expected_len = HEADER_SIZE + section.irradiance.len() + section.direction.len();
        assert_eq!(
            bytes.len(),
            expected_len,
            "shadowed mode must write zero trailer bytes",
        );
    }

    #[test]
    fn unshadowed_mode_round_trips_via_trailer_multi_layer() {
        // Multi-layer section so the trailer offset lands past concatenated
        // layer-major blobs, not just a single layer.
        let layer_count = 2u32;
        let irr_w = 2u32;
        let irr_h = 2u32;
        let dir_w = 2u32;
        let dir_h = 2u32;
        let mut irradiance = Vec::new();
        for layer in 0..layer_count {
            for texel in 0..(irr_w * irr_h) {
                let v = f32_to_f16_bits((layer * 4 + texel) as f32 * 0.2);
                for _ in 0..4 {
                    irradiance.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        let direction: Vec<u8> = (0..(layer_count * dir_w * dir_h))
            .flat_map(|i| [i as u8, (i as u8).wrapping_add(3), 128, 255])
            .collect();
        let section = LightmapSection {
            layer_count,
            irr_width: irr_w,
            irr_height: irr_h,
            irr_texel_density: 0.04,
            irradiance,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            dir_width: dir_w,
            dir_height: dir_h,
            dir_texel_density: 0.04,
            direction,
            mode: LightmapMode::Unshadowed,
        };
        let bytes = section.to_bytes();
        let restored = LightmapSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored.mode, LightmapMode::Unshadowed);
        assert_eq!(section, restored);
    }

    #[test]
    fn legacy_section_without_trailer_reads_as_shadowed() {
        // Bytes produced by a pre-trailer encoder: no magic, no mode field.
        let section = LightmapSection::placeholder();
        let bytes = section.to_bytes(); // shadowed → no trailer
        let restored = LightmapSection::from_bytes(&bytes).unwrap();
        assert_eq!(
            restored.mode,
            LightmapMode::Shadowed,
            "missing trailer must read as shadowed (main's behavior)",
        );
    }

    #[test]
    fn round_trip_bc6h_section_preserves_block_blob() {
        // 8×4 atlas → 2×1 BC6H blocks → 32 bytes of block payload. The header
        // stores `irr_len` directly, so `from_bytes` reads the blob by length
        // without recomputing block math — the test asserts the round-trip
        // reproduces dimensions, the tag, and the block-sized blob exactly.
        let irradiance: Vec<u8> = (0..32).collect();
        let direction: Vec<u8> = (0..32 * 4).map(|i| (i & 0xff) as u8).collect();
        let section = LightmapSection {
            layer_count: 1,
            irr_width: 8,
            irr_height: 4,
            irr_texel_density: 0.04,
            irradiance,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
            dir_width: 8,
            dir_height: 4,
            dir_texel_density: 0.04,
            direction,
            mode: LightmapMode::Shadowed,
        };
        let bytes = section.to_bytes();
        let restored = LightmapSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, section);
        assert_eq!(restored.irradiance_format, IRRADIANCE_FORMAT_BC6H);
    }

    #[test]
    fn from_bytes_still_accepts_rgba16f_tag() {
        // Sibling AC: BC6H acceptance must NOT regress the legacy uncompressed
        // tag. Placeholder is RGBA16F; it must continue to decode cleanly.
        let section = LightmapSection::placeholder();
        let bytes = section.to_bytes();
        let restored = LightmapSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored.irradiance_format, IRRADIANCE_FORMAT_RGBA16F);
    }

    #[test]
    fn rejects_unsupported_irradiance_format() {
        let mut section = LightmapSection::placeholder();
        let mut bytes = section.to_bytes();
        // Corrupt irradiance format tag at v2 offset 20..24.
        bytes[20..24].copy_from_slice(&99u32.to_le_bytes());
        assert!(LightmapSection::from_bytes(&bytes).is_err());
        // Also corrupt direction format tag at v2 offset 40..44.
        bytes = section.to_bytes();
        bytes[40..44].copy_from_slice(&7u32.to_le_bytes());
        assert!(LightmapSection::from_bytes(&bytes).is_err());
        // Suppress unused-must-use warning on `section`.
        section.layer_count = 1;
        let _ = section;
    }

    #[test]
    fn rejects_pre_v2_section() {
        // Pre-v2 sections had no version field — their first u32 was `width`,
        // typically a sizeable atlas dimension. Feed a realistic pre-v2 28-byte
        // header (width=1024, height=1024, density, RGBA16F, OCT_RGBA8, irr_len,
        // dir_len) plus a body. `from_bytes` reads the leading 1024 as the
        // version and must reject it as InvalidData.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1024u32.to_le_bytes()); // width (read as version)
        bytes.extend_from_slice(&1024u32.to_le_bytes()); // height
        bytes.extend_from_slice(&0.04f32.to_le_bytes()); // texel_density
        bytes.extend_from_slice(&IRRADIANCE_FORMAT_RGBA16F.to_le_bytes());
        bytes.extend_from_slice(&DIRECTION_FORMAT_OCT_RGBA8.to_le_bytes());
        bytes.extend_from_slice(&64u32.to_le_bytes()); // irr_len
        bytes.extend_from_slice(&32u32.to_le_bytes()); // dir_len
        bytes.extend_from_slice(&[0u8; 64]); // irradiance body
        bytes.extend_from_slice(&[0u8; 32]); // direction body

        let err = LightmapSection::from_bytes(&bytes).unwrap_err();
        match err {
            FormatError::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidData),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn rejects_truncated_body() {
        let bytes = LightmapSection::placeholder().to_bytes();
        let truncated = &bytes[..bytes.len() - 2];
        assert!(LightmapSection::from_bytes(truncated).is_err());
    }

    #[test]
    fn encode_direction_axis_round_trip() {
        // +Y should map close to the neutral placeholder (128, 255, 128, 255).
        let enc = encode_direction_oct([0.0, 1.0, 0.0]);
        assert_eq!(enc[0], 128);
        assert_eq!(enc[1], 255);
        assert_eq!(enc[3], 255);
    }

    #[test]
    fn f32_to_f16_known_values() {
        assert_eq!(f32_to_f16_bits(0.0), 0x0000);
        assert_eq!(f32_to_f16_bits(1.0), 0x3c00);
        assert_eq!(f32_to_f16_bits(-1.0), 0xbc00);
    }
}
