// Summed patch radiance: every static light's layer added in one fixed order.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::Vec3;

use super::layer::{SparseLayer, shade_light_layer};
use super::patches::{PatchSet, Side};
use crate::bake_control::BakeControl;
use crate::map_data::MapLight;
use crate::sh_bake::RaytracingCtx;
use crate::texel_footprint::EmbeddedEmitters;

/// Dense outgoing radiance per patch, front and back: what a gather hit
/// reads.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PatchRadiance {
    sides: Vec<[[f32; 3]; 2]>,
}

impl PatchRadiance {
    pub(crate) fn zeroed(patch_count: usize) -> Self {
        Self {
            sides: vec![[[0.0; 3]; 2]; patch_count],
        }
    }

    /// Add one layer. Callers add layers in a fixed order, so the float sum
    /// does not depend on scheduling.
    pub(crate) fn add(&mut self, layer: &SparseLayer) {
        for entry in &layer.entries {
            let sum = &mut self.sides[entry.patch as usize][entry.side as usize];
            for (s, r) in sum.iter_mut().zip(entry.radiance) {
                *s += r;
            }
        }
    }

    /// Single-bounce patch radiance: each of `lights` shades its layer and
    /// adds it, in slice order. `lights[i]`'s seed axis and embedded flag
    /// key on `i`, its position in the static-light slice.
    pub(crate) fn single_bounce(
        ctx: &RaytracingCtx<'_>,
        patches: &PatchSet,
        lights: &[&MapLight],
        embedded: &EmbeddedEmitters,
        control: &BakeControl,
    ) -> Self {
        let mut sum = Self::zeroed(patches.len());
        for (index, light) in lights.iter().enumerate() {
            let layer = shade_light_layer(
                ctx,
                patches,
                light,
                index as u64,
                embedded.is_embedded(index),
                control,
            );
            sum.add(&layer);
        }
        sum
    }

    /// Patches the sum covers: the patch set it was shaded over.
    pub(crate) fn patch_count(&self) -> usize {
        self.sides.len()
    }

    pub(crate) fn get(&self, patch: u32, side: Side) -> Vec3 {
        Vec3::from(self.sides[patch as usize][side as usize])
    }
}
