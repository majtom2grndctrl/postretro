// The probe term about a bound face's hemisphere axis: the glossy overlay's,
// whose faces bind probes of any facing. The distance search, its lookup and
// the six-lobe live-lighting ratio, each clamped to or blending over the
// hemisphere about `axis`: +Y for an up-facing face, else the face's own
// normal (`fluid_reflection_hemisphere_axis`). Concatenated after
// fluid_reflection.wgsl (the box hit, tile UV, search constants and result)
// and reflection_probe_lobes.wgsl, in the overlay's module only. Water binds
// only up-facing probes and keeps its own up-only forms in
// fluid_reflection.wgsl; about +Y these return what those return from every
// origin within FLUID_REFLECTION_SHADOW_PLANE_DEPTH of the capture point's
// plane. Deeper, the capture-shadow tests read the origin's true position.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces)

// From the capture point to the hit, clamped to the hemisphere about `axis`
// (the horizon below it, for an up-facing probe). Mirrors
// `postretro_render_cpu::fluid_surface::reflection_hemisphere::reflection_lookup_about`.
fn fluid_reflection_lookup_about(hit: vec3<f32>, capture_point: vec3<f32>, ray: vec3<f32>,
                                 axis: vec3<f32>) -> vec3<f32> {
    let level = fluid_reflection_level(hit - capture_point, axis);
    return select(level, fluid_reflection_level(ray, axis),
        dot(level, level) < FLUID_REFLECTION_DIRECTION_EPSILON);
}

// The capture's live-lighting scale toward `direction`, read from the probe's
// sums, which the sums pass (fluid_reflection_sums.wgsl) wrote this frame:
// the lobe weights blend the per-lobe sums, clamped at 0. No fragment
// evaluates a stored light. Mirrors `reflection_ratio_from_sums`.
// IP rule (owner review to change, rendering_pipeline.md §7.3a): only
// probe-bound faces of geometry with fixed position and orientation read the
// sums; a probe bound to a moving surface keeps the per-fragment ratio.
fn fluid_reflection_ratio_from_sums(probe: u32, direction: vec3<f32>) -> vec3<f32> {
    let lobes = fluid_reflection_lobe_weights(direction);
    let base = probe * FLUID_REFLECTION_LOBES;
    var ratio = vec3<f32>(1.0);
    ratio = ratio + fluid_probe_sums.lobes[base].xyz * lobes[0];
    ratio = ratio + fluid_probe_sums.lobes[base + 1u].xyz * lobes[1];
    ratio = ratio + fluid_probe_sums.lobes[base + 2u].xyz * lobes[2];
    ratio = ratio + fluid_probe_sums.lobes[base + 3u].xyz * lobes[3];
    ratio = ratio + fluid_probe_sums.lobes[base + 4u].xyz * lobes[4];
    ratio = ratio + fluid_probe_sums.lobes[base + 5u].xyz * lobes[5];
    return max(ratio, vec3<f32>(0.0));
}

// Distance search about `axis`. Mirrors
// `postretro_render_cpu::fluid_surface::reflection_search_about`: water's
// search (fluid_reflection.wgsl), its constants, result and tap bound, with
// every tap and the lookup clamped to the hemisphere about the bound face's
// axis instead of the horizon. Its state lives only inside
// `fluid_reflection_search_about`, which runs to its end before the colour
// taps and the sums blend, so only the lookup and the weight outlive it.

// h(Q) = |v|1 * w(v) - 1 for v the offset from the capture point to q,
// clamped to the hemisphere about `axis`: negative in front of the stored
// surface, at or above 0 behind it; a stored 0 (no surface) gives -1, never a
// root. `layer` is the face's probe layer, passed through unchanged.
fn fluid_reflection_search_h_about(q: vec3<f32>, capture_point: vec3<f32>, axis: vec3<f32>,
                                   tile: vec2<u32>, layer: u32) -> f32 {
    let level = fluid_reflection_level(q - capture_point, axis);
    let l1 = abs(level.x) + abs(level.y) + abs(level.z);
    if l1 < FLUID_REFLECTION_SEARCH_EPSILON {
        return -1.0;
    }
    return l1 * textureSampleLevel(fluid_probe_distance, sh_atlas_sampler,
        fluid_reflection_tile_uv(level, tile.x, tile.y), layer, 0.0).r - 1.0;
}

