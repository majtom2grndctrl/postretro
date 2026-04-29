// Data-context descriptor types: the shape of `registerLevelManifest()` return
// bundles after they cross the script FFI boundary.
// See: context/lib/scripting.md §2 (Data context lifecycle)
//
// Validation on the three discriminator keys ('progress', 'primitive',
// 'sequence') — unknown shapes, empty primitive names, out-of-range thresholds
// — runs at deserialization time so descriptor errors surface during level load
// rather than being deferred to dispatch.

use mlua::{Table, Value as LuaValue};
use rquickjs::{Array, Ctx, Object, Value as JsValue};
use thiserror::Error;

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

/// Currently minimal — entity type descriptors only carry the classname. Spawn
/// behavior, component presets, etc. land in future tasks.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EntityTypeDescriptor {
    pub(crate) classname: String,
}

/// The full bundle returned by a level's `registerLevelManifest(ctx)` export.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LevelManifest {
    pub(crate) reactions: Vec<NamedReaction>,
    pub(crate) entities: Vec<EntityTypeDescriptor>,
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
    /// Deserialize a top-level `{ entities, reactions }` object returned from
    /// a QuickJS `registerLevelManifest()` call.
    pub(crate) fn from_js_value<'js>(
        ctx: &Ctx<'js>,
        value: JsValue<'js>,
    ) -> Result<Self, DescriptorError> {
        let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
            reason: "registerLevelManifest must return an object".to_string(),
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

        let entities = if obj.contains_key("entities").map_err(js_err)? {
            let arr: Array = obj.get("entities").map_err(js_err)?;
            let mut out = Vec::with_capacity(arr.len());
            for i in 0..arr.len() {
                let item: JsValue = arr.get(i).map_err(js_err)?;
                out.push(entity_descriptor_from_js(ctx, item)?);
            }
            out
        } else {
            Vec::new()
        };

        Ok(Self {
            reactions,
            entities,
        })
    }

    /// Deserialize a top-level `{ entities, reactions }` table returned from a
    /// Luau `registerLevelManifest()` call.
    pub(crate) fn from_lua_value(value: LuaValue) -> Result<Self, DescriptorError> {
        let table = match value {
            LuaValue::Table(t) => t,
            other => {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "registerLevelManifest must return a table, got {}",
                        other.type_name()
                    ),
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

        let entities = if table.contains_key("entities").map_err(lua_err)? {
            let arr: Table = table.get("entities").map_err(lua_err)?;
            let len = arr.raw_len();
            let mut out = Vec::with_capacity(len);
            for i in 1..=(len as i64) {
                let item: LuaValue = arr.get(i).map_err(lua_err)?;
                out.push(entity_descriptor_from_lua(item)?);
            }
            out
        } else {
            Vec::new()
        };

        Ok(Self {
            reactions,
            entities,
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

fn entity_descriptor_from_js<'js>(
    _ctx: &Ctx<'js>,
    value: JsValue<'js>,
) -> Result<EntityTypeDescriptor, DescriptorError> {
    let obj = Object::from_value(value).map_err(|_| DescriptorError::InvalidShape {
        reason: "entity entry must be an object".to_string(),
    })?;
    let classname = get_required_string_js(&obj, "classname")?;
    Ok(EntityTypeDescriptor { classname })
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

fn entity_descriptor_from_lua(value: LuaValue) -> Result<EntityTypeDescriptor, DescriptorError> {
    let table = match value {
        LuaValue::Table(t) => t,
        other => {
            return Err(DescriptorError::InvalidShape {
                reason: format!("entity entry must be a table, got {}", other.type_name()),
            });
        }
    };
    let classname = get_required_string_lua(&table, "classname")?;
    Ok(EntityTypeDescriptor { classname })
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
            entities: [{ classname: "grunt" }, { classname: "heavyGunner" }],
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

        assert_eq!(manifest.entities.len(), 2);
        assert_eq!(manifest.entities[0].classname, "grunt");
        assert_eq!(manifest.entities[1].classname, "heavyGunner");

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
            entities: [],
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
            entities: [],
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
            entities: [],
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
            entities: [],
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
            entities: [],
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
            entities: [],
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
            entities: [],
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
            entities: [],
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
            entities: [],
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
        let src = "({ entities: [], reactions: [] })";
        let m = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap());
        assert!(m.entities.is_empty());
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
            entities = { { classname = "grunt" }, { classname = "heavyGunner" } },
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

        assert_eq!(m.entities.len(), 2);
        assert_eq!(m.entities[0].classname, "grunt");
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
            entities = {},
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
            entities = {},
            reactions = { { name = "x", progress = { tag = "t", at = 0.5 } } }
        }"#;
        let err = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap_err());
        assert_eq!(err, DescriptorError::MissingField { field: "fire" });
    }

    #[test]
    fn lua_unknown_shape_reaction_is_rejected() {
        let src = r#"return {
            entities = {},
            reactions = { { name = "x", tag = "t" } }
        }"#;
        let err = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap_err());
        assert_eq!(err, DescriptorError::UnknownShape);
    }

    #[test]
    fn lua_empty_primitive_name_is_rejected() {
        let src = r#"return {
            entities = {},
            reactions = { { name = "x", primitive = "", tag = "t" } }
        }"#;
        let err = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap_err());
        assert_eq!(err, DescriptorError::EmptyPrimitiveName);
    }

    #[test]
    fn lua_at_out_of_range_is_rejected() {
        let src = r#"return {
            entities = {},
            reactions = { { name = "x", progress = { tag = "t", at = 1.5, fire = "f" } } }
        }"#;
        let err = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap_err());
        assert_eq!(err, DescriptorError::AtThresholdOutOfRange { value: 1.5 });
    }

    #[test]
    fn lua_sequence_reaction_deserializes() {
        let src = r#"return {
            entities = {},
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
        let src = "return { entities = {}, reactions = {} }";
        let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
        assert!(m.entities.is_empty());
        assert!(m.reactions.is_empty());
    }
}
