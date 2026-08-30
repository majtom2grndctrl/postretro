// AnimatedBillboardDirectScatterDeltaVolumes PRL section (ID 48).
// See: context/lib/build_pipeline.md §PRL section IDs

use crate::FormatError;

/// Section-internal epoch for dense animated billboard direct-scatter deltas.
pub const ANIMATED_BILLBOARD_DIRECT_SCATTER_DELTA_VOLUMES_VERSION: u8 = 1;

/// A direct-scatter delta CSR entry always stores one 4×4×4 probe block.
pub const BILLBOARD_DIRECT_SCATTER_PROBES_PER_AFFINITY_ENTRY: usize = 64;
/// One `Rgba16Float` value has four f16 channel values.
pub const BILLBOARD_DIRECT_SCATTER_DELTA_RGBA_F16_COUNT: usize = 4;

/// Maximum encoded section-48 size accepted across compiler packing and level
/// loading. This is a cross-boundary resource policy, not an on-wire field.
/// The dense 64-probe RGBA16F block dominates the section, so 64 MiB keeps its
/// eventual storage buffer below the portable 128 MiB binding floor.
pub const MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES: u64 = 64 * 1024 * 1024;

const HEADER_SIZE: usize = 18;

/// Dense animated direct-scatter deltas for billboards.
///
/// `animation_descriptor_indices`, `affinity_offsets`, and `affinity_lights`
/// deliberately duplicate the layout of id 45. The loader proves they are
/// byte-for-byte equal to its usable id-45 sibling before exposing this
/// section. Unlike id 45, every CSR entry stores all 64 probes in x-fastest
/// 4×4×4 order: no validity masks, levels, or coarsening are encoded here.
/// RGB is a delta and alpha is reserved zero.
///
/// On disk (all little-endian):
///
/// ```text
/// u8      version (= ANIMATED_BILLBOARD_DIRECT_SCATTER_DELTA_VOLUMES_VERSION)
/// u8      affinity_factor
/// u32x3   affinity_dims
/// u32     animated_light_count
/// u32 × animated_light_count      animation_descriptor_indices
/// u32 × (product(affinity_dims) + 1) affinity_offsets
/// u32 × affinity_offsets[-1]      affinity_lights
/// u16 × (affinity_offsets[-1] × 64 × 4) delta_rgba
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimatedBillboardDirectScatterDeltaVolumesSection {
    /// One descriptor index per AnimatedBakedLights entry; `u32::MAX` remains
    /// the shared no-op sentinel.
    pub animation_descriptor_indices: Vec<u32>,
    /// Affinity-cell scale copied from id 45.
    pub affinity_factor: u8,
    /// Affinity-grid dimensions copied from id 45.
    pub affinity_dims: [u32; 3],
    /// CSR offsets copied from id 45.
    pub affinity_offsets: Vec<u32>,
    /// AnimatedBakedLights indices copied from id 45.
    pub affinity_lights: Vec<u32>,
    /// Dense RGBA16F values: `affinity_lights.len() × 64 × 4` f16 bits.
    pub delta_rgba: Vec<u16>,
}

impl AnimatedBillboardDirectScatterDeltaVolumesSection {
    pub fn affinity_cell_count(&self) -> Option<usize> {
        usize::try_from(self.affinity_dims[0])
            .ok()?
            .checked_mul(usize::try_from(self.affinity_dims[1]).ok()?)?
            .checked_mul(usize::try_from(self.affinity_dims[2]).ok()?)
    }

    pub fn expected_delta_f16_count(&self) -> Option<usize> {
        self.affinity_lights
            .len()
            .checked_mul(BILLBOARD_DIRECT_SCATTER_PROBES_PER_AFFINITY_ENTRY)?
            .checked_mul(BILLBOARD_DIRECT_SCATTER_DELTA_RGBA_F16_COUNT)
    }

