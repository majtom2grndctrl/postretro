// ProbeDepthTiles PRL section (ID 59): sparse per-direction depth moments for
// probes beside thin occluders, the moment-texel reference that points a probe
// at its tile, and the tile texture layout every consumer shares.
// See: context/lib/build_pipeline.md §PRL section IDs · context/lib/rendering_pipeline.md §4

use crate::lightmap::{eof, invalid, read_u16, read_u32};
use crate::octahedral::{encode_unquantized_uv, irradiance_tile_source_texel};

/// The one layout this build reads.
pub const PROBE_DEPTH_TILES_VERSION: u32 = 1;
/// Interior texels per tile side.
pub const PROBE_DEPTH_TILE_EDGE: u32 = 8;
/// Octahedral wrap border on each side, as SH irradiance tiles carry.
pub const PROBE_DEPTH_TILE_BORDER: u32 = 1;
/// Bordered tile side: the stride between tiles in the tile texture.
pub const PROBE_DEPTH_TILE_BORDERED_EDGE: u32 =
    PROBE_DEPTH_TILE_EDGE + 2 * PROBE_DEPTH_TILE_BORDER;
/// Interior texels in one tile, `E[d]` and `E[d²]` each.
pub const PROBE_DEPTH_TILE_INTERIOR_TEXELS: usize =
    (PROBE_DEPTH_TILE_EDGE * PROBE_DEPTH_TILE_EDGE) as usize;
/// Bordered texels in one tile, as stored on the wire and in the texture.
pub const PROBE_DEPTH_TILE_TEXELS: usize =
    (PROBE_DEPTH_TILE_BORDERED_EDGE * PROBE_DEPTH_TILE_BORDERED_EDGE) as usize;
/// Bytes of one `[f16 E[d], f16 E[d²]]` texel (`Rg16Float`).
pub const PROBE_DEPTH_TILE_TEXEL_BYTES: u64 = 4;
/// Most tiles one level carries: the 15 bits a sign-flagged reference holds
/// in the moment texel's 16-bit second channel.
pub const MAX_PROBE_DEPTH_TILES: u32 = 1 << 15;
/// Sign bit of the moment texel's second channel. Set, the low 15 bits name a
/// tile; clear, the channel is the probe's averaged `E[d²]` f16 bits, which
/// the bake never makes negative.
pub const DEPTH_TILE_REFERENCE_FLAG: u16 = 0x8000;

/// version, tile edge, border, tile count.
const HEADER_BYTES: usize = 16;

/// On-disk layout (little-endian, f16 as raw `u16` bits): `u32 version` (1),
/// `u32 tile edge` (8), `u32 border` (1), `u32 tile count` (≤ 32,768), then
/// `tile count` × `u32` dense lattice probe index (ascending, unique), then
/// `tile count` bordered tiles in index order, each 10 × 10 texels of
/// `[E[d], E[d²]]`, row-major. Zero tiles is the header alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeDepthTilesSection {
    /// Dense lattice probe index (`x + y·nx + z·nx·ny`) of each tile, ascending.
    pub probe_indices: Vec<u32>,
    /// Every tile's bordered texels, tile after tile in `probe_indices` order.
    pub texels: Vec<[u16; 2]>,
}

/// What a moment texel's second channel holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MomentSecondChannel {
    /// The probe weighs with its averaged moments; these are its `E[d²]` bits.
    Averaged(u16),
    /// The probe weighs with this tile's per-direction moments.
    Tile(u16),
}

/// The moment texel's second channel for a probe referencing `tile`.
pub fn tile_reference_bits(tile: u16) -> u16 {
    assert!(
        u32::from(tile) < MAX_PROBE_DEPTH_TILES,
        "tile {tile} is past the {MAX_PROBE_DEPTH_TILES}-tile reference range"
    );
    DEPTH_TILE_REFERENCE_FLAG | tile
}

/// Decode a moment texel's second channel from its raw bits. The flag is the
/// raw sign bit: tile 0 is f16 −0, and the top tiles are −inf or NaN as floats.
pub fn decode_moment_second_channel(bits: u16) -> MomentSecondChannel {
    if bits & DEPTH_TILE_REFERENCE_FLAG != 0 {
        MomentSecondChannel::Tile(bits & !DEPTH_TILE_REFERENCE_FLAG)
    } else {
        MomentSecondChannel::Averaged(bits)
    }
}

