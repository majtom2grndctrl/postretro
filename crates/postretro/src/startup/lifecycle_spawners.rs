//! Install-time resolution of map spawners against the level's descriptors.
//! See: context/lib/boot_sequence.md §3

use super::*;

/// Test-observable outcome of validating map-authored spawners after classname
/// dispatch. Bad authoring never aborts a level install; it leaves the spawner
/// unresolved, so the fixed-tick executor will skip it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpawnerInstallDiagnostics {
    pub(crate) missing_archetype: usize,
    pub(crate) non_ai_archetype: usize,
}

impl SpawnerInstallDiagnostics {
    pub(crate) fn invalid_total(self) -> usize {
        self.missing_archetype + self.non_ai_archetype
    }
}

/// Resolve every map spawner against this level's descriptor registry and
/// replace the session's fixed-tick spawn cache. The cache contains only AI
/// descriptors, keyed by canonical name, and carries the nav bake that later
/// runtime spawns must use for their agent capsule.
pub(crate) fn resolve_spawners_for_level(
    registry: &mut postretro_entities::EntityRegistry,
    descriptors: &[postretro_scripting_core::data_descriptors::EntityTypeDescriptor],
    agent_params: Option<postretro_foundation::NavAgentParams>,
    spawn_context: &crate::spawner::SpawnContext,
) -> SpawnerInstallDiagnostics {
    use postretro_entities::components::spawner::SpawnerComponent;

    let mut diagnostics = SpawnerInstallDiagnostics::default();
    let mut resolved_descriptors = std::collections::HashMap::new();
    let spawners: Vec<_> = registry
        .iter_with_kind(postretro_entities::ComponentKind::Spawner)
        .map(|(id, _)| id)
        .collect();

    for id in spawners {
        let mut spawner = registry
            .get_component::<SpawnerComponent>(id)
            .expect("iter_with_kind(ComponentKind::Spawner) must yield SpawnerComponent")
            .clone();
        let Some(descriptor) = crate::scripting::builtins::data_archetype::find_descriptor(
            descriptors,
            &spawner.archetype_name,
        ) else {
            diagnostics.missing_archetype += 1;
            // Empty archetype_name already warned (absent/empty key) when the
            // spawner was parsed; don't repeat it as a confusing empty-name
            // "unknown archetype" here.
            if !spawner.archetype_name.is_empty() {
                log::warn!(
                    "[Loader] entity_spawner {id}: unknown archetype `{}`; it will spawn nothing",
                    spawner.archetype_name
                );
            }
            spawner.resolved = false;
            let _ = registry.set_component(id, spawner);
            continue;
        };
        if !descriptor_materializes_ai_enemy(descriptor) {
            diagnostics.non_ai_archetype += 1;
            log::warn!(
                "[Loader] entity_spawner {id}: archetype `{}` is not an AI enemy; it will spawn nothing",
                spawner.archetype_name
            );
            spawner.resolved = false;
            let _ = registry.set_component(id, spawner);
            continue;
        }

        let canonical_name = descriptor
            .canonical_name
            .as_ref()
            .expect("find_descriptor matches only descriptors with canonical names")
            .clone();
        resolved_descriptors.insert(canonical_name, descriptor.clone());
        spawner.resolved = true;
        let _ = registry.set_component(id, spawner);
    }

    spawn_context.replace_level_data(resolved_descriptors, agent_params);
    diagnostics
}

/// Collect renderable mesh handles referenced by successfully resolved map
/// spawners. Spawner-only archetypes do not materialize a `MeshComponent` until
/// a reaction fires, so the normal registry sweep cannot discover their models.
/// Keep this tied to the resolved component rather than every descriptor: only
/// archetypes this map can actually spawn consume the level's upload budget.
pub(crate) fn resolved_spawner_mesh_models(
    registry: &postretro_entities::EntityRegistry,
    descriptors: &[postretro_scripting_core::data_descriptors::EntityTypeDescriptor],
) -> Vec<String> {
    use postretro_entities::components::spawner::SpawnerComponent;

    let mut seen = std::collections::HashSet::new();
    let mut models = Vec::new();
    let mut add_model = |model: &str| {
        if !model.is_empty() && seen.insert(model.to_string()) {
            models.push(model.to_string());
        }
    };
    for (id, _) in registry.iter_with_kind(postretro_entities::ComponentKind::Spawner) {
        let Ok(spawner) = registry.get_component::<SpawnerComponent>(id) else {
            continue;
        };
        if !spawner.resolved {
            continue;
        }
        let Some(descriptor) = crate::scripting::builtins::data_archetype::find_descriptor(
            descriptors,
            &spawner.archetype_name,
        ) else {
            continue;
        };
        let Some(mesh) = descriptor.mesh.as_ref() else {
            continue;
        };
        add_model(&mesh.model);
        let mut attachment_models: Vec<&str> =
            mesh.attachments.values().map(String::as_str).collect();
        attachment_models.sort_unstable();
        for attachment_model in attachment_models {
            add_model(attachment_model);
        }
    }
    models
}
