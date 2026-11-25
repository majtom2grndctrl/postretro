// The glossy floor layer's march: each up-facing, probe-bound glossy
// fragment's mirrored ray marched across the frame's scene depth by water's
// `screen_march`, unmodified, its hit read at the march's last sample in front
// of the depth buffer from the scene-colour chain, at the level the texel's
// lobe and the hit length set, written premultiplied
// (c x X, c) to the floor layer's target, which the overlay composites over
// the probe term on up-facing faces. Concatenated with glossy_floor_blur.wgsl
// (the chain level and its read), glossy_surface.wgsl (the camera and
// world-material groups and the world vertex), glossy_overlay_shading.wgsl
// (the overlay's point and normal, so both layers reflect one ray),
// material_shading.wgsl, surface_depth.wgsl, reflection_probe_blur.wgsl,
// fluid_motion.wgsl, fluid_reflection_ray.wgsl and screen_march.wgsl.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors; Screen-space layer)

// Binds no probe colour, distance or sums array, no capture-shadow input and
// no static diffuse: no probe quantity is reachable from this pass, so F x c
// x X is the same at every capture-shadow weight. No history: its target and
// depth clear every frame it records.

// --- Group 2: the frame's full-resolution scene depth, read by the march.
@group(2) @binding(0) var scene_depth: texture_depth_2d;

// --- Group 3 (with glossy_floor_blur.wgsl's chain and sampler): the floor
// tier's march.
struct GlossyFloorMarch {
    // March steps per ray.
    step_budget: u32,
    // Full-resolution pixels per march step.
    stride: f32,
    // Metres of view depth a step's interval may lie behind the depth buffer
    // for a hit.
    thickness: f32,
    // Padding to the uniform's 16 bytes; carries nothing.
    spare: f32,
};
@group(3) @binding(2) var<uniform> floor_march: GlossyFloorMarch;

// A culled vertex: outside the clip volume, so a face whose vertices all take
// it clips away whole.
const GLOSSY_FLOOR_CULLED: vec4<f32> = vec4<f32>(2.0, 2.0, 2.0, 1.0);
// Scene pixels the march's origin lifts off its floor along the face normal,
// at the origin's view depth. Water is absent from scene depth; a floor is
// not, and the depth a step reads is its pixel's, which at a grazing view
// lies up to half a pixel's floor nearer than the ray: a ray marched from on
// the floor hits the floor itself within its first steps. One pixel's lift
// keeps the ray's first samples in front of every floor depth its pixels
// hold, and moves the start by under a pixel on screen.
const GLOSSY_FLOOR_ORIGIN_LIFT_PIXELS: f32 = 1.0;

// The overlay's vertex stage, statement for statement, but for the clip
// position: only a probe-bound face that faces up by water's test takes the
// floor layer, and every vertex of any other face lands outside the clip
// volume. The compiler gives every face its own vertices and brush faces
// carry flat normals, so a face's vertices agree.
@vertex
fn vs_main(in: GlossyVertexInput) -> GlossyVertexOutput {
    var out: GlossyVertexOutput;
    out.world_position = in.position;
    out.world_normal = oct_decode(in.normal_oct);
    out.probe = in.probe;
    out.uv = in.base_uv;
    let sign_bit = in.tangent_packed.y & 0x8000u;
    let v_15bit = in.tangent_packed.y & 0x7FFFu;
    let v_16bit = v_15bit * 65535u / 32767u;
    out.world_tangent = oct_decode(vec2<u32>(in.tangent_packed.x, v_16bit));
    out.bitangent_sign = select(-1.0, 1.0, sign_bit != 0u);
    let takes_layer = in.probe != FLUID_REFLECTION_NO_PROBE
        && out.world_normal.y >= REFLECTION_UP_FACING_MIN_COS;
    out.clip_position = select(GLOSSY_FLOOR_CULLED,
        uniforms.view_proj * vec4<f32>(in.position, 1.0), takes_layer);
    return out;
}

