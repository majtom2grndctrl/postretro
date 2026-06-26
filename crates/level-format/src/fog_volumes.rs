// FogVolumes PRL section (ID 30): per-region volumetric fog parameters,
// the worldspawn `fog_pixel_scale` downscale factor, and the worldspawn
// `initial_gravity` scalar (m/s², negative = downward).
// See: context/lib/build_pipeline.md §PRL section IDs

use crate::FormatError;

/// Maximum number of fog volumes per level. Authoritative definition; the engine
/// re-exports this via `crate::fx::fog_volume::MAX_FOG_VOLUMES`.
pub const MAX_FOG_VOLUMES: usize = 16;

/// Maximum number of bounding planes per fog volume. Brushes with more faces
/// are rejected by the level compiler; the runtime never sees `plane_count`
/// greater than this. Authoritative definition shared between the compiler
/// (which enforces the cap) and the engine (which sizes the `fog_planes`
/// storage buffer).
pub const MAX_PLANES_PER_VOLUME: usize = 16;

/// One fog volume baked into the PRL. AABB extents are in engine space (Y-up,
/// meters). The runtime spawns one ECS entity per record at level load.
///
/// `center` and `half_diag` are derived from `min`/`max` at compile time and
/// actively consumed by the raymarch shader. `inv_half_ext` (the reciprocal
/// per-axis half-extent) is also baked from `min`/`max`; the shader reads it
/// only on the ellipsoid path (`shape_mode == 1.0`) and ignores it on the
/// legacy radial path (`shape_mode == 0.0`). `shape_mode` is a discriminant
/// flag (0.0 = legacy radial sphere/capsule fade against `half_diag`, 1.0 =
/// ellipsoid using `inv_half_ext`).
///
/// `tint` multiplies the per-step scatter color after saturation is applied; `[1, 1, 1]` is a
/// no-op. `saturation` controls color vividness via a luma-mix: 0 = greyscale, 1 = natural
/// (no effect), >1 = boosted. `min_brightness` sets a scatter floor (0.0 = none); `light_range`
/// scales how far lights reach inside the volume (1.0 = same reach as open air). `anisotropy`
/// stores the compiler-translated HG `g` value from the FGD-only `scatter_bias` KVP, and
/// `ambient_scatter` scales the static SH ambient contribution. All six scatter fields
/// (`tint`, `saturation`, `min_brightness`, `light_range`, `anisotropy`, `ambient_scatter`)
/// default to their identity values so existing maps recompiled without these KVPs behave
/// identically.
#[derive(Debug, Clone, PartialEq)]
pub struct FogVolumeRecord {
    pub min: [f32; 3],
    pub density: f32,
    pub max: [f32; 3],
    /// World-unit fade band along brush face normals, used by primitive
    /// (plane-bounded) volumes. The renderer copies this into the GPU
    /// `FogVolume.edge_softness` slot — the wire field carries the same name
    /// so both sides of the layer boundary use one term. Semantic / zero-plane
    /// volumes (`fog_lamp`, `fog_tube`) ignore this and use `radial_falloff`.
    pub edge_softness: f32,
    pub glow: f32,
    pub radial_falloff: f32,
    /// AABB center: `(min + max) * 0.5`.
    pub center: [f32; 3],
    /// Reciprocal of the AABB half-extent: `1.0 / max((max - min) * 0.5, 1e-6)`.
    /// Clamped away from zero so degenerate (zero-thickness) volumes don't
    /// produce infinities in the shader. Consumed only when `shape_mode == 1.0`
    /// (ellipsoid path); ignored on the legacy radial path.
    pub inv_half_ext: [f32; 3],
    /// Length of the AABB half-extent vector — used as the radial-falloff
    /// normalization radius.
    pub half_diag: f32,
    /// Shape discriminant: `0.0` = legacy radial (sphere/capsule fade against
    /// `half_diag`), `1.0` = ellipsoid (uses `inv_half_ext`). The shader
    /// compares with `> 0.5` to avoid float precision issues.
    pub shape_mode: f32,
    /// Scatter tint multiplier. `[1, 1, 1]` = no tint (default). Applied after saturation.
    pub tint: [f32; 3],
    /// Scatter saturation. 0 = greyscale, 1 = natural (default), >1 = boosted.
    pub saturation: f32,
    /// Minimum scatter brightness floor. `0.0` = no floor (default).
    pub min_brightness: f32,
    /// Per-volume light range multiplier. `1.0` = same reach as open air (default). Higher values
    /// increase how far lights reach inside the volume; values below 1.0 reduce it. Clamped
    /// to a small positive minimum at load time.
    pub light_range: f32,
    /// Henyey-Greenstein anisotropy `g`, translated by the compiler from
    /// authored `scatter_bias`. `0.0` = isotropic ambient haze.
    pub anisotropy: f32,
    /// Static SH ambient scatter scale. `1.0` = full ambient contribution.
    pub ambient_scatter: f32,
    /// Number of bounding planes; mirrors `planes.len()` and is baked into the
    /// fixed payload so the wire format header is self-describing.
    pub plane_count: u32,
    /// Convex bounding planes. A point `p` is inside the volume iff
    /// `dot(p, n) <= d` for every `(nx, ny, nz, d)` plane. An empty list means
    /// the AABB is the only bound (semantic-entity / box case).
    pub planes: Vec<[f32; 4]>,
    /// Author-supplied script tags (FGD `_tags`, pre-split on whitespace).
    pub tags: Vec<String>,
}

