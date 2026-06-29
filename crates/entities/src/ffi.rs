use glam::Vec3;
use mlua::{FromLua, IntoLua, Lua, Table, Value as LuaValue};
use postretro_foundation::EulerDegrees;
use rquickjs::{Array, Ctx, FromJs, IntoJs, Object, Value as JsValue};

use crate::components::fog_volume::FogAnimation;
use crate::components::light::{LightAnimation, LightComponent};
use crate::registry::{ComponentKind, ComponentValue, EntityId, FogVolumeComponent, Transform};

const JSON_CONVERSION_MAX_DEPTH: usize = 64;

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
    Ok(Vec3::new(o.get("x")?, o.get("y")?, o.get("z")?))
}

fn vec3_to_lua_table(lua: &Lua, v: Vec3) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("x", v.x)?;
    t.set("y", v.y)?;
    t.set("z", v.z)?;
    Ok(t)
}

fn vec3_from_lua_table(t: &Table) -> mlua::Result<Vec3> {
    Ok(Vec3::new(t.get("x")?, t.get("y")?, t.get("z")?))
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
        ComponentKind::PlayerMovement => "player_movement",
        ComponentKind::Weapon => "weapon",
        ComponentKind::DescriptorProvenance => "descriptor_provenance",
        ComponentKind::Mesh => "mesh",
        ComponentKind::Health => "health",
        ComponentKind::Agent => "agent",
        ComponentKind::Brain => "brain",
    }
}

fn component_kind_from_name(name: &str) -> Option<ComponentKind> {
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
            "transform" => Ok(ComponentValue::Transform(Transform::from_js(
                ctx,
                o.into_value(),
            )?)),
            "light" => Err(rquickjs::Exception::throw_type(
                ctx,
                "LightComponent is read-only via setComponent; use a LightEntityHandle capability method (pulse, fade, flicker, colorShift, or sweep) to build a setLightAnimation reaction step",
            )),
            "billboard_emitter" | "particle_state" | "sprite_visual" => {
                Err(rquickjs::Exception::throw_type(
                    ctx,
                    &format!(
                        "{kind} is bridge-managed; setComponent is not supported (use the dedicated reaction primitives instead)"
                    ),
                ))
            }
            "weapon" => Err(rquickjs::Exception::throw_type(
                ctx,
                "weapon is descriptor-owned; setComponent is not supported (update the weapon descriptor instead)",
            )),
            "fog_volume" => fog_from_js(ctx, &o),
            other => Err(rquickjs::Exception::throw_type(
                ctx,
                &format!("unknown ComponentValue kind `{other}`"),
            )),
        }
    }
}

fn fog_from_js<'js>(ctx: &Ctx<'js>, o: &Object<'js>) -> rquickjs::Result<ComponentValue> {
    let tint = match o.get::<_, JsValue>("tint") {
        Ok(v) if !v.is_null() && !v.is_undefined() => serde_json::from_value(js_to_json(ctx, v)?)
            .map_err(|e| {
            rquickjs::Exception::throw_type(ctx, &format!("FogVolume.tint: {e}"))
        })?,
        _ => [1.0, 1.0, 1.0],
    };
    let saturation = optional_js_json::<f32>(ctx, o, "saturation")?.unwrap_or(1.0);
    let min_brightness = optional_js_json::<f32>(ctx, o, "minBrightness")?.unwrap_or(0.0);
    let light_range = optional_js_json::<f32>(ctx, o, "lightRange")?.unwrap_or(1.0);
    let animation = optional_js_json::<FogAnimation>(ctx, o, "animation")?;
    Ok(ComponentValue::FogVolume(FogVolumeComponent {
        density: o.get("density")?,
        glow: o.get("glow")?,
        edge_softness: o.get("edgeSoftness")?,
        falloff: o.get("falloff")?,
        tint,
        saturation,
        min_brightness,
        light_range,
        animation,
    }))
}

fn optional_js_json<'js, T: serde::de::DeserializeOwned>(
    ctx: &Ctx<'js>,
    o: &Object<'js>,
    key: &str,
) -> rquickjs::Result<Option<T>> {
    match o.get::<_, JsValue>(key) {
        Ok(v) if !v.is_null() && !v.is_undefined() => serde_json::from_value(js_to_json(ctx, v)?)
            .map(Some)
            .map_err(|e| rquickjs::Exception::throw_type(ctx, &format!("FogVolume.{key}: {e}"))),
        _ => Ok(None),
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
            ComponentValue::Light(light) => component_to_js(ctx, ComponentValue::Light(light)),
            ComponentValue::FogVolume(fog) => fog_to_js(ctx, fog),
            other @ (ComponentValue::BillboardEmitter(_)
            | ComponentValue::ParticleState(_)
            | ComponentValue::SpriteVisual(_)
            | ComponentValue::Weapon(_)) => component_to_js(ctx, other),
            ComponentValue::PlayerMovement(_) => Err(rquickjs::Exception::throw_type(
                ctx,
                "PlayerMovement component is engine-managed and not exposed to scripts",
            )),
            ComponentValue::DescriptorProvenance(_) => Err(rquickjs::Exception::throw_type(
                ctx,
                "Descriptor provenance is engine-managed and not exposed to scripts",
            )),
            ComponentValue::Mesh(_) => Err(rquickjs::Exception::throw_type(
                ctx,
                "Mesh component is engine-managed and not exposed to scripts",
            )),
            ComponentValue::Health(_) => Err(rquickjs::Exception::throw_type(
                ctx,
                "Health component is engine-managed and not exposed to scripts",
            )),
            ComponentValue::Agent(_) => Err(rquickjs::Exception::throw_type(
                ctx,
                "Agent component is engine-managed and not exposed to scripts",
            )),
            ComponentValue::Brain(_) => Err(rquickjs::Exception::throw_type(
                ctx,
                "Brain component is engine-managed and not exposed to scripts",
            )),
        }
    }
}

