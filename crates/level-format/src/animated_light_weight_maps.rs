// AnimatedLightWeightMaps PRL section (ID 25): per-chunk compact-atlas
// rectangles with per-texel (offset, count) pairs into a flat pool of
// (light_index, weight) tuples, plus the block table that places each
// animated face's chart in the compact, paged animated atlas. Baked at compile
// time; composed at runtime into the animated lightmap contribution atlas.
//
// Format inventory: context/lib/build_pipeline.md §PRL section IDs.

use crate::FormatError;
use crate::animated_lightmap_atlas::ANIMATED_BLOCK_CAP;

/// Current section version. Version 5 keys each animated block by its static
/// lightmap cell block and block-local texels instead of static-atlas layer
/// and texels. Loaders accept this version only.
pub const ANIMATED_LIGHT_WEIGHT_MAPS_VERSION: u32 = 5;

/// Atlas rectangle for one chunk in compact-atlas coordinates, plus an offset
/// into the per-texel offset-count table and the block that owns it. The page
/// the chunk composes into is its block's `compact_layer`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChunkAtlasRect {
    pub compact_x: u32,
    pub compact_y: u32,
    pub width: u32,
    pub height: u32,
    pub texel_offset: u32, // index into the per-texel offset_counts array
    /// Index into [`AnimatedLightWeightMapsSection::blocks`].
    pub block: u32,
}

/// One animated face's chart placement rect in both spaces. The static rect
/// is the chart placement including its padding gutter, in the texels of the
/// static lightmap cell block that holds the chart; static→compact is a
/// translation plus a page change, so a chunk keeps its offset inside its
/// block in both spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimatedBlock {
    /// Id-22 cell block holding this face's chart.
    pub lightmap_block: u32,
    /// Block-local texel origin of the static rect.
    pub block_x: u16,
    pub block_y: u16,
    pub compact_x: u32,
    pub compact_y: u32,
    /// Compact-atlas page (array layer).
    pub compact_layer: u32,
    pub width: u32,
    pub height: u32,
}

/// One per-texel entry: (offset, count) into the flat `texel_lights` pool.
///
/// For a texel at position (tx, ty) within a chunk rect, the per-texel record
/// is at index `chunk_rect.texel_offset + ty * chunk_rect.width + tx`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexelLightEntry {
    pub offset: u32,
    pub count: u32,
}

/// One light-weight entry: (light_index, weight, direction_oct).
///
/// `light_index`: direct slot into the GPU `AnimationDescriptor` buffer —
/// the same namespace as `AnimatedLightChunks.chunks[i].light_indices`,
/// filtered by `!is_dynamic && animation.is_some()`. No remap is needed at
/// bake time because the chunk-list builder and the descriptor buffer use
/// the same filter and iteration order. `weight`: per-texel contribution
/// magnitude (0.0..1.0, normalized after bake).
///
/// `direction_oct`: octahedral-encoded unit vector from the texel toward
/// the light (`[u16; 2]`, same encoding as `crate::octahedral::encode`).
/// Baked because the light's geometry is static — its per-texel incoming
/// direction never changes. The compose pass weights it by the light's
/// per-frame radiance to fuse a runtime dominant-direction atlas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexelLight {
    pub light_index: u32,
    pub weight: f32,
    pub direction_oct: [u16; 2],
}

