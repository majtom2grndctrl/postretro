// Camera-eye fluid trigger: the eye-depth hysteresis band and the ramp that
// drive the fluid view tint and the world-sound muffle.
// See: context/lib/rendering_pipeline.md §7.8 (Fluid view tint) · context/lib/audio.md §1 (World muffle)

use glam::Vec3;
use postretro_collision::{EnvironmentView, EyeFluid, fluid_at_eye, fluid_volume_plane_depth};
use postretro_foundation::FluidDescriptor;
use postretro_render_cpu::flash_limiter::HITCH_CEILING_SECONDS;

/// Width of the eye-depth hysteresis band, metres. On when the camera eye is
/// inside a fluid with at least this much of it above the eye (vertical
/// depth), off once the eye is this far outside it (plane depth). Entering
/// through a side or the bottom, already deeper than this, turns on at the
/// face; the band is still this wide, since off needs the eye this far back
/// out. It must exceed the head-bob amplitude, so an eye bobbing about the
/// surface never crosses the whole band; content/dev bobs ±0.05 m
/// (`scripts/player.ts`), so a treading player sees no flicker. The depth
/// fog's proximity ramp (render-cpu `fog::FLUID_FOG_BAND`) is this same width.
pub(crate) const EYE_FLUID_MARGIN: f32 = 0.08;

/// Seconds for the tint and muffle to ramp fully in or out. A full-strength
/// tint ramps at 4.0 blend strength per second, the photosensitivity
/// limiter's rate cap, so an ordinary entry or exit is never rate-limited.
/// One frame's ramp step takes at most the limiter's hitch ceiling
/// (`HITCH_CEILING_SECONDS`), as the limiter's own allowance does, so a hitch
/// frame never steps past what the limiter admits.
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

/// Which fluid volume the camera eye counts as in, with hysteresis. Depths run
/// through the union of every volume carrying the active volume's fluid, so a
/// seam inside a pool, or a pool split into several entities, is no surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EyeFluidTrigger {
    active: Option<usize>,
}

impl EyeFluidTrigger {
    /// Advance to `eye`, where `fluid_at_eye` reported `hit`, and return the
    /// volume the eye is in.
    ///
    /// Off, it turns on in the fluid winning at the eye once the eye is
    /// `margin` below that fluid's surface: vertical depth, exact through any
    /// seam. On, it turns off once the eye is `margin` outside its fluid by
    /// plane depth, which never reads farther out than the eye truly is and
    /// outside a union is not understated by a seam, so a sideways exit keeps
    /// the band too. It moves to another fluid winning at the eye once the eye
    /// is `margin` inside that one by plane depth, so crossing between
    /// adjacent fluids follows the same band at the face between them. Another
    /// volume of the active fluid is the same fluid: no move.
    pub(crate) fn step(
        &mut self,
        view: EnvironmentView<'_>,
        eye: Vec3,
        hit: Option<EyeFluid>,
        margin: f32,
    ) -> Option<usize> {
        if let Some(volume) = self.active
            && eye_has_left(view, volume, eye, margin)
        {
            self.active = None;
        }
        if let Some(hit) = hit
            && hit.signed_depth >= margin
        {
            let active_fluid = self
                .active
                .and_then(|volume| view.volumes.volume(volume)?.fluid);
            match active_fluid {
                None => self.active = Some(hit.volume),
                Some(fluid) if fluid == hit.fluid => {}
                Some(_) => {
                    if fluid_volume_plane_depth(view, hit.volume, eye)
                        .is_some_and(|depth| depth >= margin)
                    {
                        self.active = Some(hit.volume);
                    }
                }
            }
        }
        self.active
    }

    #[cfg(test)]
    pub(crate) fn active(&self) -> Option<usize> {
        self.active
    }
}

/// Whether the eye is `margin` outside the fluid of `volume` (or that volume no
/// longer sets a fluid, or no brush of it is within the eye reach).
fn eye_has_left(view: EnvironmentView<'_>, volume: usize, eye: Vec3, margin: f32) -> bool {
    fluid_volume_plane_depth(view, volume, eye).is_none_or(|depth| depth <= -margin)
}

/// The camera eye's fluid presentation across frames: the trigger, the look of
/// the fluid it is in (held through the ramp out), the ramp, and whether the
/// eye is inside any fluid at all.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FluidEye {
    trigger: EyeFluidTrigger,
    /// Inside a fluid brush this frame, from the plane down, whatever the
    /// trigger, the ramp, or whether this machine presents that fluid.
    contained: bool,
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
    /// `None` (a fluid this machine does not declare) presents nothing, so the
    /// look held from before ramps out rather than vanishing in one frame.
    pub(crate) fn update(
        &mut self,
        view: EnvironmentView<'_>,
        eye: Vec3,
        dt: f32,
        look_of: impl FnOnce(usize) -> Option<FluidLook>,
    ) {
        let hit = fluid_at_eye(view, eye);
        self.contained = hit.is_some();
        let active = self.trigger.step(view, eye, hit, EYE_FLUID_MARGIN);
        let presented = active.and_then(look_of);
        if let Some(look) = presented {
            self.look = look;
        }
        let dt = if dt.is_finite() {
            // Capped like the flash limiter's own allowance, so one hitch frame
            // never steps the tint further than the limiter would admit.
            dt.clamp(0.0, HITCH_CEILING_SECONDS)
        } else {
            0.0
        };
        let step = dt / FLUID_PRESENTATION_RAMP_SECONDS;
        self.ramp = if presented.is_some() {
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

    /// The frame's depth fog weight (render-cpu `fog::fog_layer_opacity`): 0
    /// while the eye is inside any fluid, else one minus the ramp, so murk
    /// fades in as the tint fades out. Containment, not the trigger, gates it:
    /// between the plane and the trigger depth, during a ramp-in after a side
    /// entry or a teleport under, and in a fluid this machine does not present,
    /// the ramp alone would let outside faces of neighbouring volumes fog a
    /// swimmer's view. Exactly 1 when dry and fully ramped out.
    pub(crate) fn fog_weight(&self) -> f32 {
        if self.contained { 0.0 } else { 1.0 - self.ramp }
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
