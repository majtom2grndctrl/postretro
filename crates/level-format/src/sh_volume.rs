// SH irradiance volume section (ID 20): regular-grid L2 spherical harmonic
// probes with static RGB base coefficients and animation descriptors.
//
// See: context/lib/build_pipeline.md

use crate::FormatError;

/// One probe's static base SH L2 record: 27 f32 RGB coefficients + validity +
/// two depth-visibility moments.
///
/// `sh_coefficients` is laid out as 9 bands × 3 color channels, stored
/// channel-interleaved per band: `[band0_r, band0_g, band0_b, band1_r, ...]`.
/// The encoder/decoder writes the same order — downstream consumers should
/// treat the array as opaque and index it with the same helper both sides use.
///
/// `mean_distance` / `mean_sq_distance` are the per-probe depth moments
/// `E[d]` / `E[d²]` over the probe's sampled rays, stored as IEEE 754 binary16
/// bits (round to nearest, encode via `lightmap::f32_to_f16_bits`) — matching
/// `DeltaShProbe`'s f16 SH storage. A future runtime Chebyshev interpolant
/// reconstructs `variance = E[d²] − E[d]²` from the pair to weight each probe
/// by visibility. Invalid probes carry zeroed moments, like their zeroed SH.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShProbe {
    /// 9 bands × 3 channels = 27 f32. Channel-interleaved per band.
    pub sh_coefficients: [f32; 27],
    /// 0 = invalid (inside solid), 1 = valid (usable by runtime).
    pub validity: u8,
    /// Mean ray distance `E[d]`, f16 bits.
    pub mean_distance: u16,
    /// Mean squared ray distance `E[d²]`, f16 bits.
    pub mean_sq_distance: u16,
}

impl Default for ShProbe {
    fn default() -> Self {
        Self {
            sh_coefficients: [0.0; 27],
            validity: 0,
            mean_distance: 0,
            mean_sq_distance: 0,
        }
    }
}

/// Section-internal version written as the first u32 of every ShVolume
/// section payload. Bumped any time the on-disk layout changes so the loader
/// can reject stale `.prl` files with a clear error rather than silently
/// misread them. History: version 1 (pre-animated-flag) — no `start_active`
/// in the descriptor table; version 2 — `start_active: u32` lives
/// alongside the brightness/color counts; version 3 — direction
/// channel samples serialized after color samples, with a `direction_count`
/// field in the descriptor header; version 4 — two f16 depth
/// moments (`mean_distance`, `mean_sq_distance`) appended inside the per-probe
/// record after `validity`, growing `PROBE_STRIDE` 112 → 116; version 5
/// (current) — trailing `map-light-index → animated-light section slot` table
/// (Task 2c of `sdf-static-occluder-shadows`), `u32::MAX` = no slot. The
/// runtime resolves each map light's animated-compose-descriptor slot at load
/// from this table.
pub const SH_VOLUME_VERSION: u32 = 5;

/// Sentinel for "this map light has no animated-light section slot" in
/// `ShVolumeSection.slot_for_map_light`. Non-animated lights and any light
/// the bake excluded from the animated-baked namespace use this value.
pub const ANIMATED_SLOT_NONE: u32 = u32::MAX;

/// Byte stride of a single serialized base probe record: 27 f32 + 1 u8
/// (validity) + 2 f16 (depth moments) + 3 bytes of padding to land on a
/// 4-byte boundary = 116 bytes.
///
/// The header's `probe_stride` field is written from this constant. It is
/// forward-compat scaffolding: per-probe base data (e.g. a future Chebyshev
/// visibility term) grows the stride without breaking the loader, which advances
/// by the file's `probe_stride` field rather than this compiled-in constant.
pub const PROBE_STRIDE: u32 = 116;

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
    /// Animated cone-direction samples for spot lights (Plan 2 Sub-plan 1).
    /// Samples must be unit-length — enforced by the scripting primitive
    /// `set_light_animation` and the FGD `direction_curve` parser. The GPU
    /// evaluator does not re-normalize per frame; a `debug_assert` in the
    /// GPU writer checks the invariant in debug builds.
    pub direction: Vec<[f32; 3]>,
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
            direction: Vec::new(),
            start_active: 1,
        }
    }
}

