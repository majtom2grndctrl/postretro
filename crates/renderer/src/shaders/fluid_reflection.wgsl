// Fluid surface reflection: the distance-corrected box-projected probe term
// fs_main_reflecting composites. Concatenated after fluid_surface.wgsl.
// See: context/lib/rendering_pipeline.md §7.3a

// Reflection probes. Mirrors `postretro_render_cpu::fluid_surface::reflection`:
// each function and constant here is pinned to it, and `fs_main_reflecting`'s
// tail inlines its `reflection_fragment` after the fog composite.
const FLUID_REFLECTION_BASE_F0: f32 = 0.02;
const FLUID_REFLECTION_NO_PROBE: u32 = 0xFFFFFFFFu;
const FLUID_REFLECTION_ALL_TERMS: u32 = 0x17Fu;
const FLUID_REFLECTION_DIRECTION_EPSILON: f32 = 1.0e-6;
const FLUID_REFLECTION_COVERAGE_EPSILON: f32 = 1.0e-4;
const FLUID_REFLECTION_REFERENCE_FLOOR: f32 = 1.0e-30;
const FLUID_REFLECTION_UNORM10_MAX: u32 = 1023u;
const FLUID_REFLECTION_NO_LIGHT: u32 = 0xFFFFFFFFu;
const FLUID_REFLECTION_LIGHTS_PER_PROBE: u32 = 4u;
const FLUID_REFLECTION_RAY_MIN_Y: f32 = 1.0e-3;

// The capture bakes every lighting term together, so any isolating mask
// omits the probe term.
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

// The runtime slab test: where the reflected ray leaves the projection box.
// A component near zero is nudged off it rather than divided by. The origin
// may lie past the box (water outside an authored box, below a derived
// box's floor); a slab whose exit lies at or behind it (past a face the ray
// leaves through) does not stop the ray, which ends at another slab's exit.
fn fluid_reflection_box_hit(origin: vec3<f32>, ray: vec3<f32>, box_min: vec3<f32>,
                            box_max: vec3<f32>) -> vec3<f32> {
    let nudged = select(vec3<f32>(FLUID_REFLECTION_DIRECTION_EPSILON),
        vec3<f32>(-FLUID_REFLECTION_DIRECTION_EPSILON), ray < vec3<f32>(0.0));
    let safe = select(ray, nudged, abs(ray) < vec3<f32>(FLUID_REFLECTION_DIRECTION_EPSILON));
    let t0 = (box_min - origin) / safe;
    let t1 = (box_max - origin) / safe;
    let far = max(t0, t1);
    let lead = max(far.x, max(far.y, far.z));
    let ahead = select(vec3<f32>(lead), far, far > vec3<f32>(0.0));
    let t = max(min(ahead.x, min(ahead.y, ahead.z)), 0.0);
    return origin + ray * t;
}

// From the capture point to the hit, clamped to the horizon below it.
fn fluid_reflection_lookup(hit: vec3<f32>, capture_point: vec3<f32>, ray: vec3<f32>) -> vec3<f32> {
    let toward = hit - capture_point;
    let level = vec3<f32>(toward.x, max(toward.y, 0.0), toward.z);
    return select(level, vec3<f32>(ray.x, max(ray.y, 0.0), ray.z),
        dot(level, level) < FLUID_REFLECTION_DIRECTION_EPSILON);
}

// The SH atlas's octahedral mapping and border convention over one tile.
fn fluid_reflection_tile_uv(direction: vec3<f32>, tile_dimension: u32, tile_border: u32) -> vec2<f32> {
    let dimension = f32(tile_dimension);
    let border = f32(tile_border);
    let interior = dimension - 2.0 * border;
    return (vec2<f32>(border) + oct_encode_unquantized(direction) * interior) / dimension;
}

// Schlick with base reflectance 0.02 x reflectivity, topping at 1.
fn fluid_reflection_fresnel(cos_theta: f32, reflectivity: f32) -> f32 {
    let f0 = min(FLUID_REFLECTION_BASE_F0 * reflectivity, 1.0);
    let m = 1.0 - clamp(cos_theta, 0.0, 1.0);
    let m2 = m * m;
    return f0 + (1.0 - f0) * (m2 * m2 * m);
}