/// AnimatedLightWeightMaps section (ID 25).
///
/// On-disk layout (little-endian):
///
/// ```text
///   Header (28 bytes):
///     u32      version            (= 5)
///     u32      chunk_count
///     u32      offset_counts_len  (length of per-texel offset_counts array)
///     u32      texel_lights_len   (length of flat light weights pool)
///     u32      block_count
///     u32      page_size          (every page is page_size × page_size)
///     u32      compact_layers     (page count)
///
///   Chunk rects (24 bytes × chunk_count):
///     u32      compact_x
///     u32      compact_y
///     u32      width
///     u32      height
///     u32      texel_offset       (index into offset_counts)
///     u32      block              (index into the block table)
///
///   Blocks (28 bytes × block_count):
///     u32      lightmap_block     (id-22 cell block)
///     u16      block_x            (block-local static texel origin)
///     u16      block_y
///     u32      compact_x
///     u32      compact_y
///     u32      compact_layer
///     u32      width
///     u32      height
///
///   Offset table (8 bytes × offset_counts_len):
///     u32      offset             (into texel_lights)
///     u32      count
///
///   Light weights (12 bytes × texel_lights_len):
///     u32      light_index
///     f32      weight
///     u16      direction_oct[0]
///     u16      direction_oct[1]
/// ```
///
/// [`Self::is_consistent`] states the invariants a loader checks.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimatedLightWeightMapsSection {
    pub page_size: u32,
    pub compact_layers: u32,
    pub blocks: Vec<AnimatedBlock>,
    pub chunk_rects: Vec<ChunkAtlasRect>,
    pub offset_counts: Vec<TexelLightEntry>,
    pub texel_lights: Vec<TexelLight>,
}

const HEADER_SIZE: usize = 28;
const CHUNK_RECT_SIZE: usize = 24;
const BLOCK_SIZE: usize = 28;
const OFFSET_ENTRY_SIZE: usize = 8;
const TEXEL_LIGHT_SIZE: usize = 12;

impl AnimatedLightWeightMapsSection {
    /// Empty section — no chunks, no blocks, no pages.
    pub fn empty() -> Self {
        Self {
            page_size: 0,
            compact_layers: 0,
            blocks: Vec::new(),
            chunk_rects: Vec::new(),
            offset_counts: Vec::new(),
            texel_lights: Vec::new(),
        }
    }

    /// Static origin `(lightmap block, x, y)` of chunk `index`, in its cell
    /// block's texels: its compact position translated back through its
    /// animated block. `None` when the chunk names a block past the table or
    /// does not lie inside its block.
    pub fn chunk_block_origin(&self, index: usize) -> Option<(u32, u32, u32)> {
        let chunk = self.chunk_rects.get(index)?;
        let block = self.blocks.get(chunk.block as usize)?;
        if !rect_within(
            (chunk.compact_x, chunk.compact_y, chunk.width, chunk.height),
            (block.compact_x, block.compact_y, block.width, block.height),
        ) {
            return None;
        }
        let dx = chunk.compact_x - block.compact_x;
        let dy = chunk.compact_y - block.compact_y;
        Some((
            block.lightmap_block,
            u32::from(block.block_x).checked_add(dx)?,
            u32::from(block.block_y).checked_add(dy)?,
        ))
    }

    /// Verify internal consistency:
    ///   - chunk texel offsets partition `offset_counts`, and every
    ///     `(offset, count)` pair lies inside `texel_lights`;
    ///   - a non-empty section has a power-of-two page size and at least one
    ///     page; an empty one has no blocks and no pages;
    ///   - the block count fits the shared block-table cap, and the page count
    ///     does not exceed the block count (checked before anything is sized
    ///     from these header values);
    ///   - every chunk names a block and lies inside it;
    ///   - every block owns a chunk, lies inside its page, and overlaps no
    ///     other block on that page; no page is empty.
    pub fn is_consistent(&self) -> bool {
        self.consistency_error().is_none()
    }

