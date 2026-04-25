// Delta SH volumes section (ID 27): per-animated-light SH probe grids
// representing each light's contribution at peak brightness (brightness = 1.0,
// base color). Consumed by the runtime compose pass that blends animated lights
// into the SH irradiance volume.
//
// See: context/plans/in-progress/lighting-animated-sh/

use crate::FormatError;
use crate::lightmap::f32_to_f16_bits;

/// Section-internal version, written as the first byte of the payload. Bumped
/// whenever the on-disk layout changes so the loader can reject stale `.prl`
/// files instead of silently misreading them.
pub const DELTA_SH_VOLUMES_VERSION: u8 = 1;

/// Number of f16 values per probe: 9 SH bands × 3 color channels.
pub const PROBE_F16_COUNT: usize = 27;

/// Byte stride of one serialized probe: 27 × f16 = 54 bytes.
pub const PROBE_BYTES: usize = PROBE_F16_COUNT * 2;

/// One probe in a delta SH grid. Stores 9 SH bands × RGB as f16, packed in the
/// same channel-interleaved order as the base SH section's f32 layout:
/// `[band0_r, band0_g, band0_b, band1_r, ...]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeltaShProbe {
    pub sh_coefficients_f16: [u16; PROBE_F16_COUNT],
}

impl Default for DeltaShProbe {
    fn default() -> Self {
        Self {
            sh_coefficients_f16: [0; PROBE_F16_COUNT],
        }
    }
}

impl DeltaShProbe {
    /// Build a probe from f32 RGB SH coefficients, rounding to f16.
    pub fn from_f32(coeffs: &[f32; PROBE_F16_COUNT]) -> Self {
        let mut out = [0u16; PROBE_F16_COUNT];
        for (dst, src) in out.iter_mut().zip(coeffs.iter()) {
            *dst = f32_to_f16_bits(*src);
        }
        Self {
            sh_coefficients_f16: out,
        }
    }
}

/// Per-light delta volume: AABB-aligned grid header plus probe data.
///
/// Each animated light gets its own grid, sized to cover the light's influence
/// sphere AABB at the bake's chosen probe spacing (default 1.0m).
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaLightGrid {
    /// World-space min corner of the grid AABB, in meters.
    pub aabb_origin: [f32; 3],
    /// Uniform cell size along x/y/z, in meters.
    pub cell_size: f32,
    /// Probe count along x/y/z. The probe array has length `dims.0 * dims.1 *
    /// dims.2`, indexed in z-major then y then x order — same convention as
    /// the base SH volume.
    pub grid_dimensions: [u32; 3],
    /// Per-cell probes, z-major then y then x.
    pub probes: Vec<DeltaShProbe>,
}

impl DeltaLightGrid {
    /// Number of probes implied by the grid dimensions.
    pub fn total_probes(&self) -> usize {
        self.grid_dimensions[0] as usize
            * self.grid_dimensions[1] as usize
            * self.grid_dimensions[2] as usize
    }
}

/// Top-level header: version + animated light count + per-light descriptor
/// index table.
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaShVolumeHeader {
    /// One entry per animated light: index into the SH section's
    /// `animation_descriptors` array.
    pub animation_descriptor_indices: Vec<u32>,
}

/// Delta SH volumes section (ID 27).
///
/// On-disk layout (all little-endian):
///
/// ```text
///   Header:
///     u8   version                       (= DELTA_SH_VOLUMES_VERSION)
///     u32  animated_light_count
///     u32 × animated_light_count         AnimationDescriptorIndex table
///
///   Per light (× animated_light_count):
///     Per-light grid header (28 bytes):
///       f32 × 3 aabb_origin              (world-space min corner, meters)
///       f32     cell_size                (meters per cell, uniform)
///       u32 × 3 grid_dimensions          (probe count along x/y/z)
///     Probe data:
///       u16 × 27 × probe_count           (9 bands × RGB, f16, z-major y x)
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaShVolumesSection {
    pub header: DeltaShVolumeHeader,
    /// One grid per animated light. `grids.len()` must equal
    /// `header.animation_descriptor_indices.len()`.
    pub grids: Vec<DeltaLightGrid>,
}

