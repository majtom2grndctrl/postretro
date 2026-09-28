// Photosensitivity flash limiter: measure + limit compute passes.
// Appended to `screen_effects.wgsl` at pipeline creation, so it shares that
// file's group-0 bindings, `compose_presented`, `LUMA`, the cell grid and
// `LimiterCellParams`. Runs ahead of the resolve every frame; the resolve reads
// `cell_params` back through `presented_cell_params`.
//
// History lives only here, on the GPU: each cell's last presented mean color,
// its luminance extremum, and the global flash window. Nothing reads back.
// See context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter).

// Mirrors `LimiterFrameUniform` in render-cpu/src/flash_limiter.rs.
struct LimiterFrame {
    dt_window: f32,
    dt_rate: f32,
    enabled: u32,
    reset: u32,
    init: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

struct LimiterCellState {
    // Mean color and relative luminance the resolve last presented in this cell.
    out_rgb: vec3<f32>,
    out_lum: f32,
    // Presented luminance at the last extremum, and the direction of travel
    // since it (-1, 0, +1). A transition accumulates from the extremum.
    ext_lum: f32,
    dir: f32,
    _pad0: f32,
    _pad1: f32,
}

const FLASH_WINDOW_SLOTS: u32 = 8u;

struct LimiterGlobal {
    // Ages (seconds) of the transitions counted in the flash window; < 0 is an
    // empty slot. Ages advance by presented-frame time, so no absolute clock
    // drifts.
    ages: array<f32, 8>,
    count: u32,
    // Sign of the last counted transition (+1 brightening, -1 darkening, 0 none).
    last_sign: i32,
    // 1 while an over-budget brightening is held back.
    suppressing: u32,
    _pad: u32,
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
// With no budget left for a brightening, a change this large already counts
// as one starting, so a suppressed strobe cannot creep toward the threshold.
const SUPPRESSION_DEADBAND: f32 = 0.02;

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
var<workgroup> wg_cap_sign_mask: u32;
var<workgroup> wg_suppressing: u32;

struct Track {
    ext: f32,
    dir: f32,
}

// Follow a cell's presented luminance to `next`: a reversal of direction makes
// the previous presented value the new extremum, so a transition is the
// same-sign change accumulated from the last extremum (IRIS's model). A smooth
// strobe therefore counts at any refresh rate.
fn track(prev: LimiterCellState, next: f32) -> Track {
    let step = next - prev.out_lum;
    var t = Track(prev.ext_lum, prev.dir);
    if (step > 0.0 && prev.dir < 0.0) || (step < 0.0 && prev.dir > 0.0) {
        t.ext = prev.out_lum;
        t.dir = sign(step);
    } else if prev.dir == 0.0 && step != 0.0 {
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

// Area, in cells, of the transition of `sign` whose excursion from the last
// extremum reaches `threshold` with the darker state below the WCAG limit.
// Weighted like `rate_area`.
fn transition_area(sign_v: f32, threshold: f32) -> f32 {
    var peak = 0.0;
    for (var c = 0u; c < LIMITER_CELL_COUNT; c++) {
        if wg_darker[c] < FLASH_DARK_LIMIT {
            peak = max(peak, sign_v * wg_excursion[c]);
        }
    }
    if peak < threshold {
        return 0.0;
    }
    var area = 0.0;
    for (var c = 0u; c < LIMITER_CELL_COUNT; c++) {
        if wg_darker[c] < FLASH_DARK_LIMIT {
            area += clamp(sign_v * wg_excursion[c] / peak, 0.0, 1.0);
        }
    }
    return area;
}

// Thread 0: age the flash window, then count, admit or suppress this frame's
// transitions.
//
// Only brightenings are ever suppressed, and one is admitted only while the
// window can still hold it and the darkening that follows it. Darkenings are
// always admitted, and signs alternate, so the window never exceeds its six
// transitions and a limited strobe always comes to rest dark. Holding a frame
// brighter than its content would need detail the frame no longer has: with
// per-cell history only, the limiter could fill the cell with a flat color,
// and that fill itself flickers wherever the cell holds finer structure.
fn decide_transitions() {
    var g = limiter_global;
    if limiter.reset != 0u {
        for (var k = 0u; k < FLASH_WINDOW_SLOTS; k++) {
            g.ages[k] = -1.0;
        }
        g.last_sign = 0;
        g.suppressing = 0u;
    }

    // Window aging takes the full elapsed presented time.
    var kept: array<f32, 8>;
    var count = 0u;
    for (var k = 0u; k < FLASH_WINDOW_SLOTS; k++) {
        kept[k] = -1.0;
    }
    for (var k = 0u; k < FLASH_WINDOW_SLOTS; k++) {
        if g.ages[k] >= 0.0 {
            let age = g.ages[k] + limiter.dt_window;
            if age < FLASH_WINDOW_SECONDS {
                kept[count] = age;
                count++;
            }
        }
    }
    g.ages = kept;

    let brightening_fits = count + 2u <= FLASH_MAX_TRANSITIONS;
    if brightening_fits {
        g.suppressing = 0u;
    }

    for (var s = 0; s < 2; s++) {
        let sign_i = select(-1, 1, s == 0);
        if sign_i == g.last_sign {
            continue;
        }
        let sign_v = f32(sign_i);
        let brightening = sign_i > 0;
        if transition_area(sign_v, FLASH_LUMINANCE_THRESHOLD) >= FLASH_AREA_CELLS {
            if !brightening || brightening_fits {
                if count < FLASH_WINDOW_SLOTS {
                    g.ages[count] = 0.0;
                    count++;
                }
                g.last_sign = sign_i;
            } else {
                g.suppressing = 1u;
            }
        } else if brightening && !brightening_fits
            && transition_area(sign_v, SUPPRESSION_DEADBAND) >= FLASH_AREA_CELLS {
            g.suppressing = 1u;
        }
    }
    g.count = count;
    limiter_global = g;
    wg_suppressing = g.suppressing;
}

// One thread per cell; thread 0 also runs the whole-grid decisions between
// barriers.
@compute @workgroup_size(144)
fn cs_limit_cells(@builtin(local_invocation_index) i: u32) {
    let measured_rgb = cell_measure[i].rgb;
    let measured = dot(measured_rgb, LUMA);
    var st = cell_state[i];

    if limiter.init != 0u {
        st.out_rgb = measured_rgb;
        st.out_lum = measured;
        st.ext_lum = measured;
        st.dir = 0.0;
    }

    if limiter.enabled == 0u {
        // Off: pass content unchanged, but keep tracking what was presented so
        // re-enabling compares against the frame presented just before it.
        cell_params[i] = LimiterCellParams(1.0, 0.0, 0.0, 0.0, vec4<f32>(0.0));
        st.out_rgb = measured_rgb;
        st.out_lum = measured;
        st.ext_lum = measured;
        st.dir = 0.0;
        cell_state[i] = st;
        return;
    }

    if limiter.reset != 0u {
        // History starts this frame: no earlier transition counts.
        st.ext_lum = st.out_lum;
        st.dir = 0.0;
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
    workgroupBarrier();
    let cap_mask = workgroupUniformLoad(&wg_cap_sign_mask);
    var target_lum = measured;
    if delta > cap && (cap_mask & 1u) != 0u {
        target_lum = st.out_lum + cap;
    } else if delta < -cap && (cap_mask & 2u) != 0u {
        target_lum = st.out_lum - cap;
    }

    // Stage 2 — flash budget over the rate-capped candidate.
    let candidate = track(st, target_lum);
    wg_excursion[i] = target_lum - candidate.ext;
    wg_darker[i] = min(target_lum, candidate.ext);
    workgroupBarrier();
    if i == 0u {
        decide_transitions();
    }
    let suppressing = workgroupUniformLoad(&wg_suppressing);
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
    cell_params[i] = LimiterCellParams(gain, blend, 0.0, 0.0, vec4<f32>(prev_rgb, 0.0));

    let presented_rgb = mix(measured_rgb * gain, prev_rgb, blend);
    let tracked = track(st, dot(presented_rgb, LUMA));
    st.out_rgb = presented_rgb;
    st.out_lum = dot(presented_rgb, LUMA);
    st.ext_lum = tracked.ext;
    st.dir = tracked.dir;
    cell_state[i] = st;
}