// The reflection over the straight-alpha surface (over fog) at union
// coverage, still straight alpha: opacity never dims the reflection.
fn fluid_reflection_composite(surface: vec4<f32>, reflection: vec3<f32>, fresnel: f32) -> vec4<f32> {
    let coverage = fresnel + (1.0 - fresnel) * surface.a;
    let rgb = (reflection * fresnel + surface.rgb * ((1.0 - fresnel) * surface.a))
        / max(coverage, FLUID_REFLECTION_COVERAGE_EPSILON);
    return vec4<f32>(rgb, coverage);
}

// Live lighting by light share. The capture holds every animated light at its
// reference; each stored light scales it by its share toward the lookup
// direction times its live-over-reference change. Reads no SH and no
// promotion weight, so a switch reaches every reflection on the next frame at
// any distance, and promotion never moves it.

// The share lobes' blend weights for a lookup direction: up, +X, -X, +Z, -Z.
// Mirrors level-format `reflection_probe_lobe_weights`, which the compiler
// split the capture's light by.
fn fluid_reflection_lobe_weights(direction: vec3<f32>) -> array<f32, 5> {
    let positive = max(direction, vec3<f32>(0.0));
    let negative = max(-direction, vec3<f32>(0.0));
    let down = negative.y * negative.y * 0.25;
    let weights = array<f32, 5>(
        positive.y * positive.y,
        positive.x * positive.x + down,
        negative.x * negative.x + down,
        positive.z * positive.z + down,
        negative.z * negative.z + down,
    );
    let sum = weights[0] + weights[1] + weights[2] + weights[3] + weights[4];
    if !(sum > 0.0) {
        return array<f32, 5>(1.0, 0.0, 0.0, 0.0, 0.0);
    }
    return array<f32, 5>(weights[0] / sum, weights[1] / sum, weights[2] / sum, weights[3] / sum,
        weights[4] / sum);
}

// The reserved roster value, and any index past the descriptors, indexes
// nothing: the entry adds no term.
fn fluid_reflection_light_present(roster_index: u32) -> bool {
    return roster_index != FLUID_REFLECTION_NO_LIGHT
        && roster_index < arrayLength(&anim_descriptors);
}

// A stored light's live value: sh_compose.wgsl's `animated_light_scale`, read
// straight from descriptor `roster_index` (identity table). Clamped colour x
// clamped brightness, 0 while inactive; no promotion weight, lighting mask or
// debug override.
fn fluid_reflection_light_value(roster_index: u32) -> vec3<f32> {
    let desc = anim_descriptors[roster_index];
    if (desc.is_active == 0u) {
        return vec3<f32>(0.0);
    }

    let t = animation_curve_t(desc.period, desc.phase, uniforms.time);
    let brightness = max(
        sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, t),
        0.0,
    );
    var color = desc.base_color;
    if (desc.color_count > 0u) {
        color = max(
            sample_color_catmull_rom(desc.color_offset, desc.color_count, t, vec3<f32>(1.0)),
            vec3<f32>(0.0),
        ) * desc.base_color;
    }
    return color * brightness;
}

fn fluid_reflection_unpack_unorm10x3(word: u32) -> vec3<f32> {
    let field = FLUID_REFLECTION_UNORM10_MAX;
    return vec3<f32>(f32(word & field), f32((word >> 10u) & field), f32((word >> 20u) & field))
        / f32(field);
}

// A stored light's RGB share toward the lookup: its lobe shares, blended.
fn fluid_reflection_light_share(shares: array<vec4<u32>, 2>, lobes: array<f32, 5>) -> vec3<f32> {
    return fluid_reflection_unpack_unorm10x3(shares[0].x) * lobes[0]
        + fluid_reflection_unpack_unorm10x3(shares[0].y) * lobes[1]
        + fluid_reflection_unpack_unorm10x3(shares[0].z) * lobes[2]
        + fluid_reflection_unpack_unorm10x3(shares[0].w) * lobes[3]
        + fluid_reflection_unpack_unorm10x3(shares[1].x) * lobes[4];
}

