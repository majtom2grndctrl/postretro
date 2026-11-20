// Script-facing emissive volume component: one `emissive_volume` brush entity's
// authored glow range and its runtime brightness, colour and animation.
// See: context/lib/scripting.md §10.2 · rendering_pipeline.md §7.3

use serde::{Deserialize, Serialize};

use crate::registry::{Component, ComponentKind, ComponentValue};

/// A brightness animation installed by `setEmissiveAnimation`. One channel,
/// sampled across `period_ms` as fog's density curve is (`AnimatableScalar`'s
/// args shape: `{ periodMs, playCount?, brightness }`).
///
/// `play_count` `None` loops; `Some(n)` plays `n` periods, then the bridge
/// writes the last sample back as `brightness` and clears the animation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmissiveAnimation {
    pub period_ms: f32,
    #[serde(default)]
    pub play_count: Option<u32>,
    /// Brightness samples in `[0, 1]`, evenly spaced over one period. Never
    /// empty once installed.
    pub brightness: Vec<f32>,
}

/// One emissive volume (PRL id 60 record), spawned at level load in record
/// order. Script tags live on the entity, as fog's do.
///
/// A member face glows at texture strength × `(min + brightness × (max − min))`
/// × `color`. The map owns `min` and `max`; scripts own `brightness` and
/// `color`, each clamped to `[0, 1]` where the glow is packed, so no script
/// pushes a surface past the mapper's multiplier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmissiveVolumeComponent {
    /// Authored multiplier at brightness 0 (`emissive_min`). Read-only to
    /// scripts: no primitive writes it.
    pub min: f32,
    /// Authored multiplier at brightness 1 (`emissive_max`), `>= min`.
    /// Read-only to scripts.
    pub max: f32,
    /// Scripted position in the authored range, `[0, 1]`; 1 with no script.
    pub brightness: f32,
    /// Scripted linear tint, channels in `[0, 1]`; the authored
    /// `emissive_color` until a script writes one.
    pub color: [f32; 3],
    /// Running brightness animation; `None` holds `brightness`.
    #[serde(default)]
    pub animation: Option<EmissiveAnimation>,
    /// Install serial: names the current animation run. It changes whenever
    /// an install starts a run rather than continuing the running one, which
    /// is any install over no animation or a different one. Re-installing the
    /// running curve keeps it. So a brightness write that clears the curve,
    /// then an identical re-install in the same drain, still reads as a new
    /// run although the frame bridge never sees the gap (`research.md` P30).
    ///
    /// Every consumer that keeps an animation start time (the frame bridge,
    /// co-op presentation capture) restarts when this changes and keeps its
    /// start while it holds. Engine bookkeeping: never serialized, so scripts
    /// and the wire never see it. Install a curve only through
    /// [`Self::install_animation`]. A clear (`animation = None`, or
    /// [`Self::hold_brightness`]) needs no bump: the next install over no
    /// curve starts a new run.
    #[serde(skip)]
    pub animation_serial: u32,
}

impl EmissiveVolumeComponent {
    /// The no-script state of an authored volume: full brightness, its
    /// authored colour, no animation.
    pub fn authored(min: f32, max: f32, color: [f32; 3]) -> Self {
        Self {
            min,
            max,
            brightness: 1.0,
            color,
            animation: None,
            animation_serial: 0,
        }
    }

    /// Install `animation`. The running curve, re-installed, keeps its run
    /// (and so its start time); anything else starts a new run.
    pub fn install_animation(&mut self, animation: EmissiveAnimation) {
        if self.animation.as_ref() != Some(&animation) {
            self.animation_serial = self.animation_serial.wrapping_add(1);
        }
        self.animation = Some(animation);
    }

    /// Hold `brightness`, ending any running animation: a brightness write
    /// clears the curve, as `setSpinRate` does. The next install starts a
    /// new run, even of the curve just cleared.
    pub fn hold_brightness(&mut self, brightness: f32) {
        self.brightness = brightness;
        self.animation = None;
    }
}

impl Component for EmissiveVolumeComponent {
    const KIND: ComponentKind = ComponentKind::EmissiveVolume;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::EmissiveVolume(volume) => Some(volume),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::EmissiveVolume(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The serde shape is what `setEmissiveAnimation` args and the component's
    // script view carry: camelCase keys, `playCount` optional.
    #[test]
    fn emissive_component_serde_uses_the_script_keys() {
        let component = EmissiveVolumeComponent {
            animation: Some(EmissiveAnimation {
                period_ms: 500.0,
                play_count: Some(2),
                brightness: vec![0.0, 1.0],
            }),
            ..EmissiveVolumeComponent::authored(0.0, 2.0, [1.0, 0.5, 0.0])
        };
        let json = serde_json::to_value(ComponentValue::EmissiveVolume(component.clone())).unwrap();
        assert_eq!(json["kind"], "emissive_volume");
        assert_eq!(json["brightness"], 1.0);
        assert_eq!(json["animation"]["periodMs"], 500.0);
        assert_eq!(json["animation"]["playCount"], 2);
        assert!(
            json.get("animationSerial").is_none() && json.get("animation_serial").is_none(),
            "the install serial is engine bookkeeping, never serialized"
        );
        let back: ComponentValue = serde_json::from_value(json).unwrap();
        assert_eq!(back, ComponentValue::EmissiveVolume(component));

        let anim: EmissiveAnimation =
            serde_json::from_str(r#"{"periodMs": 100, "brightness": [1]}"#).unwrap();
        assert_eq!(anim.play_count, None);
    }

    fn curve(brightness: Vec<f32>) -> EmissiveAnimation {
        EmissiveAnimation {
            period_ms: 500.0,
            play_count: None,
            brightness,
        }
    }

    // P6 and P30 at the source of the signal: re-installing the running curve
    // keeps its run; a different curve, or the same one after a brightness
    // write cleared it, starts a new run.
    #[test]
    fn the_install_serial_changes_only_when_an_install_starts_a_new_run() {
        let mut volume = EmissiveVolumeComponent::authored(0.0, 1.0, [1.0; 3]);
        volume.install_animation(curve(vec![0.0, 1.0]));
        let first = volume.animation_serial;
        volume.install_animation(curve(vec![0.0, 1.0]));
        assert_eq!(
            volume.animation_serial, first,
            "identical re-install continues"
        );

        volume.install_animation(curve(vec![1.0, 0.0]));
        let second = volume.animation_serial;
        assert_ne!(second, first, "a different curve is a new run");

        volume.hold_brightness(0.25);
        assert_eq!((volume.brightness, volume.animation.as_ref()), (0.25, None));
        assert_eq!(
            volume.animation_serial, second,
            "a hold alone starts nothing"
        );
        volume.install_animation(curve(vec![1.0, 0.0]));
        assert_ne!(
            volume.animation_serial, second,
            "the cleared curve re-installed is a new run"
        );
    }
}
