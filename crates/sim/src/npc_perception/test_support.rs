// A synthetic id-51 level for perception and tier tests: a row of equal
// cells along x, located by x alone, with hand-built residency sets.
// See: context/lib/entity_model.md §7c

use glam::Vec3;
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_level_format::cell_residency_set::{CellResidencySetSection, ResidencyEntry};
use postretro_level_format::cell_visibility::CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;

use super::LevelCells;

const SCALE: u32 = CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;

/// Cells `width` metres long along x; cell `i` spans `[i·width, (i+1)·width)`.
pub struct Row {
    pub width: f32,
    pub residency: Option<CellResidencySetSection>,
}

impl LevelCells for Row {
    fn locate_cell(&self, position: Vec3) -> usize {
        let count = self
            .residency
            .as_ref()
            .map_or(1, CellResidencySetSection::camera_cell_count);
        ((position.x / self.width).floor().max(0.0) as usize).min(count - 1)
    }

    fn residency(&self) -> Option<&CellResidencySetSection> {
        self.residency.as_ref()
    }
}

/// A residency set over `count` cells: `sets(camera)` lists `(cell, lead in
/// metres)`, sorted here into the section's (lead, cell) order.
pub fn residency(
    count: usize,
    max_lead_metres: f32,
    sets: impl Fn(usize) -> Vec<(usize, f32)>,
) -> CellResidencySetSection {
    let mut offsets = vec![0u32];
    let mut entries = Vec::new();
    for camera in 0..count {
        let mut set: Vec<ResidencyEntry> = sets(camera)
            .into_iter()
            .map(|(cell, lead)| ResidencyEntry {
                lead: (lead * SCALE as f32).round() as u32,
                cell_id: cell as u32,
            })
            .collect();
        set.sort();
        entries.extend(set);
        offsets.push(entries.len() as u32);
    }
    CellResidencySetSection {
        max_lead: (max_lead_metres * SCALE as f32) as u32,
        offsets,
        entries,
    }
}

pub fn centre(row: &Row, cell: usize) -> Vec3 {
    Vec3::new((cell as f32 + 0.5) * row.width, 0.0, 0.0)
}

pub fn spawn_at(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
    registry.spawn(Transform {
        position,
        ..Transform::default()
    })
}

/// Three arenas of ten cells, each seeing only itself.
pub fn three_arenas() -> Row {
    Row {
        width: 2.0,
        residency: Some(residency(30, 32.0, |camera| {
            let arena = camera / 10;
            (arena * 10..arena * 10 + 10)
                .map(|cell| (cell, 0.0))
                .collect()
        })),
    }
}
