// One streamed pool's textures, its generations, and block moves: each pool
// grows and retires alone, and a retiring generation stays until its submitted
// work is done.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use postretro_render_cpu::lightmap_pool::{PoolCopy, PoolKind};

use super::super::pool::{
    PoolFormat, PoolPlane, block_copy_regions, direction_descriptor, irradiance_descriptor,
    shadowmask_descriptor,
};
use crate::render::gpu_create::GpuCreate;
use crate::render::residency::texture_bytes;

/// One pool generation's textures, each `array_layers` deep. The last layer is
/// the spare a repack stages moves through; the shader never samples it.
pub(crate) enum PoolTextures {
    Lightmap {
        irradiance: wgpu::Texture,
        direction: wgpu::Texture,
    },
    Mask {
        shadowmask: wgpu::Texture,
    },
}

impl PoolTextures {
    pub(super) fn new(
        device: &wgpu::Device,
        pool: PoolKind,
        format: PoolFormat,
        array_layers: u32,
    ) -> Self {
        match pool {
            PoolKind::Lightmap => Self::Lightmap {
                irradiance: device.new_texture(&irradiance_descriptor(format, array_layers)),
                direction: device.new_texture(&direction_descriptor(format, array_layers)),
            },
            PoolKind::Mask => Self::Mask {
                shadowmask: device.new_texture(&shadowmask_descriptor(array_layers)),
            },
        }
    }

    fn pool(&self) -> PoolKind {
        match self {
            Self::Lightmap { .. } => PoolKind::Lightmap,
            Self::Mask { .. } => PoolKind::Mask,
        }
    }

    fn all(&self) -> impl Iterator<Item = &wgpu::Texture> {
        let textures = match self {
            Self::Lightmap {
                irradiance,
                direction,
            } => [Some(irradiance), Some(direction)],
            Self::Mask { shadowmask } => [Some(shadowmask), None],
        };
        textures.into_iter().flatten()
    }

    pub(super) fn array_layers(&self) -> u32 {
        self.all()
            .next()
            .map_or(0, wgpu::Texture::depth_or_array_layers)
    }

    /// `plane`'s texture; `plane` is one of this pool's.
    pub(super) fn plane(&self, plane: PoolPlane) -> &wgpu::Texture {
        match (self, plane) {
            (Self::Lightmap { irradiance, .. }, PoolPlane::Irradiance) => irradiance,
            (Self::Lightmap { direction, .. }, PoolPlane::Direction) => direction,
            (Self::Mask { shadowmask }, PoolPlane::Shadowmask) => shadowmask,
            _ => unreachable!(
                "{plane:?} is not a plane of the {} pool",
                self.pool().name()
            ),
        }
    }

    /// Requested bytes of the generation, spare layer included.
    pub(super) fn bytes(&self) -> u64 {
        self.all()
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
        for (source, destination) in self.all().zip(next.all()) {
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

    /// Record one planned move as same-texture copies between two distinct
    /// array layers of each of this pool's textures.
    pub(super) fn record_block_copy(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        format: PoolFormat,
        copy: PoolCopy,
    ) {
        for region in block_copy_regions(self.pool(), format, copy) {
            let texture = self.plane(region.plane);
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

/// One pool's live generation, and the one its last growth retired.
pub(super) struct PoolGeneration {
    pub(super) live: PoolTextures,
    pub(super) retiring: Option<RetiringPool>,
}

impl PoolGeneration {
    pub(super) fn new(live: PoolTextures) -> Self {
        Self {
            live,
            retiring: None,
        }
    }

    /// `next` goes live and the old generation retires behind the work
    /// submitted so far on `queue`.
    pub(super) fn replace(&mut self, queue: &wgpu::Queue, next: PoolTextures) {
        debug_assert!(self.retiring.is_none(), "one generation retires at a time");
        let previous = std::mem::replace(&mut self.live, next);
        self.retiring = Some(RetiringPool::after_submitted_work(queue, previous));
    }

    /// Drop a retiring generation whose submitted work is done; whether one
    /// went.
    pub(super) fn release_completed(&mut self) -> bool {
        let done = self
            .retiring
            .as_ref()
            .is_some_and(RetiringPool::is_complete);
        if done {
            self.retiring = None;
        }
        done
    }

    pub(super) fn retiring_bytes(&self) -> u64 {
        self.retiring.as_ref().map_or(0, RetiringPool::bytes)
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

    /// Take completion out of the queue's hands, so a test decides when this
    /// generation's work is done: the returned flag is the only one read.
    #[cfg(test)]
    pub(super) fn hold_completion(&mut self) -> Arc<AtomicBool> {
        self.complete = Arc::new(AtomicBool::new(false));
        Arc::clone(&self.complete)
    }
}
