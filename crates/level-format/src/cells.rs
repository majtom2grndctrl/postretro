// Cells PRL section (ID 38): runtime visibility units preserving BSP leaf ids.
// See: context/lib/build_pipeline.md §PRL Compilation

use crate::FormatError;

pub const CELLS_VERSION: u32 = 1;
pub const HEADER_SIZE: usize = 16;
pub const CELL_RECORD_SIZE: usize = 44;
pub const PORTAL_REF_STRIDE: usize = 4;

pub const CELL_FLAG_SOLID: u32 = 1 << 0;
pub const CELL_FLAG_EXTERIOR: u32 = 1 << 1;
pub const CELL_FLAG_DRAWABLE: u32 = 1 << 2;
const KNOWN_FLAGS: u32 = CELL_FLAG_SOLID | CELL_FLAG_EXTERIOR | CELL_FLAG_DRAWABLE;

#[derive(Debug, Clone, PartialEq)]
pub struct CellRecord {
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub flags: u32,
    pub face_start: u32,
    pub face_count: u32,
    pub portal_ref_start: u32,
    pub portal_ref_count: u32,
}

impl CellRecord {
    pub fn is_solid(&self) -> bool {
        self.flags & CELL_FLAG_SOLID != 0
    }

    pub fn is_exterior(&self) -> bool {
        self.flags & CELL_FLAG_EXTERIOR != 0
    }

