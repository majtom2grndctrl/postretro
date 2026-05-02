// Value conversion adapters at the scripting FFI boundary.
// See: context/lib/scripting.md

use glam::{EulerRot, Quat, Vec3};
use mlua::{FromLua, IntoLua, Lua, Table, Value as LuaValue};
use rquickjs::{Array, Ctx, FromJs, IntoJs, Object, Value as JsValue};
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use std::fmt;

use super::components::light::{LightAnimation, LightComponent};
use super::ctx::ScriptEvent;
use super::data_descriptors::{
    EntityTypeDescriptor, entity_descriptor_from_js, entity_descriptor_from_lua,
};
use super::registry::{ComponentKind, ComponentValue, EntityId, FogVolumeComponent, Transform};

/// Three-component float vector with a permissive Deserialize accepting
/// either a JSON array of 3 numbers or an object with `x`/`y`/`z` keys.
/// Accepts both forms because the SDK emits `{x,y,z}` objects while some
/// callers and tests emit raw `[x, y, z]` arrays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Vec3Lit(pub [f32; 3]);

impl Vec3Lit {
    pub(crate) fn as_f32_3(&self) -> [f32; 3] {
        self.0
    }
}

impl Serialize for Vec3Lit {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let [x, y, z] = self.0;
        let mut st = serializer.serialize_struct("Vec3", 3)?;
        st.serialize_field("x", &x)?;
        st.serialize_field("y", &y)?;
        st.serialize_field("z", &z)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for Vec3Lit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Vec3LitVisitor;

        impl<'de> Visitor<'de> for Vec3LitVisitor {
            type Value = Vec3Lit;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an array [x, y, z] or object { x, y, z }")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec3Lit, A::Error> {
                let x: f32 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &"3 elements"))?;
                let y: f32 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &"3 elements"))?;
                let z: f32 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(2, &"3 elements"))?;
                if seq.next_element::<f32>()?.is_some() {
                    return Err(de::Error::invalid_length(4, &"3 elements"));
                }
                Ok(Vec3Lit([x, y, z]))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Vec3Lit, A::Error> {
                let mut x: Option<f32> = None;
                let mut y: Option<f32> = None;
                let mut z: Option<f32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "x" => x = Some(map.next_value()?),
                        "y" => y = Some(map.next_value()?),
                        "z" => z = Some(map.next_value()?),
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(Vec3Lit([
                    x.ok_or_else(|| de::Error::missing_field("x"))?,
                    y.ok_or_else(|| de::Error::missing_field("y"))?,
                    z.ok_or_else(|| de::Error::missing_field("z"))?,
                ]))
            }
        }

        deserializer.deserialize_any(Vec3LitVisitor)
    }
}

/// Script-facing rotation representation. Angles are in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EulerDegrees {
    pub(crate) pitch: f32,
    pub(crate) yaw: f32,
    pub(crate) roll: f32,
}

impl EulerDegrees {
    /// Convert into the engine-internal `Quat`. Order: YXZ (yaw around
    /// world-up first, then pitch, then roll) — matches the FPS authoring convention.
    pub(crate) fn to_quat(self) -> Quat {
        Quat::from_euler(
            EulerRot::YXZ,
            self.yaw.to_radians(),
            self.pitch.to_radians(),
            self.roll.to_radians(),
        )
    }

    /// Inverse of [`Self::to_quat`]. `glam::Quat::to_euler` returns radians
    /// in the same YXZ order we pack.
    pub(crate) fn from_quat(q: Quat) -> Self {
        let (yaw, pitch, roll) = q.to_euler(EulerRot::YXZ);
        Self {
            pitch: pitch.to_degrees(),
            yaw: yaw.to_degrees(),
            roll: roll.to_degrees(),
        }
    }
}

// EntityId crosses as a raw u32; both JS and Luau number (f64) losslessly hold a 32-bit integer.

impl<'js> FromJs<'js> for EntityId {
    fn from_js(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        let raw = u32::from_js(ctx, value)?;
        Ok(EntityId::from_raw(raw))
    }
}

impl<'js> IntoJs<'js> for EntityId {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        self.to_raw().into_js(ctx)
    }
}

impl FromLua for EntityId {
    fn from_lua(value: LuaValue, lua: &Lua) -> mlua::Result<Self> {
        let raw = u32::from_lua(value, lua)?;
        Ok(EntityId::from_raw(raw))
    }
}

