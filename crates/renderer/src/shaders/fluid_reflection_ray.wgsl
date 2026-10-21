// Fluid reflection ray: the reflecting gate, the mirrored ray and the style's
// start point. Binding-agnostic, so the probe term (fluid_reflection.wgsl) and
// the screen-space reflection pass start every ray from one definition. Needs
// fluid_motion.wgsl composed alongside it: `fluid_reflection_point` calls
// `fluid_displayed_offset`.
// See: context/lib/rendering_pipeline.md §7.3a

// The gate and ray. Mirrors `postretro_render_cpu::fluid_surface::reflection`:
// each function and constant here is pinned to it.
const FLUID_REFLECTION_NO_PROBE: u32 = 0xFFFFFFFFu;
const FLUID_REFLECTION_ALL_TERMS: u32 = 0x17Fu;
const FLUID_REFLECTION_RAY_MIN_Y: f32 = 1.0e-3;

// The capture bakes every lighting term together, so any isolating mask
// omits both reflection layers: the probe term and the screen-space layer
// share this gate.
fn fluid_reflection_mask_on(mask: u32) -> bool {
    return (mask & FLUID_REFLECTION_ALL_TERMS) == FLUID_REFLECTION_ALL_TERMS;
}

// From the face's up side, by the interpolated outward normal, never the
// viewer-flipped shading normal.
fn fluid_reflection_seen_from_above(outward: vec3<f32>, view: vec3<f32>) -> bool {
    return dot(outward, view) > 0.0;
}

fn fluid_reflection_gate(probe: u32, outward: vec3<f32>, view: vec3<f32>, mask: u32,
                         reflectivity: f32) -> bool {
    return probe != FLUID_REFLECTION_NO_PROBE
        && reflectivity > 0.0
        && fluid_reflection_seen_from_above(outward, view)
        && fluid_reflection_mask_on(mask);
}

fn fluid_reflection_mirror(view: vec3<f32>, outward: vec3<f32>) -> vec3<f32> {
    return outward * (2.0 * dot(view, outward)) - view;
}

// The reflected ray kept in the upper hemisphere: unchanged at or above the
// floor, else lifted to it and renormalized, azimuth kept. A pixel cell's
// tilt, or a flat mirror's grazing view, can reach the floor.
fn fluid_reflection_above_horizon(ray: vec3<f32>) -> vec3<f32> {
    if ray.y >= FLUID_REFLECTION_RAY_MIN_Y {
        return ray;
    }
    return normalize(vec3<f32>(ray.x, FLUID_REFLECTION_RAY_MIN_Y, ray.z));
}

// Reflection style. Mirrors `postretro_render_cpu::fluid_surface::reflection_style`:
// smooth reflects the flat face per fragment; pixel reflects once per water
// texel cell, from the cell's world-space centre (held to the probe's box at
// a wall rim) about a normal tilted by the cell's displayed offset, so every
// fragment of a cell reads one sample.
// Unlike the warp, this look is the author's (`render.reflections.style`).
const FLUID_REFLECTION_STYLE_PIXEL: u32 = 0u;
const FLUID_REFLECTION_STYLE_SMOOTH: u32 = 1u;
const FLUID_REFLECTION_PIXEL_TILT: f32 = 0.05;
const FLUID_REFLECTION_AXES_EPSILON: f32 = 1.0e-8;
// A pulled rim cell lands this fraction of its horizontal L1 distance from
// the capture point inside the box (at least the floor, in metres), off the
// face where the stored wall's h is 0 and a ray into it leaves at t = 0.
const FLUID_REFLECTION_RIM_INSET: f32 = 1.0e-3;
const FLUID_REFLECTION_RIM_INSET_MIN: f32 = 1.0e-4;

// The face's world step per texel along u and v.
struct FluidTexelAxes {
    u: vec3<f32>,
    v: vec3<f32>,
};

struct FluidReflectionPoint {
    position: vec3<f32>,
    normal: vec3<f32>,
};

