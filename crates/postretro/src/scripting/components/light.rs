// Script-facing light component. Mirrors `MapLight` minus compiler-only
// concerns (`bake_only`, `is_dynamic`). Populated by the light bridge at
// level load from `LevelWorld.lights`; scripts mutate it through
// `LightEntity.setAnimation` and the bridge syncs the result into the
// renderer's GPU light buffer each frame.
//
// See: context/lib/scripting.md §10

use serde::{Deserialize, Serialize};

use crate::scripting::conv::Vec3Lit;

/// Shape discriminant. Parallels `crate::prl::LightType` at the FFI boundary so
/// the scripting module stays independent of the runtime-level data types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum LightKind {
    Point,
    Spot,
    Directional,
}

/// Distance-attenuation discriminant. Parallels `crate::prl::FalloffModel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum FalloffKind {
    Linear,
    InverseDistance,
    InverseSquared,
}

/// Per-light animation curve set.
///
/// `brightness`, `color`, and `direction` are uniform samples over `period_ms`;
/// GPU evaluator samples via shared Catmull-Rom (see `curve_eval.wgsl`). `None`
/// on a channel means the channel holds constant at the static value.
///
/// `play_count`:
/// - `None` — loop forever (default GPU behavior).
/// - `Some(n)` — play `n` full periods, then the light bridge samples the final
///   keyframe value, writes it back as static `intensity`/`color`/`cone_direction`,
///   and clears `animation`. The GPU descriptor itself never carries `play_count`;
///   completion is CPU-side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LightAnimation {
    pub(crate) period_ms: f32,
    /// `None` = 0.0. Stored in `[0.0, 1.0)`; the bridge `fract`s any larger
    /// value before writing the GPU descriptor.
    #[serde(default)]
    pub(crate) phase: Option<f32>,
    /// `None` = loop forever.
    #[serde(default)]
    pub(crate) play_count: Option<u32>,
    /// `None` = "no animation on this channel; hold the static value". The
    /// bridge signals absence with `Some(false)` at the GPU descriptor's
    /// `active` slot.
    #[serde(default)]
    pub(crate) start_active: Option<bool>,
    #[serde(default)]
    pub(crate) brightness: Option<Vec<f32>>,
    #[serde(default)]
    pub(crate) color: Option<Vec<Vec3Lit>>,
    #[serde(default)]
    pub(crate) direction: Option<Vec<Vec3Lit>>,
}

/// Script-visible state of a map light. Fields that do not vary at runtime
/// (`light_type`, `falloff_model`, `cast_shadows`, cone config) are populated
/// from the source `MapLight` at level load and never mutated thereafter —
/// scripts can read them through an entity handle but there is no setter.
///
/// `origin` is held as `[f32; 3]` here (not `[f64; 3]` as in `MapLight`) — the
/// bridge casts at the population seam. Script-facing position is single
/// precision; the baker retains double precision upstream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LightComponent {
    pub(crate) origin: [f32; 3],
    pub(crate) light_type: LightKind,
    pub(crate) intensity: f32,
    pub(crate) color: [f32; 3],
    pub(crate) falloff_model: FalloffKind,
    pub(crate) falloff_range: f32,
    pub(crate) cone_angle_inner: Option<f32>,
    pub(crate) cone_angle_outer: Option<f32>,
    pub(crate) cone_direction: Option<[f32; 3]>,
    pub(crate) cast_shadows: bool,
    /// Whether the source `MapLight.is_dynamic` flag was set. Script handles
    /// read this as `isDynamic` to gate `color` animation: color animation on
    /// a baked light would produce a direct/indirect mismatch (SH indirect was
    /// baked at compile-time color). The Rust primitive enforces this at
    /// `setLightAnimation`; `sdk/lib/entities/lights.ts` / `entities/lights.luau`
    /// `wrapLightEntity` pre-checks the handle snapshot and throws a descriptive
    /// error before the primitive call.
    #[serde(default)]
    pub(crate) is_dynamic: bool,
    #[serde(default)]
    pub(crate) animation: Option<LightAnimation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_component_serde_round_trip_with_all_animation_channels() {
        let value = LightComponent {
            origin: [1.0, 2.0, 3.0],
            light_type: LightKind::Spot,
            intensity: 0.75,
            color: [1.0, 0.9, 0.8],
            falloff_model: FalloffKind::InverseSquared,
            falloff_range: 12.5,
            cone_angle_inner: Some(0.2),
            cone_angle_outer: Some(0.5),
            cone_direction: Some([0.0, -1.0, 0.0]),
            cast_shadows: true,
            is_dynamic: true,
            animation: Some(LightAnimation {
                period_ms: 1000.0,
                phase: Some(0.25),
                play_count: Some(3),
                start_active: Some(true),
                brightness: Some(vec![0.1, 1.0, 0.1]),
                color: Some(vec![Vec3Lit([1.0, 0.0, 0.0]), Vec3Lit([0.0, 0.0, 1.0])]),
                direction: Some(vec![Vec3Lit([0.0, -1.0, 0.0]), Vec3Lit([0.1, -0.99, 0.0])]),
            }),
        };
        let json = serde_json::to_string(&value).unwrap();
        let back: LightComponent = serde_json::from_str(&json).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn light_animation_defaults_accept_missing_optional_fields() {
        // A scripted animation with only `period_ms` + `brightness` should
        // deserialize without requiring `phase`, `play_count`, `color`, or
        // `direction` keys.
        let json = r#"{"periodMs": 500.0, "brightness": [0.1, 1.0]}"#;
        let anim: LightAnimation = serde_json::from_str(json).unwrap();
        assert_eq!(anim.period_ms, 500.0);
        assert_eq!(anim.phase, None);
        assert_eq!(anim.play_count, None);
        assert_eq!(anim.brightness, Some(vec![0.1, 1.0]));
        assert_eq!(anim.color, None);
        assert_eq!(anim.direction, None);
    }
}
