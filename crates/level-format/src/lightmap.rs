// Directional lightmap section (ID 22): atlas-packed per-texel irradiance plus
// dominant incoming-light direction for static (non-dynamic) lights.
// See: context/lib/rendering_pipeline.md §4, context/lib/build_pipeline.md §PRL

use crate::FormatError;

/// Byte stride of one irradiance texel on disk: RGBA16F, 4 half-floats = 8 bytes.
/// Alpha is currently unused (carries no AO term yet) and is written as 1.0 so
/// fallback samplers that read alpha don't misinterpret it as transparency.
pub const IRRADIANCE_TEXEL_BYTES: usize = 8;

/// Byte stride of one direction texel on disk: RGBA8Unorm, 4 bytes.
/// `rgb` holds the octahedral-encoded unit direction (remapped 0..1 → −1..1 on
/// sample); `a` is padding and carries 0xFF so bilinear sampling of edge
/// texels into unused neighbours doesn't corrupt direction decode.
pub const DIRECTION_TEXEL_BYTES: usize = 4;

/// Fixed 28-byte header preceding the two texel blobs.
const HEADER_SIZE: usize = 28;

/// Directional lightmap atlas.
///
/// On-disk layout (all little-endian):
///
/// ```text
///   Header (28 bytes):
///     u32 width
///     u32 height
///     f32 texel_density_meters   (world-space size of one atlas texel)
///     u32 irradiance_format      (0 = Rgba16Float)
///     u32 direction_format       (0 = Rgba8Unorm octahedral)
///     u32 irradiance_byte_count
///     u32 direction_byte_count
///
///   Irradiance blob (irradiance_byte_count bytes):
///     u16 × 4 × width × height   row-major (y * width + x)
///
///   Direction blob (direction_byte_count bytes):
///     u8 × 4 × width × height    row-major (y * width + x)
/// ```
///
/// Irradiance texels for atlas positions not covered by any face chart are
/// zero. Edge dilation is applied at bake time so bilinear sampling at chart
/// boundaries pulls valid neighbours instead of black.
#[derive(Debug, Clone, PartialEq)]
pub struct LightmapSection {
    pub width: u32,
    pub height: u32,
    /// World-space meters-per-texel used at bake time. Informational — the
    /// runtime samples the atlas through per-vertex lightmap UVs and does not
    /// need to derive world-space sizes from this field.
    pub texel_density: f32,
    /// Raw bytes for irradiance texels (Rgba16Float, row-major).
    pub irradiance: Vec<u8>,
    /// Raw bytes for direction texels (Rgba8Unorm, octahedral, row-major).
    pub direction: Vec<u8>,
    /// Which bake produced the irradiance. `Shadowed` (default) folds static-light
    /// shadows into irradiance — the `main` behavior, written without a trailer so
    /// output is byte-identical to `main`. `Unshadowed` writes a trailer recording
    /// the mode so the runtime can multiply runtime SDF visibility into the static
    /// term without double-shadowing.
    pub mode: LightmapMode,
}

/// Format tag for the irradiance blob. Only `Rgba16Float` exists today; the
/// field is versioned so future bakes can introduce compressed variants
/// (BC6H, RGBM) without a new section ID.
pub const IRRADIANCE_FORMAT_RGBA16F: u32 = 0;

/// Format tag for the direction blob. Only octahedral-in-Rgba8Unorm exists
/// today; same forward-compat rationale as `IRRADIANCE_FORMAT_RGBA16F`.
pub const DIRECTION_FORMAT_OCT_RGBA8: u32 = 0;

/// Magic tag introducing the bake-mode trailer, ASCII `"LMOD"` little-endian.
/// Trailer layout (8 bytes total) appended *after* the direction blob:
///
/// ```text
///   u32 trailer_magic (= LIGHTMAP_MODE_TRAILER_MAGIC)
///   u32 mode          (LightmapMode discriminant — 0 = shadowed, 1 = unshadowed)
/// ```
///
/// The base `from_bytes` parser only consumes `HEADER_SIZE + irr_len + dir_len`
/// bytes; trailing bytes are silently ignored. Legacy PRLs (no trailer) parse
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
            width: 1,
            height: 1,
            texel_density: 1.0,
            irradiance,
            direction,
            mode: LightmapMode::Shadowed,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let irr_len = self.irradiance.len() as u32;
        let dir_len = self.direction.len() as u32;
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
        buf.extend_from_slice(&self.width.to_le_bytes());
        buf.extend_from_slice(&self.height.to_le_bytes());
        buf.extend_from_slice(&self.texel_density.to_le_bytes());
        buf.extend_from_slice(&IRRADIANCE_FORMAT_RGBA16F.to_le_bytes());
        buf.extend_from_slice(&DIRECTION_FORMAT_OCT_RGBA8.to_le_bytes());
        buf.extend_from_slice(&irr_len.to_le_bytes());
        buf.extend_from_slice(&dir_len.to_le_bytes());
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
        let width = u32::from_le_bytes(data[0..4].try_into().unwrap());
        let height = u32::from_le_bytes(data[4..8].try_into().unwrap());
        let texel_density = f32::from_le_bytes(data[8..12].try_into().unwrap());
        let irr_format = u32::from_le_bytes(data[12..16].try_into().unwrap());
        let dir_format = u32::from_le_bytes(data[16..20].try_into().unwrap());
        let irr_len = u32::from_le_bytes(data[20..24].try_into().unwrap()) as usize;
        let dir_len = u32::from_le_bytes(data[24..28].try_into().unwrap()) as usize;

        if irr_format != IRRADIANCE_FORMAT_RGBA16F {
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

        let expected = HEADER_SIZE + irr_len + dir_len;
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
        let dir_start = irr_start + irr_len;
        let irradiance = data[irr_start..dir_start].to_vec();
        let direction = data[dir_start..dir_start + dir_len].to_vec();

        // Optional bake-mode trailer. Legacy PRLs (and shadowed-bake output) omit
        // it entirely; absence reads as `Shadowed` so missing-marker behavior
        // matches `main`. A present trailer's magic must match to be honored; any
        // other trailing bytes are ignored as forward-compat slack.
        let trailer_start = dir_start + dir_len;
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
            width,
            height,
            texel_density,
            irradiance,
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
    fn round_trip_real_atlas() {
        // Small 2×2 atlas with distinct per-texel values.
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
            width: 2,
            height: 2,
            texel_density: 0.04,
            irradiance,
            direction,
            mode: LightmapMode::Shadowed,
        };
        let bytes = section.to_bytes();
        let restored = LightmapSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
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
    fn unshadowed_mode_round_trips_via_trailer() {
        let mut section = LightmapSection::placeholder();
        section.mode = LightmapMode::Unshadowed;
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
    fn rejects_unsupported_irradiance_format() {
        let mut section = LightmapSection::placeholder();
        let mut bytes = section.to_bytes();
        // Corrupt irradiance format tag at offset 12..16.
        bytes[12..16].copy_from_slice(&99u32.to_le_bytes());
        assert!(LightmapSection::from_bytes(&bytes).is_err());
        // Also corrupt direction format tag at offset 16..20.
        bytes = section.to_bytes();
        bytes[16..20].copy_from_slice(&7u32.to_le_bytes());
        assert!(LightmapSection::from_bytes(&bytes).is_err());
        // Suppress unused-must-use warning on `section`.
        section.width = 1;
        let _ = section;
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
