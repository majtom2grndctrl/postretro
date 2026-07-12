// Shared world-material WGSL helpers for specular and normal-map shading.
// See: context/lib/rendering_pipeline.md

// No `(1-ks)` attenuation, no Fresnel — retro aesthetic wants punchy additive
// highlights, not energy conservation.
fn blinn_phong(L: vec3<f32>, V: vec3<f32>, N: vec3<f32>,
               color: vec3<f32>, spec_exp: f32, spec_int: f32) -> vec3<f32> {
    let H = normalize(L + V);
    let NdH = max(dot(N, H), 0.0);
    return color * pow(NdH, spec_exp) * spec_int;
}

// Normal-map dispatch: BC5 stores only tangent-space (x, y) in RG, so decode
// those (`* 2 - 1`) and reconstruct z = sqrt(1 - x² - y²). Renormalize
// unconditionally — BC5 endpoint quantisation plus bilinear filtering leaves
// the sampled vector slightly off unit length.
fn sample_normal(tex: texture_2d<f32>, uv: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> vec3<f32> {
    let rg = sample_post_retro(tex, aniso_sampler, uv, ddx, ddy).rg * 2.0 - 1.0;
    let z  = sqrt(max(0.0, 1.0 - dot(rg, rg)));
    return normalize(vec3<f32>(rg, z));
}

fn reconstruct_tbn_normal(
    mesh_n: vec3<f32>,
    world_tangent: vec3<f32>,
    bitangent_sign: f32,
    n_ts: vec3<f32>,
) -> vec3<f32> {
    // Degenerate-tangent guard: meshes with collapsed UVs produce zero-length
    // tangents. Skip TBN in that case to avoid NaN propagation.
    const TBN_EPS: f32 = 1.0e-4;
    if dot(world_tangent, world_tangent) >= TBN_EPS * TBN_EPS {
        // Gram-Schmidt: project out mesh_n component so T stays in the tangent plane.
        let T = normalize(world_tangent - mesh_n * dot(world_tangent, mesh_n));
        let B = cross(mesh_n, T) * bitangent_sign;
        let TBN = mat3x3<f32>(T, B, mesh_n);
        let n_ts_world = TBN * n_ts;
        if dot(n_ts_world, n_ts_world) >= TBN_EPS * TBN_EPS {
            return normalize(n_ts_world);
        }
    }
    return mesh_n;
}
