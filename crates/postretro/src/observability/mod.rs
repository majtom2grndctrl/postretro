// Headless observability vocabulary: runspec input, entity-dump output, and the
// deterministic JSON serialization that keeps two identical runs byte-identical.
// See: context/plans/in-progress/agentic-observability

// This module is the vocabulary substrate the headless driver (a later task in
// the same plan) consumes. Until that driver lands, nothing in-crate calls these
// entry points, so the staged public surface is intentionally unused.
#![allow(dead_code)]

mod document;
mod runspec;

#[allow(unused_imports)]
pub(crate) use document::{
    apply_dump, build_output_document, DumpSelection, EntityRecord, OutOfFrame, OutputDocument,
    PawnHealth, PlayerPawnSummary, TickEventRecord,
};
#[allow(unused_imports)]
pub(crate) use runspec::{
    parse_runspec, AimCommand, CommandEntry, DumpSpec, MovementCommand, RunSpec, RunSpecError,
};

use postretro_entities::ComponentKind;
use serde::Serialize;
use thiserror::Error;

/// Failure applying a [`DumpSpec`] against a registry. The only currently
/// possible failure is an unrecognized component-kind filter string; it is a
/// bad *value* (not a malformed document), so it surfaces here at dump time
/// rather than at runspec-parse time. The headless driver exits non-zero on it.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum DumpError {
    #[error("unknown component-kind filter {0:?}")]
    UnknownComponentKind(String),
}

/// Every component kind, in `ComponentKind` discriminant order. The array length
/// is pinned to [`ComponentKind::COUNT`], so adding a variant without extending
/// this list is a compile error (length mismatch) — the dump's kind iteration
/// can never silently skip a new component.
const ALL_KINDS: [ComponentKind; ComponentKind::COUNT] = [
    ComponentKind::Transform,
    ComponentKind::Light,
    ComponentKind::BillboardEmitter,
    ComponentKind::ParticleState,
    ComponentKind::SpriteVisual,
    ComponentKind::FogVolume,
    ComponentKind::PlayerMovement,
    ComponentKind::Weapon,
    ComponentKind::DescriptorProvenance,
    ComponentKind::Mesh,
    ComponentKind::Health,
    ComponentKind::Agent,
    ComponentKind::Brain,
    ComponentKind::KinematicMover,
];

/// Snake_case name for a component kind, matching `ComponentValue`'s serde
/// envelope `"kind"` tag exactly. `ComponentKind`'s own derive is PascalCase, so
/// this module owns the snake_case mapping rather than touching that derive.
///
/// Exhaustive `match` with no `_` arm on purpose: a new component kind is a
/// compile error here, forcing the author to give it a stable filter string
/// rather than have the dump filter silently miss it.
fn component_kind_snake(kind: ComponentKind) -> &'static str {
    match kind {
        ComponentKind::Transform => "transform",
        ComponentKind::Light => "light",
        ComponentKind::BillboardEmitter => "billboard_emitter",
        ComponentKind::ParticleState => "particle_state",
        ComponentKind::SpriteVisual => "sprite_visual",
        ComponentKind::FogVolume => "fog_volume",
        ComponentKind::PlayerMovement => "player_movement",
        ComponentKind::Weapon => "weapon",
        ComponentKind::DescriptorProvenance => "descriptor_provenance",
        ComponentKind::Mesh => "mesh",
        ComponentKind::Health => "health",
        ComponentKind::Agent => "agent",
        ComponentKind::Brain => "brain",
        ComponentKind::KinematicMover => "kinematic_mover",
    }
}

/// Resolve a snake_case component-kind filter string to its [`ComponentKind`].
/// `None` when the string names no known kind (the caller maps that to a
/// [`DumpError::UnknownComponentKind`]).
fn parse_component_kind(name: &str) -> Option<ComponentKind> {
    ALL_KINDS
        .into_iter()
        .find(|kind| component_kind_snake(*kind) == name)
}

