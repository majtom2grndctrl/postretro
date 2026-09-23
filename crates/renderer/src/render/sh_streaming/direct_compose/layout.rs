//! Existing direct-compose binding layouts and small wire-format helpers.

use postretro_render_cpu::frame_uniforms::LightTermMask;
use postretro_render_cpu::sh_compose::DYNAMIC_COMPOSE_GRID_DIMS_SIZE;

use crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
use crate::render::direct_sh_compose::{
    BIND_AFFINITY_LIGHTS, BIND_AFFINITY_OFFSETS, BIND_ANIMATION_DESCRIPTOR_INDICES,
    BIND_ANIMATION_DESCRIPTORS, BIND_ANIMATION_SAMPLES, BIND_BASE_SAMPLER,
    BIND_DELTA_COMPACTION_META, BIND_DELTA_SUBBLOCKS, BIND_PROBE_INDIRECTION,
    DirectShDebugOverride, dynamic_compose_grid_bgl_entry, sampler_bgl_entry, storage_bgl_entry,
    storage_texture_bgl_entry, texture_bgl_entry, uniform_bgl_entry,
};

const BIND_SELECTION_WEIGHTS: u32 = 26;
const BIND_DEBUG_OVERRIDE: u32 = 27;
const BIND_FRAME_LIGHT_TERM_MASK: u32 = 29;
const BIND_ANIMATED_LIGHT_SCALE: u32 = 26;
const BIND_ANIMATED_COMPACTION_META: u32 = 27;
const BIND_ANIMATED_PROBE_INDIRECTION: u32 = 28;
const DIRECT_COMPOSE_PARAMS_SIZE: usize = 16;
pub(super) const DEBUG_OVERRIDE_SIZE: usize = 32;
pub(super) const ANIMATED_LIGHT_SCALE_SIZE: usize = 1040;

pub(super) fn promotion_bgl_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        texture_bgl_entry(0),
        sampler_bgl_entry(BIND_BASE_SAMPLER),
        storage_texture_bgl_entry(1),
        dynamic_compose_grid_bgl_entry(),
        storage_bgl_entry(BIND_DELTA_SUBBLOCKS),
        storage_bgl_entry(BIND_AFFINITY_OFFSETS),
        storage_bgl_entry(BIND_AFFINITY_LIGHTS),
        storage_bgl_entry(BIND_SELECTION_WEIGHTS),
        uniform_bgl_entry(BIND_DEBUG_OVERRIDE),
        storage_bgl_entry(BIND_DELTA_COMPACTION_META),
        uniform_layout_entry(
            BIND_FRAME_LIGHT_TERM_MASK,
            DIRECT_COMPOSE_PARAMS_SIZE as u64,
        ),
        storage_bgl_entry(BIND_PROBE_INDIRECTION),
    ]
}

pub(super) fn animated_bgl_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        texture_bgl_entry(0),
        sampler_bgl_entry(BIND_BASE_SAMPLER),
        storage_texture_bgl_entry(1),
        dynamic_compose_grid_bgl_entry(),
        storage_bgl_entry(BIND_DELTA_SUBBLOCKS),
        storage_bgl_entry(BIND_AFFINITY_OFFSETS),
        storage_bgl_entry(BIND_ANIMATION_DESCRIPTORS),
        storage_bgl_entry(BIND_ANIMATION_SAMPLES),
        storage_bgl_entry(BIND_AFFINITY_LIGHTS),
        storage_bgl_entry(BIND_ANIMATION_DESCRIPTOR_INDICES),
        uniform_layout_entry(BIND_ANIMATED_LIGHT_SCALE, ANIMATED_LIGHT_SCALE_SIZE as u64),
        storage_bgl_entry(BIND_ANIMATED_COMPACTION_META),
        storage_bgl_entry(BIND_ANIMATED_PROBE_INDIRECTION),
    ]
}

fn uniform_layout_entry(binding: u32, size: u64) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(size),
        },
        count: None,
    }
}

pub(super) fn dynamic_grid_entry(buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding: 18,
        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer,
            offset: 0,
            size: wgpu::BufferSize::new(DYNAMIC_COMPOSE_GRID_DIMS_SIZE as u64),
        }),
    }
}

pub(super) fn texture_entry(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

pub(super) fn storage_texture_entry(
    binding: u32,
    view: &wgpu::TextureView,
) -> wgpu::BindGroupEntry<'_> {
    texture_entry(binding, view)
}

pub(super) fn sampler_entry(binding: u32, sampler: &wgpu::Sampler) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::Sampler(sampler),
    }
}

pub(super) fn storage_entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

pub(super) fn uniform_entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    storage_entry(binding, buffer)
}

pub(super) fn debug_override_bytes(value: DirectShDebugOverride) -> [u8; DEBUG_OVERRIDE_SIZE] {
    let mut bytes = [0; DEBUG_OVERRIDE_SIZE];
    bytes[0..4].copy_from_slice(&(value.enabled as u32).to_ne_bytes());
    bytes[4..8].copy_from_slice(&value.selection_index.to_ne_bytes());
    bytes[16..20].copy_from_slice(&value.weight.clamp(0.0, 1.0).to_ne_bytes());
    bytes
}

pub(super) fn light_term_mask_bytes(mask: LightTermMask) -> [u8; DIRECT_COMPOSE_PARAMS_SIZE] {
    let mut bytes = [0; DIRECT_COMPOSE_PARAMS_SIZE];
    bytes[0..4].copy_from_slice(&mask.bits().to_ne_bytes());
    bytes
}

pub(super) fn u32_bytes(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_ne_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_compose_keeps_existing_binding_numbers_and_dynamic_grid() {
        let promotion = promotion_bgl_entries();
        let animated = animated_bgl_entries();
        assert!(promotion.iter().any(|entry| matches!(entry.ty,
            wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true,
                min_binding_size: Some(size) } if entry.binding == 18 && size.get() == DYNAMIC_COMPOSE_GRID_DIMS_SIZE as u64)));
        assert_eq!(
            promotion
                .iter()
                .map(|entry| entry.binding)
                .collect::<Vec<_>>(),
            vec![0, 2, 1, 18, 20, 21, 24, 26, 27, 28, 29, 30]
        );
        assert_eq!(
            animated
                .iter()
                .map(|entry| entry.binding)
                .collect::<Vec<_>>(),
            vec![0, 2, 1, 18, 20, 21, 22, 23, 24, 25, 26, 27, 28]
        );
    }

    #[test]
    fn animated_light_scale_defaults_to_full_animated_contribution() {
        let bytes = AnimatedDirectShDebugOverride::default().bytes(&[]);
        assert_eq!(f32::from_ne_bytes(bytes[16..20].try_into().unwrap()), 1.0);
        assert_eq!(
            f32::from_ne_bytes(bytes[1036..1040].try_into().unwrap()),
            1.0
        );
    }
}
