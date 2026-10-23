use super::*;

impl FluidSurfacePass {
    pub(in crate::render) fn set_planar_override(
        &mut self,
        device: &wgpu::Device,
        on: bool,
        target: Option<(&wgpu::TextureView, (u32, u32))>,
    ) {
        let next = target.map(|(v, e)| (v.clone(), e));
        self.refresh_planar_zero_binding(device, on);
        if self.reflection.planar_on == on && self.reflection.planar_override == next {
            return;
        }
        self.reflection.planar_on = on;
        self.reflection.planar_override = next;
        let wanted = self.wanted_reflection_target();
        self.set_reflection_target(device, wanted);
        self.rebind_reflection_target(device);
    }

    pub(in crate::render) fn has_planar_probe_bound_faces(&self) -> bool {
        self.installed
            .as_ref()
            .is_some_and(InstalledFluidSurfaces::has_probe_bound_face)
    }

    pub(in crate::render) fn has_planar_reflecting_draws(
        &self,
        light_term_mask: LightTermMask,
    ) -> bool {
        self.reflection.planar_on
            && self.reflection.planar_override.is_some()
            && self.reflection.quality != FluidReflectionQuality::Off
            && self.installed.as_ref().is_some_and(|installed| {
                installed
                    .slots
                    .frame_draws(light_term_mask.bits(), self.reflection.quality)
                    .any(|(_, variant)| variant.reflections())
            })
    }
}

#[derive(Default)]
pub(super) struct PlanarFluidBindings {
    pub selected_height: Option<f32>,
    zero_group: Option<wgpu::BindGroup>,
    scene_depth: Option<wgpu::TextureView>,
}

pub(super) struct PlanarIndexRange {
    start: u32,
    count: u32,
    height: Option<f32>,
}

pub(super) fn split_planar_ranges(
    mesh: &postretro_render_cpu::fluid_surface::FluidSurfaceMesh,
) -> HashMap<u32, Vec<PlanarIndexRange>> {
    let mut output = HashMap::new();
    for volume in &mesh.volumes {
        for material in &volume.material_ranges {
            let mut ranges: Vec<PlanarIndexRange> = Vec::new();
            let end = material.index_start + material.index_count;
            for start in (material.index_start..end).step_by(3) {
                let vertices = [
                    mesh.indices[start as usize],
                    mesh.indices[start as usize + 1],
                    mesh.indices[start as usize + 2],
                ]
                .map(|i| mesh.vertices[i as usize]);
                let h = vertices[0].position[1];
                let height = vertices
                    .iter()
                    .all(|v| v.normal[1] > 0.9999 && (v.position[1] - h).abs() <= 0.01)
                    .then_some(h);
                if let Some(last) = ranges.last_mut().filter(|range| range.height == height) {
                    last.count += 3;
                } else {
                    ranges.push(PlanarIndexRange {
                        start,
                        count: 3,
                        height,
                    });
                }
            }
            output.insert(material.index_start, ranges);
        }
    }
    output
}

impl FluidSurfacePass {
    pub(in crate::render) fn set_planar_selected_height(&mut self, height: Option<f32>) {
        self.planar_bindings.selected_height = height;
    }

    fn refresh_planar_zero_binding(&mut self, device: &wgpu::Device, on: bool) {
        if !on {
            self.planar_bindings = Default::default();
            return;
        }
        let depth = self.scene_inputs.bound_scene_depth();
        if self.planar_bindings.zero_group.is_none()
            || self.planar_bindings.scene_depth.as_ref() != Some(depth)
        {
            self.planar_bindings.zero_group = Some(
                self.scene_inputs
                    .with_reflection_target(device, self.reflection.placeholder_view()),
            );
            self.planar_bindings.scene_depth = Some(depth.clone());
        }
    }

    pub(super) fn record_planar_material_range(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        range: &postretro_render_cpu::fluid_surface::FluidMaterialRange,
        element: u32,
    ) -> bool {
        if !self.reflection.planar_on {
            return false;
        }
        let Some(zero) = &self.planar_bindings.zero_group else {
            return false;
        };
        let Some(installed) = &self.installed else {
            return false;
        };
        let Some(ranges) = installed.planar_ranges.get(&range.index_start) else {
            return false;
        };
        for part in ranges {
            let selected = part
                .height
                .zip(self.planar_bindings.selected_height)
                .is_some_and(|(h, selected)| (h - selected).abs() <= 0.01);
            pass.set_bind_group(
                FLUID_SCENE_INPUTS_GROUP,
                if selected {
                    self.scene_inputs.bind_group()
                } else {
                    zero
                },
                &[],
            );
            pass.draw_indexed(part.start..part.start + part.count, 0, element..element + 1);
        }
        true
    }
}
