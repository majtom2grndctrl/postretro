// Data-context descriptors: light/mesh/entity-type descriptors.
// See: context/lib/scripting.md

use std::collections::HashMap;

use crate::components::billboard_emitter::BillboardEmitterComponent;
use crate::components::mesh::{AnimationState, InterruptPolicy};
use crate::data_descriptors::{
    AiDescriptor, DescriptorError, HealthDescriptor, PlayerMovementDescriptor, WeaponDescriptor,
};
pub use postretro_foundation::data_descriptors::LightDescriptor;

/// Authored mesh component preset attached to an [`EntityTypeDescriptor`].
/// Carries the model handle a mesh entity renders plus an optional declared
/// animation-state surface. The data-archetype spawn path materializes this into
/// a [`crate::components::mesh::MeshComponent`]: a descriptor with no
/// `animations` block yields a stateless component, otherwise the declared state
/// map is copied in via `MeshAnimation::new` with current = `default_state` and a
/// pending entry stamp.
///
/// Validation (at parse time): `model`, attachment socket names, attachment model
/// paths, and each state's `clip` are non-empty; `crossfade_ms` finite ≥ 0;
/// `travel_speed` (when present) finite > 0; `interrupt` (when present on the
/// wire) one of `"smooth"`/`"snap"`. When `animations` is present it must be
/// non-empty and `default_state` must be present and name a declared state. A
/// `defaultState` without an `animations` block is also rejected. Clip resolution
/// against the model's clip metadata is resolved at level load by
/// `resolve_mesh_entity_clips`; `AnimationState::clip_index` stays `None` at parse.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshDescriptor {
    pub model: String,
    /// When true, this mesh is collected only for shadow-depth presentation.
    /// `shadowOnly` on the script surface; omission preserves normal forward
    /// rendering. The renderer consumes the materialized component flag.
    pub shadow_only: bool,
    /// Named holder socket → content-relative prop model path. The spawn path
    /// materializes these into transiently unresolved mesh attachments; level
    /// load resolves their holder-side binding from the loaded glTF sockets.
    pub attachments: HashMap<String, String>,
    /// Per-model receiver-bias multiplier for pooled/runtime shadows, including
    /// skinned receipt from promoted static lights. The script-facing field is
    /// `shadowBiasScale`; omission preserves 1.0.
    pub shadow_bias_scale: f32,
    /// Declared state map: state name → clip + loop + crossfade + interrupt.
    /// Empty when the descriptor declared no `animations` block (stateless).
    pub animations: HashMap<String, AnimationState>,
    /// The default/spawn state name. `Some` exactly when `animations` is
    /// non-empty; parse validation rejects animations-without-default and a
    /// default that does not name a declared state.
    pub default_state: Option<String>,
    /// Optional authored locomotion calibration block (`locomotion?` on the
    /// wire). `None` when the block is absent, in which case the runtime uses
    /// the `speed_scale = true` default. Threaded onto the runtime
    /// [`crate::components::mesh::MeshAnimation`] at spawn.
    pub locomotion: Option<LocomotionDescriptor>,
}

/// Authored per-archetype locomotion calibration attached to
/// [`MeshDescriptor::locomotion`]. Sibling to the per-state `travelSpeed`
/// override; carries the `speedScale` rate-scaling toggle.
#[derive(Debug, Clone, PartialEq)]
pub struct LocomotionDescriptor {
    /// Whether locomotion rate-scaling applies to this archetype's playback.
    /// `speedScale` on the wire; defaults to `true` when the field or the whole
    /// `locomotion` block is absent (preserving speed-scaled playback).
    pub speed_scale: bool,
}

impl Default for LocomotionDescriptor {
    fn default() -> Self {
        Self { speed_scale: true }
    }
}

impl LocomotionDescriptor {
    /// Build the authored block from its optional wire field. Owning the default
    /// here keeps QuickJS, Luau, and direct Rust descriptor construction on one
    /// `speedScale` contract.
    pub fn from_optional_speed_scale(speed_scale: Option<bool>) -> Self {
        Self {
            speed_scale: speed_scale.unwrap_or(Self::default().speed_scale),
        }
    }
}

/// One parsed-but-unvalidated animation-state entry, as gathered from the wire
/// by either FFI path. `interrupt` is the raw string when present (`None` =
/// absent ⇒ defaults to `"smooth"`); validation maps it to [`InterruptPolicy`].
pub struct RawAnimationState {
    pub name: String,
    pub clip: String,
    pub looping: bool,
    pub crossfade_ms: f32,
    pub interrupt: Option<String>,
    /// Raw per-state `travelSpeed` override (`None` = absent). Positivity /
    /// finiteness is validated in [`MeshDescriptor::build`] so both FFI paths
    /// reject the same inputs.
    pub travel_speed: Option<f32>,
}

