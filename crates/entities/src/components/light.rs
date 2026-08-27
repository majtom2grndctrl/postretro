// Script-facing light component. Mirrors `MapLight` minus compiler-only
// concerns (`bake_only`, `is_dynamic`). Populated by the light bridge at
// level load from `LevelWorld.lights`; scripts mutate it through
// `LightEntity.setAnimation` and the bridge syncs the result into the
// renderer's GPU light buffer each frame.
//
// See: context/lib/scripting.md §10

use serde::{Deserialize, Serialize};

use glam::Vec3;
use postretro_foundation::Vec3Lit;

use crate::registry::EntityId;

pub use postretro_foundation::FalloffKind;

/// Shape discriminant. Parallels `postretro_level_loader::LightType` at the FFI
/// boundary so the scripting module stays independent of the runtime-level data types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightKind {
    Point,
    Spot,
    Directional,
}

/// Per-light animation curve set.
///
/// `brightness`, `color`, and `direction` are uniform samples over `period_ms`;
/// GPU evaluator samples via shared Catmull-Rom (see `curve_eval.wgsl`).
/// `radius` is sampled by the CPU light bridge because it changes both the
/// packed falloff range and the culling influence volume. `None` on a channel
/// means the channel holds constant at the static value.
///
/// `play_count`:
/// - `None` — loop forever (default GPU behavior).
/// - `Some(n)` — play `n` endpoint-clamped periods, then the light bridge writes
///   final animated values back as static state and clears `animation`. Brightness
///   multiplies authored intensity; color and direction replace their authored
///   values; radius replaces `falloff_range`. The GPU descriptor never carries
///   `play_count`; completion is CPU-side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LightAnimation {
    pub period_ms: f32,
    /// `None` = 0.0. Stored in `[0.0, 1.0)`; the bridge `fract`s any larger
    /// value before writing the GPU descriptor.
    #[serde(default)]
    pub phase: Option<f32>,
    /// `None` = loop forever.
    #[serde(default)]
    pub play_count: Option<u32>,
    /// Initial active state for this installed descriptor. `None` defaults to
    /// active; `Some(false)` makes every channel contribute zero until an
    /// explicit script mutation replaces or clears the descriptor. Clearing
    /// the animation restores the light's authored static radiance.
    #[serde(default)]
    pub start_active: Option<bool>,
    #[serde(default)]
    pub brightness: Option<Vec<f32>>,
    #[serde(default)]
    pub color: Option<Vec<Vec3Lit>>,
    #[serde(default)]
    pub direction: Option<Vec<Vec3Lit>>,
    #[serde(default)]
    pub radius: Option<Vec<f32>>,
}

/// Script-visible state of a map light. Fields that do not vary at runtime
/// (`light_type`, `falloff_model`, cone config) are populated
/// from the source `MapLight` at level load and never mutated thereafter —
/// scripts can read them through an entity handle but there is no setter.
///
/// `origin` is held as `[f32; 3]` here (not `[f64; 3]` as in `MapLight`) — the
/// bridge casts at the population seam. Script-facing position is single
/// precision; the baker retains double precision upstream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LightComponent {
    pub origin: [f32; 3],
    pub light_type: LightKind,
    pub intensity: f32,
    pub color: [f32; 3],
    pub falloff_model: FalloffKind,
    pub falloff_range: f32,
    pub cone_angle_inner: Option<f32>,
    pub cone_angle_outer: Option<f32>,
    pub cone_direction: Option<[f32; 3]>,
    /// Whether the source `MapLight.is_dynamic` flag was set. Script handles
    /// expose this as `isDynamic`, but it is not a color-animation eligibility
    /// flag. Static authored lights may accept color animation; slot-bearing
    /// static lights route through the animated-compose path.
    #[serde(default)]
    pub is_dynamic: bool,
    /// Slot into the animated-compose descriptor buffer (group 1 binding 4
    /// of the animated-lightmap compose pass) when the compiler reserved one
    /// for this map light, else `None`. Resolved once at level load from
    /// `MapLight.animated_slot`; never mutated by scripts. The light bridge
    /// keys on this to route `setLightAnimation` writes through the
    /// compose-side path instead of the legacy `is_dynamic`-gated forward
    /// path. World-query snapshots omit this internal routing field.
    #[serde(default)]
    pub animated_slot: Option<u32>,
    /// Internal bridge routing for lights that follow their entity body's
    /// render pose. World-query snapshots omit this field.
    #[serde(default)]
    pub follow_transform: bool,
    /// Runtime-only parent relation for a map light carried by a mover. Raw
    /// entity ids are re-resolved on every level install, never serialized.
    #[serde(skip)]
    pub carrier: Option<LightCarrier>,
    #[serde(default)]
    pub animation: Option<LightAnimation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LightCarrier {
    pub mover_entity: EntityId,
    pub local_offset: Vec3,
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
            is_dynamic: true,
            animated_slot: None,
            follow_transform: false,
            carrier: None,
            animation: Some(LightAnimation {
                period_ms: 1000.0,
                phase: Some(0.25),
                play_count: Some(3),
                start_active: Some(true),
                brightness: Some(vec![0.1, 1.0, 0.1]),
                color: Some(vec![Vec3Lit([1.0, 0.0, 0.0]), Vec3Lit([0.0, 0.0, 1.0])]),
                direction: Some(vec![Vec3Lit([0.0, -1.0, 0.0]), Vec3Lit([0.1, -0.99, 0.0])]),
                radius: Some(vec![4.0, 8.0, 12.0]),
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
        // `direction`, or `radius` keys.
        let json = r#"{"periodMs": 500.0, "brightness": [0.1, 1.0]}"#;
        let anim: LightAnimation = serde_json::from_str(json).unwrap();
        assert_eq!(anim.period_ms, 500.0);
        assert_eq!(anim.phase, None);
        assert_eq!(anim.play_count, None);
        assert_eq!(anim.brightness, Some(vec![0.1, 1.0]));
        assert_eq!(anim.color, None);
        assert_eq!(anim.direction, None);
        assert_eq!(anim.radius, None);
    }
}
