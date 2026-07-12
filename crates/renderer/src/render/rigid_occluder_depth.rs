// Position-only rigid-instance shadow occluder pipeline and mover recorder.
// See: context/lib/rendering_pipeline.md §7.1

use glam::Vec4;
use postretro_render_data::cone_frustum::{Aabb, aabb_intersects_frustum};

use super::kinematic_brush::KinematicBrushPass;

const RIGID_OCCLUDER_DEPTH_SHADER_SOURCE: &str =
    include_str!("../shaders/rigid_occluder_depth.wgsl");

/// A mover's conservative world-space bound supplied by the game layer. This
/// crosses the engine→renderer boundary without exposing a GPU resource.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoverOccluderAabb {
    pub mover_id: u32,
    pub world_aabb: Aabb,
}

/// Renderer-owned depth-only pipeline for rigid instances. The pipeline itself
/// is generic; movers are its sole recorder caller in this wave.
pub(crate) struct RigidOccluderDepthPass {
    pipeline: wgpu::RenderPipeline,
}

impl RigidOccluderDepthPass {
    /// Build the two-group rigid depth pipeline. The existing dynamic-offset
    /// light-space layout stays at group 0, and the caller's model-transform
    /// layout stays at group 1; no material or lighting group participates.
    pub(crate) fn new(
        device: &wgpu::Device,
        shadow_depth_format: wgpu::TextureFormat,
        light_space_bind_group_layout: &wgpu::BindGroupLayout,
        instance_transform_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rigid Occluder Depth Shader"),
            source: wgpu::ShaderSource::Wgsl(RIGID_OCCLUDER_DEPTH_SHADER_SOURCE.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rigid Occluder Depth Pipeline Layout"),
            bind_group_layouts: &[
                Some(light_space_bind_group_layout),
                Some(instance_transform_bind_group_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Rigid Occluder Depth Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: postretro_render_data::geometry::WorldVertex::STRIDE
                        as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        offset: 0,
                        shader_location: 0,
                        format: wgpu::VertexFormat::Float32x3,
                    }],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: shadow_depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                // Keep depth behavior identical to skinned and world shadow
                // occluders to avoid seams between caster classes.
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 1.5,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });

        Self { pipeline }
    }

    /// Record active kinematic movers into an already-open live shadow pass.
    /// The caller owns clearing/caching policy; this method only loads shared
    /// mover buffers and appends surviving depth draws.
    pub(crate) fn record_kinematic_movers(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kinematic_brush: &KinematicBrushPass,
        mover_aabbs: &[MoverOccluderAabb],
        light_space_bind_group: &wgpu::BindGroup,
        dynamic_offset: u32,
        cone_planes: &[Vec4; 6],
    ) -> u32 {
        if mover_aabbs.is_empty() || kinematic_brush.active_draws().is_empty() {
            return 0;
        }

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, light_space_bind_group, &[dynamic_offset]);
        pass.set_bind_group(1, kinematic_brush.instance_transform_bind_group(), &[]);
        pass.set_vertex_buffer(0, kinematic_brush.shared_vertex_buffer().slice(..));
        pass.set_index_buffer(
            kinematic_brush.shared_index_buffer().slice(..),
            wgpu::IndexFormat::Uint32,
        );

        let mut submitted = 0;
        for mover in mover_occluders_in_cone(mover_aabbs, cone_planes) {
            let Some(mover_draw_index) =
                kinematic_brush.mover_draw_index_for_mover_id(mover.mover_id)
            else {
                continue;
            };
            let Some(active_draw) = kinematic_brush
                .active_draws()
                .iter()
                .find(|draw| draw.mover_draw_index == mover_draw_index)
            else {
                continue;
            };
            let Some(index_range) = kinematic_brush.mover_index_ranges().get(mover_draw_index)
            else {
                continue;
            };
            if index_range.index_count == 0 {
                continue;
            }

            pass.draw_indexed(
                index_range.index_start..index_range.index_start + index_range.index_count,
                0,
                active_draw.instance_index..active_draw.instance_index + 1,
            );
            submitted += 1;
        }
        submitted
    }
}

/// Pure per-slot/face mover cull. It takes game-tagged world AABBs and the
/// current light cone, returning only survivors without touching GPU state.
/// The copied plane array lets the iterator borrow only the mover list, so
/// recorder use does not allocate a per-shadow-pass survivor list.
pub(crate) fn mover_occluders_in_cone<'a>(
    mover_aabbs: &'a [MoverOccluderAabb],
    cone_planes: &[Vec4; 6],
) -> impl Iterator<Item = &'a MoverOccluderAabb> {
    let cone_planes = *cone_planes;
    mover_aabbs
        .iter()
        .filter(move |mover| aabb_intersects_frustum(&mover.world_aabb, &cone_planes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_level_loader::{FalloffModel, LightType, MapLight, ShadowType};
    use postretro_lighting::light_space_matrix;
    use postretro_render_data::cone_frustum::cone_frustum_planes;

    #[test]
    fn mover_occluder_cull_excludes_outside_cone_mover() {
        // Match the skinned `instance_casts_into_cone` regression: one mover
        // lies on the cone axis and another is far off-axis at the same depth.
        let light = MapLight {
            origin: [0.0, 0.0, 0.0],
            light_type: LightType::Spot,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 20.0,
            cone_angle_inner: 0.3,
            cone_angle_outer: 0.4,
            cone_direction: [0.0, 0.0, -1.0],
            is_dynamic: true,
            casts_entity_shadows: true,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: ShadowType::StaticLightMap,
        };
        let cone_planes = cone_frustum_planes(&light_space_matrix(&light));
        let mover_aabbs = [
            MoverOccluderAabb {
                mover_id: 10,
                world_aabb: Aabb {
                    min: Vec3::new(-0.5, -0.5, -10.5),
                    max: Vec3::new(0.5, 0.5, -9.5),
                },
            },
            MoverOccluderAabb {
                mover_id: 20,
                world_aabb: Aabb {
                    min: Vec3::new(49.5, -0.5, -10.5),
                    max: Vec3::new(50.5, 0.5, -9.5),
                },
            },
        ];

        let survivors: Vec<u32> = mover_occluders_in_cone(&mover_aabbs, &cone_planes)
            .map(|mover| mover.mover_id)
            .collect();
        assert_eq!(survivors, vec![10]);
    }

    #[test]
    fn rigid_occluder_depth_wgsl_parses_and_is_vertex_only() {
        let module = naga::front::wgsl::parse_str(RIGID_OCCLUDER_DEPTH_SHADER_SOURCE)
            .expect("rigid_occluder_depth.wgsl should parse as WGSL");
        assert!(
            module
                .entry_points
                .iter()
                .any(|entry| entry.name == "vs_main" && entry.stage == naga::ShaderStage::Vertex),
            "rigid depth shader must export a vertex stage"
        );
        assert!(
            module
                .entry_points
                .iter()
                .all(|entry| entry.stage != naga::ShaderStage::Fragment),
            "rigid depth shader must not declare a fragment stage"
        );
    }

    #[test]
    fn rigid_occluder_depth_wgsl_passes_naga_validation() {
        let module = naga::front::wgsl::parse_str(RIGID_OCCLUDER_DEPTH_SHADER_SOURCE)
            .expect("rigid_occluder_depth.wgsl must parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("rigid_occluder_depth.wgsl must pass naga validation");
    }
}
