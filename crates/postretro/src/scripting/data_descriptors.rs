// Data-context descriptor types and their JS/Luau deserialization paths.
// See: context/lib/scripting.md §2 (Context Model) — data context lifecycle;
//      §10 (Reaction Primitives) — named-reaction and crossing vocabulary.

use mlua::{Table, Value as LuaValue};
use rquickjs::{Array, Ctx, Object, Value as JsValue};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use std::collections::HashMap;

use super::components::billboard_emitter::{
    BillboardEmitterComponent, BillboardEmitterComponentLit,
};
use super::components::mesh::{AnimationState, InterruptPolicy};
use super::registry::EntityId;
use crate::movement::MovementScope;
use crate::scripting::ir::{BakedIr, CURRENT_IR_VERSION, IrNode, IrType, bind};

/// Variants of a single reaction's behavior body. The `name` lives on the
/// wrapping [`NamedReaction`]; this enum captures only the descriptor shape.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ReactionDescriptor {
    Progress(ProgressDescriptor),
    Primitive(PrimitiveDescriptor),
    /// Ordered list of (entity, sequenced-primitive, args) steps. Steps fire
    /// in order at dispatch time; failures and stale entity IDs are logged as
    /// warnings rather than aborting the sequence.
    Sequence(Vec<SequenceStep>),
}

/// One step in a `sequence` reaction.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SequenceStep {
    pub(crate) id: EntityId,
    pub(crate) primitive: String,
    pub(crate) args: serde_json::Value,
}

/// Threshold reaction: counts kills against a tag and fires an event when the
/// kill ratio reaches `at`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProgressDescriptor {
    pub(crate) tag: String,
    pub(crate) at: f32,
    pub(crate) fire: String,
}

/// Primitive-action reaction. One descriptor shape, two execution arms (M13
/// HUD dynamics): when `tag` is `Some`, the primitive resolves the tag to
/// entities and mutates the `EntityRegistry`; when `tag` is `None`, it is a
/// **system reaction** — it targets no entities and instead enqueues a typed
/// `SystemReactionCommand` for the app's per-frame drain. The two arms share
/// one named-event namespace; the dispatcher picks the arm by `tag` presence.
///
/// `args` carries the primitive-specific payload (e.g. `{ "rate": 0.0 }` for
/// `setEmitterRate`, `{ "sound": "alarm" }` for `playSound`). Defaults to an
/// empty JSON object when the descriptor omits the field, so primitives that
/// take no args parse cleanly.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PrimitiveDescriptor {
    pub(crate) primitive: String,
    /// Entity tag to target. `None` ⇒ system-targeted (no entities).
    pub(crate) tag: Option<String>,
    pub(crate) on_complete: Option<String>,
    pub(crate) args: serde_json::Value,
}

/// A reaction descriptor paired with the event name it is registered under.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NamedReaction {
    pub(crate) name: String,
    pub(crate) descriptor: ReactionDescriptor,
}

/// The condition half of a state-crossing watcher (M13 HUD dynamics). A
/// crossing fires when the watched slot transitions across `threshold` in the
/// declared direction. `threshold` is stored as a fraction of the
/// registration's `max` (`raw_threshold / max`); the watcher compares it
/// against the slot's `current / max`, so a registration with no `max`
/// (default `1.0`) degrades to a raw-value comparison.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CrossingCondition {
    /// Fires on a downward crossing: `prev >= threshold && cur < threshold`.
    Below { threshold: f32 },
    /// Fires on an upward crossing: `prev <= threshold && cur > threshold`.
    Above { threshold: f32 },
}

/// A state-crossing watcher declared by `onStateCrossing` and carried back
/// through `setupLevel`'s manifest (scripting.md §12 (Non-Goals): no
/// side-effect FFI — cross-FFI values flow through setup-function returns).
/// The detector watches `slot` after each frame's
/// slot writes and, on a crossing in the condition's direction, dispatches
/// every event in `fire` synchronously through the named-reaction vocabulary.
///
/// `max` is the registration's denominator: thresholds are fractions of it.
/// It defaults to `1.0` (raw-value comparison) when the registration omits it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CrossingDescriptor {
    pub(crate) slot: String,
    pub(crate) condition: CrossingCondition,
    pub(crate) max: f32,
    pub(crate) fire: Vec<String>,
}

/// Authored light component preset attached to an [`EntityTypeDescriptor`].
/// Mirrors the runtime [`super::components::light::LightComponent`] shape but
/// only carries the script-authored fields (no animation, no cone, no
/// shadows). Spawn-time defaults fill the rest. `range` is mapped onto
/// [`super::components::light::LightComponent::falloff_range`] when the
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
/// this into a [`super::components::mesh::MeshComponent`]: a descriptor with no
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
struct RawAnimationState {
    name: String,
    clip: String,
    looping: bool,
    crossfade_ms: f32,
    interrupt: Option<String>,
}

impl MeshDescriptor {
    /// Build and validate a [`MeshDescriptor`] from the raw fields gathered by
    /// the JS / Luau parsers. Shared so both FFI paths enforce identical rules:
    /// non-empty `model`/`clip`, finite ≥ 0 `crossfadeMs`, `interrupt` in
    /// {smooth, snap}, and — when any state is declared — a present
    /// `defaultState` that names a declared state. An empty-but-present
    /// `animations` block is rejected; a wholly absent one yields a stateless
    /// descriptor (`animations` empty, `default_state` None).
    fn build(
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
/// and drained into `DataRegistry` after `setupMod()` returns.
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum FireMode {
    Semi,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ResolutionMode {
    Hitscan,
}

/// Authored weapon component preset. This is descriptor-owned tuning data:
/// maps do not override these params, and the runtime materializes a separate
/// wieldable instance entity from the descriptor at player spawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeaponDescriptor {
    pub(crate) damage: f32,
    pub(crate) range: f32,
    #[serde(rename = "fireRateMs")]
    pub(crate) cooldown_ms: f32,
    pub(crate) fire_mode: FireMode,
    pub(crate) resolution: ResolutionMode,
}

impl WeaponDescriptor {
    pub(crate) fn validate(self) -> Result<Self, DescriptorError> {
        if !self.damage.is_finite() || self.damage < 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.damage` must be a finite value >= 0.0, got {}",
                    self.damage
                ),
            });
        }
        if !self.range.is_finite() || self.range <= 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.range` must be a finite value > 0.0, got {}",
                    self.range
                ),
            });
        }
        if !self.cooldown_ms.is_finite() || self.cooldown_ms <= 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.fireRateMs` must be a finite value > 0.0, got {}",
                    self.cooldown_ms
                ),
            });
        }
        Ok(self)
    }
}

/// Authored health component preset attached to an [`EntityTypeDescriptor`].
/// `max` is the entity's hit-point ceiling; the optional `hitbox` makes the
/// entity hitscan-targetable (one world-aligned AABB, fixed per archetype).
/// Wire keys are camelCase. The data-archetype spawn path materializes this
/// into a [`super::components::health::HealthComponent`] with `current == max`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HealthDescriptor {
    pub(crate) max: f32,
    #[serde(default)]
    pub(crate) hitbox: Option<HitboxDescriptor>,
}

/// Authored hitbox sub-block: one world-aligned AABB. `half_extents` is the
/// box half-size on each axis; `offset` shifts the box center from the entity's
/// transform position (defaults to zero when absent).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HitboxDescriptor {
    pub(crate) half_extents: [f32; 3],
    #[serde(default)]
    pub(crate) offset: Option<[f32; 3]>,
}

impl HealthDescriptor {
    /// Validate bounds serde cannot enforce (the `LightDescriptor::validate`
    /// precedent): `max` finite and `> 0`; each `halfExtents` element finite and
    /// `> 0`; each `offset` element finite.
    pub(crate) fn validate(self) -> Result<Self, DescriptorError> {
        if !self.max.is_finite() || self.max <= 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.health.max` must be a finite value > 0.0, got {}",
                    self.max
                ),
            });
        }
        if let Some(hitbox) = self.hitbox.as_ref() {
            for (axis, value) in ["x", "y", "z"].iter().zip(hitbox.half_extents) {
                if !value.is_finite() || value <= 0.0 {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.health.hitbox.halfExtents.{axis}` must be a finite value > 0.0, got {value}"
                        ),
                    });
                }
            }
            if let Some(offset) = hitbox.offset {
                for (axis, value) in ["x", "y", "z"].iter().zip(offset) {
                    if !value.is_finite() {
                        return Err(DescriptorError::InvalidShape {
                            reason: format!(
                                "`components.health.hitbox.offset.{axis}` must be a finite value, got {value}"
                            ),
                        });
                    }
                }
            }
        }
        Ok(self)
    }
}

/// Authored player-movement component preset. The four core sub-objects
/// (`capsule`, `ground`, `air`, `fall`) are required when `movement` is
/// present; `dash` is optional — its absence disables dash entirely; `crouch`
/// is optional — its absence disables crouch entirely. The data-archetype
/// spawn path materializes the runtime movement component from this.
/// `ground.max_slope` is in degrees on the wire and converted to a cosine at
/// materialization (not here).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlayerMovementDescriptor {
    pub(crate) capsule: CapsuleParams,
    pub(crate) ground: GroundParams,
    pub(crate) air: AirParams,
    pub(crate) fall: FallParams,
    /// Stuck-stop deadzone enable flag. See `PlayerMovementComponent` for
    /// trigger semantics. Defaults to `true`; the JS/Luau parsers fall back
    /// to this default when the field is omitted, keeping the deadzone
    /// opt-out (not opt-in) for existing authored descriptors.
    pub(crate) stuck_stop_enabled: bool,
    /// Horizontal-displacement threshold (metres) gating the deadzone. See
    /// `PlayerMovementComponent` for tuning rationale. Defaults to `1.0e-3`.
    pub(crate) stuck_stop_threshold: f32,
    /// Optional dash tuning. Absent ⇒ dash disabled (no `DashParams`
    /// materialized). When present, all of its fields are required, matching
    /// the present-then-all-required discipline of `ground`/`air`/`fall`.
    pub(crate) dash: Option<DashParams>,
    /// Optional input-forgiveness tuning (coyote time + jump buffer). Absent ⇒
    /// the documented engine defaults apply (both ~100 ms). Per-field zero
    /// disables that grace independently. See `ForgivenessParams`.
    pub(crate) forgiveness: Option<ForgivenessParams>,
    /// Optional crouch tuning. Absent ⇒ crouch disabled (no `CrouchParams`
    /// materialized). When present, all of its fields are required, matching
    /// the present-then-all-required discipline of `dash`.
    pub(crate) crouch: Option<CrouchParams>,
    /// Optional first-person view-feel tuning (head bob, strafe tilt, ambient
    /// sway). Absent ⇒ view feel disabled (no `ViewFeelParams` materialized).
    /// A render-only camera effect — see `ViewFeelParams`.
    pub(crate) view_feel: Option<ViewFeelParams>,
}

impl PlayerMovementDescriptor {
    /// Default values for the stuck-stop fields. Used by the JS/Luau parsers
    /// when the descriptor omits them so existing scripts keep working.
    pub(crate) const DEFAULT_STUCK_STOP_ENABLED: bool = true;
    pub(crate) const DEFAULT_STUCK_STOP_THRESHOLD: f32 = 1.0e-3;
}

/// Input-forgiveness tuning: coyote time (a grounded jump permitted for a window
/// after leaving a ledge) and jump buffering (a jump pressed shortly before
/// landing fires on the landing tick). Both windows are in MILLISECONDS,
/// advanced off the same `dt` the movement tick already accumulates
/// (`dt * 1000.0` per tick), mirroring the dash cooldown's ms accounting.
///
/// This sub-object is OPTIONAL on [`PlayerMovementDescriptor`]: when the whole
/// `forgiveness` object is absent, the documented engine defaults apply (see the
/// `DEFAULT_*` constants). When the object is present, each wire field is
/// individually optional — `forgiveness_params_from_js` / `forgiveness_params_from_lua`
/// substitute per-field defaults for any omitted key; the resulting Rust struct
/// always carries concrete `f32` values (`coyote_ms == 0.0` means explicitly
/// zeroed, not absent). An explicit `0` disables that grace independently (the
/// regression fixtures pin both to zero to preserve exact edge timing). Field
/// names are camelCase on the wire (`coyoteMs`, `jumpBufferMs`) and snake_case
/// in Rust.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct ForgivenessParams {
    /// Coyote-time window in milliseconds: a grounded jump is permitted for this
    /// long after ground is lost (with no prior jump). `0` disables coyote time.
    pub(crate) coyote_ms: f32,
    /// Jump-buffer window in milliseconds: a jump pressed while airborne is
    /// retained for this long and fires on the landing tick. `0` disables jump
    /// buffering.
    pub(crate) jump_buffer_ms: f32,
}

impl ForgivenessParams {
    /// Feel-friendly engine default for the coyote-time window (milliseconds),
    /// applied when the `forgiveness` sub-object or the `coyoteMs` field is
    /// absent. ~100 ms ≈ 6 ticks at 60 Hz — a forgiving-but-tight grace.
    pub(crate) const DEFAULT_COYOTE_MS: f32 = 100.0;
    /// Feel-friendly engine default for the jump-buffer window (milliseconds),
    /// applied when the `forgiveness` sub-object or the `jumpBufferMs` field is
    /// absent. ~100 ms ≈ 6 ticks at 60 Hz.
    pub(crate) const DEFAULT_JUMP_BUFFER_MS: f32 = 100.0;

    /// The defaults materialized as a struct, used when the whole `forgiveness`
    /// sub-object is omitted.
    pub(crate) const DEFAULT: ForgivenessParams = ForgivenessParams {
        coyote_ms: Self::DEFAULT_COYOTE_MS,
        jump_buffer_ms: Self::DEFAULT_JUMP_BUFFER_MS,
    };
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CapsuleParams {
    pub(crate) radius: f32,
    pub(crate) half_height: f32,
    /// Camera attachment point measured upward from the capsule center, in
    /// meters. The camera-follow path adds this to the pawn's position each
    /// tick to derive eye position. Must lie in `(0, half_height + radius]` —
    /// the upper bound is the top of the capsule.
    pub(crate) eye_height: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GroundParams {
    pub(crate) speed: SpeedParams,
    pub(crate) accel: f32,
    pub(crate) step_height: f32,
    pub(crate) max_slope: f32,
}

/// Walk/run/crouch ground speeds. The movement tick selects `run` while the
/// sprint input is held, `crouch` while crouched, and `walk` otherwise; the
/// chosen value is the omnidirectional horizontal speed target (and airborne
/// speed cap), not a forward-only bonus. All three fields are required when
/// `ground` is present and validated non-negative finite. `crouch` is in
/// world-units/sec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SpeedParams {
    pub(crate) walk: f32,
    pub(crate) run: f32,
    pub(crate) crouch: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AirParams {
    pub(crate) forward_steer: f32,
    pub(crate) accel: f32,
    pub(crate) max_control_speed: f32,
    pub(crate) bunny_hop: bool,
    pub(crate) jumps: u32,
    pub(crate) jump_velocity: f32,
    pub(crate) jump_ceiling: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FallParams {
    pub(crate) terminal_velocity: f32,
}

/// A dash numeric field: either a bare literal or an engine-evaluated IR
/// expression over the movement-local input namespace ([`MovementScope`]).
///
/// `#[serde(untagged)]` mirrors the substrate's untagged-`IrValue` precedent: a
/// bare JSON number deserializes to [`NumberOrIr::Literal`], an op-tagged object
/// (`{"op": …}`) to [`NumberOrIr::Ir`]. The two are disjoint on the wire — a
/// number never matches the `Ir` (object) variant and an object never matches
/// the `f32` variant — so variant ordering is not load-bearing.
///
/// The hand-written JS/Luau parsers route values explicitly (object → IR,
/// scalar → literal) so the literal path keeps its range validators; the derived
/// serde impls exist so `DashParams` round-trips through `PlayerMovementComponent`'s
/// `Serialize`/`Deserialize`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum NumberOrIr {
    Literal(f32),
    Ir(IrNode),
}

/// A dash boolean field: a bare literal or an engine-evaluated IR expression.
/// The boolean analogue of [`NumberOrIr`]; see its docs for the untagged-serde
/// rationale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum BoolOrIr {
    Literal(bool),
    Ir(IrNode),
}

impl NumberOrIr {
    /// The literal value when this field is a bare scalar, else `None`.
    /// Tests use this to assert expression fields round-trip as `None`.
    pub(crate) fn literal(&self) -> Option<f32> {
        match self {
            NumberOrIr::Literal(v) => Some(*v),
            NumberOrIr::Ir(_) => None,
        }
    }
}

impl BoolOrIr {
    /// The literal value when this field is a bare boolean, else `None`. See
    /// [`NumberOrIr::literal`].
    pub(crate) fn literal(&self) -> Option<bool> {
        match self {
            BoolOrIr::Literal(v) => Some(*v),
            BoolOrIr::Ir(_) => None,
        }
    }
}

impl From<f32> for NumberOrIr {
    fn from(v: f32) -> Self {
        NumberOrIr::Literal(v)
    }
}

impl From<bool> for BoolOrIr {
    fn from(v: bool) -> Self {
        BoolOrIr::Literal(v)
    }
}

impl PartialEq<f32> for NumberOrIr {
    fn eq(&self, other: &f32) -> bool {
        matches!(self, NumberOrIr::Literal(v) if v == other)
    }
}

impl PartialEq<bool> for BoolOrIr {
    fn eq(&self, other: &bool) -> bool {
        matches!(self, BoolOrIr::Literal(v) if v == other)
    }
}

/// Dash tuning. Optional on [`PlayerMovementDescriptor`] (absent disables
/// dash); when present, all fields are required and validated. Field names are
/// camelCase on the wire (`boostSpeed`, `momentumRetention`, …) and snake_case
/// in Rust. Stored later by `PlayerMovementComponent` as `Option<DashParams>`.
///
/// Each value field accepts a bare literal OR an engine-evaluated expression
/// ([`NumberOrIr`] / [`BoolOrIr`]) over the movement-local input namespace; the
/// evaluation moment is engine-pinned per field (see `movement.md` §2). The
/// hand-written parsers validate an expression at declaration: a literal keeps
/// its existing range check; an expression is bound against
/// [`MovementScope::for_validation`] and its root type checked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DashParams {
    /// Impulse magnitude applied on dash, world-units/sec (entry-moment). A
    /// literal must be finite > 0.
    pub(crate) boost_speed: NumberOrIr,
    /// Fraction of pre-dash momentum folded into the dash, unitless `[0, 1]`
    /// (entry-moment).
    pub(crate) momentum_retention: NumberOrIr,
    /// In-dash steering authority, unitless `[0, 1]` (per-tick).
    pub(crate) steer_control: NumberOrIr,
    /// Decay rate of the dash impulse, world-units/sec² (per-tick). A literal
    /// `0` is legitimate.
    pub(crate) dash_drag: NumberOrIr,
    /// Cooldown between dashes in milliseconds (entry-moment). A literal `0` is
    /// legitimate.
    pub(crate) cooldown_ms: NumberOrIr,
    /// Number of air dashes allowed before landing. Stays a plain integer (no
    /// expression form).
    pub(crate) air_dashes: u32,
    /// Whether the dash preserves the pre-dash vertical velocity (entry-moment).
    pub(crate) preserve_vertical: BoolOrIr,
}

/// Crouch tuning. Optional on [`PlayerMovementDescriptor`] (absent disables
/// crouch); when present, all fields are required and validated. Field names are
/// camelCase on the wire (`halfHeight`, `eyeHeight`, `transitionRate`) and
/// snake_case in Rust. Stored later by `PlayerMovementComponent` as
/// `Option<CrouchParams>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CrouchParams {
    /// Crouched capsule half-height, in metres. Must be finite > 0.
    pub(crate) half_height: f32,
    /// Crouched camera attachment point measured upward from the capsule
    /// center, in metres. Must lie in `(0, crouched half_height + radius]` —
    /// the upper bound is the top of the crouched capsule.
    pub(crate) eye_height: f32,
    /// Rate at which the capsule interpolates between standing and crouched
    /// extents, per-sec. Must be finite > 0.
    pub(crate) transition_rate: f32,
}

/// First-person view-feel tuning: a render-only camera effect bundle (head bob,
/// strafe tilt, ambient sway). OPTIONAL on [`PlayerMovementDescriptor`] — absent
/// disables view feel entirely (no `ViewFeelParams` materialized). When present,
/// each of `bob`/`tilt`/`sway` is independently optional; an absent sub-object
/// disables that motion. Within a present sub-object, all tuning fields are
/// required EXCEPT the optional `groundedOnly` gate. This two-level
/// present-then-all-required discipline mirrors the optional `dash`/`crouch`
/// sub-objects, applied at two nesting levels. View feel is consumed by the
/// render-rate evaluator in `view_feel.rs`, called from `main.rs`; this is the data surface only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ViewFeelParams {
    /// Optional head-bob tuning. Absent ⇒ no head bob.
    pub(crate) bob: Option<BobParams>,
    /// Optional strafe-tilt tuning. Absent ⇒ no strafe tilt.
    pub(crate) tilt: Option<TiltParams>,
    /// Optional ambient-sway tuning. Absent ⇒ no ambient sway.
    pub(crate) sway: Option<SwayParams>,
}

