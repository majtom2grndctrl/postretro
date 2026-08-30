// Billboard sprite rendering pass: camera-facing quads for scripted
// `BillboardEmitterComponent` particles, expanded in the vertex shader from
// a storage buffer of per-sprite instance data. Lit by the full lighting
// stack (SH ambient + static multi-source specular via the chunk list +
// dynamic diffuse). Alpha-additive blend, depth test enabled, depth write
// disabled.
//
// See: context/lib/rendering_pipeline.md §7.4

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::path::Path;

use wgpu::util::DeviceExt;

use postretro_level_format::prm::{
    PORTABLE_MAX_TEXTURE_ARRAY_LAYERS, PrmFile, PrmFormat, PrmHeader, PrmReadError, PrmSlot,
    PrmSlots, cache_filename_for_key,
};
use postretro_level_format::sprite_collection::{
    SpriteSlot, collection_frame_paths, sprite_collection_filename_key,
};
use postretro_render_cpu::loaded_texture::{header_mip_count, slot_layer_levels};
use postretro_render_cpu::smoke::{SPRITE_INSTANCE_SIZE, SpriteFrame};

use super::loaded_texture::{prm_format_to_wgpu, upload_texture_array_data};

/// Byte size of `SpriteDrawParams` (two `vec4<f32>` values = 32 B).
///
/// The first `vec4` remains byte-for-byte compatible with the former layout;
/// `params2` starts at byte 16.
pub const SPRITE_DRAW_PARAMS_SIZE: usize = 32;

/// Level-install policy and draw constants for one sprite collection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteCollectionRegistration {
    /// True only when the collection comes from a map-authored billboard emitter.
    pub baked_sidecar_eligible: bool,
    /// Per-collection static-light specular response.
    pub spec_intensity: f32,
    /// Per-collection static-light Blinn-Phong exponent.
    pub spec_exponent: f32,
    /// Animation loop and particle lifetime in seconds.
    pub lifetime: f32,
    /// Additive HDR self-illumination strength.
    pub emissive: f32,
}

/// Storage-buffer dynamic-offset alignment required by wgpu / WebGPU
/// (`min_storage_buffer_offset_alignment`, 256 on every targeted backend).
/// Each collection's region in the shared instance buffer starts at a multiple
/// of this so it can be addressed by a group-6 dynamic offset. The 32-byte
/// per-instance stride is unchanged *within* a region (256 is a multiple of
/// 32, so the alignment padding is always a whole number of instance slots).
const STORAGE_DYNAMIC_OFFSET_ALIGNMENT: usize = 256;

/// Round `bytes` up to the next multiple of `STORAGE_DYNAMIC_OFFSET_ALIGNMENT`.
fn align_up_to_dynamic_offset(bytes: usize) -> usize {
    bytes.div_ceil(STORAGE_DYNAMIC_OFFSET_ALIGNMENT) * STORAGE_DYNAMIC_OFFSET_ALIGNMENT
}

/// Build the group-6 bind group over a fixed-size *window* of the instance
/// buffer. The binding is declared `has_dynamic_offset: true`, so this single
/// bind group is reused for every collection in a frame —
/// `set_bind_group(6, .., &[offset])` rebases `sprites[0]` in the shader to each
/// collection's 256-byte-aligned region.
///
/// The bound `size` is an explicit window (NOT `as_entire_binding`). wgpu-29
/// derives `maximum_dynamic_offset = buffer.size - window`, and
/// `set_bind_group` errors when any dynamic offset exceeds that maximum. With
/// `as_entire_binding` the window equals the whole buffer, so the maximum is 0
/// and any collection at offset ≥ 256 would be rejected. Binding an explicit
/// window strictly smaller than the buffer leaves headroom
/// (`buffer.size - window`) for the per-collection dynamic offsets. The caller
/// guarantees `window <= buffer.size`, `window` is a multiple of the 256-byte
/// storage alignment, and every collection's offset is `<= buffer.size - window`.
/// Rebuilt when the buffer object changes (growth) or the window changes.
fn build_instance_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    window: u64,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Sprite Instance Bind Group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: Some(NonZeroU64::new(window).expect("instance window must be non-zero")),
            }),
        }],
    })
}

/// Per-frame layout for the shared instance buffer: each collection's
/// 256-byte-aligned start offset (the dynamic offset passed at draw time) and
/// its live-sprite count (drives the `count * 6` vertex range). Computed in
/// `iter_collections` order; offsets accumulate by each region's *padded* size.
struct CollectionPlacement<'a> {
    collection: &'a str,
    packed_bytes: &'a [u8],
    offset: u32,
    live_count: u32,
}

/// The byte layout of one frame's collections in the shared instance buffer.
struct FrameLayout<'a> {
    /// Per-collection 256-byte-aligned placements, in `iter_collections` order.
    placements: Vec<CollectionPlacement<'a>>,
    /// The largest single collection's *padded* region this frame. The group-6
    /// bind-group window must be at least this so every collection's draw stays
    /// inside the bound storage slice (invariant 1). Always a multiple of the
    /// 256-byte storage alignment (it is a max of `align_up_to_dynamic_offset`
    /// values, which are 256-multiples).
    frame_max_region: usize,
    /// Start offset of the last collection — the largest dynamic offset
    /// `record_draws` will pass to `set_bind_group`. wgpu requires every dynamic
    /// offset `<= maximum_dynamic_offset = capacity - window`, so this is the
    /// binding constraint that sizes the buffer (invariant 2).
    last_offset: usize,
}

/// Plan the frame's buffer layout. Returns the per-collection placements plus
/// the two values `record_draws` needs to size the buffer and the dynamic-offset
/// window: `frame_max_region` (the largest padded region) and `last_offset` (the
/// largest dynamic offset). Collections with zero live sprites are skipped.
/// Returns `None` when nothing is drawable this frame.
///
/// Capacity is no longer folded in here: the buffer is sized in `record_draws`
/// from a *monotonic* window, so the capacity formula lives next to the growth
/// logic that owns the window.
fn plan_frame_layout<'a>(collections: &[(&'a str, &'a [u8])]) -> Option<FrameLayout<'a>> {
    let mut placements = Vec::new();
    let mut cursor = 0usize;
    let mut frame_max_region = 0usize;
    for &(collection, packed_bytes) in collections {
        let live_count = packed_bytes.len() / SPRITE_INSTANCE_SIZE;
        if live_count == 0 {
            continue;
        }
        let region = align_up_to_dynamic_offset(live_count * SPRITE_INSTANCE_SIZE);
        frame_max_region = frame_max_region.max(region);
        placements.push(CollectionPlacement {
            collection,
            packed_bytes,
            offset: cursor as u32,
            live_count: live_count as u32,
        });
        cursor += region;
    }
    if placements.is_empty() {
        return None;
    }
    let last_offset = placements.last().map(|p| p.offset as usize).unwrap_or(0);
    Some(FrameLayout {
        placements,
        frame_max_region,
        last_offset,
    })
}

// `sh_sample.wgsl` reads `sh_total_atlas`, `sh_depth_moments`, and `sh_grid`,
// declared in `billboard.wgsl`; WGSL resolves module-scope names regardless of
// textual order, so appending after is safe. The helper owns the SH
// reconstruction + 8-corner blend symbols (`sh_irradiance`,
// `sample_sh_indirect_corners_depth_aware`, `sample_sh_indirect_corners_without_depth`)
// — billboard must not redeclare them. See rendering_pipeline.md §8.
const BILLBOARD_SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/billboard.wgsl"),
    "\n",
    include_str!("../shaders/sh_sample.wgsl"),
);

/// CPU-only upload plan for one sprite-frame texture array.
struct SpriteArrayPlan<'a> {
    frames: &'a [SpriteFrame],
    width: u32,
    height: u32,
    requested_frame_count: u32,
    fallback: Option<SpriteArrayFallback>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SpriteArrayFallback {
    FrameLayerLimit,
    InvalidInput,
}

/// CPU-only description of a parsed layered sprite `.prm` upload.
///
/// `array_layer_count` is checked against the shared collection directory scan
/// before this plan is accepted. The lifecycle's draw contract independently
/// keeps its existing decoded-PNG frame-count source.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BakedSpriteArrayPlan {
    array_layer_count: u32,
    mip_level_count: u32,
    lod_max_clamp: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SpriteTextureLimits {
    max_texture_array_layers: u32,
    max_texture_dimension_2d: u32,
}

/// Parsed layered sidecar data accepted for the baked sprite upload path.
type ParsedBakedSpriteArray = (
    PrmHeader,
    [Result<PrmSlot, PrmReadError>; 4],
    BakedSpriteArrayPlan,
);

