// Shared chart-rasterization math: given a lightmap `Chart` plus its atlas
// placement, compute the world-space position of any interior texel. Consumed
// by both `lightmap_bake` (per-face static bake) and `animated_light_weight_maps`
// (per-chunk animated-light weight bake) so the two bakers never drift at
// chunk boundaries.
//
// See: context/plans/in-progress/animated-light-weight-maps/index.md

use glam::Vec3;

use crate::lightmap_bake::Chart;

/// Padding inserted around each chart in atlas texels. One texel of padding
/// plus the post-bake edge-dilation pass keeps bilinear sampling from dragging
/// black into chart interiors.
///
/// Public because the animated weight-map baker needs to resolve a chunk's
/// atlas-texel rectangle from its owning face's placement, and the placement's
/// interior offset is `(placement + CHART_PADDING_TEXELS, ...)`.
pub const CHART_PADDING_TEXELS: u32 = 2;

/// Where a chart landed in the atlas, in texel coordinates.
///
/// The interior (covered) rectangle of the chart is:
///   `[x + CHART_PADDING_TEXELS, x + width_texels - CHART_PADDING_TEXELS)` on X
///   `[y + CHART_PADDING_TEXELS, y + height_texels - CHART_PADDING_TEXELS)` on Y
#[derive(Debug, Clone, Copy)]
pub struct ChartPlacement {
    pub x: u32,
    pub y: u32,
}

/// Interior (non-padded) dimensions of a chart in atlas texels. Clamped to at
/// least 1 so a degenerate chart still maps to one texel.
pub fn chart_interior_dims(chart: &Chart) -> (i32, i32) {
    let padding = CHART_PADDING_TEXELS as i32;
    let iw = (chart.width_texels as i32 - 2 * padding).max(1);
    let ih = (chart.height_texels as i32 - 2 * padding).max(1);
    (iw, ih)
}

/// World-space position of the texel at interior coordinates `(tx, ty)` within
/// `chart`, where `(tx, ty)` are in `[0, interior_w) × [0, interior_h)`.
///
/// Matches `lightmap_bake::bake_face_chart`'s per-texel derivation exactly —
/// both bakers route their world-position lookups through this function so
/// they agree on texel centres at chunk boundaries.
pub fn chart_texel_world_position(
    chart: &Chart,
    tx: i32,
    ty: i32,
    interior_w: i32,
    interior_h: i32,
) -> Vec3 {
    let u_frac = (tx as f32 + 0.5) / interior_w as f32;
    let v_frac = (ty as f32 + 0.5) / interior_h as f32;
    let local_u = chart.uv_min[0] + u_frac * chart.uv_extent[0];
    let local_v = chart.uv_min[1] + v_frac * chart.uv_extent[1];
    chart.origin + chart.u_axis * local_u + chart.v_axis * local_v
}
