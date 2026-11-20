// Written emissive-volume presentation state: brightness, colour and the
// brightness animation a host write leaves, and its shared validation gate.
// See: context/lib/networking.md §Presentation state

use bitcode::{Decode, Encode};

use super::ValidationError;
use super::components::{COMPONENT_KIND_EMISSIVE_VOLUME, MAX_WIRE_CURVE_KEYS};

/// Written emissive state, independent of per-frame bridge samples. The
/// authored range (`min`, `max`) never rides the wire: the map owns it, and a
/// client binds the volume only when its own copy matches by index and origin.
///
/// `brightness` is the component's value when the write landed: the written
/// brightness, or the last sampled one for a write that left a curve running or
/// cleared it. Scripts and the wire carry these values, never a limited one.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WireEmissiveState {
    pub brightness: f32,
    pub color: [f32; 3],
    pub animation: Option<WireEmissiveAnimation>,
    /// Host script time the animation's run started.
    pub anim_start: Option<f64>,
}

/// A brightness animation as `setEmissiveAnimation` installs it.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WireEmissiveAnimation {
    pub period_ms: f32,
    /// `None` loops; `Some(n)` plays `n >= 1` periods.
    pub play_count: Option<u32>,
    pub brightness: Vec<f32>,
}

fn unit(value: f32) -> bool {
    (0.0..=1.0).contains(&value)
}

impl WireEmissiveAnimation {
    fn all_finite(&self) -> bool {
        self.period_ms.is_finite() && self.brightness.iter().all(|value| value.is_finite())
    }

    /// The bounds the install primitive enforces: a positive period, a count of
    /// at least one, a non-empty curve with each sample in `[0, 1]`. The
    /// [`MAX_WIRE_CURVE_KEYS`] cap is the wire's own, not the primitive's: a
    /// longer curve installs locally but cannot encode.
    fn has_valid_bounds(&self) -> bool {
        self.period_ms > 0.0
            && self.play_count != Some(0)
            && (1..=MAX_WIRE_CURVE_KEYS).contains(&self.brightness.len())
            && self.brightness.iter().copied().all(unit)
    }
}

impl WireEmissiveState {
    /// Shared receive and producer/capture gate for presentation state.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !(self.brightness.is_finite()
            && self.color.iter().all(|value| value.is_finite())
            && self.anim_start.is_none_or(f64::is_finite)
            && self
                .animation
                .as_ref()
                .is_none_or(WireEmissiveAnimation::all_finite))
        {
            return Err(ValidationError::NonFinitePresentationState(
                COMPONENT_KIND_EMISSIVE_VOLUME,
            ));
        }
        if !(unit(self.brightness)
            && self.color.iter().copied().all(unit)
            && self
                .animation
                .as_ref()
                .is_none_or(WireEmissiveAnimation::has_valid_bounds))
        {
            return Err(ValidationError::InvalidPresentationState(
                COMPONENT_KIND_EMISSIVE_VOLUME,
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> WireEmissiveState {
        WireEmissiveState {
            brightness: 0.5,
            color: [1.0, 0.25, 0.0],
            animation: Some(WireEmissiveAnimation {
                period_ms: 500.0,
                play_count: Some(1),
                brightness: vec![0.0, 1.0],
            }),
            anim_start: Some(2.0),
        }
    }

    // The gate both the host capture and client receive run: every value the
    // emissive primitives can write passes; anything they never write fails.
    #[test]
    fn emissive_state_validation_admits_primitive_values_and_refuses_the_rest() {
        assert_eq!(state().validate(), Ok(()));
        let mut held = state();
        held.animation = None;
        held.anim_start = None;
        held.brightness = 0.0;
        held.color = [0.0; 3];
        assert_eq!(held.validate(), Ok(()));
        let full = WireEmissiveState {
            animation: Some(WireEmissiveAnimation {
                period_ms: 1.0,
                play_count: None,
                brightness: vec![1.0; MAX_WIRE_CURVE_KEYS],
            }),
            ..state()
        };
        assert_eq!(full.validate(), Ok(()));

        let non_finite: [fn(&mut WireEmissiveState); 5] = [
            |s| s.brightness = f32::NAN,
            |s| s.color[1] = f32::INFINITY,
            |s| s.anim_start = Some(f64::NAN),
            |s| s.animation.as_mut().unwrap().period_ms = f32::INFINITY,
            |s| s.animation.as_mut().unwrap().brightness[0] = f32::NAN,
        ];
        for corrupt in non_finite {
            let mut bad = state();
            corrupt(&mut bad);
            assert_eq!(
                bad.validate(),
                Err(ValidationError::NonFinitePresentationState(
                    COMPONENT_KIND_EMISSIVE_VOLUME
                ))
            );
        }
        let out_of_bounds: [fn(&mut WireEmissiveState); 9] = [
            |s| s.brightness = 1.5,
            |s| s.brightness = -0.1,
            |s| s.color[2] = 1.01,
            |s| s.color[0] = -0.5,
            |s| s.animation.as_mut().unwrap().period_ms = 0.0,
            |s| s.animation.as_mut().unwrap().play_count = Some(0),
            |s| s.animation.as_mut().unwrap().brightness.clear(),
            |s| s.animation.as_mut().unwrap().brightness = vec![0.5; MAX_WIRE_CURVE_KEYS + 1],
            |s| s.animation.as_mut().unwrap().brightness[1] = 1.5,
        ];
        for corrupt in out_of_bounds {
            let mut bad = state();
            corrupt(&mut bad);
            assert_eq!(
                bad.validate(),
                Err(ValidationError::InvalidPresentationState(
                    COMPONENT_KIND_EMISSIVE_VOLUME
                ))
            );
        }
    }
}