    /// The first violated invariant of [`Self::is_consistent`], if any.
    pub fn consistency_error(&self) -> Option<String> {
        // Checked arithmetic keeps malformed wire values from panicking in
        // debug builds before the runtime boundary can reject them.
        let mut expected_offset = 0_u32;
        for chunk in &self.chunk_rects {
            if chunk.texel_offset != expected_offset {
                return Some("chunk texel offsets do not form a partition".to_owned());
            }
            let next = chunk
                .width
                .checked_mul(chunk.height)
                .and_then(|area| expected_offset.checked_add(area));
            let Some(next) = next else {
                return Some("chunk texel area overflows".to_owned());
            };
            expected_offset = next;
        }
        if self.offset_counts.len() != expected_offset as usize {
            return Some("offset_counts length does not match the chunk texel area".to_owned());
        }
        let texel_lights_len = self.texel_lights.len();
        for entry in &self.offset_counts {
            match entry.offset.checked_add(entry.count) {
                Some(end) if (end as usize) <= texel_lights_len => {}
                _ => return Some("offset_counts entry points past texel_lights".to_owned()),
            }
        }

        if self.chunk_rects.is_empty() {
            return (!self.blocks.is_empty() || self.compact_layers != 0)
                .then(|| "empty section carries blocks or pages".to_owned());
        }
        if !self.page_size.is_power_of_two() {
            return Some(format!(
                "page size {} is not a power of two",
                self.page_size
            ));
        }
        if self.compact_layers == 0 {
            return Some("non-empty section has no pages".to_owned());
        }
        if self.blocks.len() > ANIMATED_BLOCK_CAP as usize {
            return Some(format!(
                "animated block count {} exceeds the block-table cap {ANIMATED_BLOCK_CAP}",
                self.blocks.len()
            ));
        }
        // Every page holds a block, so a page count past the block count is
        // invalid — and rejecting it here keeps the header value from sizing
        // the page-occupancy table below.
        if self.compact_layers as usize > self.blocks.len() {
            return Some(format!(
                "{} pages for {} blocks: a page holds no block",
                self.compact_layers,
                self.blocks.len()
            ));
        }

        let mut block_has_chunk = vec![false; self.blocks.len()];
        for chunk in &self.chunk_rects {
            let Some(block) = self.blocks.get(chunk.block as usize) else {
                return Some(format!("chunk names block {} past the table", chunk.block));
            };
            if !rect_within(
                (chunk.compact_x, chunk.compact_y, chunk.width, chunk.height),
                (block.compact_x, block.compact_y, block.width, block.height),
            ) {
                return Some(format!("chunk lies outside block {}", chunk.block));
            }
            block_has_chunk[chunk.block as usize] = true;
        }

        let mut page_has_block = vec![false; self.compact_layers as usize];
        for (index, block) in self.blocks.iter().enumerate() {
            if !block_has_chunk[index] {
                return Some(format!("block {index} owns no chunk"));
            }
            if block.width == 0 || block.height == 0 {
                return Some(format!("block {index} has zero area"));
            }
            let Some(page) = page_has_block.get_mut(block.compact_layer as usize) else {
                return Some(format!(
                    "block {index} names page {} past the atlas",
                    block.compact_layer
                ));
            };
            *page = true;
            if !rect_within(
                (block.compact_x, block.compact_y, block.width, block.height),
                (0, 0, self.page_size, self.page_size),
            ) {
                return Some(format!("block {index} lies outside its page"));
            }
        }
        if page_has_block.iter().any(|used| !used) {
            return Some("a page holds no block".to_owned());
        }
        if let Some((a, b)) = first_overlapping_blocks(&self.blocks) {
            return Some(format!(
                "blocks {a} and {b} overlap on page {}",
                self.blocks[a].compact_layer
            ));
        }
        None
    }

    /// Largest block side, the page size's lower bound.
    pub fn largest_block_side(&self) -> u32 {
        self.blocks
            .iter()
            .map(|block| block.width.max(block.height))
            .max()
            .unwrap_or(0)
    }

    pub fn byte_len(&self) -> usize {
        HEADER_SIZE
            + self.chunk_rects.len() * CHUNK_RECT_SIZE
            + self.blocks.len() * BLOCK_SIZE
            + self.offset_counts.len() * OFFSET_ENTRY_SIZE
            + self.texel_lights.len() * TEXEL_LIGHT_SIZE
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.byte_len());

