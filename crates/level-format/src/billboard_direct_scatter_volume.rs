// BillboardDirectScatterVolume PRL section (ID 47): static direct scatter for billboards.
// See: context/lib/build_pipeline.md §PRL section IDs

use crate::FormatError;

/// Section-internal epoch for the billboard direct-scatter base grid.
pub const BILLBOARD_DIRECT_SCATTER_VOLUME_VERSION: u32 = 1;

/// One `Rgba16Float` value in f16 bit representation.
pub const BILLBOARD_DIRECT_SCATTER_RGBA_F16_COUNT: usize = 4;

/// Binary f16 representation of `1.0`, used in the alpha validity channel.
pub const BILLBOARD_DIRECT_SCATTER_VALIDITY_ONE_F16: u16 = 0x3c00;

const HEADER_SIZE: usize = 40;

/// Dense, normal-free baked static direct scatter for billboard shading.
///
/// The grid uses the same x-fastest probe order as `OctahedralShVolume` (id
/// 34). RGB is static scatter; alpha is the binary validity mirror of the
/// corresponding id-34 probe (`0.0` or `1.0`).
///
/// On disk (little-endian):
///
/// ```text
/// u32     version (= BILLBOARD_DIRECT_SCATTER_VOLUME_VERSION)
/// f32x3   grid_origin
/// f32x3   cell_size
/// u32x3   grid_dimensions
/// u16x4 × (grid_x × grid_y × grid_z)  scatter_rgba
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct BillboardDirectScatterVolumeSection {
    pub grid_origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub grid_dimensions: [u32; 3],
    /// Flat x-fastest `Rgba16Float` values as raw f16 channel bits.
    pub scatter_rgba: Vec<u16>,
}

impl BillboardDirectScatterVolumeSection {
    /// Number of probes in the dense grid, when the dimensions fit `usize`.
    pub fn total_probes(&self) -> Option<usize> {
        checked_total_probes(self.grid_dimensions)
    }

    /// Expected number of f16 halves in `scatter_rgba`.
    pub fn expected_scatter_f16_count(&self) -> Option<usize> {
        self.total_probes()?
            .checked_mul(BILLBOARD_DIRECT_SCATTER_RGBA_F16_COUNT)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.try_to_bytes()
            .expect("BillboardDirectScatterVolumeSection must satisfy its wire contract")
    }

