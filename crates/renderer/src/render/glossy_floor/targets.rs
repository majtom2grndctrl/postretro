// One level's glossy floor-layer targets: the highlight-free scene colour at
// scene extent and the scene-colour mip chain at the floor tier's fraction of
// it, with the per-level groups the chain's downsample reads.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors) · context/lib/resource_management.md §5.4

use postretro_level_format::SectionId;
use postretro_render_cpu::fluid_surface::reflection_hemisphere::REFLECTION_UP_FACING_MIN_COS;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::render_extent::Extent;
use postretro_render_data::geometry::WorldVertex;

use crate::render::gpu_create::GpuCreate;
use crate::render::residency::{ResidencyAllocation, ResidencyAllocationState, texture_row};

/// The memory ledger's row for the highlight-free scene colour.
pub const GLOSSY_HIGHLIGHT_FREE: &str = "glossy_highlight_free";
/// The memory ledger's row for the scene-colour mip chain, every level.
pub const GLOSSY_SCENE_COLOUR_CHAIN: &str = "glossy_scene_colour_chain";

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
}

/// One chain level: the view the downsample renders into, and the group it
/// reads (the highlight-free colour for level 0, the level above for the
/// rest).
struct ChainLevel {
    target: wgpu::TextureView,
    source: wgpu::BindGroup,
}

/// One level's floor-layer targets, owned by its glossy state and dropped
/// with it. Exist only while the level has an up-facing glossy face at a
/// tier that runs the floor layer.
pub(crate) struct GlossyFloorTargets {
    highlight_free: wgpu::Texture,
    highlight_free_view: wgpu::TextureView,
    chain: wgpu::Texture,
    /// Every level, for the floor march's hit to sample by level.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the floor march (Task 11b) samples the chain by level"
        )
    )]
    chain_view: wgpu::TextureView,
    levels: Vec<ChainLevel>,
    /// The tier's fraction the chain's level 0 was sized by.
    fraction: f32,
}

impl GlossyFloorLayout {
    pub(super) fn new(
        source_layout: wgpu::BindGroupLayout,
        sampler: wgpu::Sampler,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            source_layout,
            sampler,
            format,
        }
    }

    /// Bring `targets` in line with `quality` at `scene`: none at a tier that
    /// runs no floor layer or on a level without an up-facing glossy face
    /// (`floor_faces`); otherwise at the scene extent and the tier's
    /// fraction. Targets already there build nothing. Returns whether it
    /// built them.
    pub(crate) fn sync(
        &self,
        device: &wgpu::Device,
        targets: &mut Option<GlossyFloorTargets>,
        floor_faces: bool,
        quality: FluidReflectionQuality,
        scene: Extent,
    ) -> bool {
        let preset = quality.floor_layer_preset().filter(|_| floor_faces);
        let Some(preset) = preset else {
            *targets = None;
            return false;
        };
        if targets
            .as_ref()
            .is_some_and(|current| current.matches(scene, preset.fraction))
        {
            return false;
        }
        *targets = Some(self.build(device, scene, preset.fraction));
        true
    }

    /// Recreate allocated targets at `scene` (the scene targets moved),
    /// keeping their fraction; absent ones stay absent.
    pub(crate) fn resize(
        &self,
        device: &wgpu::Device,
        targets: &mut Option<GlossyFloorTargets>,
        scene: Extent,
    ) {
        if let Some(fraction) = targets.as_ref().map(|current| current.fraction) {
            *targets = Some(self.build(device, scene, fraction));
        }
    }

    fn build(&self, device: &wgpu::Device, scene: Extent, fraction: f32) -> GlossyFloorTargets {
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
                fraction,
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
        let targets = GlossyFloorTargets {
            highlight_free,
            highlight_free_view,
            chain,
            chain_view,
            levels,
            fraction,
        };
        let (scene, chain) = (targets.scene_extent(), targets.chain_extent());
        log::info!(
            "[Renderer] Glossy floor layer: highlight-free colour {}x{}, scene-colour chain {}x{} \
             in {} levels ({:?})",
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

/// Forward, the mover and skinned-mesh passes and the downsample write them
/// as attachments; the downsample and the floor march read them. Tests also
/// copy them out.
fn target_usage() -> wgpu::TextureUsages {
    let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
    #[cfg(test)]
    let usage = usage | wgpu::TextureUsages::COPY_SRC;
    usage
}

impl GlossyFloorTargets {
    fn matches(&self, scene: Extent, fraction: f32) -> bool {
        let size = self.highlight_free.size();
        size.width == scene.width.max(1)
            && size.height == scene.height.max(1)
            && self.fraction == fraction
    }

    /// The highlight-free scene colour: forward's third attachment and the
    /// mover and skinned-mesh passes' second, cleared by forward every frame
    /// it is written.
    pub(crate) fn highlight_free_view(&self) -> &wgpu::TextureView {
        &self.highlight_free_view
    }

    /// The scene-colour chain, every level, for the floor march's hit.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the floor march (Task 11b) samples the chain by level"
        )
    )]
    pub(crate) fn chain_view(&self) -> &wgpu::TextureView {
        &self.chain_view
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

    /// The targets' ledger rows: the highlight-free colour, then the chain.
    pub(crate) fn residency_rows(&self) -> [ResidencyAllocation; 2] {
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
}