        buf.extend_from_slice(&ANIMATED_LIGHT_WEIGHT_MAPS_VERSION.to_le_bytes());
        buf.extend_from_slice(&(self.chunk_rects.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(self.offset_counts.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(self.texel_lights.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(self.blocks.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.page_size.to_le_bytes());
        buf.extend_from_slice(&self.compact_layers.to_le_bytes());

        for rect in &self.chunk_rects {
            for field in [
                rect.compact_x,
                rect.compact_y,
                rect.width,
                rect.height,
                rect.texel_offset,
                rect.block,
            ] {
                buf.extend_from_slice(&field.to_le_bytes());
            }
        }

        for block in &self.blocks {
            buf.extend_from_slice(&block.lightmap_block.to_le_bytes());
            buf.extend_from_slice(&block.block_x.to_le_bytes());
            buf.extend_from_slice(&block.block_y.to_le_bytes());
            for field in [
                block.compact_x,
                block.compact_y,
                block.compact_layer,
                block.width,
                block.height,
            ] {
                buf.extend_from_slice(&field.to_le_bytes());
            }
        }

        for entry in &self.offset_counts {
            buf.extend_from_slice(&entry.offset.to_le_bytes());
            buf.extend_from_slice(&entry.count.to_le_bytes());
        }

        for light in &self.texel_lights {
            buf.extend_from_slice(&light.light_index.to_le_bytes());
            buf.extend_from_slice(&light.weight.to_le_bytes());
            buf.extend_from_slice(&light.direction_oct[0].to_le_bytes());
            buf.extend_from_slice(&light.direction_oct[1].to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "animated light weight maps section too short for header",
            )));
        }

        let version = read_u32(data, 0);
        if version != ANIMATED_LIGHT_WEIGHT_MAPS_VERSION {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "animated light weight maps section unsupported version {version} \
                     (expected {ANIMATED_LIGHT_WEIGHT_MAPS_VERSION}); recompile the .prl with \
                     the current `prl-build`"
                ),
            )));
        }

        if data.len() < HEADER_SIZE {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "animated light weight maps section too short for header",
            )));
        }

        let chunk_count = read_u32(data, 4) as usize;
        let offset_counts_len = read_u32(data, 8) as usize;
        let texel_lights_len = read_u32(data, 12) as usize;
        let block_count = read_u32(data, 16) as usize;
        let page_size = read_u32(data, 20);
        let compact_layers = read_u32(data, 24);

        let overflow = || {
            FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "animated light weight maps section size overflows",
            ))
        };
        let needed = [
            (chunk_count, CHUNK_RECT_SIZE),
            (block_count, BLOCK_SIZE),
            (offset_counts_len, OFFSET_ENTRY_SIZE),
            (texel_lights_len, TEXEL_LIGHT_SIZE),
        ]
        .into_iter()
        .try_fold(HEADER_SIZE, |size, (count, stride)| {
            size.checked_add(count.checked_mul(stride)?)
        })
        .ok_or_else(overflow)?;

        if data.len() < needed {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "animated light weight maps section truncated: need {needed} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut cursor = HEADER_SIZE;
        let mut chunk_rects = Vec::with_capacity(chunk_count);
        for _ in 0..chunk_count {
            chunk_rects.push(ChunkAtlasRect {
                compact_x: read_u32(data, cursor),
                compact_y: read_u32(data, cursor + 4),
                width: read_u32(data, cursor + 8),
                height: read_u32(data, cursor + 12),
                texel_offset: read_u32(data, cursor + 16),
                block: read_u32(data, cursor + 20),
            });
            cursor += CHUNK_RECT_SIZE;
        }

        let mut blocks = Vec::with_capacity(block_count);
        for _ in 0..block_count {
            blocks.push(AnimatedBlock {
                lightmap_block: read_u32(data, cursor),
                block_x: read_u16(data, cursor + 4),
                block_y: read_u16(data, cursor + 6),
                compact_x: read_u32(data, cursor + 8),
                compact_y: read_u32(data, cursor + 12),
                compact_layer: read_u32(data, cursor + 16),
                width: read_u32(data, cursor + 20),
                height: read_u32(data, cursor + 24),
            });
            cursor += BLOCK_SIZE;
        }

        let mut offset_counts = Vec::with_capacity(offset_counts_len);
        for _ in 0..offset_counts_len {
            offset_counts.push(TexelLightEntry {
                offset: read_u32(data, cursor),
                count: read_u32(data, cursor + 4),
            });
            cursor += OFFSET_ENTRY_SIZE;
        }

        let mut texel_lights = Vec::with_capacity(texel_lights_len);
        for _ in 0..texel_lights_len {
            texel_lights.push(TexelLight {
                light_index: read_u32(data, cursor),
                weight: read_f32(data, cursor + 4),
                direction_oct: [read_u16(data, cursor + 8), read_u16(data, cursor + 10)],
            });
            cursor += TEXEL_LIGHT_SIZE;
        }

        Ok(Self {
            page_size,
            compact_layers,
            blocks,
            chunk_rects,
            offset_counts,
            texel_lights,
        })
    }
}

