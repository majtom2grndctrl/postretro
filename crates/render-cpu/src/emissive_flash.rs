// Emissive volumes through the photosensitivity limiter: one per-value flash
// clamp per volume, fed what the volume's surface shows, and mapped back to
// the glow factor the volume table packs.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use postretro_level_format::emissive::{EMISSIVE_VALUE_CEILING, EmissiveSection, EmissiveTexture};

use crate::emissive_volume::VolumeGlow;
use crate::flash_clamp::ValueClamp;
use crate::flash_limiter::{LimiterFrameInput, LimiterFrameUniform, pack_limiter_frame};

/// Rec. 709 luminance, the weights the flash clamp's red rule desaturates
/// at. A drift test proves the two agree.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

fn luminance(rgb: [f32; 3]) -> f32 {
    rgb[0] * LUMA[0] + rgb[1] * LUMA[1] + rgb[2] * LUMA[2]
}

/// A finite value at or above 0; anything else reads 0.
fn non_negative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

/// What the limiter judges one volume by: the member texture whose glow shows
/// brightest, its baked strength × the luminance of its mean glow colour.
/// Each texture's mean glow colour stands in for its texels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EmissiveFlashSource {
    /// Strength × glow luminance of the brightest member texture; 0 for a
    /// volume with no member face, or none whose texture glows.
    peak: f32,
    /// That texture's mean glow colour (linear).
    glow: [f32; 3],
}

impl EmissiveFlashSource {
    /// A volume whose surface shows nothing: it feeds the clamp 0.
    pub const DARK: Self = Self {
        peak: 0.0,
        glow: [0.0; 3],
    };

    /// A volume judged by one texture of baked `strength` and mean glow
    /// colour `glow`. Non-finite or negative values read 0.
    pub fn new(strength: f32, glow: [f32; 3]) -> Self {
        let glow = glow.map(non_negative);
        let peak = non_negative(non_negative(strength) * luminance(glow));
        if peak > 0.0 {
            Self { peak, glow }
        } else {
            Self::DARK
        }
    }

    /// The brightest of the textures `member_textures` names (indices into
    /// `textures`, `None` for a face with no texture). An empty set, or one
    /// with no glowing texture, is [`Self::DARK`]: no NaN or −inf reaches the
    /// clamp. Ties keep the first.
    pub fn brightest(
        textures: &[EmissiveTexture],
        member_textures: impl IntoIterator<Item = Option<u32>>,
    ) -> Self {
        member_textures
            .into_iter()
            .filter_map(|index| textures.get(index? as usize))
            .map(|texture| Self::new(texture.strength, texture.glow_color))
            .fold(
                Self::DARK,
                |best, next| {
                    if next.peak > best.peak { next } else { best }
                },
            )
    }

    /// Every volume of `section`, in record order, judged by its brightest
    /// member texture. `face_texture` maps a Geometry face index to its
    /// texture index. Runs once at level load.
    pub fn for_volumes(
        section: &EmissiveSection,
        face_texture: impl Fn(u32) -> Option<u32>,
    ) -> Vec<Self> {
        section
            .volumes
            .iter()
            .map(|volume| {
                Self::brightest(
                    &section.textures,
                    volume.faces.iter().map(|&face| face_texture(face)),
                )
            })
            .collect()
    }

    /// The clamp's input for `glow`: strength `min(multiplier × peak, 1)`,
    /// colour glow colour × volume colour, so the red-flash rule sees what
    /// the surface shows.
    fn input(self, glow: VolumeGlow) -> (f32, [f32; 3]) {
        let strength = (glow.multiplier * self.peak).min(1.0);
        let rgb = std::array::from_fn(|channel| self.glow[channel] * glow.color[channel]);
        (strength, rgb)
    }

    /// The glow factor that presents what the clamp let through.
    ///
    /// - Multiplier: where the clamp passed the input strength unchanged, the
    ///   scripted multiplier exactly, so a volume past 1 presents its full
    ///   value once the clamp has caught up. Otherwise `out ÷ peak`, so the
    ///   presented peak `min(multiplier × peak, 1)` is the clamp's strength
    ///   exactly. The clamp's output stays within its past inputs, so this
    ///   multiplier never exceeds one the script presented.
    /// - Colour: where the clamp passed the colour unchanged, the scripted
    ///   colour exactly. Otherwise each channel is `out ÷ glow` where the
    ///   glow colour has that channel, so glow colour × presented colour is
    ///   the clamp's colour: desaturated toward its own luminance, luminance
    ///   unchanged, redness at the held level. A channel the glow colour lacks
    ///   cannot show and keeps its scripted value.
    ///
    /// Each packed channel stays at or under the value ceiling, so a
    /// desaturating boost to a weak glow channel keeps the ceiling's
    /// half-float bound.
    fn presented(
        self,
        glow: VolumeGlow,
        input: (f32, [f32; 3]),
        output: (f32, [f32; 3]),
    ) -> [f32; 3] {
        let (in_strength, in_rgb) = input;
        let (out_strength, out_rgb) = output;
        let multiplier = if out_strength == in_strength || self.peak <= 0.0 {
            glow.multiplier
        } else {
            out_strength / self.peak
        };
        let color = if out_rgb == in_rgb {
            glow.color
        } else {
            std::array::from_fn(|channel| {
                if self.glow[channel] > 0.0 {
                    out_rgb[channel] / self.glow[channel]
                } else {
                    glow.color[channel]
                }
            })
        };
        color.map(|channel| non_negative(channel * multiplier).min(EMISSIVE_VALUE_CEILING))
    }
}

