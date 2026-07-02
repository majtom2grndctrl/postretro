// Screen-effect uniform packing from UI-bound slot values.
// See: context/lib/rendering_pipeline.md §7.8

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use postretro_entities::SlotValue;

const SHAKE_REFERENCE_WIDTH: f32 = 1280.0;
const SHAKE_REFERENCE_HEIGHT: f32 = 720.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct EffectUniform {
    pub flash: [f32; 4],
    pub vignette: [f32; 4],
    pub shake: [f32; 2],
    pub _pad: [f32; 2],
}

pub fn pack_effect_uniform(slot_values: &HashMap<String, SlotValue>) -> EffectUniform {
    let mut uniform = EffectUniform::default();
    if let Some(v) = slot_vec4(slot_values.get("screen.flash")) {
        uniform.flash = v;
    }
    if let Some(v) = slot_vec4(slot_values.get("screen.vignette")) {
        uniform.vignette = v;
    }
    if let Some(shake) = read_array(slot_values, "screen.shake") {
        if shake.len() >= 2 {
            uniform.shake = [
                shake[0] / SHAKE_REFERENCE_WIDTH,
                shake[1] / SHAKE_REFERENCE_HEIGHT,
            ];
        }
    }
    uniform
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