/// Head-bob tuning. When present on `viewFeel`, all fields are required and
/// validated except `grounded_only`, which is optional and defaults to `true`.
/// Field names are camelCase on the wire (`verticalFrequency`,
/// `lateralFrequency`, `verticalAmplitude`, …) and snake_case in Rust.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct BobParams {
    /// Vertical bob cycles per metre travelled. Must be finite > 0.
    pub(crate) vertical_frequency: f32,
    /// Lateral bob cycles per metre travelled. Must be finite > 0.
    pub(crate) lateral_frequency: f32,
    /// Vertical bob amplitude. Must be finite ≥ 0.
    pub(crate) vertical_amplitude: f32,
    /// Lateral bob amplitude. Must be finite ≥ 0.
    pub(crate) lateral_amplitude: f32,
    /// Horizontal speed below which bob is suppressed. Must be finite ≥ 0.
    pub(crate) speed_threshold: f32,
    /// Whether bob applies only while grounded. Optional on the wire; the
    /// RESOLVED value is materialized here (default `true` when absent).
    pub(crate) grounded_only: bool,
}

impl BobParams {
    /// Default `groundedOnly` gate applied when the wire field is absent.
    pub(crate) const DEFAULT_GROUNDED_ONLY: bool = true;
}

/// Strafe-tilt tuning. When present on `viewFeel`, all fields are required and
/// validated except `grounded_only`, which is optional and defaults to `true`.
/// Field names are camelCase on the wire and snake_case in Rust; `tension`
/// stays literally `tension` everywhere (the author-facing spring knob).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct TiltParams {
    /// Maximum tilt angle in degrees. Must be finite in `[0, 90]`.
    pub(crate) max_angle: f32,
    /// Lateral speed at which the tilt reaches its reference. Must be finite > 0.
    pub(crate) speed_reference: f32,
    /// Spring tension governing how quickly tilt tracks lateral motion. Must be
    /// finite > 0.
    pub(crate) tension: f32,
    /// Whether tilt applies only while grounded. Optional on the wire; the
    /// RESOLVED value is materialized here (default `true` when absent).
    pub(crate) grounded_only: bool,
}

impl TiltParams {
    /// Default `groundedOnly` gate applied when the wire field is absent.
    pub(crate) const DEFAULT_GROUNDED_ONLY: bool = true;
}

/// Ambient-sway tuning. When present on `viewFeel`, all fields are required and
/// validated except `grounded_only`, which is optional and defaults to `false`.
/// Field names are camelCase on the wire and snake_case in Rust.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct SwayParams {
    /// Sway amplitude in degrees. Must be finite ≥ 0.
    pub(crate) amplitude: f32,
    /// Sway oscillation frequency in Hz. Must be finite > 0.
    pub(crate) frequency: f32,
    /// Scales how much movement speed modulates sway. Must be finite ≥ 0.
    pub(crate) speed_scale: f32,
    /// Whether sway applies only while grounded. Optional on the wire; the
    /// RESOLVED value is materialized here (default `false` when absent).
    pub(crate) grounded_only: bool,
}

impl SwayParams {
    /// Default `groundedOnly` gate applied when the wire field is absent.
    pub(crate) const DEFAULT_GROUNDED_ONLY: bool = false;
}

/// The full bundle returned by a level's `setupLevel(ctx)` export.
///
/// Entity-type descriptors are not part of this manifest — they arrive via
/// `setupMod()`'s `entities` field (mod-init only) and are drained into
/// `DataRegistry` before any level is loaded. `LevelManifest` carries
/// per-level reactions and state-crossing watchers.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LevelManifest {
    pub(crate) reactions: Vec<NamedReaction>,
    /// State-crossing watchers (M13 HUD dynamics). Parsed alongside `reactions`
    /// from the widened `{ reactions, crossings }` setup-manifest return and
    /// drained into the per-level `DataRegistry`; cleared on level unload.
    pub(crate) crossings: Vec<CrossingDescriptor>,
}

#[derive(Debug, Error, PartialEq)]
pub(crate) enum DescriptorError {
    #[error("reaction descriptor missing required field '{field}'")]
    MissingField { field: &'static str },
    #[error(
        "reaction has no recognizable shape (expected 'progress', 'primitive', or 'sequence' key)"
    )]
    UnknownShape,
    #[error("'sequence' field must be an array of step objects")]
    InvalidSequenceShape { reason: String },
    #[error("'primitive' field must not be empty")]
    EmptyPrimitiveName,
    #[error("'at' threshold {value} is out of range [0.0, 1.0]")]
    AtThresholdOutOfRange { value: f32 },
    #[error("manifest deserialization failed: {reason}")]
    InvalidShape { reason: String },
    #[error("crossing entry must declare exactly one of 'below' or 'above' (got {count})")]
    CrossingCondition { count: usize },
}

// --- shared validation ------------------------------------------------------

fn validate_at(value: f32) -> Result<f32, DescriptorError> {
    if !(0.0..=1.0).contains(&value) {
        return Err(DescriptorError::AtThresholdOutOfRange { value });
    }
    Ok(value)
}

fn validate_primitive_name(name: String) -> Result<String, DescriptorError> {
    if name.is_empty() {
        return Err(DescriptorError::EmptyPrimitiveName);
    }
    Ok(name)
}

/// Build a [`CrossingDescriptor`] from the raw fields gathered by either FFI
/// path. Shared so JS and Luau enforce identical rules: a non-empty `slot`,
/// exactly one of `below`/`above` (the threshold value, raw), a finite default
/// `max` of `1.0`, and a `fire` list of event names (empty is permitted — the
/// watcher fires nothing, a no-op). `raw_threshold` is divided by `max` here so
/// the stored threshold is already a fraction of `max`, matching the value the
/// detector compares against (`current / max`).
fn build_crossing(
    slot: String,
    below: Option<f32>,
    above: Option<f32>,
    max: Option<f32>,
    fire: Vec<String>,
) -> Result<CrossingDescriptor, DescriptorError> {
    if slot.is_empty() {
        return Err(DescriptorError::InvalidShape {
            reason: "crossing entry `slot` must be a non-empty string".to_string(),
        });
    }
    let max = max.unwrap_or(1.0);
    if !max.is_finite() || max <= 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!("crossing entry `max` must be a finite value > 0.0, got {max}"),
        });
    }
    let condition = match (below, above) {
        (Some(below), None) => {
            if !below.is_finite() {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("crossing entry `below` must be finite, got {below}"),
                });
            }
            CrossingCondition::Below {
                threshold: below / max,
            }
        }
        (None, Some(above)) => {
            if !above.is_finite() {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("crossing entry `above` must be finite, got {above}"),
                });
            }
            CrossingCondition::Above {
                threshold: above / max,
            }
        }
        (None, None) => return Err(DescriptorError::CrossingCondition { count: 0 }),
        (Some(_), Some(_)) => return Err(DescriptorError::CrossingCondition { count: 2 }),
    };
    Ok(CrossingDescriptor {
        slot,
        condition,
        max,
        fire,
    })
}

// --- JS deserialization -----------------------------------------------------

impl LevelManifest {
    /// Deserialize a top-level `{ reactions, crossings }` object returned from
    /// a QuickJS `setupLevel()` call. `crossings` is optional.
    pub(crate) fn from_js_value<'js>(
        ctx: &Ctx<'js>,
        value: JsValue<'js>,
    ) -> Result<Self, DescriptorError> {
        let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
            reason: "setupLevel must return an object".to_string(),
        })?;

        let reactions = if obj.contains_key("reactions").map_err(js_err)? {
            let arr: Array = obj.get("reactions").map_err(js_err)?;
            let mut out = Vec::with_capacity(arr.len());
            for i in 0..arr.len() {
                let item: JsValue = arr.get(i).map_err(js_err)?;
                out.push(named_reaction_from_js(ctx, item)?);
            }
            out
        } else {
            Vec::new()
        };

        let crossings = if obj.contains_key("crossings").map_err(js_err)? {
            let arr: Array = obj.get("crossings").map_err(js_err)?;
            let mut out = Vec::with_capacity(arr.len());
            for i in 0..arr.len() {
                let item: JsValue = arr.get(i).map_err(js_err)?;
                out.push(crossing_descriptor_from_js(&item)?);
            }
            out
        } else {
            Vec::new()
        };

        Ok(Self {
            reactions,
            crossings,
        })
    }

    /// Deserialize a top-level `{ reactions, crossings }` table returned from a
    /// Luau `setupLevel()` call. `crossings` is optional.
    pub(crate) fn from_lua_value(value: LuaValue) -> Result<Self, DescriptorError> {
        let table = match value {
            LuaValue::Table(t) => t,
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("setupLevel must return a table, got {}", other.type_name()),
                });
            }
        };

        let reactions = if table.contains_key("reactions").map_err(lua_err)? {
            let arr: Table = table.get("reactions").map_err(lua_err)?;
            let len = arr.raw_len();
            let mut out = Vec::with_capacity(len);
            for i in 1..=(len as i64) {
                let item: LuaValue = arr.get(i).map_err(lua_err)?;
                out.push(named_reaction_from_lua(item)?);
            }
            out
        } else {
            Vec::new()
        };

        let crossings = if table.contains_key("crossings").map_err(lua_err)? {
            let arr: Table = table.get("crossings").map_err(lua_err)?;
            let len = arr.raw_len();
            let mut out = Vec::with_capacity(len);
            for i in 1..=(len as i64) {
                let item: LuaValue = arr.get(i).map_err(lua_err)?;
                out.push(crossing_descriptor_from_lua(item)?);
            }
            out
        } else {
            Vec::new()
        };

        Ok(Self {
            reactions,
            crossings,
        })
    }
}

fn js_err(e: rquickjs::Error) -> DescriptorError {
    DescriptorError::InvalidShape {
        reason: e.to_string(),
    }
}

fn lua_err(e: mlua::Error) -> DescriptorError {
    DescriptorError::InvalidShape {
        reason: e.to_string(),
    }
}

fn named_reaction_from_js<'js>(
    ctx: &Ctx<'js>,
    value: JsValue<'js>,
) -> Result<NamedReaction, DescriptorError> {
    let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "reaction entry must be an object".to_string(),
    })?;

    let name: String = get_required_string_js(&obj, "name")?;

    // Discriminator: presence of `progress` / `primitive` / `sequence` keys.
    let has_progress = obj.contains_key("progress").map_err(js_err)?;
    let has_primitive = obj.contains_key("primitive").map_err(js_err)?;
    let has_sequence = obj.contains_key("sequence").map_err(js_err)?;

    let descriptor = if has_progress {
        let progress_obj: Object = obj.get("progress").map_err(js_err)?;
        ReactionDescriptor::Progress(progress_descriptor_from_js(ctx, &progress_obj)?)
    } else if has_sequence {
        let arr: Array =
            obj.get("sequence")
                .map_err(|e| DescriptorError::InvalidSequenceShape {
                    reason: e.to_string(),
                })?;
        ReactionDescriptor::Sequence(sequence_steps_from_js(ctx, &arr)?)
    } else if has_primitive {
        ReactionDescriptor::Primitive(primitive_descriptor_from_js(ctx, &obj)?)
    } else {
        return Err(DescriptorError::UnknownShape);
    };

    Ok(NamedReaction { name, descriptor })
}

fn progress_descriptor_from_js<'js>(
    _ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<ProgressDescriptor, DescriptorError> {
    let tag = get_required_string_js(obj, "tag")?;
    let at: f32 = get_required_f32_js(obj, "at")?;
    let at = validate_at(at)?;
    let fire = get_required_string_js(obj, "fire")?;
    Ok(ProgressDescriptor { tag, at, fire })
}

/// Deserialize one crossing entry from a JS object. Shape:
/// `{ slot: string, below?: number, above?: number, max?: number, fire: string[] }`.
/// Exactly one of `below`/`above` is required; `max` defaults to `1.0` (raw
/// comparison). Validation (single condition, finite bounds) is delegated to
/// [`build_crossing`] so both FFI paths share identical rules.
fn crossing_descriptor_from_js<'js>(
    value: &JsValue<'js>,
) -> Result<CrossingDescriptor, DescriptorError> {
    let obj = Object::from_value(value.clone()).map_err(|_| DescriptorError::InvalidShape {
        reason: "crossing entry must be an object".to_string(),
    })?;
    let slot = get_required_string_js(&obj, "slot")?;
    let below = get_optional_f32_js(&obj, "below")?;
    let above = get_optional_f32_js(&obj, "above")?;
    let max = get_optional_f32_js(&obj, "max")?;

    let fire_arr: Array = obj.get("fire").map_err(|_| DescriptorError::InvalidShape {
        reason: "crossing entry `fire` must be an array of event names".to_string(),
    })?;
    let mut fire = Vec::with_capacity(fire_arr.len());
    for i in 0..fire_arr.len() {
        let item: JsValue = fire_arr.get(i).map_err(js_err)?;
        fire.push(String::from_js_value_required(item, "fire")?);
    }

    build_crossing(slot, below, above, max, fire)
}

fn primitive_descriptor_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<PrimitiveDescriptor, DescriptorError> {
    let primitive = get_required_string_js(obj, "primitive")?;
    let primitive = validate_primitive_name(primitive)?;
    // `tag` is optional: absent ⇒ system-targeted reaction (no entities).
    let tag = if obj.contains_key("tag").map_err(js_err)? {
        let raw: JsValue = obj.get("tag").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            Some(String::from_js_value_required(raw, "tag")?)
        }
    } else {
        None
    };

    let on_complete = if obj.contains_key("onComplete").map_err(js_err)? {
        let raw: JsValue = obj.get("onComplete").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            Some(String::from_js_value_required(raw, "onComplete")?)
        }
    } else {
        None
    };

    // `args` is the primitive's typed payload. Absent / null defaults to an
    // empty object so primitives that take no arguments still deserialize.
    let args = if obj.contains_key("args").map_err(js_err)? {
        let raw: JsValue = obj.get("args").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            serde_json::Value::Object(Default::default())
        } else {
            super::conv::js_to_json(ctx, raw).map_err(js_err)?
        }
    } else {
        serde_json::Value::Object(Default::default())
    };

    Ok(PrimitiveDescriptor {
        primitive,
        tag,
        on_complete,
        args,
    })
}

fn sequence_steps_from_js<'js>(
    ctx: &Ctx<'js>,
    arr: &Array<'js>,
) -> Result<Vec<SequenceStep>, DescriptorError> {
    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let item: JsValue = arr.get(i).map_err(js_err)?;
        let obj = Object::from_value(item).map_err(|_| DescriptorError::InvalidSequenceShape {
            reason: format!("step {i} must be an object"),
        })?;
        let id_raw: u32 = get_required_u32_js(&obj, "id")?;
        let primitive = get_required_string_js(&obj, "primitive")?;
        let primitive = validate_primitive_name(primitive)?;
        let args = if obj.contains_key("args").map_err(js_err)? {
            let raw: JsValue = obj.get("args").map_err(js_err)?;
            super::conv::js_to_json(ctx, raw).map_err(js_err)?
        } else {
            serde_json::Value::Null
        };
        out.push(SequenceStep {
            id: EntityId::from_raw(id_raw),
            primitive,
            args,
        });
    }
    Ok(out)
}

fn get_required_u32_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<u32, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    if let Some(i) = raw.as_int() {
        if i < 0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!("'{field}' must be a non-negative integer"),
            });
        }
        return Ok(i as u32);
    }
    // Entity IDs are safe as f64: they use `index << 16 | generation`, keeping
    // the high bits clear and well within the 2^53 integer-exact range of f64.
    if let Some(f) = raw.as_float() {
        if !f.is_finite() || f < 0.0 || f > u32::MAX as f64 || f.fract() != 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!("'{field}' must be an integer in u32 range"),
            });
        }
        return Ok(f as u32);
    }
    Err(DescriptorError::InvalidShape {
        reason: format!("'{field}' must be a number"),
    })
}

/// Deserialize an entity-type descriptor from a JS object. Shape:
/// `{ canonicalName?: string, defaultWeapon?: string, components?: { light?: LightDescriptor, emitter?: BillboardEmitterComponent, movement?: PlayerMovementDescriptor, weapon?: WeaponDescriptor } }`.
/// Component sub-objects parse via `serde_json` after a recursive walk through
/// the existing `js_to_json` helper — matches how `LightAnimation` /
/// `BillboardEmitterComponent` cross the FFI elsewhere.
///
/// `canonicalName` is optional; absence means the descriptor has no direct
/// map-placement form (see `EntityTypeDescriptor`).
pub(crate) fn entity_descriptor_from_js<'js>(
    ctx: &Ctx<'js>,
    value: JsValue<'js>,
) -> Result<EntityTypeDescriptor, DescriptorError> {
    let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "entity entry must be an object".to_string(),
    })?;
    let canonical_name = if obj.contains_key("canonicalName").map_err(js_err)? {
        let raw: JsValue = obj.get("canonicalName").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            Some(String::from_js_value_required(raw, "canonicalName")?)
        }
    } else {
        None
    };
    let default_weapon = if obj.contains_key("defaultWeapon").map_err(js_err)? {
        let raw: JsValue = obj.get("defaultWeapon").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            Some(String::from_js_value_required(raw, "defaultWeapon")?)
        }
    } else {
        None
    };

    let mut light = None;
    let mut emitter = None;
    let mut movement = None;
    let mut weapon = None;
    let mut mesh = None;
    let mut health = None;

    if obj.contains_key("components").map_err(js_err)? {
        let components_val: JsValue = obj.get("components").map_err(js_err)?;
        if !components_val.is_null() && !components_val.is_undefined() {
            let components_obj =
                Object::from_value(components_val).map_err(|_| DescriptorError::InvalidShape {
                    reason: "`components` must be an object".to_string(),
                })?;
            if components_obj.contains_key("mesh").map_err(js_err)? {
                let raw: JsValue = components_obj.get("mesh").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let mesh_obj =
                        Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                            reason: "`components.mesh` must be an object".to_string(),
                        })?;
                    mesh = Some(mesh_descriptor_from_js(&mesh_obj)?);
                }
            }
            if components_obj.contains_key("movement").map_err(js_err)? {
                let raw: JsValue = components_obj.get("movement").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let m_obj =
                        Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                            reason: "`components.movement` must be an object".to_string(),
                        })?;
                    movement = Some(movement_descriptor_from_js(ctx, &m_obj)?);
                }
            }
            if components_obj.contains_key("weapon").map_err(js_err)? {
                let raw: JsValue = components_obj.get("weapon").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let json = super::conv::js_to_json(ctx, raw).map_err(js_err)?;
                    let descriptor: WeaponDescriptor =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.weapon` invalid: {e}"),
                            }
                        })?;
                    weapon = Some(descriptor.validate()?);
                }
            }
            if components_obj.contains_key("health").map_err(js_err)? {
                let raw: JsValue = components_obj.get("health").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let json = super::conv::js_to_json(ctx, raw).map_err(js_err)?;
                    let descriptor: HealthDescriptor =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.health` invalid: {e}"),
                            }
                        })?;
                    health = Some(descriptor.validate()?);
                }
            }
            if components_obj.contains_key("light").map_err(js_err)? {
                let raw: JsValue = components_obj.get("light").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let json = super::conv::js_to_json(ctx, raw).map_err(js_err)?;
                    let descriptor: LightDescriptor =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.light` invalid: {e}"),
                            }
                        })?;
                    light = Some(descriptor.validate()?);
                }
            }
            if components_obj.contains_key("emitter").map_err(js_err)? {
                let raw: JsValue = components_obj.get("emitter").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let json = super::conv::js_to_json(ctx, raw).map_err(js_err)?;
                    let lit: BillboardEmitterComponentLit =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.emitter` invalid: {e}"),
                            }
                        })?;
                    let validated =
                        lit.validate_into()
                            .map_err(|e| DescriptorError::InvalidShape {
                                reason: format!("`components.emitter` invalid: {e}"),
                            })?;
                    emitter = Some(validated);
                }
            }
        }
    }

    Ok(EntityTypeDescriptor {
        canonical_name,
        default_weapon,
        light,
        emitter,
        movement,
        weapon,
        mesh,
        health,
    })
}

/// Parse a `components.mesh` object (JS). Shape:
/// `{ model: string, animations?: { [state]: { clip, loop?, crossfadeMs?, interrupt? } }, defaultState?: string }`.
/// Gathers raw fields and delegates validation to [`MeshDescriptor::build`] so
/// both FFI paths share identical rules.
fn mesh_descriptor_from_js<'js>(obj: &Object<'js>) -> Result<MeshDescriptor, DescriptorError> {
    let model = get_required_string_js(obj, "model")?;

    let mut animations_present = false;
    let mut states = Vec::new();
    if obj.contains_key("animations").map_err(js_err)? {
        let raw: JsValue = obj.get("animations").map_err(js_err)?;
        if !raw.is_null() && !raw.is_undefined() {
            animations_present = true;
            let anim_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`components.mesh.animations` must be an object".to_string(),
            })?;
            // Iterate the map's own (name → state-object) entries.
            for entry in anim_obj.props::<String, JsValue>() {
                let (name, value) = entry.map_err(js_err)?;
                let state_obj =
                    Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
                        reason: format!("`components.mesh.animations.{name}` must be an object"),
                    })?;
                states.push(raw_animation_state_from_js(&name, &state_obj)?);
            }
        }
    }

    let default_state = if obj.contains_key("defaultState").map_err(js_err)? {
        let raw: JsValue = obj.get("defaultState").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            Some(String::from_js_value_required(raw, "defaultState")?)
        }
    } else {
        None
    };

    MeshDescriptor::build(model, states, default_state, animations_present)
}

