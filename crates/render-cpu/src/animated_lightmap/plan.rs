// CPU half of the animated-lightmap install: section validation, device and
// budget preflight, dispatch-tile expansion, buffer packing and the forward
// block table. The renderer's GPU half only creates what this plan sized.
// See: context/lib/rendering_pipeline.md §4, §7.1

use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::animated_lightmap_atlas::{
    ANIMATED_ATLAS_VRAM_BUDGET_BYTES, ANIMATED_BLOCK_CAP, ANIMATED_BLOCK_TABLE_BYTES_PER_BLOCK,
    ANIMATED_BLOCK_TABLE_HEADER_BYTES, ANIMATED_BLOCK_TABLE_UNIFORM_BYTES,
    animated_atlas_byte_estimate, animated_atlas_fits_budget,
};
use postretro_render_data::geometry::BvhLeaf;

use super::validate_cross_section;
use crate::device_limits::DeviceLimits;

/// Bytes of the forward block-table uniform, the same for every level: the
/// forward shader declares a fixed-length table sized to the shared block cap.
pub const ANIMATED_BLOCK_TABLE_BYTES: usize = ANIMATED_BLOCK_TABLE_UNIFORM_BYTES as usize;

/// What the animated lightmap binds for a level, decided before any GPU work.
#[derive(Debug, Clone, PartialEq)]
pub enum AnimatedLightmapPlan {
    /// No section 25: the dummy atlas, metered as Dummy.
    Absent,
    /// An accepted section with nothing to compose (no pages, no chunks or no
    /// texel lights): the dummy atlas, metered as Data.
    Empty,
    /// Section 25 with no static cell blocks to key on (placeholder lightmap
    /// mode): the dummy atlas, metered as Fallback. Logged once here.
    NoBlockFrame,
    /// A real compact atlas and its compose dispatch.
    Atlas(Box<AnimatedAtlasPlan>),
}

/// Everything the GPU half needs to create a dispatching atlas, already
/// validated against the device limits and the VRAM budget.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimatedAtlasPlan {
    pub extent: AnimatedAtlasExtent,
    /// Both compose-target atlases' byte estimate, for the install log.
    pub atlas_bytes: u64,
    /// The full unfiltered tile list; per-frame cull walks it.
    pub master_tiles: Vec<DispatchTile>,
    /// Tile-buffer capacity: master tiles plus the largest grid padding.
    pub tile_capacity: usize,
    /// Device `max_compute_workgroups_per_dimension`, the per-axis grid bound.
    pub max_workgroups_per_dim: u32,
    pub chunk_rects_bytes: Vec<u8>,
    pub offset_counts_bytes: Vec<u8>,
    pub texel_lights_bytes: Vec<u8>,
    /// The master list padded to `tile_capacity`, the tile buffer's seed.
    pub dispatch_tiles_bytes: Vec<u8>,
    /// Owning cell per chunk, parallel to the section's chunk rects.
    pub chunk_cell_ids: Vec<u32>,
    /// The forward block table for this atlas.
    pub block_table: Vec<u8>,
    pub chunk_count: usize,
    pub covered_texels: usize,
    pub weight_entries: usize,
}

// Compile-time guard: the plan is built on a load-pool thread.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<AnimatedLightmapPlan>();
};

impl AnimatedLightmapPlan {
    /// The forward block table the lightmap group binds beside this plan's
    /// atlas: the atlas's own table, or an empty table (every vertex resolves
    /// to no block) for every dummy path.
    pub fn block_table_bytes(&self) -> Vec<u8> {
        match self {
            Self::Atlas(atlas) => atlas.block_table.clone(),
            Self::Absent | Self::Empty | Self::NoBlockFrame => empty_animated_block_table(),
        }
    }
}

