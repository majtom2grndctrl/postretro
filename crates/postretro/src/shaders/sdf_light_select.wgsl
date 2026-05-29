// Shared K-selection helper for SDF per-light shadows (binding-agnostic).
// See: context/plans/in-progress/sdf-per-light-shadows/index.md (Rough sketch:
// "Light-selection parity") and architecture.md (the K-selection parity seam).
//
// LOAD-BEARING PARITY SEAM. The half-res SDF visibility pass and the forward
// shader MUST select the same `sdf`-tagged lights in the same order for a given
// world position, or each light's visibility slice won't line up with its
// diffuse term. This file is the single source of that selection: it is
// textually concatenated into BOTH consumer shaders at pipeline creation (the
// shared-WGSL-helper pattern — cf. `curve_eval.wgsl`). Never copy-paste it;
// two copies would drift silently and defeat the parity claim.
//
// Binding-agnostic: the consumer shader declares — by these exact lexical
// names, before this file is concatenated — the chunk-grid + spec-light
// buffers it already binds for the static light loop:
//
//     struct SpecLight {
//         position_and_range: vec4<f32>, // xyz = position, w = falloff_range
//         color_and_pad:      vec4<f32>, // xyz = color × intensity, w = sdf flag
//     };
//     struct ChunkGridInfo {
//         grid_origin: vec3<f32>,
//         cell_size: f32,
//         dims: vec3<u32>,
//         has_chunk_grid: u32,
//     };
//     var<storage, read> spec_lights:   array<SpecLight>;
//     var<uniform>       chunk_grid:    ChunkGridInfo;
//     var<storage, read> chunk_offsets: array<vec2<u32>>;
//     var<storage, read> chunk_indices: array<u32>;
//
// This helper declares NONE of those — each consumer binds its own at its own
// (group, binding). It reads them by name only.

// Per-fragment SDF shadow budget: at most this many `sdf`-tagged lights are
// traced/shaded per fragment. Seed K = 3 (the animated shadow factor keeps one
// of the 4 RGBA channels of the half-res target, leaving three for per-light
// slices). Beyond K overlapping sdf lights, extras drop (treated lit).
const SDF_SELECT_K: u32 = 3u;

// Sentinel for an unused selection slot. spec_lights is indexed by u32, so the
// all-ones value cannot collide with a real light index.
const SDF_SELECT_NONE: u32 = 0xffffffffu;

// Result of the K-selection: up to SDF_SELECT_K spec_lights indices, ordered by
// the pinned total order (influence descending, light index ascending). Unused
// slots hold SDF_SELECT_NONE. `count` is the number of valid leading slots.
struct SdfLightSelection {
    indices: array<u32, 3>, // SDF_SELECT_K
    count: u32,
};

// Decode the `sdf`-tagged flag from a SpecLight (color_and_pad.w; 1.0 ⇒ sdf).
fn sdf_select_is_sdf(sl: SpecLight) -> bool {
    return sl.color_and_pad.w > 0.5;
}

// Per-light influence at a world position. Reuses the engine's existing
// per-fragment light weighting from the forward static loop: the falloff-range
// attenuation `max(1 - dist/range, 0)` (`range == 0` ⇒ unattenuated) times the
// light's peak emitted intensity (the brightest channel of color × intensity).
// This is the ordering key, NOT a new metric — it is the same attenuation and
// intensity the shading loop already applies.
fn sdf_select_influence(sl: SpecLight, world: vec3<f32>) -> f32 {
    let to_light = sl.position_and_range.xyz - world;
    let dist = length(to_light);
    let range = sl.position_and_range.w;
    if (range > 0.0 && dist > range) {
        return 0.0;
    }
    let atten = select(1.0, max(1.0 - dist / max(range, 0.001), 0.0), range > 0.0);
    let peak = max(sl.color_and_pad.x, max(sl.color_and_pad.y, sl.color_and_pad.z));
    return atten * peak;
}

