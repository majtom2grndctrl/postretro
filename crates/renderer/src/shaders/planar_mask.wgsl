// Selected water coverage in the mirror raster's flipped screen coordinates.
struct PlanarInputs {
    main_vp: mat4x4<f32>,
    inverse_main_vp: mat4x4<f32>,
    inverse_mirror_vp: mat4x4<f32>,
    eye_height: vec4<f32>,
    mirror_eye: vec4<f32>,
    limits: vec4<f32>, // D, bandD, R, bandR; zero limit is unlimited.
    extent: vec4<f32>, // target width/height, scene width/height.
};
@group(0) @binding(0) var<uniform> inputs: PlanarInputs;
@group(0) @binding(1) var scene_depth: texture_depth_2d;
struct MaskVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) world: vec3<f32>,
};
@vertex fn vs_mask(@location(0) world: vec3<f32>) -> MaskVertex {
    var out: MaskVertex;
    out.position = inputs.main_vp * vec4<f32>(world, 1.0);
    out.position.x = -out.position.x;
    out.world = world;
    return out;
}
struct MaskFragment {
    @builtin(frag_depth) depth: f32,
    @location(0) coverage: f32,
};
@fragment fn fs_mask(in: MaskVertex) -> MaskFragment {
    let main_pixel = vec2<i32>(
        i32(inputs.extent.z) - 1 - i32(in.position.x * inputs.extent.z / inputs.extent.x),
        i32(in.position.y * inputs.extent.w / inputs.extent.y));
    let opaque = textureLoad(scene_depth, main_pixel, 0);
    if in.position.z > opaque + 0.000001 { discard; }
    let r = distance(inputs.eye_height.xyz, in.world);
    if inputs.limits.z > 0.0 && r >= inputs.limits.z { discard; }
    var out: MaskFragment;
    out.depth = 1.0;
    out.coverage = 1.0;
    return out;
}
