// Ghost look helpers shared by every ghost shader: unlit shade, presented opacity and
// the screen-door threshold, all read from the packed look lanes.
// See: context/lib/rendering_pipeline.md §7.9, §8
//
// Binding-agnostic: appended after a ghost shader's own source. The look is
// `postretro_render_cpu::ghost_instance::GhostInstanceLook::pack`:
// x = pack2x16unorm(opacity, saturation), y = pack4x8unorm(tint.rgb, 0).

// Rec. 709 luma, the photosensitivity limiter's luminance weights.
const GHOST_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// Unlit: mix the base color toward its luma by `1 - saturation`, then
// multiply by the tint. A base and tint in [0, 1] stay in [0, 1].
// CPU twin: `ghost_instance::ghost_shade`.
fn ghost_shade(base: vec3<f32>, look: vec2<u32>) -> vec3<f32> {
    let saturation = unpack2x16unorm(look.x).y;
    let tint = unpack4x8unorm(look.y).rgb;
    let luma = dot(base, GHOST_LUMA);
    return mix(vec3<f32>(luma), base, saturation) * tint;
}

fn ghost_opacity(look: vec2<u32>) -> f32 {
    return unpack2x16unorm(look.x).x;
}

// 4×4 ordered (Bayer) threshold of the scene-color pixel, centered in its
// sixteenth. CPU twin: `ghost_instance::screen_door_threshold`.
fn screen_door_threshold(pixel: vec2<u32>) -> f32 {
    let a = (pixel.x ^ pixel.y) & 3u;
    let b = pixel.y & 3u;
    let index = ((a & 1u) << 3u) | ((b & 1u) << 2u) | ((a >> 1u) << 1u) | (b >> 1u);
    return (f32(index) + 0.5) / 16.0;
}
