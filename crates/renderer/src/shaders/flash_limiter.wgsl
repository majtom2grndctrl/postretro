// Photosensitivity flash limiter: measure + limit compute passes.
// Appended to `screen_effects.wgsl` at pipeline creation, so it shares that
// file's group-0 bindings, `compose_presented`, `LUMA`, the cell grid and
// `LimiterCellParams`. Runs ahead of the resolve every frame; the resolve reads
// `cell_params` back through `presented_cell_params`.
//
// History lives only here, on the GPU: each cell's last presented mean color,
// its luminance and redness extrema, and the global flash window. Nothing reads
// back.
// See context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter).

// Mirrors `LimiterFrameUniform` in render-cpu/src/flash_limiter.rs.
struct LimiterFrame {
    dt_window: f32,
    dt_rate: f32,
    enabled: u32,
    reset: u32,
    init: u32,
    // 1 when splash frames presented since the previous resolve frame.
    splash_active: u32,
    // How long ago the splash stretch began; its drop into the splash happened
    // then.
    splash_seconds: f32,
    _pad0: u32,
    // The splash's presented color (linear).
    splash_rgb: vec4<f32>,
}

struct LimiterCellState {
    // Mean color and relative luminance the resolve last presented in this cell.
    out_rgb: vec3<f32>,
    out_lum: f32,
    // Presented luminance at the last extremum, and the direction of travel
    // since it (-1, 0, +1). A transition accumulates from the extremum.
    ext_lum: f32,
    dir: f32,
    // The same for the cell's redness (chromaticity only).
    red_ext: f32,
    red_dir: f32,
    // 1 once the current luminance / redness excursion has been counted as a
    // transition; a reversal of direction clears it (IRIS's accumulator).
    counted: f32,
    red_counted: f32,
    _pad0: f32,
    _pad1: f32,
}

const FLASH_WINDOW_SLOTS: u32 = 8u;

struct LimiterGlobal {
    // Ages (seconds) of the transitions counted in the flash window; < 0 is an
    // empty slot. Ages advance by presented-frame time, so no absolute clock
    // drifts. Luminance and red transitions share it.
    ages: array<f32, 8>,
    count: u32,
    // 1 while an over-budget brightening is held back.
    suppressing: u32,
    // 1 while an over-budget move toward saturated red is held back.
    red_suppressing: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(1) @binding(0) var<uniform> limiter: LimiterFrame;
@group(1) @binding(1) var<storage, read_write> cell_measure: array<vec4<f32>, 144>;
@group(1) @binding(2) var<storage, read_write> cell_state: array<LimiterCellState, 144>;
@group(1) @binding(3) var<storage, read_write> cell_params: array<LimiterCellParams, 144>;
@group(1) @binding(4) var<storage, read_write> limiter_global: LimiterGlobal;

// WCAG 2.2 general flash: opposing changes of at least 0.1 relative luminance
// with the darker state below 0.8; at most three flashes (six transitions) in
// any one second.
const FLASH_LUMINANCE_THRESHOLD: f32 = 0.1;
const FLASH_DARK_LIMIT: f32 = 0.8;
const FLASH_MAX_TRANSITIONS: u32 = 6u;
const FLASH_WINDOW_SECONDS: f32 = 1.0;
// WCAG flash-area threshold, 0.111 of the frame (341×256 at 1024×768), in
// cells of 1/144 of the frame.
const FLASH_AREA_CELLS: f32 = 15.984;
// Full-screen intensity change cap: black to white in 250 ms.
const INTENSITY_RATE_PER_SECOND: f32 = 4.0;
// With no budget left for an onset, any area-wide move toward one is held, so
// a suppressed strobe can neither creep toward the threshold nor reverse
// direction and start a new transition the next return would count.
const SUPPRESSION_DEADBAND: f32 = 1e-4;
const RED_SUPPRESSION_DEADBAND: f32 = 1e-4;
// Red flash: one state saturated red, R/(R+G+B) ≥ 0.8, which is redness ≥ 0.7
// on the scale below; a transition is a same-sign redness change of at least
// 0.2 accumulated from the last extremum.
const RED_SATURATED: f32 = 0.7;
const RED_TRANSITION_THRESHOLD: f32 = 0.2;

// Redness from chromaticity alone: 0 for any neutral or non-red color, 1 for
// pure red, whatever the brightness. A color too dark to have a hue reads 0.
fn redness(rgb: vec3<f32>) -> f32 {
    let total = rgb.r + rgb.g + rgb.b;
    if total < 0.003 {
        return 0.0;
    }
    return clamp((rgb.r / total - 1.0 / 3.0) * 1.5, 0.0, 1.0);
}

// ---- Measure: mean presented color per cell --------------------------------

var<workgroup> wg_sum: array<vec3<f32>, 256>;

// One workgroup per cell. Every pixel is sampled through `compose_presented`,
// the resolve's own composite, so screen-effect and scene flashes are measured
// together as the player sees them.
@compute @workgroup_size(16, 16)
fn cs_measure_cells(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    let dims = textureDimensions(scene_color_tex);
    // Pixels whose `limiter_cell_of` is this cell: [ceil(c·W/16), ceil((c+1)·W/16)).
    let x0 = (wg.x * dims.x + LIMITER_CELLS_X - 1u) / LIMITER_CELLS_X;
    let x1 = ((wg.x + 1u) * dims.x + LIMITER_CELLS_X - 1u) / LIMITER_CELLS_X;
    let y0 = (wg.y * dims.y + LIMITER_CELLS_Y - 1u) / LIMITER_CELLS_Y;
    let y1 = ((wg.y + 1u) * dims.y + LIMITER_CELLS_Y - 1u) / LIMITER_CELLS_Y;
    let inv_dims = vec2<f32>(1.0) / vec2<f32>(dims);

    var sum = vec3<f32>(0.0);
    for (var y = y0 + lid.y; y < y1; y += 16u) {
        for (var x = x0 + lid.x; x < x1; x += 16u) {
            let uv = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) * inv_dims;
            sum += compose_presented(uv).rgb;
        }
    }
    wg_sum[li] = sum;
    workgroupBarrier();
    for (var stride = 128u; stride > 0u; stride = stride >> 1u) {
        if li < stride {
            wg_sum[li] += wg_sum[li + stride];
        }
        workgroupBarrier();
    }
    if li == 0u {
        let pixels = max((x1 - x0) * (y1 - y0), 1u);
        cell_measure[wg.y * LIMITER_CELLS_X + wg.x] = vec4<f32>(wg_sum[0] / f32(pixels), 0.0);
    }
}

