// DirectShDeltaVolumes PRL section (ID 41): direct SH deltas for selected static lights.
// See: context/lib/build_pipeline.md §PRL section IDs

use crate::FormatError;
use crate::delta_sh_volumes::{
    DELTA_TILE_TEXEL_F16_COUNT, delta_probe_f16_stride, valid_probe_mask_payload_f16_count,
};
use crate::octahedral::{DEFAULT_IRRADIANCE_TILE_BORDER, RUNTIME_SUPPORTED_TILE_DIMENSION};
use crate::sh_reconstruct::Level;

/// Section-internal version, written as the first byte of the payload.
pub const DIRECT_SH_DELTA_VOLUMES_VERSION: u8 = 3;

/// Direct SH delta volumes section (ID 41), version 3.
///
/// On-disk layout (all little-endian):
///
/// ```text
///   u8       version                    (= DIRECT_SH_DELTA_VOLUMES_VERSION = 3)
///   u8       affinity_factor            (= AFFINITY_FACTOR = 4)
///   u32 × 3  affinity_dims              (affinity cells along x/y/z)
///   u32      tile_dimension             (default 6, border included)
///   u32      tile_border                (default 1)
///   u64 × affinity_cell_count            valid_probe_masks
///   u8  × affinity_cell_count            cell_levels (per-cell coarsening level)
///   u32 × (affinity_cell_count + 1)     affinity_offsets (CSR; last = list len)
///   u32 × affinity_offsets[-1]          affinity_lights (selection indices)
///   f16 × Σ(entry e) stored_delta_tiles(cell_levels[cell(e)], valid_probe_masks[cell(e)])
///       × tile_dimension × tile_dimension × 4
///                                       delta_subblocks
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct DirectShDeltaVolumesSection {
    pub affinity_factor: u8,
    pub affinity_dims: [u32; 3],
    pub tile_dimension: u32,
    pub tile_border: u32,
    /// One 64-bit valid-probe descriptor per affinity cell. Bit `local` is the
    /// canonical x-fastest local probe; entry payload tiles retain only kept
    /// bits in that order.
    pub valid_probe_masks: Vec<u64>,
    /// Per affinity-cell coarsening level (0 = L0 dense, 1 = L1 corners,
    /// 2 = L2 brick-mean). `len() == affinity_cell_count`; each byte in `0..=2`.
    /// The kept-probe set is derived from `(level, validity)` via `kept_mask`.
    pub cell_levels: Vec<u8>,
    /// CSR offsets, one per affinity cell plus a trailing total.
    pub affinity_offsets: Vec<u32>,
    /// Flat selected-light indices, grouped by affinity cell. Values index the
    /// `EntityShadowLights` order, not AlphaLights/global light indices.
    /// Unlike `DeltaShVolumesSection`, this section has no header field
    /// bounding the selection count, so `from_bytes` cannot self-validate
    /// these indices; bounds checking is the loader's job (see
    /// `validate_direct_sh_delta` in `crates/level-loader/src/prl_loader.rs`).
    pub affinity_lights: Vec<u32>,
    /// Flat probe payload, one compact valid-probe RGBA16F octahedral tile
    /// sub-block per CSR entry, index-parallel to `affinity_lights`.
    pub delta_subblocks: Vec<u16>,
}

impl DirectShDeltaVolumesSection {
    pub fn affinity_cell_count(&self) -> usize {
        self.affinity_dims[0] as usize
            * self.affinity_dims[1] as usize
            * self.affinity_dims[2] as usize
    }

    pub fn delta_probe_f16_stride(&self) -> usize {
        delta_probe_f16_stride(self.tile_dimension)
    }