/// Gather one animation-state entry from a JS object. `loop` defaults to
/// `false`, `crossfadeMs` to [`super::components::mesh::DEFAULT_CROSSFADE_MS`],
/// `interrupt` is read raw (absent ⇒ `None`). Validation is deferred to
/// [`MeshDescriptor::build`].
fn raw_animation_state_from_js<'js>(
    name: &str,
    obj: &Object<'js>,
) -> Result<RawAnimationState, DescriptorError> {
    let clip = get_required_string_js(obj, "clip")?;
    let looping = get_optional_bool_js(obj, "loop")?.unwrap_or(false);
    let crossfade_ms = get_optional_f32_js(obj, "crossfadeMs")?
        .unwrap_or(super::components::mesh::DEFAULT_CROSSFADE_MS);
    let interrupt = if obj.contains_key("interrupt").map_err(js_err)? {
        let raw: JsValue = obj.get("interrupt").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            Some(String::from_js_value_required(raw, "interrupt")?)
        }
    } else {
        None
    };
    Ok(RawAnimationState {
        name: name.to_string(),
        clip,
        looping,
        crossfade_ms,
        interrupt,
    })
}

fn movement_descriptor_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<PlayerMovementDescriptor, DescriptorError> {
    let capsule_obj: Object = get_required_object_js(obj, "capsule")?;
    let radius = validate_positive_finite(
        get_required_f32_js(&capsule_obj, "radius")?,
        "movement.capsule.radius",
    )?;
    let half_height = validate_positive_finite(
        get_required_f32_js(&capsule_obj, "halfHeight")?,
        "movement.capsule.halfHeight",
    )?;
    let eye_height = validate_in_range_finite_exclusive_min(
        get_required_f32_js(&capsule_obj, "eyeHeight")?,
        0.0,
        half_height + radius,
        "movement.capsule.eyeHeight",
    )?;
    let capsule = CapsuleParams {
        radius,
        half_height,
        eye_height,
    };

    let ground_obj: Object = get_required_object_js(obj, "ground")?;
    let speed_obj: Object = get_required_object_js(&ground_obj, "speed")?;
    let speed = SpeedParams {
        walk: validate_non_negative_finite(
            get_required_f32_js(&speed_obj, "walk")?,
            "movement.ground.speed.walk",
        )?,
        run: validate_non_negative_finite(
            get_required_f32_js(&speed_obj, "run")?,
            "movement.ground.speed.run",
        )?,
        crouch: validate_non_negative_finite(
            get_required_f32_js(&speed_obj, "crouch")?,
            "movement.ground.speed.crouch",
        )?,
    };
    let ground = GroundParams {
        speed,
        accel: validate_non_negative_finite(
            get_required_f32_js(&ground_obj, "accel")?,
            "movement.ground.accel",
        )?,
        step_height: validate_non_negative_finite(
            get_required_f32_js(&ground_obj, "stepHeight")?,
            "movement.ground.stepHeight",
        )?,
        max_slope: validate_in_range_finite(
            get_required_f32_js(&ground_obj, "maxSlope")?,
            0.0,
            90.0,
            "movement.ground.maxSlope",
        )?,
    };

    let air_obj: Object = get_required_object_js(obj, "air")?;
    let jumps_value = get_required_f32_js(&air_obj, "jumps")?;
    if !jumps_value.is_finite() || jumps_value < 0.0 || jumps_value.fract() != 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`movement.air.jumps` must be a non-negative integer, got {jumps_value}"
            ),
        });
    }
    let jumps = jumps_value as u32;
    let jump_ceiling_present = air_obj.contains_key("jumpCeiling").map_err(js_err)?;
    if jumps > 0 && !jump_ceiling_present {
        return Err(DescriptorError::MissingField {
            field: "jumpCeiling",
        });
    }
    let air = AirParams {
        forward_steer: validate_in_range_finite(
            get_required_f32_js(&air_obj, "forwardSteer")?,
            0.0,
            1.0,
            "movement.air.forwardSteer",
        )?,
        accel: validate_non_negative_finite(
            get_required_f32_js(&air_obj, "accel")?,
            "movement.air.accel",
        )?,
        max_control_speed: validate_non_negative_finite(
            get_required_f32_js(&air_obj, "maxControlSpeed")?,
            "movement.air.maxControlSpeed",
        )?,
        bunny_hop: get_required_bool_js(&air_obj, "bunnyHop")?,
        jumps,
        jump_velocity: validate_non_negative_finite(
            get_required_f32_js(&air_obj, "jumpVelocity")?,
            "movement.air.jumpVelocity",
        )?,
        jump_ceiling: if jumps > 0 || jump_ceiling_present {
            get_required_f32_js(&air_obj, "jumpCeiling")?
        } else {
            0.0
        },
    };

    let fall_obj: Object = get_required_object_js(obj, "fall")?;
    let fall = FallParams {
        terminal_velocity: validate_positive_finite(
            get_required_f32_js(&fall_obj, "terminalVelocity")?,
            "movement.fall.terminalVelocity",
        )?,
    };

    // Stuck-stop deadzone fields are optional at the wire layer: omitting
    // them yields the canonical defaults so descriptors authored before the
    // deadzone shipped continue to parse.
    let stuck_stop_enabled = get_optional_bool_js(obj, "stuckStopEnabled")?
        .unwrap_or(PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED);
    let stuck_stop_threshold = match get_optional_f32_js(obj, "stuckStopThreshold")? {
        Some(v) => validate_non_negative_finite(v, "movement.stuckStopThreshold")?,
        None => PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
    };

    // `dash` is optional: absence disables dash. When present, every field is
    // required and validated, matching the ground/air/fall discipline.
    let dash = if obj.contains_key("dash").map_err(js_err)? {
        let raw: JsValue = obj.get("dash").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let dash_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`movement.dash` must be an object".to_string(),
            })?;
            Some(dash_params_from_js(ctx, &dash_obj)?)
        }
    } else {
        None
    };

    // `forgiveness` is an optional sub-object: absence applies the engine
    // defaults at materialization. When present, each field is itself optional
    // and falls back to its engine default; an explicit 0 disables that grace.
    let forgiveness = if obj.contains_key("forgiveness").map_err(js_err)? {
        let raw: JsValue = obj.get("forgiveness").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let f_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`movement.forgiveness` must be an object".to_string(),
            })?;
            Some(forgiveness_params_from_js(&f_obj)?)
        }
    } else {
        None
    };

    // `crouch` is optional: absence disables crouch. When present, every field
    // is required and validated. The crouched `eyeHeight` bound is computed
    // against the crouched capsule extent (`crouch.halfHeight + capsule.radius`).
    let crouch = if obj.contains_key("crouch").map_err(js_err)? {
        let raw: JsValue = obj.get("crouch").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let crouch_obj =
                Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                    reason: "`movement.crouch` must be an object".to_string(),
                })?;
            Some(crouch_params_from_js(&crouch_obj, radius)?)
        }
    } else {
        None
    };

    // `viewFeel` is optional: absence disables view feel. When present, each of
    // `bob`/`tilt`/`sway` is independently optional; an absent sub-object
    // disables that motion. Within a present sub-object, all tuning fields are
    // required except the optional `groundedOnly` gate (two-level
    // present-then-all-required, mirroring the `dash`/`crouch` discipline).
    let view_feel = if obj.contains_key("viewFeel").map_err(js_err)? {
        let raw: JsValue = obj.get("viewFeel").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let vf_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`movement.viewFeel` must be an object".to_string(),
            })?;
            Some(view_feel_params_from_js(&vf_obj)?)
        }
    } else {
        None
    };

    Ok(PlayerMovementDescriptor {
        capsule,
        ground,
        air,
        fall,
        stuck_stop_enabled,
        stuck_stop_threshold,
        dash,
        forgiveness,
        crouch,
        view_feel,
    })
}

fn view_feel_params_from_js<'js>(obj: &Object<'js>) -> Result<ViewFeelParams, DescriptorError> {
    let bob = if obj.contains_key("bob").map_err(js_err)? {
        let raw: JsValue = obj.get("bob").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let bob_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`movement.viewFeel.bob` must be an object".to_string(),
            })?;
            Some(bob_params_from_js(&bob_obj)?)
        }
    } else {
        None
    };
    let tilt = if obj.contains_key("tilt").map_err(js_err)? {
        let raw: JsValue = obj.get("tilt").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let tilt_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`movement.viewFeel.tilt` must be an object".to_string(),
            })?;
            Some(tilt_params_from_js(&tilt_obj)?)
        }
    } else {
        None
    };
    let sway = if obj.contains_key("sway").map_err(js_err)? {
        let raw: JsValue = obj.get("sway").map_err(js_err)?;
        if raw.is_null() || raw.is_undefined() {
            None
        } else {
            let sway_obj = Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                reason: "`movement.viewFeel.sway` must be an object".to_string(),
            })?;
            Some(sway_params_from_js(&sway_obj)?)
        }
    } else {
        None
    };
    Ok(ViewFeelParams { bob, tilt, sway })
}

fn bob_params_from_js<'js>(obj: &Object<'js>) -> Result<BobParams, DescriptorError> {
    let vertical_frequency = validate_positive_finite(
        get_required_f32_js(obj, "verticalFrequency")?,
        "movement.viewFeel.bob.verticalFrequency",
    )?;
    let lateral_frequency = validate_positive_finite(
        get_required_f32_js(obj, "lateralFrequency")?,
        "movement.viewFeel.bob.lateralFrequency",
    )?;
    let vertical_amplitude = validate_non_negative_finite(
        get_required_f32_js(obj, "verticalAmplitude")?,
        "movement.viewFeel.bob.verticalAmplitude",
    )?;
    let lateral_amplitude = validate_non_negative_finite(
        get_required_f32_js(obj, "lateralAmplitude")?,
        "movement.viewFeel.bob.lateralAmplitude",
    )?;
    let speed_threshold = validate_non_negative_finite(
        get_required_f32_js(obj, "speedThreshold")?,
        "movement.viewFeel.bob.speedThreshold",
    )?;
    let grounded_only =
        get_optional_bool_js(obj, "groundedOnly")?.unwrap_or(BobParams::DEFAULT_GROUNDED_ONLY);
    Ok(BobParams {
        vertical_frequency,
        lateral_frequency,
        vertical_amplitude,
        lateral_amplitude,
        speed_threshold,
        grounded_only,
    })
}

fn tilt_params_from_js<'js>(obj: &Object<'js>) -> Result<TiltParams, DescriptorError> {
    let max_angle = validate_in_range_finite(
        get_required_f32_js(obj, "maxAngle")?,
        0.0,
        90.0,
        "movement.viewFeel.tilt.maxAngle",
    )?;
    let speed_reference = validate_positive_finite(
        get_required_f32_js(obj, "speedReference")?,
        "movement.viewFeel.tilt.speedReference",
    )?;
    let tension = validate_positive_finite(
        get_required_f32_js(obj, "tension")?,
        "movement.viewFeel.tilt.tension",
    )?;
    let grounded_only =
        get_optional_bool_js(obj, "groundedOnly")?.unwrap_or(TiltParams::DEFAULT_GROUNDED_ONLY);
    Ok(TiltParams {
        max_angle,
        speed_reference,
        tension,
        grounded_only,
    })
}

fn sway_params_from_js<'js>(obj: &Object<'js>) -> Result<SwayParams, DescriptorError> {
    let amplitude = validate_non_negative_finite(
        get_required_f32_js(obj, "amplitude")?,
        "movement.viewFeel.sway.amplitude",
    )?;
    let frequency = validate_positive_finite(
        get_required_f32_js(obj, "frequency")?,
        "movement.viewFeel.sway.frequency",
    )?;
    let speed_scale = validate_non_negative_finite(
        get_required_f32_js(obj, "speedScale")?,
        "movement.viewFeel.sway.speedScale",
    )?;
    let grounded_only =
        get_optional_bool_js(obj, "groundedOnly")?.unwrap_or(SwayParams::DEFAULT_GROUNDED_ONLY);
    Ok(SwayParams {
        amplitude,
        frequency,
        speed_scale,
        grounded_only,
    })
}

fn forgiveness_params_from_js<'js>(
    obj: &Object<'js>,
) -> Result<ForgivenessParams, DescriptorError> {
    let coyote_ms = match get_optional_f32_js(obj, "coyoteMs")? {
        Some(v) => validate_non_negative_finite(v, "movement.forgiveness.coyoteMs")?,
        None => ForgivenessParams::DEFAULT_COYOTE_MS,
    };
    let jump_buffer_ms = match get_optional_f32_js(obj, "jumpBufferMs")? {
        Some(v) => validate_non_negative_finite(v, "movement.forgiveness.jumpBufferMs")?,
        None => ForgivenessParams::DEFAULT_JUMP_BUFFER_MS,
    };
    Ok(ForgivenessParams {
        coyote_ms,
        jump_buffer_ms,
    })
}

fn dash_params_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<DashParams, DescriptorError> {
    let boost_speed =
        read_dash_number_js(ctx, obj, "boostSpeed", "movement.dash.boostSpeed", |v| {
            validate_positive_finite(v, "movement.dash.boostSpeed")
        })?;
    let momentum_retention = read_dash_number_js(
        ctx,
        obj,
        "momentumRetention",
        "movement.dash.momentumRetention",
        |v| validate_in_range_finite(v, 0.0, 1.0, "movement.dash.momentumRetention"),
    )?;
    let steer_control = read_dash_number_js(
        ctx,
        obj,
        "steerControl",
        "movement.dash.steerControl",
        |v| validate_in_range_finite(v, 0.0, 1.0, "movement.dash.steerControl"),
    )?;
    let dash_drag = read_dash_number_js(ctx, obj, "dashDrag", "movement.dash.dashDrag", |v| {
        validate_non_negative_finite(v, "movement.dash.dashDrag")
    })?;
    let cooldown_ms =
        read_dash_number_js(ctx, obj, "cooldownMs", "movement.dash.cooldownMs", |v| {
            validate_non_negative_finite(v, "movement.dash.cooldownMs")
        })?;
    let air_dashes_value = get_required_f32_js(obj, "airDashes")?;
    if !air_dashes_value.is_finite() || air_dashes_value < 0.0 || air_dashes_value.fract() != 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`movement.dash.airDashes` must be a non-negative integer, got {air_dashes_value}"
            ),
        });
    }
    let air_dashes = air_dashes_value as u32;
    let preserve_vertical = read_dash_bool_js(ctx, obj, "preserveVertical")?;
    Ok(DashParams {
        boost_speed,
        momentum_retention,
        steer_control,
        dash_drag,
        cooldown_ms,
        air_dashes,
        preserve_vertical,
    })
}

/// Read a dash numeric field (JS): a value that is an object converts through
/// the conv bridge to JSON, deserializes to an [`IrNode`], and is validated as a
/// `Number`-typed expression. A plain number takes the literal path with its
/// existing range validator `validate`. Absence is a [`DescriptorError::MissingField`].
fn read_dash_number_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    field: &'static str,
    path: &str,
    validate: impl FnOnce(f32) -> Result<f32, DescriptorError>,
) -> Result<NumberOrIr, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    // Arrays fall through here (is_array() excludes them) and hit the final
    // InvalidShape below. The Luau reader routes arrays into ir_node_from_json
    // instead — same InvalidShape variant, different message text by design.
    if raw.is_object() && !raw.is_array() {
        let json = super::conv::js_to_json(ctx, raw).map_err(js_err)?;
        let node = ir_node_from_json(json, path)?;
        return Ok(NumberOrIr::Ir(validate_dash_expr(
            node,
            IrType::Number,
            path,
        )?));
    }
    if let Some(i) = raw.as_int() {
        return Ok(NumberOrIr::Literal(validate(i as f32)?));
    }
    if let Some(f) = raw.as_float() {
        return Ok(NumberOrIr::Literal(validate(f as f32)?));
    }
    Err(DescriptorError::InvalidShape {
        reason: format!("'{field}' must be a number or a runtime expression"),
    })
}

/// Read a dash boolean field (JS): an object value parses as a `Bool`-typed
/// expression; a plain boolean takes the literal path. Mirror of
/// [`read_dash_number_js`] for the single boolean dash field.
fn read_dash_bool_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    field: &'static str,
) -> Result<BoolOrIr, DescriptorError> {
    let path = "movement.dash.preserveVertical";
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    if raw.is_object() && !raw.is_array() {
        let json = super::conv::js_to_json(ctx, raw).map_err(js_err)?;
        let node = ir_node_from_json(json, path)?;
        return Ok(BoolOrIr::Ir(validate_dash_expr(node, IrType::Bool, path)?));
    }
    raw.as_bool()
        .map(BoolOrIr::Literal)
        .ok_or_else(|| DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a boolean or a runtime expression"),
        })
}

/// Parse a `crouch` sub-object. `radius` is the standing capsule radius, used to
/// bound the crouched `eyeHeight` against the crouched capsule extent
/// (`half_height + radius`), mirroring how the standing `eyeHeight` is bounded.
fn crouch_params_from_js<'js>(
    obj: &Object<'js>,
    radius: f32,
) -> Result<CrouchParams, DescriptorError> {
    let half_height = validate_positive_finite(
        get_required_f32_js(obj, "halfHeight")?,
        "movement.crouch.halfHeight",
    )?;
    let eye_height = validate_in_range_finite_exclusive_min(
        get_required_f32_js(obj, "eyeHeight")?,
        0.0,
        half_height + radius,
        "movement.crouch.eyeHeight",
    )?;
    let transition_rate = validate_positive_finite(
        get_required_f32_js(obj, "transitionRate")?,
        "movement.crouch.transitionRate",
    )?;
    Ok(CrouchParams {
        half_height,
        eye_height,
        transition_rate,
    })
}

fn get_required_object_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Object<'js>, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
        reason: format!("'{field}' must be an object"),
    })
}

fn get_required_bool_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<bool, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    raw.as_bool().ok_or_else(|| DescriptorError::InvalidShape {
        reason: format!("'{field}' must be a boolean"),
    })
}

/// Validate a dash expression node at declaration: wrap it in a read-only
/// [`BakedIr`] envelope and `bind` it against [`MovementScope::for_validation`],
/// then require the bound program's root type to match the field's expected
/// type. Any `BindError` (unknown input, type-table violation) and any root-type
/// mismatch map to [`DescriptorError::InvalidShape`].
///
/// The explicit root-type check is load-bearing: `bind` with no `output` never
/// checks the root's type, so without it a bool-rooted expression in a number
/// field would silently bind and evaluate as a type-zero value.
fn validate_dash_expr(
    node: IrNode,
    expected: IrType,
    field: &str,
) -> Result<IrNode, DescriptorError> {
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: node,
    };
    let scope = MovementScope::for_validation();
    let program = bind(&baked, &scope).map_err(|e| DescriptorError::InvalidShape {
        reason: format!("`{field}` expression is invalid: {e}"),
    })?;
    if program.root_type != expected {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`{field}` expression must produce a {}, but its root produces a {}",
                ir_type_label(expected),
                ir_type_label(program.root_type)
            ),
        });
    }
    Ok(baked.root)
}

/// Deserialize a JSON value (produced from the conv bridge) into an [`IrNode`],
/// reporting a malformed node object as [`DescriptorError::InvalidShape`].
fn ir_node_from_json(value: serde_json::Value, field: &str) -> Result<IrNode, DescriptorError> {
    serde_json::from_value(value).map_err(|e| DescriptorError::InvalidShape {
        reason: format!("`{field}` is not a recognizable runtime expression: {e}"),
    })
}

fn ir_type_label(ty: IrType) -> &'static str {
    match ty {
        IrType::Number => "number",
        IrType::Bool => "boolean",
    }
}

fn validate_positive_finite(value: f32, field: &str) -> Result<f32, DescriptorError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a finite value > 0.0, got {value}"),
        });
    }
    Ok(value)
}

fn validate_non_negative_finite(value: f32, field: &str) -> Result<f32, DescriptorError> {
    if !value.is_finite() || value < 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a finite value >= 0.0, got {value}"),
        });
    }
    Ok(value)
}

fn validate_in_range_finite(
    value: f32,
    min: f32,
    max: f32,
    field: &str,
) -> Result<f32, DescriptorError> {
    if !value.is_finite() || value < min || value > max {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a finite value in [{min}, {max}], got {value}"),
        });
    }
    Ok(value)
}

/// Validate a finite value in `(min, max]` — strictly greater than `min`, at
/// most `max`. Used by `eyeHeight` which must be > 0 and at most the capsule
/// top (`half_height + radius`).
fn validate_in_range_finite_exclusive_min(
    value: f32,
    min: f32,
    max: f32,
    field: &str,
) -> Result<f32, DescriptorError> {
    if !value.is_finite() || value <= min || value > max {
        return Err(DescriptorError::InvalidShape {
            reason: format!("`{field}` must be a finite value in ({min}, {max}], got {value}"),
        });
    }
    Ok(value)
}

fn get_required_string_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<String, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    String::from_js_value_required(raw, field)
}

/// Read an optional boolean field; returns `Ok(None)` when the key is absent
/// or null/undefined, `Err` when the key is present but the value is not a
/// boolean. Used by descriptor fields that have a meaningful default.
fn get_optional_bool_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Option<bool>, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    raw.as_bool()
        .map(Some)
        .ok_or_else(|| DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a boolean"),
        })
}

