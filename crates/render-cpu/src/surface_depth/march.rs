//! The two DDAs: the view-ray march that finds what a fragment sees, and the
//! shorter light-ray march that self-shadows it against one dynamic light.
//!
//! Both walk the texel grid as an exact Amanatides-Woo DDA parameterised by
//! vertical distance in meters. The view march measures DESCENT from the
//! material's peak raise; the light march measures RISE from the hit. Per
//! texel `T` the solid's top is at signed height `s(T) · scale`, which in
//! descent-from-peak terms is `solid(T) = (peak − s(T)) · scale ≥ 0`. Writing
//! the field that way keeps the loop body the same walk it was when the field
//! was carve-only: `solid` is still "how far below the ray's start the top
//! sits", the start just moved up from the plane to the peak.

use super::height::{SurfaceDepthField, SurfaceRelief};
use super::{SURFACE_DEPTH_EPS, SURFACE_DEPTH_MAX_METERS, above};

/// Height slack, in meters, before a self-shadow march calls a texel occluding.
/// Plateaus share exact quantized values, so equality must read as lit.
pub const SURFACE_DEPTH_SHADOW_BIAS_M: f32 = 1.0e-4;
/// How far past a crossed texel boundary a side hit shifts its texture-sample
/// UV, in texels. Half a texel lands on the entered texel's center line, so
/// `sample_post_retro` reads the stone's own color rather than blending across
/// the boundary it just hit.
pub const SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS: f32 = 0.5;
/// Sentinel `t_max` / `t_delta` for an axis the ray does not move along. The
/// shader's literal, so both sides compare against the same value: any finite
/// value above every reachable `t` works, and a non-moving axis is never
/// stepped, so the sentinel is never added to.
pub const SURFACE_DEPTH_FAR: f32 = 3.4e38;

/// Which face the view ray landed on. Exactly five outcomes — the field is a
/// grid of axis-aligned boxes, so there is nothing else to hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceDepthFace {
    /// The top of a texel's solid, or the untouched plane on a flat result.
    Top,
    /// A side wall whose outward normal is `-u`.
    NegU,
    /// A side wall whose outward normal is `+u`.
    PosU,
    /// A side wall whose outward normal is `-v`.
    NegV,
    /// A side wall whose outward normal is `+v`.
    PosV,
}

impl SurfaceDepthFace {
    /// The face normal in the `(tangent, bitangent, normal)` frame.
    pub const fn normal_texel(self) -> [f32; 3] {
        match self {
            SurfaceDepthFace::Top => [0.0, 0.0, 1.0],
            SurfaceDepthFace::NegU => [-1.0, 0.0, 0.0],
            SurfaceDepthFace::PosU => [1.0, 0.0, 0.0],
            SurfaceDepthFace::NegV => [0.0, -1.0, 0.0],
            SurfaceDepthFace::PosV => [0.0, 1.0, 0.0],
        }
    }

    pub const fn is_top(self) -> bool {
        matches!(self, SurfaceDepthFace::Top)
    }
}

/// Result of one view-ray march.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceDepthHit {
    /// UV to sample the material's textures at — biased half a texel past a
    /// crossed boundary on a side hit.
    pub uv: [f32; 2],
    /// UV of the hit point itself, unbiased. Self-shadow marches start here.
    pub march_uv: [f32; 2],
    /// Signed height of the hit above the true plane, in meters. POSITIVE is
    /// raised toward the viewer. `|height_meters| ≤ min(scale,
    /// SURFACE_DEPTH_MAX_METERS)`. The hit's world position is on the view ray:
    /// `world_position + view_to_eye · (height_meters / descent)`.
    pub height_meters: f32,
    pub face: SurfaceDepthFace,
    /// Texel boundaries crossed. Only ever interesting for budgeting.
    pub steps: u32,
    /// The view ray exhausted its budget and resolved flat at the true plane
    /// (D7). A consumer treats this exactly like no march: no AO, no
    /// self-shadow.
    pub starved: bool,
}

impl SurfaceDepthHit {
    /// The flat result: original UV, height 0, geometric normal, top face.
    pub const fn flat(uv: [f32; 2]) -> Self {
        Self {
            uv,
            march_uv: uv,
            height_meters: 0.0,
            face: SurfaceDepthFace::Top,
            steps: 0,
            starved: false,
        }
    }
}

/// The vertical scale both marches use: the caller's post-fade scale, never
/// past [`SURFACE_DEPTH_MAX_METERS`]. Zero (flat) for a non-positive or NaN
/// scale. On the GPU the resolve already clamps to `MAX_METERS · fade`, so
/// this `min` is a no-op there; it makes the height bound hold for any caller.
pub fn surface_depth_march_scale(depth_scale_meters: f32) -> f32 {
    if !above(depth_scale_meters, SURFACE_DEPTH_EPS) {
        return 0.0;
    }
    depth_scale_meters.min(SURFACE_DEPTH_MAX_METERS)
}

