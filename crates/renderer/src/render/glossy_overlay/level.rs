// One level's glossy overlay state: the world probe stream, the overlay's
// group over the level's probes, and, while its set is published, the
// static-diffuse and floor-layer targets.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces) · context/lib/resource_management.md §5.4

use postretro_level_format::SectionId;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::render_extent::Extent;

use crate::render::glossy_floor::{GlossyFloorLayout, GlossyFloorTargets, binds_an_up_facing_face};
use crate::render::gpu_create::GpuCreate;
use crate::render::reflection_probe_resources::ReflectionProbeResources;
use crate::render::residency::{ResidencyAllocation, ResidencyAllocationState, texture_row};
use crate::render::{LevelGeometry, LevelReflectionProbes};

/// The memory ledger's row for the static-diffuse target.
pub const GLOSSY_STATIC_DIFFUSE: &str = "glossy_static_diffuse";

/// What a level's glossy state binds from the overlay pass, cloned for a
/// level build on any thread.
#[derive(Clone)]
pub(crate) struct GlossyLevelLayout {
    pub(super) probe_layout: wgpu::BindGroupLayout,
    pub(super) inputs_layout: wgpu::BindGroupLayout,
    pub(super) sampler: wgpu::Sampler,
    pub(super) static_diffuse_format: wgpu::TextureFormat,
    pub(super) floor: GlossyFloorLayout,
}

/// The static-diffuse target at scene extent.
struct StaticDiffuseTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// One level's glossy overlay state, owned by its level set and dropped with
/// it. Exists only for a level whose id 54 binds a world face. Its targets
/// exist only while its set is the published one: the set's build makes
/// none, publish builds them at the kept tier and scene extent, and a set
/// kept resident behind another's publish releases them, so a level change
/// never holds two sets of targets.
pub(crate) struct GlossyLevelState {
    /// One probe index per world vertex, written once at install.
    world_probes: wgpu::Buffer,
    /// The overlay's group over the level's probe records; `None` until the
    /// probes install, or when they do not.
    probe_group: Option<wgpu::BindGroup>,
    /// Forward's D_static target, at scene extent while the set is published
    /// and the reflection tier is above off; `None` otherwise.
    target: Option<StaticDiffuseTarget>,
    /// Whether a bound world face faces up by water's test: only such a
    /// level takes the floor layer.
    floor_faces: bool,
    /// The floor layer's highlight-free colour, scene-colour chain, target
    /// and march depth, while `floor_faces` holds at a tier that runs the
    /// floor layer.
    floor: Option<GlossyFloorTargets>,
    /// The overlay's inputs group, while D_static exists: D_static, the
    /// sampler, and the floor layer's target or its placeholder. Rebuilt
    /// only when either target is created, resized or released.
    inputs_group: Option<wgpu::BindGroup>,
}

