// FROZEN: the mover pass's runtime light loop as kinematic_brush.wgsl evaluated
// it at 1c7fc9e3a, the build before specular-texel-sharpness. Never edit: it is
// the byte-identity oracle for `kinematic_specular_harness.rs` (smooth style,
// untinted, no sharpness). Regenerate only by re-extracting from that commit
// (`git show 1c7fc9e3a:<path>`), prefixing the frozen identifiers `frozen_` /
// `FROZEN_`. Sources: kinematic_brush.wgsl (`accumulate_dynamic_direct`,
// `SHADOWMASK_META_VEC4S_PER_RECORD`), material_shading.wgsl (`blinn_phong`),
// light_falloff.wgsl (`light_eval_falloff`) and light_eval.wgsl
// (`light_eval_cone_attenuation`). The loop's bindings, its shadow samplers
// and the helpers specular-texel-sharpness leaves alone (animation curves, the Surface Depth
// self-shadow march) resolve from the harness by name.

const FROZEN_SHADOWMASK_META_VEC4S_PER_RECORD: u32 = 2u;

fn frozen_accumulate_dynamic_direct(
    world_pos: vec3<f32>,
    n: vec3<f32>,
    mesh_n: vec3<f32>,
    V: vec3<f32>,
    spec_exp: f32,
    spec_int: f32,
    use_dynamic: bool,
    use_specular: bool,
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
                + promoted_index * FROZEN_SHADOWMASK_META_VEC4S_PER_RECORD;
            if meta_index + 1u < arrayLength(&light_influence) {
                cache_layer = i32(light_influence[meta_index + 1u].w);
            }
        }
        let influence = light_influence[i];
        let inf_radius = influence.w;
        if inf_radius <= 1.0e30 {
            let d = world_pos - influence.xyz;
            if dot(d, d) > inf_radius * inf_radius {
                continue;
            }
        }

        let light = lights[i];
        let light_type = bitcast<u32>(light.position_and_type.w);
        let falloff_model = bitcast<u32>(light.color_and_falloff_model.w);

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

        var L: vec3<f32>;
        var attenuation: f32;
        switch light_type {
            case 0u: {
                // The DISPLACED hit position feeds direction and attenuation;
                // the shadow-map lookups below stay on the true plane, which is
                // the geometry the depth maps hold and their bias is tuned for.
                let to_light = light.position_and_type.xyz - depth.world_position;
                let dist = length(to_light);
                L = to_light / max(dist, 0.0001);
                attenuation = frozen_light_eval_falloff(dist, light.direction_and_range.w, falloff_model);
                let cube_slot = bitcast<u32>(light.cone_angles_and_pad.w);
                if cube_slot != 0xFFFFFFFFu {
                    var shadow: f32;
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
                // Displaced for direction and attenuation, true plane for the
                // shadow map — same split as the point case above.
                let to_light = light.position_and_type.xyz - depth.world_position;
                let dist = length(to_light);
                L = to_light / max(dist, 0.0001);
                let dist_falloff = frozen_light_eval_falloff(dist, light.direction_and_range.w, falloff_model);
                let cone = frozen_light_eval_cone_attenuation(L, effective_aim, light.cone_angles_and_pad.x, light.cone_angles_and_pad.y);
                attenuation = dist_falloff * cone;
                let slot_index = bitcast<u32>(light.cone_angles_and_pad.z);
                if slot_index != 0xFFFFFFFFu {
                    var shadow: f32;
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
            default: {
                L = -effective_aim;
                attenuation = 1.0;
            }
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
        var depth_visibility = 1.0;
        // Spend the self-shadow budget only on lights that actually reach this
        // fragment. A spot in range but aimed elsewhere, or a scripted light
        // whose descriptor is present but inactive, has zero attenuation or
        // zero color: marching for it burns one of the two slots on a result
        // that is then multiplied by zero, and leaves the light that genuinely
        // casts the shadow unshadowed.
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

        // Animated direct tails are diffuse-only. Selected-static records begin
        // after the descriptor-bearing prefix and retain their existing mover
        // specular path.
        if use_specular && i >= kinematic_light_params.scripted_light_count && n_dot_l > 0.0 {
            total = total + frozen_blinn_phong(L, V, n, effective_color, spec_exp, spec_int) * attenuation;
        }
    }
    return total;
}

fn frozen_blinn_phong(L: vec3<f32>, V: vec3<f32>, N: vec3<f32>,
               color: vec3<f32>, spec_exp: f32, spec_int: f32) -> vec3<f32> {
    let H = normalize(L + V);
    let NdH = max(dot(N, H), 0.0);
    return color * pow(NdH, spec_exp) * spec_int;
}

fn frozen_light_eval_falloff(distance: f32, range: f32, model: u32) -> f32 {
    switch model {
        case 0u: {
            let r = max(range, 0.001);
            return max(1.0 - distance / r, 0.0);
        }
        case 1u: {
            let r = max(range, 0.0001);
            if distance > r {
                return 0.0;
            }
            return 1.0 / max(distance, 0.0001);
        }
        case 2u: {
            let r = max(range, 0.0001);
            if distance > r {
                return 0.0;
            }
            let d2 = max(distance * distance, 0.0001);
            return 1.0 / d2;
        }
        default: {
            return 0.0;
        }
    }
}

fn frozen_light_eval_cone_attenuation(L: vec3<f32>, aim: vec3<f32>, inner_angle: f32, outer_angle: f32) -> f32 {
    let cos_angle = dot(-L, aim);
    let cos_inner = cos(inner_angle);
    let cos_outer = cos(outer_angle);
    return smoothstep(cos_outer, cos_inner, cos_angle);
}