/// Validate the parts of a parsed `.prm` required by the baked sprite path and
/// describe its D2-array texture/sampler configuration.
///
/// Missing or malformed declared slots decline the baked path so the caller can
/// retain the PNG decode fallback. The PRM reader already validates the
/// layer-major payload byte lengths; this plan pins the separate runtime frame
/// count contract before any GPU upload starts.
fn plan_baked_sprite_array(
    header: &PrmHeader,
    slots: &[Result<PrmSlot, PrmReadError>; 4],
    collection_frame_count: usize,
    limits: SpriteTextureLimits,
) -> Option<BakedSpriteArrayPlan> {
    if usize::from(header.layer_count) != collection_frame_count
        || u32::from(header.layer_count) > PORTABLE_MAX_TEXTURE_ARRAY_LAYERS
        || u32::from(header.layer_count) > limits.max_texture_array_layers
        || !header.slot_mask.contains(PrmSlots::DIFFUSE)
        || header.slot_mask.contains(PrmSlots::EMISSIVE)
    {
        return None;
    }

    let diffuse = slots[0].as_ref().ok()?;
    if diffuse.format != PrmFormat::Rgba8UnormSrgb
        || u32::from(diffuse.width) > limits.max_texture_dimension_2d
        || u32::from(diffuse.height) > limits.max_texture_dimension_2d
    {
        return None;
    }
    for (slot_index, slot_flag, expected_format) in [
        (1, PrmSlots::SPECULAR, PrmFormat::R8Unorm),
        (2, PrmSlots::NORMAL, PrmFormat::Bc5RgUnorm),
    ] {
        if !header.slot_mask.contains(slot_flag) {
            continue;
        }
        let slot = slots[slot_index].as_ref().ok()?;
        if slot.format != expected_format
            || (slot.width, slot.height) != (diffuse.width, diffuse.height)
            || u32::from(slot.width) > limits.max_texture_dimension_2d
            || u32::from(slot.height) > limits.max_texture_dimension_2d
        {
            return None;
        }
    }

    let mip_level_count = header_mip_count(slots);
    Some(BakedSpriteArrayPlan {
        array_layer_count: u32::from(header.layer_count),
        mip_level_count,
        lod_max_clamp: mip_level_count.saturating_sub(1) as f32,
    })
}

/// Parse a collection's content-addressed sidecar and accept it only when it
/// still describes the shared collection directory's frame count. Any failure
/// is a normal cache miss: callers continue through the PNG fallback path.
fn load_baked_sprite_array(
    texture_root: &Path,
    prm_cache_root: &Path,
    collection: &str,
    collection_frame_count: usize,
    limits: SpriteTextureLimits,
) -> Option<ParsedBakedSpriteArray> {
    let key = sprite_collection_filename_key(texture_root, collection);
    let prm_path = prm_cache_root.join(format!("{}.prm", cache_filename_for_key(&key)));
    let bytes = std::fs::read(&prm_path).ok()?;
    let (header_result, slots) = PrmFile::from_bytes_partial(&bytes);
    let header = match header_result {
        Ok(header) => header,
        Err(error) => {
            log::warn!(
                "[Smoke] Collection '{collection}' sidecar {} failed to parse: {error}; using PNG fallback",
                prm_path.display()
            );
            return None;
        }
    };
    if header.bundle_hash != key {
        log::warn!(
            "[Smoke] Collection '{collection}' sidecar {} does not match its lookup key; using PNG fallback",
            prm_path.display()
        );
        return None;
    }
    let Some(plan) = plan_baked_sprite_array(&header, &slots, collection_frame_count, limits)
    else {
        log::warn!(
            "[Smoke] Collection '{collection}' sidecar {} is incompatible with its collection or device limits; using PNG fallback",
            prm_path.display()
        );
        return None;
    };
    Some((header, slots, plan))
}

fn is_direct_sprite_reference(collection: &str) -> bool {
    Path::new(collection)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
}

fn should_attempt_baked_sprite_load(
    baked_sidecar_eligible: bool,
    collection: &str,
    collection_frame_count: usize,
) -> bool {
    baked_sidecar_eligible && collection_frame_count > 0 && !is_direct_sprite_reference(collection)
}

impl SpriteArrayPlan<'_> {
    fn frame_count(&self) -> u32 {
        if self.fallback == Some(SpriteArrayFallback::InvalidInput) {
            1
        } else {
            self.frames.len() as u32
        }
    }
}

/// Plan a safe D2-array upload from CPU-normalized frames.
///
/// Collections over the granted array-layer cap retain frame zero as a usable
/// one-layer degradation. Malformed data and extents beyond the active device
/// limit use a 1x1 white one-layer fallback. Returns `None` only when the
/// device cannot accept that fallback.
fn plan_sprite_array(
    frames: &[SpriteFrame],
    max_texture_array_layers: u32,
    max_texture_dimension_2d: u32,
) -> Option<SpriteArrayPlan<'_>> {
    if max_texture_array_layers == 0 || max_texture_dimension_2d == 0 {
        return None;
    }
    let Some(first) = frames.first() else {
        return Some(SpriteArrayPlan {
            frames,
            width: 1,
            height: 1,
            requested_frame_count: 0,
            fallback: Some(SpriteArrayFallback::InvalidInput),
        });
    };
    let width = first.width;
    let height = first.height;
    let expected_rgba_bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .and_then(|bytes| usize::try_from(bytes).ok());
    if width == 0
        || height == 0
        || width > max_texture_dimension_2d
        || height > max_texture_dimension_2d
        || expected_rgba_bytes.is_none()
        || frames.iter().any(|frame| {
            frame.width != width
                || frame.height != height
                || Some(frame.data.len()) != expected_rgba_bytes
        })
    {
        return Some(SpriteArrayPlan {
            frames,
            width: 1,
            height: 1,
            requested_frame_count: frames.len() as u32,
            fallback: Some(SpriteArrayFallback::InvalidInput),
        });
    }

    let requested_frame_count = frames.len() as u32;
    let fallback =
        (!sprite_frame_count_fits_device(requested_frame_count, max_texture_array_layers))
            .then_some(SpriteArrayFallback::FrameLayerLimit);
    let upload_frame_count = if fallback.is_some() { 1 } else { frames.len() };
    Some(SpriteArrayPlan {
        frames: &frames[..upload_frame_count],
        width,
        height,
        requested_frame_count,
        fallback,
    })
}

/// Whether a sprite collection's D2-array texture fits the device limit.
///
/// Kept GPU-free so the rejection boundary can be exercised in headless tests.
fn sprite_frame_count_fits_device(frame_count: u32, max_texture_array_layers: u32) -> bool {
    frame_count <= max_texture_array_layers
}

fn warn_sprite_frame_count_exceeds_device_limit(
    collection: &str,
    frame_count: u32,
    max_texture_array_layers: u32,
) {
    log::warn!(
        "[Smoke] Collection '{collection}' requires {frame_count} sprite frame array layers, \
         exceeding device maxTextureArrayLayers {max_texture_array_layers}; falling back to frame 0 as one array layer"
    );
}

fn warn_sprite_array_invalid_input(collection: &str) {
    log::warn!(
        "[Smoke][invalid-sprite-array] Collection '{collection}' has invalid sprite frame data or an extent unsupported by this device; falling back to a 1x1 white array layer"
    );
}

/// One loaded sprite sheet, shared across all emitters whose `collection`
/// matches.
pub struct SpriteSheet {
    /// Sprite sheet texture bind group (group 1 of the billboard pipeline).
    pub bind_group: wgpu::BindGroup,
    /// Number of animation frames. 1 when the collection has a single PNG.
    #[allow(dead_code)]
    pub frame_count: u32,
    /// Owns the baked specular resource alongside its retained view.
    #[allow(dead_code)]
    specular_texture: Option<wgpu::Texture>,
    /// Baked optional specular array. Shimmer binds it only when the same
    /// `NORMAL`-slot predicate that enables shimmer is true.
    #[allow(dead_code)]
    pub specular_view: Option<wgpu::TextureView>,
    /// Owns the baked normal resource alongside its retained view.
    #[allow(dead_code)]
    normal_texture: Option<wgpu::Texture>,
    /// Baked optional normal array. Its presence classifies the collection as
    /// shimmer and selects the fragment static-specular path.
    #[allow(dead_code)]
    pub normal_view: Option<wgpu::TextureView>,
    /// Slots parsed from the baked sidecar. Decode fallback sheets are
    /// diffuse-only, matching their runtime PNG upload.
    #[allow(dead_code)]
    pub slot_mask: PrmSlots,
}

/// GPU-free shimmer classification. A normal map is the sole opt-in; a
/// specular mask alone cannot create a moving highlight.
fn sprite_shimmer_flag(slot_mask: PrmSlots) -> f32 {
    if slot_mask.contains(PrmSlots::NORMAL) {
        1.0
    } else {
        0.0
    }
}

/// Pack `SpriteDrawParams` bytes for a
/// (frame_count, spec_intensity, lifetime, emissive, spec_exponent, slot_mask)
/// tuple.
fn build_draw_params(
    frame_count: u32,
    spec_intensity: f32,
    lifetime: f32,
    emissive: f32,
    spec_exponent: f32,
    slot_mask: PrmSlots,
) -> [u8; SPRITE_DRAW_PARAMS_SIZE] {
    let mut bytes = [0u8; SPRITE_DRAW_PARAMS_SIZE];
    // params.x = bitcast<f32>(frame_count)
    bytes[0..4].copy_from_slice(&frame_count.to_ne_bytes());
    bytes[4..8].copy_from_slice(&spec_intensity.to_ne_bytes());
    bytes[8..12].copy_from_slice(&lifetime.to_ne_bytes());
    bytes[12..16].copy_from_slice(&emissive.to_ne_bytes());
    // params2.x = shimmer classification; params2.y = static-spec exponent.
    bytes[16..20].copy_from_slice(&sprite_shimmer_flag(slot_mask).to_ne_bytes());
    bytes[20..24].copy_from_slice(&spec_exponent.to_ne_bytes());
    bytes
}

