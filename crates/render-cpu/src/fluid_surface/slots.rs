// Renderer-side fluid draw slots and their per-volume parameter rows.
// See: context/lib/rendering_pipeline.md §7.3a

use std::ops::Range;

use bytemuck::{Pod, Zeroable};

use super::fog::FLUID_FOG_EXTINCTION_PER_DISTANCE;
use super::motion::FluidWaveParams;
use super::{FLUID_SLOTS_PER_WINDOW, FluidSurfaceDraw, FluidSurfaceVolume};

const NO_SLOT: u32 = u32::MAX;

/// Scale from a `fogColor` (or `viewTint` rgb) to the murk's albedo, which the
/// surface's own lighting then lights. `viewTint` mixes toward its rgb after
/// tonemap, so a plain linear scale maps the same colour to a pre-tonemap
/// albedo; 0.25 takes dev water's `[0.16, 0.22, 0.19]` to the approved
/// prototype's murk, about `(0.04, 0.055, 0.048)`.
pub const FLUID_FOG_COLOR_SCALE: f32 = 0.25;

/// One fluid's look as the local manifest resolves it, GPU-free. The
/// collector takes one per volume and packs it into the volume's row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidSurfaceLook {
    pub surface_opacity: f32,
    /// Metres of path that hide 95%; `None` is clear water, drawn as before.
    pub fog_distance: Option<f32>,
    /// Murk colour in `viewTint`'s rgb space, already resolved: the fluid's
    /// `fogColor`, else its `viewTint` rgb.
    pub fog_color: [f32; 3],
    /// Seconds per cycle; `None` is no wave, whatever the amplitude.
    pub wave_period: Option<f32>,
    /// Peak sideways shift, texels; 0 is no wave.
    pub wave_amplitude: f32,
    /// Crest length range, texels, `[min, max]`.
    pub wave_length: [f32; 2],
}

impl FluidSurfaceLook {
    /// A look with only a surface opacity: no fog and no wave.
    pub fn surface_only(surface_opacity: f32) -> Self {
        Self {
            surface_opacity,
            fog_distance: None,
            fog_color: [0.0; 3],
            wave_period: None,
            wave_amplitude: 0.0,
            wave_length: [0.0; 2],
        }
    }

    /// The row the shader reads for this look. Validation has already
    /// rejected bad fields; anything non-finite still packs as "off", so a
    /// NaN never reaches the shader or re-uploads every frame.
    pub fn pack(&self) -> FluidParamsRow {
        let opacity = if self.surface_opacity.is_finite() {
            self.surface_opacity.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let extinction = match self.fog_distance {
            Some(distance) if distance.is_finite() && distance > 0.0 => {
                FLUID_FOG_EXTINCTION_PER_DISTANCE / distance
            }
            _ => 0.0,
        };
        let albedo = self.fog_color.map(|channel| {
            if channel.is_finite() {
                channel.clamp(0.0, 1.0) * FLUID_FOG_COLOR_SCALE
            } else {
                0.0
            }
        });
        let [length_min, length_max] = self.wave_length;
        // Pin P3: a declared amplitude with no period packs no wave, never a
        // frozen displacement; an unusable range packs none either, as the
        // shader divides by the pair length.
        let wave = match self.wave_period {
            Some(period)
                if period.is_finite()
                    && period > 0.0
                    && self.wave_amplitude.is_finite()
                    && self.wave_amplitude > 0.0
                    && length_min.is_finite()
                    && length_max.is_finite()
                    && length_min > 0.0
                    && length_max >= length_min =>
            {
                [1.0 / period, self.wave_amplitude, length_min, length_max]
            }
            _ => [0.0; 4],
        };
        FluidParamsRow {
            surface: [opacity, extinction, 0.0, 0.0],
            fog_albedo: [albedo[0], albedo[1], albedo[2], 0.0],
            wave,
        }
    }
}

/// One slot's per-volume parameters, three `vec4<f32>` as the shader's
/// `FluidVolumeRow` reads them. `surface.zw` and `fog_albedo.w` are spare.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct FluidParamsRow {
    /// `[opacity, fog extinction (3 / fogDistance, 0 = no fog), spare, spare]`.
    pub surface: [f32; 4],
    /// `[r, g, b, spare]`: the fog colour × [`FLUID_FOG_COLOR_SCALE`].
    pub fog_albedo: [f32; 4],
    /// `[1 / period, amplitude, length min, length max]`, all zero for no wave.
    pub wave: [f32; 4],
}

impl FluidParamsRow {
    /// A slot no draw has written. Its opacity is out of range, so the first
    /// draw of any look differs and uploads.
    pub const UNWRITTEN: Self = Self {
        surface: [-1.0, 0.0, 0.0, 0.0],
        fog_albedo: [0.0; 4],
        wave: [0.0; 4],
    };

    pub fn opacity(&self) -> f32 {
        self.surface[0]
    }

    pub fn fog_extinction(&self) -> f32 {
        self.surface[1]
    }

    /// Whether a volume with this row draws anything: a visible surface, or
    /// murk on its own at zero opacity.
    pub fn draws(&self) -> bool {
        self.opacity() > 0.0 || self.fog_extinction() > 0.0
    }

    /// The wave as the motion reference reads it.
    pub fn wave(&self) -> FluidWaveParams {
        let [inv_period, amplitude, length_min, length_max] = self.wave;
        FluidWaveParams {
            inv_period,
            amplitude,
            length_min,
            length_max,
        }
    }
}

/// Renderer-side per-frame slot state, kept GPU-free: volume index → draw
/// slot, the CPU mirror of the per-slot parameter buffer, the slots written
/// since the last upload, and this frame's draw order.
#[derive(Debug, Clone, Default)]
pub struct FluidSurfaceSlots {
    slot_of_volume: Vec<u32>,
    params: Vec<FluidParamsRow>,
    dirty: Option<Range<u32>>,
    frame_slots: Vec<u32>,
}

impl FluidSurfaceSlots {
    /// Slots for `mesh.volumes`, in order. Every slot starts unwritten, so the
    /// first draw of each volume uploads its row.
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
            params: vec![FluidParamsRow::UNWRITTEN; mesh_volumes.len()],
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
    /// are skipped. A slot uploads when any float of its row changes, so a
    /// frame that changes nothing uploads nothing.
    pub fn set_draws(&mut self, draws: &[FluidSurfaceDraw]) {
        self.frame_slots.clear();
        for draw in draws {
            let Some(&slot) = self.slot_of_volume.get(draw.volume_index as usize) else {
                continue;
            };
            if slot == NO_SLOT {
                continue;
            }
            let entry = &mut self.params[slot as usize];
            if *entry != draw.params {
                *entry = draw.params;
                self.dirty = Some(match self.dirty.take() {
                    Some(range) => range.start.min(slot)..range.end.max(slot + 1),
                    None => slot..slot + 1,
                });
            }
            self.frame_slots.push(slot);
        }
    }

    /// Slots written since the last call, and their parameter rows.
    pub fn take_dirty(&mut self) -> Option<(Range<u32>, &[FluidParamsRow])> {
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