/// SH irradiance volume section (ID 20).
///
/// On-disk layout (all little-endian):
///
/// ```text
///   Header (48 bytes):
///     u32      version                (= SH_VOLUME_VERSION)
///     f32 × 3  grid_origin            (world-space min corner, meters)
///     f32 × 3  cell_size              (meters per cell along x/y/z)
///     u32 × 3  grid_dimensions        (probe count along x/y/z)
///     u32      probe_stride           (= PROBE_STRIDE = 116)
///     u32      animated_light_count   (0 = no animation layers)
///
///   Base probe records (probe_stride bytes each, z-major then y, then x):
///     f32 × 27 sh_coefficients        (9 bands × 3 channels, RGB)
///     u8       validity               (0 = invalid, 1 = valid)
///     f16      mean_distance          (E[d], depth moment)
///     f16      mean_sq_distance       (E[d²], depth moment)
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
///       u32 direction_count         (Plan 2 Sub-plan 1: spotlight aim curve)
///       f32 × brightness_count      (brightness samples)
///       f32 × 3 × color_count       (RGB color samples)
///       f32 × 3 × direction_count   (unit aim-vector samples)
///
///   Map-light → animated-slot table (v5+, omitted iff no trailer is written):
///     u32      map_light_count      (length of slot_for_map_light)
///     u32 × map_light_count         (ANIMATED_SLOT_NONE = u32::MAX for non-animated)
/// ```
///
/// A section with `animated_light_count == 0` is valid: the loader produces
/// an empty `animation_descriptors` vector and the runtime skips
/// animated-layer processing. Per-light monochrome SH layers were removed;
/// animated indirect is handled by the SH compose pass via per-light delta SH volumes.
#[derive(Debug, Clone, PartialEq)]
pub struct ShVolumeSection {
    pub grid_origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub grid_dimensions: [u32; 3],
    pub probe_stride: u32,
    /// One entry per grid cell in z-major/y/x order. `probes.len()` must equal
    /// `grid_dimensions[0] * grid_dimensions[1] * grid_dimensions[2]`.
    pub probes: Vec<ShProbe>,
    /// One descriptor per animated light.
    pub animation_descriptors: Vec<AnimationDescriptor>,
    /// One `u32` per **map light** (full `MapLight` array, not just animated):
    /// the slot into `animation_descriptors` the map light occupies, or
    /// [`ANIMATED_SLOT_NONE`] when the light has no animated-baked slot. The
    /// inverse of the envelope-slot assignment the compiler performs while
    /// building the animated-light namespace. Runtime resolves
    /// `LightComponent.animated_slot` once at load from this table. (Task 2c.)
    ///
    /// Empty `Vec` is the legacy / no-slot-table state; loaders treat that as
    /// "no map light has a slot" and the bridge writes via the
    /// `is_dynamic`-gated forward path (legacy behavior).
    pub slot_for_map_light: Vec<u32>,
}

impl ShVolumeSection {
    pub const HEADER_SIZE: usize = 48;

    /// Number of probes expected from the grid dimensions.
    pub fn total_probes(&self) -> usize {
        self.grid_dimensions[0] as usize
            * self.grid_dimensions[1] as usize
            * self.grid_dimensions[2] as usize
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let total_probes = self.total_probes();
        debug_assert_eq!(self.probes.len(), total_probes);

        let mut buf = Vec::with_capacity(Self::HEADER_SIZE + total_probes * PROBE_STRIDE as usize);

        // Header
        buf.extend_from_slice(&SH_VOLUME_VERSION.to_le_bytes());
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
            // Depth moments (f16 bits) follow validity at byte 109.
            buf.extend_from_slice(&probe.mean_distance.to_le_bytes());
            buf.extend_from_slice(&probe.mean_sq_distance.to_le_bytes());
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
            buf.extend_from_slice(&(desc.direction.len() as u32).to_le_bytes());
            for b in &desc.brightness {
                buf.extend_from_slice(&b.to_le_bytes());
            }
            for c in &desc.color {
                for ch in c {
                    buf.extend_from_slice(&ch.to_le_bytes());
                }
            }
            for d in &desc.direction {
                for ch in d {
                    buf.extend_from_slice(&ch.to_le_bytes());
                }
            }
        }