// How far behind the capture point's plane, along its axis, a capture-shadow
// test still reads at that plane (m): twice the 2 mm capture lift, so every
// fragment on the probe's frontmost bound plane tests as the search reads.
const FLUID_REFLECTION_SHADOW_PLANE_DEPTH: f32 = 0.004;

// The offset `v` as a capture-shadow test reads it: the search's level within
// the plane depth behind the capture point's plane, bit for bit; deeper (a
// lower bound floor) its true direction, which the full-sphere tile holds,
// lifted by that depth. Mirrors `reflection_shadow_level`.
fn fluid_reflection_shadow_level(v: vec3<f32>, axis: vec3<f32>) -> vec3<f32> {
    return v - axis * clamp(dot(v, axis), -FLUID_REFLECTION_SHADOW_PLANE_DEPTH, 0.0);
}

// h for the capture-shadow tests: q read through the shadow level, so a
// fragment on a bound plane behind the capture point's tests whether the
// capture point saw it.
fn fluid_reflection_shadow_h_about(q: vec3<f32>, capture_point: vec3<f32>, axis: vec3<f32>,
                                   tile: vec2<u32>, layer: u32) -> f32 {
    let level = fluid_reflection_shadow_level(q - capture_point, axis);
    let l1 = abs(level.x) + abs(level.y) + abs(level.z);
    if l1 < FLUID_REFLECTION_SEARCH_EPSILON {
        return -1.0;
    }
    return l1 * textureSampleLevel(fluid_probe_distance, sh_atlas_sampler,
        fluid_reflection_tile_uv(level, tile.x, tile.y), layer, 0.0).r - 1.0;
}

// The capture-shadow h at the origin turned about the capture point's `axis`
// (its vertical, for an up-facing probe) by `turn` radians, keeping its
// distance and height; `toward` is the origin minus the capture point. No
// trig: the turn's sine and cosine are short Taylor series, exact to f32 at
// the fade's few degrees.
fn fluid_reflection_shadow_tap_about(capture_point: vec3<f32>, toward: vec3<f32>, turn: f32,
                                     axis: vec3<f32>, tile: vec2<u32>, layer: u32) -> f32 {
    let turn2 = turn * turn;
    let sine = turn * (1.0 - turn2 / 6.0 * (1.0 - turn2 / 20.0));
    let cosine = 1.0 - turn2 / 2.0 * (1.0 - turn2 / 12.0);
    let along = axis * dot(toward, axis);
    let across = toward - along;
    let turned = along + across * cosine + cross(axis, across) * sine;
    return fluid_reflection_shadow_h_about(capture_point + turned, capture_point, axis, tile,
        layer);
}

