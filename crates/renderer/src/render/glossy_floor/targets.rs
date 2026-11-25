// One level's glossy floor-layer targets: the highlight-free scene colour at
// scene extent, the scene-colour mip chain at the floor tier's fraction of it,
// and the floor layer's target and march depth at that fraction, with the
// per-level groups the chain's downsample and the march read.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors) · context/lib/resource_management.md §5.4

use postretro_level_format::SectionId;
use postretro_render_cpu::fluid_surface::reflection_hemisphere::REFLECTION_UP_FACING_MIN_COS;
use postretro_render_cpu::fluid_surface::reflection_quality::{
    FluidReflectionPreset, FluidReflectionQuality,
};
use postretro_render_cpu::glossy_floor_blur::GlossyFloorTermBlur;
use postretro_render_cpu::render_extent::Extent;
use postretro_render_data::geometry::WorldVertex;

use super::march::{GLOSSY_FLOOR_LAYER_FORMAT, GlossyFloorMarchUniform};
use crate::render::DEPTH_FORMAT;
use crate::render::gpu_create::GpuCreate;
use crate::render::residency::{ResidencyAllocation, ResidencyAllocationState, texture_row};

/// The memory ledger's row for the highlight-free scene colour.
pub const GLOSSY_HIGHLIGHT_FREE: &str = "glossy_highlight_free";
/// The memory ledger's row for the scene-colour mip chain, every level.
pub const GLOSSY_SCENE_COLOUR_CHAIN: &str = "glossy_scene_colour_chain";
/// The memory ledger's row for the floor layer's target, (c × X, c).
pub const GLOSSY_FLOOR_LAYER: &str = "glossy_floor_layer";
/// The memory ledger's row for the march's own depth.
pub const GLOSSY_FLOOR_LAYER_DEPTH: &str = "glossy_floor_layer_depth";

/// Whether a world probe stream binds a face that faces up by water's test:
/// a vertex of a bound face whose normal lies within 1° of +Y. The compiler
/// gives every face its own vertices and brush faces carry flat normals, so
/// one vertex speaks for its face.
pub(crate) fn binds_an_up_facing_face(vertices: &[WorldVertex], world_probes: &[u32]) -> bool {
    vertices
        .iter()
        .zip(world_probes)
        .filter(|(_, probe)| **probe != postretro_level_loader::NO_WORLD_PROBE)
        .any(|(vertex, _)| {
            postretro_level_format::octahedral::decode(vertex.normal_oct)[1]
                >= REFLECTION_UP_FACING_MIN_COS
        })
}

/// What a level's floor targets bind from the floor pass, cloned for a level
/// build on any thread.
#[derive(Clone)]
pub(crate) struct GlossyFloorLayout {
    source_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
    /// The march's group 3: the chain, its sampler and the tier's march.
    march_layout: wgpu::BindGroupLayout,
    march_sampler: wgpu::Sampler,
    /// 1×1, c = 0: what the overlay binds while no floor-layer target exists.
    placeholder: wgpu::TextureView,
}

/// One chain level: the view the downsample renders into, and the group it
/// reads (the highlight-free colour for level 0, the level above for the
/// rest).
struct ChainLevel {
    target: wgpu::TextureView,
    source: wgpu::BindGroup,
}

/// One level's floor-layer targets, owned by its glossy state and dropped
/// with it. Exist only while the level's set is published and has an
/// up-facing glossy face at a tier that runs the floor layer.
pub(crate) struct GlossyFloorTargets {
    highlight_free: wgpu::Texture,
    highlight_free_view: wgpu::TextureView,
    chain: wgpu::Texture,
    /// Every level, as the march group binds it for the hit to sample by
    /// level; a march-only tier change binds it again.
    chain_view: wgpu::TextureView,
    levels: Vec<ChainLevel>,
    /// The floor layer's target, (c × X, c) at the chain's level-0 extent,
    /// and the march's own depth beside it.
    layer: wgpu::Texture,
    layer_view: wgpu::TextureView,
    layer_depth: wgpu::Texture,
    layer_depth_view: wgpu::TextureView,
    /// The march's group 3 over the chain and the tier's march uniform.
    march_group: wgpu::BindGroup,
    /// The floor tier's preset the march was written by; its fraction sized
    /// the targets.
    preset: FluidReflectionPreset,
    /// The march uniform the group holds.
    march: GlossyFloorMarchUniform,
}

impl GlossyFloorLayout {
    pub(super) fn new(
        source_layout: wgpu::BindGroupLayout,
        sampler: wgpu::Sampler,
        format: wgpu::TextureFormat,
        march_layout: wgpu::BindGroupLayout,
        march_sampler: wgpu::Sampler,
        placeholder: wgpu::TextureView,
    ) -> Self {
        Self {
            source_layout,
            sampler,
            format,
            march_layout,
            march_sampler,
            placeholder,
        }
    }

    /// The march's trilinear, clamped chain sampler (test-only).
    #[cfg(test)]
    pub(crate) fn march_sampler(&self) -> &wgpu::Sampler {
        &self.march_sampler
    }

