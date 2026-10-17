// Cell-rate spike (debug only, never merged): the pixel reflection style
// evaluated once per water texel cell in compute into a cell atlas, which the
// reflecting fragment then reads instead of evaluating per fragment.
// Appended to fluid_surface.wgsl; reuses its reflection functions unchanged.
// See: context/lib/rendering_pipeline.md §7.3a (Reflection, Style)

// Set per pipeline. CELLS: the reflecting fragment reads its cell's atlas
// texel. RATIO_ONE: the probe term skips the live-lighting ratio (ratio 1).
override FLUID_REFLECTION_CELLS: bool = false;
override FLUID_REFLECTION_RATIO_ONE: bool = false;
override FLUID_CELL_EXPERIMENT: u32 = 0u;

// --- Fragment: the cell atlas (group 4), a 1x1 dummy without one ---
// rgb = the cell's probe capture (ratio applied), a = Fresnel x capture-shadow
// weight. The fog proximity band stays per fragment.
@group(4) @binding(4) var fluid_cell_atlas: texture_2d<f32>;

// --- Compute only (group 5 of the compute layout; the render layout's group 5
// lacks these bindings and vs/fs never touch them) ---
const FLUID_CELL_MAX_FACES: u32 = 512u;
const FLUID_CELL_TILE: u32 = 8u;
const FLUID_CELL_FLAG_CULL: u32 = 1u;
const FLUID_CELL_FLAG_COUNT: u32 = 2u;
// Workgroups per dispatch row; a larger tile list spills into y.
const FLUID_CELL_DISPATCH_ROW: u32 = 65535u;
// Texels per pixel for a cell with no usable screen footprint: flattens it.
const FLUID_CELL_FLAT_TEXELS_PER_PIXEL: f32 = 1.0e6;

// One rect face this frame: the exact affine texel-to-world map at the corner
// of its first cell, its probe and row reflectivity, and where its cells sit.
struct FluidCellFace {
    origin: vec3<f32>,
    probe: u32,
    axis_u: vec3<f32>,
    reflectivity: f32,
    axis_v: vec3<f32>,
    _pad0: u32,
    outward: vec3<f32>,
    _pad1: u32,
    cell_min: vec2<i32>,
    cell_count: vec2<u32>,
    atlas_origin: vec2<u32>,
    _pad2: vec2<u32>,
};
struct FluidCellParams {
    // Scene colour extent in pixels.
    viewport: vec2<f32>,
    // Bit 0 frustum-cull cells, bit 1 count evaluated cells.
    flags: u32,
    face_count: u32,
    faces: array<FluidCellFace, 512>,
};
@group(5) @binding(8) var<uniform> fluid_cell_params: FluidCellParams;
struct FluidCellJobs {
    evaluated: atomic<u32>,
    tile_count: u32,
    _pad: vec2<u32>,
    // (face index, (tile_y << 16) | tile_x) per 8x8 tile.
    tiles: array<vec2<u32>>,
};
@group(5) @binding(9) var<storage, read_write> fluid_cell_jobs: FluidCellJobs;
@group(5) @binding(10) var fluid_cell_atlas_out: texture_storage_2d<rgba16float, write>;

// `fluid_reflection_sample`, or with RATIO_ONE the same capture without the
// live-lighting ratio, whose work is then never evaluated.
fn fluid_reflection_sample_spike(probe: u32, world_position: vec3<f32>, view: vec3<f32>,
                                 outward: vec3<f32>) -> FluidReflectionSample {
    if !FLUID_REFLECTION_RATIO_ONE {
        return fluid_reflection_sample(probe, world_position, view, outward);
    }
    let ray = fluid_reflection_above_horizon(fluid_reflection_mirror(view, outward));
    let layer = fluid_probes.probes[probe].layer;
    let search = fluid_reflection_search(world_position, ray, fluid_probes.probes[probe].box_min,
        fluid_probes.probes[probe].box_max, fluid_probes.probes[probe].capture_point,
        fluid_probes.tile.xy, layer);
    let uv = fluid_reflection_tile_uv(search.lookup, fluid_probes.tile.x, fluid_probes.tile.y);
    let capture = textureSampleLevel(fluid_probe_array, sh_atlas_sampler, uv, layer, 0.0).rgb;
    return FluidReflectionSample(capture, search.weight);
}