// current / reference - 1 per channel; a channel the reference holds no light
// in adds no term.
fn fluid_reflection_light_change(current: vec3<f32>, reference: vec3<f32>) -> vec3<f32> {
    return select(vec3<f32>(0.0),
        current / max(reference, vec3<f32>(FLUID_REFLECTION_REFERENCE_FLOOR)) - vec3<f32>(1.0),
        reference > vec3<f32>(0.0));
}

// The capture's live-lighting scale toward `direction`, clamped at 0. Lights
// past the stored K have no entry and hold their reference.
fn fluid_reflection_ratio(probe: u32, direction: vec3<f32>) -> vec3<f32> {
    let lobes = fluid_reflection_lobe_weights(direction);
    var ratio = vec3<f32>(1.0);
    for (var i: u32 = 0u; i < FLUID_REFLECTION_LIGHTS_PER_PROBE; i = i + 1u) {
        let light = fluid_probes.probes[probe].lights[i];
        if !fluid_reflection_light_present(light.roster_index) {
            continue;
        }
        let current = fluid_reflection_light_value(light.roster_index);
        ratio = ratio + fluid_reflection_light_share(light.shares, lobes)
            * fluid_reflection_light_change(current, light.reference);
    }
    return max(ratio, vec3<f32>(0.0));
}

// Distance search. Mirrors `postretro_render_cpu::fluid_surface::reflection_search`:
// box projection corrected against the probe's stored per-texel distance.
// K sphere steps then M false-position estimates, both constant bounds; at
// most 2 + K + M - 1 distance taps, beside the capture-shadow fade's two to
// four, which depend on the origin alone. Its state lives only inside
// `fluid_reflection_search`, which runs to its end before the colour tap and
// the light-share loop, so only the lookup and the weight outlive it.
const FLUID_REFLECTION_SEARCH_STEPS: u32 = 3u;
const FLUID_REFLECTION_SEARCH_REFINEMENTS: u32 = 2u;
// The capture-shadow fade: N capture texels of angle (pi / interior each)
// beside the shadow, gated on the side tap's stored surface standing in
// front of the opposite one's, and the depth margin kappa behind it.
const FLUID_REFLECTION_SHADOW_FADE_TEXELS: f32 = 2.0;
const FLUID_REFLECTION_SHADOW_GATE_START: f32 = 6.0;
const FLUID_REFLECTION_SHADOW_GATE_FULL: f32 = 10.0;
const FLUID_REFLECTION_SHADOW_MARGIN: f32 = 0.05;
const FLUID_REFLECTION_HALF_TURN: f32 = 3.1415927;
const FLUID_REFLECTION_SEARCH_EPSILON: f32 = 1.0e-6;

// The search's result: the tile lookup direction, and the probe term's
// weight (1, except faded out in the capture shadow).
struct FluidReflectionSearch {
    lookup: vec3<f32>,
    weight: f32,
};

// h(Q) = |v|1 * w(v) - 1 for v the horizon-clamped offset from the capture
// point to q: negative in front of the stored surface, at or above 0 behind
// it; a stored 0 (no surface) gives -1, never a root. `layer` is the face's
// probe layer, passed through unchanged.
fn fluid_reflection_search_h(q: vec3<f32>, capture_point: vec3<f32>, tile: vec2<u32>,
                             layer: u32) -> f32 {
    let toward = q - capture_point;
    let level = vec3<f32>(toward.x, max(toward.y, 0.0), toward.z);
    let l1 = abs(level.x) + abs(level.y) + abs(level.z);
    if l1 < FLUID_REFLECTION_SEARCH_EPSILON {
        return -1.0;
    }
    return l1 * textureSampleLevel(fluid_probe_distance, sh_atlas_sampler,
        fluid_reflection_tile_uv(level, tile.x, tile.y), layer, 0.0).r - 1.0;
}

// The fade's angle in radians: N interior texels of pi / interior each.
fn fluid_reflection_shadow_fade_angle(tile: vec2<u32>) -> f32 {
    return FLUID_REFLECTION_SHADOW_FADE_TEXELS * FLUID_REFLECTION_HALF_TURN
        / (f32(tile.x) - 2.0 * f32(tile.y));
}

