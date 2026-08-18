// Renderer-owned light-term diagnostic state and its per-frame snapshot.
// See: context/lib/rendering_pipeline.md §4

use super::*;

impl Renderer {
    /// The UI-owned mask. Changes are captured by the next
    /// `update_per_frame_uniforms` call, never consumed mid-frame.
    #[cfg(feature = "dev-tools")]
    pub fn light_term_mask(&self) -> LightTermMask {
        self.full().light_term_mask
    }

    /// Sets the UI-owned mask for the next render frame.
    #[cfg(feature = "dev-tools")]
    pub fn set_light_term_mask(&mut self, mask: LightTermMask) {
        if self.full().light_term_mask != mask {
            self.full_mut().light_term_mask = mask;
            log::info!("[Renderer] Light-term mask: {:#09b}", mask.bits());
        }
    }

    /// Mask captured while building this frame's group-0 uniform. Later
    /// renderer consumers use this accessor rather than the live UI state so
    /// every draw path observes a checkbox toggle on the same frame.
    pub(crate) fn frame_light_term_mask(&self) -> LightTermMask {
        self.full().frame_light_term_mask
    }
}
