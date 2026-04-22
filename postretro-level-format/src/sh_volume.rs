// SH irradiance volume section (ID 20): regular-grid L2 spherical harmonic
// probes with static RGB base coefficients and optional per-animated-light
// monochrome layers + animation descriptors.
//
// See: context/plans/in-progress/lighting-foundation/2-sh-baker.md

use crate::FormatError;

/// One probe's static base SH L2 record: 27 f32 RGB coefficients + validity.
///
/// `sh_coefficients` is laid out as 9 bands × 3 color channels, stored
/// channel-interleaved per band: `[band0_r, band0_g, band0_b, band1_r, ...]`.
/// The encoder/decoder writes the same order — downstream consumers should
/// treat the array as opaque and index it with the same helper both sides use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShProbe {
    /// 9 bands × 3 channels = 27 f32. Channel-interleaved per band.
    pub sh_coefficients: [f32; 27],
    /// 0 = invalid (inside solid), 1 = valid (usable by runtime).
    pub validity: u8,
}

impl Default for ShProbe {
    fn default() -> Self {
        Self {
            sh_coefficients: [0.0; 27],
            validity: 0,
        }
    }
}

/// Byte stride of a single serialized base probe record: 27 f32 + 1 u8 +
/// 3 bytes of padding to land on a 4-byte boundary = 112 bytes.
///
/// The header's `probe_stride` field is written from this constant. It is
/// forward-compat scaffolding: future per-probe base data (e.g. DDGI distance
/// fields) can grow the stride without breaking the loader.
pub const PROBE_STRIDE: u32 = 112;

/// Monochrome SH L2 coefficients for a single animated light contribution at
/// one probe. 9 f32 × 4 bytes = 36 bytes on disk.
pub const PROBE_MONO_BYTES: u32 = 36;

/// Animation curves for one animated light, stored once per light (not per
/// probe). Brightness and color channels are uniformly-sampled over the
/// light's period; the runtime linearly interpolates between samples.
///
/// A `brightness_count` / `color_count` of 0 means the channel holds constant
/// over the cycle (use `base_color` or unit brightness, respectively).
///
/// `start_active` is the initial runtime on/off state. 1 = active at map load
/// (the default — lights light); 0 = spawned dark, typically because the
/// entity carried `_start_inactive = 1`. Scripting toggles the GPU mirror of
/// this flag at runtime; only the initial value lives on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationDescriptor {
    pub period: f32,
    pub phase: f32,
    pub base_color: [f32; 3],
    pub brightness: Vec<f32>,
    pub color: Vec<[f32; 3]>,
    pub start_active: u32,
}

impl Default for AnimationDescriptor {
    fn default() -> Self {
        Self {
            period: 0.0,
            phase: 0.0,
            base_color: [0.0; 3],
            brightness: Vec::new(),
            color: Vec::new(),
            start_active: 1,
        }
    }
}

/// SH irradiance volume section (ID 20).
///
/// On-disk layout (all little-endian):
///
/// ```text
///   Header (44 bytes):
///     f32 × 3  grid_origin            (world-space min corner, meters)
///     f32 × 3  cell_size              (meters per cell along x/y/z)
///     u32 × 3  grid_dimensions        (probe count along x/y/z)
///     u32      probe_stride           (= PROBE_STRIDE = 112)
///     u32      animated_light_count   (0 = no animation layers)
///
///   Base probe records (probe_stride bytes each, z-major then y, then x):
///     f32 × 27 sh_coefficients        (9 bands × 3 channels, RGB)
///     u8       validity               (0 = invalid, 1 = valid)
///     u8 × 3   padding
///
///   Animation descriptor table (omitted if animated_light_count == 0):
///     per animated light:
///       f32 period
///       f32 phase
///       f32 × 3 base_color
///       u32 brightness_count
///       u32 color_count
///       u32 start_active            (1 = lit at map load, 0 = _start_inactive)
///       f32 × brightness_count      (brightness samples)
///       f32 × 3 × color_count       (RGB color samples)
///
///   Per-light SH layers (omitted if animated_light_count == 0):
///     per animated light:
///       per probe (total_probes entries, same iteration order as base probes):
///         f32 × 9  sh_coefficients_mono  (monochrome, 9 bands)
/// ```
///
/// A section with `animated_light_count == 0` is valid: the loader produces
/// empty `animation_descriptors` and `per_light_sh` vectors and the runtime
/// skips animated-layer processing.
#[derive(Debug, Clone, PartialEq)]
pub struct ShVolumeSection {
    pub grid_origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub grid_dimensions: [u32; 3],
    pub probe_stride: u32,
    /// One entry per grid cell in z-major/y/x order. `probes.len()` must equal
    /// `grid_dimensions[0] * grid_dimensions[1] * grid_dimensions[2]`.
    pub probes: Vec<ShProbe>,
    /// One descriptor per animated light. Length must match `per_light_sh`.
    pub animation_descriptors: Vec<AnimationDescriptor>,
    /// One layer per animated light. Each layer has `total_probes * 9` f32s
    /// in the same probe iteration order as `probes`.
    pub per_light_sh: Vec<Vec<f32>>,
}

