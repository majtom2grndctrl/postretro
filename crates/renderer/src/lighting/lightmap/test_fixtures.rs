// Cell-block lightmap fixtures shared by the lightmap unit tests and the
// renderer's pool GPU harnesses: an id-22 index, an optional id-42 index, and
// the all-resident payloads the loader would hand install.
// See: context/lib/testing_guide.md §4

use postretro_level_format::lightmap::{
    IRRADIANCE_FORMAT_RGBA16F, LightmapBlock, LightmapBlockIndex, LightmapBlockPayload,
    LightmapMode, LightmapSection, f32_to_f16_bits,
};
use postretro_level_format::shadowmask_atlas::{ShadowmaskAtlasSection, ShadowmaskBlockIndex};

pub(crate) struct BlockFixture {
    pub(crate) index: LightmapBlockIndex,
    pub(crate) shadowmask: Option<ShadowmaskBlockIndex>,
    pub(crate) payloads: Vec<LightmapBlockPayload>,
}

/// Per-texel and per-BC-block contents of a fixture.
pub(crate) struct FixtureTexels<'a> {
    /// `(block, x, y)` → RGBA irradiance, stored as Rgba16Float.
    pub(crate) irradiance: &'a dyn Fn(usize, u32, u32) -> [f32; 4],
    /// `(block, direction x, direction y)` → Rg8 octahedral bytes.
    pub(crate) direction: &'a dyn Fn(usize, u32, u32) -> [u8; 2],
    /// `(block, mask slot, BC block x, BC block y)` → the slot's constant
    /// byte over that 4×4 BC block. `None` leaves id 42 out.
    pub(crate) shadowmask: Option<&'a dyn Fn(usize, u32, u32, u32) -> u8>,
}

/// Zero-valued texels with a shadowmask: shape-only fixtures.
pub(crate) fn zero_texels() -> FixtureTexels<'static> {
    FixtureTexels {
        irradiance: &|_, _, _| [0.0; 4],
        direction: &|_, _, _| [128, 255],
        shadowmask: Some(&|_, _, _, _| 255),
    }
}

/// A BC4 block whose every texel reads `value`: equal endpoints.
fn constant_bc4(value: u8) -> [u8; 8] {
    [value, value, 0, 0, 0, 0, 0, 0]
}

/// Rgba16Float cell blocks of `extents` with `scale`-texel direction, built
/// through `LightmapSection` so the index's blob ranges are the ones the wire
/// format writes.
pub(crate) fn block_fixture(
    extents: &[(u32, u32)],
    scale: u32,
    texels: &FixtureTexels<'_>,
) -> BlockFixture {
    let blocks: Vec<LightmapBlock> = extents
        .iter()
        .enumerate()
        .map(|(block, &(width, height))| {
            let mut irradiance = Vec::with_capacity((width * height * 8) as usize);
            for y in 0..height {
                for x in 0..width {
                    for channel in (texels.irradiance)(block, x, y) {
                        irradiance.extend_from_slice(&f32_to_f16_bits(channel).to_le_bytes());
                    }
                }
            }
            let mut direction = Vec::new();
            for y in 0..height / scale {
                for x in 0..width / scale {
                    direction.extend_from_slice(&(texels.direction)(block, x, y));
                }
            }
            LightmapBlock {
                cell_id: block as u32,
                width: width as u16,
                height: height as u16,
                irradiance,
                direction,
            }
        })
        .collect();
    let section = LightmapSection {
        direction_texel_scale: scale,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        mode: LightmapMode::Shadowed,
        blocks,
    };
    let index = section.index();
    let groups: Option<Vec<[Vec<u8>; 2]>> = texels.shadowmask.map(|mask| {
        extents
            .iter()
            .enumerate()
            .map(|(block, &(width, height))| {
                std::array::from_fn(|group| {
                    let mut plane = Vec::new();
                    for by in 0..height / 4 {
                        for bx in 0..width / 4 {
                            let slot = group as u32 * 2;
                            plane.extend_from_slice(&constant_bc4(mask(block, slot, bx, by)));
                            plane.extend_from_slice(&constant_bc4(mask(block, slot + 1, bx, by)));
                        }
                    }
                    plane
                })
            })
            .collect()
    });
    let shadowmask = groups.as_ref().map(|groups| {
        ShadowmaskAtlasSection {
            channels: vec![0, 1, 2, 3],
            blocks: groups.clone(),
        }
        .index()
    });
    let payloads = section
        .blocks
        .into_iter()
        .enumerate()
        .map(|(block, b)| LightmapBlockPayload {
            irradiance: b.irradiance,
            direction: b.direction,
            shadowmask: groups.as_ref().map(|groups| groups[block].clone()),
        })
        .collect();
    BlockFixture {
        index,
        shadowmask,
        payloads,
    }
}