impl IntoLua for EntityId {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        self.to_raw().into_lua(lua)
    }
}

fn vec3_to_js<'js>(ctx: &Ctx<'js>, v: Vec3) -> rquickjs::Result<Object<'js>> {
    let o = Object::new(ctx.clone())?;
    o.set("x", v.x)?;
    o.set("y", v.y)?;
    o.set("z", v.z)?;
    Ok(o)
}

fn vec3_from_js_object<'js>(_ctx: &Ctx<'js>, o: &Object<'js>) -> rquickjs::Result<Vec3> {
    let x: f32 = o.get("x")?;
    let y: f32 = o.get("y")?;
    let z: f32 = o.get("z")?;
    Ok(Vec3::new(x, y, z))
}

fn vec3_to_lua_table(lua: &Lua, v: Vec3) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("x", v.x)?;
    t.set("y", v.y)?;
    t.set("z", v.z)?;
    Ok(t)
}

fn vec3_from_lua_table(t: &Table) -> mlua::Result<Vec3> {
    let x: f32 = t.get("x")?;
    let y: f32 = t.get("y")?;
    let z: f32 = t.get("z")?;
    Ok(Vec3::new(x, y, z))
}

fn euler_to_js<'js>(ctx: &Ctx<'js>, e: EulerDegrees) -> rquickjs::Result<Object<'js>> {
    let o = Object::new(ctx.clone())?;
    o.set("pitch", e.pitch)?;
    o.set("yaw", e.yaw)?;
    o.set("roll", e.roll)?;
    Ok(o)
}

fn euler_from_js_object<'js>(_ctx: &Ctx<'js>, o: &Object<'js>) -> rquickjs::Result<EulerDegrees> {
    Ok(EulerDegrees {
        pitch: o.get("pitch")?,
        yaw: o.get("yaw")?,
        roll: o.get("roll")?,
    })
}

fn euler_to_lua_table(lua: &Lua, e: EulerDegrees) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("pitch", e.pitch)?;
    t.set("yaw", e.yaw)?;
    t.set("roll", e.roll)?;
    Ok(t)
}

fn euler_from_lua_table(t: &Table) -> mlua::Result<EulerDegrees> {
    Ok(EulerDegrees {
        pitch: t.get("pitch")?,
        yaw: t.get("yaw")?,
        roll: t.get("roll")?,
    })
}

impl<'js> FromJs<'js> for Transform {
    fn from_js(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        let o = Object::from_value(value).map_err(|_| {
            rquickjs::Error::new_from_js("value", "Transform object { position, rotation, scale }")
        })?;
        let pos: Object = o.get("position")?;
        let rot: Object = o.get("rotation")?;
        let scale: Object = o.get("scale")?;
        Ok(Transform {
            position: vec3_from_js_object(ctx, &pos)?,
            rotation: euler_from_js_object(ctx, &rot)?.to_quat(),
            scale: vec3_from_js_object(ctx, &scale)?,
        })
    }
}

impl<'js> IntoJs<'js> for Transform {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        let o = Object::new(ctx.clone())?;
        o.set("position", vec3_to_js(ctx, self.position)?)?;
        o.set(
            "rotation",
            euler_to_js(ctx, EulerDegrees::from_quat(self.rotation))?,
        )?;
        o.set("scale", vec3_to_js(ctx, self.scale)?)?;
        Ok(o.into_value())
    }
}

impl FromLua for Transform {
    fn from_lua(value: LuaValue, _lua: &Lua) -> mlua::Result<Self> {
        let t = match value {
            LuaValue::Table(t) => t,
            other => {
                return Err(mlua::Error::FromLuaConversionError {
                    from: other.type_name(),
                    to: "Transform".to_string(),
                    message: Some("expected a table".to_string()),
                });
            }
        };
        let pos: Table = t.get("position")?;
        let rot: Table = t.get("rotation")?;
        let scale: Table = t.get("scale")?;
        Ok(Transform {
            position: vec3_from_lua_table(&pos)?,
            rotation: euler_from_lua_table(&rot)?.to_quat(),
            scale: vec3_from_lua_table(&scale)?,
        })
    }
}