/// Every emissive volume through the photosensitivity limiter, one
/// [`ValueClamp`] per volume in id-60 record order.
///
/// Each frame: [`Self::present`] with every volume's scripted glow, culled or
/// not (the limiter judges the source, not its framing), then
/// [`Self::finish_presentation`] once the renderer knows whether the frame
/// presented. Scripts and the wire keep the scripted values; only the packed
/// table sees what this presents.
///
/// - Off, every volume presents its scripted glow and no history is kept.
/// - The frame that turns the limiter on, and the first after
///   [`Self::install`] (a level load), start every clamp fresh against its own
///   value, so that frame presents unchanged and counts nothing.
/// - A frame begun with no limiter input presents scripted glow and leaves
///   history untouched.
/// - A frame that never presented (its surface acquire failed) leaves every
///   clamp where it began; its time merges into the next frame's.
///
/// Sized at install; the frame path never allocates.
#[derive(Clone, Debug, Default)]
pub struct EmissiveFlashClamp {
    sources: Vec<EmissiveFlashSource>,
    clamps: Vec<ValueClamp>,
    /// `clamps` as the unsettled frame began, restored if it never presents.
    frame_start: Vec<ValueClamp>,
    /// Whether the previous frame ran with the limiter on.
    history_live: bool,
    /// `history_live` as the unsettled frame began.
    history_live_at_start: bool,
    /// The limiter input of the frame begun and not yet settled.
    unsettled: Option<LimiterFrameUniform>,
    /// Presented time of frames that never presented, which the next limited
    /// frame takes on.
    unconsumed: Option<LimiterFrameInput>,
}

impl EmissiveFlashClamp {
    /// A level's volumes, in record order: every clamp starts fresh on the
    /// next limited frame, and nothing from an earlier level carries over.
    pub fn install(&mut self, sources: &[EmissiveFlashSource]) {
        self.sources.clear();
        self.sources.extend_from_slice(sources);
        self.clamps.clear();
        self.clamps.resize(sources.len(), ValueClamp::default());
        self.frame_start.clone_from(&self.clamps);
        self.history_live = false;
        self.history_live_at_start = false;
        self.unsettled = None;
        self.unconsumed = None;
    }

    /// Present one frame: write each volume's packed glow factor into
    /// `presented` from its scripted glow in `scripted` (both in record
    /// order). `frame` is this frame's limiter input (presented-frame time and
    /// the fail-safe enable flag), or `None` for a frame outside the limiter.
    /// A frame begun before the last one settled settles that one as
    /// presented.
    pub fn present(
        &mut self,
        frame: Option<&LimiterFrameUniform>,
        scripted: &[VolumeGlow],
        presented: &mut [[f32; 3]],
    ) {
        debug_assert_eq!(scripted.len(), presented.len());
        self.unsettled = None;
        let Some(frame) = frame else {
            pass_through(scripted, presented);
            return;
        };
        let frame = match self.unconsumed.take() {
            Some(unconsumed) => pack_limiter_frame(
                unconsumed.merge(LimiterFrameInput {
                    elapsed_seconds: frame.dt_window,
                }),
                frame.enabled,
                frame.init,
            ),
            None => *frame,
        };
        self.frame_start.copy_from_slice(&self.clamps);
        self.history_live_at_start = self.history_live;
        let fresh = frame.enabled && (frame.init || !self.history_live);
        self.history_live = frame.enabled;
        self.unsettled = Some(frame);
        if !frame.enabled {
            pass_through(scripted, presented);
            return;
        }
        debug_assert_eq!(scripted.len(), self.clamps.len());
        for (((clamp, source), &glow), out) in self
            .clamps
            .iter_mut()
            .zip(&self.sources)
            .zip(scripted)
            .zip(presented.iter_mut())
        {
            let input = source.input(glow);
            if fresh {
                clamp.restart(input.0, input.1);
            }
            let output = clamp.limit(input.0, input.1, &frame);
            *out = source.presented(glow, input, output);
        }
    }

    /// Settle the frame last begun once the renderer knows whether it
    /// presented. A frame that presented nothing never consumes its presented
    /// time: every clamp returns to where the frame began, and its time merges
    /// into the next limited frame's, as the resolve's limiter input does.
    pub fn finish_presentation(&mut self, presented: bool) {
        let Some(frame) = self.unsettled.take() else {
            return;
        };
        if presented {
            return;
        }
        std::mem::swap(&mut self.clamps, &mut self.frame_start);
        self.history_live = self.history_live_at_start;
        // The frame's time already holds any earlier unconsumed time.
        self.unconsumed = Some(LimiterFrameInput {
            elapsed_seconds: frame.dt_window,
        });
    }
}

fn pass_through(scripted: &[VolumeGlow], presented: &mut [[f32; 3]]) {
    for (out, glow) in presented.iter_mut().zip(scripted) {
        *out = glow.factor();
    }
}

#[cfg(test)]
#[path = "emissive_flash_tests.rs"]
mod tests;
