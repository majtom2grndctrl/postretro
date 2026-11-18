//! Splits a record's planes into comparison categories: chart grid versus
//! padding ring, mask plane A versus B. The writer hashes each category and
//! the comparator walks the same split to name a first differing texel, so
//! the two can never disagree about which texel belongs where.

use super::Category;
use super::format::{ChartMeta, PlaneKind};

/// Bytes of one irradiance texel: four f32.
const IRRADIANCE_TEXEL_BYTES: usize = 16;
const DIRECTION_TEXEL_BYTES: usize = 2;
const MASK_TEXEL_BYTES: usize = 4;
/// One animated weight entry: u32 light, f32 weight, two u16 direction.
const ANIMATED_ENTRY_BYTES: usize = 12;

/// Call `visit(category, [x, y], texel_bytes)` for every texel of `plane`,
/// row-major. A mask texel visits twice: plane A's two channels, then B's.
/// Fails when `bytes` does not match `meta`'s extents.
pub(super) fn visit_texels(
    kind: PlaneKind,
    meta: &ChartMeta,
    bytes: &[u8],
    mut visit: impl FnMut(Category, [u32; 2], &[u8]),
) -> Result<(), String> {
    let (width, height) = (meta.width, meta.height);
    let texels = width as usize * height as usize;
    let in_ring = |x: u32, y: u32| {
        let p = meta.padding;
        x < p || y < p || x + p >= width || y + p >= height
    };
    let expect_len = |texel_bytes: usize, count: usize| {
        if bytes.len() == texel_bytes * count {
            Ok(())
        } else {
            Err(format!(
                "{} bytes, expected {count} texels of {texel_bytes}",
                bytes.len()
            ))
        }
    };
    match kind {
        PlaneKind::Irradiance => {
            expect_len(IRRADIANCE_TEXEL_BYTES, texels)?;
            for (index, texel) in bytes
                .as_chunks::<IRRADIANCE_TEXEL_BYTES>()
                .0
                .iter()
                .enumerate()
            {
                let (x, y) = (index as u32 % width, index as u32 / width);
                let category = if in_ring(x, y) {
                    Category::IrradianceRing
                } else {
                    Category::IrradianceGrid
                };
                visit(category, [x, y], texel);
            }
        }
        PlaneKind::Direction => {
            let [dir_w, dir_h] = meta.direction_dims;
            expect_len(DIRECTION_TEXEL_BYTES, dir_w as usize * dir_h as usize)?;
            for (index, texel) in bytes
                .as_chunks::<DIRECTION_TEXEL_BYTES>()
                .0
                .iter()
                .enumerate()
            {
                visit(
                    Category::Direction,
                    [index as u32 % dir_w, index as u32 / dir_w],
                    texel,
                );
            }
        }
        PlaneKind::Mask => {
            expect_len(MASK_TEXEL_BYTES, texels)?;
            for (index, texel) in bytes.as_chunks::<MASK_TEXEL_BYTES>().0.iter().enumerate() {
                let (x, y) = (index as u32 % width, index as u32 / width);
                let ring = in_ring(x, y);
                let (a, b) = if ring {
                    (Category::MaskARing, Category::MaskBRing)
                } else {
                    (Category::MaskAGrid, Category::MaskBGrid)
                };
                visit(a, [x, y], &texel[..2]);
                visit(b, [x, y], &texel[2..]);
            }
        }
        PlaneKind::AnimatedWeights => {
            let mut at = 0usize;
            for index in 0..texels {
                let count_bytes = bytes
                    .get(at..at + 4)
                    .ok_or_else(|| format!("texel {index} has no entry count"))?;
                let count = u32::from_le_bytes(count_bytes.try_into().expect("four bytes"));
                let end = at + 4 + count as usize * ANIMATED_ENTRY_BYTES;
                let texel = bytes
                    .get(at..end)
                    .ok_or_else(|| format!("texel {index} runs past the plane"))?;
                visit(
                    Category::AnimatedWeights,
                    [index as u32 % width, index as u32 / width],
                    texel,
                );
                at = end;
            }
            if at != bytes.len() {
                return Err(format!("{} trailing bytes", bytes.len() - at));
            }
        }
    }
    Ok(())
}

/// blake3 of each of `kind`'s categories over its texels in row-major order.
pub(super) fn category_hashes(
    kind: PlaneKind,
    meta: &ChartMeta,
    bytes: &[u8],
) -> Result<Vec<(Category, [u8; 32])>, String> {
    let categories = kind.categories();
    let mut hashers: Vec<blake3::Hasher> =
        categories.iter().map(|_| blake3::Hasher::new()).collect();
    visit_texels(kind, meta, bytes, |category, _, texel| {
        let slot = categories
            .iter()
            .position(|&c| c == category)
            .expect("a plane visits only its own categories");
        hashers[slot].update(texel);
    })?;
    Ok(categories
        .iter()
        .zip(hashers)
        .map(|(&category, hasher)| (category, *hasher.finalize().as_bytes()))
        .collect())
}

/// Each texel's position and bytes.
#[cfg(test)]
pub(super) type CategoryTexels = Vec<([u32; 2], Vec<u8>)>;

/// Every texel of `category` in `plane`, in visit order.
#[cfg(test)]
pub(super) fn category_texels(
    category: Category,
    meta: &ChartMeta,
    bytes: &[u8],
) -> Result<CategoryTexels, String> {
    let mut texels = Vec::new();
    visit_texels(PlaneKind::of(category), meta, bytes, |c, at, texel| {
        if c == category {
            texels.push((at, texel.to_vec()));
        }
    })?;
    Ok(texels)
}