    /// Exact encoded length for a layout that duplicates id 45's descriptor
    /// and CSR tables. Compiler policy uses this before materializing dense
    /// deltas; pack policy uses it before serialization.
    pub fn encoded_len_for_layout(
        animation_descriptor_count: usize,
        affinity_offset_count: usize,
        affinity_entry_count: usize,
    ) -> Option<u64> {
        let descriptor_bytes = u64::try_from(animation_descriptor_count)
            .ok()?
            .checked_mul(std::mem::size_of::<u32>() as u64)?;
        let offset_bytes = u64::try_from(affinity_offset_count)
            .ok()?
            .checked_mul(std::mem::size_of::<u32>() as u64)?;
        let light_bytes = u64::try_from(affinity_entry_count)
            .ok()?
            .checked_mul(std::mem::size_of::<u32>() as u64)?;
        let payload_bytes = u64::try_from(affinity_entry_count)
            .ok()?
            .checked_mul(BILLBOARD_DIRECT_SCATTER_PROBES_PER_AFFINITY_ENTRY as u64)?
            .checked_mul(BILLBOARD_DIRECT_SCATTER_DELTA_RGBA_F16_COUNT as u64)?
            .checked_mul(std::mem::size_of::<u16>() as u64)?;

        (HEADER_SIZE as u64)
            .checked_add(descriptor_bytes)?
            .checked_add(offset_bytes)?
            .checked_add(light_bytes)?
            .checked_add(payload_bytes)
    }

    pub fn encoded_len(&self) -> Option<u64> {
        Self::encoded_len_for_layout(
            self.animation_descriptor_indices.len(),
            self.affinity_offsets.len(),
            self.affinity_lights.len(),
        )
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.try_to_bytes().expect(
            "AnimatedBillboardDirectScatterDeltaVolumesSection must satisfy its wire contract",
        )
    }

    pub fn try_to_bytes(&self) -> crate::Result<Vec<u8>> {
        self.validate_wire_contract()?;
        let descriptor_bytes = self
            .animation_descriptor_indices
            .len()
            .checked_mul(std::mem::size_of::<u32>())
            .ok_or_else(|| {
                invalid_data("animated billboard scatter descriptor byte length overflows usize")
            })?;
        let offset_bytes = self
            .affinity_offsets
            .len()
            .checked_mul(std::mem::size_of::<u32>())
            .ok_or_else(|| {
                invalid_data("animated billboard scatter offset byte length overflows usize")
            })?;
        let light_bytes = self
            .affinity_lights
            .len()
            .checked_mul(std::mem::size_of::<u32>())
            .ok_or_else(|| {
                invalid_data("animated billboard scatter light byte length overflows usize")
            })?;
        let payload_bytes = self
            .delta_rgba
            .len()
            .checked_mul(std::mem::size_of::<u16>())
            .ok_or_else(|| {
                invalid_data("animated billboard scatter payload byte length overflows usize")
            })?;
        let output_len = HEADER_SIZE
            .checked_add(descriptor_bytes)
            .and_then(|len| len.checked_add(offset_bytes))
            .and_then(|len| len.checked_add(light_bytes))
            .and_then(|len| len.checked_add(payload_bytes))
            .ok_or_else(|| {
                invalid_data("animated billboard scatter section length overflows usize")
            })?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(output_len).map_err(|error| {
            invalid_data(format!(
                "animated billboard scatter cannot reserve {output_len} output bytes: {error}"
            ))
        })?;

        bytes.push(ANIMATED_BILLBOARD_DIRECT_SCATTER_DELTA_VOLUMES_VERSION);
        bytes.push(self.affinity_factor);
        for dimension in self.affinity_dims {
            bytes.extend_from_slice(&dimension.to_le_bytes());
        }
        bytes.extend_from_slice(&(self.animation_descriptor_indices.len() as u32).to_le_bytes());
        for descriptor in &self.animation_descriptor_indices {
            bytes.extend_from_slice(&descriptor.to_le_bytes());
        }
        for offset in &self.affinity_offsets {
            bytes.extend_from_slice(&offset.to_le_bytes());
        }
        for light in &self.affinity_lights {
            bytes.extend_from_slice(&light.to_le_bytes());
        }
        for half in &self.delta_rgba {
            bytes.extend_from_slice(&half.to_le_bytes());
        }
        Ok(bytes)
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(truncated("header"));
        }
        let version = data[0];
        if version != ANIMATED_BILLBOARD_DIRECT_SCATTER_DELTA_VOLUMES_VERSION {
            return Err(invalid_data(format!(
                "animated billboard direct scatter delta volumes section version {version}, expected {ANIMATED_BILLBOARD_DIRECT_SCATTER_DELTA_VOLUMES_VERSION} — recompile the .prl with the current `prl-build`"
            )));
        }
        let affinity_factor = data[1];
        let affinity_dims = [read_u32(data, 2), read_u32(data, 6), read_u32(data, 10)];
        let animated_light_count = usize_from_u32(read_u32(data, 14), "animated_light_count")?;
        let affinity_cell_count_usize = checked_affinity_cell_count(affinity_dims)?;