// One side's shadow edge, in radians: at most `angle`, 0 for an origin
// behind the stored surface. A far tap behind its surface, gated on standing
// in front of the opposite tap's surface, takes a half-turn tap and places
// the edge where h, interpolated linearly, crosses 0.
fn fluid_reflection_shadow_edge_about(capture_point: vec3<f32>, toward: vec3<f32>, angle: f32,
                                      side: f32, h_origin: f32, h_far: f32, h_opposite: f32,
                                      axis: vec3<f32>, tile: vec2<u32>, layer: u32) -> f32 {
    let step = (h_far - h_opposite) / max(1.0 + h_opposite, FLUID_REFLECTION_SEARCH_EPSILON);
    let h_lit = min(h_origin, 0.0);
    let gate = smoothstep(FLUID_REFLECTION_SHADOW_GATE_START * angle,
        FLUID_REFLECTION_SHADOW_GATE_FULL * angle, step);
    if h_far <= 0.0 || gate <= 0.0 {
        return angle;
    }
    let h_half = fluid_reflection_shadow_tap_about(capture_point, toward, 0.5 * side, axis, tile,
        layer);
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
fn fluid_reflection_shadow_fade_about(origin: vec3<f32>, capture_point: vec3<f32>,
                                      axis: vec3<f32>, tile: vec2<u32>, h_origin: f32,
                                      layer: u32) -> f32 {
    let angle = fluid_reflection_shadow_fade_angle(tile);
    let toward = origin - capture_point;
    let h_plus = fluid_reflection_shadow_tap_about(capture_point, toward, angle, axis, tile,
        layer);
    let h_minus = fluid_reflection_shadow_tap_about(capture_point, toward, -angle, axis, tile,
        layer);
    let edge = min(
        fluid_reflection_shadow_edge_about(capture_point, toward, angle, angle, h_origin, h_plus,
            h_minus, axis, tile, layer),
        fluid_reflection_shadow_edge_about(capture_point, toward, angle, -angle, h_origin,
            h_minus, h_plus, axis, tile, layer));
    return smoothstep(0.0, angle, edge);
}

// The search along unit `ray` from `origin` (the fragment, or the pixel
// cell's point), inside the probe's box or past it: two independent taps
// (origin, box hit); an origin behind the stored surface is in the capture
// shadow, drawing nothing past the depth margin. Else the fade beside the
// shadow weighs the term, and up to K sphere steps close a bracket, or the
// box hit does, or nothing stored lies inside the box (the box hit, plain
// box projection); then M false-position estimates on the bracket. Every
// tap and the lookup clamp to the hemisphere about `axis`, but the
// capture-shadow tests read the origin through the shadow level: an origin on
// a bound plane behind the capture point's (a lower floor sharing the probe)
// tests from its true position. It also takes the search's own tap at the
// origin, over the capture point's plane: the term is withheld, and fades by
// depth, by the larger of the two, so a lower floor only ever loses term, and
// the search starts only where both say the origin is in front.
fn fluid_reflection_search_about(origin: vec3<f32>, ray: vec3<f32>, box_min: vec3<f32>,
                                 box_max: vec3<f32>, capture_point: vec3<f32>, axis: vec3<f32>,
                                 tile: vec2<u32>, layer: u32) -> FluidReflectionSearch {
    let box_hit = fluid_reflection_box_hit(origin, ray, box_min, box_max);
    let t_box = dot(box_hit - origin, ray);
    let h_shadow = fluid_reflection_shadow_h_about(origin, capture_point, axis, tile, layer);
    let h_box = fluid_reflection_search_h_about(box_hit, capture_point, axis, tile, layer);
    var h_origin = h_shadow;
    if dot(origin - capture_point, axis) < -FLUID_REFLECTION_SHADOW_PLANE_DEPTH {
        h_origin = fluid_reflection_search_h_about(origin, capture_point, axis, tile, layer);
    }
    let h_depth = max(h_shadow, h_origin);
    if h_depth >= FLUID_REFLECTION_SHADOW_MARGIN {
        return FluidReflectionSearch(
            fluid_reflection_lookup_about(box_hit, capture_point, ray, axis),
            0.0);
    }
    let weight = fluid_reflection_shadow_fade_about(origin, capture_point, axis, tile, h_shadow,
        layer) * (1.0 - smoothstep(0.0, FLUID_REFLECTION_SHADOW_MARGIN, h_depth));
    if h_depth >= 0.0 {
        return FluidReflectionSearch(
            fluid_reflection_lookup_about(box_hit, capture_point, ray, axis),
            weight);
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
        let level = fluid_reflection_level(q - capture_point, axis);
        let radius = length(level) / (h_front + 1.0);
        let disc = b * b - (dot(from_capture, from_capture) - radius * radius);
        if disc < 0.0 {
            break;
        }
        let t_next = -b + sqrt(disc);
        if t_next <= t_front || t_next >= t_box {
            break;
        }
        let h_next = fluid_reflection_search_h_about(origin + ray * t_next, capture_point, axis,
            tile, layer);
        if h_next >= 0.0 {
            t_back = t_next;
            h_back = h_next;
            break;
        }
        t_front = t_next;
        h_front = h_next;
    }
    if !(h_back >= 0.0) {
        return FluidReflectionSearch(
            fluid_reflection_lookup_about(box_hit, capture_point, ray, axis),
            weight);
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
        let h_hit = fluid_reflection_search_h_about(origin + ray * t_hit, capture_point, axis,
            tile, layer);
        if h_hit >= 0.0 {
            t_back = t_hit;
            h_back = h_hit;
        } else {
            t_front = t_hit;
            h_front = h_hit;
        }
    }
    return FluidReflectionSearch(fluid_reflection_lookup_about(origin + ray * t_hit,
        capture_point, ray, axis), weight);
}
