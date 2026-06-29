// Script-facing fog-volume animation curve. `period_ms`, `phase`, and
// `play_count` are shared across all four channels — `density`, `saturation`,
// `min_brightness`, and `light_range` — all sampled on the same
// timeline. Installed onto
// `FogVolumeComponent` via the `setFogAnimation` reaction primitive; per-frame
// evaluation and play-count completion live in the fog bridge.
//
// Unlike `LightAnimation`, there is no `start_active` field — fog has no GPU
// descriptor for the curve and no activation event in the surface
// (`setFogAnimation null` clears the channel; reinstalling reactivates it).
//
// See: context/lib/scripting.md

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FogAnimation {
    pub period_ms: f32,
    /// `None` = no phase offset (treated as 0.0 at sampling time). When
    /// `Some`, normalized to `[0.0, 1.0)` via `rem_euclid` in `validate`
    /// before storage.
    #[serde(default)]
    pub phase: Option<f32>,
    /// `None` = loop forever. `Some(n)` plays `n` full periods, after which the
    /// fog bridge writes the final keyframe(s) back as static values and clears
    /// `animation`. Requires at least one curve (`density`, `saturation`,
    /// `min_brightness`, or `light_range`).
    #[serde(default)]
    pub play_count: Option<u32>,
    /// `None` = "no animation on this channel; hold the static density".
    #[serde(default)]
    pub density: Option<Vec<f32>>,
    /// `None` = "no animation on this channel; hold the static saturation".
    /// Values may exceed 1.0 (boosted saturation); negative values are clamped
    /// to 0.0 with a warning.
    #[serde(default)]
    pub saturation: Option<Vec<f32>>,
    #[serde(default)]
    pub min_brightness: Option<Vec<f32>>,
    #[serde(default)]
    pub light_range: Option<Vec<f32>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fog_animation_serde_round_trips() {
        let value = FogAnimation {
            period_ms: 1200.0,
            phase: Some(0.25),
            play_count: Some(2),
            density: Some(vec![0.1, 1.0, 0.1]),
            saturation: None,
            min_brightness: Some(vec![0.05, 0.2, 0.05]),
            light_range: Some(vec![1.0, 2.0, 1.0]),
        };
        let json = serde_json::to_string(&value).unwrap();
        let back: FogAnimation = serde_json::from_str(&json).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn fog_animation_defaults_accept_missing_optional_fields() {
        let json = r#"{"periodMs": 800.0}"#;
        let anim: FogAnimation = serde_json::from_str(json).unwrap();
        assert_eq!(anim.period_ms, 800.0);
        assert_eq!(anim.phase, None);
        assert_eq!(anim.play_count, None);
        assert_eq!(anim.density, None);
    }
}
