// Value conversion adapters at the scripting FFI boundary.
// See: context/lib/scripting.md

use mlua::{Lua, Value as LuaValue};
use rquickjs::{Array, Ctx, IntoJs, Object, Value as JsValue};

#[allow(unused_imports)]
pub use super::value_types::{EulerDegrees, Vec3Lit};

const JSON_CONVERSION_MAX_DEPTH: usize = 64;

// payload is a serde_json::Value walked recursively into native objects — no JSON string on the wire.

/// One step of the authored path to a value, used only to name the field in a
/// conversion error.
#[derive(Clone, Copy)]
enum PathSeg<'p> {
    Key(&'p str),
    /// JSON array position, always 0-based — Luau's 1-based table index is
    /// rebased here so equivalent data yields the same path in both runtimes.
    Index(usize),
}

/// Borrowed path segment plus its parent link. Built on the stack as the walk
/// descends, so a successful conversion allocates nothing for it.
#[derive(Clone, Copy)]
struct ConvPath<'p> {
    parent: Option<&'p ConvPath<'p>>,
    seg: PathSeg<'p>,
}

impl<'p> ConvPath<'p> {
    fn child(parent: Option<&'p ConvPath<'p>>, seg: PathSeg<'p>) -> ConvPath<'p> {
        ConvPath { parent, seg }
    }
}

fn render_path(path: Option<&ConvPath<'_>>) -> String {
    fn walk(node: &ConvPath<'_>, out: &mut String) {
        if let Some(parent) = node.parent {
            walk(parent, out);
        }
        match node.seg {
            PathSeg::Key(k) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(k);
            }
            PathSeg::Index(i) => {
                out.push('[');
                out.push_str(&i.to_string());
                out.push(']');
            }
        }
    }

    let mut out = String::new();
    if let Some(node) = path {
        walk(node, &mut out);
    }
    if out.is_empty() {
        // The converted value is itself the number — no field to name.
        out.push_str("<root>");
    }
    out
}

/// Shared rejection text for both runtimes. Parity is the contract here: a
/// value that fails in one runtime must fail in the other with the same reason
/// and the same path.
///
/// Why reject rather than degrade: JSON cannot spell `Infinity` or `NaN`, so
/// the old fallback emitted null — and every `Option<T>` descriptor field reads
/// null as "unauthored, use the default". That split one authoring mistake into
/// two outcomes: `engagementRadius: -1` errored cleanly through descriptor
/// validation, while `engagementRadius: Infinity` silently became the default.
/// Every optional numeric field on every descriptor, in both runtimes, went
/// through that seam. Rejecting here puts both on the validation path.
fn non_finite_message(path: Option<&ConvPath<'_>>, value: f64) -> String {
    format!(
        "non-finite number at `{}`: {value} — authored numbers must be finite",
        render_path(path)
    )
}

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
    js_to_json_inner(ctx, v, 0, None)
}

