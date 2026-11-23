// Data-context descriptors: light/mesh/entity-type descriptors.
// See: context/lib/scripting.md

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::components::billboard_emitter::BillboardEmitterComponent;
use crate::components::brain::TravelVariant;
use crate::components::mesh::{AnimationState, InterruptPolicy, RATE_MAX};
use crate::data_descriptors::{
    BehaviorGraphDescriptor, DescriptorError, HealthDescriptor, PlayerMovementDescriptor,
    TouchableDescriptor, WeaponDescriptor,
};
use crate::provenance::DescriptorComponentKind;

use super::weapon_mounts::{WeaponMountDescriptor, validate_weapon_mounts};
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
/// `travel_speed` (when present) finite > 0; `max_playback_rate` (when present)
/// finite and `>= RATE_MAX`; `playback_rate` (when present) finite > 0;
/// `interrupt` (when present on the
/// wire) one of `"smooth"`/`"snap"`. When `animations` is present it must be
/// non-empty and `default_state` must be present and name a declared state. A
/// `defaultState` without an `animations` block is also rejected. Clip resolution
/// against the model's clip metadata is resolved at level load by
/// `resolve_mesh_entity_clips`; `AnimationState::clip_index` stays `None` at parse.
/// `weapon_mounts` follows the rules in
/// [`super::weapon_mounts::validate_weapon_mounts`], and the readers reject any
/// key outside [`super::weapon_mounts::MESH_DESCRIPTOR_KEYS`].
#[derive(Debug, Clone, PartialEq)]
pub struct MeshDescriptor {
    pub model: String,
    /// When true, this mesh is collected only for shadow-depth presentation.
    /// `shadowOnly` on the script surface; omission preserves normal forward
    /// rendering. Player materialization retains the bit only for the owning
    /// local viewer and clears it for peer-visible avatars.
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
    /// The character's weapon-mount table (`weaponMounts` on the wire): for
    /// each weapon it may wield, the props fitted to this model's rig. `None`
    /// when the table is absent, which opts the character out of mounted
    /// weapons silently; `Some` (even empty) declares the table. Part of the
    /// mesh, so the hot-reload mesh deferral covers it.
    pub weapon_mounts: Option<Vec<WeaponMountDescriptor>>,
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
    /// Raw per-state `maxPlaybackRate` (`None` = absent ⇒ the engine default
    /// `RATE_MAX`). Validated in [`MeshDescriptor::build`].
    pub max_playback_rate: Option<f32>,
    /// Raw per-state `scaleWithSpeed` (`None` = absent ⇒ `true`).
    pub scale_with_speed: Option<bool>,
    /// Raw per-state `playbackRate` (`None` = absent ⇒ 1). Validated in
    /// [`MeshDescriptor::build`].
    pub playback_rate: Option<f32>,
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
    pub weapon_mounts: Option<Vec<WeaponMountDescriptor>>,
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
    /// Player presentation treats it as an owning-view exception: the local body
    /// remains shadow-only while peer viewers render the same avatar forward.
    /// `shadowBiasScale` is optional on the wire, defaults to 1.0, and must be
    /// finite in 0.0..=4.0. `owner` names the character in weapon-mount
    /// errors ([`super::weapon_mounts::mesh_owner_label`]).
    pub fn build(raw: RawMeshDescriptor, owner: &str) -> Result<Self, DescriptorError> {
        let RawMeshDescriptor {
            model,
            attachments,
            states,
            default_state,
            animations_present,
            locomotion,
            shadow_bias_scale,
            shadow_only,
            weapon_mounts,
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

        if let Some(entries) = weapon_mounts.as_deref() {
            validate_weapon_mounts(owner, entries)?;
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
        // Each state's authored rate fields, unset ones `None`, for the travel
        // variants' inheritance below.
        let mut rate_fields = HashMap::with_capacity(states.len());
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
            if let Some(travel_speed) = raw.travel_speed
                && (!travel_speed.is_finite() || travel_speed <= 0.0)
            {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.mesh.animations.{}.travelSpeed` must be a finite value > 0.0, got {}",
                        raw.name, travel_speed
                    ),
                });
            }
            // A present `maxPlaybackRate` only raises the stride cap: finite
            // and at least the engine default, so `RATE_MIN..=cap` stays a
            // valid range and production states cannot lower it.
            if let Some(max_playback_rate) = raw.max_playback_rate
                && (!max_playback_rate.is_finite() || max_playback_rate < RATE_MAX)
            {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.mesh.animations.{}.maxPlaybackRate` must be a finite value >= {RATE_MAX}, got {}",
                        raw.name, max_playback_rate
                    ),
                });
            }
            // A present `playbackRate` is a fixed authored speed: finite and
            // strictly positive, with no stride clamp.
            if let Some(playback_rate) = raw.playback_rate
                && (!playback_rate.is_finite() || playback_rate <= 0.0)
            {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.mesh.animations.{}.playbackRate` must be a finite value > 0.0, got {}",
                        raw.name, playback_rate
                    ),
                });
            }
            rate_fields.insert(
                raw.name.clone(),
                (
                    raw.max_playback_rate,
                    raw.scale_with_speed,
                    raw.playback_rate,
                ),
            );
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
                    max_playback_rate: raw.max_playback_rate,
                    scale_with_speed: raw.scale_with_speed.unwrap_or(true),
                    playback_rate: raw.playback_rate,
                    // Resolved against the model's clip metadata at level load
                    // by `resolve_mesh_entity_clips`; unresolved here.
                    clip_index: None,
                },
            );
        }

        // A travel variant (`<base>_left`, `_right`, `_backward`, the names the
        // AI draws while steering) inherits each rate field it leaves unset
        // from its base state, so it plays as its base does: `run_left` keeps
        // `run`'s raised cap, and a leap's `_backward` keeps its fixed 1x.
        // Resolved here, so host and client read one resolved state through
        // `MeshAnimation::stride_playback_target`.
        for (name, &(max_playback_rate, scale_with_speed, playback_rate)) in &rate_fields {
            let Some(base) = TravelVariant::ALL
                .into_iter()
                .filter(|variant| *variant != TravelVariant::Base)
                .find_map(|variant| name.strip_suffix(variant.suffix()))
            else {
                continue;
            };
            let (Some(&(base_max, base_scale, base_rate)), Some(state)) =
                (rate_fields.get(base), animations.get_mut(name))
            else {
                continue;
            };
            state.max_playback_rate = max_playback_rate.or(base_max);
            state.scale_with_speed = scale_with_speed.or(base_scale).unwrap_or(true);
            state.playback_rate = playback_rate.or(base_rate);
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
            weapon_mounts,
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
/// `inventory` declares the canonical wieldable archetypes spawned beside a
/// player pawn. Runtime state stores the resolved instance ids, one per slot.
///
/// Optional `light` / `emitter` / `movement` / `weapon` / `touchable` carry per-entity-type
/// component presets. The level-load spawn path materializes these into a
/// fresh ECS entity per matching placement.
///
/// `behavior` is the sole authoring surface for an enemy brain.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityTypeDescriptor {
    /// Resolved faction index for brain-bearing entity seeds. This is never a
    /// script-authored numeric value: manifest drain resolves an optional named
    /// `components.faction` declaration before this descriptor reaches spawn.
    pub faction: Option<f32>,
    /// Optional finite retaliation tolerance for brain-bearing entity
    /// seeds. It remains engine-owned entity state at runtime; authored guards
    /// read the resolved candidate fact rather than this storage value.
    pub tolerance: Option<f32>,
    pub canonical_name: Option<String>,
    /// Whether entities materialized from this descriptor can ghost through an
    /// armed ghost link. A descriptor attribute, not a component kind: it never
    /// enters component provenance or the networked-by-default classification,
    /// and each peer reads it from its own descriptors. Defaults to false for
    /// every descriptor, a player's included.
    pub ghost: bool,
    pub inventory: Option<InventoryDescriptor>,
    pub light: Option<LightDescriptor>,
    pub emitter: Option<BillboardEmitterComponent>,
    pub movement: Option<PlayerMovementDescriptor>,
    pub life_states: Option<crate::data_descriptors::LifeStatesDescriptor>,
    pub weapon: Option<WeaponDescriptor>,
    pub touchable: Option<TouchableDescriptor>,
    pub mesh: Option<MeshDescriptor>,
    pub health: Option<HealthDescriptor>,
    pub behavior: Option<BehaviorGraphDescriptor>,
}

impl EntityTypeDescriptor {
    /// The checks only the mesh and the behavior together can make, run by
    /// both entity readers once each has parsed its own block: a stance
    /// release that repeats (`passes`) plays a looping state, in its own
    /// state and every travel variant the mesh declares. A client replays each
    /// pass from the replicated state name alone, which a one-shot cannot do.
    /// A state the mesh does not declare is left to spawn validation's
    /// warning.
    pub fn validate_across_components(&self) -> Result<(), DescriptorError> {
        let (Some(mesh), Some(behavior)) = (self.mesh.as_ref(), self.behavior.as_ref()) else {
            return Ok(());
        };
        repeating_releases_loop(&behavior.envelope, &mesh.animations)
    }

    /// Whether this descriptor declares a component in the provenance vocabulary.
    /// Behavior is a separate authoring surface and is checked directly by callers.
    pub fn declares(&self, component: DescriptorComponentKind) -> bool {
        match component {
            DescriptorComponentKind::Weapon => self.weapon.is_some(),
            DescriptorComponentKind::Movement => self.movement.is_some(),
            DescriptorComponentKind::Light => self.light.is_some(),
            DescriptorComponentKind::Emitter => self.emitter.is_some(),
            DescriptorComponentKind::Mesh => self.mesh.is_some(),
            DescriptorComponentKind::Health => self.health.is_some(),
            DescriptorComponentKind::Touchable => self.touchable.is_some(),
        }
    }
}

fn repeating_releases_loop(
    envelope: &postretro_foundation::BehaviorGraphEnvelope,
    states: &HashMap<String, AnimationState>,
) -> Result<(), DescriptorError> {
    for activity in envelope.activities.values() {
        if let (Some(passes @ 2..), Some(base)) = (activity.passes, activity.animation.as_deref()) {
            for variant in TravelVariant::ALL {
                let state = format!("{base}{}", variant.suffix());
                if states.get(&state).is_some_and(|declared| !declared.looping) {
                    let attack = match activity.action.as_ref() {
                        Some(postretro_foundation::ActionVerb::Attack { name, .. }) => {
                            name.as_str()
                        }
                        None => "",
                    };
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "attack \"{attack}\"'s release repeats {passes} passes on animation state \"{state}\", which does not loop; a client replays each pass only from a looping state, so set `loop: true` on `components.mesh.animations.{state}`"
                        ),
                    });
                }
            }
        }
        for layer in activity.layers.values() {
            if let postretro_foundation::BehaviorLayerDescriptor::Graph(child) = layer {
                repeating_releases_loop(child, states)?;
            }
        }
    }
    Ok(())
}

/// Author-side player inventory composition. SDK builders validate typed
/// descriptor references, then lower them to canonical archetype names for
/// this runtime descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryDescriptor {
    #[serde(default)]
    pub loadout: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_state(
        name: &str,
        max_playback_rate: Option<f32>,
        scale_with_speed: Option<bool>,
        playback_rate: Option<f32>,
    ) -> RawAnimationState {
        RawAnimationState {
            name: name.to_string(),
            clip: name.to_string(),
            looping: true,
            crossfade_ms: 0.0,
            interrupt: None,
            travel_speed: None,
            max_playback_rate,
            scale_with_speed,
            playback_rate,
        }
    }

    // A travel variant inherits each rate field it leaves unset from its base
    // state, and keeps any it sets; a state with no base keeps its own.
    #[test]
    fn a_travel_variant_inherits_its_base_states_unset_rate_fields() {
        let mesh = MeshDescriptor::build(
            RawMeshDescriptor {
                model: "brute".to_string(),
                attachments: HashMap::new(),
                states: vec![
                    raw_state("run", Some(3.0), None, None),
                    raw_state("run_left", None, None, None),
                    raw_state("run_right", Some(2.0), None, None),
                    raw_state("leap", None, Some(false), Some(1.25)),
                    raw_state("leap_backward", None, None, None),
                    raw_state("leap_left", None, Some(true), Some(2.0)),
                    raw_state("idle_left", None, None, None),
                ],
                default_state: Some("run".to_string()),
                animations_present: true,
                locomotion: None,
                shadow_bias_scale: None,
                shadow_only: false,
                weapon_mounts: None,
            },
            "entity `brute`",
        )
        .expect("the mesh builds");
        let rates = |name: &str| {
            let state = &mesh.animations[name];
            (
                state.max_playback_rate,
                state.scale_with_speed,
                state.playback_rate,
            )
        };
        assert_eq!(rates("run_left"), (Some(3.0), true, None), "the raised cap");
        assert_eq!(rates("run_right"), (Some(2.0), true, None), "its own cap");
        assert_eq!(
            rates("leap_backward"),
            (None, false, Some(1.25)),
            "the base's fixed rate"
        );
        assert_eq!(
            rates("leap_left"),
            (None, true, Some(2.0)),
            "its own scaling and rate"
        );
        assert_eq!(rates("idle_left"), (None, true, None), "no base: defaults");
        assert_eq!(rates("run"), (Some(3.0), true, None), "a base is unchanged");
    }
}
