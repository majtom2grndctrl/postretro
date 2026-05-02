// FogVolumes PRL section (ID 30): per-region volumetric fog parameters and
// the worldspawn `fog_pixel_scale` downscale factor.
// See: context/lib/build_pipeline.md §PRL section IDs

use crate::FormatError;

/// Maximum number of fog volumes per level. Mirrors the engine's GPU storage
/// buffer cap so author-side overflow is caught at compile time.
// Mirrors `postretro::fx::fog_volume::MAX_FOG_VOLUMES` — keep in sync.
pub const MAX_FOG_VOLUMES: usize = 16;

/// One fog volume baked into the PRL. AABB extents are in engine space (Y-up,
/// meters); colour is linear 0–1 (no sRGB curve applied). The runtime spawns
/// one ECS entity per record at level load.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FogVolumeRecord {
    pub min: [f32; 3],
    pub density: f32,
    pub max: [f32; 3],
    pub falloff: f32,
    pub color: [f32; 3],
    pub scatter: f32,
    pub height_gradient: f32,
    pub radial_falloff: f32,
    /// Author-supplied script tags (FGD `_tags`, pre-split on whitespace).
    pub tags: Vec<String>,
}

/// FogVolumes PRL section.
///
/// On-disk layout (little-endian):
///   u32  pixel_scale
///   u32  volume_count
///   repeat volume_count:
///     f32  min_x, min_y, min_z
///     f32  density
///     f32  max_x, max_y, max_z
///     f32  falloff
///     f32  color_r, color_g, color_b
///     f32  scatter
///     f32  height_gradient
///     f32  radial_falloff
///     u32  tag_count
///     repeat tag_count:
///       u32  tag_byte_len; u8[] tag_utf8
///
/// Always emitted so the worldspawn `fog_pixel_scale` is honoured even when no
/// `env_fog_volume` brushes are present (8-byte overhead for the empty case).
#[derive(Debug, Clone, PartialEq)]
pub struct FogVolumesSection {
    pub pixel_scale: u32,
    pub volumes: Vec<FogVolumeRecord>,
}

impl Default for FogVolumesSection {
    fn default() -> Self {
        Self {
            pixel_scale: 4,
            volumes: Vec::new(),
        }
    }
}

impl FogVolumesSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.pixel_scale.to_le_bytes());
        buf.extend_from_slice(&(self.volumes.len() as u32).to_le_bytes());
        for v in &self.volumes {
            for c in v.min {
                buf.extend_from_slice(&c.to_le_bytes());
            }
            buf.extend_from_slice(&v.density.to_le_bytes());
            for c in v.max {
                buf.extend_from_slice(&c.to_le_bytes());
            }
            buf.extend_from_slice(&v.falloff.to_le_bytes());
            for c in v.color {
                buf.extend_from_slice(&c.to_le_bytes());
            }
            buf.extend_from_slice(&v.scatter.to_le_bytes());
            buf.extend_from_slice(&v.height_gradient.to_le_bytes());
            buf.extend_from_slice(&v.radial_falloff.to_le_bytes());
            buf.extend_from_slice(&(v.tags.len() as u32).to_le_bytes());
            for tag in &v.tags {
                let bytes = tag.as_bytes();
                buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                buf.extend_from_slice(bytes);
            }
        }
        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        let mut o = 0usize;
        let pixel_scale = read_u32(data, &mut o, "pixel_scale")?;
        let count = read_u32(data, &mut o, "volume count")? as usize;

        // Sanity-check: each fixed payload is 14 × f32 + u32 = 60 bytes.
        const MIN_RECORD_SIZE: usize = 60;
        let remaining = data.len().saturating_sub(o);
        if count > remaining / MIN_RECORD_SIZE {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "fog volumes: truncated — volume count {count} exceeds what remaining {remaining} bytes can hold"
                ),
            )));
        }

        let mut volumes = Vec::with_capacity(count);
        for i in 0..count {
            let min = read_vec3(data, &mut o, &format!("volume {i} min"))?;
            let density = read_f32(data, &mut o, &format!("volume {i} density"))?;
            let max = read_vec3(data, &mut o, &format!("volume {i} max"))?;
            let falloff = read_f32(data, &mut o, &format!("volume {i} falloff"))?;
            let color = read_vec3(data, &mut o, &format!("volume {i} color"))?;
            let scatter = read_f32(data, &mut o, &format!("volume {i} scatter"))?;
            let height_gradient = read_f32(data, &mut o, &format!("volume {i} height_gradient"))?;
            let radial_falloff = read_f32(data, &mut o, &format!("volume {i} radial_falloff"))?;

            let tag_count = read_u32(data, &mut o, &format!("volume {i} tag count"))? as usize;
            const MIN_TAG_SIZE: usize = 4;
            let remaining_for_tags = data.len().saturating_sub(o);
            if tag_count > remaining_for_tags / MIN_TAG_SIZE {
                return Err(FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!(
                        "fog volumes: volume {i} tag count {tag_count} exceeds what remaining {remaining_for_tags} bytes can hold"
                    ),
                )));
            }
            let mut tags = Vec::with_capacity(tag_count);
            for j in 0..tag_count {
                tags.push(read_string(data, &mut o, &format!("volume {i} tag {j}"))?);
            }

            volumes.push(FogVolumeRecord {
                min,
                density,
                max,
                falloff,
                color,
                scatter,
                height_gradient,
                radial_falloff,
                tags,
            });
        }

        Ok(Self {
            pixel_scale,
            volumes,
        })
    }
}

