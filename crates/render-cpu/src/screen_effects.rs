// Screen-effect uniform packing from UI-bound slot values.
// See: context/lib/rendering_pipeline.md §7.8

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use postretro_entities::SlotValue;

const SHAKE_REFERENCE_WIDTH: f32 = 1280.0;
const SHAKE_REFERENCE_HEIGHT: f32 = 720.0;

/// The player's resolved accessibility slots the pack step honors. Scaling
/// happens here, on the presenting machine, and never writes `screen.shake`,
/// which keeps its authored decay.
const REDUCE_MOTION_SLOT: &str = "accessibility.reduceMotion";
const SCREEN_SHAKE_SCALE_SLOT: &str = "accessibility.screenShakeScale";

/// The resolve's per-frame uniform. The effect channels pack from slots here;
/// the renderer fills the layout fields (`covers_hud`, `scene_divisor`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct EffectUniform {
    pub flash: [f32; 4],
    pub vignette: [f32; 4],
    pub shake: [f32; 2],
    pub _pad: [f32; 2],
    /// Per-effect covers-HUD switches, nonzero = the effect also reaches the
    /// UI layer. Order matches [`CoversHud`].
    pub covers_hud: [u32; 3],
    /// Surface pixels per scene pixel on each axis. 0 reads as 1.
    pub scene_divisor: u32,
}

// Mirrors the `EffectUniform` struct in shaders/screen_effects.wgsl (flash @0,
// vignette @16, shake @32, _pad @40, covers_hud @48, scene_divisor @60); wgpu
// checks the size only at draw time, so pin the layout at compile time.
const _: () = {
    assert!(std::mem::size_of::<EffectUniform>() == 64);
    assert!(std::mem::offset_of!(EffectUniform, covers_hud) == 48);
    assert!(std::mem::offset_of!(EffectUniform, scene_divisor) == 60);
};

/// Which screen effects reach the UI layer as well as the scene. The single
/// home for an effect that must cover the HUD.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoversHud {
    pub flash: bool,
    pub vignette: bool,
    pub shake: bool,
}

impl CoversHud {
    pub fn packed(self) -> [u32; 3] {
        [
            u32::from(self.flash),
            u32::from(self.vignette),
            u32::from(self.shake),
        ]
    }
}

pub fn pack_effect_uniform(slot_values: &HashMap<String, SlotValue>) -> EffectUniform {
    let mut uniform = EffectUniform::default();
    if let Some(v) = slot_vec4(slot_values.get("screen.flash")) {
        uniform.flash = v;
    }
    if let Some(v) = slot_vec4(slot_values.get("screen.vignette")) {
        uniform.vignette = v;
    }
    if let Some(shake) = read_array(slot_values, "screen.shake")
        && shake.len() >= 2
    {
        let scale = presented_shake_scale(slot_values);
        uniform.shake = [
            shake[0] * scale / SHAKE_REFERENCE_WIDTH,
            shake[1] * scale / SHAKE_REFERENCE_HEIGHT,
        ];
    }
    uniform
}

/// Reduce motion suppresses shake fully; otherwise the screen-shake slider
/// scales it. An absent or malformed slider reads as unscaled.
fn presented_shake_scale(slot_values: &HashMap<String, SlotValue>) -> f32 {
    if matches!(
        slot_values.get(REDUCE_MOTION_SLOT),
        Some(SlotValue::Boolean(true))
    ) {
        return 0.0;
    }
    match slot_values.get(SCREEN_SHAKE_SCALE_SLOT) {
        Some(SlotValue::Number(scale)) if scale.is_finite() => scale.clamp(0.0, 1.0),
        _ => 1.0,
    }
}

fn slot_vec4(value: Option<&SlotValue>) -> Option<[f32; 4]> {
    let values = read_array_value(value)?;
    if values.len() < 4 {
        return None;
    }
    Some([values[0], values[1], values[2], values[3]])
}

fn read_array<'a>(slot_values: &'a HashMap<String, SlotValue>, name: &str) -> Option<&'a [f32]> {
    read_array_value(slot_values.get(name))
}

