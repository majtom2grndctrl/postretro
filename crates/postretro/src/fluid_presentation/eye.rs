// Camera-eye fluid trigger: the eye-depth hysteresis band and the ramp that
// drive the fluid view tint and the world-sound muffle.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md §Decisions (Presentation)

use glam::Vec3;
use postretro_collision::{EnvironmentView, fluid_at_eye, fluid_volume_signed_depth};
use postretro_foundation::FluidDescriptor;

/// Half-width of the eye-depth hysteresis band, metres: on once the camera eye
/// is this far inside a fluid, off once it is this far outside. It must exceed
/// the head-bob amplitude, so an eye bobbing about the surface never crosses
/// the whole band; content/dev bobs ±0.05 m (`scripts/player.ts`), so a
/// treading player sees no flicker.
pub(crate) const EYE_FLUID_MARGIN: f32 = 0.08;

/// Seconds for the tint and muffle to ramp fully in or out. A full-strength
/// tint ramps at 4.0 blend strength per second, the photosensitivity
/// limiter's rate cap, so an ordinary entry or exit is never rate-limited.
pub(crate) const FLUID_PRESENTATION_RAMP_SECONDS: f32 = 0.25;

/// A fluid's local presentation: the view tint (rgb + strength) and muffle.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FluidLook {
    pub(crate) tint: [f32; 4],
    pub(crate) muffle: f32,
}

impl FluidLook {
    pub(crate) fn of(fluid: &FluidDescriptor) -> Self {
        Self {
            tint: fluid.view_tint,
            muffle: fluid.muffle,
        }
    }
}

/// Which fluid volume the camera eye counts as in, with hysteresis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EyeFluidTrigger {
    active: Option<usize>,
}

impl EyeFluidTrigger {
    /// Advance to `eye` and return the volume the eye is in. Off, it turns on
    /// in the winning fluid volume once the eye is `margin` deep in it. On, it
    /// turns off once the eye is `margin` outside its volume (or the volume no
    /// longer sets a fluid), and moves to another volume that wins at the eye
    /// once the eye is `margin` deep in that one. At most two eye queries.
    pub(crate) fn step(
        &mut self,
        view: EnvironmentView<'_>,
        eye: Vec3,
        margin: f32,
    ) -> Option<usize> {
        if let Some(volume) = self.active
            && fluid_volume_signed_depth(view, volume, eye).is_none_or(|depth| depth <= -margin)
        {
            self.active = None;
        }
        if let Some(hit) = fluid_at_eye(view, eye)
            && hit.signed_depth >= margin
        {
            self.active = Some(hit.volume);
        }
        self.active
    }

    #[cfg(test)]
    pub(crate) fn active(&self) -> Option<usize> {
        self.active
    }
}

/// The camera eye's fluid presentation across frames: the trigger, the look of
/// the fluid it is in (held through the ramp out), and the ramp.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FluidEye {
    trigger: EyeFluidTrigger,
    look: FluidLook,
    /// 0 dry, 1 fully presented. Moves linearly toward the trigger's state, so
    /// a reversal mid-ramp turns back from where it is.
    ramp: f32,
    /// The muffle last handed to audio, so audio is written on change only.
    applied_muffle: f32,
}

impl FluidEye {
    /// Advance one presented frame of `dt` seconds with the eye at `eye`.
    /// `look_of` reads the local presentation of the volume the eye is in;
    /// `None` (a fluid this machine does not declare) presents nothing.
    pub(crate) fn update(
        &mut self,
        view: EnvironmentView<'_>,
        eye: Vec3,
        dt: f32,
        look_of: impl FnOnce(usize) -> Option<FluidLook>,
    ) {
        let active = self.trigger.step(view, eye, EYE_FLUID_MARGIN);
        if let Some(volume) = active {
            self.look = look_of(volume).unwrap_or_default();
        }
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let step = dt / FLUID_PRESENTATION_RAMP_SECONDS;
        self.ramp = if active.is_some() {
            (self.ramp + step).min(1.0)
        } else {
            (self.ramp - step).max(0.0)
        };
    }

    /// The fluid view tint to pack: the fluid's rgb, at its strength times
    /// the ramp. Exactly `[0.0; 4]` once fully dry, so the resolve packs the
    /// mod's own uniform unchanged.
    pub(crate) fn tint(&self) -> [f32; 4] {
        if self.ramp == 0.0 {
            return [0.0; 4];
        }
        let [r, g, b, strength] = self.look.tint;
        [r, g, b, strength * self.ramp]
    }

    /// The world muffle amount: the fluid's muffle times the ramp.
    pub(crate) fn muffle(&self) -> f32 {
        self.look.muffle * self.ramp
    }

    /// The muffle amount when it differs from the one last taken.
    pub(crate) fn take_muffle_change(&mut self) -> Option<f32> {
        let muffle = self.muffle();
        (muffle != self.applied_muffle).then(|| {
            self.applied_muffle = muffle;
            muffle
        })
    }

    #[cfg(test)]
    pub(crate) fn active_volume(&self) -> Option<usize> {
        self.trigger.active()
    }

    #[cfg(test)]
    pub(crate) fn ramp(&self) -> f32 {
        self.ramp
    }
}
