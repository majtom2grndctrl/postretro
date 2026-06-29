// VM-free value newtypes shared by descriptor/component data and FFI adapters.
// See: context/lib/scripting.md §12 (Crate Architecture)

use glam::{EulerRot, Quat};
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use std::fmt;

/// Three-component float vector with a permissive Deserialize accepting
/// either a JSON array of 3 numbers or an object with `x`/`y`/`z` keys.
/// Accepts both forms because the SDK emits `{x,y,z}` objects while some
/// callers and tests emit raw `[x, y, z]` arrays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3Lit(pub [f32; 3]);

impl Vec3Lit {
    pub fn as_f32_3(&self) -> [f32; 3] {
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
pub struct EulerDegrees {
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

impl EulerDegrees {
    /// Convert into the engine-internal `Quat`. Order: YXZ (yaw around
    /// world-up first, then pitch, then roll) — matches the FPS authoring convention.
    pub fn to_quat(self) -> Quat {
        Quat::from_euler(
            EulerRot::YXZ,
            self.yaw.to_radians(),
            self.pitch.to_radians(),
            self.roll.to_radians(),
        )
    }

    /// Inverse of [`Self::to_quat`]. `glam::Quat::to_euler` returns radians
    /// in the same YXZ order we pack.
    pub fn from_quat(q: Quat) -> Self {
        let (yaw, pitch, roll) = q.to_euler(EulerRot::YXZ);
        Self {
            pitch: pitch.to_degrees(),
            yaw: yaw.to_degrees(),
            roll: roll.to_degrees(),
        }
    }
}

#[cfg(feature = "script-ffi")]
mod ffi {
    use super::{EulerDegrees, Vec3Lit};
    use mlua::{FromLua, IntoLua, Lua, Table, Value as LuaValue};
    use rquickjs::{Array, Ctx, FromJs, IntoJs, Object, Value as JsValue};

    impl<'js> FromJs<'js> for Vec3Lit {
        fn from_js(_ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
            if value.is_array() {
                let arr = Array::from_value(value)?;
                return Ok(Vec3Lit([arr.get(0)?, arr.get(1)?, arr.get(2)?]));
            }
            let obj = Object::from_value(value).map_err(|_| {
                rquickjs::Error::new_from_js("value", "Vec3 array [x,y,z] or object {x,y,z}")
            })?;
            Ok(Vec3Lit([obj.get("x")?, obj.get("y")?, obj.get("z")?]))
        }
    }

    impl<'js> IntoJs<'js> for Vec3Lit {
        fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
            let obj = Object::new(ctx.clone())?;
            obj.set("x", self.0[0])?;
            obj.set("y", self.0[1])?;
            obj.set("z", self.0[2])?;
            Ok(obj.into_value())
        }
    }

    impl FromLua for Vec3Lit {
        fn from_lua(value: LuaValue, _lua: &Lua) -> mlua::Result<Self> {
            let table = match value {
                LuaValue::Table(table) => table,
                other => {
                    return Err(mlua::Error::FromLuaConversionError {
                        from: other.type_name(),
                        to: "Vec3Lit".to_string(),
                        message: Some("expected a table".to_string()),
                    });
                }
            };
            vec3_from_lua_table(&table)
        }
    }

    impl IntoLua for Vec3Lit {
        fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
            let table = lua.create_table()?;
            table.set("x", self.0[0])?;
            table.set("y", self.0[1])?;
            table.set("z", self.0[2])?;
            Ok(LuaValue::Table(table))
        }
    }

    impl<'js> FromJs<'js> for EulerDegrees {
        fn from_js(_ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Self> {
            let obj = Object::from_value(value)
                .map_err(|_| rquickjs::Error::new_from_js("value", "EulerDegrees object"))?;
            Ok(EulerDegrees {
                pitch: obj.get("pitch")?,
                yaw: obj.get("yaw")?,
                roll: obj.get("roll")?,
            })
        }
    }

    impl<'js> IntoJs<'js> for EulerDegrees {
        fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
            let obj = Object::new(ctx.clone())?;
            obj.set("pitch", self.pitch)?;
            obj.set("yaw", self.yaw)?;
            obj.set("roll", self.roll)?;
            Ok(obj.into_value())
        }
    }

    impl FromLua for EulerDegrees {
        fn from_lua(value: LuaValue, _lua: &Lua) -> mlua::Result<Self> {
            let table = match value {
                LuaValue::Table(table) => table,
                other => {
                    return Err(mlua::Error::FromLuaConversionError {
                        from: other.type_name(),
                        to: "EulerDegrees".to_string(),
                        message: Some("expected a table".to_string()),
                    });
                }
            };
            Ok(EulerDegrees {
                pitch: table.get("pitch")?,
                yaw: table.get("yaw")?,
                roll: table.get("roll")?,
            })
        }
    }

    impl IntoLua for EulerDegrees {
        fn into_lua(self, lua: &Lua) -> mlua::Result<LuaValue> {
            let table = lua.create_table()?;
            table.set("pitch", self.pitch)?;
            table.set("yaw", self.yaw)?;
            table.set("roll", self.roll)?;
            Ok(LuaValue::Table(table))
        }
    }

    fn vec3_from_lua_table(table: &Table) -> mlua::Result<Vec3Lit> {
        if let (Ok(x), Ok(y), Ok(z)) = (table.get(1), table.get(2), table.get(3)) {
            return Ok(Vec3Lit([x, y, z]));
        }
        Ok(Vec3Lit([table.get("x")?, table.get("y")?, table.get("z")?]))
    }
}