/// Amanatides-Woo state for a 2D walk over the texel grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceDepthDda {
    pub cell: [i32; 2],
    /// Vertical distance (meters) at which the ray next crosses each axis.
    pub t_max: [f32; 2],
    /// Vertical distance (meters) between successive crossings on each axis.
    pub t_delta: [f32; 2],
    pub step: [i32; 2],
}

/// Set up the walk from `origin` (texels) along `dir` (texels per vertical
/// meter). A zero-direction axis keeps its [`SURFACE_DEPTH_FAR`] sentinel and
/// is never stepped. The zero test is `!above(|dir|, EPS)`, the shader's
/// `abs(dir) > EPS` negated, so a NaN axis is treated as zero on both sides.
pub fn surface_depth_dda_setup(origin: [f32; 2], dir: [f32; 2]) -> SurfaceDepthDda {
    let cell = [origin[0].floor() as i32, origin[1].floor() as i32];
    let mut t_max = [SURFACE_DEPTH_FAR, SURFACE_DEPTH_FAR];
    let mut t_delta = [SURFACE_DEPTH_FAR, SURFACE_DEPTH_FAR];
    let mut step = [0i32, 0i32];
    for axis in 0..2 {
        if !above(dir[axis].abs(), SURFACE_DEPTH_EPS) {
            continue;
        }
        let positive = dir[axis] > 0.0;
        step[axis] = if positive { 1 } else { -1 };
        let boundary = if positive {
            (cell[axis] + 1) as f32
        } else {
            cell[axis] as f32
        };
        t_max[axis] = (boundary - origin[axis]) / dir[axis];
        t_delta[axis] = (1.0 / dir[axis]).abs();
    }
    SurfaceDepthDda {
        cell,
        t_max,
        t_delta,
        step,
    }
}

/// March the view ray through the relief band.
///
/// `band` is the QUANTIZED relief from the uniform; it must bound every texel
/// the field returns, which load-time extraction over every mip guarantees.
/// The ray enters the band at the peak raise (D6) — at `uv0 − dir · peak` in
/// texel space, where the view ray crosses that height — and descends.
///
/// Eye bound: the march never starts behind the camera. `eye_height_meters`
/// is how far the eye sits above the true plane along the surface normal —
/// the fragment-to-eye distance times the view ray's descent cosine, which is
/// the shader's `view_distance * descent`. The ray starts at `top = min(peak,
/// eye_height / scale)`: a low slide eye or a camera hugging raised brick
/// starts at the eye instead of `peak` behind it. Pass `f32::INFINITY` for a
/// far eye; any eye above the peak marches bit-identically.
///
/// Per texel `T` the solid's top lies `solid(T) = (top − s(T)) · scale` below
/// the start. Within `T` the ray spans `[z_enter, z_exit]`:
/// * `z_enter >= solid` — the ray was already inside the solid on entry, so it
///   hit the SIDE wall it entered through.
/// * `z_exit > solid` — it meets the TOP.
/// * otherwise — the texel is entirely below the ray; step on.
///
/// The first texel is entered through the start height, so its "entry face"
/// is the geometric top: a texel at the peak resolves on the first iteration
/// with the geometric normal, and so does a texel rising above an eye inside
/// the band (`solid < 0`, a hit at the eye).
///
/// Cost levers: an empty band marches nothing (P4); a ray that leaves the
/// starting texel only below the band's floor resolves that texel's top with
/// one fetch and no loop (P2) — exactly what the loop's first iteration would
/// have returned. Termination is structural: the budget test is an integer
/// comparison, so the loop exits after at most `max_steps` iterations whatever
/// the field samples to, including a NaN. A starved march resolves FLAT at the
/// true plane (D7), so grazing starvation looks like the effect switched off.
pub fn march_surface_depth(
    field: &SurfaceDepthField<'_>,
    uv0: [f32; 2],
    dir_uv_per_meter: [f32; 2],
    depth_scale_meters: f32,
    band: SurfaceRelief,
    eye_height_meters: f32,
    max_steps: u32,
) -> SurfaceDepthHit {
    march_view_ray(
        field,
        uv0,
        dir_uv_per_meter,
        depth_scale_meters,
        band,
        eye_height_meters,
        max_steps,
        true,
    )
}

/// [`march_surface_depth`] without the single-texel early-out, so a test can
/// prove the early-out exact against the full loop.
#[cfg(test)]
pub(super) fn march_surface_depth_full_loop(
    field: &SurfaceDepthField<'_>,
    uv0: [f32; 2],
    dir_uv_per_meter: [f32; 2],
    depth_scale_meters: f32,
    band: SurfaceRelief,
    eye_height_meters: f32,
    max_steps: u32,
) -> SurfaceDepthHit {
    march_view_ray(
        field,
        uv0,
        dir_uv_per_meter,
        depth_scale_meters,
        band,
        eye_height_meters,
        max_steps,
        false,
    )
}