/// Parsed mesh fields supplied by the JS and Luau descriptor bridges before
/// shared validation turns them into a [`MeshDescriptor`].
pub struct RawMeshDescriptor {
    pub model: String,
    pub attachments: HashMap<String, String>,
    pub states: Vec<RawAnimationState>,
    pub default_state: Option<String>,
    pub animations_present: bool,
    pub locomotion: Option<LocomotionDescriptor>,
    pub shadow_bias_scale: Option<f32>,
    pub shadow_only: bool,
}

impl MeshDescriptor {
    /// Effective locomotion rate-scaling toggle. Omitting either the block or
    /// its field preserves the shared default (`true`).
    pub fn speed_scale(&self) -> bool {
        self.locomotion
            .as_ref()
            .cloned()
            .unwrap_or_default()
            .speed_scale
    }

    /// Build and validate a [`MeshDescriptor`] from the raw fields gathered by
    /// the JS / Luau parsers. Shared so both FFI paths enforce identical rules:
    /// non-empty model, attachment socket names, attachment model paths, and
    /// clips; finite ≥ 0 `crossfadeMs`; `interrupt` in {smooth, snap}; and — when
    /// any state is declared — a present `defaultState` that names a declared
    /// state. An empty-but-present `animations` block is rejected; a wholly absent
    /// one yields a stateless descriptor (`animations` empty, `default_state`
    /// None). `shadowOnly` is optional descriptor input and defaults to `false`.
    /// `shadowBiasScale` is optional on the wire, defaults to 1.0, and must be
    /// finite in 0.0..=4.0.
    pub fn build(raw: RawMeshDescriptor) -> Result<Self, DescriptorError> {
        let RawMeshDescriptor {
            model,
            attachments,
            states,
            default_state,
            animations_present,
            locomotion,
            shadow_bias_scale,
            shadow_only,
        } = raw;
        if model.is_empty() {
            return Err(DescriptorError::InvalidShape {
                reason: "`components.mesh.model` must be a non-empty string".to_string(),
            });
        }

        for (socket, attachment_model) in &attachments {
            if socket.is_empty() {
                return Err(DescriptorError::InvalidShape {
                    reason: "`components.mesh.attachments` must not contain an empty socket name"
                        .to_string(),
                });
            }
            if attachment_model.is_empty() {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.mesh.attachments.{socket}` must be a non-empty model path"
                    ),
                });
            }
        }

        let shadow_bias_scale = shadow_bias_scale.unwrap_or(1.0);
        if !shadow_bias_scale.is_finite() || !(0.0..=4.0).contains(&shadow_bias_scale) {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.mesh.shadowBiasScale` must be a finite value in 0.0..=4.0, got {shadow_bias_scale}"
                ),
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
            // A present `travelSpeed` override must be a finite ground-units /
            // animated-second value strictly greater than zero. Validated here
            // in the shared builder so QuickJS and Luau reject identical inputs.
            if let Some(travel_speed) = raw.travel_speed {
                if !travel_speed.is_finite() || travel_speed <= 0.0 {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.mesh.animations.{}.travelSpeed` must be a finite value > 0.0, got {}",
                            raw.name, travel_speed
                        ),
                    });
                }
            }
            animations.insert(
                raw.name,
                AnimationState {
                    clip: raw.clip,
                    looping: raw.looping,
                    crossfade_ms: raw.crossfade_ms,
                    interrupt,
                    // Carried straight from the validated raw state onto the
                    // runtime `AnimationState`; no extra threading needed since
                    // this map is what `MeshAnimation::new` receives.
                    travel_speed: raw.travel_speed,
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
            shadow_only,
            attachments,
            shadow_bias_scale,
            animations,
            default_state,
            locomotion,
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
pub struct EntityTypeDescriptor {
    pub canonical_name: Option<String>,
    pub default_weapon: Option<String>,
    pub light: Option<LightDescriptor>,
    pub emitter: Option<BillboardEmitterComponent>,
    pub movement: Option<PlayerMovementDescriptor>,
    pub weapon: Option<WeaponDescriptor>,
    pub mesh: Option<MeshDescriptor>,
    pub health: Option<HealthDescriptor>,
    pub ai: Option<AiDescriptor>,
}