/// Plan the animated lightmap. `static_blocks` is the static pool plan's
/// installed cell-block extents (`None` in placeholder mode);
/// `animated_light_count` is the SH volume's animated descriptor count.
/// Returns `Err` on any validation or preflight failure; the renderer logs it
/// and binds the non-dispatching dummy.
pub fn plan_animated_lightmap(
    weight_maps: Option<&AnimatedLightWeightMapsSection>,
    animated_chunks: Option<&AnimatedLightChunksSection>,
    bvh_leaves: &[BvhLeaf],
    animated_light_count: u32,
    static_blocks: Option<&[(u32, u32)]>,
    limits: &DeviceLimits,
) -> Result<AnimatedLightmapPlan, String> {
    let Some(section) = weight_maps else {
        return Ok(AnimatedLightmapPlan::Absent);
    };
    validate_weight_map_section(section)?;

    // An empty section has no pages. Guard before any atlas allocation:
    // wgpu rejects a D2 array texture with depth zero.
    if animated_atlas_extent(section.page_size, section.compact_layers).is_none() {
        return Ok(AnimatedLightmapPlan::Empty);
    }

    let Some(static_blocks) = static_blocks else {
        // Placeholder mode (no cell blocks installed), so the block-local
        // coordinates the block table translates from have no valid target.
        // Take the dummy-atlas path: the animated term contributes nothing,
        // which matches the static term already being neutral.
        log::warn!(
            "[Renderer] Animated lightmap present but no static lightmap cell blocks \
             are installed; skipping animated-light compose for this level."
        );
        return Ok(AnimatedLightmapPlan::NoBlockFrame);
    };

    validate_cross_section(
        section,
        animated_chunks,
        animated_light_count,
        static_blocks,
    )?;

    if section.chunk_rects.is_empty() || section.texel_lights.is_empty() {
        // Nothing to compose. Either no animated chunks, or every animated
        // light is SDF-typed so the baker emitted zero baked direct weight
        // (the disjoint-direct split). The chunk rects still exist (they pair
        // 1:1 with AnimatedLightChunks for the SH delta bake), but with no
        // texel-lights there is no direct term to composite: the forward pass
        // falls back to the static lightmap and runtime SDF resolves these
        // lights' direct term. Takes the same no-atlas path as a map with no
        // weight maps.
        return Ok(AnimatedLightmapPlan::Empty);
    }

    let extent = animated_atlas_preflight(
        section.page_size,
        section.compact_layers,
        limits.max_texture_dimension_2d,
        limits.max_texture_array_layers,
    )?;
    let atlas_bytes =
        animated_atlas_byte_estimate(extent.width, extent.height, extent.depth_or_array_layers);

    let master_tiles = expand_dispatch_tiles(&section.chunk_rects, &section.blocks);
    let compose_workgroup_count = u32::try_from(master_tiles.len())
        .map_err(|_| "animated lightmap dispatch tile count exceeds u32".to_owned())?;
    let max_workgroups_per_dim = limits.max_compute_workgroups_per_dimension;
    let (_, master_rows) = compose_grid(compose_workgroup_count, max_workgroups_per_dim)
        .ok_or_else(|| {
            format!(
                "[AnimatedLightmap] dispatch tile count {compose_workgroup_count} exceeds \
                 the {max_workgroups_per_dim}² compose workgroup grid",
            )
        })?;
    // A visible subset never needs more rows than the full list, and each
    // row pads by at most one tile, so this capacity holds every frame's grid.
    let tile_capacity = master_tiles.len() + master_rows as usize;
    let tile_buffer_bytes = (tile_capacity * std::mem::size_of::<DispatchTile>()) as u64;
    if tile_buffer_bytes > limits.max_storage_buffer_binding_size
        || tile_buffer_bytes > limits.max_buffer_size
    {
        return Err(format!(
            "[AnimatedLightmap] dispatch tile buffer ({tile_buffer_bytes} bytes for \
             {compose_workgroup_count} tiles) exceeds the device storage-buffer limit",
        ));
    }

    // The forward table must describe exactly this atlas; a block whose
    // offset cannot pack fails the plan before anything is created.
    let block_table = animated_block_table_bytes(Some(section))?;

    let mut seed_tiles = master_tiles.clone();
    seed_tiles.resize(tile_capacity, PADDING_TILE);
    Ok(AnimatedLightmapPlan::Atlas(Box::new(AnimatedAtlasPlan {
        extent,
        atlas_bytes,
        tile_capacity,
        max_workgroups_per_dim,
        chunk_rects_bytes: pack_chunk_rects(&section.chunk_rects),
        offset_counts_bytes: pack_offset_counts(section),
        texel_lights_bytes: pack_texel_lights(section),
        dispatch_tiles_bytes: pack_dispatch_tiles(&seed_tiles),
        chunk_cell_ids: build_chunk_cell_ids(bvh_leaves, section.chunk_rects.len()),
        block_table,
        chunk_count: section.chunk_rects.len(),
        covered_texels: section.offset_counts.len(),
        weight_entries: section.texel_lights.len(),
        master_tiles,
    })))
}

