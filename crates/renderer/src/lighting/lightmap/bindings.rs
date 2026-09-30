// Group-4 lightmap binding layout (binding numbers, BGL entries, the linear
// sampler, the animated block-table uniform bytes, the bind group builder)
// and the group-6 vertex block-table layout.
// See: context/lib/rendering_pipeline.md §4

use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::animated_lightmap_atlas::{
    ANIMATED_BLOCK_CAP, ANIMATED_BLOCK_TABLE_BYTES_PER_BLOCK, ANIMATED_BLOCK_TABLE_HEADER_BYTES,
    ANIMATED_BLOCK_TABLE_UNIFORM_BYTES,
};

/// Group 4 bindings. The layout is fixed — the fragment shader's
/// `@binding` decorators must match these values.
pub const BIND_IRRADIANCE: u32 = 0;
pub const BIND_DIRECTION: u32 = 1;
/// Non-filtering (Nearest) sampler. Used for the octahedral direction texture:
/// linear interpolation of octahedral-encoded unit vectors does not commute
/// with slerp, so the direction channel must stay nearest.
pub const BIND_SAMPLER: u32 = 2;
/// Animated-light contribution atlas (Rgba16Float). Composed each frame by
/// `render::animated_lightmap`; forward pass samples alongside the static
/// irradiance pool. See: context/lib/rendering_pipeline.md §4
pub const BIND_ANIMATED_ATLAS: u32 = 3;
/// Filtering (Linear) sampler. Used for the irradiance pool, the animated
/// atlas and the shadowmask pool so baked penumbra ramps read as continuous
/// gradients under magnification instead of stair-stepping at texel
/// boundaries. `Rgba16Float`
/// linear-filterability is a hard runtime requirement checked at init
/// (see `atlas_format_filterable`; see also `rendering_pipeline.md §4`).
pub const BIND_FILTERING_SAMPLER: u32 = 4;
/// Animated dominant-direction atlas (Rgba8Unorm: octahedral direction in `.rg`,
/// coverage flag in `.a`). Composed each frame alongside the animated irradiance
/// atlas; the forward pass samples it to apply bumped-Lambert normal-map correction
/// to the animated term. Sampled through the nearest sampler at binding 2 — oct
/// directions must not be linearly interpolated. Binding 5 here (group 4, forward
/// pass) and binding 8 in the compose shader are independent numbering spaces for
/// the same atlas.
pub const BIND_ANIMATED_DIRECTION: u32 = 5;
/// Static-light shadowmask pool (BC5 `.rg`, two mask groups side by side at
/// twice the pool width), layer- and offset-matched to the irradiance pool.
/// Sampled by forward union-subtraction and static world-specular visibility.
pub const BIND_SHADOWMASK_ATLAS: u32 = 6;
/// Animated block table for the forward shader: where each animated face's
/// block sits in the compact atlas. FRAGMENT-only uniform.
pub const BIND_ANIMATED_BLOCK_TABLE: u32 = 7;

/// Bytes of the binding-7 uniform, the same for every level: the forward
/// shader declares a fixed-length table sized to the shared block cap.
pub(crate) const ANIMATED_BLOCK_TABLE_BYTES: usize = ANIMATED_BLOCK_TABLE_UNIFORM_BYTES as usize;

/// Build the binding-7 block-table uniform for the installed section, or an
/// empty table when `section` is `None`. An empty table resolves every vertex
/// to no block, whatever ids the level's vertices carry. Layout (native-endian
/// u32s, as the GPU reads them, mirroring `AnimatedBlockTable` in forward.wgsl):
/// `0 (reserved), page_size, block_count, 0`, then per block the packed
/// `(i16 dx, i16 dy)` compact − block-local texel offset and the page. The
/// forward stage adds that offset to the fragment's block-local texel.
pub(crate) fn animated_block_table_bytes(
    section: Option<&AnimatedLightWeightMapsSection>,
) -> Vec<u8> {
    let mut bytes = vec![0_u8; ANIMATED_BLOCK_TABLE_BYTES];
    let Some(section) = section else {
        return bytes;
    };
    assert!(
        section.blocks.len() <= ANIMATED_BLOCK_CAP as usize,
        "section 25 consistency validation bounds the block count to the table cap"
    );
    let mut write = |at: usize, value: u32| bytes[at..at + 4].copy_from_slice(&value.to_ne_bytes());
    write(4, section.page_size);
    write(8, section.blocks.len() as u32);
    for (index, block) in section.blocks.iter().enumerate() {
        let dx = texel_offset(block.block_x, block.compact_x);
        let dy = texel_offset(block.block_y, block.compact_y);
        let at = ANIMATED_BLOCK_TABLE_HEADER_BYTES as usize
            + index * ANIMATED_BLOCK_TABLE_BYTES_PER_BLOCK as usize;
        write(at, u32::from(dx as u16) | (u32::from(dy as u16) << 16));
        write(at + 4, block.compact_layer);
    }
    bytes
}

/// Block-local→compact translation on one axis. A block-local coordinate lies
/// inside a 2048-texel cell block and a compact one inside an 8192-texel page,
/// so the difference always fits an `i16`.
fn texel_offset(block_coord: u16, compact_coord: u32) -> i16 {
    i16::try_from(i64::from(compact_coord) - i64::from(block_coord))
        .expect("block-local and compact coordinates lie within one 8192-texel page")
}