impl IntoLua for Transform {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        let t = lua.create_table()?;
        t.set("position", vec3_to_lua_table(lua, self.position)?)?;
        t.set(
            "rotation",
            euler_to_lua_table(lua, EulerDegrees::from_quat(self.rotation))?,
        )?;
        t.set("scale", vec3_to_lua_table(lua, self.scale)?)?;
        Ok(LuaValue::Table(t))
    }
}

fn component_kind_name(k: ComponentKind) -> &'static str {
    match k {
        ComponentKind::Transform => "transform",
        ComponentKind::Light => "light",
        ComponentKind::BillboardEmitter => "billboard_emitter",
        ComponentKind::ParticleState => "particle_state",
        ComponentKind::SpriteVisual => "sprite_visual",
        ComponentKind::FogVolume => "fog_volume",
    }
}

fn component_kind_from_name(name: &str) -> Option<ComponentKind> {
    // Accept both the canonical FFI string (snake_case, matching Rust enum
    // variants) and the shorter `worldQuery` filter aliases so authors can
    // use the same vocabulary across `world.query`, `getComponent`, and
    // `setComponent`. `setComponent` for bridge-managed components
    // (`billboard_emitter`, `particle_state`, `sprite_visual`) is rejected
    // downstream by `ComponentValue::FromJs`/`FromLua`, so the aliases
    // here cannot accidentally open a write path.
    match name {
        "transform" => Some(ComponentKind::Transform),
        "light" => Some(ComponentKind::Light),
        "billboard_emitter" | "emitter" => Some(ComponentKind::BillboardEmitter),
        "particle_state" | "particle" => Some(ComponentKind::ParticleState),
        "sprite_visual" => Some(ComponentKind::SpriteVisual),
        "fog_volume" => Some(ComponentKind::FogVolume),
        _ => None,
    }
}

impl<'js> FromJs<'js> for ComponentKind {
    fn from_js(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        let s = String::from_js(ctx, value)?;
        component_kind_from_name(&s).ok_or_else(|| {
            rquickjs::Exception::throw_type(ctx, &format!("unknown ComponentKind `{s}`"))
        })
    }
}

impl<'js> IntoJs<'js> for ComponentKind {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        component_kind_name(self).into_js(ctx)
    }
}

impl FromLua for ComponentKind {
    fn from_lua(value: LuaValue, lua: &Lua) -> mlua::Result<Self> {
        let s = String::from_lua(value, lua)?;
        component_kind_from_name(&s)
            .ok_or_else(|| mlua::Error::RuntimeError(format!("unknown ComponentKind `{s}`")))
    }
}

impl IntoLua for ComponentKind {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        component_kind_name(self).into_lua(lua)
    }
}

impl<'js> FromJs<'js> for ComponentValue {
    fn from_js(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        let o = Object::from_value(value).map_err(|_| {
            rquickjs::Error::new_from_js("value", "ComponentValue object with `kind` tag")
        })?;
        let kind: String = o.get("kind")?;
        match kind.as_str() {
            "transform" => {
                let t = Transform::from_js(ctx, o.into_value())?;
                Ok(ComponentValue::Transform(t))
            }
            "light" => {
                // setComponent("light",...) is intentionally blocked — scripts
                // use the LightEntity handle's setAnimation method instead.
                let _ = o;
                Err(rquickjs::Exception::throw_type(
                    ctx,
                    "LightComponent is read-only via setComponent; \
                     use a LightEntity handle's setAnimation method instead",
                ))
            }
            "billboard_emitter" | "particle_state" | "sprite_visual" => {
                // These components are bridge-managed (emitter bridge, particle
                // simulation) — scripts read but never write via setComponent;
                // configuration goes through dedicated reaction primitives.
                let _ = o;
                Err(rquickjs::Exception::throw_type(
                    ctx,
                    &format!(
                        "{kind} is bridge-managed; setComponent is not supported \
                         (use the dedicated reaction primitives instead)"
                    ),
                ))
            }
            "fog_volume" => {
                // setComponent accepts the four runtime-tweakable fields;
                // any AABB fields (`min`, `max`) on the input are baked
                // geometry and silently ignored.
                let density: f32 = o.get("density")?;
                let color_v: JsValue = o.get("color")?;
                let color = serde_json::from_value::<Vec3Lit>(js_to_json(ctx, color_v)?)
                    .map_err(|e| {
                        rquickjs::Exception::throw_type(
                            ctx,
                            &format!("FogVolume.color: {e}"),
                        )
                    })?
                    .as_f32_3();
                let scatter: f32 = o.get("scatter")?;
                let falloff: f32 = o.get("falloff")?;
                Ok(ComponentValue::FogVolume(FogVolumeComponent {
                    density,
                    color,
                    scatter,
                    falloff,
                }))
            }
            other => Err(rquickjs::Exception::throw_type(
                ctx,
                &format!("unknown ComponentValue kind `{other}`"),
            )),
        }
    }
}