impl DeltaShVolumesSection {
    /// Per-light grid header size on disk: 3×f32 + f32 + 3×u32 = 28 bytes.
    pub const GRID_HEADER_SIZE: usize = 28;

    pub fn to_bytes(&self) -> Vec<u8> {
        debug_assert_eq!(
            self.header.animation_descriptor_indices.len(),
            self.grids.len()
        );

        let mut buf = Vec::new();

        // Top-level header.
        buf.push(DELTA_SH_VOLUMES_VERSION);
        let count = self.grids.len() as u32;
        buf.extend_from_slice(&count.to_le_bytes());
        for idx in &self.header.animation_descriptor_indices {
            buf.extend_from_slice(&idx.to_le_bytes());
        }

        // Per-light grids.
        for grid in &self.grids {
            debug_assert_eq!(grid.probes.len(), grid.total_probes());

            for v in &grid.aabb_origin {
                buf.extend_from_slice(&v.to_le_bytes());
            }
            buf.extend_from_slice(&grid.cell_size.to_le_bytes());
            for v in &grid.grid_dimensions {
                buf.extend_from_slice(&v.to_le_bytes());
            }

            for probe in &grid.probes {
                for c in &probe.sh_coefficients_f16 {
                    buf.extend_from_slice(&c.to_le_bytes());
                }
            }
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.is_empty() {
            return Err(truncated("header"));
        }

        let mut o = 0;
        let version = data[o];
        o += 1;
        if version != DELTA_SH_VOLUMES_VERSION {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "delta sh volumes section version {version}, expected \
                     {DELTA_SH_VOLUMES_VERSION} — recompile the .prl with the current `prl-build`"
                ),
            )));
        }

        if data.len() < o + 4 {
            return Err(truncated("animated light count"));
        }
        let animated_light_count = read_u32(data, o) as usize;
        o += 4;

        let index_bytes = animated_light_count * 4;
        if data.len() < o + index_bytes {
            return Err(truncated("animation descriptor index table"));
        }
        let mut animation_descriptor_indices = Vec::with_capacity(animated_light_count);
        for _ in 0..animated_light_count {
            animation_descriptor_indices.push(read_u32(data, o));
            o += 4;
        }

        let mut grids = Vec::with_capacity(animated_light_count);
        for _ in 0..animated_light_count {
            if data.len() < o + Self::GRID_HEADER_SIZE {
                return Err(truncated("per-light grid header"));
            }
            let aabb_origin = [
                read_f32(data, o),
                read_f32(data, o + 4),
                read_f32(data, o + 8),
            ];
            o += 12;
            let cell_size = read_f32(data, o);
            o += 4;
            let grid_dimensions = [
                read_u32(data, o),
                read_u32(data, o + 4),
                read_u32(data, o + 8),
            ];
            o += 12;

            let total_probes = (grid_dimensions[0] as usize)
                .checked_mul(grid_dimensions[1] as usize)
                .and_then(|n| n.checked_mul(grid_dimensions[2] as usize))
                .ok_or_else(|| {
                    FormatError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "delta sh volume grid_dimensions {:?} overflow: total probe count exceeds usize",
                            grid_dimensions,
                        ),
                    ))
                })?;

            let probe_bytes = total_probes.checked_mul(PROBE_BYTES).ok_or_else(|| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "delta sh volume probe blob overflow: total_probes ({total_probes}) * \
                         PROBE_BYTES ({PROBE_BYTES}) exceeds usize",
                    ),
                ))
            })?;

            if data.len() < o + probe_bytes {
                return Err(truncated("per-light probe data"));
            }

            let mut probes = Vec::with_capacity(total_probes);
            for _ in 0..total_probes {
                let mut sh = [0u16; PROBE_F16_COUNT];
                for (i, slot) in sh.iter_mut().enumerate() {
                    *slot = read_u16(data, o + i * 2);
                }
                o += PROBE_BYTES;
                probes.push(DeltaShProbe {
                    sh_coefficients_f16: sh,
                });
            }

            grids.push(DeltaLightGrid {
                aabb_origin,
                cell_size,
                grid_dimensions,
                probes,
            });
        }

        Ok(Self {
            header: DeltaShVolumeHeader {
                animation_descriptor_indices,
            },
            grids,
        })
    }
}