fn component_to_js<'js>(
    ctx: &Ctx<'js>,
    component: ComponentValue,
) -> rquickjs::Result<JsValue<'js>> {
    let json = serde_json::to_value(component).map_err(|e| {
        rquickjs::Exception::throw_type(ctx, &format!("ComponentValue serialization failed: {e}"))
    })?;
    json_to_js(ctx, &json)
}

fn fog_to_js<'js>(ctx: &Ctx<'js>, fog: FogVolumeComponent) -> rquickjs::Result<JsValue<'js>> {
    let o = Object::new(ctx.clone())?;
    o.set("kind", "fog_volume")?;
    for (key, value) in fog.camel_fields() {
        o.set(key, value)?;
    }
    o.set(
        "tint",
        json_to_js(ctx, &serde_json::to_value(fog.tint).unwrap())?,
    )?;
    let anim_js = match fog.animation {
        Some(anim) => json_to_js(ctx, &serde_json::to_value(anim).unwrap())?,
        None => JsValue::new_null(ctx.clone()),
    };
    o.set("animation", anim_js)?;
    Ok(o.into_value())
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
            "transform" => Ok(ComponentValue::Transform(Transform::from_lua(
                LuaValue::Table(t),
                lua,
            )?)),
            "light" => Err(mlua::Error::RuntimeError(
                "LightComponent is read-only via setComponent; use a LightEntityHandle capability method (pulse, fade, flicker, colorShift, or sweep) to build a setLightAnimation reaction step".to_string(),
            )),
            "billboard_emitter" | "particle_state" | "sprite_visual" => Err(
                mlua::Error::RuntimeError(format!(
                    "{kind} is bridge-managed; setComponent is not supported (use the dedicated reaction primitives instead)"
                )),
            ),
            "weapon" => Err(mlua::Error::RuntimeError(
                "weapon is descriptor-owned; setComponent is not supported (update the weapon descriptor instead)".to_string(),
            )),
            "fog_volume" => fog_from_lua(t),
            other => Err(mlua::Error::RuntimeError(format!(
                "unknown ComponentValue kind `{other}`"
            ))),
        }
    }
}

fn fog_from_lua(t: Table) -> mlua::Result<ComponentValue> {
    let tint = match t.get::<LuaValue>("tint")? {
        LuaValue::Nil => [1.0, 1.0, 1.0],
        other => serde_json::from_value(lua_to_json(other)?)
            .map_err(|e| mlua::Error::RuntimeError(format!("FogVolume.tint: {e}")))?,
    };
    let animation = match t.get::<LuaValue>("animation")? {
        LuaValue::Nil => None,
        other => Some(
            serde_json::from_value::<FogAnimation>(lua_to_json(other)?)
                .map_err(|e| mlua::Error::RuntimeError(format!("FogVolume.animation: {e}")))?,
        ),
    };
    Ok(ComponentValue::FogVolume(FogVolumeComponent {
        density: t.get("density")?,
        glow: t.get("glow")?,
        edge_softness: t.get("edgeSoftness")?,
        falloff: t.get("falloff")?,
        tint,
        saturation: t.get::<Option<f32>>("saturation")?.unwrap_or(1.0),
        min_brightness: t.get::<Option<f32>>("minBrightness")?.unwrap_or(0.0),
        light_range: t.get::<Option<f32>>("lightRange")?.unwrap_or(1.0),
        animation,
    }))
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
            ComponentValue::Light(light) => component_to_lua(lua, ComponentValue::Light(light)),
            ComponentValue::FogVolume(fog) => fog_to_lua(lua, fog),
            other @ (ComponentValue::BillboardEmitter(_)
            | ComponentValue::ParticleState(_)
            | ComponentValue::SpriteVisual(_)
            | ComponentValue::Weapon(_)) => component_to_lua(lua, other),
            ComponentValue::PlayerMovement(_) => Err(mlua::Error::RuntimeError(
                "PlayerMovement component is engine-managed and not exposed to scripts".to_string(),
            )),
            ComponentValue::DescriptorProvenance(_) => Err(mlua::Error::RuntimeError(
                "Descriptor provenance is engine-managed and not exposed to scripts".to_string(),
            )),
            ComponentValue::Mesh(_) => Err(mlua::Error::RuntimeError(
                "Mesh component is engine-managed and not exposed to scripts".to_string(),
            )),
            ComponentValue::Health(_) => Err(mlua::Error::RuntimeError(
                "Health component is engine-managed and not exposed to scripts".to_string(),
            )),
            ComponentValue::Agent(_) => Err(mlua::Error::RuntimeError(
                "Agent component is engine-managed and not exposed to scripts".to_string(),
            )),
            ComponentValue::Brain(_) => Err(mlua::Error::RuntimeError(
                "Brain component is engine-managed and not exposed to scripts".to_string(),
            )),
        }
    }
}

