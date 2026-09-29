// One streamed pool generation's textures, their growth copy and block moves,
// and the retiring generation kept until its submitted work is done.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use postretro_render_cpu::lightmap_pool::PoolCopy;

use super::super::pool::{
    PoolFormat, PoolPlane, StaticPoolTextures, block_copy_regions, create_pool_textures,
};
use crate::render::residency::texture_bytes;

/// One pool generation: irradiance, direction and (when the level keeps id
/// 42) shadowmask, each `array_layers` deep. The last layer is the spare a
/// repack stages same-layer moves through; the shader never samples it.
pub(crate) struct PoolTextures {
    pub(crate) irradiance: wgpu::Texture,
    pub(crate) direction: wgpu::Texture,
    pub(crate) shadowmask: Option<wgpu::Texture>,
    array_layers: u32,
}

impl PoolTextures {
    pub(super) fn new(
        device: &wgpu::Device,
        format: PoolFormat,
        array_layers: u32,
        with_shadowmask: bool,
    ) -> Self {
        let StaticPoolTextures {
            irradiance,
            direction,
            shadowmask,
        } = create_pool_textures(device, format, array_layers, with_shadowmask);
        Self {
            irradiance,
            direction,
            shadowmask,
            array_layers,
        }
    }

    pub(super) fn array_layers(&self) -> u32 {
        self.array_layers
    }

    pub(super) fn plane(&self, plane: PoolPlane) -> Option<&wgpu::Texture> {
        match plane {
            PoolPlane::Irradiance => Some(&self.irradiance),
            PoolPlane::Direction => Some(&self.direction),
            PoolPlane::Shadowmask => self.shadowmask.as_ref(),
        }
    }

    /// Requested bytes of the set, spare layer included.
    pub(super) fn bytes(&self) -> u64 {
        [
            Some(&self.irradiance),
            Some(&self.direction),
            self.shadowmask.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(|texture| texture_bytes(texture.format(), texture.size()))
        .sum()
    }

    /// Growth: copy layers `0..layers` of every texture into `next` at origin
    /// zero, so every placement keeps its address in the new generation.
    pub(super) fn copy_layers_into(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        next: &PoolTextures,
        layers: u32,
    ) {
        if layers == 0 {
            return;
        }
        let pairs = [
            (Some(&self.irradiance), Some(&next.irradiance)),
            (Some(&self.direction), Some(&next.direction)),
            (self.shadowmask.as_ref(), next.shadowmask.as_ref()),
        ];
        for (source, destination) in pairs {
            let (Some(source), Some(destination)) = (source, destination) else {
                continue;
            };
            let size = source.size();
            encoder.copy_texture_to_texture(
                source.as_image_copy(),
                destination.as_image_copy(),
                wgpu::Extent3d {
                    width: size.width,
                    height: size.height,
                    depth_or_array_layers: layers,
                },
            );
        }
    }

    /// Record one planned block move as same-texture copies between two
    /// distinct array layers of every plane.
    pub(super) fn record_block_copy(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        format: PoolFormat,
        copy: PoolCopy,
    ) {
        for region in block_copy_regions(format, copy, self.shadowmask.is_some()) {
            let texture = self
                .plane(region.plane)
                .expect("shadowmask copies only when the pool keeps one");
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: region.src,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: region.dst,
                    aspect: wgpu::TextureAspect::All,
                },
                region.extent,
            );
        }
    }
}

/// A grown-out generation. It holds the old textures until the submission
/// that copied out of them is done; the queue callback holds only the flag,
/// so it keeps no texture alive and touches no later level's state.
pub(super) struct RetiringPool {
    textures: PoolTextures,
    complete: Arc<AtomicBool>,
}

impl RetiringPool {
    /// Retire `textures` behind the work submitted so far on `queue`.
    pub(super) fn after_submitted_work(queue: &wgpu::Queue, textures: PoolTextures) -> Self {
        let complete = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&complete);
        queue.on_submitted_work_done(move || flag.store(true, Ordering::Release));
        Self { textures, complete }
    }

    pub(super) fn is_complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    pub(super) fn bytes(&self) -> u64 {
        self.textures.bytes()
    }

    #[cfg(test)]
    pub(super) fn completion_flag(&self) -> &Arc<AtomicBool> {
        &self.complete
    }
}
