// Glossy overlay shading inputs: where a glossy fragment reflects from and
// about which normal. Forward's own Surface Depth march over forward's own
// inputs picks the texel forward shaded, the normal map and the march's face
// normal give the normal the view mirrors about, and the reflection style
// picks the point: smooth the fragment, pixel the albedo texel's centre
// (the texel-centre highlight's point, rendering_pipeline.md §4). Declares no
// bindings: it reads glossy_overlay.wgsl's `uniforms`, `base_texture`,
// `t_normal` and, through surface_depth.wgsl, `spec_texture` and `material`.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces), §7.3 (Surface Depth)

// The fragment's screen derivatives, taken once in uniform control flow, as
// forward takes them at the top of `forward_shade`.
struct GlossyDerivatives {
    ddx: vec2<f32>,
    ddy: vec2<f32>,
    ddx_world: vec3<f32>,
    ddy_world: vec3<f32>,
};

fn glossy_overlay_derivatives(in: GlossyVertexOutput) -> GlossyDerivatives {
    return GlossyDerivatives(dpdx(in.uv), dpdy(in.uv), dpdx(in.world_position),
        dpdy(in.world_position));
}

// Forward's Surface Depth march at this fragment, over forward's inputs: the
// interpolated UV, position and normal, the eye vector and its length, and
// the same derivatives, so the overlay reads the texel forward shaded. A flat
// material, or the player's switch at Off (which zeroes the material's
// Surface Depth words), returns the fragment's own UV, position and normal
// without marching; a live switch reaches both passes through the one
// material uniform on the same frame.
fn glossy_overlay_march(in: GlossyVertexOutput, d: GlossyDerivatives) -> SurfaceDepthResult {
    let mesh_n = normalize(in.world_normal);
    let view_vector = uniforms.camera_position - in.world_position;
    return surface_depth_resolve(in.uv, in.world_position, mesh_n, normalize(view_vector),
        length(view_vector), d.ddx, d.ddy, d.ddx_world, d.ddy_world);
}

// The fragment's Surface Depth result in a pipeline built with or without the
// march. A material that cannot march at any switch state (render-cpu
// `MaterialUniformPlan::can_march`) draws through the march-free pipeline:
// the march's own first early-out returns this same flat result for it, so
// the output is identical while that pipeline carries none of the march's
// registers.
fn glossy_overlay_depth(in: GlossyVertexOutput, d: GlossyDerivatives,
                        march: bool) -> SurfaceDepthResult {
    if march {
        return glossy_overlay_march(in, d);
    }
    return surface_depth_flat(in.uv, in.world_position, normalize(in.world_normal));
}

// The tangent-space normal the overlay bends by: the material's normal map,
// sampled as forward samples it, or, for a material that binds the 1x1 neutral
// placeholder (render-side `LoadedTexture::normal_mapped` false), that one
// texel read without a filter and decoded as `sample_normal` decodes it. Every
// sample of a one-texel map returns its texel, so both read one value; the
// load spares the filtered sample's cost.
fn glossy_overlay_tangent_normal(uv: vec2<f32>, d: GlossyDerivatives,
                                 normal_map: bool) -> vec3<f32> {
    if normal_map {
        return sample_normal(t_normal, uv, d.ddx, d.ddy);
    }
    let rg = textureLoad(t_normal, vec2<i32>(0), 0).rg * 2.0 - 1.0;
    let z = sqrt(max(0.0, 1.0 - dot(rg, rg)));
    return normalize(vec3<f32>(rg, z));
}

// Where the reflection starts and how it turns. `origin` is the search's
// start, on the true plane; `view` the unit vector from it to the eye;
// `normal` the normal the view mirrors about; `outward` the flat face normal,
// which keeps Fresnel, the front-side gate and the hemisphere per face.
struct GlossySurface {
    origin: vec3<f32>,
    view: vec3<f32>,
    normal: vec3<f32>,
    outward: vec3<f32>,
};

// The surface a texel of nonzero weight reflects from. The normal is
// forward's shading normal: the march's face normal on a side hit, the normal
// map's on a top (or on a flat material). Pixel style starts every fragment
// of a texel at its texel-centre point on the true plane, mapped through the
// fragment's UV->world Jacobian as forward's highlight is, so the texel reads
// one search, one sample and one weight; smooth starts at the fragment.
fn glossy_overlay_surface(in: GlossyVertexOutput, d: GlossyDerivatives,
                          depth: SurfaceDepthResult, pixel: bool,
                          normal_map: bool) -> GlossySurface {
    let mesh_n = normalize(in.world_normal);
    let n_ts = glossy_overlay_tangent_normal(depth.uv, d, normal_map);
    let n_bump = reconstruct_tbn_normal(mesh_n, in.world_tangent, in.bitangent_sign, n_ts);
    let normal = select(depth.normal, n_bump, depth.hit_top);
    let view = normalize(uniforms.camera_position - in.world_position);
    let point = specular_texel_point(!pixel, SpecularPoint(in.world_position, view), in.uv,
        depth.uv, in.world_position, uniforms.camera_position,
        vec2<f32>(textureDimensions(base_texture, 0)), d.ddx, d.ddy, d.ddx_world, d.ddy_world);
    return GlossySurface(point.position, point.view, normal, mesh_n);
}