impl<'js> IntoJs<'js> for ComponentValue {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        match self {
            ComponentValue::Transform(t) => {
                let v = t.into_js(ctx)?;
                let o = Object::from_value(v).expect("Transform encodes to an object");
                o.set("kind", "transform")?;
                Ok(o.into_value())
            }
            ComponentValue::Light(light) => {
                let json = serde_json::to_value(ComponentValue::Light(light)).map_err(|e| {
                    rquickjs::Exception::throw_type(
                        ctx,
                        &format!("LightComponent serialization failed: {e}"),
                    )
                })?;
                json_to_js(ctx, &json)
            }
            other @ (ComponentValue::BillboardEmitter(_)
            | ComponentValue::ParticleState(_)
            | ComponentValue::SpriteVisual(_)
            | ComponentValue::FogVolume(_)) => {
                let json = serde_json::to_value(&other).map_err(|e| {
                    rquickjs::Exception::throw_type(
                        ctx,
                        &format!("ComponentValue serialization failed: {e}"),
                    )
                })?;
                json_to_js(ctx, &json)
            }
        }
    }
}

impl FromLua for ComponentValue {
    fn from_lua(value: LuaValue, lua: &Lua) -> mlua::Result<Self> {
        let t = match value {
            LuaValue::Table(t) => t,
            other => {
                return Err(mlua::Error::FromLuaConversionError {
                    from: other.type_name(),
                    to: "ComponentValue".to_string(),
                    message: Some("expected a table with a `kind` field".to_string()),
                });
            }
        };
        let kind: String = t.get("kind")?;
        match kind.as_str() {
            "transform" => {
                let transform = Transform::from_lua(LuaValue::Table(t), lua)?;
                Ok(ComponentValue::Transform(transform))
            }
            "light" => {
                let _ = t;
                Err(mlua::Error::RuntimeError(
                    "LightComponent is read-only via setComponent; \
                     use a LightEntity handle's setAnimation method instead"
                        .to_string(),
                ))
            }
            "billboard_emitter" | "particle_state" | "sprite_visual" => {
                let _ = t;
                Err(mlua::Error::RuntimeError(format!(
                    "{kind} is bridge-managed; setComponent is not supported \
                     (use the dedicated reaction primitives instead)"
                )))
            }
            "fog_volume" => {
                // setComponent accepts the four runtime-tweakable fields;
                // any AABB fields (`min`, `max`) on the input are baked
                // geometry and silently ignored.
                let density: f32 = t.get("density")?;
                let color_v: LuaValue = t.get("color")?;
                let color = serde_json::from_value::<Vec3Lit>(lua_to_json(color_v)?)
                    .map_err(|e| {
                        mlua::Error::RuntimeError(format!("FogVolume.color: {e}"))
                    })?
                    .as_f32_3();
                let scatter: f32 = t.get("scatter")?;
                let falloff: f32 = t.get("falloff")?;
                Ok(ComponentValue::FogVolume(FogVolumeComponent {
                    density,
                    color,
                    scatter,
                    falloff,
                }))
            }
            other => Err(mlua::Error::RuntimeError(format!(
                "unknown ComponentValue kind `{other}`"
            ))),
        }
    }
}

