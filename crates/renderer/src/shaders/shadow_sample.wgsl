// Shared runtime shadow-map sampling helpers (binding-agnostic).
// See: context/lib/rendering_pipeline.md §4, §8
//
// `sample_spot_shadow` (spot 2D-array PCF) and `sample_point_shadow` (point
// cube-array PCF) plus their bias/resolution constants and the
// `cube_face_ndc_depth` reconstruction. The forward pass evaluates these against
// its group-5 shadow bindings; the skinned-mesh pass mirrors the same calls
// against its own group-2 b5–b8 shadow bindings. These helpers declare no
// bindings: each consumer declares the depth textures (`spot_shadow_depth`,
// `point_shadow_cube`), the comparison sampler (`spot_shadow_compare`), and the
// light-space matrices buffer at its own (group, binding) BEFORE this file is
// textually concatenated. The helpers reference those consumer-declared global
// names by lexical resolution — the same precedent as `sh_sample.wgsl` /
// `light_eval.wgsl`.
//
// No-cube gating: the body markers around `sample_point_shadow`'s body (the
// `// CUBE_SHADOW_BODY_...` BEGIN/END comment lines inside the function) MUST stay
// intact. On a no-`CUBE_ARRAY_TEXTURES` adapter, `render::strip_point_shadow_cube`
// operates on the composed consumer source (forward + this snippet), replacing
// everything between those markers with `return 1.0;` so the function references
// no stripped `point_shadow_cube` binding. The cube BINDING declaration itself
// stays with the consumer (it is a binding), tagged `// CUBE_SHADOW_BINDING`
// there. (This header avoids the exact BEGIN/END tokens so the strip's
// post-condition — no marker survives the composed-source transform — holds.)

// Tunable PCF radius (in shadow-map texels) for runtime shadow-map sampling.
// The ONE shared softness parameter: the spot path here and the point path
// (Task 5) both scale their multi-tap kernel by this. NON-ZERO so the kernel
// samples more than one texel — a 3×3 box of comparison samples spaced
// `SPOT_SHADOW_PCF_RADIUS` texels apart antialiases the shadow edge rather than
// stair-stepping a single texel. A module-scope const (not a uniform) keeps the
// group-0 `Uniforms` 3-way byte contract untouched; the depth BIAS stays
// per-pool (the depth pipeline's `DepthBiasState`), so only the radius is shared.
const SPOT_SHADOW_PCF_RADIUS: f32 = 1.0;

// Sample the shadow map for a dynamic spot light. Returns 0.0 (fully shadowed)
// to 1.0 (fully lit). Fragments outside the shadow map's projection are treated
// as unshadowed (1.0).
//
// `slot_index`: shadow-map slot from GpuLight.cone_angles_and_pad.z.
fn sample_spot_shadow(slot_index: u32, world_pos: vec3<f32>, light_proj: mat4x4<f32>) -> f32 {
    let light_clip = light_proj * vec4<f32>(world_pos, 1.0);
    // Points behind the light produce negative w; reject to avoid folding the
    // perspective divide onto the near plane.
    if light_clip.w <= 0.0 {
        return 1.0;
    }
    let light_ndc = light_clip.xyz / light_clip.w;

    // NDC x,y are in [-1, 1] (wgpu convention). Flip y for texture top-left origin.
    let uv = vec2<f32>(light_ndc.x * 0.5 + 0.5, light_ndc.y * -0.5 + 0.5);
    if uv.x < 0.0 || uv.x > 1.0 ||
       uv.y < 0.0 || uv.y > 1.0 ||
       light_ndc.z < 0.0 || light_ndc.z > 1.0 {
        return 1.0; // Unshadowed — outside cone.
    }

    // Tunable-radius PCF: average a 3×3 grid of comparison samples spaced
    // `SPOT_SHADOW_PCF_RADIUS` texels apart in UV. `textureSampleCompare`
    // (CompareFunction::Less) returns 1.0 per tap when the fragment is closer
    // than the stored occluder depth (lit); the mean is the soft visibility.
    let texel = 1.0 / vec2<f32>(textureDimensions(spot_shadow_depth));
    let step = texel * SPOT_SHADOW_PCF_RADIUS;
    var lit = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * step;
            lit = lit + textureSampleCompare(
                spot_shadow_depth,
                spot_shadow_compare,
                uv + offset,
                i32(slot_index),
                light_ndc.z
            );
        }
    }
    return lit / 9.0;
}

// Near-clip distance for a cube face's perspective projection. MUST match
// `CUBE_NEAR_CLIP` in `lighting/cube_shadow.rs` — the cube depth pass projects
// each face with this near plane, so the NDC-depth reconstruction below must use
// the same value to compare against the stored depth.
const CUBE_NEAR_CLIP: f32 = 0.1;

// Per-side resolution of each cube face, mirroring `CUBE_FACE_RESOLUTION` in
// `lighting/cube_shadow.rs` (pinned by `forward_cube_sampling_constants_match_pool`).
// Drives the PCF tap spacing below: one cube-face texel subtends `90° / this`.
const CUBE_FACE_RESOLUTION: f32 = 512.0;