fn truncated(what: &str) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        format!("delta sh volumes section truncated: {what}"),
    ))
}

fn read_f32(data: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_u16(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_probe(seed: u16) -> DeltaShProbe {
        let mut sh = [0u16; PROBE_F16_COUNT];
        for (i, c) in sh.iter_mut().enumerate() {
            *c = seed.wrapping_add(i as u16);
        }
        DeltaShProbe {
            sh_coefficients_f16: sh,
        }
    }

    fn sample_grid(seed: u16, dims: [u32; 3], origin: [f32; 3], cell: f32) -> DeltaLightGrid {
        let total = (dims[0] * dims[1] * dims[2]) as usize;
        DeltaLightGrid {
            aabb_origin: origin,
            cell_size: cell,
            grid_dimensions: dims,
            probes: (0..total)
                .map(|i| sample_probe(seed.wrapping_add(i as u16 * 31)))
                .collect(),
        }
    }

    #[test]
    fn round_trip_empty_section() {
        let section = DeltaShVolumesSection {
            header: DeltaShVolumeHeader {
                animation_descriptor_indices: Vec::new(),
            },
            grids: Vec::new(),
        };
        let bytes = section.to_bytes();
        // 1 byte version + 4 bytes count = 5 bytes.
        assert_eq!(bytes.len(), 5);
        let restored = DeltaShVolumesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_single_light() {
        let section = DeltaShVolumesSection {
            header: DeltaShVolumeHeader {
                animation_descriptor_indices: vec![7],
            },
            grids: vec![sample_grid(100, [2, 3, 4], [-1.0, -2.0, -3.0], 1.0)],
        };
        let bytes = section.to_bytes();
        let restored = DeltaShVolumesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_multiple_lights_varied_grids() {
        let section = DeltaShVolumesSection {
            header: DeltaShVolumeHeader {
                animation_descriptor_indices: vec![0, 3, 9],
            },
            grids: vec![
                sample_grid(1, [1, 1, 1], [0.0, 0.0, 0.0], 1.0),
                sample_grid(50, [3, 2, 1], [10.0, -5.0, 2.5], 0.5),
                sample_grid(200, [2, 2, 2], [-4.0, 4.0, 0.0], 2.0),
            ],
        };
        let bytes = section.to_bytes();
        let restored = DeltaShVolumesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn from_f32_round_trips_via_f16() {
        let mut coeffs = [0f32; PROBE_F16_COUNT];
        for (i, c) in coeffs.iter_mut().enumerate() {
            *c = i as f32 * 0.125;
        }
        let probe = DeltaShProbe::from_f32(&coeffs);
        // 0.0 → 0x0000; 1.0 → 0x3c00 confirms the f16 encoder is being used.
        assert_eq!(probe.sh_coefficients_f16[0], 0x0000);
        // index 8 → 1.0
        assert_eq!(probe.sh_coefficients_f16[8], 0x3c00);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = DeltaShVolumesSection::from_bytes(&[]).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_mismatched_section_version() {
        let section = DeltaShVolumesSection {
            header: DeltaShVolumeHeader {
                animation_descriptor_indices: Vec::new(),
            },
            grids: Vec::new(),
        };
        let mut bytes = section.to_bytes();
        bytes[0] = 99;
        let err = DeltaShVolumesSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("version"), "expected version error: {msg}");
    }

    #[test]
    fn rejects_truncated_probe_data() {
        let section = DeltaShVolumesSection {
            header: DeltaShVolumeHeader {
                animation_descriptor_indices: vec![0],
            },
            grids: vec![sample_grid(1, [1, 1, 2], [0.0; 3], 1.0)],
        };
        let bytes = section.to_bytes();
        let truncated_bytes = &bytes[..bytes.len() - 4];
        let err = DeltaShVolumesSection::from_bytes(truncated_bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }
}
