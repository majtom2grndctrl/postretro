// Shared lightmap helpers (binding-agnostic): the vertex-stage cell-block
// resolve, and the static, animated and shadowmask pool samplers.
// See: context/lib/rendering_pipeline.md §4, §8
//
// `resolve_lightmap_block`, `sample_lightmap_irradiance`,
// `sample_lightmap_direction`, `sample_lightmap_animated`,
// `animated_block_uv` and `sample_shadowmask_atlas`. These helpers declare no
// bindings: the consumer declares the group-4 lightmap textures
// (`lightmap_irradiance`, `lightmap_direction`, `animated_lm_atlas`,
// `shadowmask_atlas`), the samplers (`lightmap_sampler`,
// `lightmap_filtering_sampler`), the `animated_block_table` uniform with its
// `AnimatedBlockTable` struct, and the group-6 `lightmap_block_table` before
// this file is textually concatenated. The helpers reference those
// consumer-declared globals by lexical resolution — the same precedent as
// `shadow_sample.wgsl`.

// Pool layer edge in texels: `LIGHTMAP_POOL_LAYER_EDGE` in level-format. The
// 1×1 placeholders read the same texel at any coordinate, so placeholder mode
// needs no second edge.
const LIGHTMAP_POOL_LAYER_EDGE: f32 = 2048.0;
// Block-table entry flags: `BLOCK_FLAG_*` in render-cpu `lightmap_pool`. They
// ride the high 16 bits of the flat `lightmap_layer_flags` varying.
const LIGHTMAP_BLOCK_RESIDENT: u32 = 1u;
const LIGHTMAP_BLOCK_NONE: u32 = 2u;

// What the vertex stage hands the fragment stage for one vertex's block.
struct LightmapBlockVaryings {
    // Block-local texel: the block-local UV times the block extent.
    texel: vec2<f32>,
    // Pool layer (low 16 bits) and entry flags (high 16 bits).
    layer_flags: u32,
    // Pool offset (xy) and block extent (zw), in pool texels.
    rect: vec4<u32>,
};

// Resolve a vertex's cell block (block id + 1, 0 = no lightmap) through the
// block table: one table fetch per vertex. An id past the table reads entry
// 0, so placeholder mode's one-entry table serves every vertex.
fn resolve_lightmap_block(block: u32, uv: vec2<f32>) -> LightmapBlockVaryings {
    let index = select(0u, block, block < arrayLength(&lightmap_block_table));
    let entry = lightmap_block_table[index];
    let extent = vec2<u32>(entry.z & 0xffffu, entry.z >> 16u);
    var out: LightmapBlockVaryings;
    out.texel = uv * vec2<f32>(extent);
    out.layer_flags = (entry.x & 0xffffu) | (entry.w << 16u);
    out.rect = vec4<u32>(entry.y & 0xffffu, entry.y >> 16u, extent);
    return out;
}

fn lightmap_block_layer(layer_flags: u32) -> u32 {
    return layer_flags & 0xffffu;
}

fn lightmap_block_resident(layer_flags: u32) -> bool {
    return ((layer_flags >> 16u) & LIGHTMAP_BLOCK_RESIDENT) != 0u;
}

// A real block whose texels are not in the pool: a transient miss.
fn lightmap_block_missing(layer_flags: u32) -> bool {
    return ((layer_flags >> 16u) & (LIGHTMAP_BLOCK_RESIDENT | LIGHTMAP_BLOCK_NONE)) == 0u;
}

// Normalized pool UV of a block-local texel: one offset, with the texel
// clamped to the block's half-texel rect so bilinear taps never reach a
// neighbouring block. The direction pool sits at edge / scale with offsets at
// offset / scale, so this UV addresses it too; nearest sampling inside the
// clamped rect stays on the block's own direction texels.
fn lightmap_pool_uv(texel: vec2<f32>, rect: vec4<u32>) -> vec2<f32> {
    let extent = vec2<f32>(rect.zw);
    let local = clamp(texel, vec2<f32>(0.5), extent - vec2<f32>(0.5));
    return (vec2<f32>(rect.xy) + local) / LIGHTMAP_POOL_LAYER_EDGE;
}