// Per-face depth bias for the POINT cube path, in world units (subtracted from
// the light→fragment distance BEFORE projecting to NDC). Tuned SEPARATELY from
// the spot path's depth bias: the spot path applies its bias as the skinned-depth
// pipeline's hardware `DepthBiasState` (a slope/constant offset baked into the
// stored occluder depth at the depth-write stage), while this is a receiver-side
// world-unit offset applied here at sample time. The two act at DIFFERENT
// pipeline stages on DIFFERENT values, so they never double-count on one
// fragment. (The cube depth pass also carries the same hardware `DepthBiasState`
// on its entity occluders; this receiver-side bias is added on top, by design —
// cube faces are 512² vs spot 1024² and perspective-depth acne is worst near the
// far plane, so the acne/peter-panning trade-off needs its own knob.)
const POINT_SHADOW_DEPTH_BIAS: f32 = 0.08;

// Project a light-local linear depth (distance along the dominant cube-face
// axis, i.e. the largest-magnitude component of the light→fragment vector) into
// the perspective NDC depth [0,1] the cube depth pass stored. The cube faces are
// rendered with `Mat4::perspective_rh(90°, 1.0, near, far)` (wgpu z ∈ [0,1]), so
// for a view-space depth `d` (= dominant axis magnitude = -view_z) the stored
// NDC z is `far/(far-near) - (near*far)/((far-near)*d)`. Matching this exactly
// is why a plain linear-distance compare would mis-shadow.
fn cube_face_ndc_depth(d: f32, near: f32, far: f32) -> f32 {
    let a = far / (far - near);
    return a - (near * far) / ((far - near) * d);
}

// Sample the cube-array shadow map for a dynamic point light. Returns 0.0 (fully
// shadowed) to 1.0 (fully lit). The cube face is selected by hardware from the
// `light→fragment` direction vector, so there is no per-face seam to handle and
// every direction is covered.
//
// `slot_index`: cube slot from `GpuLight.cone_angles_and_pad.w`. `light_pos`:
// the light world position; `world_pos`: the shaded fragment; `far_range`: the
// light's falloff range (the cube faces' far plane). The comparison reference is
// the fragment's PERSPECTIVE NDC depth on its cube face (reconstructed from the
// dominant axis), matching what the depth pass wrote.
fn sample_point_shadow(
    slot_index: u32,
    light_pos: vec3<f32>,
    world_pos: vec3<f32>,
    far_range: f32,
) -> f32 {
    // CUBE_SHADOW_BODY_BEGIN — on a no-`CUBE_ARRAY_TEXTURES` adapter the renderer
    // replaces everything up to CUBE_SHADOW_BODY_END with `return 1.0;`, so this
    // function references no `point_shadow_cube` binding (which is stripped). The
    // point-light call site is still compiled but always reads "unshadowed".
    let to_frag = world_pos - light_pos;
    let dist = length(to_frag);
    // The depth pass clamps the far plane to >= 0.5 (`falloff_range.max(0.5)`);
    // mirror that so the NDC reconstruction uses the same far plane.
    let far = max(far_range, 0.5);
    if dist < 1e-4 {
        return 1.0;
    }
    // Direction from light toward the fragment — the cube lookup vector.
    let dir = to_frag / dist;
    // Dominant-axis magnitude = the view-space depth on the selected face. Apply
    // the world-space bias here (pull the receiver toward the light) before
    // projecting, then convert to the stored NDC depth. `textureSampleCompareLevel`
    // (CompareFunction::Less) returns 1.0 per tap when the fragment is nearer
    // than the stored occluder depth (lit). Clamp the biased depth to the near
    // plane so a fragment closer than near never produces a negative reference.
    let axis_depth = max(abs(dir.x), max(abs(dir.y), abs(dir.z))) * dist;
    let biased_depth = max(axis_depth - POINT_SHADOW_DEPTH_BIAS, CUBE_NEAR_CLIP);
    let reference = clamp(cube_face_ndc_depth(biased_depth, CUBE_NEAR_CLIP, far), 0.0, 1.0);

    // Build an orthonormal basis around `dir` so the PCF taps offset the lookup
    // vector perpendicular to it (Bevy's cube PCF pattern). `up` is chosen to
    // avoid degeneracy when `dir` is near the Y axis.
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(dir.y) > 0.99);
    let tangent = normalize(cross(up, dir));
    let bitangent = cross(dir, tangent);
    // Angular tap spacing: scale `SPOT_SHADOW_PCF_RADIUS` by one cube-face texel
    // angle (face FOV 90° over the face resolution) so the shared radius reads as
    // "texels" on the cube faces too.
    let texel_angle = SPOT_SHADOW_PCF_RADIUS * (1.5707963 / CUBE_FACE_RESOLUTION);

    var lit = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let offset = (tangent * f32(dx) + bitangent * f32(dy)) * texel_angle;
            let sample_dir = normalize(dir + offset);
            lit = lit + textureSampleCompareLevel(
                point_shadow_cube,
                spot_shadow_compare,
                sample_dir,
                i32(slot_index),
                reference
            );
        }
    }
    return lit / 9.0;
    // CUBE_SHADOW_BODY_END
}
