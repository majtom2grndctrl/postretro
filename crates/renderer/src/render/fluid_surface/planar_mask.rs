use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};
use postretro_render_cpu::fluid_surface::pack_fluid_surfaces;
use postretro_render_cpu::planar_reflection::PlanarCandidate;

/// Reuse fluid triangulation so concave rings cannot paint dry pixels.
pub(in crate::render) fn face_triangles(plane: &PlanarCandidate, output: &mut Vec<[f32; 3]>) {
    let volume = EnvironmentVolumeRecord {
        source_entity: 0,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".into(),
        brushes: Vec::new(),
        surface_faces: plane
            .faces
            .iter()
            .map(|face| EnvironmentSurfaceFace {
                texture_index: 0,
                vertices: face
                    .vertices
                    .iter()
                    .map(|p| EnvironmentSurfaceVertex {
                        position: p.to_array(),
                        uv: [0.0; 2],
                    })
                    .collect(),
            })
            .collect(),
    };
    let mesh = pack_fluid_surfaces(&[volume], &[], 1, None);
    output.clear();
    output.extend(
        mesh.indices
            .iter()
            .map(|&i| mesh.vertices[i as usize].position),
    );
}
