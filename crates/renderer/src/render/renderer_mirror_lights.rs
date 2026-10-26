// Mirror-only light admission: the planar mirror's own light records, its light reach, and the frame's admission counts.
// See: context/lib/rendering_pipeline.md §7.3a (Pool rank), §4

use super::*;
use postretro_render_cpu::mesh_instances::MeshCapOccupancy;

/// One shadow pool's occupancy this frame, split by admission rank: slots
/// held by main-view lights and by lights only the planar mirror reaches,
/// with the promoted-baked part of each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShadowPoolOccupancy {
    pub capacity: u32,
    pub main: u32,
    pub mirror_only: u32,
    pub promoted_main: u32,
    pub promoted_mirror_only: u32,
}

/// The frame's fixed-capacity admissions split by rank (`rendering_pipeline.md`
/// §7.3a, Pool rank): the mesh-instance cap and both shadow pools. Measure
/// it at one pose with and without the mirror to see what the mirror costs
/// the pools.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdmissionOccupancy {
    pub mesh: MeshCapOccupancy,
    pub spot: ShadowPoolOccupancy,
    pub cube: ShadowPoolOccupancy,
}

/// The mirror view's light records. Main-view passes read the frame's light
/// buffer, where a light only the mirror reaches never carries a shadow slot,
/// so they shade exactly as if it held none. Mirror passes read a copy that
/// carries those slots, uploaded only on frames where some mirror-only light
/// holds one; on every other frame the two match and the mirror binds the
/// main groups. Both bind groups over the copy build at full init and level
/// install, never per frame.
pub(super) struct MirrorLighting {
    lights_buffer: wgpu::Buffer,
    lighting_bind_group: wgpu::BindGroup,
    /// What the copy holds, so an unchanged frame writes nothing.
    uploaded: Vec<u8>,
    /// The copy's bytes, built in place each frame it is read.
    pub(super) scratch: Vec<u8>,
    /// Whether this frame's mirror passes read the copy.
    active: bool,
    /// AABBs of the cells only the mirror reaches this frame, set by the
    /// application before the frame and consumed by slot assignment, so a
    /// frame that sets none ranks no light as the mirror's.
    reach: Vec<(Vec3, Vec3)>,
    /// Per shadow candidate, this frame: reached by the mirror alone.
    pub(super) tags: Vec<bool>,
    /// Per shadow candidate: its slot is held as a mirror-only admission.
    /// Written after ranking, read as the next frame's incumbency rank.
    pub(super) held: Vec<bool>,
    occupancy: AdmissionOccupancy,
    reported: Option<AdmissionOccupancy>,
    reported_at: f64,
}

/// The forward lighting group's buffers other than the light records, which
/// the mirror's group shares with the main view's.
pub(super) struct SharedLightingBuffers<'a> {
    pub influence: &'a wgpu::Buffer,
    pub spec_lights: &'a wgpu::Buffer,
    pub chunk_grid_info: &'a wgpu::Buffer,
    pub chunk_grid_offsets: &'a wgpu::Buffer,
    pub chunk_grid_indices: &'a wgpu::Buffer,
}

