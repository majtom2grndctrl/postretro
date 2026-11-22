// One installed level's fluid surfaces: the GPU half of a fluid install,
// built in a level set off the frame, and that set's reflection targets.
// See: context/lib/rendering_pipeline.md §7.3a · context/lib/resource_management.md §5.3

use std::num::NonZeroU64;

use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_level_format::reflection_probes::ReflectionProbesSection;
use postretro_render_cpu::fluid_surface::prepared::PreparedFluidSurfaces;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::{
    FLUID_DRIFT_RATE_STRIDE, FLUID_PARAMS_STRIDE, FLUID_PARAMS_WINDOW_BYTES, FluidFaceDriftMap,
    FluidSurfaceDraw, FluidSurfaceSlots, FluidSurfaceVolume, fill_drift_rates,
};

use super::{planar_routing, reflection_pass};
use crate::render::fluid_reflection_probes::{
    FluidProbeResources, create_probe_index_buffer, installable_probe_section,
};
use crate::render::gpu_create::GpuCreate;

/// One installed level's fluid geometry and per-volume parameter buffer.
pub(crate) struct InstalledFluidSurfaces {
    pub(super) vertex_buffer: wgpu::Buffer,
    /// Zeroed at install: a level draws still until drift resolves, and
    /// capture, which resolves none, always draws still.
    pub(super) drift_buffer: wgpu::Buffer,
    /// One map per surface face of every volume, in record order.
    pub(super) face_drift_maps: Vec<Option<FluidFaceDriftMap>>,
    /// Reused per re-resolve.
    pub(super) drift_rates: Vec<[f32; 2]>,
    /// One probe index per vertex; the no-probe value for faces no binding
    /// names, every face of a level without probes included.
    pub(super) probe_index_buffer: wgpu::Buffer,
    pub(super) index_buffer: wgpu::Buffer,
    pub(super) params_buffer: wgpu::Buffer,
    /// The level's probe array, share table and distance array, or their
    /// dummies. Held so they drop with the level; the frame reaches them only
    /// through `params_bind_group`.
    pub(super) _probes: FluidProbeResources,
    pub(super) params_bind_group: wgpu::BindGroup,
    /// The reflection pass's group 3, the parameter window and probe table
    /// over the same buffers; `None` for a level with no probe-bound face,
    /// which never reflects.
    pub(super) reflection_probe_bind_group: Option<wgpu::BindGroup>,
    /// Indexed by draw slot.
    pub(super) volumes: Vec<FluidSurfaceVolume>,
    pub(super) slots: FluidSurfaceSlots,
    /// Every material range's triangles in runs by plane, for per-draw
    /// planar routing.
    pub(super) planar_parts: Vec<planar_routing::PlanarIndexRange>,
    /// Whether the level holds a candidate plane by the selection's own
    /// install rule (`PlanarCandidates::from_volumes`, area floor included):
    /// what holds the planar layer, with the app's verdict over it.
    pub(super) planar_candidate: bool,
    /// The planes the faces were cut along; tests and the set summary read
    /// it.
    #[cfg(any(test, feature = "test-seams"))]
    pub(super) grid: Option<postretro_render_cpu::fluid_surface::FluidGridPlanes>,
    /// Parameter-row uploads so far; tests read it.
    #[cfg(test)]
    pub(super) param_uploads: u32,
}

impl InstalledFluidSurfaces {
    /// Whether any face is bound to a reflection probe: fixed at install,
    /// and with the tier, what keys the reflection target. Only such a level
    /// builds the reflection pass's group 3.
    pub(super) fn has_probe_bound_face(&self) -> bool {
        self.reflection_probe_bind_group.is_some()
    }
}

/// One level set's fluid reflection targets: the screen-space reflection
/// target with the group 5 over it, and the planar layer with its group 5.
/// The set owns them; while it is the active set they are lent to the fluid
/// pass, which draws through them, and publishing another set hands them
/// back. Building a set never touches the pass's.
#[derive(Default)]
pub(crate) struct FluidLevelTargets {
    pub(super) reflection: Option<reflection_pass::LentReflectionTarget>,
    pub(super) planar: planar_routing::LentPlanarLayer,
}