// h at the origin turned about the capture point's vertical by `turn`
// radians, keeping its distance; `toward` is the origin minus the capture
// point. No trig: the turn's sine and cosine are short Taylor series, exact
// to f32 at the fade's few degrees.
fn fluid_reflection_shadow_tap(capture_point: vec3<f32>, toward: vec3<f32>, turn: f32,
                               tile: vec2<u32>, layer: u32) -> f32 {
    let turn2 = turn * turn;
    let sine = turn * (1.0 - turn2 / 6.0 * (1.0 - turn2 / 20.0));
    let cosine = 1.0 - turn2 / 2.0 * (1.0 - turn2 / 12.0);
    let turned = vec3<f32>(toward.x * cosine + toward.z * sine, toward.y,
        toward.z * cosine - toward.x * sine);
    return fluid_reflection_search_h(capture_point + turned, capture_point, tile, layer);
}

// One side's shadow edge, in radians: at most `angle`, 0 for an origin
// behind the stored surface. A far tap behind its surface, gated on standing
// in front of the opposite tap's surface, takes a half-turn tap and places
// the edge where h, interpolated linearly, crosses 0.
fn fluid_reflection_shadow_edge(capture_point: vec3<f32>, toward: vec3<f32>, angle: f32,
                                side: f32, h_origin: f32, h_far: f32, h_opposite: f32,
                                tile: vec2<u32>, layer: u32) -> f32 {
    let step = (h_far - h_opposite) / max(1.0 + h_opposite, FLUID_REFLECTION_SEARCH_EPSILON);
    let h_lit = min(h_origin, 0.0);
    let gate = smoothstep(FLUID_REFLECTION_SHADOW_GATE_START * angle,
        FLUID_REFLECTION_SHADOW_GATE_FULL * angle, step);
    if h_far <= 0.0 || gate <= 0.0 {
        return angle;
    }
    let h_half = fluid_reflection_shadow_tap(capture_point, toward, 0.5 * side, tile, layer);
    var crossing: f32;
    if h_half > 0.0 {
        crossing = 0.5 * angle * h_lit / (h_lit - h_half);
    } else {
        crossing = 0.5 * angle + 0.5 * angle * h_half / (h_half - h_far);
    }
    return angle + (crossing - angle) * gate;
}

// The fade beside the capture shadow: 1 with no edge within the angle,
// falling smoothly to 0 at the nearer side's edge. Two taps, three with an
// edge near, four with one each side.
fn fluid_reflection_shadow_fade(origin: vec3<f32>, capture_point: vec3<f32>, tile: vec2<u32>,
                                h_origin: f32, layer: u32) -> f32 {
    let angle = fluid_reflection_shadow_fade_angle(tile);
    let toward = origin - capture_point;
    let h_plus = fluid_reflection_shadow_tap(capture_point, toward, angle, tile, layer);
    let h_minus = fluid_reflection_shadow_tap(capture_point, toward, -angle, tile, layer);
    let edge = min(
        fluid_reflection_shadow_edge(capture_point, toward, angle, angle, h_origin, h_plus,
            h_minus, tile, layer),
        fluid_reflection_shadow_edge(capture_point, toward, angle, -angle, h_origin, h_minus,
            h_plus, tile, layer));
    return smoothstep(0.0, angle, edge);
}