fn component_to_lua(lua: &Lua, component: ComponentValue) -> mlua::Result<LuaValue> {
    let json = serde_json::to_value(component).map_err(|e| {
        mlua::Error::RuntimeError(format!("ComponentValue serialization failed: {e}"))
    })?;
    json_to_lua(lua, &json)
}

fn fog_to_lua(lua: &Lua, fog: FogVolumeComponent) -> mlua::Result<LuaValue> {
    let tbl = lua.create_table()?;
    tbl.set("kind", "fog_volume")?;
    for (key, value) in fog.camel_fields() {
        tbl.set(key, value)?;
    }
    tbl.set(
        "tint",
        json_to_lua(lua, &serde_json::to_value(fog.tint).unwrap())?,
    )?;
    let anim_lua = match fog.animation {
        Some(anim) => json_to_lua(lua, &serde_json::to_value(anim).unwrap())?,
        None => LuaValue::Nil,
    };
    tbl.set("animation", anim_lua)?;
    Ok(LuaValue::Table(tbl))
}

impl<'js> FromJs<'js> for LightAnimation {
    fn from_js(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
        serde_json::from_value::<LightAnimation>(js_to_json(ctx, value)?).map_err(|e| {
            rquickjs::Error::new_from_js_message("value", "LightAnimation", e.to_string())
        })
    }
}

impl<'js> IntoJs<'js> for LightAnimation {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        json_to_js(ctx, &serde_json::to_value(self).unwrap())
    }
}

impl FromLua for LightAnimation {
    fn from_lua(value: LuaValue, _lua: &Lua) -> mlua::Result<Self> {
        serde_json::from_value::<LightAnimation>(lua_to_json(value)?)
            .map_err(|e| mlua::Error::RuntimeError(format!("invalid LightAnimation: {e}")))
    }
}

impl IntoLua for LightAnimation {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        json_to_lua(lua, &serde_json::to_value(self).unwrap())
    }
}

impl<'js> IntoJs<'js> for LightComponent {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        json_to_js(ctx, &serde_json::to_value(self).unwrap())
    }
}

impl IntoLua for LightComponent {
    fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
        json_to_lua(lua, &serde_json::to_value(self).unwrap())
    }
}

fn json_to_js<'js>(ctx: &Ctx<'js>, v: &serde_json::Value) -> rquickjs::Result<JsValue<'js>> {
    match v {
        serde_json::Value::Null => Ok(JsValue::new_null(ctx.clone())),
        serde_json::Value::Bool(b) => b.into_js(ctx),
        serde_json::Value::Number(n) => n.as_f64().unwrap_or_default().into_js(ctx),
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

fn js_to_json<'js>(ctx: &Ctx<'js>, v: JsValue<'js>) -> rquickjs::Result<serde_json::Value> {
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
            out.push(js_to_json_inner(ctx, arr.get(i)?, depth + 1)?);
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

fn json_to_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Result<LuaValue> {
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

fn lua_to_json(value: LuaValue) -> mlua::Result<serde_json::Value> {
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
                    _ => has_only_integer_keys = false,
                }
            }
            if !integer_keys.is_empty() && has_only_integer_keys {
                if integer_keys
                    .iter()
                    .any(|&key| key < 1 || usize::try_from(key).ok().is_none_or(|key| key > len))
                    || integer_keys.len() != len
                {
                    return Err(mlua::Error::FromLuaConversionError {
                        from: "table",
                        to: "JSON array".to_string(),
                        message: Some("array keys must be exactly contiguous from 1".to_string()),
                    });
                }
                let mut out = Vec::with_capacity(len);
                for i in 1..=len {
                    out.push(lua_to_json_inner(t.get(i)?, depth + 1)?);
                }
                Ok(serde_json::Value::Array(out))
            } else {
                let mut map = serde_json::Map::new();
                for pair in t.pairs::<LuaValue, LuaValue>() {
                    let (k, v) = pair?;
                    let key = match k {
                        LuaValue::String(s) => s.to_str()?.to_string(),
                        LuaValue::Integer(i) => i.to_string(),
                        LuaValue::Number(f) => f.to_string(),
                        _ => continue,
                    };
                    map.insert(key, lua_to_json_inner(v, depth + 1)?);
                }
                Ok(serde_json::Value::Object(map))
            }
        }
        _ => Ok(serde_json::Value::Null),
    }
}
