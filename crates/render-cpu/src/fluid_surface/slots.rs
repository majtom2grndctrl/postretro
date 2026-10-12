// Renderer-side fluid draw slots and their per-volume parameter rows.
// See: context/lib/rendering_pipeline.md §7.4

use std::ops::Range;

use super::{FLUID_SLOTS_PER_WINDOW, FluidSurfaceDraw, FluidSurfaceVolume};

const NO_SLOT: u32 = u32::MAX;

/// Renderer-side per-frame slot state, kept GPU-free: volume index → draw
/// slot, the CPU mirror of the per-slot parameter buffer, the slots written
/// since the last upload, and this frame's draw order.
#[derive(Debug, Clone, Default)]
pub struct FluidSurfaceSlots {
    slot_of_volume: Vec<u32>,
    /// One `vec4` per slot: x = surface opacity, yzw reserved (zero).
    params: Vec<[f32; 4]>,
    dirty: Option<Range<u32>>,
    frame_slots: Vec<u32>,
}

impl FluidSurfaceSlots {
    /// Slots for `mesh.volumes`, in order. Every slot starts unwritten, so the
    /// first draw of each volume uploads its opacity.
    pub fn new(mesh_volumes: &[FluidSurfaceVolume]) -> Self {
        let volume_count = mesh_volumes
            .iter()
            .map(|volume| volume.volume_index as usize + 1)
            .max()
            .unwrap_or(0);
        let mut slot_of_volume = vec![NO_SLOT; volume_count];
        for (slot, volume) in mesh_volumes.iter().enumerate() {
            slot_of_volume[volume.volume_index as usize] = slot as u32;
        }
        Self {
            slot_of_volume,
            params: vec![[-1.0, 0.0, 0.0, 0.0]; mesh_volumes.len()],
            dirty: None,
            frame_slots: Vec::with_capacity(mesh_volumes.len()),
        }
    }

    pub fn slot_count(&self) -> u32 {
        self.params.len() as u32
    }

    /// Uniform windows the parameter buffer needs; at least one.
    pub fn window_count(&self) -> u32 {
        self.slot_count().div_ceil(FLUID_SLOTS_PER_WINDOW).max(1)
    }

    /// Replace this frame's draws. Unknown volumes (dry, or never installed)
    /// are skipped. Opacity changes mark their slots for upload.
    pub fn set_draws(&mut self, draws: &[FluidSurfaceDraw]) {
        self.frame_slots.clear();
        for draw in draws {
            let Some(&slot) = self.slot_of_volume.get(draw.volume_index as usize) else {
                continue;
            };
            if slot == NO_SLOT {
                continue;
            }
            let opacity = draw.surface_opacity.clamp(0.0, 1.0);
            let entry = &mut self.params[slot as usize];
            if entry[0] != opacity {
                entry[0] = opacity;
                self.dirty = Some(match self.dirty.take() {
                    Some(range) => range.start.min(slot)..range.end.max(slot + 1),
                    None => slot..slot + 1,
                });
            }
            self.frame_slots.push(slot);
        }
    }

    /// Slots written since the last call, and their parameter rows.
    pub fn take_dirty(&mut self) -> Option<(Range<u32>, &[[f32; 4]])> {
        let range = self.dirty.take()?;
        Some((
            range.clone(),
            &self.params[range.start as usize..range.end as usize],
        ))
    }

    /// This frame's slots, in draw order.
    pub fn frame_slots(&self) -> &[u32] {
        &self.frame_slots
    }

    pub fn has_draws(&self) -> bool {
        !self.frame_slots.is_empty()
    }
}

/// The uniform window and in-window element that address one slot. The
/// window's byte offset is the group's dynamic offset; the element is the
/// draw's instance index.
pub fn slot_window(slot: u32) -> (u32, u32) {
    (slot / FLUID_SLOTS_PER_WINDOW, slot % FLUID_SLOTS_PER_WINDOW)
}
