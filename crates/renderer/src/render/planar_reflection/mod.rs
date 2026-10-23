// One extra view for the planar-water spike. GPU state never aliases the main camera.
mod api;
mod record;
mod targets;
#[cfg(test)]
mod tests;

use super::*;
use postretro_render_cpu::planar_reflection::{
    MirrorCamera, PlanarCandidate, PlanarReflectionSettings,
};
use targets::PlanarTargets;

pub(super) struct PlanarReflectionPass {
    pub settings: PlanarReflectionSettings,
    pub camera: Option<MirrorCamera>,
    pub visible: VisibleCells,
    pub uniform_buffer: wgpu::Buffer,
    pub uniform_bind_group: wgpu::BindGroup,
    inputs_buffer: wgpu::Buffer,
    mask_layout: wgpu::BindGroupLayout,
    resolve_layout: wgpu::BindGroupLayout,
    mask_pipeline: wgpu::RenderPipeline,
    resolve_pipeline: wgpu::RenderPipeline,
    targets: Option<PlanarTargets>,
    vertices: Option<wgpu::Buffer>,
    vertex_count: u32,
    face_vertices: Vec<[f32; 3]>,
    mask_plane: Option<PlanarCandidate>,
    cull: Option<MirrorCull>,
    compose_union: VisibleCells,
}

struct MirrorCull {
    candidate: crate::candidate_cull::CandidateCullPipeline,
    indirect: wgpu::Buffer,
    status: wgpu::Buffer,
    ranges: crate::VisibleSpanRanges,
    leaf_count: u32,
    all_cells: Vec<u32>,
}

impl PlanarReflectionPass {
    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout) -> Self {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Planar Camera Uniform"),
            size: UNIFORM_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Planar Camera Bind Group"),
            layout: camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let inputs_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Planar Mask and Resolve Uniform"),
            size: 256,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (mask_layout, mask_pipeline) = targets::mask_pipeline(device);
        let (resolve_layout, resolve_pipeline) = targets::resolve_pipeline(device);
        #[cfg(feature = "dev-tools")]
        let on = std::env::var("POSTRETRO_PLANAR_REFLECTION").is_ok_and(|v| v == "1");
        #[cfg(not(feature = "dev-tools"))]
        let on = false;
        let settings = PlanarReflectionSettings {
            on,
            ..Default::default()
        };
        Self {
            settings,
            camera: None,
            visible: VisibleCells::Culled(Vec::new()),
            uniform_buffer,
            uniform_bind_group,
            inputs_buffer,
            mask_layout,
            resolve_layout,
            mask_pipeline,
            resolve_pipeline,
            targets: None,
            vertices: None,
            vertex_count: 0,
            face_vertices: Vec::new(),
            mask_plane: None,
            cull: None,
            compose_union: VisibleCells::Culled(Vec::new()),
        }
    }

    pub fn upload_camera(&self, queue: &uploads::UploadQueue, main: &[u8; UNIFORM_SIZE]) {
        let Some(camera) = self.camera.filter(|_| self.settings.on) else {
            return;
        };
        let mut data = *main;
        data[..64].copy_from_slice(bytemuck::cast_slice(&camera.raster_vp.to_cols_array()));
        data[64..76].copy_from_slice(bytemuck::cast_slice(&camera.eye.to_array()));
        // The mirror reuses shadow maps, never the deprecated main-view SDF upsample.
        data[96..100].copy_from_slice(&0u32.to_le_bytes());
        queue.write_buffer(&self.uniform_buffer, 0, &data);
    }

    #[cfg(feature = "dev-tools")]
    pub fn post_submit(&mut self, device: &wgpu::Device) {
        if let Some(cull) = &mut self.cull {
            cull.candidate.post_submit(device);
        }
    }

    pub fn invalidate_targets(&mut self) {
        self.targets = None;
    }
    pub fn clear_level(&mut self) {
        self.camera = None;
        self.vertices = None;
        self.vertex_count = 0;
        self.face_vertices.clear();
        self.mask_plane = None;
        self.cull = None;
        self.targets = None;
        self.visible = VisibleCells::Culled(Vec::new());
    }

    pub fn compose_visible_cells<'a>(&'a mut self, main: &'a VisibleCells) -> &'a VisibleCells {
        if !self.settings.on || self.camera.is_none() {
            return main;
        }
        let (VisibleCells::Culled(main), VisibleCells::Culled(mirror)) = (main, &self.visible)
        else {
            self.compose_union = VisibleCells::DrawAll;
            return &self.compose_union;
        };
        if !matches!(self.compose_union, VisibleCells::Culled(_)) {
            self.compose_union = VisibleCells::Culled(Vec::new());
        }
        let VisibleCells::Culled(union) = &mut self.compose_union else {
            unreachable!()
        };
        union.clear();
        union.extend_from_slice(main);
        union.extend_from_slice(mirror);
        union.sort_unstable();
        union.dedup();
        &self.compose_union
    }
}