    pub fn is_drawable(&self) -> bool {
        self.flags & CELL_FLAG_DRAWABLE != 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CellsSection {
    pub cells: Vec<CellRecord>,
    pub portal_refs: Vec<u32>,
}

impl CellsSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(
            HEADER_SIZE
                + self.cells.len() * CELL_RECORD_SIZE
                + self.portal_refs.len() * PORTAL_REF_STRIDE,
        );

        buf.extend_from_slice(&CELLS_VERSION.to_le_bytes());
        buf.extend_from_slice(&(self.cells.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(self.portal_refs.len() as u32).to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());

        for cell in &self.cells {
            for v in cell.bounds_min {
                buf.extend_from_slice(&v.to_le_bytes());
            }
            for v in cell.bounds_max {
                buf.extend_from_slice(&v.to_le_bytes());
            }
            buf.extend_from_slice(&cell.flags.to_le_bytes());
            let face_start = if cell.face_count == 0 {
                0
            } else {
                cell.face_start
            };
            let portal_ref_start = if cell.portal_ref_count == 0 {
                0
            } else {
                cell.portal_ref_start
            };
            buf.extend_from_slice(&face_start.to_le_bytes());
            buf.extend_from_slice(&cell.face_count.to_le_bytes());
            buf.extend_from_slice(&portal_ref_start.to_le_bytes());
            buf.extend_from_slice(&cell.portal_ref_count.to_le_bytes());
        }

        for portal_ref in &self.portal_refs {
            buf.extend_from_slice(&portal_ref.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(cells_invalid(format!(
                "Cells section too short for header: need {HEADER_SIZE} bytes, got {}",
                data.len()
            )));
        }

        let version = read_u32(data, 0);
        if version != CELLS_VERSION {
            return Err(cells_invalid(format!(
                "Cells section version {version}, expected {CELLS_VERSION}"
            )));
        }

        let cell_count = read_u32(data, 4);
        if cell_count == 0 {
            return Err(cells_invalid(
                "Cells section cell_count must be greater than zero",
            ));
        }
        let portal_ref_total = read_u32(data, 8);
        let reserved = read_u32(data, 12);
        if reserved != 0 {
            return Err(cells_invalid(format!(
                "Cells section reserved field must be 0, got {reserved}"
            )));
        }

        let cells_len = checked_bytes(cell_count, CELL_RECORD_SIZE, "cell_count")?;
        let refs_len = checked_bytes(portal_ref_total, PORTAL_REF_STRIDE, "portal_ref_total")?;
        let expected_len = HEADER_SIZE
            .checked_add(cells_len)
            .and_then(|v| v.checked_add(refs_len))
            .ok_or_else(|| cells_invalid("Cells section count multiplication overflow"))?;
        if data.len() != expected_len {
            return Err(cells_invalid(format!(
                "Cells section length mismatch: expected {expected_len} bytes for cell_count \
                 {cell_count} and portal_ref_total {portal_ref_total}, got {}",
                data.len()
            )));
        }

        let mut cells = Vec::with_capacity(cell_count as usize);
        let mut cursor = HEADER_SIZE;
        for cell_index in 0..cell_count {
            let bounds_min = [
                read_f32(data, cursor),
                read_f32(data, cursor + 4),
                read_f32(data, cursor + 8),
            ];
            let bounds_max = [
                read_f32(data, cursor + 12),
                read_f32(data, cursor + 16),
                read_f32(data, cursor + 20),
            ];
            let flags = read_u32(data, cursor + 24);
            let face_start = read_u32(data, cursor + 28);
            let face_count = read_u32(data, cursor + 32);
            let portal_ref_start = read_u32(data, cursor + 36);
            let portal_ref_count = read_u32(data, cursor + 40);
            cursor += CELL_RECORD_SIZE;

            validate_bounds(cell_index, bounds_min, bounds_max)?;
            validate_flags(cell_index, flags, face_start, face_count)?;
            validate_normalization(cell_index, face_start, face_count, "face_start")?;
            validate_normalization(
                cell_index,
                portal_ref_start,
                portal_ref_count,
                "portal_ref_start",
            )?;
            validate_range(
                cell_index,
                portal_ref_start,
                portal_ref_count,
                portal_ref_total,
            )?;

            cells.push(CellRecord {
                bounds_min,
                bounds_max,
                flags,
                face_start,
                face_count,
                portal_ref_start,
                portal_ref_count,
            });
        }

        let mut portal_refs = Vec::with_capacity(portal_ref_total as usize);
        for _ in 0..portal_ref_total {
            portal_refs.push(read_u32(data, cursor));
            cursor += PORTAL_REF_STRIDE;
        }

        Ok(Self { cells, portal_refs })
    }
}

fn validate_bounds(
    cell_index: u32,
    bounds_min: [f32; 3],
    bounds_max: [f32; 3],
) -> crate::Result<()> {
    for axis in 0..3 {
        let min = bounds_min[axis];
        let max = bounds_max[axis];
        if !min.is_finite() || !max.is_finite() {
            return Err(cells_invalid(format!(
                "Cells cell {cell_index} has non-finite bounds on axis {axis}: min {min}, max {max}"
            )));
        }
        if min > max {
            return Err(cells_invalid(format!(
                "Cells cell {cell_index} has inverted bounds on axis {axis}: min {min} > max {max}"
            )));
        }
    }
    Ok(())
}

fn validate_flags(
    cell_index: u32,
    flags: u32,
    _face_start: u32,
    face_count: u32,
) -> crate::Result<()> {
    if flags & !KNOWN_FLAGS != 0 {
        return Err(cells_invalid(format!(
            "Cells cell {cell_index} has unknown flag bits {:#x}",
            flags & !KNOWN_FLAGS
        )));
    }

    let solid = flags & CELL_FLAG_SOLID != 0;
    let exterior = flags & CELL_FLAG_EXTERIOR != 0;
    let drawable = flags & CELL_FLAG_DRAWABLE != 0;
    if solid && exterior {
        return Err(cells_invalid(format!(
            "Cells cell {cell_index} cannot be both solid and exterior"
        )));
    }

    let expected_drawable = !solid && !exterior && face_count > 0;
    if drawable != expected_drawable {
        return Err(cells_invalid(format!(
            "Cells cell {cell_index} drawable flag {drawable} does not match expected {expected_drawable}"
        )));
    }
    if (solid || exterior) && face_count != 0 {
        let kind = if solid { "solid" } else { "exterior" };
        return Err(cells_invalid(format!(
            "Cells {kind} cell {cell_index} must have face_count 0, got {face_count}"
        )));
    }

    Ok(())
}

fn validate_normalization(
    cell_index: u32,
    start: u32,
    count: u32,
    field: &'static str,
) -> crate::Result<()> {
    if count == 0 && start != 0 {
        return Err(cells_invalid(format!(
            "Cells cell {cell_index} must normalize {field} to 0 when count is 0, got {start}"
        )));
    }
    Ok(())
}

fn validate_range(
    cell_index: u32,
    portal_ref_start: u32,
    portal_ref_count: u32,
    portal_ref_total: u32,
) -> crate::Result<()> {
    let Some(end) = portal_ref_start.checked_add(portal_ref_count) else {
        return Err(cells_invalid(format!(
            "Cells cell {cell_index} portal_ref_start {portal_ref_start} + portal_ref_count {portal_ref_count} overflows u32"
        )));
    };
    if end > portal_ref_total {
        return Err(cells_invalid(format!(
            "Cells cell {cell_index} portal range [{portal_ref_start}, {end}) exceeds portal_ref_total {portal_ref_total}"
        )));
    }
    Ok(())
}

fn checked_bytes(count: u32, stride: usize, name: &'static str) -> crate::Result<usize> {
    (count as usize).checked_mul(stride).ok_or_else(|| {
        cells_invalid(format!(
            "Cells section count multiplication overflow for {name} {count} * stride {stride}"
        ))
    })
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_f32(data: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn cells_invalid(msg: impl Into<String>) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        msg.into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_section() -> CellsSection {
        CellsSection {
            cells: vec![
                CellRecord {
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [1.0, 1.0, 1.0],
                    flags: CELL_FLAG_DRAWABLE,
                    face_start: 4,
                    face_count: 2,
                    portal_ref_start: 0,
                    portal_ref_count: 2,
                },
                CellRecord {
                    bounds_min: [1.0, 0.0, 0.0],
                    bounds_max: [2.0, 1.0, 1.0],
                    flags: CELL_FLAG_SOLID,
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 2,
                    portal_ref_count: 1,
                },
                CellRecord {
                    bounds_min: [2.0, 0.0, 0.0],
                    bounds_max: [3.0, 1.0, 1.0],
                    flags: CELL_FLAG_EXTERIOR,
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 0,
                    portal_ref_count: 0,
                },
                CellRecord {
                    bounds_min: [3.0, 0.0, 0.0],
                    bounds_max: [4.0, 1.0, 1.0],
                    flags: 0,
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 0,
                    portal_ref_count: 0,
                },
            ],
            portal_refs: vec![7, 8, 9],
        }
    }

    fn assert_invalid_data(err: FormatError) {
        match err {
            FormatError::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidData),
            other => panic!("expected FormatError::Io(InvalidData), got {other:?}"),
        }
    }

    #[test]
    fn cells_round_trip_valid_section() {
        let section = valid_section();
        let bytes = section.to_bytes();
        let restored = CellsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
        assert_eq!(restored.to_bytes(), bytes);
    }

    #[test]
    fn cells_rejects_too_short_header() {
        let err = CellsSection::from_bytes(&[0u8; HEADER_SIZE - 1]).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cells_rejects_unsupported_version() {
        let mut bytes = valid_section().to_bytes();
        bytes[0..4].copy_from_slice(&2u32.to_le_bytes());
        let err = CellsSection::from_bytes(&bytes).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cells_rejects_zero_cell_count() {
        let bytes = [
            CELLS_VERSION.to_le_bytes(),
            0u32.to_le_bytes(),
            0u32.to_le_bytes(),
            0u32.to_le_bytes(),
        ]
        .concat();
        let err = CellsSection::from_bytes(&bytes).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cells_rejects_bad_reserved_field() {
        let mut bytes = valid_section().to_bytes();
        bytes[12..16].copy_from_slice(&1u32.to_le_bytes());
        let err = CellsSection::from_bytes(&bytes).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cells_rejects_truncated_records() {
        let mut bytes = valid_section().to_bytes();
        bytes.truncate(bytes.len() - 1);
        let err = CellsSection::from_bytes(&bytes).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cells_rejects_trailing_bytes() {
        let mut bytes = valid_section().to_bytes();
        bytes.push(0);
        let err = CellsSection::from_bytes(&bytes).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cells_rejects_unknown_flags() {
        let mut section = valid_section();
        section.cells[0].flags |= 1 << 7;
        let err = CellsSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cells_rejects_solid_exterior_combo() {
        let mut section = valid_section();
        section.cells[1].flags = CELL_FLAG_SOLID | CELL_FLAG_EXTERIOR;
        let err = CellsSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cells_rejects_drawable_flag_mismatch() {
        let mut section = valid_section();
        section.cells[0].flags = 0;
        let err = CellsSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cells_rejects_bad_bounds() {
        let mut section = valid_section();
        section.cells[0].bounds_min[0] = 2.0;
        section.cells[0].bounds_max[0] = 1.0;
        let err = CellsSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert_invalid_data(err);

        let mut section = valid_section();
        section.cells[0].bounds_min[0] = f32::NAN;
        let err = CellsSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cells_rejects_bad_face_start_normalization() {
        let mut bytes = valid_section().to_bytes();
        let cell_2_face_start = HEADER_SIZE + CELL_RECORD_SIZE + 28;
        bytes[cell_2_face_start..cell_2_face_start + 4].copy_from_slice(&4u32.to_le_bytes());
        let err = CellsSection::from_bytes(&bytes).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cells_rejects_bad_portal_ref_start_normalization() {
        let mut bytes = valid_section().to_bytes();
        let cell_3_portal_ref_start = HEADER_SIZE + CELL_RECORD_SIZE * 2 + 36;
        bytes[cell_3_portal_ref_start..cell_3_portal_ref_start + 4]
            .copy_from_slice(&1u32.to_le_bytes());
        let err = CellsSection::from_bytes(&bytes).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cells_rejects_bad_portal_ref_range() {
        let mut section = valid_section();
        section.cells[1].portal_ref_start = 3;
        section.cells[1].portal_ref_count = 1;
        let err = CellsSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cells_rejects_bad_declared_counts() {
        let mut bytes = valid_section().to_bytes();
        bytes[8..12].copy_from_slice(&999u32.to_le_bytes());
        let err = CellsSection::from_bytes(&bytes).unwrap_err();
        assert_invalid_data(err);
    }
}