impl GlossyLevelLayout {
    /// The level's glossy state, or `None` (no allocation) when its id 54
    /// binds no world face, or the bindings cannot reach the vertices. Builds
    /// the world probe stream alone: no target exists until the set
    /// publishes ([`Self::sync_targets`]).
    pub(crate) fn build(
        &self,
        device: &wgpu::Device,
        geometry: &LevelGeometry<'_>,
    ) -> Option<GlossyLevelState> {
        let LevelReflectionProbes {
            section,
            geometry_face_count,
        } = geometry.reflection_probes?;
        let stream = postretro_level_loader::build_vertex_world_probes(
            geometry.vertices.len(),
            geometry.indices,
            &geometry.bvh.leaves,
            geometry_face_count,
            &section.world_bindings,
            section.probes.len(),
        )?;
        let bytes: Vec<u8> = stream
            .iter()
            .flat_map(|probe| probe.to_le_bytes())
            .collect();
        let world_probes = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("World Probe Index Buffer"),
            contents: &bytes,
            usage: wgpu::BufferUsages::VERTEX,
        });
        Some(GlossyLevelState {
            world_probes,
            probe_group: None,
            target: None,
            floor_faces: binds_an_up_facing_face(geometry.vertices, &stream),
            floor: None,
            inputs_group: None,
        })
    }

    /// Bind the overlay's probe group over the level's installed probes.
    pub(crate) fn bind_probes(
        &self,
        device: &wgpu::Device,
        state: &mut GlossyLevelState,
        probes: Option<&ReflectionProbeResources>,
    ) {
        state.probe_group = probes.map(|probes| {
            device.new_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Glossy Overlay Probe Bind Group"),
                layout: &self.probe_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(probes.colour_view()),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: probes.table_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(probes.distance_view()),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: probes.sums_binding(),
                    },
                ],
            })
        });
    }

    /// Bring the published set's D_static target and floor-layer targets in
    /// line with `quality` and the scene `extent`: at publish, on a tier
    /// change and on a scene resize, so a resize at a moved tier builds each
    /// target once. D_static allocated above off at `extent`, released at
    /// off; the floor targets as [`GlossyFloorLayout::sync`] rules. Targets
    /// already there build nothing; the inputs group rebuilds only over a
    /// target created or released.
    pub(crate) fn sync_targets(
        &self,
        device: &wgpu::Device,
        state: &mut GlossyLevelState,
        quality: FluidReflectionQuality,
        extent: Extent,
    ) {
        let had_floor = state.floor.is_some();
        let floor_built =
            self.floor
                .sync(device, &mut state.floor, state.floor_faces, quality, extent);
        let floor_moved = floor_built || had_floor != state.floor.is_some();
        let created = if quality == FluidReflectionQuality::Off {
            state.target = None;
            false
        } else {
            let current = state.target.as_ref().map(|target| target.texture.size());
            let kept = current
                .is_some_and(|size| size.width == extent.width && size.height == extent.height);
            if !kept {
                state.target = Some(self.target(device, extent));
            }
            !kept
        };
        if created || floor_moved || state.target.is_none() {
            self.rebind_inputs(device, state);
        }
    }

    fn target(&self, device: &wgpu::Device, extent: Extent) -> StaticDiffuseTarget {
        let texture = device.new_texture(&wgpu::TextureDescriptor {
            label: Some("Glossy Static Diffuse"),
            size: wgpu::Extent3d {
                width: extent.width.max(1),
                height: extent.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.static_diffuse_format,
            usage: static_diffuse_usage(),
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        StaticDiffuseTarget { texture, view }
    }

    /// The overlay's inputs group over the current D_static and floor layer
    /// (or its placeholder); none without D_static. A released floor target
    /// leaves no group pointing at it.
    fn rebind_inputs(&self, device: &wgpu::Device, state: &mut GlossyLevelState) {
        state.inputs_group = state.target.as_ref().map(|target| {
            device.new_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Glossy Overlay Inputs Bind Group"),
                layout: &self.inputs_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&target.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(
                            self.floor.overlay_view(state.floor.as_ref()),
                        ),
                    },
                ],
            })
        });
    }
}

/// Forward writes D_static as an attachment; the overlay reads it as a
/// texture. Tests also copy it out.
fn static_diffuse_usage() -> wgpu::TextureUsages {
    let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
    #[cfg(test)]
    let usage = usage | wgpu::TextureUsages::COPY_SRC;
    usage
}

impl GlossyLevelState {
    /// Drop every target and the inputs group over them, creating nothing:
    /// the set stays resident behind another's publish, and builds its
    /// targets again at its own.
    pub(crate) fn release_targets(&mut self) {
        self.target = None;
        self.floor = None;
        self.inputs_group = None;
    }

    /// Whether forward writes D_static and the overlay can draw: the probes
    /// are bound and the target exists.
    pub(crate) fn ready(&self) -> bool {
        self.probe_group.is_some() && self.target.is_some()
    }

    pub(crate) fn world_probes(&self) -> &wgpu::Buffer {
        &self.world_probes
    }

    pub(crate) fn probe_group(&self) -> Option<&wgpu::BindGroup> {
        self.probe_group.as_ref()
    }

    pub(crate) fn inputs_group(&self) -> Option<&wgpu::BindGroup> {
        self.inputs_group.as_ref()
    }

    /// Forward's second attachment, while allocated.
    pub(crate) fn static_diffuse_view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|target| &target.view)
    }

    /// The floor layer's targets, while allocated.
    pub(crate) fn floor(&self) -> Option<&GlossyFloorTargets> {
        self.floor.as_ref()
    }

    /// Whether a bound world face faces up: the level takes the floor layer
    /// at the tiers that run it.
    #[cfg(test)]
    pub(crate) fn has_floor_faces(&self) -> bool {
        self.floor_faces
    }

    /// The ledger rows of every target allocated: D_static, then the floor
    /// layer's highlight-free colour, chain, target and march depth.
    pub(crate) fn residency_rows(&self) -> impl Iterator<Item = ResidencyAllocation> + '_ {
        let static_diffuse = self.target.as_ref().map(|target| {
            texture_row(
                GLOSSY_STATIC_DIFFUSE,
                &target.texture,
                &[SectionId::ReflectionProbes as u16],
                ResidencyAllocationState::Data,
            )
        });
        static_diffuse.into_iter().chain(
            self.floor
                .iter()
                .flat_map(GlossyFloorTargets::residency_rows),
        )
    }

    /// The target texture, for readback (tests).
    #[cfg(test)]
    pub(crate) fn static_diffuse_texture(&self) -> Option<&wgpu::Texture> {
        self.target.as_ref().map(|target| &target.texture)
    }
}
