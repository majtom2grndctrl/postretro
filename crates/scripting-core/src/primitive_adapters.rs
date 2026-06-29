// Primitive FFI adapter newtypes shared by postretro-local registrar wiring.
// See: context/lib/scripting.md §4

use std::collections::BTreeMap;

use mlua::{FromLua, IntoLua, Lua, Table as LuaTable, Value as LuaValue};
use rquickjs::{Array, Ctx, FromJs, IntoJs, Object as JsObject, Value as JsValue};
use serde_json::Value;

use crate::conv::{js_to_json, json_to_js, json_to_lua, lua_to_json};
use crate::slot_table::SlotValue;

/// Newtype that maps `None` to JS `null` and Lua `nil`, matching the SDK
/// `string | null` / `string?` surface.
pub struct NullableString(pub Option<String>);

impl<'js> IntoJs<'js> for NullableString {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        match self.0 {
            Some(s) => s.into_js(ctx),
            None => Ok(JsValue::new_null(ctx.clone())),
        }
    }
}

impl IntoLua for NullableString {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        match self.0 {
            Some(s) => s.into_lua(lua),
            None => Ok(LuaValue::Nil),
        }
    }
}

/// Opaque JSON return adapter for serde-shaped primitive results.
pub struct JsonValue(pub Value);

impl<'js> IntoJs<'js> for JsonValue {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        json_to_js(ctx, &self.0)
    }
}

impl IntoLua for JsonValue {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        json_to_lua(lua, &self.0)
    }
}

/// Filter object adapter for the `worldQuery` primitive.
pub struct WorldQueryFilterInput {
    pub component: String,
    pub tag: Option<String>,
}

impl<'js> FromJs<'js> for WorldQueryFilterInput {
    fn from_js(_ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        let obj = rquickjs::Object::from_value(value)
            .map_err(|_| rquickjs::Error::new_from_js("value", "WorldQueryFilter object"))?;
        let component: String = obj.get("component")?;
        let tag: Option<String> = obj.get("tag")?;
        Ok(Self { component, tag })
    }
}

impl FromLua for WorldQueryFilterInput {
    fn from_lua(value: LuaValue, _lua: &Lua) -> mlua::Result<Self> {
        let t = match value {
            LuaValue::Table(t) => t,
            other => {
                return Err(mlua::Error::FromLuaConversionError {
                    from: other.type_name(),
                    to: "WorldQueryFilter".to_string(),
                    message: Some("expected a table".to_string()),
                });
            }
        };
        let component: String = t.get("component")?;
        let tag: Option<String> = t.get("tag")?;
        Ok(Self { component, tag })
    }
}

pub struct StoreSchemaJson(pub Value);

impl<'js> FromJs<'js> for StoreSchemaJson {
    fn from_js(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        js_to_json(ctx, value).map(Self)
    }
}

impl FromLua for StoreSchemaJson {
    fn from_lua(value: LuaValue, _lua: &Lua) -> mlua::Result<Self> {
        lua_to_json(value).map(Self)
    }
}