impl IntoLua for ComponentValue {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        match self {
            ComponentValue::Transform(t) => {
                let v = t.into_lua(lua)?;
                if let LuaValue::Table(ref tbl) = v {
                    tbl.set("kind", "transform")?;
                }
                Ok(v)
            }
            ComponentValue::Light(light) => {
                let json = serde_json::to_value(ComponentValue::Light(light)).map_err(|e| {
                    mlua::Error::RuntimeError(format!("LightComponent serialization failed: {e}"))
                })?;
                json_to_lua(lua, &json)
            }
            other @ (ComponentValue::BillboardEmitter(_)
            | ComponentValue::ParticleState(_)
            | ComponentValue::SpriteVisual(_)
            | ComponentValue::FogVolume(_)) => {
                let json = serde_json::to_value(&other).map_err(|e| {
                    mlua::Error::RuntimeError(format!("ComponentValue serialization failed: {e}"))
                })?;
                json_to_lua(lua, &json)
            }
        }
    }
}

// payload is a serde_json::Value walked recursively into native objects — no JSON string on the wire.

pub(crate) fn json_to_js<'js>(
    ctx: &Ctx<'js>,
    v: &serde_json::Value,
) -> rquickjs::Result<JsValue<'js>> {
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

// clippy::only_used_in_recursion: `ctx` threads through for symmetry with
// `json_to_js` and reserves the hook for primitives that need it later.
#[allow(clippy::only_used_in_recursion)]
pub(super) fn js_to_json<'js>(
    ctx: &Ctx<'js>,
    v: JsValue<'js>,
) -> rquickjs::Result<serde_json::Value> {
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
            out.push(js_to_json(ctx, item)?);
        }
        return Ok(serde_json::Value::Array(out));
    }
    if let Some(obj) = v.as_object() {
        let mut map = serde_json::Map::new();
        for entry in obj.props::<String, JsValue>() {
            let (k, val) = entry?;
            map.insert(k, js_to_json(ctx, val)?);
        }
        return Ok(serde_json::Value::Object(map));
    }
    Ok(serde_json::Value::Null)
}

pub(crate) fn json_to_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Result<LuaValue> {
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
                // Lua convention: arrays are 1-indexed.
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

pub(super) fn lua_to_json(value: LuaValue) -> mlua::Result<serde_json::Value> {
    match value {
        LuaValue::Nil => Ok(serde_json::Value::Null),
        LuaValue::Boolean(b) => Ok(serde_json::Value::Bool(b)),
        LuaValue::Integer(i) => Ok(serde_json::Value::Number(serde_json::Number::from(i))),
        LuaValue::Number(f) => Ok(serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)),
        LuaValue::String(s) => Ok(serde_json::Value::String(s.to_str()?.to_string())),
        LuaValue::Table(t) => {
            // Distinguish array-like from map-like by checking for contiguous
            // integer keys starting at 1.
            let len = t.raw_len();
            let mut is_array = len > 0;
            // A table with any non-integer key is a map.
            for pair in t.clone().pairs::<LuaValue, LuaValue>() {
                let (k, _) = pair?;
                if !matches!(k, LuaValue::Integer(_)) {
                    is_array = false;
                    break;
                }
            }
            if is_array {
                let mut out = Vec::with_capacity(len);
                for i in 1..=len {
                    let v: LuaValue = t.get(i)?;
                    out.push(lua_to_json(v)?);
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
                    map.insert(key_str, lua_to_json(v)?);
                }
                Ok(serde_json::Value::Object(map))
            }
        }
        _ => Ok(serde_json::Value::Null),
    }
}

impl<'js> FromJs<'js> for ScriptEvent {
    fn from_js(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        let o = Object::from_value(value).map_err(|_| {
            rquickjs::Error::new_from_js("value", "ScriptEvent object { kind, payload }")
        })?;
        let kind: String = o.get("kind")?;
        let payload_js: JsValue = o.get("payload")?;
        let payload = js_to_json(ctx, payload_js)?;
        Ok(ScriptEvent { kind, payload })
    }
}

impl<'js> IntoJs<'js> for ScriptEvent {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        let o = Object::new(ctx.clone())?;
        o.set("kind", self.kind.as_str())?;
        o.set("payload", json_to_js(ctx, &self.payload)?)?;
        Ok(o.into_value())
    }
}

impl FromLua for ScriptEvent {
    fn from_lua(value: LuaValue, _lua: &Lua) -> mlua::Result<Self> {
        let t = match value {
            LuaValue::Table(t) => t,
            other => {
                return Err(mlua::Error::FromLuaConversionError {
                    from: other.type_name(),
                    to: "ScriptEvent".to_string(),
                    message: Some("expected a table { kind, payload }".to_string()),
                });
            }
        };
        let kind: String = t.get("kind")?;
        let payload_v: LuaValue = t.get("payload")?;
        let payload = lua_to_json(payload_v)?;
        Ok(ScriptEvent { kind, payload })
    }
}