/// Serialize any value to pretty JSON with every map (object) key in sorted
/// order, recursively. This is the determinism guarantee for the dump: several
/// `ComponentValue` payloads carry std `HashMap` fields (health zone
/// multipliers, mesh animation states) whose serde iteration order is randomized
/// per process, so a direct `serde_json::to_string` would differ byte-for-byte
/// across runs. Going through a `serde_json::Value` and sorting object keys makes
/// the output stable regardless of the hasher seed or the serde_json
/// `preserve_order` feature. Array order is data-bearing and is left untouched.
pub(crate) fn to_deterministic_json<T: Serialize>(
    value: &T,
) -> Result<String, serde_json::Error> {
    let mut json = serde_json::to_value(value)?;
    sort_json_maps(&mut json);
    serde_json::to_string_pretty(&json)
}

/// Recursively reorder every JSON object's entries into ascending key order.
fn sort_json_maps(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> =
                std::mem::take(map).into_iter().collect();
            for (_, child) in entries.iter_mut() {
                sort_json_maps(child);
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut sorted = serde_json::Map::new();
            for (key, child) in entries {
                sorted.insert(key, child);
            }
            *map = sorted;
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                sort_json_maps(item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::ComponentValue;
    use std::collections::HashMap;

    #[test]
    fn component_kind_snake_matches_component_value_serde_tag() {
        // Drift guard: the filter string must equal the `kind` tag serde emits
        // for that variant. Derive the expectation from `ComponentValue`'s own
        // serialization rather than a hand-copied literal.
        let health = ComponentValue::Health(HealthComponent {
            max: 1.0,
            current: 1.0,
            hitbox: None,
            death_handled: false,
            zone_multipliers: HashMap::new(),
            contributor_ledger: Default::default(),
        });
        let json = serde_json::to_value(&health).unwrap();
        let tag = json.get("kind").unwrap().as_str().unwrap();
        assert_eq!(tag, component_kind_snake(ComponentKind::Health));
    }

    #[test]
    fn parse_component_kind_round_trips_every_kind() {
        for kind in ALL_KINDS {
            assert_eq!(parse_component_kind(component_kind_snake(kind)), Some(kind));
        }
    }

    #[test]
    fn parse_component_kind_rejects_unknown_string() {
        assert_eq!(parse_component_kind("not_a_component"), None);
        // PascalCase (the raw `ComponentKind` derive) is deliberately NOT a
        // valid filter string — only the snake_case envelope tag is.
        assert_eq!(parse_component_kind("Health"), None);
    }

    fn health_with_multipliers(pairs: &[(&str, f32)]) -> ComponentValue {
        let mut zone_multipliers = HashMap::new();
        for (tag, factor) in pairs {
            zone_multipliers.insert((*tag).to_string(), *factor);
        }
        ComponentValue::Health(HealthComponent {
            max: 100.0,
            current: 100.0,
            hitbox: None,
            death_handled: false,
            zone_multipliers,
            contributor_ledger: Default::default(),
        })
    }

    #[test]
    fn deterministic_json_sorts_hashmap_keys_regardless_of_insertion_order() {
        // The HashMap-order determinism constraint: two logically-identical
        // payloads whose `zone_multipliers` were inserted in different orders
        // must serialize byte-for-byte identically.
        let forward = health_with_multipliers(&[("head", 2.0), ("leg", 0.5), ("torso", 1.0)]);
        let reverse = health_with_multipliers(&[("torso", 1.0), ("leg", 0.5), ("head", 2.0)]);

        let a = to_deterministic_json(&forward).unwrap();
        let b = to_deterministic_json(&reverse).unwrap();
        assert_eq!(a, b, "map key order must not leak into serialized output");
    }

    #[test]
    fn deterministic_json_is_stable_across_repeated_calls() {
        let value = health_with_multipliers(&[("a", 1.0), ("b", 2.0), ("c", 3.0)]);
        let first = to_deterministic_json(&value).unwrap();
        let second = to_deterministic_json(&value).unwrap();
        assert_eq!(first, second);
    }
}