#[derive(Clone, Debug)]
pub enum ScriptSlotValue {
    Number(f64),
    Boolean(bool),
    String(String),
    Array(Vec<f64>),
    Unsupported(&'static str),
}

impl<'js> FromJs<'js> for ScriptSlotValue {
    fn from_js(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        if let Some(value) = value.as_bool() {
            return Ok(Self::Boolean(value));
        }
        if let Some(value) = value.as_int() {
            return Ok(Self::Number(f64::from(value)));
        }
        if let Some(value) = value.as_float() {
            return Ok(Self::Number(value));
        }
        if let Some(value) = value.as_string() {
            return Ok(Self::String(value.to_string()?));
        }
        if let Some(array) = value.as_array() {
            let mut values = Vec::with_capacity(array.len());
            for index in 0..array.len() {
                let item: JsValue = array.get(index)?;
                let number = if let Some(value) = item.as_int() {
                    f64::from(value)
                } else if let Some(value) = item.as_float() {
                    value
                } else {
                    return Ok(Self::Unsupported("array containing a non-number"));
                };
                values.push(number);
            }
            return Ok(Self::Array(values));
        }
        let _ = ctx;
        Ok(Self::Unsupported("unsupported value"))
    }
}

impl FromLua for ScriptSlotValue {
    fn from_lua(value: LuaValue, _lua: &Lua) -> mlua::Result<Self> {
        match value {
            LuaValue::Boolean(value) => Ok(Self::Boolean(value)),
            LuaValue::Integer(value) => Ok(Self::Number(value as f64)),
            LuaValue::Number(value) => Ok(Self::Number(value)),
            LuaValue::String(value) => Ok(Self::String(value.to_str()?.to_string())),
            LuaValue::Table(table) => {
                let len = table.raw_len();
                for pair in table.clone().pairs::<LuaValue, LuaValue>() {
                    let (key, _) = pair?;
                    let LuaValue::Integer(index) = key else {
                        return Ok(Self::Unsupported("table with non-array keys"));
                    };
                    if index < 1 || index as usize > len {
                        return Ok(Self::Unsupported("sparse array"));
                    }
                }

                let mut values = Vec::with_capacity(len);
                for index in 1..=len {
                    match table.get::<LuaValue>(index)? {
                        LuaValue::Integer(value) => values.push(value as f64),
                        LuaValue::Number(value) => values.push(value),
                        _ => return Ok(Self::Unsupported("array containing a non-number")),
                    }
                }
                Ok(Self::Array(values))
            }
            _ => Ok(Self::Unsupported("unsupported value")),
        }
    }
}

pub struct Any(pub SlotValue);

impl<'js> IntoJs<'js> for Any {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        match self.0 {
            SlotValue::Number(value) => value.into_js(ctx),
            SlotValue::Boolean(value) => value.into_js(ctx),
            SlotValue::String(value) | SlotValue::Enum(value) => value.into_js(ctx),
            SlotValue::Array(values) => {
                let array = Array::new(ctx.clone())?;
                for (index, value) in values.into_iter().enumerate() {
                    array.set(index, value)?;
                }
                Ok(array.into_value())
            }
        }
    }
}

impl IntoLua for Any {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        match self.0 {
            SlotValue::Number(value) => value.into_lua(lua),
            SlotValue::Boolean(value) => value.into_lua(lua),
            SlotValue::String(value) | SlotValue::Enum(value) => value.into_lua(lua),
            SlotValue::Array(values) => values.into_lua(lua),
        }
    }
}

#[derive(Debug)]
pub struct StoreStateRefs(pub BTreeMap<String, String>);

impl<'js> IntoJs<'js> for StoreStateRefs {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        let object = JsObject::new(ctx.clone())?;
        for (slot_name, dotted_name) in self.0 {
            let reference = JsObject::new(ctx.clone())?;
            reference.set("slot", dotted_name)?;
            object.set(slot_name, reference)?;
        }
        Ok(object.into_value())
    }
}

impl IntoLua for StoreStateRefs {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        let table: LuaTable = lua.create_table()?;
        for (slot_name, dotted_name) in self.0 {
            let reference = lua.create_table()?;
            reference.set("slot", dotted_name)?;
            table.set(slot_name, reference)?;
        }
        Ok(LuaValue::Table(table))
    }
}

#[derive(Debug)]
pub struct StoreDefinition {
    pub declaration: StoreDeclarationManifest,
    pub state: StoreStateRefs,
}

impl<'js> IntoJs<'js> for StoreDefinition {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        let object = JsObject::new(ctx.clone())?;
        object.set("declaration", self.declaration)?;
        object.set("state", self.state)?;
        Ok(object.into_value())
    }
}

impl IntoLua for StoreDefinition {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        let table = lua.create_table()?;
        table.set("declaration", self.declaration)?;
        table.set("state", self.state)?;
        Ok(LuaValue::Table(table))
    }
}

#[derive(Debug)]
pub struct StoreDeclarationManifest {
    pub namespace: String,
    pub schema: Value,
}

impl<'js> IntoJs<'js> for StoreDeclarationManifest {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        let object = JsObject::new(ctx.clone())?;
        object.set("namespace", self.namespace)?;
        object.set("schema", json_to_js(ctx, &self.schema)?)?;
        Ok(object.into_value())
    }
}

impl IntoLua for StoreDeclarationManifest {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        let table = lua.create_table()?;
        table.set("namespace", self.namespace)?;
        table.set("schema", json_to_lua(lua, &self.schema)?)?;
        Ok(LuaValue::Table(table))
    }
}