fn read_array_value(value: Option<&SlotValue>) -> Option<&[f32]> {
    match value {
        Some(SlotValue::Array(values)) => Some(values.as_slice()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots(pairs: &[(&str, SlotValue)]) -> HashMap<String, SlotValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    // Contract: an unbound snapshot is an identity resolve.
    #[test]
    fn pack_effect_uniform_returns_identity_for_unbound_slots() {
        assert_eq!(
            pack_effect_uniform(&HashMap::new()),
            EffectUniform::default()
        );
    }

    // Contract: authored at-rest values are also an identity resolve.
    #[test]
    fn pack_effect_uniform_returns_identity_for_at_rest_slots() {
        let snapshot = slots(&[
            ("screen.flash", SlotValue::Array(vec![0.0, 0.0, 0.0, 0.0])),
            (
                "screen.vignette",
                SlotValue::Array(vec![0.0, 0.0, 0.0, 0.0]),
            ),
            ("screen.shake", SlotValue::Array(vec![0.0, 0.0])),
        ]);

        assert_eq!(pack_effect_uniform(&snapshot), EffectUniform::default());
    }

    #[test]
    fn pack_effect_uniform_maps_slots_and_converts_shake_px_to_uv() {
        let snapshot = slots(&[
            ("screen.flash", SlotValue::Array(vec![1.0, 0.2, 0.3, 0.5])),
            (
                "screen.vignette",
                SlotValue::Array(vec![0.1, 0.0, 0.4, 0.8]),
            ),
            ("screen.shake", SlotValue::Array(vec![128.0, 72.0])),
        ]);

        let uniform = pack_effect_uniform(&snapshot);

        assert_eq!(uniform.flash, [1.0, 0.2, 0.3, 0.5]);
        assert_eq!(uniform.vignette, [0.1, 0.0, 0.4, 0.8]);
        assert!((uniform.shake[0] - 0.1).abs() < 1e-6);
        assert!((uniform.shake[1] - 0.1).abs() < 1e-6);
    }

    // UO2: the switch scales only the packed offset. `screen.shake` keeps its
    // authored decay, so turning reduce motion off mid-decay resumes at the
    // remaining amplitude.
    #[test]
    fn reduce_motion_zeroes_packed_shake_and_the_slider_scales_it() {
        let shaking = |extra: &[(&str, SlotValue)]| {
            let mut snapshot = slots(&[("screen.shake", SlotValue::Array(vec![128.0, 72.0]))]);
            for (name, value) in extra {
                snapshot.insert(name.to_string(), value.clone());
            }
            pack_effect_uniform(&snapshot).shake
        };
        let full = shaking(&[]);
        assert!((full[0] - 0.1).abs() < 1e-6);

        let reduced = shaking(&[
            (REDUCE_MOTION_SLOT, SlotValue::Boolean(true)),
            (SCREEN_SHAKE_SCALE_SLOT, SlotValue::Number(1.0)),
        ]);
        assert_eq!(reduced, [0.0, 0.0]);

        let halved = shaking(&[
            (REDUCE_MOTION_SLOT, SlotValue::Boolean(false)),
            (SCREEN_SHAKE_SCALE_SLOT, SlotValue::Number(0.5)),
        ]);
        assert!((halved[0] - 0.05).abs() < 1e-6);
        assert!((halved[1] - 0.05).abs() < 1e-6);
    }

    #[test]
    fn pack_effect_uniform_uses_identity_for_missing_or_malformed_slots() {
        let snapshot = slots(&[
            ("screen.flash", SlotValue::Array(vec![0.5, 0.5, 0.5, 0.25])),
            ("screen.vignette", SlotValue::Number(1.0)),
            ("screen.shake", SlotValue::Array(vec![10.0])),
        ]);

        let uniform = pack_effect_uniform(&snapshot);

        assert_eq!(uniform.flash, [0.5, 0.5, 0.5, 0.25]);
        assert_eq!(uniform.vignette, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(uniform.shake, [0.0, 0.0]);
    }
}