/// The forward block table with no blocks: every vertex resolves to none.
pub fn empty_animated_block_table() -> Vec<u8> {
    vec![0_u8; ANIMATED_BLOCK_TABLE_BYTES]
}

/// Build the forward block-table uniform for `section`, or an empty table
/// when `section` is `None`. Layout (native-endian u32s, as the GPU reads
/// them, mirroring `AnimatedBlockTable` in forward.wgsl): `0 (reserved),
/// page_size, block_count, 0`, then per block the packed `(i16 dx, i16 dy)`
/// compact − block-local texel offset and the page. The forward stage adds
/// that offset to the fragment's block-local texel. Errs when the block count
/// passes the table cap or an offset does not fit an `i16`.
pub fn animated_block_table_bytes(
    section: Option<&AnimatedLightWeightMapsSection>,
) -> Result<Vec<u8>, String> {
    let mut bytes = empty_animated_block_table();
    let Some(section) = section else {
        return Ok(bytes);
    };
    if section.blocks.len() > ANIMATED_BLOCK_CAP as usize {
        return Err(format!(
            "animated block count {} exceeds the block-table cap {ANIMATED_BLOCK_CAP}",
            section.blocks.len()
        ));
    }
    let mut write = |at: usize, value: u32| bytes[at..at + 4].copy_from_slice(&value.to_ne_bytes());
    write(4, section.page_size);
    write(8, section.blocks.len() as u32);
    for (index, block) in section.blocks.iter().enumerate() {
        let dx = texel_offset(index, block.block_x, block.compact_x)?;
        let dy = texel_offset(index, block.block_y, block.compact_y)?;
        let at = ANIMATED_BLOCK_TABLE_HEADER_BYTES as usize
            + index * ANIMATED_BLOCK_TABLE_BYTES_PER_BLOCK as usize;
        write(at, u32::from(dx as u16) | (u32::from(dy as u16) << 16));
        write(at + 4, block.compact_layer);
    }
    Ok(bytes)
}

/// Block-local→compact translation on one axis. A valid level's block-local
/// coordinate lies inside a pool-layer cell block and its compact one inside
/// its page, so the difference fits an `i16`; anything else is refused.
fn texel_offset(block: usize, block_coord: u16, compact_coord: u32) -> Result<i16, String> {
    i16::try_from(i64::from(compact_coord) - i64::from(block_coord)).map_err(|_| {
        format!(
            "animated block {block}: compact coordinate {compact_coord} and block-local \
             coordinate {block_coord} differ by more than an i16 block-table offset"
        )
    })
}

/// Grid-padding record: the compose shader returns on this `chunk_idx`.
/// Mirrors `PADDING_TILE` in `animated_lightmap_compose.wgsl`.
pub const PADDING_TILE: DispatchTile = DispatchTile {
    chunk_idx: u32::MAX,
    tile_origin_x: 0,
    tile_origin_y: 0,
    target_page: 0,
};