        let descriptor_bytes = checked_bytes(
            animated_light_count,
            std::mem::size_of::<u32>(),
            "descriptor table",
        )?;
        let offsets_len = affinity_cell_count_usize.checked_add(1).ok_or_else(|| {
            invalid_data("animated billboard scatter offset count overflows usize")
        })?;
        let offset_bytes = checked_bytes(offsets_len, std::mem::size_of::<u32>(), "offset table")?;
        let offsets_start = HEADER_SIZE.checked_add(descriptor_bytes).ok_or_else(|| {
            invalid_data("animated billboard scatter offsets start overflows usize")
        })?;
        let lights_start = offsets_start.checked_add(offset_bytes).ok_or_else(|| {
            invalid_data("animated billboard scatter lights start overflows usize")
        })?;
        if data.len() < lights_start {
            return Err(truncated("descriptor index or affinity offset table"));
        }

        let mut animation_descriptor_indices = Vec::new();
        animation_descriptor_indices
            .try_reserve_exact(animated_light_count)
            .map_err(|error| {
                invalid_data(format!(
                    "animated billboard scatter cannot reserve descriptor table: {error}"
                ))
            })?;
        for index in 0..animated_light_count {
            animation_descriptor_indices.push(read_u32(data, HEADER_SIZE + index * 4));
        }

        let mut affinity_offsets = Vec::new();
        affinity_offsets
            .try_reserve_exact(offsets_len)
            .map_err(|error| {
                invalid_data(format!(
                    "animated billboard scatter cannot reserve offset table: {error}"
                ))
            })?;
        for index in 0..offsets_len {
            affinity_offsets.push(read_u32(data, offsets_start + index * 4));
        }
        validate_offsets(&affinity_offsets)?;
        let light_count = usize_from_u32(
            *affinity_offsets
                .last()
                .expect("affinity offset table always contains its leading zero"),
            "affinity_offsets trailing total",
        )?;
        let light_bytes =
            checked_bytes(light_count, std::mem::size_of::<u32>(), "affinity lights")?;
        let payload_start = lights_start.checked_add(light_bytes).ok_or_else(|| {
            invalid_data("animated billboard scatter payload start overflows usize")
        })?;
        if data.len() < payload_start {
            return Err(truncated("affinity lights list"));
        }

        let mut affinity_lights = Vec::new();
        affinity_lights
            .try_reserve_exact(light_count)
            .map_err(|error| {
                invalid_data(format!(
                    "animated billboard scatter cannot reserve affinity lights: {error}"
                ))
            })?;
        for index in 0..light_count {
            let light = read_u32(data, lights_start + index * 4);
            if light as usize >= animated_light_count {
                return Err(invalid_data(format!(
                    "animated billboard scatter affinity_lights[{index}] value {light} is out of range for {animated_light_count} animated light(s)"
                )));
            }
            affinity_lights.push(light);
        }

        let delta_f16_count = light_count
            .checked_mul(BILLBOARD_DIRECT_SCATTER_PROBES_PER_AFFINITY_ENTRY)
            .and_then(|count| count.checked_mul(BILLBOARD_DIRECT_SCATTER_DELTA_RGBA_F16_COUNT))
            .ok_or_else(|| {
                invalid_data("animated billboard scatter payload f16 count overflows usize")
            })?;
        let payload_bytes = checked_bytes(
            delta_f16_count,
            std::mem::size_of::<u16>(),
            "dense delta payload",
        )?;
        let expected_len = payload_start.checked_add(payload_bytes).ok_or_else(|| {
            invalid_data("animated billboard scatter section length overflows usize")
        })?;
        if data.len() != expected_len {
            return Err(invalid_data(format!(
                "animated billboard scatter section length mismatch: expected {expected_len} bytes for {light_count} CSR entr(y/ies), got {}",
                data.len()
            )));
        }