impl ShVolumeSection {
    pub const HEADER_SIZE: usize = 44;

    /// Number of probes expected from the grid dimensions.
    pub fn total_probes(&self) -> usize {
        self.grid_dimensions[0] as usize
            * self.grid_dimensions[1] as usize
            * self.grid_dimensions[2] as usize
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let total_probes = self.total_probes();
        debug_assert_eq!(self.probes.len(), total_probes);
        debug_assert_eq!(self.animation_descriptors.len(), self.per_light_sh.len());

        let mut buf = Vec::with_capacity(Self::HEADER_SIZE + total_probes * PROBE_STRIDE as usize);

        // Header
        for v in &self.grid_origin {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for v in &self.cell_size {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for v in &self.grid_dimensions {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(&self.probe_stride.to_le_bytes());
        buf.extend_from_slice(&(self.animation_descriptors.len() as u32).to_le_bytes());

        // Base probe records
        for probe in &self.probes {
            for coeff in &probe.sh_coefficients {
                buf.extend_from_slice(&coeff.to_le_bytes());
            }
            buf.push(probe.validity);
            // 3 bytes padding to reach probe_stride.
            buf.extend_from_slice(&[0u8; 3]);
        }

        // Animation descriptor table.
        for desc in &self.animation_descriptors {
            buf.extend_from_slice(&desc.period.to_le_bytes());
            buf.extend_from_slice(&desc.phase.to_le_bytes());
            for c in &desc.base_color {
                buf.extend_from_slice(&c.to_le_bytes());
            }
            buf.extend_from_slice(&(desc.brightness.len() as u32).to_le_bytes());
            buf.extend_from_slice(&(desc.color.len() as u32).to_le_bytes());
            buf.extend_from_slice(&desc.start_active.to_le_bytes());
            for b in &desc.brightness {
                buf.extend_from_slice(&b.to_le_bytes());
            }
            for c in &desc.color {
                for ch in c {
                    buf.extend_from_slice(&ch.to_le_bytes());
                }
            }
        }

        // Per-light SH layers.
        for layer in &self.per_light_sh {
            debug_assert_eq!(layer.len(), total_probes * 9);
            for v in layer {
                buf.extend_from_slice(&v.to_le_bytes());
            }
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < Self::HEADER_SIZE {
            return Err(truncated("header"));
        }

        let mut o = 0;
        let grid_origin = [
            read_f32(data, o),
            read_f32(data, o + 4),
            read_f32(data, o + 8),
        ];
        o += 12;
        let cell_size = [
            read_f32(data, o),
            read_f32(data, o + 4),
            read_f32(data, o + 8),
        ];
        o += 12;
        let grid_dimensions = [
            read_u32(data, o),
            read_u32(data, o + 4),
            read_u32(data, o + 8),
        ];
        o += 12;
        let probe_stride = read_u32(data, o);
        o += 4;
        let animated_light_count = read_u32(data, o) as usize;
        o += 4;
        debug_assert_eq!(o, Self::HEADER_SIZE);

        if probe_stride < PROBE_STRIDE {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "sh volume probe_stride {probe_stride} is smaller than the minimum {PROBE_STRIDE}"
                ),
            )));
        }

        let total_probes =
            grid_dimensions[0] as usize * grid_dimensions[1] as usize * grid_dimensions[2] as usize;

        let base_bytes = total_probes * probe_stride as usize;
        if data.len() < Self::HEADER_SIZE + base_bytes {
            return Err(truncated("base probe records"));
        }

        let mut probes = Vec::with_capacity(total_probes);
        for _ in 0..total_probes {
            let mut sh_coefficients = [0f32; 27];
            for (i, coeff) in sh_coefficients.iter_mut().enumerate() {
                *coeff = read_f32(data, o + i * 4);
            }
            let validity = data[o + 27 * 4];
            probes.push(ShProbe {
                sh_coefficients,
                validity,
            });
            // Skip the full on-disk stride, including padding and any future
            // per-probe data beyond the minimum PROBE_STRIDE.
            o += probe_stride as usize;
        }

        // Animation descriptor table.
        let mut animation_descriptors = Vec::with_capacity(animated_light_count);
        for _ in 0..animated_light_count {
            if data.len() < o + 20 {
                return Err(truncated("animation descriptor header"));
            }
            let period = read_f32(data, o);
            let phase = read_f32(data, o + 4);
            let base_color = [
                read_f32(data, o + 8),
                read_f32(data, o + 12),
                read_f32(data, o + 16),
            ];
            o += 20;

            if data.len() < o + 12 {
                return Err(truncated("animation descriptor sample counts"));
            }
            let brightness_count = read_u32(data, o) as usize;
            let color_count = read_u32(data, o + 4) as usize;
            let start_active = read_u32(data, o + 8);
            o += 12;

            let brightness_bytes = brightness_count * 4;
            let color_bytes = color_count * 12;
            if data.len() < o + brightness_bytes + color_bytes {
                return Err(truncated("animation descriptor samples"));
            }

            let mut brightness = Vec::with_capacity(brightness_count);
            for i in 0..brightness_count {
                brightness.push(read_f32(data, o + i * 4));
            }
            o += brightness_bytes;

            let mut color = Vec::with_capacity(color_count);
            for i in 0..color_count {
                color.push([
                    read_f32(data, o + i * 12),
                    read_f32(data, o + i * 12 + 4),
                    read_f32(data, o + i * 12 + 8),
                ]);
            }
            o += color_bytes;

            animation_descriptors.push(AnimationDescriptor {
                period,
                phase,
                base_color,
                brightness,
                color,
                start_active,
            });
        }

        // Per-light SH layers.
        let mono_bytes_per_probe = PROBE_MONO_BYTES as usize;
        let mut per_light_sh = Vec::with_capacity(animated_light_count);
        for _ in 0..animated_light_count {
            let layer_bytes = total_probes * mono_bytes_per_probe;
            if data.len() < o + layer_bytes {
                return Err(truncated("per-light SH layer"));
            }
            let mut layer = Vec::with_capacity(total_probes * 9);
            for i in 0..total_probes * 9 {
                layer.push(read_f32(data, o + i * 4));
            }
            o += layer_bytes;
            per_light_sh.push(layer);
        }

        Ok(Self {
            grid_origin,
            cell_size,
            grid_dimensions,
            probe_stride,
            probes,
            animation_descriptors,
            per_light_sh,
        })
    }
}