/// Row-major 2D workgroup grid `(columns, rows)` for `tile_count` one-tile
/// workgroups, with neither dimension above `max_per_dim`. Rows are balanced,
/// so the padding `columns * rows - tile_count` stays below `rows`. `None`
/// when the count exceeds `max_per_dim²`.
pub fn compose_grid(tile_count: u32, max_per_dim: u32) -> Option<(u32, u32)> {
    let rows = tile_count.div_ceil(max_per_dim).max(1);
    (rows <= max_per_dim).then(|| (tile_count.div_ceil(rows), rows))
}

/// Pad `tiles` with `PADDING_TILE` so it fills `grid` exactly; the shader
/// indexes the tile buffer by flattened grid position.
pub fn pad_tiles_to_grid(tiles: &mut Vec<DispatchTile>, (columns, rows): (u32, u32)) {
    debug_assert!(tiles.len() <= (columns * rows) as usize);
    tiles.resize((columns * rows) as usize, PADDING_TILE);
}

/// Compact-atlas dimensions: `page_count` square pages of `page_size`. `None`
/// is the no-animated-atlas path: wgpu rejects a texture whose array depth is
/// zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimatedAtlasExtent {
    pub width: u32,
    pub height: u32,
    pub depth_or_array_layers: u32,
}

pub fn animated_atlas_extent(page_size: u32, page_count: u32) -> Option<AnimatedAtlasExtent> {
    (page_count > 0).then_some(AnimatedAtlasExtent {
        width: page_size,
        height: page_size,
        depth_or_array_layers: page_count,
    })
}

pub fn validate_weight_map_section(section: &AnimatedLightWeightMapsSection) -> Result<(), String> {
    if let Some(error) = section.consistency_error() {
        return Err(format!(
            "animated light weight maps section is internally inconsistent: {error}"
        ));
    }

    // A zero-area chunk has no offset-count records to reference its nonempty
    // weight pool. Letting it through leaves compose with no dispatch tiles and
    // reaches wgpu's zero-sized storage-buffer rejection.
    if section
        .chunk_rects
        .iter()
        .any(|rect| rect.width == 0 || rect.height == 0)
    {
        return Err("animated light weight maps section contains a zero-area chunk".to_owned());
    }

    Ok(())
}

pub fn animated_atlas_preflight(
    page_size: u32,
    page_count: u32,
    max_texture_dimension_2d: u32,
    max_texture_array_layers: u32,
) -> Result<AnimatedAtlasExtent, String> {
    let extent = animated_atlas_extent(page_size, page_count)
        .ok_or_else(|| "animated lightmap atlas has zero pages".to_owned())?;

    if page_size > max_texture_dimension_2d {
        return Err(format!(
            "animated lightmap page size {page_size} exceeds device maxTextureDimension2D \
             {max_texture_dimension_2d}",
        ));
    }
    if page_count > max_texture_array_layers {
        return Err(format!(
            "animated lightmap atlas requires {page_count} pages, exceeding device \
             maxTextureArrayLayers {max_texture_array_layers}",
        ));
    }

    let atlas_bytes = animated_atlas_byte_estimate(page_size, page_size, page_count);
    if !animated_atlas_fits_budget(
        page_size,
        page_size,
        page_count,
        ANIMATED_ATLAS_VRAM_BUDGET_BYTES,
    ) {
        return Err(format!(
            "animated lightmap atlas {page_size}x{page_size}x{page_count} requires \
             {atlas_bytes} bytes, exceeding the {}-byte VRAM budget",
            ANIMATED_ATLAS_VRAM_BUDGET_BYTES,
        ));
    }

    Ok(extent)
}

