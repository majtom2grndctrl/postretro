// Premultiplied screen layer. Coverage and absolute-metre distances are the only weights.
struct PlanarInputs {
    main_vp: mat4x4<f32>,
    inverse_main_vp: mat4x4<f32>,
    inverse_mirror_vp: mat4x4<f32>,
    eye_height: vec4<f32>,
    mirror_eye: vec4<f32>,
    limits: vec4<f32>,
    extent: vec4<f32>,
};
@group(0) @binding(0) var<uniform> inputs: PlanarInputs;
@group(0) @binding(1) var mirror_color: texture_2d<f32>;
@group(0) @binding(2) var mirror_depth: texture_depth_2d;
@group(0) @binding(3) var water_coverage: texture_2d<f32>;
@vertex fn vs_resolve(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(p[index], 0.0, 1.0);
}
fn fade_distance(value: f32, limit: f32, band: f32) -> f32 {
    if limit <= 0.0 { return 1.0; }
    return clamp((limit - value) / band, 0.0, 1.0);
}
@fragment fn fs_resolve(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let mirrored = vec2<i32>(i32(inputs.extent.x) - 1 - pixel.x, pixel.y);
    if textureLoad(water_coverage, mirrored, 0).r == 0.0 { return vec4(0.0); }
    let depth = textureLoad(mirror_depth, mirrored, 0);
    if depth <= 0.0 || depth >= 1.0 { return vec4(0.0); }
    let mirror_uv = (vec2<f32>(mirrored) + vec2(0.5)) / inputs.extent.xy;
    let mirror_ndc = vec2(mirror_uv.x * 2.0 - 1.0, 1.0 - mirror_uv.y * 2.0);
    let p_h = inputs.inverse_mirror_vp * vec4(mirror_ndc, depth, 1.0);
    let p = p_h.xyz / p_h.w;
    let dist = distance(inputs.mirror_eye.xyz, p);
    let uv = position.xy / inputs.extent.xy;
    let ndc = vec2(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let far_h = inputs.inverse_main_vp * vec4(ndc, 1.0, 1.0);
    let ray = far_h.xyz / far_h.w - inputs.eye_height.xyz;
    if abs(ray.y) < 0.000001 { return vec4(0.0); }
    let t = (inputs.eye_height.w - inputs.eye_height.y) / ray.y;
    if t <= 0.0 { return vec4(0.0); }
    let r = length(ray * t);
    let c = fade_distance(dist, inputs.limits.x, inputs.limits.y)
        * fade_distance(r, inputs.limits.z, inputs.limits.w);
    let rgb = clamp(textureLoad(mirror_color, mirrored, 0).rgb, vec3(0.0), vec3(65504.0));
    return vec4(c * rgb, c);
}
