// Shared static and animated lightmap and shadowmask atlas sampling helpers
// (binding-agnostic).
// See: context/lib/rendering_pipeline.md §4, §8
//
// `sample_lightmap_irradiance`, `sample_lightmap_animated`, `animated_block_uv`
// and `sample_shadowmask_atlas`. These helpers declare no bindings: the
// consumer declares the group-4 lightmap textures (`lightmap_irradiance`,
// `animated_lm_atlas`, `shadowmask_atlas`), the linear sampler
// (`lightmap_filtering_sampler`) and the `animated_block_table` uniform with its
// `AnimatedBlockTable` struct before this file is textually concatenated. The
// helpers reference those consumer-declared globals by lexical resolution — the
// same precedent as `shadow_sample.wgsl`.

// Sample the irradiance atlas with hardware bilinear filtering through the
// linear sampler at binding 4. `layer` selects the atlas array slice.
fn sample_lightmap_irradiance(uv: vec2<f32>, layer: u32) -> vec3<f32> {
    return textureSample(lightmap_irradiance, lightmap_filtering_sampler, uv, i32(layer)).rgb;
}

// Same for the animated-light contribution atlas.
fn sample_lightmap_animated(uv: vec2<f32>, page: u32) -> vec3<f32> {
    return textureSample(animated_lm_atlas, lightmap_filtering_sampler, uv, i32(page)).rgb;
}

// A face's animated block resolved to compact-atlas UV and page.
struct AnimatedBlockUv {
    uv: vec2<f32>,
    page: u32,
    found: bool,
};

// Resolve a vertex's flat block id (0 = none, n = block n - 1) to where its
// static lightmap UV lands in the compact atlas: a texel translation plus a
// page. Ids past the table resolve to none, so an inactive atlas (empty
// table) never samples. Static and page sizes are powers of two, so the
// scale and divide are exact in f32; the integer offset can round the
// sub-texel fraction by at most ~2^-11 texel, far below one 8-bit step, so
// the bilinear footprint lands on the same texels as in the static layer.
fn animated_block_uv(static_uv: vec2<f32>, block_id: u32) -> AnimatedBlockUv {
    var out: AnimatedBlockUv;
    out.uv = vec2<f32>(0.0);
    out.page = 0u;
    out.found = false;
    if block_id == 0u || block_id > animated_block_table.block_count {
        return out;
    }
    let block = block_id - 1u;
    let packed = animated_block_table.blocks[block / 2u];
    let entry = select(packed.xy, packed.zw, (block & 1u) == 1u);
    let offset = vec2<f32>(
        f32(bitcast<i32>(entry.x << 16u) >> 16u),
        f32(bitcast<i32>(entry.x) >> 16u),
    );
    let texel = static_uv * f32(animated_block_table.static_layer_size) + offset;
    out.uv = texel / f32(animated_block_table.page_size);
    out.page = entry.y;
    out.found = true;
    return out;
}

// The atlas holds two BC5 mask groups side by side in each layer: slots 0/1
// in the left half, 2/3 in the right. Returns the four slots in one vector.
// Rejected or absent shadowmask resources bind a one-layer, two-texel white
// texture. Clamp baked multi-layer vertex indices so that fallback always
// samples that fully-visible layer instead of addressing outside the bound
// texture.
fn sample_shadowmask_atlas(lightmap_uv: vec2<f32>, lightmap_layer: u32) -> vec4<f32> {
    let last_layer = textureNumLayers(shadowmask_atlas) - 1u;
    let safe_layer = min(lightmap_layer, last_layer);
    // Clamping half a group texel inside the group gives each group its own
    // clamp-to-edge, so bilinear taps never blend across the seam.
    let group_half_texel = 1.0 / f32(textureDimensions(shadowmask_atlas).x);
    let group_u = clamp(lightmap_uv.x, group_half_texel, 1.0 - group_half_texel);
    let group0 = textureSample(
        shadowmask_atlas,
        lightmap_filtering_sampler,
        vec2<f32>(group_u * 0.5, lightmap_uv.y),
        i32(safe_layer),
    );
    let group1 = textureSample(
        shadowmask_atlas,
        lightmap_filtering_sampler,
        vec2<f32>((1.0 + group_u) * 0.5, lightmap_uv.y),
        i32(safe_layer),
    );
    return vec4<f32>(group0.rg, group1.rg);
}