#[allow(clippy::only_used_in_recursion)]
fn js_to_json_inner<'js>(
    ctx: &Ctx<'js>,
    v: JsValue<'js>,
    depth: usize,
    path: Option<&ConvPath<'_>>,
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
        // QuickJS tags only signed 32-bit values as integers. Preserve every
        // other exactly represented JavaScript integer as a JSON integer too,
        // so serde sees the same numeric shape as it does from Luau. Restrict
        // normalization to JavaScript's safe-integer range; outside it the
        // source number may already have lost integer precision.
        if f.is_finite() && f.fract() == 0.0 && f.abs() <= 9_007_199_254_740_991.0 {
            let number = if f >= 0.0 {
                serde_json::Number::from(f as u64)
            } else {
                serde_json::Number::from(f as i64)
            };
            return Ok(serde_json::Value::Number(number));
        }
        // `from_f64` returns `None` for exactly the non-finite cases, so this is
        // the Infinity/-Infinity/NaN rejection. See `non_finite_message`.
        return serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .ok_or_else(|| {
                rquickjs::Error::new_from_js_message(
                    "number",
                    "JSON number",
                    non_finite_message(path, f),
                )
            });
    }
    if let Some(s) = v.as_string() {
        return Ok(serde_json::Value::String(s.to_string()?));
    }
    if let Some(arr) = v.as_array() {
        let mut out = Vec::with_capacity(arr.len());
        for i in 0..arr.len() {
            let item: JsValue = arr.get(i)?;
            let child = ConvPath::child(path, PathSeg::Index(i));
            out.push(js_to_json_inner(ctx, item, depth + 1, Some(&child))?);
        }
        return Ok(serde_json::Value::Array(out));
    }
    if let Some(obj) = v.as_object() {
        let mut map = serde_json::Map::new();
        for entry in obj.props::<String, JsValue>() {
            let (k, val) = entry?;
            // JSON.stringify and ordinary TypeScript authoring both treat an
            // undefined object property as absent. Arrays retain their slot
            // and continue to convert undefined to null above.
            if val.is_undefined() {
                continue;
            }
            let child = ConvPath::child(path, PathSeg::Key(k.as_str()));
            let converted = js_to_json_inner(ctx, val, depth + 1, Some(&child))?;
            map.insert(k, converted);
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
    lua_to_json_inner(value, 0, None)
}

fn lua_to_json_inner(
    value: LuaValue,
    depth: usize,
    path: Option<&ConvPath<'_>>,
) -> mlua::Result<serde_json::Value> {
    if depth >= JSON_CONVERSION_MAX_DEPTH {
        return Err(mlua::Error::RuntimeError(format!(
            "maximum conversion depth of {JSON_CONVERSION_MAX_DEPTH} exceeded"
        )));
    }
    match value {
        LuaValue::Nil => Ok(serde_json::Value::Null),
        LuaValue::Boolean(b) => Ok(serde_json::Value::Bool(b)),
        LuaValue::Integer(i) => Ok(serde_json::Value::Number(serde_json::Number::from(i))),
        // `from_f64` returns `None` for exactly the non-finite cases, so this is
        // the math.huge/-math.huge/NaN rejection — the twin of the QuickJS one.
        // See `non_finite_message`.
        LuaValue::Number(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .ok_or_else(|| mlua::Error::FromLuaConversionError {
                from: "number",
                to: "JSON number".to_string(),
                message: Some(non_finite_message(path, f)),
            }),
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
                    let child = ConvPath::child(path, PathSeg::Index(i - 1));
                    out.push(lua_to_json_inner(v, depth + 1, Some(&child))?);
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
                    let child = ConvPath::child(path, PathSeg::Key(key_str.as_str()));
                    let converted = lua_to_json_inner(v, depth + 1, Some(&child))?;
                    map.insert(key_str, converted);
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

    #[test]
    fn js_to_json_preserves_safe_integral_numbers_above_i32() {
        let rt = rquickjs::Runtime::new().unwrap();
        let js = rquickjs::Context::full(&rt).unwrap();
        js.with(|ctx| {
            for (source, expected) in [
                ("2147483648", serde_json::json!(2_147_483_648_u64)),
                ("4294967295", serde_json::json!(4_294_967_295_u64)),
                ("-2147483649", serde_json::json!(-2_147_483_649_i64)),
            ] {
                let value: JsValue = ctx.eval(source).unwrap();
                assert_eq!(js_to_json(&ctx, value).unwrap(), expected);
            }
        });
    }

    #[test]
    fn both_walkers_reject_a_non_finite_number_at_the_same_path() {
        // The rejection is only useful if the author can find the field, and a
        // path that differs between the runtimes is the divergence this seam
        // exists to prevent. Luau's 1-based index is rebased to the JSON array
        // position, so the twins name the identical path for the same data.
        let expected = "non-finite number at `curve[1].t`";

        let rt = rquickjs::Runtime::new().unwrap();
        let js = rquickjs::Context::full(&rt).unwrap();
        js.with(|ctx| {
            let value: JsValue = ctx
                .eval("({ curve: [{ t: 0 }, { t: Infinity }] })")
                .unwrap();
            let err = js_to_json(&ctx, value).unwrap_err().to_string();
            assert!(err.contains(expected), "QuickJS: {err}");
        });

        let lua = Lua::new();
        let value: LuaValue = lua
            .load("return { curve = { { t = 0 }, { t = math.huge } } }")
            .eval()
            .unwrap();
        let err = lua_to_json(value).unwrap_err().to_string();
        assert!(err.contains(expected), "Luau: {err}");
    }

    #[test]
    fn both_walkers_name_the_root_for_a_bare_non_finite_number() {
        // Primitive arguments convert a bare value with no enclosing field.
        let rt = rquickjs::Runtime::new().unwrap();
        let js = rquickjs::Context::full(&rt).unwrap();
        js.with(|ctx| {
            let value: JsValue = ctx.eval("NaN").unwrap();
            let err = js_to_json(&ctx, value).unwrap_err().to_string();
            assert!(
                err.contains("non-finite number at `<root>`"),
                "QuickJS: {err}"
            );
        });

        let lua = Lua::new();
        let value: LuaValue = lua.load("return -math.huge").eval().unwrap();
        let err = lua_to_json(value).unwrap_err().to_string();
        assert!(err.contains("non-finite number at `<root>`"), "Luau: {err}");
    }

    #[test]
    fn js_to_json_omits_undefined_object_properties_but_preserves_array_slots() {
        let rt = rquickjs::Runtime::new().unwrap();
        let js = rquickjs::Context::full(&rt).unwrap();
        js.with(|ctx| {
            let value: JsValue = ctx
                .eval("({ omitted: undefined, explicit: null, values: [undefined, null] })")
                .unwrap();
            assert_eq!(
                js_to_json(&ctx, value).unwrap(),
                serde_json::json!({ "explicit": null, "values": [null, null] })
            );
        });
    }
}