/// Whether either averaged id-34 moment carries a set sign bit. A valid probe
/// that does would read as a tile reference, so its level is refused.
pub fn moment_sign_bit_set(mean_distance: u16, mean_sq_distance: u16) -> bool {
    (mean_distance | mean_sq_distance) & DEPTH_TILE_REFERENCE_FLAG != 0
}

impl ProbeDepthTilesSection {
    pub fn tile_count(&self) -> usize {
        self.probe_indices.len()
    }

    /// Bordered texels of tile `tile`.
    pub fn tile(&self, tile: usize) -> &[[u16; 2]] {
        &self.texels[tile * PROBE_DEPTH_TILE_TEXELS..(tile + 1) * PROBE_DEPTH_TILE_TEXELS]
    }

    /// Tile index of dense probe `probe`, if it has one.
    pub fn tile_of_probe(&self, probe: u32) -> Option<u16> {
        self.probe_indices
            .binary_search(&probe)
            .ok()
            .map(|tile| tile as u16)
    }

    /// The texture the runtime holds these tiles in; `None` for zero tiles.
    pub fn texture_layout(&self) -> Option<ProbeDepthTileTextureLayout> {
        ProbeDepthTileTextureLayout::for_tile_count(self.tile_count() as u32)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let count = self.tile_count();
        debug_assert!(count as u32 <= MAX_PROBE_DEPTH_TILES);
        debug_assert_eq!(self.texels.len(), count * PROBE_DEPTH_TILE_TEXELS);
        debug_assert!(self.probe_indices.windows(2).all(|w| w[0] < w[1]));
        let mut out = Vec::with_capacity(
            HEADER_BYTES + count * 4 + self.texels.len() * PROBE_DEPTH_TILE_TEXEL_BYTES as usize,
        );
        for word in [
            PROBE_DEPTH_TILES_VERSION,
            PROBE_DEPTH_TILE_EDGE,
            PROBE_DEPTH_TILE_BORDER,
            count as u32,
        ] {
            out.extend_from_slice(&word.to_le_bytes());
        }
        for index in &self.probe_indices {
            out.extend_from_slice(&index.to_le_bytes());
        }
        for [mean, mean_sq] in &self.texels {
            out.extend_from_slice(&mean.to_le_bytes());
            out.extend_from_slice(&mean_sq.to_le_bytes());
        }
        out
    }