/// Read an optional finite f32 field. Returns `Ok(None)` when absent/null,
/// `Err` when present but non-numeric. Numeric values are returned as-is;
/// callers are responsible for range validation.
fn get_optional_f32_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<Option<f32>, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Ok(None);
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Ok(None);
    }
    if let Some(i) = raw.as_int() {
        return Ok(Some(i as f32));
    }
    if let Some(f) = raw.as_float() {
        return Ok(Some(f as f32));
    }
    Err(DescriptorError::InvalidShape {
        reason: format!("'{field}' must be a number"),
    })
}

fn get_required_f32_js<'js>(
    obj: &Object<'js>,
    field: &'static str,
) -> Result<f32, DescriptorError> {
    if !obj.contains_key(field).map_err(js_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: JsValue = obj.get(field).map_err(js_err)?;
    if raw.is_null() || raw.is_undefined() {
        return Err(DescriptorError::MissingField { field });
    }
    if let Some(i) = raw.as_int() {
        return Ok(i as f32);
    }
    if let Some(f) = raw.as_float() {
        return Ok(f as f32);
    }
    Err(DescriptorError::InvalidShape {
        reason: format!("'{field}' must be a number"),
    })
}

// Small extension trait so the JS field readers above can coerce a `JsValue`
// into a `String` while reporting a `DescriptorError` on type mismatch.
trait FromJsValueRequired: Sized {
    fn from_js_value_required<'js>(
        value: JsValue<'js>,
        field: &'static str,
    ) -> Result<Self, DescriptorError>;
}

impl FromJsValueRequired for String {
    fn from_js_value_required<'js>(
        value: JsValue<'js>,
        field: &'static str,
    ) -> Result<Self, DescriptorError> {
        let s = value
            .as_string()
            .ok_or_else(|| DescriptorError::InvalidShape {
                reason: format!("'{field}' must be a string"),
            })?;
        s.to_string().map_err(js_err)
    }
}

// --- Lua deserialization ----------------------------------------------------

fn named_reaction_from_lua(value: LuaValue) -> Result<NamedReaction, DescriptorError> {
    let table = match value {
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("reaction entry must be a table, got {}", other.type_name()),
            });
        }
    };
    let name = get_required_string_lua(&table, "name")?;

    let has_progress = table.contains_key("progress").map_err(lua_err)?;
    let has_primitive = table.contains_key("primitive").map_err(lua_err)?;
    let has_sequence = table.contains_key("sequence").map_err(lua_err)?;

    let descriptor = if has_progress {
        let progress: Table = table.get("progress").map_err(lua_err)?;
        ReactionDescriptor::Progress(progress_descriptor_from_lua(&progress)?)
    } else if has_sequence {
        let arr: Table =
            table
                .get("sequence")
                .map_err(|e| DescriptorError::InvalidSequenceShape {
                    reason: e.to_string(),
                })?;
        ReactionDescriptor::Sequence(sequence_steps_from_lua(&arr)?)
    } else if has_primitive {
        ReactionDescriptor::Primitive(primitive_descriptor_from_lua(&table)?)
    } else {
        return Err(DescriptorError::UnknownShape);
    };

    Ok(NamedReaction { name, descriptor })
}

fn progress_descriptor_from_lua(table: &Table) -> Result<ProgressDescriptor, DescriptorError> {
    let tag = get_required_string_lua(table, "tag")?;
    let at = get_required_f32_lua(table, "at")?;
    let at = validate_at(at)?;
    let fire = get_required_string_lua(table, "fire")?;
    Ok(ProgressDescriptor { tag, at, fire })
}

/// Mirror of [`crossing_descriptor_from_js`] for Luau tables. Shape:
/// `{ slot: string, below?: number, above?: number, max?: number, fire: {string} }`.
/// Delegates validation to [`build_crossing`].
fn crossing_descriptor_from_lua(value: LuaValue) -> Result<CrossingDescriptor, DescriptorError> {
    let table = match value {
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("crossing entry must be a table, got {}", other.type_name()),
            });
        }
    };
    let slot = get_required_string_lua(&table, "slot")?;
    let below = get_optional_f32_lua(&table, "below")?;
    let above = get_optional_f32_lua(&table, "above")?;
    let max = get_optional_f32_lua(&table, "max")?;

    let fire_arr: Table = table
        .get("fire")
        .map_err(|_| DescriptorError::InvalidShape {
            reason: "crossing entry `fire` must be an array of event names".to_string(),
        })?;
    let len = fire_arr.raw_len();
    let mut fire = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let item: LuaValue = fire_arr.get(i).map_err(lua_err)?;
        match item {
            LuaValue::String(s) => fire.push(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "crossing entry `fire` elements must be strings, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    }

    build_crossing(slot, below, above, max, fire)
}

fn primitive_descriptor_from_lua(table: &Table) -> Result<PrimitiveDescriptor, DescriptorError> {
    let primitive = get_required_string_lua(table, "primitive")?;
    let primitive = validate_primitive_name(primitive)?;
    // `tag` is optional: absent ⇒ system-targeted reaction (no entities).
    let tag = if table.contains_key("tag").map_err(lua_err)? {
        let raw: LuaValue = table.get("tag").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::String(s) => Some(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("'tag' must be a string, got {}", other.type_name()),
                });
            }
        }
    } else {
        None
    };

    let on_complete = if table.contains_key("onComplete").map_err(lua_err)? {
        let raw: LuaValue = table.get("onComplete").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::String(s) => Some(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("'onComplete' must be a string, got {}", other.type_name()),
                });
            }
        }
    } else {
        None
    };

    // `args` carries the primitive's payload. Absent / nil defaults to an
    // empty object so primitives that take no arguments still deserialize.
    let args = if table.contains_key("args").map_err(lua_err)? {
        let raw: LuaValue = table.get("args").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => serde_json::Value::Object(Default::default()),
            other => super::conv::lua_to_json(other).map_err(lua_err)?,
        }
    } else {
        serde_json::Value::Object(Default::default())
    };

    Ok(PrimitiveDescriptor {
        primitive,
        tag,
        on_complete,
        args,
    })
}

fn sequence_steps_from_lua(arr: &Table) -> Result<Vec<SequenceStep>, DescriptorError> {
    let len = arr.raw_len();
    let mut out = Vec::with_capacity(len);
    for i in 1..=(len as i64) {
        let item: LuaValue = arr.get(i).map_err(lua_err)?;
        let step_table = match item {
            LuaValue::Table(t) => t,
            other => {
                return Err(DescriptorError::InvalidSequenceShape {
                    reason: format!("step {i} must be a table, got {}", other.type_name()),
                });
            }
        };
        let id_raw = get_required_u32_lua(&step_table, "id")?;
        let primitive = get_required_string_lua(&step_table, "primitive")?;
        let primitive = validate_primitive_name(primitive)?;
        let args = if step_table.contains_key("args").map_err(lua_err)? {
            let raw: LuaValue = step_table.get("args").map_err(lua_err)?;
            super::conv::lua_to_json(raw).map_err(lua_err)?
        } else {
            serde_json::Value::Null
        };
        out.push(SequenceStep {
            id: EntityId::from_raw(id_raw),
            primitive,
            args,
        });
    }
    Ok(out)
}

fn get_required_u32_lua(table: &Table, field: &'static str) -> Result<u32, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::Integer(i) => {
            if i < 0 || i > u32::MAX as i64 {
                Err(DescriptorError::InvalidShape {
                    reason: format!("'{field}' must be a non-negative integer in u32 range"),
                })
            } else {
                Ok(i as u32)
            }
        }
        LuaValue::Number(f) => {
            if !f.is_finite() || f < 0.0 || f > u32::MAX as f64 || f.fract() != 0.0 {
                Err(DescriptorError::InvalidShape {
                    reason: format!("'{field}' must be an integer in u32 range"),
                })
            } else {
                Ok(f as u32)
            }
        }
        other => Err(DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a number, got {}", other.type_name()),
        }),
    }
}

/// Mirror of [`entity_descriptor_from_js`] for Luau tables. Shape:
/// `{ canonicalName?: string, defaultWeapon?: string, components?: { light?: LightDescriptor, emitter?: BillboardEmitterComponent, movement?: PlayerMovementDescriptor, weapon?: WeaponDescriptor } }`.
///
/// `canonicalName` is optional; absence means the descriptor has no direct
/// map-placement form (see `EntityTypeDescriptor`).
pub(crate) fn entity_descriptor_from_lua(
    value: LuaValue,
) -> Result<EntityTypeDescriptor, DescriptorError> {
    let table = match value {
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("entity entry must be a table, got {}", other.type_name()),
            });
        }
    };
    let canonical_name = if table.contains_key("canonicalName").map_err(lua_err)? {
        let raw: LuaValue = table.get("canonicalName").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::String(s) => Some(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "'canonicalName' must be a string, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    } else {
        None
    };
    let default_weapon = if table.contains_key("defaultWeapon").map_err(lua_err)? {
        let raw: LuaValue = table.get("defaultWeapon").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::String(s) => Some(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "'defaultWeapon' must be a string, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    } else {
        None
    };

    let mut light = None;
    let mut emitter = None;
    let mut movement = None;
    let mut weapon = None;
    let mut mesh = None;
    let mut health = None;

    if table.contains_key("components").map_err(lua_err)? {
        let raw: LuaValue = table.get("components").map_err(lua_err)?;
        if !matches!(raw, LuaValue::Nil) {
            let components_table = match raw {
                LuaValue::Table(t) => t,
                other => {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!("`components` must be a table, got {}", other.type_name()),
                    });
                }
            };
            if components_table.contains_key("mesh").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("mesh").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    let mesh_table = match raw {
                        LuaValue::Table(t) => t,
                        other => {
                            return Err(DescriptorError::InvalidShape {
                                reason: format!(
                                    "`components.mesh` must be a table, got {}",
                                    other.type_name()
                                ),
                            });
                        }
                    };
                    mesh = Some(mesh_descriptor_from_lua(&mesh_table)?);
                }
            }
            if components_table.contains_key("movement").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("movement").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    let m_table = match raw {
                        LuaValue::Table(t) => t,
                        other => {
                            return Err(DescriptorError::InvalidShape {
                                reason: format!(
                                    "`components.movement` must be a table, got {}",
                                    other.type_name()
                                ),
                            });
                        }
                    };
                    movement = Some(movement_descriptor_from_lua(&m_table)?);
                }
            }
            if components_table.contains_key("weapon").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("weapon").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    let json = super::conv::lua_to_json(raw).map_err(lua_err)?;
                    let descriptor: WeaponDescriptor =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.weapon` invalid: {e}"),
                            }
                        })?;
                    weapon = Some(descriptor.validate()?);
                }
            }
            if components_table.contains_key("health").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("health").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    let json = super::conv::lua_to_json(raw).map_err(lua_err)?;
                    let descriptor: HealthDescriptor =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.health` invalid: {e}"),
                            }
                        })?;
                    health = Some(descriptor.validate()?);
                }
            }
            if components_table.contains_key("light").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("light").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    let json = super::conv::lua_to_json(raw).map_err(lua_err)?;
                    let descriptor: LightDescriptor =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.light` invalid: {e}"),
                            }
                        })?;
                    light = Some(descriptor.validate()?);
                }
            }
            if components_table.contains_key("emitter").map_err(lua_err)? {
                let raw: LuaValue = components_table.get("emitter").map_err(lua_err)?;
                if !matches!(raw, LuaValue::Nil) {
                    let json = super::conv::lua_to_json(raw).map_err(lua_err)?;
                    let lit: BillboardEmitterComponentLit =
                        serde_json::from_value(json).map_err(|e| {
                            DescriptorError::InvalidShape {
                                reason: format!("`components.emitter` invalid: {e}"),
                            }
                        })?;
                    let validated =
                        lit.validate_into()
                            .map_err(|e| DescriptorError::InvalidShape {
                                reason: format!("`components.emitter` invalid: {e}"),
                            })?;
                    emitter = Some(validated);
                }
            }
        }
    }

    Ok(EntityTypeDescriptor {
        canonical_name,
        default_weapon,
        light,
        emitter,
        movement,
        weapon,
        mesh,
        health,
    })
}

/// Mirror of [`mesh_descriptor_from_js`] for Luau tables. Gathers raw fields
/// and delegates validation to [`MeshDescriptor::build`].
fn mesh_descriptor_from_lua(table: &Table) -> Result<MeshDescriptor, DescriptorError> {
    let model = get_required_string_lua(table, "model")?;

    let mut animations_present = false;
    let mut states = Vec::new();
    if table.contains_key("animations").map_err(lua_err)? {
        let raw: LuaValue = table.get("animations").map_err(lua_err)?;
        if !matches!(raw, LuaValue::Nil) {
            animations_present = true;
            let anim_table = match raw {
                LuaValue::Table(t) => t,
                other => {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.mesh.animations` must be a table, got {}",
                            other.type_name()
                        ),
                    });
                }
            };
            // Iterate the map's (name → state-table) pairs.
            for pair in anim_table.pairs::<String, LuaValue>() {
                let (name, value) = pair.map_err(lua_err)?;
                let state_table = match value {
                    LuaValue::Table(t) => t,
                    other => {
                        return Err(DescriptorError::InvalidShape {
                            reason: format!(
                                "`components.mesh.animations.{name}` must be a table, got {}",
                                other.type_name()
                            ),
                        });
                    }
                };
                states.push(raw_animation_state_from_lua(&name, &state_table)?);
            }
        }
    }

    let default_state = if table.contains_key("defaultState").map_err(lua_err)? {
        let raw: LuaValue = table.get("defaultState").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::String(s) => Some(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("'defaultState' must be a string, got {}", other.type_name()),
                });
            }
        }
    } else {
        None
    };

    MeshDescriptor::build(model, states, default_state, animations_present)
}

/// Gather one animation-state entry from a Luau table. Mirrors
/// [`raw_animation_state_from_js`]: `loop` defaults to `false`, `crossfadeMs`
/// to [`super::components::mesh::DEFAULT_CROSSFADE_MS`], `interrupt` read raw.
fn raw_animation_state_from_lua(
    name: &str,
    table: &Table,
) -> Result<RawAnimationState, DescriptorError> {
    let clip = get_required_string_lua(table, "clip")?;
    let looping = get_optional_bool_lua(table, "loop")?.unwrap_or(false);
    let crossfade_ms = get_optional_f32_lua(table, "crossfadeMs")?
        .unwrap_or(super::components::mesh::DEFAULT_CROSSFADE_MS);
    let interrupt = if table.contains_key("interrupt").map_err(lua_err)? {
        let raw: LuaValue = table.get("interrupt").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::String(s) => Some(s.to_str().map_err(lua_err)?.to_string()),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("'interrupt' must be a string, got {}", other.type_name()),
                });
            }
        }
    } else {
        None
    };
    Ok(RawAnimationState {
        name: name.to_string(),
        clip,
        looping,
        crossfade_ms,
        interrupt,
    })
}

fn movement_descriptor_from_lua(
    table: &Table,
) -> Result<PlayerMovementDescriptor, DescriptorError> {
    let capsule_table = get_required_table_lua(table, "capsule")?;
    let radius = validate_positive_finite(
        get_required_f32_lua(&capsule_table, "radius")?,
        "movement.capsule.radius",
    )?;
    let half_height = validate_positive_finite(
        get_required_f32_lua(&capsule_table, "halfHeight")?,
        "movement.capsule.halfHeight",
    )?;
    let eye_height = validate_in_range_finite_exclusive_min(
        get_required_f32_lua(&capsule_table, "eyeHeight")?,
        0.0,
        half_height + radius,
        "movement.capsule.eyeHeight",
    )?;
    let capsule = CapsuleParams {
        radius,
        half_height,
        eye_height,
    };

    let ground_table = get_required_table_lua(table, "ground")?;
    let speed_table = get_required_table_lua(&ground_table, "speed")?;
    let speed = SpeedParams {
        walk: validate_non_negative_finite(
            get_required_f32_lua(&speed_table, "walk")?,
            "movement.ground.speed.walk",
        )?,
        run: validate_non_negative_finite(
            get_required_f32_lua(&speed_table, "run")?,
            "movement.ground.speed.run",
        )?,
        crouch: validate_non_negative_finite(
            get_required_f32_lua(&speed_table, "crouch")?,
            "movement.ground.speed.crouch",
        )?,
    };
    let ground = GroundParams {
        speed,
        accel: validate_non_negative_finite(
            get_required_f32_lua(&ground_table, "accel")?,
            "movement.ground.accel",
        )?,
        step_height: validate_non_negative_finite(
            get_required_f32_lua(&ground_table, "stepHeight")?,
            "movement.ground.stepHeight",
        )?,
        max_slope: validate_in_range_finite(
            get_required_f32_lua(&ground_table, "maxSlope")?,
            0.0,
            90.0,
            "movement.ground.maxSlope",
        )?,
    };

    let air_table = get_required_table_lua(table, "air")?;
    let jumps = get_required_u32_lua(&air_table, "jumps")?;
    let jump_ceiling_present = air_table.contains_key("jumpCeiling").map_err(lua_err)?;
    if jumps > 0 && !jump_ceiling_present {
        return Err(DescriptorError::MissingField {
            field: "jumpCeiling",
        });
    }
    let air = AirParams {
        forward_steer: validate_in_range_finite(
            get_required_f32_lua(&air_table, "forwardSteer")?,
            0.0,
            1.0,
            "movement.air.forwardSteer",
        )?,
        accel: validate_non_negative_finite(
            get_required_f32_lua(&air_table, "accel")?,
            "movement.air.accel",
        )?,
        max_control_speed: validate_non_negative_finite(
            get_required_f32_lua(&air_table, "maxControlSpeed")?,
            "movement.air.maxControlSpeed",
        )?,
        bunny_hop: get_required_bool_lua(&air_table, "bunnyHop")?,
        jumps,
        jump_velocity: validate_non_negative_finite(
            get_required_f32_lua(&air_table, "jumpVelocity")?,
            "movement.air.jumpVelocity",
        )?,
        jump_ceiling: if jumps > 0 || jump_ceiling_present {
            get_required_f32_lua(&air_table, "jumpCeiling")?
        } else {
            0.0
        },
    };

    let fall_table = get_required_table_lua(table, "fall")?;
    let fall = FallParams {
        terminal_velocity: validate_positive_finite(
            get_required_f32_lua(&fall_table, "terminalVelocity")?,
            "movement.fall.terminalVelocity",
        )?,
    };

    // Optional at the wire layer; see JS parser for rationale.
    let stuck_stop_enabled = get_optional_bool_lua(table, "stuckStopEnabled")?
        .unwrap_or(PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED);
    let stuck_stop_threshold = match get_optional_f32_lua(table, "stuckStopThreshold")? {
        Some(v) => validate_non_negative_finite(v, "movement.stuckStopThreshold")?,
        None => PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
    };

    // `dash` is optional: absence disables dash. When present, every field is
    // required and validated, mirroring the JS path.
    let dash = if table.contains_key("dash").map_err(lua_err)? {
        let raw: LuaValue = table.get("dash").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::Table(t) => Some(dash_params_from_lua(&t)?),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!("`movement.dash` must be a table, got {}", other.type_name()),
                });
            }
        }
    } else {
        None
    };

    // `forgiveness` is an optional sub-object; mirrors the JS path (absent →
    // engine defaults; each field optional with a 0-disables semantic).
    let forgiveness = if table.contains_key("forgiveness").map_err(lua_err)? {
        let raw: LuaValue = table.get("forgiveness").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::Table(t) => Some(forgiveness_params_from_lua(&t)?),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`movement.forgiveness` must be a table, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    } else {
        None
    };

    // `crouch` is optional: absence disables crouch. When present, every field
    // is required and validated, mirroring the JS path. The crouched `eyeHeight`
    // bound is computed against the crouched capsule extent
    // (`crouch.halfHeight + capsule.radius`).
    let crouch = if table.contains_key("crouch").map_err(lua_err)? {
        let raw: LuaValue = table.get("crouch").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::Table(t) => Some(crouch_params_from_lua(&t, radius)?),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`movement.crouch` must be a table, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    } else {
        None
    };

    // `viewFeel` is optional: absence disables view feel. Mirrors the JS path —
    // two-level present-then-all-required across `bob`/`tilt`/`sway`.
    let view_feel = if table.contains_key("viewFeel").map_err(lua_err)? {
        let raw: LuaValue = table.get("viewFeel").map_err(lua_err)?;
        match raw {
            LuaValue::Nil => None,
            LuaValue::Table(t) => Some(view_feel_params_from_lua(&t)?),
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`movement.viewFeel` must be a table, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    } else {
        None
    };

    Ok(PlayerMovementDescriptor {
        capsule,
        ground,
        air,
        fall,
        stuck_stop_enabled,
        stuck_stop_threshold,
        dash,
        forgiveness,
        crouch,
        view_feel,
    })
}

fn view_feel_params_from_lua(table: &Table) -> Result<ViewFeelParams, DescriptorError> {
    let bob = match read_optional_subtable_lua(table, "bob", "movement.viewFeel.bob")? {
        Some(t) => Some(bob_params_from_lua(&t)?),
        None => None,
    };
    let tilt = match read_optional_subtable_lua(table, "tilt", "movement.viewFeel.tilt")? {
        Some(t) => Some(tilt_params_from_lua(&t)?),
        None => None,
    };
    let sway = match read_optional_subtable_lua(table, "sway", "movement.viewFeel.sway")? {
        Some(t) => Some(sway_params_from_lua(&t)?),
        None => None,
    };
    Ok(ViewFeelParams { bob, tilt, sway })
}