// ---- Limit: the rules over the whole grid ----------------------------------

var<workgroup> wg_rate_delta: array<f32, 144>;
var<workgroup> wg_excursion: array<f32, 144>;
var<workgroup> wg_darker: array<f32, 144>;
var<workgroup> wg_red_excursion: array<f32, 144>;
var<workgroup> wg_red_peak: array<f32, 144>;
var<workgroup> wg_counted: array<u32, 144>;
var<workgroup> wg_cap_sign_mask: u32;
// Bits: 1 luminance up counted, 2 luminance down, 4 red up, 8 red down.
var<workgroup> wg_event_mask: u32;
var<workgroup> wg_suppressing: u32;
var<workgroup> wg_red_suppressing: u32;

struct Track {
    ext: f32,
    dir: f32,
    // The direction reversed: a new excursion starts, not yet counted.
    reversed: bool,
}

// Follow a presented value from `prev` to `next`: a reversal of direction makes
// `prev` the new extremum, so a transition is the same-sign change accumulated
// from the last extremum (IRIS's model). A smooth strobe therefore counts at
// any refresh rate.
fn track(prev: f32, ext: f32, dir: f32, next: f32) -> Track {
    let step = next - prev;
    var t = Track(ext, dir, false);
    if (step > 0.0 && dir < 0.0) || (step < 0.0 && dir > 0.0) {
        t.ext = prev;
        t.dir = sign(step);
        t.reversed = true;
    } else if dir == 0.0 && step != 0.0 {
        t.dir = sign(step);
    }
    return t;
}

// Area, in cells, of the change of `sign` held in `wg_rate_delta` beyond
// `floor`. Each cell weighs by its change relative to the largest one: a cell
// the change only partly covers sees a diluted mean, so for a uniform change
// its weight is its coverage. A change that straddles cell boundaries is
// therefore measured by the area it covers, not the cells it touches.
fn rate_area(sign_v: f32, floor_v: f32) -> f32 {
    var peak = 0.0;
    for (var c = 0u; c < LIMITER_CELL_COUNT; c++) {
        peak = max(peak, sign_v * wg_rate_delta[c]);
    }
    if peak <= floor_v {
        return 0.0;
    }
    var area = 0.0;
    for (var c = 0u; c < LIMITER_CELL_COUNT; c++) {
        area += clamp(sign_v * wg_rate_delta[c] / peak, 0.0, 1.0);
    }
    return area;
}

