// Data-context descriptor types: the shape of `registerLevelManifest()` return
// bundles after they cross the script FFI boundary.
// See: context/lib/scripting.md §2 (Data context lifecycle)
//
// Validation (unknown shapes, empty primitive names, out-of-range thresholds)
// runs at deserialization time so descriptor errors surface during level load
// rather than being deferred to dispatch.

use mlua::{Table, Value as LuaValue};
use rquickjs::{Array, Ctx, Object, Value as JsValue};
use thiserror::Error;

/// Variants of a single reaction's behavior body. The `name` lives on the
/// wrapping [`NamedReaction`]; this enum captures only the descriptor shape.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ReactionDescriptor {
    Progress(ProgressDescriptor),
    Primitive(PrimitiveDescriptor),
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
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PrimitiveDescriptor {
    pub(crate) primitive: String,
    pub(crate) tag: String,
    pub(crate) on_complete: Option<String>,
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
    #[error("reaction has no recognizable shape (expected 'progress' or 'primitive' key)")]
    UnknownShape,
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

    // Discriminator: presence of `progress` (object) vs. `primitive` (string).
    let has_progress = obj.contains_key("progress").map_err(js_err)?;
    let has_primitive = obj.contains_key("primitive").map_err(js_err)?;

    let descriptor = match (has_progress, has_primitive) {
        (true, _) => {
            let progress_obj: Object = obj.get("progress").map_err(js_err)?;
            ReactionDescriptor::Progress(progress_descriptor_from_js(ctx, &progress_obj)?)
        }
        (false, true) => {
            ReactionDescriptor::Primitive(primitive_descriptor_from_js(ctx, &obj)?)
        }
        (false, false) => return Err(DescriptorError::UnknownShape),
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
    _ctx: &Ctx<'js>,
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

    Ok(PrimitiveDescriptor {
        primitive,
        tag,
        on_complete,
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

    let descriptor = match (has_progress, has_primitive) {
        (true, _) => {
            let progress: Table = table.get("progress").map_err(lua_err)?;
            ReactionDescriptor::Progress(progress_descriptor_from_lua(&progress)?)
        }
        (false, true) => ReactionDescriptor::Primitive(primitive_descriptor_from_lua(&table)?),
        (false, false) => return Err(DescriptorError::UnknownShape),
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

    Ok(PrimitiveDescriptor {
        primitive,
        tag,
        on_complete,
    })
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

fn get_required_string_lua(
    table: &Table,
    field: &'static str,
) -> Result<String, DescriptorError> {
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
        let err = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap_err());
        assert_eq!(err, DescriptorError::MissingField { field: "fire" });
    }

    #[test]
    fn js_missing_name_field_reports_missing_field() {
        let src = r#"({
            entities: [],
            reactions: [{ progress: { tag: "t", at: 0.5, fire: "f" } }]
        })"#;
        let err = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap_err());
        assert_eq!(err, DescriptorError::MissingField { field: "name" });
    }

    #[test]
    fn js_unknown_shape_reaction_is_rejected() {
        let src = r#"({
            entities: [],
            reactions: [{ name: "x", tag: "t" }]
        })"#;
        let err = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap_err());
        assert_eq!(err, DescriptorError::UnknownShape);
    }

    #[test]
    fn js_empty_primitive_name_is_rejected() {
        let src = r#"({
            entities: [],
            reactions: [{ name: "x", primitive: "", tag: "t" }]
        })"#;
        let err = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap_err());
        assert_eq!(err, DescriptorError::EmptyPrimitiveName);
    }

    #[test]
    fn js_at_out_of_range_high_is_rejected() {
        let src = r#"({
            entities: [],
            reactions: [{ name: "x", progress: { tag: "t", at: 1.5, fire: "f" } }]
        })"#;
        let err = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap_err());
        assert_eq!(err, DescriptorError::AtThresholdOutOfRange { value: 1.5 });
    }

    #[test]
    fn js_at_out_of_range_negative_is_rejected() {
        let src = r#"({
            entities: [],
            reactions: [{ name: "x", progress: { tag: "t", at: -0.1, fire: "f" } }]
        })"#;
        let err = eval_js(src, |ctx, v| LevelManifest::from_js_value(ctx, v).unwrap_err());
        match err {
            DescriptorError::AtThresholdOutOfRange { value } => {
                assert!((value + 0.1).abs() < 1e-6);
            }
            other => panic!("expected AtThresholdOutOfRange, got {other:?}"),
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
    fn lua_empty_arrays_yield_empty_manifest() {
        let src = "return { entities = {}, reactions = {} }";
        let m = eval_lua(src, |v| LevelManifest::from_lua_value(v).unwrap());
        assert!(m.entities.is_empty());
        assert!(m.reactions.is_empty());
    }
}
