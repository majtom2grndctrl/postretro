// ScriptCallContext — the argument passed to `tick` handlers.
// See: context/lib/scripting.md
//
// `delta` and `time` come from the engine frame timer, not a separate clock.
// Scripts have no wall-clock access; this struct is the only temporal surface
// handlers see.

use mlua::{FromLua, IntoLua, Lua, Value as LuaValue};
use rquickjs::{Ctx, FromJs, IntoJs, Object, Value as JsValue};

/// Passed to `tick` handlers. `levelLoad` handlers receive no argument.
///
/// * `delta` — seconds elapsed since the previous frame's `tick` invocation
///   (wall-clock `frame_dt` from the engine frame timer). `tick` fires once
///   per render frame, not per fixed simulation tick, so this value varies
///   with frame rate. Use it to scale per-frame motion; do not assume it
///   matches the fixed tick interval.
/// * `time` — seconds since level load; resets to zero on each level load,
///   monotonic within a level. Accumulates the same `frame_dt` `delta`
///   carries each frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ScriptCallContext {
    pub(crate) delta: f32,
    pub(crate) time: f32,
}

impl<'js> IntoJs<'js> for ScriptCallContext {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        let o = Object::new(ctx.clone())?;
        o.set("delta", self.delta)?;
        o.set("time", self.time)?;
        Ok(o.into_value())
    }
}

impl<'js> FromJs<'js> for ScriptCallContext {
    fn from_js(_ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        let obj = value.into_object().ok_or_else(|| {
            rquickjs::Error::new_from_js("value", "ScriptCallContext (expected object)")
        })?;
        Ok(Self {
            delta: obj.get("delta")?,
            time: obj.get("time")?,
        })
    }
}

impl IntoLua for ScriptCallContext {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        let t = lua.create_table()?;
        t.set("delta", self.delta)?;
        t.set("time", self.time)?;
        Ok(LuaValue::Table(t))
    }
}

impl FromLua for ScriptCallContext {
    fn from_lua(value: LuaValue, _lua: &Lua) -> mlua::Result<Self> {
        match value {
            LuaValue::Table(t) => Ok(Self {
                delta: t.get("delta")?,
                time: t.get("time")?,
            }),
            other => Err(mlua::Error::FromLuaConversionError {
                from: other.type_name(),
                to: "ScriptCallContext".to_string(),
                message: Some("expected table".into()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_quickjs() {
        let rt = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&rt).unwrap();
        ctx.with(|ctx| {
            let input = ScriptCallContext {
                delta: 0.016_667,
                time: 1.5,
            };
            let v = input.into_js(&ctx).unwrap();
            let back = ScriptCallContext::from_js(&ctx, v).unwrap();
            assert!((back.delta - input.delta).abs() < 1e-6);
            assert!((back.time - input.time).abs() < 1e-6);
        });
    }

    #[test]
    fn round_trip_luau() {
        let lua = Lua::new();
        let input = ScriptCallContext {
            delta: 0.016_667,
            time: 1.5,
        };
        let v = input.into_lua(&lua).unwrap();
        let back = ScriptCallContext::from_lua(v, &lua).unwrap();
        assert!((back.delta - input.delta).abs() < 1e-6);
        assert!((back.time - input.time).abs() < 1e-6);
    }
}