impl MirrorLighting {
    /// Build the copy at the main light buffer's size and the mirror's
    /// forward group 2 over it.
    pub(super) fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        main_lights: &wgpu::Buffer,
        shared: &SharedLightingBuffers<'_>,
    ) -> Self {
        let (lights_buffer, lighting_bind_group) = Self::build(device, layout, main_lights, shared);
        Self {
            lights_buffer,
            lighting_bind_group,
            uploaded: Vec::new(),
            scratch: Vec::new(),
            active: false,
            reach: Vec::new(),
            tags: Vec::new(),
            held: Vec::new(),
            occupancy: AdmissionOccupancy::default(),
            reported: None,
            reported_at: f64::NEG_INFINITY,
        }
    }

    /// Follow a level install's reallocated light buffers. Every per-light
    /// tag resets: the candidate list is the new level's.
    pub(super) fn rebuild(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        main_lights: &wgpu::Buffer,
        shared: &SharedLightingBuffers<'_>,
    ) {
        let (lights_buffer, lighting_bind_group) = Self::build(device, layout, main_lights, shared);
        self.lights_buffer = lights_buffer;
        self.lighting_bind_group = lighting_bind_group;
        self.uploaded.clear();
        self.active = false;
        self.reach.clear();
        self.tags.clear();
        self.held.clear();
        self.occupancy = AdmissionOccupancy::default();
    }

    fn build(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        main_lights: &wgpu::Buffer,
        shared: &SharedLightingBuffers<'_>,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let lights_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Planar Mirror Lights Storage Buffer"),
            size: main_lights.size(),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lighting_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Planar Mirror Lighting Bind Group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: lights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: shared.influence.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: shared.spec_lights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: shared.chunk_grid_info.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: shared.chunk_grid_offsets.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: shared.chunk_grid_indices.as_entire_binding(),
                },
            ],
        });
        (lights_buffer, lighting_bind_group)
    }

    /// The mirror's light records, which the mover and skinned-mesh mirror
    /// groups bind.
    pub(super) fn lights_buffer(&self) -> &wgpu::Buffer {
        &self.lights_buffer
    }

    /// The forward group 2 the mirror's world binds this frame: the copy's
    /// while a mirror-only light holds a slot, else the main view's.
    pub(super) fn lighting_bind_group<'a>(
        &'a self,
        main: &'a wgpu::BindGroup,
    ) -> &'a wgpu::BindGroup {
        if self.active {
            &self.lighting_bind_group
        } else {
            main
        }
    }

    /// Whether this frame's mirror passes read the copy.
    #[cfg(test)]
    pub(super) fn active(&self) -> bool {
        self.active
    }

    /// Replace this frame's mirror light reach.
    pub(super) fn set_reach(&mut self, cells: &[(Vec3, Vec3)]) {
        self.reach.clear();
        self.reach.extend_from_slice(cells);
    }

    /// Take this frame's mirror light reach for slot assignment; hand it back
    /// through [`Self::return_reach`] so its storage is reused.
    pub(super) fn take_reach(&mut self) -> Vec<(Vec3, Vec3)> {
        std::mem::take(&mut self.reach)
    }

    /// Return the consumed reach, emptied: the next frame ranks no light as
    /// the mirror's unless the application sets a reach again.
    pub(super) fn return_reach(&mut self, mut reach: Vec<(Vec3, Vec3)>) {
        reach.clear();
        self.reach = reach;
    }

    /// Whether candidate `index` holds its slot as a mirror-only admission.
    pub(super) fn is_mirror_only(&self, index: usize) -> bool {
        self.held.get(index).copied().unwrap_or(false)
    }

    /// End the frame's light records: with a mirror-only slot held, upload
    /// [`Self::scratch`] where it changed and have the mirror read it;
    /// otherwise the mirror reads the main view's records.
    pub(super) fn finish_light_records(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        active: bool,
    ) {
        self.active = active;
        if !active || self.scratch == self.uploaded {
            return;
        }
        queue.write_buffer(&self.lights_buffer, 0, &self.scratch);
        self.uploaded.clone_from(&self.scratch);
    }

    /// Drop every per-frame admission: no candidate, no mirror records.
    pub(super) fn clear_frame(&mut self) {
        self.active = false;
        self.tags.clear();
        self.held.clear();
        self.occupancy.spot = ShadowPoolOccupancy::default();
        self.occupancy.cube = ShadowPoolOccupancy::default();
    }

    pub(super) fn set_mesh_occupancy(&mut self, mesh: MeshCapOccupancy) {
        self.occupancy.mesh = mesh;
    }

    pub(super) fn set_pool_occupancy(
        &mut self,
        spot: ShadowPoolOccupancy,
        cube: ShadowPoolOccupancy,
    ) {
        self.occupancy.spot = spot;
        self.occupancy.cube = cube;
    }

    pub(super) fn occupancy(&self) -> AdmissionOccupancy {
        self.occupancy
    }

    /// One debug line when the occupancy changed, at most once a second.
    pub(super) fn report(&mut self, now_seconds: f64) {
        if self.reported == Some(self.occupancy) || now_seconds - self.reported_at < 1.0 {
            return;
        }
        self.reported = Some(self.occupancy);
        self.reported_at = now_seconds;
        log::debug!("[Admission] {}", describe(&self.occupancy));
    }
}

/// The occupancy as one log segment, shared by the admission line and the
/// shadow debug trace.
pub(super) fn describe(occupancy: &AdmissionOccupancy) -> String {
    let pool = |pool: &ShadowPoolOccupancy| {
        format!(
            "{}+{}/{} (promoted {}+{})",
            pool.main,
            pool.mirror_only,
            pool.capacity,
            pool.promoted_main,
            pool.promoted_mirror_only
        )
    };
    let mesh = &occupancy.mesh;
    format!(
        "mesh main={} mirror_only={} dropped_main={} dropped_mirror_only={} | spot main+mirror={} | cube main+mirror={}",
        mesh.main,
        mesh.mirror_only,
        mesh.dropped_main,
        mesh.dropped_mirror_only,
        pool(&occupancy.spot),
        pool(&occupancy.cube),
    )
}

impl Renderer {
    /// This frame's light reach for the planar mirror: the bounds of the
    /// non-solid cells only the mirror reaches. Lights whose influence meets
    /// them, and no main-view cell, rank as mirror-only and light only the
    /// reflection. Set each frame before rendering; a frame that sets none
    /// has no mirror-only light.
    pub fn set_planar_mirror_light_reach(&mut self, cells: &[(Vec3, Vec3)]) {
        self.full_mut().mirror_lighting.set_reach(cells);
    }

    /// The last frame's mesh-instance cap and shadow-pool occupancy, split by
    /// admission rank. Also logged at debug level (`[Admission]`) when it
    /// changes.
    pub fn admission_occupancy(&self) -> AdmissionOccupancy {
        self.full().mirror_lighting.occupancy()
    }
}
