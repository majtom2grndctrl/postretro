// Value conversion adapters at the scripting FFI boundary.
// See: context/lib/scripting.md

use mlua::{Lua, Value as LuaValue};
use rquickjs::{Array, Ctx, IntoJs, Object, Value as JsValue};

#[allow(unused_imports)]
pub use super::value_types::{EulerDegrees, Vec3Lit};

const JSON_CONVERSION_MAX_DEPTH: usize = 64;

// payload is a serde_json::Value walked recursively into native objects — no JSON string on the wire.

pub fn json_to_js<'js>(ctx: &Ctx<'js>, v: &serde_json::Value) -> rquickjs::Result<JsValue<'js>> {
    match v {
        serde_json::Value::Null => Ok(JsValue::new_null(ctx.clone())),
        serde_json::Value::Bool(b) => b.into_js(ctx),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                (i as f64).into_js(ctx)
            } else if let Some(f) = n.as_f64() {
                f.into_js(ctx)
            } else {
                Ok(JsValue::new_null(ctx.clone()))
            }
        }
        serde_json::Value::String(s) => s.as_str().into_js(ctx),
        serde_json::Value::Array(arr) => {
            let a = Array::new(ctx.clone())?;
            for (i, item) in arr.iter().enumerate() {
                a.set(i, json_to_js(ctx, item)?)?;
            }
            Ok(a.into_value())
        }
        serde_json::Value::Object(map) => {
            let o = Object::new(ctx.clone())?;
            for (k, v) in map {
                o.set(k.as_str(), json_to_js(ctx, v)?)?;
            }
            Ok(o.into_value())
        }
    }
}

#[allow(clippy::only_used_in_recursion)]
pub fn js_to_json<'js>(ctx: &Ctx<'js>, v: JsValue<'js>) -> rquickjs::Result<serde_json::Value> {
    js_to_json_inner(ctx, v, 0)
}

#[allow(clippy::only_used_in_recursion)]
fn js_to_json_inner<'js>(
    ctx: &Ctx<'js>,
    v: JsValue<'js>,
    depth: usize,
) -> rquickjs::Result<serde_json::Value> {
    if depth >= JSON_CONVERSION_MAX_DEPTH {
        return Err(rquickjs::Error::new_from_js_message(
            "value",
            "JSON-compatible value",
            format!("maximum conversion depth of {JSON_CONVERSION_MAX_DEPTH} exceeded"),
        ));
    }
    if v.is_null() || v.is_undefined() {
        return Ok(serde_json::Value::Null);
    }
    if let Some(b) = v.as_bool() {
        return Ok(serde_json::Value::Bool(b));
    }
    if let Some(i) = v.as_int() {
        return Ok(serde_json::Value::Number(serde_json::Number::from(i)));
    }
    if let Some(f) = v.as_float() {
        return Ok(serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null));
    }
    if let Some(s) = v.as_string() {
        return Ok(serde_json::Value::String(s.to_string()?));
    }
    if let Some(arr) = v.as_array() {
        let mut out = Vec::with_capacity(arr.len());
        for i in 0..arr.len() {
            let item: JsValue = arr.get(i)?;
            out.push(js_to_json_inner(ctx, item, depth + 1)?);
        }
        return Ok(serde_json::Value::Array(out));
    }
    if let Some(obj) = v.as_object() {
        let mut map = serde_json::Map::new();
        for entry in obj.props::<String, JsValue>() {
            let (k, val) = entry?;
            map.insert(k, js_to_json_inner(ctx, val, depth + 1)?);
        }
        return Ok(serde_json::Value::Object(map));
    }
    Ok(serde_json::Value::Null)
}

pub fn json_to_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Result<LuaValue> {
    match v {
        serde_json::Value::Null => Ok(LuaValue::Nil),
        serde_json::Value::Bool(b) => Ok(LuaValue::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(LuaValue::Integer(i as mlua::Integer))
            } else if let Some(f) = n.as_f64() {
                Ok(LuaValue::Number(f))
            } else {
                Ok(LuaValue::Nil)
            }
        }
        serde_json::Value::String(s) => Ok(LuaValue::String(lua.create_string(s)?)),
        serde_json::Value::Array(arr) => {
            let t = lua.create_table()?;
            for (i, item) in arr.iter().enumerate() {
                t.set(i as i64 + 1, json_to_lua(lua, item)?)?;
            }
            Ok(LuaValue::Table(t))
        }
        serde_json::Value::Object(map) => {
            let t = lua.create_table()?;
            for (k, v) in map {
                t.set(k.as_str(), json_to_lua(lua, v)?)?;
            }
            Ok(LuaValue::Table(t))
        }
    }
}