// Area, in cells, of the luminance transition of `sign` whose excursion from
// the last extremum reaches `threshold` with the darker state below the WCAG
// limit. Weighted like `rate_area`.
fn transition_area(sign_v: f32, threshold: f32) -> f32 {
    var peak = 0.0;
    for (var c = 0u; c < LIMITER_CELL_COUNT; c++) {
        if wg_darker[c] < FLASH_DARK_LIMIT && (wg_counted[c] & 1u) == 0u {
            peak = max(peak, sign_v * wg_excursion[c]);
        }
    }
    if peak < threshold {
        return 0.0;
    }
    var area = 0.0;
    for (var c = 0u; c < LIMITER_CELL_COUNT; c++) {
        if wg_darker[c] < FLASH_DARK_LIMIT && (wg_counted[c] & 1u) == 0u {
            area += clamp(sign_v * wg_excursion[c] / peak, 0.0, 1.0);
        }
    }
    return area;
}

// Area, in cells, of the red transition of `sign`: a redness excursion of at
// least `threshold` where one of its states is saturated red.
fn red_transition_area(sign_v: f32, threshold: f32) -> f32 {
    var peak = 0.0;
    for (var c = 0u; c < LIMITER_CELL_COUNT; c++) {
        if wg_red_peak[c] >= RED_SATURATED && (wg_counted[c] & 2u) == 0u {
            peak = max(peak, sign_v * wg_red_excursion[c]);
        }
    }
    if peak < threshold {
        return 0.0;
    }
    var area = 0.0;
    for (var c = 0u; c < LIMITER_CELL_COUNT; c++) {
        if wg_red_peak[c] >= RED_SATURATED && (wg_counted[c] & 2u) == 0u {
            area += clamp(sign_v * wg_red_excursion[c] / peak, 0.0, 1.0);
        }
    }
    return area;
}

fn push_transition(g: ptr<function, LimiterGlobal>, count: ptr<function, u32>, age: f32) {
    if *count < FLASH_WINDOW_SLOTS && age < FLASH_WINDOW_SECONDS {
        (*g).ages[*count] = age;
        *count = *count + 1u;
    }
}

// Thread 0: clear on reset, then age the flash window by the full elapsed
// presented time. Returns the live count.
fn age_window(g: ptr<function, LimiterGlobal>) -> u32 {
    if limiter.reset != 0u {
        for (var k = 0u; k < FLASH_WINDOW_SLOTS; k++) {
            (*g).ages[k] = -1.0;
        }
        (*g).suppressing = 0u;
        (*g).red_suppressing = 0u;
    }
    var kept: array<f32, 8>;
    var count = 0u;
    for (var k = 0u; k < FLASH_WINDOW_SLOTS; k++) {
        kept[k] = -1.0;
    }
    for (var k = 0u; k < FLASH_WINDOW_SLOTS; k++) {
        if (*g).ages[k] >= 0.0 {
            let age = (*g).ages[k] + limiter.dt_window;
            if age < FLASH_WINDOW_SECONDS {
                kept[count] = age;
                count++;
            }
        }
    }
    (*g).ages = kept;
    return count;
}

// Thread 0, after a splash stretch: the drop from the last resolve frame into
// the splash happened when the stretch began. The splash path wrote it
// straight to the swapchain, so it cannot be suppressed; it is recorded at its
// true age so both edges of a load count against the budget.
fn record_splash_edge() {
    var g = limiter_global;
    var count = age_window(&g);
    // A fresh limiter has no earlier resolve frame to drop from: the boot
    // splash simply becomes the level the first frame is limited against.
    for (var s = 0; s < select(2, 0, limiter.init != 0u); s++) {
        let sign_i = select(-1, 1, s == 0);
        if transition_area(f32(sign_i), FLASH_LUMINANCE_THRESHOLD) >= FLASH_AREA_CELLS {
            push_transition(&g, &count, limiter.splash_seconds);
        }
    }
    g.count = count;
    limiter_global = g;
}