impl Default for FogVolumeRecord {
    fn default() -> Self {
        Self {
            min: [0.0; 3],
            density: 0.0,
            max: [0.0; 3],
            edge_softness: 0.0,
            glow: 0.0,
            radial_falloff: 0.0,
            center: [0.0; 3],
            inv_half_ext: [0.0; 3],
            half_diag: 0.0,
            shape_mode: 0.0,
            tint: [1.0; 3],
            saturation: 1.0,
            min_brightness: 0.0,
            light_range: 1.0,
            anisotropy: 0.0,
            ambient_scatter: 1.0,
            plane_count: 0,
            planes: Vec::new(),
            tags: Vec::new(),
        }
    }
}

/// FogVolumes PRL section.
///
/// On-disk layout (little-endian):
///   u32  pixel_scale
///   f32  initial_gravity
///   u32  volume_count
///   repeat volume_count:
///     f32  min_x, min_y, min_z
///     f32  density
///     f32  max_x, max_y, max_z
///     f32  edge_softness
///     f32  glow
///     f32  radial_falloff
///     f32  center_x, center_y, center_z
///     f32  inv_half_ext_x, inv_half_ext_y, inv_half_ext_z
///     f32  half_diag
///     f32  shape_mode
///     f32  tint_r, tint_g, tint_b
///     f32  saturation
///     f32  min_brightness
///     f32  light_range
///     f32  anisotropy
///     f32  ambient_scatter
///     u32  plane_count
///     repeat plane_count:
///       f32  nx, ny, nz, d
///     u32  tag_count
///     repeat tag_count:
///       u32  tag_byte_len; u8[] tag_utf8
///
/// Always emitted so the worldspawn `fog_pixel_scale` and `initial_gravity`
/// are honoured even when no `fog_volume` brushes are present (12-byte
/// overhead for the empty case).
#[derive(Debug, Clone, PartialEq)]
pub struct FogVolumesSection {
    pub pixel_scale: u32,
    /// Worldspawn `initialGravity` (m/s²). Negative = downward (Earth = -9.81),
    /// positive = upward. Authored by mappers as a required worldspawn KVP and
    /// validated by `prl-build`; the engine consumes it as the starting value
    /// for the runtime gravity register.
    pub initial_gravity: f32,
    pub volumes: Vec<FogVolumeRecord>,
}

impl Default for FogVolumesSection {
    fn default() -> Self {
        Self {
            pixel_scale: 4,
            initial_gravity: -9.81,
            volumes: Vec::new(),
        }
    }
}