/// Read an optional sub-table from a Luau table: absent/nil → `None`, a table →
/// `Some(table)`, any other type → an `InvalidShape` error keyed by `path`.
fn read_optional_subtable_lua(
    table: &Table,
    field: &'static str,
    path: &str,
) -> Result<Option<Table>, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Ok(None);
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(None),
        LuaValue::Table(t) => Ok(Some(t)),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("`{path}` must be a table, got {}", other.type_name()),
        }),
    }
}

fn bob_params_from_lua(table: &Table) -> Result<BobParams, DescriptorError> {
    let vertical_frequency = validate_positive_finite(
        get_required_f32_lua(table, "verticalFrequency")?,
        "movement.viewFeel.bob.verticalFrequency",
    )?;
    let lateral_frequency = validate_positive_finite(
        get_required_f32_lua(table, "lateralFrequency")?,
        "movement.viewFeel.bob.lateralFrequency",
    )?;
    let vertical_amplitude = validate_non_negative_finite(
        get_required_f32_lua(table, "verticalAmplitude")?,
        "movement.viewFeel.bob.verticalAmplitude",
    )?;
    let lateral_amplitude = validate_non_negative_finite(
        get_required_f32_lua(table, "lateralAmplitude")?,
        "movement.viewFeel.bob.lateralAmplitude",
    )?;
    let speed_threshold = validate_non_negative_finite(
        get_required_f32_lua(table, "speedThreshold")?,
        "movement.viewFeel.bob.speedThreshold",
    )?;
    let grounded_only =
        get_optional_bool_lua(table, "groundedOnly")?.unwrap_or(BobParams::DEFAULT_GROUNDED_ONLY);
    Ok(BobParams {
        vertical_frequency,
        lateral_frequency,
        vertical_amplitude,
        lateral_amplitude,
        speed_threshold,
        grounded_only,
    })
}

fn tilt_params_from_lua(table: &Table) -> Result<TiltParams, DescriptorError> {
    let max_angle = validate_in_range_finite(
        get_required_f32_lua(table, "maxAngle")?,
        0.0,
        90.0,
        "movement.viewFeel.tilt.maxAngle",
    )?;
    let speed_reference = validate_positive_finite(
        get_required_f32_lua(table, "speedReference")?,
        "movement.viewFeel.tilt.speedReference",
    )?;
    let tension = validate_positive_finite(
        get_required_f32_lua(table, "tension")?,
        "movement.viewFeel.tilt.tension",
    )?;
    let grounded_only =
        get_optional_bool_lua(table, "groundedOnly")?.unwrap_or(TiltParams::DEFAULT_GROUNDED_ONLY);
    Ok(TiltParams {
        max_angle,
        speed_reference,
        tension,
        grounded_only,
    })
}

fn sway_params_from_lua(table: &Table) -> Result<SwayParams, DescriptorError> {
    let amplitude = validate_non_negative_finite(
        get_required_f32_lua(table, "amplitude")?,
        "movement.viewFeel.sway.amplitude",
    )?;
    let frequency = validate_positive_finite(
        get_required_f32_lua(table, "frequency")?,
        "movement.viewFeel.sway.frequency",
    )?;
    let speed_scale = validate_non_negative_finite(
        get_required_f32_lua(table, "speedScale")?,
        "movement.viewFeel.sway.speedScale",
    )?;
    let grounded_only =
        get_optional_bool_lua(table, "groundedOnly")?.unwrap_or(SwayParams::DEFAULT_GROUNDED_ONLY);
    Ok(SwayParams {
        amplitude,
        frequency,
        speed_scale,
        grounded_only,
    })
}

fn forgiveness_params_from_lua(table: &Table) -> Result<ForgivenessParams, DescriptorError> {
    let coyote_ms = match get_optional_f32_lua(table, "coyoteMs")? {
        Some(v) => validate_non_negative_finite(v, "movement.forgiveness.coyoteMs")?,
        None => ForgivenessParams::DEFAULT_COYOTE_MS,
    };
    let jump_buffer_ms = match get_optional_f32_lua(table, "jumpBufferMs")? {
        Some(v) => validate_non_negative_finite(v, "movement.forgiveness.jumpBufferMs")?,
        None => ForgivenessParams::DEFAULT_JUMP_BUFFER_MS,
    };
    Ok(ForgivenessParams {
        coyote_ms,
        jump_buffer_ms,
    })
}

fn dash_params_from_lua(table: &Table) -> Result<DashParams, DescriptorError> {
    let boost_speed = read_dash_number_lua(table, "boostSpeed", "movement.dash.boostSpeed", |v| {
        validate_positive_finite(v, "movement.dash.boostSpeed")
    })?;
    let momentum_retention = read_dash_number_lua(
        table,
        "momentumRetention",
        "movement.dash.momentumRetention",
        |v| validate_in_range_finite(v, 0.0, 1.0, "movement.dash.momentumRetention"),
    )?;
    let steer_control =
        read_dash_number_lua(table, "steerControl", "movement.dash.steerControl", |v| {
            validate_in_range_finite(v, 0.0, 1.0, "movement.dash.steerControl")
        })?;
    let dash_drag = read_dash_number_lua(table, "dashDrag", "movement.dash.dashDrag", |v| {
        validate_non_negative_finite(v, "movement.dash.dashDrag")
    })?;
    let cooldown_ms = read_dash_number_lua(table, "cooldownMs", "movement.dash.cooldownMs", |v| {
        validate_non_negative_finite(v, "movement.dash.cooldownMs")
    })?;
    let air_dashes = get_required_u32_lua(table, "airDashes")?;
    let preserve_vertical = read_dash_bool_lua(table, "preserveVertical")?;
    Ok(DashParams {
        boost_speed,
        momentum_retention,
        steer_control,
        dash_drag,
        cooldown_ms,
        air_dashes,
        preserve_vertical,
    })
}

/// Read a dash numeric field (Luau): a table value converts through the conv
/// bridge to JSON, deserializes to an [`IrNode`], and is validated as a
/// `Number`-typed expression; a plain number takes the literal path with its
/// existing range validator. Mirror of [`read_dash_number_js`] — the
/// missing-Luau-arm parity trap is avoided by keeping the two symmetric.
fn read_dash_number_lua(
    table: &Table,
    field: &'static str,
    path: &str,
    validate: impl FnOnce(f32) -> Result<f32, DescriptorError>,
) -> Result<NumberOrIr, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::Integer(i) => Ok(NumberOrIr::Literal(validate(i as f32)?)),
        LuaValue::Number(f) => Ok(NumberOrIr::Literal(validate(f as f32)?)),
        LuaValue::Table(_) => {
            let json = super::conv::lua_to_json(raw).map_err(lua_err)?;
            let node = ir_node_from_json(json, path)?;
            Ok(NumberOrIr::Ir(validate_dash_expr(
                node,
                IrType::Number,
                path,
            )?))
        }
        other => Err(DescriptorError::InvalidShape {
            reason: format!(
                "'{field}' must be a number or a runtime expression, got {}",
                other.type_name()
            ),
        }),
    }
}

/// Read a dash boolean field (Luau): a table value parses as a `Bool`-typed
/// expression; a plain boolean takes the literal path. Mirror of
/// [`read_dash_bool_js`].
fn read_dash_bool_lua(table: &Table, field: &'static str) -> Result<BoolOrIr, DescriptorError> {
    let path = "movement.dash.preserveVertical";
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::Boolean(b) => Ok(BoolOrIr::Literal(b)),
        LuaValue::Table(_) => {
            let json = super::conv::lua_to_json(raw).map_err(lua_err)?;
            let node = ir_node_from_json(json, path)?;
            Ok(BoolOrIr::Ir(validate_dash_expr(node, IrType::Bool, path)?))
        }
        other => Err(DescriptorError::InvalidShape {
            reason: format!(
                "'{field}' must be a boolean or a runtime expression, got {}",
                other.type_name()
            ),
        }),
    }
}

/// Mirror of [`crouch_params_from_js`] for Luau tables. `radius` is the standing
/// capsule radius, used to bound the crouched `eyeHeight` against the crouched
/// capsule extent (`half_height + radius`).
fn crouch_params_from_lua(table: &Table, radius: f32) -> Result<CrouchParams, DescriptorError> {
    let half_height = validate_positive_finite(
        get_required_f32_lua(table, "halfHeight")?,
        "movement.crouch.halfHeight",
    )?;
    let eye_height = validate_in_range_finite_exclusive_min(
        get_required_f32_lua(table, "eyeHeight")?,
        0.0,
        half_height + radius,
        "movement.crouch.eyeHeight",
    )?;
    let transition_rate = validate_positive_finite(
        get_required_f32_lua(table, "transitionRate")?,
        "movement.crouch.transitionRate",
    )?;
    Ok(CrouchParams {
        half_height,
        eye_height,
        transition_rate,
    })
}

fn get_required_table_lua(table: &Table, field: &'static str) -> Result<Table, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::Table(t) => Ok(t),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a table, got {}", other.type_name()),
        }),
    }
}

fn get_required_bool_lua(table: &Table, field: &'static str) -> Result<bool, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::Boolean(b) => Ok(b),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a boolean, got {}", other.type_name()),
        }),
    }
}

fn get_required_string_lua(table: &Table, field: &'static str) -> Result<String, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::String(s) => Ok(s.to_str().map_err(lua_err)?.to_string()),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a string, got {}", other.type_name()),
        }),
    }
}

fn get_required_f32_lua(table: &Table, field: &'static str) -> Result<f32, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Err(DescriptorError::MissingField { field });
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Err(DescriptorError::MissingField { field }),
        LuaValue::Integer(i) => Ok(i as f32),
        LuaValue::Number(f) => Ok(f as f32),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a number, got {}", other.type_name()),
        }),
    }
}

/// Optional boolean read for the Luau path: absent/nil → `None`, present
/// non-boolean → `Err`. Mirrors `get_optional_bool_js`.
fn get_optional_bool_lua(
    table: &Table,
    field: &'static str,
) -> Result<Option<bool>, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Ok(None);
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(None),
        LuaValue::Boolean(b) => Ok(Some(b)),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a boolean, got {}", other.type_name()),
        }),
    }
}