impl IntoLua for ScriptEvent {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        let t = lua.create_table()?;
        t.set("kind", self.kind.as_str())?;
        t.set("payload", json_to_lua(lua, &self.payload)?)?;
        Ok(LuaValue::Table(t))
    }
}

// EntityTypeDescriptor crosses via the hand-rolled walkers in
// `data_descriptors::entity_descriptor_from_{js,lua}` rather than serde_json.
// Component sub-objects (`light`, `emitter`) carry `Vec3` and `[f32;3]` shapes
// that need the same array-or-object permissive deserialize the rest of the
// scripting surface uses; the walkers thread through `js_to_json`/`lua_to_json`
// + serde, matching how `LightAnimation` crosses.

impl<'js> FromJs<'js> for EntityTypeDescriptor {
    fn from_js(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        entity_descriptor_from_js(ctx, value).map_err(|e| {
            rquickjs::Error::new_from_js_message("value", "EntityTypeDescriptor", e.to_string())
        })
    }
}

impl FromLua for EntityTypeDescriptor {
    fn from_lua(value: LuaValue, _lua: &Lua) -> mlua::Result<Self> {
        entity_descriptor_from_lua(value).map_err(|e| mlua::Error::RuntimeError(e.to_string()))
    }
}

// LightAnimation / LightComponent cross via serde_json; `rename_all = "camelCase"` on the structs
// produces the script-facing field names (periodMs, playCount, lightType, ...) without manual mapping.

impl<'js> FromJs<'js> for LightAnimation {
    fn from_js(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        let json = js_to_json(ctx, value)?;
        serde_json::from_value::<LightAnimation>(json).map_err(|e| {
            rquickjs::Error::new_from_js_message("value", "LightAnimation", e.to_string())
        })
    }
}

impl<'js> IntoJs<'js> for LightAnimation {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        let json = serde_json::to_value(self).map_err(|e| {
            rquickjs::Error::new_from_js_message("LightAnimation", "value", e.to_string())
        })?;
        json_to_js(ctx, &json)
    }
}

impl FromLua for LightAnimation {
    fn from_lua(value: LuaValue, _lua: &Lua) -> mlua::Result<Self> {
        let json = lua_to_json(value)?;
        serde_json::from_value::<LightAnimation>(json)
            .map_err(|e| mlua::Error::RuntimeError(format!("invalid LightAnimation: {e}")))
    }
}

impl IntoLua for LightAnimation {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        let json = serde_json::to_value(self)
            .map_err(|e| mlua::Error::RuntimeError(format!("LightAnimation serialize: {e}")))?;
        json_to_lua(lua, &json)
    }
}

// LightComponent is Rust-to-script only; no FromJs/FromLua impl.
impl<'js> IntoJs<'js> for LightComponent {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        let json = serde_json::to_value(self).map_err(|e| {
            rquickjs::Error::new_from_js_message("LightComponent", "value", e.to_string())
        })?;
        json_to_js(ctx, &json)
    }
}

impl IntoLua for LightComponent {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        let json = serde_json::to_value(self)
            .map_err(|e| mlua::Error::RuntimeError(format!("LightComponent serialize: {e}")))?;
        json_to_lua(lua, &json)
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
        // Regression: LightAnimation.color/direction were declared as
        // `Vec<[f32;3]>`, which forced JSON arrays — but the SDK vocabulary
        // emits `{x,y,z}` objects matching the `Vec3` type declaration.
        let from_arr: Vec3Lit = serde_json::from_str("[1.0, 0.0, 0.0]").unwrap();
        let from_obj: Vec3Lit = serde_json::from_str(r#"{"x":1.0,"y":0.0,"z":0.0}"#).unwrap();
        assert_eq!(from_arr, Vec3Lit([1.0, 0.0, 0.0]));
        assert_eq!(from_obj, Vec3Lit([1.0, 0.0, 0.0]));
        assert_eq!(from_arr, from_obj);
    }

