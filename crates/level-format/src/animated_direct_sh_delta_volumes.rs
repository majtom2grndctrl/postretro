// AnimatedDirectShDeltaVolumes PRL section (ID 45): direct SH deltas for animated baked lights.
// See: context/plans/in-progress/animated-direct-sh-dynamic-receivers/

use crate::FormatError;
use crate::delta_sh_volumes::{
    DELTA_TILE_TEXEL_F16_COUNT, PROBES_PER_CELL, delta_probe_f16_stride,
};
use crate::octahedral::{DEFAULT_IRRADIANCE_TILE_BORDER, RUNTIME_SUPPORTED_TILE_DIMENSION};

/// Section-internal version, written as the first byte of the payload.
pub const ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION: u8 = 1;

/// Animated direct SH delta volumes section (ID 45), version 1.
///
/// This is the direct-lighting counterpart to `DeltaShVolumesSection` (ID 27).
/// Its descriptor-index table and CSR light indices are independently keyed by
/// `AnimatedBakedLights` indices; they do not reference promotion selections.
///
/// On-disk layout (all little-endian):
///
/// ```text
///   u8       version                    (= ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION = 1)
///   u8       affinity_factor            (= AFFINITY_FACTOR = 4)
///   u32 × 3  affinity_dims              (affinity cells along x/y/z)
///   u32      animated_light_count
///   u32      tile_dimension             (default 6, border included)
///   u32      tile_border                (default 1)
///   u32 × animated_light_count          animation_descriptor_indices
///   u32 × (affinity_cell_count + 1)     affinity_offsets (CSR; last = list len)
///   u32 × affinity_offsets[-1]          affinity_lights (AnimatedBakedLights indices)
///   f16 × affinity_offsets[-1] × 64 × tile_dimension × tile_dimension × 4
///                                       delta_subblocks
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct AnimatedDirectShDeltaVolumesSection {
    /// Affinity cell edge length in base probes.
    pub affinity_factor: u8,
    /// Affinity grid dimensions in cells along x/y/z.
    pub affinity_dims: [u32; 3],
    /// Full octahedral tile dimension, including border texels.
    pub tile_dimension: u32,
    /// Octahedral wrap border width.
    pub tile_border: u32,
    /// One entry per AnimatedBakedLights index: index into the shared compose
    /// descriptor array. `u32::MAX` is the runtime no-op sentinel.
    pub animation_descriptor_indices: Vec<u32>,
    /// CSR offsets, one per affinity cell plus a trailing total.
    pub affinity_offsets: Vec<u32>,
    /// Flat AnimatedBakedLights indices, grouped by affinity cell. Each value
    /// must be `< animation_descriptor_indices.len()`.
    pub affinity_lights: Vec<u32>,
    /// Flat probe payload, one dense 64-probe RGBA16F octahedral tile sub-block
    /// per CSR entry, index-parallel to `affinity_lights`.
    pub delta_subblocks: Vec<u16>,
}

impl AnimatedDirectShDeltaVolumesSection {
    /// Number of affinity cells implied by `affinity_dims`.
    pub fn affinity_cell_count(&self) -> usize {
        self.affinity_dims[0] as usize
            * self.affinity_dims[1] as usize
            * self.affinity_dims[2] as usize
    }

    /// Number of f16 halves in one probe's octahedral irradiance tile.
    pub fn delta_probe_f16_stride(&self) -> usize {
        delta_probe_f16_stride(self.tile_dimension)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.try_to_bytes()
            .expect("AnimatedDirectShDeltaVolumesSection must satisfy its wire contract")
    }

