// `setEmissiveAnimation` reaction primitive: install or clear a brightness
// animation on every target emissive volume.
// See: context/lib/scripting.md §10.2a

use serde::{Deserialize, Serialize};

use postretro_entities::{EmissiveAnimation, EntityId, EntityRegistry};

/// `{ periodMs, playCount?, brightness }`, or `null` to clear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct SetEmissiveAnimationArgs(pub(crate) Option<EmissiveAnimation>);

/// Apply `args.0` to every target, with `setFogAnimation`'s validation:
/// - `null` clears the curve. Brightness keeps its last sampled value, since
///   the frame bridge writes each sample into the component.
/// - A curve installs through `install_animation`: re-installing the running
///   curve keeps its start time; any other install starts a new run.
/// - A `periodMs` at or below 0 or non-finite, or an empty `brightness`
///   curve, warns and installs nothing; the prior animation and brightness
///   stay. `playCount` 0 reads 1, with a warning. Samples clamp to
///   `[0, 1]`, NaN reading 0, with one warning.
pub(crate) fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &SetEmissiveAnimationArgs,
) {
    if targets.is_empty() {
        log::debug!("[Scripting] setEmissiveAnimation: empty target set, no-op");
        return;
    }
    match &args.0 {
        None => super::write_targets(registry, targets, "setEmissiveAnimation", |volume| {
            volume.animation = None;
        }),
        Some(animation) => {
            let Some(animation) = validate(animation) else {
                return;
            };
            super::write_targets(registry, targets, "setEmissiveAnimation", |volume| {
                volume.install_animation(animation.clone());
            });
        }
    }
}

/// Validate `animation` and return its normalized copy, or `None` to reject
/// the install.
fn validate(animation: &EmissiveAnimation) -> Option<EmissiveAnimation> {
    if !animation.period_ms.is_finite() || animation.period_ms <= 0.0 {
        log::warn!(
            "[Scripting] setEmissiveAnimation: periodMs must be > 0 and finite (got {}); \
             skipping install",
            animation.period_ms
        );
        return None;
    }
    if animation.brightness.is_empty() {
        log::warn!(
            "[Scripting] setEmissiveAnimation: brightness curve is empty (use null to clear); \
             skipping install"
        );
        return None;
    }
    let mut next = animation.clone();
    if let Some(&sample) = next
        .brightness
        .iter()
        .find(|sample| sample.is_nan() || !(0.0..=1.0).contains(*sample))
    {
        log::warn!(
            "[Scripting] setEmissiveAnimation: brightness sample {sample} is outside [0, 1]; \
             clamping every such sample (NaN reads 0)"
        );
        for sample in &mut next.brightness {
            *sample = if sample.is_nan() {
                0.0
            } else {
                sample.clamp(0.0, 1.0)
            };
        }
    }
    if next.play_count == Some(0) {
        log::warn!("[Scripting] setEmissiveAnimation: playCount of 0 reads 1 (one-shot)");
        next.play_count = Some(1);
    }
    Some(next)
}