impl FluidLevelTargets {
    /// Whether the set holds a reflection target (tests).
    #[cfg(test)]
    pub(crate) fn has_reflection_target(&self) -> bool {
        self.reflection.is_some()
    }
}

/// What a level's fluid surfaces and own reflection targets bind from the
/// fluid pass: the group-4 layout, the reflection pass's group-3 layout and
/// group 5's layout and uniform, cloned for a level build on any thread.
#[derive(Clone)]
pub(crate) struct FluidLevelLayout {
    pub(super) params_bind_group_layout: wgpu::BindGroupLayout,
    pub(super) reflection_probe_layout: wgpu::BindGroupLayout,
    pub(super) scene_group: crate::render::fluid_scene_inputs::FluidSceneGroupLayout,
}

impl FluidLevelLayout {
    /// The GPU half of a fluid install: upload what prep packed, touching no
    /// pass state. `None` (no surface face) allocates nothing. `volumes` and
    /// `reflection_probes` are the ones `prepared` was built from.
    pub(crate) fn build_surfaces(
        &self,
        device: &wgpu::Device,
        queue: &dyn crate::render::queue_writes::QueueWrites,
        prepared: Option<PreparedFluidSurfaces>,
        volumes: &[EnvironmentVolumeRecord],
        reflection_probes: Option<&ReflectionProbesSection>,
    ) -> Option<InstalledFluidSurfaces> {
        let PreparedFluidSurfaces {
            mesh,
            probe_indices,
            has_probe_bound_face,
            planar_parts,
        } = prepared?;
        let reflection_probes = installable_probe_section(reflection_probes);
        let probe_index_buffer = create_probe_index_buffer(device, &probe_indices);
        // An install-time upload takes the raw queue, as the SH atlases do.
        let probes = FluidProbeResources::new(device, queue, reflection_probes);
        let slots = FluidSurfaceSlots::new(&mesh.volumes);
        let vertex_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Surface Vertex Buffer"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let drift_buffer = device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("Fluid Surface Drift Buffer"),
            size: (mesh.vertices.len() * FLUID_DRIFT_RATE_STRIDE) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Surface Index Buffer"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        // Zeroed at creation; each slot uploads its row on first draw.
        let params_buffer = device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("Fluid Surface Params"),
            size: u64::from(slots.window_count()) * FLUID_PARAMS_WINDOW_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let [probe_array, probe_table, probe_distance] = probes.bind_group_entries();
        let params_bind_group = device.new_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Fluid Surface Params Bind Group (group 4)"),
            layout: &self.params_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &params_buffer,
                        offset: 0,
                        size: NonZeroU64::new(FLUID_PARAMS_WINDOW_BYTES),
                    }),
                },
                probe_array,
                probe_table,
                probe_distance,
            ],
        });
        let reflection_probe_bind_group = has_probe_bound_face.then(|| {
            let [_, probe_table, _] = probes.bind_group_entries();
            reflection_pass::probe_bind_group(
                device,
                &self.reflection_probe_layout,
                &params_buffer,
                probe_table,
            )
        });
        match mesh.grid {
            Some(grid) => log::info!(
                "[Renderer] Fluid surfaces installed: {} volumes, {} triangles, {} vertices on a \
                 {:.3?} m grid",
                mesh.volumes.len(),
                mesh.indices.len() / 3,
                mesh.vertices.len(),
                grid.spacing().to_array(),
            ),
            None => log::info!(
                "[Renderer] Fluid surfaces installed: {} volumes, {} triangles, {} vertices, \
                 no grid",
                mesh.volumes.len(),
                mesh.indices.len() / 3,
                mesh.vertices.len(),
            ),
        }
        Some(InstalledFluidSurfaces {
            vertex_buffer,
            drift_buffer,
            drift_rates: vec![[0.0; 2]; mesh.vertices.len()],
            face_drift_maps: mesh.face_drift_maps,
            probe_index_buffer,
            index_buffer,
            params_buffer,
            _probes: probes,
            params_bind_group,
            reflection_probe_bind_group,
            volumes: mesh.volumes,
            slots,
            planar_parts,
            planar_candidate: planar_routing::has_candidate_plane(volumes, reflection_probes),
            #[cfg(any(test, feature = "test-seams"))]
            grid: mesh.grid,
            #[cfg(test)]
            param_uploads: 0,
        })
    }

    /// A level set's own reflection targets at `quality` over `scene_depth`,
    /// as a fluid install at that tier would key them: the screen-space
    /// target for a level with a probe-bound face at a tier that runs the
    /// screen-space layer, and the planar layer for a level holding a
    /// candidate plane (the install rule, narrowed by the app's verdict when
    /// it gave one) at a planar tier. Returns the planar layer's extent, at
    /// which the caller builds the mirror targets.
    pub(crate) fn build_targets(
        &self,
        device: &wgpu::Device,
        installed: Option<&InstalledFluidSurfaces>,
        quality: FluidReflectionQuality,
        planar_app_candidate: Option<bool>,
        scene_depth: &wgpu::TextureView,
    ) -> (FluidLevelTargets, Option<(u32, u32)>) {
        let size = scene_depth.texture().size();
        let scene = (size.width, size.height);
        let reflection = installed
            .filter(|installed| installed.has_probe_bound_face())
            .and_then(|_| quality.target_extent(scene))
            .map(|extent| {
                reflection_pass::build_lent_reflection_target(
                    device,
                    extent,
                    &self.scene_group,
                    scene_depth,
                )
            });
        let planar_candidate = installed.is_some_and(|installed| installed.planar_candidate)
            && planar_app_candidate.unwrap_or(true);
        let planar_extent = postretro_render_cpu::planar_reflection::planar_preset(quality)
            .filter(|_| planar_candidate)
            .map(|preset| preset.target_extent(scene));
        let planar = planar_extent
            .map(|extent| planar_routing::build_lent_planar_layer(device, extent, scene_depth))
            .unwrap_or_default();
        (FluidLevelTargets { reflection, planar }, planar_extent)
    }
}