/// Whether the view ray stays inside its starting texel across the whole band
/// (P2): it crosses no texel boundary before descending `band_meters`, the
/// band measured from where the ray starts.
pub fn surface_depth_single_texel_band(dda: &SurfaceDepthDda, band_meters: f32) -> bool {
    dda.t_max[0].min(dda.t_max[1]) > band_meters
}

#[allow(clippy::too_many_arguments)]
fn march_view_ray(
    field: &SurfaceDepthField<'_>,
    uv0: [f32; 2],
    dir_uv_per_meter: [f32; 2],
    depth_scale_meters: f32,
    band: SurfaceRelief,
    eye_height_meters: f32,
    max_steps: u32,
    single_texel_early_out: bool,
) -> SurfaceDepthHit {
    let scale = surface_depth_march_scale(depth_scale_meters);
    let band_m = (band.peak_raise - band.trough) * scale;
    // P4: an empty band — no map, an all-mid-gray map, or a zero scale.
    if !above(band_m, 0.0) {
        return SurfaceDepthHit::flat(uv0);
    }
    // Eye bound: start at the peak, or at the eye when it sits inside the
    // band. A far eye leaves `top == peak` exactly. `f32::min` drops a NaN.
    let top = band.peak_raise.min(eye_height_meters / scale);
    let top_m = top * scale;

    let dims = [field.width as f32, field.height as f32];
    let p0 = [uv0[0] * dims[0], uv0[1] * dims[1]];
    // Texels per meter of descent.
    let dir = [dir_uv_per_meter[0] * dims[0], dir_uv_per_meter[1] * dims[1]];
    // D6: the ray enters the band at the peak — or at the eye, if lower —
    // `top_m` above the plane, which is `dir · top_m` texels back toward the
    // viewer from `p0`.
    let start = [p0[0] - dir[0] * top_m, p0[1] - dir[1] * top_m];
    let mut dda = surface_depth_dda_setup(start, dir);

    // P2: the ray never leaves its starting texel above the band's floor, so
    // it meets this texel's top. Exactly the loop's first iteration, whatever
    // the texel holds: a top above the start (`solid <= 0`, an eye inside the
    // band) is a hit at the start, and a top the ray does not reach inside
    // this texel — below the band, or a NaN — is left to the loop.
    if single_texel_early_out && surface_depth_single_texel_band(&dda, (top - band.trough) * scale)
    {
        let solid = (top - field.texel_height(dda.cell[0], dda.cell[1])) * scale;
        if solid < dda.t_max[0].min(dda.t_max[1]) {
            let z_hit = if solid <= 0.0 { 0.0 } else { solid };
            return resolve_view_hit(
                start,
                dir,
                dims,
                top_m,
                z_hit,
                [0.0, 0.0],
                SurfaceDepthFace::Top,
                0,
            );
        }
    }

    let steps_allowed = max_steps.max(1);
    let mut z_enter = 0.0f32;
    let mut entry_face = SurfaceDepthFace::Top;
    let mut entry_bias = [0.0f32, 0.0f32];
    let mut walked = 0u32;

    let (z_hit, face, bias) = loop {
        let solid = (top - field.texel_height(dda.cell[0], dda.cell[1])) * scale;
        let z_exit = dda.t_max[0].min(dda.t_max[1]);
        if z_enter >= solid {
            break (z_enter, entry_face, entry_bias);
        }
        if z_exit > solid {
            break (solid, SurfaceDepthFace::Top, [0.0, 0.0]);
        }
        // Budget exhausted with the ray still in open space: resolve FLAT at
        // the true plane (D7). Resolving at the last crossed boundary instead
        // smeared the texture toward the viewer at grazing angles; flat reads
        // as the effect switched off for that fragment, which is honest.
        if walked + 1 >= steps_allowed {
            return SurfaceDepthHit {
                steps: walked,
                starved: true,
                ..SurfaceDepthHit::flat(uv0)
            };
        }
        if dda.t_max[0] <= dda.t_max[1] {
            dda.cell[0] += dda.step[0];
            z_enter = dda.t_max[0];
            dda.t_max[0] += dda.t_delta[0];
            entry_face = if dda.step[0] > 0 {
                SurfaceDepthFace::NegU
            } else {
                SurfaceDepthFace::PosU
            };
            entry_bias = [dda.step[0] as f32 * SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS, 0.0];
        } else {
            dda.cell[1] += dda.step[1];
            z_enter = dda.t_max[1];
            dda.t_max[1] += dda.t_delta[1];
            entry_face = if dda.step[1] > 0 {
                SurfaceDepthFace::NegV
            } else {
                SurfaceDepthFace::PosV
            };
            entry_bias = [0.0, dda.step[1] as f32 * SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS];
        }
        walked += 1;
    };

    resolve_view_hit(start, dir, dims, top_m, z_hit, bias, face, walked)
}

