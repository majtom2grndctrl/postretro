// Material placeholder textures: the checkerboard diffuse and the 1×1
// specular, normal and emissive fallbacks every unconsumed slot binds.
// See: context/lib/resource_management.md

use postretro_render_cpu::surface_depth::SurfaceReliefLevels;

use super::{LoadedTexture, upload_texture_data};

const PLACEHOLDER_SIZE: u32 = 64;
const CHECKER_SQUARE: u32 = 8;
const MAGENTA: [u8; 4] = [255, 0, 0xFF, 255];
const BLACK_RGBA: [u8; 4] = [0, 0, 0, 255];

/// The placeholder bound to the specular slot when a material has no `_s.png`
/// and no `_h.png`: a single-channel 1×1 black texel.
///
/// R (specular intensity) is 0 — the zero specular response this placeholder
/// has always provided. G is NOT flat by itself: WGSL expands a single-channel
/// sample to `(r, 0, 0, 1)`, and `g = 0` under the signed encoding is MAXIMUM
/// RAISE. What keeps a material without a height map on its true plane is the
/// has-depth bit, which `build_material_bind_group` sets only for an
/// `Rg8Unorm` slot — and this placeholder must therefore stay a non-`Rg8Unorm`
/// format, so that gate stays clear for it.
///
/// `absent_specular_placeholder_is_never_a_surface_map` pins both facts.
const SPECULAR_PLACEHOLDER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
const SPECULAR_PLACEHOLDER_PIXEL: [u8; 1] = [0];

/// Tangent-space +Z normal encoded as Rgba8Unorm: (0,0,1) → (127,127,255).
/// The 1×1 placeholder stays Rgba8Unorm because BC5 requires a 4×4-block
/// minimum. The shader samples both Rgba8Unorm and Bc5RgUnorm normals as
/// `texture_2d<f32>`, so its `.rg * 2 - 1` decode works for either format.
const NEUTRAL_NORMAL_PIXEL: [u8; 4] = [127, 127, 255, 255];

/// Build a 64×64 RGBA8 magenta/black checkerboard for the diffuse placeholder.
/// Single mip level — the placeholder doesn't need filtering at distance.
pub(super) fn generate_checkerboard_pixels() -> Vec<u8> {
    let pixel_count = (PLACEHOLDER_SIZE * PLACEHOLDER_SIZE) as usize;
    let mut data = Vec::with_capacity(pixel_count * 4);
    for y in 0..PLACEHOLDER_SIZE {
        for x in 0..PLACEHOLDER_SIZE {
            let checker_x = x / CHECKER_SQUARE;
            let checker_y = y / CHECKER_SQUARE;
            let color = if (checker_x + checker_y).is_multiple_of(2) {
                &MAGENTA
            } else {
                &BLACK_RGBA
            };
            data.extend_from_slice(color);
        }
    }
    data
}

pub(super) fn make_diffuse_placeholder(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView) {
    let data = generate_checkerboard_pixels();
    upload_texture_data(
        device,
        queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &[(PLACEHOLDER_SIZE, PLACEHOLDER_SIZE, &data)],
        "Placeholder Diffuse (Checkerboard)",
    )
}

pub(super) fn make_specular_placeholder(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView) {
    upload_texture_data(
        device,
        queue,
        SPECULAR_PLACEHOLDER_FORMAT,
        &[(1, 1, &SPECULAR_PLACEHOLDER_PIXEL[..])],
        "Placeholder Specular (Black 1x1)",
    )
}

pub(super) fn make_normal_placeholder(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView) {
    upload_texture_data(
        device,
        queue,
        wgpu::TextureFormat::Rgba8Unorm,
        &[(1, 1, &NEUTRAL_NORMAL_PIXEL[..])],
        "Placeholder Normal (Neutral 1x1)",
    )
}

pub(super) fn make_emissive_placeholder(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView) {
    upload_texture_data(
        device,
        queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &[(1, 1, &BLACK_RGBA)],
        "Placeholder Emissive (Black sRGB 1x1)",
    )
}

