//! Model prediction: the model handles the commit's real sweep will upload,
//! predicted at prep from the dispatch snapshot and hand-out 4's map
//! entities, without running a script or touching a registry.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Predict models, then fall back exactly)

use std::collections::{BTreeSet, HashSet};

use postretro_level_format::map_entity::MapEntityRecord;
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;

use crate::scripting::builtins::data_archetype::{find_descriptor, projectile_presentation_assets};
use crate::scripting::builtins::player_spawn::first_player_spawn_order;
use crate::scripting::builtins::{
    ClassnameDispatch, ENTITY_SPAWNER_CLASSNAME, PLAYER_START_CLASSNAME, PROP_MESH_CLASSNAME,
    descriptor_materializes_ai_enemy, descriptor_mesh_models, entity_spawner_archetype,
    filter_out_client_host_replicated_placements, movement_descriptor_mesh_models,
    player_spawn_entity_class, prop_mesh_model, suppressed_client_host_replicated_mesh_models,
    touchable_wieldable_world_models, weapon_presentation_models,
};
use crate::scripting::map_entity::MapEntity;

/// What the prediction reads besides the level: plain data captured on the
/// main thread at dispatch, never a borrow of live script state.
#[derive(Debug, Clone, Default)]
pub(crate) struct ModelPredictionInputs {
    /// The mod's entity descriptors (`DataRegistry::entities`). Mod-global: a
    /// level's data script composes reactions, events and trees, never
    /// descriptors, so the install's archetype sweep reads this same table.
    pub(crate) descriptors: Vec<EntityTypeDescriptor>,
    /// Every classname a built-in handler claims; the archetype sweep never
    /// re-dispatches these.
    pub(crate) builtin_classnames: BTreeSet<String>,
    /// A connected client suppresses host-replicated placements and its boot
    /// pawn, as the install does.
    pub(crate) connected_client: bool,
}

impl ModelPredictionInputs {
    pub(crate) fn capture(
        descriptors: &[EntityTypeDescriptor],
        dispatch: &ClassnameDispatch,
        connected_client: bool,
    ) -> Self {
        Self {
            descriptors: descriptors.to_vec(),
            builtin_classnames: dispatch.classnames().map(str::to_string).collect(),
            connected_client,
        }
    }
}

/// First-seen, deduplicated model handles, empty ones skipped.
#[derive(Default)]
struct Predicted {
    seen: HashSet<String>,
    ordered: Vec<String>,
}

impl Predicted {
    fn add(&mut self, model: &str) {
        if !model.is_empty() && self.seen.insert(model.to_string()) {
            self.ordered.push(model.to_string());
        }
    }

    fn extend<S: AsRef<str>>(&mut self, models: impl IntoIterator<Item = S>) {
        for model in models {
            self.add(model.as_ref());
        }
    }
}

/// The model handles the install's sweep will produce for this level, as far
/// as they are known before scripts run: the union of
/// - each placement's model, through the install's own rules: a built-in
///   `prop_mesh` takes its `model` key, a built-in `entity_spawner` its
///   archetype's mesh when that archetype is an AI enemy, any other built-in
///   none, and an unclaimed classname its descriptor's mesh;
/// - on a connected client, the suppressed host-replicated placements'
///   meshes instead of their placements, and no boot pawn;
/// - otherwise the boot pawn: the first player start, in spawn order, whose
///   `entity_class` names a descriptor;
/// - every descriptor-derived set the install unions: movement, weapon,
///   projectile body and touchable-wieldable models.
///
/// The data script and `levelLoad` add no model the sweep sees: the script
/// declares no descriptors, and `levelLoad` fires after the sweep. A model
/// the real sweep holds that this lacks is a miss, which the commit loads
/// synchronously; a model this holds that the sweep lacks costs one parse.
pub(crate) fn predict_level_models(
    map_entities: &[MapEntityRecord],
    inputs: &ModelPredictionInputs,
) -> Vec<String> {
    let descriptors = inputs.descriptors.as_slice();
    let mut predicted = Predicted::default();
    let (spawn_points, placements): (Vec<MapEntity>, Vec<MapEntity>) = map_entities
        .iter()
        .cloned()
        .map(MapEntity::from)
        .filter(|entity| !crate::startup::lifecycle::is_region_companion(&entity.classname))
        .partition(|entity| entity.classname == PLAYER_START_CLASSNAME);

    let placements = if inputs.connected_client {
        predicted.extend(suppressed_client_host_replicated_mesh_models(
            &placements,
            descriptors,
        ));
        filter_out_client_host_replicated_placements(&placements, descriptors)
    } else {
        placements
    };
    for entity in &placements {
        if inputs.builtin_classnames.contains(&entity.classname) {
            if entity.classname == PROP_MESH_CLASSNAME {
                predicted.extend(prop_mesh_model(entity));
            } else if entity.classname == ENTITY_SPAWNER_CLASSNAME
                && let Some(descriptor) = entity_spawner_archetype(entity)
                    .and_then(|archetype| find_descriptor(descriptors, archetype))
                    .filter(|descriptor| descriptor_materializes_ai_enemy(descriptor))
            {
                predicted.extend(descriptor_mesh_models(descriptor));
            }
            continue;
        }
        if let Some(descriptor) = find_descriptor(descriptors, &entity.classname) {
            predicted.extend(descriptor_mesh_models(descriptor));
        }
    }

    if !inputs.connected_client
        && let Some(pawn) = first_player_spawn_order(&spawn_points)
            .into_iter()
            .find_map(|(index, _)| {
                find_descriptor(descriptors, player_spawn_entity_class(&spawn_points[index]))
            })
    {
        predicted.extend(descriptor_mesh_models(pawn));
    }

    predicted.extend(movement_descriptor_mesh_models(descriptors));
    predicted.extend(weapon_presentation_models(descriptors));
    predicted.extend(projectile_presentation_assets(descriptors).0);
    predicted.extend(touchable_wieldable_world_models(descriptors));
    predicted.ordered
}
