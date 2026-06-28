// Data-context descriptors: light/mesh/entity-type descriptors.
// See: context/lib/scripting.md

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
use crate::scripting::components::mesh::{AnimationState, InterruptPolicy};
use crate::scripting::data_descriptors::{
    AiDescriptor, DescriptorError, HealthDescriptor, PlayerMovementDescriptor, WeaponDescriptor,
};

/// Authored light component preset attached to an [`EntityTypeDescriptor`].
/// Mirrors the runtime [`crate::scripting::components::light::LightComponent`] shape but
/// only carries the script-authored fields (no animation, no cone, no
/// shadows). Spawn-time defaults fill the rest. `range` is mapped onto
/// [`crate::scripting::components::light::LightComponent::falloff_range`] when the
/// data-archetype spawn path materializes the component.
///
/// `is_dynamic` may be set by the author but the data-archetype spawn path
/// forces `true` regardless (baked indirect lighting is not supported for
/// descriptor-spawned lights).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct LightDescriptor {
    pub(crate) color: [f32; 3],
    pub(crate) intensity: f32,
    pub(crate) range: f32,
    pub(crate) is_dynamic: bool,
}

impl LightDescriptor {
    /// Validate bounds that serde cannot enforce: `intensity` and `range`
    /// must be non-negative finite values.
    pub(crate) fn validate(self) -> Result<Self, DescriptorError> {
        if !self.intensity.is_finite() || self.intensity < 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.light.intensity` must be >= 0.0, got {}",
                    self.intensity
                ),
            });
        }
        if !self.range.is_finite() || self.range < 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.light.range` must be >= 0.0, got {}",
                    self.range
                ),
            });
        }
        Ok(self)
    }
}

/// Authored mesh component preset attached to an [`EntityTypeDescriptor`].
/// Carries the model handle a skinned-model entity renders plus an optional
/// declared animation-state surface. The data-archetype spawn path materializes
/// this into a [`crate::scripting::components::mesh::MeshComponent`]: a descriptor with no
/// `animations` block yields a stateless component, otherwise the declared state
/// map is copied in via `MeshAnimation::new` with current = `default_state` and
/// a pending entry stamp.
///
/// Validation (at parse time): `model` non-empty; each state's `clip` non-empty;
/// `crossfade_ms` finite ≥ 0; `interrupt` (when present on the wire) one of
/// `"smooth"`/`"snap"`. When `animations` is present it must be non-empty and
/// `default_state` must be present and name a declared state. A `defaultState`
/// without an `animations` block is also rejected. Clip resolution against the
/// model's clip metadata is resolved at level load by `resolve_mesh_entity_clips`;
/// `AnimationState::clip_index` stays `None` at parse.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MeshDescriptor {
    pub(crate) model: String,
    /// Declared state map: state name → clip + loop + crossfade + interrupt.
    /// Empty when the descriptor declared no `animations` block (stateless).
    pub(crate) animations: HashMap<String, AnimationState>,
    /// The default/spawn state name. `Some` exactly when `animations` is
    /// non-empty; parse validation rejects animations-without-default and a
    /// default that does not name a declared state.
    pub(crate) default_state: Option<String>,
}

/// One parsed-but-unvalidated animation-state entry, as gathered from the wire
/// by either FFI path. `interrupt` is the raw string when present (`None` =
/// absent ⇒ defaults to `"smooth"`); validation maps it to [`InterruptPolicy`].
pub(crate) struct RawAnimationState {
    pub(crate) name: String,
    pub(crate) clip: String,
    pub(crate) looping: bool,
    pub(crate) crossfade_ms: f32,
    pub(crate) interrupt: Option<String>,
}

