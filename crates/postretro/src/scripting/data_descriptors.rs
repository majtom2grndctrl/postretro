// Data-context descriptor types: `LevelManifest`/`ReactionDescriptor`, `EntityTypeDescriptor`,
// `LightDescriptor`, `WeaponDescriptor`, and `PlayerMovementDescriptor`; JS and Luau
// deserialization paths for all of them.
// See: context/lib/scripting.md §2 (Data context lifecycle)

use mlua::{Table, Value as LuaValue};
use rquickjs::{Array, Ctx, Object, Value as JsValue};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::components::billboard_emitter::{
    BillboardEmitterComponent, BillboardEmitterComponentLit,
};
use super::registry::EntityId;

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

/// Primitive-action reaction: invokes a named Rust primitive on entities
/// matching `tag`, optionally firing `on_complete` when the primitive finishes.
///
/// `args` carries the primitive-specific payload (e.g. `{ "rate": 0.0 }` for
/// `setEmitterRate`). Defaults to an empty JSON object when the descriptor
/// omits the field, so primitives that take no args parse cleanly.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PrimitiveDescriptor {
    pub(crate) primitive: String,
    pub(crate) tag: String,
    pub(crate) on_complete: Option<String>,
    pub(crate) args: serde_json::Value,
}

/// A reaction descriptor paired with the event name it is registered under.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NamedReaction {
    pub(crate) name: String,
    pub(crate) descriptor: ReactionDescriptor,
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

/// Dash tuning. Optional on [`PlayerMovementDescriptor`] (absent disables
/// dash); when present, all fields are required and validated. Field names are
/// camelCase on the wire (`boostSpeed`, `momentumRetention`, …) and snake_case
/// in Rust. Stored later by `PlayerMovementComponent` as `Option<DashParams>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DashParams {
    /// Impulse magnitude applied on dash, world-units/sec. Must be finite > 0.
    pub(crate) boost_speed: f32,
    /// Fraction of pre-dash momentum folded into the dash, unitless `[0, 1]`.
    pub(crate) momentum_retention: f32,
    /// In-dash steering authority, unitless `[0, 1]`.
    pub(crate) steer_control: f32,
    /// Decay rate of the dash impulse, world-units/sec². `0` is legitimate.
    pub(crate) dash_drag: f32,
    /// Cooldown between dashes in milliseconds. `0` is legitimate.
    pub(crate) cooldown_ms: f32,
    /// Number of air dashes allowed before landing.
    pub(crate) air_dashes: u32,
    /// Whether the dash preserves the pre-dash vertical velocity.
    pub(crate) preserve_vertical: bool,
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

/// The full bundle returned by a level's `setupLevel(ctx)` export.
///
/// Entity-type descriptors are not part of this manifest — they arrive via
/// `setupMod()`'s `entities` field (mod-init only) and are drained into
/// `DataRegistry` before any level is loaded. `LevelManifest` carries only
/// per-level reactions.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LevelManifest {
    pub(crate) reactions: Vec<NamedReaction>,
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

// --- JS deserialization -----------------------------------------------------

impl LevelManifest {
    /// Deserialize a top-level `{ reactions }` object returned from
    /// a QuickJS `setupLevel()` call.
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

        Ok(Self { reactions })
    }

    /// Deserialize a top-level `{ reactions }` table returned from a
    /// Luau `setupLevel()` call.
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

        Ok(Self { reactions })
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

fn primitive_descriptor_from_js<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
) -> Result<PrimitiveDescriptor, DescriptorError> {
    let primitive = get_required_string_js(obj, "primitive")?;
    let primitive = validate_primitive_name(primitive)?;
    let tag = get_required_string_js(obj, "tag")?;

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

    if obj.contains_key("components").map_err(js_err)? {
        let components_val: JsValue = obj.get("components").map_err(js_err)?;
        if !components_val.is_null() && !components_val.is_undefined() {
            let components_obj =
                Object::from_value(components_val).map_err(|_| DescriptorError::InvalidShape {
                    reason: "`components` must be an object".to_string(),
                })?;
            if components_obj.contains_key("movement").map_err(js_err)? {
                let raw: JsValue = components_obj.get("movement").map_err(js_err)?;
                if !raw.is_null() && !raw.is_undefined() {
                    let m_obj =
                        Object::from_value(raw).map_err(|_| DescriptorError::InvalidShape {
                            reason: "`components.movement` must be an object".to_string(),
                        })?;
                    movement = Some(movement_descriptor_from_js(&m_obj)?);
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
    })
}

fn movement_descriptor_from_js<'js>(
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
            Some(dash_params_from_js(&dash_obj)?)
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

fn dash_params_from_js<'js>(obj: &Object<'js>) -> Result<DashParams, DescriptorError> {
    let boost_speed = validate_positive_finite(
        get_required_f32_js(obj, "boostSpeed")?,
        "movement.dash.boostSpeed",
    )?;
    let momentum_retention = validate_in_range_finite(
        get_required_f32_js(obj, "momentumRetention")?,
        0.0,
        1.0,
        "movement.dash.momentumRetention",
    )?;
    let steer_control = validate_in_range_finite(
        get_required_f32_js(obj, "steerControl")?,
        0.0,
        1.0,
        "movement.dash.steerControl",
    )?;
    let dash_drag = validate_non_negative_finite(
        get_required_f32_js(obj, "dashDrag")?,
        "movement.dash.dashDrag",
    )?;
    let cooldown_ms = validate_non_negative_finite(
        get_required_f32_js(obj, "cooldownMs")?,
        "movement.dash.cooldownMs",
    )?;
    let air_dashes_value = get_required_f32_js(obj, "airDashes")?;
    if !air_dashes_value.is_finite() || air_dashes_value < 0.0 || air_dashes_value.fract() != 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`movement.dash.airDashes` must be a non-negative integer, got {air_dashes_value}"
            ),
        });
    }
    let air_dashes = air_dashes_value as u32;
    let preserve_vertical = get_required_bool_js(obj, "preserveVertical")?;
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

fn primitive_descriptor_from_lua(table: &Table) -> Result<PrimitiveDescriptor, DescriptorError> {
    let primitive = get_required_string_lua(table, "primitive")?;
    let primitive = validate_primitive_name(primitive)?;
    let tag = get_required_string_lua(table, "tag")?;

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
    let boost_speed = validate_positive_finite(
        get_required_f32_lua(table, "boostSpeed")?,
        "movement.dash.boostSpeed",
    )?;
    let momentum_retention = validate_in_range_finite(
        get_required_f32_lua(table, "momentumRetention")?,
        0.0,
        1.0,
        "movement.dash.momentumRetention",
    )?;
    let steer_control = validate_in_range_finite(
        get_required_f32_lua(table, "steerControl")?,
        0.0,
        1.0,
        "movement.dash.steerControl",
    )?;
    let dash_drag = validate_non_negative_finite(
        get_required_f32_lua(table, "dashDrag")?,
        "movement.dash.dashDrag",
    )?;
    let cooldown_ms = validate_non_negative_finite(
        get_required_f32_lua(table, "cooldownMs")?,
        "movement.dash.cooldownMs",
    )?;
    let air_dashes = get_required_u32_lua(table, "airDashes")?;
    let preserve_vertical = get_required_bool_lua(table, "preserveVertical")?;
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
                assert_eq!(p.tag, "reactorChambers");
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
                assert_eq!(p.tag, "reactorChambers");
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
        assert!(!dash.preserve_vertical);
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
        assert!(!dash.preserve_vertical);
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
        assert!(dash.preserve_vertical);
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
}