// The clip-space planes a point lies outside (wgpu depth 0..1).
fn fluid_cell_outside_bits(c: vec4<f32>) -> u32 {
    var bits = 0u;
    if c.x < -c.w {
        bits = bits | 1u;
    }
    if c.x > c.w {
        bits = bits | 2u;
    }
    if c.y < -c.w {
        bits = bits | 4u;
    }
    if c.y > c.w {
        bits = bits | 8u;
    }
    if c.z < 0.0 {
        bits = bits | 16u;
    }
    if c.z > c.w {
        bits = bits | 32u;
    }
    return bits;
}

// The fragment's max(length(ddx * dims), length(ddy * dims)), analytic at the
// cell centre: the inverse of the texel-to-pixel Jacobian, column lengths.
fn fluid_cell_texels_per_pixel(centre: vec3<f32>, axis_u: vec3<f32>, axis_v: vec3<f32>,
                               viewport: vec2<f32>) -> f32 {
    let c = uniforms.view_proj * vec4<f32>(centre, 1.0);
    let du = uniforms.view_proj * vec4<f32>(axis_u, 0.0);
    let dv = uniforms.view_proj * vec4<f32>(axis_v, 0.0);
    if c.w <= 0.0 {
        return FLUID_CELL_FLAT_TEXELS_PER_PIXEL;
    }
    let half_viewport = 0.5 * viewport;
    let w2 = c.w * c.w;
    let jpu = (du.xy * c.w - c.xy * du.w) / w2 * half_viewport;
    let jpv = (dv.xy * c.w - c.xy * dv.w) / w2 * half_viewport;
    let det = jpu.x * jpv.y - jpv.x * jpu.y;
    let scale = dot(jpu, jpu) * dot(jpv, jpv);
    if det * det <= FLUID_REFLECTION_AXES_EPSILON * scale || scale <= 0.0 {
        return FLUID_CELL_FLAT_TEXELS_PER_PIXEL;
    }
    let along_x = vec2<f32>(jpv.y, -jpu.y) / det;
    let along_y = vec2<f32>(-jpv.x, jpu.x) / det;
    return max(length(along_x), length(along_y));
}

// One thread per cell of one 8x8 tile: the pixel style's reflection at the
// cell's exact centre, through the same functions the fragment calls.
@compute @workgroup_size(8, 8)
fn cs_fluid_reflection_cells(@builtin(workgroup_id) workgroup: vec3<u32>,
                             @builtin(local_invocation_id) local_id: vec3<u32>) {
    let index = workgroup.y * FLUID_CELL_DISPATCH_ROW + workgroup.x;
    if index >= fluid_cell_jobs.tile_count {
        return;
    }
    let tile = fluid_cell_jobs.tiles[index];
    let face = fluid_cell_params.faces[min(tile.x, FLUID_CELL_MAX_FACES - 1u)];
    let local = vec2<u32>((tile.y & 0xFFFFu) * FLUID_CELL_TILE + local_id.x,
        (tile.y >> 16u) * FLUID_CELL_TILE + local_id.y);
    if local.x >= face.cell_count.x || local.y >= face.cell_count.y {
        return;
    }
    let lf = vec2<f32>(local);
    if (fluid_cell_params.flags & FLUID_CELL_FLAG_CULL) != 0u {
        let corner = face.origin + face.axis_u * lf.x + face.axis_v * lf.y;
        let outside = fluid_cell_outside_bits(uniforms.view_proj * vec4<f32>(corner, 1.0))
            & fluid_cell_outside_bits(uniforms.view_proj * vec4<f32>(corner + face.axis_u, 1.0))
            & fluid_cell_outside_bits(uniforms.view_proj * vec4<f32>(corner + face.axis_v, 1.0))
            & fluid_cell_outside_bits(
                uniforms.view_proj * vec4<f32>(corner + face.axis_u + face.axis_v, 1.0));
        if outside != 0u {
            return;
        }
    }
    if (fluid_cell_params.flags & FLUID_CELL_FLAG_COUNT) != 0u {
        atomicAdd(&fluid_cell_jobs.evaluated, 1u);
    }
    let centre = face.origin + face.axis_u * (lf.x + 0.5) + face.axis_v * (lf.y + 0.5);
    let cell = face.cell_min + vec2<i32>(local);
    let axes = FluidTexelAxes(face.axis_u, face.axis_v);
    let texels_per_pixel = fluid_cell_texels_per_pixel(centre, face.axis_u, face.axis_v,
        fluid_cell_params.viewport);
    let point = fluid_reflection_point(FLUID_REFLECTION_STYLE_PIXEL,
        vec2<f32>(cell) + vec2<f32>(0.5), centre, face.outward, axes, uniforms.time,
        texels_per_pixel, fluid_probes.probes[face.probe].box_min,
        fluid_probes.probes[face.probe].box_max, fluid_probes.probes[face.probe].capture_point);
    let view = normalize(uniforms.camera_position - point.position);
    let reflection = fluid_reflection_sample_spike(face.probe, point.position, view, point.normal);
    let fresnel = fluid_reflection_fresnel(dot(view, face.outward), face.reflectivity)
        * reflection.weight;
    textureStore(fluid_cell_atlas_out, vec2<i32>(face.atlas_origin + local),
        vec4<f32>(reflection.capture, fresnel));
}