    /// Structural decode: version first, so a later layout of any length reads
    /// as a re-bake; then tile shape, count, index order and exact length.
    /// Lattice checks need id 34: [`Self::validate_against_lattice`].
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(eof("ProbeDepthTiles section too short for its version"));
        }
        let version = read_u32(data, 0);
        if version != PROBE_DEPTH_TILES_VERSION {
            return Err(invalid(format!(
                "ProbeDepthTiles version {version} is unknown; this build reads version \
                 {PROBE_DEPTH_TILES_VERSION}; re-bake the level"
            )));
        }
        if data.len() < HEADER_BYTES {
            return Err(eof(format!(
                "ProbeDepthTiles section is {} bytes, shorter than its {HEADER_BYTES}-byte header",
                data.len()
            )));
        }
        let edge = read_u32(data, 4);
        if edge != PROBE_DEPTH_TILE_EDGE {
            return Err(invalid(format!(
                "ProbeDepthTiles tile edge {edge} is unsupported; expected {PROBE_DEPTH_TILE_EDGE}"
            )));
        }
        let border = read_u32(data, 8);
        if border != PROBE_DEPTH_TILE_BORDER {
            return Err(invalid(format!(
                "ProbeDepthTiles border {border} is unsupported; expected {PROBE_DEPTH_TILE_BORDER}"
            )));
        }
        let count = read_u32(data, 12);
        if count > MAX_PROBE_DEPTH_TILES {
            return Err(invalid(format!(
                "ProbeDepthTiles carries {count} tiles, past the {MAX_PROBE_DEPTH_TILES}-tile cap"
            )));
        }
        let count = count as usize;
        let expected = HEADER_BYTES
            + count * 4
            + count * PROBE_DEPTH_TILE_TEXELS * PROBE_DEPTH_TILE_TEXEL_BYTES as usize;
        if data.len() != expected {
            let message = format!(
                "ProbeDepthTiles section is {} bytes, expected {expected} for {count} tiles",
                data.len()
            );
            return Err(if data.len() < expected {
                eof(message)
            } else {
                invalid(message)
            });
        }
        let probe_indices: Vec<u32> = (0..count)
            .map(|i| read_u32(data, HEADER_BYTES + i * 4))
            .collect();
        if let Some(pair) = probe_indices.windows(2).find(|w| w[0] >= w[1]) {
            return Err(invalid(format!(
                "ProbeDepthTiles probe indices must ascend without repeats; {} follows {}",
                pair[1], pair[0]
            )));
        }
        let texel_start = HEADER_BYTES + count * 4;
        let texels = (0..count * PROBE_DEPTH_TILE_TEXELS)
            .map(|i| {
                let at = texel_start + i * PROBE_DEPTH_TILE_TEXEL_BYTES as usize;
                [read_u16(data, at), read_u16(data, at + 2)]
            })
            .collect();
        Ok(Self {
            probe_indices,
            texels,
        })
    }

    /// Every tile names a valid probe inside id 34's lattice. `probe_is_valid`
    /// reads id 34's validity for a dense index already inside the lattice.
    pub fn validate_against_lattice(
        &self,
        grid_dimensions: [u32; 3],
        probe_is_valid: impl Fn(u32) -> bool,
    ) -> crate::Result<()> {
        let total = grid_dimensions
            .iter()
            .try_fold(1u64, |acc, &d| acc.checked_mul(u64::from(d)))
            .unwrap_or(u64::MAX);
        for &index in &self.probe_indices {
            if u64::from(index) >= total {
                return Err(invalid(format!(
                    "ProbeDepthTiles probe index {index} is outside the {}×{}×{} probe lattice",
                    grid_dimensions[0], grid_dimensions[1], grid_dimensions[2]
                )));
            }
            if !probe_is_valid(index) {
                return Err(invalid(format!(
                    "ProbeDepthTiles probe index {index} names an invalid probe"
                )));
            }
        }
        Ok(())
    }
}

/// Where tiles sit in the runtime's one `Rg16Float` tile texture: a power of
/// two tiles per row, so the shader finds a tile with a shift, a mask and one
/// multiply by the bordered stride. Rows run only as far as the tiles need.
/// The build summary and the renderer's allocation ledger both price tiles
/// from this layout, so they agree by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeDepthTileTextureLayout {
    /// `log2` of tiles per row.
    pub tiles_per_row_shift: u32,
    /// Rows of tiles.
    pub rows: u32,
}

impl ProbeDepthTileTextureLayout {
    /// The layout for `tile_count` tiles: the smallest power-of-two row at
    /// least as wide as the tiles' square root. `None` for zero tiles, which
    /// allocate nothing.
    pub fn for_tile_count(tile_count: u32) -> Option<Self> {
        if tile_count == 0 {
            return None;
        }
        let mut shift = 0u32;
        while (1u64 << (2 * shift)) < u64::from(tile_count) {
            shift += 1;
        }
        let per_row = 1u32 << shift;
        Some(Self {
            tiles_per_row_shift: shift,
            rows: tile_count.div_ceil(per_row),
        })
    }

    pub fn tiles_per_row(&self) -> u32 {
        1 << self.tiles_per_row_shift
    }

    /// Texture extent in texels, width then height.
    pub fn extent(&self) -> [u32; 2] {
        [
            self.tiles_per_row() * PROBE_DEPTH_TILE_BORDERED_EDGE,
            self.rows * PROBE_DEPTH_TILE_BORDERED_EDGE,
        ]
    }

    /// Bytes the tile texture allocates.
    pub fn texture_bytes(&self) -> u64 {
        let [width, height] = self.extent();
        u64::from(width) * u64::from(height) * PROBE_DEPTH_TILE_TEXEL_BYTES
    }

    /// Texel origin of tile `tile`'s bordered square.
    pub fn tile_origin(&self, tile: u32) -> [u32; 2] {
        let column = tile & (self.tiles_per_row() - 1);
        let row = tile >> self.tiles_per_row_shift;
        [
            column * PROBE_DEPTH_TILE_BORDERED_EDGE,
            row * PROBE_DEPTH_TILE_BORDERED_EDGE,
        ]
    }
}

