// Data-context descriptor types: `LevelManifest`/`ReactionDescriptor`, `EntityTypeDescriptor`,
// `LightDescriptor`, and `PlayerMovementDescriptor`; JS and Luau deserialization paths for all of them.
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
/// `player_spawn` marker, or — future — by tag from another spawn). Absence
/// is structural: descriptors with no `canonical_name` cannot be matched
/// against a `MapEntity.classname` by the data-archetype dispatch.
///
/// Optional `light` / `emitter` / `movement` carry per-entity-type component
/// presets. The level-load spawn path materializes these into a fresh ECS
/// entity per matching placement.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EntityTypeDescriptor {
    pub(crate) canonical_name: Option<String>,
    pub(crate) light: Option<LightDescriptor>,
    pub(crate) emitter: Option<BillboardEmitterComponent>,
    pub(crate) movement: Option<PlayerMovementDescriptor>,
}

/// Authored player-movement component preset. All fields are required when
/// `movement` is present; the data-archetype spawn path materializes the
/// runtime movement component from this. `ground.max_slope` is in degrees on
/// the wire and converted to a cosine at materialization (not here).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlayerMovementDescriptor {
    pub(crate) capsule: CapsuleParams,
    pub(crate) ground: GroundParams,
    pub(crate) air: AirParams,
    pub(crate) fall: FallParams,
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
    pub(crate) speed: f32,
    pub(crate) accel: f32,
    pub(crate) jump_velocity: f32,
    pub(crate) step_height: f32,
    pub(crate) max_slope: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AirParams {
    pub(crate) forward_steer: f32,
    pub(crate) accel: f32,
    pub(crate) max_control_speed: f32,
    pub(crate) bunny_hop: bool,
    pub(crate) jumps: u32,
    pub(crate) jump_ceiling: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FallParams {
    pub(crate) terminal_velocity: f32,
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
/// `{ canonicalName?: string, components?: { light?: LightDescriptor, emitter?: BillboardEmitterComponent, movement?: PlayerMovementDescriptor } }`.
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

    let mut light = None;
    let mut emitter = None;
    let mut movement = None;

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
        light,
        emitter,
        movement,
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
    let ground = GroundParams {
        speed: validate_non_negative_finite(
            get_required_f32_js(&ground_obj, "speed")?,
            "movement.ground.speed",
        )?,
        accel: validate_non_negative_finite(
            get_required_f32_js(&ground_obj, "accel")?,
            "movement.ground.accel",
        )?,
        jump_velocity: validate_non_negative_finite(
            get_required_f32_js(&ground_obj, "jumpVelocity")?,
            "movement.ground.jumpVelocity",
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

    Ok(PlayerMovementDescriptor {
        capsule,
        ground,
        air,
        fall,
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
/// `{ canonicalName?: string, components?: { light?: LightDescriptor, emitter?: BillboardEmitterComponent, movement?: PlayerMovementDescriptor } }`.
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

    let mut light = None;
    let mut emitter = None;
    let mut movement = None;

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
        light,
        emitter,
        movement,
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
    let ground = GroundParams {
        speed: validate_non_negative_finite(
            get_required_f32_lua(&ground_table, "speed")?,
            "movement.ground.speed",
        )?,
        accel: validate_non_negative_finite(
            get_required_f32_lua(&ground_table, "accel")?,
            "movement.ground.accel",
        )?,
        jump_velocity: validate_non_negative_finite(
            get_required_f32_lua(&ground_table, "jumpVelocity")?,
            "movement.ground.jumpVelocity",
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

    Ok(PlayerMovementDescriptor {
        capsule,
        ground,
        air,
        fall,
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
        assert!(d.light.is_none());
        assert!(d.emitter.is_none());
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

    // --- PlayerMovementDescriptor parsing ----------------------------------

    const JS_PLAYER_MOVEMENT: &str = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: 7.0, accel: 10.0, jumpVelocity: 5.5, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpCeiling: 0.0 },
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
        assert_eq!(m.ground.speed, 7.0);
        assert_eq!(m.ground.max_slope, 45.0);
        assert_eq!(m.air.forward_steer, 0.0);
        assert!(!m.air.bunny_hop);
        assert_eq!(m.air.jumps, 0);
        assert_eq!(m.fall.terminal_velocity, 40.0);
    }

    #[test]
    fn js_movement_missing_air_field_reports_missing_field() {
        // `bunnyHop` removed from `air`.
        let src = r#"({
            canonicalName: "player",
            components: {
                movement: {
                    capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                    ground: { speed: 7.0, accel: 10.0, jumpVelocity: 5.5, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, jumps: 0, jumpCeiling: 0.0 },
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
                    ground: { speed: 7.0, accel: 10.0, jumpVelocity: 5.5, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 2 },
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
                    ground: { speed: 7.0, accel: 10.0, jumpVelocity: 5.5, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpCeiling: 0.0 },
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
                    ground: { speed: 7.0, accel: 10.0, jumpVelocity: 5.5, stepHeight: 0.3, maxSlope: 95.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpCeiling: 0.0 },
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
                    ground: { speed: 7.0, accel: 10.0, jumpVelocity: 5.5, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 1.5, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpCeiling: 0.0 },
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
                    ground: { speed: 7.0, accel: 10.0, jumpVelocity: 5.5, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpCeiling: 0.0 },
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
                    ground: { speed: 7.0, accel: 10.0, jumpVelocity: 5.5, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpCeiling: 0.0 },
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
                    ground: { speed: 7.0, accel: 10.0, jumpVelocity: 5.5, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpCeiling: 0.0 },
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
                    ground: { speed: 7.0, accel: 10.0, jumpVelocity: 5.5, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpCeiling: 0.0 },
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
                    ground: { speed: 7.0, accel: 10.0, jumpVelocity: 5.5, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpCeiling: 0.0 },
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
                    ground = { speed = 7.0, accel = 10.0, jumpVelocity = 5.5, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpCeiling = 0.0 },
                    fall = { terminalVelocity = 40.0 }
                }
            }
        }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        let m = d.movement.expect("movement present");
        assert_eq!(m.capsule.eye_height, 0.5);
        assert_eq!(m.ground.jump_velocity, 5.5);
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
                    ground = { speed = 7.0, accel = 10.0, jumpVelocity = 5.5, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpCeiling = 0.0 },
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
                    ground = { speed = 7.0, accel = 10.0, jumpVelocity = 5.5, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 2 },
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
                    ground = { speed = 7.0, accel = 10.0, jumpVelocity = 5.5, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpCeiling = 0.0 },
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
                    ground: { speed: 7.0, accel: 10.0, jumpVelocity: 5.5, stepHeight: 0.3, maxSlope: 45.0 },
                    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0 },
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
                    ground = { speed = 7.0, accel = 10.0, jumpVelocity = 5.5, stepHeight = 0.3, maxSlope = 45.0 },
                    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0 },
                    fall = { terminalVelocity = 40.0 }
                }
            }
        }"#;
        let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
        let m = d.movement.expect("movement present");
        assert_eq!(m.air.jumps, 0);
        assert_eq!(m.air.jump_ceiling, 0.0);
    }
}

