// `setEmissiveBrightness` reaction primitive: hold a brightness on every
// target emissive volume, clearing any running brightness animation.
// See: context/lib/scripting.md §10.2a

use serde::{Deserialize, Serialize};

use postretro_entities::{EntityId, EntityRegistry};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetEmissiveBrightnessArgs {
    pub(crate) brightness: f32,
}

/// Write the clamped brightness and clear the animation on every target, as
/// `setSpinRate` does: a running curve or fade stops
/// and never settles over the written value. Within one body, writes apply in
/// order, so the later of a brightness write and an animation install wins.
pub(crate) fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &SetEmissiveBrightnessArgs,
) {
    if targets.is_empty() {
        log::debug!("[Scripting] setEmissiveBrightness: empty target set, no-op");
        return;
    }
    let brightness = clamp_brightness(args.brightness);
    super::write_targets(registry, targets, "setEmissiveBrightness", |volume| {
        volume.hold_brightness(brightness);
    });
}

/// Clamp a scripted brightness to `[0, 1]`; NaN reads 0. Warns once when the
/// value changed.
fn clamp_brightness(value: f32) -> f32 {
    let fixed = if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    };
    // NaN compares unequal to its fix, so it warns too.
    if fixed != value {
        log::warn!(
            "[Scripting] setEmissiveBrightness: brightness {value} is outside [0, 1]; clamping to {fixed}"
        );
    }
    fixed
}