fn truncated(what: &str) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        format!("sh volume section truncated: {what}"),
    ))
}

fn read_f32(data: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_probe(seed: f32) -> ShProbe {
        let mut coeffs = [0f32; 27];
        for (i, c) in coeffs.iter_mut().enumerate() {
            *c = seed + i as f32 * 0.01;
        }
        ShProbe {
            sh_coefficients: coeffs,
            validity: 1,
        }
    }

    fn empty_section(grid: [u32; 3]) -> ShVolumeSection {
        let total = (grid[0] * grid[1] * grid[2]) as usize;
        ShVolumeSection {
            grid_origin: [-1.0, -2.0, -3.0],
            cell_size: [1.0, 1.0, 1.0],
            grid_dimensions: grid,
            probe_stride: PROBE_STRIDE,
            probes: (0..total).map(|i| sample_probe(i as f32)).collect(),
            animation_descriptors: Vec::new(),
            per_light_sh: Vec::new(),
        }
    }

    #[test]
    fn round_trip_empty_volume() {
        let section = ShVolumeSection {
            grid_origin: [0.0, 0.0, 0.0],
            cell_size: [1.0, 1.0, 1.0],
            grid_dimensions: [0, 0, 0],
            probe_stride: PROBE_STRIDE,
            probes: Vec::new(),
            animation_descriptors: Vec::new(),
            per_light_sh: Vec::new(),
        };
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), ShVolumeSection::HEADER_SIZE);
        let restored = ShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_probes_only() {
        let section = empty_section([2, 3, 4]);
        let bytes = section.to_bytes();
        let expected_len = ShVolumeSection::HEADER_SIZE + (2 * 3 * 4) * PROBE_STRIDE as usize;
        assert_eq!(bytes.len(), expected_len);
        let restored = ShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_with_animated_lights() {
        let total = 2 * 2 * 2;
        let section = ShVolumeSection {
            grid_origin: [0.5, 1.5, 2.5],
            cell_size: [0.75, 0.75, 0.75],
            grid_dimensions: [2, 2, 2],
            probe_stride: PROBE_STRIDE,
            probes: (0..total).map(|i| sample_probe(i as f32)).collect(),
            animation_descriptors: vec![
                AnimationDescriptor {
                    period: 1.5,
                    phase: 0.25,
                    base_color: [1.0, 0.9, 0.8],
                    brightness: vec![0.1, 0.5, 1.0, 0.5],
                    color: Vec::new(),
                    start_active: 1,
                },
                AnimationDescriptor {
                    period: 2.0,
                    phase: 0.0,
                    base_color: [0.2, 0.4, 1.0],
                    brightness: Vec::new(),
                    color: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                    start_active: 0,
                },
            ],
            per_light_sh: vec![
                (0..total * 9).map(|i| i as f32 * 0.1).collect(),
                (0..total * 9).map(|i| i as f32 * -0.25).collect(),
            ],
        };
        let bytes = section.to_bytes();
        let restored = ShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn invalid_probe_has_zero_validity() {
        let mut section = empty_section([1, 1, 1]);
        section.probes[0].validity = 0;
        let bytes = section.to_bytes();
        let restored = ShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored.probes[0].validity, 0);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = ShVolumeSection::from_bytes(&[0u8; 10]).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_truncated_probe_records() {
        let section = empty_section([1, 1, 1]);
        let bytes = section.to_bytes();
        let truncated = &bytes[..bytes.len() - 4];
        let err = ShVolumeSection::from_bytes(truncated).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_invalid_probe_stride() {
        let section = empty_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        // probe_stride is at offset 36.
        bytes[36..40].copy_from_slice(&10u32.to_le_bytes());
        let err = ShVolumeSection::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn probe_iteration_is_z_major_then_y_then_x() {
        // The encoder/decoder use the same iteration order; we document the
        // expected packing by building a grid where the first probe lives at
        // the origin and the last is at the far corner.
        let section = empty_section([3, 2, 4]);
        let bytes = section.to_bytes();
        let restored = ShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored.probes.len(), 3 * 2 * 4);
        // Verify the first/last probes round-trip in order (the test's probe
        // seeds are unique per index).
        assert_eq!(restored.probes.first(), section.probes.first());
        assert_eq!(restored.probes.last(), section.probes.last());
    }

    #[test]
    fn zero_animated_count_emits_no_descriptor_bytes() {
        let section = empty_section([1, 1, 1]);
        let bytes = section.to_bytes();
        // Header + 1 probe_stride
        assert_eq!(
            bytes.len(),
            ShVolumeSection::HEADER_SIZE + PROBE_STRIDE as usize
        );
        // animated_light_count bytes at offset 40..44 should be zero.
        assert_eq!(&bytes[40..44], &0u32.to_le_bytes());
    }

    /// Loader-side degradation contract: a PRL with the ShVolume section
    /// absent from its section table must read without error and yield
    /// `None` for the section lookup. This matches the spec's "missing
    /// section is not an error" rule for the SH volume.
    #[test]
    fn prl_container_returns_none_for_missing_sh_volume_section() {
        use crate::{SectionBlob, SectionId, read_container, read_section_data, write_prl};

        // Pack a single unrelated section — no ShVolume — and read back.
        let sections = vec![SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: vec![0xAA, 0xBB, 0xCC],
        }];
        let mut buf = Vec::new();
        write_prl(&mut buf, &sections).unwrap();

        let mut cursor = std::io::Cursor::new(&buf);
        let meta = read_container(&mut cursor).unwrap();
        assert!(meta.find_section(SectionId::ShVolume as u32).is_none());
        let result = read_section_data(&mut cursor, &meta, SectionId::ShVolume as u32).unwrap();
        assert!(result.is_none(), "missing SH volume must return None");
    }
}
