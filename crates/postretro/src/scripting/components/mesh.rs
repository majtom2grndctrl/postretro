// Mesh component: the model handle a skinned-model entity renders.
// See: context/lib/scripting.md

use serde::{Deserialize, Serialize};

/// Marks an entity as rendering a skinned model. `model` is the model handle
/// the `prop_mesh` classname handler reads from a map entity's `model` key — the
/// content-canonical path passed to `crate::model::gltf_loader::load_model`. It
/// doubles as the renderer cache key: the level-load model sweep uploads each
/// distinct handle once, and the per-frame draw planner groups instances by it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MeshComponent {
    pub(crate) model: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::registry::ComponentValue;

    #[test]
    fn mesh_component_serde_round_trip() {
        let value = MeshComponent {
            model: "decraniated".into(),
        };
        let json = serde_json::to_string(&value).unwrap();
        let back: MeshComponent = serde_json::from_str(&json).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn mesh_serializes_within_component_value_tagged_form() {
        let value = ComponentValue::Mesh(MeshComponent {
            model: "decraniated".into(),
        });
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(json["kind"], "mesh");
        assert_eq!(json["model"], "decraniated");
    }
}