// The overlay's derivatives at this fragment, per scene pixel. This pass
// rasterises at the chain's level-0 extent, the floor tier's fraction of the
// scene's, so one of its pixels spans several scene pixels; raw derivatives
// would read the exponent, the normal map, Surface Depth's level and the
// texel seam band that many mips coarse, and on a coarse enough fraction turn
// pixel style's texel-centre point into the fragment's own. Scaled to the
// scene, every shared helper reads what the overlay reads at its full-extent
// pixel, as water's reflection pass scales its texel footprint
// (`target_scale`). The UV-to-world Jacobian is a ratio of these, so it is
// unchanged.
fn glossy_floor_derivatives(in: GlossyVertexOutput) -> GlossyDerivatives {
    let d = glossy_overlay_derivatives(in);
    let scale = vec2<f32>(textureDimensions(floor_chain))
        / vec2<f32>(textureDimensions(scene_depth));
    return GlossyDerivatives(d.ddx * scale.x, d.ddy * scale.y, d.ddx_world * scale.x,
        d.ddy_world * scale.y);
}

// The texel's exponent, read as the overlay's `glossy_texel` reads it: the
// specular slot through forward's reconstruction at the UV forward shades.
fn glossy_floor_exponent(uv: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> f32 {
    let slot = sample_post_retro(spec_texture, aniso_sampler, uv, ddx, ddy);
    return specular_texel(slot, SPECULAR_EXPONENT_MIN).exponent;
}

// Where the floor layer reads the hit along the ray: the hit length from the
// ray's origin and the hit's view depth, from the read pixel, which lies on
// the ray's screen segment. Clip space is affine along the ray, so each
// screen axis gives the ray parameter; one least-squares solve over both
// takes neither in preference.
struct GlossyFloorHit {
    hit_length: f32,
    hit_depth: f32,
};

fn glossy_floor_hit(origin: vec3<f32>, ray: vec3<f32>, pixel: vec2<f32>,
                    extent: vec2<f32>) -> GlossyFloorHit {
    let ndc = vec2<f32>(pixel.x / extent.x * 2.0 - 1.0, 1.0 - pixel.y / extent.y * 2.0);
    let start = uniforms.view_proj * vec4<f32>(origin, 1.0);
    let along = uniforms.view_proj * vec4<f32>(ray, 0.0);
    let num = ndc * start.w - start.xy;
    let den = along.xy - ndc * along.w;
    let hit_length = max(dot(num, den) / max(dot(den, den), 1.0e-12), 0.0);
    return GlossyFloorHit(hit_length, start.w + hit_length * along.w);
}

// Scene pixels per unit of tangent at depth 1: the projection's y scale (the
// length of `view_proj`'s y row over a rigid view) times half the scene's
// height.
fn glossy_floor_focal(extent: vec2<f32>) -> f32 {
    let row_y = vec3<f32>(uniforms.view_proj[0].y, uniforms.view_proj[1].y,
        uniforms.view_proj[2].y);
    return length(row_y) * 0.5 * extent.y;
}

// The march's origin: `point` lifted along the face's normal by
// GLOSSY_FLOOR_ORIGIN_LIFT_PIXELS at its own view depth. Reads the point,
// the face normal and the projection alone.
fn glossy_floor_origin(point: vec3<f32>, outward: vec3<f32>, focal: f32) -> vec3<f32> {
    let depth = (uniforms.view_proj * vec4<f32>(point, 1.0)).w;
    return point + outward * (GLOSSY_FLOOR_ORIGIN_LIFT_PIXELS * max(depth, 0.0) / focal);
}

// Where the floor layer reads a hit: the march's last sample in front of the
// depth buffer, one stride back from `stop` along the march's own screen
// segment, which starts at the origin's pixel. A hit always follows a sample
// in front (the in-front latch), so the point lies on the march's grid, on
// screen, on the reflected ray. The stop can lie up to a stride past the
// surface the ray crossed, and a ray passing within the thickness behind a
// standing object stops on that object; read there, a bright strip's
// reflection streaks along the ray over what the object hides. The sample in
// front shows only what the ray had not yet reached, so a reflection never
// spills past an object's silhouette; it may fall up to a stride short. One
// read for every ray: no depth is read here, nothing is tested and no step is
// taken, and the march and its confidence are unchanged.
fn glossy_floor_read_pixel(origin: vec3<f32>, stop: vec2<f32>, extent: vec2<f32>) -> vec2<f32> {
    let start = screen_march_pixel(uniforms.view_proj * vec4<f32>(origin, 1.0), extent);
    let along = stop - start;
    return stop - along * (floor_march.stride / max(length(along), floor_march.stride));
}

// The blurred scene colour at a hit: the chain level the texel's exponent, the
// hit length and the hit's view depth set, read at the hit's read pixel.
fn glossy_floor_hit_colour(pixel: vec2<f32>, exponent: f32, hit: GlossyFloorHit,
                           extent: vec2<f32>, focal: f32) -> vec3<f32> {
    let chain_scale = f32(textureDimensions(floor_chain).y) / extent.y;
    let level = glossy_floor_blur_level(exponent, hit.hit_length, hit.hit_depth, focal,
        chain_scale, textureNumLevels(floor_chain));
    return glossy_floor_blurred_hit(pixel / extent, level);
}

// One style's and material form's march, from the overlay's point (lifted off
// the floor) about the overlay's normal, so the floor layer and the probe term
// reflect one ray.
// Derivatives, per scene pixel, come first, in uniform control flow. The
// probe term's gate (the face seen from its up side, the all-terms mask)
// refuses a fragment, which writes nothing, so its texel reads c = 0. One
// ray per target pixel: the exponent sets the blur alone, never whether, how
// far or how finely a ray marches. A miss, or a stop with no confidence,
// writes a literal 0.
fn glossy_floor_march(in: GlossyVertexOutput, pixel: bool, carve: bool,
                      normal_map: bool) -> vec4<f32> {
    let derivatives = glossy_floor_derivatives(in);
    let depth = glossy_overlay_depth(in, derivatives, carve);
    let exponent = glossy_floor_exponent(depth.uv, derivatives.ddx, derivatives.ddy);
    let surface = glossy_overlay_surface(in, derivatives, depth, pixel, normal_map);
    if !fluid_reflection_seen_from_above(surface.outward, surface.view)
        || !fluid_reflection_mask_on(uniforms.light_term_mask) {
        discard;
    }
    let ray = fluid_reflection_above_hemisphere(fluid_reflection_mirror(surface.view,
        surface.normal), fluid_reflection_hemisphere_axis(surface.outward));
    let extent = vec2<f32>(textureDimensions(scene_depth));
    let focal = glossy_floor_focal(extent);
    let origin = glossy_floor_origin(surface.origin, surface.outward, focal);
    let march = screen_march(origin, ray, uniforms.view_proj, scene_depth,
        floor_march.stride, floor_march.step_budget, floor_march.thickness);
    let read = glossy_floor_read_pixel(origin, march.pixel, extent);
    let colour = glossy_floor_hit_colour(read, exponent,
        glossy_floor_hit(origin, ray, read, extent), extent, focal);
    let premultiplied = vec4<f32>(colour * march.confidence, march.confidence);
    return select(vec4<f32>(0.0), premultiplied, march.confidence > 0.0);
}

// One entry per `render.reflections.style` and material form, as the
// overlay's: pixel marches once per albedo texel, from its texel-centre
// point; smooth per fragment. `fs_pixel` and `fs_smooth` run forward's march
// and sample the normal map; `_flat` draws materials that never march,
// `_plain` those that also bind the neutral normal placeholder.
@fragment
fn fs_pixel(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return glossy_floor_march(in, true, true, true);
}

@fragment
fn fs_smooth(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return glossy_floor_march(in, false, true, true);
}

@fragment
fn fs_pixel_flat(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return glossy_floor_march(in, true, false, true);
}

@fragment
fn fs_smooth_flat(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return glossy_floor_march(in, false, false, true);
}

@fragment
fn fs_pixel_plain(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return glossy_floor_march(in, true, false, false);
}

@fragment
fn fs_smooth_plain(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return glossy_floor_march(in, false, false, false);
}