// Thread 0: count, admit or suppress this frame's transitions. `aged` is true
// when the splash hand-off already aged the window this frame.
//
// Only onsets — brightenings, and moves toward saturated red — are ever
// suppressed, and one is admitted only while the window can still hold it and
// the return that follows it. Returns are always admitted, and signs
// alternate, so the window never exceeds its six transitions and a limited
// strobe always comes to rest dark and unsaturated. Holding a frame brighter
// than its content would need detail the frame no longer has: with per-cell
// history only, the limiter could fill the cell with a flat color, and that
// fill itself flickers wherever the cell holds finer structure. Red is limited
// by desaturation, which keeps luminance exactly.
fn decide_transitions(aged: bool) {
    var g = limiter_global;
    var count = g.count;
    if !aged {
        count = age_window(&g);
    }

    let onset_fits = count + 2u <= FLASH_MAX_TRANSITIONS;
    if onset_fits {
        g.suppressing = 0u;
        g.red_suppressing = 0u;
    }

    var events = 0u;
    for (var s = 0; s < 2; s++) {
        let sign_v = select(-1.0, 1.0, s == 0);
        let onset = s == 0;

        if transition_area(sign_v, FLASH_LUMINANCE_THRESHOLD) >= FLASH_AREA_CELLS {
            if !onset || onset_fits {
                push_transition(&g, &count, 0.0);
                events |= select(2u, 1u, onset);
            } else {
                g.suppressing = 1u;
            }
        } else if onset && !onset_fits
            && transition_area(sign_v, SUPPRESSION_DEADBAND) >= FLASH_AREA_CELLS {
            g.suppressing = 1u;
        }

        if red_transition_area(sign_v, RED_TRANSITION_THRESHOLD) >= FLASH_AREA_CELLS {
            if !onset || onset_fits {
                push_transition(&g, &count, 0.0);
                events |= select(8u, 4u, onset);
            } else {
                g.red_suppressing = 1u;
            }
        } else if onset && !onset_fits
            && red_transition_area(sign_v, RED_SUPPRESSION_DEADBAND) >= FLASH_AREA_CELLS {
            g.red_suppressing = 1u;
        }
    }
    g.count = count;
    limiter_global = g;
    wg_event_mask = events;
    wg_suppressing = g.suppressing;
    wg_red_suppressing = g.red_suppressing;
}

