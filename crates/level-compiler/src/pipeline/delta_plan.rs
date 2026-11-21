//! The delta SH bakes' CSR plan, made before any SH ray: each bake's
//! affinity cells and lights under its production membership, selection and
//! reach policy, for the SH-delta working-set gate and the bakes themselves.
//! See: context/lib/build_pipeline.md §Compiler pipeline (working-set gate)

use super::*;

/// Cheap plan-phase CSR data. Its dense f16 payload remains intentionally absent.
pub(super) struct DeltaCsrPlan {
    pub(super) affinity_offsets: Vec<u32>,
    pub(super) affinity_lights: Vec<u32>,
}

impl DeltaCsrPlan {
    /// One baked section's per-cell vectors and light list, by length: its
    /// CSR offsets and light indices, and a level byte and a valid-probe
    /// mask per cell.
    pub(super) fn metadata_bytes(&self) -> u64 {
        let cells = self.affinity_offsets.len().saturating_sub(1);
        (self.affinity_offsets.len() * size_of::<u32>()
            + self.affinity_lights.len() * size_of::<u32>()
            + cells * (size_of::<u8>() + size_of::<u64>())) as u64
    }
}

pub(super) struct PlannedDeltaBakes {
    pub(super) entity_shadow_lights:
        Option<postretro_level_format::entity_shadow_lights::EntityShadowLightsSection>,
    pub(super) entity_shadow_lights_elapsed: Duration,
    pub(super) indirect: DeltaCsrPlan,
    pub(super) direct: DeltaCsrPlan,
    pub(super) direct_static_indices: Vec<u64>,
    pub(super) animated_direct: DeltaCsrPlan,
}

/// Inputs shared by the pre-base-SH delta CSR planning pass.
pub(super) struct DeltaBakePlanInputs<'a, 'lights> {
    pub(super) bvh: &'a bvh::bvh::Bvh<f32, 3>,
    pub(super) bvh_primitives: &'a [bvh_build::BvhPrimitive],
    pub(super) geometry: &'a geometry::GeometryResult,
    pub(super) tree: &'a partition::BspTree,
    pub(super) exterior_leaves: &'a std::collections::HashSet<usize>,
    pub(super) portals: &'a [portals::Portal],
    pub(super) static_baked_lights: &'a light_namespaces::StaticBakedLights<'lights>,
    pub(super) animated_baked_lights: &'a light_namespaces::AnimatedBakedLights<'lights>,
    pub(super) alpha_lights: &'a light_namespaces::AlphaLightsNs<'lights>,
    pub(super) entity_shadow_params: map_data::EntityShadowParams,
    pub(super) probe_spacing: f32,
    pub(super) mutable_descriptors: &'a crate::delta_drop_policy::ScriptMutableDescriptorSlots,
}

impl DeltaCsrPlan {
    pub(super) fn empty() -> Self {
        Self {
            affinity_offsets: vec![0],
            affinity_lights: Vec::new(),
        }
    }

    pub(super) fn from_csr(affinity_offsets: Vec<u32>, affinity_lights: Vec<u32>) -> Self {
        debug_assert_eq!(
            affinity_offsets.last().copied().unwrap_or_default() as usize,
            affinity_lights.len(),
            "plan-phase CSR offsets must end at the flat light-list length"
        );
        Self {
            affinity_offsets,
            affinity_lights,
        }
    }

    pub(super) fn affinity_lights(&self) -> &[u32] {
        debug_assert_eq!(
            self.affinity_offsets.last().copied().unwrap_or_default() as usize,
            self.affinity_lights.len(),
            "plan-phase CSR offsets must remain aligned with the flat light list"
        );
        &self.affinity_lights
    }
}