    #[test]
    fn vec3lit_deserialize_rejects_malformed_shape() {
        assert!(serde_json::from_str::<Vec3Lit>("[1.0, 0.0]").is_err());
        assert!(serde_json::from_str::<Vec3Lit>(r#"{"x":1.0,"y":0.0}"#).is_err());
        assert!(serde_json::from_str::<Vec3Lit>("\"not a vec\"").is_err());
    }

    #[test]
    fn fog_volume_component_round_trips_through_quickjs() {
        // setComponent accepts {density, color, scatter, falloff}; getComponent
        // returns all four under `kind: "fog_volume"`. AABB fields on the input
        // are silently ignored.
        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            let v: JsValue = jsctx
                .eval(
                    r#"({
                        kind: "fog_volume",
                        density: 0.4,
                        color: [0.1, 0.2, 0.3],
                        scatter: 0.5,
                        falloff: 0.75,
                        // AABB fields silently ignored
                        min: [0.0, 0.0, 0.0],
                        max: [1.0, 1.0, 1.0],
                    })"#,
                )
                .unwrap();
            let cv = ComponentValue::from_js(&jsctx, v).unwrap();
            let ComponentValue::FogVolume(f) = cv else {
                panic!("expected FogVolume variant, got {cv:?}");
            };
            assert!((f.density - 0.4).abs() < 1e-6);
            assert_eq!(f.color, [0.1, 0.2, 0.3]);
            assert!((f.scatter - 0.5).abs() < 1e-6);
            assert!((f.falloff - 0.75).abs() < 1e-6);

            // Round-trip back to JS and read each field.
            let js_back = ComponentValue::FogVolume(f).into_js(&jsctx).unwrap();
            let o = rquickjs::Object::from_value(js_back).unwrap();
            let kind: String = o.get("kind").unwrap();
            assert_eq!(kind, "fog_volume");
            let density: f32 = o.get("density").unwrap();
            assert!((density - 0.4).abs() < 1e-6);
            let scatter: f32 = o.get("scatter").unwrap();
            assert!((scatter - 0.5).abs() < 1e-6);
            let falloff: f32 = o.get("falloff").unwrap();
            assert!((falloff - 0.75).abs() < 1e-6);
        });
    }

    #[test]
    fn fog_volume_component_round_trips_through_luau() {
        let lua = mlua::Lua::new();
        let v: LuaValue = lua
            .load(
                r#"return {
                    kind = "fog_volume",
                    density = 0.4,
                    color = { 0.1, 0.2, 0.3 },
                    scatter = 0.5,
                    falloff = 0.75,
                    -- AABB fields silently ignored
                    min = { 0.0, 0.0, 0.0 },
                    max = { 1.0, 1.0, 1.0 },
                }"#,
            )
            .eval()
            .unwrap();
        let cv = ComponentValue::from_lua(v, &lua).unwrap();
        let ComponentValue::FogVolume(f) = cv else {
            panic!("expected FogVolume variant, got {cv:?}");
        };
        assert!((f.density - 0.4).abs() < 1e-6);
        assert_eq!(f.color, [0.1, 0.2, 0.3]);
        assert!((f.scatter - 0.5).abs() < 1e-6);
        assert!((f.falloff - 0.75).abs() < 1e-6);

        // Round-trip back to Lua and read each field.
        let lua_back = ComponentValue::FogVolume(f).into_lua(&lua).unwrap();
        let LuaValue::Table(tbl) = lua_back else {
            panic!("expected Lua table");
        };
        let kind: String = tbl.get("kind").unwrap();
        assert_eq!(kind, "fog_volume");
        let density: f32 = tbl.get("density").unwrap();
        assert!((density - 0.4).abs() < 1e-6);
        let scatter: f32 = tbl.get("scatter").unwrap();
        assert!((scatter - 0.5).abs() < 1e-6);
        let falloff: f32 = tbl.get("falloff").unwrap();
        assert!((falloff - 0.75).abs() < 1e-6);
    }

    #[test]
    fn euler_identity_round_trips() {
        let e = EulerDegrees {
            pitch: 0.0,
            yaw: 0.0,
            roll: 0.0,
        };
        let q = e.to_quat();
        assert!((q.w - 1.0).abs() < 1e-6);
        let back = EulerDegrees::from_quat(q);
        assert!(back.pitch.abs() < 1e-3);
        assert!(back.yaw.abs() < 1e-3);
        assert!(back.roll.abs() < 1e-3);
    }
}
