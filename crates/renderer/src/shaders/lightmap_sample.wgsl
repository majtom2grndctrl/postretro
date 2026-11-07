// Shared lightmap helpers (binding-agnostic): vertex-stage block resolve and pool samplers.
// See: context/lib/rendering_pipeline.md §4, §8

// These helpers declare no bindings. The consumer declares the group-4
// lightmap textures (`lightmap_irradiance`, `lightmap_direction`,
// `animated_lm_atlas`, `shadowmask_atlas`), the samplers (`lightmap_sampler`,
// `lightmap_filtering_sampler`), the `animated_block_table` uniform with its
// `AnimatedBlockTable` struct, and the group-6 `lightmap_block_table` before
// this file is concatenated; the helpers read those globals by name, as
// `shadow_sample.wgsl` does.

// Pool layer edge in texels: `LIGHTMAP_POOL_LAYER_EDGE` in level-format. The
// 1×1 placeholders read the same texel at any coordinate, so placeholder mode
// needs no second edge.
const LIGHTMAP_POOL_LAYER_EDGE: f32 = 2048.0;
// Block-table entry flags: `BLOCK_FLAG_*` in render-cpu `lightmap_pool`. They
// ride the high 16 bits of the flat `lightmap_layer_flags` varying.
const LIGHTMAP_BLOCK_RESIDENT: u32 = 1u;
const LIGHTMAP_BLOCK_NONE: u32 = 2u;
const LIGHTMAP_BLOCK_PLANE_A: u32 = 4u;
const LIGHTMAP_BLOCK_PLANE_B: u32 = 8u;
const LIGHTMAP_BLOCK_NO_MASK_POOL: u32 = 16u;
// A pool address word: offset x/4 (9), offset y/4 (9), layer (14). An entry's
// fourth word: extent w (12), extent h (12), flags (8). `BlockTableEntry` in
// render-cpu `lightmap_pool` packs both.
const LIGHTMAP_ADDRESS_OFFSET_BITS: u32 = 9u;
const LIGHTMAP_ADDRESS_OFFSET_MASK: u32 = 511u;
const LIGHTMAP_ADDRESS_OFFSET_UNIT: u32 = 4u;
const LIGHTMAP_ADDRESS_LAYER_SHIFT: u32 = 18u;
const LIGHTMAP_EXTENT_BITS: u32 = 12u;
const LIGHTMAP_EXTENT_MASK: u32 = 4095u;
const LIGHTMAP_FLAGS_SHIFT: u32 = 24u;

// What the vertex stage hands the fragment stage for one vertex's block.
struct LightmapBlockVaryings {
    // Block-local texel: the block-local UV times the block extent.
    texel: vec2<f32>,
    // Pool layer (low 16 bits) and entry flags (high 16 bits).
    layer_flags: u32,
    // Pool offset (xy) and block extent (zw), in pool texels.
    rect: vec4<u32>,
    // Plane A and plane B pool addresses, still packed: only the mask path
    // decodes them.
    plane_addresses: vec2<u32>,
};

fn lightmap_address_offset(address: u32) -> vec2<u32> {
    return vec2<u32>(
        address & LIGHTMAP_ADDRESS_OFFSET_MASK,
        (address >> LIGHTMAP_ADDRESS_OFFSET_BITS) & LIGHTMAP_ADDRESS_OFFSET_MASK,
    ) * LIGHTMAP_ADDRESS_OFFSET_UNIT;
}

fn lightmap_address_layer(address: u32) -> u32 {
    return address >> LIGHTMAP_ADDRESS_LAYER_SHIFT;
}

// Resolve a vertex's cell block (block id + 1, 0 = no lightmap) through the
// block table: one table fetch per vertex. An id past the table reads entry
// 0, so placeholder mode's one-entry table serves every vertex.
fn resolve_lightmap_block(block: u32, uv: vec2<f32>) -> LightmapBlockVaryings {
    let index = select(0u, block, block < arrayLength(&lightmap_block_table));
    let entry = lightmap_block_table[index];
    let extent = vec2<u32>(
        entry.w & LIGHTMAP_EXTENT_MASK,
        (entry.w >> LIGHTMAP_EXTENT_BITS) & LIGHTMAP_EXTENT_MASK,
    );
    var out: LightmapBlockVaryings;
    out.texel = uv * vec2<f32>(extent);
    out.layer_flags = lightmap_address_layer(entry.x) | ((entry.w >> LIGHTMAP_FLAGS_SHIFT) << 16u);
    out.rect = vec4<u32>(lightmap_address_offset(entry.x), extent);
    out.plane_addresses = entry.yz;
    return out;
}

