// Delta SH volumes section (ID 27): per-animated-light SH probe deltas in a
// sparse, affinity-cell-indexed (CSR) layout. Each animated light's baked delta
// contribution at peak brightness (brightness = 1.0, base color) is stored only
// for the affinity cells it actually touches. Consumed by the runtime compose
// pass that blends animated lights into the SH irradiance volume.
//
// See: context/plans/in-progress/lighting-animated-sh/

use crate::FormatError;
use crate::lightmap::f32_to_f16_bits;

/// Section-internal version, written as the first byte of the payload. Bumped
/// whenever the on-disk layout changes so the loader can reject stale `.prl`
/// files instead of silently misreading them.
pub const DELTA_SH_VOLUMES_VERSION: u8 = 2;

/// Affinity cell edge length in base SH probes. An affinity cell is a 4×4×4
/// cube of base probes. Locked to the compose pass `@workgroup_size(4,4,4)`.
/// The canonical compiler-side value lives at
/// `crates/level-compiler/src/affinity_grid.rs`; here it is the value stored on
/// disk and validated by the loader so a mismatched bake is rejected.
pub const AFFINITY_FACTOR: u8 = 4;

/// Number of base probes in one affinity cell sub-block: 4 × 4 × 4.
pub const PROBES_PER_CELL: usize = 64;

/// Number of logical f16 SH coefficients per probe: 9 SH bands × 3 color
/// channels. Unchanged across versions.
pub const PROBE_F16_COUNT: usize = 27;

/// On-disk / GPU-buffer stride of one probe in f16 halves: 27 logical coeffs
/// plus 1 zero pad half (28 total), so each probe pairs cleanly for
/// `unpack2x16float` in the compose shader.
pub const PROBE_F16_STRIDE: usize = 28;

/// Byte stride of one serialized probe: 28 × f16 = 56 bytes. (v1 was 54 = 27 ×
/// f16; the pad half raises the wire/buffer stride to 56.)
pub const PROBE_BYTES: usize = PROBE_F16_STRIDE * 2;

/// One probe's logical SH coefficients. Stores 9 SH bands × RGB as f16, packed
/// in the same channel-interleaved order as the base SH section's f32 layout:
/// `[band0_r, band0_g, band0_b, band1_r, ...]`. The serialization pad half is
/// applied at write time, not stored here.
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

/// Delta SH volumes section (ID 27), version 2.
///
/// Sparse CSR layout keyed by affinity cell. An affinity cell is a 4×4×4 cube
/// of base SH probes (`affinity_factor`). `affinity_dims = ceil(base_dims / 4)`
/// and `affinity_cell_count = affinity_dims.x * affinity_dims.y *
/// affinity_dims.z`. For each affinity cell, `affinity_offsets` gives the range
/// of entries in `affinity_lights` (the flat list of animated-light indices
/// influencing that cell). `delta_subblocks` holds one dense 64-probe sub-block
/// per CSR entry, index-parallel to `affinity_lights`. In-cell probe order is
/// x-fastest: `local = lx + ly*4 + lz*16`.
///
/// On-disk layout (all little-endian):
///
/// ```text
///   u8       version                    (= DELTA_SH_VOLUMES_VERSION = 2)
///   u8       affinity_factor            (= AFFINITY_FACTOR = 4)
///   u32 × 3  affinity_dims              (affinity cells along x/y/z)
///   u32      animated_light_count
///   u32 × animated_light_count          animation_descriptor_indices
///   u32 × (affinity_cell_count + 1)     affinity_offsets (CSR; last = list len)
///   u32 × affinity_offsets[-1]          affinity_lights (flat light indices)
///   f16 × affinity_offsets[-1] × 64 × 28
///                                       delta_subblocks (one 64-probe sub-block
///                                       per CSR entry; each probe = 27 coeffs +
///                                       1 zero pad half = 28 halves)
/// ```
///
/// Empty animated-light case: `affinity_offsets = [0; affinity_cell_count + 1]`,
/// `affinity_lights` empty, no probe payload. The "no animation descriptor"
/// sentinel is `u32::MAX`.
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaShVolumesSection {
    /// Affinity cell edge length in base probes. Stored so the loader can
    /// reject a bake that used a different factor than it expects.
    pub affinity_factor: u8,
    /// Affinity grid dimensions in cells along x/y/z. Product is the affinity
    /// cell count, which fixes `affinity_offsets.len() == count + 1`.
    pub affinity_dims: [u32; 3],
    /// One entry per animated light: index into the SH section's
    /// `animation_descriptors` array. `u32::MAX` means "no descriptor".
    pub animation_descriptor_indices: Vec<u32>,
    /// CSR offsets, one per affinity cell plus a trailing total. Cell `c`'s
    /// light range is `affinity_offsets[c]..affinity_offsets[c + 1]`.
    pub affinity_offsets: Vec<u32>,
    /// Flat list of animated-light indices, grouped by affinity cell. Each value
    /// must be `< animation_descriptor_indices.len()`.
    pub affinity_lights: Vec<u32>,
    /// Flat probe payload, length `affinity_lights.len() * 64 * 28`. One dense
    /// 64-probe sub-block per CSR entry, index-parallel to `affinity_lights`,
    /// stored at stride-28 (the producer writes each probe's pad half as zero).
    pub delta_subblocks: Vec<u16>,
}