    /// Encode only a canonical, internally consistent section.
    pub fn try_to_bytes(&self) -> crate::Result<Vec<u8>> {
        self.validate_wire_contract()?;

        let mut buf = Vec::new();
        buf.push(ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION);
        buf.push(self.affinity_factor);
        for value in &self.affinity_dims {
            buf.extend_from_slice(&value.to_le_bytes());
        }

        buf.extend_from_slice(&(self.animation_descriptor_indices.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.tile_dimension.to_le_bytes());
        buf.extend_from_slice(&self.tile_border.to_le_bytes());
        for index in &self.animation_descriptor_indices {
            buf.extend_from_slice(&index.to_le_bytes());
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
        Ok(buf)
    }

    fn validate_wire_contract(&self) -> crate::Result<()> {
        validate_tile_geometry(self.tile_dimension, self.tile_border)?;

        let affinity_cell_count = (self.affinity_dims[0] as usize)
            .checked_mul(self.affinity_dims[1] as usize)
            .and_then(|count| count.checked_mul(self.affinity_dims[2] as usize))
            .ok_or_else(|| {
                invalid_data(format!(
                    "animated direct sh delta volumes affinity_dims {:?} overflow: cell count exceeds usize",
                    self.affinity_dims
                ))
            })?;
        let expected_offsets_len = affinity_cell_count.checked_add(1).ok_or_else(|| {
            invalid_data(
                "animated direct sh delta volumes affinity offset count exceeds usize".into(),
            )
        })?;
        if self.affinity_offsets.len() != expected_offsets_len {
            return Err(invalid_data(format!(
                "animated direct sh delta volumes affinity_offsets length {}, expected {expected_offsets_len}",
                self.affinity_offsets.len()
            )));
        }
        if self.affinity_offsets.first().copied() != Some(0) {
            return Err(invalid_data(
                "animated direct sh delta volumes affinity_offsets[0] must be 0".into(),
            ));
        }
        for (index, offsets) in self.affinity_offsets.windows(2).enumerate() {
            if offsets[0] > offsets[1] {
                return Err(invalid_data(format!(
                    "animated direct sh delta volumes affinity_offsets[{index}] ({}) > affinity_offsets[{}] ({}): offsets must be non-decreasing",
                    offsets[0],
                    index + 1,
                    offsets[1],
                )));
            }
        }
        let trailing_total = self
            .affinity_offsets
            .last()
            .copied()
            .expect("validated offset table is non-empty") as usize;
        if trailing_total != self.affinity_lights.len() {
            return Err(invalid_data(format!(
                "animated direct sh delta volumes affinity_offsets trailing total {trailing_total} does not match affinity_lights length {}",
                self.affinity_lights.len()
            )));
        }
        for (entry, &light) in self.affinity_lights.iter().enumerate() {
            if light as usize >= self.animation_descriptor_indices.len() {
                return Err(invalid_data(format!(
                    "animated direct sh delta volumes affinity_lights[{entry}] entry {light} out of range (animated_light_count = {})",
                    self.animation_descriptor_indices.len()
                )));
            }
        }

        let probe_f16_stride = delta_probe_f16_stride_checked(self.tile_dimension)?;
        let expected_subblock_count = self
            .affinity_lights
            .len()
            .checked_mul(PROBES_PER_CELL)
            .and_then(|count| count.checked_mul(probe_f16_stride))
            .ok_or_else(|| {
                invalid_data(
                    "animated direct sh delta volumes delta_subblocks count exceeds usize".into(),
                )
            })?;
        if self.delta_subblocks.len() != expected_subblock_count {
            return Err(invalid_data(format!(
                "animated direct sh delta volumes delta_subblocks length {}, expected {expected_subblock_count}",
                self.delta_subblocks.len()
            )));
        }

        u32::try_from(self.animation_descriptor_indices.len()).map_err(|_| {
            invalid_data("animated direct sh delta volumes animated light count exceeds u32".into())
        })?;
        Ok(())
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        // version(1) + affinity_factor(1) + affinity_dims(12) +
        // animated_light_count(4) + tile_dimension(4) + tile_border(4)
        const FIXED_HEADER_SIZE: usize = 1 + 1 + 12 + 4 + 4 + 4;
        if data.len() < FIXED_HEADER_SIZE {
            return Err(truncated("header"));
        }

        let mut offset = 0;
        let version = data[offset];
        offset += 1;
        if version != ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION {
            return Err(invalid_data(format!(
                "animated direct sh delta volumes section version {version}, expected \
                 {ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION} — recompile the .prl with the current `prl-build`"
            )));
        }

        let affinity_factor = data[offset];
        offset += 1;
        let affinity_dims = [
            read_u32(data, offset),
            read_u32(data, offset + 4),
            read_u32(data, offset + 8),
        ];
        offset += 12;
        let affinity_cell_count = (affinity_dims[0] as usize)
            .checked_mul(affinity_dims[1] as usize)
            .and_then(|count| count.checked_mul(affinity_dims[2] as usize))
            .ok_or_else(|| {
                invalid_data(format!(
                    "animated direct sh delta volumes affinity_dims {affinity_dims:?} overflow: \
                     cell count exceeds usize"
                ))
            })?;

        let animated_light_count = read_u32(data, offset) as usize;
        offset += 4;
        let tile_dimension = read_u32(data, offset);
        offset += 4;
        let tile_border = read_u32(data, offset);
        offset += 4;
        validate_tile_geometry(tile_dimension, tile_border)?;
        let probe_f16_stride = delta_probe_f16_stride_checked(tile_dimension)?;

        let descriptor_bytes = animated_light_count.checked_mul(4).ok_or_else(|| {
            invalid_data(format!(
                "animated direct sh delta volumes animated_light_count {animated_light_count} \
                 overflows descriptor-index table size"
            ))
        })?;
        if data.len() < offset + descriptor_bytes {
            return Err(truncated("animation descriptor index table"));
        }
        let mut animation_descriptor_indices = Vec::with_capacity(animated_light_count);
        for _ in 0..animated_light_count {
            animation_descriptor_indices.push(read_u32(data, offset));
            offset += 4;
        }

        let offsets_len = affinity_cell_count.checked_add(1).ok_or_else(|| {
            invalid_data(format!(
                "animated direct sh delta volumes affinity_cell_count {affinity_cell_count} \
                 overflows affinity_offsets length"
            ))
        })?;
        let offsets_bytes = offsets_len.checked_mul(4).ok_or_else(|| {
            invalid_data(format!(
                "animated direct sh delta volumes affinity_offsets length {offsets_len} \
                 overflows table size"
            ))
        })?;
        if data.len() < offset + offsets_bytes {
            return Err(truncated("affinity offsets table"));
        }
        let mut affinity_offsets = Vec::with_capacity(offsets_len);
        for _ in 0..offsets_len {
            affinity_offsets.push(read_u32(data, offset));
            offset += 4;
        }
        if affinity_offsets.first().copied() != Some(0) {
            return Err(invalid_data(
                "animated direct sh delta volumes affinity_offsets[0] must be 0".into(),
            ));
        }
        for index in 0..affinity_offsets.len() - 1 {
            if affinity_offsets[index] > affinity_offsets[index + 1] {
                return Err(invalid_data(format!(
                    "animated direct sh delta volumes affinity_offsets[{index}] ({}) > \
                     affinity_offsets[{}] ({}): offsets must be non-decreasing",
                    affinity_offsets[index],
                    index + 1,
                    affinity_offsets[index + 1],
                )));
            }
        }

        let list_len = *affinity_offsets
            .last()
            .expect("affinity_offsets has at least one entry (cell_count + 1 >= 1)")
            as usize;
        let lights_bytes = list_len.checked_mul(4).ok_or_else(|| {
            invalid_data(format!(
                "animated direct sh delta volumes affinity_lights length {list_len} \
                 overflows list size"
            ))
        })?;
        if data.len() < offset + lights_bytes {
            return Err(truncated("affinity lights list"));
        }
        let mut affinity_lights = Vec::with_capacity(list_len);
        for _ in 0..list_len {
            let light = read_u32(data, offset);
            if (light as usize) >= animated_light_count {
                return Err(invalid_data(format!(
                    "animated direct sh delta volumes affinity_lights entry {light} out of range \
                     (animated_light_count = {animated_light_count})"
                )));
            }
            affinity_lights.push(light);
            offset += 4;
        }

        let subblock_count = list_len
            .checked_mul(PROBES_PER_CELL)
            .and_then(|count| count.checked_mul(probe_f16_stride))
            .ok_or_else(|| {
                invalid_data(format!(
                    "animated direct sh delta volumes delta_subblocks count overflow: \
                     {list_len} entries × {PROBES_PER_CELL} probes × \
                     {probe_f16_stride} halves exceeds usize"
                ))
            })?;
        let subblock_bytes = subblock_count.checked_mul(2).ok_or_else(|| {
            invalid_data(
                "animated direct sh delta volumes delta_subblocks byte size exceeds usize".into(),
            )
        })?;
        if data.len() < offset + subblock_bytes {
            return Err(truncated("delta subblock probe data"));
        }
        let mut delta_subblocks = Vec::with_capacity(subblock_count);
        for _ in 0..subblock_count {
            delta_subblocks.push(read_u16(data, offset));
            offset += 2;
        }
        if offset != data.len() {
            return Err(invalid_data(format!(
                "animated direct sh delta volumes has {} trailing byte(s)",
                data.len() - offset
            )));
        }

        let section = Self {
            affinity_factor,
            affinity_dims,
            tile_dimension,
            tile_border,
            animation_descriptor_indices,
            affinity_offsets,
            affinity_lights,
            delta_subblocks,
        };
        section.validate_wire_contract()?;
        Ok(section)
    }
}

fn delta_probe_f16_stride_checked(tile_dimension: u32) -> crate::Result<usize> {
    (tile_dimension as usize)
        .checked_mul(tile_dimension as usize)
        .and_then(|stride| stride.checked_mul(DELTA_TILE_TEXEL_F16_COUNT))
        .ok_or_else(|| {
            invalid_data(format!(
                "animated direct sh delta volumes tile_dimension {tile_dimension} overflows probe tile stride"
            ))
        })
}

fn validate_tile_geometry(tile_dimension: u32, tile_border: u32) -> crate::Result<()> {
    if tile_border != DEFAULT_IRRADIANCE_TILE_BORDER {
        return Err(invalid_data(format!(
            "animated direct sh delta volumes tile_border {tile_border}, expected {DEFAULT_IRRADIANCE_TILE_BORDER}"
        )));
    }
    if tile_dimension != RUNTIME_SUPPORTED_TILE_DIMENSION {
        return Err(invalid_data(format!(
            "animated direct sh delta volumes tile_dimension {tile_dimension} is not supported by this runtime, which is pinned to N={RUNTIME_SUPPORTED_TILE_DIMENSION}"
        )));
    }
    if tile_dimension <= tile_border.saturating_mul(2) {
        return Err(invalid_data(format!(
            "animated direct sh delta volumes tile_dimension {tile_dimension} leaves no interior texels with border {tile_border}"
        )));
    }
    Ok(())
}

fn truncated(what: &str) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        format!("animated direct sh delta volumes section truncated: {what}"),
    ))
}