        let mut delta_rgba = Vec::new();
        delta_rgba.try_reserve_exact(delta_f16_count).map_err(|error| {
            invalid_data(format!(
                "animated billboard scatter cannot reserve {delta_f16_count} delta f16 values: {error}"
            ))
        })?;
        for offset in (payload_start..expected_len).step_by(std::mem::size_of::<u16>()) {
            delta_rgba.push(read_u16(data, offset));
        }

        let section = Self {
            animation_descriptor_indices,
            affinity_factor,
            affinity_dims,
            affinity_offsets,
            affinity_lights,
            delta_rgba,
        };
        section.validate_wire_contract()?;
        Ok(section)
    }

    fn validate_wire_contract(&self) -> crate::Result<()> {
        u32::try_from(self.animation_descriptor_indices.len()).map_err(|_| {
            invalid_data("animated billboard scatter animated light count exceeds u32")
        })?;
        let affinity_cell_count = self.affinity_cell_count().ok_or_else(|| {
            invalid_data(format!(
                "animated billboard scatter affinity_dims {:?} overflow affinity cell count",
                self.affinity_dims
            ))
        })?;
        let expected_offsets_len = affinity_cell_count.checked_add(1).ok_or_else(|| {
            invalid_data("animated billboard scatter offset count overflows usize")
        })?;
        if self.affinity_offsets.len() != expected_offsets_len {
            return Err(invalid_data(format!(
                "animated billboard scatter affinity_offsets length {}, expected {expected_offsets_len}",
                self.affinity_offsets.len()
            )));
        }
        validate_offsets(&self.affinity_offsets)?;
        let trailing_total = usize_from_u32(
            *self
                .affinity_offsets
                .last()
                .expect("validated affinity offset table is non-empty"),
            "affinity_offsets trailing total",
        )?;
        if trailing_total != self.affinity_lights.len() {
            return Err(invalid_data(format!(
                "animated billboard scatter affinity_offsets trailing total {trailing_total} does not match affinity_lights length {}",
                self.affinity_lights.len()
            )));
        }
        for (entry, &light) in self.affinity_lights.iter().enumerate() {
            if light as usize >= self.animation_descriptor_indices.len() {
                return Err(invalid_data(format!(
                    "animated billboard scatter affinity_lights[{entry}] value {light} is out of range for {} animated light(s)",
                    self.animation_descriptor_indices.len()
                )));
            }
        }
        let expected_delta_count = self.expected_delta_f16_count().ok_or_else(|| {
            invalid_data("animated billboard scatter dense delta payload count overflows usize")
        })?;
        if self.delta_rgba.len() != expected_delta_count {
            return Err(invalid_data(format!(
                "animated billboard scatter delta_rgba length {}, expected {expected_delta_count}",
                self.delta_rgba.len()
            )));
        }
        for (sample, rgba) in self
            .delta_rgba
            .chunks_exact(BILLBOARD_DIRECT_SCATTER_DELTA_RGBA_F16_COUNT)
            .enumerate()
        {
            if rgba[3] != 0 {
                return Err(invalid_data(format!(
                    "animated billboard scatter delta sample {sample} has reserved alpha {:#06x}, expected zero",
                    rgba[3]
                )));
            }
        }
        Ok(())
    }
}

fn checked_affinity_cell_count(dimensions: [u32; 3]) -> crate::Result<usize> {
    usize::try_from(dimensions[0])
        .ok()
        .and_then(|count| count.checked_mul(usize::try_from(dimensions[1]).ok()?))
        .and_then(|count| count.checked_mul(usize::try_from(dimensions[2]).ok()?))
        .ok_or_else(|| {
            invalid_data(format!(
                "animated billboard scatter affinity_dims {dimensions:?} overflow affinity cell count"
            ))
        })
}

fn validate_offsets(offsets: &[u32]) -> crate::Result<()> {
    if offsets.first().copied() != Some(0) {
        return Err(invalid_data(
            "animated billboard scatter affinity_offsets[0] must be zero",
        ));
    }
    for (index, pair) in offsets.windows(2).enumerate() {
        if pair[0] > pair[1] {
            return Err(invalid_data(format!(
                "animated billboard scatter affinity_offsets[{index}] ({}) exceeds affinity_offsets[{}] ({})",
                pair[0],
                index + 1,
                pair[1]
            )));
        }
    }
    Ok(())
}

fn checked_bytes(count: usize, stride: usize, field: &str) -> crate::Result<usize> {
    count.checked_mul(stride).ok_or_else(|| {
        invalid_data(format!(
            "animated billboard scatter {field} byte length overflows usize"
        ))
    })
}