/// Whether `inner` lies inside `outer`, both `(x, y, w, h)`. Widened so wire
/// values near `u32::MAX` cannot wrap into a false pass.
fn rect_within(inner: (u32, u32, u32, u32), outer: (u32, u32, u32, u32)) -> bool {
    let (ix, iy, iw, ih) = inner;
    let (ox, oy, ow, oh) = outer;
    ix >= ox
        && iy >= oy
        && u64::from(ix) + u64::from(iw) <= u64::from(ox) + u64::from(ow)
        && u64::from(iy) + u64::from(ih) <= u64::from(oy) + u64::from(oh)
}

/// First pair of blocks sharing a page whose rects intersect. Sort-and-sweep
/// on x per page; worst case is quadratic in blocks sharing an x column,
/// which the block cap bounds.
fn first_overlapping_blocks(blocks: &[AnimatedBlock]) -> Option<(usize, usize)> {
    let mut order: Vec<usize> = (0..blocks.len()).collect();
    order.sort_by_key(|&i| (blocks[i].compact_layer, blocks[i].compact_x, i));
    for (position, &a) in order.iter().enumerate() {
        let block_a = &blocks[a];
        let a_right = u64::from(block_a.compact_x) + u64::from(block_a.width);
        for &b in &order[position + 1..] {
            let block_b = &blocks[b];
            if block_b.compact_layer != block_a.compact_layer
                || u64::from(block_b.compact_x) >= a_right
            {
                break;
            }
            let overlap_y = u64::from(block_a.compact_y)
                < u64::from(block_b.compact_y) + u64::from(block_b.height)
                && u64::from(block_b.compact_y)
                    < u64::from(block_a.compact_y) + u64::from(block_a.height);
            if overlap_y {
                return Some((a.min(b), a.max(b)));
            }
        }
    }
    None
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_f32(data: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_u16(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two faces on two static layers packed onto one 64² page. Block 0 holds
    /// a 2×2 chunk and block 1 a 3×1 chunk, each at a non-zero offset inside
    /// its block.
    fn sample_section() -> AnimatedLightWeightMapsSection {
        AnimatedLightWeightMapsSection {
            page_size: 64,
            compact_layers: 1,
            blocks: vec![
                AnimatedBlock {
                    lightmap_block: 3,
                    block_x: 100,
                    block_y: 40,
                    compact_x: 0,
                    compact_y: 0,
                    compact_layer: 0,
                    width: 8,
                    height: 6,
                },
                AnimatedBlock {
                    lightmap_block: 11,
                    block_x: 7,
                    block_y: 9,
                    compact_x: 8,
                    compact_y: 0,
                    compact_layer: 0,
                    width: 7,
                    height: 5,
                },
            ],
            chunk_rects: vec![
                ChunkAtlasRect {
                    compact_x: 2,
                    compact_y: 2,
                    width: 2,
                    height: 2,
                    texel_offset: 0,
                    block: 0,
                },
                ChunkAtlasRect {
                    compact_x: 10,
                    compact_y: 2,
                    width: 3,
                    height: 1,
                    texel_offset: 4,
                    block: 1,
                },
            ],
            offset_counts: vec![
                TexelLightEntry {
                    offset: 0,
                    count: 2,
                },
                TexelLightEntry {
                    offset: 2,
                    count: 1,
                },
                TexelLightEntry {
                    offset: 3,
                    count: 0,
                },
                TexelLightEntry {
                    offset: 3,
                    count: 1,
                },
                TexelLightEntry {
                    offset: 4,
                    count: 2,
                },
                TexelLightEntry {
                    offset: 6,
                    count: 1,
                },
                TexelLightEntry {
                    offset: 7,
                    count: 1,
                },
            ],
            texel_lights: vec![
                TexelLight {
                    light_index: 0,
                    weight: 0.8,
                    direction_oct: [32768, 65535],
                },
                TexelLight {
                    light_index: 1,
                    weight: 0.2,
                    direction_oct: [65535, 32768],
                },
                TexelLight {
                    light_index: 2,
                    weight: 1.0,
                    direction_oct: [0, 32768],
                },
                TexelLight {
                    light_index: 3,
                    weight: 0.5,
                    direction_oct: [32768, 0],
                },
                TexelLight {
                    light_index: 4,
                    weight: 0.6,
                    direction_oct: [32768, 32768],
                },
                TexelLight {
                    light_index: 5,
                    weight: 0.3,
                    direction_oct: [16384, 49152],
                },
                TexelLight {
                    light_index: 6,
                    weight: 0.9,
                    direction_oct: [49152, 16384],
                },
                TexelLight {
                    light_index: 7,
                    weight: 0.4,
                    direction_oct: [12345, 54321],
                },
            ],
        }
    }

    fn with_version(section: &AnimatedLightWeightMapsSection, version: u32) -> Vec<u8> {
        let mut bytes = section.to_bytes();
        bytes[0..4].copy_from_slice(&version.to_le_bytes());
        bytes
    }

    #[test]
    fn v5_round_trips_blocks_pages_and_compact_chunks() {
        let section = sample_section();
        let bytes = section.to_bytes();
        assert_eq!(section.byte_len(), bytes.len());
        assert_eq!(&bytes[0..4], &5_u32.to_le_bytes());
        let restored = AnimatedLightWeightMapsSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, section);
        assert_eq!(restored.to_bytes(), bytes);
    }

    #[test]
    fn v5_header_carries_block_count_page_size_and_page_count() {
        let bytes = sample_section().to_bytes();
        assert_eq!(&bytes[16..20], &2_u32.to_le_bytes(), "block_count");
        assert_eq!(&bytes[20..24], &64_u32.to_le_bytes(), "page_size");
        assert_eq!(&bytes[24..28], &1_u32.to_le_bytes(), "compact_layers");
        // Block table follows the chunk rects.
        let first_block = HEADER_SIZE + 2 * CHUNK_RECT_SIZE;
        assert_eq!(&bytes[first_block..first_block + 4], &3_u32.to_le_bytes());
    }

    #[test]
    fn v2_through_v4_sections_are_rejected_with_a_recompile_error() {
        for version in [2_u32, 3, 4] {
            let err = AnimatedLightWeightMapsSection::from_bytes(&with_version(
                &sample_section(),
                version,
            ))
            .unwrap_err();
            let message = err.to_string();
            assert!(
                message.contains(&format!("unsupported version {version}")),
                "{message}"
            );
            assert!(message.contains("recompile"), "{message}");
            match err {
                FormatError::Io(error) => {
                    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData)
                }
                _ => unreachable!("only I/O format errors are returned by this decoder"),
            }
        }
    }

    #[test]
    fn rejects_bad_version() {
        let err = AnimatedLightWeightMapsSection::from_bytes(&with_version(&sample_section(), 999))
            .unwrap_err();
        assert!(err.to_string().contains("unsupported version 999"));
    }

    #[test]
    fn chunk_block_origin_translates_through_its_block() {
        let section = sample_section();
        assert_eq!(section.chunk_block_origin(0), Some((3, 102, 42)));
        assert_eq!(section.chunk_block_origin(1), Some((11, 9, 11)));
        assert_eq!(section.chunk_block_origin(2), None);
    }

    #[test]
    fn to_bytes_is_deterministic() {
        let section = sample_section();
        assert_eq!(section.to_bytes(), section.to_bytes());
    }

    #[test]
    fn empty_section_round_trips() {
        let section = AnimatedLightWeightMapsSection::empty();
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), HEADER_SIZE);
        assert_eq!(
            AnimatedLightWeightMapsSection::from_bytes(&bytes).unwrap(),
            section
        );
        assert!(section.is_consistent());
    }

    #[test]
    fn consistency_check_valid() {
        assert_eq!(sample_section().consistency_error(), None);
    }

    #[test]
    fn consistency_check_fails_on_wrong_offset_counts_length() {
        let mut section = sample_section();
        section.offset_counts.pop();
        assert!(!section.is_consistent());
    }

    #[test]
    fn consistency_check_fails_on_wrong_chunk_offset() {
        let mut section = sample_section();
        section.chunk_rects[1].texel_offset = 5;
        assert!(!section.is_consistent());
    }

    #[test]
    fn consistency_check_fails_when_a_chunk_names_a_block_past_the_table() {
        let mut section = sample_section();
        section.chunk_rects[1].block = 2;
        assert!(!section.is_consistent());
    }

    #[test]
    fn consistency_check_fails_when_a_chunk_leaves_its_block() {
        let mut section = sample_section();
        section.chunk_rects[0].compact_x = 7;
        assert!(!section.is_consistent());
    }

    #[test]
    fn consistency_check_fails_when_a_block_leaves_its_page() {
        let mut section = sample_section();
        section.page_size = 8;
        assert!(!section.is_consistent());
    }

    #[test]
    fn consistency_check_fails_when_blocks_overlap_on_a_page() {
        let mut section = sample_section();
        section.blocks[1].compact_x = 7;
        section.chunk_rects[1].compact_x = 9;
        let error = section
            .consistency_error()
            .expect("overlap must be rejected");
        assert!(error.contains("overlap"), "{error}");
    }

    #[test]
    fn consistency_check_accepts_blocks_sharing_coordinates_on_different_pages() {
        let mut section = sample_section();
        section.compact_layers = 2;
        section.blocks[1].compact_layer = 1;
        section.blocks[1].compact_x = 0;
        section.chunk_rects[1].compact_x = 2;
        assert_eq!(section.consistency_error(), None);
    }

    #[test]
    fn consistency_check_fails_on_an_empty_page_or_a_non_power_of_two_page() {
        let mut empty_page = sample_section();
        empty_page.compact_layers = 2;
        assert!(!empty_page.is_consistent());

        let mut odd_page = sample_section();
        odd_page.page_size = 48;
        assert!(!odd_page.is_consistent());
    }

    #[test]
    fn consistency_check_fails_on_a_block_without_chunks() {
        let mut section = sample_section();
        section.blocks.push(AnimatedBlock {
            compact_x: 20,
            ..section.blocks[0]
        });
        assert!(!section.is_consistent());
    }

    #[test]
    fn consistency_check_rejects_a_block_count_over_the_cap_naming_both() {
        let mut section = sample_section();
        let extra = section.blocks[1];
        section
            .blocks
            .resize(ANIMATED_BLOCK_CAP as usize + 1, extra);
        let error = section
            .consistency_error()
            .expect("over-cap section is rejected");
        assert!(
            error.contains(&(ANIMATED_BLOCK_CAP + 1).to_string()),
            "{error}"
        );
        assert!(error.contains(&ANIMATED_BLOCK_CAP.to_string()), "{error}");
    }

    #[test]
    fn consistency_check_rejects_more_pages_than_blocks_before_sizing_from_them() {
        let mut section = sample_section();
        section.compact_layers = u32::MAX;
        let error = section.consistency_error().expect("page count is bounded");
        assert!(error.contains("a page holds no block"), "{error}");
    }

    #[test]
    fn chunk_block_origin_is_none_for_a_chunk_leaving_its_block() {
        let mut section = sample_section();
        section.chunk_rects[0].compact_x = 7; // 2 wide from x 7 passes the 8-wide block
        assert_eq!(section.chunk_block_origin(0), None);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = AnimatedLightWeightMapsSection::from_bytes(&[5, 0, 0, 0, 0, 0]).unwrap_err();
        assert!(err.to_string().contains("too short"));
    }

    #[test]
    fn rejects_truncated_body() {
        let bytes = sample_section().to_bytes();
        let err =
            AnimatedLightWeightMapsSection::from_bytes(&bytes[..bytes.len() - 1]).unwrap_err();
        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn direction_oct_round_trips_per_texel_light() {
        let section = sample_section();
        let restored = AnimatedLightWeightMapsSection::from_bytes(&section.to_bytes()).unwrap();
        for (a, b) in section.texel_lights.iter().zip(&restored.texel_lights) {
            assert_eq!(a.direction_oct, b.direction_oct);
        }
    }
}