// Two-draw cells: the water draws through the reflection-free pipeline, then
// this thin overlay draw adds the cell's reflection with a premultiplied blend
// (One, OneMinusSrcAlpha). The single-draw composite factors exactly:
// F*R + (1 - F)*(a*S + (1 - a)*dst) is the surface over the scene, then
// (F*R, F) over that. The lighting shader stays the reflection-free one, so
// the atlas read never shares a shader with the lighting loop.
@fragment
fn fs_cells_overlay(in: VertexOutput) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(base_texture, 0));
    let V = normalize(uniforms.camera_position - in.world_position);
    let outward = normalize(in.world_normal);
    let reflectivity = fluid_volumes.slots[in.slot].surface.z;
    if !fluid_reflection_gate(in.probe, outward, V, uniforms.light_term_mask, reflectivity) {
        return vec4<f32>(0.0);
    }
    let key = clamp(vec2<i32>(floor(in.uv * dims)), in.cell_bounds.xy, in.cell_bounds.zw);
    let cell = textureLoad(fluid_cell_atlas, key + in.cell_offset, 0);
    let fresnel = cell.a * fluid_fog_proximity(uniforms.camera_position, in.world_position, outward);
    return vec4<f32>(cell.rgb * fresnel, fresnel);
}

// The cells pipeline's own fragment entry: fs_main's surface half copied
// verbatim (generated from it; keep the two identical), then the atlas read in
// place of the per-fragment reflection. fs_main also reads the atlas under
// FLUID_REFLECTION_CELLS, but there the per-fragment path still follows the
// early return in the same function; on the 1660 (Vulkan) that variant kept
// most of the reflecting variant's cost, the documented "mere presence" effect
// (rendering_pipeline.md section 7.3a, Two pipelines). This entry holds no
// per-fragment reflection code at all.
@fragment
fn fs_cells(in: VertexOutput) -> @location(0) vec4<f32> {
    // Derivatives in uniform control flow, from the unwarped UV, and the
    // world position's for the pixel reflection style's texel axes.
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);
    let ddx_world = dpdx(in.world_position);
    let ddy_world = dpdy(in.world_position);
    let dims = vec2<f32>(textureDimensions(base_texture, 0));
    let texels_per_pixel = max(length(ddx * dims), length(ddy * dims));
    let row = fluid_volumes.slots[in.slot];
    let uv = fluid_warp_uv(in.uv, in.drift_scroll, dims, row.wave, uniforms.time, texels_per_pixel);
    let albedo = sample_post_retro(base_texture, aniso_sampler, uv, ddx, ddy);

    // Two-sided: shade the side the camera sees. For a planar face this is
    // exactly the back-face test, and it holds whatever the winding. Fog reads
    // the outward normal, never this viewer-flipped one.
    let V = normalize(uniforms.camera_position - in.world_position);
    let outward = normalize(in.world_normal);
    var n = outward;
    if dot(n, V) < 0.0 {
        n = -n;
    }

    const LIGHT_TERM_AMBIENT_FLOOR: u32 = 0x01u;
    const LIGHT_TERM_INDIRECT_STATIC: u32 = 0x02u;
    const LIGHT_TERM_INDIRECT_ANIMATED: u32 = 0x04u;
    const LIGHT_TERM_DYNAMIC_DIRECT: u32 = 0x20u;
    let light_terms = uniforms.light_term_mask;
    let use_ambient_floor = (light_terms & LIGHT_TERM_AMBIENT_FLOOR) != 0u;
    let use_indirect = (light_terms & (LIGHT_TERM_INDIRECT_STATIC | LIGHT_TERM_INDIRECT_ANIMATED)) != 0u;
    let use_dynamic = (light_terms & LIGHT_TERM_DYNAMIC_DIRECT) != 0u;

    // Baked terms. Static/animated isolation happens in the compose passes;
    // the composed direct atlas already has promoted static lights removed
    // while dynamic direct is on, so the loop below adds them back.
    let sh = fluid_sample_sh(in.world_position, n, has_direct_sh());
    var lighting = select(vec3<f32>(0.0), vec3<f32>(uniforms.ambient_floor), use_ambient_floor);
    if use_indirect {
        lighting = lighting + sh.a * uniforms.indirect_scale;
    }
    lighting = lighting + sh.b * uniforms.direct_scale;

    // Runtime lights: the dynamic prefix, then the promoted-static suffix. The
    // mover-only animated-baked tail between them is skipped; fluid takes those
    // lights from the composed direct atlas, as billboards do.
    if use_dynamic {
        for (var i: u32 = 0u; i < uniforms.light_count; i = i + 1u) {
            lighting = lighting + fluid_light(i, in.world_position, n, true);
        }
        let promoted_begin = min(
            uniforms.light_count + (uniforms.has_direct >> 1u),
            uniforms.total_light_count,
        );
        for (var i: u32 = promoted_begin; i < uniforms.total_light_count; i = i + 1u) {
            lighting = lighting + fluid_light(i, in.world_position, n, false);
        }
    }

    var rgb = albedo.rgb * lighting;
    if material.emissive_strength > 0.0 {
        rgb = rgb + sample_post_retro(emissive_texture, aniso_sampler, uv, ddx, ddy).rgb
            * material.emissive_strength;
    }
    let opacity = clamp(row.surface.x, 0.0, 1.0);
    let alpha = albedo.a * opacity;
    // Seen from outside a fogged fluid, the scene behind the face lies under
    // it: composite surface over fog over scene in one output. Clear fluids
    // and faces seen from inside keep exactly the surface.
    var surface = vec4<f32>(rgb, alpha);
    let extinction = row.surface.y;
    if extinction > 0.0 && fluid_fog_seen_from_outside(outward, V) {
        let depth = textureLoad(scene_depth, vec2<i32>(in.clip_position.xy), 0);
        let path = fluid_fog_path(uniforms.view_proj, uniforms.camera_position, in.world_position,
            depth);
        let transmittance = fluid_fog_transmittance(extinction, path);
        let proximity = fluid_fog_proximity(uniforms.camera_position, in.world_position, outward);
        let fog_opacity = fluid_fog_layer_opacity(fluid_scene.fog_weight, proximity, transmittance);
        surface = fluid_fog_composite(rgb, alpha, row.fog_albedo.rgb * lighting, fog_opacity);
    }
    // The reflection-free variant ends here; the block below folds away.
    if !FLUID_REFLECTIONS {
        return surface;
    }
    // A probe-bound face seen from above under the all-terms mask mirrors its
    // probe over that at union coverage; every other fragment is unchanged.
    // The reflection starts from the style's point and sees the eye from there.
    let reflectivity = row.surface.z;
    if !fluid_reflection_gate(in.probe, outward, V, uniforms.light_term_mask, reflectivity) {
        return surface;
    }
    // Diagnostic (env POSTRETRO_SPIKE_CELL_EXPERIMENT at pipeline build):
    // 1 = the surface alone past the gate, 2 = a constant cell, no atlas read.
    if FLUID_CELL_EXPERIMENT == 1u {
        return surface;
    }
    if FLUID_CELL_EXPERIMENT == 2u {
        return fluid_reflection_composite(surface, vec3<f32>(0.1), 0.05);
    }
    // The cell's reflection, evaluated once in compute: the only probe work
    // in this entry point.
    let key = clamp(vec2<i32>(floor(in.uv * dims)), in.cell_bounds.xy, in.cell_bounds.zw);
    let cell = textureLoad(fluid_cell_atlas, key + in.cell_offset, 0);
    let cell_band = fluid_fog_proximity(uniforms.camera_position, in.world_position, outward);
    return fluid_reflection_composite(surface, cell.rgb, cell.a * cell_band);
}
