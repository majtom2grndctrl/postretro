// CPU-side UI texture data: PNG-decoded RGBA8 used by splash and 2D blits.
// World-material textures live in `render::loaded_texture` (wgpu handles).
// See: context/lib/resource_management.md

/// CPU-side decoded RGBA8 texture for UI surfaces (splash, HUD, 2D blits).
/// World materials use `render::loaded_texture::LoadedTexture` instead.
#[derive(Debug, Clone)]
pub struct UiTexture {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
