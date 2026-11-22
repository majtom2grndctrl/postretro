// FROZEN: the mover pass's runtime light loop as kinematic_brush.wgsl evaluated
// it at 74fcfd4b9, after the texel-centre point and before per-texel sharpness
// and tint. Never edit: it is the pixel-style byte-identity oracle for
// `kinematic_specular_harness.rs` (an untinted texel's specular, which tint must
// leave exactly as it was); the 1c7fc9e3a copy is the smooth-style oracle.
// Regenerate only by re-extracting from that commit (`git show
// 74fcfd4b9:<path>`), prefixing the frozen identifiers `pretint_` /
// `PRETINT_`. Source: kinematic_brush.wgsl (`mover_light_geometry`,
// `mover_texel_specular`, `accumulate_dynamic_direct`,
// `SHADOWMASK_META_VEC4S_PER_RECORD`). Every other helper (blinn_phong, the
// light-eval and animation helpers, the shadow samplers, the Surface Depth
// self-shadow march) resolves from the harness by name.

const PRETINT_SHADOWMASK_META_VEC4S_PER_RECORD: u32 = 2u;

fn pretint_mover_light_geometry(light: GpuLight, aim: vec3<f32>, position: vec3<f32>) -> vec4<f32> {
    let light_type = bitcast<u32>(light.position_and_type.w);
    if light_type > 1u {
        return vec4<f32>(-aim, 1.0);
    }
    let to_light = light.position_and_type.xyz - position;
    let dist = length(to_light);
    let L = to_light / max(dist, 0.0001);
    let falloff_model = bitcast<u32>(light.color_and_falloff_model.w);
    var attenuation = light_eval_falloff(dist, light.direction_and_range.w, falloff_model);
    if light_type == 1u {
        attenuation = attenuation
            * light_eval_cone_attenuation(L, aim, light.cone_angles_and_pad.x, light.cone_angles_and_pad.y);
    }
    return vec4<f32>(L, attenuation);
}

fn pretint_mover_texel_specular(
    light: GpuLight,
    color: vec3<f32>,
    aim: vec3<f32>,
    point: SpecularPoint,
    n: vec3<f32>,
    mesh_n: vec3<f32>,
    hit_top: bool,
    shadow: f32,
    spec_exp: f32,
    spec_int: f32,
) -> vec3<f32> {
    let geometry = pretint_mover_light_geometry(light, aim, point.position);
    let L = geometry.xyz;
    // The diffuse gate's shape: a side face also needs the light above the
    // geometric plane.
    if !(dot(n, L) > 0.0 && (hit_top || dot(mesh_n, L) > 0.0)) {
        return vec3<f32>(0.0);
    }
    return blinn_phong(L, point.view, n, color, spec_exp, spec_int) * (geometry.w * shadow);
}

