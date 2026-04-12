// Format-agnostic vertex and texture types shared by the PRL loader and renderer.
// See: context/lib/rendering_pipeline.md §6
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexturedVertex {
    pub position: [f32; 3],
    pub base_uv: [f32; 2],
    pub vertex_color: [f32; 4],
}

impl TexturedVertex {
    /// Stride in bytes: 3 + 2 + 4 = 9 floats * 4 bytes = 36 bytes.
    pub const STRIDE: usize = 36;
}

/// A contiguous run of indices within a leaf that share the same texture.
/// Pre-computed at load time to avoid per-frame sorting.
#[derive(Debug, Clone)]
pub struct TextureSubRange {
    /// Texture index (or `u32::MAX` for faces with no texture).
    pub texture_index: u32,
    /// Offset into the index buffer.
    pub index_offset: u32,
    /// Number of indices in this sub-range.
    pub index_count: u32,
}
