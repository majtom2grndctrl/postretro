use super::*;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;

impl Renderer {
    pub fn planar_reflection_settings(&self) -> PlanarReflectionSettings {
        self.full().planar_reflection.settings
    }

    /// Mirror-walk step budget for `determine_mirror_visible_cells`.
    pub fn planar_mirror_walk_steps(&self) -> u32 {
        self.full().planar_reflection.walk_steps
    }

    pub fn set_planar_reflection_settings(&mut self, settings: PlanarReflectionSettings) {
        let settings = settings.sanitized();
        let full = self.full_mut();
        if full.planar_reflection.settings == settings {
            return;
        }
        full.planar_reflection.settings = settings;
        if !settings.on {
            full.planar_reflection.clear_level();
        }
    }

    /// Owns the selected face geometry and visibility. Uploads change only when the plane changes.
    pub fn set_planar_reflection_view(
        &mut self,
        camera: Option<MirrorCamera>,
        plane: Option<&PlanarCandidate>,
        visible: &VisibleCells,
    ) {
        let Self { device, full, .. } = self;
        let full = full.as_mut().expect("planar view requires full init");
        full.fluid_surface.set_planar_selected_height(
            camera
                .filter(|_| full.planar_reflection.settings.on)
                .map(|c| c.h),
        );
        let pass = &mut full.planar_reflection;
        pass.camera = camera.filter(|_| pass.settings.on);
        match (&mut pass.visible, visible) {
            (VisibleCells::Culled(out), VisibleCells::Culled(cells)) => out.clone_from(cells),
            (out, VisibleCells::Culled(cells)) => *out = VisibleCells::Culled(cells.clone()),
            (out, VisibleCells::DrawAll) => *out = VisibleCells::DrawAll,
        }
        let selected = plane.filter(|_| pass.camera.is_some());
        // A fluid reload can replace faces without changing plane bounds or count.
        // Retain the geometry too; comparisons allocate nothing on stable frames.
        if selected != pass.mask_plane.as_ref() {
            pass.face_vertices.clear();
            if let Some(plane) = selected {
                fluid_surface::planar_mask::face_triangles(plane, &mut pass.face_vertices);
            }
            pass.vertex_count = pass.face_vertices.len() as u32;
            pass.vertices = (!pass.face_vertices.is_empty()).then(|| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Planar Selected Water Faces"),
                    contents: bytemuck::cast_slice(&pass.face_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
            });
            pass.mask_plane = selected.cloned();
        }
    }

    pub(in crate::render) fn prepare_planar_reflection_targets(&mut self) {
        let quality = self.reflection_quality;
        let Self { device, full, .. } = self;
        let full = full.as_mut().expect("planar targets require full init");
        let pass = &mut full.planar_reflection;
        let on = pass.settings.on;
        let scene_size = full.depth_view.texture().size();
        let extent = pass
            .settings
            .target_extent((scene_size.width, scene_size.height));
        if on
            && quality != FluidReflectionQuality::Off
            && full.fluid_surface.has_planar_probe_bound_faces()
        {
            if pass.targets.as_ref().is_none_or(|t| t.extent != extent) {
                pass.targets = Some(PlanarTargets::new(device, pass, &full.depth_view, extent));
            }
        } else {
            pass.targets = None;
        }
        full.fluid_surface.set_planar_override(
            device,
            on,
            pass.targets.as_ref().map(|t| (&t.resolved, t.extent)),
        );
    }
}
