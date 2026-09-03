// Dynamic-tier world-depth-cache sampler. The live pool holds only current
// entity occluders for a warm cached slot; each PCF tap takes the stricter
// result of that live depth and the stable cached world depth.

fn sample_spot_shadow_with_dynamic_world(
    slot_index: u32,
    cache_layer: i32,
    light_pos: vec3<f32>,
    world_pos: vec3<f32>,
    receiver_normal: vec3<f32>,
    bias_scale: f32,
    light_proj: mat4x4<f32>,
) -> f32 {
    let projection_y_scale = length(vec3<f32>(
        light_proj[0].y,
        light_proj[1].y,
        light_proj[2].y,
    ));
    let tan_half_fov_y = 1.0 / max(projection_y_scale, 1.0e-4);
    let distance_to_light = length(world_pos - light_pos);
    let shadow_dims = textureDimensions(spot_shadow_depth);
    let texel_world_footprint =
        2.0 * distance_to_light * tan_half_fov_y / max(f32(shadow_dims.y), 1.0);
    let receiver_offset = receiver_normal * (texel_world_footprint * bias_scale);
    let light_clip = light_proj * vec4<f32>(world_pos + receiver_offset, 1.0);
    if light_clip.w <= 0.0 {
        return 1.0;
    }
    let light_ndc = light_clip.xyz / light_clip.w;
    let uv = vec2<f32>(light_ndc.x * 0.5 + 0.5, light_ndc.y * -0.5 + 0.5);
    if uv.x < 0.0 || uv.x > 1.0 ||
       uv.y < 0.0 || uv.y > 1.0 ||
       light_ndc.z < 0.0 || light_ndc.z > 1.0 {
        return 1.0;
    }

    let texel = 1.0 / vec2<f32>(textureDimensions(spot_shadow_depth));
    let step = texel * SPOT_SHADOW_PCF_RADIUS;
    var lit = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * step;
            let pool = textureSampleCompare(
                spot_shadow_depth,
                spot_shadow_compare,
                uv + offset,
                i32(slot_index),
                light_ndc.z,
            );
            let static_world = textureSampleCompare(
                dynamic_spot_depth_cache,
                dynamic_shadow_compare,
                uv + offset,
                cache_layer,
                light_ndc.z,
            );
            lit = lit + min(pool, static_world);
        }
    }
    return lit / 9.0;
}

fn sample_point_shadow_with_dynamic_world(
    slot_index: u32,
    cache_slot: i32,
    light_pos: vec3<f32>,
    world_pos: vec3<f32>,
    receiver_normal: vec3<f32>,
    bias_scale: f32,
    far_range: f32,
) -> f32 {
    // CUBE_SHADOW_BODY_BEGIN
    let distance_to_light = length(world_pos - light_pos);
    let receiver_offset = receiver_normal * (2.0 * distance_to_light / CUBE_FACE_RESOLUTION * bias_scale);
    let to_frag = (world_pos + receiver_offset) - light_pos;
    let dist = length(to_frag);
    if dist < 1.0e-4 { return 1.0; }
    let dir = to_frag / dist;
    let lookup = vec3<f32>(dir.x, -dir.y, dir.z);
    let axis_depth = max(abs(dir.x), max(abs(dir.y), abs(dir.z))) * dist;
    let reference = clamp(
        cube_face_ndc_depth(max(axis_depth - POINT_SHADOW_DEPTH_BIAS, CUBE_NEAR_CLIP), CUBE_NEAR_CLIP, max(far_range, 0.5)),
        0.0, 1.0,
    );
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(lookup.y) > 0.99);
    let tangent = normalize(cross(up, lookup));
    let bitangent = cross(lookup, tangent);
    let texel_angle = SPOT_SHADOW_PCF_RADIUS * (1.5707963 / CUBE_FACE_RESOLUTION);
    var lit = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let sample_dir = normalize(lookup + (tangent * f32(dx) + bitangent * f32(dy)) * texel_angle);
            let pool = textureSampleCompareLevel(point_shadow_cube, spot_shadow_compare, sample_dir, i32(slot_index), reference);
            let static_world = textureSampleCompareLevel(dynamic_cube_depth_cache, dynamic_shadow_compare, sample_dir, cache_slot, reference);
            lit = lit + min(pool, static_world);
        }
    }
    return lit / 9.0;
    // CUBE_SHADOW_BODY_END
}