// One thread per cell; thread 0 also runs the whole-grid decisions between
// barriers.
@compute @workgroup_size(144)
fn cs_limit_cells(@builtin(local_invocation_index) i: u32) {
    let measured_rgb = cell_measure[i].rgb;
    let measured = dot(measured_rgb, LUMA);
    let measured_red = redness(measured_rgb);
    var st = cell_state[i];

    if limiter.init != 0u {
        st.out_rgb = measured_rgb;
        st.out_lum = measured;
        st.ext_lum = measured;
        st.dir = 0.0;
        st.red_ext = measured_red;
        st.red_dir = 0.0;
        st.counted = 0.0;
        st.red_counted = 0.0;
    }

    if limiter.enabled == 0u {
        // Off: pass content unchanged, but keep tracking what was presented so
        // re-enabling compares against the frame presented just before it.
        cell_params[i] = LimiterCellParams(1.0, 0.0, 0.0, 0.0, vec4<f32>(0.0));
        st.out_rgb = measured_rgb;
        st.out_lum = measured;
        st.ext_lum = measured;
        st.dir = 0.0;
        st.red_ext = measured_red;
        st.red_dir = 0.0;
        st.counted = 0.0;
        st.red_counted = 0.0;
        cell_state[i] = st;
        return;
    }

    if limiter.reset != 0u {
        // History starts this frame: no earlier transition counts.
        st.ext_lum = st.out_lum;
        st.dir = 0.0;
        st.red_ext = redness(st.out_rgb);
        st.red_dir = 0.0;
        st.counted = 0.0;
        st.red_counted = 0.0;
    }

    // Splash hand-off: the player last saw the splash, not this cell's last
    // resolve frame. Record the drop into it, then limit this frame against it.
    var aged = false;
    if limiter.splash_active != 0u {
        let splash_rgb = limiter.splash_rgb.rgb;
        let splash_lum = dot(splash_rgb, LUMA);
        let drop = track(st.out_lum, st.ext_lum, st.dir, splash_lum);
        wg_excursion[i] = splash_lum - drop.ext;
        wg_darker[i] = min(splash_lum, drop.ext);
        wg_counted[i] = select(u32(st.counted), 0u, drop.reversed);
        workgroupBarrier();
        if i == 0u {
            record_splash_edge();
        }
        workgroupBarrier();
        st.out_rgb = splash_rgb;
        st.out_lum = splash_lum;
        st.ext_lum = drop.ext;
        st.dir = drop.dir;
        st.red_ext = redness(splash_rgb);
        st.red_dir = 0.0;
        // The drop is spent: counted if it reached the threshold, and the
        // return reverses direction, which starts a fresh excursion either way.
        st.counted = 1.0;
        st.red_counted = 0.0;
        aged = true;
    }

    // Stage 1 — intensity rate cap, applied only when the fast change covers
    // the flash-area threshold.
    let cap = INTENSITY_RATE_PER_SECOND * limiter.dt_rate;
    let delta = measured - st.out_lum;
    wg_rate_delta[i] = delta;
    workgroupBarrier();
    if i == 0u {
        var mask = 0u;
        if rate_area(1.0, cap) >= FLASH_AREA_CELLS {
            mask |= 1u;
        }
        if rate_area(-1.0, cap) >= FLASH_AREA_CELLS {
            mask |= 2u;
        }
        wg_cap_sign_mask = mask;
    }
    let cap_mask = workgroupUniformLoad(&wg_cap_sign_mask);
    var target_lum = measured;
    if delta > cap && (cap_mask & 1u) != 0u {
        target_lum = st.out_lum + cap;
    } else if delta < -cap && (cap_mask & 2u) != 0u {
        target_lum = st.out_lum - cap;
    }

    // Stage 2 — flash budget over the rate-capped candidate, luminance and red.
    let out_red = redness(st.out_rgb);
    let candidate = track(st.out_lum, st.ext_lum, st.dir, target_lum);
    let red_candidate = track(out_red, st.red_ext, st.red_dir, measured_red);
    wg_excursion[i] = target_lum - candidate.ext;
    wg_darker[i] = min(target_lum, candidate.ext);
    wg_red_excursion[i] = measured_red - red_candidate.ext;
    wg_red_peak[i] = max(measured_red, red_candidate.ext);
    // A reversal starts a new excursion that has not been counted yet.
    let lum_counted = select(st.counted, 0.0, candidate.reversed) > 0.5;
    let red_counted = select(st.red_counted, 0.0, red_candidate.reversed) > 0.5;
    wg_counted[i] = select(0u, 1u, lum_counted) | select(0u, 2u, red_counted);
    workgroupBarrier();
    if i == 0u {
        decide_transitions(aged);
    }
    let events = workgroupUniformLoad(&wg_event_mask);
    let suppressing = workgroupUniformLoad(&wg_suppressing);
    let red_suppressing = workgroupUniformLoad(&wg_red_suppressing);
    var final_lum = target_lum;
    if suppressing != 0u && final_lum > st.out_lum {
        final_lum = st.out_lum;
    }

    // Reach the final luminance exactly: scale the frame down, or mix toward
    // the last presented mean color when the frame holds less than the target.
    var gain = 1.0;
    var blend = 0.0;
    if abs(final_lum - measured) > 1e-5 {
        if final_lum < measured {
            gain = final_lum / measured;
        } else if st.out_lum > measured + 1e-5 {
            blend = clamp((final_lum - measured) / (st.out_lum - measured), 0.0, 1.0);
        }
    }
    let prev_rgb = st.out_rgb;
    let luminance_rgb = mix(measured_rgb * gain, prev_rgb, blend);

    // Red: hold redness where it was by desaturating toward the pixel's own
    // luminance, which leaves luminance untouched. Chroma scales with
    // `1 - desaturate`, so the ratio of redness holds it.
    var desaturate = 0.0;
    let shown_red = redness(luminance_rgb);
    if red_suppressing != 0u && shown_red > out_red + 1e-4 {
        desaturate = clamp(1.0 - out_red / shown_red, 0.0, 1.0);
    }
    cell_params[i] = LimiterCellParams(gain, blend, desaturate, 0.0, vec4<f32>(prev_rgb, 0.0));

    let presented_rgb = mix(luminance_rgb, vec3<f32>(dot(luminance_rgb, LUMA)), desaturate);
    let presented_lum = dot(presented_rgb, LUMA);
    let presented_red = redness(presented_rgb);
    let tracked = track(st.out_lum, st.ext_lum, st.dir, presented_lum);
    let red_tracked = track(out_red, st.red_ext, st.red_dir, presented_red);
    // A counted event marks every cell moving its way, so the rest of that
    // excursion never counts again; a presented reversal starts a new one.
    let lum_exc = presented_lum - tracked.ext;
    let red_exc = presented_red - red_tracked.ext;
    let lum_marked = (lum_exc > 0.0 && (events & 1u) != 0u) || (lum_exc < 0.0 && (events & 2u) != 0u);
    let red_marked = (red_exc > 0.0 && (events & 4u) != 0u) || (red_exc < 0.0 && (events & 8u) != 0u);
    let lum_prior = select(st.counted, 0.0, tracked.reversed) > 0.5;
    let red_prior = select(st.red_counted, 0.0, red_tracked.reversed) > 0.5;
    st.counted = select(0.0, 1.0, lum_prior || lum_marked);
    st.red_counted = select(0.0, 1.0, red_prior || red_marked);
    st.out_rgb = presented_rgb;
    st.out_lum = presented_lum;
    st.ext_lum = tracked.ext;
    st.dir = tracked.dir;
    st.red_ext = red_tracked.ext;
    st.red_dir = red_tracked.dir;
    cell_state[i] = st;
}