fn usize_from_u32(value: u32, field: &str) -> crate::Result<usize> {
    usize::try_from(value).map_err(|_| {
        invalid_data(format!(
            "animated billboard scatter {field} does not fit usize"
        ))
    })
}

fn truncated(what: &str) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        format!("animated billboard direct scatter delta volumes section truncated: {what}"),
    ))
}

fn invalid_data(message: impl Into<String>) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    ))
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SectionId;

    fn sample_section() -> AnimatedBillboardDirectScatterDeltaVolumesSection {
        AnimatedBillboardDirectScatterDeltaVolumesSection {
            animation_descriptor_indices: vec![7, u32::MAX],
            affinity_factor: 4,
            affinity_dims: [2, 1, 1],
            affinity_offsets: vec![0, 1, 2],
            affinity_lights: vec![0, 1],
            delta_rgba: vec![
                0;
                2 * BILLBOARD_DIRECT_SCATTER_PROBES_PER_AFFINITY_ENTRY
                    * BILLBOARD_DIRECT_SCATTER_DELTA_RGBA_F16_COUNT
            ],
        }
    }

    #[test]
    fn animated_billboard_direct_scatter_deltas_round_trip_dense_payload() {
        let mut section = sample_section();
        section.delta_rgba[0] = 1;
        section.delta_rgba[1] = 2;
        section.delta_rgba[2] = 3;
        let restored =
            AnimatedBillboardDirectScatterDeltaVolumesSection::from_bytes(&section.to_bytes())
                .expect("valid dense billboard scatter deltas must decode");

        assert_eq!(restored, section);
        assert_eq!(
            restored.delta_rgba.len(),
            2 * BILLBOARD_DIRECT_SCATTER_PROBES_PER_AFFINITY_ENTRY
                * BILLBOARD_DIRECT_SCATTER_DELTA_RGBA_F16_COUNT
        );
    }

    #[test]
    fn animated_billboard_direct_scatter_deltas_reject_version_csr_and_payload_errors() {
        let mut stale = sample_section().to_bytes();
        stale[0] = ANIMATED_BILLBOARD_DIRECT_SCATTER_DELTA_VOLUMES_VERSION - 1;
        assert!(AnimatedBillboardDirectScatterDeltaVolumesSection::from_bytes(&stale).is_err());

        let mut bad_csr = sample_section();
        bad_csr.affinity_offsets[1] = 2;
        bad_csr.affinity_offsets[2] = 1;
        assert!(bad_csr.try_to_bytes().is_err());

        let mut truncated_payload = sample_section().to_bytes();
        truncated_payload.pop();
        assert!(
            AnimatedBillboardDirectScatterDeltaVolumesSection::from_bytes(&truncated_payload)
                .is_err()
        );
    }

    #[test]
    fn animated_billboard_direct_scatter_deltas_reject_nonzero_reserved_alpha() {
        let mut section = sample_section();
        section.delta_rgba[3] = 1;
        assert!(section.try_to_bytes().is_err());
    }

    #[test]
    fn animated_billboard_direct_scatter_deltas_use_exact_header_offsets() {
        let bytes = sample_section().to_bytes();

        assert_eq!(
            bytes[0],
            ANIMATED_BILLBOARD_DIRECT_SCATTER_DELTA_VOLUMES_VERSION
        );
        assert_eq!(bytes[1], 4);
        assert_eq!(&bytes[2..14], &[2, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(&bytes[14..18], &[2, 0, 0, 0]);
        assert_eq!(sample_section().encoded_len(), Some(bytes.len() as u64));
    }

    #[test]
    fn animated_billboard_direct_scatter_deltas_reject_dimensions_that_misframe_csr() {
        let mut bytes = sample_section().to_bytes();
        bytes[2..6].copy_from_slice(&3u32.to_le_bytes());

        assert!(AnimatedBillboardDirectScatterDeltaVolumesSection::from_bytes(&bytes).is_err());
    }

    #[test]
    fn animated_billboard_direct_scatter_deltas_register_section_id_48() {
        assert_eq!(
            SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32,
            48
        );
        assert_eq!(
            SectionId::from_u32(48),
            Some(SectionId::AnimatedBillboardDirectScatterDeltaVolumes)
        );
    }
}