impl DeltaShVolumesSection {
    /// Number of affinity cells implied by `affinity_dims`.
    pub fn affinity_cell_count(&self) -> usize {
        self.affinity_dims[0] as usize
            * self.affinity_dims[1] as usize
            * self.affinity_dims[2] as usize
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        debug_assert_eq!(self.affinity_offsets.len(), self.affinity_cell_count() + 1);
        debug_assert_eq!(
            self.delta_subblocks.len(),
            self.affinity_lights.len() * PROBES_PER_CELL * PROBE_F16_STRIDE
        );

        let mut buf = Vec::new();

        buf.push(DELTA_SH_VOLUMES_VERSION);
        buf.push(self.affinity_factor);
        for v in &self.affinity_dims {
            buf.extend_from_slice(&v.to_le_bytes());
        }

        let light_count = self.animation_descriptor_indices.len() as u32;
        buf.extend_from_slice(&light_count.to_le_bytes());
        for idx in &self.animation_descriptor_indices {
            buf.extend_from_slice(&idx.to_le_bytes());
        }

        for offset in &self.affinity_offsets {
            buf.extend_from_slice(&offset.to_le_bytes());
        }
        for light in &self.affinity_lights {
            buf.extend_from_slice(&light.to_le_bytes());
        }
        for half in &self.delta_subblocks {
            buf.extend_from_slice(&half.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        // Fixed header: version(1) + affinity_factor(1) + affinity_dims(12) +
        // animated_light_count(4) = 18 bytes.
        const FIXED_HEADER_SIZE: usize = 1 + 1 + 12 + 4;
        if data.len() < FIXED_HEADER_SIZE {
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

        let affinity_factor = data[o];
        o += 1;

        let affinity_dims = [
            read_u32(data, o),
            read_u32(data, o + 4),
            read_u32(data, o + 8),
        ];
        o += 12;

        let affinity_cell_count = (affinity_dims[0] as usize)
            .checked_mul(affinity_dims[1] as usize)
            .and_then(|n| n.checked_mul(affinity_dims[2] as usize))
            .ok_or_else(|| {
                invalid_data(format!(
                    "delta sh volumes affinity_dims {affinity_dims:?} overflow: \
                     cell count exceeds usize"
                ))
            })?;

        let animated_light_count = read_u32(data, o) as usize;
        o += 4;

        let index_bytes = animated_light_count.checked_mul(4).ok_or_else(|| {
            invalid_data(format!(
                "delta sh volumes animated_light_count {animated_light_count} \
                 overflow: index table size exceeds usize"
            ))
        })?;
        if data.len() < o + index_bytes {
            return Err(truncated("animation descriptor index table"));
        }
        let mut animation_descriptor_indices = Vec::with_capacity(animated_light_count);
        for _ in 0..animated_light_count {
            animation_descriptor_indices.push(read_u32(data, o));
            o += 4;
        }

        // CSR offsets: one per cell plus a trailing total.
        let offsets_len = affinity_cell_count + 1;
        let offsets_bytes = offsets_len.checked_mul(4).ok_or_else(|| {
            invalid_data(format!(
                "delta sh volumes affinity_offsets length {offsets_len} overflow: \
                 table size exceeds usize"
            ))
        })?;
        if data.len() < o + offsets_bytes {
            return Err(truncated("affinity offsets table"));
        }
        let mut affinity_offsets = Vec::with_capacity(offsets_len);
        for _ in 0..offsets_len {
            affinity_offsets.push(read_u32(data, o));
            o += 4;
        }

        // The trailing offset is the total CSR entry count = affinity_lights len.
        let list_len = *affinity_offsets
            .last()
            .expect("affinity_offsets has at least one entry (cell_count + 1 >= 1)")
            as usize;

        let lights_bytes = list_len.checked_mul(4).ok_or_else(|| {
            invalid_data(format!(
                "delta sh volumes affinity_lights length {list_len} overflow: \
                 list size exceeds usize"
            ))
        })?;
        if data.len() < o + lights_bytes {
            return Err(truncated("affinity lights list"));
        }
        let mut affinity_lights = Vec::with_capacity(list_len);
        for _ in 0..list_len {
            let light = read_u32(data, o);
            if (light as usize) >= animated_light_count {
                return Err(invalid_data(format!(
                    "delta sh volumes affinity_lights entry {light} out of range \
                     (animated_light_count = {animated_light_count})"
                )));
            }
            affinity_lights.push(light);
            o += 4;
        }

        let subblock_count = list_len
            .checked_mul(PROBES_PER_CELL)
            .and_then(|n| n.checked_mul(PROBE_F16_STRIDE))
            .ok_or_else(|| {
                invalid_data(format!(
                    "delta sh volumes delta_subblocks count overflow: \
                     {list_len} entries × {PROBES_PER_CELL} probes × \
                     {PROBE_F16_STRIDE} halves exceeds usize"
                ))
            })?;
        let subblock_bytes = subblock_count.checked_mul(2).ok_or_else(|| {
            invalid_data("delta sh volumes delta_subblocks byte size exceeds usize".to_string())
        })?;
        if data.len() < o + subblock_bytes {
            return Err(truncated("delta subblock probe data"));
        }
        let mut delta_subblocks = Vec::with_capacity(subblock_count);
        for _ in 0..subblock_count {
            delta_subblocks.push(read_u16(data, o));
            o += 2;
        }

        Ok(Self {
            affinity_factor,
            affinity_dims,
            animation_descriptor_indices,
            affinity_offsets,
            affinity_lights,
            delta_subblocks,
        })
    }
}

fn truncated(what: &str) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        format!("delta sh volumes section truncated: {what}"),
    ))
}

