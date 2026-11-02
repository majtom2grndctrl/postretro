// Level-light selection: dynamic-light filtering, entity-shadow and
// animated-baked candidate selection, and level-light slot assignment.
// See: context/lib/rendering_pipeline.md §4

use super::*;

fn uncullable_light_influence() -> LightInfluence {
    LightInfluence {
        center: Vec3::ZERO,
        radius: f32::MAX,
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FilteredDynamicLights {
    pub lights: Vec<MapLight>,
    pub influences: Vec<LightInfluence>,
    /// Original index into the full level-light list for each filtered light.
    pub source_indices: Vec<usize>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FilteredShadowCandidates {
    pub lights: Vec<MapLight>,
    pub influences: Vec<LightInfluence>,
    /// Original index into the full level-light list for each candidate.
    pub source_indices: Vec<usize>,
    /// Selection index for selected static lights; `None` for dynamic-tier and
    /// animated-baked candidates.
    pub selection_indices: Vec<Option<usize>>,
    /// `AnimatedBakedLights` index for section-45 animated-baked candidates.
    /// This deliberately does not reuse `MapLight::animated_slot` or a compose
    /// descriptor index: section 45's affinity entries, promotion weights, and
    /// depth-cache keys all use this independent namespace.
    pub animated_baked_indices: Vec<Option<usize>>,
}

/// Runtime data for one section-45 animated-baked candidate. The roster is
/// explicitly assembled at level load because it is neither part of the
/// dynamic tier nor of `EntityShadowLights`.
#[derive(Debug, Clone)]
pub(crate) struct AnimatedBakedShadowCandidate {
    pub source_index: usize,
    pub animated_baked_index: usize,
    pub light: MapLight,
    pub influence: LightInfluence,
}

/// Shadow candidate reachability uses the runtime influence volume, not the
/// authored light origin/range. Missing influence entries are uncullable,
/// matching the loader/forward-light degradation contract.
pub(crate) fn shadow_candidate_reaches_visible_cell(
    light: &MapLight,
    influence: Option<&LightInfluence>,
    reachable_cell_aabbs: &[(Vec3, Vec3)],
) -> bool {
    if light.cell_index == ALPHA_LIGHT_LEAF_UNASSIGNED {
        return false;
    }
    let influence = influence
        .cloned()
        .unwrap_or_else(uncullable_light_influence);
    postretro_lighting::light_reaches_visible_cell(
        influence.center,
        influence.radius,
        reachable_cell_aabbs,
    )
}

// Static lights are baked — including them would double-apply their contribution.
// Missing influence data means no spatial culling for that light.
pub(crate) fn filter_dynamic_lights(
    lights: &[MapLight],
    influences: &[LightInfluence],
) -> FilteredDynamicLights {
    let mut filtered = FilteredDynamicLights::default();
    for (i, l) in lights.iter().enumerate().filter(|(_, l)| l.is_dynamic) {
        let inf = influences
            .get(i)
            .cloned()
            .unwrap_or_else(uncullable_light_influence);
        filtered.lights.push(l.clone());
        filtered.influences.push(inf);
        filtered.source_indices.push(i);
    }
    filtered
}

/// Pull the spot-shadow pool's candidate set from the **full** level light
/// list: every dynamic-tier light (`is_dynamic`). A baked light's world shadow
/// is frozen in the lightmap, so it never needs a pool slot; only dynamic-tier
/// lights qualify.
///
/// Dynamic-tier spotlights cast world shadows through the shadow depth pass
/// (which renders static world geometry), so a pooled dynamic spot shadows
/// pillars and other occluders. The per-light `casts_entity_shadows` toggle
/// (FGD `_cast_entity_shadows`) is orthogonal to slot allocation — it gates
/// whether moving-ENTITY occluders are drawn into the already-allocated slot
/// (`entity_occluder_eligible`), not whether the slot exists.
///
/// Ranking runs downstream in `assign_shadow_pool_slots_with_promoted_baked`
/// (renderer_light_slots.rs): it scores this candidate slice and competes the
/// dynamic and promoted-baked lights for the pool's slots.
#[cfg(test)]
pub(crate) fn filter_entity_shadow_candidates(
    lights: &[MapLight],
    influences: &[LightInfluence],
) -> FilteredShadowCandidates {
    filter_entity_shadow_candidates_with_selection(lights, influences, &[], &[])
}

pub(crate) fn filter_entity_shadow_candidates_with_selection(
    lights: &[MapLight],
    influences: &[LightInfluence],
    entity_shadow_lights: &[u32],
    animated_baked_lights: &[AnimatedBakedShadowCandidate],
) -> FilteredShadowCandidates {
    let mut filtered = FilteredShadowCandidates::default();
    for (i, l) in lights.iter().enumerate().filter(|(_, l)| l.is_dynamic) {
        let inf = influences
            .get(i)
            .cloned()
            .unwrap_or_else(uncullable_light_influence);
        filtered.lights.push(l.clone());
        filtered.influences.push(inf);
        filtered.source_indices.push(i);
        filtered.selection_indices.push(None);
        filtered.animated_baked_indices.push(None);
    }
    // Compacted: a skipped selected-static entry is simply not a candidate. The
    // stamped `selection_index` is the RAW position in `entity_shadow_lights`,
    // so it stays aligned with the raw-length parallel vectors below.
    for (selection_index, &source_index) in entity_shadow_lights.iter().enumerate() {
        let source_index = source_index as usize;
        let Some((light, inf)) = resolve_selected_static_light(lights, influences, source_index)
        else {
            continue;
        };
        filtered.lights.push(light);
        filtered.influences.push(inf);
        filtered.source_indices.push(source_index);
        filtered.selection_indices.push(Some(selection_index));
        filtered.animated_baked_indices.push(None);
    }
    for animated in animated_baked_lights {
        filtered.lights.push(animated.light.clone());
        filtered.influences.push(animated.influence.clone());
        filtered.source_indices.push(animated.source_index);
        filtered.selection_indices.push(None);
        filtered
            .animated_baked_indices
            .push(Some(animated.animated_baked_index));
    }
    filtered
}

/// Assemble the section-45 candidate roster in `AnimatedBakedLights` order.
///
/// `MapLight::animated_slot` establishes only that a loaded map light belongs
/// to the baked-animation roster. The index retained on each output row is the
/// roster position, never that map-light slot or the section's descriptor-table
/// value. This keeps promotion keyed to the same namespace as section 45's
/// `affinity_lights` entries even if descriptor routing changes later.
pub(crate) fn animated_baked_shadow_candidates_with_direct_delta(
    lights: &[MapLight],
    influences: &[LightInfluence],
    animation_descriptor_indices: &[u32],
    affinity_lights: &[u32],
) -> Vec<AnimatedBakedShadowCandidate> {
    animation_descriptor_indices
        .iter()
        .enumerate()
        .filter_map(|(animated_baked_index, &descriptor_index)| {
            let animated_baked_index_u32 = u32::try_from(animated_baked_index).ok()?;
            // A section-45 descriptor row alone does not prove a light owns a
            // delta block. `affinity_lights` is the sparse direct-delta
            // membership set, so only rows appearing there are eligible for
            // runtime promotion.
            if !affinity_lights.contains(&animated_baked_index_u32) {
                return None;
            }
            // `animation_descriptor_indices` is the explicit section-45
            // AnimatedBakedLights roster. `animated_slot` only joins its
            // descriptor identity to a runtime MapLight; it must not become
            // the candidate's index. Looking up each roster row also leaves a
            // missing/bake-only MapLight as a hole instead of shifting every
            // later AnimatedBakedLights index down by one.
            let (source_index, light) = lights.iter().enumerate().find(|(_, light)| {
                !light.is_dynamic && light.animated_slot == Some(descriptor_index)
            })?;
            Some(AnimatedBakedShadowCandidate {
                source_index,
                animated_baked_index,
                light: light.clone(),
                influence: influences
                    .get(source_index)
                    .cloned()
                    .unwrap_or_else(uncullable_light_influence),
            })
        })
        .collect()
}

pub(crate) fn shadow_candidate_is_animated_baked(
    animated_baked_indices: &[Option<usize>],
    candidate_index: usize,
) -> bool {
    animated_baked_indices
        .get(candidate_index)
        .and_then(|index| *index)
        .is_some()
}

pub(crate) fn shadow_candidate_is_promoted_baked(
    selection_indices: &[Option<usize>],
    animated_baked_indices: &[Option<usize>],
    candidate_index: usize,
) -> bool {
    shadow_candidate_is_promoted_static(selection_indices, candidate_index)
        || shadow_candidate_is_animated_baked(animated_baked_indices, candidate_index)
}

/// Build the selected-static light/influence/source-index vectors index-parallel
/// to the selection index (0..N over `entity_shadow_lights`), NOT compacted: the
/// promotion driver, the per-selected-light weight buffer, and the baked delta
/// `affinity_lights` all key on the raw selection index, so every position must
/// occupy its slot. A skipped entry (dynamic-tier or a corrupt out-of-range
/// index) carries a sentinel that no candidate references — the driver never
/// promotes it, so the slot is never read; it exists only to keep the lookup
/// `[selection_index]` a direct aligned index.
pub(crate) fn filter_selected_static_entity_shadow_lights(
    lights: &[MapLight],
    influences: &[LightInfluence],
    entity_shadow_lights: &[u32],
) -> FilteredDynamicLights {
    let mut filtered = FilteredDynamicLights::default();
    for &source_index in entity_shadow_lights {
        let source_index = source_index as usize;
        match resolve_selected_static_light(lights, influences, source_index) {
            Some((light, inf)) => {
                filtered.lights.push(light);
                filtered.influences.push(inf);
                filtered.source_indices.push(source_index);
            }
            None => {
                filtered.lights.push(sentinel_selected_static_light());
                filtered.influences.push(uncullable_light_influence());
                filtered.source_indices.push(UNPROMOTABLE_SOURCE_INDEX);
            }
        }
    }
    filtered
}

/// Resolve one selected-static entry by its source index into the full level
/// light array. `Some(light, influence)` when the index is in range AND the
/// light is baked-tier (`!is_dynamic`); `None` when the entry can never promote
/// (out-of-range index, or a dynamic-tier light). Missing influence entries
/// fall back to uncullable, matching the loader/forward-light degradation
/// contract. Shared by the candidate filter (skips `None`) and the parallel
/// selected-static filter (fills `None` slots with a sentinel).
fn resolve_selected_static_light(
    lights: &[MapLight],
    influences: &[LightInfluence],
    source_index: usize,
) -> Option<(MapLight, LightInfluence)> {
    let light = lights.get(source_index)?;
    if light.is_dynamic {
        return None;
    }
    let influence = influences
        .get(source_index)
        .cloned()
        .unwrap_or_else(uncullable_light_influence);
    Some((light.clone(), influence))
}

/// Source-index marker for a selected-static slot that resolved to nothing.
/// Never referenced by a candidate, so never read as a promoted light index.
const UNPROMOTABLE_SOURCE_INDEX: usize = usize::MAX;

/// Placeholder light for a selected-static slot that resolved to nothing (a
/// dynamic-tier or out-of-range index in the `EntityShadowLights` section).
/// Keeps `filter_selected_static_entity_shadow_lights` index-parallel to the
/// selection index; the driver never promotes it, so it is never read.
fn sentinel_selected_static_light() -> MapLight {
    MapLight {
        origin: [0.0; 3],
        light_type: postretro_level_loader::LightType::Point,
        intensity: 0.0,
        color: [0.0; 3],
        falloff_model: postretro_level_loader::FalloffModel::Linear,
        falloff_range: 0.0,
        cone_angle_inner: 0.0,
        cone_angle_outer: 0.0,
        cone_direction: [0.0, 0.0, 1.0],
        is_dynamic: false,
        casts_entity_shadows: false,
        animated_slot: None,
        tags: Vec::new(),
        cell_index: ALPHA_LIGHT_LEAF_UNASSIGNED,
        shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
    }
}

/// Build a reverse lookup from full-level-light source index to `level_lights`
/// position, so per-candidate brightness reads are O(1) instead of a linear
/// scan of `level_light_source_indices` (fixed within a frame). Sized to the
/// largest source index present; absent indices read back `None` (candidate not
/// represented in `level_lights`).
pub(crate) fn build_level_light_index_lookup(
    level_light_source_indices: &[usize],
) -> Vec<Option<usize>> {
    let len = level_light_source_indices
        .iter()
        .copied()
        .max()
        .map_or(0, |m| m + 1);
    let mut lookup = vec![None; len];
    for (level_idx, &source_index) in level_light_source_indices.iter().enumerate() {
        if let Some(slot) = lookup.get_mut(source_index) {
            *slot = Some(level_idx);
        }
    }
    lookup
}

/// Look up a shadow candidate's per-frame effective brightness by its source
/// level-light index, via the reverse lookup from `build_level_light_index_lookup`.
/// Returns `None` when the candidate is not represented in `level_lights`. The
/// per-frame hot paths build the lookup once and call this directly, keeping the
/// per-candidate cost O(1).
pub(crate) fn level_brightness_for_candidate_indexed(
    level_light_index_lookup: &[Option<usize>],
    candidate_source_index: usize,
    effective_brightness: &[f32],
) -> Option<f32> {
    level_light_index_lookup
        .get(candidate_source_index)
        .copied()
        .flatten()
        .and_then(|level_idx| effective_brightness.get(level_idx).copied())
}

/// Translate a slot assignment from candidate-index space into
/// `level_lights`-index space. Returns a Vec the size of `level_lights`,
/// each entry either a slot or `NO_SHADOW_SLOT`. Used to pack the GPU
/// lights buffer (`pack_lights_with_slots_into`), which is keyed on the
/// filtered `level_lights` order. Production fills a reused buffer through
/// [`fill_level_light_slots`]; this allocating form serves the tests.
#[cfg(test)]
pub(crate) fn slot_assignment_for_level_lights(
    level_light_source_indices: &[usize],
    candidate_source_indices: &[usize],
    candidate_slot_assignment: &[u32],
) -> Vec<u32> {
    let mut out = Vec::new();
    fill_level_light_slots(
        level_light_source_indices,
        candidate_source_indices,
        candidate_slot_assignment,
        &[],
        &mut out,
    );
    out
}

/// Translate a candidate-space slot assignment into `level_lights` order in
/// a reused buffer, with every candidate `skipped` marks left without a slot.
pub(crate) fn fill_level_light_slots(
    level_light_source_indices: &[usize],
    candidate_source_indices: &[usize],
    candidate_slot_assignment: &[u32],
    skipped: &[bool],
    out: &mut Vec<u32>,
) {
    use crate::lighting::spot_shadow::NO_SHADOW_SLOT;
    out.clear();
    out.resize(level_light_source_indices.len(), NO_SHADOW_SLOT);
    for (cand_idx, &slot) in candidate_slot_assignment.iter().enumerate() {
        if slot == NO_SHADOW_SLOT || skipped.get(cand_idx).copied().unwrap_or(false) {
            continue;
        }
        let Some(&candidate_source_index) = candidate_source_indices.get(cand_idx) else {
            continue;
        };
        if let Some((level_idx, _)) = level_light_source_indices
            .iter()
            .enumerate()
            .find(|(_, source_index)| **source_index == candidate_source_index)
        {
            out[level_idx] = slot;
        }
    }
}

pub(crate) fn shadow_candidate_is_promoted_static(
    selection_indices: &[Option<usize>],
    candidate_index: usize,
) -> bool {
    selection_indices
        .get(candidate_index)
        .and_then(|selection| *selection)
        .is_some()
}