    pub fn try_to_bytes(&self) -> crate::Result<Vec<u8>> {
        self.validate_wire_contract()?;
        let payload_bytes = self
            .scatter_rgba
            .len()
            .checked_mul(std::mem::size_of::<u16>())
            .ok_or_else(|| {
                invalid_data("billboard direct scatter payload byte length overflows usize")
            })?;
        let output_len = HEADER_SIZE.checked_add(payload_bytes).ok_or_else(|| {
            invalid_data("billboard direct scatter section length overflows usize")
        })?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(output_len).map_err(|error| {
            invalid_data(format!(
                "billboard direct scatter cannot reserve {output_len} output bytes: {error}"
            ))
        })?;

        bytes.extend_from_slice(&BILLBOARD_DIRECT_SCATTER_VOLUME_VERSION.to_le_bytes());
        for value in self.grid_origin {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in self.cell_size {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in self.grid_dimensions {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for half in &self.scatter_rgba {
            bytes.extend_from_slice(&half.to_le_bytes());
        }
        Ok(bytes)
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(truncated("header"));
        }
        let version = read_u32(data, 0);
        if version != BILLBOARD_DIRECT_SCATTER_VOLUME_VERSION {
            return Err(invalid_data(format!(
                "billboard direct scatter volume section version {version}, expected {BILLBOARD_DIRECT_SCATTER_VOLUME_VERSION} — recompile the .prl with the current `prl-build`"
            )));
        }
        if data.len() < HEADER_SIZE {
            return Err(truncated("header"));
        }

        let grid_origin = [read_f32(data, 4), read_f32(data, 8), read_f32(data, 12)];
        let cell_size = [read_f32(data, 16), read_f32(data, 20), read_f32(data, 24)];
        let grid_dimensions = [read_u32(data, 28), read_u32(data, 32), read_u32(data, 36)];
        let scatter_f16_count = checked_total_probes(grid_dimensions)
            .and_then(|count| count.checked_mul(BILLBOARD_DIRECT_SCATTER_RGBA_F16_COUNT))
            .ok_or_else(|| {
                invalid_data(format!(
                    "billboard direct scatter grid_dimensions {grid_dimensions:?} overflow payload length"
                ))
            })?;
        let payload_bytes = scatter_f16_count
            .checked_mul(std::mem::size_of::<u16>())
            .ok_or_else(|| {
                invalid_data("billboard direct scatter payload byte length overflows usize")
            })?;
        let expected_len = HEADER_SIZE.checked_add(payload_bytes).ok_or_else(|| {
            invalid_data("billboard direct scatter section length overflows usize")
        })?;
        if data.len() != expected_len {
            return Err(invalid_data(format!(
                "billboard direct scatter section length mismatch: expected {expected_len} bytes for grid {grid_dimensions:?}, got {}",
                data.len()
            )));
        }

        let mut scatter_rgba = Vec::new();
        scatter_rgba.try_reserve_exact(scatter_f16_count).map_err(|error| {
            invalid_data(format!(
                "billboard direct scatter cannot reserve {scatter_f16_count} f16 values: {error}"
            ))
        })?;
        for offset in (HEADER_SIZE..expected_len).step_by(std::mem::size_of::<u16>()) {
            scatter_rgba.push(read_u16(data, offset));
        }

        let section = Self {
            grid_origin,
            cell_size,
            grid_dimensions,
            scatter_rgba,
        };
        section.validate_wire_contract()?;
        Ok(section)
    }

    fn validate_wire_contract(&self) -> crate::Result<()> {
        let expected = self.expected_scatter_f16_count().ok_or_else(|| {
            invalid_data(format!(
                "billboard direct scatter grid_dimensions {:?} overflow payload length",
                self.grid_dimensions
            ))
        })?;
        if self.scatter_rgba.len() != expected {
            return Err(invalid_data(format!(
                "billboard direct scatter scatter_rgba length {}, expected {expected}",
                self.scatter_rgba.len()
            )));
        }
        for (probe, rgba) in self
            .scatter_rgba
            .chunks_exact(BILLBOARD_DIRECT_SCATTER_RGBA_F16_COUNT)
            .enumerate()
        {
            if rgba[3] != 0 && rgba[3] != BILLBOARD_DIRECT_SCATTER_VALIDITY_ONE_F16 {
                return Err(invalid_data(format!(
                    "billboard direct scatter probe {probe} alpha {:#06x} is not binary f16 validity 0 or 1",
                    rgba[3]
                )));
            }
        }
        Ok(())
    }
}

fn checked_total_probes(dimensions: [u32; 3]) -> Option<usize> {
    usize::try_from(dimensions[0])
        .ok()?
        .checked_mul(usize::try_from(dimensions[1]).ok()?)?
        .checked_mul(usize::try_from(dimensions[2]).ok()?)
}

fn truncated(what: &str) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        format!("billboard direct scatter volume section truncated: {what}"),
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

fn read_f32(data: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes([
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

    fn sample_section() -> BillboardDirectScatterVolumeSection {
        BillboardDirectScatterVolumeSection {
            grid_origin: [1.0, 2.0, 3.0],
            cell_size: [0.5, 1.0, 2.0],
            grid_dimensions: [2, 1, 1],
            scatter_rgba: vec![
                1,
                2,
                3,
                BILLBOARD_DIRECT_SCATTER_VALIDITY_ONE_F16,
                4,
                5,
                6,
                0,
            ],
        }
    }

    #[test]
    fn billboard_direct_scatter_volume_round_trips_dense_x_fastest_grid() {
        let section = sample_section();
        let restored = BillboardDirectScatterVolumeSection::from_bytes(&section.to_bytes())
            .expect("valid billboard scatter section must decode");

        assert_eq!(restored, section);
        assert_eq!(
            restored.scatter_rgba[0..4],
            [1, 2, 3, BILLBOARD_DIRECT_SCATTER_VALIDITY_ONE_F16]
        );
        assert_eq!(restored.scatter_rgba[4..8], [4, 5, 6, 0]);
    }

    #[test]
    fn billboard_direct_scatter_volume_rejects_version_and_nonbinary_validity() {
        let mut stale = sample_section().to_bytes();
        stale[..4].copy_from_slice(&(BILLBOARD_DIRECT_SCATTER_VOLUME_VERSION - 1).to_le_bytes());
        assert!(
            BillboardDirectScatterVolumeSection::from_bytes(&stale)
                .expect_err("stale version must reject")
                .to_string()
                .contains("recompile")
        );

        let mut invalid = sample_section();
        invalid.scatter_rgba[3] = 0x3555;
        assert!(invalid.try_to_bytes().is_err());
    }

    #[test]
    fn billboard_direct_scatter_volume_rejects_wrong_payload_length() {
        let mut bytes = sample_section().to_bytes();
        bytes.pop();
        assert!(BillboardDirectScatterVolumeSection::from_bytes(&bytes).is_err());
    }

    #[test]
    fn billboard_direct_scatter_volume_registers_section_id_47() {
        assert_eq!(SectionId::BillboardDirectScatterVolume as u32, 47);
        assert_eq!(
            SectionId::from_u32(47),
            Some(SectionId::BillboardDirectScatterVolume)
        );
    }
}
