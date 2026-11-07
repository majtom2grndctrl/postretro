//! What the pool bounds' Build Summary blocks say in common: where a cell is,
//! and bytes and lead in an author's units.
//! See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::cell_visibility::CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;
use postretro_level_format::cells::CellsSection;

pub(super) const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct CellPlace {
    cell: u32,
    /// Engine space, metres: the shared stages hold no editor coordinates, and
    /// the inverse of the source adapter's axis swizzle and scale lives with
    /// the adapter, so this does not claim to be a map coordinate.
    center: [f32; 3],
}

impl CellPlace {
    pub(super) fn of(cell: u32, cells: &CellsSection) -> Self {
        let record = &cells.cells[cell as usize];
        Self {
            cell,
            center: std::array::from_fn(|axis| {
                (record.bounds_min[axis] + record.bounds_max[axis]) * 0.5
            }),
        }
    }

    pub(super) fn describe(&self) -> String {
        format!("in cell {}, near {}", self.cell, self.position())
    }

    pub(super) fn position(&self) -> String {
        let [x, y, z] = self.center;
        format!("({x:.1}, {y:.1}, {z:.1}) engine metres")
    }
}

pub(super) fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / MIB as f64)
}

pub(super) fn metres(lead: u32) -> String {
    format!(
        "{} m lead",
        lead as f32 / CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE as f32
    )
}