// Static irradiance through the linear sampler at binding 4. Zero for the
// no-lightmap entry and for a missed block. Pool textures carry one mip, so
// the explicit level reads what implicit-derivative sampling would, and stays
// valid under the per-block branch.
fn sample_lightmap_irradiance(texel: vec2<f32>, layer_flags: u32, rect: vec4<u32>) -> vec3<f32> {
    if !lightmap_block_resident(layer_flags) {
        return vec3<f32>(0.0);
    }
    return textureSampleLevel(
        lightmap_irradiance,
        lightmap_filtering_sampler,
        lightmap_pool_uv(texel, rect),
        i32(lightmap_block_layer(layer_flags)),
        0.0,
    ).rgb;
}

// Static dominant direction through the nearest sampler at binding 2 (oct
// directions must not lerp). A block with no texels reads the neutral +Y.
fn sample_lightmap_direction(texel: vec2<f32>, layer_flags: u32, rect: vec4<u32>) -> vec4<f32> {
    if !lightmap_block_resident(layer_flags) {
        return vec4<f32>(0.5, 1.0, 0.5, 0.0);
    }
    return textureSampleLevel(
        lightmap_direction,
        lightmap_sampler,
        lightmap_pool_uv(texel, rect),
        i32(lightmap_block_layer(layer_flags)),
        0.0,
    );
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
// block-local lightmap texel lands in the compact atlas: an integer texel
// translation (compact − block-local) plus a page. Ids past the table resolve
// to none, so an inactive atlas (empty table) never samples. The page size is
// a power of two, so the divide is exact in f32; the integer offset can round
// the sub-texel fraction by at most ~2^-11 texel, far below one 8-bit step, so
// the bilinear footprint lands on the same texels as in the static block.
fn animated_block_uv(block_texel: vec2<f32>, block_id: u32) -> AnimatedBlockUv {
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
    let texel = block_texel + offset;
    out.uv = texel / f32(animated_block_table.page_size);
    out.page = entry.y;
    out.found = true;
    return out;
}

// Each pool layer holds two BC5 mask groups side by side: slots 0/1 in the
// left half, 2/3 in the right, a block's group B at `edge + x`. Returns the
// four slots in one vector. The no-lightmap entry reads all-visible; a missed
// block reads zero, so the union subtracts nothing (and shadowmask-gated
// specular drops, see `shadowmask_visibility_for_spec_light`). A rejected or
// absent shadowmask binds a one-layer, two-texel white texture; clamping the
// pool layer keeps that fallback on its fully-visible layer.
fn sample_shadowmask_atlas(texel: vec2<f32>, layer_flags: u32, rect: vec4<u32>) -> vec4<f32> {
    let flags = layer_flags >> 16u;
    if (flags & LIGHTMAP_BLOCK_NONE) != 0u {
        return vec4<f32>(1.0);
    }
    if (flags & LIGHTMAP_BLOCK_RESIDENT) == 0u {
        return vec4<f32>(0.0);
    }
    let last_layer = textureNumLayers(shadowmask_atlas) - 1u;
    let safe_layer = min(lightmap_block_layer(layer_flags), last_layer);
    // The block's half-texel clamp (U and V) keeps each group's bilinear taps
    // inside the block's own texels in its own half: never across the seam,
    // never into a neighbouring block.
    let uv = lightmap_pool_uv(texel, rect);
    let group0 = textureSampleLevel(
        shadowmask_atlas,
        lightmap_filtering_sampler,
        vec2<f32>(uv.x * 0.5, uv.y),
        i32(safe_layer),
        0.0,
    );
    let group1 = textureSampleLevel(
        shadowmask_atlas,
        lightmap_filtering_sampler,
        vec2<f32>((1.0 + uv.x) * 0.5, uv.y),
        i32(safe_layer),
        0.0,
    );
    return vec4<f32>(group0.rg, group1.rg);
}