fn invalid_data(msg: String) -> FormatError {
    FormatError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
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

    /// Build a stride-28 sub-block (one affinity cell's 64 probes) with
    /// deterministic, pad-zeroed contents derived from `seed`.
    fn sample_subblock(seed: u16) -> Vec<u16> {
        let mut out = Vec::with_capacity(PROBES_PER_CELL * PROBE_F16_STRIDE);
        for probe in 0..PROBES_PER_CELL {
            for coeff in 0..PROBE_F16_STRIDE {
                let half = if coeff < PROBE_F16_COUNT {
                    seed.wrapping_add((probe * PROBE_F16_STRIDE + coeff) as u16)
                } else {
                    // Pad half written zero by the producer.
                    0
                };
                out.push(half);
            }
        }
        out
    }

    fn empty_section(affinity_dims: [u32; 3]) -> DeltaShVolumesSection {
        let cell_count = (affinity_dims[0] * affinity_dims[1] * affinity_dims[2]) as usize;
        DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims,
            animation_descriptor_indices: Vec::new(),
            affinity_offsets: vec![0; cell_count + 1],
            affinity_lights: Vec::new(),
            delta_subblocks: Vec::new(),
        }
    }

    #[test]
    fn round_trip_empty_section() {
        let section = empty_section([2, 1, 1]);
        let bytes = section.to_bytes();
        let restored = DeltaShVolumesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
        // All CSR offsets zero, no lights, no probe payload.
        assert!(restored.affinity_offsets.iter().all(|&o| o == 0));
        assert!(restored.affinity_lights.is_empty());
        assert!(restored.delta_subblocks.is_empty());
    }

    #[test]
    fn round_trip_single_light_single_cell() {
        // One affinity cell, one animated light touching it.
        let section = DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            animation_descriptor_indices: vec![7],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: sample_subblock(100),
        };
        let bytes = section.to_bytes();
        let restored = DeltaShVolumesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_multiple_lights_multiple_cells() {
        // Three affinity cells, three animated lights, varied per-cell ranges:
        //   cell 0 → lights [0, 2]
        //   cell 1 → (empty)
        //   cell 2 → light [1]
        let mut delta_subblocks = sample_subblock(1);
        delta_subblocks.extend(sample_subblock(50));
        delta_subblocks.extend(sample_subblock(200));

        let section = DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [3, 1, 1],
            animation_descriptor_indices: vec![0, 3, u32::MAX],
            affinity_offsets: vec![0, 2, 2, 3],
            affinity_lights: vec![0, 2, 1],
            delta_subblocks,
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
        // 0.0 → 0x0000; index 8 → 1.0 → 0x3c00 confirms the f16 encoder is used.
        assert_eq!(probe.sh_coefficients_f16[0], 0x0000);
        assert_eq!(probe.sh_coefficients_f16[8], 0x3c00);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = DeltaShVolumesSection::from_bytes(&[]).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_mismatched_section_version() {
        let section = empty_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        bytes[0] = 99;
        let err = DeltaShVolumesSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("version"), "expected version error: {msg}");
    }

    #[test]
    fn rejects_truncated_probe_data() {
        let section = DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            animation_descriptor_indices: vec![0],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: sample_subblock(5),
        };
        let bytes = section.to_bytes();
        let truncated_bytes = &bytes[..bytes.len() - 4];
        let err = DeltaShVolumesSection::from_bytes(truncated_bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_offsets_length_mismatch() {
        // affinity_dims implies 2 cells → offsets must be length 3. Serialize a
        // valid 2-cell section, then truncate the offsets table so the trailing
        // offset / lights region is short.
        let section = empty_section([2, 1, 1]);
        let mut bytes = section.to_bytes();
        // Drop the final 4 bytes (the trailing CSR total), leaving an offsets
        // table that cannot be fully read.
        bytes.truncate(bytes.len() - 4);
        let err = DeltaShVolumesSection::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_out_of_range_light_index() {
        // Single light declared, but the CSR list references light index 5.
        let section = DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            animation_descriptor_indices: vec![0],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![5],
            delta_subblocks: sample_subblock(9),
        };
        let bytes = section.to_bytes();
        let err = DeltaShVolumesSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("out of range"), "expected range error: {msg}");
    }
}
