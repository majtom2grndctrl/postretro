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
