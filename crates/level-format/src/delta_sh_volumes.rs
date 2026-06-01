// Delta SH volumes section (ID 27): per-animated-light octahedral irradiance
// deltas in a sparse, affinity-cell-indexed (CSR) layout. Each animated light's
// baked delta contribution at peak brightness (brightness = 1.0, base color) is
// stored only for the affinity cells it actually touches. Consumed by the
// runtime compose pass that blends animated lights into the irradiance atlas.
//
// See: context/plans/done/lighting-animated-sh/

use crate::FormatError;
use crate::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
    RUNTIME_SUPPORTED_TILE_DIMENSION,
};

/// Section-internal version, written as the first byte of the payload. Bumped
/// whenever the on-disk layout changes so the loader can reject stale `.prl`
/// files instead of silently misreading them.
pub const DELTA_SH_VOLUMES_VERSION: u8 = 3;

/// Affinity cell edge length in base SH probes. An affinity cell is a 4×4×4
/// cube of base probes. Locked to the compose pass `@workgroup_size(4,4,4)`.
/// The canonical compiler-side value lives at
/// `crates/level-compiler/src/affinity_grid.rs`; here it is the value stored on
/// disk and validated by the loader so a mismatched bake is rejected.
pub const AFFINITY_FACTOR: u8 = 4;

/// Number of base probes in one affinity cell sub-block: 4 × 4 × 4.
pub const PROBES_PER_CELL: usize = 64;

/// Number of f16 channels per octahedral irradiance texel: RGBA16F.
pub const DELTA_TILE_TEXEL_F16_COUNT: usize = 4;

/// Default on-disk / GPU-buffer stride of one probe tile in f16 halves:
/// 6 × 6 RGBA16F texels.
pub const DEFAULT_DELTA_PROBE_F16_STRIDE: usize = (DEFAULT_IRRADIANCE_TILE_DIMENSION as usize)
    * (DEFAULT_IRRADIANCE_TILE_DIMENSION as usize)
    * DELTA_TILE_TEXEL_F16_COUNT;

/// Default byte stride of one serialized probe tile.
pub const DEFAULT_DELTA_PROBE_BYTES: usize = DEFAULT_DELTA_PROBE_F16_STRIDE * 2;

/// Delta SH volumes section (ID 27), version 3.
///
/// Sparse CSR layout keyed by affinity cell. An affinity cell is a 4×4×4 cube
/// of base SH probes (`affinity_factor`). `affinity_dims = ceil(base_dims / 4)`
/// and `affinity_cell_count = affinity_dims.x * affinity_dims.y *
/// affinity_dims.z`. For each affinity cell, `affinity_offsets` gives the range
/// of entries in `affinity_lights` (the flat list of animated-light indices
/// influencing that cell). `delta_subblocks` holds one dense 64-probe sub-block
/// per CSR entry, index-parallel to `affinity_lights`. In-cell probe order is
/// x-fastest: `local = lx + ly*4 + lz*16`. Each probe payload is one row-major
/// octahedral tile using the same `tile_dimension`, `tile_border`, interior
/// mapping, and wrap-border convention as `OctahedralShVolume`.
///
/// On-disk layout (all little-endian):
///
/// ```text
///   u8       version                    (= DELTA_SH_VOLUMES_VERSION = 3)
///   u8       affinity_factor            (= AFFINITY_FACTOR = 4)
///   u32 × 3  affinity_dims              (affinity cells along x/y/z)
///   u32      animated_light_count
///   u32      tile_dimension             (default 6, border included)
///   u32      tile_border                (default 1)
///   u32 × animated_light_count          animation_descriptor_indices
///   u32 × (affinity_cell_count + 1)     affinity_offsets (CSR; last = list len)
///   u32 × affinity_offsets[-1]          affinity_lights (flat light indices)
///   f16 × affinity_offsets[-1] × 64 × tile_dimension × tile_dimension × 4
///                                       delta_subblocks (one 64-probe sub-block
///                                       per CSR entry; each probe = one RGBA16F
///                                       octahedral irradiance tile)
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
    /// Full octahedral tile dimension, including border texels.
    pub tile_dimension: u32,
    /// Octahedral wrap border width. Version 3 bakes with the committed value 1.
    pub tile_border: u32,
    /// One entry per animated light: index into the SH section's
    /// `animation_descriptors` array. `u32::MAX` means "no descriptor".
    pub animation_descriptor_indices: Vec<u32>,
    /// CSR offsets, one per affinity cell plus a trailing total. Cell `c`'s
    /// light range is `affinity_offsets[c]..affinity_offsets[c + 1]`.
    pub affinity_offsets: Vec<u32>,
    /// Flat list of animated-light indices, grouped by affinity cell. Each value
    /// must be `< animation_descriptor_indices.len()`.
    pub affinity_lights: Vec<u32>,
    /// Flat probe payload, length `affinity_lights.len() * 64 *
    /// tile_dimension * tile_dimension * 4`. One dense 64-probe sub-block per
    /// CSR entry, index-parallel to `affinity_lights`, stored as row-major
    /// RGBA16F octahedral tiles.
    pub delta_subblocks: Vec<u16>,
}