fn pretint_accumulate_dynamic_direct(
    world_pos: vec3<f32>,
    n: vec3<f32>,
    mesh_n: vec3<f32>,
    spec_point: SpecularPoint,
    spec_exp: f32,
    spec_int: f32,
    use_dynamic: bool,
    use_specular: bool,
    smooth_style: bool,
    depth: SurfaceDepthResult,
) -> vec3<f32> {
    var total = vec3<f32>(0.0);
    let light_count = select(0u, kinematic_light_params.light_count, use_dynamic);
    // Surface Depth self-shadowing is budgeted per fragment by
    // `depth.shadow_light_budget` (the player's on/off switch, carried in the
    // per-material uniform; zero at `Off`) and applies to the
    // DYNAMIC prefix only — the animated-baked tail and the selected-static
    // suffix are baked-tier records whose occlusion the bake already owns.
    var depth_shadow_marches: u32 = 0u;
    for (var i: u32 = 0u; i < light_count; i = i + 1u) {
        var cache_layer = -1i;
        // Animated tails retain descriptors through `scripted_light_count`,
        // but both they and selected-static suffix records use cache metadata
        // beginning directly after the dynamic prefix.
        if i >= kinematic_light_params.dynamic_light_count {
            let promoted_index = i - kinematic_light_params.dynamic_light_count;
            let meta_index = kinematic_light_params.light_count
                + promoted_index * PRETINT_SHADOWMASK_META_VEC4S_PER_RECORD;
            if meta_index + 1u < arrayLength(&light_influence) {
                cache_layer = i32(light_influence[meta_index + 1u].w);
            }
        }
        // Pixel-style specular from a selected-static record has its own gates
        // at the texel's point: the per-pixel influence test gates diffuse
        // only, and the record is skipped only when neither reaches.
        let texel_specular = use_specular
            && !smooth_style
            && i >= kinematic_light_params.scripted_light_count;
        let influence = light_influence[i];
        let inf_radius = influence.w;
        var pixel_reached = true;
        if inf_radius <= 1.0e30 {
            let inf_radius_sq = inf_radius * inf_radius;
            let d = world_pos - influence.xyz;
            pixel_reached = !(dot(d, d) > inf_radius_sq);
            let p = spec_point.position - influence.xyz;
            let point_reached = texel_specular && !(dot(p, p) > inf_radius_sq);
            if !pixel_reached && !point_reached {
                continue;
            }
        }

        let light = lights[i];
        let light_type = bitcast<u32>(light.position_and_type.w);

        var effective_color = light.color_and_falloff_model.xyz;
        var effective_aim = light.direction_and_range.xyz;
        // The dynamic tier and raw animated-baked tail own descriptors.
        // Selected-static records append after that prefix.
        if i < kinematic_light_params.scripted_light_count {
            let scripted_desc = scripted_light_descriptors[i];
            if scripted_desc.is_active != 0u {
                let cycle_t = animation_curve_t(
                    scripted_desc.period,
                    scripted_desc.phase,
                    kinematic_light_params.time,
                );
                if scripted_desc.color_count > 0u {
                    let unit_sample = max(
                        sample_color_catmull_rom(scripted_desc.color_offset, scripted_desc.color_count, cycle_t, scripted_desc.base_color),
                        vec3<f32>(0.0),
                    );
                    let intensity = light_eval_scripted_intensity_scalar(light.color_and_falloff_model.xyz, scripted_desc.base_color);
                    let brightness = max(sample_curve_catmull_rom(scripted_desc.brightness_offset, scripted_desc.brightness_count, cycle_t), 0.0);
                    effective_color = unit_sample * intensity * brightness;
                } else if scripted_desc.brightness_count > 0u {
                    let brightness = max(sample_curve_catmull_rom(scripted_desc.brightness_offset, scripted_desc.brightness_count, cycle_t), 0.0);
                    effective_color = light.color_and_falloff_model.xyz * brightness;
                }
                if light_type == 1u && scripted_desc.direction_count > 0u {
                    effective_aim = light_eval_animated_direction(scripted_desc, cycle_t, effective_aim);
                }
            } else if light_eval_scripted_descriptor_present(scripted_desc) {
                effective_color = vec3<f32>(0.0);
            }
        }

        // The DISPLACED hit position feeds direction and attenuation; the
        // shadow-map lookups below stay on the true plane, which is the
        // geometry the depth maps hold and their bias is tuned for.
        let geometry = pretint_mover_light_geometry(light, effective_aim, depth.world_position);
        let L = geometry.xyz;
        var attenuation = geometry.w;
        // This fragment's own pool shadow, also kept apart for pixel-style
        // specular, which takes its texel's value times this.
        var shadow = 1.0;
        switch light_type {
            case 0u: {
                let cube_slot = bitcast<u32>(light.cone_angles_and_pad.w);
                if cube_slot != 0xFFFFFFFFu {
                    if i >= kinematic_light_params.dynamic_light_count {
                        shadow = sample_point_shadow_with_static(
                            cube_slot,
                            cache_layer,
                            light.position_and_type.xyz,
                            world_pos,
                            mesh_n,
                            MOVER_RECEIVER_BIAS_SCALE,
                            light.direction_and_range.w,
                        );
                    } else {
                        shadow = sample_point_shadow(
                            cube_slot, light.position_and_type.xyz, world_pos, mesh_n,
                            MOVER_RECEIVER_BIAS_SCALE, light.direction_and_range.w,
                        );
                    }
                    attenuation = attenuation * shadow;
                }
            }
            case 1u: {
                let slot_index = bitcast<u32>(light.cone_angles_and_pad.z);
                if slot_index != 0xFFFFFFFFu {
                    if i >= kinematic_light_params.dynamic_light_count {
                        shadow = sample_spot_shadow_with_static(
                            slot_index,
                            cache_layer,
                            light.position_and_type.xyz,
                            world_pos,
                            mesh_n,
                            MOVER_RECEIVER_BIAS_SCALE,
                            light_space_matrices.m[slot_index],
                        );
                    } else {
                        shadow = sample_spot_shadow(
                            slot_index, light.position_and_type.xyz, world_pos, mesh_n,
                            MOVER_RECEIVER_BIAS_SCALE, light_space_matrices.m[slot_index],
                        );
                    }
                    attenuation = attenuation * shadow;
                }
            }
            default: {}
        }
        // Gate a SIDE hit on the geometric plane as well as on its shading
        // normal. A side-wall normal is an exact +/-U or +/-V axis lying IN the
        // tangent plane, so `dot(n, L) > 0` holds across half the
        // sub-plane hemisphere — a light behind opaque brush geometry would
        // otherwise light the carved face at full strength, with no shadow map
        // to stop it when the light won no slot. Top hits are left exactly as
        // they were: their normal is the normal-mapped one, which is what keeps
        // an uncarved fragment (and every fragment at `Off`) byte-identical.
        // `above_plane` is hoisted because the self-shadow gate below needs it
        // too: `surface_depth_light_visibility` returns 1.0 immediately when the
        // light is at or below the geometric plane, so without this term a
        // below-plane light on a TOP hit whose normal map tilts toward it passes
        // `contributes`, spends one of the two budget slots, and gets "lit" back
        // — leaving a later light that genuinely casts a shadow unmarched.
        let above_plane = dot(mesh_n, L) > 0.0;
        let plane_lit = depth.hit_top || above_plane;
        let n_dot_l = select(0.0, dot(n, L), plane_lit);
        if pixel_reached {
            var depth_visibility = 1.0;
            // Spend the self-shadow budget only on lights that actually reach
            // this fragment. A spot in range but aimed elsewhere, or a scripted
            // light whose descriptor is present but inactive, has zero
            // attenuation or zero color: marching for it burns one of the two
            // slots on a result that is then multiplied by zero, and leaves the
            // light that genuinely casts the shadow unshadowed.
            let contributes = n_dot_l > 0.0
                && above_plane
                && attenuation > 0.0
                && dot(effective_color, vec3<f32>(1.0)) > 0.0;
            if depth.carved
                && contributes
                && i < kinematic_light_params.dynamic_light_count
                && depth_shadow_marches < depth.shadow_light_budget {
                depth_shadow_marches = depth_shadow_marches + 1u;
                depth_visibility = surface_depth_light_visibility(depth, L);
            }
            total = total + effective_color * attenuation * max(n_dot_l, 0.0) * depth_visibility;
        }

        // Animated direct tails are diffuse-only. Selected-static records begin
        // after the descriptor-bearing prefix. Smooth style keeps the per-pixel
        // evaluation and its gates exactly; pixel style evaluates the texel's
        // point. Either way the term adds right after this light's diffuse.
        if use_specular && i >= kinematic_light_params.scripted_light_count {
            if smooth_style {
                if n_dot_l > 0.0 {
                    total = total
                        + blinn_phong(L, spec_point.view, n, effective_color, spec_exp, spec_int) * attenuation;
                }
            } else {
                total = total + pretint_mover_texel_specular(
                    light,
                    effective_color,
                    effective_aim,
                    spec_point,
                    n,
                    mesh_n,
                    depth.hit_top,
                    shadow,
                    spec_exp,
                    spec_int,
                );
            }
        }
    }
    return total;
}
