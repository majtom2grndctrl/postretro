//! Fade and ambient occlusion: the per-fragment terms that scale how much the
//! march shows.

use super::{SURFACE_DEPTH_EPS, above};

/// Screen-space LOD (in base-mip texels per pixel, log2) at which the relief
/// starts fading toward flat.
pub const SURFACE_DEPTH_FADE_LOD_START: f32 = 1.0;
/// LOD span over which the relief fades from full to flat.
pub const SURFACE_DEPTH_FADE_LOD_RANGE: f32 = 2.0;
/// Fraction of a material's fade distance spent ramping down. The ramp ends
/// exactly at `fade_distance_meters`, so the march is skipped beyond it.
pub const SURFACE_DEPTH_FADE_DISTANCE_FRACTION: f32 = 0.25;
/// How much a hit a full depth below the material's peak darkens the SH
/// indirect term.
///
/// Indirect only. SH probes sit at ~1 m spacing and know nothing of the
/// receiver's own geometry (`rendering_pipeline.md` §4), so cobblestone-scale
/// self-occlusion is a fact no other source owns — this is not double-counting
/// a light.
pub const SURFACE_DEPTH_AO_STRENGTH: f32 = 0.75;

/// Distance fade: 1 near, 0 at and beyond `fade_distance_meters`.
pub fn surface_depth_distance_fade(distance_meters: f32, fade_distance_meters: f32) -> f32 {
    if fade_distance_meters <= 0.0 {
        return 0.0;
    }
    let ramp = (fade_distance_meters * SURFACE_DEPTH_FADE_DISTANCE_FRACTION).max(SURFACE_DEPTH_EPS);
    ((fade_distance_meters - distance_meters) / ramp).clamp(0.0, 1.0)
}

/// Residency/LOD fade: 1 while base-mip texels are still several pixels wide,
/// 0 once they are far below pixel size.
///
/// `lod` is `log2` of the fragment's footprint measured in texels **of the
/// resident base mip**, so a streamed-out surface map (smaller dimensions at
/// its base level) fades out rather than popping.
pub fn surface_depth_lod_fade(lod: f32) -> f32 {
    (1.0 - (lod - SURFACE_DEPTH_FADE_LOD_START) / SURFACE_DEPTH_FADE_LOD_RANGE).clamp(0.0, 1.0)
}

/// `log2` of the fragment's footprint in base-mip texels.
pub fn surface_depth_lod(ddx_uv: [f32; 2], ddy_uv: [f32; 2], dims: [f32; 2]) -> f32 {
    let len = |d: [f32; 2]| ((d[0] * dims[0]).powi(2) + (d[1] * dims[1]).powi(2)).sqrt();
    len(ddx_uv).max(len(ddy_uv)).max(SURFACE_DEPTH_EPS).log2()
}

/// Combined fade. The shader multiplies the material's depth by this, so a
/// faded surface *flattens* toward the true plane — it never snaps between
/// marched and unmarched.
pub fn surface_depth_fade(distance_meters: f32, fade_distance_meters: f32, lod: f32) -> f32 {
    surface_depth_distance_fade(distance_meters, fade_distance_meters)
        .min(surface_depth_lod_fade(lod))
}

/// Ambient occlusion factor for the SH indirect term (D5).
///
/// Measured from the material's PEAK raise, not the plane:
/// `ao_fraction = clamp(peak_raise − hit_height / depth_scale, 0, 1)`. Mortar
/// between raised stones darkens by its depth below the stone tops wherever
/// the author put the plane; an all-mid-gray map, or a texel at the peak, gets
/// none. With `peak_raise = 0` (a carve-only map) this is exactly the
/// pre-signed formula, since `0 − h/s` and `(−h)/s` are the same float.
///
/// `peak_raise` is the quantized fraction from the uniform. `hit_height_meters`
/// and `depth_scale_meters` are both post-fade, so their ratio is the raw texel
/// value at every fade; the `fade` factor is what makes the occlusion degrade
/// with the relief instead of popping at the fade boundary. A starved march
/// (D7) is flat: its caller skips this and uses 1.
pub fn surface_depth_ambient_occlusion(
    hit_height_meters: f32,
    peak_raise: f32,
    depth_scale_meters: f32,
    fade: f32,
) -> f32 {
    if !above(depth_scale_meters, SURFACE_DEPTH_EPS) {
        return 1.0;
    }
    let below_peak = (peak_raise - hit_height_meters / depth_scale_meters).clamp(0.0, 1.0);
    1.0 - SURFACE_DEPTH_AO_STRENGTH * fade * below_peak
}