impl DeltaShVolumesSection {
    /// Number of affinity cells implied by `affinity_dims`.
    pub fn affinity_cell_count(&self) -> usize {
        self.affinity_dims[0] as usize
            * self.affinity_dims[1] as usize
            * self.affinity_dims[2] as usize
    }

    pub fn delta_probe_f16_stride(&self) -> usize {
        delta_probe_f16_stride(self.tile_dimension)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        debug_assert_eq!(self.affinity_offsets.len(), self.affinity_cell_count() + 1);
        debug_assert_eq!(
            self.delta_subblocks.len(),
            self.affinity_lights.len() * PROBES_PER_CELL * self.delta_probe_f16_stride()
        );

        let mut buf = Vec::new();

        buf.push(DELTA_SH_VOLUMES_VERSION);
        buf.push(self.affinity_factor);
        for v in &self.affinity_dims {
            buf.extend_from_slice(&v.to_le_bytes());
        }

        let light_count = self.animation_descriptor_indices.len() as u32;
        buf.extend_from_slice(&light_count.to_le_bytes());
        buf.extend_from_slice(&self.tile_dimension.to_le_bytes());
        buf.extend_from_slice(&self.tile_border.to_le_bytes());
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
        // animated_light_count(4) + tile_dimension(4) + tile_border(4) = 26 bytes.
        const FIXED_HEADER_SIZE: usize = 1 + 1 + 12 + 4 + 4 + 4;
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

        let tile_dimension = read_u32(data, o);
        o += 4;
        let tile_border = read_u32(data, o);
        o += 4;
        validate_tile_geometry(tile_dimension, tile_border)?;
        let probe_f16_stride = delta_probe_f16_stride_checked(tile_dimension)?;

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

        // Validate that offsets are non-decreasing. A non-monotonic offset would
        // allow the compose shader's `affinity_offsets[cell]..affinity_offsets[cell+1]`
        // range to overflow or index out of bounds on corrupt/adversarial input.
        for k in 0..affinity_offsets.len() - 1 {
            if affinity_offsets[k] > affinity_offsets[k + 1] {
                return Err(invalid_data(format!(
                    "delta sh volumes affinity_offsets[{k}] ({}) > affinity_offsets[{}] ({}): \
                     offsets must be non-decreasing",
                    affinity_offsets[k],
                    k + 1,
                    affinity_offsets[k + 1],
                )));
            }
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
            .and_then(|n| n.checked_mul(probe_f16_stride))
            .ok_or_else(|| {
                invalid_data(format!(
                    "delta sh volumes delta_subblocks count overflow: \
                     {list_len} entries × {PROBES_PER_CELL} probes × \
                     {probe_f16_stride} halves exceeds usize"
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
            tile_dimension,
            tile_border,
            animation_descriptor_indices,
            affinity_offsets,
            affinity_lights,
            delta_subblocks,
        })
    }
}

pub fn delta_probe_f16_stride(tile_dimension: u32) -> usize {
    tile_dimension as usize * tile_dimension as usize * DELTA_TILE_TEXEL_F16_COUNT
}

fn delta_probe_f16_stride_checked(tile_dimension: u32) -> crate::Result<usize> {
    (tile_dimension as usize)
        .checked_mul(tile_dimension as usize)
        .and_then(|n| n.checked_mul(DELTA_TILE_TEXEL_F16_COUNT))
        .ok_or_else(|| {
            invalid_data(format!(
                "delta sh volumes tile_dimension {tile_dimension} overflows probe tile stride"
            ))
        })
}

fn validate_tile_geometry(tile_dimension: u32, tile_border: u32) -> crate::Result<()> {
    if tile_border != DEFAULT_IRRADIANCE_TILE_BORDER {
        return Err(invalid_data(format!(
            "delta sh volumes tile_border {tile_border}, expected {DEFAULT_IRRADIANCE_TILE_BORDER}"
        )));
    }
    // The header stores N so a re-bake can change tile resolution without a
    // format break; reject only what *this runtime* cannot sample yet.
    if tile_dimension != RUNTIME_SUPPORTED_TILE_DIMENSION {
        return Err(invalid_data(format!(
            "delta sh volumes tile_dimension {tile_dimension} is not supported by this runtime, which is pinned to N={RUNTIME_SUPPORTED_TILE_DIMENSION}"
        )));
    }
    if tile_dimension <= tile_border.saturating_mul(2) {
        return Err(invalid_data(format!(
            "delta sh volumes tile_dimension {tile_dimension} leaves no interior texels with border {tile_border}"
        )));
    }
    Ok(())
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

    /// Build one affinity cell's 64-probe octahedral tile payload with
    /// deterministic contents derived from `seed`.
    fn sample_subblock(seed: u16) -> Vec<u16> {
        let stride = DEFAULT_DELTA_PROBE_F16_STRIDE;
        let mut out = Vec::with_capacity(PROBES_PER_CELL * stride);
        for probe in 0..PROBES_PER_CELL {
            for half_index in 0..stride {
                let half = seed.wrapping_add((probe * stride + half_index) as u16);
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
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
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
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
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
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
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
    fn default_probe_stride_matches_committed_tile_geometry() {
        assert_eq!(
            DEFAULT_DELTA_PROBE_F16_STRIDE,
            delta_probe_f16_stride(DEFAULT_IRRADIANCE_TILE_DIMENSION),
        );
        assert_eq!(
            DEFAULT_DELTA_PROBE_BYTES,
            DEFAULT_DELTA_PROBE_F16_STRIDE * 2
        );
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
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
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
    fn rejects_non_monotonic_offsets() {
        // Three affinity cells, one animated light. Serialize a valid section,
        // then corrupt an intermediate offset so it is larger than the next,
        // making the offsets non-monotonic. The loader must reject this without
        // panicking.
        let section = DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [3, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![0],
            // Valid monotonic offsets: cell 0 → [0,1), cells 1&2 → empty.
            affinity_offsets: vec![0, 1, 1, 1],
            affinity_lights: vec![0],
            delta_subblocks: sample_subblock(7),
        };
        let mut bytes = section.to_bytes();

        // Locate the affinity_offsets table in the serialized bytes and corrupt
        // index 1 so it is larger than index 2, breaking the monotonic invariant.
        // Fixed header: version(1) + affinity_factor(1) + affinity_dims(12) +
        // animated_light_count(4) + tile_dimension(4) + tile_border(4) +
        // animation_descriptor_indices(4×1) = 30 bytes.
        let offsets_start = 1 + 1 + 12 + 4 + 4 + 4 + 4;
        // offsets[1] lives at offsets_start + 1×4; write a value larger than offsets[2].
        let corrupt_offset: u32 = 99;
        let off = offsets_start + 4;
        bytes[off..off + 4].copy_from_slice(&corrupt_offset.to_le_bytes());

        let err = DeltaShVolumesSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("non-decreasing"),
            "expected non-decreasing error: {msg}"
        );
    }

    #[test]
    fn rejects_out_of_range_light_index() {
        // Single light declared, but the CSR list references light index 5.
        let section = DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
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

    #[test]
    fn rejects_unsupported_tile_border() {
        let section = empty_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        // tile_border is the u32 immediately after tile_dimension in the fixed header.
        let tile_border_offset = 1 + 1 + 12 + 4 + 4;
        bytes[tile_border_offset..tile_border_offset + 4].copy_from_slice(&2u32.to_le_bytes());
        let err = DeltaShVolumesSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("tile_border"),
            "expected tile-border error: {msg}"
        );
    }

    #[test]
    fn rejects_unsupported_tile_dimension() {
        let section = empty_section([1, 1, 1]);
        let mut bytes = section.to_bytes();
        // tile_dimension follows animated_light_count in the fixed header.
        let tile_dimension_offset = 1 + 1 + 12 + 4;
        bytes[tile_dimension_offset..tile_dimension_offset + 4]
            .copy_from_slice(&8u32.to_le_bytes());
        let err = DeltaShVolumesSection::from_bytes(&bytes).unwrap_err();
        let msg = err.to_string();
        // The error is a runtime-capability limit (the format permits other N),
        // not a "format violation" — the wording must say so.
        assert!(
            msg.contains("tile_dimension") && msg.contains("not supported by this runtime"),
            "expected runtime-capability tile-dimension error: {msg}"
        );
    }
}