/// All-slot placeholder texture: 64×64 checkerboard diffuse, 1×1 black specular,
/// 1×1 neutral normal, and 1×1 black sRGB emissive. Shared between `load_textures`' per-texture fallback path
/// and the renderer's no-level-loaded bootstrap slot.
pub(in crate::render) fn placeholder_loaded_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> LoadedTexture {
    let (diffuse_texture, diffuse_view) = make_diffuse_placeholder(device, queue);
    let (specular_texture, specular_view) = make_specular_placeholder(device, queue);
    let (normal_texture, normal_view) = make_normal_placeholder(device, queue);
    let (emissive_texture, emissive_view) = make_emissive_placeholder(device, queue);
    LoadedTexture {
        diffuse_texture,
        diffuse_view,
        specular_texture,
        specular_view,
        normal_texture,
        normal_view,
        emissive_texture,
        emissive_view,
        mip_count: 1,
        surface_relief: SurfaceReliefLevels::FLAT,
    }
}

#[cfg(test)]
mod tests {
    use super::super::upload::upload_bytes_per_pixel;
    use super::*;

    /// Surface Depth's central safety property: a material WITHOUT a height
    /// map must render exactly as it did before the feature existed.
    ///
    /// Under the signed encoding the placeholder's expanded `(r, 0, 0, 1)` has
    /// `.g == 0`, which reads as MAXIMUM RAISE, so the placeholder is not flat
    /// by itself. The has-depth gate keeps it on the true plane: the bit is set
    /// only for an `Rg8Unorm` slot. This test pins the placeholder's side of
    /// that bargain — it must stay a black single-channel texel that the gate
    /// never classifies as a surface map. Widening it to two channels would
    /// open the gate and lift every un-mapped material by its full depth.
    #[test]
    fn absent_specular_placeholder_is_never_a_surface_map() {
        assert_eq!(
            SPECULAR_PLACEHOLDER_FORMAT,
            wgpu::TextureFormat::R8Unorm,
            "the specular placeholder must stay single-channel: its expanded g = 0 reads as \
             maximum raise, so only the has-depth gate keeps it flat",
        );
        assert!(
            !crate::render::specular_slot_is_surface_map(SPECULAR_PLACEHOLDER_FORMAT),
            "the has-depth gate must never open for the specular placeholder",
        );
        assert_eq!(
            SPECULAR_PLACEHOLDER_PIXEL,
            [0],
            "the placeholder texel must stay black: R is the zero specular response, and a \
             single-channel texture has no stored G to disturb",
        );
        // One byte per texel, one texel — the placeholder must remain the
        // cheapest possible no-op rather than growing a second channel.
        assert_eq!(
            upload_bytes_per_pixel(SPECULAR_PLACEHOLDER_FORMAT),
            Some(Some(1)),
        );
        assert_eq!(
            SPECULAR_PLACEHOLDER_PIXEL.len(),
            1,
            "placeholder payload must match its format's bytes per texel",
        );
    }

    // Checkerboard placeholder pixel pattern: 64×64 magenta/black, 8-pixel squares.

    #[test]
    fn checkerboard_has_correct_dimensions() {
        let data = generate_checkerboard_pixels();
        assert_eq!(data.len(), 64 * 64 * 4);
    }

    #[test]
    fn checkerboard_top_left_is_magenta() {
        let data = generate_checkerboard_pixels();
        assert_eq!(&data[0..4], &MAGENTA);
    }

    #[test]
    fn checkerboard_alternates_correctly() {
        let data = generate_checkerboard_pixels();
        assert_eq!(&data[0..4], &MAGENTA);
        let offset_8_0 = (8 * 4) as usize;
        assert_eq!(&data[offset_8_0..offset_8_0 + 4], &BLACK_RGBA);
        let offset_16_0 = (16 * 4) as usize;
        assert_eq!(&data[offset_16_0..offset_16_0 + 4], &MAGENTA);
        let offset_0_8 = (8 * 64 * 4) as usize;
        assert_eq!(&data[offset_0_8..offset_0_8 + 4], &BLACK_RGBA);
        let offset_8_8 = ((8 * 64 + 8) * 4) as usize;
        assert_eq!(&data[offset_8_8..offset_8_8 + 4], &MAGENTA);
    }

    #[test]
    fn checkerboard_all_pixels_are_magenta_or_black() {
        let data = generate_checkerboard_pixels();
        for pixel in data.chunks(4) {
            assert!(
                pixel == MAGENTA || pixel == BLACK_RGBA,
                "unexpected pixel: {pixel:?}"
            );
        }
    }
}