    /// Expected compact payload length in f16 halves, derived from the CSR
    /// entry-to-cell mapping and the section's valid-probe descriptors.
    pub fn expected_delta_subblock_f16_count(&self) -> Option<usize> {
        valid_probe_mask_payload_f16_count(
            &self.affinity_offsets,
            &self.valid_probe_masks,
            &self.cell_levels,
            self.delta_probe_f16_stride(),
        )
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        debug_assert_eq!(self.affinity_offsets.len(), self.affinity_cell_count() + 1);
        debug_assert_eq!(self.valid_probe_masks.len(), self.affinity_cell_count());
        debug_assert_eq!(self.cell_levels.len(), self.affinity_cell_count());
        debug_assert_eq!(
            self.expected_delta_subblock_f16_count(),
            Some(self.delta_subblocks.len())
        );

        let mut buf = Vec::new();
        buf.push(DIRECT_SH_DELTA_VOLUMES_VERSION);
        buf.push(self.affinity_factor);
        for v in &self.affinity_dims {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(&self.tile_dimension.to_le_bytes());
        buf.extend_from_slice(&self.tile_border.to_le_bytes());
        for mask in &self.valid_probe_masks {
            buf.extend_from_slice(&mask.to_le_bytes());
        }
        for level in &self.cell_levels {
            buf.push(*level);
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
        const FIXED_HEADER_SIZE: usize = 1 + 1 + 12 + 4 + 4;
        if data.len() < FIXED_HEADER_SIZE {
            return Err(truncated("header"));
        }

        let mut o = 0;
        let version = data[o];
        o += 1;
        if version != DIRECT_SH_DELTA_VOLUMES_VERSION {
            return Err(invalid_data(format!(
                "direct sh delta volumes section version {version}, expected \
                 {DIRECT_SH_DELTA_VOLUMES_VERSION} — recompile the .prl with the current `prl-build`"
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
                    "direct sh delta volumes affinity_dims {affinity_dims:?} overflow: \
                     cell count exceeds usize"
                ))
            })?;

        let tile_dimension = read_u32(data, o);
        o += 4;
        let tile_border = read_u32(data, o);
        o += 4;
        validate_tile_geometry(tile_dimension, tile_border)?;
        let probe_f16_stride = delta_probe_f16_stride_checked(tile_dimension)?;

        let masks_bytes = affinity_cell_count.checked_mul(8).ok_or_else(|| {
            invalid_data(format!(
                "direct sh delta volumes valid_probe_masks length {affinity_cell_count} overflow"
            ))
        })?;
        if o.checked_add(masks_bytes)
            .is_none_or(|end| data.len() < end)
        {
            return Err(truncated("valid probe mask table"));
        }
        let mut valid_probe_masks = Vec::with_capacity(affinity_cell_count);
        for _ in 0..affinity_cell_count {
            valid_probe_masks.push(read_u64(data, o));
            o += 8;
        }

        // One coarsening-level byte per affinity cell, mirroring the mask table.
        if o.checked_add(affinity_cell_count)
            .is_none_or(|end| data.len() < end)
        {
            return Err(truncated("valid probe level table"));
        }
        let mut cell_levels = Vec::with_capacity(affinity_cell_count);
        for _ in 0..affinity_cell_count {
            let level = data[o];
            if Level::from_u8(level).is_none() {
                return Err(invalid_data(format!(
                    "direct sh delta volumes cell_levels entry {level} out of range: \
                     level must be 0..=2"
                )));
            }
            cell_levels.push(level);
            o += 1;
        }

        let offsets_len = affinity_cell_count + 1;
        let offsets_bytes = offsets_len.checked_mul(4).ok_or_else(|| {
            invalid_data(format!(
                "direct sh delta volumes affinity_offsets length {offsets_len} overflow"
            ))
        })?;
        if o.checked_add(offsets_bytes)
            .is_none_or(|end| data.len() < end)
        {
            return Err(truncated("affinity offsets table"));
        }
        let mut affinity_offsets = Vec::with_capacity(offsets_len);
        for _ in 0..offsets_len {
            affinity_offsets.push(read_u32(data, o));
            o += 4;
        }
        if affinity_offsets.first().copied() != Some(0) {
            return Err(invalid_data(format!(
                "direct sh delta volumes affinity_offsets[0] ({}) must be 0",
                affinity_offsets[0],
            )));
        }
        for k in 0..affinity_offsets.len() - 1 {
            if affinity_offsets[k] > affinity_offsets[k + 1] {
                return Err(invalid_data(format!(
                    "direct sh delta volumes affinity_offsets[{k}] ({}) > affinity_offsets[{}] ({}): \
                     offsets must be non-decreasing",
                    affinity_offsets[k],
                    k + 1,
                    affinity_offsets[k + 1],
                )));
            }
        }

        let list_len = *affinity_offsets
            .last()
            .expect("affinity_offsets has at least one entry") as usize;
        let lights_bytes = list_len.checked_mul(4).ok_or_else(|| {
            invalid_data(format!(
                "direct sh delta volumes affinity_lights length {list_len} overflow"
            ))
        })?;
        if data.len() < o + lights_bytes {
            return Err(truncated("affinity lights list"));
        }
        let mut affinity_lights = Vec::with_capacity(list_len);
        for _ in 0..list_len {
            affinity_lights.push(read_u32(data, o));
            o += 4;
        }

        let subblock_count = valid_probe_mask_payload_f16_count(
            &affinity_offsets,
            &valid_probe_masks,
            &cell_levels,
            probe_f16_stride,
        )
        .ok_or_else(|| {
            invalid_data(format!(
                "direct sh delta volumes delta_subblocks count overflow: \
                     CSR entries × valid probe counts × {probe_f16_stride} halves \
                     exceeds usize"
            ))
        })?;
        let subblock_bytes = subblock_count.checked_mul(2).ok_or_else(|| {
            invalid_data("direct sh delta volumes delta_subblocks byte size exceeds usize".into())
        })?;
        if data.len() < o + subblock_bytes {
            return Err(truncated("delta subblock probe data"));
        }
        let mut delta_subblocks = Vec::with_capacity(subblock_count);
        for _ in 0..subblock_count {
            delta_subblocks.push(read_u16(data, o));
            o += 2;
        }
        if o != data.len() {
            return Err(invalid_data(format!(
                "direct sh delta volumes has {} trailing byte(s)",
                data.len() - o
            )));
        }

        Ok(Self {
            affinity_factor,
            affinity_dims,
            tile_dimension,
            tile_border,
            valid_probe_masks,
            cell_levels,
            affinity_offsets,
            affinity_lights,
            delta_subblocks,
        })
    }
}

fn delta_probe_f16_stride_checked(tile_dimension: u32) -> crate::Result<usize> {
    (tile_dimension as usize)
        .checked_mul(tile_dimension as usize)
        .and_then(|n| n.checked_mul(DELTA_TILE_TEXEL_F16_COUNT))
        .ok_or_else(|| {
            invalid_data(format!(
                "direct sh delta volumes tile_dimension {tile_dimension} overflows probe tile stride"
            ))
        })
}

fn validate_tile_geometry(tile_dimension: u32, tile_border: u32) -> crate::Result<()> {
    if tile_border != DEFAULT_IRRADIANCE_TILE_BORDER {
        return Err(invalid_data(format!(
            "direct sh delta volumes tile_border {tile_border}, expected {DEFAULT_IRRADIANCE_TILE_BORDER}"
        )));
    }
    if tile_dimension != RUNTIME_SUPPORTED_TILE_DIMENSION {
        return Err(invalid_data(format!(
            "direct sh delta volumes tile_dimension {tile_dimension} is not supported by this runtime, which is pinned to N={RUNTIME_SUPPORTED_TILE_DIMENSION}"
        )));
    }
    if tile_dimension <= tile_border.saturating_mul(2) {
        return Err(invalid_data(format!(
            "direct sh delta volumes tile_dimension {tile_dimension} leaves no interior texels with border {tile_border}"
        )));
    }
    Ok(())
}

fn truncated(what: &str) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        format!("direct sh delta volumes section truncated: {what}"),
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

fn read_u64(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes([
        data[at],
        data[at + 1],
        data[at + 2],
        data[at + 3],
        data[at + 4],
        data[at + 5],
        data[at + 6],
        data[at + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SectionId;
    use crate::delta_sh_volumes::{
        AFFINITY_FACTOR, ALL_VALID_PROBE_MASK, DEFAULT_DELTA_PROBE_BYTES,
        DEFAULT_DELTA_PROBE_F16_STRIDE, PROBES_PER_CELL,
    };
    use crate::octahedral::DEFAULT_IRRADIANCE_TILE_DIMENSION;

    fn sample_subblock(seed: u16) -> Vec<u16> {
        sample_probe_tiles(seed, PROBES_PER_CELL)
    }

    fn sample_probe_tiles(seed: u16, probe_count: usize) -> Vec<u16> {
        let mut out = Vec::with_capacity(probe_count * DEFAULT_DELTA_PROBE_F16_STRIDE);
        for i in 0..probe_count * DEFAULT_DELTA_PROBE_F16_STRIDE {
            out.push(seed.wrapping_add(i as u16));
        }
        out
    }

    #[test]
    fn direct_sh_delta_volumes_round_trips_multiple_cells() {
        let mut delta_subblocks = sample_subblock(1);
        delta_subblocks.extend(sample_subblock(100));

        let section = DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [2, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: vec![ALL_VALID_PROBE_MASK; 2],
            cell_levels: vec![0; 2],
            affinity_offsets: vec![0, 2, 2],
            affinity_lights: vec![0, 1],
            delta_subblocks,
        };

        let restored = DirectShDeltaVolumesSection::from_bytes(&section.to_bytes()).unwrap();

        assert_eq!(restored, section);
        assert_eq!(
            DEFAULT_DELTA_PROBE_BYTES,
            DEFAULT_DELTA_PROBE_F16_STRIDE * 2
        );
        assert_eq!(
            restored.expected_delta_subblock_f16_count(),
            Some(2 * PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE),
            "all-valid descriptors retain the legacy dense-64 payload length"
        );
    }

    #[test]
    fn valid_probe_masks_compact_mixed_cells_and_allow_all_invalid_entries() {
        let mixed_mask = (1u64 << 2) | (1u64 << 7) | (1u64 << 63);
        let payload = sample_probe_tiles(200, mixed_mask.count_ones() as usize);
        let section = DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [2, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: vec![mixed_mask, 0],
            cell_levels: vec![0, 0],
            affinity_offsets: vec![0, 1, 2],
            affinity_lights: vec![0, 1],
            delta_subblocks: payload.clone(),
        };

        let restored = DirectShDeltaVolumesSection::from_bytes(&section.to_bytes()).unwrap();

        assert_eq!(restored, section);
        assert_eq!(
            restored.expected_delta_subblock_f16_count(),
            Some(3 * DEFAULT_DELTA_PROBE_F16_STRIDE)
        );
        assert_eq!(restored.delta_subblocks, payload);
    }

    #[test]
    fn direct_sh_delta_volumes_rejects_mismatched_section_version() {
        let mut bytes = DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: vec![ALL_VALID_PROBE_MASK],
            cell_levels: vec![0],
            affinity_offsets: vec![0, 0],
            affinity_lights: Vec::new(),
            delta_subblocks: Vec::new(),
        }
        .to_bytes();
        bytes[0] = DIRECT_SH_DELTA_VOLUMES_VERSION - 1;

        let err = DirectShDeltaVolumesSection::from_bytes(&bytes).unwrap_err();

        assert!(err.to_string().contains("recompile the .prl"));
    }

    #[test]
    fn direct_sh_delta_volumes_rejects_truncated_valid_probe_mask_table() {
        let bytes = vec![DIRECT_SH_DELTA_VOLUMES_VERSION, AFFINITY_FACTOR]
            .into_iter()
            .chain(
                [
                    1u32,
                    1,
                    1,
                    DEFAULT_IRRADIANCE_TILE_DIMENSION,
                    DEFAULT_IRRADIANCE_TILE_BORDER,
                ]
                .into_iter()
                .flat_map(u32::to_le_bytes),
            )
            .collect::<Vec<_>>();

        let err = DirectShDeltaVolumesSection::from_bytes(&bytes).unwrap_err();

        assert!(err.to_string().contains("valid probe mask table"));
    }

    #[test]
    fn direct_sh_delta_volumes_rejects_mask_driven_payload_length_mismatch() {
        let mask = (1u64 << 3) | (1u64 << 9);
        let section = DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: vec![mask],
            cell_levels: vec![0],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: sample_probe_tiles(30, mask.count_ones() as usize),
        };
        let mut bytes = section.to_bytes();
        bytes.truncate(bytes.len() - 2);

        let err = DirectShDeltaVolumesSection::from_bytes(&bytes).unwrap_err();

        assert!(err.to_string().contains("delta subblock probe data"));
    }

    #[test]
    fn direct_sh_delta_volumes_rejects_non_monotonic_offsets() {
        let section = DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [2, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: vec![ALL_VALID_PROBE_MASK; 2],
            cell_levels: vec![0; 2],
            affinity_offsets: vec![0, 1, 1],
            affinity_lights: vec![0],
            delta_subblocks: sample_subblock(7),
        };
        let mut bytes = section.to_bytes();
        // header(22) + masks(2×8) + cell_levels(2×1) + offsets[1] (4).
        let second_offset = 1 + 1 + 12 + 4 + 4 + 2 * 8 + 2 + 4;
        bytes[second_offset..second_offset + 4].copy_from_slice(&2u32.to_le_bytes());

        let err = DirectShDeltaVolumesSection::from_bytes(&bytes).unwrap_err();

        assert!(
            err.to_string().contains("non-decreasing"),
            "expected offset monotonicity error: {err}"
        );
    }

    #[test]
    fn direct_sh_delta_volumes_rejects_nonzero_first_offset() {
        let section = DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: vec![ALL_VALID_PROBE_MASK],
            cell_levels: vec![0],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: sample_subblock(9),
        };
        let mut bytes = section.to_bytes();
        // header(22) + masks(1×8) + cell_levels(1×1) → offsets[0].
        let first_offset = 1 + 1 + 12 + 4 + 4 + 8 + 1;
        bytes[first_offset..first_offset + 4].copy_from_slice(&1u32.to_le_bytes());

        let err = DirectShDeltaVolumesSection::from_bytes(&bytes).unwrap_err();

        assert!(
            err.to_string().contains("affinity_offsets[0]"),
            "expected first-offset validation error: {err}"
        );
    }

    #[test]
    fn mixed_level_round_trip_including_l2_brick() {
        use crate::sh_reconstruct::{Level, stored_delta_tiles};

        // Cell 0 is L1 (three valid corners + a non-corner); cell 1 is L2 over
        // 0b1111 (a single kept representative slot).
        let cell0_mask = (1u64 << 0) | (1u64 << 3) | (1u64 << 12) | (1u64 << 5);
        let cell1_mask = 0b1111u64;
        let levels = vec![Level::L1.to_u8(), Level::L2.to_u8()];
        let masks = vec![cell0_mask, cell1_mask];
        let offsets = vec![0u32, 1, 2];
        let stride = DEFAULT_DELTA_PROBE_F16_STRIDE;

        let kept0 = stored_delta_tiles(Level::L1, cell0_mask);
        let kept1 = stored_delta_tiles(Level::L2, cell1_mask);
        assert_eq!(kept0, 3);
        assert_eq!(kept1, 1);
        let payload = sample_probe_tiles(555, kept0 + kept1);

        let section = DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [2, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: masks.clone(),
            cell_levels: levels.clone(),
            affinity_offsets: offsets.clone(),
            affinity_lights: vec![0, 1],
            delta_subblocks: payload.clone(),
        };

        let restored = DirectShDeltaVolumesSection::from_bytes(&section.to_bytes()).unwrap();
        assert_eq!(restored, section);
        assert_eq!(
            valid_probe_mask_payload_f16_count(&offsets, &masks, &levels, stride),
            Some(payload.len()),
        );
    }

    #[test]
    fn rejects_out_of_range_cell_level() {
        // cell_levels byte sits after the fixed header (22) and the one u64 mask.
        let section = DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: vec![ALL_VALID_PROBE_MASK],
            cell_levels: vec![0],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: sample_subblock(11),
        };
        let mut bytes = section.to_bytes();
        let cell_level_offset = 1 + 1 + 12 + 4 + 4 + 8;
        bytes[cell_level_offset] = 3;

        let err = DirectShDeltaVolumesSection::from_bytes(&bytes).unwrap_err();
        assert!(
            err.to_string().contains("level"),
            "expected level error: {err}"
        );
    }

    #[test]
    fn direct_sh_delta_volumes_section_id_is_41() {
        assert_eq!(SectionId::DirectShDeltaVolumes as u32, 41);
        assert_eq!(
            SectionId::from_u32(41),
            Some(SectionId::DirectShDeltaVolumes)
        );
    }
}