fn invalid_data(message: String) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    ))
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SectionId;
    use crate::delta_sh_volumes::{AFFINITY_FACTOR, DEFAULT_DELTA_PROBE_F16_STRIDE};
    use crate::octahedral::DEFAULT_IRRADIANCE_TILE_DIMENSION;

    fn sample_subblock(seed: u16) -> Vec<u16> {
        (0..PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE)
            .map(|index| seed.wrapping_add(index as u16))
            .collect()
    }

    #[test]
    fn animated_direct_sh_delta_volumes_round_trip() {
        let mut delta_subblocks = sample_subblock(1);
        delta_subblocks.extend(sample_subblock(100));
        let section = AnimatedDirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [2, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![7, u32::MAX],
            affinity_offsets: vec![0, 2, 2],
            affinity_lights: vec![0, 1],
            delta_subblocks,
        };

        let restored = AnimatedDirectShDeltaVolumesSection::from_bytes(&section.to_bytes())
            .expect("valid animated direct deltas must decode");

        assert_eq!(restored, section);
    }

    #[test]
    fn animated_direct_sh_delta_volumes_header_preserves_mirrored_field_order() {
        let section = AnimatedDirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [2, 3, 4],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![9],
            affinity_offsets: vec![0; 25],
            affinity_lights: Vec::new(),
            delta_subblocks: Vec::new(),
        };

        let bytes = section.to_bytes();
        let mut expected = vec![ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION, AFFINITY_FACTOR];
        for value in [
            2u32,
            3,
            4,
            1,
            DEFAULT_IRRADIANCE_TILE_DIMENSION,
            DEFAULT_IRRADIANCE_TILE_BORDER,
            9,
        ] {
            expected.extend_from_slice(&value.to_le_bytes());
        }

        assert_eq!(&bytes[..expected.len()], expected.as_slice());
    }

    #[test]
    fn animated_direct_sh_delta_volumes_rejects_out_of_range_affinity_light() {
        let section = AnimatedDirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![0],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: sample_subblock(3),
        };
        let mut bytes = section.to_bytes();
        let affinity_light_offset = 1 + 1 + 12 + 4 + 4 + 4 + 4 + 8;
        bytes[affinity_light_offset..affinity_light_offset + 4]
            .copy_from_slice(&1u32.to_le_bytes());

        let error = AnimatedDirectShDeltaVolumesSection::from_bytes(&bytes)
            .expect_err("out-of-range AnimatedBakedLights index must be rejected");

        assert!(error.to_string().contains("out of range"));
    }

    #[test]
    fn animated_direct_sh_delta_volumes_rejects_trailing_bytes() {
        let section = AnimatedDirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![0],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: sample_subblock(5),
        };
        let mut bytes = section.to_bytes();
        bytes.push(0);

        let error = AnimatedDirectShDeltaVolumesSection::from_bytes(&bytes)
            .expect_err("section payload must be consumed exactly");

        assert!(error.to_string().contains("trailing byte"));
    }

    #[test]
    fn animated_direct_sh_delta_volumes_pack_rejects_invalid_csr_shapes() {
        let valid = AnimatedDirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![0],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: sample_subblock(7),
        };

        let mut cases = Vec::new();
        let mut bad_offset_len = valid.clone();
        bad_offset_len.affinity_offsets.pop();
        cases.push(bad_offset_len);
        let mut bad_first_offset = valid.clone();
        bad_first_offset.affinity_offsets[0] = 1;
        cases.push(bad_first_offset);
        let mut non_monotonic = valid.clone();
        non_monotonic.affinity_dims = [2, 1, 1];
        non_monotonic.affinity_offsets = vec![0, 1, 0];
        non_monotonic.affinity_lights.clear();
        non_monotonic.delta_subblocks.clear();
        cases.push(non_monotonic);
        let mut bad_trailing_offset = valid.clone();
        bad_trailing_offset.affinity_offsets[1] = 0;
        cases.push(bad_trailing_offset);
        let mut bad_subblock_len = valid;
        bad_subblock_len.delta_subblocks.pop();
        cases.push(bad_subblock_len);

        for section in cases {
            assert!(
                section.try_to_bytes().is_err(),
                "invalid constructible CSR must not serialize"
            );
        }
    }

    #[test]
    fn animated_direct_sh_delta_volumes_section_id_is_45() {
        assert_eq!(SectionId::AnimatedDirectShDeltaVolumes as u32, 45);
        assert_eq!(
            SectionId::from_u32(45),
            Some(SectionId::AnimatedDirectShDeltaVolumes)
        );
    }
}