// The texel-to-world Jacobian from one fragment's screen derivatives; zero on
// an edge-on or vanished footprint.
fn fluid_reflection_texel_axes(ddx_texel: vec2<f32>, ddy_texel: vec2<f32>, ddx_world: vec3<f32>,
                               ddy_world: vec3<f32>) -> FluidTexelAxes {
    let det = ddx_texel.x * ddy_texel.y - ddy_texel.x * ddx_texel.y;
    let scale = dot(ddx_texel, ddx_texel) * dot(ddy_texel, ddy_texel);
    if det * det <= FLUID_REFLECTION_AXES_EPSILON * scale {
        return FluidTexelAxes(vec3<f32>(0.0), vec3<f32>(0.0));
    }
    return FluidTexelAxes((ddx_world * ddy_texel.y - ddy_world * ddx_texel.y) / det,
        (ddy_world * ddx_texel.x - ddx_world * ddy_texel.x) / det);
}

// The world-space centre of the unwarped texel cell the fragment lies in.
fn fluid_reflection_cell_centre(texel_uv: vec2<f32>, world_position: vec3<f32>,
                                axes: FluidTexelAxes) -> vec3<f32> {
    let to_centre = floor(texel_uv) + vec2<f32>(0.5) - texel_uv;
    return world_position + axes.u * to_centre.x + axes.v * to_centre.y;
}

// The cell centre pulled toward the probe's box inset horizontally by the
// rim inset m: whole while its horizontal overshoot o past the face is at
// most h, half the cell's longer diagonal, then tapering to nothing at 2h,
// the pull scaled by clamp((2h - |o|) / h, 0, 1). A cell straddling a wall
// the box fits starts just in front of the wall's face, not behind the
// stored wall or on it; a cell a full cell or more outside the box stays put.
fn fluid_reflection_cell_within_box(centre: vec3<f32>, axes: FluidTexelAxes,
                                    box_min: vec3<f32>, box_max: vec3<f32>,
                                    capture_point: vec3<f32>) -> vec3<f32> {
    let past = centre - clamp(centre, box_min, box_max);
    let length_past = length(vec3<f32>(past.x, 0.0, past.z));
    let half_cell = 0.5 * max(length(axes.u + axes.v), length(axes.u - axes.v));
    let toward = centre - capture_point;
    let inset = max(FLUID_REFLECTION_RIM_INSET * (abs(toward.x) + abs(toward.z)),
        FLUID_REFLECTION_RIM_INSET_MIN);
    let margin = vec3<f32>(inset, 0.0, inset);
    let target_point = max(min(centre, box_max - margin), box_min + margin);
    let pull = vec3<f32>(centre.x - target_point.x, 0.0, centre.z - target_point.z);
    var keep = 0.0;
    if half_cell > 0.0 {
        keep = clamp((2.0 * half_cell - length_past) / half_cell, 0.0, 1.0);
    }
    return centre - pull * keep;
}

fn fluid_reflection_unit_or_zero(v: vec3<f32>) -> vec3<f32> {
    let length_sq = dot(v, v);
    return select(vec3<f32>(0.0), v * inverseSqrt(length_sq), length_sq > 0.0);
}

// The flat normal tilted along the texture's axes by the cell's offset.
fn fluid_reflection_cell_normal(outward: vec3<f32>, axes: FluidTexelAxes, offset: vec2<i32>) -> vec3<f32> {
    let tilt = fluid_reflection_unit_or_zero(axes.u) * f32(offset.x)
        + fluid_reflection_unit_or_zero(axes.v) * f32(offset.y);
    return normalize(outward + tilt * FLUID_REFLECTION_PIXEL_TILT);
}

// Where the fragment reflects from, and about which normal. The pixel
// offset is the albedo's own shimmer and rings, never drift or the wave; the
// pixel centre is held to the probe's box at a wall rim.
fn fluid_reflection_point(style: u32, texel_uv: vec2<f32>, world_position: vec3<f32>,
                          outward: vec3<f32>, axes: FluidTexelAxes, time: f32,
                          texels_per_pixel: f32, box_min: vec3<f32>,
                          box_max: vec3<f32>, capture_point: vec3<f32>) -> FluidReflectionPoint {
    if style == FLUID_REFLECTION_STYLE_SMOOTH {
        return FluidReflectionPoint(world_position, outward);
    }
    let offset = fluid_displayed_offset(vec2<i32>(floor(texel_uv)), time, texels_per_pixel);
    let centre = fluid_reflection_cell_centre(texel_uv, world_position, axes);
    return FluidReflectionPoint(
        fluid_reflection_cell_within_box(centre, axes, box_min, box_max, capture_point),
        fluid_reflection_cell_normal(outward, axes, offset));
}
