// Photosensitivity flash limiter: measure + limit compute passes.
// Appended to `screen_effects.wgsl` at pipeline creation, so it shares that
// file's group-0 bindings, `compose_presented`, `LUMA`, the cell grid and
// `LimiterCellParams`. Runs ahead of the resolve every frame; the resolve reads
// `cell_params` back through `presented_cell_params`. While the limiter is off
// only the limit pass runs, and it writes identity parameters.
//
// History lives only here, on the GPU: each cell's last presented mean color,
// its luminance and redness extrema, its luminance floor, and the global
// flash window. Nothing reads back.
// See context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter).

// Mirrors `LimiterFrameUniform` in render-cpu/src/flash_limiter.rs.
struct LimiterFrame {
    dt_window: f32,
    dt_rate: f32,
    enabled: u32,
    // 1 on the frame history starts: a fresh limiter's first enabled frame, or
    // the frame that turns it back on. That frame adopts its own measure as the
    // last presented one and empties the window, so it presents unchanged.
    init: u32,
    // 1 when splash frames presented since the previous resolve frame.
    splash_active: u32,
    // How long ago the splash stretch began; its drop into the splash happened
    // then.
    splash_seconds: f32,
    _pad0: u32,
    _pad1: u32,
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
    // Share of the cell the current luminance / redness excursion covers
    // (`fresh_cover`), kept from the last frame the cell moved.
    lum_cover: f32,
    red_cover: f32,
    // The lowest luminance, presented or measured, since the presented
    // luminance last started falling: the level a rise from here started
    // from. Below the presented trough when the rate cap kept the presented
    // fall from following the content all the way down.
    lum_floor: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

const FLASH_WINDOW_SLOTS: u32 = 8u;

// Transition kinds, as window slots record them. Onsets are even, their
// returns odd; each kind is also its area-weight slot in `wg_weight`.
const TRANSITION_LUM_UP: u32 = 0u;
const TRANSITION_LUM_DOWN: u32 = 1u;
const TRANSITION_RED_UP: u32 = 2u;
const TRANSITION_RED_DOWN: u32 = 3u;

struct LimiterGlobal {
    // Ages (seconds) of the transitions counted in the flash window, oldest
    // event first; < 0 is an empty slot. Ages advance by presented-frame time,
    // so no absolute clock drifts. Luminance and red transitions share it.
    ages: array<f32, 8>,
    count: u32,
    // Two bits per slot: the `TRANSITION_*` kind it holds.
    kinds: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    _pad3: u32,
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
// Red flash: one state saturated red, R/(R+G+B) ≥ 0.8, which is redness ≥ 0.7
// on the scale below; a transition is a same-sign redness change of at least
// 0.2 accumulated from the last extremum.
const RED_SATURATED: f32 = 0.7;
const RED_TRANSITION_THRESHOLD: f32 = 0.2;
// A per-frame change smaller than this is a cell holding still.
const STILL_STEP: f32 = 1e-5;

// Redness from chromaticity alone: 0 for any neutral or non-red color, 1 for
// pure red, whatever the brightness. A color too dark to have a hue reads 0.
fn redness(rgb: vec3<f32>) -> f32 {
    let total = rgb.r + rgb.g + rgb.b;
    if total < 0.003 {
        return 0.0;
    }
    return clamp((rgb.r / total - 1.0 / 3.0) * 1.5, 0.0, 1.0);
}

// How far to mix `rgb` toward its own luminance so its redness falls to
// `held`. Mixing by d moves the red share R/(R+G+B) along
// (r + d(L − r)) / (T + d(3L − T)), which is not linear in d; this solves it
// exactly, so a held cell presents exactly its held redness and no leak can
// ratchet. Luminance is unchanged by the mix.
fn desaturation_to_redness(rgb: vec3<f32>, held: f32) -> f32 {
    let total = rgb.r + rgb.g + rgb.b;
    let lum = dot(rgb, LUMA);
    // The red share whose redness is `held` (inverse of `redness`).
    let share = 1.0 / 3.0 + held / 1.5;
    let excess = rgb.r - share * total;
    if excess <= 0.0 {
        return 0.0;
    }
    // Positive whenever `excess` is: `share` ≥ 1/3 and L ≤ T.
    let denom = (rgb.r - lum) - share * (total - 3.0 * lum);
    return clamp(excess / max(denom, 1e-6), 0.0, 1.0);
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

// Area inputs, one block of 144 per quantity: luminance at `BLOCK_LUM`,
// redness at `BLOCK_RED`. `wg_change` is the signed change being weighed,
// `wg_step` the cell's signed step this frame.
const BLOCK_LUM: u32 = 0u;
const BLOCK_RED: u32 = 144u;
var<workgroup> wg_change: array<f32, 288>;
var<workgroup> wg_step: array<f32, 288>;
// Each cell's share of the area of each transition kind, 144 per kind, then
// its share of the area of luminance onsets the budget has to hold
// (`WEIGHT_LUM_HOLD`).
const WEIGHT_LUM_HOLD: u32 = 4u;
var<workgroup> wg_weight: array<f32, 720>;
var<workgroup> wg_cap_sign_mask: u32;
// Bit `1 << kind` for each transition counted this frame.
var<workgroup> wg_event_mask: u32;
// Bit `1 << kind` for each onset held back over budget this frame.
var<workgroup> wg_held_mask: u32;
// 1 when no luminance onset is admitted this frame, none would fit, and the
// cells that would flash from their floor reach the flash area.
var<workgroup> wg_lum_hold: u32;

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

// A cell's floor after a frame that presented `presented` over content
// `measured`, tracked as `t`: restarted when the presented luminance starts
// falling, lowered while it falls or holds, kept while it rises.
fn next_floor(floor: f32, t: Track, presented: f32, measured: f32) -> f32 {
    let low = min(presented, measured);
    if t.reversed && t.dir < 0.0 {
        return low;
    }
    if t.dir <= 0.0 {
        return min(floor, low);
    }
    return floor;
}

// Neighbour (x, y)'s change and step in `base`'s block, oriented by `sign_v`.
// Off the grid reads as unchanged.
fn neighbour(base: u32, x: i32, y: i32, sign_v: f32) -> vec2<f32> {
    if x < 0 || y < 0 || x >= i32(LIMITER_CELLS_X) || y >= i32(LIMITER_CELLS_Y) {
        return vec2<f32>(0.0);
    }
    let n = base + u32(y) * LIMITER_CELLS_X + u32(x);
    return vec2<f32>(sign_v * wg_change[n], sign_v * wg_step[n]);
}

// The change level a cell of change `e` sees along axis (dx, dy) when it sits
// on a ramp there: one side changed more and is moving with it this frame,
// the other changed less. 0 when the cell is not on a ramp along that axis.
fn ramp_level(base: u32, x: i32, y: i32, dx: i32, dy: i32, sign_v: f32, e: f32) -> f32 {
    let a = neighbour(base, x + dx, y + dy, sign_v);
    let b = neighbour(base, x - dx, y - dy, sign_v);
    var level = 0.0;
    if a.x > e && a.y > STILL_STEP && b.x < e {
        level = a.x;
    }
    if b.x > e && b.y > STILL_STEP && a.x < e {
        level = max(level, b.x);
    }
    return level;
}

// Share of cell `i` a change of `sign_v` and size `e` (> 0) covers. A cell the
// change only partly covers sits on a ramp between a cell it covers and one it
// does not, and its mean holds the change diluted by coverage; its coverage is
// its change over the ramp's larger side. A cell on no ramp is covered. So a
// uniform change weighs by the area it covers, not the cells it touches, and a
// dimmer change beside a brighter one is not shrunk by it unless the two meet
// on a one-cell-thin edge (there the cell means cannot tell a dim covered cell
// from a bright partly covered one). Only neighbours moving this frame set the
// level, so a still cell holding an old excursion shrinks nothing.
fn fresh_cover(base: u32, i: u32, sign_v: f32, e: f32) -> f32 {
    let x = i32(i % LIMITER_CELLS_X);
    let y = i32(i / LIMITER_CELLS_X);
    var level = e;
    level = max(level, ramp_level(base, x, y, 1, 0, sign_v, e));
    level = max(level, ramp_level(base, x, y, 0, 1, sign_v, e));
    level = max(level, ramp_level(base, x, y, 1, 1, sign_v, e));
    level = max(level, ramp_level(base, x, y, 1, -1, sign_v, e));
    return e / level;
}

// Cell `i`'s cover for its own `change` in `base`'s block: estimated afresh on
// a frame its excursion moves or starts, otherwise the one kept from the frame
// it last moved, so a partly covered cell weighs the same while it holds.
fn cover_of(base: u32, i: u32, change: f32, fresh: bool, kept: f32) -> f32 {
    if !fresh || change == 0.0 {
        return kept;
    }
    return fresh_cover(base, i, sign(change), abs(change));
}

// Publish cell `i`'s share of the area of each sign's change in `base`'s block:
// its cover, when it `counts` and the change it belongs to (its own change
// over its cover) reaches `level`.
fn publish_weight(base: u32, i: u32, change: f32, cover: f32, counts: bool, level: f32) {
    let kind = (base / LIMITER_CELL_COUNT) * 2u;
    let w = select(0.0, cover, counts && abs(change) >= level * cover);
    wg_weight[kind * LIMITER_CELL_COUNT + i] = select(0.0, w, change > 0.0);
    wg_weight[(kind + 1u) * LIMITER_CELL_COUNT + i] = select(0.0, w, change < 0.0);
}

// Thread 0: area, in cells, of the change of transition `kind` published this
// phase.
fn area(kind: u32) -> f32 {
    var total = 0.0;
    for (var c = 0u; c < LIMITER_CELL_COUNT; c++) {
        total += wg_weight[kind * LIMITER_CELL_COUNT + c];
    }
    return total;
}

fn push_transition(g: ptr<function, LimiterGlobal>, kind: u32, age: f32) {
    let n = (*g).count;
    if n < FLASH_WINDOW_SLOTS && age < FLASH_WINDOW_SECONDS {
        (*g).ages[n] = age;
        (*g).kinds = ((*g).kinds & ~(3u << (n * 2u))) | (kind << (n * 2u));
        (*g).count = n + 1u;
    }
}

// Returns the window still owes: onsets in it whose return it does not hold
// yet, per quantity. A return with no onset before it owes nothing.
fn owed_returns(g: LimiterGlobal) -> u32 {
    var lum = 0u;
    var red = 0u;
    for (var k = 0u; k < g.count; k++) {
        let kind = (g.kinds >> (k * 2u)) & 3u;
        let onset = (kind & 1u) == 0u;
        if kind < TRANSITION_RED_UP {
            if onset {
                lum += 1u;
            } else if lum > 0u {
                lum -= 1u;
            }
        } else {
            if onset {
                red += 1u;
            } else if red > 0u {
                red -= 1u;
            }
        }
    }
    return lum + red;
}

// An onset is admitted only while the window holds it, the returns earlier
// onsets still owe, and its own return.
fn onset_fits(g: LimiterGlobal) -> bool {
    return g.count + owed_returns(g) + 2u <= FLASH_MAX_TRANSITIONS;
}

// Thread 0: age the flash window by the full elapsed presented time, emptying
// it on the frame history starts.
fn age_window(g: ptr<function, LimiterGlobal>) {
    var kept: array<f32, 8>;
    var kinds = 0u;
    var count = 0u;
    if limiter.init == 0u {
        let n = min((*g).count, FLASH_WINDOW_SLOTS);
        for (var k = 0u; k < n; k++) {
            let age = (*g).ages[k] + limiter.dt_window;
            if (*g).ages[k] >= 0.0 && age < FLASH_WINDOW_SECONDS {
                kept[count] = age;
                kinds |= (((*g).kinds >> (k * 2u)) & 3u) << (count * 2u);
                count++;
            }
        }
    }
    for (var k = count; k < FLASH_WINDOW_SLOTS; k++) {
        kept[k] = -1.0;
    }
    (*g).ages = kept;
    (*g).kinds = kinds;
    (*g).count = count;
}

// Thread 0, after a splash stretch: the drop from the last resolve frame into
// the splash happened when the stretch began. The splash path wrote it
// straight to the swapchain, so it cannot be suppressed; it is recorded at its
// true age so both edges of a load count against the budget.
fn record_splash_edge() {
    var g = limiter_global;
    age_window(&g);
    var events = 0u;
    // A fresh limiter has no earlier resolve frame to drop from: the splash
    // simply becomes the level its first frame is limited against.
    if limiter.init == 0u {
        if area(TRANSITION_LUM_DOWN) >= FLASH_AREA_CELLS {
            push_transition(&g, TRANSITION_LUM_DOWN, limiter.splash_seconds);
            events |= 1u << TRANSITION_LUM_DOWN;
        }
        if area(TRANSITION_LUM_UP) >= FLASH_AREA_CELLS {
            push_transition(&g, TRANSITION_LUM_UP, limiter.splash_seconds);
            events |= 1u << TRANSITION_LUM_UP;
        }
    }
    limiter_global = g;
    wg_event_mask = events;
}

// Thread 0: count, admit or hold this frame's transitions. `aged` is true when
// the splash hand-off already aged the window this frame.
//
// Returns — darkenings, and moves away from saturated red — are always
// admitted, and are counted first. Onsets — brightenings, and moves toward
// saturated red — are checked one at a time, each only admitted while the
// window holds it, its own return, and the returns earlier onsets still owe,
// so a luminance and a red onset in one frame cannot share the last room. A
// held onset is suppressed per cell in `cs_limit_cells`. A return with no
// onset before it in the window, such as a bright scene going dark, is still
// counted when admitted, so it alone can carry the window past six.
//
// Holding a frame brighter than its content would need detail the frame no
// longer has: with per-cell history only, the limiter could fill the cell with
// a flat color, and that fill itself flickers wherever the cell holds finer
// structure. So only onsets are ever held, and a limited strobe comes to rest
// dark and unsaturated. Red is limited by desaturation, which keeps luminance
// exactly.
fn decide_transitions(aged: bool) {
    var g = limiter_global;
    if !aged {
        age_window(&g);
    }
    var events = 0u;
    var held = 0u;
    admit_transition(&g, TRANSITION_LUM_DOWN, &events, &held);
    admit_transition(&g, TRANSITION_RED_DOWN, &events, &held);
    admit_transition(&g, TRANSITION_LUM_UP, &events, &held);
    admit_transition(&g, TRANSITION_RED_UP, &events, &held);
    var lum_hold = 0u;
    if (events & (1u << TRANSITION_LUM_UP)) == 0u && !onset_fits(g)
        && area(WEIGHT_LUM_HOLD) >= FLASH_AREA_CELLS {
        lum_hold = 1u;
    }
    limiter_global = g;
    wg_event_mask = events;
    wg_held_mask = held;
    wg_lum_hold = lum_hold;
}

// Thread 0: count this frame's transition of `kind` when its area reaches the
// threshold — a return always, an onset only while it fits — or mark the onset
// held.
fn admit_transition(
    g: ptr<function, LimiterGlobal>,
    kind: u32,
    events: ptr<function, u32>,
    held: ptr<function, u32>,
) {
    if area(kind) < FLASH_AREA_CELLS {
        return;
    }
    if (kind & 1u) != 0u || onset_fits(*g) {
        push_transition(g, kind, 0.0);
        *events |= 1u << kind;
    } else {
        *held |= 1u << kind;
    }
}

// One thread per cell; thread 0 also runs the whole-grid decisions between
// barriers.
@compute @workgroup_size(144)
fn cs_limit_cells(@builtin(local_invocation_index) i: u32) {
    if limiter.enabled == 0u {
        // Off: identity, so the resolve presents content unchanged. The measure
        // pass did not run and no history is kept; turning the limiter back on
        // starts fresh (`init`).
        cell_params[i] = LimiterCellParams(1.0, 0.0, 0.0, 0.0, vec4<f32>(0.0));
        return;
    }

    let measured_rgb = cell_measure[i].rgb;
    let measured = dot(measured_rgb, LUMA);
    let measured_red = redness(measured_rgb);
    var st = cell_state[i];

    if limiter.init != 0u {
        // History starts this frame: it is the level the next frame is limited
        // against, and no earlier transition counts.
        st.out_rgb = measured_rgb;
        st.out_lum = measured;
        st.ext_lum = measured;
        st.dir = 0.0;
        st.red_ext = measured_red;
        st.red_dir = 0.0;
        st.counted = 0.0;
        st.red_counted = 0.0;
        st.lum_cover = 1.0;
        st.red_cover = 1.0;
        st.lum_floor = measured;
    }

    // Splash hand-off: the player last saw the splash, not this cell's last
    // resolve frame. Record the drop into it, then limit this frame against it.
    var aged = false;
    if limiter.splash_active != 0u {
        let splash_rgb = limiter.splash_rgb.rgb;
        let splash_lum = dot(splash_rgb, LUMA);
        let drop = track(st.out_lum, st.ext_lum, st.dir, splash_lum);
        let drop_change = splash_lum - drop.ext;
        let drop_step = splash_lum - st.out_lum;
        let drop_prior = select(st.counted, 0.0, drop.reversed) > 0.5;
        wg_change[BLOCK_LUM + i] = drop_change;
        wg_step[BLOCK_LUM + i] = drop_step;
        workgroupBarrier();
        let drop_fresh = drop.reversed || abs(drop_step) > STILL_STEP;
        let drop_cover = cover_of(BLOCK_LUM, i, drop_change, drop_fresh, st.lum_cover);
        let drop_counts = min(splash_lum, drop.ext) < FLASH_DARK_LIMIT && !drop_prior;
        publish_weight(BLOCK_LUM, i, drop_change, drop_cover, drop_counts, FLASH_LUMINANCE_THRESHOLD);
        workgroupBarrier();
        if i == 0u {
            record_splash_edge();
        }
        let splash_events = workgroupUniformLoad(&wg_event_mask);
        // Like a counted frame, a counted drop marks every cell moving its way.
        // A drop that did not count — such as a small brightening into the
        // splash after a fade to black — leaves the excursion open, so the
        // first change after the load that continues it still counts.
        let drop_marked = (drop_change > 0.0 && (splash_events & (1u << TRANSITION_LUM_UP)) != 0u)
            || (drop_change < 0.0 && (splash_events & (1u << TRANSITION_LUM_DOWN)) != 0u);
        st.counted = select(0.0, 1.0, drop_prior || drop_marked);
        if drop_fresh {
            st.lum_cover = drop_cover;
        }
        st.lum_floor = next_floor(st.lum_floor, drop, splash_lum, splash_lum);
        st.out_rgb = splash_rgb;
        st.out_lum = splash_lum;
        st.ext_lum = drop.ext;
        st.dir = drop.dir;
        st.red_ext = redness(splash_rgb);
        st.red_dir = 0.0;
        st.red_counted = 0.0;
        st.red_cover = 1.0;
        aged = true;
    }

    // Stage 1 — intensity rate cap, applied only when the change faster than
    // the cap covers the flash-area threshold.
    let cap = INTENSITY_RATE_PER_SECOND * limiter.dt_rate;
    let delta = measured - st.out_lum;
    wg_change[BLOCK_LUM + i] = delta;
    wg_step[BLOCK_LUM + i] = delta;
    workgroupBarrier();
    let rate_cover = cover_of(BLOCK_LUM, i, delta, true, 1.0);
    publish_weight(BLOCK_LUM, i, delta, rate_cover, abs(delta) > cap * rate_cover, 0.0);
    workgroupBarrier();
    if i == 0u {
        var mask = 0u;
        if area(TRANSITION_LUM_UP) >= FLASH_AREA_CELLS {
            mask |= 1u;
        }
        if area(TRANSITION_LUM_DOWN) >= FLASH_AREA_CELLS {
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
    let lum_change = target_lum - candidate.ext;
    let red_change = measured_red - red_candidate.ext;
    let lum_step = target_lum - st.out_lum;
    let red_step = measured_red - out_red;
    // A reversal starts a new excursion that has not been counted yet.
    let lum_counted = select(st.counted, 0.0, candidate.reversed) > 0.5;
    let red_counted = select(st.red_counted, 0.0, red_candidate.reversed) > 0.5;
    // An uncounted excursion counts toward a luminance flash while its darker
    // state is below the WCAG limit, toward a red flash while one of its states
    // is saturated red.
    let lum_counts = min(target_lum, candidate.ext) < FLASH_DARK_LIMIT && !lum_counted;
    let red_counts = max(measured_red, red_candidate.ext) >= RED_SATURATED && !red_counted;
    wg_change[BLOCK_LUM + i] = lum_change;
    wg_step[BLOCK_LUM + i] = lum_step;
    wg_change[BLOCK_RED + i] = red_change;
    wg_step[BLOCK_RED + i] = red_step;
    workgroupBarrier();
    let lum_fresh = candidate.reversed || abs(lum_step) > STILL_STEP;
    let red_fresh = red_candidate.reversed || abs(red_step) > STILL_STEP;
    let lum_cover = cover_of(BLOCK_LUM, i, lum_change, lum_fresh, st.lum_cover);
    let red_cover = cover_of(BLOCK_RED, i, red_change, red_fresh, st.red_cover);
    publish_weight(BLOCK_LUM, i, lum_change, lum_cover, lum_counts, FLASH_LUMINANCE_THRESHOLD);
    publish_weight(BLOCK_RED, i, red_change, red_cover, red_counts, RED_TRANSITION_THRESHOLD);
    // Over budget, a luminance onset is weighed on the content from the
    // cell's floor, not on the rate-capped candidate from the presented
    // trough. A capped step starts below the threshold, so the candidate
    // would pass it and hold only the step that crosses; and a cell whose
    // capped fall stopped above the content's trough would rise from there,
    // so its next fall would start a new, uncounted return. Weighed from the
    // floor, a strobing cell is held from its first step, and no cell rises
    // from a stranded level into a return the window never admitted.
    let lum_origin = min(candidate.ext, st.lum_floor);
    let hold_change = measured - lum_origin;
    let hold_counts = target_lum > st.out_lum && !lum_counted && lum_origin < FLASH_DARK_LIMIT;
    wg_weight[WEIGHT_LUM_HOLD * LIMITER_CELL_COUNT + i] = select(
        0.0,
        lum_cover,
        hold_counts && hold_change >= FLASH_LUMINANCE_THRESHOLD * lum_cover,
    );
    workgroupBarrier();
    if i == 0u {
        decide_transitions(aged);
    }
    let events = workgroupUniformLoad(&wg_event_mask);
    let held = workgroupUniformLoad(&wg_held_mask);
    let lum_hold = workgroupUniformLoad(&wg_lum_hold);

    // Over budget, only the cells that are themselves flashing are held:
    // those whose own excursion from their floor reaches the threshold. A
    // smaller brightening, such as a camera pan's drift, passes, and a strobe
    // that moves to new cells is held there too, since those cells cross the
    // threshold. Every cell a held candidate onset would flash is among them.
    let lum_held = lum_hold != 0u && hold_counts && hold_change >= FLASH_LUMINANCE_THRESHOLD;
    let final_lum = select(target_lum, st.out_lum, lum_held);

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

    // Red: a held red onset keeps each flashing cell's redness where it was by
    // desaturating toward each pixel's own luminance, which leaves luminance
    // untouched.
    var desaturate = 0.0;
    let shown_red = redness(luminance_rgb);
    let red_flashing = red_counts && red_change >= RED_TRANSITION_THRESHOLD;
    let red_held = (held & (1u << TRANSITION_RED_UP)) != 0u && red_flashing
        && shown_red > out_red;
    if red_held {
        desaturate = desaturation_to_redness(luminance_rgb, out_red);
    }
    cell_params[i] = LimiterCellParams(gain, blend, desaturate, 0.0, vec4<f32>(prev_rgb, 0.0));

    // A held cell presents exactly what it last presented, and is recorded
    // so. Recomputed from the scaled or desaturated color it would differ by
    // rounding, and a rounding step against its direction of travel would
    // read as a reversal: its excursion would restart at the held level, and
    // the rate-capped onset behind it would pass a sub-threshold step at a
    // time, a whole flash in a few frames.
    let presented_rgb = mix(luminance_rgb, vec3<f32>(dot(luminance_rgb, LUMA)), desaturate);
    let presented_lum = select(dot(presented_rgb, LUMA), st.out_lum, lum_held);
    let presented_red = select(redness(presented_rgb), out_red, red_held);
    let tracked = track(st.out_lum, st.ext_lum, st.dir, presented_lum);
    let red_tracked = track(out_red, st.red_ext, st.red_dir, presented_red);
    // A counted event marks every cell moving its way, so the rest of that
    // excursion never counts again; a presented reversal starts a new one.
    let lum_exc = presented_lum - tracked.ext;
    let red_exc = presented_red - red_tracked.ext;
    let lum_marked = (lum_exc > 0.0 && (events & (1u << TRANSITION_LUM_UP)) != 0u)
        || (lum_exc < 0.0 && (events & (1u << TRANSITION_LUM_DOWN)) != 0u);
    let red_marked = (red_exc > 0.0 && (events & (1u << TRANSITION_RED_UP)) != 0u)
        || (red_exc < 0.0 && (events & (1u << TRANSITION_RED_DOWN)) != 0u);
    let lum_prior = select(st.counted, 0.0, tracked.reversed) > 0.5;
    let red_prior = select(st.red_counted, 0.0, red_tracked.reversed) > 0.5;
    st.counted = select(0.0, 1.0, lum_prior || lum_marked);
    st.red_counted = select(0.0, 1.0, red_prior || red_marked);
    // A held cell did not move: it keeps the cover of the excursion it holds.
    if tracked.reversed || abs(presented_lum - st.out_lum) > STILL_STEP {
        st.lum_cover = lum_cover;
    }
    if red_tracked.reversed || abs(presented_red - out_red) > STILL_STEP {
        st.red_cover = red_cover;
    }
    st.lum_floor = next_floor(st.lum_floor, tracked, presented_lum, measured);
    st.out_rgb = presented_rgb;
    st.out_lum = presented_lum;
    st.ext_lum = tracked.ext;
    st.dir = tracked.dir;
    st.red_ext = red_tracked.ext;
    st.red_dir = red_tracked.dir;
    cell_state[i] = st;
}
