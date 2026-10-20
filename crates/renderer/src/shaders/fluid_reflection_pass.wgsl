// Fluid reflection pass: each reflecting fluid fragment's mirrored ray marched
// against the frame's opaque depth, written premultiplied (c x SSR, c) to the
// reflection target the fluid pass composites over the probe term. Composed
// with fluid_motion.wgsl, fluid_reflection_ray.wgsl and screen_march.wgsl.
// See: context/lib/rendering_pipeline.md §7.3a

// Binds no probe colour or distance array, no capture-shadow input and none of
// the fluid pass's group 5, which holds this pass's own target.

// --- Group 0: camera uniforms (shared with forward, billboard and fluid) ---
// Field offsets mirror the Rust `Uniforms` writer; see fluid_surface.wgsl.
struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    time: f32,
    light_term_mask: u32,
    indirect_scale: f32,
    sdf_shadow_flags: u32,
    sdf_shadow_mode: u32,
    sdf_force_visibility_one: u32,
    direct_scale: f32,
    has_scatter: u32,
    has_direct: u32,
    total_light_count: u32,
    _spec_shadowmask_flags_inert: u32,
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

// --- Group 1: the face's world material, read for its texel grid only ---
@group(1) @binding(0) var base_texture: texture_2d<f32>;

// --- Group 2: this pass's inputs, rebuilt only with the scene targets ---
// The opaque scene's full-resolution depth and colour, neither attached here.
@group(2) @binding(0) var scene_depth: texture_depth_2d;
@group(2) @binding(1) var scene_color: texture_2d<f32>;
struct FluidReflectionInputs {
    // The mod's reflection style: FLUID_REFLECTION_STYLE_SMOOTH, else pixel.
    reflection_style: u32,
    // March steps per ray.
    step_budget: u32,
    // Metres of view depth the near end of a step's view-depth interval may
    // lie behind the depth buffer for a hit; not the current sample's.
    thickness: f32,
    // Full-resolution pixels per march step.
    stride: f32,
    // The target's extent over the scene's, per axis.
    target_scale: vec2<f32>,
    // Padding to the uniform's 32 bytes; carries nothing.
    spare: vec2<f32>,
};
@group(2) @binding(2) var<uniform> reflection_inputs: FluidReflectionInputs;

// --- Group 3: the fluid pass's group 4 buffers, window and probe table only ---
// Mirrors fluid_surface.wgsl's rows and table member for member.
struct FluidVolumeRow {
    surface: vec4<f32>,
    fog_albedo: vec4<f32>,
    wave: vec4<f32>,
};
struct FluidVolumeParams {
    slots: array<FluidVolumeRow, 1360>,
};
@group(3) @binding(0) var<uniform> fluid_volumes: FluidVolumeParams;
struct FluidProbeLight {
    reference: vec3<f32>,
    roster_index: u32,
    shares: array<vec4<u32>, 2>,
};
struct FluidProbe {
    box_min: vec3<f32>,
    _pad0: u32,
    box_max: vec3<f32>,
    _pad1: u32,
    capture_point: vec3<f32>,
    layer: u32,
    lights: array<FluidProbeLight, 4>,
};
struct FluidProbeTable {
    tile: vec4<u32>,
    probes: array<FluidProbe, 256>,
};
// Read for the box and capture point the pixel style's cell point is held to.
@group(3) @binding(2) var<uniform> fluid_probes: FluidProbeTable;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // Drift never moves a reflection's cell or ray; declared to match the
    // fluid pass's vertex streams.
    @location(3) drift_rate: vec2<f32>,
    @location(4) probe: u32,
};

struct ReflectionVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) @interpolate(flat) slot: u32,
    @location(4) @interpolate(flat) probe: u32,
};

// No baked light: the reflection reads only position, texel grid and probe.
@vertex
fn vs_reflection(in: VertexInput, @builtin(instance_index) slot: u32) -> ReflectionVertexOutput {
    var out: ReflectionVertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.world_normal = in.normal;
    out.world_position = in.position;
    out.slot = slot;
    out.probe = in.probe;
    return out;
}

@fragment
fn fs_reflection(in: ReflectionVertexOutput) -> @location(0) vec4<f32> {
    // Derivatives in uniform control flow, before the gate and the march.
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);
    let ddx_world = dpdx(in.world_position);
    let ddy_world = dpdy(in.world_position);
    let dims = vec2<f32>(textureDimensions(base_texture, 0));
    // Per target pixel here; scaled to per scene pixel so the cell fade and
    // tilt match the fluid pass's for the same cell at any target extent.
    let scale = reflection_inputs.target_scale;
    let texels_per_pixel = max(length(ddx * dims * scale.x), length(ddy * dims * scale.y));
    let view_dir = normalize(uniforms.camera_position - in.world_position);
    let outward = normalize(in.world_normal);
    let reflectivity = fluid_volumes.slots[in.slot].surface.z;
    // The fluid pass's own gate: a refused fragment writes nothing, so its
    // texel reads c = 0 there.
    if !fluid_reflection_gate(in.probe, outward, view_dir, uniforms.light_term_mask, reflectivity) {
        discard;
    }
    // The texel-to-world axes are a ratio of derivatives, the same at any
    // target extent.
    let axes = fluid_reflection_texel_axes(ddx * dims, ddy * dims, ddx_world, ddy_world);
    let point = fluid_reflection_point(reflection_inputs.reflection_style, in.uv * dims,
        in.world_position, outward, axes, uniforms.time, texels_per_pixel,
        fluid_probes.probes[in.probe].box_min, fluid_probes.probes[in.probe].box_max,
        fluid_probes.probes[in.probe].capture_point);
    let view = normalize(uniforms.camera_position - point.position);
    let ray = fluid_reflection_above_horizon(fluid_reflection_mirror(view, point.normal));
    let march = screen_march(point.position, ray, uniforms.view_proj, scene_depth,
        reflection_inputs.stride, reflection_inputs.step_budget, reflection_inputs.thickness);
    let colour = textureLoad(scene_color, vec2<i32>(march.pixel), 0).rgb;
    // A miss, or a stop with no confidence, writes a literal 0: the texel it
    // loaded may hold Inf (Rgba16Float saturates), and Inf x 0 is a NaN that
    // smooth bilinear sampling would spread across the water.
    let premultiplied = vec4<f32>(colour * march.confidence, march.confidence);
    return select(vec4<f32>(0.0), premultiplied, march.confidence > 0.0);
}