// Resolve the chunk-grid candidate window for a world position. Mirrors the
// forward static loop's lookup: when the offline chunk index is present, return
// the (offset, count) into chunk_indices for the containing cell; otherwise the
// full spec buffer. `uses_chunk_list` tells the caller whether to indirect
// through chunk_indices or treat the window as direct spec_lights indices.
struct SdfChunkWindow {
    offset: u32,
    count: u32,
    uses_chunk_list: bool,
};

fn sdf_select_chunk_window(world: vec3<f32>) -> SdfChunkWindow {
    var w: SdfChunkWindow;
    w.offset = 0u;
    w.count = arrayLength(&spec_lights);
    w.uses_chunk_list = false;
    if (chunk_grid.has_chunk_grid != 0u) {
        w.uses_chunk_list = true;
        let local = world - chunk_grid.grid_origin;
        let cell = vec3<i32>(floor(local / chunk_grid.cell_size));
        let dims = vec3<i32>(chunk_grid.dims);
        if (all(cell >= vec3<i32>(0)) && all(cell < dims)) {
            let ci = u32(cell.z) * chunk_grid.dims.x * chunk_grid.dims.y
                   + u32(cell.y) * chunk_grid.dims.x
                   + u32(cell.x);
            let pair = chunk_offsets[ci];
            w.offset = pair.x;
            w.count = pair.y;
        } else {
            // Outside the authored grid: no static lights by construction.
            w.count = 0u;
        }
    }
    return w;
}

// Select up to SDF_SELECT_K `sdf`-tagged lights influencing `world`, ordered by
// the pinned total order: influence DESCENDING, tie-break light index
// ASCENDING. Returns the chosen spec_lights indices.
//
// Selection-sort over the chunk-grid candidate window keeps a running top-K: it
// scans every candidate and inserts each `sdf` light into the sorted slot list
// if it outranks the current Kth. This is the deterministic comparator the
// host-side Rust reference comparator mirrors exactly.
fn select_sdf_lights(world: vec3<f32>) -> SdfLightSelection {
    var sel: SdfLightSelection;
    sel.indices = array<u32, 3>(SDF_SELECT_NONE, SDF_SELECT_NONE, SDF_SELECT_NONE);
    sel.count = 0u;

    // Parallel influence array for the kept slots (indices[i] ↔ infl[i]).
    var infl = array<f32, 3>(0.0, 0.0, 0.0);

    let win = sdf_select_chunk_window(world);
    for (var j: u32 = 0u; j < win.count; j = j + 1u) {
        var light_idx: u32 = win.offset + j;
        if (win.uses_chunk_list) {
            light_idx = chunk_indices[win.offset + j];
        }
        let sl = spec_lights[light_idx];
        if (!sdf_select_is_sdf(sl)) {
            continue;
        }
        let influence = sdf_select_influence(sl, world);
        if (influence <= 0.0) {
            continue;
        }

        // Find the insertion slot under the pinned total order. A candidate
        // outranks a kept slot when it has strictly greater influence, or equal
        // influence and a smaller light index.
        var insert_at: u32 = SDF_SELECT_K;
        for (var s: u32 = 0u; s < SDF_SELECT_K; s = s + 1u) {
            let occupied = s < sel.count;
            if (!occupied) {
                insert_at = s;
                break;
            }
            let outranks = influence > infl[s]
                || (influence == infl[s] && light_idx < sel.indices[s]);
            if (outranks) {
                insert_at = s;
                break;
            }
        }
        if (insert_at >= SDF_SELECT_K) {
            continue; // Ranks below the current Kth — drop.
        }

        // Shift lower-ranked kept slots down by one (drop the last if full),
        // then place the candidate at insert_at.
        let new_count = min(sel.count + 1u, SDF_SELECT_K);
        var s: u32 = SDF_SELECT_K - 1u;
        loop {
            if (s <= insert_at) {
                break;
            }
            sel.indices[s] = sel.indices[s - 1u];
            infl[s] = infl[s - 1u];
            s = s - 1u;
        }
        sel.indices[insert_at] = light_idx;
        infl[insert_at] = influence;
        sel.count = new_count;
    }

    return sel;
}