/// Bytes the tile texture for `tile_count` tiles allocates; 0 for none.
pub fn probe_depth_tile_texture_bytes(tile_count: u32) -> u64 {
    ProbeDepthTileTextureLayout::for_tile_count(tile_count)
        .map_or(0, |layout| layout.texture_bytes())
}

/// Interior texel `(x + y·8)` a direction falls in, on the octahedral mapping
/// SH tiles use: interior texel `i` spans `[i/8, (i+1)/8)` of the unfolded
/// square. `dir` need not be normalized.
pub fn interior_texel_of_direction(dir: [f32; 3]) -> usize {
    let [u, v] = encode_unquantized_uv(dir[0], dir[1], dir[2]);
    let edge = PROBE_DEPTH_TILE_EDGE as f32;
    let cell = |c: f32| ((c * edge) as u32).min(PROBE_DEPTH_TILE_EDGE - 1) as usize;
    cell(v) * PROBE_DEPTH_TILE_EDGE as usize + cell(u)
}

/// A bordered tile from its interior texels (row-major, 8 × 8): the border
/// copies the octahedral wrap neighbour, as SH irradiance tiles do.
pub fn bordered_tile(interior: &[[u16; 2]]) -> Vec<[u16; 2]> {
    assert_eq!(interior.len(), PROBE_DEPTH_TILE_INTERIOR_TEXELS);
    let bordered = PROBE_DEPTH_TILE_BORDERED_EDGE;
    let mut out = Vec::with_capacity(PROBE_DEPTH_TILE_TEXELS);
    for y in 0..bordered {
        for x in 0..bordered {
            let [sx, sy] =
                irradiance_tile_source_texel(x, y, bordered, PROBE_DEPTH_TILE_BORDER);
            out.push(interior[(sy * PROBE_DEPTH_TILE_EDGE + sx) as usize]);
        }
    }
    out
}

/// `(E[d], E[d²])` of a bordered tile toward `dir`, filtered as the runtime's
/// bilinear sampler reads it: coordinate `border + uv · edge` within the tile,
/// taps on the texel centres either side. The CPU mirror for the bake's tile
/// selection and the reflection capture. `dir` need not be normalized.
pub fn sample_tile_moments(tile: &[[u16; 2]], dir: [f32; 3]) -> [f32; 2] {
    debug_assert_eq!(tile.len(), PROBE_DEPTH_TILE_TEXELS);
    let [u, v] = encode_unquantized_uv(dir[0], dir[1], dir[2]);
    let border = PROBE_DEPTH_TILE_BORDER as f32;
    let edge = PROBE_DEPTH_TILE_EDGE as f32;
    let fx = border + u * edge - 0.5;
    let fy = border + v * edge - 0.5;
    let (x0, y0) = (fx.floor(), fy.floor());
    let (ax, ay) = (fx - x0, fy - y0);
    let last = PROBE_DEPTH_TILE_BORDERED_EDGE as i32 - 1;
    let clamp = |c: f32| (c as i32).clamp(0, last) as usize;
    let (x0, y0) = (clamp(x0), clamp(y0));
    let (x1, y1) = (
        (x0 + 1).min(last as usize),
        (y0 + 1).min(last as usize),
    );
    let stride = PROBE_DEPTH_TILE_BORDERED_EDGE as usize;
    let mut out = [0.0f32; 2];
    for (x, y, w) in [
        (x0, y0, (1.0 - ax) * (1.0 - ay)),
        (x1, y0, ax * (1.0 - ay)),
        (x0, y1, (1.0 - ax) * ay),
        (x1, y1, ax * ay),
    ] {
        let [mean, mean_sq] = tile[y * stride + x];
        out[0] += w * f16_bits_to_f32(mean);
        out[1] += w * f16_bits_to_f32(mean_sq);
    }
    out
}

/// IEEE 754 binary16 bits → f32, exact.
pub fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0f32 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        0x1f if mantissa == 0.0 => sign * f32::INFINITY,
        0x1f => f32::NAN,
        _ => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

#[cfg(test)]
#[path = "probe_depth_tiles_tests.rs"]
mod tests;
