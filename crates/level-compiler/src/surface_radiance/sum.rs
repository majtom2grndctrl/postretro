// Summed patch radiance: every static light's layer added in content order.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::Vec3;

use super::layer::SparseLayer;
use super::layer_set::LayerPlan;
use super::patches::Side;
#[cfg(test)]
use super::{layer_set::resolve_layers, patches::PatchSet, reach::PatchReachIndex};
#[cfg(test)]
use crate::{
    bake_control::BakeControl, map_data::MapLight, sh_bake::RaytracingCtx,
    texel_footprint::EmbeddedEmitters,
};

/// Dense outgoing radiance per patch, front and back: what a gather hit
/// reads.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PatchRadiance {
    sides: Vec<[[f32; 3]; 2]>,
    #[cfg(test)]
    held: crate::live_bytes::Held,
}

impl PatchRadiance {
    pub(crate) fn zeroed(patch_count: usize) -> Self {
        let sides = vec![[[0.0; 3]; 2]; patch_count];
        Self {
            #[cfg(test)]
            held: crate::live_bytes::hold_vec("summed patch radiance", &sides),
            sides,
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

    /// Sum `layers` (index-parallel to `plan.layers`) over `patch_count`
    /// patches in plan order, each added once per light sharing it. The
    /// plan's order follows content alone, so the float sum, and so every
    /// byte the gather reads, is the same for any entity order.
    pub(crate) fn sum(patch_count: usize, plan: &LayerPlan, layers: &[SparseLayer]) -> Self {
        assert_eq!(
            plan.layers.len(),
            layers.len(),
            "one layer per planned layer"
        );
        let mut sum = Self::zeroed(patch_count);
        for (planned, layer) in plan.layers.iter().zip(layers) {
            for _ in 0..planned.occurrences {
                sum.add(layer);
            }
        }
        sum
    }

    /// Single-bounce patch radiance with no build cache: every light in
    /// `lights` (the static-light slice, keying `embedded`) shades its
    /// layer over its reach, and the layers sum in content order.
    #[cfg(test)]
    pub(crate) fn single_bounce(
        ctx: &RaytracingCtx<'_>,
        patches: &PatchSet,
        lights: &[&MapLight],
        embedded: &EmbeddedEmitters,
        control: &BakeControl,
    ) -> Self {
        let reach = PatchReachIndex::new(patches.patches()).counts_in_reach(lights);
        let plan = LayerPlan::new(lights, &reach, embedded);
        let layers = resolve_layers(ctx, patches, lights, &plan, None, control);
        Self::sum(patches.len(), &plan, &layers)
    }

    /// Add `scale` × `front[patch]` to each patch's front side: the later
    /// bounces joining bounce one at the map's fill.
    pub(crate) fn add_front_scaled(&mut self, front: &[[f32; 3]], scale: f32) {
        assert_eq!(front.len(), self.sides.len(), "one front value per patch");
        for (sides, value) in self.sides.iter_mut().zip(front) {
            for (s, v) in sides[Side::Front as usize].iter_mut().zip(value) {
                *s += scale * v;
            }
        }
    }

    /// Whether every side of every patch is black.
    pub(crate) fn is_zero(&self) -> bool {
        self.sides.iter().flatten().flatten().all(|&v| v == 0.0)
    }

    pub(crate) fn get(&self, patch: u32, side: Side) -> Vec3 {
        Vec3::from(self.sides[patch as usize][side as usize])
    }
}
