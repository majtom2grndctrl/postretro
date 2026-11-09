// How baked indirect light reaches the display: the renderer's production SH
// indirect weight and its soft-knee tonemap, shared so a bake that judges a
// change in display terms reads the renderer's own values.
// See: context/lib/rendering_pipeline.md §4 · context/lib/build_pipeline.md §Compiler pipeline step 12

/// The renderer's production SH indirect weight: every SH surface consumer
/// (forward, fluid, billboard, mesh and mover passes) multiplies it into SH
/// indirect, base and delta alike. The renderer seeds its diagnostics slider
/// from it; a bake that prices a change as the player sees it reads it here.
pub const DEFAULT_INDIRECT_SCALE: f32 = 0.33;

/// Where `screen_effects.wgsl`'s `soft_knee_tonemap` starts compressing: the
/// scene's peak channel passes unchanged up to here. The shader holds the
/// literal; a compiler test pins it to this value.
pub const TONEMAP_KNEE_START: f32 = 0.98;

/// `screen_effects.wgsl`'s `soft_knee_tonemap`: the identity while the peak
/// channel is at most [`TONEMAP_KNEE_START`], above it every channel scaled by
/// the compressed peak over the peak, so hue holds.
pub fn soft_knee_tonemap(color: [f32; 3]) -> [f32; 3] {
    let peak = color[0].max(color[1]).max(color[2]);
    if peak <= TONEMAP_KNEE_START {
        return color;
    }
    let width = 1.0 - TONEMAP_KNEE_START;
    let excess = peak - TONEMAP_KNEE_START;
    let compressed = TONEMAP_KNEE_START + width * excess / (excess + width);
    color.map(|channel| channel * (compressed / peak))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_knee_is_the_identity_below_its_start_and_compresses_above() {
        assert_eq!(soft_knee_tonemap([0.5, 0.25, 0.98]), [0.5, 0.25, 0.98]);
        let [r, g, b] = soft_knee_tonemap([2.0, 1.0, 0.0]);
        assert!(
            r < 1.0 && r > TONEMAP_KNEE_START,
            "peak compresses below 1: {r}"
        );
        assert!((g / r - 0.5).abs() < 1.0e-6, "hue holds");
        assert_eq!(b, 0.0);
    }
}