/// Optional f32 read for the Luau path: absent/nil → `None`, present
/// non-numeric → `Err`. Range validation is the caller's responsibility.
fn get_optional_f32_lua(
    table: &Table,
    field: &'static str,
) -> Result<Option<f32>, DescriptorError> {
    if !table.contains_key(field).map_err(lua_err)? {
        return Ok(None);
    }
    let raw: LuaValue = table.get(field).map_err(lua_err)?;
    match raw {
        LuaValue::Nil => Ok(None),
        LuaValue::Integer(i) => Ok(Some(i as f32)),
        LuaValue::Number(f) => Ok(Some(f as f32)),
        other => Err(DescriptorError::InvalidShape {
            reason: format!("'{field}' must be a number, got {}", other.type_name()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- JS path ------------------------------------------------------------

    fn eval_js<F, R>(src: &str, f: F) -> R
    where
        F: for<'js> FnOnce(&Ctx<'js>, JsValue<'js>) -> R,
    {
        let rt = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&rt).unwrap();
        ctx.with(|jsctx| {
            let value: JsValue = jsctx.eval(src).unwrap();
            f(&jsctx, value)
        })
    }

    #[test]
    fn js_manifest_parses_progress_and_primitive_reactions() {
        let src = r#"({
            reactions: [
                { name: "reactorWave1",
                  progress: { tag: "reactorWave1Monsters", at: 1.0, fire: "wave1Complete" } },
                { name: "wave1Complete",
                  primitive: "moveGeometry",
                  tag: "reactorChambers",
                  onComplete: "wave2Revealed" },
            ]
        })"#;
        let manifest = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());

        assert_eq!(manifest.reactions.len(), 2);
        assert_eq!(manifest.reactions[0].name, "reactorWave1");
        match &manifest.reactions[0].descriptor {
            ReactionDescriptor::Progress(p) => {
                assert_eq!(p.tag, "reactorWave1Monsters");
                assert!((p.at - 1.0).abs() < 1e-6);
                assert_eq!(p.fire, "wave1Complete");
            }
            other => panic!("expected progress, got {other:?}"),
        }
        match &manifest.reactions[1].descriptor {
            ReactionDescriptor::Primitive(p) => {
                assert_eq!(p.primitive, "moveGeometry");
                assert_eq!(p.tag.as_deref(), Some("reactorChambers"));
                assert_eq!(p.on_complete.as_deref(), Some("wave2Revealed"));
            }
            other => panic!("expected primitive, got {other:?}"),
        }
    }

    #[test]
    fn js_primitive_without_on_complete_is_none() {
        let src = r#"({
            reactions: [{ name: "x", primitive: "moveGeometry", tag: "t" }]
        })"#;
        let m = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
        match &m.reactions[0].descriptor {
            ReactionDescriptor::Primitive(p) => assert!(p.on_complete.is_none()),
            other => panic!("expected primitive, got {other:?}"),
        }
    }

    #[test]
    fn js_primitive_with_tag_parses_as_entity_targeted() {
        // An entity-targeted descriptor (with `tag`) still parses byte-identically:
        // `tag` round-trips as `Some`.
        let src = r#"({
            reactions: [{ name: "x", primitive: "setEmitterRate", tag: "smoke", args: { rate: 0.0 } }]
        })"#;
        let m = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
        match &m.reactions[0].descriptor {
            ReactionDescriptor::Primitive(p) => {
                assert_eq!(p.primitive, "setEmitterRate");
                assert_eq!(p.tag.as_deref(), Some("smoke"));
            }
            other => panic!("expected primitive, got {other:?}"),
        }
    }

    #[test]
    fn js_primitive_without_tag_is_system_targeted() {
        // A system reaction omits `tag` entirely; it parses with `tag == None`.
        let src = r#"({
            reactions: [{ name: "lowHealth", primitive: "playSound", args: { sound: "alarm" } }]
        })"#;
        let m = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
        match &m.reactions[0].descriptor {
            ReactionDescriptor::Primitive(p) => {
                assert_eq!(p.primitive, "playSound");
                assert!(p.tag.is_none());
            }
            other => panic!("expected primitive, got {other:?}"),
        }
    }

    #[test]
    fn js_missing_required_field_reports_missing_field() {
        // progress missing `fire`
        let src = r#"({
            reactions: [{ name: "x", progress: { tag: "t", at: 0.5 } }]
        })"#;
        let err = eval_js(src, |ctx, v| {
            LevelManifest::from_js_value(ctx, v).unwrap_err()
        });
        assert_eq!(err, DescriptorError::MissingField { field: "fire" });
    }

    #[test]
    fn js_missing_name_field_reports_missing_field() {
        let src = r#"({
            reactions: [{ progress: { tag: "t", at: 0.5, fire: "f" } }]
        })"#;
        let err = eval_js(src, |ctx, v| {
            LevelManifest::from_js_value(ctx, v).unwrap_err()
        });
        assert_eq!(err, DescriptorError::MissingField { field: "name" });
    }

    #[test]
    fn js_unknown_shape_reaction_is_rejected() {
        let src = r#"({
            reactions: [{ name: "x", tag: "t" }]
        })"#;
        let err = eval_js(src, |ctx, v| {
            LevelManifest::from_js_value(ctx, v).unwrap_err()
        });
        assert_eq!(err, DescriptorError::UnknownShape);
    }

    #[test]
    fn js_empty_primitive_name_is_rejected() {
        let src = r#"({
            reactions: [{ name: "x", primitive: "", tag: "t" }]
        })"#;
        let err = eval_js(src, |ctx, v| {
            LevelManifest::from_js_value(ctx, v).unwrap_err()
        });
        assert_eq!(err, DescriptorError::EmptyPrimitiveName);
    }

    #[test]
    fn js_at_out_of_range_high_is_rejected() {
        let src = r#"({
            reactions: [{ name: "x", progress: { tag: "t", at: 1.5, fire: "f" } }]
        })"#;
        let err = eval_js(src, |ctx, v| {
            LevelManifest::from_js_value(ctx, v).unwrap_err()
        });
        assert_eq!(err, DescriptorError::AtThresholdOutOfRange { value: 1.5 });
    }

    #[test]
    fn js_at_out_of_range_negative_is_rejected() {
        let src = r#"({
            reactions: [{ name: "x", progress: { tag: "t", at: -0.1, fire: "f" } }]
        })"#;
        let err = eval_js(src, |ctx, v| {
            LevelManifest::from_js_value(ctx, v).unwrap_err()
        });
        match err {
            DescriptorError::AtThresholdOutOfRange { value } => {
                assert!((value + 0.1).abs() < 1e-6);
            }
            other => panic!("expected AtThresholdOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn js_sequence_reaction_deserializes() {
        let src = r#"({
            reactions: [{
                name: "openVault",
                sequence: [
                    { id: 65536, primitive: "moveGeometry", args: { duration: 1.5 } },
                    { id: 131072, primitive: "playSound", args: { clip: "vault" } }
                ]
            }]
        })"#;
        let m = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
        match &m.reactions[0].descriptor {
            ReactionDescriptor::Sequence(steps) => {
                assert_eq!(steps.len(), 2);
                assert_eq!(steps[0].id.to_raw(), 65536);
                assert_eq!(steps[0].primitive, "moveGeometry");
                assert_eq!(steps[0].args["duration"].as_f64(), Some(1.5));
                assert_eq!(steps[1].id.to_raw(), 131072);
                assert_eq!(steps[1].primitive, "playSound");
                assert_eq!(steps[1].args["clip"], serde_json::json!("vault"));
            }
            other => panic!("expected sequence, got {other:?}"),
        }
    }

    #[test]
    fn js_sequence_step_missing_args_defaults_to_null() {
        let src = r#"({
            reactions: [{
                name: "x",
                sequence: [{ id: 1, primitive: "ping" }]
            }]
        })"#;
        let m = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
        match &m.reactions[0].descriptor {
            ReactionDescriptor::Sequence(steps) => {
                assert_eq!(steps.len(), 1);
                assert!(steps[0].args.is_null());
            }
            other => panic!("expected sequence, got {other:?}"),
        }
    }

    #[test]
    fn js_empty_arrays_yield_empty_manifest() {
        let src = "({ reactions: [] })";
        let m = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
        assert!(m.reactions.is_empty());
    }

    // --- Luau path ----------------------------------------------------------

    fn eval_lua<F, R>(src: &str, f: F) -> R
    where
        F: FnOnce(LuaValue) -> R,
    {
        let lua = mlua::Lua::new();
        let value: LuaValue = lua.load(src).eval().unwrap();
        f(value)
    }

    #[test]
    fn lua_manifest_parses_progress_and_primitive_reactions() {
        let src = r#"return {
            reactions = {
                { name = "reactorWave1",
                  progress = { tag = "reactorWave1Monsters", at = 1.0, fire = "wave1Complete" } },
                { name = "wave1Complete",
                  primitive = "moveGeometry",
                  tag = "reactorChambers",
                  onComplete = "wave2Revealed" },
            }
        }"#;
        let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());

        assert_eq!(m.reactions.len(), 2);
        match &m.reactions[0].descriptor {
            ReactionDescriptor::Progress(p) => {
                assert_eq!(p.tag, "reactorWave1Monsters");
                assert!((p.at - 1.0).abs() < 1e-6);
                assert_eq!(p.fire, "wave1Complete");
            }
            other => panic!("expected progress, got {other:?}"),
        }
        match &m.reactions[1].descriptor {
            ReactionDescriptor::Primitive(p) => {
                assert_eq!(p.primitive, "moveGeometry");
                assert_eq!(p.tag.as_deref(), Some("reactorChambers"));
                assert_eq!(p.on_complete.as_deref(), Some("wave2Revealed"));
            }
            other => panic!("expected primitive, got {other:?}"),
        }
    }

    #[test]
    fn lua_primitive_without_on_complete_is_none() {
        let src = r#"return {
            reactions = { { name = "x", primitive = "moveGeometry", tag = "t" } }
        }"#;
        let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
        match &m.reactions[0].descriptor {
            ReactionDescriptor::Primitive(p) => assert!(p.on_complete.is_none()),
            other => panic!("expected primitive, got {other:?}"),
        }
    }

    #[test]
    fn lua_primitive_with_tag_parses_as_entity_targeted() {
        let src = r#"return {
            reactions = { { name = "x", primitive = "setEmitterRate", tag = "smoke", args = { rate = 0.0 } } }
        }"#;
        let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
        match &m.reactions[0].descriptor {
            ReactionDescriptor::Primitive(p) => {
                assert_eq!(p.primitive, "setEmitterRate");
                assert_eq!(p.tag.as_deref(), Some("smoke"));
            }
            other => panic!("expected primitive, got {other:?}"),
        }
    }

    #[test]
    fn lua_primitive_without_tag_is_system_targeted() {
        let src = r#"return {
            reactions = { { name = "lowHealth", primitive = "playSound", args = { sound = "alarm" } } }
        }"#;
        let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
        match &m.reactions[0].descriptor {
            ReactionDescriptor::Primitive(p) => {
                assert_eq!(p.primitive, "playSound");
                assert!(p.tag.is_none());
            }
            other => panic!("expected primitive, got {other:?}"),
        }
    }

    #[test]
    fn lua_missing_required_field_reports_missing_field() {
        let src = r#"return {
            reactions = { { name = "x", progress = { tag = "t", at = 0.5 } } }
        }"#;
        let err = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap_err());
        assert_eq!(err, DescriptorError::MissingField { field: "fire" });
    }

    #[test]
    fn lua_unknown_shape_reaction_is_rejected() {
        let src = r#"return {
            reactions = { { name = "x", tag = "t" } }
        }"#;
        let err = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap_err());
        assert_eq!(err, DescriptorError::UnknownShape);
    }

    #[test]
    fn lua_empty_primitive_name_is_rejected() {
        let src = r#"return {
            reactions = { { name = "x", primitive = "", tag = "t" } }
        }"#;
        let err = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap_err());
        assert_eq!(err, DescriptorError::EmptyPrimitiveName);
    }

    #[test]
    fn lua_at_out_of_range_is_rejected() {
        let src = r#"return {
            reactions = { { name = "x", progress = { tag = "t", at = 1.5, fire = "f" } } }
        }"#;
        let err = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap_err());
        assert_eq!(err, DescriptorError::AtThresholdOutOfRange { value: 1.5 });
    }

    #[test]
    fn lua_sequence_reaction_deserializes() {
        let src = r#"return {
            reactions = {
                { name = "openVault",
                  sequence = {
                      { id = 65536, primitive = "moveGeometry", args = { duration = 1.5 } },
                      { id = 131072, primitive = "playSound", args = { clip = "vault" } },
                  } }
            }
        }"#;
        let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
        match &m.reactions[0].descriptor {
            ReactionDescriptor::Sequence(steps) => {
                assert_eq!(steps.len(), 2);
                assert_eq!(steps[0].id.to_raw(), 65536);
                assert_eq!(steps[0].primitive, "moveGeometry");
                assert_eq!(steps[1].primitive, "playSound");
            }
            other => panic!("expected sequence, got {other:?}"),
        }
    }

    #[test]
    fn lua_empty_arrays_yield_empty_manifest() {
        let src = "return { reactions = {} }";
        let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
        assert!(m.reactions.is_empty());
    }

    // --- EntityTypeDescriptor (passed as ModManifest.entities) ---------------

    #[test]
    fn entity_descriptor_with_emitter_only_deserializes() {
        let src = r#"({
            canonicalName: "smoke_pillar",
            components: {
                emitter: {
                    rate: 12.0,
                    burst: null,
                    spread: 0.3,
                    lifetime: 4.0,
                    velocity: [0, 1, 0],
                    buoyancy: 0.5,
                    drag: 0.5,
                    size_over_lifetime: [0.5, 1.0],
                    opacity_over_lifetime: [0.0, 1.0, 0.0],
                    color: [0.7, 0.7, 0.7],
                    sprite: "smoke",
                    spin_rate: 0.0
                }
            }
        })"#;
        let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        assert_eq!(d.canonical_name.as_deref(), Some("smoke_pillar"));
        assert!(d.light.is_none());
        let e = d.emitter.expect("emitter present");
        assert_eq!(e.rate, 12.0);
        assert_eq!(e.sprite, "smoke");
    }

    #[test]
    fn entity_descriptor_with_light_only_deserializes() {
        let src = r#"({
            canonicalName: "campfire",
            components: {
                light: {
                    color: [1.0, 0.6, 0.2],
                    intensity: 4.0,
                    range: 10.0,
                    is_dynamic: false
                }
            }
        })"#;
        let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        assert_eq!(d.canonical_name.as_deref(), Some("campfire"));
        assert!(d.emitter.is_none());
        let l = d.light.expect("light present");
        assert_eq!(l.color, [1.0, 0.6, 0.2]);
        assert_eq!(l.intensity, 4.0);
        assert_eq!(l.range, 10.0);
        assert!(!l.is_dynamic);
    }

    #[test]
    fn entity_descriptor_with_both_components_deserializes() {
        let src = r#"({
            canonicalName: "torch",
            components: {
                light: { color: [1, 1, 1], intensity: 2.0, range: 6.0, is_dynamic: true },
                emitter: {
                    rate: 4.0, burst: null, spread: 0.1, lifetime: 1.5,
                    velocity: [0, 1, 0], buoyancy: 0.3, drag: 0.4,
                    size_over_lifetime: [1.0], opacity_over_lifetime: [1.0, 0.0],
                    color: [1, 1, 1], sprite: "ember", spin_rate: 0.0
                }
            }
        })"#;
        let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        assert_eq!(d.canonical_name.as_deref(), Some("torch"));
        assert!(d.light.is_some());
        assert!(d.emitter.is_some());
    }

    #[test]
    fn entity_descriptor_without_components_field_deserializes() {
        let src = r#"({ canonicalName: "vignette" })"#;
        let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        assert_eq!(d.canonical_name.as_deref(), Some("vignette"));
        assert!(d.default_weapon.is_none());
        assert!(d.light.is_none());
        assert!(d.emitter.is_none());
        assert!(d.weapon.is_none());
    }

    #[test]
    fn js_entity_descriptor_with_default_weapon_and_weapon_component_deserializes() {
        let src = r#"({
            canonicalName: "player",
            defaultWeapon: "reference_pistol",
            components: {
                weapon: {
                    damage: 12.0,
                    range: 64.0,
                    fireRateMs: 180.0,
                    fireMode: "semi",
                    resolution: "hitscan"
                }
            }
        })"#;
        let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        assert_eq!(d.default_weapon.as_deref(), Some("reference_pistol"));
        let weapon = d.weapon.expect("weapon present");
        assert_eq!(weapon.damage, 12.0);
        assert_eq!(weapon.range, 64.0);
        assert_eq!(weapon.cooldown_ms, 180.0);
        assert_eq!(weapon.fire_mode, FireMode::Semi);
        assert_eq!(weapon.resolution, ResolutionMode::Hitscan);
    }

    #[test]
    fn js_top_level_weapon_key_is_not_a_component_alias() {
        let src = r#"({
            canonicalName: "player",
            weapon: {
                damage: 12.0,
                range: 64.0,
                fireRateMs: 180.0,
                fireMode: "semi",
                resolution: "hitscan"
            }
        })"#;
        let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        assert!(d.weapon.is_none());
    }

    #[test]
    fn entity_descriptor_with_emitter_only_deserializes_lua() {
        let src = r#"return {
            canonicalName = "smoke_pillar",
            components = {
                emitter = {
                    rate = 12.0,
                    spread = 0.3,
                    lifetime = 4.0,
                    velocity = { 0, 1, 0 },
                    buoyancy = 0.5,
                    drag = 0.5,
                    size_over_lifetime = { 0.5, 1.0 },
                    opacity_over_lifetime = { 0.0, 1.0, 0.0 },
                    color = { 0.7, 0.7, 0.7 },
                    sprite = "smoke",
                    spin_rate = 0.0,
                }
            }
        }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        assert_eq!(d.canonical_name.as_deref(), Some("smoke_pillar"));
        assert!(d.emitter.is_some());
    }

    #[test]
    fn entity_descriptor_with_light_only_deserializes_lua() {
        let src = r#"return {
            canonicalName = "campfire",
            components = {
                light = { color = { 1.0, 0.6, 0.2 }, intensity = 4.0, range = 10.0, is_dynamic = false }
            }
        }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        assert_eq!(d.canonical_name.as_deref(), Some("campfire"));
        let l = d.light.expect("light present");
        assert_eq!(l.intensity, 4.0);
    }

    #[test]
    fn lua_entity_descriptor_with_default_weapon_and_weapon_component_deserializes() {
        let src = r#"return {
            canonicalName = "player",
            defaultWeapon = "reference_pistol",
            components = {
                weapon = {
                    damage = 12.0,
                    range = 64.0,
                    fireRateMs = 180.0,
                    fireMode = "auto",
                    resolution = "hitscan",
                }
            }
        }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        assert_eq!(d.default_weapon.as_deref(), Some("reference_pistol"));
        let weapon = d.weapon.expect("weapon present");
        assert_eq!(weapon.damage, 12.0);
        assert_eq!(weapon.cooldown_ms, 180.0);
        assert_eq!(weapon.fire_mode, FireMode::Auto);
        assert_eq!(weapon.resolution, ResolutionMode::Hitscan);
    }

    // --- PlayerMovementDescriptor parsing ----------------------------------

    const JS_PLAYER_MOVEMENT: &str = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;

    #[test]
    fn js_movement_descriptor_full_shape_parses() {
        let d = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap()
        });
        let m = d.movement.expect("movement present");
        assert_eq!(m.capsule.radius, 0.4);
        assert_eq!(m.capsule.half_height, 0.8);
        assert_eq!(m.capsule.eye_height, 0.5);
        assert_eq!(m.ground.speed.walk, 7.0);
        assert_eq!(m.ground.speed.run, 11.0);
        assert_eq!(m.ground.speed.crouch, 3.0);
        assert_eq!(m.ground.max_slope, 45.0);
        assert_eq!(m.air.forward_steer, 0.0);
        assert!(!m.air.bunny_hop);
        assert_eq!(m.air.jumps, 0);
        assert_eq!(m.fall.terminal_velocity, 40.0);
    }

    #[test]
    fn js_movement_speed_missing_run_reports_missing_field() {
        // `ground.speed` is a nested { walk, run } object; both required.
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: { walk: 7.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert_eq!(err, DescriptorError::MissingField { field: "run" });
    }

    #[test]
    fn js_movement_speed_negative_run_is_rejected() {
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: { walk: 7.0, run: -1.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_speed_missing_run_reports_missing_field() {
        let src = r#"return {
            canonicalName = "player",
            components = {
                movement = {
                    capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                    ground = { speed = { walk = 7.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                    fall = { terminalVelocity = 40.0 }
                }
            }
        }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert_eq!(err, DescriptorError::MissingField { field: "run" });
    }

    #[test]
    fn js_movement_speed_missing_crouch_reports_missing_field() {
        // `ground.speed.crouch` is required when `ground.speed` is present.
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: { walk: 7.0, run: 11.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert_eq!(err, DescriptorError::MissingField { field: "crouch" });
    }

    #[test]
    fn lua_movement_speed_missing_crouch_reports_missing_field() {
        let src = r#"return {
            canonicalName = "player",
            components = {
                movement = {
                    capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                    ground = { speed = { walk = 7.0, run = 11.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                    fall = { terminalVelocity = 40.0 }
                }
            }
        }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert_eq!(err, DescriptorError::MissingField { field: "crouch" });
    }

    #[test]
    fn js_movement_speed_negative_crouch_is_rejected() {
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: -1.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_speed_negative_crouch_is_rejected() {
        let src = r#"return {
            canonicalName = "player",
            components = {
                movement = {
                    capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                    ground = { speed = { walk = 7.0, run = 11.0, crouch = -1.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                    fall = { terminalVelocity = 40.0 }
                }
            }
        }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_speed_zero_crouch_is_accepted() {
        // Zero crouch speed is legitimate (non-negative finite).
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 0.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        assert_eq!(d.movement.unwrap().ground.speed.crouch, 0.0);
    }

    #[test]
    fn js_movement_missing_air_field_reports_missing_field() {
        // `bunnyHop` removed from `air`.
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert_eq!(err, DescriptorError::MissingField { field: "bunnyHop" });
    }

    #[test]
    fn js_movement_jumps_positive_without_ceiling_errors() {
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 2, jumpVelocity: 5.5 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert_eq!(
            err,
            DescriptorError::MissingField {
                field: "jumpCeiling",
            }
        );
    }

    #[test]
    fn js_movement_capsule_radius_zero_is_rejected() {
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.0, halfHeight: 0.8 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_max_slope_out_of_range_is_rejected() {
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 95.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_forward_steer_out_of_range_is_rejected() {
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 1.5, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_terminal_velocity_zero_is_rejected() {
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 0.0 }
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_eye_height_zero_is_rejected() {
        // eye_height must be strictly positive: 0.0 sits at the capsule center,
        // not a sensible eye position.
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.0 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_eye_height_above_capsule_top_is_rejected() {
        // capsule top = half_height + radius = 1.2; eye_height = 1.5 exceeds it.
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 1.5 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_eye_height_at_capsule_top_is_accepted() {
        // Exactly at capsule top (half_height + radius = 1.2) is permitted.
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 1.2 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        assert_eq!(d.movement.unwrap().capsule.eye_height, 1.2);
    }

    #[test]
    fn js_movement_missing_eye_height_reports_missing_field() {
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert_eq!(err, DescriptorError::MissingField { field: "eyeHeight" });
    }

    #[test]
    fn lua_movement_descriptor_full_shape_parses() {
        let src = r#"return {
            canonicalName = "player",
            components = {
                movement = {
                    capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                    ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                    fall = { terminalVelocity = 40.0 }
                }
            }
        }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        let m = d.movement.expect("movement present");
        assert_eq!(m.capsule.eye_height, 0.5);
        assert_eq!(m.air.jump_velocity, 5.5);
        assert_eq!(m.air.jumps, 0);
        assert_eq!(m.fall.terminal_velocity, 40.0);
    }

    #[test]
    fn lua_movement_eye_height_above_capsule_top_is_rejected() {
        let src = r#"return {
            canonicalName = "player",
            components = {
                movement = {
                    capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 1.5 },
                    ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                    fall = { terminalVelocity = 40.0 }
                }
            }
        }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_jumps_positive_without_ceiling_errors() {
        let src = r#"return {
            canonicalName = "player",
            components = {
                movement = {
                    capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                    ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 2, jumpVelocity = 5.5 },
                    fall = { terminalVelocity = 40.0 }
                }
            }
        }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert_eq!(
            err,
            DescriptorError::MissingField {
                field: "jumpCeiling",
            }
        );
    }

    #[test]
    fn lua_movement_missing_capsule_block_reports_missing_field() {
        let src = r#"return {
            canonicalName = "player",
            components = {
                movement = {
                    ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                    fall = { terminalVelocity = 40.0 }
                }
            }
        }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert_eq!(err, DescriptorError::MissingField { field: "capsule" });
    }

    #[test]
    fn js_movement_jumps_zero_without_ceiling_is_valid() {
        // `jumpCeiling` is only meaningful when `air.jumps > 0`; omitting it
        // when jumps == 0 should succeed with jump_ceiling defaulting to 0.0.
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5 },
                    fall: { terminalVelocity: 40.0 }
                }
            }
        })"#;
        let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let m = d.movement.expect("movement present");
        assert_eq!(m.air.jumps, 0);
        assert_eq!(m.air.jump_ceiling, 0.0);
    }

    #[test]
    fn lua_movement_jumps_zero_without_ceiling_is_valid() {
        // `jumpCeiling` is only meaningful when `air.jumps > 0`; omitting it
        // when jumps == 0 should succeed with jump_ceiling defaulting to 0.0.
        let src = r#"return {
            canonicalName = "player",
            components = {
                movement = {
                    capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                    ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5 },
                    fall = { terminalVelocity = 40.0 }
                }
            }
        }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        let m = d.movement.expect("movement present");
        assert_eq!(m.air.jumps, 0);
        assert_eq!(m.air.jump_ceiling, 0.0);
    }

    // --- stuck_stop_* parsing ----------------------------------------------

    #[test]
    fn js_movement_stuck_stop_defaults_when_omitted() {
        // The deadzone fields are optional on the wire; omitting them yields
        // the canonical defaults (enabled, 1e-3 threshold).
        let d = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap()
        });
        let m = d.movement.expect("movement present");
        assert!(m.stuck_stop_enabled);
        assert_eq!(m.stuck_stop_threshold, 1.0e-3);
    }

    #[test]
    fn js_movement_stuck_stop_explicit_values_parse() {
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 },
                    stuckStopEnabled: false,
                    stuckStopThreshold: 0.005
                }
            }
        })"#;
        let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let m = d.movement.expect("movement present");
        assert!(!m.stuck_stop_enabled);
        assert_eq!(m.stuck_stop_threshold, 0.005);
    }

    #[test]
    fn js_movement_stuck_stop_threshold_negative_is_rejected() {
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 },
                    stuckStopThreshold: -0.1
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_stuck_stop_enabled_wrong_type_is_rejected() {
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                    fall: { terminalVelocity: 40.0 },
                    stuckStopEnabled: "yes"
                }
            }
        })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_stuck_stop_defaults_when_omitted() {
        let src = r#"return {
            canonicalName = "player",
            components = {
                movement = {
                    capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                    ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                    fall = { terminalVelocity = 40.0 }
                }
            }
        }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        let m = d.movement.expect("movement present");
        assert!(m.stuck_stop_enabled);
        assert_eq!(m.stuck_stop_threshold, 1.0e-3);
    }

    #[test]
    fn lua_movement_stuck_stop_explicit_values_parse() {
        let src = r#"return {
            canonicalName = "player",
            components = {
                movement = {
                    capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                    ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                    fall = { terminalVelocity = 40.0 },
                    stuckStopEnabled = false,
                    stuckStopThreshold = 0.005
                }
            }
        }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        let m = d.movement.expect("movement present");
        assert!(!m.stuck_stop_enabled);
        assert_eq!(m.stuck_stop_threshold, 0.005);
    }

    #[test]
    fn lua_movement_stuck_stop_threshold_negative_is_rejected() {
        let src = r#"return {
            canonicalName = "player",
            components = {
                movement = {
                    capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                    ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                    fall = { terminalVelocity = 40.0 },
                    stuckStopThreshold = -0.1
                }
            }
        }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    // --- DashParams parsing ------------------------------------------------

    /// JS movement block with a `dash` sub-object spliced into the `movement`
    /// object. `dash_body` is the inner `{ ... }` text (no `dash:` key).
    fn js_movement_with_dash(dash_body: &str) -> String {
        format!(
            r#"({{
                canonicalName: "player",
                components: {{
                    movement: {{
                        capsule: {{ radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 }},
                        ground: {{ speed: {{ walk: 7.0, run: 11.0, crouch: 3.0 }}, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 }},
                        air: {{ forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 }},
                        fall: {{ terminalVelocity: 40.0 }},
                        dash: {dash_body}
                    }}
                }}
            }})"#
        )
    }

    /// Luau movement block with a `dash` sub-table spliced in.
    fn lua_movement_with_dash(dash_body: &str) -> String {
        format!(
            r#"return {{
                canonicalName = "player",
                components = {{
                    movement = {{
                        capsule = {{ radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 }},
                        ground = {{ speed = {{ walk = 7.0, run = 11.0, crouch = 3.0 }}, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 }},
                        air = {{ forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 }},
                        fall = {{ terminalVelocity = 40.0 }},
                        dash = {dash_body}
                    }}
                }}
            }}"#
        )
    }

    const JS_DASH_FULL: &str = r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#;
    const LUA_DASH_FULL: &str = r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#;

    #[test]
    fn js_movement_dash_absent_is_valid_and_disabled() {
        let d = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap()
        });
        assert!(d.movement.expect("movement present").dash.is_none());
    }

    #[test]
    fn lua_movement_dash_absent_is_valid_and_disabled() {
        let src = r#"return {
            canonicalName = "player",
            components = {
                movement = {
                    capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                    ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                    fall = { terminalVelocity = 40.0 }
                }
            }
        }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        assert!(d.movement.expect("movement present").dash.is_none());
    }

    #[test]
    fn js_movement_dash_full_shape_parses() {
        let src = js_movement_with_dash(JS_DASH_FULL);
        let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let dash = d.movement.unwrap().dash.expect("dash present");
        assert_eq!(dash.boost_speed, 18.0);
        assert_eq!(dash.momentum_retention, 0.5);
        assert_eq!(dash.steer_control, 0.25);
        assert_eq!(dash.dash_drag, 60.0);
        assert_eq!(dash.cooldown_ms, 800.0);
        assert_eq!(dash.air_dashes, 1);
        assert_eq!(dash.preserve_vertical, false);
    }

    #[test]
    fn lua_movement_dash_full_shape_parses() {
        let src = lua_movement_with_dash(LUA_DASH_FULL);
        let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
        let dash = d.movement.unwrap().dash.expect("dash present");
        assert_eq!(dash.boost_speed, 18.0);
        assert_eq!(dash.momentum_retention, 0.5);
        assert_eq!(dash.steer_control, 0.25);
        assert_eq!(dash.dash_drag, 60.0);
        assert_eq!(dash.cooldown_ms, 800.0);
        assert_eq!(dash.air_dashes, 1);
        assert_eq!(dash.preserve_vertical, false);
    }

    #[test]
    fn js_movement_dash_zero_dash_drag_and_cooldown_accepted() {
        // dashDrag and cooldownMs both legitimately permit 0.
        let src = js_movement_with_dash(
            r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 0.0, cooldownMs: 0.0, airDashes: 0, preserveVertical: true }"#,
        );
        let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let dash = d.movement.unwrap().dash.expect("dash present");
        assert_eq!(dash.dash_drag, 0.0);
        assert_eq!(dash.cooldown_ms, 0.0);
        assert_eq!(dash.preserve_vertical, true);
    }

    #[test]
    fn js_movement_dash_boost_speed_zero_is_rejected() {
        let src = js_movement_with_dash(
            r#"{ boostSpeed: 0.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_dash_boost_speed_negative_is_rejected() {
        let src = js_movement_with_dash(
            r#"{ boostSpeed: -1.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_dash_momentum_retention_out_of_range_is_rejected() {
        let src = js_movement_with_dash(
            r#"{ boostSpeed: 18.0, momentumRetention: 1.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_dash_steer_control_out_of_range_is_rejected() {
        let src = js_movement_with_dash(
            r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: -0.1, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_dash_negative_drag_is_rejected() {
        let src = js_movement_with_dash(
            r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: -1.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_dash_negative_cooldown_is_rejected() {
        let src = js_movement_with_dash(
            r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: -5.0, airDashes: 1, preserveVertical: false }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_dash_air_dashes_non_integer_is_rejected() {
        let src = js_movement_with_dash(
            r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1.5, preserveVertical: false }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_dash_air_dashes_negative_is_rejected() {
        let src = js_movement_with_dash(
            r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: -1, preserveVertical: false }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_dash_preserve_vertical_wrong_type_is_rejected() {
        let src = js_movement_with_dash(
            r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: "yes" }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_dash_missing_field_reports_missing_field() {
        // boostSpeed omitted.
        let src = js_movement_with_dash(
            r#"{ momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert_eq!(
            err,
            DescriptorError::MissingField {
                field: "boostSpeed",
            }
        );
    }

    #[test]
    fn lua_movement_dash_boost_speed_zero_is_rejected() {
        let src = lua_movement_with_dash(
            r#"{ boostSpeed = 0.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_dash_steer_control_out_of_range_is_rejected() {
        let src = lua_movement_with_dash(
            r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 1.5, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_dash_negative_cooldown_is_rejected() {
        let src = lua_movement_with_dash(
            r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = -5.0, airDashes = 1, preserveVertical = false }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_dash_zero_drag_and_cooldown_accepted() {
        let src = lua_movement_with_dash(
            r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 0.0, cooldownMs = 0.0, airDashes = 0, preserveVertical = true }"#,
        );
        let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
        let dash = d.movement.unwrap().dash.expect("dash present");
        assert_eq!(dash.dash_drag, 0.0);
        assert_eq!(dash.cooldown_ms, 0.0);
        assert_eq!(dash.air_dashes, 0);
    }

    #[test]
    fn lua_movement_dash_air_dashes_non_integer_is_rejected() {
        let src = lua_movement_with_dash(
            r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1.5, preserveVertical = false }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_dash_preserve_vertical_wrong_type_is_rejected() {
        let src = lua_movement_with_dash(
            r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = "yes" }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_dash_missing_field_reports_missing_field() {
        // preserveVertical omitted.
        let src = lua_movement_with_dash(
            r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1 }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert_eq!(
            err,
            DescriptorError::MissingField {
                field: "preserveVertical",
            }
        );
    }

    // --- Dash expression-capable fields ------------------------------------

    #[test]
    fn js_movement_dash_number_field_accepts_expression() {
        // `boostSpeed` is a clamped read of the movement-local `speed` input — a
        // well-typed Number-rooted expression. It must bind and round-trip as an
        // `Ir` variant (literal() is therefore None).
        let src = js_movement_with_dash(
            r#"{ boostSpeed: { op: "clamp", x: { op: "input", name: "speed" }, lo: { op: "const", value: 0.0 }, hi: { op: "const", value: 30.0 } }, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
        );
        let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let dash = d.movement.unwrap().dash.expect("dash present");
        assert!(
            matches!(dash.boost_speed, NumberOrIr::Ir(_)),
            "expression field parses to the Ir variant"
        );
        assert_eq!(dash.boost_speed.literal(), None);
        // The other (literal) fields keep their literal sugar / behavior.
        assert_eq!(dash.momentum_retention, 0.5);
        assert_eq!(dash.air_dashes, 1);
        assert_eq!(dash.preserve_vertical, false);
    }

    #[test]
    fn js_movement_dash_bool_field_accepts_expression() {
        // `preserveVertical` as a Bool-rooted comparison over `verticalSpeed`.
        let src = js_movement_with_dash(
            r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: { op: "gt", a: { op: "input", name: "verticalSpeed" }, b: { op: "const", value: 0.0 } } }"#,
        );
        let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let dash = d.movement.unwrap().dash.expect("dash present");
        assert!(matches!(dash.preserve_vertical, BoolOrIr::Ir(_)));
        assert_eq!(dash.preserve_vertical.literal(), None);
    }

    #[test]
    fn js_movement_dash_expression_unknown_read_is_rejected() {
        let src = js_movement_with_dash(
            r#"{ boostSpeed: { op: "input", name: "notARealInput" }, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_dash_expression_type_table_violation_is_rejected() {
        // `clamp` requires number operands; a boolean `grounded` input violates
        // the type table. Bind rejects without panicking.
        let src = js_movement_with_dash(
            r#"{ boostSpeed: { op: "clamp", x: { op: "input", name: "grounded" }, lo: { op: "const", value: 0.0 }, hi: { op: "const", value: 1.0 } }, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_dash_expression_root_type_mismatch_is_rejected() {
        // A boolean-rooted expression in a Number field: bind alone (no output)
        // never checks the root type, so this exercises the explicit root-type
        // check in `validate_dash_expr`.
        let src = js_movement_with_dash(
            r#"{ boostSpeed: { op: "gt", a: { op: "input", name: "speed" }, b: { op: "const", value: 1.0 } }, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_dash_expression_number_rooted_in_bool_field_is_rejected() {
        // A number-rooted expression in the Boolean field `preserveVertical`
        // must be rejected: `validate_dash_expr` checks the root type against
        // `IrType::Bool` and returns `InvalidShape` on mismatch.
        let src = js_movement_with_dash(
            r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: { op: "input", name: "speed" } }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_dash_expression_number_rooted_in_bool_field_is_rejected() {
        // Mirror of the JS case: a number-rooted expression placed in the
        // Boolean field `preserveVertical` is rejected via the same
        // `validate_dash_expr(node, IrType::Bool, …)` arm.
        let src = lua_movement_with_dash(
            r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = { op = "input", name = "speed" } }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_dash_malformed_node_object_is_rejected() {
        // An object that is not a recognizable node shape (no valid `op`).
        let src = js_movement_with_dash(
            r#"{ boostSpeed: { notAnOp: 1 }, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
        );
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_dev_player_dash_expressions_bind() {
        // Guards the dev player descriptor (`content/dev/scripts/player.ts`): the
        // exact IR the authored `runtime.*` builders emit for `momentumRetention`
        // and `steerControl` must bind without a `DescriptorError`. If the scope
        // names, op vocabulary, or root types ever drift, this fails before the
        // dev map does. `momentumRetention` is a `select` on the boolean
        // `grounded` input; `steerControl` clamps an `elapsedMs / 150` ramp into
        // [0, 1] — the 150 ms window sits inside the engine's 200 ms DASH_MAX_MS
        // bound so the ramp stays observable.
        let src = js_movement_with_dash(
            r#"{
                boostSpeed: 22.0,
                momentumRetention: { op: "select", cond: { op: "input", name: "grounded" }, a: { op: "const", value: 0.4 }, b: { op: "const", value: 0.7 } },
                steerControl: { op: "clamp", x: { op: "div", a: { op: "input", name: "elapsedMs" }, b: { op: "const", value: 150.0 } }, lo: { op: "const", value: 0.0 }, hi: { op: "const", value: 1.0 } },
                dashDrag: 0,
                cooldownMs: 600,
                airDashes: 1,
                preserveVertical: false
            }"#,
        );
        let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let dash = d.movement.unwrap().dash.expect("dash present");
        // Both authored fields bind as expressions; the literal fields keep sugar.
        assert!(matches!(dash.momentum_retention, NumberOrIr::Ir(_)));
        assert!(matches!(dash.steer_control, NumberOrIr::Ir(_)));
        assert_eq!(dash.boost_speed, 22.0);
        assert_eq!(dash.air_dashes, 1);
        assert_eq!(dash.preserve_vertical, false);
    }

    #[test]
    fn lua_movement_dash_number_field_accepts_expression() {
        let src = lua_movement_with_dash(
            r#"{ boostSpeed = { op = "clamp", x = { op = "input", name = "speed" }, lo = { op = "const", value = 0.0 }, hi = { op = "const", value = 30.0 } }, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#,
        );
        let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
        let dash = d.movement.unwrap().dash.expect("dash present");
        assert!(matches!(dash.boost_speed, NumberOrIr::Ir(_)));
        assert_eq!(dash.boost_speed.literal(), None);
        assert_eq!(dash.momentum_retention, 0.5);
    }

    #[test]
    fn lua_movement_dash_bool_field_accepts_expression() {
        let src = lua_movement_with_dash(
            r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = { op = "gt", a = { op = "input", name = "verticalSpeed" }, b = { op = "const", value = 0.0 } } }"#,
        );
        let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
        let dash = d.movement.unwrap().dash.expect("dash present");
        assert!(matches!(dash.preserve_vertical, BoolOrIr::Ir(_)));
    }

    #[test]
    fn lua_movement_dash_expression_unknown_read_is_rejected() {
        let src = lua_movement_with_dash(
            r#"{ boostSpeed = { op = "input", name = "notARealInput" }, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_dash_expression_type_table_violation_is_rejected() {
        let src = lua_movement_with_dash(
            r#"{ boostSpeed = { op = "clamp", x = { op = "input", name = "grounded" }, lo = { op = "const", value = 0.0 }, hi = { op = "const", value = 1.0 } }, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_dash_expression_root_type_mismatch_is_rejected() {
        let src = lua_movement_with_dash(
            r#"{ boostSpeed = { op = "gt", a = { op = "input", name = "speed" }, b = { op = "const", value = 1.0 } }, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_dash_malformed_node_object_is_rejected() {
        let src = lua_movement_with_dash(
            r#"{ boostSpeed = { notAnOp = 1 }, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    // --- CrouchParams parsing ----------------------------------------------

    /// JS movement block with a `crouch` sub-object spliced into the `movement`
    /// object. `crouch_body` is the inner `{ ... }` text (no `crouch:` key).
    /// Capsule radius is 0.4 so the crouched `eyeHeight` upper bound is
    /// `crouch.halfHeight + 0.4`.
    fn js_movement_with_crouch(crouch_body: &str) -> String {
        format!(
            r#"({{
                canonicalName: "player",
                components: {{
                    movement: {{
                        capsule: {{ radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 }},
                        ground: {{ speed: {{ walk: 7.0, run: 11.0, crouch: 3.0 }}, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 }},
                        air: {{ forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 }},
                        fall: {{ terminalVelocity: 40.0 }},
                        crouch: {crouch_body}
                    }}
                }}
            }})"#
        )
    }

    /// Luau movement block with a `crouch` sub-table spliced in.
    fn lua_movement_with_crouch(crouch_body: &str) -> String {
        format!(
            r#"return {{
                canonicalName = "player",
                components = {{
                    movement = {{
                        capsule = {{ radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 }},
                        ground = {{ speed = {{ walk = 7.0, run = 11.0, crouch = 3.0 }}, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 }},
                        air = {{ forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 }},
                        fall = {{ terminalVelocity = 40.0 }},
                        crouch = {crouch_body}
                    }}
                }}
            }}"#
        )
    }

    const JS_CROUCH_FULL: &str = r#"{ halfHeight: 0.4, eyeHeight: 0.3, transitionRate: 8.0 }"#;
    const LUA_CROUCH_FULL: &str = r#"{ halfHeight = 0.4, eyeHeight = 0.3, transitionRate = 8.0 }"#;

    #[test]
    fn js_movement_crouch_absent_is_valid_and_disabled() {
        let d = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap()
        });
        assert!(d.movement.expect("movement present").crouch.is_none());
    }

    #[test]
    fn lua_movement_crouch_absent_is_valid_and_disabled() {
        let src = r#"return {
            canonicalName = "player",
            components = {
                movement = {
                    capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                    ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                    fall = { terminalVelocity = 40.0 }
                }
            }
        }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        assert!(d.movement.expect("movement present").crouch.is_none());
    }

    #[test]
    fn js_movement_crouch_full_shape_parses() {
        let src = js_movement_with_crouch(JS_CROUCH_FULL);
        let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let crouch = d.movement.unwrap().crouch.expect("crouch present");
        assert_eq!(crouch.half_height, 0.4);
        assert_eq!(crouch.eye_height, 0.3);
        assert_eq!(crouch.transition_rate, 8.0);
    }

    #[test]
    fn lua_movement_crouch_full_shape_parses() {
        let src = lua_movement_with_crouch(LUA_CROUCH_FULL);
        let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
        let crouch = d.movement.unwrap().crouch.expect("crouch present");
        assert_eq!(crouch.half_height, 0.4);
        assert_eq!(crouch.eye_height, 0.3);
        assert_eq!(crouch.transition_rate, 8.0);
    }

    #[test]
    fn js_movement_crouch_eye_height_at_capsule_top_is_accepted() {
        // Inclusive upper bound: halfHeight (0.4) + radius (0.4) = 0.8.
        let src =
            js_movement_with_crouch(r#"{ halfHeight: 0.4, eyeHeight: 0.8, transitionRate: 8.0 }"#);
        let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        assert_eq!(d.movement.unwrap().crouch.unwrap().eye_height, 0.8);
    }

    #[test]
    fn js_movement_crouch_half_height_zero_is_rejected() {
        let src =
            js_movement_with_crouch(r#"{ halfHeight: 0.0, eyeHeight: 0.3, transitionRate: 8.0 }"#);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_crouch_half_height_negative_is_rejected() {
        let src =
            js_movement_with_crouch(r#"{ halfHeight: -0.4, eyeHeight: 0.3, transitionRate: 8.0 }"#);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_crouch_eye_height_zero_is_rejected() {
        // Exclusive lower bound: 0 is rejected.
        let src =
            js_movement_with_crouch(r#"{ halfHeight: 0.4, eyeHeight: 0.0, transitionRate: 8.0 }"#);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_crouch_eye_height_above_capsule_top_is_rejected() {
        // halfHeight (0.4) + radius (0.4) = 0.8; 0.9 exceeds the crouched top.
        let src =
            js_movement_with_crouch(r#"{ halfHeight: 0.4, eyeHeight: 0.9, transitionRate: 8.0 }"#);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_crouch_transition_rate_zero_is_rejected() {
        let src =
            js_movement_with_crouch(r#"{ halfHeight: 0.4, eyeHeight: 0.3, transitionRate: 0.0 }"#);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_crouch_transition_rate_negative_is_rejected() {
        let src =
            js_movement_with_crouch(r#"{ halfHeight: 0.4, eyeHeight: 0.3, transitionRate: -1.0 }"#);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_crouch_missing_field_reports_missing_field() {
        // transitionRate omitted.
        let src = js_movement_with_crouch(r#"{ halfHeight: 0.4, eyeHeight: 0.3 }"#);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert_eq!(
            err,
            DescriptorError::MissingField {
                field: "transitionRate",
            }
        );
    }

    #[test]
    fn lua_movement_crouch_half_height_zero_is_rejected() {
        let src = lua_movement_with_crouch(
            r#"{ halfHeight = 0.0, eyeHeight = 0.3, transitionRate = 8.0 }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_crouch_half_height_negative_is_rejected() {
        let src = lua_movement_with_crouch(
            r#"{ halfHeight = -0.4, eyeHeight = 0.3, transitionRate = 8.0 }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_crouch_eye_height_zero_is_rejected() {
        // Exclusive lower bound: 0 is rejected.
        let src = lua_movement_with_crouch(
            r#"{ halfHeight = 0.4, eyeHeight = 0.0, transitionRate = 8.0 }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_crouch_eye_height_at_capsule_top_is_accepted() {
        // Inclusive upper bound: halfHeight (0.4) + radius (0.4) = 0.8.
        let src = lua_movement_with_crouch(
            r#"{ halfHeight = 0.4, eyeHeight = 0.8, transitionRate = 8.0 }"#,
        );
        let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
        assert_eq!(d.movement.unwrap().crouch.unwrap().eye_height, 0.8);
    }

    #[test]
    fn lua_movement_crouch_eye_height_above_capsule_top_is_rejected() {
        let src = lua_movement_with_crouch(
            r#"{ halfHeight = 0.4, eyeHeight = 0.9, transitionRate = 8.0 }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_crouch_transition_rate_zero_is_rejected() {
        let src = lua_movement_with_crouch(
            r#"{ halfHeight = 0.4, eyeHeight = 0.3, transitionRate = 0.0 }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_crouch_transition_rate_negative_is_rejected() {
        let src = lua_movement_with_crouch(
            r#"{ halfHeight = 0.4, eyeHeight = 0.3, transitionRate = -1.0 }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_crouch_missing_field_reports_missing_field() {
        // eyeHeight omitted.
        let src = lua_movement_with_crouch(r#"{ halfHeight = 0.4, transitionRate = 8.0 }"#);
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert_eq!(err, DescriptorError::MissingField { field: "eyeHeight" });
    }

    // --- ForgivenessParams parsing -----------------------------------------

    /// JS movement block with a `forgiveness` sub-object spliced in. `body` is
    /// the inner `{ ... }` text (no `forgiveness:` key).
    fn js_movement_with_forgiveness(body: &str) -> String {
        format!(
            r#"({{
                canonicalName: "player",
                components: {{
                    movement: {{
                        capsule: {{ radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 }},
                        ground: {{ speed: {{ walk: 7.0, run: 11.0, crouch: 3.0 }}, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 }},
                        air: {{ forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 }},
                        fall: {{ terminalVelocity: 40.0 }},
                        forgiveness: {body}
                    }}
                }}
            }})"#
        )
    }

    /// Luau movement block with a `forgiveness` sub-table spliced in.
    fn lua_movement_with_forgiveness(body: &str) -> String {
        format!(
            r#"return {{
                canonicalName = "player",
                components = {{
                    movement = {{
                        capsule = {{ radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 }},
                        ground = {{ speed = {{ walk = 7.0, run = 11.0, crouch = 3.0 }}, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 }},
                        air = {{ forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 }},
                        fall = {{ terminalVelocity = 40.0 }},
                        forgiveness = {body}
                    }}
                }}
            }}"#
        )
    }

    #[test]
    fn js_movement_forgiveness_absent_is_none() {
        // No `forgiveness` key → None; `from_descriptor` applies engine defaults.
        let d = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap()
        });
        assert!(d.movement.expect("movement present").forgiveness.is_none());
    }

    #[test]
    fn js_movement_forgiveness_explicit_values_parse() {
        let src = js_movement_with_forgiveness(r#"{ coyoteMs: 120.0, jumpBufferMs: 80.0 }"#);
        let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let f = d
            .movement
            .unwrap()
            .forgiveness
            .expect("forgiveness present");
        assert_eq!(f.coyote_ms, 120.0);
        assert_eq!(f.jump_buffer_ms, 80.0);
    }

    #[test]
    fn js_movement_forgiveness_omitted_fields_default_per_field() {
        // Present object, each field optional with its own engine default.
        let src = js_movement_with_forgiveness(r#"{ coyoteMs: 0.0 }"#);
        let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let f = d
            .movement
            .unwrap()
            .forgiveness
            .expect("forgiveness present");
        assert_eq!(f.coyote_ms, 0.0, "explicit 0 disables coyote");
        assert_eq!(
            f.jump_buffer_ms,
            ForgivenessParams::DEFAULT_JUMP_BUFFER_MS,
            "omitted jumpBufferMs falls back to its engine default"
        );
    }

    #[test]
    fn js_movement_forgiveness_negative_is_rejected() {
        let src = js_movement_with_forgiveness(r#"{ coyoteMs: -5.0, jumpBufferMs: 80.0 }"#);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_forgiveness_explicit_values_parse() {
        let src = lua_movement_with_forgiveness(r#"{ coyoteMs = 120.0, jumpBufferMs = 80.0 }"#);
        let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
        let f = d
            .movement
            .unwrap()
            .forgiveness
            .expect("forgiveness present");
        assert_eq!(f.coyote_ms, 120.0);
        assert_eq!(f.jump_buffer_ms, 80.0);
    }

    #[test]
    fn lua_movement_forgiveness_omitted_fields_default_per_field() {
        let src = lua_movement_with_forgiveness(r#"{ jumpBufferMs = 0.0 }"#);
        let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
        let f = d
            .movement
            .unwrap()
            .forgiveness
            .expect("forgiveness present");
        assert_eq!(f.jump_buffer_ms, 0.0, "explicit 0 disables jump buffer");
        assert_eq!(
            f.coyote_ms,
            ForgivenessParams::DEFAULT_COYOTE_MS,
            "omitted coyoteMs falls back to its engine default"
        );
    }

    #[test]
    fn lua_movement_forgiveness_negative_is_rejected() {
        let src = lua_movement_with_forgiveness(r#"{ coyoteMs = 100.0, jumpBufferMs = -1.0 }"#);
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    // --- ViewFeelParams parsing --------------------------------------------

    /// JS movement block with a `viewFeel` sub-object spliced in. `body` is the
    /// inner `{ ... }` text (no `viewFeel:` key).
    fn js_movement_with_view_feel(body: &str) -> String {
        format!(
            r#"({{
                canonicalName: "player",
                components: {{
                    movement: {{
                        capsule: {{ radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 }},
                        ground: {{ speed: {{ walk: 7.0, run: 11.0, crouch: 3.0 }}, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 }},
                        air: {{ forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 }},
                        fall: {{ terminalVelocity: 40.0 }},
                        viewFeel: {body}
                    }}
                }}
            }})"#
        )
    }

    /// Luau movement block with a `viewFeel` sub-table spliced in.
    fn lua_movement_with_view_feel(body: &str) -> String {
        format!(
            r#"return {{
                canonicalName = "player",
                components = {{
                    movement = {{
                        capsule = {{ radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 }},
                        ground = {{ speed = {{ walk = 7.0, run = 11.0, crouch = 3.0 }}, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 }},
                        air = {{ forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 }},
                        fall = {{ terminalVelocity = 40.0 }},
                        viewFeel = {body}
                    }}
                }}
            }}"#
        )
    }

    const JS_BOB_FULL: &str = r#"{ verticalFrequency: 1.8, lateralFrequency: 0.9, verticalAmplitude: 0.06, lateralAmplitude: 0.04, speedThreshold: 0.5 }"#;
    const JS_TILT_FULL: &str = r#"{ maxAngle: 3.0, speedReference: 8.0, tension: 12.0 }"#;
    const JS_SWAY_FULL: &str = r#"{ amplitude: 0.5, frequency: 0.4, speedScale: 0.2 }"#;

    // Absent `viewFeel` → no ViewFeelParams materialized.

    #[test]
    fn js_movement_view_feel_absent_is_valid_and_disabled() {
        let d = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap()
        });
        assert!(d.movement.expect("movement present").view_feel.is_none());
    }

    #[test]
    fn lua_movement_view_feel_absent_is_valid_and_disabled() {
        let src = r#"return {
            canonicalName = "player",
            components = {
                movement = {
                    capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                    ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                    fall = { terminalVelocity = 40.0 }
                }
            }
        }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        assert!(d.movement.expect("movement present").view_feel.is_none());
    }

    // Present `viewFeel` with all three motions absent is valid (empty bundle).

    #[test]
    fn js_movement_view_feel_present_empty_disables_each_motion() {
        let src = js_movement_with_view_feel("{}");
        let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let vf = d.movement.unwrap().view_feel.expect("viewFeel present");
        assert!(vf.bob.is_none());
        assert!(vf.tilt.is_none());
        assert!(vf.sway.is_none());
    }

    #[test]
    fn lua_movement_view_feel_present_empty_disables_each_motion() {
        let src = lua_movement_with_view_feel("{}");
        let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
        let vf = d.movement.unwrap().view_feel.expect("viewFeel present");
        assert!(vf.bob.is_none());
        assert!(vf.tilt.is_none());
        assert!(vf.sway.is_none());
    }

    // Full shapes parse and `groundedOnly` defaults apply (bob/tilt true, sway false).

    #[test]
    fn js_movement_view_feel_full_shape_parses_with_grounded_only_defaults() {
        let body =
            format!(r#"{{ bob: {JS_BOB_FULL}, tilt: {JS_TILT_FULL}, sway: {JS_SWAY_FULL} }}"#);
        let src = js_movement_with_view_feel(&body);
        let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let vf = d.movement.unwrap().view_feel.expect("viewFeel present");
        let bob = vf.bob.expect("bob present");
        assert_eq!(bob.vertical_frequency, 1.8);
        assert_eq!(bob.lateral_frequency, 0.9);
        assert_eq!(bob.vertical_amplitude, 0.06);
        assert_eq!(bob.lateral_amplitude, 0.04);
        assert_eq!(bob.speed_threshold, 0.5);
        assert!(bob.grounded_only, "bob groundedOnly defaults true");
        let tilt = vf.tilt.expect("tilt present");
        assert_eq!(tilt.max_angle, 3.0);
        assert_eq!(tilt.speed_reference, 8.0);
        assert_eq!(tilt.tension, 12.0);
        assert!(tilt.grounded_only, "tilt groundedOnly defaults true");
        let sway = vf.sway.expect("sway present");
        assert_eq!(sway.amplitude, 0.5);
        assert_eq!(sway.frequency, 0.4);
        assert_eq!(sway.speed_scale, 0.2);
        assert!(!sway.grounded_only, "sway groundedOnly defaults false");
    }

    #[test]
    fn lua_movement_view_feel_full_shape_parses_with_grounded_only_defaults() {
        let src = lua_movement_with_view_feel(
            r#"{
                bob = { verticalFrequency = 1.8, lateralFrequency = 0.9, verticalAmplitude = 0.06, lateralAmplitude = 0.04, speedThreshold = 0.5 },
                tilt = { maxAngle = 3.0, speedReference = 8.0, tension = 12.0 },
                sway = { amplitude = 0.5, frequency = 0.4, speedScale = 0.2 }
            }"#,
        );
        let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
        let vf = d.movement.unwrap().view_feel.expect("viewFeel present");
        let bob = vf.bob.expect("bob present");
        assert_eq!(bob.vertical_frequency, 1.8);
        assert_eq!(bob.lateral_frequency, 0.9);
        assert!(bob.grounded_only, "bob groundedOnly defaults true");
        let tilt = vf.tilt.expect("tilt present");
        assert_eq!(tilt.tension, 12.0);
        assert!(tilt.grounded_only, "tilt groundedOnly defaults true");
        let sway = vf.sway.expect("sway present");
        assert_eq!(sway.frequency, 0.4);
        assert!(!sway.grounded_only, "sway groundedOnly defaults false");
    }

    // Explicit `groundedOnly` overrides the per-motion default in both paths.

    #[test]
    fn js_movement_view_feel_grounded_only_explicit_overrides_default() {
        let body = r#"{ bob: { verticalFrequency: 1.8, lateralFrequency: 0.9, verticalAmplitude: 0.06, lateralAmplitude: 0.04, speedThreshold: 0.5, groundedOnly: false }, sway: { amplitude: 0.5, frequency: 0.4, speedScale: 0.2, groundedOnly: true } }"#;
        let src = js_movement_with_view_feel(body);
        let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let vf = d.movement.unwrap().view_feel.unwrap();
        assert!(
            !vf.bob.unwrap().grounded_only,
            "explicit false overrides bob default true"
        );
        assert!(
            vf.sway.unwrap().grounded_only,
            "explicit true overrides sway default false"
        );
    }

    #[test]
    fn lua_movement_view_feel_grounded_only_explicit_overrides_default() {
        let src = lua_movement_with_view_feel(
            r#"{ bob = { verticalFrequency = 1.8, lateralFrequency = 0.9, verticalAmplitude = 0.06, lateralAmplitude = 0.04, speedThreshold = 0.5, groundedOnly = false }, sway = { amplitude = 0.5, frequency = 0.4, speedScale = 0.2, groundedOnly = true } }"#,
        );
        let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
        let vf = d.movement.unwrap().view_feel.unwrap();
        assert!(!vf.bob.unwrap().grounded_only);
        assert!(vf.sway.unwrap().grounded_only);
    }

    #[test]
    fn js_movement_view_feel_grounded_only_non_boolean_is_rejected() {
        let body = r#"{ bob: { verticalFrequency: 1.8, lateralFrequency: 0.9, verticalAmplitude: 0.06, lateralAmplitude: 0.04, speedThreshold: 0.5, groundedOnly: "yes" } }"#;
        let src = js_movement_with_view_feel(body);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_view_feel_grounded_only_non_boolean_is_rejected() {
        let src = lua_movement_with_view_feel(
            r#"{ bob = { verticalFrequency = 1.8, lateralFrequency = 0.9, verticalAmplitude = 0.06, lateralAmplitude = 0.04, speedThreshold = 0.5, groundedOnly = "yes" } }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    // Present-then-all-required: a missing required field is rejected in both paths.

    #[test]
    fn js_movement_view_feel_bob_missing_field_reports_missing_field() {
        // verticalFrequency omitted from bob.
        let body = r#"{ bob: { lateralFrequency: 0.9, verticalAmplitude: 0.06, lateralAmplitude: 0.04, speedThreshold: 0.5 } }"#;
        let src = js_movement_with_view_feel(body);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert_eq!(
            err,
            DescriptorError::MissingField {
                field: "verticalFrequency"
            }
        );
    }

    #[test]
    fn lua_movement_view_feel_tilt_missing_field_reports_missing_field() {
        // tension omitted from tilt.
        let src =
            lua_movement_with_view_feel(r#"{ tilt = { maxAngle = 3.0, speedReference = 8.0 } }"#);
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert_eq!(err, DescriptorError::MissingField { field: "tension" });
    }

    #[test]
    fn js_movement_view_feel_sway_missing_field_reports_missing_field() {
        // speedScale omitted from sway.
        let body = r#"{ sway: { amplitude: 0.5, frequency: 0.4 } }"#;
        let src = js_movement_with_view_feel(body);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert_eq!(
            err,
            DescriptorError::MissingField {
                field: "speedScale"
            }
        );
    }

    // Value validation: positive-finite, non-negative-finite, and the
    // tilt.maxAngle [0,90] range, symmetric across JS and Luau.

    #[test]
    fn js_movement_view_feel_bob_vertical_frequency_zero_is_rejected() {
        let body = r#"{ bob: { verticalFrequency: 0.0, lateralFrequency: 0.9, verticalAmplitude: 0.06, lateralAmplitude: 0.04, speedThreshold: 0.5 } }"#;
        let src = js_movement_with_view_feel(body);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_view_feel_bob_negative_amplitude_is_rejected() {
        let body = r#"{ bob: { verticalFrequency: 1.8, lateralFrequency: 0.9, verticalAmplitude: -0.01, lateralAmplitude: 0.04, speedThreshold: 0.5 } }"#;
        let src = js_movement_with_view_feel(body);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_view_feel_bob_zero_amplitude_is_accepted() {
        // Amplitudes and speedThreshold permit 0 (non-negative finite).
        let body = r#"{ bob: { verticalFrequency: 1.8, lateralFrequency: 0.9, verticalAmplitude: 0.0, lateralAmplitude: 0.0, speedThreshold: 0.0 } }"#;
        let src = js_movement_with_view_feel(body);
        let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let bob = d.movement.unwrap().view_feel.unwrap().bob.unwrap();
        assert_eq!(bob.vertical_amplitude, 0.0);
        assert_eq!(bob.speed_threshold, 0.0);
    }

    #[test]
    fn js_movement_view_feel_tilt_max_angle_above_90_is_rejected() {
        let body = r#"{ tilt: { maxAngle: 95.0, speedReference: 8.0, tension: 12.0 } }"#;
        let src = js_movement_with_view_feel(body);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_view_feel_tilt_max_angle_negative_is_rejected() {
        let body = r#"{ tilt: { maxAngle: -1.0, speedReference: 8.0, tension: 12.0 } }"#;
        let src = js_movement_with_view_feel(body);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_view_feel_tilt_tension_zero_is_rejected() {
        let body = r#"{ tilt: { maxAngle: 3.0, speedReference: 8.0, tension: 0.0 } }"#;
        let src = js_movement_with_view_feel(body);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_movement_view_feel_sway_frequency_zero_is_rejected() {
        let body = r#"{ sway: { amplitude: 0.5, frequency: 0.0, speedScale: 0.2 } }"#;
        let src = js_movement_with_view_feel(body);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_view_feel_tilt_max_angle_above_90_is_rejected() {
        let src = lua_movement_with_view_feel(
            r#"{ tilt = { maxAngle = 95.0, speedReference = 8.0, tension = 12.0 } }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_view_feel_tilt_speed_reference_zero_is_rejected() {
        let src = lua_movement_with_view_feel(
            r#"{ tilt = { maxAngle = 3.0, speedReference = 0.0, tension = 12.0 } }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_view_feel_sway_negative_amplitude_is_rejected() {
        let src = lua_movement_with_view_feel(
            r#"{ sway = { amplitude = -0.1, frequency = 0.4, speedScale = 0.2 } }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_view_feel_sway_frequency_zero_is_rejected() {
        let src = lua_movement_with_view_feel(
            r#"{ sway = { amplitude = 0.5, frequency = 0.0, speedScale = 0.2 } }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_view_feel_bob_missing_field_reports_missing_field() {
        // lateralFrequency omitted from bob.
        let src = lua_movement_with_view_feel(
            r#"{ bob = { verticalFrequency = 1.8, verticalAmplitude = 0.06, lateralAmplitude = 0.04, speedThreshold = 0.5 } }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert_eq!(
            err,
            DescriptorError::MissingField {
                field: "lateralFrequency",
            }
        );
    }

    #[test]
    fn lua_movement_view_feel_bob_lateral_frequency_zero_is_rejected() {
        let src = lua_movement_with_view_feel(
            r#"{ bob = { verticalFrequency = 1.8, lateralFrequency = 0.0, verticalAmplitude = 0.06, lateralAmplitude = 0.04, speedThreshold = 0.5 } }"#,
        );
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    // Wrong-typed sub-object is rejected (a present sub-object must be an object/table).

    #[test]
    fn js_movement_view_feel_bob_not_an_object_is_rejected() {
        let body = r#"{ bob: 3 }"#;
        let src = js_movement_with_view_feel(body);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_movement_view_feel_sway_not_a_table_is_rejected() {
        let src = lua_movement_with_view_feel(r#"{ sway = 3 }"#);
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    // --- components.mesh -----------------------------------------------------

    #[test]
    fn js_mesh_stateless_parses_model_only() {
        let src = r#"({ components: { mesh: { model: "decraniated" } } })"#;
        let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let mesh = d.mesh.expect("mesh descriptor parsed");
        assert_eq!(mesh.model, "decraniated");
        assert!(
            mesh.animations.is_empty() && mesh.default_state.is_none(),
            "no animations block ⇒ stateless"
        );
    }

    #[test]
    fn js_mesh_animated_parses_states_and_default() {
        let src = r#"({ components: { mesh: {
            model: "decraniated",
            defaultState: "idle",
            animations: {
                idle:   { clip: "idle_clip", loop: true, crossfadeMs: 120, interrupt: "smooth" },
                attack: { clip: "attack_clip", loop: false }
            }
        } } })"#;
        let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let mesh = d.mesh.expect("mesh descriptor parsed");
        assert_eq!(mesh.default_state.as_deref(), Some("idle"));
        assert_eq!(mesh.animations.len(), 2);
        let idle = &mesh.animations["idle"];
        assert_eq!(idle.clip, "idle_clip");
        assert!(idle.looping);
        assert_eq!(idle.crossfade_ms, 120.0);
        assert_eq!(idle.interrupt, InterruptPolicy::Smooth);
        assert!(idle.clip_index.is_none(), "clip_index unresolved at parse");
        // Absent `crossfadeMs`/`interrupt` default; absent `loop` ⇒ false.
        let attack = &mesh.animations["attack"];
        assert!(!attack.looping);
        assert_eq!(
            attack.crossfade_ms,
            crate::scripting::components::mesh::DEFAULT_CROSSFADE_MS
        );
        assert_eq!(attack.interrupt, InterruptPolicy::Smooth);
    }

    #[test]
    fn js_mesh_interrupt_snap_parses() {
        let src = r#"({ components: { mesh: {
            model: "m", defaultState: "die",
            animations: { die: { clip: "death", interrupt: "snap" } }
        } } })"#;
        let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        assert_eq!(
            d.mesh.unwrap().animations["die"].interrupt,
            InterruptPolicy::Snap
        );
    }

    #[test]
    fn js_mesh_empty_model_is_rejected() {
        let src = r#"({ components: { mesh: { model: "" } } })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_mesh_empty_clip_is_rejected() {
        let src = r#"({ components: { mesh: {
            model: "m", defaultState: "idle",
            animations: { idle: { clip: "" } }
        } } })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_mesh_negative_crossfade_is_rejected() {
        let src = r#"({ components: { mesh: {
            model: "m", defaultState: "idle",
            animations: { idle: { clip: "c", crossfadeMs: -1 } }
        } } })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_mesh_unknown_interrupt_is_rejected() {
        let src = r#"({ components: { mesh: {
            model: "m", defaultState: "idle",
            animations: { idle: { clip: "c", interrupt: "instant" } }
        } } })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_mesh_animations_without_default_state_is_rejected() {
        let src = r#"({ components: { mesh: {
            model: "m",
            animations: { idle: { clip: "c" } }
        } } })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert_eq!(
            err,
            DescriptorError::MissingField {
                field: "defaultState"
            }
        );
    }

    #[test]
    fn js_mesh_default_state_not_declared_is_rejected() {
        let src = r#"({ components: { mesh: {
            model: "m", defaultState: "nope",
            animations: { idle: { clip: "c" } }
        } } })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_mesh_present_empty_animations_is_rejected() {
        let src = r#"({ components: { mesh: { model: "m", animations: {} } } })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_mesh_default_state_without_animations_is_rejected() {
        let src = r#"({ components: { mesh: { model: "m", defaultState: "idle" } } })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_mesh_stateless_parses_model_only() {
        let src = r#"return { components = { mesh = { model = "decraniated" } } }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        let mesh = d.mesh.expect("mesh descriptor parsed");
        assert_eq!(mesh.model, "decraniated");
        assert!(mesh.animations.is_empty() && mesh.default_state.is_none());
    }

    #[test]
    fn lua_mesh_animated_parses_states_and_default() {
        let src = r#"return { components = { mesh = {
            model = "decraniated",
            defaultState = "idle",
            animations = {
                idle = { clip = "idle_clip", loop = true, crossfadeMs = 120, interrupt = "snap" },
                attack = { clip = "attack_clip" }
            }
        } } }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        let mesh = d.mesh.expect("mesh descriptor parsed");
        assert_eq!(mesh.default_state.as_deref(), Some("idle"));
        assert_eq!(mesh.animations.len(), 2);
        assert_eq!(mesh.animations["idle"].interrupt, InterruptPolicy::Snap);
        assert!(mesh.animations["idle"].looping);
        // Absent `loop` ⇒ false; absent `crossfadeMs`/`interrupt` ⇒ defaults.
        let attack = &mesh.animations["attack"];
        assert!(!attack.looping);
        assert_eq!(attack.interrupt, InterruptPolicy::Smooth);
        assert_eq!(
            attack.crossfade_ms,
            crate::scripting::components::mesh::DEFAULT_CROSSFADE_MS
        );
    }

    #[test]
    fn lua_mesh_empty_model_is_rejected() {
        let src = r#"return { components = { mesh = { model = "" } } }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_mesh_animations_without_default_state_is_rejected() {
        let src = r#"return { components = { mesh = {
            model = "m",
            animations = { idle = { clip = "c" } }
        } } }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert_eq!(
            err,
            DescriptorError::MissingField {
                field: "defaultState"
            }
        );
    }

    #[test]
    fn lua_mesh_default_state_without_animations_is_rejected() {
        let src = r#"return { components = { mesh = { model = "m", defaultState = "idle" } } }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_mesh_default_state_not_declared_is_rejected() {
        let src = r#"return { components = { mesh = {
            model = "m", defaultState = "nope",
            animations = { idle = { clip = "c" } }
        } } }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_mesh_present_empty_animations_is_rejected() {
        // A present-but-empty `animations` table is rejected: the table value
        // IS present, so `animations_present` is true and the empty map is
        // rejected.
        let src = r#"return { components = { mesh = { model = "m", animations = {} } } }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_mesh_unknown_interrupt_is_rejected() {
        let src = r#"return { components = { mesh = {
            model = "m", defaultState = "idle",
            animations = { idle = { clip = "c", interrupt = "instant" } }
        } } }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_mesh_negative_crossfade_is_rejected() {
        let src = r#"return { components = { mesh = {
            model = "m", defaultState = "idle",
            animations = { idle = { clip = "c", crossfadeMs = -2.0 } }
        } } }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    // --- health component (both parsers) ------------------------------------

    #[test]
    fn js_entity_descriptor_parses_health_with_hitbox() {
        let src = r#"({
            canonicalName: "dummy",
            components: { health: { max: 80, hitbox: { halfExtents: [0.5, 1.0, 0.5], offset: [0, 0.9, 0] } } }
        })"#;
        let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        let health = d.health.expect("health parsed");
        assert_eq!(health.max, 80.0);
        let hitbox = health.hitbox.expect("hitbox parsed");
        assert_eq!(hitbox.half_extents, [0.5, 1.0, 0.5]);
        assert_eq!(hitbox.offset, Some([0.0, 0.9, 0.0]));
    }

    #[test]
    fn lua_entity_descriptor_parses_health_without_hitbox() {
        // Luau parity: a missing arm would silently drop `health`. Assert the
        // arm exists and a hitbox-less block parses (offset/hitbox optional).
        let src = r#"return {
            canonicalName = "player",
            components = { health = { max = 100 } }
        }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        let health = d.health.expect("health parsed by the Luau arm");
        assert_eq!(health.max, 100.0);
        assert!(health.hitbox.is_none());
    }

    #[test]
    fn js_health_non_positive_max_is_rejected() {
        let src = r#"({ components: { health: { max: 0 } } })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn lua_health_non_finite_max_is_rejected() {
        let src = r#"return { components = { health = { max = 1/0 } } }"#;
        let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_health_non_positive_hitbox_extent_is_rejected() {
        let src = r#"({ components: { health: { max: 50, hitbox: { halfExtents: [0.5, 0.0, 0.5] } } } })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }

    #[test]
    fn js_health_non_finite_offset_is_rejected() {
        let src = r#"({ components: { health: { max: 50, hitbox: { halfExtents: [0.5, 0.5, 0.5], offset: [0, 1/0, 0] } } } })"#;
        let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }
}
