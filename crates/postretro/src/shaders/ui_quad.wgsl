// UI quad / 9-slice shader: one instance per panel/image, vertex-expanded into
// nine regions. Corners stay fixed-size, edges/center stretch; a zero-margin
// instance degenerates to a plain quad. Panels and images only — text is
// glyphon's own pipeline. Writes linear color into the sRGB surface (the surface
// format encodes); a 1x1 white texel makes untextured panels and textured
// images share one batch.
// See: context/lib/rendering_pipeline.md

// Device-pixel viewport. Drives the device-rect -> clip-space map; rebuilt per
// frame from `surface_config`.
struct UiUbo {
    viewport_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> ubo: UiUbo;
@group(0) @binding(1) var ui_tex: texture_2d<f32>;
@group(0) @binding(2) var ui_sampler: sampler;

// One instance = one panel/image. All rects in device pixels except color/uv.
struct Instance {
    // rect.xy = top-left device px, rect.zw = size device px.
    @location(0) rect: vec4<f32>,
    // uv.xy = top-left UV, uv.zw = UV size. (0,0,1,1) samples the whole texture;
    // the 1x1 white texel makes any UV slice a solid fill.
    @location(1) uv_rect: vec4<f32>,
    @location(2) color: vec4<f32>,
    // 9-slice margins in device px: left, top, right, bottom. All-zero = plain quad.
    @location(3) margin: vec4<f32>,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

// Each of the 9 regions is two triangles (6 vertices), so 54 vertices per
// instance. vertex_index decomposes into region (0..8) and corner (0..5).
const VERTS_PER_REGION: u32 = 6u;

// Maps a region column/row index (0,1,2) plus a [0,1] corner coordinate to a
// device-pixel position and matching UV along one axis. `size` is the rect
// extent, `m0`/`m1` are the near/far margins, `uv0`/`uv_extent` describe the UV
// slice on that axis. Corners (cols/rows 0 and 2) are fixed-size; the middle
// (col/row 1) stretches to fill the remaining span.
fn axis(
    region: u32,
    corner: f32,
    size: f32,
    m0: f32,
    m1: f32,
    uv0: f32,
    uv_extent: f32,
) -> vec2<f32> {
    // Geometry split points along the axis: 0, m0, size - m1, size.
    let p0 = 0.0;
    let p1 = m0;
    let p2 = max(size - m1, m0); // never cross when margins exceed the rect
    let p3 = size;

    // UV split points mirror the geometry margins, expressed as a fraction of
    // the rect so a sub-rect of the texture slices identically. Guard size == 0.
    let inv = select(0.0, 1.0 / size, size > 0.0);
    let u0 = uv0;
    let u1 = uv0 + uv_extent * (m0 * inv);
    let u2 = uv0 + uv_extent * (max(size - m1, m0) * inv);
    let u3 = uv0 + uv_extent;

    var start_pos: f32;
    var end_pos: f32;
    var start_uv: f32;
    var end_uv: f32;
    if (region == 0u) {
        start_pos = p0; end_pos = p1; start_uv = u0; end_uv = u1;
    } else if (region == 1u) {
        start_pos = p1; end_pos = p2; start_uv = u1; end_uv = u2;
    } else {
        start_pos = p2; end_pos = p3; start_uv = u2; end_uv = u3;
    }

    let pos = mix(start_pos, end_pos, corner);
    let uv = mix(start_uv, end_uv, corner);
    return vec2<f32>(pos, uv);
}

@vertex
fn vs_main(@builtin(vertex_index) vid: u32, inst: Instance) -> VsOut {
    let region = vid / VERTS_PER_REGION;
    let local = vid % VERTS_PER_REGION;
    let col = region % 3u;
    let row = region / 3u;

    // Two triangles per region: (0,0)(1,0)(0,1) and (1,0)(1,1)(0,1).
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let c = corners[local];

    let x = axis(col, c.x, inst.rect.z, inst.margin.x, inst.margin.z, inst.uv_rect.x, inst.uv_rect.z);
    let y = axis(row, c.y, inst.rect.w, inst.margin.y, inst.margin.w, inst.uv_rect.y, inst.uv_rect.w);

    // Device-pixel position: rect top-left + region offset.
    let device_pos = vec2<f32>(inst.rect.x + x.x, inst.rect.y + y.x);

    // Device px -> NDC. y flips (device y down, clip y up).
    let ndc = vec2<f32>(
        device_pos.x / ubo.viewport_size.x * 2.0 - 1.0,
        1.0 - device_pos.y / ubo.viewport_size.y * 2.0,
    );

    var out: VsOut;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = vec2<f32>(x.y, y.y);
    out.color = inst.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Linear write; surface format does the sRGB encode. White-texel panels
    // multiply by their tint; textured images modulate by color (usually white).
    let tex = textureSample(ui_tex, ui_sampler, in.uv);
    return tex * in.color;
}