pub fn lua_to_json(value: LuaValue) -> mlua::Result<serde_json::Value> {
    lua_to_json_inner(value, 0)
}

fn lua_to_json_inner(value: LuaValue, depth: usize) -> mlua::Result<serde_json::Value> {
    if depth >= JSON_CONVERSION_MAX_DEPTH {
        return Err(mlua::Error::RuntimeError(format!(
            "maximum conversion depth of {JSON_CONVERSION_MAX_DEPTH} exceeded"
        )));
    }
    match value {
        LuaValue::Nil => Ok(serde_json::Value::Null),
        LuaValue::Boolean(b) => Ok(serde_json::Value::Bool(b)),
        LuaValue::Integer(i) => Ok(serde_json::Value::Number(serde_json::Number::from(i))),
        LuaValue::Number(f) => Ok(serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)),
        LuaValue::String(s) => Ok(serde_json::Value::String(s.to_str()?.to_string())),
        LuaValue::Table(t) => {
            let len = t.raw_len();
            let mut integer_keys = Vec::new();
            let mut has_only_integer_keys = true;
            for pair in t.clone().pairs::<LuaValue, LuaValue>() {
                let (k, _) = pair?;
                match k {
                    LuaValue::Integer(i) => integer_keys.push(i),
                    _ => {
                        has_only_integer_keys = false;
                    }
                }
            }

            if !integer_keys.is_empty() && has_only_integer_keys {
                for &key in &integer_keys {
                    if key < 1 || usize::try_from(key).ok().is_none_or(|key| key > len) {
                        return Err(mlua::Error::FromLuaConversionError {
                            from: "table",
                            to: "JSON array".to_string(),
                            message: Some(format!(
                                "array keys must be exactly the contiguous integer set 1..={len}; found key {key}"
                            )),
                        });
                    }
                }
                if integer_keys.len() != len {
                    return Err(mlua::Error::FromLuaConversionError {
                        from: "table",
                        to: "JSON array".to_string(),
                        message: Some(format!(
                            "array keys must be exactly the contiguous integer set 1..={len}; found {} integer keys",
                            integer_keys.len()
                        )),
                    });
                }

                let mut out = Vec::with_capacity(len);
                for i in 1..=len {
                    let v: LuaValue = t.get(i)?;
                    out.push(lua_to_json_inner(v, depth + 1)?);
                }
                Ok(serde_json::Value::Array(out))
            } else {
                let mut map = serde_json::Map::new();
                for pair in t.pairs::<LuaValue, LuaValue>() {
                    let (k, v) = pair?;
                    let key_str = match k {
                        LuaValue::String(s) => s.to_str()?.to_string(),
                        LuaValue::Integer(i) => i.to_string(),
                        LuaValue::Number(f) => f.to_string(),
                        _ => continue,
                    };
                    map.insert(key_str, lua_to_json_inner(v, depth + 1)?);
                }
                Ok(serde_json::Value::Object(map))
            }
        }
        _ => Ok(serde_json::Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn euler_to_quat_round_trips() {
        let e = EulerDegrees {
            pitch: 15.0,
            yaw: 45.0,
            roll: -30.0,
        };
        let q = e.to_quat();
        let back = EulerDegrees::from_quat(q);
        assert!((back.pitch - e.pitch).abs() < 1e-3, "pitch: {back:?}");
        assert!((back.yaw - e.yaw).abs() < 1e-3, "yaw: {back:?}");
        assert!((back.roll - e.roll).abs() < 1e-3, "roll: {back:?}");
    }

    #[test]
    fn vec3lit_accepts_array_and_object_forms_with_same_value() {
        let from_arr: Vec3Lit = serde_json::from_str("[1.0, 0.0, 0.0]").unwrap();
        let from_obj: Vec3Lit = serde_json::from_str(r#"{"x":1.0,"y":0.0,"z":0.0}"#).unwrap();
        assert_eq!(from_arr, Vec3Lit([1.0, 0.0, 0.0]));
        assert_eq!(from_obj, Vec3Lit([1.0, 0.0, 0.0]));
        assert_eq!(from_arr, from_obj);
    }

    #[test]
    fn lua_to_json_accepts_contiguous_integer_array_keys() {
        let lua = Lua::new();
        let value = lua.load("return { 10, 20, 30 }").eval().unwrap();

        assert_eq!(lua_to_json(value).unwrap(), serde_json::json!([10, 20, 30]));
    }
}