        // Map-light → animated-slot trailer (v5+).
        buf.extend_from_slice(&(self.slot_for_map_light.len() as u32).to_le_bytes());
        for slot in &self.slot_for_map_light {
            buf.extend_from_slice(&slot.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < Self::HEADER_SIZE {
            return Err(truncated("header"));
        }

        let mut o = 0;
        let version = read_u32(data, o);
        o += 4;
        if version != SH_VOLUME_VERSION {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "sh volume section version {version}, expected {SH_VOLUME_VERSION} — \
                     recompile the .prl with the current `prl-build`"
                ),
            )));
        }
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

        let total_probes = (grid_dimensions[0] as usize)
            .checked_mul(grid_dimensions[1] as usize)
            .and_then(|n| n.checked_mul(grid_dimensions[2] as usize))
            .ok_or_else(|| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "sh volume grid_dimensions {:?} overflow: total probe count exceeds usize",
                        grid_dimensions,
                    ),
                ))
            })?;

        let base_bytes = total_probes
            .checked_mul(probe_stride as usize)
            .ok_or_else(|| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "sh volume base_bytes overflow: total_probes ({total_probes}) * \
                         probe_stride ({probe_stride}) exceeds usize",
                    ),
                ))
            })?;
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
            // Depth moments live at fixed in-record offsets just past validity
            // (bytes 109–112). Read them relative to the record start, before
            // advancing by the file's stride.
            let mean_distance = read_u16(data, o + 27 * 4 + 1);
            let mean_sq_distance = read_u16(data, o + 27 * 4 + 3);
            probes.push(ShProbe {
                sh_coefficients,
                validity,
                mean_distance,
                mean_sq_distance,
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

            if data.len() < o + 16 {
                return Err(truncated("animation descriptor sample counts"));
            }
            let brightness_count = read_u32(data, o) as usize;
            let color_count = read_u32(data, o + 4) as usize;
            let start_active = read_u32(data, o + 8);
            let direction_count = read_u32(data, o + 12) as usize;
            o += 16;

            let brightness_bytes = brightness_count * 4;
            let color_bytes = color_count * 12;
            let direction_bytes = direction_count * 12;
            if data.len() < o + brightness_bytes + color_bytes + direction_bytes {
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

            let mut direction = Vec::with_capacity(direction_count);
            for i in 0..direction_count {
                direction.push([
                    read_f32(data, o + i * 12),
                    read_f32(data, o + i * 12 + 4),
                    read_f32(data, o + i * 12 + 8),
                ]);
            }
            o += direction_bytes;

            animation_descriptors.push(AnimationDescriptor {
                period,
                phase,
                base_color,
                brightness,
                color,
                direction,
                start_active,
            });
        }

        // Map-light → animated-slot trailer (v5+). The version gate above
        // already enforces v5 readers / v5 files only — but treat a truncated
        // trailer (zero remaining bytes) as a defensive "empty table" rather
        // than erroring, so an empty-volume `to_bytes` ↔ `from_bytes` round
        // trip still works when no animation table follows.
        let slot_for_map_light = if data.len() < o + 4 {
            Vec::new()
        } else {
            let map_light_count = read_u32(data, o) as usize;
            o += 4;
            if data.len() < o + map_light_count * 4 {
                return Err(truncated("map-light slot table"));
            }
            let mut slots = Vec::with_capacity(map_light_count);
            for i in 0..map_light_count {
                slots.push(read_u32(data, o + i * 4));
            }
            slots
        };

        Ok(Self {
            grid_origin,
            cell_size,
            grid_dimensions,
            probe_stride,
            probes,
            animation_descriptors,
            slot_for_map_light,
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

fn read_u16(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lightmap::f32_to_f16_bits;

    fn sample_probe(seed: f32) -> ShProbe {
        let mut coeffs = [0f32; 27];
        for (i, c) in coeffs.iter_mut().enumerate() {
            *c = seed + i as f32 * 0.01;
        }
        // Non-zero, f16-exact depth moments so every round-trip test exercises
        // the moment fields. 0.5/0.25 round-trip representably in f16.
        ShProbe {
            sh_coefficients: coeffs,
            validity: 1,
            mean_distance: f32_to_f16_bits(seed + 0.5),
            mean_sq_distance: f32_to_f16_bits(seed + 0.25),
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
            slot_for_map_light: Vec::new(),
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
            slot_for_map_light: Vec::new(),
        };
        let bytes = section.to_bytes();
        // Header + 4-byte map_light_count = 0.
        assert_eq!(bytes.len(), ShVolumeSection::HEADER_SIZE + 4);
        let restored = ShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_probes_only() {
        let section = empty_section([2, 3, 4]);
        let bytes = section.to_bytes();
        let expected_len =
            ShVolumeSection::HEADER_SIZE + (2 * 3 * 4) * PROBE_STRIDE as usize + 4;
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
            slot_for_map_light: vec![ANIMATED_SLOT_NONE, 0, ANIMATED_SLOT_NONE, 1],
            animation_descriptors: vec![
                AnimationDescriptor {
                    period: 1.5,
                    phase: 0.25,
                    base_color: [1.0, 0.9, 0.8],
                    brightness: vec![0.1, 0.5, 1.0, 0.5],
                    color: Vec::new(),
                    direction: Vec::new(),
                    start_active: 1,
                },
                AnimationDescriptor {
                    period: 2.0,
                    phase: 0.0,
                    base_color: [0.2, 0.4, 1.0],
                    brightness: Vec::new(),
                    color: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                    // Two unit-length direction samples: +x and +z.
                    direction: vec![[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
                    start_active: 0,
                },
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
        // Cut into the probe-record region (post-header, pre-trailer). Cutting
        // only the 4-byte slot-table count is the defensive "empty trailer"
        // path, not a truncation error — so trim past PROBE_STRIDE / 2.
        let truncated = &bytes[..ShVolumeSection::HEADER_SIZE + 4];
        let err = ShVolumeSection::from_bytes(truncated).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_invalid_probe_stride() {
        let section = empty_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        // Layout (little-endian): [version u32][origin f32×3][cell f32×3]
        // [dims u32×3][probe_stride u32][animated_light_count u32]
        // probe_stride lives at byte offset 4 + 12 + 12 + 12 = 40.
        bytes[40..44].copy_from_slice(&10u32.to_le_bytes());
        let err = ShVolumeSection::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_mismatched_section_version() {
        let section = empty_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        // Version is the first u32. Writing an older version must be rejected
        // so stale `.prl` files do not silently misread.
        bytes[0..4].copy_from_slice(&1u32.to_le_bytes());
        let err = ShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("version"),
            "expected version-mismatch error, got: {msg}",
        );
    }

    /// Old `.prl` rejection: a v4-layout fixture with the version field
    /// re-stamped to 3 must be rejected — the loader must not accept a version
    /// mismatch even when the bytes happen to be v4-compatible. Anchors the
    /// old-`.prl`-rejection AC.
    #[test]
    fn rejects_previous_section_version_three() {
        let section = empty_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        bytes[0..4].copy_from_slice(&3u32.to_le_bytes());
        let err = ShVolumeSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("version"),
            "expected version-mismatch error, got: {msg}",
        );
    }

    /// Depth moments survive the round trip: a probe with non-zero `E[d]` /
    /// `E[d²]` moments encodes and decodes byte-identically, and the moments
    /// compare equal after `to_bytes` → `from_bytes`.
    #[test]
    fn round_trip_preserves_depth_moments() {
        let mut section = empty_section([2, 1, 1]);
        section.probes[0].mean_distance = f32_to_f16_bits(3.5);
        section.probes[0].mean_sq_distance = f32_to_f16_bits(12.25);
        section.probes[1].mean_distance = f32_to_f16_bits(0.125);
        section.probes[1].mean_sq_distance = f32_to_f16_bits(0.015625);

        let bytes = section.to_bytes();
        let restored = ShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);

        // Re-encoding the decoded section reproduces the exact same bytes.
        assert_eq!(restored.to_bytes(), bytes);

        // The moments survive at their fixed in-record offsets.
        assert_eq!(restored.probes[0].mean_distance, f32_to_f16_bits(3.5));
        assert_eq!(restored.probes[0].mean_sq_distance, f32_to_f16_bits(12.25));
        assert_eq!(restored.probes[1].mean_distance, f32_to_f16_bits(0.125));
        assert_eq!(
            restored.probes[1].mean_sq_distance,
            f32_to_f16_bits(0.015625)
        );
    }

    /// Unaware-consumer contract: a reader compiled with only the minimum
    /// `PROBE_STRIDE` still reads SH coefficients and validity correctly from a
    /// record written with a LARGER stride. We simulate a future bigger stride
    /// by hand-writing records padded out to `PROBE_STRIDE + 8`, then decoding
    /// with the current loader (which advances by the file's `probe_stride`).
    #[test]
    fn reads_records_written_with_a_larger_stride() {
        let future_stride = PROBE_STRIDE + 8;
        let probe_a = sample_probe(1.0);
        let probe_b = sample_probe(2.0);
        let probes = [probe_a, probe_b];

        let mut buf = Vec::new();
        buf.extend_from_slice(&SH_VOLUME_VERSION.to_le_bytes());
        for v in [0.0f32, 0.0, 0.0] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for v in [1.0f32, 1.0, 1.0] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for v in [2u32, 1, 1] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(&future_stride.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // animated_light_count

        for probe in &probes {
            let record_start = buf.len();
            for coeff in &probe.sh_coefficients {
                buf.extend_from_slice(&coeff.to_le_bytes());
            }
            buf.push(probe.validity);
            buf.extend_from_slice(&probe.mean_distance.to_le_bytes());
            buf.extend_from_slice(&probe.mean_sq_distance.to_le_bytes());
            // Pad out to the larger future stride with trailing bytes the
            // current reader knows nothing about.
            while buf.len() - record_start < future_stride as usize {
                buf.push(0xEE);
            }
        }

        let restored = ShVolumeSection::from_bytes(&buf).unwrap();
        assert_eq!(restored.probe_stride, future_stride);
        assert_eq!(restored.probes.len(), 2);
        // SH coefficients, validity, and the moments all read correctly from
        // the fixed in-record offsets despite the unknown trailing bytes.
        assert_eq!(restored.probes[0], probe_a);
        assert_eq!(restored.probes[1], probe_b);
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
        // Header + 1 probe_stride + 4-byte empty slot-table trailer.
        assert_eq!(
            bytes.len(),
            ShVolumeSection::HEADER_SIZE + PROBE_STRIDE as usize + 4
        );
        // animated_light_count bytes at offset 44..48 should be zero
        // (header layout: version[0..4], origin[4..16], cell[16..28],
        // dims[28..40], probe_stride[40..44], animated_light_count[44..48]).
        assert_eq!(&bytes[44..48], &0u32.to_le_bytes());
    }

    /// Task 2c: round-trip the map-light → animated-slot trailer.
    /// Non-animated lights carry `ANIMATED_SLOT_NONE`; animated lights carry
    /// their compose-buffer slot. Bytes survive serialize ↔ deserialize and
    /// the slot count matches the map-light count.
    #[test]
    fn slot_for_map_light_round_trips() {
        let mut section = empty_section([1, 1, 1]);
        // 5 map lights, slots [NONE, 0, NONE, 1, NONE].
        section.slot_for_map_light = vec![
            ANIMATED_SLOT_NONE,
            0,
            ANIMATED_SLOT_NONE,
            1,
            ANIMATED_SLOT_NONE,
        ];
        section.animation_descriptors = vec![
            AnimationDescriptor::default(),
            AnimationDescriptor::default(),
        ];
        let bytes = section.to_bytes();
        let restored = ShVolumeSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored.slot_for_map_light, section.slot_for_map_light);
        assert_eq!(restored.to_bytes(), bytes);
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