impl InstalledFluidSurfaces {
    /// Replace every face's drift: `drifts` holds one world drift per surface
    /// face of every installed volume, in record order. Only the drift buffer
    /// is rewritten, so draws, slots and parameter rows carry over untouched.
    pub(crate) fn set_drift(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        drifts: &[glam::Vec3],
    ) {
        let installed = self;
        if drifts.len() != installed.face_drift_maps.len() {
            log::warn!(
                "[Renderer] Fluid surface drift count {} does not match {} surface faces; \
                 water holds still",
                drifts.len(),
                installed.face_drift_maps.len(),
            );
        }
        fill_drift_rates(
            &installed.face_drift_maps,
            drifts,
            &mut installed.drift_rates,
        );
        // Drift re-resolves at install, reload commit and fluid-table or
        // tuning refreshes. Some of those points require an empty upload
        // batch. Nothing ever batch-writes this buffer, so a direct write,
        // which lands now, is safe at every one.
        queue.direct_write_buffer(
            &installed.drift_buffer,
            0,
            bytemuck::cast_slice(&installed.drift_rates),
        );
    }

    /// Replace this frame's draws, uploading only parameter rows that changed.
    pub(crate) fn set_draws(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        draws: &[FluidSurfaceDraw],
    ) {
        let installed = self;
        installed.slots.set_draws(draws);
        if let Some((range, rows)) = installed.slots.take_dirty() {
            #[cfg(test)]
            {
                installed.param_uploads += 1;
            }
            queue.write_buffer(
                &installed.params_buffer,
                u64::from(range.start) * FLUID_PARAMS_STRIDE as u64,
                bytemuck::cast_slice(rows),
            );
        }
    }
}

/// What prep decided, read back from the install: vertex count, index and
/// probe-index buffer bytes, planar runs, whether a probe-bound face built
/// the probe bind group, and the grid planes. Test-only, for the set summary.
#[cfg(any(test, feature = "test-seams"))]
impl InstalledFluidSurfaces {
    pub(crate) fn prep_summary(&self) -> String {
        format!(
            "{:?}",
            (
                self.drift_rates.len(),
                self.index_buffer.size(),
                self.probe_index_buffer.size(),
                &self.planar_parts,
                self.reflection_probe_bind_group.is_some(),
                self.grid,
            )
        )
    }
}
