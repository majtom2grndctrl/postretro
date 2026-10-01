// Screen-space resolve: the gameplay path's sole swapchain writer, run every
// frame and never skipped at rest.
//
// Upscales the scene by nearest integer replication, tonemaps it, applies the
// screen effects (flash / vignette / shake), then composites the native-res UI
// layer over it. The UI layer is not tonemapped. Each effect reaches the UI
// layer only when its covers-HUD switch is on; all are off by default.
//
// `scene_color` is linear Rgba16Float at the scene extent. The UI layer is a
// premultiplied sRGB target at the surface extent (a 1×1 transparent texel for
// capture), so loading it decodes to linear. The output is an sRGB target,
// which performs the sole store conversion.
// See context/lib/rendering_pipeline.md §7.8.

@group(0) @binding(0) var scene_color_tex: texture_2d<f32>;

// Mirrors `EffectUniform` in render-cpu/src/screen_effects.rs.
//   flash         — rgba; `flash.a` is the over-blend weight (0 at rest → no-op).
//   vignette      — `xyz` linear tint + `w` strength (0 at rest → no edge tint).
//   shake         — screen-fraction offset; (0,0) at rest.
//   covers_hud    — flash, vignette, shake: nonzero = also reaches the UI layer.
//   scene_divisor — surface pixels per scene pixel on each axis.
struct EffectUniform {
    flash: vec4<f32>,
    vignette: vec4<f32>,
    shake: vec2<f32>,
    _pad: vec2<f32>,
    covers_hud: vec3<u32>,
    scene_divisor: u32,
}
@group(0) @binding(1) var<uniform> effect: EffectUniform;
@group(0) @binding(2) var ui_layer_tex: texture_2d<f32>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // Fullscreen triangle — three verts, covers the full clip-space quad.
    //   vid=0 → (-1,-1,0) / uv (0,1)
    //   vid=1 → ( 3,-1,0) / uv (2,1)
    //   vid=2 → (-1, 3,0) / uv (0,-1)
    var out: VsOut;
    let x = f32((vid << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(vid & 2u) * 2.0 - 1.0;
    out.clip = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

// Preserve the palette below a very late knee, then compress HDR overshoot by
// the scene's peak channel so hue remains stable. The 0.98 knee is within one
// percent of unity at 1.0, but avoids a hard clip and asymptotically approaches
// the display ceiling for bright emissive/bloom content.
fn soft_knee_tonemap(color: vec3<f32>) -> vec3<f32> {
    let peak = max(max(color.r, color.g), color.b);
    let knee_start = 0.98;
    if peak <= knee_start {
        return color;
    }
    let knee_width = 1.0 - knee_start;
    let excess = peak - knee_start;
    let compressed_peak = knee_start + knee_width * excess / (excess + knee_width);
    return color * (compressed_peak / peak);
}

// Load a texel at an unnormalized coordinate, clamped to the texture like a
// ClampToEdge sampler. Shake offsets can reach past the edge.
fn load_clamped(tex: texture_2d<f32>, coord: vec2<f32>) -> vec4<f32> {
    let last = vec2<i32>(textureDimensions(tex)) - vec2<i32>(1, 1);
    let texel = clamp(vec2<i32>(floor(coord)), vec2<i32>(0, 0), last);
    return textureLoad(tex, texel, 0);
}

// Vignette: tint/darken toward `vignette.rgb` near the edges, center
// unaffected. At rest `vignette.w == 0`, so the mix weight is exactly 0.
// Clamped as a shader-side guard: over-1 strength would extrapolate past the
// tint color.
fn apply_vignette(color: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    let centered = uv - vec2<f32>(0.5, 0.5);
    let radial = clamp(dot(centered, centered) * 2.0, 0.0, 1.0);
    let factor = clamp(effect.vignette.w * radial, 0.0, 1.0);
    return mix(color, effect.vignette.xyz, factor);
}

// Flash: over-blend toward `flash.rgb` by `flash.a`; 0 at rest is an exact
// no-op. Clamped as a shader-side guard against over-1 alpha.
fn apply_flash(color: vec3<f32>) -> vec3<f32> {
    return mix(color, effect.flash.xyz, clamp(effect.flash.a, 0.0, 1.0));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Nearest integer upscale: surface pixel p shows scene pixel
    // floor(p / divisor), so every scene pixel covers divisor × divisor surface
    // pixels and only the overshoot of the last row and column is cropped.
    // `in.clip.xy` is the pixel centre (p + 0.5), which floors to the same
    // scene pixel for any integer divisor.
    let divisor = f32(max(effect.scene_divisor, 1u));
    let scene_size = vec2<f32>(textureDimensions(scene_color_tex));
    // Shake is a screen-fraction offset; at rest it adds exactly zero.
    let scene_coord = in.clip.xy / divisor + effect.shake * scene_size;
    let scene = load_clamped(scene_color_tex, scene_coord);
    var color = soft_knee_tonemap(scene.rgb);

    let covers_flash = effect.covers_hud.x != 0u;
    let covers_vignette = effect.covers_hud.y != 0u;
    let covers_shake = effect.covers_hud.z != 0u;

    // Scene-only effects run before the UI composite.
    if !covers_vignette {
        color = apply_vignette(color, in.uv);
    }
    if !covers_flash {
        color = apply_flash(color);
    }

    // Composite the premultiplied UI layer 1:1 at native resolution.
    var ui_coord = in.clip.xy;
    if covers_shake {
        ui_coord = ui_coord + effect.shake * vec2<f32>(textureDimensions(ui_layer_tex));
    }
    let ui = load_clamped(ui_layer_tex, ui_coord);
    color = color * (1.0 - ui.a) + ui.rgb;
    let alpha = scene.a * (1.0 - ui.a) + ui.a;

    // HUD-covering effects run over the composited frame.
    if covers_vignette {
        color = apply_vignette(color, in.uv);
    }
    if covers_flash {
        color = apply_flash(color);
    }

    return vec4<f32>(color, alpha);
}