fn lightmap_block_layer(layer_flags: u32) -> u32 {
    return layer_flags & 0xffffu;
}

fn lightmap_block_resident(layer_flags: u32) -> bool {
    return ((layer_flags >> 16u) & LIGHTMAP_BLOCK_RESIDENT) != 0u;
}

// A real block whose texels are not in the pool: a transient miss. No
// production caller; kept as a test probe for the block-flag states.
fn lightmap_block_missing(layer_flags: u32) -> bool {
    return ((layer_flags >> 16u) & (LIGHTMAP_BLOCK_RESIDENT | LIGHTMAP_BLOCK_NONE)) == 0u;
}

// Normalized pool UV of a block-local texel: one offset, with the texel
// clamped to the block's half-texel rect so bilinear taps never reach a
// neighbouring block. The direction pool sits at edge / scale with offsets at
// offset / scale, so this UV addresses it too; nearest sampling inside the
// clamped rect stays on the block's own direction texels.
//
// The local texel is snapped to 1/256 texel, the subtexel precision bilinear
// filtering guarantees, before the offset is added. The sum then fits f32
// exactly at any offset in the layer, so a block samples the same weights
// wherever the pool places it. Without the snap, the rounding of `offset +
// local` depended on the offset, and a streamed pool (partial, placed in drain
// order) differed from the all-resident pool by 1 LSB on a few pixels.
fn lightmap_pool_uv(texel: vec2<f32>, rect: vec4<u32>) -> vec2<f32> {
    let extent = vec2<f32>(rect.zw);
    let clamped = clamp(texel, vec2<f32>(0.5), extent - vec2<f32>(0.5));
    let local = round(clamped * 256.0) / 256.0;
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

// One plane's two mask channels at its own pool address. The block's
// half-texel clamp keeps bilinear taps inside the plane's own texels, never in
// a neighbouring placement.
fn sample_shadowmask_plane(texel: vec2<f32>, rect: vec4<u32>, address: u32) -> vec2<f32> {
    return textureSampleLevel(
        shadowmask_atlas,
        lightmap_filtering_sampler,
        lightmap_pool_uv(texel, vec4<u32>(lightmap_address_offset(address), rect.zw)),
        i32(lightmap_address_layer(address)),
        0.0,
    ).rg;
}

// A block's four mask channels: plane A holds channels 0/1, plane B 2/3, each
// at its own address in the mask pool. An absent plane samples nothing and
// reads zero, as the all-zero BC5 it would have stored decodes to. The
// no-lightmap entry and a level with no mask pool read all-visible; a missed
// block reads zero. World specular reads neither the no-lightmap entry nor a
// miss, since both drop every owner (`static_spec_owners_readable` in
// forward.wgsl), and a miss leaves the union nothing to subtract.
fn sample_shadowmask_atlas(
    texel: vec2<f32>,
    layer_flags: u32,
    rect: vec4<u32>,
    plane_addresses: vec2<u32>,
) -> vec4<f32> {
    let flags = layer_flags >> 16u;
    if (flags & LIGHTMAP_BLOCK_NONE) != 0u {
        return vec4<f32>(1.0);
    }
    if (flags & LIGHTMAP_BLOCK_RESIDENT) == 0u {
        return vec4<f32>(0.0);
    }
    if (flags & LIGHTMAP_BLOCK_NO_MASK_POOL) != 0u {
        return vec4<f32>(1.0);
    }
    var group_a = vec2<f32>(0.0);
    var group_b = vec2<f32>(0.0);
    if (flags & LIGHTMAP_BLOCK_PLANE_A) != 0u {
        group_a = sample_shadowmask_plane(texel, rect, plane_addresses.x);
    }
    if (flags & LIGHTMAP_BLOCK_PLANE_B) != 0u {
        group_b = sample_shadowmask_plane(texel, rect, plane_addresses.y);
    }
    return vec4<f32>(group_a, group_b);
}
