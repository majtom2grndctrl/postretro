// Planar mirror light admission: its light records and reach, admission tags, held seats, counts.
// See: context/lib/rendering_pipeline.md §7.3a (Pool rank), §4

use super::renderer_light_slots::PromotionCandidateLookup;
use super::renderer_types::FullRenderer;
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
    /// The copy's spot and cube slots in level-light space, reused so a
    /// frame that builds the copy allocates nothing for them.
    pub(super) spot_level_slots: Vec<u32>,
    pub(super) cube_level_slots: Vec<u32>,
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
            spot_level_slots: Vec::new(),
            cube_level_slots: Vec::new(),
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
        // The frame clock restarts with the level, so a report time kept
        // from the old one would silence the line until the clock caught up.
        self.reported = None;
        self.reported_at = f64::NEG_INFINITY;
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

    /// Spend this frame's reach unread, as a frame with no candidate does.
    pub(super) fn clear_reach(&mut self) {
        self.reach.clear();
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

/// Each shadow candidate's admission rank (`rendering_pipeline.md` §7.3a,
/// Pool rank), candidate-indexed. Empty slices rank every light main-view,
/// as without a mirror.
#[derive(Clone, Copy, Default)]
pub(super) struct AdmissionTags<'a> {
    /// This frame has a mirror reach: some cell only the planar mirror
    /// reaches. Without one, no seat the mirror held carries over.
    pub(super) mirror_present: bool,
    /// This frame: reached by the planar mirror alone. Any main-view reach,
    /// shadow casting onto a visible receiver included, ranks a light
    /// main-view (§7.3a, Pool rank).
    pub(super) mirror_only: &'a [bool],
    /// Last frame: held its slot as a mirror-only admission.
    pub(super) held_mirror_only: &'a [bool],
}

impl AdmissionTags<'_> {
    pub(super) fn mirror_only(&self, candidate_index: usize) -> bool {
        self.mirror_only
            .get(candidate_index)
            .copied()
            .unwrap_or(false)
    }

    fn held_mirror_only(&self, candidate_index: usize) -> bool {
        self.held_mirror_only
            .get(candidate_index)
            .copied()
            .unwrap_or(false)
    }

    /// The rank an incumbent holds its prior slot at this frame, or `None`
    /// when it holds no claim. A slot held as the mirror's gives no claim
    /// against a main-view light: a light that held it as the mirror's and
    /// is main-view now competes as a fresh main-view candidate, as it would
    /// without the mirror. With no mirror reach this frame (the mirror is
    /// gone, as on a tier drop), a seat held as the mirror's gives no claim
    /// at all, so the light loses it and its promotion resets. A light that
    /// turned mirror-only this frame keeps its claim only against other
    /// mirror-only lights. A main-view incumbent no reach keeps (a promoted
    /// light's sticky window) stays main-view, as without the mirror
    /// (§7.3a, Pool rank).
    pub(super) fn incumbent_mirror_only(
        &self,
        candidate_index: usize,
        eligible: bool,
    ) -> Option<bool> {
        let mirror_only = self.mirror_only(candidate_index);
        let held = self.held_mirror_only(candidate_index);
        if held && (!self.mirror_present || (eligible && !mirror_only)) {
            return None;
        }
        Some(mirror_only || held)
    }
}

/// After this frame's assignment: which candidates hold their slot as a
/// mirror-only admission. A light the main view reaches holds its slot as
/// the main view's; a mirror-only light as the mirror's; a light neither
/// reaches (a promoted light's sticky window) at the rank it was seated by.
/// `held` enters as last frame's ranks and leaves as this frame's: each
/// entry reads only its own previous value, so the update runs in place.
pub(super) fn update_held_as_mirror_only(
    spot_assignment: &[u32],
    cube_assignment: &[u32],
    eligible_lights: &[bool],
    mirror_only: &[bool],
    held: &mut Vec<bool>,
) {
    let count = spot_assignment.len().max(cube_assignment.len());
    held.resize(count, false);
    for (index, held) in held.iter_mut().enumerate() {
        let assigned = [spot_assignment, cube_assignment].iter().any(|assignment| {
            assignment
                .get(index)
                .is_some_and(|&slot| slot != postretro_lighting::NO_SHADOW_SLOT)
        });
        let tagged = mirror_only.get(index).copied().unwrap_or(false);
        let main_view = eligible_lights.get(index).copied().unwrap_or(false) && !tagged;
        *held = assigned && !main_view && (tagged || *held);
    }
}

/// The main view's light records never carry a mirror-only light's shadow
/// slot. The dynamic prefix takes the main view's level slots directly; this
/// clears the slot on the animated-tail record and the appended
/// selected-static record of each promoted light the mirror alone holds a
/// slot for, so main-view movers and meshes shade it unshadowed.
pub(super) fn clear_mirror_only_promoted_slots(
    bytes: &mut [u8],
    full: &FullRenderer,
    lookup: &PromotionCandidateLookup,
    held: &[bool],
) {
    let size = postretro_lighting::GPU_LIGHT_SIZE;
    let mirror_only = |candidate: Option<&Option<usize>>| {
        candidate
            .copied()
            .flatten()
            .is_some_and(|candidate| held.get(candidate).copied().unwrap_or(false))
    };
    let clear = |bytes: &mut [u8], record: usize| {
        if let Some(record) = bytes.get_mut(record * size..(record + 1) * size) {
            postretro_lighting::patch_shadow_slots(record, &[postretro_lighting::NO_SHADOW_SLOT]);
            postretro_lighting::patch_cube_slots(record, &[postretro_lighting::NO_SHADOW_SLOT]);
        }
    };
    let dynamic = full.light_count as usize;
    for animated_index in 0..full.animated_baked_light_count {
        if mirror_only(lookup.animated_baked.get(animated_index)) {
            clear(bytes, dynamic + animated_index);
        }
    }
    let suffix = dynamic + full.animated_baked_light_count;
    for (appended, selection_index) in full
        .promoted_baked_records
        .iter()
        .filter_map(|record| record.source.selected_static_index())
        .enumerate()
    {
        if mirror_only(lookup.selected_static.get(selection_index)) {
            clear(bytes, suffix + appended);
        }
    }
}

/// One pool's occupancy by admission rank, from its candidate-indexed
/// assignment.
pub(super) fn pool_occupancy(
    assignment: &[u32],
    capacity: usize,
    full: &FullRenderer,
    held: &[bool],
) -> ShadowPoolOccupancy {
    let mut occupancy = ShadowPoolOccupancy {
        capacity: capacity as u32,
        ..ShadowPoolOccupancy::default()
    };
    for (candidate_index, &slot) in assignment.iter().enumerate() {
        if slot == postretro_lighting::NO_SHADOW_SLOT {
            continue;
        }
        let promoted = shadow_candidate_is_promoted_baked(
            &full.shadow_candidate_selection_indices,
            &full.shadow_candidate_animated_baked_indices,
            candidate_index,
        );
        if held.get(candidate_index).copied().unwrap_or(false) {
            occupancy.mirror_only += 1;
            occupancy.promoted_mirror_only += u32::from(promoted);
        } else {
            occupancy.main += 1;
            occupancy.promoted_main += u32::from(promoted);
        }
    }
    occupancy
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
