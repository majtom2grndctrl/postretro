// The baked lights the gate analyses, across both tiers, and those it cannot.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::map_data::{MapLight, ShadowType};

pub(crate) const STATIC_SDF_REASON: &str =
    "static SDF-shadow light: its shadow resolves at runtime, so no lightmap layer carries it";
pub(crate) const ANIMATED_SDF_REASON: &str =
    "animated SDF-shadow light: its shadow resolves at runtime, so the id-25 weights omit it";

/// A baked light the gate analyses. `source_index` is its position in the
/// map's light list, which is authored entity order.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GateLight<'a> {
    pub(crate) source_index: usize,
    pub(crate) light: &'a MapLight,
}

/// A baked light the gate cannot analyse, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotAnalysedLight {
    pub(crate) source_index: usize,
    pub(crate) reason: &'static str,
}

/// Static lightmap and animated baked lights, analysed or not, each list
/// ascending by source index across both tiers. Dynamic-tier lights are in
/// neither: they belong to the side count.
#[derive(Debug, Clone, Default)]
pub(crate) struct GateLights<'a> {
    pub(crate) analysed: Vec<GateLight<'a>>,
    pub(crate) not_analysed: Vec<NotAnalysedLight>,
}

impl<'a> GateLights<'a> {
    /// SDF-shadow lights are not analysed: neither the lightmap layers nor
    /// the animated weights carry their visibility.
    pub(crate) fn resolve(
        static_lights: &StaticBakedLights<'a>,
        animated_lights: &AnimatedBakedLights<'a>,
    ) -> Self {
        let tiers = static_lights
            .entries()
            .iter()
            .map(|entry| (entry.source_index, entry.light, STATIC_SDF_REASON))
            .chain(
                animated_lights
                    .entries()
                    .iter()
                    .map(|entry| (entry.source_index, entry.light, ANIMATED_SDF_REASON)),
            );
        let mut lights = Self::default();
        for (source_index, light, sdf_reason) in tiers {
            if light.shadow_type == ShadowType::Sdf {
                lights.not_analysed.push(NotAnalysedLight {
                    source_index,
                    reason: sdf_reason,
                });
            } else {
                lights.analysed.push(GateLight {
                    source_index,
                    light,
                });
            }
        }
        lights.analysed.sort_by_key(|light| light.source_index);
        lights.not_analysed.sort_by_key(|light| light.source_index);
        lights
    }
}