/// Group 6, binding 0: the lightmap block table the forward vertex stage
/// reads, one `vec4<u32>` per `WorldVertex::lightmap_block`
/// (see `postretro_render_cpu::lightmap_pool::BlockTableEntry`).
pub const BIND_BLOCK_TABLE: u32 = 0;

/// VERTEX-only: the vertex stage resolves each vertex's block and hands the
/// fragment stage flat varyings, so no fragment binding is spent.
pub(crate) fn block_table_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 1] {
    [wgpu::BindGroupLayoutEntry {
        binding: BIND_BLOCK_TABLE,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(
                postretro_render_cpu::lightmap_pool::BLOCK_TABLE_ENTRY_BYTES as u64,
            ),
        },
        count: None,
    }]
}

/// The linear lightmap sampler. Clamp-to-edge guards only the texture
/// border: the block-rect clamp in `lightmap_pool_uv` keeps bilinear taps
/// inside the block, and so inside the block's own half of the shadowmask
/// pool.
pub(crate) fn filtering_sampler_descriptor() -> wgpu::SamplerDescriptor<'static> {
    wgpu::SamplerDescriptor {
        label: Some("Lightmap Sampler (Linear)"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    }
}

pub(crate) fn bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 8] {
    // Two samplers (binding 2 nearest, binding 4 linear), split by what each
    // texture needs:
    //   - Irradiance (0) and animated atlas (3) are `Rgba16Float`, which is
    //     filterable in core WebGPU (only 32-bit float formats need the
    //     `float32-filterable` feature). Marked `filterable: true` and always
    //     sampled through the linear sampler so baked penumbra ramps read as
    //     continuous gradients instead of stair-stepping at texel boundaries.
    //   - Direction (1) and animated direction (5) stay `filterable: false` on
    //     the nearest sampler: linear interpolation of direction vectors does
    //     not commute with slerp (both atlases are octahedral-encoded, so
    //     both must read nearest).
    // There is one pipeline variant; the BGL is fixed. No fallback path.
    [
        wgpu::BindGroupLayoutEntry {
            binding: BIND_IRRADIANCE,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                // `texture_2d_array`: the pool's layers hold cell blocks; the
                // forward shader samples at a block's pool layer and offset.
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: BIND_DIRECTION,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                // `texture_2d_array`, layer-matched to the irradiance pool.
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: BIND_SAMPLER,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
            count: None,
        },
        // Animated-light contribution atlas (Rgba16Float) — filterable in core
        // WebGPU, always sampled through the linear sampler at binding 4.
        wgpu::BindGroupLayoutEntry {
            binding: BIND_ANIMATED_ATLAS,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: BIND_FILTERING_SAMPLER,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        // Animated dominant-direction atlas (Rgba8Unorm: octahedral in `.rg`,
        // coverage in `.a`) — `filterable: false`, nearest sampler at binding 2,
        // mirroring the static direction atlas (1).
        wgpu::BindGroupLayoutEntry {
            binding: BIND_ANIMATED_DIRECTION,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: BIND_SHADOWMASK_ATLAS,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: BIND_ANIMATED_BLOCK_TABLE,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
    ]
}

/// Everything group 4 binds besides the three static pool textures, kept so
/// a streamed pool's growth can rebind its new generation.
pub(crate) struct Group4Bindings {
    pub(crate) layout: wgpu::BindGroupLayout,
    /// Nearest, for the octahedral direction textures.
    pub(crate) sampler: wgpu::Sampler,
    /// Linear, for irradiance, the animated atlas and the shadowmask.
    pub(crate) filtering_sampler: wgpu::Sampler,
    pub(crate) animated_atlas_view: wgpu::TextureView,
    pub(crate) animated_direction_view: wgpu::TextureView,
    pub(crate) animated_block_table: wgpu::Buffer,
}

impl Group4Bindings {
    /// Group 4 over the static irradiance, direction and shadowmask textures.
    /// Bindings 0/1/6 declare `D2Array`; pinning the view dimension keeps a
    /// one-layer pool or placeholder aligned with the BGL.
    pub(crate) fn bind(
        &self,
        device: &wgpu::Device,
        irradiance: &wgpu::Texture,
        direction: &wgpu::Texture,
        shadowmask: &wgpu::Texture,
    ) -> wgpu::BindGroup {
        let array_view = |texture: &wgpu::Texture| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let irr_view = array_view(irradiance);
        let dir_view = array_view(direction);
        let shadowmask_view = array_view(shadowmask);
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lightmap Bind Group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: BIND_IRRADIANCE,
                    resource: wgpu::BindingResource::TextureView(&irr_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_DIRECTION,
                    resource: wgpu::BindingResource::TextureView(&dir_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_SAMPLER,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_ANIMATED_ATLAS,
                    resource: wgpu::BindingResource::TextureView(&self.animated_atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_FILTERING_SAMPLER,
                    resource: wgpu::BindingResource::Sampler(&self.filtering_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_ANIMATED_DIRECTION,
                    resource: wgpu::BindingResource::TextureView(&self.animated_direction_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_SHADOWMASK_ATLAS,
                    resource: wgpu::BindingResource::TextureView(&shadowmask_view),
                },
                wgpu::BindGroupEntry {
                    binding: BIND_ANIMATED_BLOCK_TABLE,
                    resource: self.animated_block_table.as_entire_binding(),
                },
            ],
        })
    }
}