impl MeshDescriptor {
    /// Build and validate a [`MeshDescriptor`] from the raw fields gathered by
    /// the JS / Luau parsers. Shared so both FFI paths enforce identical rules:
    /// non-empty `model`/`clip`, finite ≥ 0 `crossfadeMs`, `interrupt` in
    /// {smooth, snap}, and — when any state is declared — a present
    /// `defaultState` that names a declared state. An empty-but-present
    /// `animations` block is rejected; a wholly absent one yields a stateless
    /// descriptor (`animations` empty, `default_state` None).
    pub(crate) fn build(
        model: String,
        states: Vec<RawAnimationState>,
        default_state: Option<String>,
        animations_present: bool,
    ) -> Result<Self, DescriptorError> {
        if model.is_empty() {
            return Err(DescriptorError::InvalidShape {
                reason: "`components.mesh.model` must be a non-empty string".to_string(),
            });
        }

        // A present-but-empty `animations` object is rejected: the author meant
        // to declare states but declared none. (A wholly absent block ⇒
        // stateless, handled by `animations_present == false`.)
        if animations_present && states.is_empty() {
            return Err(DescriptorError::InvalidShape {
                reason:
                    "`components.mesh.animations` is present but empty; omit it for a stateless mesh"
                        .to_string(),
            });
        }

        let mut animations = HashMap::with_capacity(states.len());
        for raw in states {
            if raw.clip.is_empty() {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.mesh.animations.{}.clip` must be a non-empty string",
                        raw.name
                    ),
                });
            }
            if !raw.crossfade_ms.is_finite() || raw.crossfade_ms < 0.0 {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.mesh.animations.{}.crossfadeMs` must be a finite value >= 0.0, got {}",
                        raw.name, raw.crossfade_ms
                    ),
                });
            }
            let interrupt = match raw.interrupt.as_deref() {
                None | Some("smooth") => InterruptPolicy::Smooth,
                Some("snap") => InterruptPolicy::Snap,
                Some(other) => {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.mesh.animations.{}.interrupt` must be \"smooth\" or \"snap\", got \"{}\"",
                            raw.name, other
                        ),
                    });
                }
            };
            animations.insert(
                raw.name,
                AnimationState {
                    clip: raw.clip,
                    looping: raw.looping,
                    crossfade_ms: raw.crossfade_ms,
                    interrupt,
                    // Resolved against the model's clip metadata at level load
                    // by `resolve_mesh_entity_clips`; unresolved here.
                    clip_index: None,
                },
            );
        }

        // `defaultState` is required exactly when states are declared, and must
        // name one of them. With no states declared it must be absent — a
        // `defaultState` without an `animations` block is rejected.
        let default_state = if animations.is_empty() {
            if default_state.is_some() {
                return Err(DescriptorError::InvalidShape {
                    reason: "`components.mesh.defaultState` requires an `animations` block; no animations were declared".to_string(),
                });
            }
            None
        } else {
            let default = default_state.ok_or(DescriptorError::MissingField {
                field: "defaultState",
            })?;
            if !animations.contains_key(&default) {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.mesh.defaultState` (\"{default}\") does not name a declared animation state"
                    ),
                });
            }
            Some(default)
        };

        Ok(MeshDescriptor {
            model,
            animations,
            default_state,
        })
    }
}

/// Author-side description of an entity type. Carried on `ModManifest.entities`
/// and drained into `DataRegistry` after the mod manifest commits.
///
/// `canonical_name` is the FGD/map classname this descriptor is directly
/// placeable as. When `None`, the descriptor has no map-placement form — it
/// is only reachable via indirect routing (e.g. an `entity_class` KVP on a
/// `player_spawn` marker). Absence is structural: descriptors with no
/// `canonical_name` cannot be matched against a `MapEntity.classname` by the
/// data-archetype dispatch.
///
/// `default_weapon` is the canonical name of the wieldable archetype spawned
/// alongside this entity when routed through `player_spawn`. The descriptor
/// keeps the string; runtime state stores the resolved `EntityId`.
///
/// Optional `light` / `emitter` / `movement` / `weapon` carry per-entity-type
/// component presets. The level-load spawn path materializes these into a
/// fresh ECS entity per matching placement.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EntityTypeDescriptor {
    pub(crate) canonical_name: Option<String>,
    pub(crate) default_weapon: Option<String>,
    pub(crate) light: Option<LightDescriptor>,
    pub(crate) emitter: Option<BillboardEmitterComponent>,
    pub(crate) movement: Option<PlayerMovementDescriptor>,
    pub(crate) weapon: Option<WeaponDescriptor>,
    pub(crate) mesh: Option<MeshDescriptor>,
    pub(crate) health: Option<HealthDescriptor>,
    pub(crate) ai: Option<AiDescriptor>,
}