/// The block table must describe the resource actually bound at group 4.
/// Inactive resources bind a one-layer zero dummy, so the forward table must
/// be empty — every vertex resolves to no block — even when the decoded
/// section carried real blocks.
pub fn installed_block_table_section(
    resource_active: bool,
    decoded: Option<&AnimatedLightWeightMapsSection>,
) -> Option<&AnimatedLightWeightMapsSection> {
    decoded.filter(|_| resource_active)
}

/// One 8×8 atlas tile assigned to a chunk. Indexed by flattened 2D
/// `workgroup_id` in the compose shader.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchTile {
    pub chunk_idx: u32,
    pub tile_origin_x: u32,
    pub tile_origin_y: u32,
    pub target_page: u32,
}

/// GPU storage-buffer layout for the compose-relevant prefix of
/// `ChunkAtlasRect`, in compact-atlas coordinates. The chunk's block resolves
/// to `DispatchTile.target_page` on the CPU, so it stays out of this buffer
/// and WGSL struct.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuChunkRect {
    pub compact_x: u32,
    pub compact_y: u32,
    pub width: u32,
    pub height: u32,
    pub texel_offset: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuOffsetCount {
    pub offset: u32,
    pub count: u32,
}

/// GPU storage-buffer layout for `TexelLight`. The two `u16` octahedral
/// direction components are packed into one `u32` (low 16 bits = x, high 16
/// bits = y) so the struct stays naturally aligned for storage-buffer access
/// (12 bytes; the WGSL `TexelLight` mirrors the same packing).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuTexelLight {
    pub light_index: u32,
    pub weight: f32,
    pub direction_oct_packed: u32,
}

/// Build the chunk → cell-id table from BVH leaf ranges. Chunks not covered
/// by any leaf keep `u32::MAX` so the per-frame filter always rejects them.
/// In a valid PRL every animated chunk belongs to exactly one leaf; the
/// sentinel is a defensive fallback.
pub fn build_chunk_cell_ids(bvh_leaves: &[BvhLeaf], chunk_count: usize) -> Vec<u32> {
    let mut chunk_cell_ids = vec![u32::MAX; chunk_count];
    for leaf in bvh_leaves {
        let start = leaf.chunk_range_start as usize;
        let count = leaf.chunk_range_count as usize;
        let end = start.saturating_add(count).min(chunk_count);
        for slot in chunk_cell_ids.iter_mut().take(end).skip(start) {
            *slot = leaf.cell_id;
        }
    }
    chunk_cell_ids
}

/// Expand each chunk rect into `ceil(w/8) × ceil(h/8)` 8×8 dispatch tiles on
/// its block's page — the same page the forward block table names for that
/// block. Tile order is y-major, x-minor; order doesn't affect correctness.
pub fn expand_dispatch_tiles(
    chunk_rects: &[postretro_level_format::animated_light_weight_maps::ChunkAtlasRect],
    blocks: &[postretro_level_format::animated_light_weight_maps::AnimatedBlock],
) -> Vec<DispatchTile> {
    let mut tiles = Vec::new();
    for (chunk_idx, rect) in chunk_rects.iter().enumerate() {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        let Some(block) = blocks.get(rect.block as usize) else {
            // Section-25 validation rejects this malformed record at load, but
            // never compose it onto page 0 if a caller bypasses that boundary.
            continue;
        };
        let tiles_x = rect.width.div_ceil(8);
        let tiles_y = rect.height.div_ceil(8);
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                tiles.push(DispatchTile {
                    chunk_idx: chunk_idx as u32,
                    tile_origin_x: tx * 8,
                    tile_origin_y: ty * 8,
                    target_page: block.compact_layer,
                });
            }
        }
    }
    tiles
}

pub fn pack_chunk_rects(
    chunk_rects: &[postretro_level_format::animated_light_weight_maps::ChunkAtlasRect],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(chunk_rects.len() * std::mem::size_of::<GpuChunkRect>());
    for r in chunk_rects {
        bytes.extend_from_slice(&r.compact_x.to_ne_bytes());
        bytes.extend_from_slice(&r.compact_y.to_ne_bytes());
        bytes.extend_from_slice(&r.width.to_ne_bytes());
        bytes.extend_from_slice(&r.height.to_ne_bytes());
        bytes.extend_from_slice(&r.texel_offset.to_ne_bytes());
    }
    bytes
}