// The search along unit `ray` from `origin` (the fragment, or the pixel
// cell's point), inside the probe's box or past it: two independent taps
// (origin, box hit); an origin behind the stored surface is in the capture
// shadow, drawing nothing past the depth margin. Else the fade beside the
// shadow weighs the term, and up to K sphere steps close a bracket, or the
// box hit does, or nothing stored lies inside the box (the box hit, plain
// box projection); then M false-position estimates on the bracket.
fn fluid_reflection_search(origin: vec3<f32>, ray: vec3<f32>, box_min: vec3<f32>,
                           box_max: vec3<f32>, capture_point: vec3<f32>, tile: vec2<u32>,
                           layer: u32) -> FluidReflectionSearch {
    let box_hit = fluid_reflection_box_hit(origin, ray, box_min, box_max);
    let t_box = dot(box_hit - origin, ray);
    let h_origin = fluid_reflection_search_h(origin, capture_point, tile, layer);
    let h_box = fluid_reflection_search_h(box_hit, capture_point, tile, layer);
    if h_origin >= FLUID_REFLECTION_SHADOW_MARGIN {
        return FluidReflectionSearch(fluid_reflection_lookup(box_hit, capture_point, ray), 0.0);
    }
    let weight = fluid_reflection_shadow_fade(origin, capture_point, tile, h_origin, layer)
        * (1.0 - smoothstep(0.0, FLUID_REFLECTION_SHADOW_MARGIN, h_origin));
    if h_origin >= 0.0 {
        return FluidReflectionSearch(fluid_reflection_lookup(box_hit, capture_point, ray), weight);
    }

    // Front end of the bracket: the last sample in front of a surface. The
    // back end starts at the box hit and moves to the first sample behind one.
    var t_front = 0.0;
    var h_front = h_origin;
    var t_back = t_box;
    var h_back = h_box;
    let from_capture = origin - capture_point;
    let b = dot(from_capture, ray);
    for (var i: u32 = 0u; i < FLUID_REFLECTION_SEARCH_STEPS; i = i + 1u) {
        if h_front <= -1.0 {
            break;
        }
        let q = origin + ray * t_front;
        let toward = q - capture_point;
        let level = vec3<f32>(toward.x, max(toward.y, 0.0), toward.z);
        let radius = length(level) / (h_front + 1.0);
        let disc = b * b - (dot(from_capture, from_capture) - radius * radius);
        if disc < 0.0 {
            break;
        }
        let t_next = -b + sqrt(disc);
        if t_next <= t_front || t_next >= t_box {
            break;
        }
        let h_next = fluid_reflection_search_h(origin + ray * t_next, capture_point, tile, layer);
        if h_next >= 0.0 {
            t_back = t_next;
            h_back = h_next;
            break;
        }
        t_front = t_next;
        h_front = h_next;
    }
    if !(h_back >= 0.0) {
        return FluidReflectionSearch(fluid_reflection_lookup(box_hit, capture_point, ray), weight);
    }

    var t_hit = t_back;
    for (var refinement: u32 = 0u; refinement < FLUID_REFLECTION_SEARCH_REFINEMENTS;
         refinement = refinement + 1u) {
        if h_back != h_front {
            t_hit = t_front - h_front * (t_back - t_front) / (h_back - h_front);
        } else {
            t_hit = t_back;
        }
        if refinement + 1u == FLUID_REFLECTION_SEARCH_REFINEMENTS {
            break;
        }
        let h_hit = fluid_reflection_search_h(origin + ray * t_hit, capture_point, tile, layer);
        if h_hit >= 0.0 {
            t_back = t_hit;
            h_back = h_hit;
        } else {
            t_front = t_hit;
            h_front = h_hit;
        }
    }
    return FluidReflectionSearch(fluid_reflection_lookup(origin + ray * t_hit, capture_point, ray),
        weight);
}

// The probe term before Fresnel: the capture toward the search's lookup,
// scaled by the live-lighting ratio toward that same direction, and the
// search's weight.
struct FluidReflectionSample {
    capture: vec3<f32>,
    weight: f32,
};

// The search runs first and ends; only its lookup and weight reach the colour
// tap and the ratio loop. The layer comes from the face's probe index alone,
// and the distance taps read that same layer; the lookup picks only the texel
// and the share. A mirrored ray at the horizon is lifted above it first.
fn fluid_reflection_sample(probe: u32, world_position: vec3<f32>, view: vec3<f32>,
                           outward: vec3<f32>) -> FluidReflectionSample {
    let ray = fluid_reflection_above_horizon(fluid_reflection_mirror(view, outward));
    let layer = fluid_probes.probes[probe].layer;
    let search = fluid_reflection_search(world_position, ray, fluid_probes.probes[probe].box_min,
        fluid_probes.probes[probe].box_max, fluid_probes.probes[probe].capture_point,
        fluid_probes.tile.xy, layer);
    let uv = fluid_reflection_tile_uv(search.lookup, fluid_probes.tile.x, fluid_probes.tile.y);
    let capture = textureSampleLevel(fluid_probe_array, sh_atlas_sampler, uv, layer, 0.0).rgb;
    return FluidReflectionSample(capture * fluid_reflection_ratio(probe, search.lookup),
        search.weight);
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