/// Build the production delta CSR plans without materializing base or delta SH
/// payloads. The working-set gate calls this before the base-SH ray bake so a
/// zero-budget diagnostic remains a cheap, current-policy projection.
pub(super) fn plan_delta_bakes(inputs: DeltaBakePlanInputs<'_, '_>) -> PlannedDeltaBakes {
    let entity_shadow_lights_started = Instant::now();
    let entity_shadow_lights = if inputs.static_baked_lights.is_empty() {
        None
    } else {
        let selection_inputs = entity_shadow_select::EntityShadowSelectionInputs {
            bvh: inputs.bvh,
            primitives: inputs.bvh_primitives,
            geometry: inputs.geometry,
            static_lights: inputs.static_baked_lights,
            alpha_lights: inputs.alpha_lights,
            params: inputs.entity_shadow_params,
        };
        let section = entity_shadow_select::select_entity_shadow_lights(&selection_inputs);
        (!section.light_indices.is_empty()).then_some(section)
    };
    let entity_shadow_lights_elapsed = entity_shadow_lights_started.elapsed();

    let geometry_vertices: Vec<[f32; 3]> = inputs
        .geometry
        .geometry
        .vertices
        .iter()
        .map(|vertex| vertex.position)
        .collect();
    let indirect = if inputs.animated_baked_lights.is_empty() || geometry_vertices.is_empty() {
        DeltaCsrPlan::empty()
    } else {
        let decomposition = decompose_affinity(&AffinityInputs {
            geometry_vertices: &geometry_vertices,
            tree: inputs.tree,
            exterior_leaves: inputs.exterior_leaves,
            portals: inputs.portals,
            animated_lights: inputs.animated_baked_lights,
            probe_spacing: inputs.probe_spacing,
        });
        let (affinity_offsets, affinity_lights) = build_csr(
            &decomposition.per_light_cells,
            decomposition.affinity_cell_count(),
        );
        DeltaCsrPlan::from_csr(affinity_offsets, affinity_lights)
    };

    let animated_direct = if inputs.animated_baked_lights.is_empty() || geometry_vertices.is_empty()
    {
        DeltaCsrPlan::empty()
    } else {
        let animated_lights: Vec<_> = inputs
            .animated_baked_lights
            .entries()
            .iter()
            .map(|entry| entry.light)
            .collect();
        let reach = AffinityReachInputs {
            geometry_vertices: &geometry_vertices,
            tree: inputs.tree,
            exterior_leaves: inputs.exterior_leaves,
            portals: inputs.portals,
            probe_spacing: inputs.probe_spacing,
        };
        let policies = animated_direct_sh_bake::animated_direct_reach_policies(
            animated_lights.len(),
            inputs.mutable_descriptors,
        );
        let decomposition = crate::affinity_grid::decompose_affinity_for_lights_with_policies(
            &reach,
            &animated_lights,
            &policies,
        );
        let (affinity_offsets, affinity_lights) = build_csr(
            &decomposition.per_light_cells,
            decomposition.affinity_cell_count(),
        );
        DeltaCsrPlan::from_csr(affinity_offsets, affinity_lights)
    };

    let (direct, direct_static_indices) = if geometry_vertices.is_empty() {
        (DeltaCsrPlan::empty(), Vec::new())
    } else if let Some(selection) = entity_shadow_lights.as_ref() {
        // Id 41's CSR uses selection-slot indices. Preserve the parallel static
        // indices only for the pre-bake diagnostic histogram.
        let source_by_alpha = inputs
            .alpha_lights
            .entries()
            .iter()
            .enumerate()
            .map(|(alpha_index, entry)| (alpha_index as u32, entry.source_index))
            .collect::<HashMap<_, _>>();
        let direct_by_source = inputs
            .static_baked_lights
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.light.shadow_type == map_data::ShadowType::StaticLightMap)
            .map(|(static_index, entry)| (entry.source_index, (entry.light, static_index as u64)))
            .collect::<HashMap<_, _>>();
        let selected: Vec<_> = selection
            .light_indices
            .iter()
            .filter_map(|alpha_index| {
                source_by_alpha
                    .get(alpha_index)
                    .and_then(|source_index| direct_by_source.get(source_index))
                    .copied()
            })
            .collect();
        let selected_lights: Vec<_> = selected.iter().map(|(light, _)| *light).collect();
        let selected_static_indices = selected
            .iter()
            .map(|(_, static_index)| *static_index)
            .collect();
        let reach = AffinityReachInputs {
            geometry_vertices: &geometry_vertices,
            tree: inputs.tree,
            exterior_leaves: inputs.exterior_leaves,
            portals: inputs.portals,
            probe_spacing: inputs.probe_spacing,
        };
        let decomposition = decompose_affinity_for_lights(
            &reach,
            &selected_lights,
            AffinityReachPolicy::SELECTED_DIRECT,
        );
        let (affinity_offsets, affinity_lights) = build_csr(
            &decomposition.per_light_cells,
            decomposition.affinity_cell_count(),
        );
        (
            DeltaCsrPlan::from_csr(affinity_offsets, affinity_lights),
            selected_static_indices,
        )
    } else {
        (DeltaCsrPlan::empty(), Vec::new())
    };

    PlannedDeltaBakes {
        entity_shadow_lights,
        entity_shadow_lights_elapsed,
        indirect,
        direct,
        direct_static_indices,
        animated_direct,
    }
}

pub(super) fn ensure_bake_csr_matches_plan(
    label: &'static str,
    plan: &DeltaCsrPlan,
    bake_offsets: &[u32],
    bake_lights: &[u32],
) -> anyhow::Result<()> {
    if plan.affinity_offsets != bake_offsets || plan.affinity_lights != bake_lights {
        anyhow::bail!(
            "{label} plan/bake CSR divergence: plan has {} offsets and {} entries; bake has {} offsets and {} entries",
            plan.affinity_offsets.len(),
            plan.affinity_lights.len(),
            bake_offsets.len(),
            bake_lights.len(),
        );
    }
    Ok(())
}