    /// What the overlay binds as the floor layer: `targets`' target, or the
    /// placeholder without one.
    pub(crate) fn overlay_view<'a>(
        &'a self,
        targets: Option<&'a GlossyFloorTargets>,
    ) -> &'a wgpu::TextureView {
        targets.map_or(&self.placeholder, |targets| &targets.layer_view)
    }

    /// Bring `targets` in line with `quality` at `scene`: none at a tier that
    /// runs no floor layer or on a level without an up-facing glossy face
    /// (`floor_blur` `None`); otherwise at the scene extent and the tier's
    /// preset, the march blurring by the level's term blur.
    /// Textures already at the scene extent and the tier's fraction stay: a
    /// change between tiers of one fraction (high and very high) writes the
    /// march's uniform and group alone, and the same tier builds nothing.
    /// Returns whether it built the textures.
    pub(crate) fn sync(
        &self,
        device: &wgpu::Device,
        targets: &mut Option<GlossyFloorTargets>,
        floor_blur: Option<GlossyFloorTermBlur>,
        quality: FluidReflectionQuality,
        scene: Extent,
    ) -> bool {
        let (Some(preset), Some(term_blur)) = (quality.floor_layer_preset(), floor_blur) else {
            *targets = None;
            return false;
        };
        if let Some(current) = targets.as_mut()
            && current.sized_for(scene, preset)
        {
            if current.preset != preset {
                let march = GlossyFloorMarchUniform::of(preset, scene, term_blur);
                current.march_group = self.march_group(device, &current.chain_view, march);
                current.preset = preset;
                current.march = march;
            }
            return false;
        }
        *targets = Some(self.build(device, scene, preset, term_blur));
        true
    }

    /// The march's group 3: the chain, its sampler and `march`, in a uniform
    /// of its own.
    fn march_group(
        &self,
        device: &wgpu::Device,
        chain_view: &wgpu::TextureView,
        march: GlossyFloorMarchUniform,
    ) -> wgpu::BindGroup {
        let march_uniform = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Glossy Floor March Uniform"),
            contents: bytemuck::bytes_of(&march),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        log::info!(
            "[Renderer] Glossy floor march: {} steps at {:.2} px",
            march.step_budget,
            march.stride,
        );
        device.new_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Glossy Floor March Bind Group"),
            layout: &self.march_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(chain_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.march_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: march_uniform.as_entire_binding(),
                },
            ],
        })
    }

    fn build(
        &self,
        device: &wgpu::Device,
        scene: Extent,
        preset: FluidReflectionPreset,
        term_blur: GlossyFloorTermBlur,
    ) -> GlossyFloorTargets {
        let highlight_free = device.new_texture(&wgpu::TextureDescriptor {
            label: Some("Glossy Highlight-Free Colour"),
            size: extent_3d(scene.width, scene.height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: target_usage(),
            view_formats: &[],
        });
        let highlight_free_view =
            highlight_free.create_view(&wgpu::TextureViewDescriptor::default());
        let (width, height) =
            postretro_render_cpu::fluid_surface::reflection_quality::reflection_target_extent(
                (scene.width.max(1), scene.height.max(1)),
                preset.fraction,
            );
        let mip_level_count = chain_mip_count(width, height);
        let chain = device.new_texture(&wgpu::TextureDescriptor {
            label: Some("Glossy Scene Colour Chain"),
            size: extent_3d(width, height),
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: target_usage(),
            view_formats: &[],
        });
        let chain_view = chain.create_view(&wgpu::TextureViewDescriptor::default());
        let level_view = |level: u32| {
            chain.create_view(&wgpu::TextureViewDescriptor {
                label: Some("Glossy Scene Colour Chain Level"),
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let mut levels: Vec<ChainLevel> = Vec::with_capacity(mip_level_count as usize);
        for level in 0..mip_level_count {
            let source = match levels.last() {
                Some(above) => &above.target,
                None => &highlight_free_view,
            };
            let source = device.new_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Glossy Floor Chain Source Bind Group"),
                layout: &self.source_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            levels.push(ChainLevel {
                target: level_view(level),
                source,
            });
        }
        // The floor layer's target and the march's depth share the chain's
        // level-0 extent: one ray per target texel at the tier's fraction.
        let layer = device.new_texture(&wgpu::TextureDescriptor {
            label: Some("Glossy Floor Layer"),
            size: extent_3d(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: GLOSSY_FLOOR_LAYER_FORMAT,
            usage: target_usage(),
            view_formats: &[],
        });
        let layer_depth = device.new_texture(&wgpu::TextureDescriptor {
            label: Some("Glossy Floor Layer Depth"),
            size: extent_3d(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let march = GlossyFloorMarchUniform::of(preset, scene, term_blur);
        let march_group = self.march_group(device, &chain_view, march);
        let targets = GlossyFloorTargets {
            highlight_free,
            highlight_free_view,
            chain,
            chain_view,
            levels,
            layer_view: layer.create_view(&wgpu::TextureViewDescriptor::default()),
            layer,
            layer_depth_view: layer_depth.create_view(&wgpu::TextureViewDescriptor::default()),
            layer_depth,
            march_group,
            preset,
            march,
        };
        let (scene, chain) = (targets.scene_extent(), targets.chain_extent());
        log::info!(
            "[Renderer] Glossy floor layer: highlight-free colour {}x{}, scene-colour chain and \
             floor layer {}x{} in {} levels ({:?})",
            scene.width,
            scene.height,
            chain.width,
            chain.height,
            targets.chain_mip_count(),
            self.format,
        );
        targets
    }
}

/// Every level down to 1×1 along the longer axis: the floor march's blur
/// widens with hit distance, so the widest footprint can span the screen.
pub(crate) fn chain_mip_count(width: u32, height: u32) -> u32 {
    u32::BITS - width.max(height).max(1).leading_zeros()
}

fn extent_3d(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: width.max(1),
        height: height.max(1),
        depth_or_array_layers: 1,
    }
}

/// Forward, the mover and skinned-mesh passes, the downsample and the march
/// write them as attachments; the downsample, the march and the overlay read
/// them. Tests also copy them out, and seed the highlight-free colour.
fn target_usage() -> wgpu::TextureUsages {
    let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
    #[cfg(test)]
    let usage = usage | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST;
    usage
}

impl GlossyFloorTargets {
    /// Whether the textures already suit `scene` at `preset`: the
    /// highlight-free colour at the scene extent and the chain at the
    /// preset's fraction of it. The march sizes nothing.
    fn sized_for(&self, scene: Extent, preset: FluidReflectionPreset) -> bool {
        let size = self.highlight_free.size();
        let chain =
            postretro_render_cpu::fluid_surface::reflection_quality::reflection_target_extent(
                (scene.width.max(1), scene.height.max(1)),
                preset.fraction,
            );
        size.width == scene.width.max(1)
            && size.height == scene.height.max(1)
            && (self.chain.width(), self.chain.height()) == chain
    }

    /// The highlight-free scene colour: forward's third attachment and the
    /// mover and skinned-mesh passes' second, cleared by forward every frame
    /// it is written.
    pub(crate) fn highlight_free_view(&self) -> &wgpu::TextureView {
        &self.highlight_free_view
    }

    /// The scene-colour chain, every level, as the march group binds it.
    #[cfg(test)]
    pub(crate) fn chain_view(&self) -> &wgpu::TextureView {
        &self.chain_view
    }

    /// The floor layer's target: the march's attachment, cleared every frame
    /// it records.
    pub(crate) fn layer_view(&self) -> &wgpu::TextureView {
        &self.layer_view
    }

    /// The march's own depth, cleared every frame it records.
    pub(crate) fn layer_depth_view(&self) -> &wgpu::TextureView {
        &self.layer_depth_view
    }

    /// The march's group 3.
    pub(super) fn march_group(&self) -> &wgpu::BindGroup {
        &self.march_group
    }

    /// The march uniform the group holds (test-only).
    #[cfg(test)]
    pub(crate) fn march(&self) -> GlossyFloorMarchUniform {
        self.march
    }

    /// The chain's level count, level 0 included.
    pub(crate) fn chain_mip_count(&self) -> u32 {
        self.chain.mip_level_count()
    }

    /// The chain's level-0 extent: the floor tier's fraction of the scene's.
    pub(crate) fn chain_extent(&self) -> Extent {
        Extent {
            width: self.chain.width(),
            height: self.chain.height(),
        }
    }

    /// The scene extent the targets were built at.
    pub(crate) fn scene_extent(&self) -> Extent {
        Extent {
            width: self.highlight_free.width(),
            height: self.highlight_free.height(),
        }
    }

    /// Each level's render target and the group its downsample reads, top
    /// level first.
    pub(super) fn levels(&self) -> impl Iterator<Item = (&wgpu::TextureView, &wgpu::BindGroup)> {
        self.levels
            .iter()
            .map(|level| (&level.target, &level.source))
    }

    /// The targets' ledger rows: the highlight-free colour, the chain, the
    /// floor layer's target and the march's depth.
    pub(crate) fn residency_rows(&self) -> [ResidencyAllocation; 4] {
        let row = |name, texture| {
            texture_row(
                name,
                texture,
                &[SectionId::ReflectionProbes as u16],
                ResidencyAllocationState::Data,
            )
        };
        [
            row(GLOSSY_HIGHLIGHT_FREE, &self.highlight_free),
            row(GLOSSY_SCENE_COLOUR_CHAIN, &self.chain),
            row(GLOSSY_FLOOR_LAYER, &self.layer),
            row(GLOSSY_FLOOR_LAYER_DEPTH, &self.layer_depth),
        ]
    }

    /// The highlight-free colour texture, for readback.
    #[cfg(test)]
    pub(crate) fn highlight_free_texture(&self) -> &wgpu::Texture {
        &self.highlight_free
    }

    /// The chain texture, for readback.
    #[cfg(test)]
    pub(crate) fn chain_texture(&self) -> &wgpu::Texture {
        &self.chain
    }

    /// The floor layer's target, for readback.
    #[cfg(test)]
    pub(crate) fn layer_texture(&self) -> &wgpu::Texture {
        &self.layer
    }
}