pub fn pack_offset_counts(section: &AnimatedLightWeightMapsSection) -> Vec<u8> {
    let mut bytes =
        Vec::with_capacity(section.offset_counts.len() * std::mem::size_of::<GpuOffsetCount>());
    for oc in &section.offset_counts {
        bytes.extend_from_slice(&oc.offset.to_ne_bytes());
        bytes.extend_from_slice(&oc.count.to_ne_bytes());
    }
    bytes
}

pub fn pack_texel_lights(section: &AnimatedLightWeightMapsSection) -> Vec<u8> {
    let mut bytes =
        Vec::with_capacity(section.texel_lights.len() * std::mem::size_of::<GpuTexelLight>());
    for tl in &section.texel_lights {
        bytes.extend_from_slice(&tl.light_index.to_ne_bytes());
        bytes.extend_from_slice(&tl.weight.to_ne_bytes());
        // Pack the two octahedral u16s into one u32. WGSL unpacks via
        // `direction_oct_packed & 0xFFFF` / `>> 16` — see compose shader.
        let packed: u32 = (tl.direction_oct[0] as u32) | ((tl.direction_oct[1] as u32) << 16);
        bytes.extend_from_slice(&packed.to_ne_bytes());
    }
    bytes
}

pub fn pack_dispatch_tiles(tiles: &[DispatchTile]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(tiles));
    pack_dispatch_tiles_into(tiles, &mut bytes);
    bytes
}

/// Appends packed tile bytes into a caller-owned buffer to avoid per-frame allocation.
pub fn pack_dispatch_tiles_into(tiles: &[DispatchTile], bytes: &mut Vec<u8>) {
    bytes.reserve(std::mem::size_of_val(tiles));
    for t in tiles {
        bytes.extend_from_slice(&t.chunk_idx.to_ne_bytes());
        bytes.extend_from_slice(&t.tile_origin_x.to_ne_bytes());
        bytes.extend_from_slice(&t.tile_origin_y.to_ne_bytes());
        bytes.extend_from_slice(&t.target_page.to_ne_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::animated_light_weight_maps::AnimatedBlock;

    fn section_with_blocks(blocks: Vec<AnimatedBlock>) -> AnimatedLightWeightMapsSection {
        AnimatedLightWeightMapsSection {
            page_size: 64,
            blocks,
            ..AnimatedLightWeightMapsSection::empty()
        }
    }

    fn block(block_x: u16, compact_x: u32) -> AnimatedBlock {
        AnimatedBlock {
            lightmap_block: 0,
            block_x,
            block_y: 0,
            compact_x,
            compact_y: 0,
            width: 1,
            height: 1,
            compact_layer: 0,
        }
    }

    // What used to be the forward table's `assert!` and `expect` is now a
    // refusal the plan returns before any GPU work.
    #[test]
    fn block_table_refuses_what_it_cannot_pack() {
        let over_cap = section_with_blocks(vec![block(0, 0); ANIMATED_BLOCK_CAP as usize + 1]);
        assert!(animated_block_table_bytes(Some(&over_cap)).is_err());
        let far = section_with_blocks(vec![block(u16::MAX, 0)]);
        assert!(animated_block_table_bytes(Some(&far)).is_err());
        let near = section_with_blocks(vec![block(3, 40)]);
        let table = animated_block_table_bytes(Some(&near)).unwrap();
        assert_eq!(table.len(), ANIMATED_BLOCK_TABLE_BYTES);
        assert_eq!(
            animated_block_table_bytes(None).unwrap(),
            empty_animated_block_table()
        );
    }
}