/// Create one renderer-owned D2-array fallback layer for an optional shimmer
/// input. Every sprite bind group always supplies bindings 3 and 4; collections
/// without a `NORMAL` slot select these neutral views instead of failing load.
fn create_sprite_placeholder_array(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    format: wgpu::TextureFormat,
    pixel: &[u8],
) -> (wgpu::Texture, wgpu::TextureView) {
    let bytes_per_texel = match format {
        wgpu::TextureFormat::R8Unorm => 1,
        wgpu::TextureFormat::Rgba8Unorm => 4,
        _ => unreachable!("sprite shimmer placeholders use uncompressed unorm formats"),
    };
    assert_eq!(pixel.len(), bytes_per_texel);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixel,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_texel as u32),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some(label),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        mip_level_count: Some(1),
        array_layer_count: Some(1),
        ..Default::default()
    });
    (texture, view)
}

/// GPU resources for the billboard sprite pass.
pub struct SmokePass {
    pipeline: wgpu::RenderPipeline,

    /// Group 1 layout: sprite-frame array + sampler + draw-params uniform.
    /// Retained so per-collection bind groups can be built post-init as
    /// `register_collection` is called.
    sheet_bind_group_layout: wgpu::BindGroupLayout,

    /// Group 6 layout: the sprite instance storage buffer, declared with
    /// `has_dynamic_offset: true` so each collection draws from its own
    /// 256-byte-aligned region of the single shared buffer. Retained so the
    /// per-frame bind group can be rebuilt when the buffer grows.
    instance_bind_group_layout: wgpu::BindGroupLayout,
    /// Single shared upload target for *all* collections' packed sprite
    /// instances this frame. Grown on demand when a frame's total live-sprite
    /// footprint (padded per collection for dynamic-offset alignment) exceeds
    /// the current capacity. Replaces the old fixed 4096-sprite-per-collection
    /// buffer that silently truncated overflow.
    instance_buffer: wgpu::Buffer,
    /// Current byte capacity of `instance_buffer`.
    instance_buffer_capacity: usize,
    /// Byte size of the window the current group-6 bind group binds (its
    /// explicit `size`). Monotonically non-decreasing — it only ever grows to
    /// the largest single-collection region seen so far, so the bind group is
    /// rebuilt rarely. wgpu derives `maximum_dynamic_offset = capacity - window`
    /// from this, so `capacity` must stay `>= last_offset + window` every frame.
    instance_window: u64,
    /// Group-6 bind group over a `instance_window`-sized window of
    /// `instance_buffer`, bound with a per-collection dynamic offset at draw
    /// time. Rebuilt only when the buffer grows or the window grows (the dynamic
    /// offset, not a new bind group, selects each collection's region within a
    /// frame).
    instance_bind_group: wgpu::BindGroup,

    /// Loaded sprite-frame arrays keyed by collection name. Populated at level load.
    sheets: HashMap<String, SpriteSheet>,

    /// Shared linear sampler for sprite-frame arrays.
    sampler: wgpu::Sampler,
    /// Shared 1×1/one-layer `R8Unorm` fallback with R = 1.0 for missing
    /// specular masks. Kept alive alongside its view for all collection bind
    /// groups that need the optional-slot degradation path.
    #[allow(dead_code)]
    specular_placeholder_texture: wgpu::Texture,
    specular_placeholder_view: wgpu::TextureView,
    /// Shared 1×1/one-layer flat normal fallback with RG = 0.5. This is only
    /// selected when a collection is not shimmer, so it cannot accidentally
    /// enable a normal-mapped static-specular path.
    #[allow(dead_code)]
    normal_placeholder_texture: wgpu::Texture,
    normal_placeholder_view: wgpu::TextureView,
}

/// Group 1 (sprite-frame array) BGL entries: sprite texture (binding 0, FRAGMENT) +
/// sampler (binding 1, FRAGMENT) + draw-params uniform (binding 2,
/// VERTEX | FRAGMENT — `vs_main` reads `draw_params` for frame count / lifetime
/// and the spec-intensity term) + optional slot arrays (bindings 3/4,
/// FRAGMENT). No storage buffers here. GPU-free so the
/// billboard vertex storage-buffer budget can sum it without a device.
pub(super) fn sprite_sheet_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 5] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
    ]
}

/// Group 6 (sprite instance) BGL entry: the single shared per-sprite instance
/// storage buffer, read by `vs_main` (`sprites[sprite_index]`). This is the only
/// group-6 storage buffer and one of the six genuinely VERTEX-read storage buffers
/// in the Billboard Pipeline Layout (the other five live in group 2). `VERTEX`-only
/// visibility: the fragment stage never reads instances. GPU-free for the budget sum.
pub(super) fn sprite_instance_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 1] {
    [wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: true,
            min_binding_size: NonZeroU64::new(SPRITE_INSTANCE_SIZE as u64),
        },
        count: None,
    }]
}