/// Turn a descent `z_hit` below the start height `top_m` into the reported
/// hit. Shared by the loop and the P2 early-out so both produce bit-identical
/// results.
#[allow(clippy::too_many_arguments)]
fn resolve_view_hit(
    start: [f32; 2],
    dir: [f32; 2],
    dims: [f32; 2],
    top_m: f32,
    z_hit: f32,
    bias: [f32; 2],
    face: SurfaceDepthFace,
    walked: u32,
) -> SurfaceDepthHit {
    let hit = [start[0] + dir[0] * z_hit, start[1] + dir[1] * z_hit];
    SurfaceDepthHit {
        uv: [(hit[0] + bias[0]) / dims[0], (hit[1] + bias[1]) / dims[1]],
        march_uv: [hit[0] / dims[0], hit[1] / dims[1]],
        height_meters: top_m - z_hit,
        face,
        steps: walked,
        starved: false,
    }
}

/// The self-shadow march's step budget from the material's view-march budget:
/// half of it, never below one. The shader's `max(max_steps / 2u, 1u)`.
pub fn surface_depth_shadow_steps(max_steps: u32) -> u32 {
    (max_steps / 2).max(1)
}

/// Self-shadow one dynamic light: a second, shorter DDA from the hit point
/// toward the light. Returns 1.0 lit, 0.0 occluded.
///
/// Takes the whole view-march result, not its height, so D7 cannot be
/// skipped: a starved hit is flat, and a flat fragment never self-shadows
/// (the shader's `carved = false` gate). The march starts at the hit's
/// unbiased `march_uv`.
///
/// The ray rises from the hit and ends as soon as it climbs above the
/// material's PEAK raise (P3) — not the plane: with raised texels around, a
/// hit on the plane can still be shadowed. A top hit at the peak height has
/// nothing above it and skips the march. `peak_raise` is the quantized
/// fraction from the uniform; `max_steps` is [`surface_depth_shadow_steps`]
/// of the material's budget.
///
/// Legitimate against DYNAMIC lights only — the bake knows nothing about
/// dynamic bodies (`rendering_pipeline.md` §4: "the runtime owns only the facts
/// that involve a dynamic body"). There is deliberately NO march against baked
/// static light: the lightmap owns static-onto-static occlusion at 4 cm/texel
/// and competing with it risks the no-double-counting invariant.
///
/// The texel the ray starts in is never tested, so a hit never shadows itself:
/// a top hit sits exactly on that texel's solid, and a side hit sits on its
/// wall (and a light behind that wall is already culled by `NdotL <= 0`, since
/// the wall normal IS the shading normal).
pub fn surface_depth_light_visibility(
    field: &SurfaceDepthField<'_>,
    hit: &SurfaceDepthHit,
    light_uv_per_meter: [f32; 2],
    depth_scale_meters: f32,
    peak_raise: f32,
    max_steps: u32,
) -> f32 {
    if hit.starved {
        return 1.0;
    }
    let hit_uv = hit.march_uv;
    let scale = surface_depth_march_scale(depth_scale_meters);
    // How far the light ray must rise before it clears the band.
    let clearance = peak_raise * scale - hit.height_meters;
    if !above(clearance, SURFACE_DEPTH_SHADOW_BIAS_M) {
        return 1.0;
    }
    let dims = [field.width as f32, field.height as f32];
    let p0 = [hit_uv[0] * dims[0], hit_uv[1] * dims[1]];
    // Texels per meter of rise.
    let dir = [
        light_uv_per_meter[0] * dims[0],
        light_uv_per_meter[1] * dims[1],
    ];
    let mut dda = surface_depth_dda_setup(p0, dir);

    for _ in 0..max_steps.max(1) {
        // Rising past the peak means the ray has left the relief band.
        if dda.t_max[0].min(dda.t_max[1]) >= clearance {
            return 1.0;
        }
        let risen;
        if dda.t_max[0] <= dda.t_max[1] {
            dda.cell[0] += dda.step[0];
            risen = dda.t_max[0];
            dda.t_max[0] += dda.t_delta[0];
        } else {
            dda.cell[1] += dda.step[1];
            risen = dda.t_max[1];
            dda.t_max[1] += dda.t_delta[1];
        }
        // Both sides measured down from the peak: the ray sits `clearance −
        // risen` below it, the texel's top `solid` below it.
        let solid = (peak_raise - field.texel_height(dda.cell[0], dda.cell[1])) * scale;
        if clearance - risen > solid + SURFACE_DEPTH_SHADOW_BIAS_M {
            return 0.0;
        }
    }
    1.0
}