fn read_u32(data: &[u8], o: &mut usize, ctx: &str) -> crate::Result<u32> {
    if *o + 4 > data.len() {
        return Err(FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("fog volumes: truncated {ctx}"),
        )));
    }
    let v = u32::from_le_bytes([data[*o], data[*o + 1], data[*o + 2], data[*o + 3]]);
    *o += 4;
    Ok(v)
}

fn read_f32(data: &[u8], o: &mut usize, ctx: &str) -> crate::Result<f32> {
    if *o + 4 > data.len() {
        return Err(FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("fog volumes: truncated {ctx}"),
        )));
    }
    let v = f32::from_le_bytes([data[*o], data[*o + 1], data[*o + 2], data[*o + 3]]);
    *o += 4;
    if !v.is_finite() {
        return Err(FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("fog volumes: non-finite float in {ctx}"),
        )));
    }
    Ok(v)
}

fn read_vec3(data: &[u8], o: &mut usize, ctx: &str) -> crate::Result<[f32; 3]> {
    let x = read_f32(data, o, ctx)?;
    let y = read_f32(data, o, ctx)?;
    let z = read_f32(data, o, ctx)?;
    Ok([x, y, z])
}

fn read_string(data: &[u8], o: &mut usize, ctx: &str) -> crate::Result<String> {
    let byte_len = read_u32(data, o, &format!("{ctx} length"))? as usize;
    if *o + byte_len > data.len() {
        return Err(FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("fog volumes: truncated {ctx} payload"),
        )));
    }
    let s = std::str::from_utf8(&data[*o..*o + byte_len]).map_err(|_| {
        FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("fog volumes: invalid UTF-8 in {ctx}"),
        ))
    })?;
    *o += byte_len;
    Ok(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let section = FogVolumesSection {
            pixel_scale: 4,
            volumes: vec![],
        };
        let bytes = section.to_bytes();
        // 4 (pixel_scale) + 4 (volume_count) = 8 bytes overhead.
        assert_eq!(bytes.len(), 8);
        let restored = FogVolumesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_two_volumes_one_with_tags_one_without() {
        let section = FogVolumesSection {
            pixel_scale: 8,
            volumes: vec![
                FogVolumeRecord {
                    min: [-2.0, 0.0, -2.0],
                    density: 0.5,
                    max: [2.0, 3.0, 2.0],
                    falloff: 1.0,
                    color: [0.6, 0.7, 0.8],
                    scatter: 0.4,
                    height_gradient: 0.25,
                    radial_falloff: 0.0,
                    tags: vec!["smoke".to_string(), "ambient".to_string()],
                },
                FogVolumeRecord {
                    min: [10.0, 0.0, -5.0],
                    density: 1.5,
                    max: [12.0, 4.0, -1.0],
                    falloff: 0.5,
                    color: [1.0, 0.2, 0.1],
                    scatter: 0.9,
                    height_gradient: 0.0,
                    radial_falloff: 1.0,
                    tags: vec![],
                },
            ],
        };
        let bytes = section.to_bytes();
        let restored = FogVolumesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn pixel_scale_round_trips_independently() {
        let section = FogVolumesSection {
            pixel_scale: 1,
            volumes: vec![],
        };
        let restored = FogVolumesSection::from_bytes(&section.to_bytes()).unwrap();
        assert_eq!(restored.pixel_scale, 1);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = FogVolumesSection::from_bytes(&[0u8; 4]).unwrap_err();
        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn rejects_implausible_volume_count() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&4u32.to_le_bytes()); // pixel_scale
        buf.extend_from_slice(&u32::MAX.to_le_bytes()); // count = u32::MAX
        let err = FogVolumesSection::from_bytes(&buf).unwrap_err();
        assert!(err.to_string().contains("exceeds"));
    }

    #[test]
    fn rejects_non_finite_float_fields() {
        // Build a section with one volume whose density field is NaN.
        let valid = FogVolumesSection {
            pixel_scale: 4,
            volumes: vec![FogVolumeRecord {
                min: [0.0, 0.0, 0.0],
                density: 0.5,
                max: [1.0, 1.0, 1.0],
                falloff: 0.5,
                color: [1.0, 1.0, 1.0],
                scatter: 0.5,
                height_gradient: 0.0,
                radial_falloff: 0.0,
                tags: vec![],
            }],
        };
        let mut bytes = valid.to_bytes();
        // density is at offset: 4 (pixel_scale) + 4 (count) + 12 (min xyz) = 20
        let nan_bytes = f32::NAN.to_le_bytes();
        bytes[20..24].copy_from_slice(&nan_bytes);
        let err = FogVolumesSection::from_bytes(&bytes).unwrap_err();
        assert!(err.to_string().contains("non-finite"));

        // Also test infinity.
        let inf_bytes = f32::INFINITY.to_le_bytes();
        bytes[20..24].copy_from_slice(&inf_bytes);
        let err = FogVolumesSection::from_bytes(&bytes).unwrap_err();
        assert!(err.to_string().contains("non-finite"));
    }
}