impl SmokePass {
    /// Build the billboard pipeline. `bgls` carries the renderer-owned bind
    /// group layouts shared with the forward pass (camera, lighting, SH volume).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        depth_format: wgpu::TextureFormat,
        camera_bgl: &wgpu::BindGroupLayout,
        lighting_bgl: &wgpu::BindGroupLayout,
        sh_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Billboard Shader"),
            source: wgpu::ShaderSource::Wgsl(BILLBOARD_SHADER_SOURCE.into()),
        });

        // Group 1: sprite-frame array (binding 0) + sampler (binding 1)
        // + draw-params uniform (binding 2) + optional specular/normal arrays
        // (bindings 3/4). Entries built from the GPU-free
        // `sprite_sheet_bind_group_layout_entries` so the billboard vertex
        // storage-buffer budget (`billboard_pipeline_vertex_storage_buffer_count`)
        // reads from the same source of truth this layout is created from.
        let sheet_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Sprite Sheet BGL"),
                entries: &sprite_sheet_bind_group_layout_entries(),
            });

        // Group 6: sprite instance storage buffer. `has_dynamic_offset: true`
        // lets each collection draw from its own 256-byte-aligned region of the
        // single shared buffer — the dynamic offset rebases `sprites[0]` in the
        // shader to that collection's first instance.
        //
        // `min_binding_size` is the per-instance stride (shader-side floor): with
        // a dynamic offset and `array<SpriteInstance>` (runtime-sized), it tells
        // wgpu the bound window must cover at least one instance. The bound
        // window we actually pass (`build_instance_bind_group`'s explicit `size`)
        // is `frame_max_region` ≥ 256 B ≥ this 32-byte floor, so it is always
        // satisfied.
        //
        // NOTE: `min_binding_size` does NOT gate the dynamic offset in wgpu-29.
        // The maximum legal dynamic offset is derived solely from the bound
        // window: `maximum_dynamic_offset = buffer.size - bound_size`
        // (`min_binding_size` is validated separately and does not feed it). So
        // the real dynamic-offset gate is the explicit window passed in
        // `build_instance_bind_group`, sized and reserved by `record_draws` — NOT
        // this field. Binding the whole buffer (`as_entire_binding`) would make
        // `bound_size == buffer.size`, forcing `maximum_dynamic_offset == 0` and
        // rejecting every collection past offset 0.
        let instance_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Sprite Instance BGL"),
                entries: &sprite_instance_bind_group_layout_entries(),
            });

        // Pipeline layout: group 0 (camera), 1 (sheet), 2 (lighting),
        // 3 (SH volume), then groups 4 and 5 are unused by this pipeline
        // (group 6 sits after). wgpu allows a sparse layout — we simply
        // pass placeholder slots as `None` for the unused groups.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Billboard Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                Some(&sheet_bind_group_layout),
                Some(lighting_bgl),
                Some(sh_bgl),
                // Groups 4 and 5 are declared as None so the pipeline layout
                // only references the groups the shader actually binds.
                None,
                None,
                Some(&instance_bind_group_layout),
            ],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Billboard Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                // Depth test enabled, write disabled: sprites occlude behind
                // geometry but don't occlude each other or write into the
                // depth buffer (additive blend of translucent smoke).
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: super::SCENE_COLOR_FORMAT,
                    // Additive alpha blend: smoke accumulates without
                    // darkening the scene behind it.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        // Single shared instance storage buffer for all collections. Sized for
        // a modest initial frame footprint and grown on demand by `record_draws`
        // when a frame's padded total exceeds it — no per-collection cap.
        let instance_buffer_capacity = align_up_to_dynamic_offset(1024 * SPRITE_INSTANCE_SIZE);
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Instance Buffer"),
            size: instance_buffer_capacity as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Seed the window at one storage-alignment unit (256 B): strictly less
        // than the initial capacity (32768 B), so `maximum_dynamic_offset =
        // capacity - window > 0` even before any growth, and a multiple of the
        // 256-byte storage alignment (invariant 3). `record_draws` grows it
        // monotonically to each frame's `frame_max_region` as needed; the first
        // frame is valid because `record_draws` raises the window to at least
        // that frame's `frame_max_region` and grows capacity to keep
        // `capacity >= last_offset + window` before recording any draw.
        let instance_window = STORAGE_DYNAMIC_OFFSET_ALIGNMENT as u64;
        let instance_bind_group = build_instance_bind_group(
            device,
            &instance_bind_group_layout,
            &instance_buffer,
            instance_window,
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Sprite Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let (specular_placeholder_texture, specular_placeholder_view) =
            create_sprite_placeholder_array(
                device,
                queue,
                "Sprite Specular Placeholder Array",
                wgpu::TextureFormat::R8Unorm,
                &[255],
            );
        let (normal_placeholder_texture, normal_placeholder_view) = create_sprite_placeholder_array(
            device,
            queue,
            "Sprite Normal Placeholder Array",
            wgpu::TextureFormat::Rgba8Unorm,
            &[128, 128, 255, 255],
        );

        Self {
            pipeline,
            sheet_bind_group_layout,
            instance_bind_group_layout,
            instance_buffer,
            instance_buffer_capacity,
            instance_window,
            instance_bind_group,
            sheets: HashMap::new(),
            sampler,
            specular_placeholder_texture,
            specular_placeholder_view,
            normal_placeholder_texture,
            normal_placeholder_view,
        }
    }

    /// Register a sprite collection. Uploads each frame to its own layer of one
    /// RGBA8 texture array and creates the per-collection bind group (group 1).
    /// Frames must carry the shared dimensions guaranteed by the CPU loader.
    /// Only map-emitter collections may opt into baked sidecars; every other
    /// source stays on the decoded-PNG path.
    /// Reports and rejects duplicate collection calls, or unusable frame lists,
    /// so caller ordering cannot silently replace a draw contract.
    pub fn register_collection(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        collection: &str,
        texture_root: &Path,
        prm_cache_root: &Path,
        registration: SpriteCollectionRegistration,
    ) {
        if self.sheets.contains_key(collection) {
            log::warn!(
                "[Smoke] duplicate collection '{collection}' rejected; level installation must resolve one draw contract"
            );
            return;
        }

        let collection_frame_count =
            collection_frame_paths(texture_root, collection, SpriteSlot::Diffuse).len();
        let limits = device.limits();
        let sprite_texture_limits = SpriteTextureLimits {
            max_texture_array_layers: limits.max_texture_array_layers,
            max_texture_dimension_2d: limits.max_texture_dimension_2d,
        };
        let baked_sprite = should_attempt_baked_sprite_load(
            registration.baked_sidecar_eligible,
            collection,
            collection_frame_count,
        )
        .then(|| {
            load_baked_sprite_array(
                texture_root,
                prm_cache_root,
                collection,
                collection_frame_count,
                sprite_texture_limits,
            )
            .map(|(header, slots, plan)| (header, slots, plan, collection_frame_count))
        })
        .flatten();
        if let Some((header, slots, baked_plan, collection_frame_count)) = baked_sprite {
            let diffuse_slot = slots[0]
                .as_ref()
                .expect("baked sprite plan requires a parsed diffuse slot");
            debug_assert!(u32::from(diffuse_slot.level_count) <= baked_plan.mip_level_count);
            let diffuse_layers = slot_layer_levels(diffuse_slot, header.layer_count);
            let (_diffuse_texture, diffuse_view) = upload_texture_array_data(
                device,
                queue,
                prm_format_to_wgpu(diffuse_slot.format),
                &diffuse_layers,
                baked_plan.array_layer_count,
                u32::from(diffuse_slot.level_count),
                &format!("Baked Sprite Diffuse Array: {collection}"),
            );

            let (specular_texture, specular_view) = if header.slot_mask.contains(PrmSlots::SPECULAR)
            {
                let slot = slots[1]
                    .as_ref()
                    .expect("baked sprite plan requires declared specular slots to parse");
                debug_assert!(u32::from(slot.level_count) <= baked_plan.mip_level_count);
                let layers = slot_layer_levels(slot, header.layer_count);
                let (texture, view) = upload_texture_array_data(
                    device,
                    queue,
                    prm_format_to_wgpu(slot.format),
                    &layers,
                    baked_plan.array_layer_count,
                    u32::from(slot.level_count),
                    &format!("Baked Sprite Specular Array: {collection}"),
                );
                (Some(texture), Some(view))
            } else {
                (None, None)
            };
            let (normal_texture, normal_view) = if header.slot_mask.contains(PrmSlots::NORMAL) {
                let slot = slots[2]
                    .as_ref()
                    .expect("baked sprite plan requires declared normal slots to parse");
                debug_assert!(u32::from(slot.level_count) <= baked_plan.mip_level_count);
                let layers = slot_layer_levels(slot, header.layer_count);
                let (texture, view) = upload_texture_array_data(
                    device,
                    queue,
                    prm_format_to_wgpu(slot.format),
                    &layers,
                    baked_plan.array_layer_count,
                    u32::from(slot.level_count),
                    &format!("Baked Sprite Normal Array: {collection}"),
                );
                (Some(texture), Some(view))
            } else {
                (None, None)
            };

            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(&format!("Baked Sprite Mip Sampler: {collection}")),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                lod_max_clamp: baked_plan.lod_max_clamp,
                ..Default::default()
            });
            // `NORMAL` presence is the sole shimmer discriminator. The exact
            // same predicate controls params2.x and whether optional slot views
            // are exposed to the fragment shader, so a collection can never be
            // flagged shimmer while sampling placeholder normal data (or vice
            // versa). A shimmer collection may omit SPECULAR; that input then
            // receives the white mask placeholder.
            let is_shimmer = sprite_shimmer_flag(header.slot_mask) != 0.0;
            let bound_specular_view = if is_shimmer {
                specular_view
                    .as_ref()
                    .unwrap_or(&self.specular_placeholder_view)
            } else {
                &self.specular_placeholder_view
            };
            let bound_normal_view = if is_shimmer {
                normal_view
                    .as_ref()
                    .expect("a shimmer slot mask must have uploaded its normal array")
            } else {
                &self.normal_placeholder_view
            };
            let frame_count = collection_frame_count as u32;
            let params_bytes = build_draw_params(
                frame_count,
                registration.spec_intensity,
                registration.lifetime,
                registration.emissive,
                registration.spec_exponent,
                header.slot_mask,
            );
            let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("Sprite Draw Params: {collection}")),
                contents: &params_bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!(
                    "Baked Sprite Frame Array Bind Group: {collection}"
                )),
                layout: &self.sheet_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&diffuse_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: params_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(bound_specular_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(bound_normal_view),
                    },
                ],
            });
            self.sheets.insert(
                collection.to_string(),
                SpriteSheet {
                    bind_group,
                    frame_count,
                    specular_texture,
                    specular_view,
                    normal_texture,
                    normal_view,
                    slot_mask: header.slot_mask,
                },
            );
            return;
        }

        // A cache miss, malformed sidecar, or direct PNG reference reaches the
        // unchanged decoded-frame array upload below. Keep this after the baked
        // branch so a valid sidecar never pays the PNG pixel-decode cost.
        let frames = postretro_render_cpu::smoke::load_sprite_frames(texture_root, collection)
            .unwrap_or_default();
        let max_texture_array_layers = limits.max_texture_array_layers;
        let Some(plan) = plan_sprite_array(
            &frames,
            max_texture_array_layers,
            limits.max_texture_dimension_2d,
        ) else {
            log::warn!("[Smoke] Collection '{collection}' had no usable normalized frame array");
            return;
        };
        match plan.fallback {
            Some(SpriteArrayFallback::FrameLayerLimit) => {
                warn_sprite_frame_count_exceeds_device_limit(
                    collection,
                    plan.requested_frame_count,
                    max_texture_array_layers,
                );
            }
            Some(SpriteArrayFallback::InvalidInput) => warn_sprite_array_invalid_input(collection),
            None => {}
        }
        let frames = plan.frames;
        let width = plan.width;
        let height = plan.height;
        let frame_count = plan.frame_count();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("Sprite Frame Array: {collection}")),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: frame_count,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let fallback_pixel = [255u8; 4];
        let upload_layer = |layer: u32, data: &[u8]| {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        };
        if plan.fallback == Some(SpriteArrayFallback::InvalidInput) {
            upload_layer(0, &fallback_pixel);
        } else {
            for (layer, frame) in frames.iter().enumerate() {
                upload_layer(layer as u32, &frame.data);
            }
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some(&format!("Sprite Frame Array View: {collection}")),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            mip_level_count: Some(1),
            array_layer_count: Some(frame_count),
            ..Default::default()
        });

        let params_bytes = build_draw_params(
            frame_count,
            registration.spec_intensity,
            registration.lifetime,
            registration.emissive,
            registration.spec_exponent,
            PrmSlots::DIFFUSE,
        );
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("Sprite Draw Params: {collection}")),
            contents: &params_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("Sprite Frame Array Bind Group: {collection}")),
            layout: &self.sheet_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.specular_placeholder_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self.normal_placeholder_view),
                },
            ],
        });

        self.sheets.insert(
            collection.to_string(),
            SpriteSheet {
                bind_group,
                frame_count,
                specular_texture: None,
                specular_view: None,
                normal_texture: None,
                normal_view: None,
                slot_mask: PrmSlots::DIFFUSE,
            },
        );
    }

    /// Drop per-level sprite sheet textures and bind groups. The shared
    /// instance buffer is renderer-lifetime scratch and stays allocated.
    pub fn clear_collections(&mut self) {
        self.sheets.clear();
    }

    /// Whether any collection is registered. Used by the renderer to skip the
    /// pass entirely on levels with no emitters.
    pub fn has_any_sheet(&self) -> bool {
        !self.sheets.is_empty()
    }

    /// Upload every collection's packed sprite instances into the single shared
    /// buffer and record one draw call per collection from its own region.
    ///
    /// Each `(collection, packed_bytes)` slice carries
    /// `live_count * SPRITE_INSTANCE_SIZE` bytes (packed by
    /// `scripting::systems::particle_render::pack_particle_instance`). The caller
    /// batches all emitters sharing a collection into one slice — that batching
    /// happens upstream in `ParticleRenderCollector::collect`
    /// (`scripting/systems/particle_render.rs`), which buckets particles by
    /// `SpriteVisual.sprite`; `record_draws` itself is unaware of emitter
    /// boundaries — so a collection still issues exactly one draw — N collections
    /// produce N draws.
    ///
    /// **Buffer sizing / growth.** The frame's regions are laid out back-to-back,
    /// each padded up to the 256-byte storage dynamic-offset alignment so its
    /// start offset is a legal dynamic offset (the 32-byte per-instance stride is
    /// unchanged *within* a region). The group-6 bind group binds an explicit
    /// `window` (a monotonic high-water mark of the largest single collection's
    /// padded region), so wgpu's `maximum_dynamic_offset = capacity - window`
    /// stays `>= last_offset`. The buffer is recreated larger when
    /// `last_offset + window` exceeds capacity, and the bind group is rebuilt
    /// when the buffer object or the window changes — there is **no
    /// per-collection cap**, so a single collection may exceed the old fixed
    /// 4096-sprite buffer without silent truncation.
    ///
    /// Each collection is uploaded once at its own offset (no redundant offset-0
    /// re-upload per collection) and drawn via the dynamic-offset bind group,
    /// which rebases `sprites[0]` in the shader to that region's first instance.
    pub fn record_draws<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        collections: &[(&str, &[u8])],
    ) {
        let Some(FrameLayout {
            placements,
            frame_max_region,
            last_offset,
        }) = plan_frame_layout(collections)
        else {
            return;
        };

        // Size the dynamic-offset window and the buffer so wgpu's per-draw
        // `offset <= maximum_dynamic_offset = capacity - window` holds for every
        // collection:
        //   - `new_window = max(current, frame_max_region)` is monotonic and
        //     covers the largest collection's region (invariant 1).
        //   - `required_capacity = last_offset + new_window` makes
        //     `maximum_dynamic_offset = capacity - new_window >= last_offset`,
        //     and `last_offset` is the largest offset (invariant 2).
        // The buffer grows when capacity is short; the bind group is rebuilt
        // when the buffer object changes OR the window changes.
        let new_window = self.instance_window.max(frame_max_region as u64);
        let required_capacity = last_offset + new_window as usize;
        let need_buffer_grow = required_capacity > self.instance_buffer_capacity;
        let need_bg_rebuild = need_buffer_grow || new_window != self.instance_window;

        if need_buffer_grow {
            let new_capacity = align_up_to_dynamic_offset(required_capacity);
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Sprite Instance Buffer"),
                size: new_capacity as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_buffer_capacity = new_capacity;
        }
        if need_bg_rebuild {
            self.instance_window = new_window;
            self.instance_bind_group = build_instance_bind_group(
                device,
                &self.instance_bind_group_layout,
                &self.instance_buffer,
                self.instance_window,
            );
        }
        // Window never exceeds capacity: window <= last_offset + window =
        // required_capacity <= capacity.
        debug_assert!(self.instance_window <= self.instance_buffer_capacity as u64);

        // Upload each collection at its aligned offset (one write per collection,
        // no full re-upload at offset 0).
        for placement in &placements {
            queue.write_buffer(
                &self.instance_buffer,
                placement.offset as u64,
                placement.packed_bytes,
            );
        }

        pass.set_pipeline(&self.pipeline);
        for placement in &placements {
            let Some(sheet) = self.sheets.get(placement.collection) else {
                continue;
            };
            pass.set_bind_group(1, &sheet.bind_group, &[]);
            pass.set_bind_group(6, &self.instance_bind_group, &[placement.offset]);
            // Non-indexed draw of 6 vertices per sprite, rebased to this
            // collection's region by the group-6 dynamic offset.
            pass.draw(0..(placement.live_count * 6), 0..1);
        }
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    use postretro_level_format::prm::{STAGE_VERSION, bc5_level_count, expected_level_count};

    static NEXT_SPRITE_PRM_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    fn temp_sprite_prm_fixture_root(test_name: &str) -> std::path::PathBuf {
        let id = NEXT_SPRITE_PRM_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "postretro_baked_sprite_{test_name}_{}_{}",
            std::process::id(),
            id
        ));
        fs::create_dir_all(root.join("smoke")).expect("fixture collection must be created");
        root
    }

    fn layered_slot(format: PrmFormat, fill: u8) -> PrmSlot {
        let width = 4;
        let height = 4;
        let level_count = match format {
            PrmFormat::Bc5RgUnorm => bc5_level_count(width, height),
            _ => expected_level_count(width, height),
        };
        let mut payload = Vec::new();
        for layer in 0..2 {
            for level in 0..level_count {
                let level_width = ((width as u32) >> level).max(1);
                let level_height = ((height as u32) >> level).max(1);
                let level_size = postretro_render_cpu::loaded_texture::level_byte_size(
                    format,
                    level_width,
                    level_height,
                );
                payload.extend(vec![
                    fill.wrapping_add(layer * 0x10).wrapping_add(level);
                    level_size
                ]);
            }
        }
        PrmSlot {
            format,
            width,
            height,
            level_count,
            payload,
        }
    }

    fn companion_bearing_sprite_prm(bundle_hash: [u8; 32]) -> PrmFile {
        PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: PrmSlots::DIFFUSE | PrmSlots::SPECULAR | PrmSlots::NORMAL,
                bundle_hash,
                total_body_bytes: 0,
                layer_count: 2,
            },
            slots: [
                Some(layered_slot(PrmFormat::Rgba8UnormSrgb, 0x10)),
                Some(layered_slot(PrmFormat::R8Unorm, 0x20)),
                Some(layered_slot(PrmFormat::Bc5RgUnorm, 0x30)),
                None,
            ],
        }
    }

    fn portable_sprite_texture_limits() -> SpriteTextureLimits {
        SpriteTextureLimits {
            max_texture_array_layers: PORTABLE_MAX_TEXTURE_ARRAY_LAYERS,
            max_texture_dimension_2d: 4096,
        }
    }

    #[test]
    fn companion_sidecar_uses_the_shared_sprite_key_and_baked_plan_values() {
        let texture_root = temp_sprite_prm_fixture_root("companion-plan");
        fs::write(
            texture_root.join("smoke/smoke_00.png"),
            b"diffuse frame zero",
        )
        .expect("diffuse frame must be written");
        fs::write(
            texture_root.join("smoke/smoke_01.png"),
            b"diffuse frame one",
        )
        .expect("diffuse frame must be written");
        fs::write(
            texture_root.join("smoke/smoke_00_spec.png"),
            b"spec frame zero",
        )
        .expect("spec frame must be written");
        fs::write(
            texture_root.join("smoke/smoke_01_spec.png"),
            b"spec frame one",
        )
        .expect("spec frame must be written");
        fs::write(
            texture_root.join("smoke/smoke_00_normal.png"),
            b"normal frame zero",
        )
        .expect("normal frame must be written");
        fs::write(
            texture_root.join("smoke/smoke_01_normal.png"),
            b"normal frame one",
        )
        .expect("normal frame must be written");

        let cache_root = texture_root.join("cache");
        fs::create_dir_all(&cache_root).expect("fixture cache must be created");
        let key = sprite_collection_filename_key(&texture_root, "smoke");
        let filename = format!("{}.prm", cache_filename_for_key(&key));
        let fixture_path = cache_root.join(&filename);
        fs::write(
            &fixture_path,
            companion_bearing_sprite_prm(key)
                .to_bytes()
                .expect("fixture PRM must serialize"),
        )
        .expect("fixture sidecar must be written");
        assert_eq!(
            fixture_path.file_name().and_then(|name| name.to_str()),
            Some(filename.as_str()),
            "fixture filename must use the shared sprite collection key"
        );

        let (header, slots, plan) = load_baked_sprite_array(
            &texture_root,
            &cache_root,
            "smoke",
            2,
            portable_sprite_texture_limits(),
        )
        .expect("shared key must resolve the companion-bearing sidecar");
        assert_eq!(header.layer_count, 2);
        assert_eq!(
            header.slot_mask,
            PrmSlots::DIFFUSE | PrmSlots::SPECULAR | PrmSlots::NORMAL
        );
        assert!(slots[1].is_ok(), "specular slot must parse");
        assert!(slots[2].is_ok(), "normal slot must parse");
        assert_eq!(plan.array_layer_count, 2);
        assert_eq!(plan.mip_level_count, 3);
        assert_eq!(plan.lod_max_clamp, 2.0);

        fs::remove_dir_all(&texture_root).expect("fixture root must be removed");
    }

    #[test]
    fn mismatched_baked_header_layer_count_declines_to_the_png_fallback_plan() {
        let file = companion_bearing_sprite_prm([0xA5; 32]);
        let bytes = file.to_bytes().expect("fixture PRM must serialize");
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let header = header.expect("fixture header must parse");

        assert!(
            plan_baked_sprite_array(&header, &slots, 3, portable_sprite_texture_limits()).is_none(),
            "a sidecar whose layer count differs from the collection frame count must fall back"
        );
    }

    #[test]
    fn incompatible_sprite_slot_schema_declines_to_png_fallback() {
        // Regression: generic PRM validation admitted the legacy RGBA normal
        // format even though sprite sidecars require BC5 normal arrays.
        let texture_root = temp_sprite_prm_fixture_root("incompatible-schema");
        fs::write(
            texture_root.join("smoke/smoke_00.png"),
            b"diffuse frame zero",
        )
        .unwrap();
        fs::write(
            texture_root.join("smoke/smoke_01.png"),
            b"diffuse frame one",
        )
        .unwrap();
        let cache_root = texture_root.join("cache");
        fs::create_dir_all(&cache_root).unwrap();
        let key = sprite_collection_filename_key(&texture_root, "smoke");
        let mut file = companion_bearing_sprite_prm(key);
        file.slots[2] = Some(layered_slot(PrmFormat::Rgba8Unorm, 0x30));
        fs::write(
            cache_root.join(format!("{}.prm", cache_filename_for_key(&key))),
            file.to_bytes()
                .expect("generic PRM schema remains serializable"),
        )
        .unwrap();

        assert!(
            load_baked_sprite_array(
                &texture_root,
                &cache_root,
                "smoke",
                2,
                portable_sprite_texture_limits(),
            )
            .is_none(),
            "an incompatible sprite schema must use the PNG fallback"
        );

        fs::remove_dir_all(texture_root).unwrap();
    }

    #[test]
    fn baked_sprite_plan_declines_device_layer_and_dimension_limit_overruns() {
        let file = companion_bearing_sprite_prm([0xA5; 32]);
        let bytes = file.to_bytes().expect("fixture PRM must serialize");
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        let header = header.expect("fixture header must parse");

        assert!(
            plan_baked_sprite_array(
                &header,
                &slots,
                2,
                SpriteTextureLimits {
                    max_texture_array_layers: 1,
                    max_texture_dimension_2d: 4096,
                },
            )
            .is_none(),
            "a portable sidecar must still fit the active device layer cap"
        );
        assert!(
            plan_baked_sprite_array(
                &header,
                &slots,
                2,
                SpriteTextureLimits {
                    max_texture_array_layers: 2,
                    max_texture_dimension_2d: 2,
                },
            )
            .is_none(),
            "a portable sidecar must still fit the active device dimension cap"
        );
    }

    #[test]
    fn mismatched_baked_bundle_hash_declines_to_png_fallback() {
        use log::Level;
        use postretro_test_log_capture::LogCapture;

        let texture_root = temp_sprite_prm_fixture_root("bundle-hash-mismatch");
        fs::write(
            texture_root.join("smoke/smoke_00.png"),
            b"diffuse frame zero",
        )
        .expect("diffuse frame must be written");
        fs::write(
            texture_root.join("smoke/smoke_01.png"),
            b"diffuse frame one",
        )
        .expect("diffuse frame must be written");
        let cache_root = texture_root.join("cache");
        fs::create_dir_all(&cache_root).expect("fixture cache must be created");
        let lookup_key = sprite_collection_filename_key(&texture_root, "smoke");
        let fixture_path = cache_root.join(format!("{}.prm", cache_filename_for_key(&lookup_key)));
        fs::write(
            &fixture_path,
            companion_bearing_sprite_prm([0x5A; 32])
                .to_bytes()
                .expect("fixture PRM must serialize"),
        )
        .expect("fixture sidecar must be written");

        let capture = LogCapture::start();
        assert!(
            load_baked_sprite_array(
                &texture_root,
                &cache_root,
                "smoke",
                2,
                portable_sprite_texture_limits(),
            )
            .is_none(),
            "a sidecar stored under the lookup filename must still carry that key"
        );
        capture.assert_logged_once(Level::Warn, "does not match its lookup key");

        fs::remove_dir_all(&texture_root).expect("fixture root must be removed");
    }

    #[test]
    fn direct_png_references_never_attempt_the_collection_sidecar_path() {
        assert!(is_direct_sprite_reference("effects/impact.PNG"));
        assert!(!is_direct_sprite_reference("smoke"));
    }

    #[test]
    fn baked_sidecar_attempt_requires_map_eligible_numbered_collection() {
        assert!(should_attempt_baked_sprite_load(true, "smoke", 2));
        assert!(!should_attempt_baked_sprite_load(false, "smoke", 2));
        assert!(!should_attempt_baked_sprite_load(false, "impact", 2));
        assert!(!should_attempt_baked_sprite_load(
            true,
            "effects/impact.png",
            1
        ));
        assert!(!should_attempt_baked_sprite_load(true, "missing", 0));
    }

    /// Billboard shader must parse cleanly and declare the expected entry
    /// points. Parses the full concatenated source (billboard + the shared
    /// `sh_sample.wgsl` helper) so the helper's compilation in this pipeline is
    /// covered. Catches WGSL regressions before they reach pipeline creation.
    #[test]
    fn billboard_wgsl_parses() {
        let module = naga::front::wgsl::parse_str(BILLBOARD_SHADER_SOURCE)
            .expect("billboard shader should parse as WGSL");
        let has_vs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
        let has_fs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "fs_main" && ep.stage == naga::ShaderStage::Fragment);
        assert!(has_vs, "billboard.wgsl must export @vertex vs_main");
        assert!(has_fs, "billboard.wgsl must export @fragment fs_main");
    }

    /// The full billboard pipeline source (billboard + `sh_sample.wgsl`) must
    /// pass naga's validation, including control-flow uniformity. `parse_str`
    /// alone does not enforce this; a future edit that breaks the shared
    /// helper's compilation in the billboard pipeline is caught here at
    /// `cargo test` time, before GPU pipeline creation.
    #[test]
    fn billboard_wgsl_passes_naga_validation() {
        let module = naga::front::wgsl::parse_str(BILLBOARD_SHADER_SOURCE)
            .expect("billboard shader must parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("billboard shader must pass naga validation");
    }

    /// The `SpriteInstance` WGSL struct must match the CPU-side
    /// `SPRITE_INSTANCE_SIZE` byte layout.
    #[test]
    fn billboard_wgsl_sprite_instance_stride_matches_cpu() {
        // Parse the full concatenated source: `billboard.wgsl` references
        // symbols from `sh_sample.wgsl` and cannot parse standalone. The
        // `SpriteInstance` struct span is identical regardless of the appended
        // helper source.
        let module = naga::front::wgsl::parse_str(BILLBOARD_SHADER_SOURCE).unwrap();
        let span = module
            .types
            .iter()
            .find_map(|(_, ty)| match (&ty.name, &ty.inner) {
                (Some(name), naga::TypeInner::Struct { span, .. }) if name == "SpriteInstance" => {
                    Some(*span)
                }
                _ => None,
            })
            .expect("billboard.wgsl should declare struct SpriteInstance");
        assert_eq!(
            span as usize, SPRITE_INSTANCE_SIZE,
            "billboard.wgsl SpriteInstance stride ({span}) must match SPRITE_INSTANCE_SIZE ({SPRITE_INSTANCE_SIZE})",
        );
    }

    #[test]
    fn draw_params_layout() {
        let bytes = build_draw_params(8, 0.3, 3.0, 2.5, 12.0, PrmSlots::DIFFUSE | PrmSlots::NORMAL);
        assert_eq!(bytes.len(), SPRITE_DRAW_PARAMS_SIZE);
        // `params` retains the former 16-byte layout exactly.
        let count = u32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        assert_eq!(count, 8);
        let spec = f32::from_ne_bytes(bytes[4..8].try_into().unwrap());
        assert!((spec - 0.3).abs() < 1e-6);
        let lifetime = f32::from_ne_bytes(bytes[8..12].try_into().unwrap());
        assert!((lifetime - 3.0).abs() < 1e-6);
        let emissive = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        assert!((emissive - 2.5).abs() < 1e-6);
        let shimmer = f32::from_ne_bytes(bytes[16..20].try_into().unwrap());
        assert_eq!(shimmer, 1.0);
        let exponent = f32::from_ne_bytes(bytes[20..24].try_into().unwrap());
        assert!((exponent - 12.0).abs() < 1e-6);
        assert_eq!(bytes[24..32], [0; 8]);
    }

    #[test]
    fn diffuse_only_draw_params_keep_shimmer_clear_and_pack_default_exponent() {
        let bytes = build_draw_params(8, 0.3, 3.0, 0.0, 4.0, PrmSlots::DIFFUSE);
        assert_eq!(bytes.len(), SPRITE_DRAW_PARAMS_SIZE);
        assert_eq!(bytes[12..16], [0; 4]);
        assert_eq!(f32::from_ne_bytes(bytes[16..20].try_into().unwrap()), 0.0);
        assert_eq!(f32::from_ne_bytes(bytes[20..24].try_into().unwrap()), 4.0);
        assert_eq!(bytes[24..32], [0; 8]);
    }

    #[test]
    fn normal_slot_is_the_only_shimmer_classification_signal() {
        assert_eq!(
            sprite_shimmer_flag(PrmSlots::DIFFUSE | PrmSlots::NORMAL),
            1.0,
            "NORMAL must opt a collection into shimmer"
        );
        assert_eq!(
            sprite_shimmer_flag(PrmSlots::DIFFUSE),
            0.0,
            "diffuse-only collections stay isotropic"
        );
        assert_eq!(
            sprite_shimmer_flag(PrmSlots::DIFFUSE | PrmSlots::SPECULAR),
            0.0,
            "a specular mask without NORMAL cannot create shimmer"
        );
    }

    #[test]
    fn billboard_fragment_adds_emissive_without_a_light_term_mask_gate() {
        let fragment = include_str!("../shaders/billboard.wgsl")
            .split("@fragment\nfn fs_main")
            .nth(1)
            .expect("billboard shader must define fs_main");

        assert!(
            fragment.contains(
                "let emissive_rgb = sprite_sample.rgb * draw_params.params.w * in.opacity;"
            )
        );
        assert!(fragment.contains("(lit_rgb + emissive_rgb) * sprite_sample.a"));
        // Shimmer now legitimately reads `light_term_mask` for its static
        // specular lobe. The emissive expression remains an unconditional
        // local assignment and does not consume that mask.
    }

    #[test]
    fn billboard_emissive_obeys_instance_opacity() {
        // Regression: emissive RGB survived at full strength when opacity was zero,
        // even though the billboard's alpha and scene-lit contribution vanished.
        let fragment = include_str!("../shaders/billboard.wgsl")
            .split("@fragment\nfn fs_main")
            .nth(1)
            .expect("billboard shader must define fs_main");
        assert!(
            fragment.contains(
                "let emissive_rgb = sprite_sample.rgb * draw_params.params.w * in.opacity;"
            ),
            "billboard emissive must use the same instance opacity as scene-lit RGB"
        );
    }

    #[test]
    fn sprite_array_plan_uses_one_layer_fallback_for_empty_input() {
        let plan = plan_sprite_array(&[], 256, 4096).expect("1x1 fallback fits");
        assert_eq!(plan.width, 1);
        assert_eq!(plan.height, 1);
        assert_eq!(plan.frame_count(), 1);
        assert_eq!(plan.fallback, Some(SpriteArrayFallback::InvalidInput));
    }

    #[test]
    fn sprite_array_plan_keeps_single_frame_payload() {
        let frame = SpriteFrame {
            data: vec![0xFFu8; 4 * 2 * 2], // 2x2 white RGBA
            width: 2,
            height: 2,
        };
        let input = [frame];
        let plan = plan_sprite_array(&input, 256, 2).unwrap();
        assert_eq!(plan.width, 2);
        assert_eq!(plan.height, 2);
        assert_eq!(plan.frames.len(), 1);
        assert_eq!(plan.frames[0].data, vec![0xFFu8; 4 * 2 * 2]);
    }

    #[test]
    fn sprite_array_plan_keeps_each_normalized_layer_payload() {
        let red = SpriteFrame {
            data: vec![
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ],
            width: 2,
            height: 2,
        };
        let blue = SpriteFrame {
            data: vec![
                0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255,
            ],
            width: 2,
            height: 2,
        };
        let input = [red.clone(), blue.clone()];
        let plan = plan_sprite_array(&input, 256, 2).unwrap();
        assert_eq!(plan.width, 2);
        assert_eq!(plan.height, 2);
        assert_eq!(plan.frames.len(), 2);
        assert_eq!(plan.frames[0].data, red.data);
        assert_eq!(plan.frames[1].data, blue.data);
    }

    #[test]
    fn sprite_frame_count_at_device_limit_fits() {
        assert!(sprite_frame_count_fits_device(256, 256));
    }

    #[test]
    fn sprite_frame_count_below_device_limit_fits() {
        assert!(sprite_frame_count_fits_device(255, 256));
    }

    #[test]
    fn sprite_frame_count_above_device_limit_is_rejected() {
        assert!(!sprite_frame_count_fits_device(257, 256));
    }

    #[test]
    fn oversized_sprite_array_plan_falls_back_to_one_layer() {
        let frames = vec![
            SpriteFrame {
                data: vec![255; 4],
                width: 1,
                height: 1,
            };
            257
        ];

        let plan = plan_sprite_array(&frames, 256, 1).expect("frame zero is a safe fallback");

        assert_eq!(plan.requested_frame_count, 257);
        assert_eq!(plan.fallback, Some(SpriteArrayFallback::FrameLayerLimit));
        assert_eq!(plan.frames.len(), 1);
        assert_eq!(plan.frames[0].data, vec![255; 4]);
    }

    #[test]
    fn sprite_array_plan_accepts_extents_at_the_device_limit() {
        let input = [SpriteFrame {
            data: vec![0; 4 * 3 * 3],
            width: 3,
            height: 3,
        }];

        let plan = plan_sprite_array(&input, 1, 3).expect("extent at limit fits");

        assert_eq!(plan.width, 3);
        assert_eq!(plan.height, 3);
        assert_eq!(plan.fallback, None);
    }

    #[test]
    fn sprite_array_plan_uses_fallback_for_an_extent_over_the_device_limit() {
        let input = [SpriteFrame {
            data: vec![0; 4 * 2 * 3],
            width: 2,
            height: 3,
        }];

        let plan = plan_sprite_array(&input, 1, 2).expect("1x1 fallback fits");

        assert_eq!(plan.width, 1);
        assert_eq!(plan.height, 1);
        assert_eq!(plan.frame_count(), 1);
        assert_eq!(plan.fallback, Some(SpriteArrayFallback::InvalidInput));
    }

    #[test]
    fn sprite_array_plan_uses_fallback_for_short_or_long_rgba_payloads() {
        for data_len in [15, 17] {
            let input = [SpriteFrame {
                data: vec![0; data_len],
                width: 2,
                height: 2,
            }];

            let plan = plan_sprite_array(&input, 1, 2).expect("1x1 fallback fits");

            assert_eq!(plan.width, 1, "data length {data_len}");
            assert_eq!(plan.height, 1, "data length {data_len}");
            assert_eq!(plan.frame_count(), 1, "data length {data_len}");
            assert_eq!(plan.fallback, Some(SpriteArrayFallback::InvalidInput));
        }
    }

    #[test]
    fn sprite_array_plan_uses_fallback_when_rgba_byte_count_overflows() {
        let input = [SpriteFrame {
            data: Vec::new(),
            width: u32::MAX,
            height: u32::MAX,
        }];

        let plan = plan_sprite_array(&input, 1, u32::MAX).expect("1x1 fallback fits");

        assert_eq!(plan.width, 1);
        assert_eq!(plan.height, 1);
        assert_eq!(plan.frame_count(), 1);
        assert_eq!(plan.fallback, Some(SpriteArrayFallback::InvalidInput));
    }

    #[test]
    fn sprite_array_plan_uses_fallback_for_a_zero_extent() {
        let input = [SpriteFrame {
            data: Vec::new(),
            width: 0,
            height: 1,
        }];

        let plan = plan_sprite_array(&input, 1, 1).expect("1x1 fallback fits");

        assert_eq!(plan.width, 1);
        assert_eq!(plan.height, 1);
        assert_eq!(plan.fallback, Some(SpriteArrayFallback::InvalidInput));
    }

    #[test]
    fn oversized_sprite_collection_warns_before_gpu_upload() {
        use log::Level;
        use postretro_test_log_capture::LogCapture;

        let capture = LogCapture::start();
        warn_sprite_frame_count_exceeds_device_limit("test", 257, 256);
        capture.assert_logged_once(
            Level::Warn,
            "[Smoke] Collection 'test' requires 257 sprite frame array layers, exceeding device maxTextureArrayLayers 256; falling back to frame 0 as one array layer",
        );
    }

    #[test]
    fn invalid_sprite_collection_warns_before_gpu_upload() {
        use log::Level;
        use postretro_test_log_capture::LogCapture;

        let capture = LogCapture::start();
        warn_sprite_array_invalid_input("test");
        capture.assert_logged_once(
            Level::Warn,
            "[Smoke][invalid-sprite-array] Collection 'test' has invalid sprite frame data or an extent unsupported by this device; falling back to a 1x1 white array layer",
        );
    }

    #[test]
    fn sprite_sheet_layout_binds_filterable_d2_array_material_inputs() {
        let entries = sprite_sheet_bind_group_layout_entries();
        assert_eq!(entries.len(), 5);
        for (binding, entry) in [(0, &entries[0]), (3, &entries[3]), (4, &entries[4])] {
            assert_eq!(entry.binding, binding);
            assert!(matches!(
                entry.ty,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                }
            ));
        }
        assert_eq!(entries[3].visibility, wgpu::ShaderStages::FRAGMENT);
        assert_eq!(entries[4].visibility, wgpu::ShaderStages::FRAGMENT);
    }

    #[test]
    fn billboard_shader_samples_the_flat_frame_array_layer() {
        let shader = include_str!("../shaders/billboard.wgsl");
        assert!(shader.contains("var sprite_texture: texture_2d_array<f32>;"));
        assert!(shader.contains("var sprite_specular: texture_2d_array<f32>;"));
        assert!(shader.contains("var sprite_normal: texture_2d_array<f32>;"));
        assert!(shader.contains("@location(4) @interpolate(flat) frame_idx: u32,"));
        assert!(shader.contains("@location(5) @interpolate(flat) rotation: f32,"));
        assert!(shader.contains("out.uv = vec2<f32>(cd.z, cd.w);"));
        assert!(shader.contains("out.frame_idx = frame_idx;"));
        assert!(shader.contains("layer: u32,"));
        assert!(shader.contains("textureDimensions(tex, 0)"));
        assert!(shader.contains("textureSampleGrad(tex, samp, uv_recon, i32(layer), ddx, ddy);"));
        assert!(shader.contains("in.frame_idx,"));
    }

    /// A dummy packed slice of `n` sprite instances (contents irrelevant to the
    /// layout planner — only the byte length matters).
    fn packed(n: usize) -> Vec<u8> {
        vec![0u8; n * SPRITE_INSTANCE_SIZE]
    }

    #[test]
    fn align_up_rounds_to_256_byte_boundary() {
        assert_eq!(align_up_to_dynamic_offset(0), 0);
        assert_eq!(align_up_to_dynamic_offset(1), 256);
        assert_eq!(align_up_to_dynamic_offset(256), 256);
        assert_eq!(align_up_to_dynamic_offset(257), 512);
        // One 32-byte instance still pads up to a full 256-byte region.
        assert_eq!(align_up_to_dynamic_offset(SPRITE_INSTANCE_SIZE), 256);
    }

    #[test]
    fn plan_layout_empty_or_all_zero_returns_none() {
        assert!(plan_frame_layout(&[]).is_none());
        let empty: Vec<u8> = Vec::new();
        assert!(plan_frame_layout(&[("smoke", &empty)]).is_none());
    }

    /// The capacity the buffer must reach for a given frame layout, computed the
    /// same way `record_draws` does from a monotonic window. Mirrors the growth
    /// math so the layout tests can assert the binding contract without a GPU.
    fn required_capacity(layout: &FrameLayout, prior_window: u64) -> (u64, usize) {
        let window = prior_window.max(layout.frame_max_region as u64);
        (window, layout.last_offset + window as usize)
    }

    #[test]
    fn plan_layout_single_collection_starts_at_zero_with_full_count() {
        let bytes = packed(10);
        let layout = plan_frame_layout(&[("smoke", &bytes)]).unwrap();
        assert_eq!(layout.placements.len(), 1);
        assert_eq!(layout.placements[0].offset, 0);
        assert_eq!(layout.placements[0].live_count, 10);
    }

    #[test]
    fn plan_layout_offsets_are_256_aligned_and_non_overlapping() {
        // 10 instances = 320 bytes → padded to 512; next region starts at 512.
        let a = packed(10);
        let b = packed(3);
        let layout = plan_frame_layout(&[("smoke", &a), ("spark", &b)]).unwrap();
        let placements = &layout.placements;
        assert_eq!(placements.len(), 2);
        assert_eq!(placements[0].offset, 0);
        assert_eq!(placements[1].offset, 512);
        for p in placements {
            assert_eq!(
                p.offset as usize % STORAGE_DYNAMIC_OFFSET_ALIGNMENT,
                0,
                "every collection's dynamic offset must be 256-byte aligned",
            );
        }
        // Region 0 spans bytes [0, 320) of live data within its 512-byte padded
        // region, so region 1 at offset 512 cannot overlap it.
        assert!(placements[1].offset as usize >= a.len());
    }

    #[test]
    fn plan_layout_single_collection_exceeds_old_4096_cap_without_truncation() {
        // The old buffer silently truncated a collection to 4096 sprites. The
        // planner preserves the full count and reserves a region large enough to
        // hold all of them.
        let count = 9000;
        let bytes = packed(count);
        let layout = plan_frame_layout(&[("smoke", &bytes)]).unwrap();
        assert_eq!(layout.placements[0].live_count, count as u32);
        // A fresh pass would seed its window at the 256-byte alignment unit;
        // `record_draws` raises it to this frame's `frame_max_region`.
        let (_window, capacity) =
            required_capacity(&layout, STORAGE_DYNAMIC_OFFSET_ALIGNMENT as u64);
        assert!(
            capacity >= count * SPRITE_INSTANCE_SIZE,
            "buffer capacity must hold every live sprite, not just 4096",
        );
    }

    #[test]
    fn plan_layout_capacity_covers_min_binding_size_window_at_every_offset() {
        // The group-6 BGL declares `min_binding_size = SPRITE_INSTANCE_SIZE`
        // (one instance stride). This is the shader-side floor on the bound
        // window — NOT the dynamic-offset gate (see the note in `SmokePass::new`).
        // The capacity must still clear that floor for *every* collection's
        // offset, so each region holds at least one instance. The per-collection
        // padded region (≥ 256 B) the capacity reserves dominates the 32-byte
        // floor, so this holds by construction; the test pins it so a future
        // capacity-formula change can't silently violate the binding contract.
        let a = packed(10);
        let b = packed(3);
        let c = packed(50);
        let layout = plan_frame_layout(&[("smoke", &a), ("spark", &b), ("dust", &c)]).unwrap();
        let (_window, capacity) =
            required_capacity(&layout, STORAGE_DYNAMIC_OFFSET_ALIGNMENT as u64);
        for p in &layout.placements {
            assert!(
                p.offset as usize + SPRITE_INSTANCE_SIZE <= capacity,
                "offset {} + min_binding_size {} must fit in capacity {}",
                p.offset,
                SPRITE_INSTANCE_SIZE,
                capacity,
            );
        }
    }

    #[test]
    fn plan_layout_capacity_covers_the_largest_collection_window() {
        // The dynamic-offset bind-group window is sized to the largest region,
        // so the reported capacity must cover `last_offset + largest_window`
        // even when the largest collection is not last.
        let big = packed(100); // 3200 B → padded 3328 (13 × 256)
        let small = packed(2); // 64 B → padded 256
        let layout = plan_frame_layout(&[("big", &big), ("small", &small)]).unwrap();
        let last = layout.placements.last().unwrap();
        let largest_window = align_up_to_dynamic_offset(100 * SPRITE_INSTANCE_SIZE);
        // The window is the largest region even when it is not the last
        // collection; capacity must cover `last_offset + window`.
        let (window, capacity) =
            required_capacity(&layout, STORAGE_DYNAMIC_OFFSET_ALIGNMENT as u64);
        assert_eq!(window as usize, largest_window);
        assert_eq!(capacity, last.offset as usize + largest_window);
    }

    /// THE regression guard for the wgpu-29 dynamic-offset bug. With
    /// `as_entire_binding()` the bound window equals the whole buffer, so
    /// wgpu-29 derives `maximum_dynamic_offset = buffer.size - window = 0` and
    /// rejects every collection past offset 0. The fix binds an explicit window
    /// (`record_draws`'s monotonic high-water mark) so capacity is sized to
    /// `last_offset + window`. This test pins, for a multi-collection frame,
    /// that the window/capacity math `record_draws` uses keeps every dynamic
    /// offset legal:
    ///   - `last_offset + window <= capacity` (window fits, invariant 2), and
    ///   - `maximum_dynamic_offset = capacity - window >= every placement.offset`
    ///     (every collection's offset is admissible, the exact wgpu-29 gate).
    ///
    /// Also checks the window itself is 256-aligned (storage size alignment,
    /// invariant 3) so the bound `size` is a legal storage binding size.
    #[test]
    fn dynamic_offset_never_exceeds_maximum_for_every_collection() {
        // A spread of collection sizes (largest is NOT last) so the window is
        // driven by an interior collection and the gate is non-trivial.
        let a = packed(7); // 224 B → padded 256
        let big = packed(300); // 9600 B → padded 9728 (38 × 256)
        let c = packed(40); // 1280 B → padded 1280 (5 × 256)
        let d = packed(1); // 32 B → padded 256
        let layout = plan_frame_layout(&[("a", &a), ("big", &big), ("c", &c), ("d", &d)]).unwrap();

        // Seed the prior window the way a fresh `SmokePass` does: one alignment
        // unit. `record_draws` raises it to this frame's `frame_max_region`.
        let (window, capacity) =
            required_capacity(&layout, STORAGE_DYNAMIC_OFFSET_ALIGNMENT as u64);

        // Window is a legal storage binding size (multiple of 256) and fits.
        assert_eq!(
            window as usize % STORAGE_DYNAMIC_OFFSET_ALIGNMENT,
            0,
            "bound window must be a multiple of the 256-byte storage alignment",
        );
        assert!(
            layout.last_offset + window as usize <= capacity,
            "last_offset {} + window {} must fit in capacity {} (invariant 2)",
            layout.last_offset,
            window,
            capacity,
        );

        // The wgpu-29 gate: every collection's dynamic offset must be
        // <= maximum_dynamic_offset = capacity - window.
        let maximum_dynamic_offset = capacity - window as usize;
        for p in &layout.placements {
            assert!(
                p.offset as usize <= maximum_dynamic_offset,
                "collection '{}' offset {} exceeds maximum_dynamic_offset {} \
                 (capacity {} - window {})",
                p.collection,
                p.offset,
                maximum_dynamic_offset,
                capacity,
                window,
            );
        }
    }
}
