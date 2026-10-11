// Mask-owner eligibility: which static lights may own a face's mask channels,
// and the compact spec-light index each one carries in the owner table.
// See: context/lib/build_pipeline.md §PRL (ShadowmaskAtlas ownership)

use std::collections::HashMap;

use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::shadowmask_atlas::SHADOWMASK_OWNER_EMPTY;

use crate::light_namespaces::{AlphaLightsNs, StaticBakedLights};
use crate::map_data::{MapLight, ShadowType};

/// Most compact static spec lights an owner entry can name: indices are u16 and
/// `0xFFFF` marks an empty channel.
pub(crate) const MAX_STATIC_SPEC_LIGHTS: usize = SHADOWMASK_OWNER_EMPTY as usize;

/// A static light that may own mask channels.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EligibleLight<'a> {
    /// Position in the parsed light list: the walk's partition key.
    pub(crate) source_index: usize,
    /// Compact static spec-light index (`!is_dynamic` AlphaLights order), as the
    /// owner table stores it.
    pub(crate) spec_index: u16,
    /// In the entity-shadow selection (id 40): ranks ahead above the floor.
    pub(crate) promoted: bool,
    pub(crate) light: &'a MapLight,
}

/// The level's compact static spec-light count: every `!is_dynamic` AlphaLights
/// record, owner-eligible or not, because the runtime indexes `spec_lights`
/// over all of them.
pub(crate) fn static_spec_light_count(alpha_lights: &AlphaLightsNs<'_>) -> usize {
    alpha_lights
        .entries()
        .iter()
        .filter(|entry| !entry.light.is_dynamic)
        .count()
}

/// Every owner-eligible light, in ascending spec index: non-dynamic,
/// non-animated, not bake-only, `StaticLightMap`, of any light type. Id 40 marks
/// the promotion tier only; it never decides eligibility.
pub(crate) fn owner_eligible_lights<'a>(
    static_lights: &StaticBakedLights<'a>,
    alpha_lights: &AlphaLightsNs<'a>,
    selection: Option<&EntityShadowLightsSection>,
) -> Vec<EligibleLight<'a>> {
    // Spec index and AlphaLights index by source light. Bake-only lights have no
    // AlphaLights record, so they never appear here.
    let mut spec_by_source = HashMap::new();
    let mut spec_index = 0usize;
    for (alpha_index, entry) in alpha_lights.entries().iter().enumerate() {
        if entry.light.is_dynamic {
            continue;
        }
        spec_by_source.insert(entry.source_index, (spec_index, alpha_index as u32));
        spec_index += 1;
    }
    let promoted = |alpha_index: u32| {
        selection.is_some_and(|selection| selection.light_indices.contains(&alpha_index))
    };
    static_lights
        .entries()
        .iter()
        .filter(|entry| entry.light.shadow_type == ShadowType::StaticLightMap)
        .filter_map(|entry| {
            let &(spec_index, alpha_index) = spec_by_source.get(&entry.source_index)?;
            Some(EligibleLight {
                source_index: entry.source_index,
                // The caller checks the count against MAX_STATIC_SPEC_LIGHTS
                // before any eligible light is built.
                spec_index: u16::try_from(spec_index)
                    .expect("static spec-light count checked against the owner-index cap"),
                promoted: promoted(alpha_index),
                light: entry.light,
            })
        })
        .collect()
}