impl FogVolumesSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.pixel_scale.to_le_bytes());
        buf.extend_from_slice(&self.initial_gravity.to_le_bytes());
        buf.extend_from_slice(&(self.volumes.len() as u32).to_le_bytes());
        for v in &self.volumes {
            for c in v.min {
                buf.extend_from_slice(&c.to_le_bytes());
            }
            buf.extend_from_slice(&v.density.to_le_bytes());
            for c in v.max {
                buf.extend_from_slice(&c.to_le_bytes());
            }
            buf.extend_from_slice(&v.edge_softness.to_le_bytes());
            buf.extend_from_slice(&v.glow.to_le_bytes());
            buf.extend_from_slice(&v.radial_falloff.to_le_bytes());
            for c in v.center {
                buf.extend_from_slice(&c.to_le_bytes());
            }
            for c in v.inv_half_ext {
                buf.extend_from_slice(&c.to_le_bytes());
            }
            buf.extend_from_slice(&v.half_diag.to_le_bytes());
            buf.extend_from_slice(&v.shape_mode.to_le_bytes());
            for c in v.tint {
                buf.extend_from_slice(&c.to_le_bytes());
            }
            buf.extend_from_slice(&v.saturation.to_le_bytes());
            buf.extend_from_slice(&v.min_brightness.to_le_bytes());
            buf.extend_from_slice(&v.light_range.to_le_bytes());
            buf.extend_from_slice(&v.anisotropy.to_le_bytes());
            buf.extend_from_slice(&v.ambient_scatter.to_le_bytes());
            buf.extend_from_slice(&(v.planes.len() as u32).to_le_bytes());
            for plane in &v.planes {
                for c in plane {
                    buf.extend_from_slice(&c.to_le_bytes());
                }
            }
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
        let initial_gravity = read_f32(data, &mut o, "initial_gravity")?;
        let count = read_u32(data, &mut o, "volume count")? as usize;
        if count > MAX_FOG_VOLUMES {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "fog volumes: volume count {count} exceeds MAX_FOG_VOLUMES {MAX_FOG_VOLUMES}"
                ),
            )));
        }

        // Sanity-check: each fixed payload is 26 × f32 + 2 × u32 = 112 bytes
        // (includes plane_count and tag_count headers; planes and tags are
        // variable-length and validated against remaining bytes below).
        const MIN_RECORD_SIZE: usize = 112;
        let remaining = data.len().saturating_sub(o);
        if count > remaining / MIN_RECORD_SIZE {
            // FormatError has no Parse variant; Io is the closest proxy for
            // corrupt-data failures at the format boundary.
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
            let edge_softness = read_f32(data, &mut o, &format!("volume {i} edge_softness"))?;
            let glow = read_f32(data, &mut o, &format!("volume {i} glow"))?;
            let radial_falloff = read_f32(data, &mut o, &format!("volume {i} radial_falloff"))?;
            let center = read_vec3(data, &mut o, &format!("volume {i} center"))?;
            let inv_half_ext = read_vec3(data, &mut o, &format!("volume {i} inv_half_ext"))?;
            let half_diag = read_f32(data, &mut o, &format!("volume {i} half_diag"))?;
            let shape_mode = read_f32(data, &mut o, &format!("volume {i} shape_mode"))?;
            let tint = read_vec3(data, &mut o, &format!("volume {i} tint"))?;
            let saturation = read_f32(data, &mut o, &format!("volume {i} saturation"))?;
            let min_brightness = read_f32(data, &mut o, &format!("volume {i} min_brightness"))?;
            let light_range = read_f32(data, &mut o, &format!("volume {i} light_range"))?;
            let anisotropy = read_f32(data, &mut o, &format!("volume {i} anisotropy"))?;
            let ambient_scatter = read_f32(data, &mut o, &format!("volume {i} ambient_scatter"))?;

            let plane_count = read_u32(data, &mut o, &format!("volume {i} plane count"))? as usize;
            const PLANE_SIZE: usize = 16;
            let remaining_for_planes = data.len().saturating_sub(o);
            if plane_count > remaining_for_planes / PLANE_SIZE {
                return Err(FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!(
                        "fog volumes: volume {i} plane count {plane_count} exceeds what remaining {remaining_for_planes} bytes can hold"
                    ),
                )));
            }
            let mut planes = Vec::with_capacity(plane_count);
            for j in 0..plane_count {
                let nx = read_f32(data, &mut o, &format!("volume {i} plane {j} nx"))?;
                let ny = read_f32(data, &mut o, &format!("volume {i} plane {j} ny"))?;
                let nz = read_f32(data, &mut o, &format!("volume {i} plane {j} nz"))?;
                let d = read_f32(data, &mut o, &format!("volume {i} plane {j} d"))?;
                planes.push([nx, ny, nz, d]);
            }

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
                edge_softness,
                glow,
                radial_falloff,
                center,
                inv_half_ext,
                half_diag,
                shape_mode,
                tint,
                saturation,
                min_brightness,
                light_range,
                anisotropy,
                ambient_scatter,
                plane_count: plane_count as u32,
                planes,
                tags,
            });
        }

        Ok(Self {
            pixel_scale,
            initial_gravity,
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
        // FormatError has no Parse variant; Io is the closest proxy for
        // corrupt-data failures at the format boundary.
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
            initial_gravity: -9.81,
            volumes: vec![],
        };
        let bytes = section.to_bytes();
        // 4 (pixel_scale) + 4 (initial_gravity) + 4 (volume_count) = 12 bytes overhead.
        assert_eq!(bytes.len(), 12);
        let restored = FogVolumesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn fog_volume_record_default_preserves_identity_scatter_fields() {
        let record = FogVolumeRecord::default();
        assert_eq!(record.tint, [1.0; 3]);
        assert_eq!(record.saturation, 1.0);
        assert_eq!(record.light_range, 1.0);
        assert_eq!(record.anisotropy, 0.0);
        assert_eq!(record.ambient_scatter, 1.0);
    }

    #[test]
    fn round_trip_two_volumes_one_with_tags_one_without() {
        let section = FogVolumesSection {
            pixel_scale: 8,
            initial_gravity: -9.81,
            volumes: vec![
                FogVolumeRecord {
                    min: [-2.0, 0.0, -2.0],
                    density: 0.5,
                    max: [2.0, 3.0, 2.0],
                    edge_softness: 1.0,
                    glow: 0.4,
                    radial_falloff: 0.0,
                    center: [0.0, 1.5, 0.0],
                    inv_half_ext: [0.5, 1.0 / 1.5, 0.5],
                    half_diag: 2.5,
                    shape_mode: 0.0,
                    tint: [1.0, 1.0, 1.0],
                    saturation: 1.0,
                    min_brightness: 0.0,
                    light_range: 1.0,
                    anisotropy: 0.0,
                    ambient_scatter: 1.0,
                    plane_count: 0,
                    planes: vec![],
                    tags: vec!["smoke".to_string(), "ambient".to_string()],
                },
                FogVolumeRecord {
                    min: [10.0, 0.0, -5.0],
                    density: 1.5,
                    max: [12.0, 4.0, -1.0],
                    edge_softness: 0.5,
                    glow: 0.9,
                    radial_falloff: 1.0,
                    center: [11.0, 2.0, -3.0],
                    inv_half_ext: [1.0, 0.5, 0.5],
                    half_diag: 3.0,
                    shape_mode: 0.0,
                    tint: [1.0, 0.5, 0.2],
                    saturation: 1.5,
                    min_brightness: 0.0,
                    light_range: 1.0,
                    anisotropy: 0.7,
                    ambient_scatter: 0.25,
                    plane_count: 0,
                    planes: vec![],
                    tags: vec![],
                },
            ],
        };
        let bytes = section.to_bytes();
        let restored = FogVolumesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_preserves_directional_fog_fields() {
        let mut volume = make_volume(vec![], vec![]);
        volume.anisotropy = 0.9;
        volume.ambient_scatter = 0.25;
        let section = FogVolumesSection {
            pixel_scale: 4,
            initial_gravity: -9.81,
            volumes: vec![volume],
        };

        let bytes = section.to_bytes();
        let record_offset = 12;
        let anisotropy_offset = record_offset + 24 * 4;
        let ambient_scatter_offset = record_offset + 25 * 4;
        assert_eq!(
            f32::from_le_bytes(
                bytes[anisotropy_offset..anisotropy_offset + 4]
                    .try_into()
                    .unwrap()
            )
            .to_bits(),
            0.9_f32.to_bits()
        );
        assert_eq!(
            f32::from_le_bytes(
                bytes[ambient_scatter_offset..ambient_scatter_offset + 4]
                    .try_into()
                    .unwrap()
            )
            .to_bits(),
            0.25_f32.to_bits()
        );

        let restored = FogVolumesSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored.volumes[0].anisotropy.to_bits(), 0.9_f32.to_bits());
        assert_eq!(
            restored.volumes[0].ambient_scatter.to_bits(),
            0.25_f32.to_bits()
        );
        assert_eq!(section, restored);
    }

    #[test]
    fn pixel_scale_round_trips_independently() {
        let section = FogVolumesSection {
            pixel_scale: 1,
            initial_gravity: -9.81,
            volumes: vec![],
        };
        let restored = FogVolumesSection::from_bytes(&section.to_bytes()).unwrap();
        assert_eq!(restored.pixel_scale, 1);
    }

    #[test]
    fn initial_gravity_round_trips_independently() {
        let section = FogVolumesSection {
            pixel_scale: 4,
            initial_gravity: 12.5,
            volumes: vec![],
        };
        let restored = FogVolumesSection::from_bytes(&section.to_bytes()).unwrap();
        assert!(
            (restored.initial_gravity - 12.5).abs() < 1e-6,
            "initial_gravity round-trip: got {}",
            restored.initial_gravity
        );
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
        buf.extend_from_slice(&(-9.81f32).to_le_bytes()); // initial_gravity
        buf.extend_from_slice(&u32::MAX.to_le_bytes()); // count = u32::MAX
        let err = FogVolumesSection::from_bytes(&buf).unwrap_err();
        assert!(err.to_string().contains("exceeds"));
    }

    #[test]
    fn rejects_volume_count_over_renderer_slot_cap() {
        let section = FogVolumesSection {
            pixel_scale: 4,
            initial_gravity: -9.81,
            volumes: vec![FogVolumeRecord::default(); MAX_FOG_VOLUMES + 1],
        };
        let err = FogVolumesSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert!(err.to_string().contains("MAX_FOG_VOLUMES"));
    }

    fn make_volume(planes: Vec<[f32; 4]>, tags: Vec<String>) -> FogVolumeRecord {
        let plane_count = planes.len() as u32;
        FogVolumeRecord {
            min: [-1.0, -1.0, -1.0],
            density: 0.5,
            max: [1.0, 1.0, 1.0],
            edge_softness: 0.5,
            glow: 0.5,
            radial_falloff: 0.0,
            center: [0.0, 0.0, 0.0],
            inv_half_ext: [1.0, 1.0, 1.0],
            half_diag: 1.732_050_8,
            shape_mode: 0.0,
            tint: [1.0, 1.0, 1.0],
            saturation: 1.0,
            min_brightness: 0.0,
            light_range: 1.0,
            anisotropy: 0.0,
            ambient_scatter: 1.0,
            plane_count,
            planes,
            tags,
        }
    }

    #[test]
    fn round_trip_six_plane_box_preserves_every_component() {
        let planes: Vec<[f32; 4]> = vec![
            [1.0, 0.0, 0.0, 1.5],
            [-1.0, 0.0, 0.0, 1.25],
            [0.0, 1.0, 0.0, 2.5],
            [0.0, -1.0, 0.0, 2.25],
            [0.0, 0.0, 1.0, 3.5],
            [0.0, 0.0, -1.0, 3.25],
        ];
        let section = FogVolumesSection {
            pixel_scale: 4,
            initial_gravity: -9.81,
            volumes: vec![make_volume(planes.clone(), vec![])],
        };
        let restored = FogVolumesSection::from_bytes(&section.to_bytes()).unwrap();
        let restored_planes = &restored.volumes[0].planes;
        assert_eq!(restored_planes.len(), planes.len());
        for (got, want) in restored_planes.iter().zip(planes.iter()) {
            for (g, w) in got.iter().zip(want.iter()) {
                assert_eq!(g.to_bits(), w.to_bits());
            }
        }
        assert_eq!(restored.volumes[0].plane_count, 6);
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_five_plane_wedge_preserves_every_component() {
        let inv_sqrt2 = 1.0_f32 / 2.0_f32.sqrt();
        let planes: Vec<[f32; 4]> = vec![
            [1.0, 0.0, 0.0, 1.0],
            [-1.0, 0.0, 0.0, 1.0],
            [0.0, -1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 1.0],
            [0.0, inv_sqrt2, -inv_sqrt2, 0.25],
        ];
        let section = FogVolumesSection {
            pixel_scale: 4,
            initial_gravity: -9.81,
            volumes: vec![make_volume(planes.clone(), vec![])],
        };
        let restored = FogVolumesSection::from_bytes(&section.to_bytes()).unwrap();
        let restored_planes = &restored.volumes[0].planes;
        assert_eq!(restored_planes.len(), planes.len());
        for (got, want) in restored_planes.iter().zip(planes.iter()) {
            for (g, w) in got.iter().zip(want.iter()) {
                assert_eq!(g.to_bits(), w.to_bits());
            }
        }
        assert_eq!(restored.volumes[0].plane_count, 5);
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_zero_plane_volume_round_trips() {
        let section = FogVolumesSection {
            pixel_scale: 4,
            initial_gravity: -9.81,
            volumes: vec![make_volume(vec![], vec![])],
        };
        let bytes = section.to_bytes();
        let restored = FogVolumesSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored.volumes[0].plane_count, 0);
        assert!(restored.volumes[0].planes.is_empty());
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_planes_and_tags_coexist() {
        let planes: Vec<[f32; 4]> = vec![
            [1.0, 0.0, 0.0, 0.5],
            [0.0, 1.0, 0.0, 0.75],
            [0.0, 0.0, 1.0, 1.25],
        ];
        let tags = vec!["smoke".to_string(), "indoor".to_string()];
        let section = FogVolumesSection {
            pixel_scale: 4,
            initial_gravity: -9.81,
            volumes: vec![make_volume(planes.clone(), tags.clone())],
        };
        let restored = FogVolumesSection::from_bytes(&section.to_bytes()).unwrap();
        assert_eq!(restored.volumes[0].planes, planes);
        assert_eq!(restored.volumes[0].tags, tags);
        assert_eq!(section, restored);
    }

    #[test]
    fn rejects_non_finite_float_fields() {
        // Build a section with one volume whose density field is NaN.
        let valid = FogVolumesSection {
            pixel_scale: 4,
            initial_gravity: -9.81,
            volumes: vec![FogVolumeRecord {
                min: [0.0, 0.0, 0.0],
                density: 0.5,
                max: [1.0, 1.0, 1.0],
                edge_softness: 0.5,
                glow: 0.5,
                radial_falloff: 0.0,
                center: [0.5, 0.5, 0.5],
                inv_half_ext: [2.0, 2.0, 2.0],
                half_diag: 0.866_025_4,
                shape_mode: 0.0,
                tint: [1.0, 1.0, 1.0],
                saturation: 1.0,
                min_brightness: 0.0,
                light_range: 1.0,
                anisotropy: 0.0,
                ambient_scatter: 1.0,
                plane_count: 0,
                planes: vec![],
                tags: vec![],
            }],
        };
        let mut bytes = valid.to_bytes();
        // density is at offset: 4 (pixel_scale) + 4 (initial_gravity) + 4 (count) + 12 (min xyz) = 24
        let nan_bytes = f32::NAN.to_le_bytes();
        bytes[24..28].copy_from_slice(&nan_bytes);
        let err = FogVolumesSection::from_bytes(&bytes).unwrap_err();
        assert!(err.to_string().contains("non-finite"));

        // Also test infinity.
        let inf_bytes = f32::INFINITY.to_le_bytes();
        bytes[24..28].copy_from_slice(&inf_bytes);
        let err = FogVolumesSection::from_bytes(&bytes).unwrap_err();
        assert!(err.to_string().contains("non-finite"));
    }
}
